//! `navigera mcp` end to end: the JSON-RPC handshake an MCP client performs,
//! then real tool calls against Chrome. Pages are `data:` URLs (no network).

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

fn tool_exe() -> std::path::PathBuf {
    let exe = std::env::current_exe().expect("test binary path");
    let name = if cfg!(windows) { "navigera.exe" } else { "navigera" };
    exe.parent().and_then(|p| p.parent()).map(|p| p.join(name)).expect("tool binary path")
}

fn chrome_available() -> bool {
    let found = navigera::browser::resolve_executable_for(None, true).is_some();
    assert!(
        found || std::env::var_os("NAVIGERA_REQUIRE_BROWSER").is_none(),
        "NAVIGERA_REQUIRE_BROWSER is set but no Chrome/Chromium was found"
    );
    found
}

struct Client {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    next_id: u64,
}

impl Client {
    fn start() -> Client {
        Client::start_with(&[])
    }

    /// With these environment variables; a decider is configured only when
    /// they say so.
    fn start_with(env: &[(&str, &str)]) -> Client {
        let mut child = Command::new(tool_exe())
            .arg("mcp")
            .env_remove("NAVIGERA_DECIDER_KEY")
            .env_remove("TYPESAFE_API_KEY")
            .env_remove("NAVIGERA_DECIDER_URL")
            .envs(env.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn navigera mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Client { child, stdin, stdout, next_id: 1 }
    }

    fn send(&mut self, message: &Value) {
        writeln!(self.stdin, "{message}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        let reply: Value = serde_json::from_str(line.trim()).unwrap_or_else(|e| panic!("not JSON ({e}): {line}"));
        assert_eq!(reply["id"], id, "{reply}");
        assert_eq!(reply["jsonrpc"], "2.0");
        reply
    }

    /// `tools/call`, returning (text content, isError, the whole result).
    fn call(&mut self, name: &str, arguments: Value) -> (String, bool, Value) {
        let reply = self.request("tools/call", json!({"name": name, "arguments": arguments}));
        let result = reply["result"].clone();
        let text = result["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|c| c["type"] == "text")
            .filter_map(|c| c["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        (text, result["isError"].as_bool().unwrap_or(false), result)
    }
}

fn page(html: &str) -> String {
    format!("data:text/html,{}", html.replace(' ', "%20").replace('#', "%23"))
}

#[test]
fn mcp_handshake_without_a_browser() {
    let mut c = Client::start();
    let init = c.request(
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}),
    );
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["result"]["serverInfo"]["name"], "navigera");
    assert!(init["result"]["capabilities"]["tools"].is_object());
    c.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));

    let unknown = c.request("initialize", json!({"protocolVersion": "1999-01-01"}));
    assert_eq!(unknown["result"]["protocolVersion"], "2025-11-25", "offers its newest revision");

