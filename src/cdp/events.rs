//! Background CDP event watcher: JavaScript dialogs, pages opened by pages
//! (popups / `target=_blank`), and per-tab navigation state.
//!
//! Why a background task: a page that calls `alert()`/`confirm()` blocks
//! its renderer until the dialog is answered, so the CDP command that
//! triggered it (a click, an eval) never returns. The watcher answers the
//! dialog the moment Chrome announces it, per the session's policy, and
//! logs it so the next response can tell the agent what happened.
//!
//! Lightpanda answers a dialog the moment the page opens it, with whatever
//! response was armed beforehand (`LP.handleJavaScriptDialog`); the CDP
//! event only reports it. There the watcher arms the session's policy ahead
//! of time and re-arms it after each dialog, and reports what the page
//! actually got.
//!
//! Navigation state lets ops wait for a navigation that an earlier action
//! started (a link click, a JS redirect) instead of reading the old page:
//! Chrome reports `Page.frameStartedLoading` for a link click *before* the
//! click's own `Input.dispatchMouseEvent` response, and a JS-scheduled
//! navigation ~15 ms after it. Tracking events costs nothing when nothing
//! navigates (no fixed grace sleep per click).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::broadcast;

use super::client::CdpClient;

/// How the watcher answers `alert` / `confirm` / `prompt` / `beforeunload`.
#[derive(Clone, Debug, PartialEq)]
pub struct DialogPolicy {
    pub accept: bool,
    /// Text typed into `prompt()` dialogs when accepting.
    pub prompt_text: Option<String>,
}

impl Default for DialogPolicy {
    fn default() -> Self {
        // An agent that clicked "Delete" or "Place order" meant it; accepting
        // keeps flows moving. Every dialog is reported in the next response.
        DialogPolicy { accept: true, prompt_text: None }
    }
}

/// Main-frame navigation state of one tab (keyed by CDP session id).
#[derive(Clone, Debug, Default)]
pub struct NavState {
    /// Between `frameStartedLoading` and `frameStoppedLoading`.
    pub loading: bool,
    /// `DOMContentLoaded` fired since the last navigation started.
    pub dom_ready: bool,
    /// A navigation was requested (form submit, link, `location = …`) but
    /// has not started loading yet; when it was requested.
    pub requested_at: Option<std::time::Instant>,
    /// Bumped on every navigation request or load start.
    pub generation: u64,
    /// When the navigating document committed (main-frame
    /// `frameNavigated`): from then on the DOM is the new page, even if it
    /// is still being parsed.
    pub committed_at: Option<std::time::Instant>,
}

/// A fetch/XHR open this long is a long poll or a stream, not data the
/// page is about to render: it stops holding a snapshot, and the URL's
/// later requests (a long poll's reconnects) never hold one. A data request
/// slower than this is what `wait` is for.
const LONG_REQUEST: Duration = Duration::from_secs(1);

/// A tab's in-flight `fetch`/XHR requests (`Network` events), so a read
/// right after an action can wait for the data it triggered to arrive.
#[derive(Clone, Debug, Default)]
pub struct NetState {
    /// request id -> when it started, its method and URL without the query
    /// (`POST /collect`), and the quiet clock before it started (put back if
    /// it turns out to be background traffic and nothing else moved it).
    pub pending: HashMap<String, (std::time::Instant, String, Option<std::time::Instant>)>,
    /// Methods and URLs (without the query) whose requests are background
    /// traffic: one
    /// stayed open past [`LONG_REQUEST`] (a long poll, a stream), or the
    /// server answered 202/204, no content (an analytics beacon). Their
    /// requests neither hold a snapshot nor move the quiet clock.
    pub background: std::collections::HashSet<String>,
    /// When the last fetch/XHR started or ended, background traffic left
    /// out (see [`NetState::clock_moves`]).
    pub last: Option<std::time::Instant>,
    /// The recent moves of that clock: when, and which method and URL
    /// moved it. A URL found to be background traffic later (a beacon's
    /// 204 arriving after the next beacon started) is taken out again,
    /// whatever order the events came in.
    pub clock_moves: Vec<(std::time::Instant, String)>,
    /// URL without its query string -> when its last requests started (the
    /// last three), to tell a poll from data the page is waiting for.
    pub starts: HashMap<String, Vec<std::time::Instant>>,
    /// When a `wait` last succeeded on this tab: the agent already synced
    /// with the page then, so a snapshot needs no quiet wait unless a
    /// new request has started since (see [`skip_quiet_wait`]).
    pub synced: Option<std::time::Instant>,
    /// How many non-poll requests had started when that `wait` succeeded.
    pub synced_seq: Option<u64>,
    /// Non-poll requests started on this tab (polls excluded, as in
    /// [`Shared::net_state`]): a new one after the sync means the page
    /// changed after the agent synced with it.
    pub seq: u64,
    /// The request count and navigation generation when the agent's last
    /// action (a click, a key, a fill) started: requests after it, with no
    /// navigation since, are that action's own traffic.
    pub action: Option<(u64, u64)>,
}

impl NetState {
    /// The quiet clock moves: `target` started or ended now.
    fn touch(&mut self, target: &str) {
        let now = std::time::Instant::now();
        self.clock_moves.retain(|(t, _)| now.duration_since(*t) < Duration::from_secs(10));
        if self.clock_moves.len() >= 64 {
            self.clock_moves.remove(0);
        }
        self.clock_moves.push((now, target.to_string()));
        self.recompute_last();
    }

