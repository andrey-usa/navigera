//! The accessibility tree built from a DOM snapshot, for large pages.
//!
//! Chrome's own tree (`Accessibility.getFullAXTree`) costs about 1 ms per
//! table row: 2 s for a 2,000-row table, most of a snapshot's time. The
//! `DOMSnapshot.captureSnapshot` that `ax` already takes for clickable
//! elements holds every node's tag, attributes, text, form state, layout
//! box and computed styles in about a tenth of that, so the tree is built
//! here instead: roles from the HTML-AAM mappings, names from the
//! accessible-name steps (labelledby, label, native sources, content,
//! title, placeholder), the states the renderer prints, and same-process
//! frames. The nodes have the shape of Chrome's, so [`super::ax::render`]
//! prints them the same way.
//!
//! What is not modelled: names from CSS generated content, Chrome's
//! layout-table heuristics, a `<dialog>`'s modality (`showModal` leaves no
//! trace in the DOM), and closed shadow roots (not in the snapshot). Small
//! pages keep Chrome's tree; this is for the ones where it is slow.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use super::ax::Frame;

/// The tree of a snapshot's main document, and of each same-process frame.
pub struct Built {
    pub nodes: Vec<Value>,
    pub frames: Vec<Frame>,
    pub title: String,
    pub url: String,
}

/// Index of `display` in the computed styles `ax` asks the snapshot for
/// (after cursor, pointer-events, opacity, visibility).
const STYLE_VISIBILITY: usize = 3;
const STYLE_DISPLAY: usize = 4;
const STYLE_CONTENT: usize = 5;

/// Roles named from their content (ARIA "name from content", as
/// Chrome applies it).
const FROM_CONTENT: &[&str] = &[
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
    "rowheader",
    "switch",
    "tab",
    "tooltip",
    "treeitem",
    "term",
    "doc-backlink",
    "doc-biblioref",
    "doc-glossref",
    "doc-noteref",
    "DisclosureTriangle",
    "LayoutTableCell",
];

/// Tags never in the tree.
const SKIP: &[&str] =
    &["HEAD", "SCRIPT", "STYLE", "TEMPLATE", "NOSCRIPT", "TITLE", "META", "LINK", "BASE", "BR", "WBR", "DATALIST"];

pub fn build(snap: &Value) -> Option<Built> {
    build_from(snap, None)
}

/// The tree of one element's subtree (`ax --selector`, a ref scope): the
/// element is the first node, frames inside it follow. Building only the
/// scope keeps a scoped snapshot of a large page from paying for the
/// whole page's tree. `None` when the element is not in the snapshot;
/// no nodes when it is hidden (it, or an ancestor, is not exposed).
pub fn build_scoped(snap: &Value, scope_backend: u64) -> Option<Built> {
    build_from(snap, Some(scope_backend))
}

fn build_from(snap: &Value, scope: Option<u64>) -> Option<Built> {
    let strings: Vec<&str> = snap.get("strings")?.as_array()?.iter().map(|v| v.as_str().unwrap_or("")).collect();
    let documents = snap.get("documents")?.as_array()?;
    let docs: Vec<Doc> = documents.iter().map(|d| Doc::new(d, &strings)).collect();
    let main = docs.first()?;
    let mut frames = Vec::new();
    let mut nodes = Vec::new();
    // Where the walk starts: each document's root, or the scope element in
    // whichever document holds it.
    let mut start: Option<(usize, usize)> = None;
    if let Some(backend) = scope {
        let found = docs
            .iter()
            .enumerate()
            .find_map(|(d, doc)| doc.backend.iter().position(|&b| b == backend as i64).map(|i| (d, i)));
        let (d, i) = found?;
        if !docs[d].exposed_with_ancestors(i) {
            return Some(Built { nodes, frames, title: main.title.to_string(), url: main.url.to_string() });
        }
        start = Some((d, i));
    }
    // Frames hang under their <iframe>; each is a document of its own.
    let mut queue = vec![(start.map_or(0, |(d, _)| d), None::<u64>)];
    let mut seen = HashSet::new();
    while let Some((d, owner)) = queue.pop() {
        if !seen.insert(d) || d >= docs.len() {
            continue;
        }
        let mut out = Vec::new();
        let mut owned = Vec::new();
        let from = match (owner, start) {
            (None, Some((_, i))) => Some(i),
            _ => None,
        };
        docs[d].emit(d, from, &mut out, &mut owned);
        for (iframe_backend, child_doc) in owned {
            queue.push((child_doc, Some(iframe_backend)));
        }
        match owner {
            None => nodes = out,
            Some(owner_backend_id) => frames.push(Frame { owner_backend_id, nodes: out }),
        }
    }
    Some(Built { nodes, frames, title: main.title.to_string(), url: main.url.to_string() })
}

struct Doc<'a> {
    s: &'a [&'a str],
    parent: Vec<i64>,
    ty: Vec<i64>,
    tag: Vec<&'a str>,
    value: Vec<i64>,
    backend: Vec<i64>,
    attrs: Vec<Vec<(&'a str, &'a str)>>,
    children: Vec<Vec<usize>>,
    /// Nodes inside a shadow tree.
    shadow: HashSet<usize>,
    input_value: HashMap<usize, &'a str>,
    text_value: HashMap<usize, &'a str>,
    checked: HashSet<usize>,
    selected: HashSet<usize>,
    content_doc: HashMap<usize, usize>,
    pseudo: HashSet<usize>,
    /// node -> (layout text, visibility, display)
    layout: HashMap<usize, (Option<&'a str>, &'a str, &'a str)>,
    /// Generated content's alternative text (`content: "→" / "next"`).
    content_alt: HashMap<usize, String>,
    /// Generated content made of counters (a footnote's "a", "b"): shown,
    /// but Chrome leaves it out of names.
    counters: HashSet<usize>,
    /// Elements with a click listener (`isClickable`).
    listening: HashSet<usize>,
    /// Has a laid-out node somewhere inside (a `display: contents` box).
    rendered_inside: Vec<bool>,
    by_id: HashMap<&'a str, usize>,
    /// control -> `<label for>` elements naming it.
    labels_for: HashMap<usize, Vec<usize>>,
    /// Tables Chrome reads as layout, not data (see `data_table`).
    layout_tables: HashSet<usize>,
    /// `<label>`s of a checkbox or radio: Chrome leaves them (and their
    /// text) out, the control carries the name.
    check_labels: HashSet<usize>,
    url: &'a str,
    base: &'a str,
    title: &'a str,
}

fn ints(v: &Value, key: &str) -> Vec<i64> {
    v.get(key).and_then(Value::as_array).into_iter().flatten().map(|x| x.as_i64().unwrap_or(-1)).collect()
}

