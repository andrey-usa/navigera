//! What the page said and fetched: console messages, uncaught exceptions,
//! browser log entries, and every network request, per tab. Fed by the event
//! watcher (`events.rs`), read by `console`, `errors` and `network`.
//!
//! Both logs are bounded rings: an agent asks about the last few hundred
//! entries, never the whole history of a long session.

use std::collections::{HashMap, VecDeque};

use serde_json::{json, Value};

/// Entries kept per tab and log.
const MAX_CONSOLE: usize = 1000;
const MAX_REQUESTS: usize = 1000;
/// Longest console text kept (a logged 2 MB JSON string is not useful).
const MAX_TEXT: usize = 2000;
/// Most of a request's body the log keeps for `network <n>`, in bytes.
const MAX_POST_DATA: usize = 16 * 1024;
/// Most of a request's URL the log keeps: a longer one (Chrome sends up to
/// 2 MiB) keeps its start and says how much was cut. A server takes 8 KiB
/// by default (nginx, Apache), so a URL a site works with is kept whole.
const URL_KEPT: usize = 16 << 10;
/// Most of a request's or a response's headers the log keeps, names and
/// values: past it the rest are left out, and one header says how many.
/// With [`URL_KEPT`] and [`MAX_POST_DATA`], counted in bytes with what says
/// what was cut, an entry's URL, body and headers keep 64 KiB at most, so
/// the [`MAX_REQUESTS`] of a tab stay under 64 MiB (a page that sent a
/// thousand 64 KiB URLs with 64 KiB of headers each made 128 MiB).
const HEADERS_KEPT: usize = 16 << 10;
/// The header that says how many headers were left out.
const LEFT_OUT: &str = "navigera-left-out";
/// Most of a URL the request list shows, on the page's origin too: the
/// whole is in `network <n>`.
const LISTED: usize = 200;

#[derive(Clone, Debug)]
pub struct ConsoleEntry {
    /// `log`, `info`, `warn`, `error`, `debug`.
    pub level: String,
    /// `console`, `exception` or `browser` (a Log-domain entry: failed
    /// loads, CSP and mixed-content reports).
    pub source: &'static str,
    pub text: String,
    /// `url:line` where it came from, when known.
    pub location: String,
}

#[derive(Clone, Debug, Default)]
pub struct Request {
    /// Session-wide number shown to the agent (`#12`).
    pub n: u64,
    pub id: String,
    pub method: String,
    pub url: String,
    /// CDP resource type: Document, XHR, Fetch, Script, Image, …
    pub kind: String,
    pub status: Option<u64>,
    pub mime: String,
    pub request_headers: Value,
    pub response_headers: Value,
    pub post_data: Option<String>,
    /// The request has a body CDP didn't inline (`Network.getRequestPostData`).
    pub has_post_data: bool,
    /// Bytes on the wire (`encodedDataLength`).
    pub size: Option<u64>,
    /// CDP monotonic timestamps (seconds).
    pub started: f64,
    pub ended: Option<f64>,
    /// Wall-clock start (seconds since the epoch), for HAR.
    pub wall: f64,
    pub failed: Option<String>,
    pub from_cache: bool,
}

#[derive(Default)]
pub struct TabLogs {
    pub console: VecDeque<ConsoleEntry>,
    pub requests: VecDeque<Request>,
    /// Uncaught exceptions not yet reported in a response note.
    pub unreported_exceptions: Vec<String>,
}

#[derive(Default)]
pub struct Logs {
    pub tabs: HashMap<String, TabLogs>,
    next_request: u64,
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// The bytes of `…(+N bytes)` but N's digits, which say what [`cut`] cut.
const CUT_SAYS: usize = "…(+ bytes)".len();

/// Decimal digits of `n`.
fn digits(n: usize) -> usize {
    n.checked_ilog10().map_or(1, |d| d as usize + 1)
}

/// `text` in `max` bytes at most: whole when it fits, else its start, cut
/// at a character, and how many bytes were cut (`max` leaves room for
/// that). Its buffer is its length: a `format!` grew a 16 KiB cut to 32
/// KiB, and the log keeps a thousand of them.
fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    // Less than all of `text` is cut, so its length's digits are room enough.
    let mut end = max.saturating_sub(CUT_SAYS + digits(text.len()));
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let says = format!("…(+{} bytes)", text.len() - end);
    let mut out = String::with_capacity(end + says.len());
    out.push_str(&text[..end]);
    out.push_str(&says);
    out
}

/// A request's URL as the log keeps it ([`URL_KEPT`]).
pub fn kept_url(url: &str) -> String {
    cut(url, URL_KEPT)
}

