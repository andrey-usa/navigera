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

/// Clicks that load a new document, and clicks that look as if they would.
const NAV: &str = r##"<!doctype html><html><head><title>Nav</title></head><body>
<form action="/next.html"><button id="submit">Go</button></form>
<form action="/next.html"><input required name="q"><button id="invalid">Needs a value</button></form>
<form id="scripted" action="/next.html"></form>
<button type="button" id="by-form" onclick="document.getElementById('scripted').submit()">Submits by script</button>
<div id="by-script" onclick="location.href = '/next.html'">Leaves by script</div>
<a id="routed" href="/next.html" onclick="event.preventDefault(); document.title = 'Routed'">Router link</a>
<a id="hash" href="#here">Same page</a>
<button type="button" id="plain" onclick="document.title = 'Pressed'">Plain</button>
<a id="blank" href="/next.html" target="_blank">Other tab</a>
<button type="button" id="opens" onclick="window.open('/next.html')">Opens a tab</button>
</body></html>"##;

const FRAME: &str = r#"<!doctype html><html><head><title>Inner</title></head><body><button onclick="this.textContent = 'pressed'">In frame</button></body></html>"#;

/// A file the server sends as a download.
const CSV: &str = "a,b\n1,2\n";

const MORE: &str = r#"<!doctype html><html><head><title>More</title></head><body style="margin:0;user-select:none;-webkit-user-select:none">
<input type="file" id="one"> <input type="file" id="many" multiple>
<div id="card" draggable="true" style="width:80px;height:30px;background:#cde">Card</div>
<div id="zone" style="width:120px;height:40px;background:#eee">Drop here</div>
<div id="pad" style="width:300px;height:100px;background:#ddd"></div>
<div id="knob" style="width:30px;height:30px;background:#888"></div>
<script>
window.seen = [];
const card = document.getElementById('card'), zone = document.getElementById('zone'), pad = document.getElementById('pad'), knob = document.getElementById('knob');
card.addEventListener('dragstart', (e) => e.dataTransfer.setData('text/plain', 'card'));
zone.addEventListener('dragover', (e) => e.preventDefault());
zone.addEventListener('drop', (e) => { e.preventDefault(); zone.textContent = 'got ' + e.dataTransfer.getData('text/plain'); });
for (const type of ['mousedown', 'mouseup', 'click']) pad.addEventListener(type, (e) => window.seen.push(type + (e.shiftKey ? '+shift' : '') + (type === 'click' ? '' : '@' + e.offsetX + ',' + e.offsetY)));
pad.addEventListener('mousemove', (e) => { if (e.buttons === 1) window.heldMoves = (window.heldMoves || 0) + 1; });
knob.addEventListener('mousedown', () => { window.knob = 'down'; });
document.addEventListener('mouseup', (e) => { if (window.knob === 'down') window.knob = 'dropped on ' + e.target.id; });
document.addEventListener('keydown', (e) => window.seen.push('down ' + e.key));
document.addEventListener('keyup', (e) => window.seen.push('up ' + e.key));
</script></body></html>"#;

/// A server for the pages, on a thread (not Python's `http.server`:
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
                    // A form's GET adds a query.
                    let (status, body) = match path.split('?').next().unwrap_or("/") {
                        "/" => ("200 OK", INDEX),
                        "/next.html" => ("200 OK", NEXT),
                        "/frame.html" => ("200 OK", FRAME),
                        "/more.html" => ("200 OK", MORE),
                        "/nav.html" => ("200 OK", NAV),
                        "/report.csv" => ("200 OK", CSV),
                        _ => ("404 Not Found", "not found"),
                    };
                    let kind = if body == CSV {
                        "text/csv\r\nContent-Disposition: attachment; filename=\"report.csv\""
                    } else {
                        "text/html; charset=utf-8"
                    };
                    let head = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\r\n",
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
    session_in(args, &[], script)
}