impl<'a> Doc<'a> {
    fn new(doc: &'a Value, s: &'a [&'a str]) -> Doc<'a> {
        let at = |i: i64| -> &'a str {
            if i >= 0 {
                s.get(i as usize).copied().unwrap_or("")
            } else {
                ""
            }
        };
        let n = &doc["nodes"];
        let parent = ints(n, "parentIndex");
        let count = parent.len();
        let ty = ints(n, "nodeType");
        let tag: Vec<&str> = ints(n, "nodeName").into_iter().map(at).collect();
        let value = ints(n, "nodeValue");
        let backend = ints(n, "backendNodeId");
        let attrs: Vec<Vec<(&str, &str)>> = (0..count)
            .map(|i| {
                let raw = n.get("attributes").and_then(|a| a.get(i)).and_then(Value::as_array);
                raw.map(|r| {
                    r.chunks(2)
                        .map(|p| (at(p[0].as_i64().unwrap_or(-1)), at(p.get(1).and_then(Value::as_i64).unwrap_or(-1))))
                        .collect()
                })
                .unwrap_or_default()
            })
            .collect();
        let rare_str = |key: &str| -> HashMap<usize, &'a str> {
            let idx = ints(&n[key], "index");
            let val = ints(&n[key], "value");
            idx.into_iter().zip(val).filter(|(i, _)| *i >= 0).map(|(i, v)| (i as usize, at(v))).collect()
        };
        let rare_int = |key: &str| -> HashMap<usize, usize> {
            let idx = ints(&n[key], "index");
            let val = ints(&n[key], "value");
            idx.into_iter()
                .zip(val)
                .filter(|(i, v)| *i >= 0 && *v >= 0)
                .map(|(i, v)| (i as usize, v as usize))
                .collect()
        };
        let rare_bool = |key: &str| -> HashSet<usize> {
            ints(&n[key], "index").into_iter().filter(|i| *i >= 0).map(|i| i as usize).collect()
        };
        let mut children = vec![Vec::new(); count];
        for (i, &p) in parent.iter().enumerate() {
            if p >= 0 && (p as usize) < count {
                children[p as usize].push(i);
            }
        }
        let l = &doc["layout"];
        let styles = l.get("styles").and_then(Value::as_array);
        let ltext = ints(l, "text");
        let mut layout = HashMap::new();
        let mut content_alt = HashMap::new();
        let mut counters = HashSet::new();
        for (k, node) in ints(l, "nodeIndex").into_iter().enumerate() {
            if node < 0 {
                continue;
            }
            let content = at(styles
                .and_then(|s| s.get(k))
                .and_then(|s| s.get(STYLE_CONTENT))
                .and_then(Value::as_i64)
                .unwrap_or(-1));
            if let Some(alt) = content_alt_text(content) {
                content_alt.insert(node as usize, alt);
            }
            if content.contains("counter") {
                counters.insert(node as usize);
            }
            let style = |j: usize| {
                at(styles.and_then(|s| s.get(k)).and_then(|s| s.get(j)).and_then(Value::as_i64).unwrap_or(-1))
            };
            let text = ltext.get(k).copied().filter(|t| *t >= 0).map(at);
            layout.insert(node as usize, (text, style(STYLE_VISIBILITY), style(STYLE_DISPLAY)));
        }
        let mut rendered_inside = vec![false; count];
        for i in (0..count).rev() {
            if (layout.contains_key(&i) || rendered_inside[i]) && parent[i] >= 0 {
                rendered_inside[parent[i] as usize] = true;
            }
        }
        let mut by_id = HashMap::new();
        for (i, a) in attrs.iter().enumerate() {
            if let Some((_, id)) = a.iter().find(|(k, _)| *k == "id") {
                by_id.entry(*id).or_insert(i);
            }
        }
        let mut labels_for: HashMap<usize, Vec<usize>> = HashMap::new();
        for (i, a) in attrs.iter().enumerate() {
            if tag[i] == "LABEL" {
                if let Some(target) = a.iter().find(|(k, _)| *k == "for").and_then(|(_, f)| by_id.get(f)) {
                    labels_for.entry(*target).or_default().push(i);
                }
            }
        }
        let mut doc = Doc {
            s,
            ty,
            tag,
            value,
            backend,
            attrs,
            children,
            shadow: rare_str("shadowRootType").into_keys().collect(),
            input_value: rare_str("inputValue"),
            text_value: rare_str("textValue"),
            checked: rare_bool("inputChecked"),
            selected: rare_bool("optionSelected"),
            content_doc: rare_int("contentDocumentIndex"),
            pseudo: rare_str("pseudoType").into_keys().collect(),
            layout,
            content_alt,
            counters,
            listening: rare_bool("isClickable"),
            rendered_inside,
            by_id,
            labels_for,
            url: at(doc.get("documentURL").and_then(Value::as_i64).unwrap_or(-1)),
            base: at(doc.get("baseURL").and_then(Value::as_i64).unwrap_or(-1)),
            title: at(doc.get("title").and_then(Value::as_i64).unwrap_or(-1)),
            parent,
            layout_tables: HashSet::new(),
            check_labels: HashSet::new(),
        };
        for i in 0..count {
            match doc.tag[i] {
                "TABLE" if !doc.data_table(i) => {
                    doc.layout_tables.insert(i);
                }
                "LABEL" if doc.labels_check(i) => {
                    doc.check_labels.insert(i);
                }
                _ => {}
            }
        }
        doc
    }

    /// Chrome's data-table test, without its border and colour checks:
    /// a role, a caption, header cells or sections, header attributes, or
    /// 20 rows make a data table; one row or one column, a layout table.
    fn data_table(&self, t: usize) -> bool {
        if let Some(role) = self.attr(t, "role").map(|r| r.trim().to_ascii_lowercase()) {
            return matches!(role.as_str(), "table" | "grid" | "treegrid");
        }
        if self.has(t, "summary") {
            return true;
        }
        let (mut rows, mut max_cols) = (0usize, 0usize);
        let mut stack: Vec<usize> = self.children[t].clone();
        while let Some(k) = stack.pop() {
            match self.tag[k] {
                "CAPTION" | "THEAD" | "TFOOT" | "COLGROUP" | "COL" | "TH" => return true,
                "TD" if ["headers", "abbr", "axis", "scope"].iter().any(|a| self.has(k, a)) => return true,
                "TR" => {
                    rows += 1;
                    let cols = self.children[k].iter().filter(|&&c| matches!(self.tag[c], "TD" | "TH")).count();
                    max_cols = max_cols.max(cols);
                    stack.extend(self.children[k].iter().copied());
                }
                "TBODY" => stack.extend(self.children[k].iter().copied()),
                // Cells' own content, but not a table inside one.
                "TD" => {}
                _ => {}
            }
        }
        rows >= 20 && max_cols > 1
    }

    /// A checkbox's or radio's `<label>` (around it or `for` it): Chrome
    /// makes the label's own text presentational, the control carries it
    /// as its name. Text in an element inside the label stays.
    fn labels_check(&self, l: usize) -> bool {
        let checkable =
            |c: usize| self.tag[c] == "INPUT" && matches!(self.input_type(c).as_str(), "checkbox" | "radio");
        if let Some(target) = self.attr(l, "for").and_then(|f| self.by_id.get(f)) {
            return checkable(*target);
        }
        let mut stack = self.children[l].clone();
        while let Some(k) = stack.pop() {
            if matches!(self.tag[k], "INPUT" | "SELECT" | "TEXTAREA" | "BUTTON") {
                return checkable(k);
            }
            stack.extend(self.children[k].iter().copied());
        }
        false
    }

