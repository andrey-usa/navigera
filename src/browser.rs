//! Browser sessions on the from-scratch CDP engine (`crate::cdp`).
//!
//! `BrowserSession` is the unit the CLI drives: one browser, a tab list with
//! an active tab, navigation with retry, and the script/text helpers the
//! agent needs. The wire protocol in `navigera.rs` is unchanged.

use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::de::DeserializeOwned;

use crate::cdp::{self, AxOptions, Browser, DialogPolicy, LaunchOptions, Page, Target, DEFAULT_ELEMENT_WAIT};

/// Tokio runtime for the CDP engine: 2 workers is plenty for a sequential
/// CLI, and keeps RSS/CPU far below a default multi-thread runtime.
fn engine_runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("cdp-engine")
        .build()
        .context("build CDP engine runtime")
}

/// Chromium launch flags.
///
/// Beyond the basics, this is the standard automation set that go-rod,
/// chromiumoxide, Puppeteer and Playwright all ship by default: no
/// background networking, component updates, crash reporter, sync, metrics
/// or Translate competing with the page for CPU, and no throttling of
/// background tabs. `--no-startup-window` skips Chrome's initial tab (the
/// session opens its own), saving a renderer process per launch. Site
/// isolation is relaxed as go-rod does, so cross-site iframes share a
/// renderer instead of each spawning one (we already run `--no-sandbox`).
/// Chrome honours only the last `--disable-features` / `--enable-features`
/// flag, so each list is a single flag.
const CHROME_FLAGS: &[&str] = &[
    "--no-sandbox",
    "--disable-dev-shm-usage",
    "--disable-blink-features=AutomationControlled",
    "--no-first-run",
    "--no-default-browser-check",
    "--no-startup-window",
    "--disable-default-apps",
    "--disable-infobars",
    "--window-size=1440,900",
    "--disable-background-networking",
    "--disable-background-timer-throttling",
    "--disable-backgrounding-occluded-windows",
    "--disable-renderer-backgrounding",
    "--disable-breakpad",
    "--disable-client-side-phishing-detection",
    "--disable-component-extensions-with-background-pages",
    "--disable-component-update",
    "--disable-extensions",
    "--disable-hang-monitor",
    "--disable-ipc-flooding-protection",
    "--disable-popup-blocking",
    "--disable-prompt-on-repost",
    "--disable-sync",
    "--metrics-recording-only",
    "--password-store=basic",
    "--use-mock-keychain",
    "--force-color-profile=srgb",
    "--disable-site-isolation-trials",
    // Not `OptimizationHints`: disabling it makes Chrome 151 segfault on the
    // first page (bisected in chrome-bisect run 37464048004; every other
    // feature here is fine there). Its hint fetches are network work that
    // `--disable-background-networking` already stops.
    "--disable-features=Translate,TranslateUI,MediaRouter,DialMediaRouteProvider,AutofillServerCommunication,CertificateTransparencyComponentUpdater,InterestFeedContentSuggestions,site-per-process",
    "--enable-features=NetworkService,NetworkServiceInProcess",
];

/// [`CHROME_FLAGS`] adjusted by two diagnostic knobs, for bisecting a
/// browser-version-specific failure in CI without a rebuild:
/// `NAVIGERA_DROP_FLAGS` (comma-separated prefixes to remove) and
/// `NAVIGERA_EXTRA_FLAGS` (space-separated flags to add).
fn chrome_flags() -> Vec<String> {
    let drop: Vec<String> = std::env::var("NAVIGERA_DROP_FLAGS")
        .map(|v| v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect())
        .unwrap_or_default();
    let mut flags: Vec<String> = CHROME_FLAGS
        .iter()
        .filter(|f| !drop.iter().any(|d| f.starts_with(d.as_str())))
        .map(|f| f.to_string())
        .collect();
    if let Ok(extra) = std::env::var("NAVIGERA_EXTRA_FLAGS") {
        flags.extend(extra.split_whitespace().map(str::to_string));
    }
    flags
}

/// Common Chromium/Chrome/Edge install paths used when no override is given.
fn common_executables() -> Vec<String> {
    let mut paths = Vec::new();
    // Windows: Chrome first (what the tests and CI target), Edge (always
    // installed on Windows) as the fallback.
    let program_dirs: Vec<String> = ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"]
        .iter()
        .filter_map(|var| std::env::var(var).ok())
        .collect();
    for p in &program_dirs {
        paths.push(format!("{p}\\Google\\Chrome\\Application\\chrome.exe"));
    }
    if let Ok(p) = std::env::var("LOCALAPPDATA") {
        paths.push(format!("{p}\\Google\\Chrome\\Application\\chrome.exe"));
    }
    for p in &program_dirs {
        paths.push(format!("{p}\\Microsoft\\Edge\\Application\\msedge.exe"));
    }
    paths.push("/usr/bin/google-chrome".into());
    paths.push("/usr/bin/google-chrome-stable".into());
    paths.push("/usr/bin/chromium".into());
    paths.push("/usr/bin/chromium-browser".into());
    paths.push("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".into());
    paths.push("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge".into());
    paths
}