    /// The clock without the moves of what is now known to be background.
    fn recompute_last(&mut self) {
        let background = &self.background;
        self.last =
            self.clock_moves.iter().filter(|(_, t)| t.is_empty() || !background.contains(t)).map(|(at, _)| *at).max();
    }

    /// Requests in flight that may still bring data the page renders: one
    /// open past [`LONG_REQUEST`] is a long poll or a stream, and from now
    /// on its URL is background traffic.
    fn live_pending(&mut self) -> usize {
        let mut live = 0;
        for (started, key, _) in self.pending.values() {
            if started.elapsed() < LONG_REQUEST {
                live += 1;
            } else if !key.is_empty() {
                self.background.insert(key.clone());
            }
        }
        live
    }
}

/// A snapshot may skip the network-quiet wait when a `wait` already synced
/// with the page and no new request has started since, and none is in
/// flight. A request that started before the wait and lands after it is
/// fine: its data is in the DOM by snapshot time. A request that started
/// after the wait is not: the page changed after the sync, so the snapshot
/// waits for quiet as usual. (Polls don't count: they neither start new
/// data nor move the quiet clock.)
pub(crate) fn skip_quiet_wait(synced_seq: Option<u64>, current_seq: u64, pending: usize) -> bool {
    synced_seq.is_some_and(|s| s == current_seq) && pending == 0
}

/// A request to a URL that has already been requested twice or more, at a
/// steady interval (this gap within half to double the one before, between
/// [`POLL_MIN_INTERVAL`] and [`POLL_MAX_INTERVAL`]), is a poll: it neither
/// counts as pending nor moves the quiet clock. `starts` are the earlier
/// starts of that URL, oldest first.
fn is_poll(starts: &[std::time::Instant], now: std::time::Instant) -> bool {
    let [.., a, b] = starts else { return false };
    let before = b.duration_since(*a);
    let gap = now.duration_since(*b);
    (POLL_MIN_INTERVAL..=POLL_MAX_INTERVAL).contains(&before) && gap >= before / 2 && gap <= before * 2
}

/// The shortest gap between requests that can make them a poll: a burst
/// to one path (a `Promise.all`, a quick chain of pages) is loading, not
/// polling, and must keep the wait.
const POLL_MIN_INTERVAL: Duration = Duration::from_millis(250);

/// The longest gap between requests that still makes them a poll.
const POLL_MAX_INTERVAL: Duration = Duration::from_secs(10);

/// Sessions on one browser (forks, or one session across `reset`s) each
/// keep their own routes, HTTP credentials and dialog answer, keyed by the
/// browser context their tabs live in: this key ("" for the default
/// context). A page is registered with the key of the session that opened
/// or attached it.
pub type Scope = String;

#[derive(Default)]
pub struct Shared {
    /// How dialogs are answered, per scope (the default when unset).
    pub policy: Mutex<HashMap<Scope, DialogPolicy>>,
    /// Dialogs answered since the last drain: (scope, `{type, message,
    /// accepted}`).
    pub dialogs: Mutex<Vec<(Scope, Value)>>,
    /// Page session id -> the scope it belongs to.
    pub scopes: Mutex<HashMap<String, Scope>>,
    /// Frame id of a subframe -> the scope of the tab it is in (a download
    /// a subframe starts is that tab's).
    pub frame_scopes: Mutex<HashMap<String, Scope>>,
    /// Page targets opened by a page (they carry an `openerId`), in order:
    /// (target, opener).
    pub opened: Mutex<Vec<(String, String)>>,
    /// An Electron app (`start --electron`): every page target it opens is
    /// one of its windows, adopted as a tab (an app's new window has no
    /// opener); its DevTools windows are not.
    pub adopt_windows: std::sync::atomic::AtomicBool,
    /// Page targets that went away (e.g. `window.close()`).
    pub destroyed: Mutex<Vec<String>>,
    /// session id -> target id, registered when a tab is attached. A page
    /// target's main frame id equals its target id.
    pub sessions: Mutex<HashMap<String, String>>,
    pub nav: Mutex<HashMap<String, NavState>>,
    pub net: Mutex<HashMap<String, NetState>>,
    /// Lightpanda: one page target per process, dialogs answered from an
    /// armed response, popups opened as frames that are not targets.
    pub single_target: AtomicBool,
    /// session id -> the dialog response armed there (Lightpanda).
    pub armed: Mutex<HashMap<String, DialogPolicy>>,
    /// Top-level frames a page opened that are not targets (Lightpanda's
    /// `target=_blank` / `window.open`), in order: (frame id, url).
    pub popups: Mutex<Vec<(String, String)>>,
    /// Frames announced by `Page.frameAttached` (iframes): not popups.
    pub child_frames: Mutex<std::collections::HashSet<String>>,
    /// Events the watcher has handled (see [`Shared::caught_up`]).
    pub processed: std::sync::atomic::AtomicU64,
    /// Console, exceptions and network requests per tab.
    pub logs: Mutex<super::logs::Logs>,
    /// Files the pages downloaded (`downloads`, `wait --download`).
    pub downloads: Mutex<super::downloads::Downloads>,
    /// `route` rules per scope, first match wins; requests matching none
    /// continue.
    pub routes: Mutex<HashMap<Scope, Vec<Route>>>,
    /// `set credentials` per scope: answers HTTP auth challenges.
    pub credentials: Mutex<HashMap<Scope, (String, String)>>,
    /// Several sessions share this browser (`BrowserSession::fork`): each
    /// takes only the tabs its own tabs opened or closed.
    pub forked: std::sync::atomic::AtomicBool,
}

