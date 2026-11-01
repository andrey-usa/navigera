//! Accessibility snapshot rendering: Chrome's `Accessibility.getFullAXTree`
//! nodes in, an indented text tree for agents out.
//!
//! ```text
//! page "Acme — Cart" http://127.0.0.1:8000/cart
//! - banner:
//!   - link "Acme Supply" [ref=12] url=/
//!   - navigation "Main":
//!     - link "Products" [ref=15] url=/products
//! - heading "Your cart" [level=1]
//! - list:
//!   - listitem:
//!     - text: Brass Sprocket × 2
//!     - button "Remove" [ref=48]
//! - combobox "Country" [ref=51] value="Canada" options: "United States", "Canada", "Mexico"
//! ```
//!
//! Why a tree and not a flat list: in a product grid every "Add to cart"
//! button has the same name; only the nesting says which product it belongs
//! to. Wrapper nodes (`generic`, `none`, label boxes) are collapsed into
//! their children, text that repeats an ancestor's name is dropped, and refs
//! (`backendNodeId`s, usable with `click`/`fill`/`select`/… `--ref`) are
//! only printed on nodes an agent can act on, so a 500-card listing stays a
//! fraction of the raw tree's size.

use std::collections::HashMap;

use serde_json::Value;

/// Roles an agent can act on; always get a ref.
pub const INTERACTIVE: &[&str] = &[
    "button",
    "link",
    "textbox",
    "searchbox",
    "checkbox",
    "radio",
    "combobox",
    "listbox",
    "option",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "tab",
    "switch",
    "slider",
    "spinbutton",
    "treeitem",
    "DisclosureTriangle",
];

/// Roles that only wrap other nodes: collapsed (children promoted) unless
/// they are focusable or carry a name of their own.
const WRAPPERS: &[&str] = &[
    "generic",
    "none",
    "presentation",
    "LabelText",
    "LayoutTable",
    "LayoutTableRow",
    "LayoutTableCell",
    "Section",
    "Div",
    "Pre",
];

/// Roles never printed (their text is carried by the parent).
const SILENT: &[&str] = &["InlineTextBox", "LineBreak"];

/// Roles whose accessible name may come from their content (ARIA "name
/// from content", plus Chrome's text and disclosure roles). Any other role
/// is named by its author (aria-label, a caption, a legend) or not at all.
const NAME_FROM_CONTENT: &[&str] = &[
    "button",
    "cell",
    "checkbox",
    "columnheader",
    "gridcell",
    "heading",
    "link",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "option",
    "radio",
    "row",
    "rowheader",
    "sectionhead",
    "switch",
    "tab",
    "tooltip",
    "treeitem",
    "term",
    "StaticText",
    "LabelText",
    "DisclosureTriangle",
];

/// Rendering options.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Print a ref on every node with a DOM node, not just actionable ones.
    pub all_refs: bool,
    /// Stop after this many lines (0 = unlimited).
    pub limit: usize,
    /// Render only the subtree of the node with this backend DOM node id.
    pub root_backend_id: Option<u64>,
    /// Page origin (`http://host:port`) for shortening same-origin link urls.
    pub origin: String,
    /// What the page makes clickable beyond the tree's actionable roles.
    pub clicks: Clickables,
    /// The lazy walk gives up (and the caller reads the whole tree in one
    /// call instead) once it projects more child fetches than this to reach
    /// the limit. `None`: no budget.
    pub lazy_budget: Option<usize>,
    /// Texts, names and values in full (`ax --full-text`), not cut at
    /// [`TEXT_MAX`] / [`NAME_MAX`].
    pub full_text: bool,
}

/// Fetched nodes the lazy walk reaches before it projects its total from
/// the lines they gave.
pub const LAZY_PROBE: usize = 24;

/// Elements a user could click though the tree gives them no actionable
/// role (a `<div>` row with a click handler, an icon `<span>`), by backend
/// node id; and `#id` / `.class` per element, printed for actionable nodes
/// with no name or text, so `[ref=7]` and `[ref=8]` aren't two blanks.
#[derive(Clone, Debug, Default)]
pub struct Clickables {
    pub nodes: std::collections::HashSet<u64>,
    pub hints: HashMap<u64, String>,
    /// The page's own text of a marked element, for one the tree shows
    /// without any (a label Chrome leaves out with its hidden control).
    pub texts: HashMap<u64, String>,
}

/// One frame's AX nodes, spliced under the AX node of its owner element
/// (`<iframe>` backend node id).
#[derive(Clone)]
pub struct Frame {
    pub owner_backend_id: u64,
    pub nodes: Vec<Value>,
}

pub struct Rendered {
    pub text: String,
    /// Lines emitted before truncation.
    pub lines: usize,
    /// Lines dropped by `limit`.
    pub truncated: usize,
    /// Texts, names and values cut short, and the characters they left out.
    pub cuts: usize,
    pub cut_chars: usize,
}

fn field<'a>(node: &'a Value, key: &str) -> &'a Value {
    node.get(key).and_then(|v| v.get("value")).unwrap_or(&Value::Null)
}

fn text_of(node: &Value, key: &str) -> String {
    match field(node, key) {
        Value::String(s) => squash(s),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// Collapse runs of whitespace (AX names keep the page's newlines/indent).
fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Characters of text a line shows before it cuts.
const TEXT_MAX: usize = 300;
/// Characters of a name or value shown before it cuts.
const NAME_MAX: usize = 150;

/// `s` cut to `max` characters, saying how much was left out and, given the
/// node it came from, how to read all of it.
fn clip(s: &str, max: usize, read_by: Option<u64>) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push_str(&cut_note(n - max, read_by));
    out
}

fn cut_note(omitted: usize, read_by: Option<u64>) -> String {
    match read_by {
        Some(id) => format!("… (+{omitted} chars: get text {id})"),
        None => format!("… (+{omitted} chars)"),
    }
}

fn quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
}

/// `s` quoted, cut to `max` characters with the cut said outside the quotes
/// (inside, it would read as part of the name).
fn quote_clip(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return quote(s);
    }
    let cut: String = s.chars().take(max).collect();
    format!("{}{}", quote(&cut), cut_note(n - max, None))
}

pub(crate) fn prop<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    node.get("properties")?
        .as_array()?
        .iter()
        .find(|p| p.get("name").and_then(Value::as_str) == Some(name))
        .and_then(|p| p.get("value"))
        .and_then(|v| v.get("value"))
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s == "true",
        _ => false,
    }
}

/// Batch lazy fetch: given AX node ids, return the child node objects of
/// those nodes (one `Accessibility.getChildAXNodes` per id, sent together),
/// and whether any call failed. A failed call is not "no children": the
/// caller falls back to the full tree rather than print a partial one.
pub type AxFetch = Box<dyn FnMut(&[String]) -> (Vec<Value>, bool)>;

/// How many siblings' children one lazy round trip prefetches: the walk
/// visits siblings in order, so a window covers the next lines without
/// fetching the whole list (a 32000-item list is one call for its items,
/// then windows of their children).
const LAZY_WINDOW: usize = 128;

