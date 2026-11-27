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
use crate::cdp::downloads::Download;
use crate::cdp::{Route, RouteAction};

/// Events every session listens to.
const EVENTS: &[&str] = &[
    "log.entryAdded",
    "browsingContext.userPromptOpened",
    "browsingContext.navigationStarted",
    "browsingContext.domContentLoaded",
    "browsingContext.load",
];

/// The downloads a page starts, subscribed apart: a server without them
/// still drives pages, and `downloads` then lists none.
const DOWNLOAD_EVENTS: &[&str] = &["browsingContext.downloadWillBegin", "browsingContext.downloadEnd"];

/// A navigation's URL after its redirects, before the document's scripts
/// (Firefox 140 on): `set geo` grants a redirect's target there. Subscribed
/// apart, as a server without it still drives pages.
const COMMIT_EVENTS: &[&str] = &["browsingContext.navigationCommitted"];

/// `network`: the requests of this session's tabs, subscribed apart (a
/// server without them still drives pages, and `network` then says so).
const NETWORK_EVENTS: &[&str] =
    &["network.beforeRequestSent", "network.responseStarted", "network.responseCompleted", "network.fetchError"];

/// The largest response or request body the browser keeps for
/// `network <n>` (the log keeps 16 KiB of a request's).
const BODY_MAX: u64 = 10 << 20;

/// A data collector's parameters: response bodies, and request bodies too
/// when `requests`.
fn collect_params(requests: bool) -> Value {
    let types: &[&str] = if requests { &["request", "response"] } else { &["response"] };
    json!({ "dataTypes": types, "maxEncodedDataSize": BODY_MAX, "collectorType": "blob" })
}

/// What the `network` that started the request log ([`Bidi::start_log`]) says.
const LOG_STARTED: &str = "on this engine navigera listens to requests from the session's first `network` on, \
     which is now (a browser listened to slows every page load); reload or goto again to list a page's requests";

/// How long `network` waits for the answers of requests sent just before.
const CATCH_UP: Duration = Duration::from_millis(150);

/// Seconds since the epoch, as BiDi's timestamps (in ms) over 1000.
fn now_s() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// One hop of a request in the log: the entry `network` lists, with what
/// [`log_request`] places its events by.
#[derive(Debug)]
struct Hop {
    r: crate::cdp::logs::Request,
    /// Its place in the request's redirect chain (`redirectCount`), which
    /// with the request's id names it.
    count: u64,
    /// The redirect status its response's own reason phrase names, when
    /// the status it came with is no redirect's.
    phrase: Option<u64>,
}

/// A request listed (numbered after `cleared`), sent within the last 2 s,
/// with no status and no error yet.
fn fresh_pending(log: &VecDeque<Hop>, cleared: u64, now: f64) -> bool {
    log.iter()
        .rev()
        .take_while(|h| h.r.n > cleared)
        .take(64)
        .any(|h| h.r.status.is_none() && h.r.failed.is_none() && (now - h.r.started).abs() < 2.0)
}

/// The requests `network` lists: those after the last `--clear`.
fn listed(log: &VecDeque<Hop>, cleared: u64) -> Vec<crate::cdp::logs::Request> {
    log.iter().filter(|h| h.r.n > cleared).map(|h| h.r.clone()).collect()
}

/// The redirect status a standard reason phrase names (HTTP/1.x; HTTP/2
/// has none).
fn redirect_phrase(text: &str) -> Option<u64> {
    Some(match text.trim().to_ascii_lowercase().as_str() {
        "multiple choices" => 300,
        "moved permanently" => 301,
        "found" | "moved temporarily" => 302,
        "see other" => 303,
        "temporary redirect" => 307,
        "permanent redirect" => 308,
        _ => return None,
    })
}

/// Most requests kept, as on Chrome: older ones are dropped.
const REQUESTS_MAX: usize = 1000;

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

