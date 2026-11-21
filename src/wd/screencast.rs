//! `screencast start <dir>` / `screencast stop` on the WebDriver BiDi
//! engines (Firefox; a WebDriver server with `--bidi`). BiDi has no
//! screencast: nothing sends a frame when the page's pixels change, as
//! Chrome does. So a thread of its own, on a second connection to the
//! session, takes a JPEG of the viewport of the tab the session acts on,
//! again and again, while commands run on the first connection. A look that
//! shows what the one before showed keeps no frame, and while the page stays
//! still the looks space out (from [`GAP_MIN`] after a change, doubling up to
//! [`GAP_MAX`]); a still page still costs a screenshot each [`GAP_MAX`],
//! where Chrome's costs nothing. The files and the player are Chrome's
//! ([`crate::screencast`]); a frame is as large as the viewport, as a BiDi
//! screenshot has no scale.

use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::{self, stream::MaybeTlsStream, Message, WebSocket};

use crate::screencast::{epoch_ms, Frame, Options};

/// The time from one look to the next after a change.
const GAP_MIN: Duration = Duration::from_millis(100);
/// The longest time between two looks, while the page stays still.
const GAP_MAX: Duration = Duration::from_secs(1);
/// How long one screenshot may take.
const SHOT: Duration = Duration::from_secs(10);
/// How long a read waits before the deadline is looked at again.
const READ_SLICE: Duration = Duration::from_millis(250);
/// Failed looks in a row before the recording reports why (a tab that
/// closed fails once or twice until the session is in another).
const FAILS_SAID: u32 = 3;

type Ws = WebSocket<MaybeTlsStream<TcpStream>>;

/// What the recording thread hands back: its frames and the first error
/// worth saying.
type Taken = (Vec<Frame>, Option<String>);

/// A recording in progress.
pub struct Recorder {
    dir: PathBuf,
    /// Epoch ms when it started: frame times are counted from it.
    started: f64,
    /// The top-level context the session acts on: the one looked at.
    tab: Arc<Mutex<String>>,
    /// `true`: one last look, then stop; `false`: stop now.
    stop: mpsc::Sender<bool>,
    thread: Option<JoinHandle<Taken>>,
}

