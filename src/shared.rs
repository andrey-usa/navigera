//! One browser for many named sessions: each session works in a browser
//! context of its own on a browser that a small host process keeps.
//!
//! Every `-s <name>` used to launch a whole Chrome, and most of a Chrome's
//! memory is per browser, not per page: its browser, GPU, network, storage
//! and utility processes. One more browser context mostly adds its
//! renderer. So parallel agents, each on its own session, now cost one
//! browser plus a renderer each, as Playwright's one driver with a context
//! per test does. `start --own-browser` (or `NAVIGERA_SHARED_BROWSER=0`)
//! keeps a browser per session.
//!
//! How:
//! - The host (`navigera --session <socket> browser-host`, spawned detached
//!   by the first session that needs it) launches Chrome as a session would
//!   (same flags; the private pipe on Linux/macOS) and listens on a socket
//!   in the per-user session directory, `browser-<key>.sock` (on Windows the
//!   loopback-port-plus-token file sessions use).
//! - Each client gets a browser-level CDP session of its own
//!   (`Target.attachToBrowserTarget`), so its domain state (target
//!   discovery, download events) and its events are its own. The host
//!   forwards messages, renumbering command ids and routing by CDP session
//!   id; a client can only address the sessions it opened, and cannot close
//!   the browser. No DevTools port is opened: on Linux/macOS only this
//!   user's processes can reach the browser, as with a browser of one's own.
//! - A client's browser contexts are disposed when its connection ends, a
//!   killed session server's included.
//! - The host closes the browser and exits once no client has been
//!   connected for the grace period (`NAVIGERA_SHARED_GRACE_S`, default
//!   30 s: the next agent's start is warm), or when the browser dies: every
//!   client's connection then ends, and their next op says so.
//! - Which browser: one per launch key (navigera's version, the executable,
//!   headless or not, natural mode, the launch flags including
//!   `NAVIGERA_EXTRA_FLAGS`, `NAVIGERA_DROP_FLAGS` and
//!   `NAVIGERA_NO_SANDBOX`), per OS user (the session directory is the
//!   user's own). A file lock around find-or-launch makes two sessions
//!   starting at once share one browser, and keeps the host from closing
//!   the browser while a session is joining it.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};
#[cfg(test)]
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::cdp::CdpClient;
use crate::protocol::SessionConfig;
use crate::session::endpoint::{self, Listener, Stream};

/// How long the host keeps the browser after its last session left.
pub const DEFAULT_GRACE_S: u64 = 30;

/// How long a session waits for a host it started to listen.
const HOST_START: Duration = Duration::from_secs(60);

/// Does a session started with `config` go on the shared browser? Not
/// when it asked for its own (`--own-browser`, `NAVIGERA_SHARED_BROWSER=0`),
/// or works in a browser it does not launch (`--attach`, `--profile`), or
/// runs Lightpanda (one context per process), or asked for a DevTools port
/// on Linux/macOS (`--transport ws`, to open DevTools on it).
pub fn wanted(config: &SessionConfig) -> bool {
    if config.own_browser
        || crate::browser::is_lightpanda(&config.engine)
        || config.attach.is_some()
        || config.profile.is_some()
    {
        return false;
    }
    if std::env::var("NAVIGERA_SHARED_BROWSER").is_ok_and(|v| matches!(v.trim(), "0" | "false" | "no" | "off")) {
        return false;
    }
    let transport = config.transport.clone().or_else(|| std::env::var("NAVIGERA_CDP_TRANSPORT").ok());
    if cfg!(unix) && transport.is_some_and(|t| t.trim() == "ws") {
        return false;
    }
    socket_fits()
}

/// Does the host's socket path fit a Unix socket address? Under a session
/// directory with a long path (a deep `$XDG_RUNTIME_DIR` or `$TMPDIR`) it
/// would not bind, and the session takes a browser of its own instead.
fn socket_fits() -> bool {
    if !cfg!(unix) {
        return true;
    }
    // The address holds the path and its NUL: 108 bytes on Linux, 104 on
    // macOS and the BSDs.
    let max = if cfg!(target_os = "linux") { 107 } else { 103 };
    match paths(&"0".repeat(16)) {
        Ok((socket, _, _)) if socket.as_os_str().len() > max => {
            eprintln!(
                "navigera: {} is too long for a socket address: this session has a browser of its own",
                socket.display()
            );
            false
        }
        _ => true,
    }
}

/// What a shared browser is launched as; sessions asking for the same
/// share one.
pub struct Spec {
    /// The resolved browser executable.
    pub exe: String,
    pub headless: bool,
    pub natural: bool,
}

impl Spec {
    /// The launch key: a hash of everything that makes two browsers differ.
    fn key(&self) -> String {
        let mut text = format!("{}\n{}\n{}\n{}\n", env!("CARGO_PKG_VERSION"), self.exe, self.headless, self.natural);
        for flag in crate::browser::chrome_flags() {
            text.push_str(&flag);
            text.push('\n');
        }
        format!("{:016x}", fnv1a(text.as_bytes()))
    }
}

