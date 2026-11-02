//! Minimal async CDP JSON-RPC client over a single websocket.
//!
//! One background task owns both halves of the socket: it forwards queued
//! outgoing messages and dispatches incoming ones — `{id, result|error}` to
//! the pending caller, everything else to the event broadcast. `send` is a
//! plain request/response with a timeout; `subscribe` + `wait_event` cover
//! the few events we need (page load).

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::{broadcast, mpsc, oneshot};

type Responder = oneshot::Sender<Result<Value, Refusal>>;

/// What Chrome answers a command sent to a tab that is gone, and what a
/// command still waiting when its tab's session ends fails with.
pub const SESSION_GONE: &str = "Session with given id not found";

/// Start of the error for a command the browser did not answer in time.
pub const NO_ANSWER: &str = "the browser did not answer";

/// Start of the error for a connection to the browser that has ended.
pub const CLOSED: &str = "the connection to the browser closed";

/// A request the shared browser's host never handed to the browser (its
/// writer was already gone): the request positively did not run.
pub const NEVER_REACHED: &str = "the request never reached the browser";

/// The wire code the shared host gives that refusal: with the exact
/// message it classifies a refusal coming over the socket, where a Rust
/// type cannot cross. A browser or page cannot author it.
pub const NEVER_REACHED_CODE: i64 = -32099;

/// The hook a failed write hands its frame and the unsent queue to (the
/// shared-browser host's router classifies them as never-reached).
type WriteFailed = Arc<Mutex<Option<Box<dyn Fn(String, Vec<String>) + Send + 'static>>>>;

/// This request positively never reached the browser: it did not run, so
/// running it now is not a replay. Carried as a type, never matched by
/// text: a browser- or page-controlled message cannot impersonate it.
#[derive(Debug)]
pub struct NeverReached(pub String);

impl std::fmt::Display for NeverReached {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{NEVER_REACHED} ({})", self.0)
    }
}

impl std::error::Error for NeverReached {}

/// Why a request has no answer.
enum Refusal {
    /// The connection ended with the request maybe written: unknown
    /// whether the browser ran it. Never retried.
    Closed,
    /// The request positively never went out (see [`NeverReached`]).
    NeverReached,
    /// The browser (or the shared host) refused it, with its message.
    Other(String),
}

/// A command waiting for its answer, and the tab session it went to.
type Pending = HashMap<u64, (Option<String>, Responder)>;

/// What every incoming message is handed to, as its JSON text, once
/// [`CdpClient::take_raw`] diverted them (the shared-browser host routes
/// them right there, on the task that read them).
pub type RawHandler = Box<dyn FnMut(&str) + Send>;
type RawTap = Arc<Mutex<Option<RawHandler>>>;

pub struct CdpClient {
    inner: Arc<Inner>,
}

struct Inner {
    next_id: AtomicU64,
    /// Shared with the pump task so responses dispatched from the read loop
    /// reach the `send` caller: every `send` inserts its responder here and the
    /// pump removes it in `dispatch`.
    pending: Arc<Mutex<Pending>>,
    /// Outgoing JSON texts for the pump task. `shutdown` takes it, which
    /// makes the pump's `rx.recv()` return `None` and the task exits,
    /// closing the websocket.
    writer: Arc<Mutex<Option<mpsc::UnboundedSender<String>>>>,
    events: broadcast::Sender<Value>,
    /// Events broadcast so far: a subscriber that has handled this many has
    /// seen everything that arrived before the current point.
    events_sent: Arc<AtomicU64>,
    /// Set once the transport is gone (browser exited, pipe/socket closed):
    /// `send` then fails at once instead of waiting out its timeout.
    closed: Arc<AtomicBool>,
    /// Requests whose effect on the browser cannot be certified absent:
    /// answered, maybe-written, or timed out. A typed never-reached
    /// request does not count - it positively did not run.
    effects_uncertain: Arc<AtomicU64>,
    raw: RawTap,
    on_close: OnClose,
    /// A raw (routed) frame whose write failed, plus everything queued
    /// behind it: NUL-framed, so a failed write's frame never completed.
    write_failed: WriteFailed,
}