    let list = c.request("tools/list", json!({}));
    let names: Vec<&str> =
        list["result"]["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
    for want in
        ["browser_navigate", "browser_snapshot", "browser_click", "browser_fill", "browser_run", "browser_close"]
    {
        assert!(names.contains(&want), "{want} missing from {names:?}");
    }
    let schemas = serde_json::to_string(&list["result"]["tools"]).unwrap();
    assert!(schemas.len() < 8_000, "tool list stays small for the model's context: {} bytes", schemas.len());

    assert!(c.request("ping", json!({}))["result"].is_object());
    assert_eq!(c.request("resources/list", json!({}))["error"]["code"], -32601);
    let (text, is_error, _) = c.call("browser_teleport", json!({}));
    assert!(is_error && text.contains("unknown tool"), "{text}");

    drop(c.stdin);
    assert!(c.child.wait().unwrap().success());
}

#[test]
fn mcp_drives_a_page() {
    if !chrome_available() {
        eprintln!("skipping: no Chrome/Chromium/Edge binary installed");
        return;
    }
    let mut c = Client::start();
    c.request("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t"}}));
    c.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));

    let html = page(
        "<title>Form</title><h1>Order</h1>\
         <label>Name <input id=name></label>\
         <select id=c aria-label=Country><option>Canada</option><option>Mexico</option></select>\
         <button onclick=\"document.getElementById('out').textContent='Thanks '+document.getElementById('name').value+' from '+document.getElementById('c').value\">Send</button>\
         <button onclick=\"alert('careful')\">Warn</button>\
         <p id=out></p>",
    );
    let (text, is_error, _) = c.call("browser_navigate", json!({"url": html}));
    assert!(!is_error && text.starts_with("Page: Form"), "{text}");

    let (tree, is_error, _) = c.call("browser_snapshot", json!({}));
    assert!(!is_error, "{tree}");
    let ref_of = |tree: &str, needle: &str| -> u64 {
        let line = tree.lines().find(|l| l.contains(needle)).unwrap_or_else(|| panic!("{needle} not in {tree}"));
        let at = line.find("ref=").unwrap() + 4;
        line[at..].chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap()
    };
    let name = ref_of(&tree, "textbox \"Name\"");
    let country = ref_of(&tree, "combobox \"Country\"");
    let send = ref_of(&tree, "button \"Send\"");

    // One call: fill, select, click, then the snapshot that shows the result.
    let (text, is_error, _) = c.call(
        "browser_run",
        json!({"steps": [
            {"op": "fill", "ref": name, "value": "Ada"},
            {"op": "select", "ref": country, "value": ["Mexico"]},
            {"op": "click", "ref": send},
        ]}),
    );
    assert!(!is_error, "{text}");
    assert!(text.contains("Thanks Ada from Mexico"), "browser_run ends with a snapshot: {text}");

    // A failing step stops the run and names itself.
    let (text, is_error, _) = c.call(
        "browser_run",
        json!({"steps": [{"op": "click", "selector": "#missing", "timeout_ms": 300}, {"op": "click", "ref": send}]}),
    );
    assert!(is_error && text.contains("step 1 (click) failed"), "{text}");

    // Dialogs are answered and reported; snapshot:true returns the tree too.
    let (text, is_error, _) = c.call("browser_click", json!({"text": "Warn", "snapshot": true}));
    assert!(!is_error, "{text}");
    assert!(text.contains("Dialog (alert) \"careful\" was accepted"), "{text}");
    assert!(text.contains("button \"Send\""), "snapshot:true appends the tree: {text}");

    let (text, is_error, _) = c.call("browser_evaluate", json!({"expression": "() => document.title.length"}));
    assert!(!is_error && text.trim() == "4", "{text}");

    let (_, is_error, result) = c.call("browser_screenshot", json!({}));
    assert!(!is_error, "{result}");
    let image = result["content"].as_array().unwrap().iter().find(|c| c["type"] == "image").expect("an image");
    assert_eq!(image["mimeType"], "image/png");
    assert!(image["data"].as_str().unwrap().len() > 1000);

    let (text, is_error, _) = c.call("browser_tabs", json!({"action": "list"}));
    assert!(!is_error && text.contains("Form"), "{text}");

    // A step can't reach a file outside the working directory, or the
    // browser's own pages: not as a route's body, not as a saved state.
    let secret = std::env::temp_dir().join(format!("navigera-mcp-secret-{}", std::process::id()));
    std::fs::write(&secret, "TOPSECRET").unwrap();
    for step in [
        json!({"op": "route", "pattern": "*/leak", "body_file": secret.display().to_string()}),
        json!({"op": "state", "action": "save", "path": secret.with_extension("json").display().to_string()}),
    ] {
        let (text, is_error, _) = c.call("browser_run", json!({"steps": [step]}));
        assert!(is_error && text.contains("outside the working directory"), "{text}");
    }
    assert!(!secret.with_extension("json").exists());
    std::fs::remove_file(&secret).ok();
    let (text, is_error, _) = c.call("browser_navigate", json!({"url": "chrome://version"}));
    assert!(is_error && text.contains("browser's own pages"), "{text}");

    // Closing is not the end: the next call starts a fresh browser.
    let (text, is_error, _) = c.call("browser_close", json!({}));
    assert!(!is_error, "{text}");
    let (text, is_error, _) = c.call("browser_navigate", json!({"url": page("<title>Again</title>ok")}));
    assert!(!is_error && text.contains("Again"), "{text}");

    drop(c.stdin);
    assert!(c.child.wait().unwrap().success(), "EOF shuts the server and its browser down");
}

/// Between calls the server reads a large page a navigation loaded ahead
/// of its snapshot while the agent thinks: the first `browser_snapshot`
/// then patches that read instead of capturing the page (and shows what
/// changed since). Calls sent at once leave the capture to the snapshot.
#[test]
fn action_tools_say_what_changed_unless_asked_for_the_state() {
    if !chrome_available() {
        eprintln!("skipping: no Chrome/Chromium/Edge binary installed");
        return;
    }
    let mut c = Client::start();
    c.request("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t"}}));
    c.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));

    let html = page(
        "<title>Shop</title><h1>Cart</h1>\
         <button onclick=\"document.getElementById('n').textContent=parseInt(document.getElementById('n').textContent)+1\">Add</button>\
         <p>count: <span id=n>0</span></p>",
    );
    let (text, is_error, _) = c.call("browser_navigate", json!({"url": html}));
    assert!(!is_error && text.starts_with("Page: Shop"), "{text}");

    let (tree, is_error, _) = c.call("browser_snapshot", json!({}));
    assert!(!is_error, "{tree}");
    let add = tree.lines().find(|l| l.contains("button \"Add\"")).expect("Add button");
    let at = add.find("ref=").unwrap() + 4;
    let add_ref: u64 = add[at..].chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap();

    // An action says what changed, not the whole state again.
    let (text, is_error, _) = c.call("browser_click", json!({"ref": add_ref}));
    assert!(!is_error, "{text}");
    assert!(text.contains('1') && !text.contains("Page: Shop"), "the change, not the state: {text}");

    // state: true keeps the full page state.
    let (text, is_error, _) = c.call("browser_click", json!({"ref": add_ref, "state": true}));
    assert!(!is_error, "{text}");
    assert!(text.starts_with("Page: Shop"), "state:true keeps the full state: {text}");

    // The WebMCP tools are deferred: listed by browser_more, callable by name.
    let (text, is_error, _) = c.call("browser_more", json!({}));
    assert!(!is_error && text.contains("browser_webmcp_list") && text.contains("browser_webmcp_call"), "{text}");
    let (text, _, _) = c.call("browser_webmcp_list", json!({}));
    assert!(!text.is_empty(), "webmcp-list answers on a page with no WebMCP tools");

    let _ = c.call("browser_close", json!({}));
}