/// Browsers installed by Playwright / Puppeteer / `@puppeteer/browsers`
/// (newest first), so a machine with only those still works out of the box.
/// For headless sessions chrome-headless-shell comes first: it starts 2-3x
/// faster than full Chrome (no browser UI layer to bring up).
fn cached_browsers(headless: bool) -> Vec<String> {
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_default();
    let mut roots = vec![format!("{home}/.cache/ms-playwright")];
    if let Ok(p) = std::env::var("LOCALAPPDATA") {
        roots.push(format!("{p}/ms-playwright"));
    }
    roots.push(format!("{home}/Library/Caches/ms-playwright"));
    if let Ok(p) = std::env::var("PLAYWRIGHT_BROWSERS_PATH") {
        roots.insert(0, p);
    }
    let pick = |root: &str, prefix: &str, rel: &[&str]| -> Vec<String> {
        let mut dirs: Vec<String> = std::fs::read_dir(root)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.starts_with(prefix))
                    .collect()
            })
            .unwrap_or_default();
        dirs.sort_by(|a, b| b.cmp(a));
        dirs.iter()
            .flat_map(|d| rel.iter().map(move |r| format!("{root}/{d}/{r}")))
            .filter(|p| Path::new(p).is_file())
            .collect()
    };
    let mut shells = Vec::new();
    let mut full = Vec::new();
    for root in &roots {
        shells.extend(pick(
            root,
            "chromium_headless_shell-",
            &[
                "chrome-linux/headless_shell",
                "chrome-headless-shell-linux64/chrome-headless-shell",
                "chrome-mac/headless_shell",
                "chrome-win/headless_shell.exe",
                "chrome-headless-shell-win64/chrome-headless-shell.exe",
            ],
        ));
        full.extend(pick(
            root,
            "chromium-",
            &[
                "chrome-linux/chrome",
                "chrome-linux64/chrome",
                "chrome-mac/Chromium.app/Contents/MacOS/Chromium",
                "chrome-win/chrome.exe",
                "chrome-win64/chrome.exe",
            ],
        ));
    }
    let puppeteer = format!("{home}/.cache/puppeteer");
    shells.extend(pick(
        &format!("{puppeteer}/chrome-headless-shell"),
        "linux-",
        &["chrome-headless-shell-linux64/chrome-headless-shell"],
    ));
    shells.extend(pick(
        &format!("{puppeteer}/chrome-headless-shell"),
        "win64-",
        &["chrome-headless-shell-win64/chrome-headless-shell.exe"],
    ));
    full.extend(pick(&format!("{puppeteer}/chrome"), "linux-", &["chrome-linux64/chrome"]));
    full.extend(pick(&format!("{puppeteer}/chrome"), "win64-", &["chrome-win64/chrome.exe"]));
    if headless {
        shells.into_iter().chain(full).collect()
    } else {
        full
    }
}

/// Resolve which executable to launch, honoring (in order) an explicit CLI
/// override, `$NAVIGERA_CHROMIUM` / `$CHROME_BIN`, the common install
/// paths, then browsers cached by Playwright/Puppeteer.
pub fn resolve_executable(cli_override: Option<&str>) -> Option<String> {
    resolve_executable_for(cli_override, true)
}

/// [`resolve_executable`] for a headed or headless launch.
pub fn resolve_executable_for(cli_override: Option<&str>, headless: bool) -> Option<String> {
    if let Some(path) = cli_override {
        if !path.trim().is_empty() {
            return Some(path.to_string());
        }
    }
    for var in ["NAVIGERA_CHROMIUM", "CHROME_BIN"] {
        if let Ok(path) = std::env::var(var) {
            if !path.trim().is_empty() {
                return Some(path);
            }
        }
    }
    common_executables()
        .into_iter()
        .find(|p| Path::new(p).exists())
        .or_else(|| cached_browsers(headless).into_iter().next())
}

/// True when `--engine` selects the Lightpanda CDP server instead of Chromium.
pub fn is_lightpanda(engine: &str) -> bool {
    matches!(engine.trim().to_ascii_lowercase().as_str(), "lightpanda" | "panda")
}

