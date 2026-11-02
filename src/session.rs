//! Named background sessions: one warm browser behind a local socket,
//! driven by one shell command per op.
//!
//! Why this exists: `serve` keeps a browser warm, but only for as long as the
//! caller holds its stdin open. An AI agent driving a shell runs each tool
//! call as a separate command, so it cannot keep that pipe alive between
//! calls — which is exactly why agents end up writing Python/Node wrapper
//! scripts around `serve`. A session server owns the browser instead:
//!
//! ```text
//! navigera --session s start              # detached server, returns at once
//! navigera --session s goto --url https://example.com
//! navigera --session s ax                 # compact a11y view with refs
//! navigera --session s click --ref 12
//! navigera --session s quit               # browser + server shut down
//! ```
//!
//! The wire format is the `serve` protocol verbatim (one JSON command per
//! line, one JSON response per line), so any client that can talk to
//! `serve` can talk to a session too.
//!
//! The socket: a Unix socket on Linux/macOS. On Windows a loopback TCP
//! port whose address and a random token sit in the session file (in the
//! user's own temp dir); a client must send the token as its first line, so
//! nothing that can only reach the port (a web page posting to localhost,
//! another user) can drive the browser.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{ExitCode, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::protocol::{self, Command, Driver, SessionConfig};

use endpoint::{connect, Listener, Stream};

/// The sockets sessions (and the shared-browser host) listen on: a Unix
/// socket, or on Windows a loopback port whose address and token sit in a
/// file.
#[cfg(unix)]
pub(crate) use self::endpoint_unix as endpoint;
#[cfg(windows)]
pub(crate) use self::endpoint_windows as endpoint;

#[cfg(unix)]
pub(crate) mod endpoint_unix {
    use std::io;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;

    pub type Stream = UnixStream;

    pub struct Listener(UnixListener);

    pub fn connect(path: &Path) -> io::Result<Stream> {
        UnixStream::connect(path)
    }

    impl Listener {
        pub fn bind(path: &Path) -> io::Result<Listener> {
            UnixListener::bind(path).map(Listener)
        }

        pub fn accept(&self) -> io::Result<Stream> {
            self.0.accept().map(|(stream, _)| stream)
        }
    }
}

#[cfg(windows)]
pub(crate) mod endpoint_windows {
    use std::io::{self, Read, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::path::Path;
    use std::time::Duration;

    pub type Stream = TcpStream;

    pub struct Listener {
        inner: TcpListener,
        token: String,
    }

    /// The session file holds `127.0.0.1:<port> <token>`.
    fn read_session_file(path: &Path) -> io::Result<(SocketAddr, String)> {
        let text = std::fs::read_to_string(path)?;
        let bad = || io::Error::new(io::ErrorKind::InvalidData, "malformed session file");
        let (addr, token) = text.trim().split_once(' ').ok_or_else(bad)?;
        Ok((addr.parse().map_err(|_| bad())?, token.to_string()))
    }

    pub fn connect(path: &Path) -> io::Result<Stream> {
        let (addr, token) = read_session_file(path)?;
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
        stream.set_nodelay(true)?;
        stream.write_all(format!("{token}\n").as_bytes())?;
        Ok(stream)
    }

    /// 128 random bits as hex: std's `RandomState` keys come from the OS
    /// RNG, so SipHash under them is unpredictable to anyone else.
    fn random_token() -> String {
        use std::hash::{BuildHasher, Hasher};
        let nanos =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        (0..2u64)
            .map(|i| {
                let mut h = std::collections::hash_map::RandomState::new().build_hasher();
                h.write_u64(i);
                h.write_u128(nanos);
                h.write_u32(std::process::id());
                format!("{:016x}", h.finish())
            })
            .collect()
    }

    impl Listener {
        pub fn bind(path: &Path) -> io::Result<Listener> {
            let inner = TcpListener::bind("127.0.0.1:0")?;
            let token = random_token();
            // Write-then-rename: a client never reads a half-written file.
            let tmp = path.with_extension("sock.tmp");
            std::fs::write(&tmp, format!("{} {token}\n", inner.local_addr()?))?;
            std::fs::rename(&tmp, path)?;
            Ok(Listener { inner, token })
        }

        /// Next client that proves it read the session file.
        pub fn accept(&self) -> io::Result<Stream> {
            let (mut stream, _) = self.inner.accept()?;
            stream.set_nodelay(true)?;
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
            // Byte by byte: anything after the token line is the first
            // command and must stay in the socket for the line reader.
            let mut line = Vec::with_capacity(40);
            let mut byte = [0u8; 1];
            while line.len() <= 128 {
                if stream.read(&mut byte)? == 0 || byte[0] == b'\n' {
                    break;
                }
                line.push(byte[0]);
            }
            if line.strip_suffix(b"\r").unwrap_or(&line[..]) != self.token.as_bytes() {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "bad session token"));
            }
            stream.set_read_timeout(None)?;
            Ok(stream)
        }
    }
}

/// Socket path for a session: a value containing a path separator is used
/// as-is, a bare name maps to `<session dir>/<name>.sock` (see
/// [`session_dir`]; on Windows the file holds the loopback address + token).
pub fn socket_path(name: &str) -> std::io::Result<PathBuf> {
    if name.contains('/') || (cfg!(windows) && name.contains('\\')) {
        Ok(PathBuf::from(name))
    } else {
        Ok(session_dir()?.join(format!("{name}.sock")))
    }
}