#[test]
fn mcp_reads_a_large_page_ahead_while_the_agent_thinks() {
    if !chrome_available() {
        eprintln!("skipping: no Chrome/Chromium/Edge binary installed");
        return;
    }
    let trace = std::env::temp_dir().join(format!("nv-mcp-ahead-{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&trace);
    let mut c = Client::start_with(&[("NAVIGERA_CDP_TRACE", trace.to_str().unwrap()), ("NAVIGERA_PRE_READ", "1")]);
    c.request("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t"}}));
    c.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    let captures = || {
        std::fs::read_to_string(&trace)
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains("\"DOMSnapshot.captureSnapshot\""))
            .count()
    };
    let think = || std::thread::sleep(std::time::Duration::from_millis(1500));
    // 1,000 rows: past the 2,500 nodes a page needs to be read ahead.
    let rows = page(
        "<title>Rows</title><table><tbody id=rows></tbody></table><script>\
         for (let i = 0; i < 1000; i++) { const r = rows.insertRow(); r.id = 'r' + i; \
         r.insertCell().textContent = 'SKU-' + String(i).padStart(5, '0'); r.insertCell().textContent = 'Item ' + i; }\
         </script>",
    );
    think();
    let (text, is_error, _) = c.call("browser_navigate", json!({"url": rows}));
    assert!(!is_error, "{text}");
    think();
    let before = captures();
    assert!(before >= 1, "the page was read ahead while the agent thought");
    let (text, is_error, _) = c.call(
        "browser_evaluate",
        json!({"expression": "() => { document.querySelector('#r5 td:nth-child(2)').textContent = 'changed after navigate'; return 1 }"}),
    );
    assert!(!is_error, "{text}");
    think();
    let (tree, is_error, _) = c.call("browser_snapshot", json!({}));
    assert!(!is_error && tree.contains("changed after navigate") && tree.contains("SKU-00000"), "{tree}");
    assert_eq!(captures(), before, "the first snapshot patched what was read ahead");
    // Calls sent at once: no read ahead, the snapshot captures.
    for _ in 0..3 {
        c.call("browser_evaluate", json!({"expression": "() => 1"}));
    }
    c.call("browser_navigate", json!({"url": rows}));
    let before = captures();
    let (tree, _, _) = c.call("browser_snapshot", json!({}));
    assert!(tree.contains("SKU-00000"), "{tree}");
    assert_eq!(captures(), before + 1, "a script's snapshot captures for itself");
    drop(c.stdin);
    assert!(c.child.wait().unwrap().success());
    let _ = std::fs::remove_file(&trace);
}

/// The decision tools exist only when a decider is configured, so the
/// default list stays the one agents know.
#[test]
fn decision_tools_appear_only_with_a_decider() {
    let names = |env: &[(&str, &str)]| {
        let mut c = Client::start_with(env);
        c.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t"}}),
        );
        let tools = c.request("tools/list", json!({}))["result"]["tools"].clone();
        let _ = c.child.kill();
        tools
    };
    let plain = names(&[]);
    let deciding = names(&[("NAVIGERA_DECIDER_URL", "http://127.0.0.1:9/v1/systemone")]);
    let has = |tools: &Value, name: &str| tools.as_array().unwrap().iter().any(|t| t["name"] == name);
    assert!(!has(&plain, "browser_find") && !has(&plain, "browser_check"));
    assert!(has(&deciding, "browser_find") && has(&deciding, "browser_check"));
    let click =
        |tools: &Value| tools.as_array().unwrap().iter().find(|t| t["name"] == "browser_click").unwrap().clone();
    assert!(click(&plain)["inputSchema"]["properties"].get("find").is_none());
    assert!(click(&deciding)["inputSchema"]["properties"].get("find").is_some());
}
