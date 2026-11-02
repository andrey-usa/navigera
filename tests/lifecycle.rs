//! Session lifecycle: `sessions` lists and cleans, `--parent` ties a
//! session to its caller, `start --idle` ends a forgotten one, and a
//! command that came in while another client's was running is told so.
//!
//! Each test has a session directory of its own (`$XDG_RUNTIME_DIR`, or
//! `%TEMP%` on Windows), so `sessions --clean --all` touches only its own.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

fn chrome() -> bool {
    let found = navigera::browser::resolve_executable_for(None, true);
    assert!(
        found.is_some() || std::env::var_os("NAVIGERA_REQUIRE_BROWSER").is_none(),
        "NAVIGERA_REQUIRE_BROWSER is set but no Chrome/Chromium was found"
    );
    found.is_some()
}

/// A session directory of the test's own; every session in it is quit
/// when the test ends.
struct Dir {
    root: tempfile::TempDir,
}

impl Dir {
    fn new() -> Dir {
        Dir { root: tempfile::tempdir().unwrap() }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_navigera"));
        if cfg!(windows) {
            cmd.env("TEMP", self.root.path()).env("TMP", self.root.path());
        } else {
            cmd.env("XDG_RUNTIME_DIR", self.root.path());
        }
        cmd.env("NAVIGERA_SHARED_GRACE_S", "2").env_remove("NAVIGERA_SESSION").env_remove("NAVIGERA_PARENT_PID");
        cmd.stdin(Stdio::null());
        cmd
    }

    fn run(&self, args: &[&str]) -> (bool, Value) {
        let out = self.command().args(args).output().expect("run navigera");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|_| panic!("{args:?}: not JSON: {stdout} / {}", String::from_utf8_lossy(&out.stderr)));
        (out.status.success(), value)
    }

    fn ok(&self, args: &[&str]) -> Value {
        let (success, r) = self.run(args);
        assert!(success && r["ok"] == true, "{args:?} failed: {r}");
        r
    }

    fn sessions(&self) -> Value {
        self.ok(&["sessions"])["result"].clone()
    }

    fn names(&self) -> Vec<String> {
        let list = self.sessions();
        list["sessions"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap().to_string()).collect()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = self.command().args(["sessions", "--clean", "--all"]).output();
    }
}

fn eventually(what: &str, within: Duration, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + within;
    while !f() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A process to be a session's `--parent`.
fn caller() -> std::process::Child {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("powershell");
        c.args(["-NoProfile", "-Command", "Start-Sleep -Seconds 120"]);
        c
    } else {
        let mut c = Command::new("sleep");
        c.arg("120");
        c
    };
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).spawn().expect("spawn a caller")
}

/// `--parent <pid>`: the session quits within a few seconds of its caller
/// being killed; a parent that isn't running is refused at `start`.
#[test]
fn a_session_ends_with_its_parent() {
    if !chrome() {
        return;
    }
    let dir = Dir::new();
    let mut parent = caller();
    let pid = parent.id().to_string();
    dir.ok(&["-s", "child", "start", "--parent", &pid]);
    dir.ok(&["-s", "other", "start"]);
    let list = dir.sessions();
    let child = list["sessions"].as_array().unwrap().iter().find(|s| s["name"] == "child").cloned().unwrap();
    assert_eq!(child["parent"].as_u64(), Some(u64::from(parent.id())), "{list}");

    parent.kill().unwrap();
    parent.wait().unwrap();
    let killed = Instant::now();
    eventually("the session is gone within 5 s of its parent", Duration::from_secs(5), || dir.names() == ["other"]);
    eprintln!("session gone {:?} after its parent", killed.elapsed());
    let (ok, r) = dir.run(&["-s", "child", "url"]);
    assert!(!ok, "{r}");
    assert!(r["error"].as_str().unwrap().contains("--parent"), "the log says why: {r}");

    let (ok, r) = dir.run(&["-s", "late", "start", "--parent", &pid]);
    assert!(!ok && r["error"].as_str().unwrap().contains("no process"), "{r}");
}