/// Where named sessions keep their socket and log, private to this user.
///
/// A socket in a shared directory (`/tmp`) under a predictable name can be
/// squatted: another user binds it first and every command (and every value
/// `fill` types) goes to them. So on Linux/macOS the directory is
/// `$XDG_RUNTIME_DIR/navigera`, or `$TMPDIR/navigera-<uid>`, created 0700,
/// and refused if it is a symlink, not ours, or open to others. Windows'
/// `%TEMP%` is already per user.
#[cfg(unix)]
pub fn session_dir() -> std::io::Result<PathBuf> {
    extern "C" {
        fn geteuid() -> u32;
    }
    // SAFETY: geteuid has no preconditions and cannot fail.
    let uid = unsafe { geteuid() };
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_absolute() && p.is_dir());
    let dir = match runtime {
        Some(base) => base.join("navigera"),
        None => std::env::temp_dir().join(format!("navigera-{uid}")),
    };
    ensure_private_dir(&dir, uid)?;
    Ok(dir)
}

/// A directory in the temp dir only this user can enter (`$TMPDIR/navigera-<uid>`,
/// 0700, checked as [`session_dir`]'s is), for files too big for
/// `$XDG_RUNTIME_DIR`: the default downloads. Windows' `%TEMP%` is per user.
#[cfg(unix)]
pub fn private_temp_dir() -> std::io::Result<PathBuf> {
    extern "C" {
        fn geteuid() -> u32;
    }
    // SAFETY: geteuid has no preconditions and cannot fail.
    let uid = unsafe { geteuid() };
    let dir = std::env::temp_dir().join(format!("navigera-{uid}"));
    ensure_private_dir(&dir, uid)?;
    Ok(dir)
}

#[cfg(not(unix))]
pub fn private_temp_dir() -> std::io::Result<PathBuf> {
    Ok(std::env::temp_dir())
}

/// Create `dir` 0700, or accept an existing one only if it is a real
/// directory owned by `uid` that no one else can enter.
#[cfg(unix)]
fn ensure_private_dir(dir: &std::path::Path, uid: u32) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = std::fs::symlink_metadata(dir)?;
    let refuse = |why: String| {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("refusing session directory {}: {why}; remove it, or pass a socket path", dir.display()),
        ))
    };
    if !meta.file_type().is_dir() {
        return refuse("not a directory (a symlink?)".into());
    }
    if meta.uid() != uid {
        return refuse(format!("owned by uid {}, not {uid}", meta.uid()));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        return refuse(format!("mode {:o} lets other users in", meta.permissions().mode() & 0o777));
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn session_dir() -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join("navigera");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Create (truncate) a session log without following a symlink planted at
/// its path.
pub(crate) fn create_log(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Linux's value differs by architecture; macOS/BSD use 0x100.
        const O_NOFOLLOW: i32 = if !cfg!(target_os = "linux") {
            0x100
        } else if cfg!(any(target_arch = "aarch64", target_arch = "arm")) {
            0o100000
        } else {
            0o400000
        };
        options.custom_flags(O_NOFOLLOW).mode(0o600);
    }
    options.open(path)
}

/// `$NAVIGERA_SESSION`, if set: the default session for client calls.
pub fn env_session() -> Option<String> {
    std::env::var("NAVIGERA_SESSION").ok().filter(|s| !s.trim().is_empty())
}

fn now_s() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn print_json(output: &mut dyn Write, value: &Value, config: &SessionConfig) {
    protocol::print_cli_response(output, value, config);
}

