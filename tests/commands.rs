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
            cookies.contains("theme=dark") && cookies.contains("token=t1; ") && cookies.contains("HttpOnly"),
            "[{engine}] {cookies}"
        );
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
        assert!(b.text(&["cookies"]).contains("sid=s3cret"), "[{engine}]");
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

/// `--compact`: an action that left the page as it was prints `ok`, one that
/// moved it only what changed; reads print their value; errors as `--raw`.
#[test]
fn compact_output_says_only_what_changed() {
    let Some(site) = edge_site() else { return };
    let base = &site.base;
    for (engine, bin) in engines() {
        let a = Agent::new("compact", engine, bin.clone());
        let compact = |args: &[&str]| {
            let mut cmd = Command::new(tool_exe());
            cmd.args(["--session", &a.session, "--engine", engine, "--compact"]);
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
        assert_eq!(compact(&["click", "--testid", "save-btn"]).1, "ok", "[{engine}]");
        assert_eq!(compact(&["text", "--selector", "#out"]).1, "saved", "[{engine}]");
        let (ok, out, err) = compact(&["click", "--text", "Nowhere", "--timeout-ms", "300"]);
        assert!(!ok && out.is_empty() && err.contains("error: click"), "[{engine}] {out} {err}");
        // The full envelope is still there without the flag.
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