/// FNV-1a: stable across builds and platforms, unlike std's hasher.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3))
}

/// The host's socket, lock and log for a launch key.
fn paths(key: &str) -> Result<(PathBuf, PathBuf, PathBuf)> {
    let dir = crate::session::session_dir().context("session directory")?;
    Ok((
        dir.join(format!("browser-{key}.sock")),
        dir.join(format!("browser-{key}.lock")),
        dir.join(format!("browser-{key}.log")),
    ))
}

/// Held by a session while it joins the shared browser: the host does not
/// close the browser, and no second host starts, until it is dropped.
pub struct Lease {
    _lock: std::fs::File,
}

fn lock_file(path: &Path) -> Result<std::fs::File> {
    // The session directory is this user's alone (0700, checked), so
    // nothing else can have planted a link here.
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .with_context(|| format!("open {}", path.display()))
}

/// Connect to the shared browser `spec` describes, starting its host if
/// none runs. Returns the connection, the lease to drop once the session's
/// context exists, and the host's socket.
pub fn connect(spec: &Spec) -> Result<(Stream, Lease, PathBuf)> {
    let (socket, lock_path, log) = paths(&spec.key())?;
    let lock = lock_file(&lock_path)?;
    lock.lock().with_context(|| format!("lock {}", lock_path.display()))?;
    let lease = Lease { _lock: lock };
    if let Ok(stream) = endpoint::connect(&socket) {
        return Ok((stream, lease, socket));
    }
    // A socket nobody answers on is left over from a host that crashed.
    let _ = std::fs::remove_file(&socket);
    let mut child = spawn_host(spec, &socket, &log)?;
    let deadline = Instant::now() + HOST_START;
    let started = Instant::now();
    loop {
        if let Ok(stream) = endpoint::connect(&socket) {
            // Reaped when it exits, so a long-lived session server keeps
            // no zombie behind.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return Ok((stream, lease, socket));
        }
        if let Ok(Some(status)) = child.try_wait() {
            bail!("the shared browser did not start ({status}):\n{}", crate::session::log_tail(&log, 20));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("the shared browser did not start within {} s; see {}", HOST_START.as_secs(), log.display());
        }
        let step = if started.elapsed() < Duration::from_secs(3) { 5 } else { 50 };
        std::thread::sleep(Duration::from_millis(step));
    }
}

fn spawn_host(spec: &Spec, socket: &Path, log: &Path) -> Result<std::process::Child> {
    let exe = std::env::current_exe().context("locate the navigera binary")?;
    let log = crate::session::create_log(log).with_context(|| format!("create {}", log.display()))?;
    let mut cmd = std::process::Command::new(exe);
    // `--session` first: a program that embeds navigera (primavera) hands
    // such a command line to navigera's CLI.
    cmd.arg("--session").arg(socket).arg("--chromium").arg(&spec.exe);
    if !spec.headless {
        cmd.arg("--headed");
    }
    if spec.natural {
        cmd.arg("--natural");
    }
    cmd.arg("browser-host").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::from(log));
    crate::proc::spawn_detached(&mut cmd).context("spawn the shared browser's host")
}

/// A CDP client over a connection to the host (NUL-terminated JSON, as on
/// Chrome's pipe). Must run where `handle`'s runtime can register sockets.
pub fn client(stream: Stream, handle: &tokio::runtime::Handle) -> Result<CdpClient> {
    let _guard = handle.enter();
    stream.set_nonblocking(true).context("shared browser connection")?;
    #[cfg(unix)]
    let (reader, writer) = tokio::net::UnixStream::from_std(stream).context("shared browser connection")?.into_split();
    #[cfg(windows)]
    let (reader, writer) = tokio::net::TcpStream::from_std(stream).context("shared browser connection")?.into_split();
    Ok(CdpClient::connect_framed(reader, writer))
}

/// Where a client's messages go: its socket, written by whichever thread
/// routes the message (a client that stops reading for the write timeout
/// loses its connection, not everyone their answers).
enum Out {
    Socket(Stream),
    #[cfg(test)]
    Channel(mpsc::Sender<Vec<u8>>),
}

/// How long a write to a client may block.
const CLIENT_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// A command the browser has yet to answer.
enum Pending {
    /// A client's: its own id, and what the answer tells the router.
    Client { client: u64, id: u64, kind: Kind },
    /// The browser-level session a new client gets.
    Root(u64),
    /// The host's own.
    Host,
}

#[derive(Clone, Copy)]
enum Kind {
    Plain,
    /// The answer names a CDP session the client opened.
    Attach,
    /// The answer names a browser context the client made.
    CreateContext,
}

struct Client {
    out: Out,
    /// Its browser-level CDP session, once the browser made it.
    root: Option<String>,
    /// What it sent before that.
    queued: Vec<String>,
    /// The browser contexts it made, disposed when it goes.
    contexts: HashSet<String>,
}

impl Client {
    fn send(&self, text: String) {
        let mut bytes = text.into_bytes();
        bytes.push(0);
        match &self.out {
            // A failed write ends the connection; its reader sees that and
            // the client goes.
            Out::Socket(stream) => {
                let mut writer = stream;
                if writer.write_all(&bytes).is_err() {
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                }
            }
            #[cfg(test)]
            Out::Channel(tx) => {
                let _ = tx.send(bytes);
            }
        }
    }

