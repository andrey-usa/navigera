//! WebDriver engines: Firefox over WebDriver BiDi, Safari over classic
//! WebDriver (`safaridriver`), and any W3C WebDriver server (`--webdriver`).
//!
//! Chrome keeps its own engine (CDP, [`crate::cdp`]). These engines drive
//! the page through what both WebDriver protocols share: run a function in
//! the page, send input actions, navigate, take a screenshot, cookies and
//! tabs. They read the page in page JavaScript: the snapshot comes back as
//! the columns of Chrome's DOM snapshot ([`SNAPSHOT`]), so the same tree
//! builder and printer make `ax`'s text on every engine. Locators, `fill`
//! and `select` run the same page functions Chrome's engine runs. An op
//! only CDP can do says which engine to use instead.

pub mod bidi;
pub mod classic;

use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

use crate::cdp::Target;
use crate::protocol::{Command, Outcome, SessionConfig};

/// `--engine` names served here.
pub const ENGINES: &[&str] = &["firefox", "safari", "safari-preview", "webdriver"];

/// Whether `config` asks for one of these engines.
pub fn wanted(config: &SessionConfig) -> bool {
    config.webdriver.is_some() || ENGINES.contains(&config.engine.as_str())
}

/// The engine `config` names, as `status` and a session's engine check say it.
pub fn engine_name(config: &SessionConfig) -> &'static str {
    if config.webdriver.is_some() {
        return "webdriver";
    }
    match config.engine.as_str() {
        "firefox" => "firefox",
        "safari" | "safari-preview" => "safari",
        _ => "webdriver",
    }
}

/// The page read as Chrome's DOM snapshot columns (see `snapshot.js`).
const SNAPSHOT: &str = include_str!("snapshot.js");

/// The most nodes one snapshot reads: a page past it is read in part, and
/// says so.
const SNAPSHOT_MAX_NODES: u64 = 400_000;

/// Records the elements a page gives click-like listeners, for the
/// snapshot's clickable column. A preload script where the protocol has
/// them (BiDi); classic WebDriver installs it after each navigation, so it
/// misses listeners a page added while it loaded.
pub const LISTEN: &str = r#"() => {
    const K = Symbol.for('navigera.listen');
    if (window[K]) return;
    const seen = new WeakSet();
    Object.defineProperty(window, K, { value: seen, enumerable: false });
    const kinds = new Set(['click', 'mousedown', 'mouseup', 'pointerdown', 'pointerup', 'touchstart', 'touchend']);
    const add = EventTarget.prototype.addEventListener;
    EventTarget.prototype.addEventListener = function (type, fn, opts) {
        if (kinds.has(type) && this && this.nodeType === 1) seen.add(this);
        return add.call(this, type, fn, opts);
    };
}"#;

/// Console entries a page logs, kept in the page for an engine with no log
/// events (classic WebDriver): the newest 500.
pub const CONSOLE_HOOK: &str = r#"() => {
    const K = Symbol.for('navigera.console');
    if (window[K]) return;
    const out = [];
    Object.defineProperty(window, K, { value: out, enumerable: false });
    const keep = (level, args) => {
        let text;
        try { text = args.map((a) => typeof a === 'string' ? a : JSON.stringify(a)).join(' '); } catch (_) { text = args.join(' '); }
        out.push({ level, text, url: location.href, ts: Date.now() });
        if (out.length > 500) out.shift();
    };
    for (const level of ['log', 'info', 'warn', 'error', 'debug']) {
        const orig = console[level];
        console[level] = function (...args) { keep(level === 'warn' ? 'warning' : level, args); return orig.apply(this, args); };
    }
    window.addEventListener('error', (e) => keep('error', [String(e.message || e)]));
    window.addEventListener('unhandledrejection', (e) => keep('error', ['Uncaught (in promise) ' + String(e.reason && e.reason.message || e.reason)]));
}"#;

/// Dialogs answered in the page, for an engine with no dialog events
/// (classic WebDriver): `alert`, `confirm` and `prompt` record what they
/// asked and answer by the session's policy, as BiDi's dialog handling does.
/// Called again, it only changes the policy.
pub const DIALOG_HOOK: &str = r#"(accept, promptText) => {
    const K = Symbol.for('navigera.dialogs');
    const state = window[K] || { seen: [] };
    state.accept = accept; state.promptText = promptText;
    if (window[K]) return;
    Object.defineProperty(window, K, { value: state, enumerable: false });
    const answer = (type, message, def) => {
        state.seen.push({ type, message: String(message === undefined ? '' : message), accepted: type === 'alert' || state.accept });
        if (state.seen.length > 100) state.seen.shift();
        if (type === 'alert') return undefined;
        if (!state.accept) return type === 'prompt' ? null : false;
        return type === 'prompt' ? (state.promptText != null ? state.promptText : (def === undefined ? '' : String(def))) : true;
    };
    window.alert = function (m) { return answer('alert', m); };
    window.confirm = function (m) { return answer('confirm', m); };
    window.prompt = function (m, d) { return answer('prompt', m, d); };
}"#;