fn ax_id(node: &Value) -> &str {
    node.get("nodeId").and_then(Value::as_str).unwrap_or("")
}

fn child_ids(node: &Value) -> Vec<String> {
    node.get("childIds")
        .and_then(Value::as_array)
        .map(|ids| ids.iter().filter_map(|id| id.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

fn backend_id(node: &Value) -> Option<u64> {
    node.get("backendDOMNodeId").and_then(Value::as_u64)
}

struct Tree<'a> {
    /// Node objects by AX id, in first-seen order. Nodes are addressed by
    /// index (not reference) so fetching more children mid-walk cannot
    /// invalidate a node the walk already holds.
    nodes: Vec<Value>,
    by_id: HashMap<String, usize>,
    by_backend: HashMap<u64, usize>,
    frame_roots: HashMap<u64, Vec<usize>>,
    frame_root_ids: std::collections::HashSet<usize>,
    opts: &'a Options,
    lines: Vec<String>,
    fetch: Option<AxFetch>,
    /// Nodes whose children the lazy fetch already asked for (a node the
    /// browser answered without is not asked again).
    fetched: std::collections::HashSet<usize>,
    fetch_failed: bool,
    /// Child fetches so far, and the fetched nodes the walk has reached
    /// (see [`Options::lazy_budget`]): fetches run ahead of the walk by up
    /// to a window of siblings, so the lines per fetch are read off the
    /// fetches it has used.
    calls: usize,
    used: std::collections::HashSet<usize>,
    /// The output limit was reached mid-walk: no more lines are printed.
    stopped: bool,
    /// Line indices whose node's children were cut off by the limit (the
    /// trailing ':' the full walk would have added is fixed up at the end).
    boundary: Vec<(usize, usize)>,
    /// Texts cut short so far, and the characters they left out.
    cuts: std::cell::Cell<usize>,
    cut_chars: std::cell::Cell<usize>,
}

impl<'a> Tree<'a> {
    /// [`clip`], counting the cut (or the text in full with `full_text`).
    fn clip(&self, s: &str, max: usize, read_by: Option<u64>) -> String {
        if self.opts.full_text {
            return s.to_string();
        }
        self.count_cut(s, max);
        clip(s, max, read_by)
    }

    /// [`quote_clip`], counting the cut (or quoted in full with `full_text`).
    fn quote_clip(&self, s: &str, max: usize) -> String {
        if self.opts.full_text {
            return quote(s);
        }
        self.count_cut(s, max);
        quote_clip(s, max)
    }

    fn count_cut(&self, s: &str, max: usize) {
        let n = s.chars().count();
        if n > max {
            self.cuts.set(self.cuts.get() + 1);
            self.cut_chars.set(self.cut_chars.get() + n - max);
        }
    }

    fn insert(&mut self, node: Value) -> usize {
        let id = ax_id(&node).to_string();
        if let Some(&i) = self.by_id.get(&id) {
            return i;
        }
        let i = self.nodes.len();
        if let Some(b) = backend_id(&node) {
            self.by_backend.entry(b).or_insert(i);
        }
        self.by_id.insert(id, i);
        self.nodes.push(node);
        i
    }

    fn children(&mut self, idx: usize) -> Vec<usize> {
        let ids = child_ids(&self.nodes[idx]);
        if self.fetch.is_some() && ids.iter().any(|id| !self.by_id.contains_key(id)) {
            // This node plus the siblings the walk visits next, in one
            // round trip; siblings already fetched, or not yet known, wait.
            let mut todo: Vec<String> = Vec::new();
            if !self.fetched.contains(&idx) {
                todo.push(ax_id(&self.nodes[idx]).to_string());
            }
            if let Some(parent) = self.nodes[idx].get("parentId").and_then(Value::as_str).map(str::to_string) {
                if let Some(&p) = self.by_id.get(&parent) {
                    let sibs = child_ids(&self.nodes[p]);
                    if let Some(pos) = sibs.iter().position(|s| s == ax_id(&self.nodes[idx])) {
                        for s in sibs.iter().skip(pos + 1).take(LAZY_WINDOW - 1) {
                            if let Some(&si) = self.by_id.get(s) {
                                if !self.fetched.contains(&si) {
                                    todo.push(s.clone());
                                }
                            }
                        }
                    }
                }
            }
            // Never more fetches than the budget allows, whatever the
            // projection: past it, one full read is the cheaper way.
            if let Some(budget) = self.opts.lazy_budget {
                if self.calls + todo.len() > budget {
                    self.give_up("its fetch budget is spent");
                    todo.clear();
                }
            }
            if !todo.is_empty() {
                let done: Vec<usize> = todo.iter().filter_map(|id| self.by_id.get(id).copied()).collect();
                if let Some(mut fetch) = self.fetch.take() {
                    let (got, failed) = fetch(&todo);
                    self.fetch = Some(fetch);
                    if failed {
                        self.fetch_failed = true;
                    }
                    self.calls += todo.len();
                    for i in done {
                        self.fetched.insert(i);
                    }
                    for n in got {
                        self.insert(n);
                    }
                }
            }
        }
        if self.fetched.contains(&idx) && self.used.insert(idx) {
            self.check_budget();
        }
        let mut out: Vec<usize> = ids.iter().filter_map(|id| self.by_id.get(id).copied()).collect();
        // An <iframe>'s document lives in another frame's tree.
        if let Some(backend) = backend_id(&self.nodes[idx]) {
            if let Some(roots) = self.frame_roots.get(&backend) {
                out.extend(roots.iter().copied());
            }
        }
        out
    }

    /// On a page where each line costs many fetches (deep wrappers, wide
    /// rows), reading the whole tree at once is cheaper: once the fetches
    /// used project past the budget, give up, and the caller does that.
    fn check_budget(&mut self) {
        let Some(budget) = self.opts.lazy_budget else { return };
        let used = self.used.len();
        let projected = used * self.opts.limit / self.lines.len().max(1);
        if used >= LAZY_PROBE && projected > budget {
            let why = format!("{used} fetches for {} lines project {projected}, past {budget}", self.lines.len());
            self.give_up(&why);
        }
    }

    fn give_up(&mut self, why: &str) {
        crate::timing::log(&format!("[ax] lazy walk gives up for one full read: {why}"));
        self.fetch = None;
        self.fetch_failed = true;
        self.stopped = true;
    }

    fn role(node: &Value) -> &str {
        field(node, "role").as_str().unwrap_or("")
    }

    fn push_line(&mut self, line: String) {
        self.lines.push(line);
        // One line past the limit: the walk stops, and the truncation step
        // still sees there was more (without it, a walk that ends exactly
        // at the limit could not tell "done" from "cut off").
        if self.opts.limit > 0 && self.lines.len() > self.opts.limit {
            self.stopped = true;
        }
    }

    /// The node's name as the agent should read it. Some engines (Lightpanda)
    /// name a table, list or `<b>` with all of its text: that is content,
    /// not a label, and as a name it would hide the very text it repeats.
    fn name(&mut self, idx: usize) -> String {
        let name = text_of(&self.nodes[idx], "name");
        let role = Self::role(&self.nodes[idx]).to_string();
        if name.is_empty() || NAME_FROM_CONTENT.contains(&role.as_str()) || INTERACTIVE.contains(&role.as_str()) {
            return name;
        }
        let mut text = Vec::new();
        self.descendant_text(idx, &mut text, 0);
        if !text.is_empty() && squash(&text.join(" ")) == name {
            return String::new();
        }
        name
    }

    fn descendant_text(&mut self, idx: usize, out: &mut Vec<String>, depth: usize) {
        if depth > 64 {
            return;
        }
        for child in self.children(idx) {
            if Self::role(&self.nodes[child]) == "StaticText" {
                let t = text_of(&self.nodes[child], "name");
                if !t.is_empty() {
                    out.push(t);
                }
            } else {
                self.descendant_text(child, out, depth + 1);
            }
        }
    }

    /// A wrapper the page made clickable, that holds nothing else to act on
    /// (a listener on a container is the page listening for its children),
    /// with something to tell it by: a name, text or its `#id` / `.class`.
    /// A run of blank `clickable` lines (a hidden menu's tiles) is nothing
    /// an agent can choose between.
    fn is_clickable(&mut self, idx: usize) -> bool {
        let Some(id) = backend_id(&self.nodes[idx]) else { return false };
        if !self.opts.clicks.nodes.contains(&id)
            || INTERACTIVE.contains(&Self::role(&self.nodes[idx]))
            || self.holds_actionable(idx, 0)
        {
            return false;
        }
        if !text_of(&self.nodes[idx], "name").is_empty()
            || self.opts.clicks.hints.contains_key(&id)
            || self.opts.clicks.texts.contains_key(&id)
        {
            return true;
        }
        let mut text = Vec::new();
        self.descendant_text(idx, &mut text, 0);
        text.iter().any(|t| !t.trim().is_empty())
    }

    fn holds_actionable(&mut self, idx: usize, depth: usize) -> bool {
        depth < 64
            && self.children(idx).into_iter().any(|child| {
                let role = Self::role(&self.nodes[child]).to_string();
                INTERACTIVE.contains(&role.as_str())
                    || backend_id(&self.nodes[child]).is_some_and(|id| self.opts.clicks.nodes.contains(&id))
                    || self.holds_actionable(child, depth + 1)
            })
    }

    fn is_transparent(&mut self, idx: usize) -> bool {
        if self.is_clickable(idx) {
            return false;
        }
        if self.nodes[idx].get("ignored").and_then(Value::as_bool).unwrap_or(false) {
            return true;
        }
        let role = Self::role(&self.nodes[idx]).to_string();
        if role == "RootWebArea" && !self.is_frame_root(idx) {
            return true;
        }
        WRAPPERS.contains(&role.as_str()) && self.name(idx).is_empty() && !truthy(prop(&self.nodes[idx], "focusable"))
    }

    fn is_frame_root(&self, idx: usize) -> bool {
        // Child-frame documents are rendered as `- document "Title":`.
        self.frame_root_ids.contains(&idx)
    }

    /// Children with transparent wrappers expanded in place.
    fn effective_children(&mut self, idx: usize, out: &mut Vec<usize>) {
        for child in self.children(idx) {
            if SILENT.contains(&Self::role(&self.nodes[child])) {
                continue;
            }
            if self.is_transparent(child) {
                self.effective_children(child, out);
            } else {
                out.push(child);
            }
        }
    }

    fn wants_ref(&mut self, idx: usize, role: &str) -> bool {
        self.opts.all_refs
            || INTERACTIVE.contains(&role)
            || truthy(prop(&self.nodes[idx], "focusable"))
            || self.is_clickable(idx)
    }

    fn states(node: &Value, role: &str) -> Vec<String> {
        let mut out = Vec::new();
        match prop(node, "checked") {
            Some(Value::String(s)) if s == "true" => out.push("checked".into()),
            Some(Value::String(s)) if s == "mixed" => out.push("checked=mixed".into()),
            Some(Value::Bool(true)) => out.push("checked".into()),
            _ => {}
        }
        for (name, label) in [
            ("pressed", "pressed"),
            ("disabled", "disabled"),
            ("required", "required"),
            ("readonly", "readonly"),
            ("modal", "modal"),
        ] {
            if truthy(prop(node, name)) {
                out.push(label.into());
            }
        }
        if role != "option" && truthy(prop(node, "selected")) {
            out.push("selected".into());
        }
        match prop(node, "expanded") {
            Some(v) if truthy(Some(v)) => out.push("expanded".into()),
            Some(_) if role != "combobox" => out.push("collapsed".into()),
            _ => {}
        }
        if let Some(Value::String(s)) = prop(node, "invalid") {
            if s != "false" {
                out.push("invalid".into());
            }
        }
        if let Some(level) = prop(node, "level").and_then(Value::as_u64) {
            out.push(format!("level={level}"));
        }
        out
    }

    fn short_url(&self, url: &str) -> String {
        if !self.opts.origin.is_empty() {
            if let Some(rest) = url.strip_prefix(&self.opts.origin) {
                if rest.starts_with('/') {
                    return rest.to_string();
                }
            }
        }
        url.to_string()
    }

    fn walk(&mut self, idx: usize, depth: usize, covered: &str) {
        if self.stopped {
            return;
        }
        let role = Self::role(&self.nodes[idx]).to_string();
        if SILENT.contains(&role.as_str()) {
            return;
        }
        if self.is_transparent(idx) {
            for child in self.children(idx) {
                if self.stopped {
                    break;
                }
                self.walk(child, depth, covered);
            }
            return;
        }
        let indent = "  ".repeat(depth);
        let name = self.name(idx);

        if role == "StaticText" {
            if name.is_empty() || (!covered.is_empty() && covered.contains(&name)) {
                return;
            }
            let read_by = backend_id(&self.nodes[idx]);
            self.push_line(format!("{indent}- text: {}", self.clip(&name, TEXT_MAX, read_by)));
            return;
        }

        // A <select>'s options go on its line. Lightpanda names a select
        // inside a <label> with the label's whole text, options included
        // ("Currency USD EUR GBP …"): those words come off the name.
        let mut options = Vec::new();
        if role == "combobox" || role == "listbox" {
            collect_options(self, idx, &mut options);
        }
        let name = without_options(name, &options);

        let clickable = self.is_clickable(idx);
        let shown_role = if self.is_frame_root(idx) {
            "document"
        } else if clickable
            && (WRAPPERS.contains(&role.as_str()) || self.nodes[idx].get("ignored") == Some(&Value::Bool(true)))
        {
            "clickable"
        } else {
            role.as_str()
        };
        let mut line = format!("{indent}- {shown_role}");
        if !name.is_empty() {
            line.push(' ');
            line.push_str(&self.quote_clip(&name, NAME_MAX));
        } else if let Some(text) = backend_id(&self.nodes[idx])
            .filter(|_| clickable)
            .and_then(|id| self.opts.clicks.texts.get(&id))
            .filter(|_| {
                let mut seen = Vec::new();
                self.descendant_text(idx, &mut seen, 0);
                seen.iter().all(|t| t.trim().is_empty())
            })
        {
            line.push(' ');
            line.push_str(&self.quote_clip(text, NAME_MAX));
        }
        let mut tags = Self::states(&self.nodes[idx], &role);
        if let Some(id) = backend_id(&self.nodes[idx]) {
            if self.wants_ref(idx, &role) {
                // Nothing to read it by (an icon button, an empty `<span>`
                // with a handler): say which element it is.
                if name.is_empty() && text_of(&self.nodes[idx], "value").is_empty() {
                    let mut text = Vec::new();
                    self.descendant_text(idx, &mut text, 0);
                    if text.iter().all(|t| t.trim().is_empty()) {
                        if let Some(hint) = self.opts.clicks.hints.get(&id) {
                            line.push(' ');
                            line.push_str(hint);
                        }
                        // `aria-describedby`: a tooltip kept elsewhere.
                        let described = text_of(&self.nodes[idx], "description");
                        if !described.is_empty() && !line.contains(&quote(&described)) {
                            line.push_str(&format!(" description={}", quote_clip(&described, 60)));
                        }
                    }
                }
                tags.push(format!("ref={id}"));
            }
        }
        if !tags.is_empty() {
            line.push_str(&format!(" [{}]", tags.join(", ")));
        }
        // A checkbox's or radio's state is [checked]; its value attribute
        // ("on" unless set, which Lightpanda reports) reads like a state.
        let value = if CHECKABLE.contains(&role.as_str()) { String::new() } else { text_of(&self.nodes[idx], "value") };
        if !value.is_empty() && value != name {
            line.push_str(&format!(" value={}", self.quote_clip(&value, NAME_MAX)));
        }
        if role == "link" {
            if let Some(Value::String(url)) = prop(&self.nodes[idx], "url") {
                if !url.is_empty() && !url.starts_with("javascript:") {
                    line.push_str(&format!(" url={}", self.short_url(url)));
                }
            }
        }

        let mut kids = Vec::new();
        self.effective_children(idx, &mut kids);

        // <select>: list the options on the combobox line, not as a subtree.
        if (role == "combobox" || role == "listbox") && !options.is_empty() {
            let shown: Vec<String> = options.iter().take(25).map(|o| quote_clip(o, 60)).collect();
            line.push_str(&format!(" options: {}", shown.join(", ")));
            if options.len() > 25 {
                line.push_str(&format!(", …(+{})", options.len() - 25));
            }
            self.push_line(line);
            return;
        }

        // Unnamed node holding only text: inline it (`- paragraph: …`).
        let child_cover = if name.is_empty() { covered.to_string() } else { name.clone() };
        if !kids.is_empty() && kids.iter().all(|k| Self::role(&self.nodes[*k]) == "StaticText") {
            let text: Vec<String> = kids
                .iter()
                .map(|k| text_of(&self.nodes[*k], "name"))
                .filter(|t| !t.is_empty() && !child_cover.contains(t.as_str()) && *t != value)
                .collect();
            if !text.is_empty() {
                line.push_str(&format!(": {}", self.clip(&text.join(" "), TEXT_MAX, backend_id(&self.nodes[idx]))));
            }
            self.push_line(line);
            return;
        }
        // A named wrapper around one same-named actionable child (a heading
        // holding its link): one line, name printed once.
        if kids.len() == 1 && !name.is_empty() && !INTERACTIVE.contains(&role.as_str()) {
            let only = kids[0];
            let only_role = Self::role(&self.nodes[only]).to_string();
            if text_of(&self.nodes[only], "name") == name && INTERACTIVE.contains(&only_role.as_str()) {
                let mut grand = Vec::new();
                self.effective_children(only, &mut grand);
                let leafish = grand.iter().all(|k| Self::role(&self.nodes[*k]) == "StaticText");
                if leafish {
                    let inner = self.describe(only, &only_role);
                    line.push_str(&format!(": {inner}"));
                    self.push_line(line);
                    return;
                }
            }
        }
        // An unnamed table cell or list item around one control: one line
        // (Lightpanda leaves cells unnamed, so each checkbox in a table
        // took two lines where Chrome's tree takes one).
        let controls: Vec<usize> =
            kids.iter().copied().filter(|k| Self::role(&self.nodes[*k]) != "StaticText").collect();
        let echoes_only = controls.len() == 1 && {
            let control_name = self.name(controls[0]);
            kids.iter()
                .filter(|k| Self::role(&self.nodes[**k]) == "StaticText")
                .all(|k| text_of(&self.nodes[*k], "name").trim().trim_end_matches(':') == control_name.trim())
        };
        if echoes_only && name.is_empty() && matches!(role.as_str(), "cell" | "gridcell" | "listitem") {
            let only = controls[0];
            let only_role = Self::role(&self.nodes[only]).to_string();
            let mut grand = Vec::new();
            self.effective_children(only, &mut grand);
            if INTERACTIVE.contains(&only_role.as_str())
                && grand.iter().all(|k| Self::role(&self.nodes[*k]) == "StaticText")
            {
                let only_name = self.name(only);
                let mut inner = self.describe(only, &only_role);
                if !only_name.is_empty() {
                    inner.insert_str(only_role.len(), &format!(" {}", self.quote_clip(&only_name, NAME_MAX)));
                }
                line.push_str(&format!(": {inner}"));
                self.push_line(line);
                return;
            }
        }
        let has_ref = line.contains("ref=");
        if kids.is_empty()
            && name.is_empty()
            && !has_ref
            && value.is_empty()
            && !INTERACTIVE.contains(&role.as_str())
            && role != "Iframe"
        {
            return; // an empty live region / paragraph says nothing
        }
        let at = self.lines.len();
        self.push_line(line);
        if self.stopped {
            // The limit cut inside this node's children: its line stays,
            // and the trailing ':' is fixed up once the walk unwinds.
            self.boundary.push((at, idx));
            return;
        }
        self.walk_kids(kids, depth + 1, &child_cover);
        if self.lines.len() > at + 1 {
            self.lines[at].push(':');
        } else if name.is_empty() && !has_ref && value.is_empty() && !INTERACTIVE.contains(&role.as_str()) {
            self.lines.truncate(at); // all its children were redundant
        }
    }

    /// Walk sibling nodes, dropping label text that sits next to the control
    /// it names (`<label>Email <input>`): it is already the control's name.
    fn walk_kids(&mut self, kids: Vec<usize>, depth: usize, cover: &str) {
        let bare = |t: &str| t.trim_end_matches([':', '*', ' ']).to_string();
        let non_text: Vec<usize> =
            kids.iter().copied().filter(|k| Self::role(&self.nodes[*k]) != "StaticText").collect();
        let sibling_names: Vec<String> = non_text
            .iter()
            .map(|k| {
                let mut options = Vec::new();
                if matches!(Self::role(&self.nodes[*k]), "combobox" | "listbox") {
                    collect_options(self, *k, &mut options);
                }
                bare(&without_options(text_of(&self.nodes[*k], "name"), &options))
            })
            .filter(|n| !n.is_empty())
            .collect();
        let mut run = Vec::new();
        for child in kids {
            if self.stopped {
                break;
            }
            if Self::role(&self.nodes[child]) == "StaticText" {
                let t = bare(&text_of(&self.nodes[child], "name"));
                if !t.is_empty() && sibling_names.contains(&t) {
                    continue;
                }
                let name = self.name(child);
                if !name.trim().is_empty() && (cover.is_empty() || !cover.contains(&name)) {
                    run.push((name, backend_id(&self.nodes[child])));
                }
                continue;
            }
            self.text_lines(&mut run, depth);
            self.walk(child, depth, cover);
        }
        self.text_lines(&mut run, depth);
    }

    /// Adjacent text, packed into `- text:` lines of up to [`TEXT_MAX`]
    /// characters: a page of `<div>`s holding a word or two each took a
    /// line per `<div>`. A text longer than that has a line of its own,
    /// cut, with the ref to read all of it by.
    fn text_lines(&mut self, run: &mut Vec<(String, Option<u64>)>, depth: usize) {
        let indent = "  ".repeat(depth);
        let mut line = String::new();
        let mut len = 0;
        for (text, read_by) in std::mem::take(run) {
            if self.stopped {
                return;
            }
            let text = text.trim();
            let n = text.chars().count();
            if !line.is_empty() && (n > TEXT_MAX || len + 1 + n > TEXT_MAX) {
                self.push_line(format!("{indent}- text: {}", std::mem::take(&mut line)));
                len = 0;
                if self.stopped {
                    return;
                }
            }
            if n > TEXT_MAX {
                self.push_line(format!("{indent}- text: {}", self.clip(text, TEXT_MAX, read_by)));
                continue;
            }
            if !line.is_empty() {
                line.push(' ');
                len += 1;
            }
            line.push_str(text);
            len += n;
        }
        if !line.is_empty() && !self.stopped {
            self.push_line(format!("{indent}- text: {line}"));
        }
    }

    /// `role [tags] url=…` for a child inlined after its same-named parent.
    fn describe(&mut self, idx: usize, role: &str) -> String {
        let mut line = role.to_string();
        let mut tags = Self::states(&self.nodes[idx], role);
        if let Some(id) = backend_id(&self.nodes[idx]) {
            if self.wants_ref(idx, role) {
                tags.push(format!("ref={id}"));
            }
        }
        if !tags.is_empty() {
            line.push_str(&format!(" [{}]", tags.join(", ")));
        }
        if role == "link" {
            if let Some(Value::String(url)) = prop(&self.nodes[idx], "url") {
                if !url.is_empty() && !url.starts_with("javascript:") {
                    line.push_str(&format!(" url={}", self.short_url(url)));
                }
            }
        }
        line
    }
}

/// `name` without the option texts a `<select>`'s name can end with:
/// Lightpanda names a select inside a `<label>` with all of the label's
/// text, options included ("Currency USD EUR GBP …").
fn without_options(name: String, options: &[String]) -> String {
    let words: Vec<&str> = options.iter().map(|o| o.strip_suffix(" (selected)").unwrap_or(o).trim()).collect();
    let tail = words.join(" ");
    match name.strip_suffix(tail.as_str()) {
        Some(head) if !tail.is_empty() => head.trim_end().to_string(),
        _ => name,
    }
}

/// Roles whose state is checked or not, shown as `[checked]`.
const CHECKABLE: &[&str] = &["checkbox", "radio", "switch", "menuitemcheckbox", "menuitemradio"];

fn collect_options(tree: &mut Tree<'_>, idx: usize, out: &mut Vec<String>) {
    for child in tree.children(idx) {
        if Tree::role(&tree.nodes[child]) == "option" {
            let mut label = text_of(&tree.nodes[child], "name");
            if truthy(prop(&tree.nodes[child], "selected")) {
                label.push_str(" (selected)");
            }
            out.push(label);
        } else {
            collect_options(tree, child, out);
        }
    }
}

/// Render the main-frame `nodes` (plus child `frames`) as the text tree.
pub fn render(header: &str, nodes: &[Value], frames: &[Frame], opts: &Options) -> Rendered {
    let rendered = render_inner(header, nodes, frames, opts, None).0;
    // What the cuts saved is small: the whole text costs less than the
    // agent's next call to read it.
    if !opts.full_text && rendered.cuts > 0 && rendered.text.len() + rendered.cut_chars <= FULL_TEXT_BUDGET {
        return render_inner(header, nodes, frames, &Options { full_text: true, ..opts.clone() }, None).0;
    }
    rendered
}

/// A snapshot that would be at most this many bytes with every text in
/// full prints them in full (see [`render`]).
pub const FULL_TEXT_BUDGET: usize = 32_000;

/// Render from a lazily fetched seed: `seed` holds the root (or, for a
/// scope, the partial tree around it) and `fetch` supplies children on
/// demand, so the walk stops at the output limit instead of reading the
/// whole page. Returns whether any fetch failed: on failure the caller
/// falls back to [`render`] rather than print a partial tree. Pages under
/// the limit render byte-identical to [`render`]; over the limit, the
/// boundary line may keep a container line whose children fall past the
/// cut (the full walk would have decided it from those children).
pub fn render_lazy(header: &str, seed: &[Value], frames: &[Frame], opts: &Options, fetch: AxFetch) -> (Rendered, bool) {
    render_inner(header, seed, frames, opts, Some(fetch))
}

fn render_inner(
    header: &str,
    nodes: &[Value],
    frames: &[Frame],
    opts: &Options,
    fetch: Option<AxFetch>,
) -> (Rendered, bool) {
    let mut tree = Tree {
        nodes: Vec::new(),
        by_id: HashMap::new(),
        by_backend: HashMap::new(),
        frame_roots: HashMap::new(),
        frame_root_ids: std::collections::HashSet::new(),
        opts,
        lines: Vec::new(),
        fetch,
        fetched: std::collections::HashSet::new(),
        fetch_failed: false,
        calls: 0,
        used: std::collections::HashSet::new(),
        stopped: false,
        boundary: Vec::new(),
        cuts: std::cell::Cell::new(0),
        cut_chars: std::cell::Cell::new(0),
    };
    for n in nodes.iter().chain(frames.iter().flat_map(|f| f.nodes.iter())) {
        tree.insert(n.clone());
    }
    for f in frames {
        if let Some(root) = f.nodes.iter().find(|n| n.get("parentId").is_none()) {
            if let Some(&root_idx) = root.get("nodeId").and_then(Value::as_str).and_then(|id| tree.by_id.get(id)) {
                tree.frame_roots.entry(f.owner_backend_id).or_default().push(root_idx);
                tree.frame_root_ids.insert(root_idx);
            }
        }
    }
    if !header.is_empty() {
        tree.lines.push(header.to_string());
    }
    let start: Option<usize> = match opts.root_backend_id {
        Some(backend) => tree.by_backend.get(&backend).copied(),
        None => tree.nodes.iter().position(|n| n.get("parentId").is_none()),
    };
    if let Some(root) = start {
        if tree.is_transparent(root) {
            let mut kids = Vec::new();
            tree.effective_children(root, &mut kids);
            tree.walk_kids(kids, 0, "");
        } else {
            tree.walk(root, 0, "");
        }
    }
    // The limit cut inside a node's children: its line stays, and it gets
    // the trailing ':' if it has children to print (in the full walk its
    // children decide that; here only the immediate children are known,
    // which is the same answer whenever any of them prints).
    let boundary = std::mem::take(&mut tree.boundary);
    for (at, idx) in boundary {
        let mut kids = Vec::new();
        tree.effective_children(idx, &mut kids);
        if !kids.is_empty() && !tree.lines[at].ends_with(':') {
            tree.lines[at].push(':');
        }
    }
    let fetch_failed = tree.fetch_failed;
    let lazy = tree.fetch.is_some();
    let tree_stopped = tree.stopped;
    let (cuts, cut_chars) = (tree.cuts.get(), tree.cut_chars.get());
    let total = tree.lines.len();
    let mut lines = tree.lines;
    let mut truncated = 0;
    if opts.limit > 0 && total > opts.limit {
        truncated = total - opts.limit;
        lines.truncate(opts.limit);
        // The walk stops a line past the limit (lazy or not), so how many
        // lines the whole tree has is not known: no count, rather than a
        // wrong one ("1 more lines" on a 3,000-line page).
        let more = if lazy || tree_stopped { "more lines".to_string() } else { format!("{truncated} more lines") };
        lines.push(format!("… {more} (scope with `ax --selector <css>`, or raise `--limit`)"));
    }
    // Cut text said once, at the top, with how to read it all: an agent
    // that reads on without following the cut marks misses what they hide.
    if cuts > 0 {
        let at = usize::from(!header.is_empty()).min(lines.len());
        lines.insert(at, cut_summary(cuts, cut_chars));
    }
    (Rendered { lines: lines.len(), text: lines.join("\n"), truncated, cuts, cut_chars }, fetch_failed)
}

/// The line that says a snapshot cut text, and how to read it in full.
fn cut_summary(cuts: usize, chars: usize) -> String {
    let (what, them) = if cuts == 1 { ("text", "it") } else { ("texts", "one") };
    format!(
        "… {} {what} cut short here ({} chars left out): `get text <ref>` reads {them} in full, \
         `text` the whole page (`--selector <css>` a part), `ax --full-text` the tree uncut",
        thousands(cuts),
        thousands(chars)
    )
}

/// `75987` -> `75,987`.
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Lines for what [`crate::cdp::page`]'s `SHADOW_AX` found inside shadow
/// roots, appended to a Lightpanda snapshot: one heading per root, then its
/// elements as the tree would show them, but with no refs -- the engine gives
/// those nodes no id -- and the targeting that does reach them said once.
pub fn render_shadow(found: &[Value]) -> String {
    let mut out = String::new();
    let mut host = 0;
    for e in found {
        let h = e.get("host").and_then(Value::as_u64).unwrap_or(0);
        let role = e.get("role").and_then(Value::as_str).unwrap_or("generic");
        let name = e.get("name").and_then(Value::as_str).unwrap_or("");
        if h != host {
            host = h;
            let tag = e.get("tag").and_then(Value::as_str).unwrap_or("element");
            out.push_str(&format!(
                "- shadow root of <{tag}> (Lightpanda leaves it out of the tree and gives it no refs; \
                 target it with --text \"<name>\"):\n"
            ));
        }
        let mut line = format!("  - {role}");
        if !name.is_empty() {
            line.push_str(&format!(" {}", serde_json::to_string(name).unwrap_or_default()));
        }
        if let Some(level) = e.get("level").and_then(Value::as_u64) {
            line.push_str(&format!(" [level={level}]"));
        }
        if let Some(v) = e.get("value").and_then(Value::as_str) {
            line.push_str(&format!(" value={}", serde_json::to_string(v).unwrap_or_default()));
        }
        if let Some(c) = e.get("checked").and_then(Value::as_bool) {
            line.push_str(if c { " [checked]" } else { " [unchecked]" });
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(id: &str, parent: Option<&str>, role: &str, name: &str, kids: &[&str], backend: u64) -> Value {
        let mut n = json!({
            "nodeId": id,
            "ignored": false,
            "role": {"type": "role", "value": role},
            "name": {"type": "computedString", "value": name},
            "childIds": kids,
            "backendDOMNodeId": backend,
        });
        if let Some(p) = parent {
            n["parentId"] = json!(p);
        }
        n
    }

    #[test]
    fn renders_nested_tree_with_refs_on_actionable_nodes_only() {
        let nodes = vec![
            node("1", None, "RootWebArea", "Shop", &["2"], 1),
            node("2", Some("1"), "generic", "", &["3", "6"], 2),
            node("3", Some("2"), "heading", "Widget", &["4"], 3),
            node("4", Some("3"), "StaticText", "Widget", &[], 4),
            node("6", Some("2"), "button", "Add to cart", &["7"], 6),
            node("7", Some("6"), "StaticText", "Add to cart", &[], 7),
        ];
        let out = render("page \"Shop\"", &nodes, &[], &Options::default());
        assert_eq!(out.text, "page \"Shop\"\n- heading \"Widget\"\n- button \"Add to cart\" [ref=6]");
    }

    #[test]
    fn what_the_page_made_clickable_gets_a_ref_and_a_container_does_not() {
        let nodes = vec![
            node("1", None, "RootWebArea", "Inbox", &["2"], 1),
            // #app listens for its children's clicks: not a control itself.
            node("2", Some("1"), "generic", "", &["3", "6", "7"], 2),
            node("3", Some("2"), "generic", "", &["4", "5"], 3),
            node("4", Some("3"), "StaticText", "Gilli", &[], 4),
            node("5", Some("3"), "StaticText", "Quam.", &[], 5),
            node("6", Some("2"), "generic", "", &[], 6),
            node("7", Some("2"), "button", "", &[], 7),
        ];
        let clicks = Clickables {
            nodes: [2, 3, 6].into_iter().collect(),
            hints: [(2, "#app".to_string()), (6, "#send-forward".to_string()), (7, "#button-1".to_string())]
                .into_iter()
                .collect(),
            ..Clickables::default()
        };
        let out = render("page \"Inbox\"", &nodes, &[], &Options { clicks, ..Options::default() });
        assert_eq!(
            out.text,
            "page \"Inbox\"\n- clickable [ref=3]: Gilli Quam.\n- clickable #send-forward [ref=6]\n- button #button-1 [ref=7]"
        );
        // Without them, the row is bare text and the icon is not there.
        let out = render("", &nodes, &[], &Options::default());
        assert_eq!(out.text, "- text: Gilli Quam.\n- button [ref=7]");
    }

    /// Text in sibling `<div>`s shares lines of up to 300 characters; a
    /// control between runs ends one.
    #[test]
    fn adjacent_text_shares_lines() {
        let mut nodes = vec![node("1", None, "RootWebArea", "", &[], 1)];
        let mut kids = Vec::new();
        for i in 0..100 {
            let (div, text) = (format!("d{i}"), format!("t{i}"));
            nodes.push(node(&div, Some("1"), "generic", "", &[&text], 10 + 2 * i));
            nodes.push(node(&text, Some(&div), "StaticText", "cell", &[], 11 + 2 * i));
            kids.push(div);
            if i == 49 {
                nodes.push(node("b", Some("1"), "button", "Next", &[], 500));
                kids.push("b".into());
            }
        }
        let ids: Vec<&str> = kids.iter().map(String::as_str).collect();
        nodes[0] = node("1", None, "RootWebArea", "", &ids, 1);
        let out = render("", &nodes, &[], &Options::default());
        let lines: Vec<&str> = out.text.lines().collect();
        // 50 "cell"s are 249 characters: one line each side of the button.
        assert_eq!(lines.len(), 3, "{}", out.text);
        assert_eq!(lines[0], format!("- text: {}", vec!["cell"; 50].join(" ")));
        assert_eq!(lines[1], "- button \"Next\" [ref=500]");
        // Past 300 characters a run takes another line, never cutting a text.
        let long: Vec<Value> = (0..80)
            .flat_map(|i| {
                let (div, text) = (format!("d{i}"), format!("t{i}"));
                [
                    node(&div, Some("1"), "generic", "", &[&text], 10 + 2 * i),
                    node(&text, Some(&div), "StaticText", "words", &[], 11 + 2 * i),
                ]
            })
            .collect();
        let ids: Vec<String> = (0..80).map(|i| format!("d{i}")).collect();
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        let mut all = vec![node("1", None, "RootWebArea", "", &ids, 1)];
        all.extend(long);
        let out = render("", &all, &[], &Options::default());
        let lines: Vec<&str> = out.text.lines().collect();
        assert_eq!(lines.len(), 2, "{}", out.text);
        assert!(lines.iter().all(|l| l.len() <= "- text: ".len() + TEXT_MAX && !l.contains('…')), "{}", out.text);
        assert_eq!(out.text.matches("words").count(), 80);
    }

    /// A cut says how much it left out and how to read the rest; a name's
    /// cut is said outside its quotes.
    #[test]
    fn a_cut_says_what_it_left_out() {
        // Over the full-text budget: the texts are cut, each says what it
        // left out, and a line at the top says it once for the snapshot.
        let long = "x".repeat(20_000);
        let name = "n".repeat(200);
        let nodes = vec![
            node("1", None, "RootWebArea", "", &["2", "3", "5"], 1),
            node("2", Some("1"), "StaticText", &long, &[], 2),
            node("3", Some("1"), "paragraph", "", &["4"], 3),
            node("4", Some("3"), "StaticText", &long, &[], 4),
            node("5", Some("1"), "link", &name, &[], 5),
        ];
        let out = render("page \"Terms\" http://x.test/", &nodes, &[], &Options::default());
        let lines: Vec<&str> = out.text.lines().collect();
        assert_eq!(lines[0], "page \"Terms\" http://x.test/");
        assert_eq!(
            lines[1],
            "… 3 texts cut short here (39,450 chars left out): `get text <ref>` reads one in full, \
             `text` the whole page (`--selector <css>` a part), `ax --full-text` the tree uncut"
        );
        assert_eq!(lines[2], format!("- text: {}… (+19700 chars: get text 2)", "x".repeat(300)));
        assert_eq!(lines[3], format!("- paragraph: {}… (+19700 chars: get text 3)", "x".repeat(300)));
        assert_eq!(lines[4], format!("- link \"{}\"… (+50 chars) [ref=5]", "n".repeat(150)));
        assert_eq!((out.cuts, out.cut_chars), (3, 39_450));

        // --full-text: nothing cut, no summary line.
        let full = render("", &nodes, &[], &Options { full_text: true, ..Options::default() });
        assert_eq!(full.cuts, 0);
        assert!(full.text.starts_with(&format!("- text: {long}\n")), "{}", &full.text[..80]);
        assert!(full.text.contains(&format!("- link \"{name}\" [ref=5]")));
    }

    /// A page whose texts in full fit the budget prints them in full: the
    /// cuts would save less than the agent's next call costs.
    #[test]
    fn texts_in_full_when_they_fit_the_budget() {
        let long = "y".repeat(1000);
        let nodes = vec![
            node("1", None, "RootWebArea", "", &["2"], 1),
            node("2", Some("1"), "paragraph", "", &["3"], 2),
            node("3", Some("2"), "StaticText", &long, &[], 3),
        ];
        let out = render("", &nodes, &[], &Options::default());
        assert_eq!(out.text, format!("- paragraph: {long}"));
        assert_eq!(out.cuts, 0);
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn inlines_text_only_children_and_limits_output() {
        let nodes = vec![
            node("1", None, "RootWebArea", "", &["2", "4"], 1),
            node("2", Some("1"), "paragraph", "", &["3"], 2),
            node("3", Some("2"), "StaticText", "Returns accepted for 30 days.", &[], 3),
            node("4", Some("1"), "paragraph", "", &["5"], 4),
            node("5", Some("4"), "StaticText", "Second", &[], 5),
        ];
        let out = render("", &nodes, &[], &Options::default());
        assert_eq!(out.text, "- paragraph: Returns accepted for 30 days.\n- paragraph: Second");
        let out = render("", &nodes, &[], &Options { limit: 1, ..Options::default() });
        assert_eq!(out.truncated, 1);
        assert!(out.text.starts_with("- paragraph: Returns"));
        // The walk stops a line past the limit, so the count would be wrong
        // on a longer tree: none is given.
        assert!(
            out.text.ends_with("… more lines (scope with `ax --selector <css>`, or raise `--limit`)"),
            "{}",
            out.text
        );
    }

    #[test]
    fn splices_iframe_documents_and_lists_select_options() {
        let mut combo = node("4", Some("1"), "combobox", "Color", &["5"], 4);
        combo["value"] = json!({"type": "string", "value": "Blue"});
        let mut blue = node("7", Some("5"), "option", "Blue", &[], 7);
        blue["properties"] = json!([{"name": "selected", "value": {"type": "boolean", "value": true}}]);
        let nodes = vec![
            node("1", None, "RootWebArea", "Outer", &["2", "4"], 1),
            node("2", Some("1"), "Iframe", "Pay", &[], 14),
            combo,
            node("5", Some("4"), "MenuListPopup", "", &["6", "7"], 5),
            node("6", Some("5"), "option", "Red", &[], 6),
            blue,
        ];
        let mut inner = node("70", None, "RootWebArea", "Card form", &["71"], 70);
        inner["frameId"] = json!("F2");
        let frames = vec![Frame {
            owner_backend_id: 14,
            nodes: vec![inner, node("71", Some("70"), "button", "Pay now", &[], 71)],
        }];
        let out = render("", &nodes, &frames, &Options::default());
        assert_eq!(
            out.text,
            "- Iframe \"Pay\":\n  - document \"Card form\":\n    - button \"Pay now\" [ref=71]\n\
             - combobox \"Color\" [ref=4] value=\"Blue\" options: \"Red\", \"Blue (selected)\""
        );
    }

    /// What Lightpanda's tree says about a form, rendered as Chrome's reads:
    /// a select named with its label's whole text (options included) and
    /// the label text beside it, a checkbox with the default value "on",
    /// and an unnamed table cell around one checkbox and its label text.
    #[test]
    fn lightpanda_form_quirks_render_as_chrome_reads() {
        let mut combo = node("4", Some("2"), "combobox", "Currency USD EUR", &["5"], 4);
        combo["value"] = json!({"type": "string", "value": "USD"});
        let mut usd = node("6", Some("5"), "option", "USD", &[], 6);
        usd["properties"] = json!([{"name": "selected", "value": {"type": "boolean", "value": true}}]);
        let mut cb = node("9", Some("8"), "checkbox", "Email alerts for Drive", &[], 9);
        cb["value"] = json!({"type": "string", "value": "on"});
        let nodes = vec![
            node("1", None, "RootWebArea", "Prefs", &["2", "8"], 1),
            node("2", Some("1"), "group", "", &["3", "4"], 2),
            node("3", Some("2"), "StaticText", "Currency", &[], 3),
            combo,
            node("5", Some("4"), "MenuListPopup", "", &["6", "7"], 5),
            usd,
            node("7", Some("5"), "option", "EUR", &[], 7),
            node("8", Some("1"), "cell", "", &["9", "10"], 8),
            cb,
            node("10", Some("8"), "StaticText", "Email alerts for Drive", &[], 10),
        ];
        let out = render("", &nodes, &[], &Options::default());
        assert_eq!(
            out.text,
            "- group:\n  - combobox \"Currency\" [ref=4] value=\"USD\" options: \"USD (selected)\", \"EUR\"\n\
             - cell: checkbox \"Email alerts for Drive\" [ref=9]"
        );
    }

    #[test]
    fn shadow_content_renders_under_its_root_without_refs() {
        let found = vec![
            json!({"host": 1, "tag": "acme-coupon", "role": "textbox", "name": "Coupon code", "value": "SAVE10"}),
            json!({"host": 1, "tag": "acme-coupon", "role": "button", "name": "Apply coupon"}),
            json!({"host": 2, "tag": "x-prefs", "role": "checkbox", "name": "Email me", "checked": false}),
            json!({"host": 2, "tag": "x-prefs", "role": "heading", "name": "Prefs", "level": 3}),
        ];
        let text = render_shadow(&found);
        assert_eq!(text.matches("- shadow root of <").count(), 2, "{text}");
        assert!(text.contains("<acme-coupon>"), "{text}");
        assert!(text.contains("  - textbox \"Coupon code\" value=\"SAVE10\"\n"), "{text}");
        assert!(text.contains("  - checkbox \"Email me\" [unchecked]\n"), "{text}");
        assert!(text.contains("  - heading \"Prefs\" [level=3]\n"), "{text}");
        assert!(!text.contains("[ref="), "{text}");
        assert_eq!(render_shadow(&[]), "");
    }

    /// A lazy fetch over a map from parent id to child objects.
    fn lazy_fetch(all: Vec<Value>) -> (AxFetch, std::rc::Rc<std::cell::Cell<usize>>) {
        let by_parent: HashMap<String, Vec<Value>> = {
            let mut m: HashMap<String, Vec<Value>> = HashMap::new();
            for n in &all {
                if let Some(p) = n.get("parentId").and_then(Value::as_str) {
                    m.entry(p.to_string()).or_default().push(n.clone());
                }
            }
            m
        };
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let calls_in = calls.clone();
        let fetch: AxFetch = Box::new(move |parents: &[String]| {
            calls_in.set(calls_in.get() + 1);
            let mut out = Vec::new();
            for p in parents {
                if let Some(kids) = by_parent.get(p) {
                    out.extend(kids.iter().cloned());
                }
            }
            (out, false)
        });
        (fetch, calls)
    }

    #[test]
    fn lazy_render_matches_full_render_under_the_limit() {
        let nodes = vec![
            node("1", None, "RootWebArea", "Shop", &["2"], 1),
            node("2", Some("1"), "generic", "", &["3", "6"], 2),
            node("3", Some("2"), "heading", "Widget", &["4"], 3),
            node("4", Some("3"), "StaticText", "Widget", &[], 4),
            node("6", Some("2"), "button", "Add to cart", &["7"], 6),
            node("7", Some("6"), "StaticText", "Add to cart", &[], 7),
        ];
        let full = render("page \"Shop\"", &nodes, &[], &Options::default());
        let (fetch, _) = lazy_fetch(nodes.clone());
        let (lazy, failed) = render_lazy("page \"Shop\"", &nodes[..1], &[], &Options::default(), fetch);
        assert!(!failed);
        assert_eq!(lazy.text, full.text, "lazy must match full under the limit");
    }

    #[test]
    fn lazy_render_stops_fetching_past_the_limit() {
        let item_ids: Vec<String> = (0..300).map(|i| format!("item{i}")).collect();
        let item_refs: Vec<&str> = item_ids.iter().map(String::as_str).collect();
        let mut nodes =
            vec![node("1", None, "RootWebArea", "Big", &["2"], 1), node("2", Some("1"), "list", "", &item_refs, 2)];
        for i in 0..300 {
            nodes.push(node(
                Box::leak(format!("item{i}").into_boxed_str()),
                Some("2"),
                "listitem",
                "",
                &[Box::leak(format!("text{i}").into_boxed_str())],
                100 + i as u64,
            ));
            nodes.push(node(
                Box::leak(format!("text{i}").into_boxed_str()),
                Some(Box::leak(format!("item{i}").into_boxed_str())),
                "StaticText",
                &format!("Item {i}"),
                &[],
                1000 + i as u64,
            ));
        }
        let opts = Options { limit: 5, ..Options::default() };
        let full = render("", &nodes, &[], &opts);
        let (fetch, calls) = lazy_fetch(nodes.clone());
        let (lazy, failed) = render_lazy("", &nodes[..1], &[], &opts, fetch);
        assert!(!failed);
        // Same kept lines as the full walk; only the truncation count is
        // unknowable without reading the whole page.
        let full_kept: Vec<&str> = full.text.lines().take(5).collect();
        let lazy_kept: Vec<&str> = lazy.text.lines().take(5).collect();
        assert_eq!(lazy_kept, full_kept);
        assert!(lazy.text.contains("more lines"));
        // One call for the list's items, then windows of item children:
        // far fewer than one call per item.
        assert!(calls.get() < 20, "fetch batches: {} calls", calls.get());
    }

    #[test]
    fn lazy_render_reports_a_failed_fetch() {
        let nodes = [node("1", None, "RootWebArea", "Shop", &["2"], 1), node("2", Some("1"), "button", "Buy", &[], 2)];
        let fetch: AxFetch = Box::new(|_| (Vec::new(), true));
        let (_, failed) = render_lazy("", &nodes[..1], &[], &Options::default(), fetch);
        assert!(failed);
    }
}