/// [`session`] with environment variables set for navigera.
fn session_in(args: &[&str], env: &[(&str, &std::path::Path)], script: &[Value]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_navigera"))
        .envs(env.iter().copied())
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
    // Printing is the browser's own: Firefox and Chrome have it, Safari's
    // driver does not.
    let pdf = std::env::temp_dir().join(format!("nv-wd-{}-{}.pdf", std::process::id(), args.len()));
    let mut script = script;
    // `eval`'s forms, as on Chrome: an arrow function is called; statements
    // with `await` and `return` run as a function's body.
    script.insert(script.len() - 1, json!({ "id": 22, "op": "eval", "expression": "() => 6 * 7" }));
    script.insert(
        script.len() - 1,
        json!({ "id": 23, "op": "eval", "expression": "const n = await Promise.resolve(6); return n * 7" }),
    );
    // A SyntaxError the code throws as it runs is its answer, and the code
    // runs once: it is not taken for source that did not parse.
    script.insert(
        script.len() - 1,
        json!({ "id": 24, "op": "eval", "expression": "() => document.querySelector('a[')" }),
    );
    script.insert(
        script.len() - 1,
        json!({ "id": 25, "op": "eval", "expression": "window.__runs = (window.__runs || 0) + 1; JSON.parse('{bad')" }),
    );
    script.insert(script.len() - 1, json!({ "id": 26, "op": "eval", "expression": "window.__runs" }));
    // `ax --grep`, and a wait for what never comes ends at the element
    // wait (5 s), as on Chrome, not the navigation timeout.
    script.insert(script.len() - 1, json!({ "id": 27, "op": "grep", "query": "Send" }));
    script.insert(script.len() - 1, json!({ "id": 28, "op": "wait", "text": "Not on this page" }));
    script.insert(script.len() - 1, json!({ "id": 29, "op": "scroll", "to": "top" }));
    if engine != "safari" {
        script.insert(script.len() - 1, json!({ "id": 21, "op": "pdf", "path": pdf.display().to_string() }));
    }
    let a = session(args, &script);
    if engine != "safari" {
        let bytes = std::fs::read(&pdf)
            .unwrap_or_else(|e| panic!("{engine}: no PDF ({e}): {:?}", a.iter().find(|x| x["id"] == 21)));
        assert!(bytes.starts_with(b"%PDF-") && bytes.len() > 500, "{engine}: {} bytes", bytes.len());
        assert_eq!(ok(&a, 21)["bytes"], bytes.len(), "{engine}");
        let _ = std::fs::remove_file(&pdf);
    }
    assert_eq!(ok(&a, 1)["title"], "Fixture");
    assert_eq!(ok(&a, 22), 42, "{engine}: an arrow function is called");
    assert_eq!(ok(&a, 23), 42, "{engine}: await and return at the top");
    for id in [24, 25] {
        let step = a.iter().find(|x| x["id"] == id).unwrap();
        assert_eq!(step["ok"], false, "{engine}: step {id} throws: {step}");
    }
    assert_eq!(ok(&a, 26), 1, "{engine}: the statements ran once");
    let grep = ok(&a, 27).as_str().unwrap_or_default().to_string();
    assert!(grep.contains("line with \"Send\"") && grep.contains("button \"Send\" [ref="), "{engine}: {grep}");
    let waited = a.iter().find(|x| x["id"] == 28).unwrap();
    assert!(waited["error"].as_str().unwrap_or("").contains("within 5000 ms"), "{engine}: {waited}");
    let at = ok(&a, 29);
    assert!(at["scrollY"] == 0 && at["scrollHeight"].is_number() && at["viewport"].is_number(), "{engine}: {at}");
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

/// A click answers with the page it leads to, and with the page it is on
/// when it leads nowhere: a submit, a script that submits a form or sets
/// `location`; a form that fails validation, a link a router handles, a
/// link into the same page, a plain button.
fn nav_clicks(args: &[&str], engine: &str) {
    let site = serve();
    let cases = [
        ("#submit", "Next"),
        ("#by-form", "Next"),
        ("#by-script", "Next"),
        ("#invalid", "Nav"),
        ("#routed", "Routed"),
        ("#hash", "Nav"),
        ("#plain", "Pressed"),
    ];
    let mut script = Vec::new();
    for (i, (selector, _)) in cases.iter().enumerate() {
        let id = i as u64 * 2;
        script.push(json!({ "id": id + 1, "op": "goto", "url": format!("{site}/nav.html") }));
        script.push(json!({ "id": id + 2, "op": "click", "selector": selector }));
    }
    script.push(json!({ "id": 100, "op": "quit" }));
    let a = session(args, &script);
    for (i, (selector, title)) in cases.iter().enumerate() {
        let id = i as u64 * 2;
        assert_eq!(ok(&a, id + 1)["title"], "Nav", "{engine}");
        assert_eq!(ok(&a, id + 2)["title"], *title, "{engine}: the click on {selector}: {}", ok(&a, id + 2));
    }
    // A click that opens a tab says so.
    let script: Vec<Value> = vec![
        json!({ "id": 1, "op": "goto", "url": format!("{site}/nav.html") }),
        json!({ "id": 2, "op": "click", "selector": "#blank" }),
        json!({ "id": 3, "op": "click", "selector": "#opens" }),
        json!({ "id": 4, "op": "click", "selector": "#plain" }),
        json!({ "id": 5, "op": "quit" }),
    ];
    let a = session(args, &script);
    // Enter in a form's field submits it; a character typed there does not.
    let script: Vec<Value> = vec![
        json!({ "id": 1, "op": "goto", "url": format!("{site}/nav.html") }),
        json!({ "id": 2, "op": "type", "selector": "input[name=q]", "value": "ab" }),
        json!({ "id": 3, "op": "press", "selector": "input[name=q]", "key": "c" }),
        json!({ "id": 4, "op": "press", "selector": "input[name=q]", "key": "Enter" }),
        json!({ "id": 5, "op": "quit" }),
    ];
    let keys = session(args, &script);
    assert_eq!(ok(&keys, 2)["title"], "Nav", "{engine}");
    assert_eq!(ok(&keys, 3)["title"], "Nav", "{engine}");
    assert_eq!(ok(&keys, 4)["title"], "Next", "{engine}: Enter in a form's field: {}", ok(&keys, 4));
    assert!(ok(&keys, 4)["url"].as_str().unwrap_or("").ends_with("/next.html?q=abc"), "{engine}: {}", ok(&keys, 4));
    assert_eq!(ok(&a, 2)["tabs"], 2, "{engine}: a link with target=_blank: {}", ok(&a, 2));
    assert_eq!(ok(&a, 3)["tabs"], 3, "{engine}: window.open in a click handler: {}", ok(&a, 3));
    // The session's tab is still the one in front: behind the new one a
    // driver waited 5 s for a frame at every press.
    assert_eq!(ok(&a, 4)["title"], "Pressed", "{engine}");
    let took = |id: u64| a.iter().find(|x| x["id"] == id).and_then(|x| x["elapsed_ms"].as_f64()).unwrap_or(f64::MAX);
    assert!(
        took(3) < 3000.0 && took(4) < 3000.0,
        "{engine}: a click behind an opened tab took {} and {} ms",
        took(3),
        took(4)
    );
}

/// The ops past the core set, on every engine: files for a file input, a
/// drag of a `draggable` and of a plain element, the raw mouse and held
/// keys, storage, the viewport, and a script that runs before the page's.
/// `bidi`: the engine speaks WebDriver BiDi (it has preload scripts).
fn more_ops(args: &[&str], engine: &str, bidi: bool) {
    let site = serve();
    let dir = std::env::temp_dir().join(format!("nv-wd-up-{}-{}-{bidi}", std::process::id(), args.len()));
    std::fs::create_dir_all(&dir).unwrap();
    let (a_txt, b_txt) = (dir.join("a.txt"), dir.join("b.txt"));
    std::fs::write(&a_txt, "alpha").unwrap();
    std::fs::write(&b_txt, "be").unwrap();
    let (fa, fb) = (a_txt.display().to_string(), b_txt.display().to_string());
    // A request the page makes, its outcome kept for the next step.
    let ask = |path: &str| {
        format!("window.got = undefined; fetch('{path}').then((r) => r.text().then((t) => {{ window.got = r.status + ' ' + r.headers.get('content-type') + ' ' + t; }}), () => {{ window.got = 'failed'; }}); 0")
    };
    let files = "[...document.querySelectorAll('input[type=file]')].map((i) => [...i.files].map((f) => f.name + ':' + f.size).join(','))";
    let script: Vec<Value> = vec![
        json!({ "id": 1, "op": "goto", "url": format!("{site}/more.html") }),
        json!({ "id": 2, "op": "upload", "selector": "#one", "files": [fa] }),
        json!({ "id": 3, "op": "upload", "selector": "#many", "files": [fa, fb] }),
        json!({ "id": 4, "op": "eval", "expression": files }),
        json!({ "id": 5, "op": "upload", "selector": "#one", "files": [fa, fb] }),
        json!({ "id": 6, "op": "upload", "selector": "#pad", "files": [fa] }),
        json!({ "id": 7, "op": "drag", "from": { "selector": "#card" }, "to": { "selector": "#zone" } }),
        json!({ "id": 8, "op": "text", "selector": "#zone" }),
        json!({ "id": 9, "op": "drag", "from": { "selector": "#knob" }, "to": { "selector": "#pad" } }),
        json!({ "id": 10, "op": "eval", "expression": "[window.knob, window.heldMoves > 0]" }),
        json!({ "id": 11, "op": "eval", "expression": "window.seen.length = 0" }),
        json!({ "id": 12, "op": "keydown", "key": "Shift" }),
        json!({ "id": 13, "op": "mouse", "action": "click", "x": 10, "y": 20, "selector": "#pad" }),
        json!({ "id": 14, "op": "keyup", "key": "Shift" }),
        json!({ "id": 15, "op": "mouse", "action": "down", "selector": "#pad" }),
        json!({ "id": 16, "op": "mouse", "action": "move", "x": 200, "y": 60, "selector": "#pad", "steps": 4 }),
        json!({ "id": 17, "op": "mouse", "action": "up" }),
        json!({ "id": 18, "op": "eval", "expression": "window.seen" }),
        json!({ "id": 19, "op": "storage", "action": "set", "key": "k", "value": "v 1" }),
        json!({ "id": 20, "op": "storage", "key": "k" }),
        json!({ "id": 21, "op": "storage" }),
        json!({ "id": 22, "op": "storage", "action": "clear" }),
        json!({ "id": 23, "op": "storage" }),
        json!({ "id": 24, "op": "set", "what": "viewport", "values": ["800", "600"] }),
        json!({ "id": 25, "op": "eval", "expression": "[window.innerWidth, window.innerHeight]" }),
        json!({ "id": 26, "op": "init-script", "source": "window.early = 'before the page';" }),
        json!({ "id": 27, "op": "reload" }),
        json!({ "id": 28, "op": "eval", "expression": "String(window.early)" }),
        json!({ "id": 29, "op": "init-script", "clear": true }),
        json!({ "id": 31, "op": "route", "pattern": "*/api/data*", "body": "{\"n\":7}" }),
        json!({ "id": 32, "op": "route", "pattern": "*/api/gone", "abort": true }),
        json!({ "id": 33, "op": "eval", "expression": ask("/api/data?x=1") }),
        json!({ "id": 34, "op": "wait", "js": "window.got !== undefined" }),
        json!({ "id": 35, "op": "eval", "expression": "window.got" }),
        json!({ "id": 36, "op": "eval", "expression": ask("/api/gone") }),
        json!({ "id": 37, "op": "wait", "js": "window.got !== undefined" }),
        json!({ "id": 38, "op": "eval", "expression": "window.got" }),
        json!({ "id": 39, "op": "route" }),
        json!({ "id": 40, "op": "unroute" }),
        json!({ "id": 41, "op": "eval", "expression": ask("/api/data?x=1") }),
        json!({ "id": 42, "op": "wait", "js": "window.got !== undefined" }),
        json!({ "id": 43, "op": "eval", "expression": "window.got" }),
        json!({ "id": 44, "op": "set", "what": "locale", "values": ["de-DE"] }),
        json!({ "id": 30, "op": "quit" }),
    ];
    let a = session(args, &script);
    let _ = std::fs::remove_dir_all(&dir);
    let failed = |id: u64| {
        let x = a.iter().find(|x| x["id"] == id).unwrap();
        assert_eq!(x["ok"], false, "{engine}: step {id} should fail: {x}");
        x["error"].to_string()
    };
    assert_eq!(ok(&a, 2)["files"], 1, "{engine}");
    assert_eq!(ok(&a, 3)["files"], 2, "{engine}");
    assert_eq!(ok(&a, 4), &json!(["a.txt:5", "a.txt:5,b.txt:2"]), "{engine}: the inputs' files");
    assert!(failed(5).contains("takes one file"), "{engine}: {}", failed(5));
    assert!(failed(6).contains("not an <input type=file>"), "{engine}: {}", failed(6));
    assert_eq!(ok(&a, 7)["drag"]["dropped"], true, "{engine}: {}", ok(&a, 7));
    assert_eq!(ok(&a, 8), "got card", "{engine}: the drop's data");
    assert_eq!(ok(&a, 9)["drag"]["path"], "mouse", "{engine}: {}", ok(&a, 9));
    assert_eq!(ok(&a, 10), &json!(["dropped on pad", true]), "{engine}: a held mouse from the knob to the pad");
    assert_eq!(ok(&a, 12)["down"], "Shift", "{engine}");
    assert_eq!(ok(&a, 15)["mouse"]["held"], json!(["left"]), "{engine}");
    assert_eq!(ok(&a, 17)["mouse"]["held"], json!([]), "{engine}");
    let seen: Vec<String> = ok(&a, 18).as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string()).collect();
    let want = [
        "down Shift",
        "mousedown+shift@10,20",
        "mouseup+shift@10,20",
        "click+shift",
        "up Shift",
        "mousedown@150,50",
        "mouseup@200,60",
        "click",
    ];
    // (A click's own offset is not compared: Safari gives it a pixel less
    // than the press and release it follows.)
    assert_eq!(seen, want, "{engine}: keys held over a click, then a press, a move and a release");
    assert_eq!(ok(&a, 19), true, "{engine}");
    assert_eq!(ok(&a, 20), "v 1", "{engine}");
    assert_eq!(ok(&a, 21), &json!({ "k": "v 1" }), "{engine}");
    assert_eq!(ok(&a, 23), &json!({}), "{engine}");
    assert_eq!(ok(&a, 24)["viewport"], json!([800, 600]), "{engine}: {}", ok(&a, 24));
    assert_eq!(ok(&a, 25), &json!([800, 600]), "{engine}");
    if bidi {
        assert_eq!(ok(&a, 26)["init_scripts"], 1, "{engine}");
        assert_eq!(ok(&a, 28), "before the page", "{engine}: the script ran in the reloaded page");
        assert_eq!(ok(&a, 29)["removed"], 1, "{engine}");
        // Requests answered by a rule, refused by one, and let through
        // again once the rules are gone.
        assert_eq!(ok(&a, 31)["action"], "fulfill 200", "{engine}");
        assert_eq!(ok(&a, 35), "200 application/json {\"n\":7}", "{engine}: a request answered by its rule");
        assert_eq!(ok(&a, 38), "failed", "{engine}: a request its rule aborts");
        let rules = ok(&a, 39).as_str().unwrap();
        assert!(
            rules.contains("*/api/data* -> fulfill 200 (1 hits)") && rules.contains("*/api/gone -> abort (1 hits)"),
            "{engine}: {rules}"
        );
        assert_eq!(ok(&a, 40)["removed"], 2, "{engine}");
        assert!(
            ok(&a, 43).as_str().unwrap().starts_with("404 "),
            "{engine}: the server's own answer again: {}",
            ok(&a, 43)
        );
    } else {
        assert!(failed(31).contains("classic WebDriver cannot see a page's requests"), "{engine}: {}", failed(31));
        // (Several statements in one `eval`, and the server's own answer.)
        assert!(ok(&a, 35).as_str().unwrap().starts_with("404 "), "{engine}: {}", ok(&a, 35));
        assert!(failed(26).contains("classic WebDriver has no script"), "{engine}: {}", failed(26));
        assert!(failed(44).contains("classic WebDriver has no emulation"), "{engine}: {}", failed(44));
        assert_eq!(ok(&a, 28), "undefined", "{engine}");
    }
    assert_eq!(ok(&a, 30)["bye"], true);
}

