//! `screencast start <dir>` / `screencast stop` on the WebDriver BiDi
//! engines (Firefox; a WebDriver server with `--bidi`). BiDi has no
//! screencast: nothing sends a frame when the page's pixels change, as
//! Chrome does. So the session's own connection asks for a JPEG of the
//! viewport of the tab the session acts on, again and again, while its
//! commands run and between them: a browser answers a screenshot sent
//! behind a pending evaluate or navigation (Firefox 160: in 44 ms behind a
//! 1 s evaluate; chromedriver: in 52-78 ms), and every read of the
//! connection sends the look that is due and takes the answer of the one
//! on its way (`Bidi::read_one`). No thread and no second connection: Firefox
//! serves none to a session made with `session.new`.
//!
//! A look that shows what the one before showed keeps no frame, and while
//! the page stays still the looks space out (from [`GAP_MIN`] after a
//! change, doubling up to [`GAP_MAX`]); a still page still costs a
//! screenshot each [`GAP_MAX`], where Chrome's costs nothing. The files and
//! the player are Chrome's ([`crate::screencast`]); a frame is as large as
//! the viewport, as a BiDi screenshot has no scale.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::screencast::{epoch_ms, Frame, Options};

/// The time from one look to the next after a change.
const GAP_MIN: Duration = Duration::from_millis(100);
/// The longest time between two looks, while the page stays still.
const GAP_MAX: Duration = Duration::from_secs(1);
/// How long one look may take, and `stop` in all.
const SHOT: Duration = Duration::from_secs(10);
/// How often the session looks for the answer of a look on its way, while
/// no command reads the connection.
const ANSWER_TICK: Duration = Duration::from_millis(20);
/// Failed looks in a row before the recording reports why (a tab that
/// closed fails once or twice until the session is in another).
const FAILS_SAID: u32 = 3;

/// A look on its way: its command's id, when it was sent (the frame's time
/// is this, as epoch ms), and until when its answer is waited for.
struct Asked {
    id: u64,
    at: f64,
    sent: Instant,
    by: Instant,
}

/// A recording in progress: what was looked at, and when to look next.
pub struct Looks {
    dir: PathBuf,
    /// Epoch ms when it started: frame times are counted from it.
    started: f64,
    quality: f64,
    frames: Vec<Frame>,
    /// The image of the last frame kept: a look that shows the same keeps none.
    kept: Vec<u8>,
    /// The first error worth saying, and why the last look failed.
    error: Option<String>,
    failed: Option<String>,
    gap: Duration,
    fails: u32,
    /// When the next look is due, and the one on its way.
    due: Instant,
    asked: Option<Asked>,
    /// The last look got no answer in time.
    timed_out: bool,
    /// `stop`: no look by the clock from then on, and nothing waited for
    /// after this; and whether its one last look was sent.
    ends: Option<Instant>,
    last: bool,
}