    /// A text node right inside a checkbox's or radio's label.
    fn in_check_label(&self, i: usize) -> bool {
        self.parent[i] >= 0 && self.check_labels.contains(&(self.parent[i] as usize))
    }

    fn table_of(&self, i: usize) -> Option<usize> {
        self.ancestor_tag(i, &["TABLE"], 4)
    }

    fn attr(&self, i: usize, name: &str) -> Option<&'a str> {
        self.attrs[i].iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| *v)
    }

    fn has(&self, i: usize, name: &str) -> bool {
        self.attr(i, name).is_some()
    }

    fn text_of_node(&self, i: usize) -> &'a str {
        match self.layout.get(&i) {
            Some((Some(t), _, _)) => t,
            _ => {
                let v = self.value[i];
                if v >= 0 {
                    self.s.get(v as usize).copied().unwrap_or("")
                } else {
                    ""
                }
            }
        }
    }

    fn input_type(&self, i: usize) -> String {
        self.attr(i, "type").unwrap_or("text").trim().to_ascii_lowercase()
    }

    /// The tree's children of `i`: a shadow host's shadow tree, with each
    /// `<slot>` replaced by the host's children assigned to it.
    fn kids(&self, i: usize) -> Vec<usize> {
        let mut kids = self.kids_unordered(i);
        // The snapshot lists an element's pseudo-elements first; `::after`
        // comes after its content.
        if kids.iter().any(|&k| self.tag[k] == "::after") {
            let (after, rest): (Vec<usize>, Vec<usize>) = kids.into_iter().partition(|&k| self.tag[k] == "::after");
            kids = rest;
            kids.extend(after);
        }
        kids
    }

    fn kids_unordered(&self, i: usize) -> Vec<usize> {
        let all = &self.children[i];
        if self.tag[i] == "SLOT" && self.shadow.contains(&i) {
            if let Some(host) = self.host_of(i) {
                let name = self.attr(i, "name").unwrap_or("");
                let assigned: Vec<usize> = self.children[host]
                    .iter()
                    .copied()
                    .filter(|&k| !self.shadow.contains(&k))
                    .filter(|&k| match self.ty[k] {
                        1 => self.attr(k, "slot").unwrap_or("") == name,
                        3 => name.is_empty(),
                        _ => false,
                    })
                    .collect();
                if !assigned.is_empty() {
                    return assigned;
                }
            }
            return all.clone();
        }
        if !self.shadow.contains(&i) && all.iter().any(|k| self.shadow.contains(k)) {
            return all.iter().copied().filter(|k| self.shadow.contains(k)).collect();
        }
        all.clone()
    }

    /// The shadow host of a node in a (top-level) shadow tree.
    fn host_of(&self, mut i: usize) -> Option<usize> {
        for _ in 0..256 {
            let p = self.parent[i];
            if p < 0 {
                return None;
            }
            i = p as usize;
            if !self.shadow.contains(&i) {
                return Some(i);
            }
        }
        None
    }

    /// Exposed at all (a wrapper whose children are, counts).
    fn shown(&self, i: usize) -> bool {
        match self.ty[i] {
            3 => {
                matches!(self.layout.get(&i), Some((_, vis, _)) if *vis != "hidden" && *vis != "collapse")
                    && !html_blank(self.text_of_node(i))
                    && !self.in_check_label(i)
            }
            1 => {
                let tag = self.tag[i];
                if SKIP.contains(&tag) {
                    return false;
                }
                if self.pseudo.contains(&i) {
                    return self.is_marker(i) || self.generated(i);
                }
                if self.attr(i, "aria-hidden").is_some_and(|v| v.trim() == "true") || self.has(i, "inert") {
                    return false;
                }
                if tag == "INPUT" && self.input_type(i) == "hidden" {
                    return false;
                }
                if self.layout.contains_key(&i) || self.rendered_inside[i] {
                    return true;
                }
                // A closed <select>'s options have no box.
                matches!(tag, "OPTION" | "OPTGROUP") && self.in_select(i)
            }
            9 => true,
            _ => false,
        }
    }

    /// Is any radio of `i`'s group (same name, same form) checked?
    fn radio_group_checked(&self, i: usize) -> bool {
        let Some(name) = self.attr(i, "name") else { return self.checked.contains(&i) };
        let form = self.ancestor_tag(i, &["FORM"], 256);
        self.checked.iter().any(|&c| {
            self.tag[c] == "INPUT" && self.attr(c, "name") == Some(name) && self.ancestor_tag(c, &["FORM"], 256) == form
        })
    }

    /// `::before`/`::after` with text (CSS generated content).
    fn generated(&self, i: usize) -> bool {
        self.pseudo.contains(&i)
            && matches!(self.tag[i], "::before" | "::after")
            && matches!(self.layout.get(&i), Some((Some(t), vis, _)) if !t.is_empty() && *vis != "hidden")
            && !self.generated_text(i).trim().is_empty()
    }

    /// What generated content reads as: its alternative text if it has
    /// one (`content: "[" / ""` reads as nothing), else the text shown.
    fn generated_text(&self, i: usize) -> String {
        match self.content_alt.get(&i) {
            Some(alt) => alt.clone(),
            None => self.text_of_node(i).to_string(),
        }
    }

    /// A list item's `::marker` (Chrome's `ListMarker`).
    fn is_marker(&self, i: usize) -> bool {
        self.pseudo.contains(&i)
            && self.tag[i] == "::marker"
            && self.parent[i] >= 0
            && self.tag[self.parent[i] as usize] == "LI"
            && self.layout.contains_key(&i)
    }

    /// How many lists a list item is in.
    fn list_depth(&self, i: usize) -> u64 {
        let mut depth = 0;
        let mut at = self.parent[i];
        for _ in 0..256 {
            if at < 0 {
                break;
            }
            if matches!(self.tag[at as usize], "UL" | "OL" | "MENU") {
                depth += 1;
            }
            at = self.parent[at as usize];
        }
        depth
    }

    fn in_select(&self, i: usize) -> bool {
        let mut at = self.parent[i];
        for _ in 0..3 {
            if at < 0 {
                return false;
            }
            if self.tag[at as usize] == "SELECT" {
                return self.layout.contains_key(&(at as usize));
            }
            at = self.parent[at as usize];
        }
        false
    }

    fn invisible(&self, i: usize) -> bool {
        matches!(self.layout.get(&i), Some((_, vis, _)) if *vis == "hidden" || *vis == "collapse")
    }

    fn is_block(&self, i: usize) -> bool {
        match self.layout.get(&i) {
            Some((_, _, display)) => !display.is_empty() && *display != "inline" && *display != "contents",
            None => false,
        }
    }

    fn ancestor_tag(&self, i: usize, tags: &[&str], within: usize) -> Option<usize> {
        let mut at = self.parent[i];
        for _ in 0..within {
            if at < 0 {
                return None;
            }
            if tags.contains(&self.tag[at as usize]) {
                return Some(at as usize);
            }
            at = self.parent[at as usize];
        }
        None
    }

    fn role(&self, i: usize) -> String {
        if let Some(role) = self.attr(i, "role") {
            if let Some(first) = role.split_whitespace().next() {
                let r = first.to_ascii_lowercase();
                match r.as_str() {
                    "img" => return "image".into(),
                    // Presentation is ignored on what can be focused.
                    "none" | "presentation" if !self.focusable(i) => return "none".into(),
                    "none" | "presentation" => {}
                    _ if r.chars().all(|c| c.is_ascii_lowercase() || c == '-') => return r,
                    _ => {}
                }
            }
        }
        let tag = self.tag[i];
        match tag {
            "A" | "AREA" if self.has(i, "href") => "link",
            // An anchor without href that the page listens to.
            "A" if self.listening.contains(&i) => "link",
            "BUTTON" => "button",
            "INPUT" => match self.input_type(i).as_str() {
                "button" | "submit" | "reset" | "image" | "file" => "button",
                "checkbox" => "checkbox",
                "radio" => "radio",
                "range" => "slider",
                "number" => "spinbutton",
                "date" => "date",
                "time" => "InputTime",
                "datetime-local" | "month" | "week" => "DateTime",
                "color" => "ColorWell",
                "search" if !self.has(i, "list") => "searchbox",
                _ if self.has(i, "list") => "combobox",
                _ => "textbox",
            },
            "SELECT" => {
                let size = self.attr(i, "size").and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(0);
                if self.has(i, "multiple") || size > 1 {
                    "listbox"
                } else {
                    "combobox"
                }
            }
            "OPTION" => "option",
            "OPTGROUP" | "FIELDSET" | "DETAILS" => "group",
            "TEXTAREA" => "textbox",
            "H1" | "H2" | "H3" | "H4" | "H5" | "H6" => "heading",
            // An inline <p> (a "Note:" run-in title) reads as text.
            "P" if matches!(self.layout.get(&i), Some((_, _, "inline"))) => "generic",
            "P" => "paragraph",
            "UL" | "OL" | "MENU" => "list",
            "DL" => "DescriptionList",
            "ABBR" => "Abbr",
            "LI" => "listitem",
            "DT" => "term",
            "DD" => "definition",
            "TABLE" if self.layout_tables.contains(&i) => "LayoutTable",
            "TABLE" => "table",
            "TR" | "TD" | "TH" | "TBODY" | "THEAD" | "TFOOT"
                if self.table_of(i).is_some_and(|t| self.layout_tables.contains(&t)) =>
            {
                match tag {
                    "TR" => "LayoutTableRow",
                    "TD" | "TH" => "LayoutTableCell",
                    _ => "generic",
                }
            }
            // A data table's body is a row group only with an id.
            "TBODY" if self.has(i, "id") => "rowgroup",
            "TR" => "row",
            "TD" => {
                if self
                    .ancestor_tag(i, &["TABLE"], 4)
                    .is_some_and(|t| self.attr(t, "role").is_some_and(|r| matches!(r.trim(), "grid" | "treegrid")))
                {
                    "gridcell"
                } else {
                    "cell"
                }
            }
            "TH" => match self.attr(i, "scope").map(|s| s.trim().to_ascii_lowercase()) {
                Some(s) if s == "row" || s == "rowgroup" => "rowheader",
                Some(s) if s == "col" || s == "colgroup" => "columnheader",
                // A header cell in a row of data cells heads that row.
                _ if self.ancestor_tag(i, &["THEAD"], 2).is_none()
                    && self.parent[i] >= 0
                    && self.children[self.parent[i] as usize].iter().any(|&c| self.tag[c] == "TD") =>
                {
                    "rowheader"
                }
                _ => "columnheader",
            },
            "THEAD" | "TFOOT" => "rowgroup",
            "CAPTION" => "caption",
            "NAV" => "navigation",
            "MAIN" => "main",
            "ASIDE"
                if self.ancestor_tag(i, &["ARTICLE", "ASIDE", "NAV", "SECTION"], 64).is_some()
                    && !self.has(i, "aria-label")
                    && !self.has(i, "aria-labelledby")
                    && !self.has(i, "title") =>
            {
                "generic"
            }
            "ASIDE" => "complementary",
            "HEADER" | "FOOTER" => {
                if self.ancestor_tag(i, &["ARTICLE", "ASIDE", "MAIN", "NAV", "SECTION"], 64).is_some() {
                    if tag == "HEADER" {
                        "sectionheader"
                    } else {
                        "sectionfooter"
                    }
                } else if tag == "HEADER" {
                    "banner"
                } else {
                    "contentinfo"
                }
            }
            "SECTION" if self.has(i, "aria-label") || self.has(i, "aria-labelledby") || self.has(i, "title") => {
                "region"
            }
            "FORM" => "form",
            "ARTICLE" => "article",
            "FIGURE" => "figure",
            "DIALOG" => "dialog",
            "SUMMARY" => "DisclosureTriangle",
            "LABEL" => "LabelText",
            "LEGEND" => "Legend",
            "FIGCAPTION" => "Figcaption",
            "IMG" if self.attr(i, "alt") == Some("") => "none",
            "IMG" => "image",
            "svg" | "SVG" if self.svg_title(i).is_some() => "image",
            "svg" | "SVG" => "SvgRoot",
            "HR" => "separator",
            "PROGRESS" => "progressbar",
            "METER" => "meter",
            "OUTPUT" => "status",
            "IFRAME" | "FRAME" => "Iframe",
            "BLOCKQUOTE" => "blockquote",
            "CODE" => "code",
            "STRONG" => "strong",
            "EM" => "emphasis",
            "MARK" => "mark",
            "DEL" | "S" => "deletion",
            "INS" => "insertion",
            "SUB" => "subscript",
            "SUP" => "superscript",
            "TIME" => "time",
            // Chrome exposes a drag handle as a group.
            _ if self.attr(i, "draggable").is_some_and(|d| d.trim() == "true") => "group",
            _ => "generic",
        }
        .to_string()
    }

    /// Something to act on inside (a link, a control, a focusable box).
    fn holds_controls(&self, i: usize) -> bool {
        let mut stack = self.children[i].clone();
        let mut seen = 0;
        while let Some(k) = stack.pop() {
            seen += 1;
            if seen > 500 {
                return true;
            }
            if self.ty[k] == 1
                && (self.focusable(k) || matches!(self.tag[k], "A" | "BUTTON" | "INPUT" | "SELECT" | "TEXTAREA"))
            {
                return true;
            }
            stack.extend(self.children[k].iter().copied());
        }
        false
    }

    fn svg_title(&self, i: usize) -> Option<String> {
        self.children[i].iter().find(|&&k| self.tag[k].eq_ignore_ascii_case("title")).map(|&k| self.content(k, true))
    }

    fn focusable(&self, i: usize) -> bool {
        // Chrome's Iframe node is not; the frame's document is.
        if self.disabled(i) || matches!(self.tag[i], "IFRAME" | "FRAME") {
            return false;
        }
        if self.attr(i, "tabindex").is_some_and(|t| t.trim().parse::<i32>().is_ok()) {
            return true;
        }
        match self.tag[i] {
            "A" | "AREA" => self.has(i, "href"),
            "BUTTON" | "SELECT" | "TEXTAREA" => true,
            "INPUT" => self.input_type(i) != "hidden",
            "SUMMARY" => self.ancestor_tag(i, &["DETAILS"], 1).is_some(),
            _ => self.attr(i, "contenteditable").is_some_and(|v| matches!(v.trim(), "" | "true" | "plaintext-only")),
        }
    }

    fn disabled(&self, i: usize) -> bool {
        if self.attr(i, "aria-disabled").is_some_and(|v| v.trim() == "true") {
            return true;
        }
        let tag = self.tag[i];
        if !matches!(tag, "BUTTON" | "INPUT" | "SELECT" | "TEXTAREA" | "OPTION" | "OPTGROUP" | "FIELDSET") {
            return false;
        }
        if self.has(i, "disabled") {
            return true;
        }
        // Inside a disabled fieldset, unless in its first legend.
        let mut at = self.parent[i];
        let mut via = i;
        for _ in 0..64 {
            if at < 0 {
                return false;
            }
            let a = at as usize;
            if self.tag[a] == "FIELDSET" && self.has(a, "disabled") {
                let first_legend = self.children[a].iter().copied().find(|&k| self.tag[k] == "LEGEND");
                return first_legend != Some(via);
            }
            via = a;
            at = self.parent[a];
        }
        false
    }

    /// Text of `ids` (an IDREF list), for labelledby and describedby: the
    /// elements' content, hidden or not.
    fn idref_text(&self, ids: &str) -> String {
        let parts: Vec<String> = ids
            .split_whitespace()
            .filter_map(|id| self.by_id.get(id))
            .map(|&k| {
                self.attr(k, "aria-label")
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .unwrap_or_else(|| self.content(k, true))
            })
            .filter(|t| !t.is_empty())
            .collect();
        squash(&parts.join(" "))
    }

    fn name(&self, i: usize, role: &str) -> String {
        if let Some(ids) = self.attr(i, "aria-labelledby") {
            let t = self.idref_text(ids);
            if !t.is_empty() {
                return t;
            }
        }
        if let Some(l) = self.attr(i, "aria-label") {
            let l = squash(l);
            if !l.is_empty() {
                return l;
            }
        }
        let tag = self.tag[i];
        let native = match tag {
            "INPUT" => match self.input_type(i).as_str() {
                "button" | "submit" | "reset" => {
                    let v = self.attr(i, "value").map(squash).unwrap_or_default();
                    if v.is_empty() {
                        match self.input_type(i).as_str() {
                            "submit" => "Submit".to_string(),
                            "reset" => "Reset".to_string(),
                            _ => String::new(),
                        }
                    } else {
                        v
                    }
                }
                "file" => {
                    let l = self.label_text(i);
                    if l.is_empty() {
                        "Choose File".to_string()
                    } else {
                        l
                    }
                }
                "image" => self
                    .attr(i, "alt")
                    .or(self.attr(i, "value"))
                    .map(squash)
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| "Submit".into()),
                _ => self.label_text(i),
            },
            "SELECT" | "TEXTAREA" | "METER" | "PROGRESS" | "OUTPUT" => self.label_text(i),
            "IMG" | "AREA" => self.attr(i, "alt").map(squash).unwrap_or_default(),
            "FIELDSET" => self.first_child_text(i, "LEGEND"),
            "TABLE" => self.first_child_text(i, "CAPTION"),
            "svg" | "SVG" => self.svg_title(i).unwrap_or_default(),
            "IFRAME" | "FRAME" => self.attr(i, "title").map(squash).unwrap_or_default(),
            _ => String::new(),
        };
        if !native.is_empty() {
            return native;
        }
        if FROM_CONTENT.contains(&role) || (tag == "BUTTON") {
            let t = self.content(i, false);
            if !t.is_empty() {
                return t;
            }
        }
        if let Some(t) = self.attr(i, "title") {
            let t = squash(t);
            if !t.is_empty() {
                return t;
            }
        }
        if matches!(role, "textbox" | "searchbox" | "combobox" | "spinbutton") {
            if let Some(p) = self.attr(i, "placeholder").or(self.attr(i, "aria-placeholder")) {
                return squash(p);
            }
        }
        String::new()
    }

    fn first_child_text(&self, i: usize, tag: &str) -> String {
        self.children[i].iter().find(|&&k| self.tag[k] == tag).map(|&k| self.content(k, false)).unwrap_or_default()
    }

    /// A form control's labels: `<label for>` and the `<label>` around it.
    fn label_text(&self, i: usize) -> String {
        let mut labels: Vec<usize> = self.labels_for.get(&i).cloned().unwrap_or_default();
        if let Some(l) = self.ancestor_tag(i, &["LABEL"], 64) {
            if !labels.contains(&l) {
                labels.push(l);
            }
        }
        let parts: Vec<String> =
            labels.iter().map(|&l| self.content_except(l, Some(i), false)).filter(|t| !t.is_empty()).collect();
        squash(&parts.join(" "))
    }

    fn content(&self, i: usize, hidden_too: bool) -> String {
        self.content_except(i, None, hidden_too)
    }

    /// The text of `i`'s content, as a name from content reads it: text
    /// nodes, an embedded control's value, an image's alt, an element's
    /// own aria-label; block boxes kept apart by a space.
    fn content_except(&self, i: usize, skip: Option<usize>, hidden_too: bool) -> String {
        let mut out = String::new();
        self.collect_text(i, skip, hidden_too, &mut out, 0);
        squash(&out)
    }

    fn collect_text(&self, i: usize, skip: Option<usize>, hidden_too: bool, out: &mut String, depth: usize) {
        if depth > 64 {
            return;
        }
        for k in self.kids(i) {
            if Some(k) == skip {
                continue;
            }
            match self.ty[k] {
                3 => {
                    // Whitespace between elements counts: it keeps words apart.
                    let laid =
                        matches!(self.layout.get(&k), Some((_, vis, _)) if *vis != "hidden" && *vis != "collapse");
                    if hidden_too || laid || self.in_select(k) || self.tag[i] == "OPTION" {
                        out.push_str(self.text_of_node(k));
                    }
                }
                1 => {
                    let tag = self.tag[k];
                    if tag == "BR" || tag == "WBR" {
                        out.push(' ');
                        continue;
                    }
                    if self.pseudo.contains(&k) {
                        if self.generated(k) && !self.counters.contains(&k) {
                            out.push_str(&self.generated_text(k));
                        }
                        continue;
                    }
                    if !hidden_too && !self.shown(k) {
                        continue;
                    }
                    if SKIP.contains(&tag) {
                        continue;
                    }
                    let block = self.is_block(k);
                    if block {
                        out.push(' ');
                    }
                    if let Some(l) = self.attr(k, "aria-label").map(squash).filter(|l| !l.is_empty()) {
                        out.push_str(&l);
                    } else {
                        match tag {
                            "IMG" | "AREA" => out.push_str(self.attr(k, "alt").unwrap_or("")),
                            "svg" | "SVG" => {
                                if let Some(t) = self.svg_title(k) {
                                    out.push_str(&t);
                                }
                            }
                            "INPUT" => match self.input_type(k).as_str() {
                                "checkbox" | "radio" | "hidden" => {}
                                "button" | "submit" | "reset" => out.push_str(self.attr(k, "value").unwrap_or("")),
                                _ => out.push_str(self.input_value.get(&k).copied().unwrap_or("")),
                            },
                            "TEXTAREA" => out.push_str(self.text_value.get(&k).copied().unwrap_or("")),
                            "SELECT" => out.push_str(&self.select_value(k)),
                            _ => self.collect_text(k, skip, hidden_too, out, depth + 1),
                        }
                    }
                    if block {
                        out.push(' ');
                    }
                }
                _ => {}
            }
        }
    }

    fn options(&self, i: usize, out: &mut Vec<usize>) {
        for &k in &self.children[i] {
            match self.tag[k] {
                "OPTION" => out.push(k),
                "OPTGROUP" => self.options(k, out),
                _ => {}
            }
        }
    }

    fn select_value(&self, i: usize) -> String {
        let mut opts = Vec::new();
        self.options(i, &mut opts);
        let chosen = opts.iter().copied().find(|o| self.selected.contains(o)).or(opts.first().copied());
        chosen.map(|o| self.option_label(o)).unwrap_or_default()
    }

    fn option_label(&self, o: usize) -> String {
        self.attr(o, "label").map(squash).filter(|l| !l.is_empty()).unwrap_or_else(|| self.content(o, true))
    }

    fn value_of(&self, i: usize, role: &str) -> Option<String> {
        match self.tag[i] {
            "INPUT" => match self.input_type(i).as_str() {
                "file" => Some(
                    self.input_value
                        .get(&i)
                        .filter(|v| !v.is_empty())
                        .map(|v| v.rsplit(['\\', '/']).next().unwrap_or(v).to_string())
                        .unwrap_or_else(|| "No file chosen".into()),
                ),
                "checkbox" | "radio" | "button" | "submit" | "reset" | "image" | "hidden" => None,
                "password" => self.input_value.get(&i).map(|v| "•".repeat(v.chars().count())),
                _ => self.input_value.get(&i).map(|v| v.to_string()),
            },
            "TEXTAREA" => self.text_value.get(&i).or(self.input_value.get(&i)).map(|v| v.to_string()),
            "SELECT" if role == "combobox" => Some(self.select_value(i)),
            "PROGRESS" | "METER" => self.attr(i, "value").map(str::to_string),
            _ if role == "textbox" && self.attr(i, "contenteditable").is_some_and(|v| v.trim() != "false") => {
                Some(self.content(i, false))
            }
            _ if matches!(role, "progressbar" | "slider" | "spinbutton" | "scrollbar" | "meter") => {
                self.attr(i, "aria-valuetext").or(self.attr(i, "aria-valuenow")).map(|v| v.trim().to_string())
            }
            _ => None,
        }
        .filter(|v| !v.is_empty())
    }

    fn resolve(&self, href: &str) -> String {
        percent_encode(&self.resolve_raw(href))
    }

    fn resolve_raw(&self, href: &str) -> String {
        let href = href.trim();
        let base = if self.base.is_empty() { self.url } else { self.base };
        if href.is_empty() {
            return base.to_string();
        }
        if let Some(colon) = href.find(':') {
            if href[..colon].chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) && colon > 0 {
                // `http://host` is `http://host/`, `http://host?q` is `http://host/?q`.
                if let Some((scheme, rest)) = href.split_once("://") {
                    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
                    if !rest[end..].starts_with('/') {
                        return format!("{scheme}://{}/{}", &rest[..end], &rest[end..]);
                    }
                }
                return href.to_string();
            }
        }
        let Some(scheme_end) = base.find("://") else { return href.to_string() };
        let after = &base[scheme_end + 3..];
        let host_end = after.find('/').map(|j| scheme_end + 3 + j).unwrap_or(base.len());
        let origin = &base[..host_end];
        if href.starts_with("//") {
            return self.resolve_raw(&format!("{}{href}", &base[..scheme_end + 1]));
        }
        if href.starts_with('/') {
            return format!("{origin}{href}");
        }
        let no_frag = base.split('#').next().unwrap_or(base);
        if href.starts_with('#') {
            return format!("{no_frag}{href}");
        }
        let no_query = no_frag.split('?').next().unwrap_or(no_frag);
        if href.starts_with('?') {
            return format!("{no_query}{href}");
        }
        let dir = match no_query[host_end..].rfind('/') {
            Some(j) => &no_query[..host_end + j + 1],
            None => &no_query[..host_end],
        };
        let joined = if dir.ends_with('/') { format!("{dir}{href}") } else { format!("{dir}/{href}") };
        // `./` and `../` segments of the path.
        let (head, path) = joined.split_at(host_end);
        let mut segs: Vec<&str> = Vec::new();
        let trailing = path.ends_with('/') || path.ends_with("/.") || path.ends_with("/..");
        for seg in path.split('/') {
            match seg {
                "" | "." => {}
                ".." => {
                    segs.pop();
                }
                s => segs.push(s),
            }
        }
        let mut out = format!("{head}/{}", segs.join("/"));
        if trailing && !out.ends_with('/') {
            out.push('/');
        }
        out
    }

    fn properties(&self, i: usize, role: &str) -> Vec<Value> {
        let mut props = Vec::new();
        let mut push = |name: &str, value: Value| props.push(json!({ "name": name, "value": { "value": value } }));
        if self.focusable(i) {
            push("focusable", json!(true));
        }
        let tag = self.tag[i];
        if tag == "INPUT" && matches!(self.input_type(i).as_str(), "checkbox" | "radio") {
            push("checked", json!(if self.checked.contains(&i) { "true" } else { "false" }));
        } else if let Some(c) = self.attr(i, "aria-checked") {
            if matches!(
                role,
                "checkbox" | "radio" | "switch" | "menuitemcheckbox" | "menuitemradio" | "treeitem" | "option"
            ) {
                push("checked", json!(c.trim()));
            }
        }
        if let Some(p) = self.attr(i, "aria-pressed") {
            push("pressed", json!(p.trim()));
        }
        if self.disabled(i) {
            push("disabled", json!(true));
        }
        let radio = tag == "INPUT" && self.input_type(i) == "radio";
        if radio {
            if self.has(i, "required") && !self.radio_group_checked(i) {
                push("invalid", json!("true"));
            }
        } else if self.has(i, "required") || self.attr(i, "aria-required").is_some_and(|v| v.trim() == "true") {
            push("required", json!(true));
        }
        if (matches!(tag, "INPUT" | "TEXTAREA") && self.has(i, "readonly"))
            || self.attr(i, "aria-readonly").is_some_and(|v| v.trim() == "true")
        {
            push("readonly", json!(true));
        }
        if self.attr(i, "aria-modal").is_some_and(|v| v.trim() == "true") {
            push("modal", json!(true));
        }
        if role != "option" {
            if let Some(sel) = self.attr(i, "aria-selected") {
                push("selected", json!(sel.trim() == "true"));
            }
        } else if self.selected.contains(&i) || self.attr(i, "aria-selected").is_some_and(|v| v.trim() == "true") {
            push("selected", json!(true));
        }
        if let Some(e) = self.attr(i, "aria-expanded") {
            push("expanded", json!(e.trim() == "true"));
        } else if tag == "SUMMARY" {
            if let Some(d) = self.ancestor_tag(i, &["DETAILS"], 1) {
                push("expanded", json!(self.has(d, "open")));
            }
        }
        if let Some(v) = self.attr(i, "aria-invalid") {
            push("invalid", json!(v.trim()));
        }
        let level = match tag {
            "H1" => Some(1),
            "H2" => Some(2),
            "H3" => Some(3),
            "H4" => Some(4),
            "H5" => Some(5),
            "H6" => Some(6),
            _ => None,
        };
        let level = self
            .attr(i, "aria-level")
            .and_then(|l| l.trim().parse::<u64>().ok())
            .or(level)
            .or_else(|| (tag == "LI").then(|| self.list_depth(i)).filter(|d| *d > 0));
        if let Some(l) = level {
            if matches!(role, "heading" | "treeitem" | "listitem" | "row") {
                push("level", json!(l));
            }
        }
        if role == "link" {
            if let Some(h) = self.attr(i, "href") {
                push("url", json!(self.resolve(h)));
            }
        }
        props
    }

    /// Append this document's nodes to `out`; `frames` gets each
    /// <iframe> with a document in the snapshot.
    /// Is `i` in the tree with every ancestor up to its document (through
    /// shadow hosts)? A scoped walk starts below them, so it checks.
    fn exposed_with_ancestors(&self, i: usize) -> bool {
        let mut at = i as i64;
        for _ in 0..4096 {
            if at < 0 {
                return true;
            }
            if !self.shown(at as usize) {
                return false;
            }
            at = self.parent[at as usize];
        }
        false
    }

    /// Emit the AX nodes of document `d` from its root, or from `from` (a
    /// scoped snapshot's element) down.
    fn emit(&self, d: usize, from: Option<usize>, out: &mut Vec<Value>, frames: &mut Vec<(u64, usize)>) {
        let Some(root) = from.or_else(|| (0..self.ty.len()).find(|&i| self.ty[i] == 9)) else { return };
        let id = |i: usize| format!("d{d}n{i}");
        // Iterative walk: (node, parent AX id).
        let mut stack: Vec<(usize, Option<String>)> = vec![(root, None)];
        // AX children per emitted node, filled as the walk reaches them.
        let mut pending_children: HashMap<String, Vec<String>> = HashMap::new();
        let mut order: Vec<(String, Value)> = Vec::new();
        while let Some((i, parent)) = stack.pop() {
            if !self.shown(i) {
                continue;
            }
            let my = id(i);
            let mut node = match self.ty[i] {
                9 => json!({
                    "role": { "value": "RootWebArea" },
                    "name": { "value": self.title },
                    "properties": [
                        { "name": "focusable", "value": { "value": true } },
                        { "name": "url", "value": { "value": self.url } },
                    ],
                }),
                3 => json!({
                    "role": { "value": "StaticText" },
                    "name": { "value": squash(self.text_of_node(i)) },
                }),
                _ if self.is_marker(i) => json!({
                    "role": { "value": "ListMarker" },
                    "name": { "value": squash(self.text_of_node(i)) },
                }),
                // Generated content: Chrome's ignored box around its text.
                _ if self.pseudo.contains(&i) => json!({
                    "role": { "value": "none" },
                    "properties": [],
                    "childIds": [],
                    "generated": squash(&self.generated_text(i)),
                }),
                // `visibility: hidden`: not there for a user, though a
                // child that sets it visible is.
                _ if self.invisible(i) => json!({ "role": { "value": "generic" }, "properties": [] }),
                _ => {
                    let role = self.role(i);
                    let mut n = json!({ "role": { "value": role } });
                    if role != "generic" && role != "none" {
                        let name = self.name(i, &role);
                        if !name.is_empty() {
                            n["name"] = json!({ "value": name });
                        }
                        if let Some(v) = self.value_of(i, &role) {
                            n["value"] = json!({ "value": v });
                        }
                    } else {
                        let named = self
                            .attr(i, "aria-labelledby")
                            .map(|ids| self.idref_text(ids))
                            .filter(|t| !t.is_empty())
                            .or_else(|| self.attr(i, "aria-label").map(squash).filter(|t| !t.is_empty()))
                            .or_else(|| {
                                // A custom element's title (a `<relative-time>`'s
                                // date); Chrome names no plain <span> by it.
                                self.attr(i, "title")
                                    .filter(|_| self.tag[i].contains('-'))
                                    .map(squash)
                                    .filter(|t| !t.is_empty() && !self.holds_controls(i))
                            });
                        if let Some(t) = named {
                            n["name"] = json!({ "value": t });
                        }
                    }
                    if let Some(desc) = self.attr(i, "aria-describedby").map(|ids| self.idref_text(ids)) {
                        if !desc.is_empty() {
                            n["description"] = json!({ "value": desc });
                        }
                    }
                    n["properties"] = Value::Array(self.properties(i, &role));
                    n
                }
            };
            node["nodeId"] = json!(my);
            node["ignored"] = json!(false);
            if self.backend[i] > 0 {
                node["backendDOMNodeId"] = json!(self.backend[i]);
            }
            if let Some(p) = &parent {
                node["parentId"] = json!(p);
                pending_children.entry(p.clone()).or_default().push(my.clone());
            }
            if self.ty[i] == 1 {
                if let Some(&doc) = self.content_doc.get(&i) {
                    if self.backend[i] > 0 {
                        frames.push((self.backend[i] as u64, doc));
                    }
                }
            }
            let mut generated = node.as_object_mut().and_then(|o| o.remove("generated"));
            // A form field's text (a button's label, a field's value) is a
            // text node inside it in Chrome's tree, as names compare with.
            if self.tag[i] == "INPUT" && generated.is_none() {
                let role = node.pointer("/role/value").and_then(Value::as_str).unwrap_or("").to_string();
                generated = match role.as_str() {
                    "button" if self.input_type(i) != "image" => node.pointer("/name/value").cloned(),
                    "textbox" | "searchbox" | "spinbutton" | "combobox" => node.pointer("/value/value").cloned(),
                    _ => None,
                };
            }
            order.push((my.clone(), node));
            if let Some(text) = generated {
                let text_id = format!("{my}t");
                pending_children.entry(my.clone()).or_default().push(text_id.clone());
                order.push((
                    text_id.clone(),
                    json!({ "nodeId": text_id, "ignored": false, "parentId": my, "role": { "value": "StaticText" }, "name": { "value": text } }),
                ));
            }
            // The options of a closed select are its children; a text node
            // or a frame has none here.
            if self.ty[i] == 3 || self.content_doc.contains_key(&i) || self.pseudo.contains(&i) {
                continue;
            }
            let kids = self.kids(i);
            for &k in kids.iter().rev() {
                if self.tag[i] == "OPTION" && self.ty[k] == 3 {
                    continue;
                }
                stack.push((k, Some(my.clone())));
            }
        }
        for (my, mut node) in order {
            node["childIds"] = json!(pending_children.remove(&my).unwrap_or_default());
            out.push(node);
        }
    }
}

