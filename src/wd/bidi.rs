//! WebDriver BiDi: Firefox launched by navigera, or the BiDi side of a
//! WebDriver session (`--bidi`, the session's `webSocketUrl`).
//!
//! One blocking WebSocket, read only while a command waits for its answer:
//! events that arrive meanwhile (console entries, dialogs, navigations) are
//! taken as they come, so nothing runs in the background and nothing locks.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::net::TcpStream;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::{self, stream::MaybeTlsStream, Message, WebSocket};

use super::{wrap, Wait, Watch, Wire, LISTEN};
use crate::cdp::{Route, RouteAction};

/// Events every session listens to.
const EVENTS: &[&str] = &[
    "log.entryAdded",
    "browsingContext.userPromptOpened",
    "browsingContext.navigationStarted",
    "browsingContext.domContentLoaded",
    "browsingContext.load",
];

/// In the user's browser (`--attach`), also which frames and opened tabs
/// belong to this session's tabs: their console lines and dialogs are its
/// own, and an event names the frame, not the tab.
const ATTACHED_EVENTS: &[&str] = &["browsingContext.contextCreated", "browsingContext.contextDestroyed"];

/// The frames a session keeps track of in the user's browser; a page with
/// more keeps the newest.
const FRAMES_MAX: usize = 1024;

/// The longest a single read waits before the deadline is looked at again.
const READ_SLICE: Duration = Duration::from_millis(250);

/// How long an action is watched for a navigation it starts.
const NAV_START_WATCH: Duration = Duration::from_millis(30);

/// Most console entries kept between reads.
const CONSOLE_MAX: usize = 1000;

/// The most of one console entry's text that is kept, in bytes.
const CONSOLE_TEXT_MAX: usize = 16 << 10;

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
    /// `--attach`: the user's own Firefox. Only the windows and tabs this
    /// session opened (`own`) are acted on, listened to and closed; the
    /// browser itself is left running.
    attached: bool,
    own: Vec<String>,
    /// `route`: the rules, and the intercept that holds every request for
    /// them while there is one.
    routes: Vec<Route>,
    intercept: Option<String>,
    /// `init-script`: the preload scripts this session added.
    preloads: Vec<String>,
    /// `set viewport`, for the tabs this session opens after it.
    viewport: Option<Value>,
    /// `set locale|timezone|media|offline|ua`: each `emulation.*` command
    /// in force (its own parameters), sent again to a tab this session
    /// opens in the user's Firefox.
    emulations: Vec<(String, Value)>,
    /// The frames inside `own` tabs, each with its tab.
    frames: Vec<(String, String)>,
    closed: bool,
}

/// How to get a Firefox that `--attach firefox` can find.
const ATTACH_HINT: &str = "start Firefox with `firefox --remote-debugging-port` (quit it first: a running Firefox \
     ignores the flag), then `--attach firefox`; or `--engine firefox` starts a separate one";

/// The directories that hold Firefox's profiles, for this user.
pub fn firefox_profile_roots() -> Vec<std::path::PathBuf> {
    let mut roots = Vec::new();
    if cfg!(windows) {
        if let Some(d) = std::env::var_os("APPDATA") {
            roots.push(std::path::PathBuf::from(d).join("Mozilla").join("Firefox").join("Profiles"));
        }
    } else if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
        if cfg!(target_os = "macos") {
            roots.push(home.join("Library/Application Support/Firefox/Profiles"));
        } else {
            roots.push(home.join(".mozilla/firefox"));
            // The snap and the flatpak keep a home of their own.
            roots.push(home.join("snap/firefox/common/.mozilla/firefox"));
            roots.push(home.join(".var/app/org.mozilla.firefox/.mozilla/firefox"));
        }
    }
    roots
}

/// The BiDi endpoints running Firefoxes wrote into their profiles
/// (`WebDriverBiDiServer.json`, there while the remote agent listens),
/// the most recently started first.
pub fn firefox_endpoints_in(roots: &[std::path::PathBuf]) -> Vec<String> {
    let mut found: Vec<(std::time::SystemTime, String)> = Vec::new();
    for root in roots {
        let Ok(profiles) = std::fs::read_dir(root) else { continue };
        // A user has a handful of profiles; a directory of thousands is not one.
        for profile in profiles.flatten().take(256) {
            let file = profile.path().join("WebDriverBiDiServer.json");
            let Ok(meta) = std::fs::metadata(&file) else { continue };
            if meta.len() > 4096 {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
            let (Some(host), Some(port)) = (v["ws_host"].as_str(), v["ws_port"].as_u64()) else { continue };
            // The file is the user's, but only a loopback listener is
            // their own browser.
            let local = matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]");
            if !local || port == 0 || port > 65535 {
                continue;
            }
            let host =
                if host.contains(':') && !host.starts_with('[') { format!("[{host}]") } else { host.to_string() };
            let at = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            found.push((at, format!("ws://{host}:{port}/session")));
        }
    }
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    let mut out: Vec<String> = Vec::new();
    for (_, url) in found {
        if !out.contains(&url) {
            out.push(url);
        }
    }
    out
}