/// Resolve the `lightpanda` binary for direct `lightpanda serve` launch.
///
/// Order: explicit `--chromium <path>` (kept as the generic binary override),
/// `$LIGHTPANDA_BIN`, then `lightpanda` on `PATH`.
fn resolve_lightpanda_bin(cli_override: Option<&str>) -> Result<String> {
    if let Some(path) = cli_override {
        if !path.trim().is_empty() {
            // `--chromium` doubles as the lightpanda override, so a Chromium
            // path that leaked in from $CHROME_BIN-style defaults would be
            // launched as `chrome serve --port …` and then time out waiting
            // for a /json/version that never comes. Fail fast instead.
            let file =
                Path::new(path).file_name().map(|f| f.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if ["chrome", "chromium", "msedge", "edge", "brave"].iter().any(|b| file.contains(b)) {
                bail!(
                    "--engine lightpanda was given a Chromium binary ({path}); pass the lightpanda binary via --chromium or $LIGHTPANDA_BIN"
                );
            }
            return Ok(path.to_string());
        }
    }
    if let Ok(path) = std::env::var("LIGHTPANDA_BIN") {
        if !path.trim().is_empty() {
            return Ok(path);
        }
    }
    if let Ok(path) = which_lightpanda() {
        return Ok(path);
    }
    bail!("lightpanda binary not found — install it or set --chromium <path> / $LIGHTPANDA_BIN")
}

/// `lightpanda` on `PATH` (no external `which` dependency).
fn which_lightpanda() -> Result<String> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(if cfg!(windows) { "lightpanda.exe" } else { "lightpanda" });
        if candidate.is_file() {
            return Ok(candidate.to_string_lossy().into_owned());
        }
    }
    anyhow::bail!("lightpanda not on PATH")
}

/// A browser navigera connects to instead of launching its own.
#[derive(Debug, Clone)]
pub enum AttachTarget {
    /// `--attach <chrome|beta|…|user-data-dir|ws://…|http://…|port>`.
    Running(String),
    /// `--profile <name|dir>`: the persistent navigera browser on that
    /// profile, started when it isn't running.
    Profile { name: String, executable: Option<String>, headless: bool },
}

/// CDP transport when none is asked for. The pipe (fds 3/4) is Unix-only
/// here; Windows uses a DevTools WebSocket on a random localhost port, and
/// a kill-on-close job object stands in for the pipe's "Chrome exits with
/// its driver" guarantee (see `cdp::procjob`).
pub const DEFAULT_TRANSPORT: &str = if cfg!(unix) { "pipe" } else { "ws" };

/// How long a click on a link/submit button or an Enter key waits for the
/// navigation it probably triggers to be requested (it can trail the input
/// event's CDP reply by a task or two). Ends early once it arrives.
const NAV_GRACE: Duration = Duration::from_millis(150);

/// One browser + tab list, driven by the from-scratch CDP engine.
pub struct BrowserSession {
    browser: Browser,
    pages: Vec<Page>,
    active: usize,
    /// Executable that was launched (browser binary or `lightpanda`); kept so
    /// [`BrowserSession::close`] can run engine-specific cleanup.
    exe: String,
    /// True when the underlying engine is Lightpanda, which does not emit the
    /// CDP `Page.loadEventFired` event — those sessions navigate with a
    /// `commit` wait instead.
    lightpanda: bool,
    timeout_ms: f64,
    /// (tab target, navigation generation) whose load we stopped waiting
    /// for after a full navigation timeout (a page that streams forever,
    /// a hung server). Later ops don't wait on it again.
    abandoned: std::sync::Mutex<Option<(String, u64)>>,
    /// Connected to a browser navigera doesn't own (`--attach`,
    /// `--profile`): closing only disconnects.
    attached: bool,
    /// Owns the tokio runtime; declared LAST so it drops last, after the
    /// browser/client/pages that use it.
    _runtime: tokio::runtime::Runtime,
}

impl BrowserSession {
    /// Launch a headless CDP browser ready for navigation.
    ///
    /// `engine` is `chrome` (Chromium/Chrome/Edge) or `lightpanda` (a
    /// Lightpanda CDP server, started directly as `lightpanda serve`).
    pub fn launch(engine: &str, headless: bool, executable: Option<&str>, nav_timeout_ms: f64) -> Result<Self> {
        Self::launch_with(engine, headless, executable, nav_timeout_ms, None)
    }