/// Session server: launch the browser, then serve the line protocol on the
/// socket, one connection at a time, until `quit` or the idle timeout.
pub fn serve_socket(config: &SessionConfig, name: &str) -> ExitCode {
    let path = match socket_path(name) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("navigera: {e}");
            return ExitCode::from(1);
        }
    };
    if connect(&path).is_ok() {
        eprintln!("navigera: a session is already listening on {}", path.display());
        return ExitCode::from(1);
    }
    // A socket file nobody answers on is left over from a crashed server.
    let _ = std::fs::remove_file(&path);

    let mut driver = match Driver::launch_session(config) {
        Ok(driver) => driver,
        Err(e) => {
            eprintln!("navigera: {e:#}");
            return ExitCode::from(1);
        }
    };
    let listener = match Listener::bind(&path) {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("navigera: bind {}: {e}", path.display());
            driver.session().close();
            return ExitCode::from(1);
        }
    };
    eprintln!("navigera: session listening on {} ({})", path.display(), driver.session().endpoint());

    let last_activity = Arc::new(AtomicU64::new(now_s()));
    if config.idle_timeout_s > 0 {
        spawn_idle_watchdog(path.clone(), Arc::clone(&last_activity), config.idle_timeout_s);
    }
    let parent = config.parent_pid.and_then(crate::proc::ParentWatch::new);
    match (config.parent_pid, &parent) {
        (Some(pid), None) => {
            eprintln!("navigera: process {pid} (--parent) is not running");
            driver.session().close();
            let _ = std::fs::remove_file(&path);
            return ExitCode::from(1);
        }
        (Some(pid), Some(_)) => eprintln!("navigera: the session quits when process {pid} (--parent) exits"),
        _ => {}
    }
    let server = ServerInfo { pid: std::process::id(), started_at: now_s(), parent: config.parent_pid };
    if let Some(parent) = parent {
        spawn_parent_watchdog(path.clone(), parent);
    }

    // Connections are taken as they come and served one at a time; one
    // that came in while another was being served gets a note (two
    // callers on one session name drive one browser).
    let (connections, incoming) = std::sync::mpsc::channel::<(Stream, Instant)>();
    std::thread::spawn(move || loop {
        let Ok(stream) = listener.accept() else { continue };
        if connections.send((stream, Instant::now())).is_err() {
            return;
        }
    });
    // When the last connection that ran a command was done: one that only
    // connected (`start` checking the socket is up) kept no one waiting.
    let mut free_since = Instant::now();
    let mut callers = Callers::default();
    loop {
        // Between commands: a storage snapshot an op skipped for being too
        // soon after the last one is taken once the session is idle.
        let (stream, arrived) = match incoming.recv_timeout(Duration::from_millis(250)) {
            Ok(next) => next,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                driver.snapshot_if_idle();
                continue;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let waited = arrived < free_since;
        let (quit, ran) = serve_connection(&mut driver, stream, &last_activity, &server, waited, &mut callers);
        if ran {
            free_since = Instant::now();
        }
        if quit {
            break;
        }
        // The shared browser went away: this session's error said so, and
        // `start` makes a new one (on a relaunched browser).
        if driver.shared_browser_gone() {
            eprintln!("navigera: the shared browser is gone; shutting the session down");
            break;
        }
    }
    driver.session().close();
    crate::timing::report();
    // Log first: a client that finds the socket gone quotes this line.
    eprintln!("navigera: session {} shut down", path.display());
    let _ = std::fs::remove_file(&path);
    ExitCode::SUCCESS
}

/// What `status` adds about the session server itself.
struct ServerInfo {
    pid: u32,
    started_at: u64,
    parent: Option<u32>,
}

/// The note a command gets when it came in while another client's command
/// was running on the session.
pub const SHARED_NAME_NOTE: &str = "another command was running on this session when this one came in: a session \
     name is shared by every process that uses it, so two agents on one name drive one browser (give each its own -s name)";

/// Serve one client connection; returns whether a `quit` was answered and
/// whether it ran any command. `waited`: it came in while another
/// connection's command was running.
fn serve_connection(
    driver: &mut Driver,
    stream: Stream,
    last: &AtomicU64,
    server: &ServerInfo,
    waited: bool,
    callers: &mut Callers,
) -> (bool, bool) {
    let reader = match stream.try_clone() {
        Ok(read_half) => BufReader::new(read_half),
        Err(_) => return (false, false),
    };
    let mut ran = false;
    let mut writer = stream;
    let mut waited = waited;
    for line in reader.lines() {
        let Ok(raw) = line else { return (false, ran) };
        // `sessions` asks for status without making the session busy.
        let (is_status, probe, caller) = match serde_json::from_str::<Value>(&raw) {
            Ok(v) => (
                v["op"] == "status",
                v["op"] == "status" && v[PROBE] == true,
                v[CALLER].as_str().filter(|_| v["op"] != "status" && v["op"] != "quit").map(str::to_string),
            ),
            Err(_) => (false, false, None),
        };
        let caller_note = caller.as_deref().and_then(|c| callers.before(c, driver));
        let idle_s = now_s().saturating_sub(last.load(Ordering::Relaxed));
        if !probe {
            last.store(now_s(), Ordering::Relaxed);
        }
        let Some((mut responses, is_quit)) = protocol::handle_line(driver, &raw) else {
            continue;
        };
        // A `sessions` probe is no one's command either.
        ran |= !probe;
        if let Some(c) = caller.as_deref() {
            let url = responses.first().and_then(|r| r.result.as_ref()).and_then(|r| r["url"].as_str());
            callers.after(c, url);
        }
        if let Some(first) = responses.first_mut() {
            if waited {
                first.notes.get_or_insert_with(Vec::new).push(SHARED_NAME_NOTE.into());
                waited = false;
            } else if let Some(note) = caller_note {
                first.notes.get_or_insert_with(Vec::new).push(note);
            }
            if let (true, Some(result)) = (is_status, first.result.as_mut()) {
                result["server"] = json!({
                    "pid": server.pid,
                    "started_at": server.started_at,
                    "idle_s": idle_s,
                    "parent": server.parent,
                });
            }
        }
        let written = responses.iter().try_for_each(|response| protocol::write_response(&mut writer, response, false));
        if !probe {
            last.store(now_s(), Ordering::Relaxed);
        }
        if is_quit {
            return (true, ran);
        }
        if written.is_err() {
            return (false, ran);
        }
    }
    (false, ran)
}

/// After `idle_s` without traffic, send ourselves a `quit` over the socket so
/// the browser shuts down through the normal path.
fn spawn_idle_watchdog(path: PathBuf, last: Arc<AtomicU64>, idle_s: u64) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(idle_s.clamp(1, 5)));
        if now_s().saturating_sub(last.load(Ordering::Relaxed)) < idle_s {
            continue;
        }
        eprintln!("navigera: idle for {idle_s}s, shutting the session down");
        quit_session(&path);
        break;
    });
}