/// `set` over WebDriver BiDi's `emulation.*` (Firefox): locale, timezone,
/// the color scheme, the user agent and offline, for every tab, a new one
/// too; what Firefox cannot emulate is refused with the engine to use.
fn emulate_ops(args: &[&str], engine: &str) {
    let site = serve();
    let set = |id: u64, what: &str, values: &[&str]| json!({ "id": id, "op": "set", "what": what, "values": values });
    let eval = |id: u64, expression: &str| json!({ "id": id, "op": "eval", "expression": expression });
    let fetch = "window.got = undefined; fetch('/more.html').then((r) => { window.got = r.status; }, () => { window.got = 'failed'; }); 0";
    // Another server is another origin: the permission follows the tab there.
    let other = serve();
    let position = "new Promise((ok) => navigator.geolocation.getCurrentPosition(\
                    (p) => ok([p.coords.latitude, p.coords.longitude, p.coords.accuracy]), (e) => ok('error ' + e.code), \
                    { timeout: 5000 }))";
    let state = "[navigator.language, new Intl.NumberFormat().format(1234.5), Intl.DateTimeFormat().resolvedOptions().timeZone, \
                 matchMedia('(prefers-color-scheme: dark)').matches, navigator.userAgent]";
    let script: Vec<Value> = vec![
        json!({ "id": 1, "op": "goto", "url": format!("{site}/more.html") }),
        set(2, "locale", &["de-DE"]),
        set(3, "timezone", &["Asia/Tokyo"]),
        set(4, "media", &["dark"]),
        set(5, "ua", &["NavigeraTest/1.0", "(emulated)"]),
        json!({ "id": 6, "op": "reload" }),
        eval(7, state),
        set(8, "media", &["print"]),
        set(9, "cpu", &["4"]),
        set(10, "offline", &[]),
        eval(11, fetch),
        json!({ "id": 12, "op": "wait", "js": "window.got !== undefined" }),
        eval(13, "window.got"),
        set(14, "offline", &["off"]),
        eval(15, fetch),
        json!({ "id": 16, "op": "wait", "js": "window.got !== undefined" }),
        eval(17, "window.got"),
        json!({ "id": 18, "op": "tab-new" }),
        json!({ "id": 19, "op": "goto", "url": format!("{site}/more.html") }),
        eval(20, state),
        set(21, "locale", &["off"]),
        set(22, "timezone", &["off"]),
        set(23, "media", &["off"]),
        set(24, "ua", &["off"]),
        json!({ "id": 25, "op": "reload" }),
        eval(26, state),
        set(27, "device", &["iPhone", "15"]),
        eval(28, "[innerWidth, devicePixelRatio, navigator.userAgent.includes('iPhone')]"),
        set(29, "device", &["off"]),
        set(30, "geo", &["52.52", "13.405"]),
        eval(31, position),
        json!({ "id": 32, "op": "goto", "url": format!("{other}/more.html") }),
        eval(33, position),
        set(34, "geo", &["off"]),
        json!({ "id": 40, "op": "quit" }),
    ];
    let a = session(args, &script);
    let failed = |id: u64| {
        let x = a.iter().find(|x| x["id"] == id).unwrap();
        assert_eq!(x["ok"], false, "{engine}: step {id} should fail: {x}");
        x["error"].to_string()
    };
    assert_eq!(ok(&a, 2), &json!({ "locale": "de-DE" }), "{engine}");
    assert_eq!(ok(&a, 4), &json!({ "media": "dark" }), "{engine}");
    let emulated = json!(["de-DE", "1.234,5", "Asia/Tokyo", true, "NavigeraTest/1.0 (emulated)"]);
    assert_eq!(ok(&a, 7), &emulated, "{engine}: the page sees every override");
    assert!(failed(8).contains("--engine chrome"), "{engine}: {}", failed(8));
    assert!(failed(9).contains("--engine chrome"), "{engine}: {}", failed(9));
    assert_eq!(ok(&a, 10), &json!({ "offline": true }), "{engine}");
    assert_eq!(ok(&a, 13), "failed", "{engine}: a request while offline");
    assert_eq!(ok(&a, 17), 200, "{engine}: online again");
    assert_eq!(ok(&a, 20), &emulated, "{engine}: a tab opened after the overrides has them too");
    let back = ok(&a, 26);
    assert!(
        back[0] != "de-DE" && back[2] != "Asia/Tokyo" && back[4] != "NavigeraTest/1.0 (emulated)",
        "{engine}: {back}"
    );
    assert_eq!(ok(&a, 27)["device"], "iPhone 15", "{engine}");
    assert!(ok(&a, 27)["note"].as_str().is_some_and(|n| n.contains("touch")), "{engine}: {}", ok(&a, 27));
    assert_eq!(ok(&a, 28), &json!([393, 3, true]), "{engine}: the page sees the device");
    assert_eq!(ok(&a, 29), &json!({ "device": "default" }), "{engine}");
    let nums = |v: &Value| v.as_array().map(|n| n.iter().map(Value::as_f64).collect::<Vec<_>>());
    let berlin = Some(vec![Some(52.52), Some(13.405), Some(50.0)]);
    assert_eq!(
        (nums(&ok(&a, 30)["geo"]), ok(&a, 30)["accuracy"].as_f64()),
        (nums(&json!([52.52, 13.405])), Some(50.0))
    );
    assert_eq!(nums(ok(&a, 31)), berlin, "{engine}: the page reads the position, no prompt: {}", ok(&a, 31));
    assert_eq!(nums(ok(&a, 33)), berlin, "{engine}: and so does the next origin: {}", ok(&a, 33));
    assert_eq!(ok(&a, 34), &json!({ "geo": "default" }), "{engine}");
    assert_eq!(ok(&a, 40)["bye"], true);
}

