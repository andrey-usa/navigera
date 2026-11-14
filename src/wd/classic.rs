//! Classic W3C WebDriver over HTTP: Safari through `safaridriver`, and any
//! WebDriver server (`--webdriver <url>`: geckodriver, chromedriver,
//! msedgedriver, a Selenium Grid). With `--bidi` the session asks for a
//! `webSocketUrl` and runs over BiDi instead (see [`super::bidi`]).
//!
//! The HTTP client is a few lines over one kept-alive TCP connection: the
//! driver is on this machine or a network next to it, and no crate is
//! needed for that.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

use super::{wrap, Wait, Wire, CONSOLE_HOOK, DIALOG_HOOK, LISTEN};

/// The key WebDriver names an element reference by.
const ELEMENT_KEY: &str = "element-6066-11e4-a52f-4f735466cecf";

/// The most an answer may hold (a full-page screenshot of a long page).
const BODY_MAX: usize = 256 << 20;

/// A WebDriver server: where it is, and the connection to it.
struct Http {
    host: String,
    /// `/wd/hub` for a grid, empty for a driver.
    base: String,
    auth: Option<String>,
    conn: Option<BufReader<TcpStream>>,
}

impl Http {
    fn new(url: &str) -> Result<Self> {
        let rest = url.strip_prefix("http://").ok_or_else(|| {
            if url.starts_with("https://") {
                anyhow!("--webdriver {}: https:// servers are not served yet; run a local tunnel to it, or use the server's http:// address", mask(url))
            } else {
                anyhow!("--webdriver {}: give the server's http:// URL, like http://127.0.0.1:4444", mask(url))
            }
        })?;
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], rest[i..].trim_end_matches('/')),
            None => (rest, ""),
        };
        let (auth, host) = match authority.rsplit_once('@') {
            Some((user, host)) => {
                use base64::Engine;
                (Some(base64::engine::general_purpose::STANDARD.encode(user)), host)
            }
            None => (None, authority),
        };
        Ok(Self { host: host.to_string(), base: path.to_string(), auth, conn: None })
    }

    fn stream(&mut self, timeout: Duration) -> Result<&mut BufReader<TcpStream>> {
        if self.conn.is_none() {
            let addr = std::net::ToSocketAddrs::to_socket_addrs(&self.host)
                .with_context(|| format!("WebDriver server {}", self.host))?
                .next()
                .ok_or_else(|| anyhow!("WebDriver server {}: no address", self.host))?;
            let s = TcpStream::connect_timeout(&addr, timeout.min(Duration::from_secs(10)))
                .with_context(|| format!("could not connect to the WebDriver server at {}", self.host))?;
            s.set_nodelay(true).ok();
            self.conn = Some(BufReader::new(s));
        }
        let conn = self.conn.as_mut().unwrap();
        conn.get_ref().set_read_timeout(Some(timeout)).ok();
        conn.get_ref().set_write_timeout(Some(timeout)).ok();
        Ok(conn)
    }

    /// One request; the answer's `value`. A connection the server closed
    /// is made again once, for a request that never reached it.
    fn request(&mut self, method: &str, path: &str, body: Option<&Value>, timeout: Duration) -> Result<Value> {
        let body = body.map(|b| b.to_string());
        let mut head =
            format!("{method} {}{path} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\n", self.base, self.host);
        if let Some(a) = &self.auth {
            head.push_str(&format!("Authorization: Basic {a}\r\n"));
        }
        match &body {
            Some(b) => head.push_str(&format!(
                "Content-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\n\r\n",
                b.len()
            )),
            None => head.push_str("Content-Length: 0\r\n\r\n"),
        }
        for attempt in 0..2 {
            let conn = self.stream(timeout)?;
            let sent = conn.get_mut().write_all(head.as_bytes()).and_then(|_| match &body {
                Some(b) => conn.get_mut().write_all(b.as_bytes()),
                None => Ok(()),
            });
            if sent.is_err() && attempt == 0 {
                self.conn = None;
                continue;
            }
            sent.with_context(|| format!("{method} {path}: the WebDriver server took no request"))?;
            match read_response(self.conn.as_mut().unwrap()) {
                Ok((status, text, keep)) => {
                    if !keep {
                        self.conn = None;
                    }
                    let v: Value = serde_json::from_str(&text).map_err(|_| {
                        anyhow!(
                            "{method} {path}: the WebDriver server answered {status} with no JSON: {}",
                            text.chars().take(200).collect::<String>()
                        )
                    })?;
                    let mut v = v;
                    let value = v.get_mut("value").map(Value::take).unwrap_or(Value::Null);
                    if status >= 400 || value.get("error").is_some_and(|e| e.is_string()) {
                        let code = value["error"].as_str().unwrap_or("error");
                        let msg = value["message"].as_str().unwrap_or("");
                        bail!("{code}: {}", msg.lines().next().unwrap_or(msg));
                    }
                    return Ok(value);
                }
                // An idle kept connection the server had closed: nothing was
                // read, so the request never ran.
                Err(e) if attempt == 0 && format!("{e:#}").contains("closed before") => {
                    self.conn = None;
                }
                Err(e) => {
                    self.conn = None;
                    return Err(e.context(format!("{method} {path}")));
                }
            }
        }
        bail!("{method} {path}: the WebDriver server closed the connection twice")
    }
}

