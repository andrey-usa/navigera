//! Obscura (`--engine obscura`) and Moli (`--engine moli`) end to end: the
//! first compatibility pass on each, through navigera's own protocol layer.
//! Navigation, the accessibility tree and acting on its refs, basic input
//! (fill, a keyboard submit, a checkbox click, a select), tabs, a
//! screenshot, and cleanup: after `quit` the engine's CDP server is gone.
//! The pages are rendered by the server, so the pass does not lean on what
//! an engine does with content its scripts add later (the differences
//! that remain are listed in docs/navigera.md).
//!
//! Each engine runs when `$OBSCURA_BIN` / `$MOLI_BIN` or the binary on PATH
//! is found; with `NAVIGERA_REQUIRE_OBSCURA=1` / `NAVIGERA_REQUIRE_MOLI=1`
//! (CI) a missing binary is a failure.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use serde_json::Value;

fn tool_exe() -> String {
    env!("CARGO_BIN_EXE_navigera").to_string()
}

fn python() -> &'static str {
    if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}

/// The engine's binary: `$<ENGINE>_BIN`, else `<engine>` on PATH.
fn binary(engine: &str) -> Option<PathBuf> {
    let var = format!("{}_BIN", engine.to_ascii_uppercase());
    let found = std::env::var_os(&var).map(PathBuf::from).filter(|p| p.is_file()).or_else(|| {
        std::env::split_paths(&std::env::var_os("PATH")?)
            .map(|d| d.join(if cfg!(windows) { format!("{engine}.exe") } else { engine.to_string() }))
            .find(|p| p.is_file())
    });
    let require = format!("NAVIGERA_REQUIRE_{}", engine.to_ascii_uppercase());
    assert!(
        found.is_some() || std::env::var_os(&require).is_none(),
        "{require} is set but no {engine} binary was found"
    );
    found
}

struct Site {
    child: Child,
    base: String,
}

