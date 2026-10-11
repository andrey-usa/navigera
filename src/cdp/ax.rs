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
}

/// One frame's AX nodes, spliced under the AX node of its owner element
/// (`<iframe>` backend node id).
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

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

fn quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
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

struct Tree<'a> {
    by_id: HashMap<&'a str, &'a Value>,
    frame_root_ids: std::collections::HashSet<&'a str>,
    frames: HashMap<u64, Vec<&'a Value>>,
    opts: &'a Options,
    lines: Vec<String>,
}

impl<'a> Tree<'a> {
    fn children(&self, node: &'a Value) -> Vec<&'a Value> {
        let mut out: Vec<&'a Value> = node
            .get("childIds")
            .and_then(Value::as_array)
            .map(|ids| ids.iter().filter_map(|id| id.as_str().and_then(|id| self.by_id.get(id).copied())).collect())
            .unwrap_or_default();
        // An <iframe>'s document lives in another frame's tree.
        if let Some(backend) = node.get("backendDOMNodeId").and_then(Value::as_u64) {
            if let Some(roots) = self.frames.get(&backend) {
                out.extend(roots.iter().copied());
            }
        }
        out
    }

    fn role(node: &Value) -> &str {
        field(node, "role").as_str().unwrap_or("")
    }

    fn is_transparent(&self, node: &Value) -> bool {
        if node.get("ignored").and_then(Value::as_bool).unwrap_or(false) {
            return true;
        }
        let role = Self::role(node);
        if role == "RootWebArea" && !self.is_frame_root(node) {
            return true;
        }
        WRAPPERS.contains(&role) && text_of(node, "name").is_empty() && !truthy(prop(node, "focusable"))
    }

    fn is_frame_root(&self, node: &Value) -> bool {
        // Child-frame documents are rendered as `- document "Title":`.
        node.get("nodeId").and_then(Value::as_str).is_some_and(|id| self.frame_root_ids.contains(id))
    }

    /// Children with transparent wrappers expanded in place.
    fn effective_children(&self, node: &'a Value, out: &mut Vec<&'a Value>) {
        for child in self.children(node) {
            if SILENT.contains(&Self::role(child)) {
                continue;
            }
            if self.is_transparent(child) {
                self.effective_children(child, out);
            } else {
                out.push(child);
            }
        }
    }

    fn wants_ref(&self, node: &Value, role: &str) -> bool {
        self.opts.all_refs || INTERACTIVE.contains(&role) || truthy(prop(node, "focusable"))
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

    fn walk(&mut self, node: &'a Value, depth: usize, covered: &str) {
        let role = Self::role(node).to_string();
        if SILENT.contains(&role.as_str()) {
            return;
        }
        if self.is_transparent(node) {
            for child in self.children(node) {
                self.walk(child, depth, covered);
            }
            return;
        }
        let indent = "  ".repeat(depth);
        let name = text_of(node, "name");

        if role == "StaticText" {
            if name.is_empty() || (!covered.is_empty() && covered.contains(&name)) {
                return;
            }
            self.lines.push(format!("{indent}- text: {}", clip(&name, 300)));
            return;
        }

        let shown_role = if self.is_frame_root(node) { "document" } else { role.as_str() };
        let mut line = format!("{indent}- {shown_role}");
        if !name.is_empty() {
            line.push(' ');
            line.push_str(&quote(&clip(&name, 150)));
        }
        let mut tags = Self::states(node, &role);
        if let Some(id) = node.get("backendDOMNodeId").and_then(Value::as_u64) {
            if self.wants_ref(node, &role) {
                tags.push(format!("ref={id}"));
            }
        }
        if !tags.is_empty() {
            line.push_str(&format!(" [{}]", tags.join(", ")));
        }
        let value = text_of(node, "value");
        if !value.is_empty() && value != name {
            line.push_str(&format!(" value={}", quote(&clip(&value, 150))));
        }
        if role == "link" {
            if let Some(Value::String(url)) = prop(node, "url") {
                if !url.is_empty() && !url.starts_with("javascript:") {
                    line.push_str(&format!(" url={}", self.short_url(url)));
                }
            }
        }

        let mut kids = Vec::new();
        self.effective_children(node, &mut kids);

        // <select>: list the options on the combobox line, not as a subtree.
        if role == "combobox" || role == "listbox" {
            let mut options = Vec::new();
            collect_options(self, node, &mut options);
            if !options.is_empty() {
                let shown: Vec<String> = options.iter().take(25).map(|o| quote(&clip(o, 60))).collect();
                line.push_str(&format!(" options: {}", shown.join(", ")));
                if options.len() > 25 {
                    line.push_str(&format!(", …(+{})", options.len() - 25));
                }
                self.lines.push(line);
                return;
            }
        }

        // Unnamed node holding only text: inline it (`- paragraph: …`).
        let child_cover = if name.is_empty() { covered.to_string() } else { name.clone() };
        if !kids.is_empty() && kids.iter().all(|k| Self::role(k) == "StaticText") {
            let text: Vec<String> = kids
                .iter()
                .map(|k| text_of(k, "name"))
                .filter(|t| !t.is_empty() && !child_cover.contains(t.as_str()) && *t != value)
                .collect();
            if !text.is_empty() {
                line.push_str(&format!(": {}", clip(&text.join(" "), 300)));
            }
            self.lines.push(line);
            return;
        }
        // A named wrapper around one same-named actionable child (a heading
        // holding its link): one line, name printed once.
        if kids.len() == 1 && !name.is_empty() && !INTERACTIVE.contains(&role.as_str()) {
            let only = kids[0];
            let only_role = Self::role(only).to_string();
            if text_of(only, "name") == name && INTERACTIVE.contains(&only_role.as_str()) {
                let mut grand = Vec::new();
                self.effective_children(only, &mut grand);
                let leafish = grand.iter().all(|k| Self::role(k) == "StaticText");
                if leafish {
                    let inner = self.describe(only, &only_role);
                    line.push_str(&format!(": {inner}"));
                    self.lines.push(line);
                    return;
                }
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
        self.lines.push(line);
        self.walk_kids(kids, depth + 1, &child_cover);
        if self.lines.len() > at + 1 {
            self.lines[at].push(':');
        } else if name.is_empty() && !has_ref && value.is_empty() && !INTERACTIVE.contains(&role.as_str()) {
            self.lines.truncate(at); // all its children were redundant
        }
    }

    /// Walk sibling nodes, dropping label text that sits next to the control
    /// it names (`<label>Email <input>`): it is already the control's name.
    fn walk_kids(&mut self, kids: Vec<&'a Value>, depth: usize, cover: &str) {
        let bare = |t: &str| t.trim_end_matches([':', '*', ' ']).to_string();
        let sibling_names: Vec<String> = kids
            .iter()
            .filter(|k| Self::role(k) != "StaticText")
            .map(|k| bare(&text_of(k, "name")))
            .filter(|n| !n.is_empty())
            .collect();
        for child in kids {
            if Self::role(child) == "StaticText" {
                let t = bare(&text_of(child, "name"));
                if !t.is_empty() && sibling_names.contains(&t) {
                    continue;
                }
            }
            self.walk(child, depth, cover);
        }
    }

    /// `role [tags] url=…` for a child inlined after its same-named parent.
    fn describe(&self, node: &Value, role: &str) -> String {
        let mut line = role.to_string();
        let mut tags = Self::states(node, role);
        if let Some(id) = node.get("backendDOMNodeId").and_then(Value::as_u64) {
            if self.wants_ref(node, role) {
                tags.push(format!("ref={id}"));
            }
        }
        if !tags.is_empty() {
            line.push_str(&format!(" [{}]", tags.join(", ")));
        }
        if role == "link" {
            if let Some(Value::String(url)) = prop(node, "url") {
                if !url.is_empty() && !url.starts_with("javascript:") {
                    line.push_str(&format!(" url={}", self.short_url(url)));
                }
            }
        }
        line
    }
}

fn collect_options(tree: &Tree<'_>, node: &Value, out: &mut Vec<String>) {
    for child in tree.children(node) {
        if Tree::role(child) == "option" {
            let mut label = text_of(child, "name");
            if truthy(prop(child, "selected")) {
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
    let mut by_id: HashMap<&str, &Value> = HashMap::new();
    for n in nodes.iter().chain(frames.iter().flat_map(|f| f.nodes.iter())) {
        if let Some(id) = n.get("nodeId").and_then(Value::as_str) {
            by_id.insert(id, n);
        }
    }
    let mut frame_roots: HashMap<u64, Vec<&Value>> = HashMap::new();
    let mut frame_root_ids = std::collections::HashSet::new();
    for f in frames {
        if let Some(root) = f.nodes.iter().find(|n| n.get("parentId").is_none()) {
            frame_roots.entry(f.owner_backend_id).or_default().push(root);
            if let Some(id) = root.get("nodeId").and_then(Value::as_str) {
                frame_root_ids.insert(id);
            }
        }
    }
    let mut tree = Tree { by_id, frame_root_ids, frames: frame_roots, opts, lines: Vec::new() };
    if !header.is_empty() {
        tree.lines.push(header.to_string());
    }
    let start: Option<&Value> = match opts.root_backend_id {
        Some(backend) => nodes
            .iter()
            .chain(frames.iter().flat_map(|f| f.nodes.iter()))
            .find(|n| n.get("backendDOMNodeId").and_then(Value::as_u64) == Some(backend)),
        None => nodes.iter().find(|n| n.get("parentId").is_none()),
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
    let total = tree.lines.len();
    let mut lines = tree.lines;
    let mut truncated = 0;
    if opts.limit > 0 && total > opts.limit {
        truncated = total - opts.limit;
        lines.truncate(opts.limit);
        lines.push(format!("… {truncated} more lines (scope with `ax --selector <css>`, or raise `--limit`)"));
    }
    Rendered { lines: lines.len(), text: lines.join("\n"), truncated }
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
        assert!(out.text.contains("1 more lines"));
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
}