impl Looks {
    /// A recording into `dir` (made if missing; frames of an earlier
    /// recording there are replaced). Its first look is due at once.
    pub fn start(dir: &Path, opts: &Options) -> Result<Looks> {
        std::fs::create_dir_all(dir).with_context(|| format!("screencast: making {}", dir.display()))?;
        crate::screencast::clear_frames(dir);
        Ok(Looks {
            dir: dir.to_path_buf(),
            started: epoch_ms(),
            quality: f64::from(opts.quality) / 100.0,
            frames: Vec::new(),
            kept: Vec::new(),
            error: None,
            failed: None,
            gap: GAP_MIN,
            fails: 0,
            due: Instant::now(),
            asked: None,
            timed_out: false,
            ends: None,
            last: false,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Whether a look is to be sent now ([`Self::ask`], then [`Self::sent`]).
    /// A look on its way whose time ran out counts as failed first.
    pub fn due(&mut self, now: Instant) -> bool {
        self.expire(now);
        self.asked.is_none() && self.ends.is_none() && now >= self.due
    }

    /// The parameters of `browsingContext.captureScreenshot` for a look at
    /// `context`.
    pub fn ask(&self, context: &str) -> Value {
        json!({
            "context": context,
            "origin": "viewport",
            "format": { "type": "image/jpeg", "quality": self.quality },
        })
    }

    /// The look was sent as command `id`.
    pub fn sent(&mut self, id: u64) {
        let sent = Instant::now();
        let by = self.ends.map_or(sent + SHOT, |e| e.min(sent + SHOT));
        self.last |= self.ends.is_some();
        self.asked = Some(Asked { id, at: epoch_ms(), sent, by });
    }

    /// The look could not be sent: the connection is gone, and so is the
    /// recording (what it kept stays).
    pub fn unsent(&mut self, why: String) {
        self.error.get_or_insert_with(|| format!("screencast: {why}"));
        self.failed = Some(why);
        self.ends = Some(Instant::now());
    }

    /// How long a read may wait before this recording needs it back: until
    /// the next look is due, until the one on its way runs out of time, or
    /// `None` when it needs nothing more.
    pub fn wake_in(&self, now: Instant) -> Option<Duration> {
        match &self.asked {
            Some(a) => Some(a.by.saturating_duration_since(now)),
            None if self.ends.is_some() => None,
            None => Some(self.due.saturating_duration_since(now)),
        }
    }

    /// How long the session may wait for its next command before it reads
    /// the connection for this recording: while a look is on its way, soon,
    /// so that its answer is taken and the next look goes out on time.
    pub fn tick(&self, now: Instant) -> Option<Duration> {
        let wake = self.wake_in(now)?;
        Some(if self.asked.is_some() { wake.min(ANSWER_TICK) } else { wake })
    }

    /// Whether `message` answers the look on its way.
    pub fn answers(&self, message: &Value) -> bool {
        self.asked.as_ref().is_some_and(|a| message["id"].as_u64() == Some(a.id))
    }

    /// The answer of the look on its way: a frame when it shows something
    /// new.
    pub fn answered(&mut self, message: &Value) {
        let Some(a) = self.asked.take() else { return };
        self.timed_out = false;
        match screenshot_bytes(message) {
            Ok(bytes) => {
                self.fails = 0;
                self.failed = None;
                let (gap, keep) = after_look(self.gap, bytes != self.kept);
                self.gap = gap;
                if keep {
                    if let Err(e) =
                        crate::screencast::push_frame(&self.dir, self.started, &mut self.frames, &bytes, a.at)
                    {
                        self.error.get_or_insert(format!("{e:#}"));
                    }
                    self.kept = bytes;
                }
            }
            // A navigation under way, a tab that just closed: soon again,
            // then less and less often.
            Err(why) => self.fail(why),
        }
        self.due = a.sent + self.gap;
    }

    fn fail(&mut self, why: String) {
        self.fails += 1;
        if self.fails >= FAILS_SAID {
            self.error.get_or_insert_with(|| format!("screencast: {why}"));
        }
        self.failed = Some(why);
        self.gap = after_look(self.gap, false).0;
    }

    /// A look on its way whose time ran out failed; its answer, if it still
    /// comes, is not this recording's any more.
    fn expire(&mut self, now: Instant) {
        if self.asked.as_ref().is_some_and(|a| now >= a.by) {
            self.asked = None;
            self.timed_out = true;
            self.fail(format!("no screenshot in {} s", SHOT.as_secs()));
            self.due = now + self.gap;
        }
    }

    /// `stop` was asked: the look on its way is still waited for, then one
    /// last look is wanted ([`Self::last_wanted`]); nothing after [`SHOT`]
    /// from now.
    pub fn stop(&mut self) {
        self.ends = Some(Instant::now() + SHOT);
        if let Some(a) = self.asked.as_mut() {
            a.by = a.by.min(Instant::now() + SHOT);
        }
    }

    /// After `stop`, whether its one last look is to be sent: not while one
    /// is on its way, not after one that got no answer in time (the browser
    /// would not answer the next sooner), not once `stop`'s time is up.
    pub fn last_wanted(&mut self, now: Instant) -> bool {
        self.expire(now);
        self.asked.is_none() && !self.timed_out && !self.last && self.ends.is_some_and(|e| now < e)
    }

    /// Whether a look is on its way (after `stop`: waited for).
    pub fn waiting(&mut self, now: Instant) -> bool {
        self.expire(now);
        self.asked.is_some()
    }

    /// Write the player and say what was recorded. A recording that kept no
    /// frame always says why.
    pub fn finish(self) -> Result<Value> {
        let ms = crate::screencast::recording_ms(self.started, epoch_ms(), &self.frames);
        let player = self.dir.join("index.html");
        crate::protocol::write_own(&player, crate::screencast::player_html(&self.frames, ms).as_bytes())
            .with_context(|| format!("screencast: writing {}", player.display()))?;
        let mut out = json!({
            "screencast": self.dir.display().to_string(),
            "frames": self.frames.len(),
            "ms": ms,
            "player": player.display().to_string(),
        });
        let error = match (self.error, self.frames.is_empty()) {
            (Some(e), _) => Some(e),
            (None, true) => Some(format!(
                "screencast: no frame was taken ({})",
                self.failed.as_deref().unwrap_or("the browser answered no screenshot")
            )),
            (None, false) => None,
        };
        if let Some(e) = error {
            out["error"] = json!(e);
        }
        Ok(out)
    }
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

/// The image in a `captureScreenshot` answer, or why there is none.
fn screenshot_bytes(answer: &Value) -> std::result::Result<Vec<u8>, String> {
    if let Some(e) = answer["error"].as_str() {
        let message = answer["message"].as_str().unwrap_or("");
        return Err(format!("the browser took no screenshot: {e}: {message}"));
    }
    let data = answer["result"]["data"].as_str().ok_or_else(|| "a screenshot without data".to_string())?;
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| format!("a screenshot that is not base64 ({e})"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot(id: u64, data: &str) -> Value {
        json!({ "id": id, "type": "success", "result": { "data": data } })
    }

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
        assert_eq!(screenshot_bytes(&shot(3, "/9j/")).ok(), Some(vec![0xff, 0xd8, 0xff]));
        let refused = json!({ "id": 3, "type": "error", "error": "no such frame", "message": "gone" });
        let why = screenshot_bytes(&refused).unwrap_err();
        assert!(why.contains("no such frame: gone"), "{why}");
        assert!(screenshot_bytes(&json!({ "id": 3, "result": {} })).is_err());
    }

    /// One look at a time; a new image is a frame, the same one is not; an
    /// answer to another command is not a look's; the next look is due a
    /// gap after the last was sent.
    #[test]
    fn looks_keep_what_changed_one_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let mut l = Looks::start(dir.path(), &Options::default()).unwrap();
        let now = Instant::now();
        assert!(l.due(now), "the first look is due at once");
        assert_eq!(l.ask("tab")["context"], "tab");
        l.sent(7);
        assert!(!l.due(Instant::now()), "not while one is on its way");
        assert!(!l.answers(&shot(6, "/9j/")) && l.answers(&shot(7, "/9j/")));
        l.answered(&shot(7, "/9j/"));
        assert!(!l.due(Instant::now()) && l.due(Instant::now() + GAP_MIN), "a gap after it was sent");
        l.sent(8);
        l.answered(&shot(8, "/9j/"));
        l.sent(9);
        l.answered(&shot(9, "/9j/2Q=="));
        l.stop();
        let out = l.finish().unwrap();
        assert_eq!(out["frames"], 2, "{out}");
        assert!(out.get("error").is_none(), "{out}");
        assert!(dir.path().join("frame-00002.jpg").exists() && dir.path().join("index.html").exists());
    }

    /// `stop` behind a look that never answers waits for that look only:
    /// no second look after a timeout, and a recording without a frame
    /// says why (it read as a success, after 20 s).
    #[test]
    fn stop_behind_a_look_without_answer_waits_once_and_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let mut l = Looks::start(dir.path(), &Options::default()).unwrap();
        assert!(l.due(Instant::now()));
        l.sent(1);
        l.stop();
        let later = Instant::now() + SHOT + Duration::from_millis(1);
        assert!(!l.waiting(later), "the look's time is up by stop's");
        assert!(!l.last_wanted(later), "no last look after one the browser did not answer");
        // Its answer, late, is not the recording's.
        assert!(!l.answers(&shot(1, "/9j/")));
        let out = l.finish().unwrap();
        assert_eq!(out["frames"], 0);
        let error = out["error"].as_str().unwrap_or("");
        assert!(error.contains("no frame was taken") && error.contains("no screenshot in 10 s"), "{out}");
    }

    /// After `stop`, the look on its way is waited for and then one last
    /// look is wanted; nothing goes out by the clock any more.
    #[test]
    fn stop_takes_the_look_on_its_way_then_one_last() {
        let dir = tempfile::tempdir().unwrap();
        let mut l = Looks::start(dir.path(), &Options::default()).unwrap();
        assert!(l.due(Instant::now()));
        l.sent(1);
        l.stop();
        assert!(l.waiting(Instant::now()) && !l.last_wanted(Instant::now()));
        l.answered(&shot(1, "/9j/"));
        assert!(!l.due(Instant::now() + GAP_MAX), "no look by the clock after stop");
        assert!(l.last_wanted(Instant::now()));
        l.sent(2);
        assert_eq!(l.tick(Instant::now()), Some(ANSWER_TICK));
        l.answered(&shot(2, "/9j/2Q=="));
        assert!(!l.last_wanted(Instant::now()), "one last look only");
        assert_eq!(l.finish().unwrap()["frames"], 2);
    }

    /// A connection that fails ends the recording with what it kept, and
    /// says why.
    #[test]
    fn a_look_that_cannot_be_sent_ends_the_recording() {
        let dir = tempfile::tempdir().unwrap();
        let mut l = Looks::start(dir.path(), &Options::default()).unwrap();
        l.unsent("the browser connection failed".into());
        assert!(!l.due(Instant::now() + GAP_MAX) && l.wake_in(Instant::now()).is_none());
        let out = l.finish().unwrap();
        assert!(out["error"].as_str().is_some_and(|e| e.contains("connection failed")), "{out}");
    }
}
