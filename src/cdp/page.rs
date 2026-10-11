//! Tab-level operations, implemented directly on CDP.
//!
//! The key performance decision: `Runtime.evaluate` with
//! `returnByValue: true` returns object results in ONE round trip — no
//! evaluate + callFunctionOn + releaseObject dance. Expressions are wrapped
//! so bare arrows (`() => …`), IIFEs, and plain expressions all evaluate to
//! their value. Element actions by CSS selector or visible text are one
//! `Runtime.evaluate` too (find, auto-wait and act in the page); actions by
//! `ax` ref resolve the node first (`DOM.resolveNode` + `callFunctionOn`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use base64::Engine as _;
use serde_json::{json, Value};

use super::ax;
use super::client::CdpClient;
use super::events::Shared;

const CMD_TIMEOUT: Duration = Duration::from_secs(30);

/// How long, after a navigation commits, ops wait for its DOMContentLoaded
/// before working on the partly parsed document.
const POST_COMMIT_WAIT: Duration = Duration::from_secs(5);

/// Default auto-wait for an element when the op carries no `timeout_ms`:
/// long enough for SPA rendering, short enough that an agent with a wrong
/// selector hears about it quickly.
pub const DEFAULT_ELEMENT_WAIT: Duration = Duration::from_millis(5000);

/// What an element op acts on.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// First element matching a CSS selector (auto-waits).
    Selector(String),
    /// A `backendNodeId` from `ax` (`[ref=N]`).
    Ref(u64),
    /// The best element whose visible text / label / value matches (auto-waits).
    Text(String),
}

impl Target {
    pub fn label(&self) -> String {
        match self {
            Target::Selector(s) => format!("selector {s:?}"),
            Target::Ref(r) => format!("ref {r}"),
            Target::Text(t) => format!("text {t:?}"),
        }
    }
}

/// Scroll the element into view and, if it is visible and actually receives
/// a pointer event at its centre, return that point for a *trusted* CDP
/// mouse event (`isTrusted: true`, default actions, focus, user activation
/// — what real sites expect). Otherwise (no layout, zero size, covered by
/// another element, or `force`), fall back to DOM-dispatched events and
/// report it.
const CLICK_PREP: &str = r#"async function (el, force, settleMs) {
    // Mouse events are dispatched in top-level viewport coordinates: add the
    // offsets of every iframe on the way up (null if one is cross-origin).
    const topLevel = (doc, x, y) => {
        let w = doc.defaultView;
        while (w && w !== w.top) {
            const fe = w.frameElement;
            if (!fe) return null;
            const fr = fe.getBoundingClientRect();
            x += fr.left + fe.clientLeft; y += fr.top + fe.clientTop;
            w = w.parent;
        }
        return { x, y };
    };
    // Will a click here likely load a new document (link, form submit)?
    const mayNavigate = (e) => {
        const a = e.closest('a[href]');
        if (a) { const h = a.getAttribute('href') || ''; return !(h.startsWith('#') || h.startsWith('javascript:')); }
        const c = e.closest('button, input');
        return !!(c && c.form && (c.type === 'submit' || c.type === 'image'));
    };
    // A styled checkbox/radio is often a hidden or covered <input> inside a
    // visible <label>: clicking the label is what a user does and toggles it.
    const labels = (el.labels ? [...el.labels] : []);
    const onLabel = (hit) => labels.some((l) => l === hit || l.contains(hit));
    const deadline = Date.now() + (force ? 0 : settleMs);
    let hit = null;
    while (!force) {
        el.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
        let box = el, r = el.getBoundingClientRect();
        if (!(r.width > 0 && r.height > 0)) {
            const l = labels.find((l) => { const b = l.getBoundingClientRect(); return b.width > 0 && b.height > 0; });
            if (l) { box = l; l.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' }); r = l.getBoundingClientRect(); }
        }
        const x = r.left + r.width / 2, y = r.top + r.height / 2;
        const root = box.getRootNode(), doc = box.ownerDocument;
        hit = (r.width > 0 && r.height > 0) ? (root.elementFromPoint ? root.elementFromPoint(x, y) : doc.elementFromPoint(x, y)) : null;
        if (hit && (hit === el || el.contains(hit) || (hit.shadowRoot && hit.shadowRoot.contains(el)) || onLabel(hit))) {
            const p = topLevel(doc, x, y);
            if (p) {
                // Count the press reaching this document: Chrome can ack a
                // dispatched mouse event it never delivered (see click()).
                const w = doc.defaultView;
                w.top.__nvSeen = 0;
                for (const t of ['pointerdown', 'mousedown', 'mouseup', 'click'])
                    w.addEventListener(t, () => { w.top.__nvSeen++; }, { capture: true, once: true });
                return Object.assign(p, { nav: mayNavigate(el), check: true });
            }
            // Inside a cross-origin frame: our coordinates are frame-local;
            // the caller asks CDP for the box instead.
            return { xframe: true, nav: mayNavigate(el) };
        }
        // Covered (a toast, a closing overlay) or not laid out yet: give it a
        // moment, like a user would, before falling back to DOM events. An
        // element with no box at all rarely gets one: don't wait long.
        if (Date.now() >= deadline || (!hit && Date.now() >= deadline - settleMs + 300)) break;
        await new Promise((res) => setTimeout(res, 100));
    }
    for (const type of ['pointerdown', 'mousedown', 'pointerup', 'mouseup', 'click']) {
        el.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, composed: true, view: el.ownerDocument.defaultView }));
    }
    const by = hit && hit !== el ? (hit.id ? '#' + hit.id : hit.tagName.toLowerCase() + (hit.className ? '.' + String(hit.className).split(' ')[0] : '')) : null;
    return { synthetic: true, covered: !!hit && !force, covered_by: by, nav: mayNavigate(el) };
}"#;

/// Hover: like CLICK_PREP but the fallback only fires enter/over events.
const HOVER_PREP: &str = r#"async function (el, settleMs) {
    const deadline = Date.now() + settleMs;
    for (;;) {
        el.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
        const r = el.getBoundingClientRect();
        const x = r.left + r.width / 2, y = r.top + r.height / 2;
        const doc = el.ownerDocument, root = el.getRootNode();
        const hit = (r.width > 0 && r.height > 0) ? (root.elementFromPoint ? root.elementFromPoint(x, y) : doc.elementFromPoint(x, y)) : null;
        if (hit && (hit === el || el.contains(hit) || (hit.shadowRoot && hit.shadowRoot.contains(el)))) {
            let ox = 0, oy = 0, w = doc.defaultView;
            while (w && w !== w.top) {
                const fe = w.frameElement;
                if (!fe) { ox = NaN; break; }
                const fr = fe.getBoundingClientRect();
                ox += fr.left + fe.clientLeft; oy += fr.top + fe.clientTop;
                w = w.parent;
            }
            if (!Number.isNaN(ox)) return { x: x + ox, y: y + oy };
            return { xframe: true };
        }
        if (Date.now() >= deadline) break;
        await new Promise((res) => setTimeout(res, 100));
    }
    for (const type of ['pointerover', 'pointerenter', 'mouseover', 'mouseenter']) {
        el.dispatchEvent(new MouseEvent(type, { bubbles: true, composed: true, view: window }));
    }
    return { synthetic: true };
}"#;