/// `--parent`: once that process exits, the session quits (an agent that
/// crashed leaves no session behind).
fn spawn_parent_watchdog(path: PathBuf, parent: crate::proc::ParentWatch) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(500));
        if parent.alive() {
            continue;
        }
        eprintln!("navigera: process {} (--parent) exited, shutting the session down", parent.pid());
        quit_session(&path);
        break;
    });
}

/// Send a session a `quit` and wait for its answer.
fn quit_session(path: &std::path::Path) -> bool {
    let Ok(mut stream) = connect(path) else { return false };
    let _ = stream.write_all(b"{\"op\":\"quit\"}\n");
    let _ = stream.flush();
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply).is_ok_and(|n| n > 0)
}

/// Client: send one command to the session server and print its response.
/// `batch` on a session: every step over one connection, in order, each
/// answered before the next is sent; the first failure stops the rest
/// unless `keep_going`.
pub fn batch(
    config: &SessionConfig,
    name: &str,
    steps: &[crate::protocol::BatchStep],
    keep_going: bool,
    output: &mut dyn Write,
) -> ExitCode {
    let started = Instant::now();
    let path = match socket_path(name) {
        Ok(path) => path,
        Err(e) => return fail(output, config, e.to_string()),
    };
    let stream = match connect(&path) {
        Ok(stream) => stream,
        Err(e) => {
            return fail(
                output,
                config,
                format!(
                    "no navigera session at {} ({e}); start one with `{} -s {name} start`",
                    path.display(),
                    crate::cli::program()
                ),
            )
        }
    };
    let mut reader = match stream.try_clone() {
        Ok(read_half) => BufReader::new(read_half),
        Err(e) => return fail(output, config, format!("session connection: {e}")),
    };
    let caller = caller_id();
    let mut responses = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        let mut request = match serde_json::to_value(&step.command) {
            Ok(value) => value,
            Err(e) => return fail(output, config, format!("encode step {}: {e}", i + 1)),
        };
        request["id"] = json!(i + 1);
        if let Some(caller) = &caller {
            request[CALLER] = json!(caller);
        }
        localize_request(&mut request, config);
        let mut writer = &stream;
        if writer.write_all(format!("{request}\n").as_bytes()).and_then(|_| writer.flush()).is_err() {
            responses.push(json!({ "id": i + 1, "ok": false, "error": "session closed the connection" }));
            break;
        }
        let mut line = String::new();
        let _ = reader.read_line(&mut line);
        let response: Value = serde_json::from_str(line.trim()).unwrap_or_else(|_| {
            json!({ "id": i + 1, "ok": false, "error": "session ended without a response (see its log next to the socket)" })
        });
        let failed = response["ok"] != true;
        responses.push(response);
        if (failed && !keep_going) || matches!(step.command, Command::Quit { .. }) {
            break;
        }
    }
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let response = protocol::batch_response(steps, &responses, started.elapsed().as_secs_f64() * 1000.0);
    if protocol::print_batch(output, &response, config) {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

pub fn client(config: &SessionConfig, name: &str, command: &Command, output: &mut dyn Write) -> ExitCode {
    let path = match socket_path(name) {
        Ok(path) => path,
        Err(e) => return fail(output, config, e.to_string()),
    };
    let stream = match connect(&path) {
        Ok(stream) => stream,
        Err(e) => {
            let mut error = format!(
                "no navigera session at {} ({e}); start one with `{} -s {name} start`",
                path.display(),
                crate::cli::program()
            );
            // A session that ran before says why it stopped (idle timeout,
            // browser crash, quit) in its log, which outlives the socket.
            let log_path = path.with_extension("log");
            let tail = log_tail(&log_path, 5);
            if !tail.is_empty() {
                error.push_str(&format!("\nlast lines of {}:\n{tail}", log_path.display()));
            }
            print_json(
                output,
                &json!({ "id": null, "ok": false, "error": crate::protocol::plain_error(&error) }),
                config,
            );
            return ExitCode::from(1);
        }
    };
    let mut request = match serde_json::to_value(command) {
        Ok(value) => value,
        Err(e) => {
            print_json(output, &json!({ "id": null, "ok": false, "error": format!("encode command: {e}") }), config);
            return ExitCode::from(1);
        }
    };
    request["id"] = json!(1);
    if let Some(caller) = caller_id() {
        request[CALLER] = json!(caller);
    }
    if config.compact {
        request["compact"] = json!(true);
    }
    localize_request(&mut request, config);
    let mut writer = &stream;
    // One write: on Windows (TCP) a split line would wait out Nagle.
    if writer.write_all(format!("{request}\n").as_bytes()).and_then(|_| writer.flush()).is_err() {
        print_json(output, &json!({ "id": null, "ok": false, "error": "session closed the connection" }), config);
        return ExitCode::from(1);
    }
    // Done sending: the server sees EOF after answering and takes the next
    // client, so a crashed client can't wedge the session.
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut line = String::new();
    let _ = BufReader::new(&stream).read_line(&mut line);
    let response: Value = match serde_json::from_str(line.trim()) {
        Ok(value) => value,
        Err(_) => json!({
            "id": null,
            "ok": false,
            "error": "session ended without a response (see its log next to the socket)",
        }),
    };
    if matches!(command, Command::Quit { .. }) {
        // Return only once the server has let go of the socket, so a `start`
        // right after `quit` gets a fresh session, not the dying one.
        let deadline = Instant::now() + Duration::from_secs(10);
        while path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    if protocol::print_cli_response(output, &response, config) {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// The session server runs in another directory (wherever `start` ran), so
/// file paths a client passes are made absolute in the client's own working
/// directory first: `upload ./doc.txt` and `screenshot shot.png` mean the
/// caller's files. A global `--timeout-ms` given to a client call applies to
/// that op (the server's own default was fixed at `start`).
fn localize_request(request: &mut Value, config: &SessionConfig) {
    let cwd = std::env::current_dir().unwrap_or_default();
    let absolute = |p: &str| -> String {
        let path = std::path::Path::new(p);
        if path.is_absolute() {
            p.to_string()
        } else {
            cwd.join(path).display().to_string()
        }
    };
    match request.get("op").and_then(Value::as_str) {
        Some("upload") => {
            if let Some(files) = request.get_mut("files").and_then(Value::as_array_mut) {
                for f in files.iter_mut() {
                    if let Some(p) = f.as_str() {
                        *f = json!(absolute(p));
                    }
                }
            }
        }
        Some("screenshot") => {
            let path = match request.get("path").and_then(Value::as_str) {
                Some(p) => absolute(p),
                None => absolute(&format!(
                    "shot-{}.png",
                    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
                )),
            };
            request["path"] = json!(path);
        }
        _ => {}
    }
    // Files the server writes for the caller: relative to the caller.
    for key in ["har", "body_file", "path"] {
        if let Some(p) = request.get(key).and_then(Value::as_str) {
            request[key] = json!(absolute(p));
        }
    }
    if request.get("timeout_ms").is_none() && config.timeout_ms != crate::protocol::DEFAULT_TIMEOUT_MS {
        request["timeout_ms"] = json!(config.timeout_ms);
    }
}

/// The last `n` lines of a session log ("" when there is none).
pub(crate) fn log_tail(path: &std::path::Path, n: usize) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// The engine a running session server reports (`status`), if it answers.
fn running_engine(stream: Stream) -> Option<String> {
    let mut writer = &stream;
    writer.write_all(b"{\"id\":1,\"op\":\"status\"}\n").ok()?;
    writer.flush().ok()?;
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line).ok()?;
    let response: Value = serde_json::from_str(line.trim()).ok()?;
    response.pointer("/result/engine").and_then(Value::as_str).map(str::to_string)
}

fn fail(output: &mut dyn Write, config: &SessionConfig, error: String) -> ExitCode {
    print_json(output, &json!({ "id": null, "ok": false, "error": crate::protocol::plain_error(&error) }), config);
    ExitCode::from(1)
}

/// What `start` tells an agent when a decision model is on.
pub const DECIDER_NOTE: &str = "a decision model is on: act with `click --find \"<what>\"` or \
     `fill --find \"<what>\" <value>` (any element command takes --find) and verify with \
     `check \"<yes/no question>\"`, instead of reading `ax` for refs; read `ax` only for text you must report, \
     or when --find says it is unsure.";

/// `start`: spawn a detached session server and wait until it accepts.
pub fn start(config: &SessionConfig, name: &str, output: &mut dyn Write) -> ExitCode {
    let path = match socket_path(name) {
        Ok(path) => path,
        Err(e) => return fail(output, config, e.to_string()),
    };
    let log_path = path.with_extension("log");
    let (socket_str, log_str) = (path.display().to_string(), log_path.display().to_string());
    if let Ok(stream) = connect(&path) {
        // The running session must be the browser this `start` asks for: a
        // Chrome command sent to a Lightpanda session left over from another
        // run would quietly test the wrong engine.
        let running = running_engine(stream);
        let wanted = if crate::browser::is_lightpanda(&config.engine) { "lightpanda" } else { "chrome" };
        if let Some(running) = running.filter(|r| r != wanted) {
            return fail(
                output,
                config,
                format!(
                    "session {name} is already running the {running} engine, not {wanted}; `navigera --session {name} quit` first, or pick another --session"
                ),
            );
        }
        print_json(
            output,
            &json!({ "id": null, "ok": true, "result": {
                "session": name, "socket": socket_str, "log": log_str, "already_running": true,
            }}),
            config,
        );
        return ExitCode::SUCCESS;
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return fail(output, config, format!("locate navigera binary: {e}")),
    };
    let log = match create_log(&log_path) {
        Ok(file) => file,
        Err(e) => return fail(output, config, format!("create {}: {e}", log_path.display())),
    };

    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--session")
        .arg(name)
        .arg("--engine")
        .arg(&config.engine)
        .arg("--timeout-ms")
        .arg(config.timeout_ms.to_string())
        .arg("--idle-timeout-s")
        .arg(config.idle_timeout_s.to_string());
    if config.headed {
        cmd.arg("--headed");
    }
    if let Some(chromium) = &config.chromium {
        cmd.arg("--chromium").arg(chromium);
    }
    if let Some(transport) = &config.transport {
        cmd.arg("--transport").arg(transport);
    }
    if let Some(spec) = &config.attach {
        cmd.arg(format!("--attach={spec}"));
    }
    if let Some(name) = &config.profile {
        cmd.arg("--profile").arg(name);
    }
    if let Some(app) = &config.electron {
        cmd.arg("--electron").arg(app);
    }
    for arg in &config.electron_args {
        cmd.arg(format!("--electron-arg={arg}"));
    }
    if config.headless {
        cmd.arg("--headless");
    }
    if let Some(from) = &config.from {
        cmd.arg("--from").arg(from);
    }
    if let Some(from_profile) = &config.from_profile {
        cmd.arg("--from-profile").arg(from_profile);
    }
    if config.natural {
        cmd.arg("--natural");
    }
    if let Some(ms) = config.wait_ms {
        cmd.arg("--wait-ms").arg(ms.to_string());
    }
    if let Some(dir) = &config.download_dir {
        cmd.arg("--download-dir").arg(dir);
    }
    if config.own_browser {
        cmd.arg("--own-browser");
    }
    let parent = config
        .parent_pid
        .or_else(|| std::env::var("NAVIGERA_PARENT_PID").ok().and_then(|v| v.trim().parse().ok()).filter(|p| *p > 0));
    if let Some(pid) = parent {
        if !crate::proc::pid_alive(pid) {
            return fail(output, config, format!("--parent {pid}: no process {pid} is running"));
        }
        cmd.arg("--parent").arg(pid.to_string());
    }
    cmd.arg("serve").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::from(log));
    let mut child = match crate::proc::spawn_detached(&mut cmd) {
        Ok(child) => child,
        Err(e) => return fail(output, config, format!("spawn session server: {e}")),
    };

    // Attaching to the user's Chrome waits for them to click Allow.
    let wait = if config.attach.is_some() { 180 } else { 60 };
    if config.attach.is_some() {
        eprintln!("navigera: connecting to your browser; if Chrome asks, click Allow");
    }
    let deadline = Instant::now() + Duration::from_secs(wait);
    loop {
        if connect(&path).is_ok() {
            let result = json!({ "session": name, "socket": socket_str, "log": log_str, "pid": child.id() });
            // The server inherited this environment. Saying so lets an agent
            // know `find`/`check` work without reading a key variable (and
            // printing the key into its own context).
            let mut response = json!({ "id": null, "ok": true, "result": result });
            // An Electron app: its Electron version, and whether `eval --main`
            // can reach its main process.
            if config.electron.is_some() || config.attach.is_some() {
                if let Probe::Ready(status) = probe(&path) {
                    if let Some(electron) = status.get("electron").filter(|e| e.is_object()) {
                        response["result"]["electron"] = electron.clone();
                        if electron["main_process"] == false {
                            let note =
                                if config.attach.is_some() { ELECTRON_ATTACHED_NOTE } else { ELECTRON_NO_MAIN_NOTE };
                            response["notes"] = json!([note]);
                        }
                    }
                }
            }
            if let Some(decider) = crate::decide::Decider::from_env() {
                response["result"]["decider"] = json!(decider.model());
                // The contract, where the agent decides how to proceed: a
                // skill section further down is read once, if at all.
                response["notes"] = json!([DECIDER_NOTE]);
            }
            print_json(output, &response, config);
            return ExitCode::SUCCESS;
        }
        if let Ok(Some(status)) = child.try_wait() {
            let tail = log_tail(&log_path, 20);
            return fail(output, config, format!("session server exited during startup ({status}):\n{tail}"));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return fail(
                output,
                config,
                format!("session server did not listen within {wait}s; see {}", log_path.display()),
            );
        }
        // A failed connect costs microseconds, and the server is usually up
        // within a few hundred ms: checking every 5 ms keeps `start` from
        // waiting out a 50 ms step after it is ready. Slower after that.
        let step = if Instant::now() + Duration::from_secs(wait - 3) < deadline { 5 } else { 50 };
        std::thread::sleep(Duration::from_millis(step));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    #[test]
    fn session_dir_is_created_private_and_refused_when_not() {
        let base = tempfile::tempdir().unwrap();
        let uid = std::fs::metadata(base.path()).unwrap().uid();

        let fresh = base.path().join("fresh");
        ensure_private_dir(&fresh, uid).unwrap();
        assert_eq!(std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777, 0o700);
        ensure_private_dir(&fresh, uid).expect("an existing private dir is fine");

        let open = base.path().join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
        let err = ensure_private_dir(&open, uid).unwrap_err().to_string();
        assert!(err.contains("lets other users in"), "{err}");

        let link = base.path().join("link");
        std::os::unix::fs::symlink(&fresh, &link).unwrap();
        let err = ensure_private_dir(&link, uid).unwrap_err().to_string();
        assert!(err.contains("not a directory"), "{err}");

        let err = ensure_private_dir(&fresh, uid + 1).unwrap_err().to_string();
        assert!(err.contains("owned by uid"), "{err}");
    }

    #[test]
    fn a_log_path_that_is_a_symlink_is_not_followed() {
        let base = tempfile::tempdir().unwrap();
        let victim = base.path().join("victim");
        std::fs::write(&victim, "keep me").unwrap();
        let log = base.path().join("s.log");
        std::os::unix::fs::symlink(&victim, &log).unwrap();
        assert!(create_log(&log).is_err());
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
        let plain = base.path().join("plain.log");
        create_log(&plain).unwrap();
        assert_eq!(std::fs::metadata(&plain).unwrap().permissions().mode() & 0o777, 0o600);
    }
}

/// Marks a `status` from `sessions`: it does not count as using the
/// session (an idle one stays idle).
const PROBE: &str = "probe";

/// `start --electron` on an app whose main process can't be reached.
const ELECTRON_ATTACHED_NOTE: &str = "an Electron app navigera attached to: its windows are the tabs, quit \
     leaves it running (quit --close-browser ends it), and eval --main needs the launch with --electron <app>";

const ELECTRON_NO_MAIN_NOTE: &str = "this app turns Node's inspector off (Electron's EnableNodeCliInspectArguments \
     fuse), so eval --main can't reach its main process; its windows work as usual";

/// The request field naming who sent a command (see [`caller_id`]).
const CALLER: &str = "caller";

/// Who is calling: `$NAVIGERA_CALLER`, else (Linux) the first process up
/// the tree that is not a shell, a wrapper (`timeout`, `env`, ...) or
/// navigera's own launcher (the npm or Python shim): the agent that runs
/// its commands through a fresh shell each time stays one caller. Its pid
/// and start time, so a reused pid is someone else. None elsewhere.
fn caller_id() -> Option<String> {
    if let Some(v) = std::env::var("NAVIGERA_CALLER").ok().filter(|v| !v.trim().is_empty()) {
        return Some(v);
    }
    linux_caller()
}

#[cfg(target_os = "linux")]
fn linux_caller() -> Option<String> {
    const PASS: &[&str] = &[
        "sh", "bash", "dash", "zsh", "fish", "ksh", "mksh", "tcsh", "csh", "busybox", "env", "timeout", "nice",
        "nohup", "stdbuf", "setsid", "xargs", "time", "sudo", "su", "doas", "script", "flock", "ionice", "taskset",
        "chrt",
    ];
    let mut pid = std::os::unix::process::parent_id();
    for _ in 0..12 {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // `pid (comm) state ppid ...`: comm may hold spaces and parens.
        let (head, rest) = stat.rsplit_once(')')?;
        let comm = head.split_once('(')?.1;
        let fields: Vec<&str> = rest.split_whitespace().collect();
        let ppid: u32 = fields.get(1)?.parse().ok()?;
        let started = fields.get(19).copied().unwrap_or("0");
        let launcher = matches!(comm, "node" | "python" | "python3" | "deno" | "bun")
            && std::fs::read(format!("/proc/{pid}/cmdline"))
                .map(|c| String::from_utf8_lossy(&c).contains("navigera"))
                .unwrap_or(false);
        if !PASS.contains(&comm) && !launcher {
            return Some(format!("{comm} {pid}/{started}"));
        }
        if ppid <= 1 {
            return None;
        }
        pid = ppid;
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn linux_caller() -> Option<String> {
    None
}

/// Who used the session, for telling a caller that another one has been
/// at it: each caller's tab URL after its last command, and the callers
/// another one has run commands after since.
#[derive(Default)]
struct Callers {
    last: Option<(String, Instant)>,
    url_after: std::collections::HashMap<String, String>,
    overtaken: std::collections::HashSet<String>,
}

impl Callers {
    /// The note for `caller`'s command, if another caller has used the
    /// session since its last one (or recently, for a caller new here).
    fn before(&mut self, caller: &str, driver: &Driver) -> Option<String> {
        if self.overtaken.remove(caller) {
            let before = self.url_after.get(caller).cloned().unwrap_or_default();
            let now = driver.session().url();
            return Some(if !before.is_empty() && !now.is_empty() && before != now {
                format!(
                    "another process used this session since your last command and moved its tab: {before} -> {now} \
                     (a session name is shared by every process that uses it: give each agent its own -s name)"
                )
            } else {
                "another process used this session since your last command (a session name is shared by every \
                 process that uses it: give each agent its own -s name)"
                    .to_string()
            });
        }
        match &self.last {
            Some((other, at))
                if other != caller
                    && !self.url_after.contains_key(caller)
                    && at.elapsed() < Duration::from_secs(600) =>
            {
                Some(format!(
                    "another process used this session {} s ago (a session name is shared by every process that uses \
                     it: give each agent its own -s name)",
                    at.elapsed().as_secs()
                ))
            }
            _ => None,
        }
    }

    /// `caller`'s command ran, leaving its tab at `url`.
    fn after(&mut self, caller: &str, url: Option<&str>) {
        for other in self.url_after.keys() {
            if other != caller {
                self.overtaken.insert(other.clone());
            }
        }
        if let Some(url) = url {
            self.url_after.insert(caller.to_string(), url.to_string());
        }
        self.last = Some((caller.to_string(), Instant::now()));
    }
}

/// What a session server answers `status` with, or why it doesn't.
enum Probe {
    Ready(Value),
    /// Connected, no answer in time: a command is running.
    Busy,
    /// Nobody listens: a file left by a server that died.
    Stale,
}

fn probe(path: &std::path::Path) -> Probe {
    let Ok(stream) = connect(path) else { return Probe::Stale };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut writer = &stream;
    let request = format!("{{\"id\":1,\"op\":\"status\",\"{PROBE}\":true}}\n");
    if writer.write_all(request.as_bytes()).and_then(|_| writer.flush()).is_err() {
        return Probe::Busy;
    }
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut line = String::new();
    match BufReader::new(&stream).read_line(&mut line) {
        Ok(n) if n > 0 => match serde_json::from_str::<Value>(line.trim()) {
            Ok(v) => Probe::Ready(v["result"].clone()),
            Err(_) => Probe::Busy,
        },
        _ => Probe::Busy,
    }
}

fn mib(kb: u64) -> f64 {
    (kb as f64 / 1024.0 * 10.0).round() / 10.0
}

/// The shared browsers' hosts: (socket, host pid).
fn shared_hosts(dir: &std::path::Path) -> Vec<(PathBuf, u32)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|x| x == "sock")
                && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("browser-"))
        })
        .filter_map(|p| {
            let pid = std::fs::read_to_string(p.with_extension("pid")).ok()?.trim().parse().ok()?;
            crate::proc::pid_alive(pid).then_some((p, pid))
        })
        .collect()
}