/// `sessions` lists each session with its browser and memory and the
/// shared browser; `--clean` quits the idle ones and drops stale files;
/// `--clean --all` quits the rest, closes the shared browser and says how
/// much memory that freed.
#[test]
fn sessions_lists_and_cleans() {
    if !chrome() {
        return;
    }
    let dir = Dir::new();
    dir.ok(&["-s", "a", "start"]);
    dir.ok(&["-s", "b", "start", "--own-browser"]);
    dir.ok(&["-s", "a", "goto", "data:text/html,<title>A</title>"]);
    let list = dir.sessions();
    let sessions = list["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2, "{list}");
    let a = sessions.iter().find(|s| s["name"] == "a").unwrap();
    let b = sessions.iter().find(|s| s["name"] == "b").unwrap();
    assert_eq!(a["browser"], "shared (1 session)", "{list}");
    assert_eq!(b["browser"], "own", "{list}");
    assert!(a["pid"].as_u64().unwrap() > 0 && a["url"].as_str().unwrap().contains("title"), "{list}");
    let shared = list["shared_browsers"].as_array().unwrap();
    assert_eq!(shared.len(), 1, "{list}");
    assert!(shared[0]["memory_mib"].as_f64().unwrap() > 20.0, "the shared browser's memory: {list}");
    assert!(b["memory_mib"].as_f64().unwrap() > 20.0, "an own browser counts in its session: {list}");
    assert!(a["memory_mib"].as_f64().unwrap() < 50.0, "the shared browser is not counted in A: {list}");

    // A file no server answers on is stale; `--clean` removes it and
    // quits nothing that was used within the hour.
    let session_dir = dir.root.path().join("navigera");
    std::fs::write(session_dir.join("ghost.sock"), "").unwrap();
    let list = dir.sessions();
    let ghost = list["sessions"].as_array().unwrap().iter().find(|s| s["name"] == "ghost").cloned().unwrap();
    assert!(ghost["state"].as_str().unwrap().starts_with("stale"), "{list}");
    let cleaned = dir.ok(&["sessions", "--clean", "--idle", "3600"])["result"].clone();
    assert_eq!(cleaned["removed"], serde_json::json!(["ghost"]), "{cleaned}");
    assert_eq!(cleaned["quit"], serde_json::json!([]), "{cleaned}");
    assert_eq!(dir.names(), ["a", "b"]);

    let cleaned = dir.ok(&["sessions", "--clean", "--all"])["result"].clone();
    assert_eq!(cleaned["quit"], serde_json::json!(["a", "b"]), "{cleaned}");
    assert_eq!(cleaned["shared_browsers"], serde_json::json!([]), "the shared browser closed at once: {cleaned}");
    assert!(cleaned["freed_mib"].as_f64().unwrap() > 40.0, "{cleaned}");
    assert!(dir.names().is_empty());
}

/// `start --idle <s>`: a session nobody uses ends after that long.
#[test]
fn an_idle_session_ends() {
    if !chrome() {
        return;
    }
    let dir = Dir::new();
    dir.ok(&["-s", "nap", "start", "--idle", "1"]);
    eventually("the idle session ends", Duration::from_secs(10), || dir.names().is_empty());
}

/// A command that came in while another client's command ran on the same
/// session is told the name is shared; one after the other, no note.
#[test]
fn a_second_caller_on_one_name_is_told() {
    if !chrome() {
        return;
    }
    let dir = Dir::new();
    dir.ok(&["-s", "one", "start"]);
    let first = dir.ok(&["-s", "one", "eval", "1"]);
    assert!(first.get("notes").is_none(), "{first}");
    let slow = dir.command().args(["-s", "one", "wait", "--ms", "1500"]).stdout(Stdio::piped()).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(400));
    let second = dir.ok(&["-s", "one", "eval", "2"]);
    let notes = second["notes"].as_array().cloned().unwrap_or_default();
    assert!(notes.iter().any(|n| n.as_str().unwrap_or("").contains("own -s name")), "{second}");
    let out = slow.wait_with_output().unwrap();
    let slow: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(slow.get("notes").is_none(), "{slow}");
    let third = dir.ok(&["-s", "one", "eval", "3"]);
    assert!(third.get("notes").is_none(), "{third}");
}