/// Fill `el` with `value`: native value setter (so React & co. see it) plus
/// `input`/`change` events. Contenteditable hosts get their text replaced.
const FILL_EL: &str = r#"function (el, value) {
    el.focus();
    if (el.isContentEditable) {
        el.textContent = value;
        el.dispatchEvent(new InputEvent('input', { bubbles: true, composed: true }));
        return true;
    }
    if (el instanceof HTMLSelectElement) throw new Error('fill: this is a <select>; use `select --value`');
    if (!('value' in el)) throw new Error('fill: element is not an input, textarea or contenteditable (' + el.tagName.toLowerCase() + ')');
    const proto = (el instanceof HTMLTextAreaElement) ? HTMLTextAreaElement.prototype
        : (el instanceof HTMLInputElement) ? HTMLInputElement.prototype : null;
    const desc = proto ? Object.getOwnPropertyDescriptor(proto, 'value') : null;
    if (desc && desc.set) { desc.set.call(el, value); } else { el.value = value; }
    el.dispatchEvent(new Event('input', { bubbles: true, composed: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
    return true;
}"#;

/// Choose `<select>` options by value or visible label (exact first, then
/// case-insensitive), firing input/change like a user would.
const SELECT_EL: &str = r#"function (el, wanted) {
    if (!(el instanceof HTMLSelectElement)) {
        throw new Error('select: element is ' + el.tagName.toLowerCase() + ', not a <select>; for a custom dropdown, click it and then click the option by ref');
    }
    const opts = Array.from(el.options);
    const pick = (w) => opts.find(o => o.value === w) || opts.find(o => o.label.trim() === w)
        || opts.find(o => o.label.trim().toLowerCase() === w.toLowerCase());
    const chosen = wanted.map(w => { const o = pick(w); if (!o) throw new Error('select: no option ' + JSON.stringify(w) + ' (have: ' + opts.map(o => JSON.stringify(o.label.trim())).slice(0, 30).join(', ') + ')'); return o; });
    if (!el.multiple && chosen.length > 1) throw new Error('select: single-choice <select>, got ' + chosen.length + ' values');
    for (const o of opts) o.selected = chosen.includes(o);
    el.focus();
    el.dispatchEvent(new Event('input', { bubbles: true, composed: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
    return chosen.map(o => o.label.trim());
}"#;

/// Focus an element (for `press`/`type` with a target).
const FOCUS_EL: &str = r#"function (el) { el.scrollIntoView({ block: 'center', behavior: 'instant' }); el.focus(); return document.activeElement === el || el.contains(document.activeElement); }"#;

/// Scroll an element into view (or scroll inside it by `dy`).
const SCROLL_EL: &str = r#"function (el, dy) {
    if (dy) { el.scrollBy({ top: dy, behavior: 'instant' }); }
    else { el.scrollIntoView({ block: 'center', behavior: 'instant' }); }
    return { scrollY: Math.round(window.scrollY), scrollHeight: document.documentElement.scrollHeight };
}"#;

/// The element itself (for ops that need its remote object, like upload).
const IDENTITY: &str = "function (el) { return el; }";

/// Auto-wait: resolve the first match for `sel`, polling every 50 ms for up
/// to `ms` (SPAs render after load; an agent shouldn't have to poll).
const WAIT_FOR: &str = r#"async function (sel, ms, what) {
    const end = Date.now() + ms;
    for (;;) {
        const el = document.querySelector(sel);
        if (el) return el;
        if (Date.now() >= end) throw new Error(what + ': no element matches selector ' + JSON.stringify(sel) + ' within ' + ms + ' ms (run `ax` to see the page and act by --ref)');
        await new Promise((r) => setTimeout(r, 50));
    }
}"#;

/// Auto-wait by visible text: the best element whose accessible label,
/// text or value matches `text` — exact (case-insensitive) beats contains,
/// actionable elements beat plain ones, visible beats hidden. Walks open
/// shadow roots.
const WAIT_TEXT: &str = r#"async function (text, ms, what) {
    const norm = (s) => (s || '').replace(/\s+/g, ' ').trim().toLowerCase();
    const want = norm(text);
    // The longest word is the cheap pre-filter for text nodes.
    const key = want.split(' ').reduce((a, b) => (b.length > a.length ? b : a), '');
    const SKIP = new Set(['SCRIPT', 'STYLE', 'TEMPLATE', 'NOSCRIPT', 'HEAD', 'TITLE', 'META', 'LINK']);
    const ACTION = 'a,button,input,select,textarea,summary,label,option,[role],[onclick],[tabindex],[contenteditable=""],[contenteditable="true"]';
    const maxLen = want.length * 4 + 40;
    const label = (el) => norm(el.getAttribute('aria-label') || (el.labels && el.labels[0] && el.labels[0].innerText)
        || el.innerText || el.value || el.getAttribute('placeholder') || el.getAttribute('title') || el.getAttribute('alt'));
    // Candidates without touching layout: parents of text nodes holding the
    // phrase (and a few small ancestors, for text split across inline
    // tags), plus elements whose label comes from attributes or a value.
    // The whole phrase is tried first; only if no text node holds it is the
    // longest word used. Walks open shadow roots; never <head>, scripts or
    // styles. Big containers (lists, tables) are never climbed into.
    const lenCache = new WeakMap();
    const textLen = (el) => {
        let n = lenCache.get(el);
        if (n === undefined) { n = (el.textContent || '').length; lenCache.set(el, n); }
        return n;
    };
    const fromText = (root, needle, out) => {
        const tw = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
        for (let n = tw.nextNode(); n; n = tw.nextNode()) {
            if (!n.data.replace(/\s+/g, ' ').toLowerCase().includes(needle)) continue;
            let p = n.parentElement;
            for (let up = 0; p && up < 4; up++, p = p.parentElement) {
                if (SKIP.has(p.tagName) || p.childElementCount > 12 || textLen(p) > maxLen * 2) break;
                out.add(p);
            }
        }
        for (const el of root.querySelectorAll('*')) if (el.shadowRoot) fromText(el.shadowRoot, needle, out);
        return out;
    };
    const fromAttrs = (root, out) => {
        for (const el of root.querySelectorAll('[aria-label],[placeholder],[title],[alt],input,textarea,select')) out.add(el);
        for (const el of root.querySelectorAll('*')) if (el.shadowRoot) fromAttrs(el.shadowRoot, out);
        return out;
    };
    const candidates = (root) => {
        let out = fromText(root, want, new Set());
        if (out.size === 0 && key !== want) out = fromText(root, key, out);
        return fromAttrs(root, out);
    };
    const visible = (el) => {
        const r = el.getBoundingClientRect();
        if (!(r.width > 0 && r.height > 0)) return false;
        return el.checkVisibility ? el.checkVisibility({ visibilityProperty: true }) : true;
    };
    const end = Date.now() + ms;
    for (;;) {
        let best = null, bestScore = -1;
        const roots = candidates(document.body || document.documentElement);
        for (const el of roots) {
            if (SKIP.has(el.tagName) || el.closest('head')) continue;
            const l = label(el);
            if (!l || l.length > maxLen) continue;
            const exact = l === want, has = l.includes(want);
            if (!exact && !has) continue;
            // Visible beats everything: an exact match on a hidden element
            // must not win over the button the agent can see.
            const score = (visible(el) ? 8 : 0) + (exact ? 4 : 0) + (el.matches(ACTION) ? 2 : 0) - l.length / 1000;
            if (score > bestScore) { best = el; bestScore = score; }
        }
        // A <label> stands for its control (fill/select/upload need the input).
        if (best && best.tagName === 'LABEL' && best.control) return best.control;
        if (best) return best;
        if (Date.now() >= end) throw new Error(what + ': no element with text ' + JSON.stringify(text) + ' within ' + ms + ' ms (run `ax` to see the page and act by --ref)');
        await new Promise((r) => setTimeout(r, 100));
    }
}"#;

/// Key name -> (key, code, windowsVirtualKeyCode, text) for `press`.
fn key_info(name: &str) -> Option<(String, String, u32, Option<String>)> {
    let named: &[(&str, &str, u32, Option<&str>)] = &[
        ("Enter", "Enter", 13, Some("\r")),
        ("Tab", "Tab", 9, None),
        ("Escape", "Escape", 27, None),
        ("Backspace", "Backspace", 8, None),
        ("Delete", "Delete", 46, None),
        ("ArrowUp", "ArrowUp", 38, None),
        ("ArrowDown", "ArrowDown", 40, None),
        ("ArrowLeft", "ArrowLeft", 37, None),
        ("ArrowRight", "ArrowRight", 39, None),
        ("Home", "Home", 36, None),
        ("End", "End", 35, None),
        ("PageUp", "PageUp", 33, None),
        ("PageDown", "PageDown", 34, None),
        ("Space", "Space", 32, Some(" ")),
        ("Control", "ControlLeft", 17, None),
        ("Shift", "ShiftLeft", 16, None),
        ("Alt", "AltLeft", 18, None),
        ("Meta", "MetaLeft", 91, None),
    ];
    let aliases = [
        ("Esc", "Escape"),
        ("Return", "Enter"),
        ("Up", "ArrowUp"),
        ("Down", "ArrowDown"),
        ("Left", "ArrowLeft"),
        ("Right", "ArrowRight"),
        ("Ctrl", "Control"),
        ("Cmd", "Meta"),
        (" ", "Space"),
    ];
    let name = aliases.iter().find(|(a, _)| a.eq_ignore_ascii_case(name)).map(|(_, k)| *k).unwrap_or(name);
    if let Some((key, code, vk, text)) = named.iter().find(|(k, ..)| k.eq_ignore_ascii_case(name)) {
        let key = if *key == "Space" { " " } else { key };
        return Some((key.to_string(), code.to_string(), *vk, text.map(str::to_string)));
    }
    if let Some(n) = name.strip_prefix(['F', 'f']).and_then(|n| n.parse::<u32>().ok()) {
        if (1..=12).contains(&n) {
            return Some((format!("F{n}"), format!("F{n}"), 111 + n, None));
        }
    }
    let mut chars = name.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else { return None };
    let upper = c.to_ascii_uppercase();
    let code = if c.is_ascii_alphabetic() {
        format!("Key{upper}")
    } else if c.is_ascii_digit() {
        format!("Digit{c}")
    } else {
        String::new()
    };
    let vk = if c.is_ascii_alphanumeric() { upper as u32 } else { 0 };
    Some((c.to_string(), code, vk, Some(c.to_string())))
}

/// The character Shift produces on a US keyboard.
fn shifted_char(c: char) -> char {
    const PLAIN: &str = "1234567890-=[]\\;',./`";
    const SHIFTED: &str = "!@#$%^&*()_+{}|:\"<>?~";
    if c.is_ascii_lowercase() {
        return c.to_ascii_uppercase();
    }
    PLAIN.chars().position(|p| p == c).and_then(|i| SHIFTED.chars().nth(i)).unwrap_or(c)
}

/// Does Enter on the focused element (through open shadow roots and
/// same-origin frames) possibly navigate? Form controls, links, buttons.
const ENTER_MAY_NAVIGATE: &str = r#"(() => {
    let e = document.activeElement;
    for (;;) {
        if (e && e.shadowRoot && e.shadowRoot.activeElement) { e = e.shadowRoot.activeElement; continue; }
        if (e && (e.tagName === 'IFRAME' || e.tagName === 'FRAME')) {
            let d = null; try { d = e.contentDocument; } catch (_) {}
            if (!d) return true;
            e = d.activeElement; continue;
        }
        break;
    }
    if (!e || e === document.body) return false;
    return !!(e.form || e.closest('a[href], button, [role=button], [role=link]'));
})()"#;