/// The URL as shown: any user and key in it masked.
pub fn mask(url: &str) -> String {
    match (url.find("://"), url.rfind('@')) {
        (Some(s), Some(at)) if at > s => format!("{}://***@{}", &url[..s], &url[at + 1..]),
        _ => url.to_string(),
    }
}

/// (status, body, keep-alive) of one HTTP/1.1 answer.
fn read_response(r: &mut BufReader<TcpStream>) -> Result<(u16, String, bool)> {
    let mut line = String::new();
    if r.read_line(&mut line)? == 0 {
        bail!("the connection closed before an answer");
    }
    let status: u16 = line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| anyhow!("not an HTTP answer: {line:?}"))?;
    let (mut length, mut chunked, mut keep) = (None, false, !line.starts_with("HTTP/1.0"));
    loop {
        line.clear();
        if r.read_line(&mut line)? == 0 {
            bail!("the connection closed in the answer's headers");
        }
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            let v = v.trim();
            match k.trim().to_ascii_lowercase().as_str() {
                "content-length" => length = v.parse::<usize>().ok(),
                "transfer-encoding" => chunked = v.eq_ignore_ascii_case("chunked"),
                "connection" => keep = !v.eq_ignore_ascii_case("close"),
                _ => {}
            }
        }
    }
    let mut body = Vec::new();
    if chunked {
        loop {
            line.clear();
            r.read_line(&mut line)?;
            let size = usize::from_str_radix(line.trim().split(';').next().unwrap_or("0"), 16)
                .map_err(|_| anyhow!("bad chunk size {line:?}"))?;
            if size == 0 {
                line.clear();
                r.read_line(&mut line)?;
                break;
            }
            if body.len() + size > BODY_MAX {
                bail!("the answer is over {} MB", BODY_MAX >> 20);
            }
            let at = body.len();
            body.resize(at + size, 0);
            r.read_exact(&mut body[at..])?;
            line.clear();
            r.read_line(&mut line)?;
        }
    } else if let Some(n) = length {
        if n > BODY_MAX {
            bail!("the answer is over {} MB", BODY_MAX >> 20);
        }
        body.resize(n, 0);
        r.read_exact(&mut body)?;
    } else {
        r.take(BODY_MAX as u64).read_to_end(&mut body)?;
        keep = false;
    }
    Ok((status, String::from_utf8_lossy(&body).into_owned(), keep))
}

/// A WebDriver session: the server, the session id, the driver process
/// navigera started (safaridriver), and what the browser said it is.
pub struct Session {
    http: Http,
    id: String,
    caps: Value,
    endpoint: String,
    driver: Option<Child>,
    ended: bool,
}

