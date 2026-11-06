//! `--allow-domains` end to end, on Chrome: navigation and requests to
//! other hosts are refused at the Fetch request stage, before route
//! rules, on every redirect hop; `--allow-domains-docs-only` checks
//! top-level navigation only.
//!
//! Skipped (not failed) when no Chrome is found, like the other browser
//! tests: `NAVIGERA_REQUIRE_BROWSER=1` turns a missing one into a failure.
//! Two hostnames for one local server (`127.0.0.1` allowed, `localhost`
//! denied) stand in for two sites.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};

use serde_json::{json, Value};

fn tool_exe() -> String {
    env!("CARGO_BIN_EXE_navigera").to_string()
}

fn chrome_available() -> bool {
    let found = navigera::browser::resolve_executable_for(None, true).is_some();
    assert!(found || std::env::var_os("NAVIGERA_REQUIRE_BROWSER").is_none(), "no Chrome found");
    found
}

fn python() -> &'static str {
    if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}

struct Site {
    child: Child,
    port: u16,
}

impl Drop for Site {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn site() -> Site {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/allow_site.py");
    let mut child = Command::new(python()).arg(script).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let port: u16 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
    Site { child, port }
}

/// A `serve` process: one JSON command per line in, one JSON response
/// per line out.
struct Serve {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl Serve {
    fn spawn(args: &[&str], allow_env: Option<&str>) -> Serve {
        let mut cmd = Command::new(tool_exe());
        cmd.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        if let Some(list) = allow_env {
            cmd.env("NAVIGERA_ALLOW_DOMAINS", list);
        }
        let mut child = cmd.spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Serve { child, stdin, stdout }
    }

    fn call(&mut self, id: u64, op: &str, extra: Value) -> Value {
        let mut req = json!({ "id": id, "op": op });
        for (k, v) in extra.as_object().unwrap() {
            req[k] = v.clone();
        }
        writeln!(self.stdin, "{}", req).unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(line.trim()).unwrap()
    }

    fn quit(&mut self) {
        let _ = self.call(999, "quit", json!({}));
        let _ = self.child.wait();
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn eval_fetch(s: &mut Serve, id: u64, url: &str) -> String {
    let expr = format!(
        "(async () => {{ try {{ const r = await fetch({url:?}); return 'ok:' + await r.text(); }} catch (e) {{ return 'failed'; }} }})()"
    );
    let r = s.call(id, "eval", json!({ "expression": expr }));
    assert!(r["ok"].as_bool().unwrap(), "eval failed: {r}");
    match &r["result"] {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

#[test]
fn strict_refuses_other_hosts_and_docs_only_does_not() {
    if !chrome_available() {
        return;
    }
    let site = site();
    let allowed = format!("http://127.0.0.1:{}/", site.port);
    let denied = format!("http://localhost:{}/other", site.port);
    let denied_data = format!("http://localhost:{}/data", site.port);

    // Strict: the allowed host loads, the other host is refused by name,
    // and a redirect hop to it is refused too.
    let mut s = Serve::spawn(&["serve", "--allow-domains", "127.0.0.1"], None);
    let r = s.call(1, "goto", json!({ "url": allowed }));
    assert!(r["ok"].as_bool().unwrap(), "allowed goto failed: {r}");
    assert!(eval_fetch(&mut s, 2, &denied_data).contains("failed"));
    let r = s.call(3, "goto", json!({ "url": denied }));
    assert!(!r["ok"].as_bool().unwrap(), "denied goto succeeded: {r}");
    assert!(
        r["error"].as_str().unwrap().contains("refused by --allow-domains: localhost"),
        "refusal must name the host and the flag: {r}"
    );
    let r = s.call(4, "goto", json!({ "url": format!("http://127.0.0.1:{}/redir", site.port) }));
    assert!(!r["ok"].as_bool().unwrap(), "redirect to a denied host succeeded: {r}");
    assert!(
        r["error"].as_str().unwrap().contains("refused by --allow-domains: localhost"),
        "a redirect hop's refusal names its target: {r}"
    );
    // A fulfill route for everything cannot override the denial.
    let r = s.call(5, "route", json!({ "pattern": "*", "status": 200, "body": "SPOOFED" }));
    assert!(r["ok"].as_bool().unwrap(), "route failed: {r}");
    let r = s.call(6, "goto", json!({ "url": denied }));
    assert!(!r["ok"].as_bool().unwrap(), "a route overrode the allowlist: {r}");
    s.quit();

    // Docs-only: subresource fetches to the other host pass, top-level
    // navigation to it is still refused.
    let mut s = Serve::spawn(&["serve", "--allow-domains", "127.0.0.1", "--allow-domains-docs-only"], None);
    let r = s.call(1, "goto", json!({ "url": allowed }));
    assert!(r["ok"].as_bool().unwrap(), "allowed goto failed: {r}");
    assert!(eval_fetch(&mut s, 2, &denied_data).contains("ok:DATA-OK"));
    let r = s.call(3, "goto", json!({ "url": denied }));
    assert!(!r["ok"].as_bool().unwrap(), "docs-only let a top-level navigation through: {r}");
    s.quit();

    // The environment variable sets the same policy.
    let mut s = Serve::spawn(&["serve"], Some("127.0.0.1"));
    let r = s.call(1, "goto", json!({ "url": denied }));
    assert!(!r["ok"].as_bool().unwrap(), "env allowlist let a denied host through: {r}");
    s.quit();
}

/// A popup's first request goes out before navigera has attached to the
/// popup, so the tab's own interception can't refuse it: the browser-wide
/// pause has to. Both ways a page opens one, in both modes (a popup's
/// document is a top-level navigation): nothing reaches the other host,
/// and `status` names it.
#[test]
fn popups_to_other_hosts_are_refused_before_their_first_request() {
    if !chrome_available() {
        return;
    }
    let site = site();
    let allowed = format!("http://127.0.0.1:{}/", site.port);
    let hits = format!("http://127.0.0.1:{}/hits", site.port);
    for docs_only in [false, true] {
        let mut args = vec!["serve", "--allow-domains", "127.0.0.1"];
        if docs_only {
            args.push("--allow-domains-docs-only");
        }
        let mut s = Serve::spawn(&args, None);
        let r = s.call(1, "goto", json!({ "url": allowed }));
        assert!(r["ok"].as_bool().unwrap(), "allowed goto failed: {r}");
        let sink = format!("http://localhost:{}/sink", site.port);
        let opens = [
            format!("window.open({:?}); true", format!("{sink}?open")),
            format!(
                "(() => {{ const a = document.createElement('a'); a.href = {:?}; a.target = '_blank'; \
                 document.body.append(a); a.click(); return true; }})()",
                format!("{sink}?blank")
            ),
        ];
        for (i, open) in opens.iter().enumerate() {
            let r = s.call(10 + i as u64, "eval", json!({ "expression": open }));
            assert!(r["ok"].as_bool().unwrap(), "eval failed: {r}");
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        // The popups are tabs now; the count is read from the first one.
        let r = s.call(19, "tab-select", json!({ "index": 0 }));
        assert!(r["ok"].as_bool().unwrap(), "tab-select failed: {r}");
        assert_eq!(eval_fetch(&mut s, 20, &hits), "ok:0", "a popup reached the other host (docs-only: {docs_only})");
        let r = s.call(21, "status", json!({}));
        let denied = r["result"]["allow_domains"]["denied"].to_string();
        assert!(denied.contains("localhost"), "status must name the refused host: {r}");
        s.quit();
    }
}
