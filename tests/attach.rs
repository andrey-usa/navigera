//! `--attach` and `--profile`: navigera works in a browser it doesn't own
//! and never closes it. "The user's browser" here is a Chrome the test
//! starts itself with remote debugging on its own profile — the same
//! `DevToolsActivePort` handshake Chrome 144+ uses for the default profile
//! once chrome://inspect/#remote-debugging is on.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

fn tool_exe() -> String {
    env!("CARGO_BIN_EXE_navigera").to_string()
}

/// The fixture servers are Python; Windows installs it as `python`.
fn python() -> &'static str {
    if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}

fn chrome() -> Option<String> {
    let found = navigera::browser::resolve_executable_for(None, true);
    assert!(
        found.is_some() || std::env::var_os("NAVIGERA_REQUIRE_BROWSER").is_none(),
        "NAVIGERA_REQUIRE_BROWSER is set but no Chrome/Chromium was found"
    );
    found
}

fn session_name(test: &str) -> String {
    std::env::temp_dir().join(format!("nv-attach-{test}-{}.sock", std::process::id())).display().to_string()
}

/// One CLI step against a session; returns (exit ok, JSON).
fn run(session: &str, args: &[&str], env: &[(&str, &str)]) -> (bool, Value) {
    let mut cmd = Command::new(tool_exe());
    cmd.arg("--session").arg(session).args(args).stdin(Stdio::null());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("run navigera");
    let text = String::from_utf8_lossy(&out.stdout);
    let value = serde_json::from_str(text.trim()).unwrap_or_else(|_| {
        panic!("stdout must be one JSON object for {args:?}: {text}\nstderr: {}", String::from_utf8_lossy(&out.stderr))
    });
    (out.status.success(), value)
}

fn ok(session: &str, args: &[&str]) -> Value {
    let (success, r) = run(session, args, &[]);
    assert!(success && r["ok"] == true, "{args:?} failed: {r}");
    r["result"].clone()
}

