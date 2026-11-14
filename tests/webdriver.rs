//! The WebDriver engines end to end, one scenario on each: Firefox over
//! WebDriver BiDi (`--engine firefox`), Safari through safaridriver
//! (`--engine safari`, macOS), and a WebDriver server (`--webdriver`) over
//! classic WebDriver and over BiDi (`--bidi`), with chromedriver.
//!
//! Each runs when its browser is named (not found on PATH: the main CI
//! jobs' runners carry these browsers too, and run them only here, in the
//! `webdriver` workflow):
//! - Firefox: `$NAVIGERA_FIREFOX`; `NAVIGERA_REQUIRE_FIREFOX=1` makes a
//!   missing one a failure.
//! - Safari: macOS with `NAVIGERA_SAFARI=1` (safaridriver must be enabled
//!   once, `sudo safaridriver --enable`); `NAVIGERA_REQUIRE_SAFARI=1` too.
//! - chromedriver: `$CHROMEDRIVER` (and `$CHROME_BIN` for its browser);
//!   `NAVIGERA_REQUIRE_CHROMEDRIVER=1`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const INDEX: &str = r#"<!doctype html><html><head><title>Fixture</title></head><body>
<h1>Shop</h1>
<nav><a href="/next.html">Next page</a></nav>
<form onsubmit="event.preventDefault(); document.getElementById('out').textContent = 'sent ' + document.getElementById('name').value + ' ' + document.getElementById('size').value + ' ' + document.getElementById('agree').checked;">
<label for="name">Name</label> <input id="name" placeholder="Your name">
<label>Size <select id="size"><option value="s">Small</option><option value="l">Large</option></select></label>
<label><input type="checkbox" id="agree"> I agree</label>
<button type="submit">Send</button>
</form>
<div id="out" role="status"></div>
<div id="js" style="cursor:pointer">Clickable div</div>
<iframe id="inner" src="/frame.html" style="width:300px;height:80px"></iframe>
<script>document.getElementById('js').addEventListener('click', () => { console.log('div clicked'); document.getElementById('out').textContent = 'div'; });</script>
</body></html>"#;

const NEXT: &str = r#"<!doctype html><html><head><title>Next</title></head><body><p>Second page</p>
<button onclick="document.body.dataset.answer = String(confirm('sure?'))">Confirm</button></body></html>"#;

const FRAME: &str = r#"<!doctype html><html><head><title>Inner</title></head><body><button onclick="this.textContent = 'pressed'">In frame</button></body></html>"#;

/// A server for the three pages, on a thread (not Python's `http.server`:
/// on macOS runners it spends ~35 s per start resolving the host name).
fn serve() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            std::thread::spawn(move || {
                let mut s = s;
                let mut r = BufReader::new(s.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                    loop {
                        let mut h = String::new();
                        if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                            break;
                        }
                    }
                    let (status, body) = match path.as_str() {
                        "/" => ("200 OK", INDEX),
                        "/next.html" => ("200 OK", NEXT),
                        "/frame.html" => ("200 OK", FRAME),
                        _ => ("404 Not Found", "not found"),
                    };
                    let head = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\r\n",
                        body.len()
                    );
                    if s.write_all(head.as_bytes()).and_then(|_| s.write_all(body.as_bytes())).is_err() {
                        return;
                    }
                }
            });
        }
    });
    format!("http://127.0.0.1:{port}")
}

/// Run `script` (JSON lines) through `navigera <args> serve`; the answers.
fn session(args: &[&str], script: &[Value]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_navigera"))
        .args(args)
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("navigera starts");
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut answers = Vec::new();
    for cmd in script {
        writeln!(stdin, "{cmd}").unwrap();
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        if line.is_empty() {
            let mut err = String::new();
            child.stderr.take().unwrap().read_to_string(&mut err).ok();
            panic!("navigera ended before answering {cmd}: {err}");
        }
        answers.push(serde_json::from_str::<Value>(&line).unwrap_or_else(|e| panic!("{e}: {line}")));
    }
    drop(stdin);
    let started = Instant::now();
    while child.try_wait().unwrap().is_none() {
        assert!(started.elapsed() < Duration::from_secs(30), "navigera did not exit after quit");
        std::thread::sleep(Duration::from_millis(50));
    }
    answers
}

fn ok(answers: &[Value], id: u64) -> &Value {
    let a = answers.iter().find(|a| a["id"] == id).unwrap_or_else(|| panic!("no answer {id}"));
    assert_eq!(a["ok"], true, "step {id} failed: {a}");
    &a["result"]
}