/// A request's body as the log keeps it ([`MAX_POST_DATA`]), also one the
/// browser is asked for after its event.
pub fn kept_post_data(body: &str) -> String {
    cut(body, MAX_POST_DATA)
}

/// A request's or a response's headers as the log keeps them
/// ([`HEADERS_KEPT`]): whole when they fit, else those that fit, the start
/// of the first that does not, and [`LEFT_OUT`] saying what was left out.
/// Only what is kept is copied.
pub fn kept_headers(headers: Option<&Value>) -> Value {
    let Some(Value::Object(all)) = headers else { return headers.cloned().unwrap_or(Value::Null) };
    let size =
        |name: &str, value: &Value| name.len() + value.as_str().map_or_else(|| value.to_string().len(), str::len);
    let total: usize = all.iter().map(|(k, v)| size(k, v)).sum();
    if total <= HEADERS_KEPT {
        return Value::Object(all.clone());
    }
    // Room for what says what was left out: at most every header, `total` bytes.
    let budget = HEADERS_KEPT - (LEFT_OUT.len() + " more,  bytes".len() + digits(all.len()) + digits(total));
    let mut kept = serde_json::Map::new();
    let (mut used, mut left, mut bytes) = (0, 0usize, 0usize);
    for (name, value) in all {
        let need = size(name, value);
        let room = budget.saturating_sub(used + name.len());
        if used + need <= budget {
            used += need;
            kept.insert(name.clone(), value.clone());
        } else if let Some(text) = value.as_str().filter(|t| room > CUT_SAYS + digits(t.len())) {
            kept.insert(name.clone(), json!(cut(text, room)));
            used = budget;
        } else {
            left += 1;
            bytes += need;
        }
    }
    if left > 0 {
        kept.insert(LEFT_OUT.into(), json!(format!("{left} more, {bytes} bytes")));
    }
    Value::Object(kept)
}

/// One console argument as the page's own `console` would show it.
fn remote_object_text(arg: &Value) -> String {
    if let Some(v) = arg.get("value") {
        return match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
    }
    if let Some(v) = arg.get("unserializableValue").and_then(Value::as_str) {
        return v.to_string();
    }
    if let Some(preview) = arg.get("preview") {
        let array = preview.get("subtype").and_then(Value::as_str) == Some("array");
        let props: Vec<String> = preview
            .get("properties")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|p| {
                let raw = p.get("value").and_then(Value::as_str).unwrap_or("…");
                let value = if p.get("type").and_then(Value::as_str) == Some("string") {
                    format!("'{raw}'")
                } else {
                    raw.to_string()
                };
                if array {
                    value
                } else {
                    format!("{}: {value}", p.get("name").and_then(Value::as_str).unwrap_or(""))
                }
            })
            .collect();
        let overflow = if preview.get("overflow").and_then(Value::as_bool) == Some(true) { ", …" } else { "" };
        let (open, close) = if array { ("[", "]") } else { ("{", "}") };
        return format!("{open}{}{overflow}{close}", props.join(", "));
    }
    arg.get("description")
        .and_then(Value::as_str)
        .or_else(|| arg.get("type").and_then(Value::as_str))
        .unwrap_or("")
        .to_string()
}

fn location(url: Option<&str>, line: Option<u64>) -> String {
    match (url.filter(|u| !u.is_empty()), line) {
        (Some(u), Some(l)) => format!("{u}:{}", l + 1),
        (Some(u), None) => u.to_string(),
        _ => String::new(),
    }
}

/// A request a page sent, as a click's effect check sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Sent {
    /// `METHOD /path (status)`.
    pub label: String,
    pub done: bool,
    pub report: Report,
}

/// Whether a request looks like the page reporting on itself rather than
/// doing what was asked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Report {
    /// A beacon, or a well-known analytics or monitoring service.
    Surely,
    /// A first-party path that reads like reporting (`/event/…`, `/collect`,
    /// `/log`): judged by what the page does once it is answered.
    Maybe,
    /// Anything else: the page is doing something.
    No,
}