fn active_port(dir: &Path, within: Duration) -> Option<(u16, String)> {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(dir.join("DevToolsActivePort")) {
            let mut lines = text.lines();
            if let (Some(port), Some(path)) = (lines.next(), lines.next()) {
                if let Ok(port) = port.trim().parse() {
                    return Some((port, path.trim().to_string()));
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

/// The body of a GET on the DevTools HTTP endpoint at `port`.
fn devtools_get(port: u16, path: &str) -> Option<String> {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    write!(s, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").ok()?;
    // Chrome keeps the connection open: read exactly Content-Length bytes.
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let body = loop {
        let n = s.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buf).into_owned();
        if let Some(end) = text.find("\r\n\r\n") {
            let len = text[..end]
                .lines()
                .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().to_string()))
                .and_then(|v| v.parse::<usize>().ok())?;
            if buf.len() >= end + 4 + len {
                break String::from_utf8_lossy(&buf[end + 4..end + 4 + len]).into_owned();
            }
        }
    };
    Some(body)
}

/// Page targets (id, url) of the browser serving DevTools on `port`.
fn page_targets(port: u16) -> Option<Vec<(String, String)>> {
    let json: Value = serde_json::from_str(&devtools_get(port, "/json/list")?).ok()?;
    Some(
        json.as_array()?
            .iter()
            .filter(|t| t["type"] == "page")
            .map(|t| (t["id"].as_str().unwrap_or("").to_string(), t["url"].as_str().unwrap_or("").to_string()))
            .collect(),
    )
}

/// Page targets (url list) of the browser serving DevTools on `port`.
fn page_urls(port: u16) -> Option<Vec<String>> {
    Some(page_targets(port)?.into_iter().map(|(_, url)| url).collect())
}

struct Browser(Child);

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The user's own browser, with one tab of theirs open. A CI Windows
/// runner sometimes starts Chrome without it ever writing its
/// DevToolsActivePort: a second launch, on a fresh profile, gets one.
fn users_browser(exe: &str) -> (Browser, tempfile::TempDir, u16, String) {
    let launch = || {
        let profile = tempfile::tempdir().unwrap();
        let mut user = Browser(
            Command::new(exe)
                .args(["--headless=new", "--no-sandbox", "--no-first-run", "--remote-debugging-port=0"])
                .arg(format!("--user-data-dir={}", profile.path().display()))
                .arg("data:text/html,<title>Mine</title>users%20own%20tab")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("start the user's browser"),
        );
        match active_port(profile.path(), Duration::from_secs(20)) {
            Some((port, _)) => {
                // The port can be up before the start-up tab is listed (a
                // loaded runner: the tests then found no "Mine"), and right
                // after the first listing Chrome can report the tab's url as
                // "" for a moment. Capture the tab's target id while it is
                // listed with "Mine"; the tests use the id, not a fresh
                // lookup that can race that blank listing.
                let end = std::time::Instant::now() + Duration::from_secs(10);
                let mut mine = None;
                while mine.is_none() && std::time::Instant::now() < end {
                    // Listed with its title, not only its url: the url is
                    // there before the document is parsed, and a test that
                    // attached in between read an empty title.
                    mine = devtools_get(port, "/json/list")
                        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                        .and_then(|list| {
                            list.as_array()?.iter().find_map(|t| {
                                let url = t["url"].as_str().unwrap_or("");
                                (t["type"] == "page" && url.contains("Mine") && t["title"] == "Mine")
                                    .then(|| t["id"].as_str().unwrap_or("").to_string())
                            })
                        });
                    if mine.is_none() {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
                if mine.is_none() {
                    eprintln!("the start-up tab was never listed; launching again");
                }
                mine.map(|id| (user, profile, port, id))
            }
            None => {
                eprintln!("no DevToolsActivePort after 20 s (exited: {:?}); launching again", user.0.try_wait());
                None
            }
        }
    };
    launch().or_else(launch).expect("DevToolsActivePort")
}

#[test]
fn attach_never_closes_the_users_browser() {
    let Some(exe) = chrome() else { return };
    let (mut user, profile, port, _mine) = users_browser(&exe);
    let dir = profile.path().display().to_string();
    let s = session_name("user");

    let started = ok(&s, &["start", "--attach", &dir]);
    assert!(started["pid"].is_number(), "{started}");
    let state = ok(&s, &["goto", "data:text/html,<title>Agent</title><h1>agent page</h1>"]);
    assert_eq!(state["title"], "Agent");
    assert!(ok(&s, &["ax"]).as_str().unwrap_or("").contains("agent page"));
    ok(&s, &["tab-new", "data:text/html,<title>Agent2</title>"]);
    let bye = ok(&s, &["quit"]);
    assert_eq!(bye["browser"], "left open", "{bye}");
    assert_eq!(bye["closed_tabs"], 2, "navigera closes the tabs it opened: {bye}");

    std::thread::sleep(Duration::from_millis(500));
    assert!(user.0.try_wait().unwrap().is_none(), "the user's browser must still run");
    let urls = page_urls(port).expect("browser still serves DevTools");
    assert!(urls.iter().any(|u| u.contains("Mine")), "the user's tab is untouched: {urls:?}");
    assert!(!urls.iter().any(|u| u.contains("Agent")), "nothing of navigera's is left behind: {urls:?}");

    // A second session reuses the same browser and sees the user's tab
    // beside its own; `quit --keep-tabs` leaves its window for the user.
    ok(&s, &["start", "--attach", &dir]);
    let tabs = ok(&s, &["tab-list"]);
    assert_eq!(tabs["tabs"].as_array().map(Vec::len), Some(1), "navigera's own tab: {tabs}");
    let theirs = tabs["user_tabs"].as_array().cloned().unwrap_or_default();
    assert!(theirs.iter().any(|t| t["title"] == "Mine"), "the user's tab is listed as theirs: {tabs}");
    assert_eq!(ok(&s, &["status"])["user_tabs"], theirs.len(), "status counts them");
    ok(&s, &["goto", "data:text/html,<title>Kept</title>"]);
    let bye = ok(&s, &["quit", "--keep-tabs"]);
    assert!(bye.get("closed_tabs").is_none(), "{bye}");
    let urls = page_urls(port).expect("browser still serves DevTools");
    assert!(urls.iter().any(|u| u.contains("Kept")), "--keep-tabs leaves navigera's tab: {urls:?}");
    assert!(user.0.try_wait().unwrap().is_none(), "still running after the second session");
}

#[test]
fn an_attached_session_works_in_the_users_tab_and_leaves_it_open() {
    let Some(exe) = chrome() else { return };
    let (user, profile, port, mine) = users_browser(&exe);
    let dir = profile.path().display().to_string();
    let s = session_name("users-tab");

    // In the tab the user is on (theirs is the only one): no window of
    // navigera's own opens.
    ok(&s, &["start", "--attach", &dir, "--tab", "current"]);
    assert_eq!(ok(&s, &["eval", "document.title"]), "Mine");
    let tabs = ok(&s, &["tab-list"]);
    assert_eq!(tabs["tabs"][0]["user"], true, "{tabs}");
    assert_eq!(page_targets(port).map(|t| t.len()), Some(1), "no window of navigera's own");
    let bye = ok(&s, &["quit"]);
    assert_eq!(bye["user_tabs_left"], 1, "{bye}");
    assert!(bye.get("closed_tabs").is_none(), "{bye}");
    assert!(page_targets(port).is_some_and(|t| t.iter().any(|(id, _)| *id == mine)), "the user's tab stays open");

    // From a window of its own to the user's tab by its id: the unused
    // blank window closes; `reset` lets the user's tab go, and `quit`
    // closes only what navigera opened.
    ok(&s, &["start", "--attach", &dir]);
    let state = ok(&s, &["tab-select", &mine[..8]]);
    assert_eq!(state["title"], "Mine", "{state}");
    assert_eq!(page_targets(port).map(|t| t.len()), Some(1), "navigera's blank window closed");
    ok(&s, &["reset"]);
    assert_eq!(page_targets(port).map(|t| t.len()), Some(2), "reset opens a window of its own");
    ok(&s, &["quit"]);
    let left = page_targets(port).unwrap_or_default();
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(left[0].0, mine, "only the user's tab is left");

    // `show` hands navigera's tab to the user: `quit` leaves it open.
    ok(&s, &["start", "--attach", &dir]);
    ok(&s, &["goto", "data:text/html,<title>Shown</title>"]);
    let shown = ok(&s, &["show"]);
    assert_eq!(shown["visible"], true, "{shown}");
    let bye = ok(&s, &["quit"]);
    assert_eq!(bye["user_tabs_left"], 1, "{bye}");
    let urls = page_urls(port).unwrap_or_default();
    assert!(urls.iter().any(|u| u.contains("Shown")), "{urls:?}");
    drop(user);
}

#[test]
fn a_tab_closed_under_an_attached_session_is_named_and_replaced() {
    let Some(exe) = chrome() else { return };
    let (_user, _profile, port, id) = users_browser(&exe);
    let s = session_name("closed");
    ok(&s, &["start", "--attach", &format!("ws://127.0.0.1:{port}/devtools/page/{id}")]);
    assert_eq!(ok(&s, &["eval", "document.title"]), "Mine");

    // The user closes the tab.
    devtools_get(port, &format!("/json/close/{id}")).expect("close the tab");
    let deadline = Instant::now() + Duration::from_secs(10);
    while page_targets(port).is_some_and(|t| t.iter().any(|(t, _)| *t == id)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }

    // Every op that needs the tab says it is gone, not a raw CDP error.
    for args in [&["eval", "document.title"][..], &["goto", "data:text/html,x"][..]] {
        let (success, r) = run(&s, args, &[]);
        let error = r["error"].as_str().unwrap_or("");
        assert!(!success && error.contains("the tab is closed") && error.contains("tab-new"), "{args:?}: {r}");
    }
    assert_eq!(ok(&s, &["status"])["closed"], true);
    let tabs = ok(&s, &["tab-list"]);
    assert_eq!(tabs["tabs"][0]["closed"], true, "{tabs}");

    // A new tab takes its place; the closed one is gone from the list.
    let tabs = ok(&s, &["tab-new", "data:text/html,<title>Again</title>"]);
    assert_eq!(tabs["tabs"].as_array().map(Vec::len), Some(1), "{tabs}");
    assert!(tabs["tabs"][0].get("closed").is_none(), "{tabs}");
    assert_eq!(ok(&s, &["eval", "document.title"]), "Again");
    assert!(ok(&s, &["status"]).get("closed").is_none());
    ok(&s, &["quit"]);
}

#[test]
fn profile_browser_outlives_sessions_and_keeps_cookies() {
    let Some(_) = chrome() else { return };
    let site_script = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/edge_site.py");
    let mut site = Browser(
        Command::new(python())
            .arg(site_script)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("python fixture site"),
    );
    let mut line = String::new();
    BufReader::new(site.0.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let base = line.split_whitespace().nth(1).expect("LISTENING line").to_string();

    let profile = tempfile::tempdir().unwrap();
    let dir = profile.path().join("p").display().to_string();
    let s = session_name("profile");
    // Chrome for Testing builds (the CI matrix) ship no setuid sandbox.
    let env = [("NAVIGERA_EXTRA_FLAGS", "--no-sandbox")];
    let start = |s: &str| {
        let (success, r) = run(s, &["start", "--profile", &dir, "--headless"], &env);
        assert!(success && r["ok"] == true, "start --profile: {r}");
    };

    // A failed assertion must not leave the (detached) profile browser up.
    struct CloseOnDrop(String);
    impl Drop for CloseOnDrop {
        fn drop(&mut self) {
            let _ = run(&self.0, &["quit", "--close-browser"], &[]);
        }
    }

    start(&s);
    let _closer = CloseOnDrop(s.clone());
    let first = active_port(Path::new(&dir), Duration::from_secs(1)).expect("profile browser endpoint");
    // Only the session's window: Chrome opens no blank startup window of its own.
    let pages = page_urls(first.0).expect("profile browser targets");
    assert_eq!(pages.len(), 1, "one page after start --profile, not Chrome's startup tab too");
    ok(&s, &["goto", &format!("{base}/title")]);
    ok(&s, &["eval", "(document.cookie = 'nv=kept; max-age=3600; path=/', 1)"]);
    assert_eq!(ok(&s, &["quit"])["browser"], "left open");

    let started = Instant::now();
    start(&s);
    assert!(started.elapsed() < Duration::from_secs(10), "reconnecting is quick");
    assert_eq!(
        active_port(Path::new(&dir), Duration::from_secs(1)),
        Some(first.clone()),
        "same browser, not a new one"
    );
    ok(&s, &["goto", &format!("{base}/title")]);
    let cookie = ok(&s, &["eval", "document.cookie"]);
    assert!(cookie.as_str().unwrap_or("").contains("nv=kept"), "cookie survives the session: {cookie}");

    // Opt-in: close the profile browser itself.
    assert_eq!(ok(&s, &["quit", "--close-browser"])["browser"], "closed");
    let deadline = Instant::now() + Duration::from_secs(15);
    while page_urls(first.0).is_some() {
        assert!(Instant::now() < deadline, "the profile browser should exit after --close-browser");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn attach_needs_a_session() {
    let out = Command::new(tool_exe())
        .args(["--attach", "chrome", "goto", "example.com"])
        .stdin(Stdio::null())
        .output()
        .expect("run navigera");
    let r: Value = serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).expect("one JSON line");
    assert_eq!(r["ok"], false);
    assert!(r["error"].as_str().unwrap().contains("use a session"), "{r}");
}

#[test]
fn a_tab_closed_while_an_op_waits_on_it_ends_the_op() {
    let Some(exe) = chrome() else { return };
    let (_user, _profile, port, id) = users_browser(&exe);
    let s = session_name("midop");
    ok(&s, &["start", "--attach", &format!("ws://127.0.0.1:{port}/devtools/page/{id}")]);
    let tabs = ok(&s, &["tab-new", "data:text/html,<title>Second</title>"]);
    let second = tabs["tabs"][1]["target"].as_str().expect("second tab").to_string();
    // Each tab says what it shows.
    assert_eq!(tabs["tabs"][1]["title"], "Second", "{tabs}");
    assert!(tabs["tabs"][1]["url"].as_str().is_some_and(|u| u.starts_with("data:text/html")), "{tabs}");
    let slow = "new Promise(r => setTimeout(() => r(document.title), 20000))";
    let close_soon = |target: String| {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(800));
            devtools_get(port, &format!("/json/close/{target}"));
        })
    };

    // The active tab of two: the op ends when it closes, on the other tab.
    let closer = close_soon(second);
    let started = Instant::now();
    let (success, r) = run(&s, &["eval", slow], &[]);
    closer.join().unwrap();
    assert!(!success && r["error"].as_str().unwrap_or("").contains("closed while this ran"), "{r}");
    assert!(started.elapsed() < Duration::from_secs(10), "waited {:?}", started.elapsed());
    assert_eq!(ok(&s, &["eval", "document.title"]), "Mine");

    // The only tab: the op ends, and the session knows the tab is gone.
    let closer = close_soon(id);
    let started = Instant::now();
    let (success, r) = run(&s, &["eval", slow], &[]);
    closer.join().unwrap();
    assert!(!success && r["error"].as_str().unwrap_or("").contains("the tab is closed"), "{r}");
    assert!(started.elapsed() < Duration::from_secs(10), "waited {:?}", started.elapsed());
    assert_eq!(ok(&s, &["status"])["closed"], true);
    ok(&s, &["quit"]);
}

#[test]
fn two_sessions_in_the_users_browser_never_list_each_others_windows_as_theirs() {
    let Some(exe) = chrome() else { return };
    let (_user, profile, port, _mine) = users_browser(&exe);
    let dir = profile.path().display().to_string();
    let (a, b) = (session_name("pair-a"), session_name("pair-b"));
    ok(&a, &["start", "--attach", &dir]);
    ok(&b, &["start", "--attach", &dir]);
    ok(&a, &["goto", "data:text/html,<title>AgentA</title>a"]);
    ok(&b, &["goto", "data:text/html,<title>AgentB</title>b"]);
    let titles = |s: &str| -> Vec<String> {
        ok(s, &["tab-list"])["user_tabs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| t["title"].as_str().map(String::from))
            .collect()
    };
    let (seen_a, seen_b) = (titles(&a), titles(&b));
    assert!(seen_a.iter().any(|t| t == "Mine") && seen_b.iter().any(|t| t == "Mine"), "{seen_a:?} {seen_b:?}");
    assert!(!seen_a.iter().any(|t| t.starts_with("Agent")), "A lists B's window as the user's: {seen_a:?}");
    assert!(!seen_b.iter().any(|t| t.starts_with("Agent")), "B lists A's window as the user's: {seen_b:?}");
    ok(&b, &["quit"]);

    // `quit --keep-tabs` after `reset` leaves the reset context's tab too.
    ok(&a, &["reset"]);
    ok(&a, &["goto", "data:text/html,<title>AfterReset</title>r"]);
    ok(&a, &["quit", "--keep-tabs"]);
    let urls = page_urls(port).expect("browser still serves DevTools");
    assert!(urls.iter().any(|u| u.contains("AfterReset")), "--keep-tabs after reset: {urls:?}");
}

/// A certificate authority and a `localhost` certificate it signed, made
/// with `openssl` for this run (no key is kept in the repository).
#[cfg(target_os = "linux")]
fn test_certs() -> Option<tempfile::TempDir> {
    let dir = tempfile::tempdir().unwrap();
    let openssl =
        |args: &[&str]| Command::new("openssl").args(args).current_dir(dir.path()).stdin(Stdio::null()).output();
    if openssl(&["version"]).is_err() {
        assert!(std::env::var_os("NAVIGERA_REQUIRE_BROWSER").is_none(), "the wss test needs openssl on the PATH");
        return None;
    }
    std::fs::write(dir.path().join("leaf.cnf"), "subjectAltName=DNS:localhost\nextendedKeyUsage=serverAuth\n").unwrap();
    let ec = ["-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:prime256v1", "-nodes"];
    for step in [
        [
            &["req", "-x509"][..],
            &ec,
            &["-keyout", "ca.key", "-out", "ca.pem", "-days", "2", "-subj", "/CN=navigera test CA"],
            &["-addext", "basicConstraints=critical,CA:TRUE", "-addext", "keyUsage=critical,keyCertSign"],
        ]
        .concat(),
        [&["req"][..], &ec, &["-keyout", "leaf.key", "-out", "leaf.csr", "-subj", "/CN=localhost"]].concat(),
        [
            "x509",
            "-req",
            "-in",
            "leaf.csr",
            "-CA",
            "ca.pem",
            "-CAkey",
            "ca.key",
            "-CAcreateserial",
            "-out",
            "leaf.pem",
            "-days",
            "2",
            "-extfile",
            "leaf.cnf",
        ]
        .to_vec(),
    ] {
        let out = openssl(&step).unwrap();
        assert!(out.status.success(), "openssl {step:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
    Some(dir)
}

/// A fixture server (`tests/fixtures/<script>`) and the port it printed:
/// the TLS front of a cloud browser, or a proxy.
#[cfg(target_os = "linux")]
struct Fixture(Child, u16);

#[cfg(target_os = "linux")]
impl Fixture {
    fn start(script: &str, args: &[String]) -> Fixture {
        let mut child = Command::new(python())
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(script))
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|e| panic!("start {script}: {e}"));
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
        let port = line.trim().parse().unwrap_or_else(|_| panic!("{script} prints its port, got {line:?}"));
        Fixture(child, port)
    }
}

#[cfg(target_os = "linux")]
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The test's Chrome behind a TLS front, as a cloud browser is reached:
/// the front, and the `wss://` endpoint with `key` in its query.
#[cfg(target_os = "linux")]
fn cloud_browser(port: u16, path: &str, certs: &Path, key: &str) -> (Fixture, String) {
    let pem = |f: &str| certs.join(f).display().to_string();
    let relay = Fixture::start("tls_relay.py", &[port.to_string(), pem("leaf.pem"), pem("leaf.key")]);
    let url = format!("wss://localhost:{}{path}?token={key}", relay.1);
    (relay, url)
}

/// One navigera call, in an environment without the machine's proxy or CA
/// settings, plus `env`; its stdout and stderr together, for what it shows.
#[cfg(target_os = "linux")]
fn shown(session: &str, args: &[&str], env: &[(&str, &str)]) -> (bool, String) {
    let mut cmd = Command::new(tool_exe());
    cmd.arg("--session").arg(session).args(args).stdin(Stdio::null()).env_remove("SSL_CERT_FILE");
    for var in ["https_proxy", "HTTPS_PROXY", "http_proxy", "all_proxy", "ALL_PROXY", "no_proxy", "NO_PROXY"] {
        cmd.env_remove(var);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("run navigera");
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

/// A browser in the cloud is reached at a `wss://` endpoint with the
/// service's key in its query. navigera connects over TLS, checking the
/// certificate, and the key is in no output, log or command line. (Linux:
/// there `SSL_CERT_FILE` names the test's CA to the system verifier.)
#[cfg(target_os = "linux")]
#[test]
fn a_wss_endpoint_attaches_over_tls_and_its_key_is_never_shown() {
    let Some(exe) = chrome() else { return };
    let Some(certs) = test_certs() else { return };
    let (_user, profile, port, _mine) = users_browser(&exe);
    let (_, path) = active_port(profile.path(), Duration::from_secs(5)).expect("the browser endpoint's path");
    let key = "k3y-n0t-to-be-shown";
    let (relay, url) = cloud_browser(port, &path, certs.path(), key);
    let ca = certs.path().join("ca.pem").display().to_string();
    let s = session_name("wss");

    // A certificate this machine doesn't trust: refused, saying where a
    // private CA goes.
    let (ok_untrusted, text) = shown(&s, &["start", "--attach", &url], &[]);
    assert!(!ok_untrusted, "an untrusted certificate must be refused: {text}");
    assert!(text.contains("SSL_CERT_FILE") && text.contains("token=***"), "{text}");
    assert!(!text.contains(key), "the key is shown: {text}");

    // Trusted, and the endpoint from the environment (no command line has it).
    let env = [("SSL_CERT_FILE", ca.as_str()), ("NAVIGERA_ATTACH", url.as_str())];
    let (started, text) = shown(&s, &["start"], &env);
    assert!(started, "{text}");
    assert!(!text.contains(key), "the key is shown: {text}");
    let json_start = text.find('{').expect("start's JSON");
    let result: Value = serde_json::from_str(text[json_start..].lines().next().unwrap()).unwrap();
    let pid = result["result"]["pid"].as_u64().or(result["pid"].as_u64()).expect("the server's pid");
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap();
    assert!(!String::from_utf8_lossy(&cmdline).contains(key), "the key is on the server's command line");

    let mut seen = String::new();
    for args in
        [&["goto", "data:text/html,<title>Cloud</title><h1>over tls</h1>"][..], &["text"], &["status"], &["quit"]]
    {
        let (ok_step, text) = shown(&s, args, &[]);
        assert!(ok_step, "{args:?}: {text}");
        seen.push_str(&text);
    }
    assert!(seen.contains("over tls"), "{seen}");
    assert!(seen.contains("\"endpoint\":\"wss://localhost:") && seen.contains("token=***"), "{seen}");
    assert!(seen.contains("left open"), "{seen}");
    assert!(!seen.contains(key), "the key is shown: {seen}");
    let log = result["result"]["log"].as_str().or(result["log"].as_str()).expect("the session log");
    assert!(!std::fs::read_to_string(log).unwrap_or_default().contains(key), "the key is in the session log");
    drop(relay);
}

/// Behind a corporate proxy a cloud browser is reached through it: a
/// `wss://` endpoint goes through `$HTTPS_PROXY` as a CONNECT tunnel (curl's
/// rules: `$NO_PROXY` skips it), with the credentials in its URL, and
/// neither the proxy's password nor the endpoint's key is shown.
#[cfg(target_os = "linux")]
#[test]
fn a_wss_endpoint_is_reached_through_the_proxy_the_environment_names() {
    let Some(exe) = chrome() else { return };
    let Some(certs) = test_certs() else { return };
    let (_user, profile, port, _mine) = users_browser(&exe);
    let (_, path) = active_port(profile.path(), Duration::from_secs(5)).expect("the browser endpoint's path");
    let key = "k3y-n0t-to-be-shown";
    let (relay, url) = cloud_browser(port, &path, certs.path(), key);
    let log = certs.path().join("tunnels.log");
    let proxy = Fixture::start("connect_proxy.py", &[log.display().to_string(), "ann:p@ss".into()]);
    let ca = certs.path().join("ca.pem").display().to_string();
    let s = session_name("proxy");
    let secrets = [key, "p%40ss", "p@ss", "ann:"];

    // Without its credentials: the proxy says so, and the fix is named.
    let bare = format!("http://127.0.0.1:{}", proxy.1);
    let env = [("SSL_CERT_FILE", ca.as_str()), ("NAVIGERA_ATTACH", url.as_str()), ("HTTPS_PROXY", bare.as_str())];
    let (ok_bare, text) = shown(&s, &["start"], &env);
    assert!(!ok_bare, "{text}");
    assert!(text.contains("wants credentials") && text.contains("from $HTTPS_PROXY"), "{text}");

    // With them (percent-encoded): through the tunnel, the session works.
    let with = format!("http://ann:p%40ss@127.0.0.1:{}", proxy.1);
    let env = [("SSL_CERT_FILE", ca.as_str()), ("NAVIGERA_ATTACH", url.as_str()), ("HTTPS_PROXY", with.as_str())];
    let mut seen = String::new();
    let (started, text) = shown(&s, &["start"], &env);
    assert!(started, "{text}");
    seen.push_str(&text);
    for args in [&["goto", "data:text/html,<title>Proxied</title><h1>through the proxy</h1>"][..], &["text"], &["quit"]]
    {
        let (ok_step, text) = shown(&s, args, &[]);
        assert!(ok_step, "{args:?}: {text}");
        seen.push_str(&text);
    }
    assert!(seen.contains("through the proxy"), "{seen}");
    let tunnels = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(tunnels.contains(&format!("CONNECT localhost:{}", relay.1)), "the proxy opened no tunnel: {tunnels:?}");
    for secret in secrets {
        assert!(!seen.contains(secret), "{secret:?} is shown: {seen}");
    }

    // `$NO_PROXY` names the host: straight there, past a proxy that is down.
    let dead = "http://127.0.0.1:9";
    let env = [
        ("SSL_CERT_FILE", ca.as_str()),
        ("NAVIGERA_ATTACH", url.as_str()),
        ("HTTPS_PROXY", dead),
        ("NO_PROXY", "example.test,localhost"),
    ];
    let (started, text) = shown(&s, &["start"], &env);
    assert!(started, "{text}");
    let (ok_quit, text) = shown(&s, &["quit"], &[]);
    assert!(ok_quit, "{text}");
    drop(proxy);
    drop(relay);
}
