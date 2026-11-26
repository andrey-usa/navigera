//! navigera runs Chrome's network service in the browser process
//! (`NetworkServiceInProcess2`: one process fewer) only where that service
//! would have had no sandbox of its own, and this asks the browser on the
//! machine the tests run on. Where navigera's launch keeps the service apart,
//! its process shows the sandbox it gets (`--service-sandbox-type=`); where
//! navigera puts it in the browser process, a second launch with the feature
//! taken out shows what it would have had, and that must be `none`.
//!
//! Chrome's default differs by platform, release and policy: macOS sandboxes
//! the service, Linux and Windows do when a field trial or an administrator
//! asks. So this runs on each machine and browser CI tests on, Edge too where
//! it is installed, and on CI it says what it found in a notice.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;

const FEATURE: &str = "NetworkServiceInProcess2";

/// The browsers to ask: the one navigera picks, and Edge where it is
/// installed (its default for this sandbox is its own).
fn browsers() -> Vec<String> {
    let found = navigera::browser::resolve_executable_for(None, true);
    assert!(
        found.is_some() || std::env::var_os("NAVIGERA_REQUIRE_BROWSER").is_none(),
        "NAVIGERA_REQUIRE_BROWSER is set but no Chrome/Chromium was found"
    );
    let edge: &[&str] = if cfg!(windows) {
        &[
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
            r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        ]
    } else if cfg!(target_os = "macos") {
        &["/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"]
    } else {
        &["/usr/bin/microsoft-edge", "/opt/microsoft/msedge/msedge"]
    };
    let mut all: Vec<String> = found.into_iter().collect();
    if let Some(edge) = edge.iter().find(|p| Path::new(p).exists()) {
        if !all.iter().any(|b| b == edge) {
            all.push(edge.to_string());
        }
    }
    all
}

/// What the test did and what its page server saw, in order, with the time
/// since the test started: a failure's message carries it, so a launch that
/// hangs on one machine in fifty says whether its request reached the server.
static TIMELINE: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn mark(what: String) {
    static START: OnceLock<Instant> = OnceLock::new();
    let at = START.get_or_init(Instant::now).elapsed().as_secs_f64();
    if let Ok(mut lines) = TIMELINE.lock() {
        lines.push(format!("{at:8.3}s {what}"));
    }
}

/// A page over HTTP, so the browser has fetched through its network service.
/// Each connection is read on a thread of its own: Edge 157 on Windows may
/// open a connection it sends nothing on and send the request on another,
/// and a server that read one connection at a time waited on the first
/// until navigera gave up on Page.navigate at 35 s (2 of 120 repeats on
/// windows-2025, 0 of 120 with a thread per connection: runs 45015728760
/// and 45015731300).
fn serve_page() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local port");
    let url = format!("http://{}/", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for (n, stream) in listener.incoming().enumerate() {
            let Ok(stream) = stream else { continue };
            let peer = stream.peer_addr().map(|a| a.port()).unwrap_or(0);
            mark(format!("server: connection {n} from port {peer} accepted"));
            std::thread::spawn(move || answer(n, stream));
        }
    });
    url
}

/// Read a request from connection `n` and answer it with the page.
fn answer(n: usize, mut stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let mut request = [0u8; 4096];
    let read = stream.read(&mut request);
    let got = *read.as_ref().unwrap_or(&0);
    let line = String::from_utf8_lossy(&request[..got]).lines().next().unwrap_or("").to_string();
    match read {
        Ok(0) => return mark(format!("server: connection {n} closed with nothing sent")),
        Ok(_) => mark(format!("server: connection {n} read {got} bytes: {line:?}")),
        Err(e) => return mark(format!("server: connection {n}: {e}")),
    }
    let body = "<!doctype html><title>network</title><p>network service</p>";
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// The browser's own list of its targets, asked over DevTools HTTP at the
/// endpoint `status` names, apart from navigera's connection.
fn devtools_list(endpoint: &str) -> String {
    let Some(addr) = endpoint.strip_prefix("ws://").and_then(|rest| rest.split('/').next()) else {
        return format!("(no ws:// endpoint: {endpoint:?})");
    };
    let Some(addr) = addr.to_socket_addrs().ok().and_then(|mut a| a.next()) else {
        return format!("(endpoint {addr:?} does not resolve)");
    };
    let mut stream = match TcpStream::connect_timeout(&addr, Duration::from_secs(3)) {
        Ok(s) => s,
        Err(e) => return format!("(connect {addr}: {e})"),
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = write!(stream, "GET /json/list HTTP/1.0\r\nHost: {addr}\r\n\r\n");
    let mut answer = Vec::new();
    let read = stream.read_to_end(&mut answer);
    let text = String::from_utf8_lossy(&answer);
    let body = text.split_once("\r\n\r\n").map_or(&*text, |(_, b)| b);
    let body: String = body.chars().filter(|c| !c.is_control()).take(3000).collect();
    match read {
        Ok(_) => body,
        Err(e) => format!("(read after {} bytes: {e}) {body}", answer.len()),
    }
}

/// The last `n` lines of a file, or why there are none.
fn tail(path: &Path, n: usize) -> String {
    match std::fs::read(path) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let lines: Vec<&str> = text.lines().collect();
            lines[lines.len().saturating_sub(n)..].join("\n")
        }
        Err(e) => format!("({e})"),
    }
}