/// In the user's browser, a tab one of this session's pages opened (a
/// link with a target, `window.open`) is this session's too: `tree` is
/// `browsingContext.getTree`'s answer, where each tab names the tab that
/// opened it, also when the link said `noopener`.
fn adopt_opened(own: &mut Vec<String>, tree: &Value) {
    let Some(tabs) = tree["contexts"].as_array() else { return };
    // A tab opened by an adopted tab is found on the next pass.
    loop {
        let before = own.len();
        for tab in tabs {
            let (Some(id), Some(opener)) = (tab["context"].as_str(), tab["originalOpener"].as_str()) else {
                continue;
            };
            if own.iter().any(|o| o == opener) && !own.iter().any(|o| o == id) {
                own.push(id.to_string());
            }
        }
        if own.len() == before {
            return;
        }
    }
}

/// The tab of this session an event about `context` belongs to: the tab
/// itself, or the tab a frame is in.
fn tab_of<'a>(own: &'a [String], frames: &'a [(String, String)], context: &str) -> Option<&'a str> {
    own.iter()
        .find(|o| o.as_str() == context)
        .or_else(|| frames.iter().find(|(frame, _)| frame == context).map(|(_, tab)| tab))
        .map(String::as_str)
}

/// `browsingContext.contextCreated` in the user's browser: a frame in one
/// of this session's tabs (or in a frame of one) is noted with its tab,
/// and a tab one of its pages opened is its own from now, before the
/// next listing.
/// A status code's usual words, for a response navigera makes up.
fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "",
    }
}

fn context_created(own: &mut Vec<String>, frames: &mut Vec<(String, String)>, info: &Value) {
    let Some(id) = info["context"].as_str() else { return };
    if tab_of(own, frames, id).is_some() {
        return;
    }
    match info["parent"].as_str() {
        Some(parent) => {
            let Some(tab) = tab_of(own, frames, parent).map(str::to_string) else { return };
            if frames.len() >= FRAMES_MAX {
                frames.remove(0);
            }
            frames.push((id.to_string(), tab));
        }
        None => {
            if info["originalOpener"].as_str().is_some_and(|opener| tab_of(own, frames, opener).is_some()) {
                own.push(id.to_string());
            }
        }
    }
}

/// `browsingContext.contextDestroyed`: the frame, the frames inside it
/// (named in the event, with no event of their own) and, for a tab, all
/// its frames are forgotten.
fn context_destroyed(frames: &mut Vec<(String, String)>, info: &Value) {
    let Some(id) = info["context"].as_str() else { return };
    frames.retain(|(frame, tab)| frame != id && tab != id);
    // As deep as a browser nests frames; a deeper answer is not walked.
    fn inner(frames: &mut Vec<(String, String)>, info: &Value, depth: usize) {
        for child in info["children"].as_array().into_iter().flatten().take(FRAMES_MAX) {
            if let Some(id) = child["context"].as_str() {
                frames.retain(|(frame, _)| frame != id);
            }
            if depth < 32 {
                inner(frames, child, depth + 1);
            }
        }
    }
    inner(frames, info, 0);
}

/// The endpoints `--attach <spec>` means on the Firefox engine: `firefox`
/// or `auto` (what running Firefoxes wrote, then the usual port), a port,
/// `host:port`, or a `ws://` URL.
fn attach_candidates(spec: &str) -> Result<Vec<String>> {
    let spec = spec.trim();
    if spec.eq_ignore_ascii_case("firefox") || spec == crate::cdp::attach::AUTO {
        let mut all = firefox_endpoints_in(&firefox_profile_roots());
        let usual = "ws://127.0.0.1:9222/session".to_string();
        if !all.contains(&usual) {
            all.push(usual);
        }
        return Ok(all);
    }
    if let Some(rest) = spec.strip_prefix("ws://") {
        // Firefox serves new sessions at /session.
        return Ok(vec![if rest.contains('/') { spec.to_string() } else { format!("{spec}/session") }]);
    }
    if spec.starts_with("wss://") || spec.contains("://") {
        bail!("--attach {}: Firefox is reached at a ws:// endpoint, a port, or `firefox`", super::classic::mask(spec));
    }
    if spec.parse::<u16>().is_ok_and(|p| p > 0) {
        return Ok(vec![format!("ws://127.0.0.1:{spec}/session")]);
    }
    match spec.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() && port.parse::<u16>().is_ok_and(|p| p > 0) => {
            Ok(vec![format!("ws://{spec}/session")])
        }
        _ => bail!("--attach {spec:?} with --engine firefox: `firefox`, a port, host:port or a ws:// endpoint"),
    }
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

/// A session that failed half-way (a refused capability, say) still ends
/// its Firefox and its driver.
impl Drop for Bidi {
    fn drop(&mut self) {
        Wire::close(self);
    }
}

