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
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Stdio};

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

/// A page over HTTP, so the browser has fetched through its network service.
fn serve_page() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local port");
    let url = format!("http://{}/", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            let body = "<!doctype html><title>network</title><p>network service</p>";
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    url
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
        let out = cmd.arg("-s").arg("net").args(args).output().expect("run navigera");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let value: Value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|_| panic!("{args:?}: not JSON: {stdout} / {}", String::from_utf8_lossy(&out.stderr)));
        assert!(out.status.success() && value["ok"] == true, "{} {args:?} failed: {value}", self.exe);
        value
    }

    /// Start the browser, open `url` and read the process list.
    fn seen(&self, url: &str) -> Seen {
        self.run(&["start", "--own-browser"]);
        self.run(&["goto", url]);
        let agent = self.run(&["eval", "navigator.userAgent"])["result"].as_str().unwrap_or_default().to_string();
        let all = processes();
        let main: Vec<&(u32, u32, String)> =
            all.iter().filter(|(_, _, cmd)| cmd.contains(&self.marker) && !cmd.contains("--type=")).collect();
        assert_eq!(main.len(), 1, "{}: one browser process carries {}: {main:?}", self.exe, self.marker);
        let (root, _, browser) = main[0];
        let mut tree = vec![*root];
        let mut next = 0;
        while next < tree.len() {
            let parent = tree[next];
            let children: Vec<u32> = all
                .iter()
                .filter(|(pid, ppid, _)| *ppid == parent && !tree.contains(pid))
                .map(|(pid, ..)| *pid)
                .collect();
            tree.extend(children);
            next += 1;
        }
        let network = all
            .iter()
            .filter(|(pid, _, cmd)| tree.contains(pid) && cmd.contains("network.mojom.NetworkService"))
            .map(|(_, _, cmd)| sandbox_type(cmd).unwrap_or_else(|| "(no --service-sandbox-type)".into()))
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
    }
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