    /// [`BrowserSession::launch`] with an explicit CDP transport: `pipe`
    /// (default) or `ws` (a DevTools port, e.g. to attach Chrome DevTools).
    pub fn launch_with(
        engine: &str,
        headless: bool,
        executable: Option<&str>,
        nav_timeout_ms: f64,
        transport: Option<&str>,
    ) -> Result<Self> {
        let transport = transport
            .map(str::to_string)
            .or_else(|| std::env::var("NAVIGERA_CDP_TRANSPORT").ok())
            .unwrap_or_else(|| DEFAULT_TRANSPORT.into());
        if !matches!(transport.as_str(), "pipe" | "ws") {
            bail!("--transport takes pipe|ws, got {transport:?}");
        }
        if transport == "pipe" && !cfg!(unix) {
            bail!("--transport pipe needs Linux/macOS; Windows uses ws (the default there)");
        }
        let launch_started = std::time::Instant::now();
        let lightpanda = is_lightpanda(engine);
        let runtime = engine_runtime()?;
        let handle = runtime.handle().clone();

        let (browser, exe) = if lightpanda {
            // Lightpanda is a CDP server, not a Chromium executable: start
            // `lightpanda serve` directly and connect to its CDP endpoint.
            // No Chromium flags, no shim, no X server involved.
            let bin = resolve_lightpanda_bin(executable)?;
            let browser = Browser::launch_lightpanda(&handle, &bin)?;
            crate::timing::log(&format!("[browser] engine=lightpanda launched {} via {bin}", browser.ws_url()));
            (browser, bin)
        } else {
            let Some(exe) = resolve_executable_for(executable, headless) else {
                bail!(
                    "no Chrome/Chromium/Edge found — install Chrome, or `npx @puppeteer/browsers install chrome-headless-shell@stable` and pass its path via --chromium or $CHROME_BIN"
                );
            };
            let browser = Browser::launch(
                &handle,
                &LaunchOptions {
                    exe: exe.clone(),
                    headless,
                    chrome_flags: chrome_flags(),
                    debugging_port: None,
                    pipe: transport == "pipe",
                },
            )?;
            crate::timing::log(&format!(
                "[browser] engine=chrome launched {} via {exe} (headless={headless})",
                browser.ws_url(),
            ));
            (browser, exe)
        };
        crate::timing::record("browser_up", launch_started);
        let page_started = std::time::Instant::now();
        let page = if lightpanda {
            browser.new_page_lightpanda(Some("about:blank"))?
        } else {
            // The first command is where a browser that died at startup
            // (missing libraries, bad flag) shows up: say why.
            browser.new_page(Some("about:blank")).map_err(|e| anyhow::anyhow!("{e:#} ({})", browser.death_report()))?
        };
        crate::timing::record("first_page", page_started);
        crate::timing::record("launch_total", launch_started);
        Ok(Self {
            browser,
            pages: vec![page],
            active: 0,
            exe,
            lightpanda,
            timeout_ms: nav_timeout_ms,
            abandoned: std::sync::Mutex::new(None),
            attached: false,
            _runtime: runtime,
        })
    }

    /// Work in a browser that is already running (`--attach`) or in the
    /// persistent `--profile` browser (started first if needed). The agent
    /// gets a window of its own; closing the session only disconnects —
    /// the browser, its profile and every window stay open.
    pub fn attach(target: &AttachTarget, nav_timeout_ms: f64) -> Result<Self> {
        let started = std::time::Instant::now();
        let runtime = engine_runtime()?;
        let handle = runtime.handle().clone();
        let (ws_url, what) = match target {
            AttachTarget::Running(spec) => (cdp::attach::resolve(spec)?, format!("attach {spec}")),
            AttachTarget::Profile { name, executable, headless } => {
                let dir = cdp::attach::profile_dir(name)?;
                let Some(exe) = resolve_executable_for(executable.as_deref(), *headless) else {
                    bail!("no Chrome/Chromium/Edge found for --profile — pass --chromium <path> or set $CHROME_BIN");
                };
                let url = cdp::attach::ensure_profile_browser(&exe, &dir, *headless)?;
                (url, format!("profile {}", dir.display()))
            }
        };
        // Chrome asks the user to Allow a connection to their own profile:
        // give them time to click.
        let browser = Browser::connect(&handle, &ws_url, Duration::from_secs(120))?;
        crate::timing::log(&format!("[browser] {what}: connected to {ws_url}"));
        crate::timing::record("browser_up", started);
        let page = browser.new_page_in(Some("about:blank"), true)?;
        crate::timing::record("launch_total", started);
        Ok(Self {
            browser,
            pages: vec![page],
            active: 0,
            exe: what,
            lightpanda: false,
            timeout_ms: nav_timeout_ms,
            abandoned: std::sync::Mutex::new(None),
            attached: true,
            _runtime: runtime,
        })
    }