/// A URL as the browser reports it: non-ASCII bytes, spaces, quotes and
/// angle brackets percent-encoded (what is already encoded stays).
fn percent_encode(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for b in url.bytes() {
        if b >= 0x80 || matches!(b, b' ' | b'"' | b'<' | b'>' | b'`') || b < 0x20 {
            out.push_str(&format!("%{b:02X}"));
        } else {
            out.push(b as char);
        }
    }
    out
}

/// The alternative text of a computed `content` value, if it has one:
/// `"[" / ""` is empty, `url(x.svg) / "Next"` is "Next".
fn content_alt_text(content: &str) -> Option<String> {
    let (_, alt) = content.rsplit_once(" / ")?;
    let mut out = String::new();
    let mut quoted = false;
    for c in alt.chars() {
        match c {
            '"' => quoted = !quoted,
            c if quoted => out.push(c),
            _ => {}
        }
    }
    Some(out)
}

/// Only HTML's collapsible whitespace (a no-break space is text).
fn html_blank(s: &str) -> bool {
    s.chars().all(|c| matches!(c, ' ' | '\n' | '\t' | '\r' | '\x0c'))
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (tag, parent index, attributes, text, laid out)
    type Spec<'a> = (&'a str, i64, &'a [(&'a str, &'a str)], Option<&'a str>, bool);

    /// A snapshot of one document from node specs.
    fn snap(nodes: &[Spec]) -> Value {
        let mut strings: Vec<String> = Vec::new();
        let mut idx = |s: &str| -> i64 {
            match strings.iter().position(|x| x == s) {
                Some(i) => i as i64,
                None => {
                    strings.push(s.to_string());
                    (strings.len() - 1) as i64
                }
            }
        };
        let mut parent = vec![-1];
        let mut ty = vec![9];
        let mut name = vec![idx("#document")];
        let mut value = vec![-1];
        let mut backend = vec![1];
        let mut attributes = vec![json!([])];
        let mut layout_nodes = vec![];
        let mut styles = vec![];
        let mut texts = vec![];
        let (vis, disp) = (idx("visible"), idx("block"));
        for (k, (tag, p, attrs, text, laid)) in nodes.iter().enumerate() {
            let i = k + 1;
            parent.push(*p);
            let is_text = *tag == "#text";
            ty.push(if is_text { 3 } else { 1 });
            name.push(idx(tag));
            value.push(text.map(&mut idx).unwrap_or(-1));
            backend.push(i as i64 + 1);
            let a: Vec<i64> = attrs.iter().flat_map(|(n, v)| [idx(n), idx(v)]).collect();
            attributes.push(json!(a));
            if *laid {
                layout_nodes.push(i as i64);
                styles.push(json!([-1, -1, -1, vis, disp]));
                texts.push(text.map(&mut idx).unwrap_or(-1));
            }
        }
        let (url, title) = (idx("http://x.test/a/b"), idx("T"));
        json!({
            "strings": strings,
            "documents": [{
                "documentURL": url,
                "baseURL": -1,
                "title": title,
                "nodes": {
                    "parentIndex": parent, "nodeType": ty, "nodeName": name, "nodeValue": value,
                    "backendNodeId": backend, "attributes": attributes,
                },
                "layout": { "nodeIndex": layout_nodes, "styles": styles, "text": texts },
            }],
        })
    }

    fn render(nodes: &[Spec]) -> String {
        let built = build(&snap(nodes)).unwrap();
        super::super::ax::render("", &built.nodes, &built.frames, &super::super::ax::Options::default()).text
    }

    #[test]
    fn roles_names_and_states_from_markup() {
        let out = render(&[
            ("MAIN", 0, &[], None, true),
            ("H2", 1, &[], None, true),
            ("#text", 2, &[], Some("Orders"), true),
            ("LABEL", 1, &[("for", "q")], None, true),
            ("#text", 4, &[], Some("Search"), true),
            ("INPUT", 1, &[("id", "q"), ("type", "search"), ("required", "")], None, true),
            ("A", 1, &[("href", "../c?x=1")], None, true),
            ("#text", 7, &[], Some("Next  page"), true),
            ("BUTTON", 1, &[("aria-label", "Close")], None, true),
            ("#text", 9, &[], Some("X"), true),
            ("DIV", 1, &[("hidden", "")], None, false),
            ("#text", 11, &[], Some("gone"), false),
            ("DIV", 1, &[("aria-hidden", "true")], None, true),
            ("#text", 13, &[], Some("hidden too"), true),
        ]);
        assert_eq!(
            out,
            "- main:\n  - heading \"Orders\" [level=2]\n  - searchbox \"Search\" [required, ref=7]\n  \
             - link \"Next page\" [ref=8] url=http://x.test/c?x=1\n  - button \"Close\" [ref=10]: X"
        );
    }

    /// A scoped build holds the element's subtree only, named as in the
    /// whole tree (a `<label for>` outside the scope still names a field);
    /// a hidden element, or one inside a hidden ancestor, gives no nodes;
    /// an element not in the snapshot gives none at all.
    #[test]
    fn a_scoped_build_holds_the_scope_only() {
        let specs: &[Spec] = &[
            ("MAIN", 0, &[], None, true),
            ("H2", 1, &[], None, true),
            ("#text", 2, &[], Some("Orders"), true),
            ("LABEL", 1, &[("for", "q")], None, true),
            ("#text", 4, &[], Some("Search"), true),
            ("FORM", 1, &[], None, true),
            ("INPUT", 6, &[("id", "q"), ("type", "search")], None, true),
            ("DIV", 1, &[("hidden", "")], None, false),
            ("DIV", 1, &[("aria-hidden", "true")], None, true),
            ("SPAN", 9, &[], None, true),
            ("#text", 10, &[], Some("hidden too"), true),
        ];
        let snapshot = snap(specs);
        // Spec k is node k + 1, backend id k + 2.
        let form = 7;
        let built = build_scoped(&snapshot, form).unwrap();
        assert_eq!(built.nodes.first().and_then(|n| n["backendDOMNodeId"].as_u64()), Some(form));
        assert!(built.nodes.iter().all(|n| n["name"]["value"] != "Orders"), "outside the scope");
        let opts = super::super::ax::Options { root_backend_id: Some(form), ..Default::default() };
        let out = super::super::ax::render("", &built.nodes, &built.frames, &opts).text;
        assert_eq!(out, "- form:\n  - searchbox \"Search\" [ref=8]");
        assert!(build_scoped(&snapshot, 9).unwrap().nodes.is_empty(), "a hidden element");
        assert!(build_scoped(&snapshot, 11).unwrap().nodes.is_empty(), "inside an aria-hidden ancestor");
        assert!(build_scoped(&snapshot, 999).is_none(), "not in the snapshot");
    }

    #[test]
    fn urls_resolve_against_the_document() {
        let s = snap(&[]);
        let strings: Vec<&str> = s["strings"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        let doc = Doc::new(&s["documents"][0], &strings);
        assert_eq!(doc.resolve("/x"), "http://x.test/x");
        assert_eq!(doc.resolve("y"), "http://x.test/a/y");
        assert_eq!(doc.resolve("../z/"), "http://x.test/z/");
        assert_eq!(doc.resolve("#top"), "http://x.test/a/b#top");
        assert_eq!(doc.resolve("?q=1"), "http://x.test/a/b?q=1");
        assert_eq!(doc.resolve("//cdn.test/p"), "http://cdn.test/p");
        assert_eq!(doc.resolve("mailto:a@b.c"), "mailto:a@b.c");
    }
}