impl Recorder {
    /// Start recording `tab` of the session another connection joins at
    /// `endpoint`, into `dir` (made if missing; frames of an earlier
    /// recording there are replaced).
    pub fn start(endpoint: &str, tab: &str, dir: &Path, opts: &Options, timeout: Duration) -> Result<Recorder> {
        std::fs::create_dir_all(dir).with_context(|| format!("screencast: making {}", dir.display()))?;
        let ws = super::bidi::open_ws(endpoint, timeout).context("screencast: a second connection to the session")?;
        crate::screencast::clear_frames(dir);
        let started = epoch_ms();
        let tab = Arc::new(Mutex::new(tab.to_string()));
        let (stop, stopped) = mpsc::channel();
        let quality = f64::from(opts.quality) / 100.0;
        let (shared, out) = (tab.clone(), dir.to_path_buf());
        let thread = std::thread::Builder::new()
            .name("screencast".into())
            .spawn(move || record(ws, &shared, &stopped, &out, started, quality))
            .context("screencast: starting its thread")?;
        Ok(Recorder { dir: dir.to_path_buf(), started, tab, stop, thread: Some(thread) })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Look at `tab` from now on (the session moved to another tab).
    pub fn follow(&self, tab: &str) {
        let mut t = self.tab.lock().unwrap();
        if *t != tab {
            *t = tab.to_string();
        }
    }

    /// One last look, then stop, write the player, and say what was
    /// recorded.
    pub fn stop(mut self) -> Result<Value> {
        let _ = self.stop.send(true);
        let (frames, error) = match self.thread.take().map(JoinHandle::join) {
            Some(Ok(taken)) => taken,
            _ => (Vec::new(), Some("screencast: its thread failed".to_string())),
        };
        let ms = (epoch_ms() - self.started).max(0.0) as u64;
        let player = self.dir.join("index.html");
        crate::protocol::write_own(&player, crate::screencast::player_html(&frames, ms).as_bytes())
            .with_context(|| format!("screencast: writing {}", player.display()))?;
        let mut out = json!({
            "screencast": self.dir.display().to_string(),
            "frames": frames.len(),
            "ms": ms,
            "player": player.display().to_string(),
        });
        if let Some(e) = error {
            out["error"] = json!(e);
        }
        Ok(out)
    }
}

impl Drop for Recorder {
    /// The session ends while recording: the thread stops without a last
    /// look and is not waited for (its connection goes with the browser).
    fn drop(&mut self) {
        if self.thread.is_some() {
            let _ = self.stop.send(false);
        }
    }
}

/// Why a look failed: the connection is gone (`true`), or the browser
/// refused this one screenshot.
struct Failed(bool, String);

/// The thread: look, keep the frame if it differs from the last one kept,
/// wait the gap, again; until told to stop.
fn record(mut ws: Ws, tab: &Mutex<String>, stopped: &mpsc::Receiver<bool>, dir: &Path, started: f64, q: f64) -> Taken {
    let mut frames = Vec::new();
    let mut error = None;
    let mut kept: Vec<u8> = Vec::new();
    let (mut gap, mut fails, mut id, mut last) = (GAP_MIN, 0, 0u64, false);
    loop {
        let looked = Instant::now();
        let at = epoch_ms();
        let context = tab.lock().unwrap().clone();
        id += 1;
        match shot(&mut ws, id, &context, q) {
            Ok(bytes) => {
                fails = 0;
                let (next, keep) = after_look(gap, bytes != kept);
                gap = next;
                if keep {
                    if let Err(e) = crate::screencast::push_frame(dir, started, &mut frames, &bytes, at) {
                        error.get_or_insert(format!("{e:#}"));
                    }
                    kept = bytes;
                }
            }
            Err(Failed(gone, why)) => {
                fails += 1;
                if gone || fails >= FAILS_SAID {
                    error.get_or_insert(format!("screencast: {why}"));
                }
                if gone {
                    break;
                }
                // A navigation under way, a tab that just closed: soon
                // again, then less and less often.
                gap = after_look(gap, false).0;
            }
        }
        if last {
            break;
        }
        match stopped.recv_timeout(gap.saturating_sub(looked.elapsed())) {
            Ok(true) => last = true,
            Ok(false) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
    let _ = ws.close(None);
    let _ = ws.flush();
    (frames, error)
}

/// The gap to the next look, and whether to keep this one's frame: after a
/// change, a frame and the shortest gap; else none, and twice the gap, up
/// to [`GAP_MAX`].
fn after_look(gap: Duration, changed: bool) -> (Duration, bool) {
    if changed {
        (GAP_MIN, true)
    } else {
        ((gap * 2).min(GAP_MAX), false)
    }
}

/// A JPEG of `context`'s viewport.
fn shot(ws: &mut Ws, id: u64, context: &str, quality: f64) -> std::result::Result<Vec<u8>, Failed> {
    let ask = json!({
        "id": id,
        "method": "browsingContext.captureScreenshot",
        "params": { "context": context, "origin": "viewport", "format": { "type": "image/jpeg", "quality": quality } },
    });
    ws.send(Message::text(ask.to_string()))
        .map_err(|e| Failed(true, format!("the second connection to the session failed ({e})")))?;
    if let MaybeTlsStream::Plain(s) = ws.get_mut() {
        s.set_read_timeout(Some(READ_SLICE)).ok();
    }
    let deadline = Instant::now() + SHOT;
    loop {
        if Instant::now() > deadline {
            return Err(Failed(false, format!("no screenshot in {} s", SHOT.as_secs())));
        }
        match ws.read() {
            Ok(Message::Text(t)) => {
                // Events (a browser may send the session's events to every
                // connection) and a late answer to an earlier look.
                let Ok(v) = serde_json::from_str::<Value>(t.as_str()) else { continue };
                if v["id"].as_u64() == Some(id) {
                    return screenshot_bytes(&v);
                }
            }
            Ok(Message::Close(_)) => return Err(Failed(true, "the browser closed the second connection".into())),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(e) => return Err(Failed(true, format!("the second connection to the session failed ({e})"))),
        }
    }
}

/// The image in a `captureScreenshot` answer.
fn screenshot_bytes(answer: &Value) -> std::result::Result<Vec<u8>, Failed> {
    if let Some(e) = answer["error"].as_str() {
        let message = answer["message"].as_str().unwrap_or("");
        return Err(Failed(false, format!("the browser took no screenshot: {e}: {message}")));
    }
    let data = answer["result"]["data"].as_str().ok_or_else(|| Failed(false, "a screenshot without data".into()))?;
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| Failed(false, format!("a screenshot that is not base64 ({e})")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_still_page_is_looked_at_less_often_and_a_change_at_once() {
        let mut gap = GAP_MIN;
        let mut gaps = Vec::new();
        for _ in 0..6 {
            let (next, keep) = after_look(gap, false);
            assert!(!keep, "an unchanged look keeps no frame");
            gap = next;
            gaps.push(gap.as_millis());
        }
        assert_eq!(gaps, [200, 400, 800, 1000, 1000, 1000]);
        assert_eq!(after_look(gap, true), (GAP_MIN, true));
    }

    #[test]
    fn a_screenshot_answer_gives_its_image_or_says_why_not() {
        let ok = json!({ "id": 3, "type": "success", "result": { "data": "/9j/" } });
        assert_eq!(screenshot_bytes(&ok).ok(), Some(vec![0xff, 0xd8, 0xff]));
        let refused = json!({ "id": 3, "type": "error", "error": "no such frame", "message": "gone" });
        let Err(Failed(gone, why)) = screenshot_bytes(&refused) else { panic!("an error answer is no image") };
        assert!(!gone && why.contains("no such frame: gone"), "{why}");
        assert!(screenshot_bytes(&json!({ "id": 3, "result": {} })).is_err());
    }
}
