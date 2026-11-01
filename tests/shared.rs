//! Named sessions share one browser, each in a browser context of its own:
//! one browser process for several sessions, nothing of one session in
//! another, contexts gone with their session (killed or not), the browser
//! gone after the grace period, and a clear error when it dies.
//!
//! Each test gets a browser of its own: a marker flag in
//! `NAVIGERA_EXTRA_FLAGS` makes its launch key differ from every other
//! test's, and lets the test find its browser among the processes.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

fn tool_exe() -> String {
    env!("CARGO_BIN_EXE_navigera").to_string()
}

fn chrome() -> bool {
    let found = navigera::browser::resolve_executable_for(None, true);
    assert!(
        found.is_some() || std::env::var_os("NAVIGERA_REQUIRE_BROWSER").is_none(),
        "NAVIGERA_REQUIRE_BROWSER is set but no Chrome/Chromium was found"
    );
    found.is_some()
}

struct Site {
    child: std::process::Child,
    url: String,
}

impl Drop for Site {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn site() -> Option<Site> {
    let python = if cfg!(windows) { "python" } else { "python3" };
    let mut child = Command::new(python)
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/interactions_site.py"))
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .ok()?;
    let mut line = String::new();
    BufReader::new(child.stdout.take()?).read_line(&mut line).ok()?;
    let url = line.trim().strip_prefix("LISTENING ")?.to_string();
    Some(Site { child, url })
}

/// One test's shared browser: its marker and its grace period.
struct Browser {
    marker: String,
    grace_s: u64,
}

impl Browser {
    fn new(test: &str, grace_s: u64) -> Browser {
        Browser { marker: format!("--nv-shared-test={test}-{}", std::process::id()), grace_s }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(tool_exe());
        cmd.env("NAVIGERA_EXTRA_FLAGS", &self.marker)
            .env("NAVIGERA_SHARED_GRACE_S", self.grace_s.to_string())
            .env_remove("NAVIGERA_SHARED_BROWSER")
            .env_remove("NAVIGERA_SESSION");
        cmd
    }

    fn start(&self, name: &str, extra: &[&str]) -> Session {
        let s = self.session(name);
        let r = s.run_with(self.command(), &[&["start", "--download-dir", &s.downloads_str()], extra].concat());
        assert!(r.0 && r.1["ok"] == true, "start {name}: {}", r.1);
        s
    }

    fn session(&self, name: &str) -> Session {
        let sock = std::env::temp_dir()
            .join(format!("nv-shared-{}-{name}-{}.sock", self.marker.rsplit('=').next().unwrap(), std::process::id()))
            .display()
            .to_string();
        Session { sock, downloads: tempfile::tempdir().unwrap() }
    }

    /// Pids of this test's browser processes (Chrome's main process: no
    /// `--type=`).
    fn browsers(&self) -> Vec<u32> {
        processes()
            .into_iter()
            .filter(|(_, cmd)| cmd.contains(&self.marker) && !cmd.contains("--type="))
            .map(|(pid, _)| pid)
            .collect()
    }

    /// Wait until `browsers()` has `n` entries.
    fn wait_browsers(&self, n: usize, within: Duration) -> Vec<u32> {
        let deadline = Instant::now() + within;
        loop {
            let found = self.browsers();
            if found.len() == n || Instant::now() > deadline {
                return found;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

/// (pid, command line) of every process.
fn processes() -> Vec<(u32, String)> {
    let out = if cfg!(windows) {
        Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Get-CimInstance Win32_Process | ForEach-Object { \"$($_.ProcessId) $($_.CommandLine)\" }",
            ])
            .output()
    } else {
        Command::new("ps").args(["-axww", "-o", "pid=,command="]).output()
    };
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let l = l.trim_start();
            let (pid, cmd) = l.split_once(' ')?;
            Some((pid.parse().ok()?, cmd.to_string()))
        })
        .collect()
}

fn kill(pid: u32) {
    if cfg!(windows) {
        let _ = Command::new("taskkill").args(["/F", "/PID", &pid.to_string()]).output();
    } else {
        let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
    }
}

struct Session {
    sock: String,
    downloads: tempfile::TempDir,
}

impl Session {
    fn downloads_str(&self) -> String {
        self.downloads.path().display().to_string()
    }

    fn run_with(&self, mut cmd: Command, args: &[&str]) -> (bool, Value) {
        let out = cmd.arg("--session").arg(&self.sock).args(args).output().expect("run navigera");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|_| panic!("{args:?}: not JSON: {stdout} / {}", String::from_utf8_lossy(&out.stderr)));
        (out.status.success(), value)
    }