/// A WebSocket to a BiDi endpoint (`ws://` only).
pub(crate) fn open_ws(url: &str, timeout: Duration) -> Result<WebSocket<MaybeTlsStream<TcpStream>>> {
    // The endpoint as shown: a grid's may carry a key.
    let shown = super::classic::mask(url);
    let rest = url.strip_prefix("ws://").ok_or_else(|| {
        anyhow!("BiDi endpoint {shown}: only ws:// endpoints on this machine or a network are served (no wss:// yet)")
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
    let (ws, _) = tungstenite::client::client_with_config(
        url,
        MaybeTlsStream::Plain(stream),
        Some(crate::cdp::client::ws_config()),
    )
    .map_err(|e| anyhow!("BiDi handshake with {shown} failed: {e}"))?;
    Ok(ws)
}

pub struct Bidi {
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
    /// `screencast start`, until `stop`: its looks go out and come back on
    /// this connection ([`Self::read_one`]).
    looks: Option<super::screencast::Looks>,
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
    /// In the user's browser, an intercept per tab that became this
    /// session's after the rules were set ([`Self::adopt`]), and the
    /// commands that ask for one whose answers have not come yet.
    more_intercepts: Vec<String>,
    asked_intercepts: Vec<u64>,
    /// `init-script`: the preload scripts this session added.
    preloads: Vec<String>,
    /// `set viewport`, for the tabs this session opens after it.
    viewport: Option<Value>,
    /// `set locale|timezone|media|offline|ua`: each `emulation.*` command
    /// in force (its own parameters), sent again to a tab this session
    /// opens in the user's Firefox.
    emulations: Vec<(String, Value)>,
    /// `set geo`: the origins granted the geolocation permission while it
    /// is on (`None`: off, or a Firefox a person may be using, where it
    /// grants none: [`persons_browser`]), each with the setting it had
    /// before when that was read (`set geo` reads the page's own in a
    /// headless Firefox you attached to); `off`, and the session's end,
    /// give each that setting back.
    geo: Option<Vec<(String, Option<String>)>>,
    /// While `set geo` reads the page's own setting: the URLs navigations
    /// reached meanwhile, granted after it ([`Self::geo_grant`]).
    geo_held: Option<Vec<String>>,
    /// `downloads`: what this session's pages downloaded, oldest first
    /// (`guid` is the download's navigation id).
    downloads: Vec<Download>,
    /// `network`: the requests of this session's tabs, oldest first, and
    /// the number of the last one; `None`: not listened to yet, or the
    /// browser sends no network events (then `route` subscribes on its own).
    requests: Option<VecDeque<Hop>>,
    /// Whether the request log was started ([`Self::start_log`]), and
    /// whether the `network` that started it has yet to say so.
    log_started: bool,
    log_note: bool,
    last_request: u64,
    /// The last request `network --clear` took. Those up to it stay in
    /// the log, not shown, so that an event of theirs that comes after
    /// does not list them again.
    cleared: u64,
    /// The data collector that keeps response bodies for `network <n>` (in
    /// the user's browser, its first tab's: [`Self::adopt`] adds one per tab).
    collector: Option<String>,
    /// Whether the collectors keep request bodies too: a BiDi event carries
    /// none, only its size.
    collects_requests: bool,
    /// In the user's browser, each tab's collector, removed when the tab
    /// closes ([`Self::tab_gone`]), and the commands that ask for one whose
    /// answers have not come yet (`None`: its tab closed meanwhile).
    tab_collectors: Vec<(String, String)>,
    asked_collectors: Vec<(u64, Option<String>)>,
    /// The frames inside `own` tabs, each with its tab.
    frames: Vec<(String, String)>,
    closed: bool,
}

/// The `emulation.*` command of `set geo`, which also grants its permission.
pub const GEO: &str = "emulation.setGeolocationOverride";

/// A browser a person may be using: a Firefox you attached to, with a
/// window. A permission there is the origin's in the whole profile, so a
/// tab of theirs on that origin would read the browser's own position
/// without asking: `set geo` grants none there.
pub fn persons_browser(browser: &Value) -> bool {
    browser["attached"] == true && browser["headless"] != true
}

/// `set geo`'s answer in such a Firefox.
pub const GEO_ASKS: &str = "no location permission granted: in your own Firefox it would be the site's in the whole profile, your tabs too. A page asks you, as without navigera";

/// A BiDi header list (`[{name, value: {type, value}}]`) as the object CDP
/// gives (a repeated name's values joined by a newline, as Chrome does).
fn header_object(list: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for h in list.as_array().into_iter().flatten() {
        let (Some(name), Some(value)) = (h["name"].as_str(), h["value"]["value"].as_str()) else { continue };
        match out.get_mut(name) {
            Some(Value::String(had)) => {
                had.push('\n');
                had.push_str(value);
            }
            _ => {
                out.insert(name.to_string(), json!(value));
            }
        }
    }
    Value::Object(out)
}

/// A request's kind under Chrome's names, from what Fetch says it is for.
fn request_kind(request: &Value) -> &'static str {
    match (request["initiatorType"].as_str().unwrap_or(""), request["destination"].as_str().unwrap_or("")) {
        ("xmlhttprequest", _) => "XHR",
        ("fetch" | "beacon", _) => "Fetch",
        (_, "document" | "iframe" | "frame") => "Document",
        (_, "script" | "worker" | "sharedworker" | "serviceworker") => "Script",
        (_, "style") => "Stylesheet",
        (_, "image") => "Image",
        (_, "font") => "Font",
        (_, "audio" | "video" | "track") => "Media",
        (_, "manifest") => "Manifest",
        _ => "Other",
    }
}

/// A request's end by its fetch timings (`request.timings`, in ms):
/// `started` (in s) plus the time from `requestTime` to `responseEnd`;
/// None while the response has not ended.
fn timed_end(request: &Value, started: f64) -> Option<f64> {
    let timings = &request["timings"];
    let (from, to) = (timings["requestTime"].as_f64()?, timings["responseEnd"].as_f64()?);
    (to > 0.0 && to >= from).then(|| started + (to - from) / 1000.0)
}

/// One `network.*` event into the request log, as Chrome's would be: each
/// hop of a redirect (the request's id and its `redirectCount`) is an
/// entry of its own, opened by whichever of its events comes first.
/// chromedriver's BiDi mapper holds a hop's `beforeRequestSent` until a
/// CDP event from the browser process has come, so the hop's response can
/// come before it, and the hop a redirect leads to gets none at all when
/// that event went to the hop before: a response that found no entry was
/// dropped, and the request read as pending, or was not listed.
///
/// The same mix-up can give a hop that redirected the status (and raw
/// headers) of the hop it led to, while its reason phrase stays its own:
/// once a later hop shows it redirected, a status that is no redirect's
/// is taken from that phrase when it names one (HTTP/2 sends none).
fn log_request(log: &mut VecDeque<Hop>, last: &mut u64, method: &str, params: &Value) {
    if !NETWORK_EVENTS.contains(&method) {
        return;
    }
    let request = &params["request"];
    let Some(id) = request["request"].as_str() else { return };
    // BiDi times are milliseconds since the epoch.
    let at = params["timestamp"].as_f64().map(|ms| ms / 1000.0);
    let count = params["redirectCount"].as_u64().unwrap_or(0);
    // A later hop of the request is after it in the log: one pass from the
    // end finds the hop and whether it redirected.
    let mut redirected = false;
    let found = log.iter().rposition(|h| {
        redirected |= h.r.id == id && h.count > count;
        h.r.id == id && h.count == count
    });
    let i = match found {
        // The hop announced again (an intercepted request is, for each
        // phase it is held in), or after its response: the one entry.
        Some(_) if method == "network.beforeRequestSent" => return,
        Some(i) => i,
        None => {
            for h in log.iter_mut().filter(|h| h.r.id == id && h.count < count) {
                // A redirect whose own response was not reported: the hop
                // ends where the next begins.
                if h.r.ended.is_none() && h.r.failed.is_none() {
                    h.r.ended = at.or(Some(h.r.started));
                }
                if let Some(status) = h.phrase.take() {
                    h.r.status = Some(status);
                }
            }
            *last += 1;
            let method = request["method"].as_str().unwrap_or("GET");
            // A browser sends no body with a GET or a HEAD; chromium-bidi
            // gives the GET after a 303 the size of the POST before it.
            let size = if matches!(method, "GET" | "HEAD") { 0 } else { request["bodySize"].as_u64().unwrap_or(0) };
            let started = at.unwrap_or(0.0);
            if log.len() >= REQUESTS_MAX {
                log.pop_front();
            }
            let r = crate::cdp::logs::Request {
                n: *last,
                id: id.to_string(),
                method: method.to_string(),
                url: crate::cdp::logs::kept_url(request["url"].as_str().unwrap_or("")),
                kind: request_kind(request).to_string(),
                request_headers: crate::cdp::logs::kept_headers(Some(&header_object(&request["headers"]))),
                has_post_data: size > 0,
                started,
                wall: started,
                ..Default::default()
            };
            log.push_back(Hop { r, count, phrase: None });
            log.len() - 1
        }
    };
    match method {
        // The status and headers when they arrive (as Chrome's
        // `responseReceived`), the size and end when the body has: a
        // server may report only the first (chromedriver 147 over BiDi
        // sends no `responseCompleted` for a fetch).
        "network.responseStarted" | "network.responseCompleted" => {
            let response = &params["response"];
            let status = response["status"].as_u64();
            let phrase = status
                .filter(|s| !(300..400).contains(s))
                .and(response["statusText"].as_str().and_then(redirect_phrase));
            let Hop { r, phrase: own, .. } = &mut log[i];
            *own = phrase;
            r.status = if redirected { phrase.or(status) } else { status };
            r.mime = response["mimeType"].as_str().unwrap_or("").split(';').next().unwrap_or("").trim().to_string();
            r.response_headers = crate::cdp::logs::kept_headers(Some(&header_object(&response["headers"])));
            r.from_cache = response["fromCache"] == true;
            if method == "network.responseCompleted" {
                r.size = response["bytesReceived"].as_u64();
                let end = at.unwrap_or(r.started);
                // A server that stamps every event of a request with its
                // start (chromedriver's): the fetch's own times, which it
                // counts from the request.
                r.ended = Some(if end > r.started { end } else { timed_end(request, r.started).unwrap_or(end) });
            }
        }
        "network.fetchError" => {
            let r = &mut log[i].r;
            let why = params["errorText"].as_str().unwrap_or("failed");
            r.failed = Some(if why.is_empty() { "failed".into() } else { why.to_string() });
            r.ended = at.or(Some(r.started));
        }
        _ => {}
    }
}

/// `scheme://host[:port]` of an http(s) URL, without its user info; None
/// for any other (about:blank, data:, file:), which has no origin to grant.
fn page_origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    (!host.is_empty()).then(|| format!("{}://{}", scheme.to_ascii_lowercase(), host.to_ascii_lowercase()))
}