/// `sessions`: every named session in the session directory (name, server
/// pid, age, idle time, browser, memory) and the shared browsers. With
/// `clean`: quit the sessions idle for `idle_s` (default 300) or more, or
/// every one with `all`; drop files no server answers on; close shared
/// browsers left without a session; say how much memory that freed.
pub fn sessions(
    config: &SessionConfig,
    clean: bool,
    all: bool,
    idle_s: Option<u64>,
    output: &mut dyn Write,
) -> ExitCode {
    let dir = match session_dir() {
        Ok(dir) => dir,
        Err(e) => return fail(output, config, e.to_string()),
    };
    let threshold = idle_s.unwrap_or(300);
    let table = crate::proc::process_table();
    let hosts = shared_hosts(&dir);
    let host_pids: Vec<u32> = hosts.iter().map(|(_, pid)| *pid).collect();
    let mut listed = Vec::new();
    let (mut quit, mut removed) = (Vec::new(), Vec::new());
    let mut before_kb = 0;
    let mut paths: Vec<PathBuf> =
        std::fs::read_dir(&dir).map(|entries| entries.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    paths.sort();
    for path in paths {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        let Some(name) = name.strip_suffix(".sock") else { continue };
        if name.starts_with("browser-") {
            continue;
        }
        match probe(&path) {
            Probe::Stale => {
                if clean {
                    let _ = std::fs::remove_file(&path);
                    removed.push(name.to_string());
                } else {
                    listed.push(
                        json!({ "name": name, "state": "stale: no server answers (`sessions --clean` removes it)" }),
                    );
                }
            }
            Probe::Busy => listed.push(json!({ "name": name, "state": "busy: a command is running" })),
            Probe::Ready(status) => {
                let server = &status["server"];
                let pid = server["pid"].as_u64().unwrap_or(0) as u32;
                let kb = if pid > 0 { crate::proc::tree_kb(&table, pid, &host_pids) } else { 0 };
                before_kb += kb;
                let idle = server["idle_s"].as_u64().unwrap_or(0);
                let mut entry = json!({
                    "name": name,
                    "state": "ready",
                    "pid": pid,
                    "age_s": now_s().saturating_sub(server["started_at"].as_u64().unwrap_or(now_s())),
                    "idle_s": idle,
                    "browser": status["browser"],
                    "url": status["url"],
                    "memory_mib": mib(kb),
                });
                if let Some(parent) = server["parent"].as_u64() {
                    entry["parent"] = json!(parent);
                }
                if clean && (all || idle >= threshold) && quit_session(&path) {
                    let deadline = Instant::now() + Duration::from_secs(10);
                    while path.exists() && Instant::now() < deadline {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    quit.push(name.to_string());
                } else {
                    listed.push(entry);
                }
            }
        }
    }
    let mut browsers = Vec::new();
    for (socket, pid) in &hosts {
        let kb = crate::proc::tree_kb(&table, *pid, &[]);
        before_kb += kb;
        browsers.push(json!({ "socket": socket.display().to_string(), "pid": pid, "memory_mib": mib(kb) }));
    }
    let mut result = json!({ "sessions": listed, "shared_browsers": browsers, "memory_mib": mib(before_kb) });
    if clean {
        // A shared browser no listed session uses any more closes now,
        // not after its grace period.
        let left_shared = listed.iter().any(|s| s["browser"].as_str().is_some_and(|b| b.starts_with("shared")));
        if !left_shared {
            for (socket, pid) in &hosts {
                crate::shared::close_if_unused(socket);
                let deadline = Instant::now() + Duration::from_secs(5);
                while crate::proc::pid_alive(*pid) && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
        // What is left, measured again.
        std::thread::sleep(Duration::from_millis(300));
        let table = crate::proc::process_table();
        let mut after_kb: u64 = listed
            .iter()
            .filter_map(|s| s["pid"].as_u64())
            .map(|p| crate::proc::tree_kb(&table, p as u32, &host_pids))
            .sum();
        let mut browsers = Vec::new();
        for (socket, pid) in shared_hosts(&dir) {
            let kb = crate::proc::tree_kb(&table, pid, &[]);
            after_kb += kb;
            browsers.push(json!({ "socket": socket.display().to_string(), "pid": pid, "memory_mib": mib(kb) }));
        }
        result["sessions"] = json!(listed);
        result["shared_browsers"] = json!(browsers);
        result["quit"] = json!(quit);
        result["removed"] = json!(removed);
        result["memory_mib"] = json!(mib(after_kb));
        result["freed_mib"] = json!(mib(before_kb.saturating_sub(after_kb)));
    }
    print_json(output, &json!({ "id": null, "ok": true, "result": result }), config);
    ExitCode::SUCCESS
}