/// The element a ref names, from the snapshot's table.
const REF_EL: &str = r#"function (id, what) {
    const T = window[Symbol.for('navigera.ids')];
    const n = T && T.byId.get(id);
    const el = n && n.deref();
    if (!el || !el.isConnected) throw new Error(what + ': ref ' + id + ' is not on the page any more; run `ax` again for fresh refs');
    return el.nodeType === 1 ? el : el.parentElement;
}"#;

/// Where a click lands: the element scrolled into view, its middle in the
/// top window's viewport (frames' offsets added).
const POINT_EL: &str = r#"function (el, force) {
    const box = () => el.getBoundingClientRect();
    let r = box();
    const vw = window.innerWidth, vh = window.innerHeight;
    if (r.top < 0 || r.left < 0 || r.bottom > vh || r.right > vw || el.ownerDocument !== document) {
        el.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
        r = box();
    }
    if (!force) {
        if (!(r.width > 0 && r.height > 0)) throw new Error('the element has no size on the page (hidden or not laid out); `ax` shows what is there');
        if (el.disabled) throw new Error('the element is disabled');
    }
    let x = r.left + r.width / 2, y = r.top + r.height / 2;
    for (let w = el.ownerDocument.defaultView; w && w.frameElement; w = w.parent) {
        const f = w.frameElement.getBoundingClientRect();
        x += f.left + w.frameElement.clientLeft;
        y += f.top + w.frameElement.clientTop;
    }
    return { x: Math.max(0, Math.round(x)), y: Math.max(0, Math.round(y)) };
}"#;

/// The page's location and title.
const URL_TITLE: &str = "() => [location.href, document.title]";

/// How long a navigation waits: until the document is parsed, until it
/// loaded (the default), or not at all.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Wait {
    None,
    Interactive,
    Complete,
}

/// What a WebDriver protocol does for the ops here.
pub trait Wire: Send {
    /// What `status` says about the browser (its name and version).
    fn describe(&self) -> Value;
    fn navigate(&mut self, url: &str, wait: Wait, timeout: Duration) -> Result<()>;
    /// Run `function` (an async function of `argsJson`, a JSON array as
    /// text) in the page; its answer is JSON text (see [`wrap`]).
    fn call(&mut self, function: &str, args_json: &str, timeout: Duration) -> Result<String>;
    /// `eval`: the value of a script, as JSON.
    fn evaluate(&mut self, source: &str, timeout: Duration) -> Result<Value>;
    /// W3C input actions (the same shape in both protocols).
    fn actions(&mut self, actions: Value) -> Result<()>;
    /// Wait for a navigation an action started, if one did.
    fn settle(&mut self, _timeout: Duration) {}
    fn screenshot(&mut self, full_page: bool) -> Result<Vec<u8>>;
    fn history(&mut self, delta: i64) -> Result<()>;
    fn reload(&mut self, timeout: Duration) -> Result<()>;
    fn tabs(&mut self) -> Result<Vec<String>>;
    fn current_tab(&self) -> String;
    fn tab_new(&mut self) -> Result<String>;
    fn tab_select(&mut self, id: &str) -> Result<()>;
    fn tab_close(&mut self, id: &str) -> Result<()>;
    fn cookies(&mut self) -> Result<Vec<Value>>;
    fn set_cookie(&mut self, cookie: Value) -> Result<()>;
    fn clear_cookies(&mut self) -> Result<()>;
    /// Console entries since the last call.
    fn console(&mut self) -> Vec<Value>;
    /// Dialogs answered since the last call.
    fn dialogs(&mut self) -> Vec<Value>;
    fn set_dialog_policy(&mut self, accept: bool, prompt_text: Option<String>);
    /// Where the session runs (the driver's URL, with any key masked).
    fn endpoint(&self) -> String;
    fn close(&mut self);
}

/// `function` as the page runs it for [`Wire::call`]: its arguments from
/// one JSON text, its answer (or error) as one JSON text, so a large answer
/// crosses the protocol as one string and not as a tree of typed values.
pub fn wrap(function: &str) -> String {
    format!(
        "async (argsJson) => {{ try {{ const v = await ({function})(...JSON.parse(argsJson)); \
         return JSON.stringify({{ ok: v === undefined ? null : v }}); }} \
         catch (e) {{ return JSON.stringify({{ err: String(e && e.message || e) }}); }} }}"
    )
}

#[derive(serde::Deserialize)]
struct Reply<'a> {
    #[serde(borrow)]
    ok: Option<&'a serde_json::value::RawValue>,
    err: Option<String>,
}

/// The `ok` of a [`wrap`]ped answer, as JSON text; its `err` as the error.
fn ok_text(answer: &str) -> Result<&str> {
    let reply: Reply = serde_json::from_str(answer).map_err(|e| anyhow!("the page's answer is not JSON ({e})"))?;
    match (reply.ok, reply.err) {
        (_, Some(err)) => Err(anyhow!(err)),
        (Some(ok), None) => Ok(ok.get()),
        (None, None) => Ok("null"),
    }
}