/// Hooks run by the first `close_all` after the transport goes: watchers
/// that must react as soon as it does, without polling `is_closed`.
type OnClose = Arc<Mutex<Vec<Box<dyn Fn() + Send + 'static>>>>;

/// Mark the connection closed: `send` fails at once instead of waiting
/// out its timeout, raw readers see the channel end, and close watchers
/// run. Requests still waiting are answered by [`drain_closed`] - by the
/// task that owns the writes, after it has classified the frame it was
/// writing, so a write failure's proof is not raced away.
fn close_all(closed: &AtomicBool, raw: &RawTap, on_close: &OnClose) {
    closed.store(true, Ordering::SeqCst);
    for hook in std::mem::take(&mut *on_close.lock().unwrap()) {
        hook();
    }
    // A raw reader sees its channel end: the browser is gone.
    // The raw handler goes with the connection.
    raw.lock().unwrap().take();
}

/// Fail every request still waiting: written or not, it stays unknown
/// whether the browser ran it, so the answer is CLOSED, never retried.
fn drain_closed(pending: &Mutex<Pending>) {
    let mut pending = pending.lock().unwrap();
    for (_, (_, responder)) in pending.drain() {
        let _ = responder.send(Err(Refusal::Closed));
    }
}

/// The `"id"` of a framed outgoing message, for the writer task to say
/// which request a failed write concerned.
fn framed_id(bytes: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(bytes).ok()?;
    let at = text.find("\"id\":")? + 5;
    text[at..].trim_start().chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().ok()
}

impl Clone for CdpClient {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

impl CdpClient {
    /// Connect to the browser-level debugger URL and start the pump task.
    pub async fn connect(ws_url: &str) -> Result<Self> {
        // Bound the connect so a stale/unreachable DevTools URL fails in 15s
        // instead of hanging the CLI forever.
        Self::connect_within(ws_url, Duration::from_secs(15)).await
    }

