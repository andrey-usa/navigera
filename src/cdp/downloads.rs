//! Files the page downloads: the browser saves each one under its download
//! id in a directory navigera chose (`Browser.setDownloadBehavior`
//! `allowAndName`), and reports it starting and finishing; a finished file
//! is renamed to the name the page suggested.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

#[derive(Clone, Debug)]
pub struct Download {
    pub n: u64,
    pub guid: String,
    pub url: String,
    /// The file name the page suggested (`Content-Disposition`, `<a download>`).
    pub suggested: String,
    /// `inProgress`, `completed` or `canceled`.
    pub state: String,
    pub received: u64,
    pub total: u64,
    /// Where it was saved, once complete (none when the browser keeps it,
    /// as with `--attach`).
    pub path: Option<PathBuf>,
    /// The scope (browser context) of the tab that started it.
    pub scope: String,
    /// The directory the browser saves it in (its scope's, when it has
    /// its own).
    pub dir: Option<PathBuf>,
    /// A `wait --download` answered with it (see [`next_for_wait`]).
    pub taken: bool,
}

/// What `wait --download` makes of the downloads so far (oldest first).
#[derive(Debug, PartialEq)]
pub enum Wait {
    /// This one finished: the answer.
    Done(usize),
    /// This one was canceled: the wait fails with it.
    Canceled(usize),
    /// None yet, or the next one is still coming.
    Pending,
}

/// The error of a wait that reached a canceled download.
pub fn canceled_text(d: &Download) -> String {
    format!("download {} was canceled ({}); the next `wait --download` takes the one after it", d.suggested, d.url)
}

/// The error of a wait that saw no download end in time: what it saw, and
/// which of those earlier waits already took.
pub fn none_text(items: &[Download], glob: &str, wait: std::time::Duration) -> String {
    let seen: Vec<String> = items
        .iter()
        .map(|d| {
            let state = match d.state.as_str() {
                "inProgress" => "in progress",
                other => other,
            };
            if d.taken {
                format!("{} ({state}, taken by an earlier wait)", d.suggested)
            } else {
                format!("{} ({state})", d.suggested)
            }
        })
        .collect();
    format!(
        "no finished download{} within {} ms (seen: {})",
        if glob.is_empty() { String::new() } else { format!(" matching {glob:?}") },
        wait.as_millis(),
        if seen.is_empty() { "none".to_string() } else { seen.join(", ") }
    )
}

/// `wait --download <glob>`: the oldest download whose file name matches
/// that no earlier wait took, once it ends. Taking downloads in the order
/// they began (not the newest finished one) gives each wait its own file:
/// a click, a wait, a second click and a second wait answer with the second
/// file even while an earlier one sits finished in the list, and an earlier
/// canceled one fails only the wait that reaches it. The caller marks the
/// answer `taken`.
pub fn next_for_wait(items: &[Download], glob: &str) -> Wait {
    let at = items.iter().position(|d| !d.taken && (glob.is_empty() || crate::cdp::glob_match(glob, &d.suggested)));
    match at {
        Some(i) if items[i].state == "completed" => Wait::Done(i),
        Some(i) if items[i].state == "canceled" => Wait::Canceled(i),
        _ => Wait::Pending,
    }
}

impl Download {
    pub fn to_json(&self) -> Value {
        json!({
            "n": self.n,
            "file": self.suggested,
            "path": self.path.as_ref().map(|p| p.display().to_string()),
            "bytes": if self.state == "completed" { self.received.max(self.total) } else { self.received },
            "state": match self.state.as_str() { "inProgress" => "in progress", other => other },
            "url": self.url,
        })
    }
}

#[derive(Default)]
pub struct Downloads {
    /// Where files go; `None` when the browser's own setting is kept.
    pub dir: Option<PathBuf>,
    /// A browser context's own directory (`downloads --dir`, a fork's or a
    /// `reset`'s new context), by scope; others use `dir`.
    pub dirs: std::collections::HashMap<String, PathBuf>,
    pub items: Vec<Download>,
    next: u64,
}

impl Downloads {
    /// `scope`: the scope of the tab the download started in (see
    /// `events::Scope`), for `downloadWillBegin`.
    pub fn on_event(&mut self, method: &str, params: &Value, scope: &str) {
        let guid = params.get("guid").and_then(Value::as_str).unwrap_or("").to_string();
        match method {
            "Browser.downloadWillBegin" => {
                self.next += 1;
                let suggested = params.get("suggestedFilename").and_then(Value::as_str).unwrap_or("download");
                self.items.push(Download {
                    n: self.next,
                    guid,
                    url: params.get("url").and_then(Value::as_str).unwrap_or("").to_string(),
                    suggested: safe_name(suggested),
                    state: "inProgress".to_string(),
                    received: 0,
                    total: 0,
                    path: None,
                    scope: scope.to_string(),
                    dir: self.dirs.get(scope).or(self.dir.as_ref()).cloned(),
                    taken: false,
                });
                if self.items.len() > 1000 {
                    self.items.remove(0);
                }
            }
            "Browser.downloadProgress" => {
                let taken: Vec<PathBuf> = self.items.iter().filter_map(|d| d.path.clone()).collect();
                let Some(d) = self.items.iter_mut().find(|d| d.guid == guid) else {
                    return;
                };
                d.received = params.get("receivedBytes").and_then(Value::as_f64).unwrap_or(0.0) as u64;
                d.total = params.get("totalBytes").and_then(Value::as_f64).unwrap_or(0.0) as u64;
                let state = params.get("state").and_then(Value::as_str).unwrap_or("inProgress");
                if state == "completed" && d.state != "completed" {
                    if let Some(dir) = d.dir.clone() {
                        d.path = Some(settle(&dir, &d.guid, &d.suggested, &taken));
                    }
                }
                d.state = state.to_string();
            }
            _ => {}
        }
    }
}

