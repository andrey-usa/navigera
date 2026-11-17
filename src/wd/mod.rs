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

use crate::cdp::{Route, Target};
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

/// The console entries a session keeps between `console --clear`s.
const CONSOLE_KEPT: usize = 1000;

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

/// The page expression for the element an op kept for the protocol's own
/// element command (`upload`).
pub const PICKED: &str = "window[Symbol.for('navigera.picked')]";

/// Keep a file input for [`PICKED`], after checking it can take `count`
/// files.
const PICK_FILE_INPUT: &str = r#"function (el, count) {
    if (!(el instanceof el.ownerDocument.defaultView.HTMLInputElement) || el.type !== 'file') throw new Error('the element is not an <input type=file> (it is <' + el.localName + '>); `ax` shows the file input, or click what opens the chooser');
    if (count > 1 && !el.multiple) throw new Error('the file input takes one file (no `multiple`), got ' + count);
    window[Symbol.for('navigera.picked')] = el;
    return true;
}"#;

/// A point `mouse` aims at: the element scrolled into view, then (dx, dy)
/// from its top-left corner, or its middle, in the top window's viewport.
const MOUSE_POINT_EL: &str = r#"function (el, dx, dy) {
    let r = el.getBoundingClientRect();
    if (r.top < 0 || r.left < 0 || r.bottom > window.innerHeight || r.right > window.innerWidth || el.ownerDocument !== document) {
        el.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
        r = el.getBoundingClientRect();
    }
    let x = r.left + (dx == null ? r.width / 2 : dx), y = r.top + (dy == null ? r.height / 2 : dy);
    for (let w = el.ownerDocument.defaultView; w && w.frameElement; w = w.parent) {
        const f = w.frameElement.getBoundingClientRect();
        x += f.left + w.frameElement.clientLeft;
        y += f.top + w.frameElement.clientTop;
    }
    return { x: Math.max(0, Math.round(x)), y: Math.max(0, Math.round(y)) };
}"#;

/// The viewport's size.
const VIEWPORT_SIZE: &str = "() => [window.innerWidth, window.innerHeight]";

/// Mouse moves from the press to the target of a `drag`, as on Chrome.
const DRAG_STEPS: i64 = 8;

/// The first moves of a held mouse: past any drag threshold.
const DRAG_START_STEPS: [i64; 5] = [3, 6, 10, 15, 20];

/// One pointer source's actions.
fn pointer(acts: Vec<Value>) -> Value {
    json!([{ "type": "pointer", "id": "mouse", "parameters": { "pointerType": "mouse" }, "actions": acts }])
}