/// Two processes on one session name, one after the other: the one that
/// comes in is told another used the session; the first one's next command
/// is told its tab was moved, and from where to where; then nothing more.
#[test]
fn a_caller_whose_tab_another_one_moved_is_told() {
    if !chrome() {
        return;
    }
    let dir = Dir::new();
    let as_caller = |who: &str, args: &[&str]| -> Value {
        let out = dir.command().env("NAVIGERA_CALLER", who).args(args).output().expect("run navigera");
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|_| panic!("{args:?}: {}", String::from_utf8_lossy(&out.stdout)))
    };
    let notes = |r: &Value| -> String { r["notes"].as_array().map(|n| format!("{n:?}")).unwrap_or_default() };
    dir.ok(&["-s", "two", "start"]);
    let a = as_caller("agent-a", &["-s", "two", "goto", "data:text/html,<title>one</title>"]);
    assert!(a.get("notes").is_none(), "{a}");
    let b = as_caller("agent-b", &["-s", "two", "goto", "data:text/html,<title>two</title>"]);
    assert!(notes(&b).contains("another process used this session"), "{b}");
    let a = as_caller("agent-a", &["-s", "two", "get", "title"]);
    assert_eq!(a["result"], "two");
    let said = notes(&a);
    assert!(
        said.contains("moved its tab") && said.contains("<title>one</title> -> data:text/html,<title>two</title>"),
        "{a}"
    );
    let a = as_caller("agent-a", &["-s", "two", "get", "title"]);
    assert!(a.get("notes").is_none(), "{a}");
}

/// `quit --close-browser` on a shared session: the browser stays while
/// another session uses it, and closes at once, not after its grace
/// period, with the last one.
#[test]
fn quit_close_browser_closes_a_shared_browser_nobody_else_uses() {
    if !chrome() {
        return;
    }
    let dir = Dir::new();
    for name in ["a", "b"] {
        // A grace period longer than the test: only the quit can close it.
        let out = dir.command().env("NAVIGERA_SHARED_GRACE_S", "60").args(["-s", name, "start"]).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    }
    let shared = || dir.sessions()["shared_browsers"].as_array().unwrap().len();
    assert_eq!(shared(), 1);
    let r = dir.ok(&["-s", "a", "quit", "--close-browser"]);
    assert_eq!(
        r["result"]["browser"], "left open: 1 other session uses it; it closes on its own once none does",
        "{r}"
    );
    assert_eq!(dir.names(), ["b"]);
    assert_eq!(shared(), 1, "b still uses the browser");
    let r = dir.ok(&["-s", "b", "quit", "--close-browser"]);
    assert_eq!(r["result"]["browser"], "closed: no other session used it", "{r}");
    eventually("the shared browser closes at once", Duration::from_secs(5), || shared() == 0);
}

/// Under a session directory whose path is too long for the shared
/// browser's socket address, a session starts with a browser of its own
/// instead of failing.
#[cfg(unix)]
#[test]
fn a_long_session_directory_still_starts() {
    if !chrome() {
        return;
    }
    let dir = Dir::new();
    // A directory whose `navigera/s.sock` fits a socket address (107 bytes
    // on Linux, 103 elsewhere) and whose `navigera/browser-<key>.sock`,
    // 23 bytes longer, does not.
    let base = if cfg!(target_os = "linux") { 80 } else { 76 };
    let root_len = dir.root.path().as_os_str().len();
    if root_len + 2 > base {
        return;
    }
    let deep = dir.root.path().join("d".repeat(base - root_len - 1));
    std::fs::create_dir_all(&deep).unwrap();
    let run = |args: &[&str]| {
        let out = dir.command().env("XDG_RUNTIME_DIR", &deep).args(args).output().unwrap();
        serde_json::from_slice::<Value>(&out.stdout).unwrap()
    };
    let r = run(&["-s", "s", "start"]);
    assert_eq!(r["ok"], true, "{r}");
    let list = run(&["sessions"]);
    assert_eq!(list["result"]["sessions"][0]["browser"], "own", "{list}");
    let log = std::fs::read_to_string(deep.join("navigera").join("s.log")).unwrap();
    assert!(log.contains("too long for a socket address"), "{log}");
    assert_eq!(run(&["-s", "s", "quit"])["ok"], true);
}