impl Drop for Site {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The Acme Supply fixture site on a free port.
fn acme() -> Option<Site> {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/bench/site/server.py");
    let mut child = Command::new(python())
        .args([script, "--port", "0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .ok()?;
    let mut line = String::new();
    BufReader::new(child.stdout.take()?).read_line(&mut line).ok()?;
    let base = line.split_whitespace().nth(1)?.to_string();
    Some(Site { child, base })
}

fn server_state(base: &str) -> Value {
    use std::io::{Read, Write};
    let addr = base.trim_start_matches("http://");
    let mut s = std::net::TcpStream::connect(addr).expect("connect site");
    write!(s, "GET /__state HTTP/1.0\r\nHost: {addr}\r\n\r\n").unwrap();
    let mut body = String::new();
    s.read_to_string(&mut body).unwrap();
    serde_json::from_str(&body[body.find("\r\n\r\n").expect("http body") + 4..]).expect("state json")
}

/// One named session on `engine`, driven one CLI call per step like an agent.
struct Agent {
    session: String,
    engine: &'static str,
    bin: PathBuf,
}

impl Agent {
    fn new(engine: &'static str, bin: PathBuf) -> Self {
        let session =
            std::env::temp_dir().join(format!("nv-{engine}-first-{}.sock", std::process::id())).display().to_string();
        let agent = Agent { session, engine, bin };
        agent.ok(&["start", "--idle-timeout-s", "300"]);
        agent
    }

    fn run(&self, args: &[&str]) -> Value {
        let out = Command::new(tool_exe())
            .args(["--session", &self.session, "--engine", self.engine, "--chromium"])
            .arg(&self.bin)
            .args(args)
            .env("NAVIGERA_OUTPUT", "json")
            // `serve` is headless: no X server, no xvfb-run.
            .env_remove("DISPLAY")
            .stdin(Stdio::null())
            .output()
            .expect("run navigera");
        let text = String::from_utf8_lossy(&out.stdout);
        serde_json::from_str(text.trim()).unwrap_or_else(|_| {
            panic!(
                "stdout must be one JSON object for {args:?}: {text}\nstderr: {}",
                String::from_utf8_lossy(&out.stderr)
            )
        })
    }

    fn ok(&self, args: &[&str]) -> Value {
        let r = self.run(args);
        assert_eq!(r["ok"], true, "{}: {args:?} failed: {r}", self.engine);
        r["result"].clone()
    }

    fn ax(&self) -> String {
        self.ok(&["ax"]).as_str().expect("ax returns text").to_string()
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = Command::new(tool_exe()).args(["--session", &self.session, "quit"]).stdin(Stdio::null()).output();
    }
}

/// The ref the tree gives the first line that contains `what`.
fn ref_of(tree: &str, what: &str) -> String {
    let line = tree.lines().find(|l| l.contains(what)).unwrap_or_else(|| panic!("no {what:?} in the tree:\n{tree}"));
    let at = line.find("ref=").unwrap_or_else(|| panic!("{what:?} has no ref: {line}")) + 4;
    line[at..].chars().take_while(char::is_ascii_digit).collect()
}

fn first_pass(engine: &'static str) {
    let Some(bin) = binary(engine) else {
        eprintln!("skipping: no {engine} binary");
        return;
    };
    let Some(site) = acme() else {
        eprintln!("skipping: python3 not available for the fixture site");
        return;
    };
    let url = |path: &str| format!("{}{path}", site.base);
    let agent = Agent::new(engine, bin.clone());

    // The session runs the engine asked for, from its binary.
    let status = agent.ok(&["status"]);
    assert_eq!(status["engine"], engine, "{status}");
    assert_eq!(status["executable"], bin.display().to_string(), "{status}");
    let endpoint = status["endpoint"].as_str().expect("an endpoint").to_string();

    // Navigation and the tree: a form read with refs, acted on by ref.
    let page = agent.ok(&["goto", &url("/login")]);
    assert_eq!(page["title"], "Sign in — Acme Supply", "{page}");
    let tree = agent.ax();
    assert!(tree.starts_with("page \"Sign in — Acme Supply\""), "the tree names its page:\n{tree}");
    for want in ["textbox \"Email\"", "textbox \"Password\"", "button \"Sign in\""] {
        assert!(tree.contains(want), "{want} missing:\n{tree}");
    }
    let email = ref_of(&tree, "textbox \"Email\"");
    agent.ok(&["fill", &email, "demo@acme.test"]);
    assert_eq!(agent.ok(&["get", "value", "--selector", "#email"]), "demo@acme.test");
    agent.ok(&["fill", "--selector", "#password", "hunter2"]);
    // A keyboard submit: the form posts and the server signs the session in.
    agent.ok(&["press", "Enter", "--selector", "#password"]);
    agent.ok(&["wait", "--url", "/account"]);
    assert_eq!(server_state(&site.base)["logins"][0], "demo@acme.test", "the server saw the sign-in");

    // An eval with an effect runs once: Obscura 0.2.7 runs what it is asked
    // to check under `throwOnSideEffect`, and the read tried first sent this
    // POST twice (the check, then the awaited read).
    let posted = agent.ok(&[
        "eval",
        "fetch('/api/tickets', {method: 'POST', headers: {'content-type': 'application/json'}, \
         body: JSON.stringify({category: 'Billing', priority: 'low', message: 'once'})}).then(r => r.status)",
    ]);
    // Obscura answers a whole number as a float (200.0).
    assert_eq!(posted.as_f64(), Some(200.0), "{posted}");
    let tickets = server_state(&site.base)["tickets"].as_array().map(Vec::len);
    assert_eq!(tickets, Some(1), "one eval, one POST");
    // What navigera asks each browser before it tries an eval as a read:
    // V8 refuses the write under the check (Moli hands `Runtime` to V8's
    // own inspector); Obscura runs it (a no-op: `undefined` is not
    // writable), so there every eval is an action. The one POST above is
    // the proof: the read tried first would have sent it twice.
    let probe = agent.ok(&[
        "cdp",
        "Runtime.evaluate",
        r#"{"expression":"globalThis.undefined = undefined","returnByValue":true,"throwOnSideEffect":true}"#,
    ]);
    let refused = probe["result"]["exceptionDetails"]["exception"]["className"] == "EvalError";
    assert_eq!(refused, engine != "obscura", "{engine}: {probe}");

    // Dialogs: answered per the session's policy and reported, on an engine
    // that announces them (Moli) and on one whose own `confirm()` answers
    // false and `prompt()` null at once and tells no one (Obscura 0.2.7):
    // there navigera's page-side functions answer instead.
    let r = agent.run(&["eval", "confirm('Delete it?')"]);
    assert_eq!(r["result"], true, "{engine}: accepted by default: {r}");
    assert_eq!(r["dialogs"][0]["type"], "confirm", "{engine}: {r}");
    assert_eq!(r["dialogs"][0]["message"], "Delete it?", "{engine}: {r}");
    agent.ok(&["dialog", "--dismiss"]);
    let r = agent.run(&["eval", "[confirm('Really?'), prompt('Name?', 'x')]"]);
    assert_eq!(r["result"], serde_json::json!([false, null]), "{engine}: {r}");
    assert_eq!(r["dialogs"].as_array().map(Vec::len), Some(2), "{engine}: {r}");
    assert_eq!(r["dialogs"][1]["type"], "prompt", "{engine}: {r}");
    assert_eq!(r["dialogs"][1]["accepted"], false, "{engine}: {r}");
    // A new document keeps the policy.
    agent.ok(&["dialog", "--accept", "--prompt-text", "Ada"]);
    agent.ok(&["goto", &url("/docs")]);
    let r = agent.run(&["eval", "prompt('Name?')"]);
    assert_eq!(r["result"], "Ada", "{engine}: {r}");
    assert_eq!(r["dialogs"][0]["accepted"], true, "{engine}: {r}");

    // A click that changes a control's state: the first box not ticked
    // (near the top: Moli 1.1.27 keeps an element's box where it was before
    // a scroll), and a select.
    agent.ok(&["goto", &url("/preferences")]);
    let before = agent.ax();
    let unticked = before
        .lines()
        .find(|l| l.contains("checkbox \"") && !l.contains("[checked"))
        .unwrap_or_else(|| panic!("no unticked checkbox:\n{before}"));
    let name = unticked.split('"').nth(1).expect("a named checkbox").to_string();
    let box_ref = ref_of(unticked, "ref=");
    agent.ok(&["click", &box_ref]);
    assert_eq!(agent.ok(&["is", "checked", &box_ref]), true, "the click ticked {name:?}");
    agent.ok(&["select", "--selector", "select[name=currency]", "EUR"]);
    assert_eq!(agent.ok(&["get", "value", "--selector", "select[name=currency]"]), "EUR");

    // Tabs: a second one opens, is listed, and closes.
    agent.ok(&["tab-new", &url("/docs")]);
    let tabs = agent.ok(&["tab-list"]);
    assert_eq!(tabs["tabs"].as_array().map(Vec::len), Some(2), "{tabs}");
    assert!(tabs["tabs"][1]["url"].as_str().unwrap_or_default().ends_with("/docs"), "{tabs}");
    agent.ok(&["tab-close"]);
    assert_eq!(agent.ok(&["tab-list"])["tabs"].as_array().map(Vec::len), Some(1));

    // A screenshot is a PNG of the page.
    let shot = std::env::temp_dir().join(format!("nv-{engine}-first-{}.png", std::process::id()));
    agent.ok(&["screenshot", &shot.display().to_string()]);
    let png = std::fs::read(&shot).expect("the screenshot file");
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n") && png.len() > 1000, "{} bytes", png.len());
    let _ = std::fs::remove_file(&shot);

    // Cleanup: quit takes the engine's server down with the session.
    agent.ok(&["quit"]);
    let port = endpoint.trim_start_matches("ws://").split(['/', ':']).nth(1).expect("a port").to_string();
    let addr = format!("127.0.0.1:{port}");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::net::TcpStream::connect(&addr).is_ok() {
        assert!(std::time::Instant::now() < deadline, "{engine} still listens on {addr} after quit");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[test]
fn first_pass_on_obscura() {
    first_pass("obscura");
}

#[test]
fn first_pass_on_moli() {
    first_pass("moli");
}

/// Content a page's script adds after the first look: a button that shows
/// up later is waited for, clicked with the mouse, and what it adds is
/// waited for and read. Moli 1.1.27 keeps the layout it first built until
/// a screenshot, so without navigera laying the page out again none of
/// this has a box or text there.
fn late_content(engine: &'static str) {
    let Some(bin) = binary(engine) else {
        eprintln!("skipping: no {engine} binary");
        return;
    };
    let Some(site) = acme() else {
        eprintln!("skipping: python3 not available for the fixture site");
        return;
    };
    let agent = Agent::new(engine, bin);
    agent.ok(&["goto", &format!("{}/login", site.base)]);
    // The first look lays the page out.
    agent.ax();
    agent.ok(&[
        "eval",
        "setTimeout(() => { const b = document.createElement('button'); b.id = 'late'; \
         b.textContent = 'Late action'; b.onclick = () => { const p = document.createElement('p'); \
         p.textContent = 'Late result shown'; document.body.prepend(p); }; document.body.prepend(b); }, 300); 0",
    ]);
    agent.ok(&["wait", "--text", "Late action"]);
    let clicked = agent.ok(&["click", "--selector", "#late"]);
    assert!(clicked.get("synthetic_click").is_none(), "{engine}: the late button has a box to click: {clicked}");
    agent.ok(&["wait", "--text", "Late result shown"]);
    let text = agent.ok(&["text"]);
    assert!(text.as_str().is_some_and(|t| t.contains("Late result shown")), "{engine}: text reads it: {text}");
    // Added with no wait in between: the click finds its box at once.
    agent.ok(&[
        "eval",
        "const n = document.createElement('button'); n.id = 'now'; n.textContent = 'Now'; \
         n.onclick = () => { document.title = 'now clicked'; }; document.body.prepend(n); 0",
    ]);
    let clicked = agent.ok(&["click", "--selector", "#now"]);
    assert!(clicked.get("synthetic_click").is_none(), "{engine}: {clicked}");
    assert_eq!(agent.ok(&["eval", "document.title"]), "now clicked", "{engine}");
    agent.ok(&["quit"]);
}

#[test]
fn late_content_on_obscura() {
    late_content("obscura");
}

#[test]
fn late_content_on_moli() {
    late_content("moli");
}