fn modifier_bit(name: &str) -> Option<u32> {
    match name.to_ascii_lowercase().as_str() {
        "alt" | "option" => Some(1),
        "control" | "ctrl" => Some(2),
        "meta" | "cmd" | "command" => Some(4),
        "shift" => Some(8),
        _ => None,
    }
}

/// JS-side wait budget: a bit under the CDP command timeout, so the page's
/// own "not found" error wins the race against the transport timeout.
fn wait_budget_ms(timeout: Duration) -> u128 {
    timeout.as_millis().saturating_sub(250).max(1)
}

/// Readable message from a CDP `exceptionDetails` (first lines of the
/// exception description, not the whole JSON blob).
fn exception_message(details: &Value) -> String {
    let description = details
        .get("exception")
        .and_then(|e| e.get("description").or_else(|| e.get("value")))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| details.get("text").and_then(Value::as_str).unwrap_or("exception").to_string());
    description.lines().filter(|l| !l.trim_start().starts_with("at ")).take(4).collect::<Vec<_>>().join("\n")
}

/// `node[key].value` as a string ("" when absent).
fn ax_str<'a>(node: &'a Value, key: &str) -> &'a str {
    node.get(key).and_then(|v| v.get("value")).and_then(Value::as_str).unwrap_or("")
}

/// `ax` options passed down from the protocol.
#[derive(Clone, Debug, Default)]
pub struct AxOptions {
    pub max_depth: Option<u32>,
    /// Legacy flat JSON (`[{ref, role, name, value?}]`) instead of text.
    pub json: bool,
    /// With `json`: every non-ignored node in the raw shape.
    pub all: bool,
    pub all_refs: bool,
    pub limit: usize,
    pub scope: Option<Target>,
}

#[derive(Clone)]
pub struct Page {
    handle: tokio::runtime::Handle,
    client: CdpClient,
    session_id: String,
    target_id: String,
    shared: Arc<Shared>,
}

impl Page {
    pub fn new(
        handle: tokio::runtime::Handle,
        client: CdpClient,
        session_id: String,
        target_id: String,
        shared: Arc<Shared>,
    ) -> Self {
        Self { handle, client, session_id, target_id, shared }
    }