/// The setting to give an origin back at `set geo off`, from the one it had
/// before (`None`: not read): one that was `granted` keeps navigera's grant
/// (nothing to send); one that was `denied` is denied again; else asking,
/// which is `prompt` (Firefox: no setting of the origin's own).
fn geo_restore(before: Option<&str>) -> Option<&'static str> {
    match before {
        Some("granted") => None,
        Some("denied") => Some("denied"),
        _ => Some("prompt"),
    }
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
                "headless": caps["moz:headless"].as_bool().unwrap_or(false),
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
        let ws = open_ws(url, timeout)?;
        // The endpoint as shown: a grid's may carry a key.
        let shown = super::classic::mask(url);
        Ok(Self {
            ws,
            looks: None,
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
            more_intercepts: Vec::new(),
            asked_intercepts: Vec::new(),
            viewport: None,
            emulations: Vec::new(),
            geo: None,
            geo_held: None,
            downloads: Vec::new(),
            requests: None,
            log_started: false,
            log_note: false,
            last_request: 0,
            cleared: 0,
            collector: None,
            collects_requests: false,
            tab_collectors: Vec::new(),
            asked_collectors: Vec::new(),
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
        let _ = self.send("session.subscribe", json!({ "events": DOWNLOAD_EVENTS }), timeout);
        let _ = self.send("session.subscribe", json!({ "events": COMMIT_EVENTS }), timeout);
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

    /// Start the request log, once, at the session's first `network`: the
    /// network events of its tabs and the collectors that keep their
    /// bodies. Not at launch: a browser that sends them does work for each
    /// request a page makes, and a goto waits for it (a page of 300 images
    /// took three times as long on Firefox 160, a third longer over
    /// chromedriver), whether `network` is read or not. In the user's
    /// Firefox, only navigera's own tabs ([`Self::adopt`] adds the next):
    /// theirs are not reported to it, nor slowed by being.
    fn start_log(&mut self, timeout: Duration) {
        if std::mem::replace(&mut self.log_started, true) {
            return;
        }
        let mut subscribe = json!({ "events": NETWORK_EVENTS });
        if self.attached {
            subscribe["contexts"] = json!(self.own);
        }
        if self.send("session.subscribe", subscribe, timeout).is_err() {
            return;
        }
        self.requests = Some(VecDeque::new());
        self.log_note = true;
        if !self.attached {
            self.collector = self.add_collector(None, timeout);
            return;
        }
        // A collector per tab, removed when its tab closes ([`Self::tab_gone`]).
        for tab in self.own.clone() {
            let Some(x) = self.add_collector(Some(json!([tab])), timeout) else { break };
            self.collector.get_or_insert_with(|| x.clone());
            self.tab_collectors.push((tab, x));
        }
    }

    /// A data collector for `contexts` (all the session's when None):
    /// request bodies too where the browser keeps them; one that refuses
    /// the data type keeps response bodies, and so do the session's later
    /// collectors. A Firefox before network.addDataCollector lists
    /// requests without their bodies.
    fn add_collector(&mut self, contexts: Option<Value>, timeout: Duration) -> Option<String> {
        let tries: &[bool] = if self.collector.is_some() { &[self.collects_requests] } else { &[true, false] };
        for &requests in tries {
            let mut collect = collect_params(requests);
            if let Some(contexts) = &contexts {
                collect["contexts"] = contexts.clone();
            }
            match self.send("network.addDataCollector", collect, timeout) {
                Ok(r) => {
                    let x = r["collector"].as_str().map(str::to_string);
                    self.collects_requests = requests && x.is_some();
                    return x;
                }
                Err(e) if requests && format!("{e:#}").contains(": invalid argument:") => {}
                Err(_) => return None,
            }
        }
        None
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

    /// One message, or None when `slice` passed without one. While a
    /// recording is on, a look that is due goes out first, and the answer
    /// of the one on its way is taken here, not returned.
    fn read_one(&mut self, slice: Duration) -> Result<Option<Value>> {
        let until = Instant::now() + slice;
        loop {
            let wait = self.look(until);
            if let MaybeTlsStream::Plain(s) = self.ws.get_mut() {
                s.set_read_timeout(Some(wait.max(Duration::from_millis(1)))).ok();
            }
            match self.ws.read() {
                Ok(Message::Text(t)) => {
                    // Firefox writes a lone UTF-16 surrogate as `\ud800`, which
                    // JSON in UTF-8 cannot hold: read as U+FFFD.
                    let text = t.as_str();
                    let fixed =
                        |e| crate::cdp::lone_surrogates_replaced(text).map_or(Err(e), |f| serde_json::from_str(&f));
                    let v: Value = serde_json::from_str(text)
                        .or_else(fixed)
                        .map_err(|e| anyhow!("the browser sent no JSON ({e})"))?;
                    if let Some(looks) = self.looks.as_mut().filter(|l| l.answers(&v)) {
                        looks.answered(&v);
                        if Instant::now() < until {
                            continue;
                        }
                        return Ok(None);
                    }
                    if v["type"] != "event" {
                        if !self.asked_intercepts.is_empty() {
                            self.took_intercept(&v);
                        }
                        if !self.asked_collectors.is_empty() {
                            self.took_collector(&v);
                        }
                    }
                    return Ok(Some(v));
                }
                Ok(Message::Close(_)) => {
                    self.closed = true;
                    bail!("the browser closed the BiDi connection")
                }
                Ok(_) => return Ok(None),
                Err(tungstenite::Error::Io(e))
                    if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
                {
                    // A read cut short for the next look reads on.
                    if Instant::now() >= until {
                        return Ok(None);
                    }
                }
                Err(e) => {
                    self.closed = true;
                    bail!("the BiDi connection failed ({e})")
                }
            }
        }
    }

    /// A recording's look, if one is due, sent; how long a read may wait
    /// before the recording needs it back, at most until `until`.
    fn look(&mut self, until: Instant) -> Duration {
        let now = Instant::now();
        let left = until.saturating_duration_since(now);
        let Some(looks) = self.looks.as_mut() else { return left };
        if looks.due(now) {
            let ask = looks.ask(&self.context);
            self.send_look(ask);
        }
        self.looks.as_ref().and_then(|l| l.wake_in(Instant::now())).map_or(left, |w| w.min(left))
    }

    /// A recording's look, sent as `browsingContext.captureScreenshot`.
    fn send_look(&mut self, ask: Value) {
        let sent = self.write("browsingContext.captureScreenshot", ask);
        if let Some(looks) = self.looks.as_mut() {
            match sent {
                Ok(id) => looks.sent(id),
                Err(e) => looks.unsent(format!("{e:#}")),
            }
        }
    }

    /// Read until `stop`'s look on its way has come back (or its time is
    /// up), taking events meanwhile.
    fn await_look(&mut self) {
        while !self.closed && self.looks.as_mut().is_some_and(|l| l.waiting(Instant::now())) {
            match self.read_one(READ_SLICE) {
                Ok(Some(v)) if v["type"] == "event" => self.on_event(v),
                Ok(_) => {}
                Err(_) => break,
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
                let tabs = self.own.len();
                context_created(&mut self.own, &mut self.frames, params);
                if self.own.len() > tabs {
                    // A tab one of its pages opened: the session's emulations,
                    // `set geo`'s position among them, before its page runs
                    // (a grant is the origin's in every tab). Written, as the
                    // reader is mid-message.
                    let id = self.own[tabs].clone();
                    self.adopt(&id, false);
                }
            }
            "browsingContext.contextDestroyed" if self.attached => {
                context_destroyed(&mut self.frames, params);
                if let Some(id) = params["context"].as_str() {
                    self.tab_gone(id);
                }
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
            m @ ("network.beforeRequestSent"
            | "network.responseStarted"
            | "network.responseCompleted"
            | "network.fetchError") => {
                if self.heard(params["context"].as_str()) {
                    if let Some(log) = &mut self.requests {
                        log_request(log, &mut self.last_request, m, params);
                    }
                }
                if m == "network.beforeRequestSent" && params["isBlocked"] == true {
                    let (method, p) = self.route_answer(params);
                    let _ = self.write(method, p);
                }
            }
            "browsingContext.downloadWillBegin" if self.heard(params["context"].as_str()) => {
                let text = |key: &str| params[key].as_str().unwrap_or("").to_string();
                self.downloads.push(Download {
                    n: self.downloads.last().map_or(1, |d| d.n + 1),
                    guid: text("navigation"),
                    url: text("url"),
                    suggested: text("suggestedFilename"),
                    state: "inProgress".into(),
                    received: 0,
                    total: 0,
                    path: None,
                    scope: String::new(),
                    dir: None,
                    taken: false,
                });
            }
            "browsingContext.downloadEnd" => {
                let (nav, url) = (params["navigation"].as_str().unwrap_or(""), params["url"].as_str().unwrap_or(""));
                let started = |d: &&mut Download| {
                    d.state == "inProgress" && if nav.is_empty() { d.url == url } else { d.guid == nav }
                };
                if let Some(d) = self.downloads.iter_mut().rev().find(started) {
                    if params["status"] == "complete" {
                        d.state = "completed".into();
                        // The browser's own path (Firefox: no size in the event).
                        d.path = params["filepath"].as_str().map(std::path::PathBuf::from);
                        d.received = d.path.as_ref().and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len());
                        d.total = d.received;
                    } else {
                        d.state = "canceled".into();
                    }
                }
            }
            "browsingContext.navigationStarted" => {
                let context = params["context"].as_str();
                let url = params["url"].as_str().unwrap_or("").to_string();
                if self.ours(context) {
                    // A link to another origin, in any tab of the session (one
                    // its page opened too): written now, not waited for, as
                    // the reader is mid-message here.
                    let _ = self.geo_grant(&url, false, None);
                }
                if context == Some(self.context.as_str()) {
                    self.nav = Some((url, false));
                }
            }
            "browsingContext.navigationCommitted" if self.ours(params["context"].as_str()) => {
                // The URL after a redirect, which `navigationStarted` (the URL
                // asked for) did not name.
                let _ = self.geo_grant(params["url"].as_str().unwrap_or(""), false, None);
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
                // The request log already hears the event (in the user's
                // Firefox only in the first tab: the rules hold every one).
                if self.requests.is_none() || self.attached {
                    self.send("session.subscribe", subscribe, timeout)?;
                }
                let r = self.send("network.addIntercept", intercept, timeout)?;
                let id = r["intercept"].as_str().ok_or_else(|| anyhow!("the browser named no intercept"))?;
                self.intercept = Some(id.to_string());
            }
            (Some(id), true) => {
                let id = id.clone();
                for more in std::mem::take(&mut self.more_intercepts) {
                    let _ = self.send("network.removeIntercept", json!({ "intercept": more }), timeout);
                }
                self.send("network.removeIntercept", json!({ "intercept": id }), timeout)?;
                self.intercept = None;
                let mut unsubscribe = json!({ "events": ["network.beforeRequestSent"] });
                if self.attached {
                    unsubscribe["contexts"] = json!(self.own);
                }
                if self.requests.is_none() {
                    let _ = self.send("session.unsubscribe", unsubscribe, timeout);
                }
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

    /// `set geo`: the origin of `url` may read the emulated position
    /// without a prompt. The permission is per origin (WebDriver BiDi has
    /// no other kind), so it is never the whole profile's: each origin gets
    /// it as a navigation reaches it, and [`Self::geo_revoke`] takes it
    /// back. Nothing while geo is off, or for a URL with no origin.
    /// `before` is the origin's setting before, when it was read: one
    /// already `granted` is left as it is.
    fn geo_grant(&mut self, url: &str, wait: bool, before: Option<String>) -> Result<()> {
        if let Some(held) = &mut self.geo_held {
            held.push(url.to_string());
            return Ok(());
        }
        let Some(granted) = &mut self.geo else { return Ok(()) };
        let Some(origin) = page_origin(url).filter(|o| !granted.iter().any(|(g, _)| g == o)) else {
            return Ok(());
        };
        let had = before.as_deref() == Some("granted");
        granted.push((origin.clone(), before));
        if had {
            return Ok(());
        }
        let params = json!({ "descriptor": { "name": "geolocation" }, "state": "granted", "origin": origin });
        if wait {
            self.send("permissions.setPermission", params, Duration::from_secs(10)).map(|_| ())
        } else {
            self.write("permissions.setPermission", params).map(|_| ())
        }
    }

    /// Every origin [`Self::geo_grant`] granted gets back the setting it
    /// had ([`geo_restore`]).
    fn geo_revoke(&mut self) {
        for (origin, before) in self.geo.take().unwrap_or_default() {
            let Some(state) = geo_restore(before.as_deref()) else { continue };
            let params = json!({ "descriptor": { "name": "geolocation" }, "state": state, "origin": origin });
            let _ = self.send("permissions.setPermission", params, Duration::from_secs(5));
        }
    }

    /// The active tab's URL and, in a Firefox you attached to, the
    /// geolocation setting its origin has, read in the page (WebDriver BiDi
    /// can set a permission, not read one) in a realm of navigera's own, the
    /// two in one go so they are the same document's: `granted`, `denied`
    /// or `prompt`, `None` when it cannot be read.
    fn geo_here(&mut self) -> Result<(String, Option<String>)> {
        if self.attached {
            let r = self.send(
                "script.evaluate",
                json!({
                    "expression": "navigator.permissions.query({ name: 'geolocation' }).then((s) => s.state + ' ' + location.href)",
                    "awaitPromise": true,
                    "target": { "context": self.context, "sandbox": "navigera" },
                    "resultOwnership": "none",
                }),
                Duration::from_secs(5),
            );
            let read = r.ok().filter(|r| r["type"] == "success");
            if let Some((state, url)) = read.as_ref().and_then(|r| r["result"]["value"].as_str()?.split_once(' ')) {
                let state = matches!(state, "granted" | "denied" | "prompt").then(|| state.to_string());
                return Ok((url.to_string(), state));
            }
        }
        let tree = self.send(
            "browsingContext.getTree",
            json!({ "root": self.context, "maxDepth": 0 }),
            Duration::from_secs(10),
        )?;
        Ok((tree["contexts"][0]["url"].as_str().unwrap_or("").to_string(), None))
    }

    /// A tab of the user's Firefox that became this session's (it opened
    /// it, or one of its pages did): the session's emulations, its request
    /// log, response bodies and `route` rules, which in the user's browser
    /// cover only the tabs they name. `wait` false: written, for a caller
    /// that is reading a message; an intercept's answer is taken in
    /// [`Self::read_one`]. A tab a page opens is heard from its
    /// `contextCreated` on: a request it sent before that is not listed or
    /// held.
    fn adopt(&mut self, id: &str, wait: bool) {
        self.emulate_in(id, wait);
        let mut calls = Vec::new();
        if self.requests.is_some() {
            calls.push(("session.subscribe", json!({ "events": NETWORK_EVENTS, "contexts": [id] })));
        } else if self.intercept.is_some() {
            calls.push(("session.subscribe", json!({ "events": ["network.beforeRequestSent"], "contexts": [id] })));
        }
        if self.collector.is_some() {
            let mut collect = collect_params(self.collects_requests);
            collect["contexts"] = json!([id]);
            calls.push(("network.addDataCollector", collect));
        }
        if self.intercept.is_some() {
            calls.push(("network.addIntercept", json!({ "phases": ["beforeRequestSent"], "contexts": [id] })));
        }
        for (method, params) in calls {
            let intercept = method == "network.addIntercept";
            let collector = method == "network.addDataCollector";
            if wait {
                if let Ok(r) = self.send(method, params, Duration::from_secs(10)) {
                    if let Some(x) = r["intercept"].as_str().filter(|_| intercept) {
                        self.more_intercepts.push(x.to_string());
                    }
                    if let Some(x) = r["collector"].as_str().filter(|_| collector) {
                        self.tab_collectors.push((id.to_string(), x.to_string()));
                    }
                }
            } else if let Ok(asked) = self.write(method, params) {
                if intercept {
                    self.asked_intercepts.push(asked);
                }
                if collector {
                    self.asked_collectors.push((asked, Some(id.to_string())));
                }
            }
        }
    }

    /// The answer to a collector [`Self::adopt`] asked for without waiting:
    /// kept with its tab, or, the tab closed meanwhile, removed.
    fn took_collector(&mut self, v: &Value) {
        let Some(i) = v["id"].as_u64().and_then(|id| self.asked_collectors.iter().position(|(a, _)| *a == id)) else {
            return;
        };
        let (_, tab) = self.asked_collectors.swap_remove(i);
        let Some(x) = v["result"]["collector"].as_str() else { return };
        match tab {
            Some(tab) => self.tab_collectors.push((tab, x.to_string())),
            None => {
                let _ = self.write("network.removeDataCollector", json!({ "collector": x }));
            }
        }
    }

    /// A tab of the user's browser closed: its response bodies go with it.
    /// Firefox drops no collector with its tab: what one collected could
    /// still be read after the tab closed, until the session ends (Firefox
    /// 160). Written, as the reader may be mid-message.
    fn tab_gone(&mut self, tab: &str) {
        for asked in self.asked_collectors.iter_mut().filter(|(_, t)| t.as_deref() == Some(tab)) {
            asked.1 = None;
        }
        let (gone, kept) = std::mem::take(&mut self.tab_collectors).into_iter().partition(|(t, _)| t == tab);
        self.tab_collectors = kept;
        for (_, x) in gone {
            let _ = self.write("network.removeDataCollector", json!({ "collector": x }));
        }
    }

    /// The answer to an intercept [`Self::adopt`] asked for without
    /// waiting: kept with the session's, or, the rules gone meanwhile,
    /// removed.
    fn took_intercept(&mut self, v: &Value) {
        let Some(i) = v["id"].as_u64().and_then(|id| self.asked_intercepts.iter().position(|&a| a == id)) else {
            return;
        };
        self.asked_intercepts.swap_remove(i);
        let Some(x) = v["result"]["intercept"].as_str() else { return };
        if self.intercept.is_some() {
            self.more_intercepts.push(x.to_string());
        } else {
            let _ = self.write("network.removeIntercept", json!({ "intercept": x }));
        }
    }

    /// The session's emulations (`set locale`, `ua`, `geo`, ...) and its
    /// viewport, in a tab of the user's Firefox that has none of them: one
    /// navigera opened, or one its page opened. `wait` false: written, for
    /// a caller that is reading a message.
    fn emulate_in(&mut self, id: &str, wait: bool) {
        let mut calls: Vec<(String, Value)> = Vec::new();
        if let Some(mut viewport) = self.viewport.clone() {
            viewport["context"] = json!(id);
            calls.push(("browsingContext.setViewport".into(), viewport));
        }
        for (method, mut params) in self.emulations.clone() {
            params["contexts"] = json!([id]);
            calls.push((method, params));
        }
        for (method, params) in calls {
            if wait {
                let _ = self.send(&method, params, Duration::from_secs(30));
            } else {
                let _ = self.write(&method, params);
            }
        }
    }

    /// The top-level contexts of a `getTree` answer.
    fn tops_of(&mut self, tree: &Value) -> Vec<String> {
        if self.attached {
            let tabs = self.own.len();
            adopt_opened(&mut self.own, tree);
            // Opened tabs no `contextCreated` named: the session's
            // emulations and requests too, late.
            let opened = self.own[tabs..].to_vec();
            for id in &opened {
                self.adopt(id, true);
            }
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
        // Before the page runs: it may ask for its position as it loads.
        let _ = self.geo_grant(url, true, None);
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
        // Every open tab of the session, as on Chrome (tabs opened later get
        // it as they open); the current one first: its error is the
        // command's, and `off` waits in it. Another tab may be closing.
        let mut tabs = self.top_contexts().map(|all| self.tabs_among(all)).unwrap_or_default();
        tabs.retain(|t| *t != self.context);
        params["context"] = json!(self.context);
        self.send("browsingContext.setViewport", params.clone(), Duration::from_secs(30))?;
        for id in tabs {
            params["context"] = json!(id);
            let _ = self.send("browsingContext.setViewport", params.clone(), Duration::from_secs(30));
        }
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
        if method == GEO {
            if params["coordinates"].is_null() {
                self.geo_revoke();
            } else if self.geo.is_none() && !persons_browser(&self.browser) {
                // In a Firefox you attached to the page's origin may have a
                // setting of its own: read before any grant, to give it back
                // at `off`. A navigation's grant waits for the read: written
                // first, it would be read back as the origin's own.
                self.geo = Some(Vec::new());
                self.geo_held = Some(Vec::new());
                let here = self.geo_here();
                let held = self.geo_held.take().unwrap_or_default();
                if let Err(e) = here.and_then(|(url, before)| self.geo_grant(&url, true, before)) {
                    // Geo stays off, so the next `set geo` reads and grants.
                    // A navigation's grant may have gone out while the
                    // page's own waited: given back with the rest.
                    self.geo_revoke();
                    return Err(e);
                }
                for url in held {
                    let _ = self.geo_grant(&url, true, None);
                }
            }
        }
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

    fn requests(&mut self, clear: bool) -> Result<Vec<crate::cdp::logs::Request>> {
        self.idle();
        self.start_log(Duration::from_secs(10));
        // A request sent in the last moments with no answer yet (in
        // flight, or its answer on its way to us) is waited for, a little.
        // One pending longer (a long poll) costs nothing.
        let deadline = Instant::now() + CATCH_UP;
        let cleared = self.cleared;
        while Instant::now() < deadline
            && self.requests.as_ref().is_some_and(|log| fresh_pending(log, cleared, now_s()))
        {
            match self.read_one(Duration::from_millis(5)) {
                Ok(Some(v)) if v["type"] == "event" => self.on_event(v),
                Ok(_) => {}
                Err(_) => break,
            }
        }
        let Some(log) = &self.requests else {
            bail!("this browser sends no WebDriver BiDi network events, so navigera has no request list here")
        };
        let shown = listed(log, cleared);
        if clear {
            self.cleared = self.last_request;
        }
        Ok(shown)
    }

    fn requests_note(&mut self) -> Option<&'static str> {
        std::mem::take(&mut self.log_note).then_some(LOG_STARTED)
    }

    fn response_body(&mut self, request: &str) -> Result<(String, bool)> {
        if self.collector.is_none() {
            bail!("this browser keeps no response bodies for WebDriver BiDi (network.addDataCollector)")
        }
        // No collector named: any of the session's, one per tab in the
        // user's browser ([`Self::adopt`]).
        let p = json!({ "dataType": "response", "request": request });
        let r = self.send("network.getData", p, Duration::from_secs(30))?;
        let bytes = &r["bytes"];
        let value = bytes["value"].as_str().unwrap_or_default().to_string();
        Ok((value, bytes["type"] == "base64"))
    }

    fn request_body(&mut self, request: &str) -> Result<String> {
        if !self.collects_requests {
            bail!(
                "this browser keeps no request bodies for WebDriver BiDi (network.addDataCollector, dataTypes request)"
            )
        }
        let p = json!({ "dataType": "request", "request": request });
        let r = self.send("network.getData", p, Duration::from_secs(30))?;
        let bytes = &r["bytes"];
        let value = bytes["value"].as_str().unwrap_or_default();
        Ok(if bytes["type"] == "base64" {
            use base64::Engine as _;
            let raw = base64::engine::general_purpose::STANDARD.decode(value).context("network.getData")?;
            String::from_utf8_lossy(&raw).into_owned()
        } else {
            value.to_string()
        })
    }

    fn downloads(&mut self, clear: bool) -> Result<Vec<Download>> {
        self.idle();
        Ok(if clear { std::mem::take(&mut self.downloads) } else { self.downloads.clone() })
    }

    fn set_downloads_dir(&mut self, dir: &std::path::Path) -> Result<()> {
        if self.attached {
            bail!(
                "in a Firefox you attached to, downloads go where it saves them: WebDriver BiDi would change that \
                 for every tab of it; `--engine firefox` starts one of navigera's own"
            );
        }
        let behavior = json!({ "type": "allowed", "destinationFolder": dir.display().to_string() });
        self.send("browser.setDownloadBehavior", json!({ "downloadBehavior": behavior }), Duration::from_secs(10))?;
        Ok(())
    }

    fn wait_download(&mut self, glob: &str, timeout: Duration) -> Result<Download> {
        use crate::cdp::downloads::{next_for_wait, Wait};
        let deadline = Instant::now() + timeout;
        loop {
            if let Wait::Done(i) | Wait::Canceled(i) = next_for_wait(&self.downloads, glob) {
                self.downloads[i].taken = true;
                let d = &self.downloads[i];
                if d.state == "canceled" {
                    bail!("{}", crate::cdp::downloads::canceled_text(d));
                }
                return Ok(d.clone());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                bail!("{}", crate::cdp::downloads::none_text(&self.downloads, glob, timeout));
            }
            // Read until something arrives (or the deadline): no polling.
            if let Some(v) = self.read_one(left.min(READ_SLICE))? {
                if v["type"] == "event" {
                    self.on_event(v);
                }
            }
        }
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

    fn screencast_start(&mut self, dir: &std::path::Path, quality: Option<&str>, size: Option<&str>) -> Result<()> {
        if let Some(looks) = &self.looks {
            bail!("screencast: already recording into {}; `screencast stop` first", looks.dir().display());
        }
        if size.is_some() {
            bail!(
                "screencast --size: not over WebDriver BiDi (its screenshot is the viewport's size); `set viewport \
                 <w>x<h>` sizes the frames, or drop --size"
            );
        }
        let opts = crate::screencast::Options::parse(quality, None)?;
        self.looks = Some(super::screencast::Looks::start(dir, &opts)?);
        // The first look, at the page as it is now.
        self.idle();
        Ok(())
    }

    fn screencast_stop(&mut self) -> Result<Value> {
        let Some(looks) = self.looks.as_mut() else {
            bail!("screencast: nothing is recording; `screencast start <dir>` first")
        };
        looks.stop();
        self.await_look();
        if self.looks.as_mut().is_some_and(|l| l.last_wanted(Instant::now())) && !self.closed {
            let ask = self.looks.as_ref().map(|l| l.ask(&self.context)).unwrap_or_default();
            self.send_look(ask);
            self.await_look();
        }
        match self.looks.take() {
            Some(looks) => looks.finish(),
            None => bail!("screencast: nothing is recording; `screencast start <dir>` first"),
        }
    }

    fn tick(&self) -> Option<Duration> {
        self.looks.as_ref().and_then(|l| l.tick(Instant::now()))
    }

    fn pause(&mut self, wait: Duration) {
        // Reading, not sleeping: a recording looks on, a dialog or a request
        // held for a `route` rule is answered meanwhile.
        let until = Instant::now() + wait;
        while !self.closed {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            match self.read_one(left.min(READ_SLICE)) {
                Ok(Some(v)) if v["type"] == "event" => self.on_event(v),
                Ok(_) => {}
                Err(_) => break,
            }
        }
        std::thread::sleep(until.saturating_duration_since(Instant::now()));
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
        if self.attached {
            self.own.push(id.clone());
            self.adopt(&id, true);
            let was = std::mem::replace(&mut self.context, id.clone());
            self.listen_in(&id, Duration::from_secs(10));
            self.context = was;
        } else if let Some(mut viewport) = self.viewport.clone() {
            // The emulations cover every tab of navigera's own Firefox; the
            // viewport is a tab's.
            viewport["context"] = json!(id);
            let _ = self.send("browsingContext.setViewport", viewport, Duration::from_secs(30));
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
        // Its contextDestroyed has done this, when it came first.
        self.tab_gone(id);
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
                // The user's own profile keeps a grant; navigera's own goes
                // with its profile.
                self.geo_revoke();
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
            .and_then(|a| super::ok_text(&a).ok().and_then(|t| super::page_value(&t).ok()))
            .and_then(|v| v[0].as_str().map(str::to_string))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In a Firefox you attached to, a tab's response bodies go when the
    /// tab closes: Firefox kept a collector's bodies after its tab closed,
    /// and navigera removed none (one per tab it took on). A page's tab
    /// closed by the page, the session's tab closed by `tab-close`, and a
    /// tab that closed before its collector's answer came.
    #[test]
    fn a_closed_tabs_collector_is_removed() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let firefox = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            let mut removed = Vec::new();
            let destroyed = |tab: &str| {
                json!({ "type": "event", "method": "browsingContext.contextDestroyed", "params": { "context": tab } })
                    .to_string()
            };
            while let Ok(Message::Text(text)) = ws.read() {
                let v: Value = serde_json::from_str(text.as_str()).unwrap();
                let result = match v["method"].as_str().unwrap_or("") {
                    "network.addDataCollector" => {
                        let tab = v["params"]["contexts"][0].as_str().unwrap().to_string();
                        if tab == "late" {
                            ws.send(Message::text(destroyed(&tab))).unwrap();
                        }
                        json!({ "collector": format!("c-{tab}") })
                    }
                    "network.removeDataCollector" => {
                        removed.push(v["params"]["collector"].as_str().unwrap().to_string());
                        json!({})
                    }
                    "session.status" if removed.is_empty() => {
                        ws.send(Message::text(destroyed("pop"))).unwrap();
                        json!({})
                    }
                    _ => json!({}),
                };
                ws.send(Message::text(json!({ "type": "success", "id": v["id"], "result": result }).to_string()))
                    .unwrap();
            }
            removed
        });
        let mut b = Bidi::connect(&format!("ws://{addr}/"), Duration::from_secs(5)).unwrap();
        b.attached = true;
        b.context = "tab".into();
        b.own.push("tab".into());
        b.collector = Some("c-tab".into());
        b.tab_collectors.push(("tab".into(), "c-tab".into()));
        let pump = |b: &mut Bidi| b.send("session.status", json!({}), Duration::from_secs(5)).unwrap();
        b.adopt("pop", false);
        pump(&mut b);
        b.tab_close("tab").unwrap();
        b.adopt("late", false);
        pump(&mut b);
        pump(&mut b);
        assert!(b.tab_collectors.is_empty() && b.asked_collectors.is_empty(), "{:?}", b.tab_collectors);
        b.closed = true;
        drop(b);
        assert_eq!(firefox.join().unwrap(), ["c-pop", "c-tab", "c-late"]);
    }

    /// A session listens to no network events until its first `network`
    /// (a browser listened to slows every load); that one subscribes, adds
    /// the collector and says the log starts now, once.
    #[test]
    fn the_request_log_starts_at_the_first_network() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let firefox = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            let mut asked = Vec::new();
            while let Ok(Message::Text(text)) = ws.read() {
                let v: Value = serde_json::from_str(text.as_str()).unwrap();
                let method = v["method"].as_str().unwrap_or("").to_string();
                let events = v["params"]["events"].to_string();
                asked.push(if method == "session.subscribe" { format!("{method} {events}") } else { method.clone() });
                let result = match method.as_str() {
                    "browsingContext.getTree" => json!({ "contexts": [{ "context": "tab" }] }),
                    "network.addDataCollector" => json!({ "collector": "c1" }),
                    _ => json!({}),
                };
                ws.send(Message::text(json!({ "type": "success", "id": v["id"], "result": result }).to_string()))
                    .unwrap();
            }
            asked
        });
        let mut b = Bidi::connect(&format!("ws://{addr}/"), Duration::from_secs(5)).unwrap();
        b.start(Duration::from_secs(5)).unwrap();
        let at_start = b.send("session.status", json!({}), Duration::from_secs(5)).map(|_| ());
        assert!(at_start.is_ok() && b.requests.is_none() && b.collector.is_none());
        assert_eq!(b.requests(false).unwrap().len(), 0);
        assert_eq!(b.requests_note(), Some(LOG_STARTED));
        b.requests(false).unwrap();
        assert_eq!((b.requests_note(), b.collector.as_deref()), (None, Some("c1")));
        b.closed = true;
        drop(b);
        let asked = firefox.join().unwrap();
        let status = asked.iter().position(|m| m == "session.status").unwrap();
        assert!(!asked[..status].iter().any(|m| m.contains("network.")), "{asked:?}");
        let network: Vec<_> = asked[status..].iter().filter(|m| m.contains("network.")).collect();
        assert_eq!(network.len(), 2, "{asked:?}");
        assert!(network[0].contains("network.beforeRequestSent") && network[1] == "network.addDataCollector");
    }

    /// A request's body is asked of the browser (a BiDi event carries only
    /// its size) where its collectors take the `request` data type, and
    /// read as text; a browser that refuses the type keeps response bodies,
    /// and `network <n>` says it has none of the request's.
    #[test]
    fn request_bodies_come_from_a_collector_that_takes_them() {
        use std::net::TcpListener;
        use tungstenite::Message;
        for takes_requests in [true, false] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let firefox = std::thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                let mut ws = tungstenite::accept(stream).unwrap();
                let mut asked = Vec::new();
                while let Ok(Message::Text(text)) = ws.read() {
                    let v: Value = serde_json::from_str(text.as_str()).unwrap();
                    let p = &v["params"];
                    let answer = match v["method"].as_str().unwrap_or("") {
                        "network.addDataCollector" => {
                            asked.push(p["dataTypes"].clone());
                            if p["dataTypes"].to_string().contains("request") && !takes_requests {
                                json!({ "type": "error", "id": v["id"], "error": "invalid argument",
                                        "message": "Expected \"dataTypes\" values to be one of response" })
                            } else {
                                json!({ "type": "success", "id": v["id"], "result": { "collector": "c1" } })
                            }
                        }
                        "network.getData" => {
                            assert_eq!(p["dataType"], "request");
                            asked.push(p["request"].clone());
                            let bytes = json!({ "type": "base64", "value": "cT3DqSZuPTE=" });
                            json!({ "type": "success", "id": v["id"], "result": { "bytes": bytes } })
                        }
                        _ => json!({ "type": "success", "id": v["id"], "result": {} }),
                    };
                    ws.send(Message::text(answer.to_string())).unwrap();
                }
                asked
            });
            let mut b = Bidi::connect(&format!("ws://{addr}/"), Duration::from_secs(5)).unwrap();
            b.start_log(Duration::from_secs(5));
            assert_eq!((b.collector.as_deref(), b.collects_requests), (Some("c1"), takes_requests));
            let body = b.request_body("r1");
            b.closed = true;
            drop(b);
            let asked = firefox.join().unwrap();
            if takes_requests {
                assert_eq!(body.unwrap(), "q=\u{e9}&n=1");
                assert_eq!(asked, [json!(["request", "response"]), json!("r1")]);
            } else {
                assert!(format!("{:#}", body.unwrap_err()).contains("no request bodies"));
                assert_eq!(asked, [json!(["request", "response"]), json!(["response"])]);
            }
        }
    }

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

#[cfg(test)]
mod geo_tests {
    use super::{geo_restore, page_origin, persons_browser, Bidi, GEO};
    use crate::wd::Wire;
    use serde_json::{json, Value};
    use std::collections::HashMap;
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::{self, Message};

    /// A Firefox for `set geo` on `page` (its one tab, `tab`): each origin's
    /// geolocation setting starts as `settings` says; right after the
    /// position override a `navigationCommitted` of that page arrives (a
    /// navigation the command did not wait for); a read in the page says
    /// the setting its origin has then. The first `failing` reads of the
    /// page (in the page, then of the tree) fail. The
    /// `permissions.setPermission` calls it got, in order.
    fn geo_firefox(
        page: &'static str,
        settings: &'static [(&'static str, &'static str)],
        mut failing: u32,
    ) -> (String, std::thread::JoinHandle<Vec<(String, String)>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            let mut now: HashMap<String, String> =
                settings.iter().map(|(o, s)| (o.to_string(), s.to_string())).collect();
            let origin = page_origin(page).unwrap();
            let mut sets = Vec::new();
            while let Ok(Message::Text(text)) = ws.read() {
                let v: Value = serde_json::from_str(text.as_str()).unwrap();
                let p = &v["params"];
                let mut then = None;
                let method = v["method"].as_str().unwrap_or("");
                if failing > 0 && matches!(method, "script.evaluate" | "browsingContext.getTree") {
                    if method == "browsingContext.getTree" {
                        failing -= 1;
                    }
                    let error = json!({ "type": "error", "id": v["id"], "error": "no such frame", "message": "gone" });
                    ws.send(Message::text(error.to_string())).unwrap();
                    continue;
                }
                let result = match method {
                    GEO => {
                        then = Some(json!({ "type": "event", "method": "browsingContext.navigationCommitted",
                            "params": { "context": "tab", "url": page } }));
                        json!({})
                    }
                    "browsingContext.getTree" => json!({ "contexts": [{ "context": "tab", "url": page }] }),
                    "permissions.setPermission" => {
                        let (o, state) = (p["origin"].as_str().unwrap(), p["state"].as_str().unwrap());
                        sets.push((o.to_string(), state.to_string()));
                        now.insert(o.to_string(), state.to_string());
                        json!({})
                    }
                    "script.evaluate" => {
                        let state = now.get(&origin).cloned().unwrap_or_else(|| "prompt".into());
                        let with_url = p["expression"].as_str().unwrap_or("").contains("location.href");
                        let value = if with_url { format!("{state} {page}") } else { state };
                        json!({ "type": "success", "result": { "type": "string", "value": value } })
                    }
                    _ => json!({}),
                };
                let answer = json!({ "type": "success", "id": v["id"], "result": result });
                ws.send(Message::text(answer.to_string())).unwrap();
                if let Some(event) = then {
                    ws.send(Message::text(event.to_string())).unwrap();
                }
            }
            sets
        });
        (format!("ws://{addr}/"), server)
    }

    /// `set geo <lat> <lon>`, then `set geo off`, in a Firefox navigera
    /// attached to (`headless`: one no person uses): what it was sent.
    /// `failing`: the first `set geo`s whose read of the page fails.
    fn set_geo_on_and_off(
        headless: bool,
        settings: &'static [(&'static str, &'static str)],
        failing: u32,
    ) -> Vec<(String, String)> {
        let (url, firefox) = geo_firefox("https://blocked.test/page", settings, failing);
        let mut b = Bidi::connect(&url, Duration::from_secs(5)).unwrap();
        b.attached = true;
        b.browser = json!({ "attached": true, "headless": headless });
        b.context = "tab".into();
        b.own.push("tab".into());
        let on = json!({ "coordinates": { "latitude": 1, "longitude": 2 } });
        for _ in 0..failing {
            assert!(b.emulate(GEO, on.clone()).is_err(), "a read that failed");
        }
        b.emulate(GEO, on).unwrap();
        b.emulate(GEO, json!({ "coordinates": null })).unwrap();
        b.closed = true;
        drop(b);
        firefox.join().unwrap()
    }

    /// A navigation's grant that arrives while `set geo` reads the page's
    /// own setting waits for the read: granted first, it was read back as
    /// the site's own, and `off` set a site the user blocked to asking.
    #[test]
    fn set_geo_off_gives_a_blocked_site_its_block_back_after_a_navigation_meanwhile() {
        let sent = set_geo_on_and_off(true, &[("https://blocked.test", "denied")], 0);
        let to = |state: &str| ("https://blocked.test".to_string(), state.to_string());
        assert_eq!(sent, [to("granted"), to("denied")]);
    }

    /// A `set geo` whose read of the page failed granted nothing, and the
    /// next one reads and grants: it had left geo on, so a retry granted
    /// nothing.
    #[test]
    fn set_geo_after_a_failed_read_reads_and_grants() {
        let sent = set_geo_on_and_off(true, &[("https://blocked.test", "denied")], 1);
        let to = |state: &str| ("https://blocked.test".to_string(), state.to_string());
        assert_eq!(sent, [to("granted"), to("denied")]);
    }

    /// A `set geo` whose grant for the page failed takes back a grant a
    /// navigation got while that one waited: it forgot it, so `off` sent
    /// nothing and the second site kept the permission.
    #[test]
    fn a_failed_grant_takes_back_a_navigation_granted_while_it_waited() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let firefox = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            let mut sets = Vec::new();
            while let Ok(Message::Text(text)) = ws.read() {
                let v: Value = serde_json::from_str(text.as_str()).unwrap();
                let p = &v["params"];
                let result = match v["method"].as_str().unwrap_or("") {
                    "script.evaluate" => {
                        json!({ "type": "success", "result": { "type": "string", "value": "denied https://blocked.test/page" } })
                    }
                    "permissions.setPermission" => {
                        let (o, state) = (p["origin"].as_str().unwrap(), p["state"].as_str().unwrap());
                        sets.push((o.to_string(), state.to_string()));
                        if (o, state) == ("https://blocked.test", "granted") {
                            let event = json!({ "type": "event", "method": "browsingContext.navigationCommitted",
                                "params": { "context": "tab", "url": "https://second.test/page" } });
                            ws.send(Message::text(event.to_string())).unwrap();
                            let error = json!({ "type": "error", "id": v["id"], "error": "unknown error", "message": "refused" });
                            ws.send(Message::text(error.to_string())).unwrap();
                            continue;
                        }
                        json!({})
                    }
                    _ => json!({}),
                };
                let answer = json!({ "type": "success", "id": v["id"], "result": result });
                ws.send(Message::text(answer.to_string())).unwrap();
            }
            sets
        });
        let mut b = Bidi::connect(&format!("ws://{addr}/"), Duration::from_secs(5)).unwrap();
        b.attached = true;
        b.browser = json!({ "attached": true, "headless": true });
        b.context = "tab".into();
        b.own.push("tab".into());
        assert!(b.emulate(GEO, json!({ "coordinates": { "latitude": 1, "longitude": 2 } })).is_err());
        b.emulate(GEO, json!({ "coordinates": null })).unwrap();
        b.closed = true;
        drop(b);
        let to = |o: &str, state: &str| (o.to_string(), state.to_string());
        let sent = firefox.join().unwrap();
        let (blocked, second) = ("https://blocked.test", "https://second.test");
        assert_eq!(sent, [to(blocked, "granted"), to(second, "granted"), to(blocked, "denied"), to(second, "prompt")]);
    }

    /// In a Firefox a person may be using (attached, with a window) a
    /// grant would be the site's in their whole profile: none is sent, not
    /// for a navigation either, and `off` has nothing to give back.
    #[test]
    fn set_geo_grants_nothing_in_a_firefox_a_person_uses() {
        assert!(persons_browser(&json!({ "attached": true, "headless": false })));
        assert!(!persons_browser(&json!({ "attached": true, "headless": true })));
        assert!(!persons_browser(&json!({ "headless": false })));
        assert_eq!(set_geo_on_and_off(false, &[("https://blocked.test", "denied")], 0), []);
    }

    #[test]
    fn set_geo_off_gives_an_origin_back_the_setting_it_had() {
        // Allowed before: navigera's grant is the same setting; nothing to send.
        assert_eq!(geo_restore(Some("granted")), None);
        assert_eq!(geo_restore(Some("denied")), Some("denied"));
        assert_eq!(geo_restore(Some("prompt")), Some("prompt"));
        // Not read (an origin first reached while geo was on): asking again.
        assert_eq!(geo_restore(None), Some("prompt"));
    }

    #[test]
    fn a_page_origin_is_scheme_host_and_port() {
        assert_eq!(page_origin("http://127.0.0.1:8080/more.html?q=1#x").as_deref(), Some("http://127.0.0.1:8080"));
        assert_eq!(page_origin("HTTPS://User:pw@Example.COM/a").as_deref(), Some("https://example.com"));
        assert_eq!(page_origin("https://a.test#frag").as_deref(), Some("https://a.test"));
        for none in ["about:blank", "data:text/html,x", "file:///tmp/a.html", "https:///nohost", ""] {
            assert_eq!(page_origin(none), None, "{none}");
        }
    }
}