/// What a `route` does with the requests its pattern matches.
#[derive(Clone, Debug, PartialEq)]
pub enum RouteAction {
    Abort,
    Fulfill { status: u16, headers: Vec<(String, String)>, body_base64: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    /// Glob over the full URL: `*` any run, `?` one character.
    pub pattern: String,
    pub action: RouteAction,
    pub hits: u64,
}

/// `*`/`?` glob match (the syntax of CDP's `Fetch` url patterns).
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// The answer to a paused request: (method, params) for the CDP call.
fn route_answer(shared: &Shared, session: &str, request_id: &str, url: &str) -> (&'static str, Value) {
    let scope = shared.scope_of(session);
    let mut routes = shared.routes.lock().unwrap();
    let Some(route) = routes.get_mut(&scope).and_then(|rs| rs.iter_mut().find(|r| glob_match(&r.pattern, url))) else {
        return ("Fetch.continueRequest", json!({ "requestId": request_id }));
    };
    route.hits += 1;
    match &route.action {
        RouteAction::Abort => {
            ("Fetch.failRequest", json!({ "requestId": request_id, "errorReason": "BlockedByClient" }))
        }
        RouteAction::Fulfill { status, headers, body_base64 } => (
            "Fetch.fulfillRequest",
            json!({
                "requestId": request_id,
                "responseCode": status,
                "responseHeaders": headers.iter().map(|(k, v)| json!({ "name": k, "value": v })).collect::<Vec<_>>(),
                "body": body_base64,
            }),
        ),
    }
}

impl Shared {
    pub fn register(&self, session_id: &str, target_id: &str, scope: &str) {
        self.sessions.lock().unwrap().insert(session_id.to_string(), target_id.to_string());
        self.scopes.lock().unwrap().insert(session_id.to_string(), scope.to_string());
    }

    /// The scope a page session belongs to ("" when unknown).
    pub fn scope_of(&self, session_id: &str) -> Scope {
        self.scopes.lock().unwrap().get(session_id).cloned().unwrap_or_default()
    }

    /// How dialogs in `scope` are answered.
    pub fn policy_for(&self, scope: &str) -> DialogPolicy {
        self.policy.lock().unwrap().get(scope).cloned().unwrap_or_default()
    }

    /// A session moved to a new browser context (`reset`): its routes,
    /// credentials and dialog answer go with it; `keep: false` drops them.
    pub fn move_scope(&self, from: &str, to: &str, keep: bool) {
        fn mv<T>(map: &Mutex<HashMap<Scope, T>>, from: &str, to: &str, keep: bool) {
            let mut m = map.lock().unwrap();
            if let Some(v) = m.remove(from) {
                if keep {
                    m.insert(to.to_string(), v);
                }
            }
        }
        mv(&self.routes, from, to, keep);
        mv(&self.credentials, from, to, keep);
        mv(&self.policy, from, to, keep);
        {
            let mut frames = self.frame_scopes.lock().unwrap();
            if keep {
                for s in frames.values_mut().filter(|s| *s == from) {
                    *s = to.to_string();
                }
            } else {
                frames.retain(|_, s| s != from);
            }
        }
        // A session's downloads stay its own across `reset`; `reset --all`
        // forgets them (the files stay where they are).
        let mut downloads = self.downloads.lock().unwrap();
        // The new context has its directory already (set before the move).
        downloads.dirs.remove(from);
        if keep {
            for d in downloads.items.iter_mut().filter(|d| d.scope == from) {
                d.scope = to.to_string();
            }
        } else {
            downloads.items.retain(|d| d.scope != from);
        }
        drop(downloads);
        // Dialogs not reported yet still are, to the same session; after
        // `reset --all` there are none, as in a new session.
        let mut dialogs = self.dialogs.lock().unwrap();
        if keep {
            for (s, _) in dialogs.iter_mut().filter(|(s, _)| s == from) {
                *s = to.to_string();
            }
        } else {
            dialogs.retain(|(s, _)| s != from);
        }
        drop(dialogs);
        // The old tabs' sessions follow along only when what they held does:
        // after `reset --all`, an event of an old tab still on its way (a
        // dialog answered just before) belongs to no one, not to the new
        // session.
        if keep {
            let mut scopes = self.scopes.lock().unwrap();
            for s in scopes.values_mut().filter(|s| *s == from) {
                *s = to.to_string();
            }
        }
    }

    /// A session closed (a fork): what its scope held goes with it.
    pub fn drop_scope(&self, scope: &str) {
        self.routes.lock().unwrap().remove(scope);
        self.credentials.lock().unwrap().remove(scope);
        self.policy.lock().unwrap().remove(scope);
        self.dialogs.lock().unwrap().retain(|(s, _)| s != scope);
        {
            let mut downloads = self.downloads.lock().unwrap();
            downloads.items.retain(|d| d.scope != scope);
            downloads.dirs.remove(scope);
        }
        self.frame_scopes.lock().unwrap().retain(|_, s| s != scope);
        self.scopes.lock().unwrap().retain(|_, s| s != scope);
    }

    pub fn unregister(&self, session_id: &str) {
        self.sessions.lock().unwrap().remove(session_id);
        self.scopes.lock().unwrap().remove(session_id);
        self.nav.lock().unwrap().remove(session_id);
        self.net.lock().unwrap().remove(session_id);
    }

    /// (fetch/XHR requests in flight, when the last one started or ended).
    /// Requests open longer than [`LONG_REQUEST`] are long polls or streams,
    /// not data the page is about to render: they don't count.
    pub fn net_state(&self, session_id: &str) -> (usize, Option<std::time::Instant>) {
        self.net.lock().unwrap().get_mut(session_id).map(|n| (n.live_pending(), n.last)).unwrap_or((0, None))
    }