    /// True when connected to a browser navigera doesn't own.
    pub fn is_attached(&self) -> bool {
        self.attached
    }

    /// Ask the browser itself to shut down (`quit --close-browser` on an
    /// attached session): Chrome closes its windows and saves its profile.
    pub fn close_browser(&self) -> Result<()> {
        self.browser.close_remote()
    }

    fn timeout(&self, override_ms: Option<f64>) -> Duration {
        Duration::from_secs_f64(override_ms.unwrap_or(self.timeout_ms) / 1000.0)
    }

    /// (element auto-wait, CDP command timeout) for an element op: the op's
    /// own `timeout_ms` bounds the wait; without one, the element wait is
    /// [`DEFAULT_ELEMENT_WAIT`] (fast feedback on a wrong selector).
    fn element_budget(&self, override_ms: Option<f64>) -> (Duration, Duration) {
        let wait = match override_ms {
            Some(ms) => Duration::from_secs_f64(ms / 1000.0),
            None => DEFAULT_ELEMENT_WAIT.min(self.timeout(None)),
        };
        (wait, self.timeout(None).max(wait + Duration::from_secs(1)))
    }

    fn active_tab(&self) -> &Page {
        &self.pages[self.active]
    }

    /// Number of open pages (tabs) in this session.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Index of the active tab.
    pub fn active_page(&self) -> usize {
        self.active
    }

    /// Target ids of all open tabs.
    pub fn page_targets(&self) -> Vec<String> {
        self.pages.iter().map(|p| p.target_id().to_string()).collect()
    }

    /// Open a new tab and make it active; returns its index.
    pub fn new_page(&mut self) -> Result<usize> {
        if self.lightpanda {
            // Lightpanda's CDP is single-target: `Target.createTarget` replies
            // `TargetAlreadyLoaded` as soon as a page exists.
            bail!("the lightpanda engine supports one tab only; use the chrome engine for tabs");
        }
        let page = self.browser.new_page(Some("about:blank"))?;
        self.pages.push(page);
        self.active = self.pages.len() - 1;
        Ok(self.active)
    }

    /// Switch the active tab by zero-based index.
    pub fn select_page(&mut self, index: usize) -> Result<()> {
        if index >= self.pages.len() {
            bail!("no tab {index} (have {})", self.pages.len());
        }
        self.active = index;
        self.bring_to_front()
    }

    /// Make the active tab the foreground one.
    fn bring_to_front(&self) -> Result<()> {
        self.browser.activate_target(self.active_tab().target_id())
    }

    /// Close one tab by index; the last remaining tab cannot be closed.
    pub fn close_page(&mut self, index: usize) -> Result<()> {
        if self.pages.len() <= 1 {
            bail!("refusing to close the session's only tab; use `quit` instead");
        }
        if index >= self.pages.len() {
            bail!("no tab {index} (have {})", self.pages.len());
        }
        let timeout = self.timeout(None);
        self.pages[index].close_target(timeout)?;
        self.pages.remove(index);
        let closed_active = index == self.active;
        if self.active >= self.pages.len() {
            self.active = self.pages.len() - 1;
        } else if index < self.active {
            self.active -= 1;
        }
        if closed_active {
            // The tab we act on next was in the background.
            self.bring_to_front()?;
        }
        Ok(())
    }

    /// Adopt tabs the page opened itself (`target=_blank`, `window.open`)
    /// and drop tabs the page closed (`window.close()`). A newly opened tab
    /// becomes active, as it would in a visible browser. Returns the
    /// indices of adopted tabs.
    pub fn sync_tabs(&mut self) -> Vec<usize> {
        let shared = std::sync::Arc::clone(self.browser.shared());
        let mut adopted = Vec::new();
        for (target_id, opener) in shared.take_opened() {
            if self.pages.iter().any(|p| p.target_id() == target_id) {
                continue;
            }
            // In someone else's browser, only tabs our own tabs opened are
            // ours; the user's popups stay theirs.
            if self.attached && !self.pages.iter().any(|p| p.target_id() == opener) {
                continue;
            }
            match self.browser.attach_page(&target_id) {
                Ok(page) => {
                    self.pages.push(page);
                    self.active = self.pages.len() - 1;
                    adopted.push(self.active);
                }
                Err(e) => eprintln!("[browser] could not adopt new tab {target_id}: {e:#}"),
            }
        }
        for target_id in shared.take_destroyed() {
            if self.pages.len() <= 1 {
                break;
            }
            if let Some(index) = self.pages.iter().position(|p| p.target_id() == target_id) {
                shared.unregister(self.pages[index].session_id());
                self.pages.remove(index);
                if self.active >= self.pages.len() {
                    self.active = self.pages.len() - 1;
                } else if index < self.active {
                    self.active -= 1;
                }
            }
        }
        adopted
    }