    pub fn target_id(&self) -> &str {
        &self.target_id
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    fn block_on<F: std::future::Future>(&self, fut: F) -> F::Output {
        self.handle.block_on(fut)
    }

    /// Page events only. `Runtime.enable` is deliberately not sent: nothing
    /// here needs execution-context or console events, it costs a round
    /// trip per tab plus a stream of events, and it is the best-known
    /// automation fingerprint anti-bot scripts probe for.
    pub async fn enable(&self) -> Result<()> {
        self.client.send("Page.enable", json!({}), Some(&self.session_id), CMD_TIMEOUT).await?;
        // fetch/XHR tracking for `network_quiet` (best effort: an engine
        // without the Network domain just never waits for data).
        // `NAVIGERA_NO_NETWORK=1` (diagnostic) leaves the Network domain off.
        if std::env::var_os("NAVIGERA_NO_NETWORK").is_none() {
            let _ = self.client.send("Network.enable", json!({}), Some(&self.session_id), CMD_TIMEOUT).await;
        }
        Ok(())
    }

    /// Wait (at most `max`) until the page has produced two animation
    /// frames: by then a tab brought to the front has painted and takes
    /// input. Returns false on timeout (a page that can't paint).
    pub fn wait_frame(&self, max: Duration) -> bool {
        let ms = max.as_millis();
        let js = format!(
            "new Promise(r => {{ setTimeout(() => r(false), {ms}); \
             requestAnimationFrame(() => requestAnimationFrame(() => r(true))); }})"
        );
        self.send(
            "Runtime.evaluate",
            json!({ "expression": js, "awaitPromise": true, "returnByValue": true }),
            max + Duration::from_secs(1),
        )
        .ok()
        .and_then(|r| r.pointer("/result/value").and_then(Value::as_bool))
        .unwrap_or(false)
    }

    /// Wait until no fetch/XHR is in flight and none started or ended for
    /// `quiet`, at most `max`. A single-page app usually renders what it
    /// fetched right after the response: reading the page earlier shows
    /// its "Loading…" placeholder. Returns at once on a page that has been
    /// quiet (typically: the agent was thinking for seconds).
    pub fn network_quiet(&self, quiet: Duration, max: Duration) -> bool {
        let deadline = std::time::Instant::now() + max;
        loop {
            let (pending, last) = self.shared.net_state(&self.session_id);
            let idle_for = last.map(|t| t.elapsed()).unwrap_or(Duration::MAX);
            if pending == 0 && idle_for >= quiet {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn send(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        self.block_on(self.client.send(method, params, Some(&self.session_id), timeout))
    }

    /// Evaluate a JS expression; the result comes back by value in one round
    /// trip. Bare arrow functions are invoked; IIFEs and plain expressions
    /// evaluate to their value.
    pub fn evaluate(&self, expression: &str, timeout: Duration) -> Result<Value> {
        // Wrap once: if the expression evaluates to a function, call it.
        // Handles `() => …`, `(() => …)()`, and plain `document.title`.
        let wrapped = format!(
            "(() => {{ const __r = ({}\n); return (typeof __r === 'function') ? __r() : __r; }})()",
            expression
        );
        self.evaluate_raw(&wrapped, timeout)
    }

    fn evaluate_raw(&self, expression: &str, timeout: Duration) -> Result<Value> {
        let res = self.send(
            "Runtime.evaluate",
            json!({
                "expression": expression,
                "returnByValue": true,
                "awaitPromise": true,
            }),
            timeout,
        )?;
        if let Some(details) = res.get("exceptionDetails") {
            bail!("{}", exception_message(details));
        }
        // returnByValue => result.result.value holds the JSON value.
        Ok(res.get("result").and_then(|r| r.get("value")).cloned().unwrap_or(Value::Null))
    }

    /// Navigate the main frame. `wait` is `load` (default), `domcontentloaded`
    /// or `commit` (return once the navigation is committed).
    ///
    /// Subscribes to CDP events *before* navigating so a fast local page that
    /// loads before the response returns isn't missed.
    pub fn navigate(&self, url: &str, wait: &str, timeout: Duration) -> Result<()> {
        self.block_on(async {
            let mut events = self.client.subscribe();
            let res = self
                .client
                .send(
                    "Page.navigate",
                    json!({ "url": url }),
                    Some(&self.session_id),
                    timeout,
                )
                .await
                .with_context(|| format!("navigate to {url}"))?;
            if let Some(error) = res.get("errorText").and_then(Value::as_str) {
                bail!("navigate to {url}: {error}");
            }
            if wait == "commit" {
                if res.get("loaderId").is_some() {
                    self.shared.mark_loading(&self.session_id);
                }
                return Ok(());
            }
            // A same-document navigation (hash change) has no loaderId and
            // fires no load event.
            if res.get("loaderId").is_none() {
                return Ok(());
            }
            let loader = res.get("loaderId").and_then(Value::as_str).unwrap_or("").to_string();
            let want = if wait == "domcontentloaded" {
                "Page.domContentEventFired"
            } else {
                "Page.loadEventFired"
            };
            // Count load events only after OUR navigation committed (its
            // frameNavigated carries the loaderId Page.navigate returned), so
            // a late load event from the previous document can't end the wait.
            let mut committed = false;
            self.wait_event(&mut events, timeout, |method, params| {
                if method == "Page.frameNavigated" {
                    let frame = params.get("frame").unwrap_or(&Value::Null);
                    let main = frame.get("parentId").is_none();
                    let ours = loader.is_empty()
                        || frame.get("loaderId").and_then(Value::as_str) == Some(loader.as_str());
                    if main && ours {
                        committed = true;
                    }
                }
                committed && method == want
            })
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "timed out after {} ms waiting for {} of {url} (a slow subresource? try `goto --wait domcontentloaded`)",
                    timeout.as_millis(),
                    if wait == "domcontentloaded" { "DOMContentLoaded" } else { "load" }
                )
            })?;
            Ok(())
        })
    }

    /// Wait for an event of THIS tab's session satisfying `pred(method, params)`.
    async fn wait_event(
        &self,
        events: &mut tokio::sync::broadcast::Receiver<Value>,
        timeout: Duration,
        mut pred: impl FnMut(&str, &Value) -> bool,
    ) -> Result<()> {
        tokio::time::timeout(timeout, async {
            loop {
                match events.recv().await {
                    Ok(event) => {
                        if event.get("sessionId").and_then(Value::as_str) != Some(self.session_id.as_str()) {
                            continue;
                        }
                        let method = event.get("method").and_then(Value::as_str).unwrap_or("");
                        if pred(method, event.get("params").unwrap_or(&Value::Null)) {
                            return Ok(());
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(e) => bail!("CDP event stream ended: {e}"),
                }
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("timed out after {} ms", timeout.as_millis()))?
    }

    /// Navigate and return as soon as the navigation commits (no load wait).
    pub fn navigate_commit(&self, url: &str, timeout: Duration) -> Result<()> {
        self.navigate(url, "commit", timeout)
    }

    /// Back/forward through session history (`delta` = -1 / +1).
    pub fn history(&self, delta: i64, timeout: Duration) -> Result<()> {
        let history = self.send("Page.getNavigationHistory", json!({}), timeout)?;
        let index = history.get("currentIndex").and_then(Value::as_i64).unwrap_or(0) + delta;
        let entries = history.get("entries").and_then(Value::as_array).cloned().unwrap_or_default();
        let Some(entry) = usize::try_from(index).ok().and_then(|i| entries.get(i)) else {
            bail!("no {} history entry", if delta < 0 { "previous" } else { "next" });
        };
        let id = entry.get("id").cloned().unwrap_or(Value::Null);
        self.block_on(async {
            let mut events = self.client.subscribe();
            self.client
                .send("Page.navigateToHistoryEntry", json!({ "entryId": id }), Some(&self.session_id), timeout)
                .await?;
            // Committed (cross-document, back/forward cache) or a
            // same-document history step: either way the URL has changed.
            self.wait_event(&mut events, timeout, |method, params| match method {
                "Page.frameNavigated" => params.get("frame").is_some_and(|f| f.get("parentId").is_none()),
                "Page.navigatedWithinDocument" => true,
                _ => false,
            })
            .await
        })?;
        self.settle(timeout)
    }

    pub fn reload(&self, timeout: Duration) -> Result<()> {
        self.block_on(async {
            let mut events = self.client.subscribe();
            self.client.send("Page.reload", json!({}), Some(&self.session_id), timeout).await?;
            let mut committed = false;
            self.wait_event(&mut events, timeout, |method, params| {
                if method == "Page.frameNavigated" && params.get("frame").is_some_and(|f| f.get("parentId").is_none()) {
                    committed = true;
                }
                committed && method == "Page.loadEventFired"
            })
            .await
            .map_err(|_| anyhow::anyhow!("timed out waiting for reload"))
        })
    }

    /// If an earlier action started a navigation in this tab, wait (up to
    /// `timeout`) until its new document is parsed. No-op otherwise.
    pub fn settle(&self, timeout: Duration) -> Result<()> {
        let end = Instant::now() + timeout;
        loop {
            let state = self.shared.nav_state(&self.session_id);
            // A request that never starts loading (cancelled, 204, download)
            // stops counting after 2 s.
            let pending = state.requested_at.is_some_and(|t| t.elapsed() < Duration::from_secs(2));
            if !pending && (!state.loading || state.dom_ready) {
                return Ok(());
            }
            // Committed but still parsing after POST_COMMIT_WAIT: the page
            // streams or hangs on a resource. Its DOM is the new page, so
            // reading it is safe; waiting longer only stalls the agent
            // (the op result then says `loading: true`).
            if !pending && state.committed_at.is_some_and(|t| t.elapsed() >= POST_COMMIT_WAIT) {
                return Ok(());
            }
            if Instant::now() >= end {
                bail!("timed out after {} ms waiting for a navigation to finish", timeout.as_millis());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Navigation generation (bumped by every request / load start).
    pub fn nav_generation(&self) -> u64 {
        self.shared.nav_state(&self.session_id).generation
    }

    /// After an action that may navigate (a link, a submit, Enter in a
    /// form): give the navigation up to `grace` to be requested — form
    /// submissions are scheduled as a task, so the request can trail the
    /// input event's CDP reply — then wait for the new document.
    pub fn navigation_started(&self, since_generation: u64, grace: Duration) -> bool {
        let end = Instant::now() + grace;
        loop {
            if self.nav_generation() > since_generation {
                return true;
            }
            if Instant::now() >= end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// True while a navigation is requested or the document is still being
    /// parsed (before DOMContentLoaded).
    pub fn is_loading(&self) -> bool {
        let state = self.shared.nav_state(&self.session_id);
        state.requested_at.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) || (state.loading && !state.dom_ready)
    }

    pub fn title(&self, timeout: Duration) -> Result<String> {
        let value = self.evaluate_raw("document.title", timeout)?;
        Ok(value.as_str().unwrap_or_default().to_string())
    }

    pub fn url(&self, timeout: Duration) -> Result<String> {
        let value = self.evaluate_raw("location.href", timeout)?;
        Ok(value.as_str().unwrap_or_default().to_string())
    }

    /// Run `function (el, ...args)` on the element `target` names, waiting
    /// up to `wait` for selector/text targets to appear. One round trip for
    /// selector/text; resolve + call + release for refs.
    fn on_element(
        &self,
        target: &Target,
        what: &str,
        function: &str,
        args: &[Value],
        wait: Duration,
        timeout: Duration,
    ) -> Result<Value> {
        let arg_list: Vec<String> = args.iter().map(Value::to_string).collect();
        let extra = if arg_list.is_empty() { String::new() } else { format!(", {}", arg_list.join(", ")) };
        let wait_ms = wait_budget_ms(wait.min(timeout));
        let finder = match target {
            Target::Selector(sel) => format!("({WAIT_FOR})({}, {wait_ms}, {})", json!(sel), json!(what)),
            Target::Text(text) => format!("({WAIT_TEXT})({}, {wait_ms}, {})", json!(text), json!(what)),
            Target::Ref(node) => {
                let fn_on_this = format!(
                    "function (...args) {{ const el = this.nodeType === 1 ? this : this.parentElement; \
                     if (!el) throw new Error({}); return ({function})(el, ...args); }}",
                    json!(format!("{what}: ref is not inside an element"))
                );
                return self.call_on_ref(*node, &fn_on_this, args, true, timeout).map(|(v, _)| v);
            }
        };
        let expr = format!("(async () => {{ const el = await {finder}; return ({function})(el{extra}); }})()");
        self.evaluate_raw(&expr, timeout)
    }

    /// Resolve an `ax` ref (`backendNodeId`) and call `function` on it.
    /// Returns the value (or remote object id when `by_value` is false).
    fn call_on_ref(
        &self,
        backend_node_id: u64,
        function: &str,
        args: &[Value],
        by_value: bool,
        timeout: Duration,
    ) -> Result<(Value, Option<String>)> {
        self.block_on(async {
            let resolved = self
                .client
                .send(
                    "DOM.resolveNode",
                    json!({ "backendNodeId": backend_node_id, "objectGroup": "nv-ref" }),
                    Some(&self.session_id),
                    timeout,
                )
                .await
                .map_err(|_| {
                    anyhow::anyhow!(
                        "ref {backend_node_id} not found (stale after navigation or re-render? take a fresh `ax`)"
                    )
                })?;
            let object_id = resolved
                .get("object")
                .and_then(|o| o.get("objectId"))
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("ref {backend_node_id} has no JS object"))?
                .to_string();
            let arguments: Vec<Value> = args.iter().map(|v| json!({ "value": v })).collect();
            let res = self
                .client
                .send(
                    "Runtime.callFunctionOn",
                    json!({
                        "objectId": object_id,
                        "functionDeclaration": function,
                        "arguments": arguments,
                        "returnByValue": by_value,
                        "awaitPromise": true,
                    }),
                    Some(&self.session_id),
                    timeout,
                )
                .await;
            if by_value {
                // Free the resolved handle whatever happened.
                let _ = self
                    .client
                    .send(
                        "Runtime.releaseObjectGroup",
                        json!({ "objectGroup": "nv-ref" }),
                        Some(&self.session_id),
                        timeout,
                    )
                    .await;
            }
            let res = res.with_context(|| format!("call on ref {backend_node_id}"))?;
            if let Some(details) = res.get("exceptionDetails") {
                bail!("{}", exception_message(details));
            }
            let object = res.get("result").and_then(|r| r.get("objectId")).and_then(Value::as_str).map(str::to_string);
            Ok((res.get("result").and_then(|r| r.get("value")).cloned().unwrap_or(Value::Null), object))
        })
    }

    /// Click: a trusted CDP mouse click at the element's centre, or
    /// DOM-dispatched events when it has no hittable box (see [`CLICK_PREP`]).
    /// Returns `{synthetic: true}` when the fallback was used.
    pub fn click(&self, target: &Target, wait: Duration, timeout: Duration) -> Result<Value> {
        let settle = json!(wait.as_millis().min(3000) as u64);
        let mut point = self.on_element(target, "click", CLICK_PREP, &[json!(false), settle.clone()], wait, timeout)?;
        if point.get("xframe").and_then(Value::as_bool) == Some(true) {
            // Inside a cross-origin iframe the page can't see its frame's
            // offset; CDP reports the box in top-level coordinates.
            match target {
                Target::Ref(node) => match self.content_quad_center(*node, timeout) {
                    Some((x, y)) => {
                        point["x"] = json!(x);
                        point["y"] = json!(y);
                    }
                    None => return self.on_element(target, "click", CLICK_PREP, &[json!(true), settle], wait, timeout),
                },
                _ => return self.on_element(target, "click", CLICK_PREP, &[json!(true), settle], wait, timeout),
            }
        }
        if !self.mouse_click(&point, timeout)? {
            // Engine without the Input domain (e.g. Lightpanda): DOM events.
            return self.on_element(target, "click", CLICK_PREP, &[json!(true), settle], wait, timeout);
        }
        let check = point.get("check").and_then(Value::as_bool) == Some(true)
            && std::env::var_os("NAVIGERA_NO_CLICK_CHECK").is_none(); // diagnostic knob
        if check && !self.press_arrived(timeout) {
            eprintln!(
                "[click] the browser did not deliver the mouse events at ({}, {}); sending them again",
                point["x"], point["y"]
            );
            // Chrome acked the mouse events without delivering them: no
            // pointerdown/mousedown/mouseup/click reached the page. Seen in
            // CI right after a tab switch, when the tab has no hit-test data
            // yet and the browser finds no target for the event. Nothing
            // happened on the page, so sending it again can't double-click.
            self.wait_frame(Duration::from_millis(500));
            self.mouse_click(&point, timeout)?;
            if !self.press_arrived(timeout) {
                eprintln!("[click] still not delivered; dispatching DOM events");
                let mut res = self.on_element(target, "click", CLICK_PREP, &[json!(true), settle], wait, timeout)?;
                res["dropped"] = json!(true);
                return Ok(res);
            }
            point["redelivered"] = json!(true);
        }
        Ok(point)
    }

    /// Did the click's press reach the page (`__nvSeen`, set by CLICK_PREP)?
    /// An error (the click navigated away, the context is gone) counts as
    /// arrived: only a definite "nothing came" triggers a resend.
    /// A slow page gets 300 ms to process the events before "nothing came"
    /// is believed (a dropped event never arrives; a queued one does).
    fn press_arrived(&self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_millis(300);
        loop {
            let seen = self
                .send("Runtime.evaluate", json!({ "expression": "window.__nvSeen", "returnByValue": true }), timeout)
                .ok()
                .map(|r| r.pointer("/result/value").and_then(Value::as_u64));
            match seen {
                Some(Some(0)) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                Some(Some(0)) => return false,
                _ => return true,
            }
        }
    }

    /// Centre of a node's first content quad, in top-level viewport
    /// coordinates (works inside cross-origin iframes).
    fn content_quad_center(&self, backend_node_id: u64, timeout: Duration) -> Option<(f64, f64)> {
        let res = self.send("DOM.getContentQuads", json!({ "backendNodeId": backend_node_id }), timeout).ok()?;
        let quad: Vec<f64> =
            res.get("quads")?.as_array()?.first()?.as_array()?.iter().filter_map(Value::as_f64).collect();
        if quad.len() != 8 {
            return None;
        }
        let x = (quad[0] + quad[2] + quad[4] + quad[6]) / 4.0;
        let y = (quad[1] + quad[3] + quad[5] + quad[7]) / 4.0;
        Some((x, y))
    }

    /// Dispatch a real mouse click at `point.{x,y}`. Ok(false) when the
    /// engine has no Input domain (the very first event fails), so the
    /// caller can fall back to DOM events without ever clicking twice; a
    /// failure after the press is an error. No-op (Ok(true)) when the page
    /// already used DOM events.
    fn mouse_click(&self, point: &Value, timeout: Duration) -> Result<bool> {
        let (Some(x), Some(y)) = (point.get("x").and_then(Value::as_f64), point.get("y").and_then(Value::as_f64))
        else {
            return Ok(true);
        };
        for (i, (kind, button, buttons)) in
            [("mouseMoved", "none", 0), ("mousePressed", "left", 1), ("mouseReleased", "left", 0)]
                .into_iter()
                .enumerate()
        {
            let sent = self.send(
                "Input.dispatchMouseEvent",
                json!({
                    "type": kind, "x": x, "y": y,
                    "button": button, "buttons": buttons, "clickCount": 1,
                }),
                timeout,
            );
            match sent {
                Ok(_) => {}
                Err(_) if i == 0 => return Ok(false),
                Err(e) => return Err(e),
            }
        }
        Ok(true)
    }

    /// Move the mouse over the element (opens hover menus and tooltips).
    /// Returns `{synthetic: true}` when only DOM events could be sent.
    pub fn hover(&self, target: &Target, wait: Duration, timeout: Duration) -> Result<Value> {
        let settle = json!(wait.as_millis().min(3000) as u64);
        let mut point = self.on_element(target, "hover", HOVER_PREP, &[settle], wait, timeout)?;
        if point.get("xframe").and_then(Value::as_bool) == Some(true) {
            let center = match target {
                Target::Ref(node) => self.content_quad_center(*node, timeout),
                _ => None,
            };
            match center {
                Some((x, y)) => point = json!({ "x": x, "y": y }),
                None => bail!("hover: element is inside a cross-origin iframe; target it by ref from `ax`"),
            }
        }
        if let (Some(x), Some(y)) = (point.get("x").and_then(Value::as_f64), point.get("y").and_then(Value::as_f64)) {
            self.send(
                "Input.dispatchMouseEvent",
                json!({ "type": "mouseMoved", "x": x, "y": y, "button": "none", "buttons": 0 }),
                timeout,
            )?;
            // :hover styles apply on the next frame; let two pass so an `ax`
            // right after sees the opened menu.
            self.evaluate_raw(
                "new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(() => r(true))))",
                timeout,
            )?;
        }
        Ok(point)
    }

    /// Set an input/textarea/contenteditable's value (native setter + events).
    pub fn fill(&self, target: &Target, value: &str, wait: Duration, timeout: Duration) -> Result<()> {
        self.on_element(target, "fill", FILL_EL, &[json!(value)], wait, timeout)?;
        Ok(())
    }

    /// Pick `<select>` options by value or label; returns the chosen labels.
    pub fn select(&self, target: &Target, values: &[String], wait: Duration, timeout: Duration) -> Result<Value> {
        self.on_element(target, "select", SELECT_EL, &[json!(values)], wait, timeout)
    }

    /// Scroll: an element into view (or inside it by `dy`), else the window.
    pub fn scroll(
        &self,
        target: Option<&Target>,
        dy: Option<f64>,
        to: Option<&str>,
        wait: Duration,
        timeout: Duration,
    ) -> Result<Value> {
        if let Some(target) = target {
            return self.on_element(target, "scroll", SCROLL_EL, &[json!(dy.unwrap_or(0.0))], wait, timeout);
        }
        let action = match to {
            Some("top") => "window.scrollTo({ top: 0, behavior: 'instant' })".to_string(),
            Some("bottom") => {
                "window.scrollTo({ top: document.documentElement.scrollHeight, behavior: 'instant' })".to_string()
            }
            Some(other) => bail!("scroll --to takes top|bottom, got {other:?}"),
            None => format!("window.scrollBy({{ top: {}, behavior: 'instant' }})", dy.unwrap_or(800.0)),
        };
        self.evaluate_raw(
            &format!("(() => {{ {action}; return {{ scrollY: Math.round(window.scrollY), scrollHeight: document.documentElement.scrollHeight, viewport: window.innerHeight }}; }})()"),
            timeout,
        )
    }

    /// Focus `target` (if any), then press a key or chord like `Enter`,
    /// `Control+a`, `Shift+Tab`.
    /// Press a key or chord. Returns true when the key may start a
    /// navigation (Enter on a form field, link or button), so the caller
    /// knows to wait for one; other keys cost no navigation grace.
    pub fn press(&self, target: Option<&Target>, chord: &str, wait: Duration, timeout: Duration) -> Result<bool> {
        if let Some(target) = target {
            self.on_element(target, "press", FOCUS_EL, &[], wait, timeout)?;
        }
        let enter = matches!(chord.to_ascii_lowercase().as_str(), "enter" | "return");
        // Enter navigates only from a form control, a link or a button.
        // (Cross-origin frames hide their focus: assume it might.)
        let may_navigate =
            enter && self.evaluate_raw(ENTER_MAY_NAVIGATE, timeout).ok().and_then(|v| v.as_bool()).unwrap_or(true);
        let parts: Vec<&str> = chord.split('+').filter(|p| !p.is_empty()).collect();
        let parts = if chord.ends_with("++") || chord == "+" {
            let mut p = parts;
            p.push("+");
            p
        } else {
            parts
        };
        let Some((last, mods)) = parts.split_last() else {
            bail!("press needs a key, e.g. Enter, Tab, Control+a");
        };
        let mut modifiers = 0;
        for m in mods {
            modifiers |= modifier_bit(m)
                .ok_or_else(|| anyhow::anyhow!("press: unknown modifier {m:?} (use Control, Shift, Alt, Meta)"))?;
        }
        let (key, code, vk, text) = key_info(last).ok_or_else(|| {
            anyhow::anyhow!("press: unknown key {last:?} (use Enter, Tab, Escape, ArrowDown, a single character, …)")
        })?;
        // Shift on a single character types its shifted form (US layout):
        // Shift+a is "A", Shift+1 is "!".
        let (key, text) = match (&text, modifiers & 8 != 0) {
            (Some(t), true) if t.chars().count() == 1 && t != "\r" && t != " " => {
                let shifted = shifted_char(t.chars().next().unwrap_or_default()).to_string();
                (shifted.clone(), Some(shifted))
            }
            _ => (key, text),
        };
        // With Control/Alt/Meta held the key produces no text (a shortcut).
        let text = if modifiers & (1 | 2 | 4) != 0 { None } else { text };
        let mut down = json!({
            "type": if text.is_some() { "keyDown" } else { "rawKeyDown" },
            "key": key, "code": code, "windowsVirtualKeyCode": vk, "modifiers": modifiers,
        });
        if let Some(t) = &text {
            down["text"] = json!(t);
            down["unmodifiedText"] = json!(t);
        }
        self.send("Input.dispatchKeyEvent", down, timeout)?;
        self.send(
            "Input.dispatchKeyEvent",
            json!({ "type": "keyUp", "key": key, "code": code, "windowsVirtualKeyCode": vk, "modifiers": modifiers }),
            timeout,
        )?;
        Ok(may_navigate)
    }

    /// Type text key by key into `target` (or the focused element): real
    /// keyDown/keyUp per character, so key listeners and autocomplete fire.
    pub fn type_text(&self, target: Option<&Target>, text: &str, wait: Duration, timeout: Duration) -> Result<()> {
        if let Some(target) = target {
            self.on_element(target, "type", FOCUS_EL, &[], wait, timeout)?;
        }
        for c in text.chars() {
            if c == '\n' {
                self.press(None, "Enter", wait, timeout)?;
                continue;
            }
            let s = c.to_string();
            let (key, code, vk, _) = key_info(&s).unwrap_or((s.clone(), String::new(), 0, None));
            self.send(
                "Input.dispatchKeyEvent",
                json!({ "type": "keyDown", "key": key, "code": code, "windowsVirtualKeyCode": vk, "text": s, "unmodifiedText": s }),
                timeout,
            )?;
            self.send(
                "Input.dispatchKeyEvent",
                json!({ "type": "keyUp", "key": key, "code": code, "windowsVirtualKeyCode": vk }),
                timeout,
            )?;
        }
        Ok(())
    }

    /// Set the files of an `<input type=file>`.
    pub fn upload(&self, target: &Target, files: &[String], wait: Duration, timeout: Duration) -> Result<()> {
        let files: Vec<String> = files
            .iter()
            .map(|f| {
                std::fs::canonicalize(f)
                    .map(|p| p.display().to_string())
                    .map_err(|e| anyhow::anyhow!("upload: {f}: {e}"))
            })
            .collect::<Result<_>>()?;
        let mut params = json!({ "files": files });
        match target {
            Target::Ref(node) => params["backendNodeId"] = json!(node),
            _ => {
                // Find the element (auto-wait), keep it as a remote object.
                let finder = match target {
                    Target::Selector(sel) => {
                        format!("({WAIT_FOR})({}, {}, 'upload')", json!(sel), wait_budget_ms(wait))
                    }
                    Target::Text(t) => format!("({WAIT_TEXT})({}, {}, 'upload')", json!(t), wait_budget_ms(wait)),
                    Target::Ref(_) => unreachable!(),
                };
                let res = self.send(
                    "Runtime.evaluate",
                    json!({ "expression": format!("(async () => {{ const el = await {finder}; return ({IDENTITY})(el); }})()"),
                            "awaitPromise": true, "objectGroup": "nv-upload" }),
                    timeout,
                )?;
                if let Some(details) = res.get("exceptionDetails") {
                    bail!("{}", exception_message(details));
                }
                let object = res
                    .get("result")
                    .and_then(|r| r.get("objectId"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("upload: element has no remote object"))?;
                params["objectId"] = json!(object);
            }
        }
        let result = self.send("DOM.setFileInputFiles", params, timeout);
        let _ = self.send("Runtime.releaseObjectGroup", json!({ "objectGroup": "nv-upload" }), timeout);
        result.map_err(|e| anyhow::anyhow!("upload: {e:#} (is it an <input type=file>?)"))?;
        Ok(())
    }

    /// First matching element's textContent, if present.
    pub fn text_content(&self, selector: &str, timeout: Duration) -> Result<Option<String>> {
        let sel = serde_json::to_string(selector)?;
        let value =
            self.evaluate_raw(&format!("(document.querySelector({sel}) || {{}}).textContent ?? null"), timeout)?;
        Ok(value.as_str().map(str::to_string))
    }

    /// One poll of a `wait` condition; true when satisfied.
    pub fn check(&self, js_predicate: &str, timeout: Duration) -> Result<bool> {
        Ok(self.evaluate_raw(js_predicate, timeout)?.as_bool().unwrap_or(false))
    }

    /// PNG screenshot bytes; `full_page` captures beyond the viewport.
    pub fn screenshot_png(&self, timeout: Duration, full_page: bool) -> Result<Vec<u8>> {
        let mut params = json!({ "format": "png" });
        if full_page {
            params["captureBeyondViewport"] = Value::Bool(true);
        }
        let res = self.send("Page.captureScreenshot", params, timeout)?;
        let data =
            res.get("data").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("captureScreenshot: no data"))?;
        base64::engine::general_purpose::STANDARD.decode(data).context("screenshot base64 decode")
    }

    /// Child frames' AX trees, each keyed by its owner `<iframe>` node.
    fn frame_trees(&self, timeout: Duration) -> Vec<ax::Frame> {
        let Ok(tree) = self.send("Page.getFrameTree", json!({}), timeout) else {
            return Vec::new();
        };
        let mut ids = Vec::new();
        fn collect(node: &Value, out: &mut Vec<String>) {
            for child in node.get("childFrames").and_then(Value::as_array).into_iter().flatten() {
                if let Some(id) = child.get("frame").and_then(|f| f.get("id")).and_then(Value::as_str) {
                    out.push(id.to_string());
                }
                collect(child, out);
            }
        }
        collect(tree.get("frameTree").unwrap_or(&Value::Null), &mut ids);
        let mut frames = Vec::new();
        for id in ids.into_iter().take(20) {
            let Ok(owner) = self.send("DOM.getFrameOwner", json!({ "frameId": id }), timeout) else {
                continue;
            };
            let Some(owner_backend_id) = owner.get("backendNodeId").and_then(Value::as_u64) else {
                continue;
            };
            // Out-of-process frames live in another target; skipped here.
            let Ok(res) = self.send("Accessibility.getFullAXTree", json!({ "frameId": id }), timeout) else {
                continue;
            };
            let nodes = res.get("nodes").and_then(Value::as_array).cloned().unwrap_or_default();
            frames.push(ax::Frame { owner_backend_id, nodes });
        }
        frames
    }

    /// Backend node id of the element `target` names (for `ax --selector`).
    fn backend_id(&self, target: &Target, wait: Duration, timeout: Duration) -> Result<u64> {
        if let Target::Ref(node) = target {
            return Ok(*node);
        }
        let finder = match target {
            Target::Selector(sel) => format!("({WAIT_FOR})({}, {}, 'ax')", json!(sel), wait_budget_ms(wait)),
            Target::Text(t) => format!("({WAIT_TEXT})({}, {}, 'ax')", json!(t), wait_budget_ms(wait)),
            Target::Ref(_) => unreachable!(),
        };
        let res = self.send(
            "Runtime.evaluate",
            json!({ "expression": format!("(async () => await {finder})()"), "awaitPromise": true, "objectGroup": "nv-ax" }),
            timeout,
        )?;
        if let Some(details) = res.get("exceptionDetails") {
            bail!("{}", exception_message(details));
        }
        let object = res
            .get("result")
            .and_then(|r| r.get("objectId"))
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("ax: scope element has no remote object"))?
            .to_string();
        let described = self.send("DOM.describeNode", json!({ "objectId": object }), timeout);
        let _ = self.send("Runtime.releaseObjectGroup", json!({ "objectGroup": "nv-ax" }), timeout);
        described?
            .get("node")
            .and_then(|n| n.get("backendNodeId"))
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("ax: no backend node id for scope"))
    }

    /// Accessibility snapshot for agents (see [`ax`]): an indented text
    /// tree with `[ref=N]` on actionable nodes, iframes included. `json`
    /// keeps the older flat `[{ref, role, name, value?}]` list.
    pub fn ax_tree(&self, timeout: Duration, opts: &AxOptions) -> Result<Value> {
        let mut params = json!({});
        if let Some(d) = opts.max_depth {
            params["depth"] = json!(d);
        }
        let res = self.send("Accessibility.getFullAXTree", params, timeout)?;
        let nodes = res.get("nodes").and_then(Value::as_array).cloned().unwrap_or_default();
        if opts.json {
            return Ok(flat_ax(&nodes, opts.all));
        }
        let frames = self.frame_trees(timeout);
        let root = nodes.iter().find(|n| n.get("parentId").is_none());
        let title = root.map(|n| ax_str(n, "name").to_string()).unwrap_or_default();
        let url = root
            .and_then(|n| n.get("properties").and_then(Value::as_array))
            .and_then(|props| props.iter().find(|p| p.get("name").and_then(Value::as_str) == Some("url")))
            .and_then(|p| p.get("value").and_then(|v| v.get("value")).and_then(Value::as_str))
            .unwrap_or_default()
            .to_string();
        let origin = url
            .find("://")
            .and_then(|i| url[i + 3..].find('/').map(|j| url[..i + 3 + j].to_string()))
            .unwrap_or_default();
        let root_backend_id = match &opts.scope {
            Some(target) => Some(self.backend_id(target, DEFAULT_ELEMENT_WAIT, timeout)?),
            None => None,
        };
        let rendered = ax::render(
            &format!("page {} {url}", serde_json::to_string(&title).unwrap_or_default()),
            &nodes,
            &frames,
            &ax::Options { all_refs: opts.all_refs, limit: opts.limit, root_backend_id, origin },
        );
        Ok(Value::String(rendered.text))
    }

    /// Close this tab's target.
    pub fn close_target(&self, timeout: Duration) -> Result<()> {
        self.shared.unregister(&self.session_id);
        self.block_on(async {
            self.client.send("Target.closeTarget", json!({ "targetId": self.target_id }), None, timeout).await?;
            Ok(())
        })
    }
}

/// AX roles an agent can act on; always kept by the flat JSON view.
const AX_INTERACTIVE: &[&str] = ax::INTERACTIVE;

/// AX roles that carry no information of their own in the flat view.
const AX_STRUCTURAL: &[&str] = &["generic", "none", "presentation", "InlineTextBox", "LineBreak"];

/// The older flat JSON snapshot (`ax --format json`).
fn flat_ax(nodes: &[Value], all: bool) -> Value {
    let live = nodes.iter().filter(|n| !n.get("ignored").and_then(Value::as_bool).unwrap_or(false));
    if all {
        return Value::Array(
            live.map(|n| {
                json!({
                    "id": n.get("nodeId").and_then(Value::as_str).unwrap_or(""),
                    "role": ax_str(n, "role"),
                    "name": ax_str(n, "name"),
                    "backendNodeId": n.get("backendDOMNodeId").and_then(Value::as_u64).unwrap_or(0),
                })
            })
            .collect(),
        );
    }
    let names: std::collections::HashMap<&str, &str> =
        nodes.iter().filter_map(|n| Some((n.get("nodeId")?.as_str()?, ax_str(n, "name")))).collect();
    Value::Array(
        live.filter_map(|n| {
            let role = ax_str(n, "role");
            let name = ax_str(n, "name");
            let node_ref = n.get("backendDOMNodeId").and_then(Value::as_u64)?;
            if !AX_INTERACTIVE.contains(&role) {
                if name.is_empty() || AX_STRUCTURAL.contains(&role) {
                    return None;
                }
                let parent_name =
                    n.get("parentId").and_then(Value::as_str).and_then(|p| names.get(p).copied()).unwrap_or("");
                if role == "StaticText" && name == parent_name {
                    return None;
                }
            }
            let mut out = json!({ "ref": node_ref, "role": role, "name": name });
            let value = ax_str(n, "value");
            if !value.is_empty() {
                out["value"] = json!(value);
            }
            if role == "link" {
                if let Some(Value::String(url)) = ax::prop(n, "url") {
                    if !url.is_empty() && !url.starts_with("javascript:") {
                        out["url"] = json!(url);
                    }
                }
            }
            Some(out)
        })
        .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_chords_resolve() {
        assert_eq!(key_info("Enter").unwrap().3.as_deref(), Some("\r"));
        assert_eq!(key_info("esc").unwrap().0, "Escape");
        assert_eq!(key_info("a").unwrap(), ("a".into(), "KeyA".into(), 65, Some("a".into())));
        assert_eq!(key_info("F5").unwrap().2, 116);
        assert!(key_info("Bogus").is_none());
        assert_eq!(modifier_bit("Ctrl"), Some(2));
    }

    #[test]
    fn exception_messages_drop_stack_frames() {
        let details = json!({"text": "Uncaught", "exception": {"description": "Error: click: no element\n    at foo (x.js:1:1)\n    at bar"}});
        assert_eq!(exception_message(&details), "Error: click: no element");
    }
}