    /// Data requests in flight that started in the last 10 s, long ones
    /// included: whether a cover over the page may be a loading overlay
    /// about to go (a click waits for one; see `Page::click`).
    pub fn fetching(&self, session_id: &str) -> usize {
        let recent = |t: &std::time::Instant| t.elapsed() < Duration::from_secs(10);
        self.net
            .lock()
            .unwrap()
            .get(session_id)
            .map(|n| n.pending.values().filter(|p| recent(&p.0)).count())
            .unwrap_or(0)
    }

    /// (Requests started, requests in flight) on this tab, for the
    /// snapshot skip check: the first is the sequence a `wait` records.
    pub fn net_seq_pending(&self, session_id: &str) -> (u64, usize) {
        self.net.lock().unwrap().get_mut(session_id).map(|n| (n.seq, n.live_pending())).unwrap_or((0, 0))
    }

    /// An action is about to run on this tab (see [`NetState::action`]).
    pub fn mark_action(&self, session_id: &str, nav_generation: u64) {
        let mut net = self.net.lock().unwrap();
        let state = net.entry(session_id.to_string()).or_default();
        state.action = Some((state.seq, nav_generation));
    }

    /// Has the last action's own traffic started (a request since it, no
    /// navigation since)? Then the snapshot after it waits for those
    /// requests and a short settle, not the whole quiet window, which is
    /// there for traffic that hasn't started yet (a debounced search, a
    /// page's data fetch after it loads).
    pub fn action_traffic_seen(&self, session_id: &str, nav_generation: u64) -> bool {
        let net = self.net.lock().unwrap();
        net.get(session_id).and_then(|n| n.action.map(|a| (a, n.seq))).is_some_and(
            |((seq_then, generation_then), seq_now)| seq_now > seq_then && generation_then == nav_generation,
        )
    }

    /// The request sequence a `wait` last recorded on this tab, if any.
    pub fn synced_seq(&self, session_id: &str) -> Option<u64> {
        self.net.lock().unwrap().get(session_id).and_then(|n| n.synced_seq)
    }

    /// A `wait` just succeeded on this tab: remember how many requests had
    /// started, so the next snapshot can skip its quiet wait while no new
    /// request starts.
    pub fn mark_network_synced(&self, session_id: &str) {
        if let Some(n) = self.net.lock().unwrap().get_mut(session_id) {
            n.synced = Some(std::time::Instant::now());
            n.synced_seq = Some(n.seq);
        }
    }

    /// Seed a freshly attached tab's navigation state: its load may have
    /// started before we attached (a popup), so no start event will come.
    /// Events that arrive later update it as usual.
    pub fn seed_nav(&self, session_id: &str, ready_state: &str, pending_url: bool) {
        let mut nav = self.nav.lock().unwrap();
        let state = nav.entry(session_id.to_string()).or_default();
        if state.generation > 0 {
            return; // events already arrived after attach: they're newer
        }
        match (pending_url, ready_state) {
            // Still on the initial about:blank, the real URL not committed.
            (true, _) => {
                state.loading = true;
                state.dom_ready = false;
                state.committed_at = None;
            }
            (false, "loading") => {
                state.loading = true;
                state.dom_ready = false;
                state.committed_at = Some(std::time::Instant::now());
            }
            _ => state.dom_ready = true,
        }
        state.generation = 1;
    }

    pub fn nav_state(&self, session_id: &str) -> NavState {
        self.nav.lock().unwrap().get(session_id).cloned().unwrap_or_default()
    }

    /// What a page did that its own scripts can't watch from the inside,
    /// as counters: navigations, dialogs, tabs and popups it opened,
    /// downloads. A click compares them before and after (requests are
    /// judged apart: see `Page::click_effect`).
    pub fn activity(&self, session_id: &str) -> [u64; 5] {
        let scope = self.scope_of(session_id);
        [
            self.nav_state(session_id).generation,
            self.dialogs.lock().unwrap().iter().filter(|(s, _)| *s == scope).count() as u64,
            self.opened.lock().unwrap().len() as u64,
            self.popups.lock().unwrap().len() as u64,
            self.downloads.lock().unwrap().items.iter().filter(|d| d.scope == scope).count() as u64,
        ]
    }

    /// Mark a navigation we started ourselves as in flight, so ops issued
    /// right after a `goto --wait commit` don't race it.
    pub fn mark_loading(&self, session_id: &str) {
        let mut nav = self.nav.lock().unwrap();
        let state = nav.entry(session_id.to_string()).or_default();
        state.loading = true;
        state.dom_ready = false;
    }

    /// The dialogs answered in `scope` since the last call; other scopes'
    /// stay for their sessions.
    pub fn take_dialogs(&self, scope: &str) -> Vec<Value> {
        let mut all = self.dialogs.lock().unwrap();
        let (taken, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut *all).into_iter().partition(|(s, _)| s == scope);
        *all = kept;
        taken.into_iter().map(|(_, d)| d).collect()
    }

    pub fn take_opened(&self) -> Vec<(String, String)> {
        std::mem::take(&mut *self.opened.lock().unwrap())
    }

    /// The opened tabs whose opener `mine` claims; the others stay for the
    /// session they belong to.
    pub fn take_opened_by(&self, mine: impl Fn(&str) -> bool) -> Vec<(String, String)> {
        let mut all = self.opened.lock().unwrap();
        let (taken, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut *all).into_iter().partition(|(_, o)| mine(o));
        *all = kept;
        taken
    }

