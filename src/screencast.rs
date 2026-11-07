//! `screencast start <dir>` / `screencast stop`: what the tab the session
//! acts on showed, frame by frame, as JPEG files in `<dir>` with the time of
//! each, and a player (`<dir>/index.html`) that plays them at the pace they
//! came. Chrome sends a frame when the page's pixels change, so a still page
//! costs nothing. The recording follows the session from tab to tab.
//!
//! No video encoder: a frame list keeps navigera's binary small, opens in any
//! browser, and a test report can show it as it is.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use tokio::sync::{broadcast, oneshot};

use crate::cdp::CdpClient;

/// How long the start, the stop and each frame's acknowledgement may take.
const SEND: Duration = Duration::from_secs(5);

/// What `screencast start` asks Chrome for.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    /// JPEG quality, 0-100.
    pub quality: u8,
    /// The largest frame; Chrome scales the page down into it.
    pub max_width: u32,
    pub max_height: u32,
}

impl Default for Options {
    fn default() -> Self {
        Options { quality: 60, max_width: 1280, max_height: 720 }
    }
}

impl Options {
    /// `--quality <n>` and `--size <w>x<h>`.
    pub fn parse(quality: Option<&str>, size: Option<&str>) -> Result<Options> {
        let mut o = Options::default();
        if let Some(q) = quality {
            o.quality = match q.parse::<u8>() {
                Ok(n) if n <= 100 => n,
                _ => bail!("screencast --quality must be 0-100, got {q:?}"),
            };
        }
        if let Some(s) = size {
            let parsed =
                s.split_once(['x', 'X']).and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?)));
            match parsed {
                Some((w, h)) if w >= 16 && h >= 16 => (o.max_width, o.max_height) = (w, h),
                _ => bail!("screencast --size is <width>x<height> (at least 16x16), got {s:?}"),
            }
        }
        Ok(o)
    }
}

/// One frame: its file in the recording's directory, and when the page
/// showed it, in ms from the start.
#[derive(Clone, Debug)]
pub struct Frame {
    pub file: String,
    pub ms: u64,
}

/// The tab being recorded: its CDP session and the task that writes its
/// frames.
struct Tab {
    session_id: String,
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

/// A recording in progress.
pub struct Recording {
    dir: PathBuf,
    opts: Options,
    /// Epoch ms when it started: frame times are counted from it.
    started: f64,
    frames: Arc<Mutex<Vec<Frame>>>,
    /// A frame that couldn't be written (the first error).
    error: Arc<Mutex<Option<String>>>,
    tab: Option<Tab>,
    handle: tokio::runtime::Handle,
    client: CdpClient,
}

fn epoch_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
}