    /// End the connection.
    fn close(&self) {
        match &self.out {
            Out::Socket(stream) => {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
            #[cfg(test)]
            Out::Channel(_) => {}
        }
    }
}

/// The fields of a CDP message the router reads; the rest passes as is.
#[derive(Deserialize)]
struct Envelope<'a> {
    id: Option<u64>,
    #[serde(borrow)]
    method: Option<Cow<'a, str>>,
    #[serde(rename = "sessionId", borrow)]
    session_id: Option<Cow<'a, str>>,
    #[serde(borrow)]
    params: Option<&'a RawValue>,
    #[serde(borrow)]
    result: Option<&'a RawValue>,
    #[serde(borrow)]
    error: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct Fields<'a> {
    #[serde(rename = "sessionId", borrow)]
    session_id: Option<Cow<'a, str>>,
    #[serde(rename = "browserContextId", borrow)]
    context: Option<Cow<'a, str>>,
}

fn fields(raw: Option<&RawValue>) -> Option<Fields<'_>> {
    serde_json::from_str(raw?.get()).ok()
}

/// A CDP message's envelope read without parsing the message: the id at
/// the front (`{"id":N,`), the method after it or at the front
/// (`"method":"M"`), and a top-level `sessionId` at the end
/// (`,"sessionId":"S"}`). Chrome and navigera's client both write the id
/// first and the session last, so the router can renumber and route the
/// answers and events it only passes on (nearly all of them: a large
/// snapshot is thousands) with string edits instead of a parse of the whole
/// message. A raw `,"sessionId":"` cannot occur inside a JSON string (its
/// quotes are escaped), and an object nested at the end would leave `}}`,
/// so a text that ends that way names the top-level session.
#[derive(Debug, PartialEq)]
struct Shape<'a> {
    /// The id and where its digits end.
    id: Option<(u64, usize)>,
    method: Option<&'a str>,
    /// The session and where `,"sessionId"` starts.
    session: Option<(&'a str, usize)>,
}

fn shape(text: &str) -> Shape<'_> {
    let plain = |v: &str| !v.is_empty() && !v.contains(['"', '\\']);
    let id = text.strip_prefix("{\"id\":").and_then(|rest| {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        let end = 6 + digits;
        (digits > 0 && text.as_bytes().get(end) == Some(&b','))
            .then(|| rest[..digits].parse().ok().map(|n| (n, end)))?
    });
    let method_at = match id {
        Some((_, end)) => text[end..].strip_prefix(",\"method\":\"").map(|_| end + 11),
        None => text.strip_prefix("{\"method\":\"").map(|_| 11),
    };
    let method = method_at.and_then(|at| text[at..].split_once('"').map(|(m, _)| m)).filter(|m| plain(m));
    let session = text
        .strip_suffix("\"}")
        .and_then(|body| body.rfind(",\"sessionId\":\"").map(|at| (&body[at + 14..], at)))
        .filter(|(v, _)| plain(v));
    Shape { id, method, session }
}

/// `text` with its id (digits ending at `end`) replaced.
fn with_id(text: &str, end: usize, id: u64) -> String {
    format!("{{\"id\":{id}{}", &text[end..])
}

/// `text` without its trailing `,"sessionId":"…"` (starting at `at`).
fn without_session(text: &str, at: usize) -> String {
    let mut out = String::with_capacity(at + 1);
    out.push_str(&text[..at]);
    out.push('}');
    out
}