    /// Dialogs answered since the last call (`{type, message, accepted}`).
    pub fn take_dialogs(&self) -> Vec<serde_json::Value> {
        self.browser.shared().take_dialogs()
    }

    /// How future `alert`/`confirm`/`prompt` dialogs are answered.
    pub fn set_dialog_policy(&self, accept: bool, prompt_text: Option<String>) {
        self.browser.set_dialog_policy(DialogPolicy { accept, prompt_text });
    }

    /// Wait for a navigation an earlier action started in the active tab.
    /// Never fails: after the full navigation timeout the load is
    /// abandoned (later ops don't wait on it again) and the op goes on with
    /// whatever document is there. Returns true when the tab settled.
    pub fn settle(&self) -> bool {
        self.settle_within(self.timeout(None))
    }

    /// [`BrowserSession::settle`] bounded by `budget` (an op's own
    /// timeout). Only a wait that used the whole navigation timeout marks
    /// the load abandoned; a shorter op budget leaves it for the next op.
    pub fn settle_within(&self, budget: Duration) -> bool {
        let tab = self.active_tab();
        let key = (tab.target_id().to_string(), tab.nav_generation());
        if self.abandoned.lock().ok().is_some_and(|a| a.as_ref() == Some(&key)) {
            return false;
        }
        if tab.settle(budget).is_ok() {
            return true;
        }
        if budget >= self.timeout(None) {
            if let Ok(mut a) = self.abandoned.lock() {
                *a = Some(key);
            }
        }
        false
    }

    /// Settle after an action: up to the op's own `timeout_ms` if it gave
    /// one, else the navigation timeout.
    fn settle_for(&self, timeout_ms: Option<f64>) -> bool {
        self.settle_within(
            timeout_ms.map(|ms| Duration::from_secs_f64(ms / 1000.0)).unwrap_or_else(|| self.timeout(None)),
        )
    }

    /// Let fetch/XHR data the active tab is loading arrive before a read
    /// (see [`Page::network_quiet`]).
    pub fn network_quiet(&self, quiet: Duration, max: Duration) -> bool {
        self.active_tab().network_quiet(quiet, max)
    }

    /// The default navigation timeout (`--timeout-ms`).
    pub fn nav_timeout(&self) -> Duration {
        self.timeout(None)
    }

    /// True while the active tab's document is still loading (before
    /// DOMContentLoaded), e.g. after a settle gave up.
    pub fn is_loading(&self) -> bool {
        self.active_tab().is_loading()
    }

    /// Current URL and title in one round trip.
    pub fn url_title(&self) -> (String, String) {
        let v = self.active_tab().evaluate("[location.href, document.title]", self.timeout(None)).unwrap_or_default();
        let get = |i: usize| v.get(i).and_then(|x| x.as_str()).unwrap_or_default().to_string();
        (get(0), get(1))
    }

    /// Current page URL.
    pub fn url(&self) -> String {
        self.active_tab().url(self.timeout(None)).unwrap_or_default()
    }

    /// Navigate the active tab and wait for `load`.
    pub fn goto(&self, url: &str) -> Result<()> {
        self.goto_wait(url, "load", None)
    }

    /// Navigate the active tab; `wait` is `load`, `domcontentloaded` or
    /// `commit`. Retries benign aborts (third-party trackers can abort the
    /// main-frame load mid-flight).
    ///
    /// Lightpanda sessions use a `commit` wait: Lightpanda emits no
    /// `Page.loadEventFired`, so a full wait would just burn the navigation
    /// timeout. Callers confirm content by polling the DOM.
    pub fn goto_wait(&self, url: &str, wait: &str, timeout_ms: Option<f64>) -> Result<()> {
        if !matches!(wait, "load" | "domcontentloaded" | "commit") {
            bail!("--wait takes load|domcontentloaded|commit, got {wait:?}");
        }
        let (wait, timeout) = if self.lightpanda {
            ("commit", Duration::from_millis(500))
        } else {
            // Honors --timeout-ms (was a hard-coded 40 s per attempt).
            (wait, self.timeout(timeout_ms))
        };
        let max_attempts = 3;
        let mut attempt = 0;
        loop {
            match self.active_tab().navigate(url, wait, timeout) {
                Ok(_) => return Ok(()),
                Err(e) => {
                    let msg = format!("{e:#}");
                    if self.lightpanda && msg.contains("timed out") {
                        return Ok(());
                    }
                    attempt += 1;
                    let transient = msg.contains("ERR_ABORTED") || msg.contains("ERR_CONNECTION_RESET");
                    if transient && attempt <= max_attempts {
                        eprintln!("[browser] nav hiccup (attempt {attempt}): {msg}");
                        std::thread::sleep(Duration::from_millis(500 * attempt as u64));
                        continue;
                    }
                    return Err(e);
                }
            }
        }
    }