/// Rename `<dir>/<guid>` to the suggested name (`name (2).ext` if taken).
/// Left under its id when the rename fails.
fn settle(dir: &Path, guid: &str, suggested: &str, taken: &[PathBuf]) -> PathBuf {
    let from = dir.join(guid);
    let (stem, ext) = match suggested.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (suggested.to_string(), String::new()),
    };
    for i in 1..1000 {
        let name = if i == 1 { suggested.to_string() } else { format!("{stem} ({i}){ext}") };
        let to = dir.join(&name);
        if to.exists() || taken.contains(&to) {
            continue;
        }
        return match std::fs::rename(&from, &to) {
            Ok(()) => to,
            Err(_) => from,
        };
    }
    from
}

/// A file name the page can't use to leave the directory.
fn safe_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name).trim();
    let clean: String = base
        .chars()
        .map(|c| if c.is_control() || matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect();
    if clean.is_empty() || clean == "." || clean == ".." {
        "download".to_string()
    } else {
        clean
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_wait_takes_the_oldest_download_no_wait_took() {
        let d = |n: u64, name: &str, state: &str| Download {
            n,
            guid: format!("g{n}"),
            url: String::new(),
            suggested: name.into(),
            state: state.into(),
            received: 0,
            total: 0,
            path: None,
            scope: String::new(),
            dir: None,
            taken: false,
        };
        let mut items = vec![d(1, "a.csv", "completed"), d(2, "b.csv", "inProgress")];
        assert_eq!(next_for_wait(&items, "*.csv"), Wait::Done(0));
        items[0].taken = true;
        // The finished a.csv is not the answer again: b.csv is waited for.
        assert_eq!(next_for_wait(&items, "*.csv"), Wait::Pending);
        items[1].state = "completed".into();
        assert_eq!(next_for_wait(&items, "*.csv"), Wait::Done(1));
        // A glob skips the others; a canceled one fails only its own wait.
        let mut items = vec![d(1, "c.csv", "canceled"), d(2, "r.pdf", "completed"), d(3, "b.csv", "inProgress")];
        assert_eq!(next_for_wait(&items, "*.pdf"), Wait::Done(1));
        assert_eq!(next_for_wait(&items, "*.csv"), Wait::Canceled(0));
        items[0].taken = true;
        assert_eq!(next_for_wait(&items, "*.csv"), Wait::Pending);
        assert_eq!(next_for_wait(&[], ""), Wait::Pending);
    }

    #[test]
    fn names_stay_inside_the_directory() {
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name("a\\b\\report.csv"), "report.csv");
        assert_eq!(safe_name(".."), "download");
        assert_eq!(safe_name("q?.txt"), "q_.txt");
    }

    #[test]
    fn a_finished_download_gets_its_name_and_a_second_one_a_number() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = Downloads { dir: Some(dir.path().to_path_buf()), ..Default::default() };
        for guid in ["g1", "g2"] {
            std::fs::write(dir.path().join(guid), guid).unwrap();
            d.on_event(
                "Browser.downloadWillBegin",
                &json!({"guid": guid, "url": "http://x/r.csv", "suggestedFilename": "r.csv"}),
                "",
            );
            d.on_event(
                "Browser.downloadProgress",
                &json!({"guid": guid, "receivedBytes": 2, "totalBytes": 2, "state": "completed"}),
                "",
            );
        }
        let names: Vec<String> = d
            .items
            .iter()
            .map(|i| i.path.as_ref().unwrap().file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["r.csv", "r (2).csv"]);
        assert_eq!(std::fs::read_to_string(dir.path().join("r (2).csv")).unwrap(), "g2");
        assert_eq!(d.items[0].to_json()["state"], "completed");
    }

    #[test]
    fn a_scope_with_its_own_directory_starts_its_names_over() {
        let shared = tempfile::tempdir().unwrap();
        let own = tempfile::tempdir().unwrap();
        let mut d = Downloads { dir: Some(shared.path().to_path_buf()), ..Default::default() };
        d.dirs.insert("ctx".into(), own.path().to_path_buf());
        std::fs::write(shared.path().join("r.csv"), "old").unwrap();
        for (guid, scope, dir) in [("g1", "", shared.path()), ("g2", "ctx", own.path())] {
            std::fs::write(dir.join(guid), guid).unwrap();
            d.on_event(
                "Browser.downloadWillBegin",
                &json!({"guid": guid, "url": "http://x/r.csv", "suggestedFilename": "r.csv"}),
                scope,
            );
            d.on_event(
                "Browser.downloadProgress",
                &json!({"guid": guid, "receivedBytes": 2, "totalBytes": 2, "state": "completed"}),
                scope,
            );
        }
        assert_eq!(d.items[0].path.as_deref(), Some(shared.path().join("r (2).csv").as_path()));
        assert_eq!(d.items[1].path.as_deref(), Some(own.path().join("r.csv").as_path()));
        assert_eq!(std::fs::read_to_string(own.path().join("r.csv")).unwrap(), "g2");
    }
}
