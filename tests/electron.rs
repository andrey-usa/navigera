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
    // and --electron-arg reaches the app's command line. The app first opens
    // a hidden helper window, and mounts its form a second after load.
    let (started, r) = run(
        &session,
        &[
            "start",
            "--electron",
            app.to_str().unwrap(),
            &format!("--electron-arg=--pid-file={}", pid_file.display()),
            "--electron-arg=--helper-window",
            "--electron-arg=--late-ui",
            "--idle-timeout-s",
            "300",
        ],
    );
    assert!(started && r["ok"] == true, "{r}");
    // `start` says which Electron, and that its main process answers.
    let electron = &r["result"]["electron"];
    assert!(electron["version"].as_str().is_some_and(|v| v.starts_with("45.")), "{r}");
    assert_eq!(electron["main_process"], true, "{r}");
    let pid: u32 = wait_for(Duration::from_secs(10), "the app's pid file", || {
        std::fs::read_to_string(&pid_file).ok()?.trim().parse().ok()
    });
    assert!(alive(pid), "the app runs");

    // The session starts on the window the app shows (not the hidden helper),
    // once it shows something: the late form is there without a wait.
    let tree = ax(&session);
    assert!(tree.contains("heading \"Orders\""), "{tree}");
    let tabs = ok(&session, &["tab-list"]);
    assert_eq!(tabs["tabs"].as_array().map(Vec::len), Some(2), "the helper is a tab too: {tabs}");
    assert!(tabs["url"].as_str().is_some_and(|u| u.contains("index.html")), "{tabs}");
    let active = tabs["active"].as_u64().unwrap() as usize;
    let orders_tab = tabs["tabs"][active]["target"].as_str().unwrap().to_string();
    assert!(tree.contains("textbox \"Customer\""), "{tree}");
    assert!(tree.contains("button \"Save order\""), "{tree}");
    assert!(tree.contains("Electron "), "the preload's bridge reached the page:\n{tree}");
    let href = eval(&session, "location.href");
    assert!(href.as_str().is_some_and(|u| u.starts_with("file://") && u.contains("index.html")), "{href}");
    let agent = eval(&session, "navigator.userAgent");
    assert!(agent.as_str().is_some_and(|ua| ua.contains("Electron/")), "{agent}");

    // The main process: a function gets the electron module, an expression
    // is evaluated, a promise is awaited.
    assert_eq!(eval_main(&session, "({ app }) => app.getName()"), "navigera-electron-fixture");
    let version = eval_main(&session, "process.versions.electron");
    assert!(version.as_str().is_some_and(|v| v.starts_with("45.")), "{version}");
    assert_eq!(eval_main(&session, "async ({ app }) => { await app.whenReady(); return app.isReady(); }"), true);
    assert_eq!(eval_main(&session, "({ BrowserWindow }) => BrowserWindow.getAllWindows().length"), 2);
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
    wait_for(Duration::from_secs(10), "the Settings window as a third tab", || {
        (tab_count(&session) == 3).then_some(())
    });
    let tabs = ok(&session, &["tab-list"]);
    assert!(tabs["url"].as_str().is_some_and(|u| u.ends_with("settings.html")), "the new window is active: {tabs}");
    let settings_tab = tabs["tabs"][tabs["active"].as_u64().unwrap() as usize]["target"].as_str().unwrap().to_string();
    assert_eq!(eval_main(&session, "({ BrowserWindow }) => BrowserWindow.getAllWindows().length"), 3);
    wait_for(Duration::from_secs(5), "the Settings window's content", || {
        ax(&session).contains("heading \"Settings\"").then_some(())
    });
    ok(&session, &["click", "--label", "Compact rows"]);
    assert_eq!(eval(&session, "document.getElementById('compact').checked"), true);
    ok(&session, &["tab-select", &orders_tab]);
    assert!(ax(&session).contains("heading \"Orders\""));
    ok(&session, &["tab-select", &settings_tab]);

    // The window closes itself: the session goes on in the window the app
    // shows, not in the hidden helper.
    ok(&session, &["click", "--role", "button", "--name", "Close"]);
    wait_for(Duration::from_secs(10), "the Settings window to go", || (tab_count(&session) == 2).then_some(()));
    assert!(ax(&session).contains("heading \"Orders\""));
    let tabs = ok(&session, &["tab-list"]);
    assert!(tabs["url"].as_str().is_some_and(|u| u.contains("index.html")), "{tabs}");

    // An app is not a browser: it opens its own windows.
    let (success, r) = run(&session, &["tab-new"]);
    assert!(!success && r.to_string().contains("Electron app"), "{r}");

    // quit ends the app.
    ok(&session, &["quit"]);
    wait_for(Duration::from_secs(10), "the app to exit after quit", || (!alive(pid)).then_some(()));
}