    /// Navigate without waiting for load (commit semantics).
    pub fn goto_commit(&self, url: &str, timeout_ms: Option<f64>) -> Result<()> {
        self.active_tab().navigate_commit(url, self.timeout(timeout_ms))
    }

    pub fn back(&self) -> Result<()> {
        self.active_tab().history(-1, self.timeout(None))
    }

    pub fn forward(&self) -> Result<()> {
        self.active_tab().history(1, self.timeout(None))
    }

    pub fn reload(&self) -> Result<()> {
        self.active_tab().reload(self.timeout(None))
    }

    /// Run a JS expression in the active tab; result must be JSON.
    pub fn evaluate<T: DeserializeOwned>(&self, expression: &str) -> Result<T> {
        self.evaluate_with_timeout(expression, None)
    }

    /// Run a JS expression with an explicit timeout override.
    pub fn evaluate_with_timeout<T: DeserializeOwned>(&self, expression: &str, timeout_ms: Option<f64>) -> Result<T> {
        let value = self.active_tab().evaluate(expression, self.timeout(timeout_ms))?;
        Ok(serde_json::from_value(value)?)
    }

    /// Click the element `target` names (trusted mouse click when hittable).
    /// If the click starts a navigation, waits for the new document.
    pub fn click_target(&self, target: &Target, timeout_ms: Option<f64>) -> Result<serde_json::Value> {
        let (wait, timeout) = self.element_budget(timeout_ms);
        let generation = self.active_tab().nav_generation();
        let point = self.active_tab().click(target, wait, timeout)?;
        if point.get("nav").and_then(|v| v.as_bool()) == Some(true) {
            self.active_tab().navigation_started(generation, NAV_GRACE);
        }
        self.settle_for(timeout_ms);
        Ok(point)
    }

    pub fn hover(&self, target: &Target, timeout_ms: Option<f64>) -> Result<serde_json::Value> {
        let (wait, timeout) = self.element_budget(timeout_ms);
        self.active_tab().hover(target, wait, timeout)
    }

    pub fn fill_target(&self, target: &Target, value: &str, timeout_ms: Option<f64>) -> Result<()> {
        let (wait, timeout) = self.element_budget(timeout_ms);
        self.active_tab().fill(target, value, wait, timeout)
    }

    pub fn select(&self, target: &Target, values: &[String], timeout_ms: Option<f64>) -> Result<serde_json::Value> {
        let (wait, timeout) = self.element_budget(timeout_ms);
        let chosen = self.active_tab().select(target, values, wait, timeout)?;
        self.settle_for(timeout_ms);
        Ok(chosen)
    }

    pub fn press(&self, target: Option<&Target>, key: &str, timeout_ms: Option<f64>) -> Result<()> {
        let (wait, timeout) = self.element_budget(timeout_ms);
        let generation = self.active_tab().nav_generation();
        let may_navigate = self.active_tab().press(target, key, wait, timeout)?;
        if may_navigate {
            // Enter in a form submits it (implicit submission); on a link
            // it follows it.
            self.active_tab().navigation_started(generation, NAV_GRACE);
        }
        self.settle_for(timeout_ms);
        Ok(())
    }

    pub fn type_text(&self, target: Option<&Target>, text: &str, timeout_ms: Option<f64>) -> Result<()> {
        let (wait, timeout) = self.element_budget(timeout_ms);
        self.active_tab().type_text(target, text, wait, timeout)?;
        self.settle_for(timeout_ms);
        Ok(())
    }

    pub fn scroll(
        &self,
        target: Option<&Target>,
        dy: Option<f64>,
        to: Option<&str>,
        timeout_ms: Option<f64>,
    ) -> Result<serde_json::Value> {
        let (wait, timeout) = self.element_budget(timeout_ms);
        self.active_tab().scroll(target, dy, to, wait, timeout)
    }

    pub fn upload(&self, target: &Target, files: &[String], timeout_ms: Option<f64>) -> Result<()> {
        let (wait, timeout) = self.element_budget(timeout_ms);
        self.active_tab().upload(target, files, wait, timeout)
    }

