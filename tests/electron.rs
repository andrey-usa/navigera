//! `start --electron`: navigera launches an Electron app and drives its
//! windows as tabs. The app (`tests/fixtures/electron-app`) saves a form
//! through its main process and opens a second window of its own.
//!
//! Runs when the fixture's `node_modules` holds Electron (`npm install` in
//! `tests/fixtures/electron-app`) and, on Linux, a display is set (CI runs
//! it under `xvfb-run`); with `NAVIGERA_REQUIRE_ELECTRON=1` (CI) a missing
//! Electron is a failure.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

fn tool_exe() -> String {
    env!("CARGO_BIN_EXE_navigera").to_string()
}

fn required() -> bool {
    std::env::var_os("NAVIGERA_REQUIRE_ELECTRON").is_some()
}

/// The fixture app's directory, when Electron is installed in it and can
/// open a window here.
fn fixture_app() -> Option<PathBuf> {
    let app = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("electron-app");
    let binary = if cfg!(target_os = "macos") {
        "Electron.app/Contents/MacOS/Electron"
    } else if cfg!(windows) {
        "electron.exe"
    } else {
        "electron"
    };
    let installed = app.join("node_modules").join("electron").join("dist").join(binary).is_file();
    assert!(
        installed || !required(),
        "NAVIGERA_REQUIRE_ELECTRON is set but {} has no Electron: run `npm install` there",
        app.display()
    );
    let display = !cfg!(target_os = "linux")
        || std::env::var_os("DISPLAY").is_some()
        || std::env::var_os("WAYLAND_DISPLAY").is_some();
    assert!(display || !required(), "NAVIGERA_REQUIRE_ELECTRON is set but there is no display: run under xvfb-run");
    (installed && display).then_some(app)
}

fn session_name(test: &str) -> String {
    std::env::temp_dir().join(format!("nv-electron-{test}-{}.sock", std::process::id())).display().to_string()
}

/// One CLI step against a session; returns (exit ok, JSON).
fn run(session: &str, args: &[&str]) -> (bool, Value) {
    let out = Command::new(tool_exe())
        .arg("--session")
        .arg(session)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("run navigera");
    let text = String::from_utf8_lossy(&out.stdout);
    let value = serde_json::from_str(text.trim()).unwrap_or_else(|_| {
        panic!("stdout must be one JSON object for {args:?}: {text}\nstderr: {}", String::from_utf8_lossy(&out.stderr))
    });
    (out.status.success(), value)
}

fn ok(session: &str, args: &[&str]) -> Value {
    let (success, r) = run(session, args);
    assert!(success && r["ok"] == true, "{args:?} failed: {r}");
    r["result"].clone()
}

fn ax(session: &str) -> String {
    ok(session, &["ax"]).as_str().expect("ax returns text").to_string()
}

fn eval(session: &str, js: &str) -> Value {
    ok(session, &["eval", js])
}

/// `eval --main`: in the app's main process.
fn eval_main(session: &str, js: &str) -> Value {
    ok(session, &["eval", "--main", js])
}

/// Poll `probe` until it gives a value, for up to `within`.
fn wait_for<T>(within: Duration, what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + within;
    loop {
        if let Some(value) = probe() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out after {within:?} waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn alive(pid: u32) -> bool {
    if cfg!(windows) {
        let out =
            Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]).output().expect("run tasklist");
        String::from_utf8_lossy(&out.stdout).split_whitespace().any(|word| word == pid.to_string())
    } else {
        Command::new("kill").args(["-0", &pid.to_string()]).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
    }
}

fn tab_count(session: &str) -> usize {
    ok(session, &["tab-list"])["tabs"].as_array().map_or(0, Vec::len)
}

/// Ends the session (and with it the app) if the test fails midway.
struct Quit(String);

impl Drop for Quit {
    fn drop(&mut self) {
        let _ = Command::new(tool_exe()).args(["--session", &self.0, "quit"]).stdin(Stdio::null()).output();
    }
}