impl Recording {
    /// Start recording the tab of `session_id` into `dir` (made if missing;
    /// frames of an earlier recording there are replaced).
    pub fn start(
        handle: &tokio::runtime::Handle,
        client: &CdpClient,
        session_id: &str,
        dir: &Path,
        opts: Options,
    ) -> Result<Recording> {
        std::fs::create_dir_all(dir).with_context(|| format!("screencast: making {}", dir.display()))?;
        clear_frames(dir);
        let mut r = Recording {
            dir: dir.to_path_buf(),
            opts,
            started: epoch_ms(),
            frames: Arc::default(),
            error: Arc::default(),
            tab: None,
            handle: handle.clone(),
            client: client.clone(),
        };
        r.follow(session_id)?;
        Ok(r)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Record the tab of `session_id` from now on (the session moved to
    /// another tab). The same tab: nothing to do.
    pub fn follow(&mut self, session_id: &str) -> Result<()> {
        if self.tab.as_ref().is_some_and(|t| t.session_id == session_id) {
            return Ok(());
        }
        self.leave_tab();
        // Subscribe before the start, so the first frame isn't missed.
        let rx = self.client.subscribe();
        let (stop, stopped) = oneshot::channel();
        let task = self.handle.spawn(write_frames(
            self.client.clone(),
            rx,
            stopped,
            session_id.to_string(),
            self.dir.clone(),
            self.started,
            self.frames.clone(),
            self.error.clone(),
        ));
        let params = json!({
            "format": "jpeg",
            "quality": self.opts.quality,
            "maxWidth": self.opts.max_width,
            "maxHeight": self.opts.max_height,
        });
        let started =
            self.handle.block_on(self.client.send_harmless("Page.startScreencast", params, Some(session_id), SEND));
        self.tab = Some(Tab { session_id: session_id.to_string(), stop, task });
        if let Err(e) = started {
            self.leave_tab();
            return Err(e.context("screencast: Page.startScreencast"));
        }
        Ok(())
    }

    /// Stop recording the current tab (its last frames written). A tab that
    /// is gone has nothing more to send.
    fn leave_tab(&mut self) {
        let Some(tab) = self.tab.take() else { return };
        let _ = self.handle.block_on(self.client.send_harmless(
            "Page.stopScreencast",
            json!({}),
            Some(&tab.session_id),
            SEND,
        ));
        let _ = tab.stop.send(());
        let _ = self.handle.block_on(async { tokio::time::timeout(SEND, tab.task).await });
    }

    /// Stop, write the player, and say what was recorded.
    pub fn stop(mut self) -> Result<Value> {
        self.leave_tab();
        let ms = (epoch_ms() - self.started).max(0.0) as u64;
        let frames = self.frames.lock().unwrap().clone();
        let player = self.dir.join("index.html");
        crate::protocol::write_own(&player, player_html(&frames, ms).as_bytes())
            .with_context(|| format!("screencast: writing {}", player.display()))?;
        let mut out = json!({
            "screencast": self.dir.display().to_string(),
            "frames": frames.len(),
            "ms": ms,
            "player": player.display().to_string(),
        });
        if let Some(e) = self.error.lock().unwrap().take() {
            out["error"] = json!(e);
        }
        Ok(out)
    }
}

/// Frames left in `dir` by an earlier recording, so the player's list is
/// the only one there.
fn clear_frames(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if is_frame_name(&name) || name == "index.html" {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// `frame-00001.jpg`.
fn is_frame_name(name: &str) -> bool {
    name.strip_prefix("frame-")
        .and_then(|n| n.strip_suffix(".jpg"))
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The task that writes one tab's frames: each `Page.screencastFrame` of
/// `session_id` becomes the next file, and is acknowledged (Chrome sends no
/// more frames until it is). After the stop, frames already sent are kept.
#[allow(clippy::too_many_arguments)]
async fn write_frames(
    client: CdpClient,
    mut rx: broadcast::Receiver<Value>,
    mut stopped: oneshot::Receiver<()>,
    session_id: String,
    dir: PathBuf,
    started: f64,
    frames: Arc<Mutex<Vec<Frame>>>,
    error: Arc<Mutex<Option<String>>>,
) {
    let on_event = |event: Value| -> Option<i64> {
        if event.get("method").and_then(Value::as_str) != Some("Page.screencastFrame")
            || event.get("sessionId").and_then(Value::as_str) != Some(session_id.as_str())
        {
            return None;
        }
        let p = &event["params"];
        let ack = p.get("sessionId").and_then(Value::as_i64);
        if let Err(e) = save_frame(p, &dir, started, &frames) {
            error.lock().unwrap().get_or_insert(format!("{e:#}"));
        }
        ack
    };
    loop {
        tokio::select! {
            _ = &mut stopped => break,
            got = rx.recv() => match got {
                Ok(event) => {
                    if let Some(ack) = on_event(event) {
                        let _ = client
                            .send_harmless("Page.screencastFrameAck", json!({ "sessionId": ack }), Some(&session_id), SEND)
                            .await;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            },
        }
    }
    while let Ok(event) = rx.try_recv() {
        on_event(event);
    }
}

fn save_frame(p: &Value, dir: &Path, started: f64, frames: &Mutex<Vec<Frame>>) -> Result<()> {
    use base64::Engine as _;
    let data = p.get("data").and_then(Value::as_str).context("screencast: a frame without data")?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).context("screencast: a frame's data")?;
    // When the page showed it (seconds since the epoch), else now.
    let at = p["metadata"]["timestamp"].as_f64().map(|s| s * 1000.0).unwrap_or_else(epoch_ms);
    let mut frames = frames.lock().unwrap();
    let file = format!("frame-{:05}.jpg", frames.len() + 1);
    crate::protocol::write_own(dir.join(&file), &bytes).with_context(|| format!("screencast: writing {file}"))?;
    let ms = (at - started).max(0.0) as u64;
    // A frame stamped before the one before it (two tabs' clocks) plays
    // right after it.
    let ms = frames.last().map_or(ms, |f: &Frame| ms.max(f.ms));
    frames.push(Frame { file, ms });
    Ok(())
}

/// A page that plays `frames` at their times: play/pause (space), a
/// scrubber, step a frame (arrows), speed.
pub fn player_html(frames: &[Frame], ms: u64) -> String {
    let list: Vec<Value> = frames.iter().map(|f| json!([f.file, f.ms])).collect();
    let data = serde_json::to_string(&json!({ "frames": list, "ms": ms })).unwrap_or_default().replace("</", "<\\/");
    PLAYER.replace("/*DATA*/null", &data)
}

const PLAYER: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Screencast</title>
<style>
:root{color-scheme:light dark;--bg:#fff;--fg:#1d1d1f;--muted:#6e6e73;--line:#d2d2d7}
@media (prefers-color-scheme:dark){:root{--bg:#161617;--fg:#f5f5f7;--muted:#a1a1a6;--line:#424245}}
body{margin:0;background:var(--bg);color:var(--fg);font:14px system-ui,sans-serif}
main{max-width:1280px;margin:0 auto;padding:16px}
img{display:block;max-width:100%;border:1px solid var(--line);background:#000}
.bar{display:flex;gap:12px;align-items:center;margin-top:10px;flex-wrap:wrap}
input[type=range]{flex:1;min-width:160px}
.t{font-variant-numeric:tabular-nums;color:var(--muted)}
button,select{font:inherit}
p.none{color:var(--muted)}
</style></head><body><main>
<img id="f" alt="screencast frame">
<div class="bar"><button id="p">Play</button><input id="s" type="range" min="0" value="0" step="1">
<span class="t" id="t"></span><select id="r"><option>0.25</option><option>0.5</option><option selected>1</option><option>2</option><option>4</option></select></div>
</main><script>
const D=/*DATA*/null, F=D.frames, end=Math.max(D.ms,F.length?F[F.length-1][1]:0);
const img=document.getElementById('f'), s=document.getElementById('s'), t=document.getElementById('t'), p=document.getElementById('p'), r=document.getElementById('r');
let at=0, playing=false, last=0;
s.max=end;
const fmt=ms=>(ms/1000).toFixed(2)+' s';
const frameAt=ms=>{let lo=0,hi=F.length-1,k=0;while(lo<=hi){const m=(lo+hi)>>1;if(F[m][1]<=ms){k=m;lo=m+1}else hi=m-1}return k};
function show(){if(!F.length){img.replaceWith(Object.assign(document.createElement('p'),{className:'none',textContent:'No frames: the page showed nothing new.'}));return}
const k=frameAt(at);if(img.dataset.k!=k){img.src=F[k][0];img.dataset.k=k}s.value=at;t.textContent=fmt(at)+' / '+fmt(end)+'  frame '+(k+1)+'/'+F.length}
function tick(now){if(!playing)return;at+=(now-last)*Number(r.value);last=now;if(at>=end){at=end;playing=false;p.textContent='Play'}show();if(playing)requestAnimationFrame(tick)}
function play(){if(!F.length)return;if(playing){playing=false;p.textContent='Play';return}if(at>=end)at=0;playing=true;p.textContent='Pause';last=performance.now();requestAnimationFrame(tick)}
p.onclick=play;s.oninput=()=>{at=Number(s.value);show()};
document.onkeydown=e=>{if(e.key===' '){e.preventDefault();play()}else if(e.key==='ArrowRight'||e.key==='ArrowLeft'){const k=frameAt(at)+(e.key==='ArrowRight'?1:-1);if(F[k]){at=F[k][1];show()}}};
show();
</script></body></html>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_read_quality_and_size() {
        assert_eq!(Options::parse(None, None).unwrap(), Options::default());
        let o = Options::parse(Some("80"), Some("800x600")).unwrap();
        assert_eq!((o.quality, o.max_width, o.max_height), (80, 800, 600));
        assert!(Options::parse(Some("101"), None).is_err());
        assert!(Options::parse(None, Some("800")).is_err());
        assert!(Options::parse(None, Some("8x8")).is_err());
    }

    #[test]
    fn the_player_carries_the_frames_and_closes_no_script_early() {
        let frames =
            vec![Frame { file: "frame-00001.jpg".into(), ms: 0 }, Frame { file: "frame-00002.jpg".into(), ms: 340 }];
        let html = player_html(&frames, 900);
        assert!(html.contains(r#"{"frames":[["frame-00001.jpg",0],["frame-00002.jpg",340]],"ms":900}"#), "{html}");
        assert!(!html.contains("/*DATA*/"));
        assert_eq!(html.matches("</script>").count(), 1);
    }

    #[test]
    fn only_frames_are_cleared() {
        assert!(is_frame_name("frame-00001.jpg"));
        assert!(!is_frame_name("frame-.jpg"));
        assert!(!is_frame_name("frame-1.png"));
        assert!(!is_frame_name("photo.jpg"));
    }
}