/// An IDE agent's way in: `navigera mcp --electron <app>`. The first tool
/// call starts the app; snapshot, fill, click and the main process work as
/// in a session; closing the client ends the app.
#[test]
fn an_mcp_client_drives_an_electron_app() {
    use std::io::{BufRead, BufReader, Write};
    let Some(app) = fixture_app() else {
        eprintln!("skipping: no Electron in tests/fixtures/electron-app (run `npm install` there)");
        return;
    };
    let scratch = tempfile::tempdir().unwrap();
    let pid_file = scratch.path().join("app.pid");
    let mut child = Command::new(tool_exe())
        .args(["mcp", "--electron", app.to_str().unwrap(), "--electron-arg=--late-ui"])
        .arg(format!("--electron-arg=--pid-file={}", pid_file.display()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn navigera mcp");
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut id = 0;
    let mut call = |method: &str, params: Value| -> Value {
        id += 1;
        writeln!(stdin, "{}", serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        serde_json::from_str(line.trim()).unwrap_or_else(|e| panic!("not JSON ({e}): {line}"))
    };
    let text = |reply: &Value| -> String {
        reply["result"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
    };
    call(
        "initialize",
        serde_json::json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t"}}),
    );
    let tool = |name: &str, args: Value| serde_json::json!({"name": name, "arguments": args});

    let snapshot = call("tools/call", tool("browser_snapshot", serde_json::json!({})));
    assert!(text(&snapshot).contains("heading \"Orders\""), "{snapshot}");
    let name = call(
        "tools/call",
        tool("browser_evaluate", serde_json::json!({"expression": "({ app }) => app.getName()", "main": true})),
    );
    assert!(text(&name).contains("navigera-electron-fixture"), "{name}");
    call("tools/call", tool("browser_fill", serde_json::json!({"selector": "#customer", "value": "Grace"})));
    call("tools/call", tool("browser_click", serde_json::json!({"text": "Save order"})));
    let saved = wait_for(Duration::from_secs(5), "the saved order", || {
        let r = call(
            "tools/call",
            tool(
                "browser_evaluate",
                serde_json::json!({"expression": "document.getElementById('status').textContent"}),
            ),
        );
        text(&r).contains("Saved A-1001 for Grace").then_some(r)
    });
    assert!(saved["result"]["isError"] != true, "{saved}");
    let pid: u32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
    assert!(alive(pid));

    // The client goes away: so do the server and the app.
    drop(stdin);
    assert!(child.wait().unwrap().success());
    wait_for(Duration::from_secs(10), "the app to exit with the MCP server", || (!alive(pid)).then_some(()));
}

#[test]
fn a_single_instance_app_already_running_is_named() {
    let Some(app) = fixture_app() else {
        eprintln!("skipping: no Electron in tests/fixtures/electron-app (run `npm install` there)");
        return;
    };
    let app = app.to_str().unwrap();
    let first = session_name("single-first");
    let _quit_first = Quit(first.clone());
    // An app with its own user agent: the version comes from its main process.
    let (started, r) = run(
        &first,
        &[
            "start",
            "--electron",
            app,
            "--electron-arg=--single-instance",
            "--electron-arg=--custom-user-agent=OrdersApp/2.0",
        ],
    );
    assert!(started && r["ok"] == true, "{r}");
    assert_eq!(eval(&first, "navigator.userAgent"), "OrdersApp/2.0");
    let version = r["result"]["electron"]["version"].as_str().unwrap_or_default();
    assert!(version.starts_with("45."), "{r}");
    // A second copy hands over to the first and exits: `start` says why.
    let second = session_name("single-second");
    let _quit_second = Quit(second.clone());
    let (started, r) = run(&second, &["start", "--electron", app, "--electron-arg=--single-instance"]);
    assert!(!started, "{r}");
    let text = r.to_string();
    // Where it quits (before its DevTools port, after it, while navigera
    // connects) depends on the machine; the error says it exited, and why.
    assert!(text.contains("exited") && text.contains("single-instance"), "{r}");
    // The first copy is unharmed.
    assert!(ax(&first).contains("Orders"));
    ok(&first, &["quit"]);
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