/// A WebDriver key value for a key name (`Enter`, `ArrowDown`, `a`).
fn key_value(name: &str) -> Result<String> {
    let code = match name {
        "Enter" | "Return" => '\u{E007}',
        "Tab" => '\u{E004}',
        "Escape" | "Esc" => '\u{E00C}',
        "Backspace" => '\u{E003}',
        "Delete" | "Del" => '\u{E017}',
        "Space" | " " => return Ok(" ".into()),
        "ArrowUp" | "Up" => '\u{E013}',
        "ArrowDown" | "Down" => '\u{E015}',
        "ArrowLeft" | "Left" => '\u{E012}',
        "ArrowRight" | "Right" => '\u{E014}',
        "Home" => '\u{E011}',
        "End" => '\u{E010}',
        "PageUp" => '\u{E00E}',
        "PageDown" => '\u{E00F}',
        "Insert" => '\u{E016}',
        "Shift" => '\u{E008}',
        "Control" | "Ctrl" => '\u{E009}',
        "Alt" | "Option" => '\u{E00A}',
        "Meta" | "Command" | "Cmd" => '\u{E03D}',
        "ControlOrMeta" => {
            if cfg!(target_os = "macos") {
                '\u{E03D}'
            } else {
                '\u{E009}'
            }
        }
        f if f.len() >= 2 && f.starts_with('F') && f[1..].parse::<u32>().is_ok_and(|n| (1..=12).contains(&n)) => {
            char::from_u32(0xE031 + f[1..].parse::<u32>().unwrap() - 1).unwrap()
        }
        other if other.chars().count() == 1 => return Ok(other.to_string()),
        other => bail!("press: unknown key {other:?} (keys: Enter, Tab, Escape, Backspace, Delete, Arrow*, Home, End, PageUp, PageDown, F1-F12, a character; with modifiers: Control+a)"),
    };
    Ok(code.to_string())
}

/// Key actions for `combo` (`Enter`, `Control+a`, `Shift+Tab`): the
/// modifiers held around the key.
fn press_actions(combo: &str) -> Result<Value> {
    let parts: Vec<&str> = if combo == "+" { vec!["+"] } else { combo.split('+').filter(|p| !p.is_empty()).collect() };
    let (key, mods) = parts.split_last().ok_or_else(|| anyhow!("press: no key"))?;
    let mut down = Vec::new();
    for m in mods {
        down.push(key_value(m)?);
    }
    let key = key_value(key)?;
    let mut acts = Vec::new();
    for m in &down {
        acts.push(json!({ "type": "keyDown", "value": m }));
    }
    acts.push(json!({ "type": "keyDown", "value": key }));
    acts.push(json!({ "type": "keyUp", "value": key }));
    for m in down.iter().rev() {
        acts.push(json!({ "type": "keyUp", "value": m }));
    }
    Ok(json!([{ "type": "key", "id": "keyboard", "actions": acts }]))
}

/// Key actions that type `text`, one character at a time.
fn type_actions(text: &str) -> Value {
    let mut acts = Vec::with_capacity(text.chars().count() * 2);
    for c in text.chars() {
        let v = if c == '\n' { "\u{E007}".to_string() } else { c.to_string() };
        acts.push(json!({ "type": "keyDown", "value": v }));
        acts.push(json!({ "type": "keyUp", "value": v }));
    }
    json!([{ "type": "key", "id": "keyboard", "actions": acts }])
}

/// A press of `button` at (x, y) in the viewport, `count` times.
fn click_actions(x: i64, y: i64, button: u8, count: u32) -> Value {
    let mut acts = vec![json!({ "type": "pointerMove", "x": x, "y": y, "origin": "viewport", "duration": 0 })];
    for _ in 0..count.max(1) {
        acts.push(json!({ "type": "pointerDown", "button": button }));
        acts.push(json!({ "type": "pointerUp", "button": button }));
    }
    json!([{ "type": "pointer", "id": "mouse", "parameters": { "pointerType": "mouse" }, "actions": acts }])
}

fn button_number(button: Option<&str>) -> Result<u8> {
    Ok(match button.unwrap_or("left") {
        "left" => 0,
        "middle" => 1,
        "right" => 2,
        other => bail!("click --button: left, middle or right, not {other:?}"),
    })
}

/// The `op` name of a command, for messages.
fn op_name(command: &Command) -> String {
    serde_json::to_value(command)
        .ok()
        .and_then(|v| v.get("op").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| "this op".into())
}

pub struct Driver {
    wire: Box<dyn Wire>,
    engine: &'static str,
    timeout: Duration,
    /// Console entries read from the wire and not yet returned.
    console: Vec<Value>,
}

impl Driver {
    pub fn launch(config: &SessionConfig) -> Result<Self> {
        let timeout = Duration::from_millis(config.timeout_ms.max(1.0) as u64);
        let caps = match &config.capabilities {
            Some(raw) => serde_json::from_str::<Value>(raw).map_err(|e| anyhow!("--capabilities: not JSON ({e})"))?,
            None => json!({}),
        };
        if !caps.is_object() {
            bail!("--capabilities must be a JSON object, like '{{\"platformName\":\"iOS\"}}'");
        }
        let engine = engine_name(config);
        let wire: Box<dyn Wire> = match (&config.webdriver, config.engine.as_str()) {
            (Some(url), _) => classic::connect(url, caps, config.bidi, timeout)?,
            (None, "firefox") => {
                Box::new(bidi::Bidi::launch_firefox(config.chromium.as_deref(), !config.headed, caps, timeout)?)
            }
            (None, "safari" | "safari-preview") => classic::launch_safari(
                config.engine == "safari-preview",
                config.chromium.as_deref(),
                caps,
                config.bidi,
                timeout,
            )?,
            (None, other) => {
                bail!("--engine {other}: not a WebDriver engine (firefox, safari, safari-preview, webdriver)")
            }
        };
        Ok(Self { wire, engine, timeout, console: Vec::new() })
    }

