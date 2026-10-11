//! Browser-level CDP: launch, tab targets, shutdown.

use std::process::Child;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use std::sync::Arc;

use super::client::CdpClient;
use super::events::{self, DialogPolicy, Shared};
use super::page::Page;
use super::transport::{self, LaunchedChrome};

const CMD_TIMEOUT: Duration = Duration::from_secs(30);

pub struct LaunchOptions {
    pub exe: String,
    pub headless: bool,
    /// Extra Chromium flags (skipped for non-Chromium engines).
    pub chrome_flags: Vec<String>,
    /// Pre-picked remote debugging port (used by engines whose launcher
    /// needs to know it, e.g. the Lightpanda shim).
    pub debugging_port: Option<u16>,
    /// CDP over `--remote-debugging-pipe` instead of a WebSocket port.
    pub pipe: bool,
}

/// A launched browser: one CDP client on the browser target, plus the child
/// process handle so `close` can reap it without orphans.
pub struct Browser {
    handle: tokio::runtime::Handle,
    client: CdpClient,
    child: Mutex<Option<Child>>,
    /// The child leads its own process group (every browser we launch on
    /// Unix; lightpanda possibly under xvfb-run): close kills the whole
    /// group, not just the browser process or the wrapper.
    kill_group: bool,
    /// Kept alive for the session; the temp profile is deleted by `close`
    /// (or on drop). `None` for engines that need no profile (Lightpanda).
    profile_dir: Mutex<Option<tempfile::TempDir>>,
    ws_url: String,
    /// Chrome's stderr tail (pipe launches), for "why did it die" errors.
    stderr: Option<transport::StderrTail>,
    /// Dialog log, popups and per-tab navigation state (see `events`).
    shared: Arc<Shared>,
    /// Windows: kill-on-close job holding the browser tree, so Chrome dies
    /// with this process even when it is killed (see `procjob`).
    job: Option<super::procjob::ProcJob>,
}

impl Browser {
    /// Launch `lightpanda serve` directly (no Chromium flags, no shim) and
    /// connect the browser-level CDP session.
    ///
    /// Lightpanda is a CDP server, not a Chromium executable: it must be
    /// started as `lightpanda serve --host … --port …`, and the browser
    /// WebSocket URL is discovered via its `/json/version` endpoint.
    pub fn launch_lightpanda(handle: &tokio::runtime::Handle, bin: &str) -> Result<Self> {
        let port = free_port().context("pick a free port for lightpanda serve")?;
        // The lightpanda nightly currently requires an X server even for
        // `serve` (upstream bug — it is supposed to be headless). Wrap with
        // `xvfb-run` when there is no display and xvfb is available.
        let (program, args) = if std::env::var_os("DISPLAY").is_none() && xvfb_available() {
            eprintln!("[browser] no $DISPLAY — launching lightpanda serve under xvfb-run");
            (
                "xvfb-run".to_string(),
                vec![
                    "-a".to_string(),
                    bin.to_string(),
                    "serve".to_string(),
                    "--host".to_string(),
                    "127.0.0.1".to_string(),
                    "--port".to_string(),
                    port.to_string(),
                ],
            )
        } else {
            (
                bin.to_string(),
                vec![
                    "serve".to_string(),
                    "--host".to_string(),
                    "127.0.0.1".to_string(),
                    "--port".to_string(),
                    port.to_string(),
                ],
            )
        };
        // Write child output to a temp file for diagnostics (not piped to avoid
        // the pipe-buffer deadlock). The file is read only on poll failure.
        let log_path = std::env::temp_dir().join(format!("lightpanda-serve-{port}.log"));
        let log_file = std::fs::File::create(&log_path).ok();
        let log_file_err = log_file.as_ref().and_then(|f| f.try_clone().ok());
        let mut cmd = std::process::Command::new(&program);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(log_file.map(std::process::Stdio::from).unwrap_or(std::process::Stdio::null()))
            .stderr(log_file_err.map(std::process::Stdio::from).unwrap_or(std::process::Stdio::null()))
            .spawn()
            .with_context(|| format!("spawn `{program} {}`", args.join(" ")))?;
        let ws_url = match transport::poll_ws_url_standalone(port, Duration::from_secs(60)) {
            Ok(url) => url,
            Err(e) => {
                let log_tail = std::fs::read_to_string(&log_path)
                    .map(|s| {
                        let lines: Vec<&str> = s.lines().collect();
                        let start = lines.len().saturating_sub(30);
                        lines[start..].join("\n")
                    })
                    .unwrap_or_default();
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!(
                    "`{program} {}` did not come up on 127.0.0.1:{port}: {e:#}\nlog tail ({log_path:?}):\n{log_tail}",
                    args.join(" ")
                );
            }
        };
        crate::timing::log(&format!("[browser] lightpanda serve on 127.0.0.1:{port} -> {ws_url}"));
        let client = handle.block_on(CdpClient::connect(&ws_url)).context("CDP connect to lightpanda")?;
        let shared = Arc::new(Shared::default());
        events::spawn(handle, &client, Arc::clone(&shared));
        Ok(Self {
            handle: handle.clone(),
            client,
            child: Mutex::new(Some(child)),
            kill_group: cfg!(unix),
            profile_dir: Mutex::new(None),
            ws_url,
            stderr: None,
            shared,
            job: None,
        })
    }