#[test]
fn an_electron_app_is_driven_through_its_windows() {
    let Some(app) = fixture_app() else {
        eprintln!("skipping: no Electron in tests/fixtures/electron-app (run `npm install` there)");
        return;
    };
    let scratch = tempfile::tempdir().unwrap();
    let pid_file = scratch.path().join("app.pid");
    let session = session_name("app");
    let _quit = Quit(session.clone());
    // The app's directory: navigera runs the Electron in its node_modules,
    // and --electron-arg reaches the app's command line.
    ok(
        &session,
        &[
            "start",
            "--electron",
            app.to_str().unwrap(),
            &format!("--electron-arg=--pid-file={}", pid_file.display()),
            "--idle-timeout-s",
            "300",
        ],
    );
    let pid: u32 = wait_for(Duration::from_secs(10), "the app's pid file", || {
        std::fs::read_to_string(&pid_file).ok()?.trim().parse().ok()
    });
    assert!(alive(pid), "the app runs");

    // The app's first window is the session's tab, read like a page.
    let tree = ax(&session);
    assert!(tree.contains("heading \"Orders\""), "{tree}");
    assert!(tree.contains("textbox \"Customer\""), "{tree}");
    assert!(tree.contains("button \"Save order\""), "{tree}");
    assert!(tree.contains("Electron "), "the preload's bridge reached the page:\n{tree}");
    let href = eval(&session, "location.href");
    assert!(href.as_str().is_some_and(|u| u.starts_with("file://") && u.ends_with("index.html")), "{href}");
    let agent = eval(&session, "navigator.userAgent");
    assert!(agent.as_str().is_some_and(|ua| ua.contains("Electron/")), "{agent}");

    // The main process: a function gets the electron module, an expression
    // is evaluated, a promise is awaited.
    assert_eq!(eval_main(&session, "({ app }) => app.getName()"), "navigera-electron-fixture");
    let version = eval_main(&session, "process.versions.electron");
    assert!(version.as_str().is_some_and(|v| v.starts_with("45.")), "{version}");
    assert_eq!(eval_main(&session, "async ({ app }) => { await app.whenReady(); return app.isReady(); }"), true);
    assert_eq!(eval_main(&session, "({ BrowserWindow }) => BrowserWindow.getAllWindows().length"), 1);
    let (thrown, r) = run(&session, &["eval", "--main", "() => { throw new Error('nope') }"]);
    assert!(!thrown && r.to_string().contains("nope"), "{r}");

    // A save goes through the main process (IPC) and back.
    ok(&session, &["fill", "--label", "Customer", "Ada"]);
    ok(&session, &["fill", "--label", "Quantity", "3"]);
    ok(&session, &["click", "--role", "button", "--name", "Save order"]);
    let status = "document.getElementById('status').textContent";
    wait_for(Duration::from_secs(5), "the saved order", || {
        (eval(&session, status) == "Saved A-1003 for Ada").then_some(())
    });
    assert!(ax(&session).contains("Saved A-1003 for Ada"));
    ok(&session, &["fill", "--label", "Customer", ""]);
    ok(&session, &["click", "--role", "button", "--name", "Save order"]);
    wait_for(Duration::from_secs(5), "the main process's validation message", || {
        (eval(&session, status) == "Customer is required").then_some(())
    });

    // A window the app opens itself (no opener, unlike a popup) becomes a
    // tab, and the active one.
    ok(&session, &["click", "--role", "button", "--name", "Settings"]);
    wait_for(Duration::from_secs(10), "the Settings window as a second tab", || {
        (tab_count(&session) == 2).then_some(())
    });
    let tabs = ok(&session, &["tab-list"]);
    assert_eq!(tabs["active"], 1, "{tabs}");
    assert_eq!(eval_main(&session, "({ BrowserWindow }) => BrowserWindow.getAllWindows().length"), 2);
    wait_for(Duration::from_secs(5), "the Settings window's content", || {
        ax(&session).contains("heading \"Settings\"").then_some(())
    });
    ok(&session, &["click", "--label", "Compact rows"]);
    assert_eq!(eval(&session, "document.getElementById('compact').checked"), true);
    ok(&session, &["tab-select", "0"]);
    assert!(ax(&session).contains("heading \"Orders\""));
    ok(&session, &["tab-select", "1"]);

    // The window closes itself: the session goes on in the other one.
    ok(&session, &["click", "--role", "button", "--name", "Close"]);
    wait_for(Duration::from_secs(10), "the Settings window to go", || (tab_count(&session) == 1).then_some(()));
    assert!(ax(&session).contains("heading \"Orders\""));

    // An app is not a browser: it opens its own windows.
    let (success, r) = run(&session, &["tab-new"]);
    assert!(!success && r.to_string().contains("Electron app"), "{r}");

    // quit ends the app.
    ok(&session, &["quit"]);
    wait_for(Duration::from_secs(10), "the app to exit after quit", || (!alive(pid)).then_some(()));
}

#[test]
fn a_missing_app_is_named() {
    let session = session_name("missing");
    let _quit = Quit(session.clone());
    let missing = std::env::temp_dir().join(format!("nv-no-such-app-{}", std::process::id()));
    let (success, r) = run(&session, &["start", "--electron", missing.to_str().unwrap()]);
    assert!(!success, "{r}");
    let text = r.to_string();
    assert!(text.contains("nv-no-such-app") && text.contains("no such file"), "{r}");
}