    pub fn engine(&self) -> &'static str {
        self.engine
    }

    pub fn endpoint(&self) -> String {
        self.wire.endpoint()
    }

    pub fn close(&mut self) {
        self.wire.close();
    }

    pub fn take_dialogs(&mut self) -> Vec<Value> {
        self.wire.dialogs()
    }

    pub fn url(&mut self) -> String {
        self.url_title().map(|(u, _)| u).unwrap_or_default()
    }

    pub fn run_op(&mut self, command: &Command) -> Result<Outcome> {
        let result = self.dispatch(command)?;
        let dialogs = self.wire.dialogs();
        Ok(Outcome { result, dialogs, new_tabs: Vec::new(), notes: Vec::new(), recovered: None })
    }

    /// Run `function` in the page with `args`; its answer.
    fn call(&mut self, function: &str, args: Value) -> Result<Value> {
        let answer = self.wire.call(function, &args.to_string(), self.timeout)?;
        Ok(serde_json::from_str(ok_text(&answer)?)?)
    }

    fn url_title(&mut self) -> Result<(String, String)> {
        let v = self.call(URL_TITLE, json!([]))?;
        let at = |i: usize| v.get(i).and_then(Value::as_str).unwrap_or("").to_string();
        Ok((at(0), at(1)))
    }

    fn state(&mut self) -> Result<Value> {
        let (url, title) = self.url_title()?;
        let tabs = self.wire.tabs()?;
        let current = self.wire.current_tab();
        let tab = tabs.iter().position(|t| *t == current).unwrap_or(0);
        Ok(json!({ "tab": tab, "tabs": tabs.len(), "url": url, "title": title }))
    }

    /// The page expression that finds `target` (auto-waiting as Chrome's
    /// engine does), as an element.
    fn finder(&self, target: &Target, what: &str) -> String {
        match target {
            Target::Ref(id) => format!("({REF_EL})({id}, {})", json!(what)),
            other => crate::cdp::page::finder_js(other, crate::browser::element_wait().as_millis(), what),
        }
    }

    /// Find `target`, then run `then(el, ...args)` on it, in one call.
    fn on(&mut self, target: &Target, what: &str, then: &str, args: Value) -> Result<Value> {
        let finder = self.finder(target, what);
        let function =
            format!("async (...args) => {{ const el = await ({finder}); return await ({then})(el, ...args); }}");
        self.call(&function, args).map_err(|e| e.context(what.to_string()))
    }

    fn point(&mut self, target: &Target, what: &str, force: bool) -> Result<(i64, i64)> {
        let p = self.on(target, what, POINT_EL, json!([force]))?;
        let at = |k: &str| p.get(k).and_then(Value::as_f64).unwrap_or(0.0) as i64;
        Ok((at("x"), at("y")))
    }

    fn act(&mut self, actions: Value) -> Result<()> {
        self.wire.actions(actions)?;
        self.wire.settle(self.timeout);
        Ok(())
    }

    fn not_here(&self, command: &Command) -> anyhow::Error {
        anyhow!(
            "{}: not on the {} engine yet; run it with --engine chrome (the WebDriver engines do goto, ax, click, fill, select, check, press, type, text, eval, wait, screenshot, tabs, cookies, console)",
            op_name(command),
            self.engine
        )
    }

    fn snapshot(&mut self, opts: &AxView, scope: Option<&Target>) -> Result<Value> {
        let started = Instant::now();
        // Scoped: the element's id first (its subtree is built alone).
        let root = match scope {
            Some(t) => {
                let id = self.on(
                    t,
                    "ax --selector",
                    "(el) => { const T = window[Symbol.for('navigera.ids')]; return T ? T.byNode.get(el) : null; }",
                    json!([]),
                );
                match id? {
                    Value::Number(n) => n.as_u64(),
                    _ => None,
                }
            }
            None => None,
        };
        // Ids are made by the snapshot itself; a scope found before the first
        // snapshot is named again below.
        let answer = self.wire.call(SNAPSHOT, &json!([SNAPSHOT_MAX_NODES]).to_string(), self.timeout)?;
        let text = ok_text(&answer).context("ax")?;
        let snap = crate::cdp::snap::Snap::from_json(text)
            .ok_or_else(|| anyhow!("ax: the page's snapshot has no document"))?;
        crate::timing::record("ax_wd_snapshot", started);
        let root = match (scope, root) {
            (Some(t), None) => {
                let v = self.on(
                    t,
                    "ax --selector",
                    "(el) => window[Symbol.for('navigera.ids')].byNode.get(el)",
                    json!([]),
                )?;
                v.as_u64()
            }
            (_, r) => r,
        };
        let built = match root {
            Some(r) => crate::cdp::domax::build_scoped(&snap, r),
            None => crate::cdp::domax::build(&snap),
        }
        .ok_or_else(|| anyhow!("ax: the snapshot has no document"))?;
        if opts.json_list {
            return Ok(flat_list(&built, opts.limit));
        }
        let url = built.url.clone();
        let origin = url
            .find("://")
            .and_then(|i| url[i + 3..].find('/').map(|j| url[..i + 3 + j].to_string()))
            .unwrap_or_default();
        let header = format!("page {} {url}", serde_json::to_string(&built.title).unwrap_or_default());
        let render_opts = crate::cdp::ax::Options {
            full_text: opts.full_text,
            all_refs: opts.all_refs,
            limit: opts.limit,
            root_backend_id: root,
            origin,
            clicks: crate::cdp::page::clickables_of(&snap),
            lazy_budget: None,
        };
        let rendered = crate::cdp::ax::render_nodes(&header, &built.nodes, &built.frames, &render_opts);
        let mut text = rendered.text;
        if text_says_truncated(&answer) {
            text.push_str(&format!("\n(the page has more than {SNAPSHOT_MAX_NODES} nodes: read in part)"));
        }
        Ok(Value::String(text))
    }

    fn dispatch(&mut self, command: &Command) -> Result<Value> {
        match command {
            Command::Open { url: Some(url) } | Command::Goto { url, .. } if !url.is_empty() => {
                let (wait, timeout_ms) = match command {
                    Command::Goto { wait, timeout_ms, .. } => (wait.as_deref(), *timeout_ms),
                    _ => (None, None),
                };
                let wait = match wait {
                    None | Some("load") => Wait::Complete,
                    Some("domcontentloaded") | Some("interactive") => Wait::Interactive,
                    Some("none") | Some("commit") => Wait::None,
                    Some(other) => bail!("goto --wait: load, domcontentloaded or none, not {other:?}"),
                };
                let timeout = timeout_ms.map_or(self.timeout, |ms| Duration::from_millis(ms.max(1.0) as u64));
                let url = crate::protocol::normalize_url(url);
                self.wire.navigate(&url, wait, timeout).map_err(|e| e.context(format!("goto {url}")))?;
                self.state()
            }
            Command::Open { .. } => self.state(),
            Command::Back {} => {
                self.wire.history(-1).context("back")?;
                self.state()
            }
            Command::Forward {} => {
                self.wire.history(1).context("forward")?;
                self.state()
            }
            Command::Reload {} => {
                self.wire.reload(self.timeout).context("reload")?;
                self.state()
            }
            Command::Url {} => self.state(),
            Command::Title {} => Ok(json!(self.url_title()?.1)),
            Command::Status {} => {
                let mut state = self.state()?;
                state["engine"] = json!(self.engine);
                state["endpoint"] = json!(self.wire.endpoint());
                state["browser"] = self.wire.describe();
                state["version"] = json!(env!("CARGO_PKG_VERSION"));
                Ok(state)
            }
            Command::Ax { max_depth: _, all, format, limit, refs, scope, full_text, timeout_ms: _ } => {
                // Serve-protocol clients that predate the text tree get the
                // JSON list Chrome's engine gives them.
                let json_list = match format.as_deref() {
                    None | Some("json") => true,
                    Some("text") => false,
                    Some(other) => bail!("ax --format takes text|json, got {other:?}"),
                };
                if all.is_some() {
                    return Err(self.not_here(command));
                }
                let view = AxView {
                    full_text: *full_text,
                    all_refs: refs.as_deref() == Some("all"),
                    limit: limit.unwrap_or(crate::protocol::DEFAULT_AX_LIMIT),
                    json_list,
                };
                let scope = scope.optional("ax")?;
                self.snapshot(&view, scope.as_ref())
            }
            Command::Click { target, timeout_ms: _, button, force, until_gone } => {
                if until_gone.is_some() {
                    return Err(self.not_here(command));
                }
                let t = target.target("click")?;
                let (x, y) = self.point(&t, "click", *force)?;
                self.act(click_actions(x, y, button_number(button.as_deref())?, 1))?;
                self.state()
            }
            Command::Dblclick { target, timeout_ms: _ } => {
                let t = target.target("dblclick")?;
                let (x, y) = self.point(&t, "dblclick", false)?;
                self.act(click_actions(x, y, 0, 2))?;
                self.state()
            }
            Command::Hover { target, timeout_ms: _ } => {
                let t = target.target("hover")?;
                let (x, y) = self.point(&t, "hover", false)?;
                self.act(json!([{ "type": "pointer", "id": "mouse", "parameters": { "pointerType": "mouse" },
                    "actions": [{ "type": "pointerMove", "x": x, "y": y, "origin": "viewport", "duration": 0 }] }]))?;
                self.state()
            }
            Command::Fill { target, value, timeout_ms: _, keys, force: _ } => {
                let t = target.target("fill")?;
                if *keys {
                    self.on(&t, "fill", crate::cdp::page::FOCUS_SELECT_ALL, json!([]))?;
                    self.act(type_actions(value))?;
                } else {
                    self.on(&t, "fill", crate::cdp::page::FILL_EL_JS, json!([value]))?;
                }
                self.state()
            }
            Command::InsertText { value, target, timeout_ms: _ } => {
                if let Some(t) = target.optional("insert-text")? {
                    self.on(&t, "insert-text", crate::cdp::page::FOCUS_EL, json!([]))?;
                }
                self.act(type_actions(value))?;
                self.state()
            }
            Command::Select { target, value, timeout_ms: _, add } => {
                let t = target.target("select")?;
                let chosen = self.on(&t, "select", crate::cdp::page::SELECT_EL_JS, json!([value, add]))?;
                let mut state = self.state()?;
                state["selected"] = chosen;
                Ok(state)
            }
            Command::SetChecked { checked, target, timeout_ms: _ } => {
                let t = target.target(if *checked { "check" } else { "uncheck" })?;
                let now = self.on(&t, "check", "(el) => !!el.checked", json!([]))?;
                if now.as_bool() != Some(*checked) {
                    let (x, y) = self.point(&t, "check", false)?;
                    self.act(click_actions(x, y, 0, 1))?;
                    let after = self.on(&t, "check", "(el) => !!el.checked", json!([]))?;
                    if after.as_bool() != Some(*checked) {
                        bail!("check: the click did not change the element's state (still {after})");
                    }
                }
                self.state()
            }
            Command::Focus { target, timeout_ms: _ } => {
                let t = target.target("focus")?;
                self.on(&t, "focus", crate::cdp::page::FOCUS_EL, json!([]))?;
                self.state()
            }
            Command::Press { key, target, timeout_ms: _ } => {
                if let Some(t) = target.optional("press")? {
                    self.on(&t, "press", crate::cdp::page::FOCUS_EL, json!([]))?;
                }
                self.act(press_actions(key)?)?;
                self.state()
            }
            Command::Type { value, target, timeout_ms: _ } => {
                if let Some(t) = target.optional("type")? {
                    self.on(&t, "type", crate::cdp::page::FOCUS_EL, json!([]))?;
                }
                self.act(type_actions(value))?;
                self.state()
            }
            Command::Scroll { target, by, to, timeout_ms: _ } => {
                match target.optional("scroll")? {
                    Some(t) => {
                        self.on(
                            &t,
                            "scroll",
                            "(el) => el.scrollIntoView({ block: 'center', behavior: 'instant' })",
                            json!([]),
                        )?;
                    }
                    None => {
                        let js = "(by, to) => { if (to === 'top') scrollTo(0, 0); else if (to === 'bottom') scrollTo(0, document.documentElement.scrollHeight); else scrollBy(0, by); }";
                        self.call(js, json!([by.unwrap_or(600.0), to]))?;
                    }
                }
                self.state()
            }
            Command::Text { selector, grep, context, limit, timeout_ms: _ } => {
                let text = if selector.is_empty() {
                    self.call("() => document.body ? document.body.innerText : ''", json!([]))?
                } else {
                    self.on(&Target::Selector(selector.clone()), "text", "(el) => el.innerText", json!([]))?
                };
                match (grep, &text) {
                    (Some(query), Value::String(all)) => Ok(json!(crate::grep::grep(
                        all,
                        query,
                        context.unwrap_or(crate::grep::DEFAULT_CONTEXT),
                        limit.unwrap_or(crate::grep::DEFAULT_PASSAGES),
                    ))),
                    _ => Ok(text),
                }
            }
            Command::Get { what, name_of, target, timeout_ms: _ } => {
                let t = target.target("get")?;
                self.on(&t, "get", crate::cdp::page::GET_EL, json!([what, name_of]))
            }
            Command::Is { state, target, timeout_ms: _ } => {
                let t = target.target("is")?;
                self.on(&t, "is", crate::cdp::page::IS_EL, json!([state]))
            }
            Command::Locate { target, timeout_ms: _ } => {
                let t = target.target("locate")?;
                self.on(
                    &t,
                    "locate",
                    "(el) => { const r = el.getBoundingClientRect(); return { x: r.left, y: r.top, width: r.width, height: r.height, tag: el.tagName.toLowerCase() }; }",
                    json!([]),
                )
            }
            Command::Eval { expression, timeout_ms, main: _ } => {
                let timeout = timeout_ms.map_or(self.timeout, |ms| Duration::from_millis(ms.max(1.0) as u64));
                self.wire.evaluate(expression, timeout).context("eval")
            }
            Command::Wait { selector, text, url, gone, js, frame, ms, idle: _, timeout_ms, download } => {
                if frame.is_some() || download.is_some() {
                    return Err(self.not_here(command));
                }
                let timeout = timeout_ms.map_or(self.timeout, |ms| Duration::from_millis(ms.max(1.0) as u64));
                let started = Instant::now();
                if let Some(ms) = ms {
                    std::thread::sleep(Duration::from_millis(ms.max(0.0) as u64));
                }
                let poll = |me: &mut Self, js: &str, args: Value, what: &str| -> Result<()> {
                    loop {
                        if me.call(js, args.clone())?.as_bool() == Some(true) {
                            return Ok(());
                        }
                        if started.elapsed() >= timeout {
                            bail!("wait: {what} after {} ms", timeout.as_millis());
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                };
                if let Some(sel) = selector {
                    poll(self, "(s) => { const el = document.querySelector(s); return !!el && el.getClientRects().length > 0; }", json!([sel]), &format!("no visible element matches {sel:?}"))?;
                }
                if let Some(t) = text {
                    poll(
                        self,
                        "(t) => !!document.body && document.body.innerText.includes(t)",
                        json!([t]),
                        &format!("the page does not show {t:?}"),
                    )?;
                }
                if let Some(sel) = gone {
                    poll(self, "(s) => { const el = document.querySelector(s); return !el || el.getClientRects().length === 0; }", json!([sel]), &format!("{sel:?} is still on the page"))?;
                }
                if let Some(pattern) = url {
                    loop {
                        let (now, _) = self.url_title()?;
                        if crate::cdp::glob_match(pattern, &now) || now.contains(pattern.as_str()) {
                            break;
                        }
                        if started.elapsed() >= timeout {
                            bail!("wait: the URL is {now}, not {pattern:?}, after {} ms", timeout.as_millis());
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                }
                if let Some(src) = js {
                    let f = format!("async () => !!(await ({src}))");
                    poll(self, &f, json!([]), "the condition is still false")?;
                }
                Ok(json!({ "waited_ms": started.elapsed().as_millis() as u64 }))
            }
            Command::Screenshot { path, full_page, target, annotate, quality, timeout_ms: _ } => {
                if *annotate || quality.is_some() || target.optional("screenshot")?.is_some() {
                    return Err(self.not_here(command));
                }
                let bytes = self.wire.screenshot(full_page.unwrap_or(false)).context("screenshot")?;
                let path = path.clone().unwrap_or_else(|| {
                    format!(
                        "shot-{}.png",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis())
                            .unwrap_or(0)
                    )
                });
                crate::protocol::write_own(&path, &bytes)
                    .map_err(|e| anyhow::Error::from(e).context(format!("screenshot: writing {path}")))?;
                let path = std::path::absolute(&path).map(|p| p.display().to_string()).unwrap_or(path);
                Ok(json!({ "path": path, "bytes": bytes.len() }))
            }
            Command::TabList {} => {
                let tabs = self.wire.tabs()?;
                let current = self.wire.current_tab();
                let mut out = Vec::new();
                for (i, t) in tabs.iter().enumerate() {
                    out.push(json!({ "index": i, "id": t, "active": *t == current }));
                }
                Ok(json!(out))
            }
            Command::TabNew { url } => {
                let id = self.wire.tab_new()?;
                self.wire.tab_select(&id)?;
                if let Some(url) = url.as_deref().filter(|u| !u.is_empty()) {
                    let url = crate::protocol::normalize_url(url);
                    self.wire.navigate(&url, Wait::Complete, self.timeout)?;
                }
                self.state()
            }
            Command::TabSelect { index, target: _ } => {
                let tabs = self.wire.tabs()?;
                let id = tabs
                    .get(*index)
                    .ok_or_else(|| anyhow!("tab-select: no tab {index} ({} open)", tabs.len()))?
                    .clone();
                self.wire.tab_select(&id)?;
                self.state()
            }
            Command::TabClose { index } => {
                let tabs = self.wire.tabs()?;
                let current = self.wire.current_tab();
                let id = match index {
                    Some(i) => {
                        tabs.get(*i).ok_or_else(|| anyhow!("tab-close: no tab {i} ({} open)", tabs.len()))?.clone()
                    }
                    None => current.clone(),
                };
                if tabs.len() <= 1 {
                    bail!("tab-close: it is the only tab; `quit` ends the session");
                }
                self.wire.tab_close(&id)?;
                if id == current {
                    let left = self.wire.tabs()?;
                    if let Some(first) = left.first() {
                        self.wire.tab_select(&first.clone())?;
                    }
                }
                self.state()
            }
            Command::Cookies {
                values: _,
                action,
                name,
                value,
                url,
                domain,
                path,
                expires,
                http_only,
                secure,
                same_site,
                all: _,
                format: _,
            } => match action.as_deref().unwrap_or("list") {
                "list" | "get" => Ok(json!(self.wire.cookies()?)),
                "set" => {
                    let name = name.as_deref().ok_or_else(|| anyhow!("cookies set: --name is needed"))?;
                    let mut c = json!({ "name": name, "value": value.clone().unwrap_or_default() });
                    if let Some(d) = domain {
                        c["domain"] = json!(d);
                    } else if let Some(u) = url {
                        c["url"] = json!(u);
                    }
                    if let Some(p) = path {
                        c["path"] = json!(p);
                    }
                    if let Some(e) = expires {
                        c["expiry"] = json!(*e as i64);
                    }
                    c["httpOnly"] = json!(http_only);
                    c["secure"] = json!(secure);
                    if let Some(s) = same_site {
                        c["sameSite"] = json!(s);
                    }
                    self.wire.set_cookie(c)?;
                    Ok(json!({ "set": name }))
                }
                "clear" | "delete" => {
                    self.wire.clear_cookies()?;
                    Ok(json!({ "cleared": true }))
                }
                other => bail!("cookies: list, set or clear, not {other:?}"),
            },
            Command::Console { level, limit, clear, format: _ } => {
                let fresh = self.wire.console();
                self.console.extend(fresh);
                let keep = |e: &Value| match level.as_deref() {
                    None | Some("all") => true,
                    Some(l) => e.get("level").and_then(Value::as_str) == Some(l),
                };
                let mut out: Vec<Value> = self.console.iter().filter(|e| keep(e)).cloned().collect();
                if let Some(n) = limit {
                    let skip = out.len().saturating_sub(*n);
                    out.drain(..skip);
                }
                if *clear {
                    self.console.clear();
                }
                Ok(json!(out))
            }
            Command::Errors { limit, clear, format: _ } => {
                let fresh = self.wire.console();
                self.console.extend(fresh);
                let mut out: Vec<Value> = self
                    .console
                    .iter()
                    .filter(|e| e.get("level").and_then(Value::as_str) == Some("error"))
                    .cloned()
                    .collect();
                if let Some(n) = limit {
                    let skip = out.len().saturating_sub(*n);
                    out.drain(..skip);
                }
                if *clear {
                    self.console.retain(|e| e.get("level").and_then(Value::as_str) != Some("error"));
                }
                Ok(json!(out))
            }
            Command::Dialog { accept, prompt_text } => {
                self.wire.set_dialog_policy(accept.unwrap_or(true), prompt_text.clone());
                Ok(json!({ "dialogs": if accept.unwrap_or(true) { "accept" } else { "dismiss" } }))
            }
            Command::Quit { .. } => {
                self.wire.close();
                Ok(json!({ "bye": true, "browser": "closed" }))
            }
            _ => Err(self.not_here(command)),
        }
    }
}

/// What `ax` prints, for the WebDriver engines.
struct AxView {
    full_text: bool,
    all_refs: bool,
    limit: usize,
    /// The flat `[{ref, role, name, value?}]` list instead of the text.
    json_list: bool,
}

/// Roles the flat list leaves out: structure with nothing to act on or read.
const LIST_SKIP: &[&str] = &["generic", "none", "presentation", "InlineTextBox", "LineBreak", ""];

/// The flat list of a built tree's nodes (the serve protocol's original
/// `ax` answer).
fn flat_list(built: &crate::cdp::domax::Built, limit: usize) -> Value {
    let mut out = Vec::new();
    let nodes = built.nodes.iter().chain(built.frames.iter().flat_map(|f| f.nodes.iter()));
    for n in nodes.filter(|n| !n.ignored && !LIST_SKIP.contains(&n.role.as_str())) {
        let mut item = json!({ "ref": n.backend, "role": n.role, "name": n.name });
        if !n.value.is_empty() {
            item["value"] = json!(n.value);
        }
        out.push(item);
        if out.len() >= limit {
            break;
        }
    }
    Value::Array(out)
}

/// Whether the snapshot's answer says it stopped at its node limit.
fn text_says_truncated(answer: &str) -> bool {
    answer.ends_with(",\"truncated\":true}}")
}

impl Drop for Driver {
    fn drop(&mut self) {
        self.wire.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wrapped_answer_gives_its_value_or_its_error() {
        assert_eq!(ok_text(r#"{"ok":{"a":1}}"#).unwrap(), r#"{"a":1}"#);
        assert_eq!(ok_text(r#"{"ok":null}"#).unwrap(), "null");
        let e = ok_text(r##"{"err":"click: no element matches \"#x\""}"##).unwrap_err();
        assert!(format!("{e}").contains("no element"), "{e}");
        assert!(ok_text("not json").is_err());
    }

    #[test]
    fn keys_map_to_webdriver_values_and_combos_hold_their_modifiers() {
        assert_eq!(key_value("Enter").unwrap(), "\u{E007}");
        assert_eq!(key_value("F5").unwrap(), "\u{E035}");
        assert_eq!(key_value("a").unwrap(), "a");
        assert!(key_value("Nope").is_err());
        let acts = press_actions("Control+a").unwrap();
        let list = acts[0]["actions"].as_array().unwrap();
        let kinds: Vec<&str> = list.iter().map(|a| a["type"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["keyDown", "keyDown", "keyUp", "keyUp"]);
        assert_eq!(list[0]["value"], "\u{E009}");
        assert_eq!(list[3]["value"], "\u{E009}");
        assert_eq!(press_actions("+").unwrap()[0]["actions"][0]["value"], "+");
    }

    #[test]
    fn the_engines_are_named_from_the_config() {
        let mut c = SessionConfig { engine: "firefox".into(), ..SessionConfig::default() };
        assert!(wanted(&c));
        assert_eq!(engine_name(&c), "firefox");
        c.engine = "chrome".into();
        assert!(!wanted(&c));
        c.webdriver = Some("http://127.0.0.1:4444".into());
        assert!(wanted(&c));
        assert_eq!(engine_name(&c), "webdriver");
    }
}