#[cfg(test)]
mod network_tests {
    use super::{log_request, REQUESTS_MAX};
    use serde_json::{json, Value};
    use std::collections::VecDeque;

    fn sent(id: &str, url: &str, ms: f64, redirects: u64) -> Value {
        json!({ "context": "c", "redirectCount": redirects, "timestamp": ms, "isBlocked": false,
                "request": { "request": id, "url": url, "method": "POST", "bodySize": 3,
                             "initiatorType": "fetch", "destination": "",
                             "headers": [{ "name": "a", "value": { "type": "string", "value": "1" } },
                                         { "name": "a", "value": { "type": "string", "value": "2" } }] } })
    }

    fn done(id: &str, status: u64, ms: f64) -> Value {
        json!({ "context": "c", "timestamp": ms, "redirectCount": 0, "request": { "request": id },
                "response": { "status": status, "mimeType": "text/html; charset=utf-8", "fromCache": false,
                              "bytesReceived": 120, "headers": [] } })
    }

    /// An event of a later hop of the request.
    fn hop(mut event: Value, n: u64) -> Value {
        event["redirectCount"] = json!(n);
        event
    }

    #[test]
    fn a_redirect_is_two_entries_under_one_id() {
        let (mut log, mut last) = (VecDeque::new(), 0);
        log_request(&mut log, &mut last, "network.beforeRequestSent", &sent("1", "http://a/x", 1000.0, 0));
        log_request(&mut log, &mut last, "network.responseCompleted", &done("1", 302, 1010.0));
        log_request(&mut log, &mut last, "network.beforeRequestSent", &sent("1", "http://a/y", 1011.0, 1));
        log_request(&mut log, &mut last, "network.responseCompleted", &hop(done("1", 200, 1030.0), 1));
        let got: Vec<_> = log.iter().map(|h| (h.r.n, h.r.url.as_str(), h.r.status)).collect();
        assert_eq!(got, [(1, "http://a/x", Some(302)), (2, "http://a/y", Some(200))]);
        let r = &log[1].r;
        assert_eq!(
            (r.kind.as_str(), r.mime.as_str(), r.size, r.has_post_data),
            ("Fetch", "text/html", Some(120), true)
        );
        assert_eq!(r.request_headers, json!({ "a": "1\n2" }));
        assert!((r.ended.unwrap() - r.started - 0.019).abs() < 1e-9);
    }