impl Bidi {
    /// Start Firefox with a throwaway profile and connect to its BiDi
    /// endpoint.
    pub fn launch_firefox(exe: Option<&str>, headless: bool, caps: Value, timeout: Duration) -> Result<Self> {
        let exe = firefox_executable(exe).ok_or_else(|| {
            anyhow!("no Firefox found: install Firefox, or give its path with --executable (or $NAVIGERA_FIREFOX)")
        })?;
        let profile = tempfile::Builder::new()
            .prefix("navigera-firefox-")
            .tempdir()
            .context("a profile directory for Firefox")?;
        // Downloads a page starts stay in the profile, which goes with the session.
        let downloads = serde_json::to_string(&profile.path().join("downloads").display().to_string())?;
        let prefs = format!(
            "{PREFS}user_pref(\"browser.download.folderList\", 2);\nuser_pref(\"browser.download.dir\", {downloads});\n"
        );
        std::fs::write(profile.path().join("user.js"), prefs).context("writing Firefox's profile preferences")?;
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
            let mut r = BufReader::new(stderr);
            let mut raw = Vec::new();
            loop {
                // A line in pieces of at most 64 KB, whatever its bytes: the
                // pipe is read to its end so Firefox never blocks on it.
                raw.clear();
                match (&mut r).take(64 << 10).read_until(b'\n', &mut raw) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let line = String::from_utf8_lossy(&raw).trim_end().to_string();
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
        // Certificates are checked, as on Chrome; `--capabilities
        // '{"acceptInsecureCerts":true}'` is for a test site without one.
        let mut always = json!({ "unhandledPromptBehavior": { "default": "ignore" } });
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

    /// The user's running Firefox (`--attach firefox`): a session on its
    /// BiDi endpoint, working in a window this session opens. Firefox
    /// serves BiDi only when started with `--remote-debugging-port`, and
    /// one session at a time.
    pub fn attach_firefox(spec: &str, caps: Value, timeout: Duration) -> Result<Self> {
        let started = Instant::now();
        let candidates = attach_candidates(spec)?;
        let named = candidates.len() == 1;
        let mut refused: Option<anyhow::Error> = None;
        for url in &candidates {
            let mut me = match Self::connect(url, timeout) {
                Ok(me) => me,
                Err(e) if named => return Err(e.context(format!("no Firefox answers there; {ATTACH_HINT}"))),
                Err(_) => continue,
            };
            me.attached = true;
            // Dialogs in the user's own tabs are theirs to answer.
            let mut always = json!({ "unhandledPromptBehavior": { "default": "ignore" } });
            if let (Some(a), Some(extra)) = (always.as_object_mut(), caps.as_object()) {
                for (k, v) in extra {
                    a.insert(k.clone(), v.clone());
                }
            }
            let session = match me.send("session.new", json!({ "capabilities": { "alwaysMatch": always } }), timeout) {
                Ok(s) => s,
                Err(e) => {
                    // Nothing of this session exists there: just hang up.
                    me.closed = true;
                    let _ = me.ws.close(None);
                    let shown = super::classic::mask(url);
                    let e = if e.to_string().contains("session not created") {
                        anyhow!(
                            "the browser at {shown} already has an automation session (Firefox serves one at a \
                             time): end the other client, then start again ({e})"
                        )
                    } else {
                        anyhow!("the browser at {shown} refused a BiDi session: {e}")
                    };
                    if named {
                        return Err(e);
                    }
                    refused.get_or_insert(e);
                    continue;
                }
            };
            let caps = &session["capabilities"];
            me.browser = json!({
                "name": caps["browserName"].as_str().unwrap_or("firefox"),
                "version": caps["browserVersion"].as_str().unwrap_or(""),
                "executable": "attach Firefox",
                "headless": false,
                "attached": true,
                "startup_ms": started.elapsed().as_millis() as u64,
            });
            me.start(timeout)?;
            return Ok(me);
        }
        match refused {
            Some(e) => Err(e),
            None => bail!("no running Firefox serves remote debugging: {ATTACH_HINT}"),
        }
    }

    /// Whether an event about `context` is this session's to act on: in
    /// the user's browser, only what it opened.
    fn ours(&self, context: Option<&str>) -> bool {
        !self.attached || context.is_some_and(|c| self.own.iter().any(|o| o == c))
    }

    /// Whether an event from `context` is this session's to hear: its
    /// tabs, and in the user's browser the frames inside them too.
    fn heard(&self, context: Option<&str>) -> bool {
        !self.attached || context.is_some_and(|c| tab_of(&self.own, &self.frames, c).is_some())
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
        // The endpoint as shown: a grid's may carry a key.
        let shown = super::classic::mask(url);
        let rest = url.strip_prefix("ws://").ok_or_else(|| {
            anyhow!(
                "BiDi endpoint {shown}: only ws:// endpoints on this machine or a network are served (no wss:// yet)"
            )
        })?;
        let authority = rest.split(['/', '?']).next().unwrap_or(rest);
        let host = authority.rsplit('@').next().unwrap_or(authority);
        let addr = std::net::ToSocketAddrs::to_socket_addrs(host)
            .with_context(|| format!("BiDi endpoint {host}"))?
            .next()
            .ok_or_else(|| anyhow!("BiDi endpoint {host}: no address"))?;
        let stream = TcpStream::connect_timeout(&addr, timeout.min(Duration::from_secs(10)))
            .with_context(|| format!("could not connect to the BiDi endpoint {shown}"))?;
        stream.set_nodelay(true).ok();
        stream.set_read_timeout(Some(timeout)).ok();
        let (ws, _) = tungstenite::client::client(url, MaybeTlsStream::Plain(stream))
            .map_err(|e| anyhow!("BiDi handshake with {shown} failed: {e}"))?;
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
            endpoint: shown,
            preloads: Vec::new(),
            routes: Vec::new(),
            intercept: None,
            viewport: None,
            emulations: Vec::new(),
            child: None,
            _profile: None,
            classic: None,
            attached: false,
            own: Vec::new(),
            frames: Vec::new(),
            closed: false,
        })
    }