/// The same scenario on every engine: read, act by role, label, text and
/// ref, a frame, console, a dialog, navigation and history.
fn scenario(args: &[&str], engine: &str) {
    let site = serve();
    let script: Vec<Value> = vec![
        json!({ "id": 1, "op": "goto", "url": format!("{site}/") }),
        json!({ "id": 2, "op": "ax", "format": "text" }),
        json!({ "id": 3, "op": "fill", "label": "Name", "value": "Ada" }),
        json!({ "id": 4, "op": "select", "selector": "#size", "value": "Large" }),
        json!({ "id": 5, "op": "set-checked", "selector": "#agree", "checked": true }),
        json!({ "id": 6, "op": "click", "role": "button", "name": "Send" }),
        json!({ "id": 7, "op": "text", "selector": "#out" }),
        json!({ "id": 8, "op": "click", "text": "Clickable div" }),
        json!({ "id": 9, "op": "console" }),
        json!({ "id": 10, "op": "eval", "expression": "({ a: 1, b: [document.title, 2] })" }),
        json!({ "id": 11, "op": "click", "role": "button", "name": "In frame" }),
        json!({ "id": 12, "op": "eval", "expression": "document.getElementById('inner').contentDocument.body.innerText" }),
        json!({ "id": 13, "op": "ax", "format": "text" }),
        json!({ "id": 14, "op": "click", "text": "Next page" }),
        json!({ "id": 15, "op": "dialog", "accept": false }),
        json!({ "id": 16, "op": "click", "role": "button", "name": "Confirm" }),
        json!({ "id": 17, "op": "eval", "expression": "document.body.dataset.answer" }),
        json!({ "id": 18, "op": "back" }),
        json!({ "id": 19, "op": "status" }),
        json!({ "id": 20, "op": "quit" }),
    ];
    let a = session(args, &script);
    assert_eq!(ok(&a, 1)["title"], "Fixture");
    let tree = ok(&a, 2).as_str().unwrap().to_string();
    for want in [
        "heading \"Shop\"",
        "link \"Next page\"",
        "textbox \"Name\"",
        "combobox \"Size\"",
        "checkbox \"I agree\"",
        "button \"Send\"",
    ] {
        assert!(tree.contains(want), "{engine}: no {want} in\n{tree}");
    }
    assert_eq!(ok(&a, 7), "sent Ada l true", "{engine}");
    let console = ok(&a, 9).to_string();
    assert!(console.contains("div clicked"), "{engine}: console {console}");
    assert_eq!(ok(&a, 10), &json!({ "a": 1, "b": ["Fixture", 2] }), "{engine}");
    assert_eq!(ok(&a, 12), "pressed", "{engine}: the click in the frame");
    let tree = ok(&a, 13).as_str().unwrap();
    assert!(tree.contains("button \"pressed\""), "{engine}: the frame's tree\n{tree}");
    assert_eq!(ok(&a, 14)["title"], "Next", "{engine}");
    let confirm = a.iter().find(|x| x["id"] == 16).unwrap();
    assert_eq!(confirm["ok"], true, "{engine}: {confirm}");
    let dialogs = confirm["dialogs"].to_string();
    assert!(dialogs.contains("sure?") && dialogs.contains("false"), "{engine}: dialogs {confirm}");
    assert_eq!(ok(&a, 17), "false", "{engine}: confirm answered by the dismiss policy");
    assert_eq!(ok(&a, 18)["title"], "Fixture", "{engine}");
    assert_eq!(ok(&a, 19)["engine"], engine);
    assert_eq!(ok(&a, 20)["bye"], true);
}

fn found(env: &str) -> Option<String> {
    std::env::var_os(env).map(std::path::PathBuf::from).filter(|p| p.is_file()).map(|p| p.display().to_string())
}

fn required(var: &str) -> bool {
    std::env::var(var).is_ok_and(|v| v == "1")
}

#[test]
fn firefox_over_bidi_reads_acts_and_navigates() {
    let Some(firefox) = found("NAVIGERA_FIREFOX") else {
        assert!(!required("NAVIGERA_REQUIRE_FIREFOX"), "NAVIGERA_REQUIRE_FIREFOX is set but no Firefox was found");
        eprintln!("skipped: no Firefox");
        return;
    };
    scenario(&["--engine", "firefox", "--executable", &firefox], "firefox");
}

#[test]
fn safari_over_safaridriver_reads_acts_and_navigates() {
    if !cfg!(target_os = "macos") || !required("NAVIGERA_SAFARI") {
        assert!(!required("NAVIGERA_REQUIRE_SAFARI"), "NAVIGERA_REQUIRE_SAFARI is set but Safari is not enabled here");
        eprintln!("skipped: Safari runs on macOS with NAVIGERA_SAFARI=1");
        return;
    }
    scenario(&["--engine", "safari"], "safari");
}

/// chromedriver on a free port, for the `--webdriver` scenarios.
fn chromedriver() -> Option<(std::process::Child, String, Value)> {
    let Some(exe) = found("CHROMEDRIVER") else {
        assert!(
            !required("NAVIGERA_REQUIRE_CHROMEDRIVER"),
            "NAVIGERA_REQUIRE_CHROMEDRIVER is set but no chromedriver was found"
        );
        return None;
    };
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut cmd = Command::new(exe);
    cmd.arg(format!("--port={port}")).arg("--disable-build-check").stdout(Stdio::null()).stderr(Stdio::null());
    // Its own process group: a wrapper script's chromedriver goes with it.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let child = cmd.spawn().expect("chromedriver starts");
    let started = Instant::now();
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(started.elapsed() < Duration::from_secs(20), "chromedriver did not listen");
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut opts = json!({ "args": ["--headless=new", "--no-sandbox", "--disable-gpu"] });
    if let Ok(bin) = std::env::var("CHROME_BIN") {
        opts["binary"] = json!(bin);
    }
    Some((child, format!("http://127.0.0.1:{port}"), json!({ "browserName": "chrome", "goog:chromeOptions": opts })))
}

#[test]
fn a_webdriver_server_is_driven_over_classic_webdriver_and_over_bidi() {
    let Some((mut driver, url, caps)) = chromedriver() else {
        eprintln!("skipped: no chromedriver");
        return;
    };
    let caps = caps.to_string();
    scenario(&["--webdriver", &url, "--capabilities", &caps], "webdriver");
    scenario(&["--webdriver", &url, "--bidi", "--capabilities", &caps], "webdriver");
    #[cfg(unix)]
    {
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        // SAFETY: kill(2) on the process group the driver was started in.
        unsafe {
            kill(-(driver.id() as i32), 9);
        }
    }
    let _ = driver.kill();
    let _ = driver.wait();
}
