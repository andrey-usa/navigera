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

#[cfg(unix)]
mod endpoint {
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
mod endpoint {
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
/// as-is, a bare name maps to `$TMPDIR/navigera-<name>.sock` (on
/// Windows `%TEMP%`; there the file holds the loopback address + token).
pub fn socket_path(name: &str) -> PathBuf {
    if name.contains('/') || (cfg!(windows) && name.contains('\\')) {
        PathBuf::from(name)
    } else {
        std::env::temp_dir().join(format!("navigera-{name}.sock"))
    }
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
    let path = socket_path(name);
    if connect(&path).is_ok() {
        eprintln!("navigera: a session is already listening on {}", path.display());
        return ExitCode::from(1);
    }
    // A socket file nobody answers on is left over from a crashed server.
    let _ = std::fs::remove_file(&path);

    let mut driver = match Driver::launch(config) {
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

    loop {
        let Ok(stream) = listener.accept() else { continue };
        last_activity.store(now_s(), Ordering::Relaxed);
        if serve_connection(&mut driver, stream, &last_activity) {
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

/// Serve one client connection; returns true once a `quit` was answered.
fn serve_connection(driver: &mut Driver, stream: Stream, last: &AtomicU64) -> bool {
    let reader = match stream.try_clone() {
        Ok(read_half) => BufReader::new(read_half),
        Err(_) => return false,
    };
    let mut writer = stream;
    for line in reader.lines() {
        let Ok(raw) = line else { return false };
        last.store(now_s(), Ordering::Relaxed);
        let Some((responses, is_quit)) = protocol::handle_line(driver, &raw) else {
            continue;
        };
        let written = responses.iter().try_for_each(|response| protocol::write_response(&mut writer, response, false));
        last.store(now_s(), Ordering::Relaxed);
        if is_quit {
            return true;
        }
        if written.is_err() {
            return false;
        }
    }
    false
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
        if let Ok(mut stream) = connect(&path) {
            let _ = stream.write_all(b"{\"op\":\"quit\"}\n");
            let _ = stream.flush();
            let mut reply = String::new();
            let _ = BufReader::new(stream).read_line(&mut reply);
        }
        break;
    });
}

/// Client: send one command to the session server and print its response.
pub fn client(config: &SessionConfig, name: &str, command: &Command, output: &mut dyn Write) -> ExitCode {
    let path = socket_path(name);
    let stream = match connect(&path) {
        Ok(stream) => stream,
        Err(e) => {
            let mut error = format!(
                "no navigera session at {} ({e}); start one with `navigera --session {name} start`",
                path.display()
            );
            // A session that ran before says why it stopped (idle timeout,
            // browser crash, quit) in its log, which outlives the socket.
            let log_path = path.with_extension("log");
            let tail = log_tail(&log_path, 5);
            if !tail.is_empty() {
                error.push_str(&format!("\nlast lines of {}:\n{tail}", log_path.display()));
            }
            print_json(output, &json!({ "id": null, "ok": false, "error": error }), config);
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
    if request.get("timeout_ms").is_none() && config.timeout_ms != crate::protocol::DEFAULT_TIMEOUT_MS {
        request["timeout_ms"] = json!(config.timeout_ms);
    }
}

/// The last `n` lines of a session log ("" when there is none).
fn log_tail(path: &std::path::Path, n: usize) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

fn fail(output: &mut dyn Write, config: &SessionConfig, error: String) -> ExitCode {
    print_json(output, &json!({ "id": null, "ok": false, "error": error }), config);
    ExitCode::from(1)
}

/// `start`: spawn a detached session server and wait until it accepts.
pub fn start(config: &SessionConfig, name: &str, output: &mut dyn Write) -> ExitCode {
    let path = socket_path(name);
    let log_path = path.with_extension("log");
    let (socket_str, log_str) = (path.display().to_string(), log_path.display().to_string());
    if connect(&path).is_ok() {
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
    let log = match std::fs::File::create(&log_path) {
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
    if config.headless {
        cmd.arg("--headless");
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
            print_json(
                output,
                &json!({ "id": null, "ok": true, "result": {
                    "session": name, "socket": socket_str, "log": log_str, "pid": child.id(),
                }}),
                config,
            );
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
        std::thread::sleep(Duration::from_millis(50));
    }
}