impl Session {
    fn new(
        mut http: Http,
        endpoint: String,
        mut caps: Value,
        driver: Option<Child>,
        timeout: Duration,
    ) -> Result<Self> {
        let bound = timeout.max(Duration::from_secs(60));
        let body = json!({ "capabilities": { "alwaysMatch": caps } });
        let v = match http.request("POST", "/session", Some(&body), bound) {
            // A driver that doesn't take the prompt behavior: its own default.
            Err(e) if format!("{e:#}").contains("nhandledPromptBehavior") => {
                if let Some(c) = caps.as_object_mut() {
                    c.remove("unhandledPromptBehavior");
                }
                http.request("POST", "/session", Some(&json!({ "capabilities": { "alwaysMatch": caps } })), bound)
            }
            other => other,
        }
        .context("the WebDriver server refused a session")?;
        let id = v["sessionId"].as_str().ok_or_else(|| anyhow!("the WebDriver server gave no session id"))?.to_string();
        Ok(Self { http, id, caps: v["capabilities"].clone(), endpoint, driver, ended: false })
    }

    fn cmd(&mut self, method: &str, path: &str, body: Option<Value>, timeout: Duration) -> Result<Value> {
        let path = format!("/session/{}{path}", self.id);
        self.http.request(method, &path, body.as_ref(), timeout)
    }

    pub fn describe(&self) -> Value {
        json!({
            "name": self.caps["browserName"].as_str().unwrap_or(""),
            "version": self.caps["browserVersion"].as_str().unwrap_or(""),
            "platform": self.caps["platformName"].as_str().unwrap_or(""),
        })
    }

    pub fn endpoint(&self) -> String {
        self.endpoint.clone()
    }

    pub fn end(&mut self) {
        if !self.ended {
            self.ended = true;
            let _ = self.http.request("DELETE", &format!("/session/{}", self.id), None, Duration::from_secs(10));
        }
        if let Some(mut child) = self.driver.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.end();
    }
}

/// A free port on this machine, for a driver navigera starts.
fn free_port() -> Result<u16> {
    let l = std::net::TcpListener::bind("127.0.0.1:0").context("no free port for the driver")?;
    Ok(l.local_addr()?.port())
}