/// A file a page downloads is listed, waited for and saved where
/// `downloads --dir` says.
fn download_ops(args: &[&str], engine: &str) {
    let site = serve();
    let dir = tempfile::tempdir().unwrap();
    let start = "(() => { const a = document.createElement('a'); a.href = '/report.csv'; document.body.append(a); a.click(); return 1; })()";
    let script: Vec<Value> = vec![
        json!({ "id": 1, "op": "goto", "url": format!("{site}/more.html") }),
        json!({ "id": 2, "op": "downloads" }),
        json!({ "id": 3, "op": "eval", "expression": start }),
        json!({ "id": 4, "op": "wait", "download": "*.csv" }),
        json!({ "id": 5, "op": "downloads", "dir": dir.path().display().to_string() }),
        // As on Chrome, `wait --download` takes the newest finished one:
        // the first is forgotten so the wait is for the next.
        json!({ "id": 11, "op": "downloads", "clear": true }),
        json!({ "id": 6, "op": "eval", "expression": start }),
        json!({ "id": 7, "op": "wait", "download": "" }),
        json!({ "id": 8, "op": "downloads", "format": "json" }),
        json!({ "id": 9, "op": "wait", "download": "*.pdf", "timeout_ms": 300 }),
        json!({ "id": 10, "op": "quit" }),
    ];
    let a = session(args, &script);
    assert_eq!(ok(&a, 2), "(no downloads)", "{engine}");
    let first = &ok(&a, 4)["download"];
    assert_eq!(
        (&first["state"], &first["file"], &first["bytes"]),
        (&json!("completed"), &json!("report.csv"), &json!(CSV.len())),
        "{engine}: {first}"
    );
    let second = &ok(&a, 7)["download"];
    let saved = std::path::Path::new(second["path"].as_str().unwrap_or(""));
    // Both sides canonical: a temporary directory may be named through a
    // link (macOS) or by its short name (Windows).
    let parent = saved.parent().and_then(|p| p.canonicalize().ok());
    assert_eq!(parent, dir.path().canonicalize().ok(), "{engine}: {second}");
    assert_eq!(std::fs::read_to_string(saved).unwrap(), CSV, "{engine}");
    assert_eq!(ok(&a, 8).as_array().map(Vec::len), Some(1), "{engine}: {}", ok(&a, 8));
    let x = a.iter().find(|x| x["id"] == 9).unwrap();
    assert!(x["error"].to_string().contains("no finished download matching \"*.pdf\""), "{engine}: {x}");
}