    /// Launch the engine and connect the browser-level CDP session.
    pub fn launch(handle: &tokio::runtime::Handle, opts: &LaunchOptions) -> Result<Self> {
        let started = std::time::Instant::now();
        #[cfg(unix)]
        if opts.pipe && opts.debugging_port.is_none() {
            let launched =
                transport::launch_chrome_pipe(&opts.exe, opts.headless, &opts.chrome_flags).context("launch chrome")?;
            crate::timing::record("spawn", started);
            let client = {
                let _guard = handle.enter();
                handle.block_on(CdpClient::connect_pipe(launched.from_browser, launched.to_browser))
            }
            .context("CDP pipe connect")?;
            let mut browser =
                Self::finish(handle, client, Some(launched.child), Some(launched.profile_dir), "pipe".into());
            browser.stderr = Some(launched.stderr);
            return Ok(browser);
        }
        let LaunchedChrome { child, profile_dir, ws_url, job } =
            transport::launch_chrome(&opts.exe, opts.headless, &opts.chrome_flags, opts.debugging_port)
                .context("launch chrome")?;
        crate::timing::record("devtools_url", started);
        let connect_started = std::time::Instant::now();
        let client = handle.block_on(CdpClient::connect(&ws_url)).context("CDP connect")?;
        crate::timing::record("ws_connect", connect_started);
        let mut browser = Self::finish(handle, client, Some(child), Some(profile_dir), ws_url);
        browser.job = job;
        Ok(browser)
    }

    /// Connect to a browser someone else runs (`--attach`, `--profile`).
    /// navigera owns no process here: `close` only disconnects, and the
    /// browser, its windows and tabs stay as they are.
    pub fn connect(handle: &tokio::runtime::Handle, ws_url: &str, bound: Duration) -> Result<Self> {
        let started = std::time::Instant::now();
        let client = handle
            .block_on(CdpClient::connect_within(ws_url, bound))
            .with_context(|| format!("connect to the running browser at {ws_url}"))?;
        crate::timing::record("ws_connect", started);
        Ok(Self::finish(handle, client, None, None, ws_url.to_string()))
    }

    fn finish(
        handle: &tokio::runtime::Handle,
        client: CdpClient,
        child: Option<Child>,
        profile_dir: Option<tempfile::TempDir>,
        ws_url: String,
    ) -> Self {
        let shared = Arc::new(Shared::default());
        events::spawn(handle, &client, Arc::clone(&shared));
        // Popups (`target=_blank`, `window.open`) announce themselves as
        // `Target.targetCreated` with an `openerId`; the session adopts them
        // as tabs. One round trip, before any page exists.
        if std::env::var_os("NAVIGERA_NO_DISCOVER").is_none() {
            let _ = handle.block_on(client.send(
                "Target.setDiscoverTargets",
                json!({ "discover": true }),
                None,
                CMD_TIMEOUT,
            ));
        }
        Self {
            handle: handle.clone(),
            client,
            // Launched browsers lead their own process group (see
            // `transport::own_process_group`); attached ones have no child.
            kill_group: cfg!(unix) && child.is_some(),
            child: Mutex::new(child),
            profile_dir: Mutex::new(profile_dir),
            ws_url,
            shared,
            stderr: None,
            job: None,
        }
    }

    fn block_on<F: std::future::Future>(&self, fut: F) -> F::Output {
        self.handle.block_on(fut)
    }

    pub fn ws_url(&self) -> &str {
        &self.ws_url
    }

