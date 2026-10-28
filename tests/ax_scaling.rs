//! `ax` on a long page must cost what it prints, not the page size: the
//! text tree is fetched lazily to the output limit, so a bounded `ax` on
//! 8000 list items is far quicker than the whole-tree read (`--limit 0`)
//! on the same page, and only a few times slower than on 2000 items even
//! though the page is four times larger (the rest is the whole-page
//! DOMSnapshot behind the clickable refs, which is linear in the page).
//!
//! Bounds are generous for CI timing noise: measured on sandbox
//! headless-shell 156, lazy/full at 8000 items was about 3.7 s / 9–15 s
//! and lazy 8000/2000 about 3.6x.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

fn tool_exe() -> String {
    env!("CARGO_BIN_EXE_navigera").to_string()
}

fn chrome() -> bool {
    navigera::browser::resolve_executable_for(None, true).is_some()
}

struct Site {
    child: Child,
    url: String,
}

impl Drop for Site {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn site() -> Option<Site> {
    let python = if cfg!(windows) { "python" } else { "python3" };
    let mut child = Command::new(python)
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ax_scaling_site.py"))
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .ok()?;
    let mut line = String::new();
    BufReader::new(child.stdout.take()?).read_line(&mut line).ok()?;
    let url = line.trim().strip_prefix("LISTENING ")?.to_string();
    Some(Site { child, url })
}

struct Session {
    name: String,
    _downloads: Option<tempfile::TempDir>,
}

impl Session {
    fn start(test: &str) -> Session {
        let name = std::env::temp_dir()
            .join(format!("nv-ax-scaling-{test}-{}.sock", std::process::id()))
            .display()
            .to_string();
        let downloads = tempfile::tempdir().unwrap();
        let dir = downloads.path().display().to_string();
        let s = Session { name, _downloads: Some(downloads) };
        s.ok(&["start", "--download-dir", &dir]);
        s
    }

    fn run(&self, args: &[&str]) -> (bool, Value) {
        let out = Command::new(tool_exe())
            .arg("--session")
            .arg(&self.name)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("run navigera");
        let text = String::from_utf8_lossy(&out.stdout);
        let value = serde_json::from_str(text.trim()).unwrap_or_else(|_| {
            panic!(
                "stdout must be one JSON object for {args:?}: {text}\nstderr: {}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
        (out.status.success(), value)
    }

    fn ok(&self, args: &[&str]) -> Value {
        let (success, r) = self.run(args);
        assert!(success && r["ok"] == true, "{args:?} failed: {r}");
        r["result"].clone()
    }

    /// Time one `ax` run; `--raw` prints the tree as text, not JSON, so
    /// this reads stdout directly instead of going through `ok`.
    fn timed_ax(&self, args: &[&str]) -> (Duration, String) {
        let mut full = vec!["--raw", "ax"];
        full.extend_from_slice(args);
        let started = Instant::now();
        let out = Command::new(tool_exe())
            .arg("--session")
            .arg(&self.name)
            .args(&full)
            .stdin(Stdio::null())
            .output()
            .expect("run navigera");
        let took = started.elapsed();
        assert!(out.status.success(), "{full:?} failed: {}", String::from_utf8_lossy(&out.stderr));
        (took, String::from_utf8_lossy(&out.stdout).to_string())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = Command::new(tool_exe()).arg("--session").arg(&self.name).arg("quit").output();
    }
}

#[test]
fn ax_on_a_long_page_costs_what_it_prints() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("long-page");
    s.ok(&["goto", &format!("{}/list?n=8000", site.url)]);
    let (lazy_8000, tree) = s.timed_ax(&[]);
    assert!(tree.contains("Item 0"), "ax must print the first items");
    let (full_8000, _) = s.timed_ax(&["--limit", "0"]);
    assert!(
        lazy_8000 < full_8000.mul_f64(0.75),
        "bounded ax on 8000 items ({lazy_8000:?}) must beat the whole-tree read ({full_8000:?})"
    );
    s.ok(&["goto", &format!("{}/list?n=2000", site.url)]);
    let (lazy_2000, _) = s.timed_ax(&[]);
    assert!(
        lazy_8000 < lazy_2000.mul_f64(5.0),
        "ax on 8000 items ({lazy_8000:?}) must stay within 5x of 2000 items ({lazy_2000:?})"
    );
}