/// The processes of the browser started with `marker`: (pid, command line).
fn browser_tree(all: &[(u32, u32, String)], marker: &str) -> Vec<(u32, String)> {
    let marked = |cmd: &str| cmd.split_whitespace().any(|arg| arg.trim_matches('"') == marker);
    let mut tree: Vec<u32> =
        all.iter().filter(|(_, _, cmd)| marked(cmd) && !cmd.contains("--type=")).map(|(pid, ..)| *pid).collect();
    let mut next = 0;
    while next < tree.len() {
        let parent = tree[next];
        let children: Vec<u32> =
            all.iter().filter(|(pid, ppid, _)| *ppid == parent && !tree.contains(pid)).map(|(pid, ..)| *pid).collect();
        tree.extend(children);
        next += 1;
    }
    all.iter().filter(|(pid, ..)| tree.contains(pid)).map(|(pid, _, cmd)| (*pid, cmd.clone())).collect()
}

/// (pid, parent pid, command line) of every process.
fn processes() -> Vec<(u32, u32, String)> {
    let out = if cfg!(windows) {
        Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Get-CimInstance Win32_Process | ForEach-Object { \"$($_.ProcessId) $($_.ParentProcessId) $($_.CommandLine)\" }",
            ])
            .output()
    } else {
        Command::new("ps").args(["-axww", "-o", "pid=,ppid=,command="]).output()
    };
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let field = |s: &str| -> (String, String) {
        let s = s.trim_start();
        let end = s.find(char::is_whitespace).unwrap_or(s.len());
        (s[..end].to_string(), s[end..].to_string())
    };
    text.lines()
        .filter_map(|line| {
            let (pid, rest) = field(line);
            let (ppid, cmd) = field(&rest);
            Some((pid.parse().ok()?, ppid.parse().ok()?, cmd.trim().to_string()))
        })
        .collect()
}

/// The value of `--service-sandbox-type=` in a command line.
fn sandbox_type(cmd: &str) -> Option<String> {
    let key = "--service-sandbox-type=";
    let rest = &cmd[cmd.find(key)? + key.len()..];
    Some(rest[..rest.find(|c: char| c.is_whitespace() || c == '"').unwrap_or(rest.len())].to_string())
}

/// Whether a command line's `--enable-features` names `feature`.
fn enables(cmd: &str, feature: &str) -> bool {
    cmd.split_whitespace()
        .filter_map(|arg| arg.trim_matches('"').strip_prefix("--enable-features="))
        .any(|list| list.split(',').any(|f| f == feature))
}

/// What one launch showed: the browser's command line, the sandbox type of
/// each network service process under it, and the page's user agent.
struct Seen {
    browser: String,
    network: Vec<String>,
    agent: String,
}

/// A session of navigera's on `exe` in a session directory of its own,
/// quit when dropped.
struct Launch {
    exe: String,
    marker: String,
    without_feature: bool,
    dir: tempfile::TempDir,
}

