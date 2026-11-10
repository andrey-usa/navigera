//! `eval` as a read: an expression V8 shows changes nothing is answered in
//! one message, without the after-action arm; anything else runs as an
//! action, as before, exactly once, and a refusal of the read never reaches
//! the caller.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_navigera");
const PAGE: &str = "data:text/html,<title>t</title><p>x</p>";

fn chrome() -> bool {
    let found = navigera::browser::resolve_executable_for(None, false).is_some();
    assert!(
        found || std::env::var_os("NAVIGERA_REQUIRE_BROWSER").is_none(),
        "NAVIGERA_REQUIRE_BROWSER is set but no Chrome/Chromium was found"
    );
    found
}

/// A session of the test's own, in a session directory of its own; its
/// browser is stopped however the test ends.
struct Session(PathBuf);

impl Session {
    fn start(test: &str) -> Session {
        let dir = std::env::temp_dir().join(format!("nv-eval-read-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let s = Session(dir);
        s.ok(&["start"]);
        s.ok(&["goto", PAGE]);
        s
    }

    fn run(&self, args: &[&str]) -> (bool, Value) {
        let out = command(&self.0).args(args).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        let value = serde_json::from_str(text.trim()).unwrap_or_else(|_| {
            panic!("{args:?}: stdout is not one JSON object: {text}\n{}", String::from_utf8_lossy(&out.stderr))
        });
        (out.status.success(), value)
    }

    fn ok(&self, args: &[&str]) -> Value {
        let (success, r) = self.run(args);
        assert!(success && r["ok"] == true, "{args:?} failed: {r}");
        r["result"].clone()
    }

    fn eval(&self, expression: &str) -> Value {
        self.ok(&["eval", expression])
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = command(&self.0).arg("quit").output();
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn command(dir: &Path) -> Command {
    let mut cmd = Command::new(BIN);
    cmd.args(["-s", "work"])
        .env("NAVIGERA_OUTPUT", "json")
        .env("XDG_RUNTIME_DIR", dir)
        .env("TEMP", dir)
        .env("TMP", dir)
        .stdin(Stdio::null());
    cmd
}

#[test]
fn a_read_is_answered_without_arming_the_page() {
    if !chrome() {
        return;
    }
    let s = Session::start("read");
    // The arm an action sends first would be on the page by now.
    assert_eq!(s.eval("typeof window.__nvMut"), "undefined");
    assert_eq!(s.eval("document.title"), "t");
    assert_eq!(s.eval("() => document.querySelectorAll('p').length"), 1);
    assert_eq!(s.eval("typeof window.__nvMut"), "undefined", "a read armed the page");
}

#[test]
fn a_write_runs_once_as_an_action() {
    if !chrome() {
        return;
    }
    let s = Session::start("write");
    // Refused as a read, then run: the count says it ran once, not twice.
    assert_eq!(s.eval("(() => { window.c = (window.c || 0) + 1; return window.c })()"), 1);
    assert_eq!(s.eval("window.c"), 1);
    // A write with no assignment in its text: tried as a read, refused by
    // V8 before it ran, then run once as an action.
    assert_eq!(s.eval("document.body.appendChild(document.createElement('i')).tagName"), "I");
    assert_eq!(s.eval("document.querySelectorAll('i').length"), 1);
    assert_eq!(s.eval("document.title = 'u'"), "u");
    assert_eq!(s.eval("document.title"), "u");
    // It ran as an action: the arm went first.
    assert_eq!(s.eval("typeof window.__nvMut"), "object");
}

#[test]
fn statements_and_errors_answer_as_before() {
    if !chrome() {
        return;
    }
    let s = Session::start("forms");
    assert_eq!(s.eval("let a = 2; a * 3"), 6);
    assert_eq!(s.eval("return 1 + 1"), 2);
    let (success, r) = s.run(&["eval", "throw new Error('boom')"]);
    assert!(!success && r["ok"] == false, "{r}");
    assert!(r.to_string().contains("boom"), "the page's error, not the read's refusal: {r}");
}