    /// [`CdpClient::connect`] with its own bound: attaching to a user's
    /// Chrome waits while Chrome asks them to allow the connection.
    pub async fn connect_within(ws_url: &str, bound: Duration) -> Result<Self> {
        let (ws, _) = tokio::time::timeout(bound, tokio_tungstenite::connect_async(ws_url))
            .await
            .map_err(|_| anyhow::anyhow!("could not connect to the browser at {ws_url} within {} s", bound.as_secs()))?
            .with_context(|| format!("connect to the browser at {ws_url}"))?;
        let (mut sink, mut stream) = ws.split();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let (event_tx, _) = broadcast::channel::<Value>(512);
        let pending: Arc<Mutex<Pending>> = Arc::new(Mutex::new(HashMap::new()));
        let pending_pump = Arc::clone(&pending);
        let event_tx_pump = event_tx.clone();
        let events_sent = Arc::new(AtomicU64::new(0));
        let sent_pump = Arc::clone(&events_sent);
        let closed = Arc::new(AtomicBool::new(false));
        let closed_pump = Arc::clone(&closed);
        let raw: RawTap = Arc::default();
        let raw_pump = Arc::clone(&raw);
        let on_close: OnClose = Arc::default();
        let on_close_pump = Arc::clone(&on_close);
        let writer_slot = Arc::new(Mutex::new(Some(tx)));

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    outgoing = rx.recv() => {
                        match outgoing {
                            Some(text) => {
                                let msg = tokio_tungstenite::tungstenite::Message::Text(text.into());
                                if sink.send(msg).await.is_err() {
                                    break;
                                }
                            }
                            None => break, // client dropped: shut down
                        }
                    }
                    incoming = stream.next() => {
                        match incoming {
                            Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                                dispatch(&text, &pending_pump, &event_tx_pump, &sent_pump, &raw_pump);
                            }
                            Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) | None => break,
                            Some(Ok(_)) => {} // ping/pong/binary: ignore
                            Some(Err(_)) => break,
                        }
                    }
                }
            }
            // Connection lost: messages never attempted positively never
            // went out; everything else stays merely unanswered.
            close_all(&closed_pump, &raw_pump, &on_close_pump);
            while let Ok(unsent) = rx.try_recv() {
                if let Some(id) = framed_id(unsent.as_bytes()) {
                    if let Some((_, responder)) = pending_pump.lock().unwrap().remove(&id) {
                        let _ = responder.send(Err(Refusal::NeverReached));
                    }
                }
            }
            drain_closed(&pending_pump);
        });

        Ok(Self {
            inner: Arc::new(Inner {
                next_id: AtomicU64::new(1),
                pending,
                writer: writer_slot,
                events: event_tx,
                events_sent,
                closed,
                effects_uncertain: Arc::new(AtomicU64::new(0)),
                raw,
                on_close,
                write_failed: Arc::new(Mutex::new(None)),
            }),
        })
    }

    /// Connect over `--remote-debugging-pipe` fds: NUL-terminated JSON
    /// messages, one writer task and one reader task.
    #[cfg(unix)]
    pub async fn connect_pipe(from_browser: std::os::fd::OwnedFd, to_browser: std::os::fd::OwnedFd) -> Result<Self> {
        let reader =
            tokio::net::unix::pipe::Receiver::from_owned_fd(from_browser).context("CDP pipe (browser -> us)")?;
        let writer = tokio::net::unix::pipe::Sender::from_owned_fd(to_browser).context("CDP pipe (us -> browser)")?;
        Ok(Self::connect_framed(reader, writer))
    }

    /// Connect over a byte stream carrying NUL-terminated JSON messages, as
    /// Chrome's pipe does: the pipe itself, or a shared-browser host's
    /// socket (see `crate::shared`).
    pub fn connect_framed<R, W>(mut reader: R, mut writer: W) -> Self
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
        W: tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let (event_tx, _) = broadcast::channel::<Value>(512);
        let pending: Arc<Mutex<Pending>> = Arc::new(Mutex::new(HashMap::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let raw: RawTap = Arc::default();
        let on_close: OnClose = Arc::default();
        let on_close_w = Arc::clone(&on_close);
        let write_failed: WriteFailed = Arc::new(Mutex::new(None));

        let (closed_w, pending_w, raw_w) = (Arc::clone(&closed), Arc::clone(&pending), Arc::clone(&raw));
        let write_failed_w = Arc::clone(&write_failed);
        let writer_slot = Arc::new(Mutex::new(Some(tx)));
        let writer_end = Arc::clone(&writer_slot);

        tokio::spawn(async move {
            while let Some(text) = rx.recv().await {
                let mut bytes = text.into_bytes();
                bytes.push(0);
                if writer.write_all(&bytes).await.is_err() {
                    // The other end is gone (EPIPE). A failed write_all kept
                    // back at least the terminating NUL, so this one frame
                    // can never be dispatched: it positively never ran.
                    // Answer it apart from everything still merely
                    // unanswered, which may have been written.
                    let failed = String::from_utf8_lossy(&bytes[..bytes.len() - 1]).into_owned();
                    if let Some(id) = framed_id(&bytes) {
                        if let Some((_, responder)) = pending_w.lock().unwrap().remove(&id) {
                            let _ = responder.send(Err(Refusal::NeverReached));
                        }
                    }
                    // Routed (raw) traffic has no pending entry here: hand
                    // the failed frame and the unsent queue to the router.
                    let mut unsent = Vec::new();
                    while let Ok(rest) = rx.try_recv() {
                        // Never attempted a write: never sent, either.
                        if let Some(id) = framed_id(rest.as_bytes()) {
                            if let Some((_, responder)) = pending_w.lock().unwrap().remove(&id) {
                                let _ = responder.send(Err(Refusal::NeverReached));
                            }
                        }
                        unsent.push(rest);
                    }
                    if let Some(hook) = write_failed_w.lock().unwrap().take() {
                        hook(failed, unsent);
                    }
                    close_all(&closed_w, &raw_w, &on_close_w);
                    break;
                }
            }
            // Written or not, what is still waiting cannot be told apart:
            // answered CLOSED, never retried.
            drain_closed(&pending_w);
            // Dropping the writer closes the stream: Chrome exits on its
            // pipe closing, a host lets this client's context go.
            let _ = writer.shutdown().await;
        });
        let pending_pump = Arc::clone(&pending);
        let event_tx_pump = event_tx.clone();
        let events_sent = Arc::new(AtomicU64::new(0));
        let sent_pump = Arc::clone(&events_sent);
        let closed_pump = Arc::clone(&closed);
        let raw_pump = Arc::clone(&raw);
        let on_close_pump = Arc::clone(&on_close);
        tokio::spawn(async move {
            let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
            let mut chunk = vec![0u8; 256 * 1024];
            let mut scanned = 0;
            loop {
                match reader.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        while let Some(pos) = buf[scanned..].iter().position(|b| *b == 0) {
                            let end = scanned + pos;
                            if let Ok(text) = std::str::from_utf8(&buf[..end]) {
                                dispatch(text, &pending_pump, &event_tx_pump, &sent_pump, &raw_pump);
                            }
                            buf.drain(..=end);
                            scanned = 0;
                        }
                        scanned = buf.len();
                    }
                }
            }
            close_all(&closed_pump, &raw_pump, &on_close_pump);
            // Ending the writer task ends the connection: it classifies
            // the frame it was writing, then answers the rest CLOSED.
            writer_end.lock().unwrap().take();
        });
        Self {
            inner: Arc::new(Inner {
                next_id: AtomicU64::new(1),
                pending,
                writer: writer_slot,
                events: event_tx,
                events_sent,
                closed,
                effects_uncertain: Arc::new(AtomicU64::new(0)),
                raw,
                on_close,
                write_failed,
            }),
        }
    }

    /// A sender that writes JSON texts to the browser as they are.
    pub fn raw_writer(&self) -> Result<mpsc::UnboundedSender<String>> {
        self.inner.writer.lock().unwrap().clone().ok_or_else(|| anyhow::anyhow!("{CLOSED}"))
    }

    /// From now on every incoming message is handed, as its JSON text, to
    /// `handler`, on the task that read it, instead of to `send` callers and
    /// subscribers. For the shared-browser host, which routes messages
    /// without reading them twice or passing them between threads.
    /// [`CdpClient::is_closed`] says when the connection has ended.
    pub fn take_raw(&self, handler: RawHandler) {
        *self.inner.raw.lock().unwrap() = Some(handler);
    }

    /// Send one CDP command and await its `{result}` (or `{error}`).
    pub async fn send(
        &self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
        timeout: Duration,
    ) -> Result<Value> {
        trace(method, &params);
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        if self.inner.closed.load(Ordering::SeqCst) {
            // Known-closed transport, nothing written: this one request
            // positively never went out (unlike a drained pending request,
            // whose write may have landed).
            return Err(NeverReached(method.to_string()).into());
        }
        self.inner.pending.lock().unwrap().insert(id, (session_id.map(str::to_string), tx));
        // The transport may have closed (and drained `pending`) between the
        // check and the insert.
        if self.inner.closed.load(Ordering::SeqCst) {
            self.inner.pending.lock().unwrap().remove(&id);
            return Err(NeverReached(method.to_string()).into());
        }

        let mut msg = serde_json::json!({
            "id": id,
            "method": method,
            "params": params,
        });
        if let Some(session) = session_id {
            msg["sessionId"] = Value::String(session.to_string());
        }
        if let Some(writer) = self.inner.writer.lock().unwrap().as_ref() {
            if writer.send(msg.to_string()).is_err() {
                // The writer task is gone: the message never left.
                self.inner.pending.lock().unwrap().remove(&id);
                return Err(NeverReached(method.to_string()).into());
            }
        } else {
            self.inner.pending.lock().unwrap().remove(&id);
            return Err(NeverReached(method.to_string()).into());
        }

        // Every outcome but a typed never-reached leaves the request's
        // effect unproven: it answered, it may have been written, or its
        // answer never came.
        let response = match tokio::time::timeout(timeout, rx).await {
            Err(_) => {
                self.inner.effects_uncertain.fetch_add(1, Ordering::SeqCst);
                anyhow::bail!("{NO_ANSWER} within {} ms ({method})", timeout.as_millis());
            }
            Ok(Err(_)) => {
                self.inner.effects_uncertain.fetch_add(1, Ordering::SeqCst);
                anyhow::bail!("{CLOSED} ({method})");
            }
            Ok(Ok(response)) => response,
        };
        match response {
            Ok(value) => {
                self.inner.effects_uncertain.fetch_add(1, Ordering::SeqCst);
                Ok(value)
            }
            Err(Refusal::NeverReached) => Err(NeverReached(method.to_string()).into()),
            Err(Refusal::Closed) => {
                self.inner.effects_uncertain.fetch_add(1, Ordering::SeqCst);
                anyhow::bail!("{CLOSED} ({method})")
            }
            Err(Refusal::Other(e)) => {
                self.inner.effects_uncertain.fetch_add(1, Ordering::SeqCst);
                anyhow::bail!("the browser refused {method}: {e}")
            }
        }
    }

    /// Requests whose effect on the browser cannot be certified absent
    /// (see `effects_uncertain`): an op that has none new across it could
    /// not have changed the browser.
    pub fn effects_uncertain(&self) -> u64 {
        self.inner.effects_uncertain.load(Ordering::SeqCst)
    }

    /// Has the connection ended (the browser, or a shared browser's host,
    /// gone)?
    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::SeqCst)
    }

    /// For a raw (routed) connection: run `f` once if a frame's write
    /// fails, with that frame and everything still queued behind it. A
    /// failed frame never completed (no terminating NUL), and queued
    /// frames were never written: all of it positively never ran.
    pub fn on_write_failed(&self, f: impl Fn(String, Vec<String>) + Send + 'static) {
        *self.inner.write_failed.lock().unwrap() = Some(Box::new(f));
    }

    /// Run `f` as soon as the connection ends, on the task that notices:
    /// a watcher reacts at once instead of polling [`CdpClient::is_closed`].
    /// A connection already closed never runs it.
    pub fn on_close(&self, f: impl Fn() + Send + 'static) {
        self.inner.on_close.lock().unwrap().push(Box::new(f));
    }

    /// How many events have been broadcast to subscribers so far.
    pub fn events_sent(&self) -> u64 {
        self.inner.events_sent.load(Ordering::SeqCst)
    }

    /// Subscribe to CDP events (method-keyed messages without `id`).
    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.inner.events.subscribe()
    }

    /// Wait for the first event matching `pred`.
    pub async fn wait_event(&self, mut pred: impl FnMut(&Value) -> bool, timeout: Duration) -> Result<Value> {
        let mut rx = self.subscribe();
        tokio::time::timeout(timeout, async {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if pred(&event) {
                            return Ok(event);
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(e) => anyhow::bail!("{CLOSED} (events: {e})"),
                }
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("{NO_ANSWER}: the event waited for did not come"))?
    }

    /// Stop the pump task; the websocket closes when the task drops it.
    /// After this, `send` fails fast.
    pub fn shutdown(&self) {
        self.inner.writer.lock().unwrap().take();
    }
}