const REPORT_HOSTS: &[&str] = &[
    "google-analytics.com",
    "analytics.google.com",
    "googletagmanager.com",
    "doubleclick.net",
    "googlesyndication.com",
    "segment.io",
    "segment.com",
    "mixpanel.com",
    "amplitude.com",
    "hotjar.com",
    "hotjar.io",
    "fullstory.com",
    "heapanalytics.com",
    "nr-data.net",
    "datadoghq.com",
    "datadoghq.eu",
    "sentry.io",
    "bugsnag.com",
    "clarity.ms",
    "bat.bing.com",
    "facebook.com",
    "facebook.net",
    "px.ads.linkedin.com",
    "posthog.com",
    "plausible.io",
    "quantserve.com",
    "scorecardresearch.com",
    "omtrdc.net",
    "demdex.net",
    "adobedc.net",
    "analytics.tiktok.com",
    "ct.pinterest.com",
    "analytics.twitter.com",
    "mc.yandex.ru",
    "lr-ingest.io",
    "cloudflareinsights.com",
    "logx.optimizely.com",
    "events.launchdarkly.com",
    "statsigapi.net",
    "snowplowanalytics.com",
];

const REPORT_SEGMENTS: &[&str] = &[
    "collect",
    "log",
    "logs",
    "event",
    "events",
    "analytics",
    "track",
    "tracking",
    "beacon",
    "telemetry",
    "rum",
    "metrics",
    "pixel",
    "ping",
    "stats",
    "ingest",
];

fn report_kind(kind: &str, host: &str, path: &str) -> Report {
    let host = host.rsplit('@').next().unwrap_or(host).split(':').next().unwrap_or(host).to_ascii_lowercase();
    let path_l = path.to_ascii_lowercase();
    // Beacons, analytics services, and what the browser or the page loads
    // on its own while a click is watched (the tab's icon, lazy images,
    // fonts): none of it is the click doing something. A click that shows
    // an image changes the DOM, and that is seen.
    if kind == "Ping"
        || matches!(kind, "Image" | "Media" | "Font" | "Manifest" | "TextTrack" | "CSPViolationReport" | "Prefetch")
        || path_l.ends_with("/favicon.ico")
        || path_l.contains("apple-touch-icon")
        || REPORT_HOSTS.iter().any(|h| host == *h || host.ends_with(&format!(".{h}")))
    {
        return Report::Surely;
    }
    // Reporting-like paths, and code a click may load before it renders
    // (a lazily imported chunk): judged by what follows.
    if matches!(kind, "Script" | "Stylesheet")
        || path.split('/').any(|seg| REPORT_SEGMENTS.contains(&seg.to_ascii_lowercase().as_str()))
    {
        return Report::Maybe;
    }
    Report::No
}

impl Logs {
    /// Requests seen so far, all tabs.
    pub fn requests_started(&self) -> u64 {
        self.next_request
    }

    /// Scripts a tab is still loading: code that may wire up what is on the
    /// page once it runs (see `Page::wired_later`).
    pub fn scripts_loading(&self, session: &str) -> usize {
        let Some(tab) = self.tabs.get(session) else { return 0 };
        tab.requests.iter().filter(|r| r.kind == "Script" && r.ended.is_none() && r.failed.is_none()).count()
    }

    /// Requests a tab started after request `#after` (a number from
    /// [`Logs::requests_started`]).
    pub fn requests_after(&self, session: &str, after: u64) -> Vec<Sent> {
        let Some(tab) = self.tabs.get(session) else { return Vec::new() };
        tab.requests
            .iter()
            .filter(|r| r.n > after)
            .map(|r| {
                // The path, without scheme, host or query.
                let rest = r.url.split_once("://").map_or(r.url.as_str(), |(_, rest)| rest);
                let (host, path) = rest.find('/').map_or((rest, "/"), |i| (&rest[..i], &rest[i..]));
                let path = clip(path.split(['?', '#']).next().unwrap_or("/"), 80);
                let status = r.status.map(|s| format!(" ({s})")).unwrap_or_default();
                Sent {
                    label: format!("{} {path}{status}", r.method),
                    done: r.ended.is_some() || r.failed.is_some(),
                    report: report_kind(&r.kind, host, &path),
                }
            })
            .collect()
    }

    fn tab(&mut self, session: &str) -> &mut TabLogs {
        self.tabs.entry(session.to_string()).or_default()
    }

    fn push_console(&mut self, session: &str, entry: ConsoleEntry) {
        let tab = self.tab(session);
        if tab.console.len() >= MAX_CONSOLE {
            tab.console.pop_front();
        }
        tab.console.push_back(entry);
    }