    /// Listen to the events, record listeners in every new document, and
    /// pick the first tab.
    fn start(&mut self, timeout: Duration) -> Result<()> {
        self.send("session.subscribe", json!({ "events": EVENTS }), timeout)?;
        if self.attached {
            // A browser without these events still works: a frame's
            // console lines are then not heard.
            let _ = self.send("session.subscribe", json!({ "events": ATTACHED_EVENTS }), timeout);
            // A window of its own, as in the user's Chrome: their tabs are
            // not navigated, read or scripted. (A background tab would not
            // do: it paints no frames and its timers are slowed.)
            let made = match self.send("browsingContext.create", json!({ "type": "window" }), timeout) {
                Ok(made) => made,
                Err(_) => self.send("browsingContext.create", json!({ "type": "tab" }), timeout)?,
            };
            self.context = made["context"].as_str().ok_or_else(|| anyhow!("the browser opened no window"))?.to_string();
            self.own.push(self.context.clone());
            self.listen_in(&self.context.clone(), timeout);
            return Ok(());
        }
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

    /// Record listeners in one tab of the user's browser: its documents
    /// from the next on, and the one it shows now. A Firefox that cannot
    /// scope a preload script to a tab goes without (clicks still work;
    /// the tree marks fewer elements clickable).
    fn listen_in(&mut self, context: &str, timeout: Duration) {
        let _ = self.send(
            "script.addPreloadScript",
            json!({ "functionDeclaration": LISTEN, "contexts": [context] }),
            timeout,
        );
        let _ = self.call(LISTEN, "[]", timeout);
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
            "log.entryAdded" if self.heard(params["source"]["context"].as_str()) => {
                let level = match params["level"].as_str().unwrap_or("info") {
                    "warn" => "warning",
                    l => l,
                };
                self.console.push_back(json!({
                    "level": level,
                    "text": super::clip(params["text"].as_str().unwrap_or(""), CONSOLE_TEXT_MAX),
                    "type": params["type"],
                    "url": params["source"]["context"],
                    "ts": params["timestamp"],
                }));
                if self.console.len() > CONSOLE_MAX {
                    self.console.pop_front();
                }
            }
            "browsingContext.contextCreated" if self.attached => {
                context_created(&mut self.own, &mut self.frames, params);
            }
            "browsingContext.contextDestroyed" if self.attached => {
                context_destroyed(&mut self.frames, params);
            }
            "browsingContext.userPromptOpened" if self.heard(params["context"].as_str()) => {
                // A frame's dialog blocks its tab, and the tab is what is answered.
                let named = params["context"].as_str().unwrap_or(&self.context);
                let context = match tab_of(&self.own, &self.frames, named) {
                    Some(tab) if self.attached => tab,
                    _ => named,
                }
                .to_string();
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
            "network.beforeRequestSent" if params["isBlocked"] == true => {
                let (method, p) = self.route_answer(params);
                let _ = self.write(method, p);
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

    /// What happens to a held request: the first rule whose glob matches
    /// its URL answers it, as on Chrome; no rule lets it go on.
    fn route_answer(&mut self, params: &Value) -> (&'static str, Value) {
        let request = &params["request"];
        let id = request["request"].clone();
        let url = request["url"].as_str().unwrap_or("");
        let method = request["method"].as_str().unwrap_or("GET");
        let go_on = ("network.continueRequest", json!({ "request": id }));
        let fail = ("network.failRequest", json!({ "request": id }));
        let fulfill = |status: u16, headers: &[(String, String)], body: &str| {
            let headers: Vec<Value> =
                headers.iter().map(|(k, v)| json!({ "name": k, "value": { "type": "string", "value": v } })).collect();
            (
                "network.provideResponse",
                json!({ "request": id, "statusCode": status, "reasonPhrase": reason(status), "headers": headers,
                        "body": { "type": "base64", "value": body } }),
            )
        };
        let Some(route) = self.routes.iter_mut().find(|r| crate::cdp::glob_match(&r.pattern, url)) else {
            return go_on;
        };
        route.hits += 1;
        match &mut route.action {
            RouteAction::Abort => fail,
            RouteAction::Fulfill { status, headers, body_base64 } => fulfill(*status, headers, body_base64),
            RouteAction::Har { entries, not_found_continue } => {
                let same = |e: &crate::cdp::HarEntry| e.method.eq_ignore_ascii_case(method) && e.url == url;
                let pick = entries.iter().position(|e| !e.used && same(e)).or_else(|| entries.iter().rposition(same));
                match pick {
                    Some(i) => {
                        let e = &mut entries[i];
                        e.used = true;
                        if e.status == 0 {
                            fail
                        } else {
                            fulfill(e.status, &e.headers, &e.body_base64)
                        }
                    }
                    None if *not_found_continue => go_on,
                    None => fail,
                }
            }
            // Refused when the rule was asked for.
            RouteAction::Modify { .. } => go_on,
        }
    }

    /// Hold every request of this session's tabs for the rules, or let
    /// them all go again when the last rule went.
    fn sync_intercept(&mut self) -> Result<()> {
        let timeout = Duration::from_secs(30);
        match (&self.intercept, self.routes.is_empty()) {
            (None, false) => {
                let mut subscribe = json!({ "events": ["network.beforeRequestSent"] });
                let mut intercept = json!({ "phases": ["beforeRequestSent"] });
                if self.attached {
                    // In the user's own Firefox: this session's tabs only.
                    subscribe["contexts"] = json!(self.own);
                    intercept["contexts"] = json!(self.own);
                }
                self.send("session.subscribe", subscribe, timeout)?;
                let r = self.send("network.addIntercept", intercept, timeout)?;
                let id = r["intercept"].as_str().ok_or_else(|| anyhow!("the browser named no intercept"))?;
                self.intercept = Some(id.to_string());
            }
            (Some(id), true) => {
                let id = id.clone();
                self.send("network.removeIntercept", json!({ "intercept": id }), timeout)?;
                self.intercept = None;
                let mut unsubscribe = json!({ "events": ["network.beforeRequestSent"] });
                if self.attached {
                    unsubscribe["contexts"] = json!(self.own);
                }
                let _ = self.send("session.unsubscribe", unsubscribe, timeout);
            }
            _ => {}
        }
        Ok(())
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
        Ok(self.tops_of(&tree))
    }

    /// The top-level contexts of a `getTree` answer.
    fn tops_of(&mut self, tree: &Value) -> Vec<String> {
        if self.attached {
            adopt_opened(&mut self.own, tree);
        }
        tree["contexts"]
            .as_array()
            .map(|c| c.iter().filter_map(|c| c["context"].as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    }

    /// The session's tabs among the browser's.
    fn tabs_among(&mut self, all: Vec<String>) -> Vec<String> {
        if !self.attached {
            return all;
        }
        // In the user's browser: the tabs this session opened, or its
        // pages did, and that are still there.
        self.own.retain(|o| all.contains(o));
        self.own.clone()
    }

    fn call_params(&self, function: &str, args_json: &str) -> Value {
        // `script.evaluate` of the call, the arguments written into it as a
        // string, not `script.callFunction`: Firefox runs a function made by
        // callFunction's declaration with every global it names looked up
        // the slow way (the snapshot's walk, 3,040 nodes: 10.5-12.9 ms
        // against 6.4-7.6 for the same function made by an evaluation).
        let args = serde_json::to_string(args_json).unwrap_or_else(|_| "\"[]\"".to_string());
        json!({
            "expression": format!("({})({args})", wrap(function)),
            "awaitPromise": true,
            "target": { "context": self.context },
            "resultOwnership": "none",
        })
    }

    /// One form of [`Wire::evaluate`]: `source` as BiDi's `script.evaluate`
    /// runs it, its value as JSON.
    fn evaluate_once(&mut self, source: &str, timeout: Duration) -> Result<Value> {
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
            let details = &r["exceptionDetails"];
            let text = details["text"].as_str().unwrap_or("the script threw").to_string();
            // Only code that ran has a stack: a SyntaxError without one is
            // the source not parsing (see `DidNotParse`).
            let ran =
                details.pointer("/stackTrace/callFrames").and_then(Value::as_array).is_some_and(|f| !f.is_empty());
            if !ran && text.contains("SyntaxError") {
                return Err(crate::cdp::page::DidNotParse(text).into());
            }
            bail!("{text}");
        }
        Ok(Self::remote_to_json(&r["result"]))
    }

    /// What a `script.callFunction` answer holds: the function's text.
    fn call_answer(r: &Value) -> Result<String> {
        if r["type"] == "exception" {
            bail!("{}", r["exceptionDetails"]["text"].as_str().unwrap_or("the page's script failed"));
        }
        match &r["result"]["value"] {
            Value::String(s) => Ok(s.clone()),
            _ => bail!("the page's answer was not text ({})", r["result"]["type"]),
        }
    }

    /// Two commands sent together, and the answer to each: the browser
    /// works on both at once, one round trip for the two.
    fn send_two(
        &mut self,
        a: (&str, Value),
        b: (&str, Value),
        timeout: Duration,
    ) -> Result<(Result<Value>, Result<Value>)> {
        if self.closed {
            bail!("{}: the browser is gone", a.0);
        }
        let ids = [self.write(a.0, a.1)?, self.write(b.0, b.1)?];
        let names = [a.0, b.0];
        let mut got: [Option<Result<Value>>; 2] = [None, None];
        let deadline = Instant::now() + timeout;
        while got.iter().any(Option::is_none) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                bail!("{}: no answer from the browser in {} ms", names[0], timeout.as_millis());
            }
            let Some(mut v) = self.read_one(left.min(READ_SLICE))? else { continue };
            if v["type"] == "event" {
                self.on_event(v);
                continue;
            }
            let Some(i) = ids.iter().position(|id| v["id"].as_u64() == Some(*id)) else { continue };
            got[i] = Some(match v["type"].as_str() {
                Some("success") => Ok(v["result"].take()),
                _ => Err(anyhow!(
                    "{}: {}: {}",
                    names[i],
                    v["error"].as_str().unwrap_or("error"),
                    v["message"].as_str().unwrap_or("")
                )),
            });
        }
        let [a, b] = got;
        Ok((a.unwrap_or_else(|| Err(anyhow!("no answer"))), b.unwrap_or_else(|| Err(anyhow!("no answer")))))
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
        let params = self.call_params(function, args_json);
        let mut tried = false;
        loop {
            match self.send("script.evaluate", params.clone(), timeout) {
                Ok(r) => return Self::call_answer(&r),
                // A document replaced while the call ran (a navigation): once
                // more, in the new one.
                Err(e)
                    if !tried && {
                        let m = format!("{e:#}");
                        // (Chromium's BiDi words it "Cannot find context".)
                        m.contains("no such frame") || m.contains("discarded") || m.contains("Cannot find context")
                    } =>
                {
                    tried = true;
                    self.settle(timeout, Watch::Planned);
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// `eval` as on Chrome: one expression (an arrow function is called);
    /// else the source run as a script, its last expression the result;
    /// else as the body of an async function, `await` and `return` working.
    fn evaluate(&mut self, source: &str, timeout: Duration) -> Result<Value> {
        let forms = [
            crate::cdp::page::call_if_function(source),
            source.to_string(),
            format!("(async () => {{\n{source}\n}})()"),
        ];
        // Only source that did not parse goes on to the next form (see
        // `DidNotParse`); when none parses, the error of the source as
        // written, which points into the user's own text.
        let mut unparsed = Vec::new();
        for form in forms {
            match self.evaluate_once(&form, timeout) {
                Err(e) if crate::cdp::page::did_not_parse(&e) => unparsed.push(e),
                other => return other,
            }
        }
        Err(unparsed.into_iter().nth(1).unwrap_or_else(|| anyhow!("the script did not parse")))
    }

    fn call_and_tabs(&mut self, function: &str, args_json: &str, timeout: Duration) -> Result<(String, Vec<String>)> {
        let (called, tree) = self.send_two(
            ("script.evaluate", self.call_params(function, args_json)),
            ("browsingContext.getTree", json!({ "maxDepth": 0 })),
            timeout,
        )?;
        let answer = match called {
            Ok(r) => Self::call_answer(&r)?,
            // The document was replaced as the call came (a navigation):
            // made again by itself, which waits for the new one.
            Err(_) => self.call(function, args_json, timeout)?,
        };
        let all = match tree {
            Ok(tree) => self.tops_of(&tree),
            Err(_) => self.top_contexts()?,
        };
        Ok((answer, self.tabs_among(all)))
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

    fn set_files(&mut self, files: &[String], timeout: Duration) -> Result<()> {
        let r = self.send(
            "script.callFunction",
            json!({
                "functionDeclaration": format!("() => {}", super::PICKED),
                "awaitPromise": false,
                "target": { "context": self.context },
                "resultOwnership": "none",
            }),
            timeout,
        )?;
        let node = r["result"]["sharedId"]
            .as_str()
            .ok_or_else(|| anyhow!("the file input is gone from the page; run `ax` again"))?
            .to_string();
        self.send(
            "input.setFiles",
            json!({ "context": self.context, "element": { "sharedId": node }, "files": files }),
            timeout,
        )?;
        Ok(())
    }

    fn set_viewport(&mut self, size: Option<(u32, u32)>, scale: Option<f64>) -> Result<()> {
        let mut params = match size {
            Some((width, height)) => json!({ "viewport": { "width": width, "height": height } }),
            None => json!({ "viewport": null, "devicePixelRatio": null }),
        };
        if let (Some(_), Some(scale)) = (size, scale) {
            params["devicePixelRatio"] = json!(scale);
        }
        self.viewport = size.map(|_| params.clone());
        params["context"] = json!(self.context);
        self.send("browsingContext.setViewport", params, Duration::from_secs(30))?;
        Ok(())
    }

    fn emulate(&mut self, method: &str, params: Value) -> Result<()> {
        let mut scoped = params.clone();
        if self.attached {
            // In the user's own Firefox: this session's tabs only.
            scoped["contexts"] = json!(self.own);
        } else {
            // Navigera's own Firefox: every tab, and the ones opened later.
            scoped["userContexts"] = json!(["default"]);
        }
        self.send(method, scoped, Duration::from_secs(30))?;
        self.emulations.retain(|(m, _)| m != method);
        if params.as_object().is_some_and(|o| o.values().any(|v| !v.is_null())) {
            self.emulations.push((method.to_string(), params));
        }
        Ok(())
    }

    fn add_init_script(&mut self, source: &str) -> Result<()> {
        let mut params = json!({ "functionDeclaration": format!("() => {{\n{source}\n}}") });
        if self.attached {
            // In the user's own Firefox: this session's tabs only.
            params["contexts"] = json!(self.own);
        }
        let r = self.send("script.addPreloadScript", params, Duration::from_secs(30))?;
        let id = r["script"].as_str().ok_or_else(|| anyhow!("the browser named no script"))?;
        self.preloads.push(id.to_string());
        Ok(())
    }

    fn clear_init_scripts(&mut self) -> Result<usize> {
        let ids = std::mem::take(&mut self.preloads);
        for id in &ids {
            self.send("script.removePreloadScript", json!({ "script": id }), Duration::from_secs(30))?;
        }
        Ok(ids.len())
    }

    fn add_route(&mut self, route: Route) -> Result<()> {
        if matches!(route.action, RouteAction::Modify { .. }) {
            bail!("--modify: not over WebDriver BiDi yet (a response's body cannot be read and sent on changed); run it with --engine chrome");
        }
        let had = self.routes.clone();
        self.routes.retain(|r| r.pattern != route.pattern);
        self.routes.push(route);
        if let Err(e) = self.sync_intercept() {
            self.routes = had;
            return Err(e);
        }
        Ok(())
    }

    fn remove_routes(&mut self, pattern: Option<&str>) -> Result<usize> {
        let before = self.routes.len();
        self.routes.retain(|r| pattern.is_some_and(|p| r.pattern != p));
        self.sync_intercept()?;
        Ok(before - self.routes.len())
    }

    fn routes(&self) -> Vec<Route> {
        self.routes.clone()
    }

    fn idle(&mut self) {
        // Only what has already arrived: a read that finds nothing ends it.
        if self.closed {
            return;
        }
        for _ in 0..256 {
            match self.read_one(Duration::from_millis(1)) {
                Ok(Some(v)) if v["type"] == "event" => self.on_event(v),
                Ok(Some(_)) => {}
                _ => break,
            }
        }
    }

    fn settle(&mut self, timeout: Duration, watch: Watch) {
        // A navigation the action started announces itself within a few
        // milliseconds; then its load is waited for.
        let watch_until = Instant::now()
            + match watch {
                Watch::No => Duration::ZERO,
                Watch::Brief => NAV_START_WATCH,
                Watch::Planned => super::NAV_PLANNED_WATCH,
            };
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

    fn pdf(&mut self, landscape: bool, background: bool) -> Result<Vec<u8>> {
        let r = self.send(
            "browsingContext.print",
            json!({
                "context": self.context,
                "orientation": if landscape { "landscape" } else { "portrait" },
                "background": background,
            }),
            Duration::from_secs(60),
        )?;
        let data = r["data"].as_str().ok_or_else(|| anyhow!("the browser sent no PDF"))?;
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| anyhow!("the PDF is not base64 ({e})"))
    }

    fn history(&mut self, delta: i64) -> Result<()> {
        self.nav = None;
        self.send(
            "browsingContext.traverseHistory",
            json!({ "context": self.context, "delta": delta }),
            Duration::from_secs(30),
        )?;
        self.settle(Duration::from_secs(30), Watch::Brief);
        Ok(())
    }

    fn reload(&mut self, timeout: Duration) -> Result<()> {
        self.send("browsingContext.reload", json!({ "context": self.context, "wait": "complete" }), timeout)?;
        Ok(())
    }

    fn tabs(&mut self) -> Result<Vec<String>> {
        let all = self.top_contexts()?;
        Ok(self.tabs_among(all))
    }

    fn current_tab(&self) -> String {
        self.context.clone()
    }

    fn front(&mut self) {
        // Headless it may refuse.
        let _ = self.send("browsingContext.activate", json!({ "context": self.context }), Duration::from_secs(10));
    }

    fn tab_new(&mut self) -> Result<String> {
        let mut params = json!({ "type": "tab" });
        if self.attached {
            // Beside this session's tab, in its window, not the user's.
            params["referenceContext"] = json!(self.context);
        }
        let r = self.send("browsingContext.create", params, Duration::from_secs(30))?;
        let id = r["context"].as_str().map(str::to_string).ok_or_else(|| anyhow!("the browser made no tab"))?;
        if let Some(mut viewport) = self.viewport.clone() {
            viewport["context"] = json!(id);
            let _ = self.send("browsingContext.setViewport", viewport, Duration::from_secs(30));
        }
        if self.attached {
            self.own.push(id.clone());
            for (method, params) in self.emulations.clone() {
                let mut params = params;
                params["contexts"] = json!([id]);
                let _ = self.send(&method, params, Duration::from_secs(30));
            }
            let was = std::mem::replace(&mut self.context, id.clone());
            self.listen_in(&id, Duration::from_secs(10));
            self.context = was;
        }
        Ok(id)
    }

    fn tab_select(&mut self, id: &str) -> Result<()> {
        if !self.ours(Some(id)) {
            bail!("tab {id} is not one this session opened: in your own Firefox it works in its own tabs only");
        }
        // `activate` brings a headed tab to the front; headless it may refuse.
        let _ = self.send("browsingContext.activate", json!({ "context": id }), Duration::from_secs(10));
        self.context = id.to_string();
        Ok(())
    }

    fn tab_close(&mut self, id: &str) -> Result<()> {
        if !self.ours(Some(id)) {
            bail!("tab {id} is not one this session opened: your own tabs are yours to close");
        }
        self.own.retain(|o| o != id);
        self.frames.retain(|(_, tab)| tab != id);
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
        // At most a buffer's worth: a page that never stops logging does
        // not hold the command.
        for _ in 0..CONSOLE_MAX {
            match self.read_one(Duration::from_millis(1)) {
                Ok(Some(v)) if v["type"] == "event" => self.on_event(v),
                Ok(Some(_)) => {}
                _ => break,
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
            if self.attached {
                // The user's browser stays: this session's tabs go, and
                // the session ends so the next client can have one. The
                // tabs its pages opened since the last look go with them.
                let _ = self.top_contexts();
                for id in std::mem::take(&mut self.own) {
                    let _ = self.send("browsingContext.close", json!({ "context": id }), Duration::from_secs(5));
                }
                let _ = self.send("session.end", json!({}), Duration::from_secs(5));
            } else if self.classic.is_none() {
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

    /// A running Firefox is found by the file its remote agent keeps in
    /// the profile: the newest first, loopback only, nothing else read.
    #[test]
    fn a_running_firefox_is_found_by_its_profile_file() {
        let root = tempfile::tempdir().unwrap();
        let put = |profile: &str, text: &str| {
            let dir = root.path().join(profile);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("WebDriverBiDiServer.json"), text).unwrap();
        };
        put("a.default", r#"{"ws_host":"127.0.0.1","ws_port":9222}"#);
        std::thread::sleep(Duration::from_millis(20));
        put("b.work", r#"{"ws_host":"localhost","ws_port":41234}"#);
        put("c.elsewhere", r#"{"ws_host":"203.0.113.9","ws_port":9222}"#);
        put("d.broken", "not json");
        put("e.noport", r#"{"ws_host":"127.0.0.1","ws_port":0}"#);
        put("f.huge", &format!(r#"{{"ws_host":"127.0.0.1","ws_port":1,"pad":"{}"}}"#, "x".repeat(5000)));
        std::fs::create_dir_all(root.path().join("g.stopped")).unwrap();
        let roots = [root.path().to_path_buf(), root.path().join("no-such-dir")];
        assert_eq!(
            firefox_endpoints_in(&roots),
            vec!["ws://localhost:41234/session".to_string(), "ws://127.0.0.1:9222/session".to_string()]
        );
    }

    /// A tab this session's page opened is its own, and so is a tab that
    /// one opened; the user's tabs and what they open stay theirs.
    #[test]
    fn a_tab_opened_by_an_own_tab_is_adopted() {
        let tree = json!({ "contexts": [
            { "context": "user", "originalOpener": null },
            { "context": "second", "originalOpener": "first" },
            { "context": "ours", "originalOpener": null },
            { "context": "first", "originalOpener": "ours" },
            { "context": "theirs", "originalOpener": "user" },
            { "context": "old", "originalOpener": "closed" },
        ] });
        let mut own = vec!["ours".to_string()];
        adopt_opened(&mut own, &tree);
        assert_eq!(own, ["ours", "first", "second"]);
        adopt_opened(&mut own, &tree);
        assert_eq!(own.len(), 3);
        // An answer with no tabs changes nothing.
        adopt_opened(&mut own, &json!({}));
        assert_eq!(own.len(), 3);
    }

    /// A frame in one of the session's tabs is heard as that tab, at any
    /// depth; the user's frames are not; a tab its page opens is its own
    /// at once; a closed frame takes the frames inside it along.
    #[test]
    fn a_frame_in_an_own_tab_belongs_to_that_tab() {
        let mut own = vec!["ours".to_string()];
        let mut frames: Vec<(String, String)> = Vec::new();
        let made =
            |own: &mut Vec<String>, frames: &mut Vec<(String, String)>, v: Value| context_created(own, frames, &v);
        made(&mut own, &mut frames, json!({ "context": "f1", "parent": "ours" }));
        made(&mut own, &mut frames, json!({ "context": "f2", "parent": "f1" }));
        made(&mut own, &mut frames, json!({ "context": "theirs", "parent": "user" }));
        made(&mut own, &mut frames, json!({ "context": "f1", "parent": "ours" }));
        assert_eq!(frames.len(), 2);
        assert_eq!(tab_of(&own, &frames, "f2"), Some("ours"));
        assert_eq!(tab_of(&own, &frames, "ours"), Some("ours"));
        assert_eq!(tab_of(&own, &frames, "theirs"), None);
        assert_eq!(tab_of(&own, &frames, "user"), None);
        // Opened from a frame of ours: ours. Opened by the user's tab: not.
        made(&mut own, &mut frames, json!({ "context": "pop", "parent": null, "originalOpener": "f2" }));
        made(&mut own, &mut frames, json!({ "context": "other", "parent": null, "originalOpener": "user" }));
        made(&mut own, &mut frames, json!({ "context": "plain", "parent": null, "originalOpener": null }));
        assert_eq!(own, ["ours", "pop"]);
        made(&mut own, &mut frames, json!({ "context": "f3", "parent": "pop" }));
        assert_eq!(tab_of(&own, &frames, "f3"), Some("pop"));
        // The outer frame goes, and names the one inside it.
        context_destroyed(&mut frames, &json!({ "context": "f1", "children": [{ "context": "f2", "children": [] }] }));
        assert_eq!(frames, [("f3".to_string(), "pop".to_string())]);
        context_destroyed(&mut frames, &json!({ "context": "pop", "children": null }));
        assert!(frames.is_empty());
        // More frames than are kept: the oldest are dropped.
        for i in 0..FRAMES_MAX + 10 {
            made(&mut own, &mut frames, json!({ "context": format!("n{i}"), "parent": "ours" }));
        }
        assert_eq!(frames.len(), FRAMES_MAX);
        assert_eq!(tab_of(&own, &frames, "n0"), None);
        assert_eq!(tab_of(&own, &frames, &format!("n{}", FRAMES_MAX + 9)), Some("ours"));
    }

    #[test]
    fn an_attach_spec_names_firefox_endpoints() {
        let one = |spec: &str| attach_candidates(spec).unwrap();
        assert_eq!(one("9333"), vec!["ws://127.0.0.1:9333/session"]);
        assert_eq!(one("localhost:9333"), vec!["ws://localhost:9333/session"]);
        assert_eq!(one("ws://127.0.0.1:9333"), vec!["ws://127.0.0.1:9333/session"]);
        assert_eq!(one("ws://127.0.0.1:9333/session"), vec!["ws://127.0.0.1:9333/session"]);
        // By name: whatever is running, then the usual port, once.
        let by_name = one("firefox");
        assert_eq!(by_name.last().map(String::as_str), Some("ws://127.0.0.1:9222/session"));
        assert_eq!(by_name.iter().filter(|u| u.as_str() == "ws://127.0.0.1:9222/session").count(), 1);
        for bad in ["wss://x.test/session?key=secret", "http://127.0.0.1:9222", "chrome", "host:0", ":9222"] {
            let err = attach_candidates(bad).unwrap_err().to_string();
            assert!(!err.contains("secret"), "{err}");
        }
    }

    /// Nothing listening: the error says how to start a Firefox that can
    /// be attached to.
    #[test]
    fn attaching_to_no_firefox_says_how_to_start_one() {
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let err = match Bidi::attach_firefox(&free.to_string(), json!({}), Duration::from_secs(2)) {
            Ok(_) => panic!("nothing listens there"),
            Err(e) => format!("{e:#}"),
        };
        assert!(err.contains("--remote-debugging-port") && err.contains("--engine firefox"), "{err}");
    }
}