/// A lone UTF-16 surrogate in a page (half of an emoji pair) reads as
/// U+FFFD in `text`, `ax` and `eval`, where it failed the command: the
/// page's and the browser's JSON write it as `\ud800`, which JSON in UTF-8
/// cannot hold.
fn lone_surrogates(args: &[&str], engine: &str) {
    let site = serve();
    let eval = |id: u64, expression: &str| json!({ "id": id, "op": "eval", "expression": expression });
    // An `eval` value holding one crosses Firefox's own BiDi server; the
    // other servers refuse it themselves (chromedriver over BiDi does not
    // answer at all), and over classic chromedriver the error says what to
    // do. `text` and `ax` come back on every engine.
    let firefox = args.contains(&"firefox");
    let classic_chromedriver = args.contains(&"--webdriver") && !args.contains(&"--bidi");
    let mut script: Vec<Value> = vec![
        json!({ "id": 1, "op": "goto", "url": format!("{site}/more.html") }),
        eval(2, "(document.body.innerHTML = '<p>x\\ud800y</p><button>Go\\udc00</button>', 1)"),
        json!({ "id": 3, "op": "text" }),
        json!({ "id": 4, "op": "ax", "format": "text" }),
    ];
    if firefox || classic_chromedriver {
        script.push(eval(5, "'\\ud800' + '\\ud83d\\ude00'"));
    }
    script.push(json!({ "id": 9, "op": "quit" }));
    let a = session(args, &script);
    assert!(ok(&a, 3).as_str().is_some_and(|t| t.contains("x\u{fffd}y")), "{engine}: {}", ok(&a, 3));
    assert!(ok(&a, 4).as_str().is_some_and(|t| t.contains("Go\u{fffd}")), "{engine}: {}", ok(&a, 4));
    if firefox {
        assert_eq!(ok(&a, 5), "\u{fffd}😀", "{engine}");
    } else if classic_chromedriver {
        let x = a.iter().find(|x| x["id"] == 5).unwrap();
        assert!(x["error"].to_string().contains(".toWellFormed()"), "{engine}: {x}");
    }
}