    /// Feed one CDP event; ignores everything that isn't a log or network event.
    pub fn on_event(&mut self, method: &str, params: &Value, session: &str) {
        if session.is_empty() {
            return;
        }
        match method {
            "Runtime.consoleAPICalled" => {
                let kind = params.get("type").and_then(Value::as_str).unwrap_or("log");
                let level = match kind {
                    "error" | "assert" => "error",
                    "warning" => "warn",
                    "info" => "info",
                    "debug" => "debug",
                    _ => "log",
                };
                let text: Vec<String> = params
                    .get("args")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(remote_object_text)
                    .collect();
                let frame = params.pointer("/stackTrace/callFrames/0");
                let loc = location(
                    frame.and_then(|f| f.get("url")).and_then(Value::as_str),
                    frame.and_then(|f| f.get("lineNumber")).and_then(Value::as_u64),
                );
                self.push_console(
                    session,
                    ConsoleEntry {
                        level: level.into(),
                        source: "console",
                        text: clip(&text.join(" "), MAX_TEXT),
                        location: loc,
                    },
                );
            }
            "Runtime.exceptionThrown" => {
                let d = params.get("exceptionDetails").cloned().unwrap_or(Value::Null);
                // `description` carries "TypeError: … \n    at …": the first
                // line is the message, the stack follows.
                let text = d
                    .pointer("/exception/description")
                    .and_then(Value::as_str)
                    .or_else(|| d.pointer("/exception/value").and_then(Value::as_str))
                    .or_else(|| d.get("text").and_then(Value::as_str))
                    .unwrap_or("exception")
                    .to_string();
                let loc = location(d.get("url").and_then(Value::as_str), d.get("lineNumber").and_then(Value::as_u64));
                let first = text.lines().next().unwrap_or("").to_string();
                self.tab(session).unreported_exceptions.push(first);
                self.push_console(
                    session,
                    ConsoleEntry {
                        level: "error".into(),
                        source: "exception",
                        text: clip(&text, MAX_TEXT),
                        location: loc,
                    },
                );
            }
            "Log.entryAdded" => {
                let e = params.get("entry").cloned().unwrap_or(Value::Null);
                let level = match e.get("level").and_then(Value::as_str).unwrap_or("info") {
                    "error" => "error",
                    "warning" => "warn",
                    "verbose" => return,
                    _ => "info",
                };
                let text = e.get("text").and_then(Value::as_str).unwrap_or("").to_string();
                let url = e.get("url").and_then(Value::as_str);
                // Chrome asks every origin for /favicon.ico on its own: not
                // the page's error.
                if url.is_some_and(|u| u.ends_with("/favicon.ico")) {
                    return;
                }
                let loc = location(url, e.get("lineNumber").and_then(Value::as_u64));
                self.push_console(
                    session,
                    ConsoleEntry { level: level.into(), source: "browser", text: clip(&text, MAX_TEXT), location: loc },
                );
            }
            "Network.requestWillBeSent" => {
                let Some(id) = params.get("requestId").and_then(Value::as_str) else { return };
                let req = params.get("request").unwrap_or(&Value::Null);
                // A redirect reuses the request id: close the hop that
                // redirected, then log the next one as its own entry.
                if let Some(redirect) = params.get("redirectResponse") {
                    let ts = params.get("timestamp").and_then(Value::as_f64);
                    if let Some(prev) =
                        self.tab(session).requests.iter_mut().rev().find(|r| r.id == id && r.ended.is_none())
                    {
                        prev.status = redirect.get("status").and_then(Value::as_u64);
                        prev.response_headers = kept_headers(redirect.get("headers"));
                        prev.mime = redirect.get("mimeType").and_then(Value::as_str).unwrap_or("").to_string();
                        prev.ended = ts;
                    }
                }
                self.next_request += 1;
                let entry = Request {
                    n: self.next_request,
                    id: id.to_string(),
                    method: req.get("method").and_then(Value::as_str).unwrap_or("GET").to_string(),
                    url: kept_url(req.get("url").and_then(Value::as_str).unwrap_or("")),
                    kind: params.get("type").and_then(Value::as_str).unwrap_or("Other").to_string(),
                    request_headers: kept_headers(req.get("headers")),
                    post_data: req.get("postData").and_then(Value::as_str).map(kept_post_data),
                    has_post_data: req.get("hasPostData").and_then(Value::as_bool) == Some(true),
                    started: params.get("timestamp").and_then(Value::as_f64).unwrap_or(0.0),
                    wall: params.get("wallTime").and_then(Value::as_f64).unwrap_or(0.0),
                    ..Request::default()
                };
                let tab = self.tab(session);
                if tab.requests.len() >= MAX_REQUESTS {
                    tab.requests.pop_front();
                }
                tab.requests.push_back(entry);
            }
            "Network.responseReceived" => {
                let Some(id) = params.get("requestId").and_then(Value::as_str) else { return };
                let resp = params.get("response").unwrap_or(&Value::Null);
                if let Some(r) = self.find_open(session, id) {
                    r.status = resp.get("status").and_then(Value::as_u64);
                    r.mime = resp.get("mimeType").and_then(Value::as_str).unwrap_or("").to_string();
                    r.response_headers = kept_headers(resp.get("headers"));
                    r.from_cache = resp.get("fromDiskCache").and_then(Value::as_bool) == Some(true)
                        || resp.get("fromServiceWorker").and_then(Value::as_bool) == Some(true);
                    if let Some(t) = params.get("type").and_then(Value::as_str) {
                        r.kind = t.to_string();
                    }
                }
            }
            "Network.loadingFinished" => {
                let Some(id) = params.get("requestId").and_then(Value::as_str) else { return };
                let ts = params.get("timestamp").and_then(Value::as_f64);
                let size = params.get("encodedDataLength").and_then(Value::as_f64).map(|s| s as u64);
                if let Some(r) = self.find_open(session, id) {
                    r.ended = ts.or(Some(r.started));
                    r.size = size;
                }
            }
            "Network.loadingFailed" => {
                let Some(id) = params.get("requestId").and_then(Value::as_str) else { return };
                let ts = params.get("timestamp").and_then(Value::as_f64);
                let why = if params.get("canceled").and_then(Value::as_bool) == Some(true) {
                    "canceled".to_string()
                } else {
                    params
                        .get("blockedReason")
                        .and_then(Value::as_str)
                        // `inspector`: a `route` rule failed it.
                        .map(|b| if b == "inspector" { "route".to_string() } else { format!("blocked: {b}") })
                        .or_else(|| params.get("errorText").and_then(Value::as_str).map(str::to_string))
                        .unwrap_or_else(|| "failed".into())
                };
                if let Some(r) = self.find_open(session, id) {
                    r.ended = ts.or(Some(r.started));
                    r.failed = Some(why);
                }
            }
            _ => {}
        }
    }

