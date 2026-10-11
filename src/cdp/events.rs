//! Background CDP event watcher: JavaScript dialogs, pages opened by pages
//! (popups / `target=_blank`), and per-tab navigation state.
//!
//! Why a background task: a page that calls `alert()`/`confirm()` blocks
//! its renderer until the dialog is answered, so the CDP command that
//! triggered it (a click, an eval) never returns. The watcher answers the
//! dialog the moment Chrome announces it, per the session's policy, and
//! logs it so the next response can tell the agent what happened.
//!
//! Navigation state lets ops wait for a navigation that an earlier action
//! started (a link click, a JS redirect) instead of reading the old page:
//! Chrome reports `Page.frameStartedLoading` for a link click *before* the
//! click's own `Input.dispatchMouseEvent` response, and a JS-scheduled
//! navigation ~15 ms after it. Tracking events costs nothing when nothing
//! navigates (no fixed grace sleep per click).

use std::collections::HashMap;
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

/// A fetch/XHR open this long is a long poll or a stream.
const LONG_REQUEST: Duration = Duration::from_secs(5);

/// A tab's in-flight `fetch`/XHR requests (`Network` events), so a read
/// right after an action can wait for the data it triggered to arrive.
#[derive(Clone, Debug, Default)]
pub struct NetState {
    /// request id -> when it started.
    pub pending: HashMap<String, std::time::Instant>,
    /// When the last fetch/XHR started or ended.
    pub last: Option<std::time::Instant>,
}

#[derive(Default)]
pub struct Shared {
    pub policy: Mutex<DialogPolicy>,
    /// Dialogs answered since the last drain: `{type, message, accepted}`.
    pub dialogs: Mutex<Vec<Value>>,
    /// Page targets opened by a page (they carry an `openerId`), in order:
    /// (target, opener).
    pub opened: Mutex<Vec<(String, String)>>,
    /// Page targets that went away (e.g. `window.close()`).
    pub destroyed: Mutex<Vec<String>>,
    /// session id -> target id, registered when a tab is attached. A page
    /// target's main frame id equals its target id.
    pub sessions: Mutex<HashMap<String, String>>,
    pub nav: Mutex<HashMap<String, NavState>>,
    pub net: Mutex<HashMap<String, NetState>>,
}

impl Shared {
    pub fn register(&self, session_id: &str, target_id: &str) {
        self.sessions.lock().unwrap().insert(session_id.to_string(), target_id.to_string());
    }

    pub fn unregister(&self, session_id: &str) {
        self.sessions.lock().unwrap().remove(session_id);
        self.nav.lock().unwrap().remove(session_id);
        self.net.lock().unwrap().remove(session_id);
    }

    /// (fetch/XHR requests in flight, when the last one started or ended).
    /// Requests open longer than [`LONG_REQUEST`] are long polls or streams,
    /// not data the page is about to render: they don't count.
    pub fn net_state(&self, session_id: &str) -> (usize, Option<std::time::Instant>) {
        self.net
            .lock()
            .unwrap()
            .get(session_id)
            .map(|n| (n.pending.values().filter(|t| t.elapsed() < LONG_REQUEST).count(), n.last))
            .unwrap_or((0, None))
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

    /// Mark a navigation we started ourselves as in flight, so ops issued
    /// right after a `goto --wait commit` don't race it.
    pub fn mark_loading(&self, session_id: &str) {
        let mut nav = self.nav.lock().unwrap();
        let state = nav.entry(session_id.to_string()).or_default();
        state.loading = true;
        state.dom_ready = false;
    }

    pub fn take_dialogs(&self) -> Vec<Value> {
        std::mem::take(&mut *self.dialogs.lock().unwrap())
    }

    pub fn take_opened(&self) -> Vec<(String, String)> {
        std::mem::take(&mut *self.opened.lock().unwrap())
    }

    pub fn take_destroyed(&self) -> Vec<String> {
        std::mem::take(&mut *self.destroyed.lock().unwrap())
    }
}

/// Spawn the watcher on the engine runtime. It lives as long as the runtime.
pub fn spawn(handle: &tokio::runtime::Handle, client: &CdpClient, shared: Arc<Shared>) {
    let mut rx = client.subscribe();
    let client = client.clone();
    handle.spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => on_event(&client, &shared, &event),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

fn on_event(client: &CdpClient, shared: &Shared, event: &Value) {
    let Some(method) = event.get("method").and_then(Value::as_str) else {
        return;
    };
    let params = event.get("params").cloned().unwrap_or(Value::Null);
    let session = event.get("sessionId").and_then(Value::as_str).unwrap_or("");
    match method {
        "Page.javascriptDialogOpening" => {
            let policy = shared.policy.lock().unwrap().clone();
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
            shared.dialogs.lock().unwrap().push(json!({
                "type": kind,
                "message": params.get("message").and_then(Value::as_str).unwrap_or(""),
                "accepted": accept,
            }));
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
        "Network.requestWillBeSent" => {
            let kind = params.get("type").and_then(Value::as_str).unwrap_or("");
            if session.is_empty() || !matches!(kind, "XHR" | "Fetch") {
                return;
            }
            if let Some(id) = params.get("requestId").and_then(Value::as_str) {
                let mut net = shared.net.lock().unwrap();
                let state = net.entry(session.to_string()).or_default();
                let now = std::time::Instant::now();
                state.pending.entry(id.to_string()).or_insert(now);
                state.last = Some(now);
            }
        }
        "Network.loadingFinished" | "Network.loadingFailed" => {
            if let Some(id) = params.get("requestId").and_then(Value::as_str) {
                let mut net = shared.net.lock().unwrap();
                if let Some(state) = net.get_mut(session) {
                    if state.pending.remove(id).is_some() {
                        state.last = Some(std::time::Instant::now());
                    }
                }
            }
        }
        "Target.targetCreated" => {
            let info = params.get("targetInfo").cloned().unwrap_or(Value::Null);
            let is_page = info.get("type").and_then(Value::as_str) == Some("page");
            let opener = info.get("openerId").and_then(Value::as_str).unwrap_or("");
            if is_page && !opener.is_empty() {
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