impl Launch {
    fn new(exe: &str, label: &str, without_feature: bool) -> Launch {
        Launch {
            exe: exe.to_string(),
            marker: format!("--nv-network-sandbox-test={label}-{}", std::process::id()),
            without_feature,
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn run(&self, args: &[&str]) -> Value {
        mark(format!("{}: navigera {args:?}", self.marker));
        let out = self.command(args).output().expect("run navigera");
        let stdout = String::from_utf8_lossy(&out.stdout);
        mark(format!("{}: navigera {args:?} answered (exit {:?})", self.marker, out.status.code()));
        let value =
            serde_json::from_str::<Value>(stdout.trim()).ok().filter(|v| out.status.success() && v["ok"] == true);
        let Some(value) = value else {
            panic!(
                "{} {args:?} failed: {stdout} / {}\n{}",
                self.exe,
                String::from_utf8_lossy(&out.stderr),
                self.diagnose()
            );
        };
        value
    }

    /// navigera's command for this launch's session.
    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_navigera"));
        if cfg!(windows) {
            cmd.env("TEMP", self.dir.path()).env("TMP", self.dir.path());
        } else {
            cmd.env("XDG_RUNTIME_DIR", self.dir.path());
        }
        let extra = std::env::var("NAVIGERA_EXTRA_FLAGS").map(|e| format!("{e} ")).unwrap_or_default();
        cmd.env("NAVIGERA_CHROMIUM", &self.exe)
            .env("NAVIGERA_EXTRA_FLAGS", format!("{extra}{}", self.marker))
            .env_remove("NAVIGERA_DROP_FLAGS")
            .env_remove("NAVIGERA_SESSION")
            .env_remove("NAVIGERA_SHARED_BROWSER")
            .stdin(Stdio::null());
        if self.without_feature {
            // The whole `--enable-features=NetworkServiceInProcess2,…` flag;
            // `seen` checks that the feature is gone.
            cmd.env("NAVIGERA_DROP_FLAGS", format!("--enable-features={FEATURE}"));
        }
        // The commands navigera sends, for a failure's message (unless a CI
        // job already collects them in a file of its own).
        if std::env::var_os("NAVIGERA_CDP_TRACE").is_none() {
            cmd.env("NAVIGERA_CDP_TRACE", self.dir.path().join("cdp-trace.jsonl"));
        }
        cmd.env("NAVIGERA_VERBOSE", "1");
        cmd.arg("-s").arg("net").args(args);
        cmd
    }

    /// What there is to read when a command failed, before the session's
    /// directory goes: the timeline, the browser's own view of its tabs,
    /// navigera's status and tabs, the browser's processes, the last CDP
    /// commands navigera sent and the session's logs.
    fn diagnose(&self) -> String {
        ps_snapshot(&self.marker);
        let mut out = String::from("---- diagnostics\ntimeline:\n");
        if let Ok(lines) = TIMELINE.lock() {
            out += &lines.join("\n");
        }
        let quick = |args: &[&str]| {
            let out = self.command(args).output();
            out.map(|o| String::from_utf8_lossy(&o.stdout).trim().chars().take(2000).collect::<String>())
                .unwrap_or_else(|e| format!("({e})"))
        };
        let status = quick(&["status"]);
        let endpoint = serde_json::from_str::<Value>(&status)
            .ok()
            .and_then(|v| v["result"]["endpoint"].as_str().map(str::to_string))
            .unwrap_or_default();
        out += &format!("\nstatus: {status}\ndevtools /json/list: {}", devtools_list(&endpoint));
        out += &format!("\ntab-list: {}", quick(&["tab-list"]));
        out += "\nbrowser processes:";
        for (pid, cmd) in browser_tree(&processes(), &self.marker) {
            let kind =
                cmd.split_whitespace().find(|a| a.starts_with("--type=") || a.starts_with("--utility-sub-type="));
            let kind = kind.unwrap_or(if cmd.contains("--type=") { "?" } else { "browser" });
            out += &format!("\n  {pid} {kind}");
        }
        out += &format!("\nlast CDP commands sent:\n{}", tail(&self.dir.path().join("cdp-trace.jsonl"), 15));
        let mut logs: Vec<_> =
            walk(self.dir.path()).into_iter().filter(|p| p.extension().is_some_and(|e| e == "log")).collect();
        logs.sort();
        for log in logs.iter().take(6) {
            out += &format!("\n{}:\n{}", log.display(), tail(log, 40));
        }
        out
    }

    /// Start the browser, open `url` and read the process list.
    fn seen(&self, url: &str) -> Seen {
        self.run(&["start", "--own-browser"]);
        self.run(&["goto", url]);
        let agent = self.run(&["eval", "navigator.userAgent"])["result"].as_str().unwrap_or_default().to_string();
        let all = processes();
        // The whole argument: `…-123` is the start of another run's `…-1234`.
        let marked = |cmd: &str| cmd.split_whitespace().any(|arg| arg.trim_matches('"') == self.marker);
        let main: Vec<&(u32, u32, String)> =
            all.iter().filter(|(_, _, cmd)| marked(cmd) && !cmd.contains("--type=")).collect();
        assert_eq!(main.len(), 1, "{}: one browser process carries {}: {main:?}", self.exe, self.marker);
        let browser = &main[0].2;
        let network = browser_tree(&all, &self.marker)
            .into_iter()
            .filter(|(_, cmd)| cmd.contains("network.mojom.NetworkService"))
            .map(|(_, cmd)| sandbox_type(&cmd).unwrap_or_else(|| "(no --service-sandbox-type)".into()))
            .collect();
        if self.without_feature {
            assert!(
                !enables(browser, FEATURE),
                "NAVIGERA_DROP_FLAGS=--enable-features={FEATURE} no longer takes the feature out of the launch \
                 (the flag starts differently now): change how this test leaves it out. {browser}"
            );
        }
        Seen { browser: browser.clone(), network, agent }
    }
}

impl Drop for Launch {
    fn drop(&mut self) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_navigera"));
        if cfg!(windows) {
            cmd.env("TEMP", self.dir.path()).env("TMP", self.dir.path());
        } else {
            cmd.env("XDG_RUNTIME_DIR", self.dir.path());
        }
        let _ = cmd.args(["-s", "net", "quit"]).stdin(Stdio::null()).output();
        // A repeat harness keeps every launch's session logs, passed or not.
        if let Some(keep) = std::env::var_os("NV_KEEP_DIR") {
            let to = Path::new(&keep).join(self.marker.trim_start_matches('-'));
            let _ = std::fs::create_dir_all(&to);
            for file in walk(self.dir.path()) {
                if let Some(name) = file.file_name().filter(|_| file.extension().is_some_and(|e| e == "log")) {
                    let _ = std::fs::copy(&file, to.join(name));
                }
            }
        }
    }
}