/// `NAVIGERA_CDP_TRACE=<file>`: append every CDP command this process sends as one
/// JSON line `{"method", "params"}` with values reduced to their shape
/// (short strings kept so enum values can be checked). CI validates the
/// trace against the running browser's own `/json/protocol`
/// (`tools/cdp_check.py`), so a renamed method or parameter in a new Chrome
/// fails a test run instead of an agent's session.
fn trace(method: &str, params: &Value) {
    use std::io::Write as _;
    static FILE: std::sync::OnceLock<Option<Mutex<std::fs::File>>> = std::sync::OnceLock::new();
    let file = FILE.get_or_init(|| {
        let path = std::env::var_os("NAVIGERA_CDP_TRACE")?;
        std::fs::OpenOptions::new().create(true).append(true).open(path).ok().map(Mutex::new)
    });
    let Some(file) = file else { return };
    // One write() per line: several navigera processes append to the
    // same file in parallel, and O_APPEND keeps single writes whole.
    let mut line = serde_json::json!({ "method": method, "params": shape(params, 0) }).to_string();
    line.push('\n');
    if let Ok(mut f) = file.lock() {
        let _ = f.write_all(line.as_bytes());
    }
}

/// A value's shape for the trace: objects/arrays recursed (bounded), short
/// strings kept verbatim (enum candidates), everything else as a type tag.
fn shape(value: &Value, depth: usize) -> Value {
    match value {
        Value::Object(map) if depth < 4 => {
            Value::Object(map.iter().map(|(k, v)| (k.clone(), shape(v, depth + 1))).collect())
        }
        Value::Array(items) if depth < 4 => {
            Value::Array(items.first().map(|v| shape(v, depth + 1)).into_iter().collect())
        }
        Value::String(s) if s.len() <= 40 && !s.contains(char::is_whitespace) => Value::String(s.clone()),
        Value::String(_) => Value::String("<string>".into()),
        Value::Number(_) => Value::String("<number>".into()),
        Value::Bool(_) => Value::String("<boolean>".into()),
        Value::Null => Value::Null,
        Value::Object(_) => Value::String("<object>".into()),
        Value::Array(_) => Value::String("<array>".into()),
    }
}