    fn run(&self, args: &[&str]) -> (bool, Value) {
        self.run_with(Command::new(tool_exe()), args)
    }

    fn ok(&self, args: &[&str]) -> Value {
        let (success, r) = self.run(args);
        assert!(success && r["ok"] == true, "{args:?} failed: {r}");
        r["result"].clone()
    }

    fn fails(&self, args: &[&str]) -> String {
        let (success, r) = self.run(args);
        assert!(!success && r["ok"] == false, "{args:?} should fail: {r}");
        r["error"].as_str().unwrap_or_default().to_string()
    }

    fn text(&self, args: &[&str]) -> String {
        let r = self.ok(args);
        r.as_str().map(str::to_string).unwrap_or_else(|| r.to_string())
    }

    fn sessions_on_browser(&self) -> String {
        self.ok(&["status"])["browser"].as_str().unwrap_or_default().to_string()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = Command::new(tool_exe()).arg("--session").arg(&self.sock).arg("quit").output();
    }
}

/// Wait until `f` holds, or fail with `what`.
fn eventually(what: &str, within: Duration, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + within;
    while !f() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Two sessions: one browser process, two contexts; cookies, storage and
/// downloads of one are not the other's; `quit` of one leaves the other
/// working; the browser exits after the grace once both are gone.
#[test]
fn two_sessions_share_one_browser_and_nothing_else() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let browser = Browser::new("two", 3);
    let a = browser.start("a", &[]);
    let b = browser.start("b", &[]);
    assert_eq!(browser.wait_browsers(1, Duration::from_secs(5)).len(), 1, "one browser for both");
    assert_eq!(a.sessions_on_browser(), "shared (2 sessions)");

    let page = format!("{}/download", site.url);
    a.ok(&["goto", &page]);
    b.ok(&["goto", &page]);
    a.ok(&["eval", "localStorage.setItem('who', 'a'); document.cookie = 'who=a; path=/'; 1"]);
    assert_eq!(a.text(&["eval", "localStorage.getItem('who') + ' ' + document.cookie"]), "a who=a");
    b.ok(&["goto", &page]);
    assert_eq!(b.text(&["eval", "String(localStorage.getItem('who')) + ' [' + document.cookie + ']'"]), "null []");
    assert_eq!(a.ok(&["cookies", "--all", "--json"]).as_array().map(Vec::len), Some(1));
    assert_eq!(b.ok(&["cookies", "--all", "--json"]).as_array().map(Vec::len), Some(0), "B sees no cookie of A's");

    // A download lands in the session's own directory only.
    a.ok(&["click", "--selector", "#dl"]);
    let r = a.ok(&["wait", "--download", "*.csv"]);
    let path = r["download"]["path"].as_str().expect("a path").to_string();
    assert!(path.starts_with(&a.downloads_str()), "{path} not in A's {}", a.downloads_str());
    assert_eq!(std::fs::read_dir(b.downloads.path()).unwrap().count(), 0, "nothing in B's directory");
    assert_eq!(b.ok(&["downloads", "--json"]).as_array().map(Vec::len), Some(0));

    // A tab A opens is not B's.
    a.ok(&["tab-new"]);
    assert_eq!(b.ok(&["tab-list"])["tabs"].as_array().map(Vec::len), Some(1));

    // `quit --close-browser` closes A's context, not the browser B uses.
    let bye = a.ok(&["quit", "--close-browser"]);
    assert!(bye["browser"].as_str().unwrap_or("").starts_with("left open: 1 other session"), "{bye}");
    assert_eq!(b.sessions_on_browser(), "shared (1 session)");
    assert_eq!(b.text(&["eval", "document.title"]), "download");
    assert_eq!(browser.browsers().len(), 1);

    b.ok(&["quit"]);
    // Kept warm for the grace, then gone: still there a few of the host's
    // 250 ms checks later (A's --close-browser, with B still on it, must
    // not close it the moment B leaves).
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(browser.browsers().len(), 1, "kept for the grace period");
    assert!(browser.wait_browsers(0, Duration::from_secs(15)).is_empty(), "the browser exits after the grace");
}

/// Sessions starting at the same moment end up on one browser.
#[test]
fn simultaneous_starts_make_one_browser() {
    if !chrome() {
        return;
    }
    let browser = Browser::new("simul", 2);
    let sessions: Vec<Session> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..3)
            .map(|i| {
                scope.spawn({
                    let browser = &browser;
                    move || browser.start(&format!("s{i}"), &[])
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(browser.wait_browsers(1, Duration::from_secs(5)).len(), 1, "one browser for three starts");
    assert_eq!(sessions[0].sessions_on_browser(), "shared (3 sessions)");
}

/// A session server killed outright takes its context with it.
#[test]
fn a_killed_session_leaves_no_context_behind() {
    if !chrome() {
        return;
    }
    let browser = Browser::new("killed", 2);
    let a = browser.session("a");
    let (_, started) = a.run_with(browser.command(), &["start"]);
    let pid = started["result"]["pid"].as_u64().expect("start says its server's pid") as u32;
    let b = browser.start("b", &[]);
    assert_eq!(b.sessions_on_browser(), "shared (2 sessions)");
    kill(pid);
    eventually("A's context is disposed", Duration::from_secs(10), || b.sessions_on_browser() == "shared (1 session)");
    assert_eq!(b.text(&["eval", "1 + 1"]), "2");
}

/// `--own-browser`: a browser per session, as before.
#[test]
fn own_browser_keeps_a_browser_per_session() {
    if !chrome() {
        return;
    }
    let browser = Browser::new("own", 2);
    let a = browser.start("a", &["--own-browser"]);
    let _b = browser.start("b", &["--own-browser"]);
    assert_eq!(browser.wait_browsers(2, Duration::from_secs(5)).len(), 2, "two browsers");
    assert_eq!(a.ok(&["status"])["browser"], "own");
}

/// Two sessions with a cookie, localStorage and two tabs each (the second
/// tab last loaded by a form POST, in `a`), the browser killed: each
/// session's next command relaunches or joins one browser, reopens its tabs,
/// restores its storage, and says what was lost, as a note and as the
/// `recovered` field.
#[test]
fn a_killed_shared_browser_is_relaunched_and_sessions_carry_on() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let browser = Browser::new("dead", 5);
    let a = browser.start("a", &[]);
    let b = browser.start("b", &[]);
    let page = format!("{}/download", site.url);
    for (s, who) in [(&a, "a"), (&b, "b")] {
        s.ok(&["goto", &page]);
        s.ok(&["eval", &format!("localStorage.setItem('who', '{who}'); document.cookie = 'who={who}; path=/'; 1")]);
        s.ok(&["tab-new", &format!("{}/download?tab={who}", site.url)]);
    }
    // A tab whose last document load was a form POST.
    a.ok(&[
        "eval",
        "document.body.insertAdjacentHTML('beforeend', '<form id=pf method=post action=/posted></form>'); \
         setTimeout(() => document.getElementById('pf').submit(), 0); 1",
    ]);
    eventually("the POST loaded", Duration::from_secs(20), || a.text(&["eval", "location.pathname"]) == "/posted");
    b.ok(&["tab-select", "0"]);
    // The storage snapshot is taken once a session has been idle for 1 s.
    std::thread::sleep(Duration::from_millis(1800));
    let pids = browser.wait_browsers(1, Duration::from_secs(5));
    assert_eq!(pids.len(), 1);
    kill(pids[0]);
    // The sessions see the connection end (a command racing it is in flight).
    std::thread::sleep(Duration::from_millis(300));

    // Both at once: one relaunch, the other joins it.
    let started = Instant::now();
    let probe = ["eval", "[localStorage.getItem('who'), document.cookie, location.pathname]"];
    let (ra, rb) = std::thread::scope(|scope| {
        let ha = scope.spawn(|| a.run(&probe));
        let hb = scope.spawn(|| b.run(&probe));
        (ha.join().unwrap(), hb.join().unwrap())
    });
    assert!(started.elapsed() < Duration::from_secs(15), "recovery took {:?}", started.elapsed());
    for (name, (success, r)) in [("a", ra), ("b", rb)] {
        assert!(success && r["ok"] == true, "{name}: {r}");
        assert_eq!(r["recovered"]["browser"], "relaunched", "{name}: {r}");
        assert_eq!(r["recovered"]["tabs"], 2, "{name}: {r}");
        assert_eq!(r["recovered"]["reopened"], 2, "{name}: {r}");
        assert!(r["recovered"]["lost"].as_array().is_some_and(|l| !l.is_empty()), "{name}: {r}");
        let note = r["notes"][0].as_str().unwrap_or_default();
        assert!(note.contains("relaunched") && note.contains("lost:"), "{name}: {note}");
        // Storage and cookie came back.
        assert_eq!(r["result"][0], name, "{name}: {r}");
        assert_eq!(r["result"][1], format!("who={name}"), "{name}: {r}");
        // `a`'s active tab was last loaded by the form POST; `b` had none.
        let as_get = if name == "a" { serde_json::json!([1]) } else { serde_json::json!([]) };
        assert_eq!(r["recovered"]["reopened_as_get"], as_get, "{name}: {r}");
    }
    let tabs = a.ok(&["tab-list"]);
    assert_eq!(tabs["tabs"].as_array().map(Vec::len), Some(2), "{tabs}");
    // One browser for both.
    assert_eq!(browser.wait_browsers(1, Duration::from_secs(5)).len(), 1, "one relaunch, one browser");
    assert_eq!(a.sessions_on_browser(), "shared (2 sessions)");
    // `b` had tab 0 active when it died; it still is.
    let tabs = b.ok(&["tab-list"]);
    assert_eq!(tabs["active"], 0, "{tabs}");
    assert_eq!(tabs["tabs"].as_array().map(Vec::len), Some(2), "{tabs}");
}

/// The tab a form POST loaded is reopened with GET, and the note says so.
#[test]
fn a_tab_loaded_by_a_post_is_reopened_with_get_and_named() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let browser = Browser::new("post", 5);
    let a = browser.start("a", &[]);
    a.ok(&["goto", &format!("{}/download", site.url)]);
    a.ok(&[
        "eval",
        "document.body.insertAdjacentHTML('beforeend', '<form id=pf method=post action=/posted></form>'); \
         setTimeout(() => document.getElementById('pf').submit(), 0); 1",
    ]);
    eventually("the POST loaded", Duration::from_secs(20), || a.text(&["eval", "location.pathname"]) == "/posted");

    let pids = browser.wait_browsers(1, Duration::from_secs(5));
    kill(pids[0]);
    std::thread::sleep(Duration::from_millis(300));
    let (success, r) = a.run(&["eval", "location.pathname"]);
    assert!(success && r["ok"] == true, "{r}");
    assert_eq!(r["result"], "/posted", "{r}");
    assert_eq!(r["recovered"]["reopened_as_get"], serde_json::json!([0]), "{r}");
    assert!(r["notes"][0].as_str().unwrap_or_default().contains("form POST"), "{r}");
}

/// A command running when the browser dies fails and is not retried (it may
/// have had a side effect); the next command recovers.
#[test]
fn a_command_running_when_the_browser_dies_is_not_retried() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let browser = Browser::new("inflight", 5);
    let a = browser.start("a", &[]);
    a.ok(&["goto", &format!("{}/download", site.url)]);
    let pids = browser.wait_browsers(1, Duration::from_secs(5));
    let err = std::thread::scope(|scope| {
        let h = scope.spawn(|| a.fails(&["eval", "new Promise(r => setTimeout(r, 6000))"]));
        std::thread::sleep(Duration::from_millis(700));
        kill(pids[0]);
        h.join().unwrap()
    });
    assert!(err.contains("was not retried") && err.contains("next command relaunches"), "{err}");
    let (success, r) = a.run(&["eval", "1 + 1"]);
    assert!(success && r["ok"] == true && r["result"] == 2, "{r}");
    assert_eq!(r["recovered"]["browser"], "relaunched", "{r}");
}

/// A browser that dies again right after a recovery ends the session, with
/// a plain error: no crash loop. `start` makes a new one.
#[test]
fn a_second_crash_right_after_a_recovery_ends_the_session() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let browser = Browser::new("loop", 5);
    let a = browser.start("a", &[]);
    a.ok(&["goto", &format!("{}/download", site.url)]);
    kill(browser.wait_browsers(1, Duration::from_secs(5))[0]);
    std::thread::sleep(Duration::from_millis(300));
    // A command that reaches the session before it has seen the browser go
    // counts as in flight and fails unretried (a loaded machine is slow to
    // see it); the next one is the first after the crash.
    let (mut success, mut r) = a.run(&["eval", "1"]);
    if !success && r.to_string().contains("was not retried") {
        (success, r) = a.run(&["eval", "1"]);
    }
    assert!(success && r["recovered"]["browser"] == "relaunched", "{r}");
    kill(browser.wait_browsers(1, Duration::from_secs(5))[0]);
    std::thread::sleep(Duration::from_millis(300));
    let mut err = a.fails(&["eval", "1"]);
    if err.contains("was not retried") {
        err = a.fails(&["eval", "1"]);
    }
    assert!(err.contains("crashed again") && err.contains("start"), "{err}");
    let a = browser.start("a", &[]);
    assert_eq!(a.text(&["eval", "1 + 1"]), "2");
}