fn quoted(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

fn command(id: u64, method: &str, params: &str, session: Option<&str>) -> String {
    let mut text = format!("{{\"id\":{id},\"method\":{},\"params\":{params}", quoted(method));
    if let Some(session) = session {
        text.push_str(",\"sessionId\":");
        text.push_str(&quoted(session));
    }
    text.push('}');
    text
}

fn answer(id: u64, result: Option<&RawValue>, error: Option<&RawValue>, session: Option<&str>) -> String {
    let mut text = match (result, error) {
        (_, Some(error)) => format!("{{\"id\":{id},\"error\":{}", error.get()),
        (Some(result), None) => format!("{{\"id\":{id},\"result\":{}", result.get()),
        (None, None) => format!("{{\"id\":{id},\"result\":{{}}"),
    };
    if let Some(session) = session {
        text.push_str(",\"sessionId\":");
        text.push_str(&quoted(session));
    }
    text.push('}');
    text
}

fn refusal(id: u64, message: &str) -> String {
    format!("{{\"id\":{id},\"error\":{{\"code\":-32000,\"message\":{}}}}}", quoted(message))
}

fn event(method: &str, params: Option<&RawValue>, session: Option<&str>) -> String {
    let mut text = format!("{{\"method\":{},\"params\":{}", quoted(method), params.map_or("{}", RawValue::get));
    if let Some(session) = session {
        text.push_str(",\"sessionId\":");
        text.push_str(&quoted(session));
    }
    text.push('}');
    text
}

/// The host's own method: close the browser as soon as no session uses it
/// (`sessions --clean`), not after the grace period.
pub(crate) const CLOSE_IF_UNUSED: &str = "Navigera.closeIfUnused";

/// Ask the host on `socket` to close its browser once it has no session.
pub fn close_if_unused(socket: &Path) {
    let Ok(mut stream) = endpoint::connect(socket) else { return };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.write_all(format!("{}\0", command(1, CLOSE_IF_UNUSED, "{}", None)).as_bytes());
    // The answer comes once the host made this connection a client; it
    // counts as gone once this returns and the stream drops.
    let mut answer = Vec::new();
    let _ = BufReader::new(&stream).read_until(0, &mut answer);
}

/// What the host tells a client that tries to close the browser.
pub const NOT_CLOSED: &str = "this browser is shared with other sessions: it closes on its own once no session uses it";

struct Router {
    to_chrome: tokio::sync::mpsc::UnboundedSender<String>,
    /// Ids the host sends with: above what the launch used, and within
    /// the int32 Chrome takes.
    next_id: u64,
    clients: HashMap<u64, Client>,
    pending: HashMap<u64, Pending>,
    /// Which client each CDP session belongs to.
    owners: HashMap<String, u64>,
    /// Since when no client has been connected.
    idle_since: Option<Instant>,
    /// `Navigera.closeIfUnused` came: close as soon as no client is left,
    /// without the grace period.
    close_now: bool,
}

impl Router {
    fn send_chrome(&mut self, pending: Pending, method: &str, params: &str, session: Option<&str>) {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, pending);
        let _ = self.to_chrome.send(command(id, method, params, session));
    }

    fn new_client(&mut self, id: u64, out: Out) {
        self.clients.insert(id, Client { out, root: None, queued: Vec::new(), contexts: HashSet::new() });
        self.idle_since = None;
        self.close_now = false;
        self.send_chrome(Pending::Root(id), "Target.attachToBrowserTarget", "{}", None);
    }

    fn on_client(&mut self, cid: u64, text: String) {
        let Some(client) = self.clients.get_mut(&cid) else { return };
        let Some(root) = client.root.clone() else {
            client.queued.push(text);
            return;
        };
        // A command the router only passes on: renumbered, and sent on the
        // client's own session when it names none.
        let sh = shape(&text);
        if let (Some((id, end)), Some(method)) = (sh.id, sh.method) {
            let special = matches!(
                method,
                CLOSE_IF_UNUSED
                    | "Browser.close"
                    | "Browser.crash"
                    | "Browser.crashGpuProcess"
                    | "Target.attachToTarget"
                    | "Target.attachToBrowserTarget"
                    | "Target.createBrowserContext"
                    | "Target.disposeBrowserContext"
            );
            if !special {
                let addressed = match sh.session {
                    None => true,
                    Some((s, _)) => self.owners.get(s) == Some(&cid),
                };
                if !addressed {
                    client.send(refusal(id, crate::cdp::SESSION_GONE));
                    return;
                }
                let global = self.next_id;
                self.next_id += 1;
                self.pending.insert(global, Pending::Client { client: cid, id, kind: Kind::Plain });
                let mut out = with_id(&text, end, global);
                if sh.session.is_none() {
                    out.pop();
                    out.push_str(",\"sessionId\":");
                    out.push_str(&quoted(&root));
                    out.push('}');
                }
                let _ = self.to_chrome.send(out);
                return;
            }
        }
        let Ok(message) = serde_json::from_str::<Envelope>(&text) else { return };
        let (Some(id), Some(method)) = (message.id, message.method.as_deref()) else { return };
        let session = match message.session_id.as_deref() {
            None => root.as_str(),
            Some(s) if self.owners.get(s) == Some(&cid) => s,
            Some(_) => {
                client.send(refusal(id, crate::cdp::SESSION_GONE));
                return;
            }
        };
        if method == CLOSE_IF_UNUSED {
            self.close_now = true;
            // How many other clients use the browser: it closes once they
            // and this one are gone.
            let others = self.clients.len().saturating_sub(1);
            let result = RawValue::from_string(format!("{{\"others\":{others}}}")).ok();
            if let Some(client) = self.clients.get(&cid) {
                client.send(answer(id, result.as_deref(), None, None));
            }
            return;
        }
        if matches!(method, "Browser.close" | "Browser.crash" | "Browser.crashGpuProcess") {
            client.send(refusal(id, NOT_CLOSED));
            return;
        }
        let kind = match method {
            "Target.attachToTarget" | "Target.attachToBrowserTarget" => Kind::Attach,
            "Target.createBrowserContext" => Kind::CreateContext,
            "Target.disposeBrowserContext" => {
                if let Some(context) = fields(message.params).and_then(|f| f.context) {
                    client.contexts.remove(context.as_ref());
                }
                Kind::Plain
            }
            _ => Kind::Plain,
        };
        let params = message.params.map_or("{}", RawValue::get);
        let session = session.to_string();
        self.send_chrome(Pending::Client { client: cid, id, kind }, method, params, Some(&session));
    }

    fn on_chrome(&mut self, text: &str) {
        // An answer to a plain command, or an event the router does not
        // read: passed on with string edits.
        let sh = shape(text);
        if let Some((global, end)) = sh.id {
            if let Some(Pending::Client { kind: Kind::Plain, .. }) = self.pending.get(&global) {
                let Some(Pending::Client { client: cid, id, .. }) = self.pending.remove(&global) else { return };
                let Some(client) = self.clients.get(&cid) else { return };
                let out = match sh.session {
                    Some((s, at)) if client.root.as_deref() == Some(s) => with_id(&without_session(text, at), end, id),
                    _ => with_id(text, end, id),
                };
                client.send(out);
                return;
            }
        } else if let (Some(method), Some((session, at))) = (sh.method, sh.session) {
            if !matches!(method, "Target.attachedToTarget" | "Target.detachedFromTarget") {
                let Some(&cid) = self.owners.get(session) else { return };
                if let Some(client) = self.clients.get(&cid) {
                    let root = client.root.as_deref() == Some(session);
                    client.send(if root { without_session(text, at) } else { text.to_string() });
                }
                return;
            }
        }
        let Ok(message) = serde_json::from_str::<Envelope>(text) else { return };
        if let Some(id) = message.id {
            match self.pending.remove(&id) {
                Some(Pending::Client { client: cid, id, kind }) => {
                    let Some(client) = self.clients.get_mut(&cid) else { return };
                    if message.error.is_none() {
                        let named = fields(message.result);
                        match (kind, named) {
                            (Kind::Attach, Some(Fields { session_id: Some(s), .. })) => {
                                self.owners.insert(s.into_owned(), cid);
                            }
                            (Kind::CreateContext, Some(Fields { context: Some(c), .. })) => {
                                client.contexts.insert(c.into_owned());
                            }
                            _ => {}
                        }
                    }
                    let session = message.session_id.as_deref().filter(|s| client.root.as_deref() != Some(*s));
                    client.send(answer(id, message.result, message.error, session));
                }
                Some(Pending::Root(cid)) => {
                    let root = fields(message.result).and_then(|f| f.session_id).map(Cow::into_owned);
                    match (self.clients.get_mut(&cid), root) {
                        (Some(client), Some(root)) => {
                            self.owners.insert(root.clone(), cid);
                            client.root = Some(root);
                            for queued in std::mem::take(&mut client.queued) {
                                self.on_client(cid, queued);
                            }
                        }
                        // The client left before its session was made.
                        (None, Some(root)) => {
                            let params = format!("{{\"sessionId\":{}}}", quoted(&root));
                            self.send_chrome(Pending::Host, "Target.detachFromTarget", &params, None);
                        }
                        // The browser refused: the client's connection ends.
                        (Some(_), None) => {
                            eprintln!("navigera: the browser refused a session: {text}");
                            self.clients.remove(&cid);
                        }
                        (None, None) => {}
                    }
                }
                Some(Pending::Host) | None => {}
            }
            return;
        }
        // Events on the host's own connection concern no client.
        let (Some(method), Some(session)) = (message.method.as_deref(), message.session_id.as_deref()) else {
            return;
        };
        let Some(&cid) = self.owners.get(session) else { return };
        let child = fields(message.params).and_then(|f| f.session_id);
        // A session opened beneath a client's (auto-attach: frames,
        // workers) is that client's.
        if method == "Target.attachedToTarget" {
            if let Some(child) = &child {
                self.owners.insert(child.to_string(), cid);
            }
        }
        if let Some(client) = self.clients.get(&cid) {
            let session = Some(session).filter(|s| client.root.as_deref() != Some(*s));
            client.send(event(method, message.params, session));
        }
        if method == "Target.detachedFromTarget" {
            if let Some(child) = &child {
                self.owners.remove(child.as_ref());
            }
        }
    }

    fn gone(&mut self, cid: u64) {
        let Some(client) = self.clients.remove(&cid) else { return };
        // Its contexts (and their tabs) first, then its session.
        for context in &client.contexts {
            let params = format!("{{\"browserContextId\":{}}}", quoted(context));
            self.send_chrome(Pending::Host, "Target.disposeBrowserContext", &params, None);
        }
        if let Some(root) = &client.root {
            let params = format!("{{\"sessionId\":{}}}", quoted(root));
            self.send_chrome(Pending::Host, "Target.detachFromTarget", &params, None);
        }
        self.owners.retain(|_, owner| *owner != cid);
        if self.clients.is_empty() {
            self.idle_since = Some(Instant::now());
        }
    }
}