/// The whole process table with state, age and wait channel, for a launch
/// that hangs: saved beside the session logs when a repeat harness asks.
fn ps_snapshot(marker: &str) {
    let Some(keep) = std::env::var_os("NV_KEEP_DIR") else { return };
    if !cfg!(target_os = "linux") {
        return;
    }
    let out =
        Command::new("ps").args(["-ww", "-eo", "pid,ppid,pgid,stat,etimes,time,wchan:32,args", "--forest"]).output();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_else(|e| format!("({e})"));
    let _ = std::fs::write(Path::new(&keep).join(format!("ps-{}.txt", marker.trim_start_matches('-'))), text);
}

/// The files under `dir`, two levels down at most.
fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let mut dirs = vec![(dir.to_path_buf(), 0)];
    while let Some((dir, depth)) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() && depth < 2 => dirs.push((path, depth + 1)),
                Ok(t) if t.is_file() => files.push(path),
                _ => {}
            }
        }
    }
    files
}

/// `Chrome/158`, `Edg/142`: the product tokens of a user agent.
fn product(agent: &str) -> String {
    agent
        .split_whitespace()
        .filter(|t| t.starts_with("Chrome/") || t.starts_with("HeadlessChrome/") || t.starts_with("Edg/"))
        .map(|t| t.split('.').next().unwrap_or(t))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A line GitHub shows on the run's page and returns through its checks API.
fn notice(text: &str) {
    if std::env::var_os("GITHUB_ACTIONS").is_some() {
        // Straight to the process's stdout: the test harness captures only
        // `print!`, and a passing test's captured output is dropped.
        let _ = writeln!(std::io::stdout().lock(), "::notice title=network service sandbox::{text}");
    }
}

#[test]
fn the_network_service_runs_in_the_browser_process_only_where_it_would_have_no_sandbox() {
    let browsers = browsers();
    if browsers.is_empty() {
        return;
    }
    let url = serve_page();
    for (i, exe) in browsers.into_iter().enumerate() {
        let ours = Launch::new(&exe, &format!("{i}-navigera"), false);
        let seen = ours.seen(&url);
        let name = format!("{} ({exe})", product(&seen.agent));
        if !enables(&seen.browser, FEATURE) {
            // Kept apart: the service's process shows the browser's own choice.
            assert_eq!(seen.network.len(), 1, "{name}: one network service process when navigera keeps it apart");
            notice(&format!("{name}: kept in a process of its own, sandbox {}", seen.network[0]));
            continue;
        }
        assert!(
            seen.network.is_empty(),
            "{name}: {FEATURE} is on, yet the network service has a process of its own ({:?})",
            seen.network
        );
        drop(ours);
        let apart = Launch::new(&exe, &format!("{i}-apart"), true);
        let would = apart.seen(&url);
        assert_eq!(would.network.len(), 1, "{name}: one network service process without {FEATURE}: {}", would.browser);
        notice(&format!(
            "{name}: in the browser process; without {FEATURE} its process has sandbox {}",
            would.network[0]
        ));
        assert_eq!(
            would.network[0], "none",
            "{name}: without {FEATURE} the browser sandboxes its network service (--service-sandbox-type={}), \
             and navigera runs it in the browser process, outside that sandbox. Leave the feature out for this \
             browser and platform (`ENABLE_FEATURES` and `chrome_flags` in src/browser.rs).",
            would.network[0]
        );
    }
}