/// A list of allowed hosts is held by Chrome's engine only: a WebDriver
/// engine refuses to start with one, rather than run unrestricted.
#[test]
fn an_allowlist_is_refused_on_a_webdriver_engine() {
    for engine in [&["--engine", "firefox"][..], &["--engine", "safari"], &["--webdriver", "http://127.0.0.1:9"]] {
        let out = Command::new(env!("CARGO_BIN_EXE_navigera"))
            .args(engine)
            .args(["--allow-domains", "example.com", "goto", "https://example.com/"])
            .env_remove("NAVIGERA_SESSION")
            .output()
            .expect("navigera starts");
        let said = String::from_utf8_lossy(&out.stderr).to_string() + &String::from_utf8_lossy(&out.stdout);
        assert!(!out.status.success(), "{engine:?}: {said}");
        assert!(said.contains("--allow-domains is not on the"), "{engine:?}: {said}");
    }
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
    more_ops(&["--engine", "firefox", "--executable", &firefox], "firefox", true);
    emulate_ops(&["--engine", "firefox", "--executable", &firefox], "firefox");
    lone_surrogates(&["--engine", "firefox", "--executable", &firefox], "firefox");
    download_ops(&["--engine", "firefox", "--executable", &firefox], "firefox");
    nav_clicks(&["--engine", "firefox", "--executable", &firefox], "firefox");
}