/// Start `exe` (a WebDriver server that takes `--port`) and wait until it
/// answers `/status`.
fn start_driver(exe: &str, args: &[String], timeout: Duration) -> Result<(Child, String)> {
    let port = free_port()?;
    let mut child = std::process::Command::new(exe)
        .arg(format!("--port={port}"))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("could not start {exe}"))?;
    let url = format!("http://127.0.0.1:{port}");
    let mut http = Http::new(&url)?;
    let started = Instant::now();
    let bound = timeout.max(Duration::from_secs(20));
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            bail!("{exe} exited ({status}) before it served WebDriver");
        }
        if let Ok(v) = http.request("GET", "/status", None, Duration::from_secs(2)) {
            if v["ready"].as_bool() != Some(false) {
                return Ok((child, url));
            }
        }
        if started.elapsed() > bound {
            let _ = child.kill();
            bail!("{exe} did not answer on port {port} in {} s", bound.as_secs());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Safari through safaridriver (macOS). `preview`: Safari Technology
/// Preview's own driver. Capabilities may add `platformName: "iOS"` with
/// `safari:useSimulator` or a device (Apple's own capabilities).
pub fn launch_safari(
    preview: bool,
    exe: Option<&str>,
    caps: Value,
    bidi: bool,
    timeout: Duration,
) -> Result<Box<dyn Wire>> {
    let default = if preview {
        "/Applications/Safari Technology Preview.app/Contents/MacOS/safaridriver"
    } else {
        "/usr/bin/safaridriver"
    };
    let exe = exe.unwrap_or(default);
    if !std::path::Path::new(exe).exists() {
        bail!(
            "{exe} is not here: Safari's driver ships with macOS (Safari Technology Preview has its own); \
             on another system, drive a Mac's safaridriver with --webdriver http://<mac>:<port>"
        );
    }
    let (child, url) = start_driver(exe, &[], timeout).map_err(|e| {
        e.context("safaridriver did not start: run `safaridriver --enable` once (it asks for an administrator password), and turn on Develop > Allow Remote Automation in Safari")
    })?;
    let mut always = json!({ "browserName": if preview { "Safari Technology Preview" } else { "safari" } });
    if let (Some(a), Some(extra)) = (always.as_object_mut(), caps.as_object()) {
        for (k, v) in extra {
            a.insert(k.clone(), v.clone());
        }
    }
    start(Http::new(&url)?, url, always, bidi, Some(child), timeout)
}

/// Any WebDriver server at `url`.
pub fn connect(url: &str, caps: Value, bidi: bool, timeout: Duration) -> Result<Box<dyn Wire>> {
    start(Http::new(url)?, mask(url), caps, bidi, None, timeout)
}

fn start(
    http: Http,
    endpoint: String,
    mut caps: Value,
    bidi: bool,
    driver: Option<Child>,
    timeout: Duration,
) -> Result<Box<dyn Wire>> {
    if let Some(a) = caps.as_object_mut() {
        // Prompts stay open for navigera to answer by the session's policy
        // (a driver that accepts them itself would answer before it).
        a.entry("unhandledPromptBehavior").or_insert(json!("ignore"));
        if bidi {
            a.insert("webSocketUrl".into(), json!(true));
        }
    }
    let session = Session::new(http, endpoint, caps, driver, timeout)?;
    if bidi {
        let Some(ws) = session.caps["webSocketUrl"].as_str().map(str::to_string) else {
            bail!(
                "--bidi: this browser's driver gave no BiDi endpoint (no webSocketUrl in its capabilities); drop --bidi to use classic WebDriver"
            );
        };
        return Ok(Box::new(super::bidi::Bidi::over(session, &ws, timeout)?));
    }
    let window = String::new();
    let mut me = Classic { s: session, window, accept_dialogs: true, prompt_text: None, dialogs: Vec::new() };
    me.window =
        me.s.cmd("GET", "/window", None, timeout).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    Ok(Box::new(me))
}

/// A classic session's ops.
pub struct Classic {
    s: Session,
    window: String,
    accept_dialogs: bool,
    prompt_text: Option<String>,
    dialogs: Vec<Value>,
}

impl Classic {
    /// Run `body` with this session's dialog policy: a dialog that blocks
    /// the command is answered, and the command sent again once.
    fn with_dialogs<T>(&mut self, mut body: impl FnMut(&mut Session) -> Result<T>) -> Result<T> {
        for _ in 0..3 {
            match body(&mut self.s) {
                Err(e) if format!("{e:#}").contains("unexpected alert open") => {
                    let text = self.s.cmd("GET", "/alert/text", None, Duration::from_secs(5)).ok();
                    let message = text.as_ref().and_then(Value::as_str).unwrap_or("").to_string();
                    if let Some(t) = &self.prompt_text {
                        let _ = self.s.cmd("POST", "/alert/text", Some(json!({ "text": t })), Duration::from_secs(5));
                    }
                    let answer = if self.accept_dialogs { "/alert/accept" } else { "/alert/dismiss" };
                    let _ = self.s.cmd("POST", answer, Some(json!({})), Duration::from_secs(5));
                    self.dialogs.push(json!({ "message": message, "accepted": self.accept_dialogs }));
                }
                other => return other,
            }
        }
        body(&mut self.s)
    }

    /// The page hooks a classic session can't preload: installed in the
    /// document there is now.
    fn hook(&mut self) {
        let script = format!("({LISTEN})(); ({CONSOLE_HOOK})(); ({DIALOG_HOOK})(arguments[0], arguments[1]);");
        let body = json!({ "script": script, "args": [self.accept_dialogs, self.prompt_text] });
        let _ = self.s.cmd("POST", "/execute/sync", Some(body), Duration::from_secs(10));
    }
}

impl Wire for Classic {
    fn describe(&self) -> Value {
        self.s.describe()
    }

    fn navigate(&mut self, url: &str, _wait: Wait, timeout: Duration) -> Result<()> {
        self.with_dialogs(|s| s.cmd("POST", "/url", Some(json!({ "url": url })), timeout))?;
        self.hook();
        Ok(())
    }

    fn call(&mut self, function: &str, args_json: &str, timeout: Duration) -> Result<String> {
        // The page's hooks first: a document loaded since the last call has
        // none yet (each returns at once when it is there).
        let script = format!(
            "const done = arguments[arguments.length - 1]; \
             try {{ ({LISTEN})(); ({CONSOLE_HOOK})(); ({DIALOG_HOOK})(arguments[1], arguments[2]); }} catch (_) {{}} \
             ({})(arguments[0]).then(done, (e) => done(JSON.stringify({{ err: String(e && e.message || e) }})));",
            wrap(function)
        );
        let body = json!({ "script": script, "args": [args_json, self.accept_dialogs, self.prompt_text] });
        let v = self
            .with_dialogs(|s| s.cmd("POST", "/execute/async", Some(body.clone()), timeout + Duration::from_secs(1)))?;
        match v {
            Value::String(s) => Ok(s),
            other => bail!("the page's answer was not text ({other})"),
        }
    }

    fn evaluate(&mut self, source: &str, timeout: Duration) -> Result<Value> {
        // An expression first; statements (a syntax error as an expression)
        // as a function body.
        let as_expr = json!({ "script": format!("return (\n{source}\n);"), "args": [] });
        match self.with_dialogs(|s| s.cmd("POST", "/execute/sync", Some(as_expr.clone()), timeout)) {
            Err(e) if format!("{e:#}").to_ascii_lowercase().contains("syntax") => {
                let body = json!({ "script": source, "args": [] });
                self.with_dialogs(|s| s.cmd("POST", "/execute/sync", Some(body.clone()), timeout))
            }
            other => other,
        }
        .map(|v| match v {
            Value::Object(ref o) if o.contains_key(ELEMENT_KEY) => json!("<element>"),
            v => v,
        })
    }

    fn actions(&mut self, actions: Value) -> Result<()> {
        let body = json!({ "actions": actions });
        self.with_dialogs(|s| s.cmd("POST", "/actions", Some(body.clone()), Duration::from_secs(30)))?;
        let _ = self.s.cmd("DELETE", "/actions", None, Duration::from_secs(10));
        Ok(())
    }

    fn settle(&mut self, timeout: Duration) {
        // A driver answers once a navigation the action started has loaded.
        if self.with_dialogs(|s| s.cmd("GET", "/url", None, timeout)).is_ok() {
            self.hook();
        }
    }

    fn screenshot(&mut self, full_page: bool) -> Result<Vec<u8>> {
        if full_page {
            bail!("screenshot --full-page: classic WebDriver takes the viewport only; drop --full-page, or use --engine firefox or chrome");
        }
        let v = self.with_dialogs(|s| s.cmd("GET", "/screenshot", None, Duration::from_secs(30)))?;
        let data = v.as_str().ok_or_else(|| anyhow!("the driver sent no image"))?;
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| anyhow!("the image is not base64 ({e})"))
    }

    fn history(&mut self, delta: i64) -> Result<()> {
        let path = if delta < 0 { "/back" } else { "/forward" };
        for _ in 0..delta.unsigned_abs() {
            self.with_dialogs(|s| s.cmd("POST", path, Some(json!({})), Duration::from_secs(60)))?;
        }
        self.hook();
        Ok(())
    }

    fn reload(&mut self, timeout: Duration) -> Result<()> {
        self.with_dialogs(|s| s.cmd("POST", "/refresh", Some(json!({})), timeout))?;
        self.hook();
        Ok(())
    }

    fn tabs(&mut self) -> Result<Vec<String>> {
        let v = self.s.cmd("GET", "/window/handles", None, Duration::from_secs(10))?;
        Ok(v.as_array().map(|a| a.iter().filter_map(|h| h.as_str().map(str::to_string)).collect()).unwrap_or_default())
    }

    fn current_tab(&self) -> String {
        self.window.clone()
    }

    fn tab_new(&mut self) -> Result<String> {
        let v = self.s.cmd("POST", "/window/new", Some(json!({ "type": "tab" })), Duration::from_secs(30))?;
        v["handle"].as_str().map(str::to_string).ok_or_else(|| anyhow!("the driver made no tab"))
    }

    fn tab_select(&mut self, id: &str) -> Result<()> {
        self.s.cmd("POST", "/window", Some(json!({ "handle": id })), Duration::from_secs(10))?;
        self.window = id.to_string();
        Ok(())
    }

    fn tab_close(&mut self, id: &str) -> Result<()> {
        let back = self.window.clone();
        if id != back {
            self.s.cmd("POST", "/window", Some(json!({ "handle": id })), Duration::from_secs(10))?;
        }
        self.s.cmd("DELETE", "/window", None, Duration::from_secs(10))?;
        if id != back {
            self.s.cmd("POST", "/window", Some(json!({ "handle": back })), Duration::from_secs(10))?;
        }
        Ok(())
    }

    fn cookies(&mut self) -> Result<Vec<Value>> {
        let v = self.s.cmd("GET", "/cookie", None, Duration::from_secs(10))?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }

    fn set_cookie(&mut self, mut cookie: Value) -> Result<()> {
        if let Some(c) = cookie.as_object_mut() {
            c.remove("url");
        }
        self.s.cmd("POST", "/cookie", Some(json!({ "cookie": cookie })), Duration::from_secs(10))?;
        Ok(())
    }

    fn clear_cookies(&mut self) -> Result<()> {
        self.s.cmd("DELETE", "/cookie", None, Duration::from_secs(10))?;
        Ok(())
    }

    fn console(&mut self) -> Vec<Value> {
        let script =
            "const K = Symbol.for('navigera.console'); const out = window[K] || []; return out.splice(0, out.length);";
        self.s
            .cmd("POST", "/execute/sync", Some(json!({ "script": script, "args": [] })), Duration::from_secs(10))
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
    }

    fn dialogs(&mut self) -> Vec<Value> {
        let script =
            "const s = window[Symbol.for('navigera.dialogs')]; return s ? s.seen.splice(0, s.seen.length) : [];";
        let mut out = std::mem::take(&mut self.dialogs);
        if let Ok(Value::Array(seen)) =
            self.s.cmd("POST", "/execute/sync", Some(json!({ "script": script, "args": [] })), Duration::from_secs(10))
        {
            out.extend(seen);
        }
        out
    }

    fn set_dialog_policy(&mut self, accept: bool, prompt_text: Option<String>) {
        self.accept_dialogs = accept;
        self.prompt_text = prompt_text;
        self.hook();
    }

    fn endpoint(&self) -> String {
        self.s.endpoint()
    }

    fn close(&mut self) {
        self.s.end();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_servers_url_keeps_its_path_and_its_key_is_never_shown() {
        let h = Http::new("http://user:key@grid.example:4444/wd/hub/").unwrap();
        assert_eq!(h.host, "grid.example:4444");
        assert_eq!(h.base, "/wd/hub");
        assert!(h.auth.is_some());
        assert_eq!(mask("http://user:key@grid.example:4444/wd/hub"), "http://***@grid.example:4444/wd/hub");
        assert_eq!(mask("http://127.0.0.1:9515"), "http://127.0.0.1:9515");
        let e = Http::new("https://user:key@grid.example/wd/hub").err().unwrap();
        assert!(!format!("{e}").contains("key@"), "{e}");
    }

    #[test]
    fn answers_are_read_by_length_and_by_chunks() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 15\r\n\r\n{\"value\":\"one\"}").unwrap();
            let _ = s.read(&mut buf);
            let (a, b) = (r#"{"value":{"error":"no such w"#, r#"indow","message":""}}"#);
            let chunked = format!("{:x}\r\n{a}\r\n{:x}\r\n{b}\r\n0\r\n\r\n", a.len(), b.len());
            s.write_all(format!("HTTP/1.1 404 Not Found\r\nTransfer-Encoding: chunked\r\n\r\n{chunked}").as_bytes())
                .unwrap();
        });
        let mut h = Http::new(&format!("http://127.0.0.1:{port}")).unwrap();
        assert_eq!(h.request("GET", "/a", None, Duration::from_secs(5)).unwrap(), json!("one"));
        let e = h.request("GET", "/b", None, Duration::from_secs(5)).unwrap_err();
        assert!(format!("{e:#}").contains("no such window"), "{e:#}");
    }
}