    fn find_open(&mut self, session: &str, id: &str) -> Option<&mut Request> {
        self.tabs.get_mut(session)?.requests.iter_mut().rev().find(|r| r.id == id)
    }

    /// Uncaught exceptions since the last call, for a response note.
    pub fn take_exceptions(&mut self, session: &str) -> Vec<String> {
        self.tabs.get_mut(session).map(|t| std::mem::take(&mut t.unreported_exceptions)).unwrap_or_default()
    }
}

/// Console filter: `errors` is `level == error`.
pub fn console_text(entries: &[ConsoleEntry]) -> String {
    entries
        .iter()
        .map(|e| {
            let tag = if e.source == "exception" { "exception".to_string() } else { e.level.clone() };
            if e.location.is_empty() {
                format!("[{tag}] {}", e.text)
            } else {
                format!("[{tag}] {}  ({})", e.text, e.location)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn console_json(entries: &[ConsoleEntry]) -> Value {
    Value::Array(
        entries
            .iter()
            .map(|e| json!({ "level": e.level, "source": e.source, "text": e.text, "location": e.location }))
            .collect(),
    )
}

fn human_bytes(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / 1024.0 / 1024.0)
    }
}

/// `url` relative to `origin` when it is on it, at most [`LISTED`]
/// characters.
fn short(url: &str, origin: &str) -> String {
    if !origin.is_empty() {
        if let Some(rest) = url.strip_prefix(origin) {
            if rest.starts_with('/') {
                return clip(rest, LISTED);
            }
        }
    }
    clip(url, LISTED)
}

/// One line per request: `#7 POST 201 Fetch /api/orders 512 B 23 ms`.
pub fn requests_text(requests: &[Request], origin: &str) -> String {
    requests
        .iter()
        .map(|r| {
            let status = match (&r.failed, r.status) {
                (Some(why), _) => format!("FAILED({why})"),
                (None, Some(s)) => s.to_string(),
                (None, None) => "pending".to_string(),
            };
            let mut line = format!("#{} {} {} {} {}", r.n, r.method, status, r.kind, short(&r.url, origin));
            if let Some(size) = r.size {
                line.push_str(&format!(" {}", human_bytes(size)));
            }
            if let Some(end) = r.ended {
                line.push_str(&format!(" {:.0} ms", ((end - r.started) * 1000.0).max(0.0)));
            }
            if r.from_cache {
                line.push_str(" (cache)");
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `network`'s filters: a URL part, a kind (any case), failed ones (an
/// error, or a status of 400 or more), and those after request `#since`.
pub fn pick_requests(
    all: Vec<Request>,
    filter: Option<&str>,
    kind: Option<&str>,
    failed: bool,
    since: Option<u64>,
) -> Vec<Request> {
    let kind = kind.map(str::to_ascii_lowercase);
    all.into_iter()
        .filter(|r| filter.is_none_or(|f| r.url.contains(f)))
        .filter(|r| kind.as_ref().is_none_or(|k| r.kind.to_ascii_lowercase() == *k))
        .filter(|r| !failed || r.failed.is_some() || r.status.is_some_and(|s| s >= 400))
        .filter(|r| since.is_none_or(|n| r.n > n))
        .collect()
}

/// The full record of one request (bodies are added by the caller).
/// The requests that redirected, each with the number of the hop it led
/// to. A redirect's hops share the request's id, and the browser keeps one
/// response body per id, the last hop's (Chrome's `Network.getResponseBody`,
/// WebDriver BiDi's `network.getData`): no hop before it has a body of its
/// own to show.
pub fn redirected(all: &[Request]) -> HashMap<u64, u64> {
    let mut next: HashMap<&str, u64> = HashMap::new();
    let mut out = HashMap::new();
    for r in all.iter().rev() {
        if let Some(&later) = next.get(r.id.as_str()) {
            out.insert(r.n, later);
        }
        next.insert(r.id.as_str(), r.n);
    }
    out
}

/// `network <n>`'s body for a hop that redirected ([`redirected`]).
pub fn redirect_body(next: u64) -> String {
    format!("(none: it redirected, to #{next}, and the browser keeps the body of a request's last hop only)")
}

pub fn request_json(r: &Request) -> Value {
    json!({
        "n": r.n,
        "method": r.method,
        "url": r.url,
        "type": r.kind,
        "status": r.status,
        "failed": r.failed,
        "mime": r.mime,
        "size": r.size,
        "ms": r.ended.map(|e| ((e - r.started) * 1000.0).max(0.0).round()),
        "from_cache": r.from_cache,
        "request_headers": r.request_headers,
        "post_data": r.post_data,
        "response_headers": r.response_headers,
    })
}

fn har_headers(headers: &Value) -> Value {
    Value::Array(
        headers
            .as_object()
            .into_iter()
            .flatten()
            .map(
                |(k, v)| json!({ "name": k, "value": v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()) }),
            )
            .collect(),
    )
}

/// HAR 1.2 of the logged requests (no response bodies).
pub fn har(requests: &[Request], creator_version: &str) -> Value {
    let entries: Vec<Value> = requests
        .iter()
        .map(|r| {
            let ms = r.ended.map(|e| ((e - r.started) * 1000.0).max(0.0)).unwrap_or(0.0);
            let started = chrono_like_iso(r.wall);
            let query: Vec<Value> = r
                .url
                .split_once('?')
                .map(|(_, q)| q.split('&').filter(|p| !p.is_empty()).collect::<Vec<_>>())
                .unwrap_or_default()
                .into_iter()
                .map(|p| {
                    let (k, v) = p.split_once('=').unwrap_or((p, ""));
                    json!({ "name": k, "value": v })
                })
                .collect();
            let mut request = json!({
                "method": r.method, "url": r.url, "httpVersion": "HTTP/1.1",
                "headers": har_headers(&r.request_headers), "queryString": query, "cookies": [],
                "headersSize": -1, "bodySize": r.post_data.as_ref().map(|p| p.len() as i64).unwrap_or(0),
            });
            if let Some(p) = &r.post_data {
                let mime = r
                    .request_headers
                    .as_object()
                    .and_then(|h| h.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-type")))
                    .and_then(|(_, v)| v.as_str())
                    .unwrap_or("");
                request["postData"] = json!({ "mimeType": mime, "text": p });
            }
            json!({
                "startedDateTime": started,
                "time": ms,
                "request": request,
                "response": {
                    "status": r.status.unwrap_or(0), "statusText": r.failed.clone().unwrap_or_default(),
                    "httpVersion": "HTTP/1.1", "headers": har_headers(&r.response_headers), "cookies": [],
                    "content": { "size": r.size.unwrap_or(0), "mimeType": r.mime },
                    "redirectURL": "", "headersSize": -1, "bodySize": r.size.map(|s| s as i64).unwrap_or(-1),
                },
                "cache": {},
                "timings": { "send": 0, "wait": ms, "receive": 0 },
                "_resourceType": r.kind.to_ascii_lowercase(),
            })
        })
        .collect();
    json!({ "log": {
        "version": "1.2",
        "creator": { "name": "navigera", "version": creator_version },
        "pages": [],
        "entries": entries,
    }})
}

/// RFC 3339 UTC time from seconds since the epoch (HAR wants ISO 8601).
fn chrono_like_iso(epoch_s: f64) -> String {
    let secs = epoch_s.floor() as i64;
    let millis = ((epoch_s - secs as f64) * 1000.0).round() as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}.{millis:03}Z")
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_hop_that_redirected_names_the_hop_it_led_to() {
        let r = |n: u64, id: &str| super::Request { n, id: id.into(), ..Default::default() };
        // Two redirects of request "a" (#1 -> #3 -> #4), "b" between them.
        let all = [r(1, "a"), r(2, "b"), r(3, "a"), r(4, "a"), r(5, "c")];
        let hops = super::redirected(&all);
        let mut got: Vec<_> = hops.into_iter().collect();
        got.sort();
        assert_eq!(got, [(1, 3), (3, 4)]);
        assert!(super::redirect_body(3).contains("#3"));
    }

    #[test]
    fn reporting_requests_are_told_apart() {
        assert_eq!(report_kind("Ping", "shop.test", "/cart"), Report::Surely);
        assert_eq!(report_kind("Fetch", "www.google-analytics.com", "/g/collect"), Report::Surely);
        assert_eq!(report_kind("Fetch", "region1.google-analytics.com:443", "/x"), Report::Surely);
        assert_eq!(report_kind("Fetch", "shop.test", "/event/modal"), Report::Maybe);
        assert_eq!(report_kind("XHR", "shop.test:8080", "/api/v1/Events"), Report::Maybe);
        assert_eq!(report_kind("Fetch", "shop.test", "/api/cart"), Report::No);
        assert_eq!(report_kind("Fetch", "notfacebook.com", "/x"), Report::No);
        assert_eq!(report_kind("Other", "shop.test", "/favicon.ico"), Report::Surely);
        assert_eq!(report_kind("Image", "shop.test", "/img/lazy.jpg"), Report::Surely);
        assert_eq!(report_kind("Script", "shop.test", "/chunks/editor.js"), Report::Maybe);
        assert_eq!(report_kind("Document", "shop.test", "/next"), Report::No);
    }
    use super::*;

    #[test]
    fn console_and_exceptions_are_logged_with_their_origin() {
        let mut logs = Logs::default();
        logs.on_event(
            "Runtime.consoleAPICalled",
            &json!({"type": "warning", "args": [{"type": "string", "value": "low stock"}, {"type": "number", "value": 3}],
                    "stackTrace": {"callFrames": [{"url": "http://x/app.js", "lineNumber": 9}]}}),
            "s1",
        );
        logs.on_event(
            "Runtime.exceptionThrown",
            &json!({"exceptionDetails": {"text": "Uncaught", "url": "http://x/app.js", "lineNumber": 41,
                    "exception": {"description": "TypeError: cart is undefined\n    at add (app.js:42)"}}}),
            "s1",
        );
        let entries: Vec<_> = logs.tabs["s1"].console.iter().cloned().collect();
        let text = console_text(&entries);
        assert!(text.contains("[warn] low stock 3  (http://x/app.js:10)"), "{text}");
        let arr = json!({"type": "object", "subtype": "array", "preview": {"subtype": "array", "properties": [
            {"name": "0", "value": "1"}, {"name": "1", "value": "2"}]}});
        assert_eq!(remote_object_text(&arr), "[1, 2]");
        assert!(text.contains("[exception] TypeError: cart is undefined"), "{text}");
        assert_eq!(logs.take_exceptions("s1"), vec!["TypeError: cart is undefined".to_string()]);
        assert!(logs.take_exceptions("s1").is_empty());
    }

    #[test]
    fn requests_follow_redirects_and_failures() {
        let mut logs = Logs::default();
        let send = |logs: &mut Logs, id: &str, url: &str, t: f64, redirect: Option<Value>| {
            let mut p = json!({"requestId": id, "type": "Document", "timestamp": t, "wallTime": 1.7e9,
                               "request": {"url": url, "method": "GET", "headers": {}}});
            if let Some(r) = redirect {
                p["redirectResponse"] = r;
            }
            logs.on_event("Network.requestWillBeSent", &p, "s");
        };
        send(&mut logs, "1", "http://h/old", 1.0, None);
        send(&mut logs, "1", "http://h/new", 1.05, Some(json!({"status": 301, "headers": {}})));
        logs.on_event(
            "Network.responseReceived",
            &json!({"requestId": "1", "type": "Document",
            "response": {"status": 200, "mimeType": "text/html", "headers": {"a": "b"}}}),
            "s",
        );
        logs.on_event(
            "Network.loadingFinished",
            &json!({"requestId": "1", "timestamp": 1.1, "encodedDataLength": 2048}),
            "s",
        );
        send(&mut logs, "2", "http://h/api", 1.2, None);
        logs.on_event(
            "Network.loadingFailed",
            &json!({"requestId": "2", "timestamp": 1.3, "errorText": "net::ERR_FAILED"}),
            "s",
        );
        let reqs: Vec<_> = logs.tabs["s"].requests.iter().cloned().collect();
        let text = requests_text(&reqs, "http://h");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "#1 GET 301 Document /old 50 ms");
        assert_eq!(lines[1], "#2 GET 200 Document /new 2.0 KB 50 ms");
        assert!(lines[2].starts_with("#3 GET FAILED(net::ERR_FAILED) Document /api"), "{text}");
        let har = har(&reqs, "0");
        assert_eq!(har["log"]["entries"].as_array().unwrap().len(), 3);
        assert_eq!(har["log"]["entries"][1]["response"]["status"], 200);
    }

    /// A page's URL and headers cost the log what fits under its bounds,
    /// however long they are, and the list shows 200 characters of a path
    /// on the page's own origin too (a thousand 64 KiB URLs listed 62 MiB).
    #[test]
    fn long_urls_and_headers_are_kept_in_part_and_listed_short() {
        let mut logs = Logs::default();
        let url = format!("http://h/r?q={}é", "a".repeat(1 << 20));
        let big: serde_json::Map<String, Value> =
            (0..64).map(|i| (format!("x-pad-{i:02}"), json!("b".repeat(1024)))).collect();
        // A body of 4-byte characters: 16 Ki of them were 64 KiB.
        let body = "😀".repeat(1 << 15);
        let p = json!({"requestId": "1", "type": "Fetch", "timestamp": 1.0, "wallTime": 1.7e9,
                       "request": {"url": url, "method": "POST", "headers": {"accept": "*/*"}, "postData": body}});
        logs.on_event("Network.requestWillBeSent", &p, "s");
        let resp = json!({"requestId": "1", "type": "Fetch",
                          "response": {"status": 200, "mimeType": "text/plain", "headers": Value::Object(big)}});
        logs.on_event("Network.responseReceived", &resp, "s");
        let r = &logs.tabs["s"].requests[0];
        // 1048591 bytes: 16365 kept, 12 + 7 say what was cut.
        assert!(r.url.len() == URL_KEPT && r.url.ends_with("a…(+1032226 bytes)"), "{}", &r.url[r.url.len() - 40..]);
        assert_eq!(r.url.capacity(), r.url.len(), "a cut URL's buffer is its length");
        assert!(r.url.starts_with("http://h/r?q=aaa"));
        let post = r.post_data.as_deref().unwrap();
        assert!(
            post.len() <= MAX_POST_DATA && post.len() > MAX_POST_DATA - 32 && post.starts_with('😀'),
            "{}",
            post.len()
        );
        assert_eq!(r.request_headers, json!({"accept": "*/*"}), "headers that fit are kept whole");
        let headers = r.response_headers.as_object().unwrap();
        let kept: usize = headers.iter().map(|(k, v)| k.len() + v.as_str().unwrap().len()).sum();
        assert!(kept <= HEADERS_KEPT && kept > HEADERS_KEPT - 64, "{kept}, what says what was left out included");
        // 15 whole (1032 bytes each), the start of the 16th, 48 left out.
        assert_eq!(headers[LEFT_OUT], "48 more, 49536 bytes", "{headers:?}");
        assert!(headers["x-pad-15"].as_str().unwrap().ends_with(" bytes)"));
        let text = requests_text(&logs.tabs["s"].requests.iter().cloned().collect::<Vec<_>>(), "http://h");
        assert!(text.chars().count() < 260 && text.contains("/r?q=aaa"), "{text}");
        // A URL cut inside a character keeps the whole character before.
        assert_eq!(cut(&format!("a{}", "é".repeat(10)), 16), "a…(+20 bytes)");
        assert_eq!(cut("abc", 3), "abc");
        for max in 0..40 {
            let text = "é".repeat(30);
            let c = cut(&text, max);
            assert!(c.len() <= max.max(CUT_SAYS + 2) && c.is_char_boundary(c.len()), "{max}: {c}");
        }
        assert_eq!(kept_headers(None), json!(null));
        assert_eq!(kept_headers(Some(&json!({"a": "b"}))), json!({"a": "b"}));
    }

    #[test]
    fn iso_time() {
        assert_eq!(chrono_like_iso(0.0), "1970-01-01T00:00:00.000Z");
        assert_eq!(chrono_like_iso(1_792_475_590.483), "2026-10-20T05:53:10.483Z");
    }
}