/// A Firefox the user started with remote debugging is found by name and
/// by its port, worked in through a window of navigera's own, and left
/// running with the user's tab as it was.
#[test]
fn a_running_firefox_is_attached_to_and_left_running() {
    let Some(firefox) = found("NAVIGERA_FIREFOX") else {
        assert!(!required("NAVIGERA_REQUIRE_FIREFOX"), "NAVIGERA_REQUIRE_FIREFOX is set but no Firefox was found");
        eprintln!("skipped: no Firefox");
        return;
    };
    let site = serve();
    // A home of the user's, with the profile where Firefox keeps profiles.
    let home = std::env::temp_dir().join(format!("nv-ff-home-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let (var, profiles) = if cfg!(windows) {
        ("APPDATA", home.join("Mozilla").join("Firefox").join("Profiles"))
    } else if cfg!(target_os = "macos") {
        ("HOME", home.join("Library/Application Support/Firefox/Profiles"))
    } else {
        ("HOME", home.join(".mozilla/firefox"))
    };
    let profile = profiles.join("u1.default");
    std::fs::create_dir_all(&profile).unwrap();
    let mut browser = Command::new(&firefox)
        .arg("--remote-debugging-port=0")
        .arg("--profile")
        .arg(&profile)
        .args(["--no-remote", "--new-instance", "--headless"])
        .arg(format!("{site}/next.html"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("Firefox starts");
    // The file navigera finds it by, written once the endpoint is up.
    let file = profile.join("WebDriverBiDiServer.json");
    let started = Instant::now();
    let port = loop {
        let port = std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v["ws_port"].as_u64());
        if let Some(port) = port {
            break port.to_string();
        }
        if started.elapsed() > Duration::from_secs(90) || browser.try_wait().unwrap().is_some() {
            let _ = browser.kill();
            panic!("Firefox wrote no {} in {:?}", file.display(), started.elapsed());
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let script: Vec<Value> = vec![
        json!({ "id": 1, "op": "status" }),
        json!({ "id": 2, "op": "goto", "url": format!("{site}/") }),
        json!({ "id": 3, "op": "fill", "label": "Name", "value": "Ada" }),
        json!({ "id": 4, "op": "click", "role": "button", "name": "Send" }),
        json!({ "id": 5, "op": "text", "selector": "#out" }),
        json!({ "id": 6, "op": "tab-list" }),
        json!({ "id": 7, "op": "tab-new" }),
        json!({ "id": 8, "op": "tab-list" }),
        // A tab its page opens is its own too: listed, and switched to.
        json!({ "id": 10, "op": "tab-select", "index": 0 }),
        // A frame in its tab is its own: the frame's console line is
        // heard and the frame's dialog answered.
        json!({ "id": 16, "op": "eval", "expression": "(document.getElementById('inner').contentWindow.console.log('from the frame'), 1)" }),
        json!({ "id": 17, "op": "eval", "expression": "(document.getElementById('inner').contentWindow.alert('in the frame'), 2)" }),
        json!({ "id": 18, "op": "console" }),
        json!({ "id": 11, "op": "eval", "expression": "(document.body.insertAdjacentHTML('beforeend', '<button onclick=\"window.w = window.open(\\'/next.html\\')\">Pop</button>'), 1)" }),
        json!({ "id": 12, "op": "click", "role": "button", "name": "Pop" }),
        json!({ "id": 13, "op": "wait", "js": "!!window.w && window.w.document.title === 'Next'" }),
        json!({ "id": 14, "op": "tab-list" }),
        json!({ "id": 15, "op": "tab-select", "index": 2 }),
        json!({ "id": 9, "op": "quit" }),
    ];
    let home_env: [(&str, &std::path::Path); 1] = [(var, &home)];
    // By name (the profile's file), then by port: the second works only
    // if the first ended its session, since Firefox serves one at a time.
    for (how, args, env) in [
        ("by name", vec!["--attach", "firefox"], &home_env[..]),
        ("by port", vec!["--engine", "firefox", "--attach", port.as_str()], &[][..]),
    ] {
        let a = session_in(&args, env, &script);
        let status = ok(&a, 1);
        assert_eq!(status["engine"], "firefox", "{how}: {status}");
        assert_eq!(ok(&a, 2)["title"], "Fixture", "{how}");
        assert_eq!(ok(&a, 5), "sent Ada s false", "{how}");
        // Only what it opened counts: its window, not the user's tab.
        let tabs = |id: u64| ok(&a, id).as_array().map(Vec::len);
        assert_eq!((tabs(6), tabs(8)), (Some(1), Some(2)), "{how}: tabs {} then {}", ok(&a, 6), ok(&a, 8));
        assert_eq!(ok(&a, 17), 2, "{how}: a dialog in a frame");
        let console = ok(&a, 18).to_string();
        assert!(console.contains("from the frame"), "{how}: a frame's console line is not heard: {console}");
        assert_eq!(tabs(14), Some(3), "{how}: a tab the page opened is not listed: {}", ok(&a, 14));
        assert_eq!(ok(&a, 15)["title"], "Next", "{how}");
        assert_eq!(ok(&a, 9)["bye"], true, "{how}");
        assert!(browser.try_wait().unwrap().is_none(), "{how}: the user's Firefox was closed");
    }
    // The user's tab is where they left it; navigera's windows are gone.
    let after = [
        json!({ "id": 1, "op": "tab-new" }),
        json!({ "id": 2, "op": "eval", "expression": "1 + 1" }),
        json!({ "id": 3, "op": "quit" }),
    ];
    let a = session_in(&["--engine", "firefox", "--attach", &port], &[], &after);
    assert_eq!(ok(&a, 2), 2);
    let _ = browser.kill();
    let _ = browser.wait();
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn safari_over_safaridriver_reads_acts_and_navigates() {
    if !cfg!(target_os = "macos") || !required("NAVIGERA_SAFARI") {
        assert!(!required("NAVIGERA_REQUIRE_SAFARI"), "NAVIGERA_REQUIRE_SAFARI is set but Safari is not enabled here");
        eprintln!("skipped: Safari runs on macOS with NAVIGERA_SAFARI=1");
        return;
    }
    scenario(&["--engine", "safari"], "safari");
    more_ops(&["--engine", "safari"], "safari", false);
    nav_clicks(&["--engine", "safari"], "safari");
    lone_surrogates(&["--engine", "safari"], "safari");
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
    more_ops(&["--webdriver", &url, "--capabilities", &caps], "webdriver", false);
    more_ops(&["--webdriver", &url, "--bidi", "--capabilities", &caps], "webdriver", true);
    nav_clicks(&["--webdriver", &url, "--capabilities", &caps], "webdriver");
    nav_clicks(&["--webdriver", &url, "--bidi", "--capabilities", &caps], "webdriver");
    lone_surrogates(&["--webdriver", &url, "--capabilities", &caps], "webdriver");
    lone_surrogates(&["--webdriver", &url, "--bidi", "--capabilities", &caps], "webdriver");
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