fn dispatch(text: &str, pending: &Mutex<Pending>, events: &broadcast::Sender<Value>, sent: &AtomicU64, raw: &RawTap) {
    if let Some(handler) = raw.lock().unwrap().as_mut() {
        handler(text);
        return;
    }
    let Ok(mut value) = serde_json::from_str::<Value>(text) else {
        return;
    };
    if let Some(id) = value.get("id").and_then(Value::as_u64) {
        let responder = pending.lock().unwrap().remove(&id);
        if let Some((_, responder)) = responder {
            if let Some(error) = value.get("error") {
                let refusal = if error.get("code").and_then(Value::as_i64) == Some(NEVER_REACHED_CODE)
                    && error.get("message").and_then(Value::as_str) == Some(NEVER_REACHED)
                {
                    Refusal::NeverReached
                } else {
                    Refusal::Other(error.to_string())
                };
                let _ = responder.send(Err(refusal));
            } else {
                // Moved out, not copied: a snapshot's result is megabytes.
                let result = value.get_mut("result").map(Value::take).unwrap_or(Value::Null);
                let _ = responder.send(Ok(result));
            }
        }
        return;
    }
    if let Some(method) = value.get("method").and_then(Value::as_str) {
        // The tab closed (or was detached): Chrome never answers what was
        // still waiting on it, so it fails now rather than at its timeout.
        if method == "Target.detachedFromTarget" {
            if let Some(gone) = value.pointer("/params/sessionId").and_then(Value::as_str) {
                let mut pending = pending.lock().unwrap();
                let ids: Vec<u64> =
                    pending.iter().filter(|(_, (s, _))| s.as_deref() == Some(gone)).map(|(id, _)| *id).collect();
                for id in ids {
                    if let Some((_, responder)) = pending.remove(&id) {
                        let _ = responder.send(Err(Refusal::Other(format!(
                            "{SESSION_GONE}: the tab's session ended while this ran"
                        ))));
                    }
                }
            }
        }
        sent.fetch_add(1, Ordering::SeqCst);
        let _ = events.send(value);
    }
}