    /// Open a new tab target, attach a session, enable Page/Runtime domains.
    /// Create a page on Lightpanda: it requires an explicit browser context
    /// per connection (one context + one page per process).
    pub fn new_page_lightpanda(&self, url: Option<&str>) -> Result<Page> {
        self.block_on(async {
            let ctx = self
                .client
                .send("Target.createBrowserContext", json!({}), None, CMD_TIMEOUT)
                .await
                .context("Target.createBrowserContext")?;
            let browser_context_id = ctx
                .get("browserContextId")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("createBrowserContext: no browserContextId"))?
                .to_string();
            let target = self
                .client
                .send(
                    "Target.createTarget",
                    json!({
                        "url": url.unwrap_or("about:blank"),
                        "browserContextId": browser_context_id,
                    }),
                    None,
                    CMD_TIMEOUT,
                )
                .await
                .context("Target.createTarget")?;
            let target_id = target
                .get("targetId")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("createTarget: no targetId"))?
                .to_string();
            let session = self
                .client
                .send("Target.attachToTarget", json!({ "targetId": target_id, "flatten": true }), None, CMD_TIMEOUT)
                .await
                .context("Target.attachToTarget")?;
            let session_id = session
                .get("sessionId")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("attachToTarget: no sessionId"))?
                .to_string();
            self.shared.register(&session_id, &target_id);
            let page =
                Page::new(self.handle.clone(), self.client.clone(), session_id, target_id, Arc::clone(&self.shared));
            page.enable().await?;
            Ok(page)
        })
    }

    /// The watcher's shared state (dialog log, popups, navigation).
    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    /// How dialogs are answered from now on.
    pub fn set_dialog_policy(&self, policy: DialogPolicy) {
        *self.shared.policy.lock().unwrap() = policy;
    }

    /// Attach to a page target the browser opened on its own (a popup) and
    /// enable it like a tab we created.
    pub fn attach_page(&self, target_id: &str) -> Result<Page> {
        self.block_on(async {
            let session = self
                .client
                .send("Target.attachToTarget", json!({ "targetId": target_id, "flatten": true }), None, CMD_TIMEOUT)
                .await
                .context("Target.attachToTarget")?;
            let session_id = session
                .get("sessionId")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("attachToTarget: no sessionId"))?
                .to_string();
            self.shared.register(&session_id, target_id);
            let page = Page::new(
                self.handle.clone(),
                self.client.clone(),
                session_id.clone(),
                target_id.to_string(),
                Arc::clone(&self.shared),
            );
            page.enable().await?;
            // The popup's navigation started before we attached: read where
            // it stands so the next op waits for it instead of reading the
            // initial about:blank.
            let target_url = self
                .client
                .send("Target.getTargetInfo", json!({ "targetId": target_id }), None, CMD_TIMEOUT)
                .await
                .ok()
                .and_then(|i| i.pointer("/targetInfo/url").and_then(Value::as_str).map(str::to_string))
                .unwrap_or_default();
            let doc = self
                .client
                .send(
                    "Runtime.evaluate",
                    json!({ "expression": "[document.readyState, location.href]", "returnByValue": true }),
                    Some(&session_id),
                    CMD_TIMEOUT,
                )
                .await
                .ok()
                .and_then(|r| r.pointer("/result/value").cloned())
                .unwrap_or_default();
            let ready = doc.get(0).and_then(Value::as_str).unwrap_or("complete");
            let href = doc.get(1).and_then(Value::as_str).unwrap_or("");
            let pending = href == "about:blank" && !target_url.is_empty() && target_url != "about:blank";
            self.shared.seed_nav(&session_id, ready, pending);
            Ok(page)
        })
    }

    pub fn new_page(&self, url: Option<&str>) -> Result<Page> {
        self.new_page_in(url, false)
    }

    /// [`Browser::new_page`] in a window of its own: in a user's browser
    /// the agent works beside their tabs, not among them.
    pub fn new_page_in(&self, url: Option<&str>, new_window: bool) -> Result<Page> {
        let mut params = json!({ "url": url.unwrap_or("about:blank") });
        if new_window {
            params["newWindow"] = json!(true);
        }
        self.block_on(async {
            let target = self
                .client
                .send("Target.createTarget", params, None, CMD_TIMEOUT)
                .await
                .context("Target.createTarget")?;
            let target_id = target
                .get("targetId")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("createTarget: no targetId"))?
                .to_string();
            let session = self
                .client
                .send("Target.attachToTarget", json!({ "targetId": target_id, "flatten": true }), None, CMD_TIMEOUT)
                .await
                .context("Target.attachToTarget")?;
            let session_id = session
                .get("sessionId")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("attachToTarget: no sessionId"))?
                .to_string();
            self.shared.register(&session_id, &target_id);
            let page =
                Page::new(self.handle.clone(), self.client.clone(), session_id, target_id, Arc::clone(&self.shared));
            page.enable().await?;
            Ok(page)
        })
    }

    /// List open tab targets: (target_id, url).
    pub fn tab_targets(&self) -> Result<Vec<(String, String)>> {
        self.block_on(async {
            let targets = self.client.send("Target.getTargets", json!({}), None, CMD_TIMEOUT).await?;
            let mut out = Vec::new();
            if let Some(list) = targets.get("targetInfos").and_then(Value::as_array) {
                for t in list {
                    if t.get("type").and_then(Value::as_str) != Some("page") {
                        continue;
                    }
                    let id = t.get("targetId").and_then(Value::as_str).unwrap_or("");
                    let url = t.get("url").and_then(Value::as_str).unwrap_or("");
                    out.push((id.to_string(), url.to_string()));
                }
            }
            Ok(out)
        })
    }

    /// `Browser.close`: the browser shuts down gracefully (profile saved).
    /// The reply may never come: the connection drops as it exits.
    pub fn close_remote(&self) -> Result<()> {
        let _ = self.block_on(self.client.send("Browser.close", json!({}), None, Duration::from_secs(5)));
        Ok(())
    }

    pub fn activate_target(&self, target_id: &str) -> Result<()> {
        self.block_on(async {
            self.client.send("Target.activateTarget", json!({ "targetId": target_id }), None, CMD_TIMEOUT).await?;
            Ok(())
        })
    }

    pub fn close_target(&self, target_id: &str) -> Result<()> {
        self.block_on(async {
            self.client.send("Target.closeTarget", json!({ "targetId": target_id }), None, CMD_TIMEOUT).await?;
            Ok(())
        })
    }

    /// If the browser process has exited (crashed, killed), how.
    /// Exit status plus the browser's last stderr lines, for errors when
    /// the browser died (at launch or mid-session).
    pub fn death_report(&self) -> String {
        let status = self.exit_status().unwrap_or_else(|| "no exit status yet".into());
        match self.stderr.as_ref().map(|s| s.tail()).filter(|t| !t.is_empty()) {
            Some(tail) => format!("exited: {status}; its stderr ends with:\n{tail}"),
            None => format!("exited: {status}"),
        }
    }

    pub fn exit_status(&self) -> Option<String> {
        let mut slot = self.child.lock().ok()?;
        let child = slot.as_mut()?;
        match child.try_wait() {
            Ok(Some(status)) => Some(status.to_string()),
            _ => None,
        }
    }

    /// Shut the browser down: kill it, delete its profile, reap it.
    ///
    /// The profile is a throwaway temp dir, so Chrome's graceful shutdown
    /// (flushing prefs, history, caches) saves nothing we keep — it only
    /// costs time: navigera used to send `Browser.close` and wait up to
    /// 500 ms for the process to exit, which every session and cold start
    /// paid. go-rod (leakless) and chromiumoxide (kill-on-drop) don't wait
    /// for it either. Renderer and helper processes exit on their own once
    /// the browser process is gone.
    ///
    /// On Unix nothing here waits for the kernel to tear the killed browser
    /// down (~20 ms for Chrome): SIGKILL can't be ignored, the profile is
    /// deleted meanwhile (unlinking files a dying process still holds is
    /// fine, and a removed directory takes no new entries), and the zombie
    /// is reaped on a background thread — or by init once we exit. Only if
    /// the delete fails (a file appeared mid-walk) do we wait and retry.
    /// Windows can't delete files a process still holds open, so there the
    /// browser is reaped first.
    pub fn close(&self) {
        let started = std::time::Instant::now();
        self.client.shutdown();
        let profile = self.profile_dir.lock().ok().and_then(|mut slot| slot.take());
        let Some(mut child) = self.child.lock().ok().and_then(|mut slot| slot.take()) else {
            drop(profile);
            crate::timing::record("close", started);
            return;
        };
        if self.kill_group {
            // Negative pid = the whole process group (wrapper and all).
            #[cfg(unix)]
            {
                extern "C" {
                    fn kill(pid: i32, sig: i32) -> i32;
                }
                // SAFETY: plain syscall; the group was created at spawn.
                unsafe {
                    kill(-(child.id() as i32), 9);
                }
            }
        }
        if let Some(job) = &self.job {
            job.terminate();
        }
        let _ = child.kill();
        let deleted = cfg!(unix) && profile.as_ref().is_none_or(|p| std::fs::remove_dir_all(p.path()).is_ok());
        if deleted {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        } else {
            let _ = child.wait();
        }
        // Deletes the profile if it is still there (a no-op otherwise).
        drop(profile);
        crate::timing::record("close", started);
    }
}

/// True when `xvfb-run` is on PATH.
fn xvfb_available() -> bool {
    std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p).any(|d| d.join(if cfg!(windows) { "xvfb-run.exe" } else { "xvfb-run" }).is_file())
        })
        .unwrap_or(false)
}

/// Pick a free localhost TCP port by binding to port 0 and releasing it.
fn free_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").context("bind 127.0.0.1:0 for a free port")?;
    Ok(listener.local_addr()?.port())
}