/// The host: launch the browser, serve sessions until it has been unused
/// for the grace period or dies. `config.session` is the socket path.
pub fn serve_host(config: &SessionConfig) -> ExitCode {
    let Some(socket) = config.session.as_deref().map(PathBuf::from) else {
        eprintln!("navigera: browser-host needs --session <socket path>");
        return ExitCode::from(2);
    };
    crate::browser::set_natural(config.natural);
    let grace = std::env::var("NAVIGERA_SHARED_GRACE_S")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map_or(Duration::from_secs(DEFAULT_GRACE_S), Duration::from_secs);
    let (runtime, browser) = match crate::browser::launch_host(!config.headed, config.chromium.as_deref()) {
        Ok(launched) => launched,
        Err(e) => {
            eprintln!("navigera: browser launch failed: {e:#}");
            return ExitCode::from(1);
        }
    };
    let to_chrome = match browser.raw_writer() {
        Ok(writer) => writer,
        Err(e) => {
            eprintln!("navigera: {e:#}");
            browser.close();
            return ExitCode::from(1);
        }
    };
    let listener = match Listener::bind(&socket) {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("navigera: bind {}: {e}", socket.display());
            browser.close();
            return ExitCode::from(1);
        }
    };
    eprintln!(
        "navigera: shared browser on {} ({}); closes {} s after its last session",
        socket.display(),
        browser.ws_url(),
        grace.as_secs()
    );
    // For `sessions`: whose memory the shared browser is.
    let pid_file = socket.with_extension("pid");
    let _ = std::fs::write(&pid_file, format!("{}\n", std::process::id()));

    let router = Arc::new(Mutex::new(Router {
        to_chrome,
        // Chrome takes ids that fit an int32; the launch used a few low ones.
        next_id: 1 << 30,
        clients: HashMap::new(),
        pending: HashMap::new(),
        owners: HashMap::new(),
        idle_since: Some(Instant::now()),
        close_now: false,
    }));
    // The browser's messages are routed on the task that reads them: no
    // hop to another thread on the way to the client's socket.
    {
        let router = Arc::clone(&router);
        browser.take_raw(Box::new(move |text| {
            if let Ok(mut router) = router.lock() {
                router.on_chrome(text);
            }
        }));
    }
    // The browser-level session the launch opened watches every target;
    // nothing reads that here.
    if let Ok(mut r) = router.lock() {
        r.send_chrome(Pending::Host, "Target.setDiscoverTargets", "{\"discover\":false}", None);
    }
    {
        let router = Arc::clone(&router);
        std::thread::spawn(move || {
            let mut next = 1u64;
            loop {
                let Ok(stream) = listener.accept() else { continue };
                serve_client(next, stream, &router);
                next += 1;
            }
        });
    }
    let lock_path = socket.with_extension("lock");
    let why = loop {
        std::thread::sleep(Duration::from_millis(250));
        if browser.is_closed() {
            break format!("the browser exited ({})", browser.death_report());
        }
        let Ok(r) = router.lock() else { break "the router failed".to_string() };
        let wait = if r.close_now { Duration::ZERO } else { grace };
        if !r.idle_since.is_some_and(|since| since.elapsed() >= wait) {
            continue;
        }
        // A session joining holds the lock from its connect until its
        // context exists: not now, then. Holding the router, no client can
        // arrive while this decides.
        let Ok(lock) = lock_file(&lock_path) else { continue };
        if lock.try_lock().is_err() {
            continue;
        }
        if r.clients.is_empty() {
            // Gone before the lock is let go: the next session to take it
            // starts a new host.
            let _ = std::fs::remove_file(&socket);
            break format!("no session for {} s", grace.as_secs());
        }
    };
    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_file(&pid_file);
    // Every client's connection ends with its socket.
    if let Ok(mut r) = router.lock() {
        for client in r.clients.values() {
            client.close();
        }
        r.clients.clear();
    }
    browser.close();
    drop(runtime);
    eprintln!("navigera: shared browser closed: {why}");
    ExitCode::SUCCESS
}

