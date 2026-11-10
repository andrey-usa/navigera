//! The README's Quick start, run as it is written: each `$ navigera …` line
//! of its console block, its output (stdout and stderr) compared with the
//! lines under it. The one change: `https://example.com` is served from a
//! local page with the same title, so the test needs no network, and its
//! address is read back as `https://example.com` in what the commands print.
//! The session's `log`, `pid` and `socket` are masked: they change from run
//! to run and from machine to machine.
//! The session name is kept; a private session directory keeps it apart
//! from any other session called `work`.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_navigera");
const SITE: &str = "https://example.com";

fn chrome() -> bool {
    let found = navigera::browser::resolve_executable_for(None, false).is_some();
    assert!(
        found || std::env::var_os("NAVIGERA_REQUIRE_BROWSER").is_none(),
        "NAVIGERA_REQUIRE_BROWSER is set but no Chrome/Chromium was found"
    );
    found
}

/// A JSON line with its run-specific values (`log`, `pid`, `socket`) masked;
/// any other line as it is.
fn mask(line: &str) -> String {
    match serde_json::from_str::<Value>(line) {
        Ok(Value::Object(mut fields)) => {
            for key in ["log", "pid", "socket"] {
                if fields.contains_key(key) {
                    fields.insert(key.into(), Value::String(format!("<{key}>")));
                }
            }
            Value::Object(fields).to_string()
        }
        _ => line.to_string(),
    }
}

/// A page like example.com's at a local address, for as long as the test runs.
fn serve() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    std::thread::spawn(move || {
        let page = "<!doctype html><html><head><title>Example Domain</title></head>\
                    <body><h1>Example Domain</h1></body></html>";
        for mut stream in listener.incoming().flatten() {
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buf[..n]),
                }
            }
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
                page.len()
            );
        }
    });
    base
}

/// A session directory of the test's own (`$XDG_RUNTIME_DIR` on Linux and
/// macOS, `%TEMP%` on Windows), so `-s work` is this test's session alone.
fn private_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nv-readme-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn navigera(dir: &Path, args: &[String]) -> Command {
    let mut cmd = Command::new(BIN);
    cmd.args(args)
        .env_remove("NAVIGERA_OUTPUT")
        .env("XDG_RUNTIME_DIR", dir)
        .env("TEMP", dir)
        .env("TMP", dir)
        .stdin(Stdio::null());
    cmd
}

/// Stops the session's browser however the test ends.
struct Quit<'a>(&'a Path);

impl Drop for Quit<'_> {
    fn drop(&mut self) {
        let _ = navigera(self.0, &["-s".into(), "work".into(), "quit".into()]).output();
        let _ = std::fs::remove_dir_all(self.0);
    }
}

#[test]
fn the_readme_quick_start_runs_as_written() {
    if !chrome() {
        eprintln!("skipped: no Chrome/Chromium found");
        return;
    }
    let readme = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"))
        .unwrap()
        // A Windows checkout has CRLF line ends.
        .replace("\r\n", "\n");
    let section = &readme[readme.find("\n## Quick start\n").expect("README.md has a Quick start")..];
    let open = section.find("```console\n").expect("the Quick start has a console block") + "```console\n".len();
    let block = &section[open..open + section[open..].find("```").unwrap()];
    let lines: Vec<&str> = block.lines().collect();

    let base = serve();
    let dir = private_dir();
    let _quit = Quit(&dir);
    let (mut i, mut ran) = (0, 0);
    while i < lines.len() {
        let command = lines[i].strip_prefix("$ ").unwrap_or_else(|| panic!("not a command: {}", lines[i]));
        i += 1;
        let start = i;
        while i < lines.len() && !lines[i].starts_with("$ ") {
            i += 1;
        }
        let expected: Vec<String> = lines[start..i].iter().map(|l| mask(l)).collect();
        let mut words = command.split_whitespace();
        assert_eq!(words.next(), Some("navigera"), "the Quick start runs navigera only: {command}");
        let args: Vec<String> = words.map(|w| w.replace(SITE, &base)).collect();
        let out = navigera(&dir, &args).output().unwrap();
        ran += 1;
        let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
        let got: Vec<String> = text.lines().map(|l| mask(&l.replace(&base, SITE))).collect();
        assert!(out.status.success(), "README.md's Quick start: `{command}` failed:\n{text}");
        assert_eq!(got, expected, "README.md's Quick start: `{command}` printed {got:?}, not {expected:?}");
    }
    assert!(ran >= 2, "the Quick start ran {ran} command(s)");
}
