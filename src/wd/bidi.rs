//! WebDriver BiDi: Firefox launched by navigera, or the BiDi side of a
//! WebDriver session (`--bidi`, the session's `webSocketUrl`).
//!
//! One blocking WebSocket, read only while a command waits for its answer:
//! events that arrive meanwhile (console entries, dialogs, navigations) are
//! taken as they come, so nothing runs in the background and nothing locks.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::net::TcpStream;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::{self, stream::MaybeTlsStream, Message, WebSocket};

use super::{wrap, Wait, Wire, LISTEN};

/// Events every session listens to.
const EVENTS: &[&str] = &[
    "log.entryAdded",
    "browsingContext.userPromptOpened",
    "browsingContext.navigationStarted",
    "browsingContext.domContentLoaded",
    "browsingContext.load",
];

/// The longest a single read waits before the deadline is looked at again.
const READ_SLICE: Duration = Duration::from_millis(250);

/// How long an action is watched for a navigation it starts.
const NAV_START_WATCH: Duration = Duration::from_millis(30);

/// Most console entries kept between reads.
const CONSOLE_MAX: usize = 1000;

pub struct Bidi {
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: u64,
    /// The top-level browsing context ops act on.
    context: String,
    console: VecDeque<Value>,
    dialogs: Vec<Value>,
    accept_dialogs: bool,
    prompt_text: Option<String>,
    /// A navigation seen since the last action: its URL, and whether it
    /// loaded.
    nav: Option<(String, bool)>,
    browser: Value,
    endpoint: String,
    /// The Firefox navigera started, and its profile.
    child: Option<Child>,
    _profile: Option<tempfile::TempDir>,
    /// The classic session this BiDi connection belongs to (`--bidi`):
    /// ended with it.
    classic: Option<super::classic::Session>,
    closed: bool,
}

/// Where Firefox is: the path given, `$NAVIGERA_FIREFOX`, the one navigera
/// installed, `firefox` on the PATH, or the usual install places.
pub fn firefox_executable(given: Option<&str>) -> Option<String> {
    if let Some(p) =
        given.map(str::to_string).or_else(|| std::env::var("NAVIGERA_FIREFOX").ok()).filter(|p| !p.is_empty())
    {
        return Some(p);
    }
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Some(dir) = crate::install::root() {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            let mut found: Vec<_> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("firefox")))
                .collect();
            found.sort();
            for d in found.into_iter().rev() {
                candidates.push(d.join("firefox").join("firefox"));
                candidates.push(d.join("Firefox.app/Contents/MacOS/firefox"));
                candidates.push(d.join("firefox").join("firefox.exe"));
            }
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            candidates.push(dir.join(if cfg!(windows) { "firefox.exe" } else { "firefox" }));
        }
    }
    if cfg!(target_os = "macos") {
        candidates.push("/Applications/Firefox.app/Contents/MacOS/firefox".into());
        candidates.push("/Applications/Firefox Nightly.app/Contents/MacOS/firefox".into());
    }
    if cfg!(windows) {
        for base in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Ok(pf) = std::env::var(base) {
                candidates.push(std::path::Path::new(&pf).join("Mozilla Firefox").join("firefox.exe"));
            }
        }
    }
    candidates.into_iter().find(|p| p.is_file()).map(|p| p.display().to_string())
}

/// Profile preferences for a quiet automation profile: no first-run pages,
/// no update or telemetry traffic. Firefox's remote agent adds its own
/// recommended automation preferences on top.
const PREFS: &str = r#"user_pref("browser.shell.checkDefaultBrowser", false);
user_pref("browser.startup.homepage_override.mstone", "ignore");
user_pref("browser.startup.page", 0);
user_pref("browser.aboutwelcome.enabled", false);
user_pref("datareporting.policy.dataSubmissionEnabled", false);
user_pref("datareporting.healthreport.uploadEnabled", false);
user_pref("toolkit.telemetry.reportingpolicy.firstRun", false);
user_pref("app.update.disabledForTesting", true);
user_pref("app.normandy.enabled", false);
user_pref("browser.newtabpage.enabled", false);
user_pref("extensions.update.enabled", false);
user_pref("browser.safebrowsing.malware.enabled", false);
user_pref("browser.safebrowsing.phishing.enabled", false);
user_pref("network.captive-portal-service.enabled", false);
user_pref("network.connectivity-service.enabled", false);
"#;

fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        // SAFETY: kill(2) with the child's own process group (made at spawn,
        // `process_group(0)`); a group already gone answers ESRCH.
        unsafe {
            kill(-(child.id() as i32), 9);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

impl Bidi {
    /// Start Firefox with a throwaway profile and connect to its BiDi
    /// endpoint.
    pub fn launch_firefox(exe: Option<&str>, headless: bool, caps: Value, timeout: Duration) -> Result<Self> {
        let exe = firefox_executable(exe).ok_or_else(|| {
            anyhow!("no Firefox found: `navigera install-browser firefox`, or give its path with --executable (or $NAVIGERA_FIREFOX)")
        })?;
        let profile = tempfile::Builder::new()
            .prefix("navigera-firefox-")
            .tempdir()
            .context("a profile directory for Firefox")?;
        std::fs::write(profile.path().join("user.js"), PREFS).context("writing Firefox's profile preferences")?;
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("--remote-debugging-port=0")
            .arg("--profile")
            .arg(profile.path())
            .arg("--no-remote")
            .arg("--new-instance");
        if headless {
            cmd.arg("--headless");
        }
        cmd.arg("about:blank").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
        // Firefox reads MOZ_HEADLESS too; a caller's MOZ_* settings that
        // would change the profile or the remote agent are dropped.
        cmd.env_remove("MOZ_REMOTE_SETTINGS_DEVTOOLS").env("MOZ_CRASHREPORTER_DISABLE", "1");
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd.spawn().with_context(|| format!("could not start Firefox ({exe})"))?;
        let stderr = child.stderr.take().expect("piped stderr");
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            let mut sent = false;
            let mut tail = VecDeque::new();
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if !sent {
                    if let Some(at) = line.find("WebDriver BiDi listening on ") {
                        let _ = tx.send(line[at + "WebDriver BiDi listening on ".len()..].trim().to_string());
                        sent = true;
                    } else {
                        tail.push_back(line);
                        if tail.len() > 20 {
                            tail.pop_front();
                        }
                    }
                }
            }
            if !sent {
                let _ = tx.send(format!("!{}", Vec::from(tail).join("\n")));
            }
        });
        let started = Instant::now();
        let bound = timeout.max(Duration::from_secs(30));
        let ws_base = match rx.recv_timeout(bound) {
            Ok(line) if !line.starts_with('!') => line,
            Ok(tail) => {
                kill_tree(&mut child);
                bail!("Firefox exited before its BiDi endpoint was up; its last output:\n{}", &tail[1..]);
            }
            Err(_) => {
                kill_tree(&mut child);
                bail!("Firefox did not open its BiDi endpoint in {} s ({exe})", bound.as_secs());
            }
        };
        let url = format!("{}/session", ws_base.trim_end_matches('/'));
        let mut me = match Self::connect(&url, timeout) {
            Ok(me) => me,
            Err(e) => {
                kill_tree(&mut child);
                return Err(e);
            }
        };
        me.child = Some(child);
        me._profile = Some(profile);
        let mut always = json!({ "acceptInsecureCerts": true, "unhandledPromptBehavior": { "default": "ignore" } });
        if let (Some(a), Some(extra)) = (always.as_object_mut(), caps.as_object()) {
            for (k, v) in extra {
                a.insert(k.clone(), v.clone());
            }
        }
        let session = me
            .send("session.new", json!({ "capabilities": { "alwaysMatch": always } }), timeout)
            .context("Firefox refused a BiDi session")?;
        let caps = &session["capabilities"];
        me.browser = json!({
            "name": caps["browserName"].as_str().unwrap_or("firefox"),
            "version": caps["browserVersion"].as_str().unwrap_or(""),
            "executable": exe,
            "headless": headless,
            "startup_ms": started.elapsed().as_millis() as u64,
        });
        me.endpoint = ws_base;
        me.start(timeout)?;
        Ok(me)
    }

    /// The BiDi side of a classic session that asked for `webSocketUrl`.
    pub fn over(session: super::classic::Session, ws_url: &str, timeout: Duration) -> Result<Self> {
        let mut me = Self::connect(ws_url, timeout)?;
        me.browser = session.describe();
        me.endpoint = session.endpoint();
        me.classic = Some(session);
        me.start(timeout)?;
        Ok(me)
    }

    fn connect(url: &str, timeout: Duration) -> Result<Self> {
        let rest = url.strip_prefix("ws://").ok_or_else(|| {
            anyhow!("BiDi endpoint {url}: only ws:// endpoints on this machine or a network are served (no wss:// yet)")
        })?;
        let host = rest.split('/').next().unwrap_or(rest);
        let addr = std::net::ToSocketAddrs::to_socket_addrs(host)
            .with_context(|| format!("BiDi endpoint {host}"))?
            .next()
            .ok_or_else(|| anyhow!("BiDi endpoint {host}: no address"))?;
        let stream = TcpStream::connect_timeout(&addr, timeout.min(Duration::from_secs(10)))
            .with_context(|| format!("could not connect to the BiDi endpoint {url}"))?;
        stream.set_nodelay(true).ok();
        stream.set_read_timeout(Some(timeout)).ok();
        let (ws, _) = tungstenite::client::client(url, MaybeTlsStream::Plain(stream))
            .map_err(|e| anyhow!("BiDi handshake with {url} failed: {e}"))?;
        Ok(Self {
            ws,
            next_id: 1,
            context: String::new(),
            console: VecDeque::new(),
            dialogs: Vec::new(),
            accept_dialogs: true,
            prompt_text: None,
            nav: None,
            browser: Value::Null,
            endpoint: url.to_string(),
            child: None,
            _profile: None,
            classic: None,
            closed: false,
        })
    }

    /// Listen to the events, record listeners in every new document, and
    /// pick the first tab.
    fn start(&mut self, timeout: Duration) -> Result<()> {
        self.send("session.subscribe", json!({ "events": EVENTS }), timeout)?;
        self.send("script.addPreloadScript", json!({ "functionDeclaration": LISTEN }), timeout)?;
        let tree = self.send("browsingContext.getTree", json!({ "maxDepth": 0 }), timeout)?;
        self.context = tree["contexts"]
            .as_array()
            .and_then(|c| c.first())
            .and_then(|c| c["context"].as_str())
            .ok_or_else(|| anyhow!("the browser has no tab"))?
            .to_string();
        // The tab open before the preload script: listeners are recorded
        // from its next document on, and in it from now.
        let _ = self.call(LISTEN, "[]", timeout);
        Ok(())
    }

    fn write(&mut self, method: &str, params: Value) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        let text = json!({ "id": id, "method": method, "params": params }).to_string();
        self.ws.send(Message::text(text)).map_err(|e| anyhow!("{method}: the browser connection failed ({e})"))?;
        Ok(id)
    }

    /// One message, or None when `slice` passed without one.
    fn read_one(&mut self, slice: Duration) -> Result<Option<Value>> {
        if let MaybeTlsStream::Plain(s) = self.ws.get_mut() {
            s.set_read_timeout(Some(slice.max(Duration::from_millis(1)))).ok();
        }
        match self.ws.read() {
            Ok(Message::Text(t)) => {
                Ok(Some(serde_json::from_str(t.as_str()).map_err(|e| anyhow!("the browser sent no JSON ({e})"))?))
            }
            Ok(Message::Close(_)) => {
                self.closed = true;
                bail!("the browser closed the BiDi connection")
            }
            Ok(_) => Ok(None),
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
            {
                Ok(None)
            }
            Err(e) => {
                self.closed = true;
                bail!("the BiDi connection failed ({e})")
            }
        }
    }

    /// Send a command and wait for its answer, taking events on the way.
    fn send(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        if self.closed {
            bail!("{method}: the browser is gone");
        }
        let id = self.write(method, params)?;
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                bail!("{method}: no answer from the browser in {} ms", timeout.as_millis());
            }
            let Some(mut v) = self.read_one(left.min(READ_SLICE))? else { continue };
            match v["type"].as_str() {
                Some("event") => self.on_event(v),
                Some("success") if v["id"].as_u64() == Some(id) => return Ok(v["result"].take()),
                Some("error") if v["id"].as_u64() == Some(id) => {
                    let code = v["error"].as_str().unwrap_or("error");
                    let msg = v["message"].as_str().unwrap_or("");
                    bail!("{method}: {code}: {msg}");
                }
                // The answer to a command sent from an event (a dialog's).
                _ => {}
            }
        }
    }

    fn on_event(&mut self, v: Value) {
        let params = &v["params"];
        match v["method"].as_str().unwrap_or("") {
            "log.entryAdded" => {
                let level = match params["level"].as_str().unwrap_or("info") {
                    "warn" => "warning",
                    l => l,
                };
                self.console.push_back(json!({
                    "level": level,
                    "text": params["text"].as_str().unwrap_or(""),
                    "type": params["type"],
                    "url": params["source"]["context"],
                    "ts": params["timestamp"],
                }));
                if self.console.len() > CONSOLE_MAX {
                    self.console.pop_front();
                }
            }
            "browsingContext.userPromptOpened" => {
                let context = params["context"].as_str().unwrap_or(&self.context).to_string();
                let kind = params["type"].as_str().unwrap_or("alert").to_string();
                let accept = self.accept_dialogs || kind == "alert";
                let mut p = json!({ "context": context, "accept": accept });
                if kind == "prompt" && accept {
                    if let Some(t) = &self.prompt_text {
                        p["userText"] = json!(t);
                    }
                }
                let _ = self.write("browsingContext.handleUserPrompt", p);
                self.dialogs.push(json!({
                    "type": kind,
                    "message": params["message"],
                    "accepted": accept,
                }));
            }
            "browsingContext.navigationStarted" => {
                if params["context"].as_str() == Some(self.context.as_str()) {
                    self.nav = Some((params["url"].as_str().unwrap_or("").to_string(), false));
                }
            }
            "browsingContext.load" if params["context"].as_str() == Some(self.context.as_str()) => {
                if let Some((_, loaded)) = &mut self.nav {
                    *loaded = true;
                }
            }
            _ => {}
        }
    }

    fn remote_to_json(v: &Value) -> Value {
        let inner = &v["value"];
        match v["type"].as_str().unwrap_or("") {
            "undefined" | "null" => Value::Null,
            "string" | "boolean" => inner.clone(),
            "number" => match inner {
                Value::Number(_) => inner.clone(),
                Value::String(s) if s == "-0" => json!(0),
                _ => Value::Null,
            },
            "bigint" | "date" => inner.clone(),
            "array" | "set" => {
                Value::Array(inner.as_array().map(|a| a.iter().map(Self::remote_to_json).collect()).unwrap_or_default())
            }
            "object" | "map" => {
                let mut out = serde_json::Map::new();
                for pair in inner.as_array().into_iter().flatten() {
                    let key = match &pair[0] {
                        Value::String(s) => s.clone(),
                        other => Self::remote_to_json(other).to_string(),
                    };
                    out.insert(key, Self::remote_to_json(&pair[1]));
                }
                Value::Object(out)
            }
            "regexp" => {
                json!(format!("/{}/{}", inner["pattern"].as_str().unwrap_or(""), inner["flags"].as_str().unwrap_or("")))
            }
            "node" => {
                json!(format!("<{}>", inner["localName"].as_str().or(inner["nodeName"].as_str()).unwrap_or("node")))
            }
            other => json!(format!("[{other}]")),
        }
    }

    fn top_contexts(&mut self) -> Result<Vec<String>> {
        let tree = self.send("browsingContext.getTree", json!({ "maxDepth": 0 }), Duration::from_secs(10))?;
        Ok(tree["contexts"]
            .as_array()
            .map(|c| c.iter().filter_map(|c| c["context"].as_str().map(str::to_string)).collect())
            .unwrap_or_default())
    }
}