fn move_to(x: i64, y: i64) -> Value {
    json!({ "type": "pointerMove", "x": x, "y": y, "origin": "viewport", "duration": 0 })
}

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
    /// Input actions after which a button or a key stays held, for the
    /// calls that follow (`mouse down`, `keydown`).
    fn actions_keep(&mut self, actions: Value) -> Result<()> {
        self.actions(actions)
    }
    /// Give the file input the page keeps under [`PICKED`] these files
    /// (absolute paths on the browser's machine).
    fn set_files(&mut self, files: &[String], timeout: Duration) -> Result<()>;
    /// The viewport's size in CSS pixels and its pixel ratio; `None` is the
    /// browser's own again.
    fn set_viewport(&mut self, size: Option<(u32, u32)>, scale: Option<f64>) -> Result<()>;
    /// A script every new document runs before its own.
    fn add_init_script(&mut self, source: &str) -> Result<()>;
    /// Take the init scripts away again; how many there were.
    fn clear_init_scripts(&mut self) -> Result<usize>;
    /// `route`: answer the requests a rule's glob matches the rule's way,
    /// from now on (a rule with the same glob is replaced).
    fn add_route(&mut self, _route: Route) -> Result<()> {
        bail!("classic WebDriver cannot see a page's requests; use --engine firefox or chrome (or --bidi with a server that has it)")
    }
    /// Remove the rule for `pattern`, or every rule; how many went.
    fn remove_routes(&mut self, _pattern: Option<&str>) -> Result<usize> {
        Ok(0)
    }
    fn routes(&self) -> Vec<Route> {
        Vec::new()
    }
    /// Between commands: take what the browser sent meanwhile (a request
    /// waiting for its rule, a dialog).
    fn idle(&mut self) {}
    /// Wait for a navigation an action started, if one did.
    fn settle(&mut self, _timeout: Duration) {}
    fn screenshot(&mut self, full_page: bool) -> Result<Vec<u8>>;
    /// The page printed to PDF, by the browser's own print.
    fn pdf(&mut self, landscape: bool, background: bool) -> Result<Vec<u8>>;
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
    /// Where `mouse` left the pointer, and the buttons it holds (bit 1
    /// left, 2 right, 4 middle).
    pointer: (i64, i64),
    buttons: u8,
    /// Keys `keydown` holds.
    keys: Vec<String>,
    init_scripts: Vec<String>,
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
        // The list of hosts is held by Chrome's request interception. Here
        // nothing would hold it, and a session that looks restricted and
        // is not is worse than none.
        if config.allow_domains.is_some() {
            bail!("--allow-domains is not on the {engine} engine yet: nothing would keep its pages to the list. Run it with --engine chrome, or drop the list");
        }
        let wire: Box<dyn Wire> = match (&config.webdriver, config.engine.as_str()) {
            (Some(url), _) => classic::connect(url, caps, config.bidi, timeout)?,
            (None, "firefox") if config.attach.is_some() => Box::new(bidi::Bidi::attach_firefox(
                config.attach.as_deref().unwrap_or(crate::cdp::attach::AUTO),
                caps,
                timeout,
            )?),
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
        Ok(Self {
            wire,
            engine,
            timeout,
            console: Vec::new(),
            pointer: (0, 0),
            buttons: 0,
            keys: Vec::new(),
            init_scripts: Vec::new(),
        })
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

    /// Between commands (the session's tick): requests waiting for a
    /// `route` rule and dialogs are answered.
    pub fn idle(&mut self) {
        self.wire.idle();
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
        // While `mouse down` or `keydown` holds something, nothing lets go
        // of it but its own `up`.
        if self.buttons != 0 || !self.keys.is_empty() {
            self.wire.actions_keep(actions)?;
        } else {
            self.wire.actions(actions)?;
        }
        self.wire.settle(self.timeout);
        Ok(())
    }

    fn not_here(&self, command: &Command) -> anyhow::Error {
        anyhow!(
            "{}: not on the {} engine yet; run it with --engine chrome (the WebDriver engines do goto, ax, click, fill, select, check, press, type, text, eval, wait, screenshot, pdf, tabs, cookies, console, upload, drag, mouse, keydown, keyup, storage, init-script, set viewport, route, unroute)",
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
        // The page's own script made this: checked before it is indexed.
        let snap = crate::cdp::snap::Snap::from_page_json(text).map_err(|why| {
            anyhow!("ax: the page's snapshot {why}; the page may have changed the built-ins its reader uses: try `text`, or --engine chrome")
        })?;
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
        // Chrome's tree builder trusts Chrome's capture. This one is the
        // page's: whatever the check above let through that it cannot
        // print is an error here, not the end of the session.
        let printed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Result<Value, String>> {
            let built = match root {
                Some(r) => crate::cdp::domax::build_scoped(&snap, r),
                None => crate::cdp::domax::build(&snap),
            }
            .ok_or_else(|| anyhow!("ax: the snapshot has no document"))?;
            if opts.json_list {
                return Ok(Ok(flat_list(&built, opts.limit)));
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
            Ok(Err(rendered.text))
        }))
        .map_err(|_| {
            anyhow!("ax: the page's snapshot could not be printed; the page may have changed the built-ins its reader uses: try `text`, or --engine chrome")
        })??;
        let mut text = match printed {
            Ok(list) => return Ok(list),
            Err(text) => text,
        };
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
            Command::Pdf { path, landscape, background } => {
                let bytes = self.wire.pdf(*landscape, *background).context("pdf")?;
                let path = path.clone().unwrap_or_else(|| {
                    format!(
                        "page-{}.pdf",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis())
                            .unwrap_or(0)
                    )
                });
                crate::protocol::write_own(&path, &bytes)
                    .map_err(|e| anyhow::Error::from(e).context(format!("pdf: writing {path}")))?;
                let path = std::path::absolute(&path).map(|p| p.display().to_string()).unwrap_or(path);
                Ok(json!({ "path": path, "bytes": bytes.len() }))
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
                self.gather_console();
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
                self.gather_console();
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
            Command::Upload { target, files, timeout_ms: _ } => {
                let t = target.target("upload")?;
                let files: Vec<String> = files
                    .iter()
                    .map(|f| {
                        std::fs::canonicalize(f)
                            .map(|p| p.display().to_string())
                            .map_err(|e| anyhow!("upload: {f}: {e}"))
                    })
                    .collect::<Result<_>>()?;
                self.on(&t, "upload", PICK_FILE_INPUT, json!([files.len()]))?;
                self.wire.set_files(&files, self.timeout).map_err(|e| e.context("upload"))?;
                Ok(json!({ "files": files.len() }))
            }
            Command::Drag { from, to, from_at, to_at, timeout_ms: _ } => {
                let (f, t) = (from.target("drag")?, to.target("drag")?);
                let what = format!("drag {} to {}", f.label(), t.label());
                let html5 = self
                    .on(&f, "drag", crate::cdp::page::DRAG_IS_HTML5, json!([]))
                    .map_err(|e| e.context(what.clone()))?;
                let r = if html5 == Value::Bool(true) {
                    // A `draggable` source: the events a browser fires,
                    // dragstart through dragend, in the page (no driver
                    // starts a real drag from input actions on every
                    // browser).
                    self.on(&f, "drag", crate::cdp::page::DRAG_START, json!([]))
                        .map_err(|e| e.context(what.clone()))?;
                    let dropped = self
                        .on(&t, "drag", crate::cdp::page::DRAG_DROP, json!([]))
                        .map_err(|e| e.context(what.clone()))?;
                    json!({ "html5": true, "dropped": dropped, "path": "synthetic" })
                } else {
                    let at = |p: &Option<Box<[f64; 2]>>| match p.as_deref() {
                        Some([x, y]) => json!([x, y]),
                        None => json!([null, null]),
                    };
                    let a = self.on(&f, "drag", MOUSE_POINT_EL, at(from_at)).map_err(|e| e.context(what.clone()))?;
                    let (ax, ay) = (a["x"].as_i64().unwrap_or(0), a["y"].as_i64().unwrap_or(0));
                    let mut acts = vec![move_to(ax, ay), json!({ "type": "pointerDown", "button": 0 })];
                    acts.extend(DRAG_START_STEPS.iter().map(|d| move_to(ax + d, ay + d)));
                    self.wire.actions_keep(pointer(acts)).map_err(|e| e.context(what.clone()))?;
                    // The target is found with the mouse held: a drop zone
                    // may only be there once the drag began.
                    let b = self.on(&t, "drag", MOUSE_POINT_EL, at(to_at));
                    let (sx, sy) = (ax + 20, ay + 20);
                    let mut acts = Vec::new();
                    let end = match &b {
                        Ok(b) => {
                            let (bx, by) = (b["x"].as_i64().unwrap_or(0), b["y"].as_i64().unwrap_or(0));
                            acts.extend(
                                (1..=DRAG_STEPS)
                                    .map(|i| move_to(sx + (bx - sx) * i / DRAG_STEPS, sy + (by - sy) * i / DRAG_STEPS)),
                            );
                            (bx, by)
                        }
                        Err(_) => (sx, sy),
                    };
                    acts.push(json!({ "type": "pointerUp", "button": 0 }));
                    self.buttons &= !1;
                    self.pointer = end;
                    self.act(pointer(acts)).map_err(|e| e.context(what.clone()))?;
                    b.map_err(|e| e.context(what.clone()))?;
                    json!({ "from": { "x": ax, "y": ay }, "to": { "x": end.0, "y": end.1 }, "path": "mouse" })
                };
                let mut out = self.state()?;
                out["drag"] = r;
                Ok(out)
            }
            Command::Mouse { action, x, y, button, count, steps, target, timeout_ms: _ } => {
                let (number, bit) = match button.as_deref().unwrap_or("left") {
                    "left" | "" => (0u8, 1u8),
                    "middle" => (1, 4),
                    "right" => (2, 2),
                    other => bail!("mouse --button takes left, right or middle, not {other:?}"),
                };
                let offset = match (action.as_str(), x, y) {
                    ("move" | "click", Some(x), Some(y)) => Some((*x, *y)),
                    _ => None,
                };
                let at = match target.optional("mouse")? {
                    Some(t) => {
                        let args = match offset {
                            Some((dx, dy)) => json!([dx, dy]),
                            None => json!([null, null]),
                        };
                        let p = self.on(&t, &format!("mouse {action}"), MOUSE_POINT_EL, args)?;
                        Some((p["x"].as_i64().unwrap_or(0), p["y"].as_i64().unwrap_or(0)))
                    }
                    None => {
                        if let (Some((px, py)), "click") = (offset, action.as_str()) {
                            let v = self.call(VIEWPORT_SIZE, json!([]))?;
                            let (w, h) =
                                (v[0].as_f64().unwrap_or(f64::INFINITY), v[1].as_f64().unwrap_or(f64::INFINITY));
                            if px < 0.0 || py < 0.0 || px >= w || py >= h {
                                bail!("mouse click: ({px}, {py}) is outside the viewport ({w}x{h}): scroll first, or aim from an element (`mouse click <x> <y> --selector <css>` scrolls it into view)");
                            }
                        }
                        offset.map(|(x, y)| (x.round() as i64, y.round() as i64))
                    }
                };
                if matches!(action.as_str(), "move" | "click") && at.is_none() {
                    bail!("mouse {action} needs x and y");
                }
                let mut acts = Vec::new();
                if let Some((x, y)) = at {
                    let (px, py) = self.pointer;
                    let n = i64::from(steps.unwrap_or(1).clamp(1, 1000));
                    acts.extend((1..=n).map(|i| move_to(px + (x - px) * i / n, py + (y - py) * i / n)));
                    self.pointer = (x, y);
                }
                let mut wheel = None;
                match action.as_str() {
                    "move" => {}
                    "down" => {
                        acts.push(json!({ "type": "pointerDown", "button": number }));
                        self.buttons |= bit;
                    }
                    "up" => acts.push(json!({ "type": "pointerUp", "button": number })),
                    "click" => {
                        for _ in 0..count.unwrap_or(1).clamp(1, 10) {
                            acts.push(json!({ "type": "pointerDown", "button": number }));
                            acts.push(json!({ "type": "pointerUp", "button": number }));
                        }
                    }
                    "wheel" => {
                        let (dx, dy) = (x.unwrap_or(0.0).round() as i64, y.unwrap_or(0.0).round() as i64);
                        wheel = Some(json!({ "type": "wheel", "id": "wheel", "actions": [{ "type": "scroll",
                            "x": self.pointer.0, "y": self.pointer.1, "deltaX": dx, "deltaY": dy, "origin": "viewport", "duration": 0 }] }));
                    }
                    other => bail!("mouse takes move, down, up, click or wheel, not {other:?}"),
                }
                let mut sources = Vec::new();
                if !acts.is_empty() {
                    sources.push(pointer(acts)[0].clone());
                }
                sources.extend(wheel);
                // An `up` lets go after its own actions ran held.
                let sent = if sources.is_empty() { Ok(()) } else { self.act(Value::Array(sources)) };
                if action == "up" {
                    self.buttons &= !bit;
                    if self.buttons == 0 && self.keys.is_empty() {
                        // Nothing held any more: the driver's input state
                        // goes back to rest (the pointer stays put).
                        let _ = self.wire.actions(pointer(vec![move_to(self.pointer.0, self.pointer.1)]));
                    }
                }
                sent.map_err(|e| e.context(format!("mouse {action}")))?;
                let held: Vec<&str> = [(1u8, "left"), (2, "right"), (4, "middle")]
                    .iter()
                    .filter(|(b, _)| self.buttons & b != 0)
                    .map(|(_, n)| *n)
                    .collect();
                let mut state = self.state()?;
                state["mouse"] = json!({ "x": self.pointer.0, "y": self.pointer.1, "held": held });
                Ok(state)
            }
            Command::KeyDown { key } => {
                let value = key_value(key).map_err(|e| e.context("keydown"))?;
                if !self.keys.contains(&value) {
                    self.keys.push(value.clone());
                }
                self.act(
                    json!([{ "type": "key", "id": "keyboard", "actions": [{ "type": "keyDown", "value": value }] }]),
                )
                .map_err(|e| e.context("keydown"))?;
                Ok(json!({ "down": key }))
            }
            Command::KeyUp { key } => {
                let value = key_value(key).map_err(|e| e.context("keyup"))?;
                let sent = self.act(
                    json!([{ "type": "key", "id": "keyboard", "actions": [{ "type": "keyUp", "value": value }] }]),
                );
                self.keys.retain(|k| *k != value);
                if self.buttons == 0 && self.keys.is_empty() {
                    let _ = self.wire.actions(
                        json!([{ "type": "key", "id": "keyboard", "actions": [{ "type": "pause", "duration": 0 }] }]),
                    );
                }
                sent.map_err(|e| e.context("keyup"))?;
                Ok(json!({ "up": key }))
            }
            Command::Storage { area, action, key, value } => {
                let area = match area.as_deref().unwrap_or("local") {
                    "local" => "localStorage",
                    "session" => "sessionStorage",
                    other => bail!("storage takes local|session, got {other:?}"),
                };
                let js = match (action.as_deref().unwrap_or("get"), key, value) {
                    ("get", Some(_), _) => "(k) => A.getItem(k)",
                    ("get", None, _) => "() => Object.fromEntries(Object.entries(A))",
                    ("set", Some(_), Some(_)) => "(k, v) => (A.setItem(k, v), true)",
                    ("remove", Some(_), _) => "(k) => (A.removeItem(k), true)",
                    ("clear", _, _) => "() => (A.clear(), true)",
                    (other, ..) => bail!("storage: {other} needs a key (set: and a value)"),
                };
                self.call(&js.replace('A', area), json!([key, value])).map_err(|e| e.context("storage"))
            }
            Command::InitScript { source: Some(source), .. } => {
                self.wire.add_init_script(source).map_err(|e| e.context("init-script"))?;
                self.init_scripts.push(source.clone());
                Ok(json!({ "init_scripts": self.init_scripts.len() }))
            }
            Command::InitScript { source: None, clear: true } => {
                self.wire.clear_init_scripts().map_err(|e| e.context("init-script"))?;
                Ok(json!({ "removed": std::mem::take(&mut self.init_scripts).len() }))
            }
            Command::InitScript { source: None, clear: false } => {
                if self.init_scripts.is_empty() {
                    return Ok(json!("(no init scripts)"));
                }
                Ok(json!(self
                    .init_scripts
                    .iter()
                    .enumerate()
                    .map(|(i, src)| format!(
                        "{}: {}",
                        i + 1,
                        src.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect::<String>()
                    ))
                    .collect::<Vec<_>>()
                    .join("\n")))
            }
            Command::Set { what, values, scale, mobile, accuracy } if what == "viewport" => {
                if *mobile {
                    bail!("set viewport --mobile: not on the {} engine (a mobile viewport is Chrome's emulation); drop --mobile, or run it with --engine chrome", self.engine);
                }
                let plan = crate::emulate::plan("viewport", values, *scale, false, *accuracy, false)?;
                let size = match &plan.result["viewport"] {
                    Value::Array(wh) => {
                        Some((wh[0].as_f64().unwrap_or(0.0) as u32, wh[1].as_f64().unwrap_or(0.0) as u32))
                    }
                    _ => None,
                };
                self.wire.set_viewport(size, *scale).map_err(|e| e.context("set viewport"))?;
                let v = self.call(VIEWPORT_SIZE, json!([]))?;
                Ok(match size {
                    Some(_) => json!({ "viewport": v, "scale": scale }),
                    None => json!({ "viewport": "default" }),
                })
            }
            Command::Route { pattern: None, har: None, .. } => {
                let routes = self.wire.routes();
                if routes.is_empty() {
                    return Ok(json!("(no routes)"));
                }
                Ok(json!(routes
                    .iter()
                    .map(|r| format!("{} -> {} ({} hits)", r.pattern, r.action.describe(), r.hits))
                    .collect::<Vec<_>>()
                    .join("\n")))
            }
            Command::Route { .. } => {
                let route = crate::protocol::route_of(command)?;
                let (pattern, what) = (route.pattern.clone(), route.action.describe());
                self.wire.add_route(route).map_err(|e| e.context("route"))?;
                Ok(json!({ "route": pattern, "action": what }))
            }
            Command::Unroute { pattern } => {
                let removed = self.wire.remove_routes(pattern.as_deref()).map_err(|e| e.context("unroute"))?;
                Ok(json!({ "removed": removed }))
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

/// `text` within `max` bytes, cut at a character, with a mark when cut.
pub(crate) fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… (+{} bytes)", &text[..end], text.len() - end)
}

/// Whether the snapshot's answer says it stopped at its node limit.
fn text_says_truncated(answer: &str) -> bool {
    answer.ends_with(",\"truncated\":true}}")
}

impl Driver {
    /// The entries logged since the last look, added to the kept ones:
    /// the newest [`CONSOLE_KEPT`], so a long session's log has a size.
    fn gather_console(&mut self) {
        let fresh = self.wire.console();
        self.console.extend(fresh);
        let over = self.console.len().saturating_sub(CONSOLE_KEPT);
        self.console.drain(..over);
    }
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
