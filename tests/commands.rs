//! The inspection and emulation commands end to end, on Chrome and on
//! Lightpanda: console/errors/network, cookies, storage, pdf, set, route,
//! init-script, locators, get/is, dblclick, check/uncheck, keys.
//!
//! Each scenario runs on every engine it finds (`$CHROME_BIN` or a system
//! Chrome; `$LIGHTPANDA_BIN` or `lightpanda` on PATH);
//! `NAVIGERA_REQUIRE_BROWSER=1` / `NAVIGERA_REQUIRE_LIGHTPANDA=1` turn a
//! missing one into a failure.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use serde_json::{json, Value};

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

/// (engine, browser binary) pairs to run on.
fn engines() -> Vec<(&'static str, Option<PathBuf>)> {
    let mut out = Vec::new();
    let chrome = navigera::browser::resolve_executable_for(None, true).is_some();
    assert!(chrome || std::env::var_os("NAVIGERA_REQUIRE_BROWSER").is_none(), "no Chrome found");
    if chrome {
        out.push(("chrome", None));
    }
    let lightpanda = std::env::var_os("LIGHTPANDA_BIN").map(PathBuf::from).filter(|p| p.is_file()).or_else(|| {
        std::env::split_paths(&std::env::var_os("PATH")?)
            .map(|d| d.join(if cfg!(windows) { "lightpanda.exe" } else { "lightpanda" }))
            .find(|p| p.is_file())
    });
    assert!(lightpanda.is_some() || std::env::var_os("NAVIGERA_REQUIRE_LIGHTPANDA").is_none(), "no lightpanda found");
    if let Some(bin) = lightpanda {
        out.push(("lightpanda", Some(bin)));
    }
    out
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

fn edge_site() -> Option<Site> {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/edge_site.py");
    let mut child =
        Command::new(python()).args([script, "0"]).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().ok()?;
    let mut line = String::new();
    BufReader::new(child.stdout.take()?).read_line(&mut line).ok()?;
    let base = line.split_whitespace().nth(1)?.to_string();
    Some(Site { child, base })
}

struct Agent {
    session: String,
    engine: &'static str,
    bin: Option<PathBuf>,
}

impl Agent {
    fn new(name: &str, engine: &'static str, bin: Option<PathBuf>) -> Self {
        let session = std::env::temp_dir()
            .join(format!("nv-cmd-{name}-{engine}-{}.sock", std::process::id()))
            .display()
            .to_string();
        let agent = Agent { session, engine, bin };
        agent.ok(&["start", "--idle-timeout-s", "300"]);
        agent
    }

    fn run(&self, args: &[&str]) -> Value {
        let mut cmd = Command::new(tool_exe());
        cmd.args(["--session", &self.session, "--engine", self.engine]);
        if let Some(bin) = &self.bin {
            cmd.arg("--chromium").arg(bin);
        }
        let out = cmd.args(args).stdin(Stdio::null()).output().expect("run navigera");
        let text = String::from_utf8_lossy(&out.stdout);
        serde_json::from_str(text.trim())
            .unwrap_or_else(|_| panic!("[{}] stdout must be one JSON object for {args:?}: {text}", self.engine))
    }

    fn response(&self, args: &[&str]) -> Value {
        let r = self.run(args);
        assert_eq!(r["ok"], true, "[{}] {args:?} failed: {r}", self.engine);
        r
    }

    fn ok(&self, args: &[&str]) -> Value {
        self.response(args)["result"].clone()
    }

    fn text(&self, args: &[&str]) -> String {
        match self.ok(args) {
            Value::String(s) => s,
            other => other.to_string(),
        }
    }

    fn err(&self, args: &[&str]) -> String {
        let r = self.run(args);
        assert_eq!(r["ok"], false, "[{}] {args:?} should fail: {r}", self.engine);
        r["error"].as_str().unwrap_or_default().to_string()
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = Command::new(tool_exe()).args(["--session", &self.session, "quit"]).stdin(Stdio::null()).output();
    }
}

#[test]
fn console_errors_and_network() {
    let Some(site) = edge_site() else { return };
    for (engine, bin) in engines() {
        let a = Agent::new("log", engine, bin);
        let state = a.ok(&["goto", &format!("{}/console", site.base)]);
        a.ok(&["wait", "--js", "document.title.startsWith('echo')"]);
        assert_eq!(state["url"].as_str().map(|u| u.ends_with("/console")), Some(true));
        let console = a.text(&["console"]);
        assert!(console.contains("[log] hello"), "[{engine}] {console}");
        assert!(console.contains("[warn] careful"), "[{engine}] {console}");
        assert!(a.text(&["errors"]).contains("[error] bad thing"), "[{engine}]");
        assert!(!a.text(&["errors"]).contains("careful"), "[{engine}] errors are errors only");
        // A handler that throws: the click says so.
        let r = a.response(&["click", "--selector", "#boom"]);
        let note = r["notes"][0].as_str().unwrap_or_default();
        assert!(note.contains("cartTotal is not defined"), "[{engine}] {r}");
        assert!(a.text(&["errors"]).contains("[exception] ReferenceError"), "[{engine}]");
        a.ok(&["console", "--clear"]);
        assert_eq!(a.text(&["console"]), "(no console messages)");

        let list = a.text(&["network"]);
        assert!(list.contains("GET 200 Document /console"), "[{engine}] {list}");
        let line = list
            .lines()
            .find(|l| l.contains("POST 200 Fetch /api/echo"))
            .unwrap_or_else(|| panic!("[{engine}] {list}"));
        let n = line.split_whitespace().next().unwrap().trim_start_matches('#');
        let req = a.ok(&["network", n]);
        assert_eq!(req["post_data"], r#"{"q":"widget"}"#, "[{engine}] {req}");
        assert!(req["body"].as_str().unwrap().contains("widget"), "[{engine}] {req}");
        let har = std::env::temp_dir().join(format!("nv-{engine}-{}.har", std::process::id()));
        a.ok(&["network", "--har", har.to_str().unwrap()]);
        let har: Value = serde_json::from_slice(&std::fs::read(&har).unwrap()).unwrap();
        assert!(har["log"]["entries"].as_array().unwrap().len() >= 2, "[{engine}]");
    }
}

#[test]
fn cookies_storage_pdf_init_scripts_and_routes() {
    let Some(site) = edge_site() else { return };
    let base = &site.base;
    for (engine, bin) in engines() {
        let a = Agent::new("state", engine, bin);
        a.ok(&["goto", &format!("{base}/form")]);
        a.ok(&["cookies", "set", "theme", "dark", "--same-site", "Lax"]);
        a.ok(&["cookies", "set", "token", "t1", "--http-only"]);
        let cookies = a.text(&["cookies"]);
        assert!(
            cookies.contains("theme=dark")
                && cookies.contains("token=(HttpOnly, hidden: the browser sends it")
                && !cookies.contains("t1"),
            "[{engine}] an HttpOnly value stays out of the list: {cookies}"
        );
        let cookies = a.text(&["cookies", "--values"]);
        assert!(cookies.contains("token=t1; ") && cookies.contains("HttpOnly"), "[{engine}] {cookies}");
        assert_eq!(a.ok(&["eval", "document.cookie"]), "theme=dark", "[{engine}] HttpOnly stays out of JS");
        a.ok(&["cookies", "delete", "theme"]);
        assert!(!a.text(&["cookies"]).contains("theme"), "[{engine}]");
        a.ok(&["cookies", "clear"]);
        assert_eq!(a.text(&["cookies"]), "(no cookies)");

        a.ok(&["storage", "set", "k", "v"]);
        assert_eq!(a.ok(&["storage", "get", "k"]), "v");
        a.ok(&["storage", "session", "set", "s", "1"]);
        assert_eq!(a.ok(&["storage", "session"]), serde_json::json!({"s": "1"}));
        a.ok(&["storage", "clear"]);
        assert_eq!(a.ok(&["storage"]), serde_json::json!({}));

        let pdf = std::env::temp_dir().join(format!("nv-{engine}-{}.pdf", std::process::id()));
        let r = a.ok(&["pdf", "--path", pdf.to_str().unwrap()]);
        assert!(r["bytes"].as_u64().unwrap() > 1000, "[{engine}] {r}");
        assert!(std::fs::read(&pdf).unwrap().starts_with(b"%PDF"), "[{engine}]");

        a.ok(&["init-script", "window.__seen = (window.__seen || 0) + 1"]);
        a.ok(&["goto", &format!("{base}/form")]);
        assert_eq!(a.ok(&["eval", "window.__seen"]), 1, "[{engine}] runs once per document");
        a.ok(&["init-script", "--clear"]);
        a.ok(&["goto", &format!("{base}/form")]);
        assert_eq!(a.ok(&["eval", "String(window.__seen)"]), "undefined", "[{engine}]");

        // A mocked API and a blocked image.
        a.ok(&["route", "*/api/echo*", "--status", "200", "--body", r#"{"got":"mocked"}"#]);
        a.ok(&["route", "*.png"]);
        a.ok(&["goto", &format!("{base}/console")]);
        a.ok(&["wait", "--js", "document.title === 'echo mocked'"]);
        assert!(a.text(&["route"]).contains("*/api/echo* -> fulfill 200 (1 hits)"), "[{engine}]");
        assert_eq!(a.ok(&["unroute"])["removed"], 2);

        // HTTP basic auth.
        a.ok(&["set", "credentials", "ada", "lovelace"]);
        a.ok(&["goto", &format!("{base}/auth")]);
        assert_eq!(a.ok(&["text", "--selector", "h1"]), "welcome ada", "[{engine}]");
        a.ok(&["set", "credentials", "off"]);
    }
}

/// `state save` in one session and `state load` in a fresh one: the cookies
/// (HttpOnly included) and the page's localStorage come back, in the
/// `{cookies, origins}` storage-state format.
#[test]
fn state_saved_in_one_session_loads_in_another() {
    let Some(site) = edge_site() else { return };
    let base = &site.base;
    for (engine, bin) in engines() {
        let file = std::env::temp_dir().join(format!("nv-state-{engine}-{}.json", std::process::id()));
        let file = file.to_str().unwrap();
        {
            let a = Agent::new("save", engine, bin.clone());
            a.ok(&["goto", &format!("{base}/form")]);
            a.ok(&["cookies", "set", "sid", "s3cret", "--http-only"]);
            a.ok(&["storage", "set", "cart", "[1,2]"]);
            let r = a.ok(&["state", "save", file]);
            assert_eq!(r["origins"], 1, "[{engine}] {r}");
            let saved: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
            let sid = saved["cookies"].as_array().unwrap().iter().find(|c| c["name"] == "sid").cloned();
            let sid = sid.unwrap_or_else(|| panic!("[{engine}] {saved}"));
            assert_eq!(sid["httpOnly"], true, "[{engine}] {saved}");
            assert_eq!(saved["origins"][0]["origin"], base.as_str(), "[{engine}] {saved}");
            assert_eq!(saved["origins"][0]["localStorage"][0]["name"], "cart", "[{engine}] {saved}");
        }
        let b = Agent::new("load", engine, bin);
        let r = b.ok(&["state", "load", file]);
        assert_eq!((r["cookies"].as_u64(), r["origins"].as_u64()), (Some(1), Some(1)), "[{engine}] {r}");
        b.ok(&["goto", &format!("{base}/form")]);
        assert!(b.text(&["cookies", "--values"]).contains("sid=s3cret"), "[{engine}]");
        assert_eq!(b.ok(&["storage", "get", "cart"]), "[1,2]", "[{engine}]");
        assert!(b.err(&["state", "load", "/nonexistent/state.json"]).contains("state load"), "[{engine}]");
        let _ = std::fs::remove_file(file);
    }
}

/// HTML5 drag and drop onto a drop zone, and a slider dragged with the
/// button held, on both engines.
#[test]
fn drag_drops_a_card_and_moves_a_slider() {
    let Some(site) = edge_site() else { return };
    let base = &site.base;
    for (engine, bin) in engines() {
        let a = Agent::new("drag", engine, bin);
        a.ok(&["goto", &format!("{base}/drag")]);
        let r = a.ok(&["drag", "#card", "#done"]);
        assert_eq!(r["drag"]["html5"], true, "[{engine}] {r}");
        assert_eq!(r["drag"]["dropped"], true, "[{engine}] {r}");
        assert_eq!(a.ok(&["eval", "done.contains(card)"]), true, "[{engine}]");
        // Dropped back where nothing accepts it: reported, not an error.
        let r = a.ok(&["drag", "--from-text", "Task 1", "--to-text", "Done"]);
        assert_eq!(r["drag"]["dropped"], true, "[{engine}] {r}");
        let r = a.ok(&["drag", "#card", "#out"]);
        assert_eq!(r["drag"]["dropped"], false, "[{engine}] {r}");
        a.ok(&["drag", "#knob", "#track"]);
        assert_eq!(a.ok(&["eval", "out.textContent"]), "knob 140px", "[{engine}]");
        assert!(a.err(&["drag", "#card", "#nowhere", "--timeout-ms", "300"]).contains("drag"), "[{engine}]");
    }
}

#[test]
fn screenshot_of_one_element() {
    // A card far below the fold, the page's only button beside it.
    let page = "data:text/html,<style>body{margin:0}%23card{margin:1500px 0 0 40px;width:200px;height:120px;background:red}</style><div id=card><button>Buy</button></div>";
    let file = std::env::temp_dir().join(format!("nv-el-{}.png", std::process::id()));
    let file = file.to_str().unwrap();
    let size = |path: &str| {
        let b = std::fs::read(path).unwrap();
        (u32::from_be_bytes(b[16..20].try_into().unwrap()), u32::from_be_bytes(b[20..24].try_into().unwrap()))
    };
    for (engine, bin) in engines() {
        if engine == "lightpanda" {
            continue;
        }
        let a = Agent::new("elshot", engine, bin);
        a.ok(&["goto", page]);
        a.ok(&["screenshot", file, "--selector", "#card"]);
        assert_eq!(size(file), (200, 120), "[{engine}] cropped to the card, scrolled into view");
        let r = a.text(&["ax"]);
        let button = r.split("[ref=").nth(1).and_then(|t| t.split(']').next()).unwrap().to_string();
        a.ok(&["screenshot", &button, "--path", file]);
        let (w, h) = size(file);
        assert!(w < 200 && h < 120 && w > 10, "[{engine}] a bare number is a ref: {w}x{h}");
        assert!(a.err(&["screenshot", file, "--full-page", "--selector", "#card"]).contains("not both"));
        // --annotate: each ref outlined and numbered for the capture only.
        a.ok(&["goto", "data:text/html,<button>One</button> <a href=%23x>Two</a> <input aria-label=Three>"]);
        let before = a.text(&["ax"]);
        a.ok(&["screenshot", file]);
        let plain = std::fs::read(file).unwrap();
        let r = a.ok(&["screenshot", file, "--annotate"]);
        assert_eq!(r["refs"], 3, "[{engine}] {r}");
        assert_ne!(std::fs::read(file).unwrap(), plain, "[{engine}] the labels are in the image");
        assert_eq!(a.ok(&["eval", "!!document.getElementById('__nv_marks')"]), false, "[{engine}] and gone after");
        assert_eq!(a.text(&["ax"]), before, "[{engine}] the page reads as before");
        assert!(a.err(&["screenshot", file, "--annotate", "--selector", "button"]).contains("not one element"));
    }
    let _ = std::fs::remove_file(file);
}

#[test]
fn passkeys() {
    let Some(site) = edge_site() else { return };
    // A passkey's relying party is a domain: localhost, not 127.0.0.1.
    let page = format!("{}/passkey", site.base.replace("127.0.0.1", "localhost"));
    let file = std::env::temp_dir().join(format!("nv-passkey-{}.json", std::process::id()));
    let file = file.to_str().unwrap();
    for (engine, bin) in engines() {
        let a = Agent::new("passkey", engine, bin);
        if engine == "lightpanda" {
            assert!(a.err(&["passkey", "on"]).contains("no WebAuthn"), "[{engine}]");
            continue;
        }
        assert_eq!(a.ok(&["passkey"])["passkeys"], "off");
        assert_eq!(a.ok(&["passkey", "on"])["transport"], "internal");
        a.ok(&["goto", &page]);
        let sign_in = |a: &Agent, expect: &str| {
            a.ok(&["click", "--selector", "#login"]);
            a.ok(&["wait", "--text", expect]);
        };
        a.ok(&["click", "--selector", "#reg"]);
        a.ok(&["wait", "--text", "created"]);
        let list = a.ok(&["passkey", "list"]);
        assert_eq!(list["passkeys"][0]["rp_id"], "localhost", "{list}");
        assert_eq!(list["passkeys"][0]["user"], "ada@example.com", "{list}");
        assert!(list["passkeys"][0].get("private_key").is_none() && list.to_string().len() < 300, "{list}");
        sign_in(&a, "signed in as ada");
        // A tab opened later holds the passkey too.
        a.ok(&["tab-new", &page]);
        sign_in(&a, "signed in as ada");
        // A fingerprint that doesn't match.
        assert_eq!(a.ok(&["passkey", "verify", "off"])["user_verification"], "fails");
        sign_in(&a, "error NotAllowedError");
        a.ok(&["passkey", "verify", "on"]);
        // `state save` keeps it, `reset` drops it, `state load` brings it back.
        assert_eq!(a.ok(&["state", "save", file])["passkeys"], 1);
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
        let c = &saved["credentials"][0];
        assert!(c["privateKey"].is_string() && c["publicKey"].is_string() && c["rpId"] == "localhost", "{c}");
        a.ok(&["reset"]);
        a.ok(&["goto", &page]);
        assert_eq!(a.ok(&["passkey", "list"])["passkeys"], serde_json::json!([]), "the authenticator stays, empty");
        sign_in(&a, "error NotAllowedError");
        assert_eq!(a.ok(&["state", "load", file])["passkeys"], 1);
        sign_in(&a, "signed in as ada");
        a.ok(&["passkey", "clear"]);
        sign_in(&a, "error NotAllowedError");
        assert_eq!(a.ok(&["passkey", "off"])["passkeys"], "off");
    }
    let _ = std::fs::remove_file(file);
}

#[test]
fn a11y_audits_with_refs_and_leaves_the_page_alone() {
    let page = "data:text/html,<html><title>a</title><main><h1>Shop</h1><img src=x.png width=10 height=10><button></button><form id=f><input></form></main></html>";
    for (engine, bin) in engines() {
        let a = Agent::new("a11y", engine, bin);
        a.ok(&["goto", page]);
        if engine == "lightpanda" {
            assert!(a.err(&["a11y"]).contains("no layout"), "[{engine}]");
            continue;
        }
        let r = a.ok(&["a11y"]);
        let rules: Vec<&str> = r["rules"].as_array().unwrap().iter().filter_map(|x| x["rule"].as_str()).collect();
        for want in ["button-name", "image-alt", "label", "html-has-lang"] {
            assert!(rules.contains(&want), "[{engine}] {want} in {rules:?}");
        }
        assert_eq!(r["rules"][0]["impact"], "critical", "worst first: {r}");
        // The flagged button is the ref `ax` shows for it.
        let tree = a.text(&["ax"]);
        let button = r["rules"].as_array().unwrap().iter().find(|x| x["rule"] == "button-name").unwrap();
        let ref_ = button["nodes"][0]["ref"].as_u64().unwrap();
        assert!(tree.contains(&format!("button [ref={ref_}]")), "[{engine}] ref {ref_} in\n{tree}");
        // Scoped, with a rule left out; and the page never sees axe.
        let r = a.ok(&["a11y", "--selector", "#f"]);
        assert_eq!(r["rules"].as_array().unwrap().len(), 1, "{r}");
        assert_eq!(r["rules"][0]["rule"], "label");
        let r = a.ok(&["a11y", "--ignore", "image-alt,button-name"]);
        assert!(!r.to_string().contains("image-alt") && r["violations"].as_u64() >= Some(2), "{r}");
        assert_eq!(a.ok(&["eval", "typeof window.axe"]), "undefined", "[{engine}]");
        assert!(a.err(&["a11y", "--selector", "#nope"]).contains("no element matches"));
    }
}

#[test]
fn vitals() {
    // A heading painted at once, a paragraph that pushes it down once the
    // page has painted (a shift before the first paint is no layout shift,
    // and a slow runner paints later than any fixed delay), a quick button
    // and one whose handler keeps the page busy for 250 ms. The browser
    // records a shift with the frame that shows it, not when the DOM
    // changes: `window.shifted` says it has (the test waits for that, not
    // for the paragraph's text, which is there a frame earlier).
    let page = "data:text/html,<h1 id=t>Hello</h1><button id=slow onclick=\"let t=performance.now();while(performance.now()-t<250){}\">slow</button><button id=quick>quick</button><script>new PerformanceObserver((l)=>{if(l.getEntries().some(e=>e.value>0))window.shifted=true}).observe({type:'layout-shift',buffered:true});new PerformanceObserver((l,o)=>{if(!l.getEntries().some(e=>e.name==='first-contentful-paint'))return;o.disconnect();setTimeout(()=>{const p=document.createElement('p');p.style.height='300px';p.textContent='pushed';document.body.prepend(p)},100)}).observe({type:'paint',buffered:true})</script>";
    for (engine, bin) in engines() {
        let a = Agent::new("vitals", engine, bin);
        a.ok(&["goto", page]);
        if engine == "lightpanda" {
            assert!(a.err(&["vitals"]).contains("no web vitals"), "[{engine}]");
            continue;
        }
        a.ok(&["wait", "--text", "pushed"]);
        a.ok(&["wait", "--js", "window.shifted === true"]);
        let v = a.ok(&["vitals"]);
        assert!(v["lcp"].as_f64().is_some_and(|n| n > 0.0), "{v}");
        assert_eq!(v["lcp_element"], "#t", "{v}");
        assert!(v["fcp"].is_number() && v["ttfb"].is_number() && v["load"].is_number(), "{v}");
        assert!(v["cls"].as_f64().is_some_and(|n| n > 0.01), "the paragraph moved the page: {v}");
        assert!(v["cls_elements"].as_array().is_some_and(|m| m.contains(&"#t".into())), "{v}");
        assert_eq!((v["interactions"].as_u64(), v["inp"].is_null()), (Some(0), true), "{v}");
        assert!(v["rating"].get("inp").is_none() && v["rating"]["lcp"] == "good", "{v}");
        a.ok(&["click", "--selector", "#quick"]);
        let v = a.ok(&["vitals"]);
        assert!(v["interactions"].as_u64() >= Some(1), "{v}");
        // Quicker than the browser keeps (104 ms), unless the machine is slow.
        assert!(v["inp"].as_f64().is_some_and(|n| n < 250.0) || v["inp_under_ms"] == 104.0, "{v}");
        a.ok(&["click", "--selector", "#slow"]);
        let v = a.ok(&["vitals"]);
        // Event timings are rounded to 8 ms: a 250 ms handler reads 248 or 256.
        assert!(v["inp"].as_f64().is_some_and(|n| n >= 240.0), "{v}");
        assert_eq!((v["inp_element"].as_str(), v["inp_event"].as_str()), (Some("#slow"), Some("click")), "{v}");
        assert_ne!(v["rating"]["inp"], "good", "{v}");
    }
}

#[test]
fn emulation() {
    let Some(site) = edge_site() else { return };
    for (engine, bin) in engines() {
        let a = Agent::new("emu", engine, bin);
        a.ok(&["goto", &format!("{}/form", site.base)]);
        a.ok(&["set", "viewport", "800", "600"]);
        assert_eq!(a.ok(&["eval", "[innerWidth, innerHeight]"]), serde_json::json!([800, 600]), "[{engine}]");
        a.ok(&["set", "geo", "52.52", "13.405"]);
        let geo = a.ok(&[
            "eval",
            "() => new Promise(r => navigator.geolocation.getCurrentPosition(p => r([p.coords.latitude, p.coords.longitude]), e => r(e.message), {timeout: 3000}))",
        ]);
        assert_eq!(geo, serde_json::json!([52.52, 13.405]), "[{engine}]");
        a.ok(&["set", "headers", "X-Probe:", "1"]);
        if engine == "chrome" {
            a.ok(&["set", "device", "Pixel", "8"]);
            assert_eq!(a.ok(&["eval", "devicePixelRatio"]), 2.625);
            a.ok(&["set", "network", "slow-3g"]);
            let rtt = a.ok(&["eval", "navigator.connection.rtt"]).as_f64().unwrap();
            assert!(rtt >= 1000.0, "[{engine}] the page sees a slow network: {rtt}");
            let started = std::time::Instant::now();
            a.ok(&["goto", &format!("{}/form", site.base)]);
            assert!(started.elapsed() >= std::time::Duration::from_millis(1900), "{:?}", started.elapsed());
            a.ok(&["set", "network", "off"]);
            // The same work, unthrottled then six times slower: the quickest
            // of two runs each way, so a loaded machine's pause can't decide
            // it. Chrome throttles by pausing the main thread in slices, so
            // a short loop comes out between 2x and 9x; the bounds only say
            // that it slowed down and came back.
            let busy = "(() => { const t = performance.now(); let x = 0; for (let i = 0; i < 5e7; i++) x += i % 7; return performance.now() - t; })()";
            let ms = |a: &Agent| (0..2).map(|_| a.ok(&["eval", busy]).as_f64().unwrap()).fold(f64::MAX, f64::min);
            let plain = ms(&a);
            assert_eq!(a.ok(&["set", "cpu", "6"]), serde_json::json!({"cpu": 6.0}));
            let slow = ms(&a);
            assert!(slow > plain * 1.8, "[{engine}] set cpu 6: {plain:.0} ms -> {slow:.0} ms");
            assert_eq!(a.ok(&["set", "cpu", "off"]), serde_json::json!({"cpu": "default"}));
            let back = ms(&a);
            assert!(back * 1.5 < slow, "[{engine}] set cpu off: {slow:.0} ms -> {back:.0} ms");
            a.ok(&["set", "media", "dark"]);
            assert_eq!(a.ok(&["eval", "matchMedia('(prefers-color-scheme: dark)').matches"]), true);
            a.ok(&["set", "timezone", "Asia/Tokyo"]);
            assert_eq!(a.ok(&["eval", "Intl.DateTimeFormat().resolvedOptions().timeZone"]), "Asia/Tokyo");
            // Replayed on a tab opened later.
            a.ok(&["tab-new", &format!("{}/form", site.base)]);
            assert_eq!(a.ok(&["eval", "devicePixelRatio"]), 2.625, "a new tab gets the device too");
            a.ok(&["set", "offline", "on"]);
            assert_eq!(a.ok(&["eval", "navigator.onLine"]), false, "the page is told it is offline");
            assert!(a.err(&["goto", &format!("{}/form", site.base)]).contains("INTERNET_DISCONNECTED"));
            a.ok(&["set", "offline", "off"]);
            a.ok(&["goto", &format!("{}/form", site.base)]);
            assert_eq!(a.ok(&["eval", "navigator.onLine"]), true, "and back online");
        } else {
            // Settings Lightpanda would accept and ignore are refused.
            assert!(a.err(&["set", "media", "dark"]).contains("does not emulate"));
            assert!(a.err(&["set", "timezone", "Asia/Tokyo"]).contains("at launch only"));
            assert!(a.err(&["set", "offline", "on"]).contains("does not emulate going offline"));
            assert!(a.err(&["set", "cpu", "4"]).contains("does not emulate a slower CPU"));
            a.ok(&["set", "offline", "off"]);
            assert!(a.ok(&["set", "device", "Pixel 8"])["note"].as_str().unwrap().contains("viewport only"));
        }
    }
}

#[test]
fn permissions_are_granted_denied_and_reset_and_more_devices_are_known() {
    let Some(site) = edge_site() else { return };
    for (engine, bin) in engines() {
        if engine != "chrome" {
            continue;
        }
        let a = Agent::new("perm", engine, bin);
        a.ok(&["goto", &format!("{}/form", site.base)]);
        let state = |name: &str| {
            a.text(&["eval", &format!("() => navigator.permissions.query({{ name: '{name}' }}).then(p => p.state)")])
        };
        assert_eq!(state("notifications"), "prompt");
        a.ok(&["set", "permissions", "grant", "notifications", "geolocation"]);
        assert_eq!(state("notifications"), "granted");
        assert_eq!(state("geolocation"), "granted");
        a.ok(&["set", "permissions", "deny", "notifications"]);
        assert_eq!(state("notifications"), "denied");
        assert_eq!(state("geolocation"), "granted", "the other permission is untouched");
        a.ok(&["set", "permissions", "reset", "notifications", "geolocation"]);
        assert_eq!(state("notifications"), "prompt");
        a.ok(&["set", "permissions", "grant", "camera", "microphone", "clipboard"]);
        assert_eq!(state("camera"), "granted");
        assert_eq!(state("microphone"), "granted");
        assert!(a.err(&["set", "permissions", "grant", "teleport"]).contains("unknown permission"));
        assert!(a.err(&["set", "permissions", "grant"]).contains("set permissions"));
        a.ok(&["set", "device", "iPhone", "16", "Pro"]);
        assert_eq!(a.ok(&["eval", "[screen.width, devicePixelRatio]"]), serde_json::json!([402, 3]));
        a.ok(&["set", "device", "Pixel", "9"]);
        assert_eq!(a.ok(&["eval", "[screen.width, devicePixelRatio]"]), serde_json::json!([360, 3]));
    }
}

#[test]
fn http_sends_the_browsers_cookies_and_keeps_what_it_sets() {
    let Some(site) = edge_site() else { return };
    let base = &site.base;
    for (engine, bin) in engines() {
        if engine == "lightpanda" {
            continue;
        }
        let a = Agent::new("http", engine, bin);
        a.ok(&["goto", &format!("{base}/form")]);
        a.ok(&["cookies", "set", "sid", "s3cret-value", "--http-only"]);
        // The browser's cookies for the URL go with the request, HttpOnly
        // too, and no value is printed (the site reports names only).
        let r = a.text(&["http", &format!("{base}/api/whoami")]);
        assert!(r.starts_with("HTTP 200 · ") && r.contains("sent the browser's 1 cookie(s)"), "[{engine}] {r}");
        assert!(r.contains(r#""cookies": ["sid"]"#) && r.contains(r#""sid_length": 12"#), "[{engine}] {r}");
        assert!(!r.contains("s3cret"), "[{engine}] {r}");
        // A JSON body is a POST with its type; a form, a form.
        let r = a.ok(&["http", "POST", &format!("{base}/api/whoami"), "--json", r#"{"n":1}"#, "--as-json"]);
        let got: Value = serde_json::from_str(r["body"].as_str().unwrap()).unwrap();
        assert_eq!(
            (&got["method"], &got["content_type"], &got["body"]),
            (&json!("POST"), &json!("application/json"), &json!(r#"{"n":1}"#)),
            "[{engine}] {r}"
        );
        assert_eq!((&r["status"], &r["cookies_sent"]), (&json!(200), &json!(1)), "[{engine}] {r}");
        let r = a.ok(&["http", &format!("{base}/api/whoami"), "--form", "a=1", "--as-json"]);
        let got: Value = serde_json::from_str(r["body"].as_str().unwrap()).unwrap();
        assert_eq!((&got["method"], &got["body"]), (&json!("POST"), &json!("a=1")), "[{engine}] {r}");
        // A sign-in through the API: the cookie it sets is in the browser
        // before the redirect's next hop, which sends it (a 303 is a GET).
        let r = a.ok(&["http", "POST", &format!("{base}/api/sign-in"), "--json", "{}", "--as-json"]);
        let got: Value = serde_json::from_str(r["body"].as_str().unwrap()).unwrap();
        assert_eq!((&got["method"], &got["cookies"]), (&json!("GET"), &json!(["fresh", "sid"])), "[{engine}] {r}");
        assert_eq!(r["cookies_stored"], json!(["fresh"]), "[{engine}] {r}");
        assert!(r["url"].as_str().is_some_and(|u| u.ends_with("/api/whoami")), "[{engine}] {r}");
        assert!(!r.to_string().contains("f-0123456789"), "[{engine}] the value it set is not shown: {r}");
        assert!(a.text(&["cookies", "--values"]).contains("fresh=f-0123456789"), "[{engine}]");
        assert!(a.err(&["http", "ftp://example.com/x"]).contains("not an http:// or https:// URL"), "[{engine}]");
        // A redirect to another host: the Authorization line written for
        // the first stays there, and a cookie the first answer names for
        // the other host is not stored (so not sent to it either).
        let key = ["--header", "Authorization: Bearer k-123", "--as-json"];
        let r = a.ok(&[&["http", &format!("{base}/api/whoami")], &key[..]].concat());
        let got: Value = serde_json::from_str(r["body"].as_str().unwrap()).unwrap();
        assert_eq!(got["authorization"], true, "[{engine}] {r}");
        let r = a.ok(&[&["http", &format!("{base}/api/hop")], &key[..]].concat());
        let got: Value = serde_json::from_str(r["body"].as_str().unwrap()).unwrap();
        assert!(r["url"].as_str().is_some_and(|u| u.starts_with("http://localhost:")), "[{engine}] {r}");
        assert_eq!((&got["authorization"], &got["cookies"]), (&json!(false), &json!([])), "[{engine}] {r}");
        assert_eq!(r["cookies_stored"], json!([]), "[{engine}] {r}");
        assert!(!a.text(&["cookies", "--all"]).contains("planted"), "[{engine}]");
    }
}

#[test]
fn click_until_gone_loads_a_whole_list() {
    let Some(site) = edge_site() else { return };
    for (engine, bin) in engines() {
        let a = Agent::new("more", engine, bin);
        a.ok(&["goto", &format!("{}/more", site.base)]);
        // By its text, which reads "Loading…" between pages: the clicks
        // follow the button, and wait for each page.
        let r = a.ok(&["click", "--text", "Load more", "--until-gone"]);
        assert_eq!((&r["clicks"], &r["stopped"]), (&json!(3), &json!("the element is gone")), "[{engine}] {r}");
        assert_eq!(a.ok(&["get", "count", "li"]), 12, "[{engine}]");
        a.ok(&["goto", &format!("{}/more", site.base)]);
        let r = a.ok(&["click", "--selector", "#m", "--until-gone", "--max", "2"]);
        assert_eq!(r["clicks"], 2, "[{engine}] {r}");
        assert!(r["stopped"].as_str().is_some_and(|s| s.contains("limit")), "[{engine}] {r}");
        a.ok(&["wait", "--text", "item 9"]);
        // A button that stops doing anything ends the clicks; that is
        // not an error.
        a.ok(&["goto", &format!("{}/counter", site.base)]);
        a.ok(&[
            "eval",
            "() => { next.onclick = () => { if (+n.textContent < 2) n.textContent = +n.textContent + 1 } }",
        ]);
        let r = a.ok(&["click", "--selector", "#next", "--until-gone"]);
        assert_eq!(
            (&r["clicks"], &r["stopped"]),
            (&json!(3), &json!("click 3 changed nothing on the page")),
            "[{engine}] {r}"
        );
        assert!(r.get("no_effect").is_none(), "[{engine}] the stop says it: {r}");
        // By ref, a button that takes itself away (a cookie banner's
        // "Accept"): one click, and the session answers the next command.
        // This spun for good once: a ref read again finds the same element.
        a.ok(&["goto", "data:text/html,<div id=banner><button id=ok onclick=\"banner.remove()\">Accept all</button></div><p>page</p>"]);
        let ax = a.ok(&["ax"]).as_str().unwrap_or_default().to_string();
        let r#ref = ax
            .split("Accept all\" [ref=")
            .nth(1)
            .and_then(|t| t.split(']').next())
            .unwrap_or_else(|| panic!("{ax}"))
            .to_string();
        let started = std::time::Instant::now();
        let r = a.ok(&["click", &r#ref, "--until-gone"]);
        assert_eq!((&r["clicks"], &r["stopped"]), (&json!(1), &json!("the element is gone")), "[{engine}] {r}");
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "[{engine}] {:?}", started.elapsed());
        assert_eq!(a.ok(&["get", "count", "#banner"]), 0, "[{engine}] the session still answers");
    }
}

#[test]
fn locators_reads_states_and_keys() {
    let Some(site) = edge_site() else { return };
    for (engine, bin) in engines() {
        let a = Agent::new("el", engine, bin);
        a.ok(&["goto", &format!("{}/form", site.base)]);
        a.ok(&["fill", "--label", "Email address", "a@b.c"]);
        assert_eq!(a.ok(&["get", "value", "--placeholder", "you@example.com"]), "a@b.c", "[{engine}]");
        a.ok(&["click", "--role", "button", "--name", "Save"]);
        assert_eq!(a.ok(&["get", "text", "#out"]), "saved", "[{engine}]");
        a.ok(&["click", "--role", "button", "--name", "cancel"]);
        assert_eq!(a.ok(&["get", "text", "#out"]), "cancel", "[{engine}] names match case-insensitively");
        a.ok(&["click", "--testid", "save-btn"]);
        assert_eq!(a.ok(&["get", "text", "--role", "listitem", "--nth", "-1"]), "three", "[{engine}]");
        assert_eq!(a.ok(&["get", "count", "li"]), 3);
        // An element's text reads as the page shows it: a list as lines
        // (Lightpanda has no layout, so it reads as textContent there).
        let list = a.ok(&["text", "--selector", "ul"]);
        if engine == "chrome" {
            assert_eq!(list, "one\ntwo\nthree", "[{engine}]");
        } else {
            assert!(list.as_str().is_some_and(|t| t.contains("one") && t.contains("three")), "[{engine}] {list}");
        }
        assert_eq!(a.ok(&["get", "attr", "title", "--role", "link"]), "Go");
        assert_eq!(a.ok(&["get", "style", "color", "--role", "link"]), "rgb(255, 0, 0)");
        assert_eq!(a.ok(&["is", "visible", "#hide"]), false);
        assert_eq!(a.ok(&["is", "hidden", "#hide"]), true);
        assert_eq!(a.ok(&["is", "visible", "#nope", "--timeout-ms", "100"]), false);
        assert_eq!(a.ok(&["is", "disabled", "#off"]), true);
        assert_eq!(a.ok(&["is", "editable", "#ro"]), false);

        let r = a.ok(&["check", "--label", "I accept the terms"]);
        assert_eq!((r["checked"].as_bool(), r["changed"].as_bool()), (Some(true), Some(true)), "[{engine}] {r}");
        assert_eq!(a.ok(&["check", "--label", "I accept the terms"])["changed"], false, "[{engine}] already checked");
        a.ok(&["uncheck", "#news"]);
        assert_eq!(a.ok(&["is", "checked", "#news"]), false);

        a.ok(&["dblclick", "#dbl"]);
        assert_eq!(a.ok(&["get", "text", "#dbl"]), "double true", "[{engine}] a trusted double click");

        a.ok(&["focus", "#em"]);
        assert_eq!(a.ok(&["is", "focused", "#em"]), true);
        a.ok(&["keydown", "Shift"]);
        a.ok(&["press", "a"]);
        a.ok(&["keyup", "Shift"]);
        assert_eq!(a.ok(&["get", "text", "#keys"]), "d:Shift d:A(shift) u:A u:Shift", "[{engine}]");
        a.ok(&["insert-text", "!", "--selector", "#em"]);
        assert_eq!(a.ok(&["get", "value", "#em"]), "a@b.cA!", "[{engine}]");
        a.ok(&["wait", "--idle"]);
    }
}

/// `route --modify` changes a real response; `network --har` records the
/// bodies and `route --har` replays them with the site gone. Chrome only:
/// Lightpanda has no response-stage interception.
#[test]
fn har_replay_and_modified_responses() {
    let Some(site) = edge_site() else { return };
    let base = site.base.clone();
    let Some((engine, bin)) = engines().into_iter().find(|(e, _)| *e == "chrome") else { return };
    {
        let a = Agent::new("har", engine, bin);
        a.ok(&["route", "*/api/echo*", "--modify", "--replace", "widget", "gadget", "--set-header", "X-Seen: 1"]);
        a.ok(&["goto", &format!("{base}/console")]);
        a.ok(&["wait", "--js", "document.title.includes('gadget')"]);
        assert!(
            a.text(&["route"]).contains("*/api/echo* -> modify (1 headers, 1 replacements) (1 hits)"),
            "[{engine}]"
        );
        a.ok(&["unroute"]);
        assert!(a.err(&["route", "*/x", "--set-header", "A: b"]).contains("go with --modify"), "[{engine}]");

        a.ok(&["network", "--clear"]);
        a.ok(&["goto", &format!("{base}/console")]);
        a.ok(&["wait", "--js", "document.title.includes('widget')"]);
        let har = std::env::temp_dir().join(format!("nv-replay-{engine}-{}.har", std::process::id()));
        a.ok(&["network", "--har", har.to_str().unwrap()]);
        let recorded: Value = serde_json::from_slice(&std::fs::read(&har).unwrap()).unwrap();
        let echo = recorded["log"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["request"]["url"].as_str().unwrap_or("").contains("/api/echo"))
            .unwrap_or_else(|| panic!("[{engine}] {recorded}"));
        assert!(echo["response"]["content"]["text"].as_str().unwrap_or("").contains("widget"), "[{engine}] {echo}");

        drop(site);
        a.ok(&["route", "--har", har.to_str().unwrap()]);
        a.ok(&["goto", &format!("{base}/console")]);
        a.ok(&["wait", "--js", "document.title.includes('widget')"]);
        let routes = a.text(&["route"]);
        assert!(routes.starts_with("* -> har (") && !routes.contains(" 0 replayed"), "[{engine}] {routes}");
        a.ok(&["unroute"]);
        // Not in the HAR: aborted by default.
        a.ok(&["route", "--har", har.to_str().unwrap()]);
        let r = a.run(&["goto", &format!("{base}/nowhere")]);
        assert!(r.to_string().contains("BLOCKED_BY_CLIENT") || r["ok"] == false, "[{engine}] {r}");
    }
}

/// `batch`: a form flow in one call; a failing step stops the rest (or not,
/// with --keep-going); a bad line runs nothing.
#[test]
fn a_batch_runs_steps_in_one_call() {
    let Some(site) = edge_site() else { return };
    let base = &site.base;
    for (engine, bin) in engines() {
        let a = Agent::new("batch", engine, bin);
        let dir = std::env::temp_dir();
        let file = |name: &str, text: String| {
            let path = dir.join(format!("nv-batch-{name}-{engine}-{}.txt", std::process::id()));
            std::fs::write(&path, text).unwrap();
            path.display().to_string()
        };
        let flow = file(
            "flow",
            format!(
                "# the form, start to end\ngoto {base}/form\nfill --label 'Email address' a@b.c\n\
                 check --label 'I accept the terms'\nclick --testid save-btn\ntext --selector '#out'\n"
            ),
        );
        let r = a.response(&["batch", &flow]);
        assert_eq!((r["result"]["steps"].as_u64(), r["result"]["ran"].as_u64()), (Some(5), Some(5)), "[{engine}] {r}");
        let results = r["result"]["results"].as_array().unwrap();
        assert_eq!(results.last().unwrap()["result"], "saved", "[{engine}] {r}");
        assert!(r["result"]["state"]["url"].as_str().unwrap().ends_with("/form"), "[{engine}] {r}");
        assert_eq!(a.ok(&["eval", "document.querySelector('[placeholder]').value"]), "a@b.c", "[{engine}]");

        let bad = file("bad", format!("goto {base}/form\nclick --text Nowhere --timeout-ms 300\neval '1 + 1'\n"));
        let r = a.run(&["batch", &bad]);
        assert_eq!((r["ok"].as_bool(), r["result"]["ran"].as_u64()), (Some(false), Some(2)), "[{engine}] {r}");
        assert!(r["error"].as_str().unwrap().starts_with("step 2 (line 2: click --text Nowhere"), "[{engine}] {r}");
        let r = a.run(&["batch", &bad, "--keep-going"]);
        assert_eq!(r["result"]["ran"], 3, "[{engine}] {r}");
        assert_eq!(r["result"]["results"][0]["result"], 2, "[{engine}] {r}");

        let typo = file("typo", format!("goto {base}/console\nclik --text Save\n"));
        let r = a.run(&["batch", &typo]);
        assert!(r["ok"] == false && r["error"].as_str().unwrap().contains("nothing ran"), "[{engine}] {r}");
        assert!(
            a.ok(&["url"])["url"].as_str().unwrap_or_default().ends_with("/form"),
            "[{engine}] the bad file ran nothing"
        );
    }
}

/// The default output (compact): an action that left the page as it was
/// prints `ok`, one that moved it only what changed (after a look, the
/// changed lines of the tree), in a batch too; reads print their value
/// (`text` without the page's scripts); errors as `--raw`. `--output json`
/// gives the full line.
#[test]
fn compact_output_says_only_what_changed() {
    let Some(site) = edge_site() else { return };
    let base = &site.base;
    for (engine, bin) in engines() {
        let a = Agent::new("compact", engine, bin.clone());
        let compact = |args: &[&str]| {
            let mut cmd = Command::new(tool_exe());
            // No flag and no NAVIGERA_OUTPUT: the default.
            cmd.env_remove("NAVIGERA_OUTPUT");
            cmd.args(["--session", &a.session, "--engine", engine]);
            if let Some(bin) = &bin {
                cmd.arg("--chromium").arg(bin);
            }
            let out = cmd.args(args).stdin(Stdio::null()).output().expect("run navigera");
            (
                out.status.success(),
                String::from_utf8_lossy(&out.stdout).trim().to_string(),
                String::from_utf8_lossy(&out.stderr).to_string(),
            )
        };
        let (ok, out, _) = compact(&["goto", &format!("{base}/form")]);
        assert!(ok && out.contains("\"title\":\"form\"") && out.contains("/form"), "[{engine}] {out}");
        assert_eq!(compact(&["fill", "--label", "Email address", "a@b.c"]).1, "ok", "[{engine}]");
        assert_eq!(
            compact(&["check", "--label", "I accept the terms"]).1,
            r#"{"changed":true,"checked":true}"#,
            "[{engine}]"
        );
        // After a look, an action says what it changed on the page: the
        // lines of that tree that are new (`+`), gone (`-`) or different (`~`).
        assert!(compact(&["ax"]).0, "[{engine}]");
        let (ok, out, _) = compact(&["click", "--testid", "save-btn"]);
        assert!(ok && out.starts_with("+ ") && out.contains("saved"), "[{engine}] {out}");
        // Again: nothing it could see changed, so `ok`.
        assert_eq!(compact(&["click", "--testid", "save-btn"]).1, "ok", "[{engine}]");
        assert_eq!(compact(&["text", "--selector", "#out"]).1, "saved", "[{engine}]");
        // A check the change shows is not said again as JSON first.
        let (ok, out, _) = compact(&["uncheck", "--label", "I accept the terms"]);
        assert!(ok && !out.starts_with('{') && out.contains("I accept the terms"), "[{engine}] {out}");
        // A batch's actions say what they changed, as each does alone; a
        // step that changed nothing says nothing.
        let batch = |steps: &str| {
            let mut cmd = Command::new(tool_exe());
            cmd.env_remove("NAVIGERA_OUTPUT");
            cmd.args(["--session", &a.session, "batch", "-"]);
            let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().expect("run navigera");
            std::io::Write::write_all(&mut child.stdin.take().unwrap(), steps.as_bytes()).unwrap();
            let out = child.wait_with_output().expect("batch");
            (out.status.success(), String::from_utf8_lossy(&out.stdout).trim().to_string())
        };
        let (ok, out) = batch("fill --label 'Email address' z\nclick --text Cancel\n");
        assert!(ok && out.contains("cancel") && !out.lines().any(|l| l == "ok"), "[{engine}] {out}");
        assert_eq!(batch("fill --label 'Email address' y\n"), (true, "ok".to_string()), "[{engine}]");
        // Actions one after another report once, at the last: what the run
        // changed since the look, not each step's own change.
        assert!(compact(&["ax"]).0, "[{engine}]");
        let (ok, out) = batch("click --testid save-btn\nclick --text Cancel\nclick --testid save-btn\n");
        assert_eq!((ok, out.as_str()), (true, "+ paragraph: saved\n- paragraph: cancel"), "[{engine}]");
        // A change one step made and the next undid is no change.
        assert_eq!(batch("click --text Cancel\nclick --testid save-btn\n"), (true, "ok".to_string()), "[{engine}]");
        // An entry before a click leaves its report to the click, and is not
        // reported as a change itself.
        let (ok, out) = batch("fill --label 'Email address' q@r.s\nclick --text Cancel\n");
        assert_eq!((ok, out.as_str()), (true, "+ paragraph: cancel\n- paragraph: saved"), "[{engine}]");
        // The step that was to report failed: what the ones before it
        // changed is still said.
        let (ok, out) = batch("click --testid save-btn\nclick --text Nowhere --timeout-ms 300\n");
        assert_eq!((ok, out.as_str()), (false, "+ paragraph: saved\n- paragraph: cancel"), "[{engine}]");
        // The same without a session (a browser for the batch alone).
        let mut cmd = Command::new(tool_exe());
        cmd.env_remove("NAVIGERA_OUTPUT").env_remove("NAVIGERA_SESSION").args(["--engine", engine]);
        if let Some(bin) = &bin {
            cmd.arg("--chromium").arg(bin);
        }
        let mut child =
            cmd.args(["batch", "-"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().expect("run navigera");
        let steps = format!(
            "goto {base}/form\nax\nclick --text Cancel\nclick --testid save-btn\nclick --text Nowhere --timeout-ms 300\n"
        );
        std::io::Write::write_all(&mut child.stdin.take().unwrap(), steps.as_bytes()).unwrap();
        let out = child.wait_with_output().expect("batch");
        let text = String::from_utf8_lossy(&out.stdout);
        let changed: Vec<&str> = text.lines().filter(|l| l.starts_with("+ ")).collect();
        assert!(
            !out.status.success() && changed == ["+ paragraph: saved"] && !text.contains("cancel"),
            "[{engine}] {text}"
        );
        // The page's own script isn't text it shows.
        let (ok, out, _) = compact(&["text", "--selector", "body"]);
        assert!(ok && out.contains("I accept the terms") && !out.contains("addEventListener"), "[{engine}] {out}");
        let (ok, out, err) = compact(&["click", "--text", "Nowhere", "--timeout-ms", "300"]);
        assert!(!ok && out.is_empty() && err.contains("error: click"), "[{engine}] {out} {err}");
        // The full line for programs (this harness runs with NAVIGERA_OUTPUT=json).
        assert_eq!(a.ok(&["fill", "--label", "Email address", "x"])["title"], "form", "[{engine}]");
    }
}

/// WebMCP: the browser navigera starts has it on; a page's tools are listed
/// and called, a tool not marked read-only needs --confirm, and everything
/// says it is the page's. Chrome only (153 and later have WebMCP).
#[test]
fn webmcp_tools_are_listed_and_called() {
    let Some(site) = edge_site() else { return };
    let Some((engine, bin)) = engines().into_iter().find(|(e, _)| *e == "chrome") else { return };
    let a = Agent::new("webmcp", engine, bin);
    a.ok(&["goto", &format!("{}/webmcp", site.base)]);
    let r = a.run(&["webmcp-list"]);
    if r["ok"] == false && r["error"].as_str().unwrap_or_default().contains("has no WebMCP") {
        eprintln!("skipping: {}", r["error"]);
        return;
    }
    let list = a.ok(&["webmcp-list"]);
    assert!(list["untrusted"].as_str().unwrap().starts_with("page-provided"), "{list}");
    let tools = list["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert_eq!(names, ["add", "read_count"], "{list}");
    assert_eq!((tools[0]["read_only"].as_bool(), tools[1]["read_only"].as_bool()), (Some(false), Some(true)), "{list}");
    assert_eq!(tools[0]["input_schema"]["required"][0], "n", "{list}");

    let r = a.ok(&["webmcp-call", "read_count"]);
    assert_eq!(r["output"]["content"][0]["text"], "0", "{r}");
    let e = a.err(&["webmcp-call", "add", "--params", r#"{"n":5}"#]);
    assert!(e.contains("--confirm"), "{e}");
    assert_eq!(a.ok(&["text", "--selector", "#count"]), "0", "refused means not run");
    let r = a.ok(&["webmcp-call", "add", "--params", r#"{"n":5}"#, "--confirm"]);
    assert_eq!(r["output"]["content"][0]["text"], "5", "{r}");
    assert_eq!(a.ok(&["text", "--selector", "#count"]), "5");
    assert!(a.err(&["webmcp-call", "nope"]).contains("it has: "));
}

/// `screencast`: what the tab showed, frame by frame with times, into a
/// directory with a player; it follows the session to a new tab.
#[test]
fn a_screencast_records_frames_with_times_and_follows_the_session_to_a_new_tab() {
    let Some((engine, bin)) = engines().into_iter().find(|(e, _)| *e == "chrome") else { return };
    let a = Agent::new("screencast", engine, bin);
    let dir = tempfile::tempdir().unwrap();
    let rec = dir.path().join("rec");
    let page = "data:text/html,<body><button onclick=\"let n = 0; const t = setInterval(() => { \
        document.body.style.background = 'hsl(' + n * 40 + ',70%,60%)'; if (++n > 5) clearInterval(t); }, 60)\">go</button>";
    assert!(a.err(&["screencast", "stop"]).contains("nothing is recording"));
    a.ok(&["goto", page]);
    let r = a.ok(&["screencast", "start", rec.to_str().unwrap(), "--size", "320x240", "--quality", "50"]);
    assert_eq!(r["screencast"], rec.display().to_string(), "{r}");
    assert!(a.err(&["screencast", "start", rec.to_str().unwrap()]).contains("already recording"));
    a.ok(&["click", "--text", "go"]);
    a.ok(&["wait", "--ms", "600"]);
    a.ok(&["tab-new", page]);
    a.ok(&["click", "--text", "go"]);
    a.ok(&["wait", "--ms", "600"]);
    let r = a.ok(&["screencast", "stop"]);
    let frames = r["frames"].as_u64().unwrap();
    assert!(frames >= 8, "the colour changes of both tabs: {r}");
    let html = std::fs::read_to_string(rec.join("index.html")).unwrap();
    let list = html.split("\"frames\":").nth(1).unwrap();
    let times: Vec<u64> = list.split("\",").skip(1).map_while(|t| t.split(']').next()?.parse().ok()).collect();
    assert_eq!(times.len() as u64, frames, "{list}");
    assert!(times.windows(2).all(|w| w[0] <= w[1]), "times in order: {times:?}");
    assert!(*times.last().unwrap() <= r["ms"].as_u64().unwrap());
    let first = std::fs::read(rec.join("frame-00001.jpg")).unwrap();
    assert_eq!(&first[..2], &[0xFF, 0xD8], "a JPEG");
    assert!(std::fs::metadata(rec.join(format!("frame-{frames:05}.jpg"))).is_ok());
}

/// `clock`: a page's dates and timers at the test's command, in later
/// documents too, while navigera's own waits keep real time.
#[test]
fn the_clock_starts_paused_runs_forward_and_carries_into_new_documents() {
    let Some((engine, bin)) = engines().into_iter().find(|(e, _)| *e == "chrome") else { return };
    let a = Agent::new("clock", engine, bin);
    let page = "data:text/html,<p id=now></p><p id=ticks>0</p><p id=later>waiting</p>\
        <button id=b onclick=\"setTimeout(() => later.textContent = 'clicked', 1000)\">go</button>\
        <script>const show = () => now.textContent = new Date().toISOString(); show(); let n = 0;\
        setInterval(() => { ticks.textContent = ++n; show(); }, 1000);\
        setTimeout(() => later.textContent = 'five minutes', 300000);</script>";
    assert!(a.err(&["clock", "run-for", "1s"]).contains("clock install"), "no clock yet");
    let r = a.ok(&["clock", "install", "--time", "2024-02-02T08:00:00Z", "--paused"]);
    assert_eq!(r["time"], "2024-02-02T08:00:00.000Z", "{r}");
    a.ok(&["goto", page]);
    assert_eq!(
        a.text(&["text", "--selector", "#now"]),
        "2024-02-02T08:00:00.000Z",
        "the page starts at the clock's time"
    );
    // Paused: a second of real time changes nothing; navigera's own wait still ends.
    a.ok(&["wait", "--ms", "1100"]);
    assert_eq!(a.text(&["text", "--selector", "#ticks"]), "0");
    let r = a.ok(&["clock", "run-for", "3s"]);
    assert_eq!((r["fired"].as_u64(), r["time"].as_str()), (Some(3), Some("2024-02-02T08:00:03.000Z")), "{r}");
    assert_eq!(a.text(&["text", "--selector", "#ticks"]), "3");
    assert_eq!(a.text(&["text", "--selector", "#now"]), "2024-02-02T08:00:03.000Z");
    a.ok(&["clock", "run-for", "05:00"]);
    assert_eq!(a.text(&["text", "--selector", "#later"]), "five minutes");
    assert_eq!(a.text(&["text", "--selector", "#ticks"]), "303", "an interval fires as often as it comes");
    // A click whose effect waits for a timer: the click returns, the timer waits for the clock.
    a.ok(&["click", "--selector", "#b"]);
    assert_eq!(a.text(&["text", "--selector", "#later"]), "five minutes");
    let r = a.ok(&["clock", "fast-forward", "10s"]);
    assert_eq!(r["fired"], 2, "each timer that came due fires once: {r}");
    assert_eq!(a.text(&["text", "--selector", "#later"]), "clicked");
    // eval is the page's code: it reads the page's clock.
    a.ok(&["clock", "set", "2030-01-01T00:00:00Z"]);
    assert_eq!(a.ok(&["eval", "new Date().getUTCFullYear()"]), 2030);
    assert_eq!(a.ok(&["eval", "Date.now()"]), 1_893_456_000_000u64);
    // A new document goes on from there, and a resumed clock flows.
    a.ok(&["clock", "resume"]);
    a.ok(&["goto", page]);
    let now = a.text(&["text", "--selector", "#now"]);
    assert!(now.starts_with("2030-01-01T00:00:0"), "{now}");
    let r = a.ok(&["clock", "pause-at", "2030-01-01T01:00:00Z"]);
    assert_eq!((r["paused"].as_bool(), r["time"].as_str()), (Some(true), Some("2030-01-01T01:00:00.000Z")), "{r}");
    a.ok(&["clock", "uninstall"]);
    a.ok(&["goto", page]);
    let year = a.ok(&["eval", "new Date().getUTCFullYear()"]).as_u64().unwrap();
    assert!(year >= 2026, "real time again: {year}");
}