/// T-0226: the never-reached classification, with a scripted peer on the
/// other end of a socket pair so each delivery case is deterministic.
/// The invariant under test: a request is only ever marked NEVER_REACHED
/// (recoverable) when the peer provably could not dispatch it; anything
/// it may have dispatched stays CLOSED (never retried), so a mutation
/// runs at most once.
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, UnixStream};

    const T: Duration = Duration::from_secs(5);

    /// Read one NUL-terminated frame; the count of bytes read.
    async fn read_frame(read: &mut (impl AsyncReadExt + Unpin), buf: &mut Vec<u8>) -> Vec<u8> {
        let mut chunk = [0u8; 8192];
        loop {
            let n = read.read(&mut chunk).await.unwrap();
            assert!(n > 0, "peer: connection ended mid-frame");
            buf.extend_from_slice(&chunk[..n]);
            if let Some(at) = buf.iter().position(|b| *b == 0) {
                return buf.drain(..=at).collect();
            }
        }
    }

    fn framed_pair() -> (CdpClient, UnixStream) {
        let (a, b) = UnixStream::pair().unwrap();
        let (r, w) = a.into_split();
        (CdpClient::connect_framed(r, w), b)
    }

    /// The peer is already gone: the write fails before a byte lands.
    #[tokio::test]
    async fn write_to_a_gone_peer_never_reached_it() {
        let (client, peer) = framed_pair();
        drop(peer);
        let err = client.send("Page.navigate", json!({}), None, T).await.unwrap_err();
        assert!(format!("{err:#}").contains(NEVER_REACHED), "{err:#}");
    }

    /// The peer takes a payload prefix and goes: the frame's NUL never
    /// lands, so it can never be dispatched even though bytes did.
    #[tokio::test]
    async fn a_partial_payload_without_its_terminator_never_reached() {
        let (client, mut peer) = framed_pair();
        let dispatches = Arc::new(AtomicUsize::new(0));
        let d = Arc::clone(&dispatches);
        let peer_task = tokio::spawn(async move {
            let mut got = vec![0u8; 100];
            let n = peer.read(&mut got).await.unwrap();
            assert!(n > 0 && !got[..n].contains(&0), "peer saw a terminator");
            drop(peer); // gone before the rest of the frame
            d
        });
        let big = "A".repeat(4 << 20);
        let err = client.send("X.mutate", json!({ "big": big }), None, T).await.unwrap_err();
        let dispatches = peer_task.await.unwrap();
        assert!(format!("{err:#}").contains(NEVER_REACHED), "{err:#}");
        assert_eq!(dispatches.load(Ordering::SeqCst), 0, "no dispatch, so no replay risk");
    }

    /// A full frame lands and the peer dispatches it, then dies without
    /// answering: the request may have run, so it stays CLOSED and is
    /// never retried - the mutation happens at most once.
    #[tokio::test]
    async fn an_answered_never_frame_stays_closed_and_is_not_retried() {
        let (client, mut peer) = framed_pair();
        let dispatches = Arc::new(AtomicUsize::new(0));
        let d = Arc::clone(&dispatches);
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let frame = read_frame(&mut peer, &mut buf).await;
            let text = std::str::from_utf8(&frame).unwrap();
            assert!(text.contains("X.mutate"));
            d.fetch_add(1, Ordering::SeqCst); // dispatched
            drop(peer); // response lost
        });
        let err = client.send("X.mutate", json!({}), None, T).await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains(CLOSED) && !msg.contains(NEVER_REACHED), "{msg}");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(dispatches.load(Ordering::SeqCst), 1, "dispatched once, never replayed");
    }

    /// The frame being written fails and what was queued behind it was
    /// never attempted: the router is handed both, all positively unsent.
    #[tokio::test]
    async fn the_failed_frame_and_the_unsent_queue_are_handed_over() {
        let (client, mut peer) = framed_pair();
        let handed = Arc::new(Mutex::new(None));
        let h = Arc::clone(&handed);
        client.on_write_failed(move |failed, unsent| {
            *h.lock().unwrap() = Some((failed, unsent));
        });
        let writer = client.raw_writer().unwrap();
        let big = format!("{{\"id\":7,\"method\":\"X.mutate\",\"params\":{{\"big\":\"{}\"}}}}", "A".repeat(4 << 20));
        writer.send(big.clone()).unwrap();
        writer.send("{\"id\":8,\"method\":\"X.mutate\",\"params\":{}}".to_string()).unwrap();
        let mut got = vec![0u8; 100];
        let n = peer.read(&mut got).await.unwrap();
        assert!(n > 0);
        drop(peer);
        for _ in 0..100 {
            if handed.lock().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let (failed, unsent) = handed.lock().unwrap().take().expect("write failure reported");
        assert_eq!(failed, big, "the frame mid-write");
        assert_eq!(unsent, ["{\"id\":8,\"method\":\"X.mutate\",\"params\":{}}"], "never attempted");
        assert!(client.is_closed());
    }

    /// A frame the peer never even read - but whose write succeeded -
    /// stays CLOSED: dispatch cannot be disproven, so no retry, and the
    /// mutation still runs at most once.
    #[tokio::test]
    async fn a_written_but_unread_frame_stays_closed() {
        let (client, mut peer) = framed_pair();
        let dispatches = Arc::new(AtomicUsize::new(0));
        let d = Arc::clone(&dispatches);
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let frame = read_frame(&mut peer, &mut buf).await;
            let id: u64 = serde_json::from_str::<Value>(std::str::from_utf8(&frame[..frame.len() - 1]).unwrap())
                .unwrap()["id"]
                .as_u64()
                .unwrap();
            d.fetch_add(1, Ordering::SeqCst);
            let answer = format!("{{\"id\":{id},\"result\":{{}}}}");
            let mut w = answer.into_bytes();
            w.push(0);
            peer.write_all(&w).await.unwrap();
            // Never read the second frame; die with it unread.
            tokio::time::sleep(Duration::from_millis(300)).await;
            drop(peer);
        });
        let ok = client.send("X.mutate", json!({ "n": 1 }), None, T).await.unwrap();
        assert_eq!(ok, json!({}));
        tokio::time::sleep(Duration::from_millis(50)).await;
        let err = client.send("X.mutate", json!({ "n": 2 }), None, T).await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains(CLOSED) && !msg.contains(NEVER_REACHED), "{msg}");
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(dispatches.load(Ordering::SeqCst), 1, "second mutation never dispatched, never retried");
    }

    /// Over a websocket the same rule holds: a write that may have
    /// landed stays CLOSED; a send on a known-closed connection never
    /// went out.
    #[tokio::test]
    async fn websocket_write_loss_stays_closed_and_pre_send_never_reached() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let dispatches = Arc::new(AtomicUsize::new(0));
        let d = Arc::clone(&dispatches);
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            use futures_util::StreamExt;
            use tokio_tungstenite::tungstenite::Message;
            if let Some(Ok(Message::Text(_))) = ws.next().await {
                d.fetch_add(1, Ordering::SeqCst); // dispatched, answer withheld
            }
            drop(ws); // connection lost with the request maybe executed
        });
        let client = CdpClient::connect_within(&format!("ws://{addr}"), T).await.unwrap();
        let err = client.send("X.mutate", json!({}), None, T).await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains(CLOSED) && !msg.contains(NEVER_REACHED), "{msg}");
        for _ in 0..100 {
            if client.is_closed() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let err = client.send("X.mutate", json!({}), None, T).await.unwrap_err();
        assert!(format!("{err:#}").contains(NEVER_REACHED), "{err:#}");
        assert_eq!(dispatches.load(Ordering::SeqCst), 1, "dispatched once, never replayed");
    }
}