impl Wire for Bidi {
    fn describe(&self) -> Value {
        self.browser.clone()
    }

    fn navigate(&mut self, url: &str, wait: Wait, timeout: Duration) -> Result<()> {
        let wait = match wait {
            Wait::None => "none",
            Wait::Interactive => "interactive",
            Wait::Complete => "complete",
        };
        self.nav = None;
        self.send("browsingContext.navigate", json!({ "context": self.context, "url": url, "wait": wait }), timeout)?;
        Ok(())
    }

    fn call(&mut self, function: &str, args_json: &str, timeout: Duration) -> Result<String> {
        let params = json!({
            "functionDeclaration": wrap(function),
            "awaitPromise": true,
            "target": { "context": self.context },
            "arguments": [{ "type": "string", "value": args_json }],
            "resultOwnership": "none",
        });
        let mut tried = false;
        loop {
            match self.send("script.callFunction", params.clone(), timeout) {
                Ok(r) => {
                    if r["type"] == "exception" {
                        let text = r["exceptionDetails"]["text"].as_str().unwrap_or("the page's script failed");
                        bail!("{text}");
                    }
                    return match &r["result"]["value"] {
                        Value::String(s) => Ok(s.clone()),
                        _ => bail!("the page's answer was not text ({})", r["result"]["type"]),
                    };
                }
                // A document replaced while the call ran (a navigation): once
                // more, in the new one.
                Err(e)
                    if !tried && {
                        let m = format!("{e:#}");
                        m.contains("no such frame") || m.contains("discarded")
                    } =>
                {
                    tried = true;
                    self.settle(timeout);
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn evaluate(&mut self, source: &str, timeout: Duration) -> Result<Value> {
        let r = self.send(
            "script.evaluate",
            json!({
                "expression": source,
                "awaitPromise": true,
                "target": { "context": self.context },
                "resultOwnership": "none",
                "serializationOptions": { "maxObjectDepth": 20, "maxDomDepth": 0 },
            }),
            timeout,
        )?;
        if r["type"] == "exception" {
            bail!("{}", r["exceptionDetails"]["text"].as_str().unwrap_or("the script threw"));
        }
        Ok(Self::remote_to_json(&r["result"]))
    }

    fn actions(&mut self, actions: Value) -> Result<()> {
        self.nav = None;
        self.send(
            "input.performActions",
            json!({ "context": self.context, "actions": actions }),
            Duration::from_secs(30),
        )?;
        Ok(())
    }

    fn settle(&mut self, timeout: Duration) {
        // A navigation the action started announces itself within a few
        // milliseconds; then its load is waited for.
        let watch_until = Instant::now() + NAV_START_WATCH;
        while self.nav.is_none() && Instant::now() < watch_until {
            match self.read_one(watch_until.saturating_duration_since(Instant::now())) {
                Ok(Some(v)) if v["type"] == "event" => self.on_event(v),
                Ok(_) => {}
                Err(_) => return,
            }
        }
        // A page restored from the back-forward cache fires no load event:
        // its URL and readyState say when it is there.
        let deadline = Instant::now() + timeout;
        let mut looked = Instant::now();
        while matches!(self.nav, Some((_, false))) && Instant::now() < deadline {
            match self.read_one(READ_SLICE) {
                Ok(Some(v)) if v["type"] == "event" => self.on_event(v),
                Ok(_) => {}
                Err(_) => return,
            }
            if matches!(self.nav, Some((_, false))) && looked.elapsed() >= READ_SLICE {
                looked = Instant::now();
                let want = self.nav.as_ref().map(|(u, _)| u.clone()).unwrap_or_default();
                let here = self
                    .send(
                        "script.evaluate",
                        json!({ "expression": "location.href + ' ' + document.readyState", "awaitPromise": false,
                                "target": { "context": self.context }, "resultOwnership": "none" }),
                        deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(1)),
                    )
                    .ok()
                    .and_then(|r| r["result"]["value"].as_str().map(str::to_string))
                    .unwrap_or_default();
                if here == format!("{want} complete") || (want.is_empty() && here.ends_with(" complete")) {
                    break;
                }
            }
        }
        self.nav = None;
    }

    fn screenshot(&mut self, full_page: bool) -> Result<Vec<u8>> {
        let origin = if full_page { "document" } else { "viewport" };
        let r = self.send(
            "browsingContext.captureScreenshot",
            json!({ "context": self.context, "origin": origin }),
            Duration::from_secs(30),
        )?;
        let data = r["data"].as_str().ok_or_else(|| anyhow!("the browser sent no image"))?;
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| anyhow!("the image is not base64 ({e})"))
    }

    fn history(&mut self, delta: i64) -> Result<()> {
        self.nav = None;
        self.send(
            "browsingContext.traverseHistory",
            json!({ "context": self.context, "delta": delta }),
            Duration::from_secs(30),
        )?;
        self.settle(Duration::from_secs(30));
        Ok(())
    }

    fn reload(&mut self, timeout: Duration) -> Result<()> {
        self.send("browsingContext.reload", json!({ "context": self.context, "wait": "complete" }), timeout)?;
        Ok(())
    }

    fn tabs(&mut self) -> Result<Vec<String>> {
        self.top_contexts()
    }

    fn current_tab(&self) -> String {
        self.context.clone()
    }

    fn tab_new(&mut self) -> Result<String> {
        let r = self.send("browsingContext.create", json!({ "type": "tab" }), Duration::from_secs(30))?;
        r["context"].as_str().map(str::to_string).ok_or_else(|| anyhow!("the browser made no tab"))
    }

    fn tab_select(&mut self, id: &str) -> Result<()> {
        // `activate` brings a headed tab to the front; headless it may refuse.
        let _ = self.send("browsingContext.activate", json!({ "context": id }), Duration::from_secs(10));
        self.context = id.to_string();
        Ok(())
    }

    fn tab_close(&mut self, id: &str) -> Result<()> {
        self.send("browsingContext.close", json!({ "context": id }), Duration::from_secs(10))?;
        Ok(())
    }

    fn cookies(&mut self) -> Result<Vec<Value>> {
        let r = self.send(
            "storage.getCookies",
            json!({ "partition": { "type": "context", "context": self.context } }),
            Duration::from_secs(10),
        )?;
        Ok(r["cookies"]
            .as_array()
            .map(|cs| {
                cs.iter()
                    .map(|c| {
                        json!({
                            "name": c["name"],
                            "value": Self::remote_to_json(&c["value"]),
                            "domain": c["domain"],
                            "path": c["path"],
                            "httpOnly": c["httpOnly"],
                            "secure": c["secure"],
                            "sameSite": c["sameSite"],
                            "expires": c.get("expiry").cloned().unwrap_or(Value::Null),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    fn set_cookie(&mut self, cookie: Value) -> Result<()> {
        let domain = match cookie["domain"].as_str() {
            Some(d) => d.to_string(),
            None => {
                let url = cookie["url"].as_str().map(str::to_string).unwrap_or_else(|| self.endpoint_page_url());
                url.split("://").nth(1).and_then(|r| r.split(['/', ':']).next()).unwrap_or("").to_string()
            }
        };
        let mut c = json!({
            "name": cookie["name"],
            "value": { "type": "string", "value": cookie["value"] },
            "domain": domain,
            "path": cookie["path"].as_str().unwrap_or("/"),
            "httpOnly": cookie["httpOnly"].as_bool().unwrap_or(false),
            "secure": cookie["secure"].as_bool().unwrap_or(false),
        });
        if let Some(s) = cookie["sameSite"].as_str() {
            c["sameSite"] = json!(s.to_ascii_lowercase());
        }
        if let Some(e) = cookie["expiry"].as_i64() {
            c["expiry"] = json!(e);
        }
        self.send(
            "storage.setCookie",
            json!({ "cookie": c, "partition": { "type": "context", "context": self.context } }),
            Duration::from_secs(10),
        )?;
        Ok(())
    }

    fn clear_cookies(&mut self) -> Result<()> {
        self.send(
            "storage.deleteCookies",
            json!({ "partition": { "type": "context", "context": self.context } }),
            Duration::from_secs(10),
        )?;
        Ok(())
    }

    fn console(&mut self) -> Vec<Value> {
        // Entries logged since the last command are still on the socket.
        while let Ok(Some(v)) = self.read_one(Duration::from_millis(1)) {
            if v["type"] == "event" {
                self.on_event(v);
            }
        }
        self.console.drain(..).collect()
    }

    fn dialogs(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.dialogs)
    }

    fn set_dialog_policy(&mut self, accept: bool, prompt_text: Option<String>) {
        self.accept_dialogs = accept;
        self.prompt_text = prompt_text;
    }

    fn endpoint(&self) -> String {
        self.endpoint.clone()
    }

    fn close(&mut self) {
        if !self.closed {
            if self.classic.is_none() {
                let _ = self.send("browser.close", json!({}), Duration::from_secs(5));
            }
            let _ = self.ws.close(None);
            self.closed = true;
        }
        if let Some(mut s) = self.classic.take() {
            s.end();
        }
        if let Some(mut child) = self.child.take() {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if let Ok(Some(_)) = child.try_wait() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            kill_tree(&mut child);
        }
    }
}

impl Bidi {
    /// The current page's URL, for a cookie set with neither a domain nor a
    /// URL.
    fn endpoint_page_url(&mut self) -> String {
        self.call(super::URL_TITLE, "[]", Duration::from_secs(10))
            .ok()
            .and_then(|a| super::ok_text(&a).ok().and_then(|t| serde_json::from_str::<Value>(t).ok()))
            .and_then(|v| v[0].as_str().map(str::to_string))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_values_read_as_plain_json() {
        let v = json!({ "type": "object", "value": [
            ["a", { "type": "number", "value": 1 }],
            ["b", { "type": "array", "value": [{ "type": "string", "value": "x" }, { "type": "null" }] }],
            ["c", { "type": "number", "value": "NaN" }],
            ["d", { "type": "regexp", "value": { "pattern": "a+", "flags": "g" } }],
            ["e", { "type": "function" }],
        ]});
        assert_eq!(
            Bidi::remote_to_json(&v),
            json!({ "a": 1, "b": ["x", null], "c": null, "d": "/a+/g", "e": "[function]" })
        );
        assert_eq!(Bidi::remote_to_json(&json!({ "type": "undefined" })), Value::Null);
    }
}