    #[test]
    fn the_get_after_a_303_has_no_body_whatever_size_it_comes_with() {
        let (mut log, mut last) = (VecDeque::new(), 0);
        log_request(&mut log, &mut last, "network.beforeRequestSent", &sent("1", "http://a/x", 1000.0, 0));
        log_request(&mut log, &mut last, "network.responseCompleted", &done("1", 303, 1010.0));
        let mut get = sent("1", "http://a/y", 1011.0, 1);
        get["request"]["method"] = json!("GET");
        get["request"]["bodySize"] = json!(8);
        log_request(&mut log, &mut last, "network.beforeRequestSent", &get);
        let got: Vec<_> = log.iter().map(|h| (h.r.method.as_str(), h.r.has_post_data)).collect();
        assert_eq!(got, [("POST", true), ("GET", false)]);
    }

    #[test]
    fn an_unreported_redirect_hop_ends_where_the_next_begins_and_a_repeat_is_one_entry() {
        let (mut log, mut last) = (VecDeque::new(), 0);
        log_request(&mut log, &mut last, "network.beforeRequestSent", &sent("1", "http://a/x", 1000.0, 0));
        log_request(&mut log, &mut last, "network.beforeRequestSent", &sent("1", "http://a/x", 1001.0, 0));
        log_request(&mut log, &mut last, "network.beforeRequestSent", &sent("1", "http://a/y", 1005.0, 1));
        log_request(
            &mut log,
            &mut last,
            "network.fetchError",
            &json!({ "timestamp": 1009.0, "redirectCount": 1, "request": { "request": "1" }, "errorText": "NS_ERROR_NET_RESET" }),
        );
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].r.ended, Some(1.005));
        assert_eq!(log[1].r.failed.as_deref(), Some("NS_ERROR_NET_RESET"));
    }

    /// chromedriver's mapper (chromium-bidi) holds `beforeRequestSent`
    /// for a CDP event from the browser process; a hop's response that
    /// comes first opens its entry, and the announcement after it adds no
    /// second one.
    #[test]
    fn a_hop_heard_of_first_by_its_response_is_one_entry() {
        let (mut log, mut last) = (VecDeque::new(), 0);
        let mut started = done("1", 404, 1000.0);
        started["request"] = sent("1", "http://a/nope", 1000.0, 0)["request"].clone();
        log_request(&mut log, &mut last, "network.responseStarted", &started);
        log_request(&mut log, &mut last, "network.beforeRequestSent", &sent("1", "http://a/nope", 1000.0, 0));
        let got: Vec<_> = log.iter().map(|h| (h.r.n, h.r.url.as_str(), h.r.status, h.r.kind.as_str())).collect();
        assert_eq!(got, [(1, "http://a/nope", Some(404), "Fetch")]);
        assert!(!super::fresh_pending(&log, 0, 1.0), "answered: not waited for");
    }

    /// The hop a redirect leads to, of which chromedriver sent only the
    /// response (its `beforeRequestSent` never came): listed, after the
    /// hop that redirected, which keeps its own status.
    #[test]
    fn the_hop_a_redirect_leads_to_is_listed_without_its_announcement() {
        let (mut log, mut last) = (VecDeque::new(), 0);
        log_request(&mut log, &mut last, "network.beforeRequestSent", &sent("7", "http://a/to", 1000.0, 0));
        log_request(&mut log, &mut last, "network.responseStarted", &done("7", 302, 1000.0));
        log_request(&mut log, &mut last, "network.responseCompleted", &done("7", 302, 1000.0));
        let mut target = hop(done("7", 200, 1003.0), 1);
        target["request"] = sent("7", "http://a/next.html", 1003.0, 1)["request"].clone();
        log_request(&mut log, &mut last, "network.responseStarted", &target);
        // A late event of the first hop is its own, not the target's.
        log_request(&mut log, &mut last, "network.responseStarted", &done("7", 302, 1000.0));
        let got: Vec<_> = log.iter().map(|h| (h.r.n, h.count, h.r.url.as_str(), h.r.status)).collect();
        assert_eq!(got, [(1, 0, "http://a/to", Some(302)), (2, 1, "http://a/next.html", Some(200))]);
        assert!(log[0].r.ended.is_some() && log[1].r.ended.is_none());
    }

    /// chromedriver's mapper can give a hop that redirected the status of
    /// the hop it led to (its CDP record took the next hop's raw response
    /// when that came before the redirect), its reason phrase its own:
    /// the hop after it shows it redirected, the phrase gives its status.
    #[test]
    fn a_redirect_reported_with_the_next_hops_status_takes_its_own_phrase() {
        let (mut log, mut last) = (VecDeque::new(), 0);
        log_request(&mut log, &mut last, "network.beforeRequestSent", &sent("7", "http://a/to", 1000.0, 0));
        let mut mixed = done("7", 200, 1000.0);
        mixed["response"]["statusText"] = json!("Found");
        log_request(&mut log, &mut last, "network.responseStarted", &mixed);
        assert_eq!(log[0].r.status, Some(200), "no later hop yet: as reported");
        log_request(&mut log, &mut last, "network.responseCompleted", &mixed);
        let mut target = hop(done("7", 200, 1000.0), 1);
        target["request"] = sent("7", "http://a/next.html", 1000.0, 1)["request"].clone();
        target["response"]["statusText"] = json!("OK");
        log_request(&mut log, &mut last, "network.responseStarted", &target);
        assert_eq!((log[0].r.status, log[1].r.status), (Some(302), Some(200)));
        // A late event of the redirect, the later hop known: at once.
        log_request(&mut log, &mut last, "network.responseStarted", &mixed);
        assert_eq!(log[0].r.status, Some(302));
        // A phrase of no redirect, or a request that did not redirect,
        // keeps the status it came with.
        log_request(&mut log, &mut last, "network.beforeRequestSent", &sent("8", "http://a/made", 1001.0, 0));
        let mut created = done("8", 201, 1001.0);
        created["response"]["statusText"] = json!("Found");
        log_request(&mut log, &mut last, "network.responseStarted", &created);
        assert_eq!(log[2].r.status, Some(201));
    }

    /// `network --clear` hides what it took: an event of a cleared request
    /// that comes after does not list it again.
    #[test]
    fn a_cleared_request_stays_cleared_when_its_events_come_after() {
        let (mut log, mut last) = (VecDeque::new(), 0);
        log_request(&mut log, &mut last, "network.beforeRequestSent", &sent("1", "http://a/x", 1000.0, 0));
        let cleared = last;
        log_request(&mut log, &mut last, "network.responseStarted", &done("1", 200, 1000.0));
        log_request(&mut log, &mut last, "network.beforeRequestSent", &sent("2", "http://a/y", 1001.0, 0));
        let shown: Vec<_> = super::listed(&log, cleared).into_iter().map(|r| (r.n, r.url)).collect();
        assert_eq!(shown, [(2, "http://a/y".to_string())]);
        assert_eq!(log[0].r.status, Some(200), "the hidden entry took its event");
        // A cleared request still pending is not waited for.
        assert!(!super::fresh_pending(&log, last, 1.0));
        assert!(super::fresh_pending(&log, cleared, 1.0));
    }

    #[test]
    fn a_response_that_only_started_has_its_status_and_no_end() {
        let (mut log, mut last) = (VecDeque::new(), 0);
        log_request(&mut log, &mut last, "network.beforeRequestSent", &sent("1", "http://a/x", 1000.0, 0));
        log_request(&mut log, &mut last, "network.responseStarted", &done("1", 404, 1004.0));
        assert_eq!((log[0].r.status, log[0].r.ended, log[0].r.size), (Some(404), None, None));
        log_request(&mut log, &mut last, "network.responseCompleted", &done("1", 404, 1009.0));
        assert_eq!((log[0].r.ended, log[0].r.size), (Some(1.009), Some(120)));
    }

    #[test]
    fn a_server_that_stamps_each_event_with_the_start_has_its_end_from_the_timings() {
        // chromedriver's: every event at the request's wall time, the
        // timings counted from the request (`requestTime` 0).
        let (mut log, mut last) = (VecDeque::new(), 0);
        log_request(&mut log, &mut last, "network.beforeRequestSent", &sent("1", "http://a/slow", 5000.0, 0));
        let mut slow = done("1", 200, 5000.0);
        slow["request"]["timings"] = json!({ "timeOrigin": 5000.0, "requestTime": 0.0, "responseEnd": 2003.85 });
        log_request(&mut log, &mut last, "network.responseCompleted", &slow);
        assert!((log[0].r.ended.unwrap() - log[0].r.started - 2.00385).abs() < 1e-9, "{:?}", log[0].r.ended);
        // Firefox's: the event at its own time, the timings since the
        // epoch: the event's time is the end.
        log_request(&mut log, &mut last, "network.beforeRequestSent", &sent("2", "http://a/x", 7000.0, 0));
        let mut ff = done("2", 200, 7040.0);
        ff["request"]["timings"] = json!({ "timeOrigin": 0.0, "requestTime": 6900.0, "responseEnd": 7039.5 });
        log_request(&mut log, &mut last, "network.responseCompleted", &ff);
        assert_eq!(log[1].r.ended, Some(7.04));
        // No timings, or none ended: the event's time.
        log_request(&mut log, &mut last, "network.beforeRequestSent", &sent("3", "http://a/y", 8000.0, 0));
        let mut none = done("3", 200, 8000.0);
        none["request"]["timings"] = json!({ "requestTime": 0.0, "responseEnd": 0.0 });
        log_request(&mut log, &mut last, "network.responseCompleted", &none);
        assert_eq!(log[2].r.ended, Some(8.0));
    }

    #[test]
    fn only_a_request_sent_just_now_without_an_answer_is_waited_for() {
        let (mut log, mut last) = (VecDeque::new(), 0);
        log_request(&mut log, &mut last, "network.beforeRequestSent", &sent("1", "http://a/x", 100_000.0, 0));
        assert!(super::fresh_pending(&log, 0, 101.0));
        assert!(!super::fresh_pending(&log, 0, 103.0), "a long poll is not waited for");
        log_request(&mut log, &mut last, "network.responseStarted", &done("1", 200, 100_500.0));
        assert!(!super::fresh_pending(&log, 0, 101.0));
    }

    #[test]
    fn the_log_keeps_the_newest_thousand() {
        let (mut log, mut last) = (VecDeque::new(), 0);
        for i in 0..REQUESTS_MAX + 5 {
            log_request(&mut log, &mut last, "network.beforeRequestSent", &sent(&i.to_string(), "http://a/", 0.0, 0));
        }
        assert_eq!((log.len(), log[0].r.n, last), (REQUESTS_MAX, 6, REQUESTS_MAX as u64 + 5));
    }
}