    pub fn take_destroyed(&self) -> Vec<String> {
        std::mem::take(&mut *self.destroyed.lock().unwrap())
    }

    /// The closed tabs `mine` claims; the others stay.
    pub fn take_destroyed_by(&self, mine: impl Fn(&str) -> bool) -> Vec<String> {
        let mut all = self.destroyed.lock().unwrap();
        let (taken, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut *all).into_iter().partition(|t| mine(t));
        *all = kept;
        taken
    }

    /// URLs of the popups opened since the last call.
    pub fn take_popups(&self) -> Vec<String> {
        std::mem::take(&mut *self.popups.lock().unwrap()).into_iter().map(|(_, url)| url).collect()
    }

    /// Wait (at most `max`) until the watcher has handled every event that
    /// arrived before now. An engine that reports a dialog or a popup just
    /// before its command's response (Lightpanda) would otherwise have the
    /// op return before the watcher logged it.
    pub fn caught_up(&self, client: &CdpClient, max: Duration) -> bool {
        let target = client.events_sent();
        let deadline = std::time::Instant::now() + max;
        while self.processed.load(Ordering::SeqCst) < target {
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_micros(200));
        }
        true
    }

    pub fn is_single_target(&self) -> bool {
        self.single_target.load(Ordering::Relaxed)
    }
}

/// Arm the session's dialog policy on Lightpanda, where a dialog takes the
/// response armed before it opens. One armed response covers one dialog.
pub async fn arm_dialog(client: &CdpClient, shared: &Shared, session: &str) {
    if !shared.is_single_target() {
        return; // Chrome waits for the answer: the watcher sends it then.
    }
    let policy = shared.policy_for(&shared.scope_of(session));
    let mut params = json!({ "accept": policy.accept });
    if let Some(text) = &policy.prompt_text {
        params["promptText"] = json!(text);
    }
    let armed = client.send("LP.handleJavaScriptDialog", params, Some(session), Duration::from_secs(10)).await;
    let mut map = shared.armed.lock().unwrap();
    match armed {
        Ok(_) => map.insert(session.to_string(), policy),
        // An engine without the LP domain dismisses every dialog.
        Err(_) => map.remove(session),
    };
}

/// Lightpanda opens `target=_blank` / `window.open` pages as top-level
/// frames inside the opener's session, not as targets. Such a frame starts
/// navigating (before the response of the click that opened it) without
/// ever being attached as a child frame, which is what tells it apart from
/// an iframe.
fn note_popup(shared: &Shared, method: &str, params: &Value, session: &str) {
    let frame = || params.get("frameId").or_else(|| params.pointer("/frame/id")).and_then(Value::as_str);
    match method {
        "Page.frameAttached" => {
            if let Some(id) = frame() {
                shared.child_frames.lock().unwrap().insert(id.to_string());
            }
        }
        "Page.frameStartedNavigating" | "Page.frameNavigated" => {
            let (Some(id), Some(url)) =
                (frame(), params.get("url").or_else(|| params.pointer("/frame/url")).and_then(Value::as_str))
            else {
                return;
            };
            let main = shared.sessions.lock().unwrap().get(session).cloned();
            if main.as_deref() == Some(id)
                || params.pointer("/frame/parentId").is_some()
                || shared.child_frames.lock().unwrap().contains(id)
            {
                return;
            }
            let mut popups = shared.popups.lock().unwrap();
            match popups.iter_mut().find(|(f, _)| f == id) {
                Some(entry) => entry.1 = url.to_string(),
                None => popups.push((id.to_string(), url.to_string())),
            }
        }
        _ => {}
    }
}