    /// Poll `js_predicate` (an expression returning a boolean) every 100 ms
    /// until true or `timeout_ms` (default: the element wait) runs out.
    /// Survives navigations (the predicate is re-evaluated in each new
    /// document); returns the waited milliseconds.
    pub fn wait_until(&self, js_predicate: &str, what: &str, timeout_ms: Option<f64>) -> Result<f64> {
        let (wait, timeout) = self.element_budget(timeout_ms);
        let started = std::time::Instant::now();
        loop {
            // A navigation in flight: wait for it, but never past this
            // op's own budget.
            self.settle_within(wait.saturating_sub(started.elapsed()));
            if let Ok(true) = self.active_tab().check(js_predicate, timeout) {
                return Ok((started.elapsed().as_secs_f64() * 1000.0).round());
            }
            if started.elapsed() >= wait {
                bail!("wait: {what} not met within {} ms", wait.as_millis());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Click the first element matching `selector`.
    pub fn click(&self, selector: &str, timeout_ms: Option<f64>) -> Result<()> {
        self.click_target(&Target::Selector(selector.to_string()), timeout_ms).map(|_| ())
    }

    /// Fill the first matching field with `value`.
    pub fn fill(&self, selector: &str, value: &str, timeout_ms: Option<f64>) -> Result<()> {
        self.fill_target(&Target::Selector(selector.to_string()), value, timeout_ms)
    }

    /// Click the node with this `backendNodeId` (an `ax` ref).
    pub fn click_ref(&self, backend_node_id: u64, timeout_ms: Option<f64>) -> Result<()> {
        self.click_target(&Target::Ref(backend_node_id), timeout_ms).map(|_| ())
    }

    /// Fill the node with this `backendNodeId` (an `ax` ref) with `value`.
    pub fn fill_ref(&self, backend_node_id: u64, value: &str, timeout_ms: Option<f64>) -> Result<()> {
        self.fill_target(&Target::Ref(backend_node_id), value, timeout_ms)
    }

    /// First element's text content for `selector`, if present.
    pub fn text(&self, selector: &str) -> Result<Option<String>> {
        self.text_with_timeout(selector, None)
    }

    /// First element's text content with an explicit timeout override.
    pub fn text_with_timeout(&self, selector: &str, timeout_ms: Option<f64>) -> Result<Option<String>> {
        self.active_tab().text_content(selector, self.timeout(timeout_ms))
    }

    /// Page title of the active tab.
    pub fn title(&self) -> Result<String> {
        self.active_tab().title(self.timeout(None))
    }

    /// Accessibility snapshot of the active tab (text tree by default).
    pub fn ax(&self, opts: &AxOptions, timeout_ms: Option<f64>) -> Result<serde_json::Value> {
        self.active_tab().ax_tree(self.timeout(timeout_ms), opts)
    }

    /// Legacy flat accessibility snapshot (`[{ref, role, name, value?}]`).
    pub fn ax_tree(&self, max_depth: Option<u32>, all: bool, timeout_ms: Option<f64>) -> Result<serde_json::Value> {
        self.ax(&AxOptions { max_depth, json: true, all, ..AxOptions::default() }, timeout_ms)
    }

    /// Viewport PNG screenshot bytes (`full_page` captures beyond viewport).
    pub fn screenshot_png(&self, full_page: bool) -> Result<Vec<u8>> {
        self.active_tab().screenshot_png(self.timeout(None), full_page)
    }

    /// True when this session drives Lightpanda rather than Chromium.
    pub fn is_lightpanda(&self) -> bool {
        self.lightpanda
    }

    /// The CDP WebSocket endpoint this session is attached to.
    pub fn endpoint(&self) -> String {
        self.browser.ws_url().to_string()
    }

    /// Shut the browser down. (Lightpanda is now launched directly as
    /// `lightpanda serve` and reaped like Chrome; the old shim's `--stop`
    /// cleanup step no longer applies — the real binary has no such flag.)
    pub fn close(&self) {
        // Never panic from close: a panicking shutdown turns a successful
        // session into a failed process exit.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.browser.close();
        }));
    }

    /// `Some(status)` once the browser process is gone.
    pub fn browser_exit(&self) -> Option<String> {
        self.browser.exit_status().map(|_| self.browser.death_report())
    }

    /// The launched executable (browser binary or `lightpanda`).
    pub fn executable(&self) -> &str {
        &self.exe
    }
}

// Re-exported for the rare caller that still names the engine explicitly.
pub use cdp::{Browser as CdpBrowser, Page as CdpPage};