/// Take a client: its socket is written by whoever routes to it, and a
/// thread of its own reads it and routes what it sends.
fn serve_client(id: u64, stream: Stream, router: &Arc<Mutex<Router>>) {
    let Ok(read_half) = stream.try_clone() else { return };
    let _ = stream.set_write_timeout(Some(CLIENT_WRITE_TIMEOUT));
    if let Ok(mut r) = router.lock() {
        r.new_client(id, Out::Socket(stream));
    }
    let router = Arc::clone(router);
    std::thread::spawn(move || {
        let mut reader = BufReader::with_capacity(256 * 1024, read_half);
        let mut buf = Vec::with_capacity(4096);
        loop {
            buf.clear();
            match reader.read_until(0, &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if buf.last() == Some(&0) {
                        buf.pop();
                    }
                    let Ok(text) = String::from_utf8(std::mem::take(&mut buf)) else { continue };
                    let Ok(mut r) = router.lock() else { return };
                    r.on_client(id, text);
                }
            }
        }
        if let Ok(mut r) = router.lock() {
            r.gone(id);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_keep_their_payload_and_take_the_new_envelope() {
        let raw: Box<RawValue> = RawValue::from_string("{\"a\":[1,2,{\"b\":\"\\u00e9\"}]}".into()).unwrap();
        assert_eq!(
            command(7, "Page.navigate", raw.get(), Some("S1")),
            "{\"id\":7,\"method\":\"Page.navigate\",\"params\":{\"a\":[1,2,{\"b\":\"\\u00e9\"}]},\"sessionId\":\"S1\"}"
        );
        assert_eq!(answer(3, Some(&raw), None, None), "{\"id\":3,\"result\":{\"a\":[1,2,{\"b\":\"\\u00e9\"}]}}");
        let err: Box<RawValue> = RawValue::from_string("{\"code\":-32000}".into()).unwrap();
        assert_eq!(
            answer(3, None, Some(&err), Some("S")),
            "{\"id\":3,\"error\":{\"code\":-32000},\"sessionId\":\"S\"}"
        );
        assert_eq!(event("Page.loadEventFired", None, None), "{\"method\":\"Page.loadEventFired\",\"params\":{}}");
        let refused: serde_json::Value = serde_json::from_str(&refusal(9, NOT_CLOSED)).unwrap();
        assert_eq!(refused["id"], 9);
        assert_eq!(refused["error"]["message"], NOT_CLOSED);
    }

    #[test]
    fn the_envelope_is_read_without_parsing_and_only_where_it_is_certain() {
        let sh = shape(r#"{"id":42,"result":{"nodes":[{"sessionId":"inner"}]},"sessionId":"S1"}"#);
        assert_eq!(sh.id, Some((42, 8)));
        assert_eq!(sh.session.map(|s| s.0), Some("S1"));
        // A nested sessionId at the end is not the message's.
        let sh = shape(r#"{"id":7,"result":{"sessionId":"inner"}}"#);
        assert_eq!((sh.id.map(|i| i.0), sh.session), (Some(7), None));
        // Quotes inside strings are escaped, so they can't fake the suffix.
        let sh = shape(r#"{"id":3,"result":{},"note":"a,\"sessionId\":\"x"}"#);
        assert!(sh.id.is_some());
        assert_eq!(sh.session, None);
        let sh = shape(r#"{"method":"Page.loadEventFired","params":{"t":1},"sessionId":"P"}"#);
        assert_eq!((sh.id, sh.method, sh.session.map(|s| s.0)), (None, Some("Page.loadEventFired"), Some("P")));
        let sh = shape(r#"{"id":9,"method":"Runtime.evaluate","params":{"expression":"1"}}"#);
        assert_eq!((sh.id.map(|i| i.0), sh.method, sh.session), (Some(9), Some("Runtime.evaluate"), None));
        // Not in the shape: the router parses it.
        assert_eq!(shape(r#"{"result":{},"id":1}"#), Shape { id: None, method: None, session: None });
        // The edits.
        let text = r#"{"id":42,"result":{"x":1},"sessionId":"S1"}"#;
        let sh = shape(text);
        let (end, at) = (sh.id.unwrap().1, sh.session.unwrap().1);
        assert_eq!(with_id(&without_session(text, at), end, 5), r#"{"id":5,"result":{"x":1}}"#);
        assert_eq!(with_id(text, end, 5), r#"{"id":5,"result":{"x":1},"sessionId":"S1"}"#);
    }

    /// The router's string edits give what a parse and rewrite gave:
    /// the client's own id back, its root session stripped, a command
    /// with no session sent on the root.
    #[test]
    fn passed_on_messages_come_out_as_before() {
        let (mut r, mut chrome) = router();
        let (a_out, a) = mpsc::channel();
        r.new_client(1, Out::Channel(a_out));
        let attach = json(&chrome.try_recv().unwrap());
        r.on_chrome(&format!(r#"{{"id":{},"result":{{"sessionId":"RA"}}}}"#, attach["id"]));
        r.on_client(1, r#"{"id":11,"method":"Target.getTargets","params":{}}"#.into());
        let sent = json(&chrome.try_recv().unwrap());
        assert_eq!((sent["method"].as_str(), sent["sessionId"].as_str()), (Some("Target.getTargets"), Some("RA")));
        r.on_chrome(&format!(r#"{{"id":{},"result":{{"targetInfos":[]}},"sessionId":"RA"}}"#, sent["id"]));
        assert_eq!(received(&a), vec![json(r#"{"id":11,"result":{"targetInfos":[]}}"#)]);
        // A tab session's command keeps its session both ways.
        r.owners.insert("PA".into(), 1);
        r.on_client(1, r#"{"id":12,"method":"Runtime.evaluate","params":{"expression":"1"},"sessionId":"PA"}"#.into());
        let sent = json(&chrome.try_recv().unwrap());
        assert_eq!(sent["sessionId"], "PA");
        r.on_chrome(&format!(r#"{{"id":{},"result":{{"result":{{"value":1}}}},"sessionId":"PA"}}"#, sent["id"]));
        assert_eq!(received(&a), vec![json(r#"{"id":12,"result":{"result":{"value":1}},"sessionId":"PA"}"#)]);
    }

    #[test]
    fn the_launch_key_follows_what_makes_browsers_differ() {
        let spec = |exe: &str, headless, natural| Spec { exe: exe.into(), headless, natural };
        let a = spec("/usr/bin/chrome", true, false).key();
        assert_eq!(a, spec("/usr/bin/chrome", true, false).key());
        assert_ne!(a, spec("/usr/bin/chrome", false, false).key());
        assert_ne!(a, spec("/usr/bin/chrome", true, true).key());
        assert_ne!(a, spec("/opt/chrome", true, false).key());
        assert_eq!(a.len(), 16);
    }

    fn router() -> (Router, tokio::sync::mpsc::UnboundedReceiver<String>) {
        let (to_chrome, from_router) = tokio::sync::mpsc::unbounded_channel();
        let router = Router {
            to_chrome,
            next_id: 100,
            clients: HashMap::new(),
            pending: HashMap::new(),
            owners: HashMap::new(),
            idle_since: None,
            close_now: false,
        };
        (router, from_router)
    }

    fn json(text: &str) -> serde_json::Value {
        serde_json::from_str(text).unwrap()
    }

    fn received(rx: &mpsc::Receiver<Vec<u8>>) -> Vec<serde_json::Value> {
        rx.try_iter()
            .map(|mut b| {
                assert_eq!(b.pop(), Some(0));
                json(std::str::from_utf8(&b).unwrap())
            })
            .collect()
    }

    /// Two clients: each gets its own root session, sees only its own
    /// answers and events, can't address the other's sessions or close the
    /// browser, and leaves with its contexts disposed.
    #[test]
    fn the_router_keeps_clients_apart() {
        let (mut r, mut chrome) = router();
        let (a_out, a) = mpsc::channel();
        let (b_out, b) = mpsc::channel();
        r.new_client(1, Out::Channel(a_out));
        r.new_client(2, Out::Channel(b_out));
        let attach_a = json(&chrome.try_recv().unwrap());
        let attach_b = json(&chrome.try_recv().unwrap());
        assert_eq!(attach_a["method"], "Target.attachToBrowserTarget");
        // A message sent before the root exists waits for it.
        r.on_client(1, r#"{"id":1,"method":"Target.createBrowserContext","params":{}}"#.into());
        assert!(chrome.try_recv().is_err());
        r.on_chrome(&format!(r#"{{"id":{},"result":{{"sessionId":"RA"}}}}"#, attach_a["id"]));
        r.on_chrome(&format!(r#"{{"id":{},"result":{{"sessionId":"RB"}}}}"#, attach_b["id"]));
        let create = json(&chrome.try_recv().unwrap());
        assert_eq!(create["sessionId"], "RA");
        assert_eq!(create["method"], "Target.createBrowserContext");
        r.on_chrome(&format!(r#"{{"id":{},"result":{{"browserContextId":"CA"}},"sessionId":"RA"}}"#, create["id"]));
        assert_eq!(received(&a), vec![json(r#"{"id":1,"result":{"browserContextId":"CA"}}"#)]);
        assert!(received(&b).is_empty());

        // A's tab session is A's: its events go to A only, with the id.
        r.on_client(1, r#"{"id":2,"method":"Target.attachToTarget","params":{"targetId":"T"}}"#.into());
        let attach = json(&chrome.try_recv().unwrap());
        r.on_chrome(&format!(r#"{{"id":{},"result":{{"sessionId":"PA"}},"sessionId":"RA"}}"#, attach["id"]));
        r.on_chrome(r#"{"method":"Page.loadEventFired","params":{"timestamp":1},"sessionId":"PA"}"#);
        r.on_chrome(r#"{"method":"Target.targetCreated","params":{"targetInfo":{}},"sessionId":"RB"}"#);
        let got = received(&a);
        assert_eq!(got[1], json(r#"{"method":"Page.loadEventFired","params":{"timestamp":1},"sessionId":"PA"}"#));
        assert_eq!(received(&b), vec![json(r#"{"method":"Target.targetCreated","params":{"targetInfo":{}}}"#)]);

        // B may not use A's session, nor close the browser.
        r.on_client(2, r#"{"id":5,"method":"Runtime.evaluate","params":{},"sessionId":"PA"}"#.into());
        r.on_client(2, r#"{"id":6,"method":"Browser.close","params":{}}"#.into());
        assert!(chrome.try_recv().is_err());
        let refused = received(&b);
        assert_eq!(refused[0]["id"], 5);
        assert!(refused[0]["error"]["message"].as_str().unwrap().contains(crate::cdp::SESSION_GONE));
        assert_eq!(refused[1]["error"]["message"], NOT_CLOSED);

        // Frames auto-attached under A's tab are A's too.
        r.on_chrome(r#"{"method":"Target.attachedToTarget","params":{"sessionId":"FA"},"sessionId":"PA"}"#);
        assert_eq!(r.owners.get("FA"), Some(&1));

        // A leaves: its context is disposed, then its root detached.
        r.gone(1);
        let dispose = json(&chrome.try_recv().unwrap());
        assert_eq!(dispose["method"], "Target.disposeBrowserContext");
        assert_eq!(dispose["params"]["browserContextId"], "CA");
        assert!(dispose.get("sessionId").is_none());
        let detach = json(&chrome.try_recv().unwrap());
        assert_eq!(detach["params"]["sessionId"], "RA");
        assert!(!r.owners.values().any(|c| *c == 1));
        assert!(r.idle_since.is_none(), "B is still there");
        r.gone(2);
        assert!(r.idle_since.is_some());
    }
}