/// Spawn the watcher on the engine runtime. It lives as long as the runtime.
pub fn spawn(handle: &tokio::runtime::Handle, client: &CdpClient, shared: Arc<Shared>) {
    let mut rx = client.subscribe();
    let client = client.clone();
    handle.spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    on_event(&client, &shared, &event);
                    shared.processed.fetch_add(1, Ordering::SeqCst);
                }
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    shared.processed.fetch_add(missed, Ordering::SeqCst);
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// A tab's fetch/XHR traffic, for the quiet wait (see [`NetState`]).
fn note_network(shared: &Shared, method: &str, params: &Value, session: &str) {
    match method {
        "Network.requestWillBeSent" => {
            let kind = params.get("type").and_then(Value::as_str).unwrap_or("");
            if session.is_empty() || !matches!(kind, "XHR" | "Fetch") {
                return;
            }
            if let Some(id) = params.get("requestId").and_then(Value::as_str) {
                let mut net = shared.net.lock().unwrap();
                let state = net.entry(session.to_string()).or_default();
                let now = std::time::Instant::now();
                let url = params.pointer("/request/url").and_then(Value::as_str).unwrap_or("");
                let key = url.split(['?', '#']).next().unwrap_or("").to_string();
                let method = params.pointer("/request/method").and_then(Value::as_str).unwrap_or("GET");
                let target = if key.is_empty() { String::new() } else { format!("{method} {key}") };
                let mut poll = state.background.contains(&target);
                if !key.is_empty() {
                    let starts = state.starts.entry(key.clone()).or_default();
                    poll |= is_poll(starts, now);
                    starts.push(now);
                    if starts.len() > 3 {
                        starts.remove(0);
                    }
                }
                if !poll {
                    state.seq += 1;
                    let before = state.last;
                    state.pending.entry(id.to_string()).or_insert((now, target.clone(), before));
                    state.touch(&target);
                }
            }
        }
        // No content back (202, 204): a beacon the page doesn't read, so
        // that URL's requests are background traffic.
        "Network.responseReceived" => {
            let status = params.pointer("/response/status").and_then(Value::as_u64);
            if let (Some(202 | 204), Some(id)) = (status, params.get("requestId").and_then(Value::as_str)) {
                let mut net = shared.net.lock().unwrap();
                if let Some(state) = net.get_mut(session) {
                    if let Some((_, key, _)) = state.pending.remove(id) {
                        if !key.is_empty() {
                            // It never was data, nor are the same URL's
                            // requests still out: none of them moves the
                            // quiet clock, however the events interleaved.
                            state.pending.retain(|_, (_, k, _)| *k != key);
                            state.background.insert(key);
                        }
                        state.recompute_last();
                    }
                }
            }
        }
        "Network.loadingFinished" | "Network.loadingFailed" => {
            if let Some(id) = params.get("requestId").and_then(Value::as_str) {
                let mut net = shared.net.lock().unwrap();
                if let Some(state) = net.get_mut(session) {
                    if let Some((started, key, _)) = state.pending.remove(id) {
                        if started.elapsed() >= LONG_REQUEST && !key.is_empty() {
                            state.background.insert(key.clone());
                        }
                        // A long poll's end is background too.
                        if state.background.contains(&key) {
                            state.recompute_last();
                        } else {
                            state.touch(&key);
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

fn on_event(client: &CdpClient, shared: &Arc<Shared>, event: &Value) {
    let Some(method) = event.get("method").and_then(Value::as_str) else {
        return;
    };
    let params = event.get("params").cloned().unwrap_or(Value::Null);
    let session = event.get("sessionId").and_then(Value::as_str).unwrap_or("");
    if shared.is_single_target() {
        note_popup(shared, method, &params, session);
    }
    shared.logs.lock().unwrap().on_event(method, &params, session);
    // Subframes, with the scope of the tab they are in.
    match method {
        "Page.frameAttached" => {
            if let Some(id) = params.get("frameId").and_then(Value::as_str) {
                let scope = shared.scope_of(session);
                shared.frame_scopes.lock().unwrap().insert(id.to_string(), scope);
            }
        }
        "Page.frameDetached" => {
            if let Some(id) = params.get("frameId").and_then(Value::as_str) {
                shared.frame_scopes.lock().unwrap().remove(id);
            }
        }
        _ => {}
    }
    if matches!(method, "Browser.downloadWillBegin" | "Browser.downloadProgress") {
        // Started by a tab's main frame (its frame id is the tab's target
        // id) or one of its subframes: that tab's scope.
        let frame = params.get("frameId").and_then(Value::as_str).unwrap_or("");
        let tab = shared.sessions.lock().unwrap().iter().find(|(_, t)| *t == frame).map(|(s, _)| s.clone());
        let scope = match tab {
            Some(s) => shared.scope_of(&s),
            None => shared.frame_scopes.lock().unwrap().get(frame).cloned().unwrap_or_default(),
        };
        shared.downloads.lock().unwrap().on_event(method, &params, &scope);
    }
    match method {
        "Page.javascriptDialogOpening" if shared.is_single_target() => {
            // Already answered by the page's engine: report what it got (the
            // armed response, else a dismissal) and arm the next one.
            let kind = params.get("type").and_then(Value::as_str).unwrap_or("alert");
            let armed = shared.armed.lock().unwrap().remove(session);
            shared.dialogs.lock().unwrap().push((
                shared.scope_of(session),
                json!({
                    "type": kind,
                    "message": params.get("message").and_then(Value::as_str).unwrap_or(""),
                    "accepted": armed.as_ref().is_some_and(|p| p.accept),
                }),
            ));
            if !session.is_empty() {
                let client = client.clone();
                let session = session.to_string();
                let shared = Arc::clone(shared);
                tokio::spawn(async move { arm_dialog(&client, &shared, &session).await });
            }
        }
        "Page.javascriptDialogOpening" => {
            let scope = shared.scope_of(session);
            let policy = shared.policy_for(&scope);
            let kind = params.get("type").and_then(Value::as_str).unwrap_or("alert");
            // `beforeunload` is always accepted: refusing it would cancel the
            // navigation the agent just asked for.
            let accept = policy.accept || kind == "beforeunload";
            let mut answer = json!({ "accept": accept });
            if accept && kind == "prompt" {
                // Without `dialog --prompt-text`, accept with the page's own
                // default value, as pressing OK would.
                let text = policy
                    .prompt_text
                    .clone()
                    .unwrap_or_else(|| params.get("defaultPrompt").and_then(Value::as_str).unwrap_or("").to_string());
                answer["promptText"] = json!(text);
            }
            shared.dialogs.lock().unwrap().push((
                scope,
                json!({
                    "type": kind,
                    "message": params.get("message").and_then(Value::as_str).unwrap_or(""),
                    "accepted": accept,
                }),
            ));
            let client = client.clone();
            let session = session.to_string();
            tokio::spawn(async move {
                let _ = client
                    .send(
                        "Page.handleJavaScriptDialog",
                        answer,
                        Some(&session).filter(|s| !s.is_empty()).map(|s| s.as_str()),
                        Duration::from_secs(10),
                    )
                    .await;
            });
        }
        "Fetch.requestPaused" => {
            let id = params.get("requestId").and_then(Value::as_str).unwrap_or("").to_string();
            let url = params.pointer("/request/url").and_then(Value::as_str).unwrap_or("");
            let (method, answer) = route_answer(shared, session, &id, url);
            let client = client.clone();
            let session = session.to_string();
            tokio::spawn(async move {
                let s = Some(session.as_str()).filter(|s| !s.is_empty());
                if client.send(method, answer, s, Duration::from_secs(10)).await.is_err()
                    && method != "Fetch.continueRequest"
                {
                    // A rule the engine can't apply must not hang the request.
                    let _ = client
                        .send("Fetch.continueRequest", json!({ "requestId": id }), s, Duration::from_secs(10))
                        .await;
                }
            });
        }
        "Fetch.authRequired" => {
            let id = params.get("requestId").and_then(Value::as_str).unwrap_or("").to_string();
            let response = match shared.credentials.lock().unwrap().get(&shared.scope_of(session)).cloned() {
                Some((user, pass)) => json!({ "response": "ProvideCredentials", "username": user, "password": pass }),
                None => json!({ "response": "Default" }),
            };
            let client = client.clone();
            let session = session.to_string();
            tokio::spawn(async move {
                let s = Some(session.as_str()).filter(|s| !s.is_empty());
                let _ = client
                    .send(
                        "Fetch.continueWithAuth",
                        json!({ "requestId": id, "authChallengeResponse": response }),
                        s,
                        Duration::from_secs(10),
                    )
                    .await;
            });
        }
        "Network.requestWillBeSent"
        | "Network.responseReceived"
        | "Network.loadingFinished"
        | "Network.loadingFailed" => {
            note_network(shared, method, &params, session);
        }
        "Target.targetCreated" => {
            let info = params.get("targetInfo").cloned().unwrap_or(Value::Null);
            let is_page = info.get("type").and_then(Value::as_str) == Some("page");
            let opener = info.get("openerId").and_then(Value::as_str).unwrap_or("");
            let url = info.get("url").and_then(Value::as_str).unwrap_or("");
            let window =
                shared.adopt_windows.load(std::sync::atomic::Ordering::Relaxed) && !url.starts_with("devtools://");
            if is_page && (!opener.is_empty() || window) {
                if let Some(id) = info.get("targetId").and_then(Value::as_str) {
                    shared.opened.lock().unwrap().push((id.to_string(), opener.to_string()));
                }
            }
        }
        "Target.targetDestroyed" => {
            if let Some(id) = params.get("targetId").and_then(Value::as_str) {
                shared.destroyed.lock().unwrap().push(id.to_string());
            }
        }
        "Page.frameStartedLoading"
        | "Page.frameStoppedLoading"
        | "Page.domContentEventFired"
        | "Page.frameRequestedNavigation"
        | "Page.navigatedWithinDocument"
        | "Page.frameNavigated" => {
            if session.is_empty() {
                return;
            }
            let main_frame = shared.sessions.lock().unwrap().get(session).cloned();
            let Some(main_frame) = main_frame else { return };
            // domContentEventFired is page-level (main frame) and has no
            // frameId; frameNavigated carries the frame object.
            let frame =
                params.get("frameId").or_else(|| params.get("frame").and_then(|f| f.get("id"))).and_then(Value::as_str);
            if let Some(frame) = frame {
                if frame != main_frame {
                    return;
                }
            }
            // A link opening a new tab is "requested" by this frame but
            // navigates the other tab.
            if method == "Page.frameRequestedNavigation"
                && params.get("disposition").and_then(Value::as_str).is_some_and(|d| d != "currentTab")
            {
                return;
            }
            let mut nav = shared.nav.lock().unwrap();
            let state = nav.entry(session.to_string()).or_default();
            match method {
                "Page.frameRequestedNavigation" => {
                    state.requested_at = Some(std::time::Instant::now());
                    state.generation += 1;
                }
                "Page.frameStartedLoading" => {
                    // The old document's requests die with it.
                    if let Some(net) = shared.net.lock().unwrap().get_mut(session) {
                        net.pending.clear();
                        // A new document's requests are not the old one's polls,
                        // and a wait on the old document does not sync the new one.
                        net.starts.clear();
                        net.synced = None;
                        net.synced_seq = None;
                    }
                    state.loading = true;
                    state.dom_ready = false;
                    state.requested_at = None;
                    state.committed_at = None;
                    state.generation += 1;
                }
                "Page.frameNavigated" => state.committed_at = Some(std::time::Instant::now()),
                "Page.frameStoppedLoading" => {
                    state.loading = false;
                    state.dom_ready = true;
                    state.requested_at = None;
                }
                "Page.navigatedWithinDocument" => state.requested_at = None,
                _ => state.dom_ready = true,
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(base: std::time::Instant, ms: u64) -> std::time::Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn a_steady_repeat_is_a_poll_and_the_first_two_are_not() {
        let t0 = std::time::Instant::now();
        assert!(!is_poll(&[], at(t0, 300)));
        assert!(!is_poll(&[t0], at(t0, 300)));
        // Third request, 300 ms after a second that came 300 ms after the first.
        assert!(is_poll(&[t0, at(t0, 300)], at(t0, 600)));
        assert!(is_poll(&[t0, at(t0, 300)], at(t0, 750)));
        // A burst after a steady run, or a long silence, is not.
        assert!(!is_poll(&[t0, at(t0, 300)], at(t0, 310)));
        assert!(!is_poll(&[t0, at(t0, 300)], at(t0, 3000)));
        // Too slow to be a poll worth ignoring.
        assert!(!is_poll(&[t0, at(t0, 20_000)], at(t0, 40_000)));
        // A burst to one path is loading: too quick to be a poll.
        assert!(!is_poll(&[t0, at(t0, 1)], at(t0, 2)));
        assert!(!is_poll(&[t0, at(t0, 120)], at(t0, 240)));
    }

    #[test]
    fn a_snapshot_skips_the_quiet_wait_only_after_a_wait_with_no_new_request() {
        // No wait yet: never skip.
        assert!(!skip_quiet_wait(None, 0, 0));
        // Wait succeeded, no new request, none in flight: skip.
        assert!(skip_quiet_wait(Some(3), 3, 0));
        // A request started after the wait: do not skip, even once it lands.
        assert!(!skip_quiet_wait(Some(3), 4, 0));
        // A request still in flight: do not skip.
        assert!(!skip_quiet_wait(Some(3), 3, 1));
    }

    /// Beacons whose 204s come back after the next beacon started leave
    /// the quiet clock where data left it, in any order of events.
    #[test]
    fn interleaved_beacons_leave_the_quiet_clock_alone() {
        let shared = Shared::default();
        let ev = |method: &str, params: Value| note_network(&shared, method, &params, "s");
        let sent = |id: &str, method: &str, url: &str| json!({ "requestId": id, "type": "Fetch", "request": { "url": url, "method": method } });
        // Data first: it sets the clock.
        ev("Network.requestWillBeSent", sent("d", "GET", "http://x.test/api"));
        ev("Network.loadingFinished", json!({ "requestId": "d" }));
        let (_, after_data) = shared.net_state("s");
        assert!(after_data.is_some());
        // Two beacons out before either answers.
        ev("Network.requestWillBeSent", sent("b1", "POST", "http://x.test/collect"));
        ev("Network.requestWillBeSent", sent("b2", "POST", "http://x.test/collect?e=2"));
        assert_eq!(shared.net_state("s").0, 2);
        ev("Network.responseReceived", json!({ "requestId": "b1", "response": { "status": 204 } }));
        // The second, same URL, no longer counts either.
        let (pending, last) = shared.net_state("s");
        assert_eq!((pending, last), (0, after_data), "the clock is back where the data left it");
        ev("Network.loadingFinished", json!({ "requestId": "b1" }));
        ev("Network.responseReceived", json!({ "requestId": "b2", "response": { "status": 204 } }));
        ev("Network.loadingFinished", json!({ "requestId": "b2" }));
        assert_eq!(shared.net_state("s"), (0, after_data));
        // A later beacon to that URL is background from the start.
        ev("Network.requestWillBeSent", sent("b3", "POST", "http://x.test/collect"));
        assert_eq!(shared.net_state("s"), (0, after_data));
    }

    #[test]
    fn globs() {
        assert!(glob_match("*", "http://a/b"));
        assert!(glob_match("*/api/*", "http://h:1/api/cart?x=1"));
        assert!(glob_match("*.png", "http://h/i/logo.png"));
        assert!(!glob_match("*.png", "http://h/i/logo.png?v=2"));
        assert!(glob_match("http://h/?", "http://h/a"));
        assert!(!glob_match("http://h/?", "http://h/ab"));
        assert!(glob_match("*doubleclick*", "https://ad.doubleclick.net/x"));
    }

    #[test]
    fn first_matching_route_answers() {
        let shared = Shared::default();
        shared.register("s1", "t1", "");
        shared.routes.lock().unwrap().entry(String::new()).or_default().extend([
            Route { pattern: "*.png".into(), action: RouteAction::Abort, hits: 0 },
            Route {
                pattern: "*/api/*".into(),
                action: RouteAction::Fulfill {
                    status: 200,
                    headers: vec![("content-type".into(), "application/json".into())],
                    body_base64: "e30=".into(),
                },
                hits: 0,
            },
        ]);
        assert_eq!(route_answer(&shared, "s1", "1", "http://h/a.png").0, "Fetch.failRequest");
        let (m, p) = route_answer(&shared, "s1", "2", "http://h/api/x");
        assert_eq!((m, p["responseCode"].as_u64()), ("Fetch.fulfillRequest", Some(200)));
        assert_eq!(route_answer(&shared, "s1", "3", "http://h/").0, "Fetch.continueRequest");
        assert_eq!(shared.routes.lock().unwrap()[""][0].hits, 1);
    }

    #[test]
    fn scopes_keep_their_routes_and_dialogs_apart() {
        let shared = Shared::default();
        shared.register("a", "ta", "ctx-a");
        shared.register("b", "tb", "ctx-b");
        shared
            .routes
            .lock()
            .unwrap()
            .insert("ctx-a".into(), vec![Route { pattern: "*".into(), action: RouteAction::Abort, hits: 0 }]);
        assert_eq!(route_answer(&shared, "a", "1", "http://h/").0, "Fetch.failRequest");
        assert_eq!(route_answer(&shared, "b", "2", "http://h/").0, "Fetch.continueRequest");
        shared.policy.lock().unwrap().insert("ctx-b".into(), DialogPolicy { accept: false, prompt_text: None });
        assert!(shared.policy_for("ctx-a").accept);
        assert!(!shared.policy_for("ctx-b").accept);
        shared.dialogs.lock().unwrap().push(("ctx-a".into(), json!({"type": "alert"})));
        assert!(shared.take_dialogs("ctx-b").is_empty());
        // `reset` moves a scope along; `reset --all` drops what it held.
        shared.move_scope("ctx-a", "ctx-a2", true);
        assert_eq!(shared.take_dialogs("ctx-a2").len(), 1);
        assert!(shared.routes.lock().unwrap().contains_key("ctx-a2"));
        shared.move_scope("ctx-b", "ctx-b2", false);
        assert!(shared.policy_for("ctx-b2").accept);
    }
}
