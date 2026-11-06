//! A DOM snapshot (`DOMSnapshot.captureSnapshot`) held as plain columns.
//!
//! The tree built from a snapshot (`domax`) and the clickable elements
//! (`page::clickables_from`) both read it, and a large page's snapshot is
//! kept after a read: what an action changed is then read from the page
//! for the changed subtrees only and patched in ([`Snap::patch`]), instead
//! of capturing the whole page again (a 10,000-row table: ~480 ms in
//! Chrome alone). Kept as integer columns, not as the JSON values the
//! answer came in: a number in 8 bytes, where a JSON value takes 32.

use std::collections::HashMap;

use serde_json::Value;

use super::fxhash::{FxMap, FxSet};

/// The computed styles `ax` asks the snapshot for, in this order:
/// cursor, pointer-events, opacity, visibility, display, content.
pub const STYLES: usize = 6;

/// The computed styles a capture asks for: the [`STYLES`] kept per layout
/// object, then `content-visibility`, read only to tell which content a
/// display lock skips (see [`Snap::from_value`]).
pub const CAPTURE_STYLES: [&str; STYLES + 1] =
    ["cursor", "pointer-events", "opacity", "visibility", "display", "content", "content-visibility"];

#[derive(Clone, Default, Debug)]
pub struct Snap {
    pub strings: Vec<String>,
    pub docs: Vec<SnapDoc>,
}

/// One document of a snapshot: one entry per node in each column (string
/// columns hold indices into [`Snap::strings`], -1 for none), plus the
/// sparse ones as (node, value) pairs in node order.
#[derive(Clone, Default, Debug)]
pub struct SnapDoc {
    pub url: i64,
    pub base: i64,
    pub title: i64,
    pub parent: Vec<i64>,
    pub ty: Vec<i64>,
    pub name: Vec<i64>,
    pub value: Vec<i64>,
    pub backend: Vec<i64>,
    /// Name, value, name, value...
    pub attrs: Vec<Vec<i64>>,
    pub shadow_root_type: Vec<(usize, i64)>,
    pub input_value: Vec<(usize, i64)>,
    pub text_value: Vec<(usize, i64)>,
    pub pseudo_type: Vec<(usize, i64)>,
    pub content_document: Vec<(usize, i64)>,
    pub input_checked: Vec<usize>,
    pub option_selected: Vec<usize>,
    pub clickable: Vec<usize>,
    pub layout: Vec<Layout>,
}

/// A layout object: its node, its text (a text node's, after
/// `text-transform`), the styles of [`STYLES`] and its box.
#[derive(Clone, Debug)]
pub struct Layout {
    pub node: usize,
    pub text: i64,
    pub styles: [i64; STYLES],
    pub bounds: [f64; 4],
}

fn ints(v: &Value, key: &str) -> Vec<i64> {
    v.get(key).and_then(Value::as_array).into_iter().flatten().map(|x| x.as_i64().unwrap_or(-1)).collect()
}

fn rare(v: &Value, key: &str) -> Vec<(usize, i64)> {
    let r = &v[key];
    let index = ints(r, "index");
    let value = ints(r, "value");
    index.into_iter().zip(value).filter(|(i, _)| *i >= 0).map(|(i, v)| (i as usize, v)).collect()
}

fn rare_bool(v: &Value, key: &str) -> Vec<usize> {
    ints(&v[key], "index").into_iter().filter(|i| *i >= 0).map(|i| i as usize).collect()
}

impl Snap {
    /// The columns of a `DOMSnapshot.captureSnapshot` answer; None
    /// without strings or documents.
    ///
    /// Content a display lock skips has no layout objects here, though
    /// Chrome keeps the ones it had when last rendered and the capture
    /// lists them: a `<details>` opened once and closed again would show
    /// its body, and `content-visibility: hidden` or `hidden=until-found`
    /// content shown once would stay. Skipped: what is under an element
    /// with `content-visibility: hidden` (from the capture's
    /// [`CAPTURE_STYLES`]), and what is in a closed `<details>` but its
    /// summary and its own pseudo-elements. Told from the parent only, top
    /// down, as the change journal's read (`page::JOURNAL_DELTA`) tells it:
    /// a patched snapshot and a capture must agree.
    pub fn from_value(snap: &Value) -> Option<Snap> {
        let strings: Vec<String> =
            snap.get("strings")?.as_array()?.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect();
        let words = Self::words(&strings);
        let docs = snap.get("documents")?.as_array()?.iter().map(|d| SnapDoc::from_value(d, words[0])).collect();
        Some(Self::unlocked(strings, docs, words))
    }

    /// [`Snap::from_value`] from the answer's JSON text, read straight
    /// into the columns: no JSON value per number on the way (a
    /// 10,000-row table's 7 MB: ~50 ms as values, see `wire`). None when
    /// the text isn't a capture.
    pub fn from_json(text: &str) -> Option<Snap> {
        let capture: wire::Capture = serde_json::from_str(text).ok()?;
        let words = Self::words(&capture.strings);
        let docs = capture.documents.into_iter().map(|d| SnapDoc::from_wire(d, words[0])).collect();
        Some(Self::unlocked(capture.strings, docs, words))
    }

    /// The string indices of "hidden", "DETAILS", "SUMMARY" and "open"
    /// (-1 for none).
    fn words(strings: &[String]) -> [i64; 4] {
        let mut words = [-1i64; 4];
        for (i, s) in strings.iter().enumerate() {
            let slot = match s.as_str() {
                "hidden" => 0,
                "DETAILS" => 1,
                "SUMMARY" => 2,
                "open" => 3,
                _ => continue,
            };
            if words[slot] < 0 {
                words[slot] = i as i64;
            }
        }
        words
    }

    /// The documents without the layout of what a display lock skips.
    fn unlocked(strings: Vec<String>, docs: Vec<(SnapDoc, Vec<usize>)>, words: [i64; 4]) -> Snap {
        let [_, details, summary, open] = words;
        let docs = docs
            .into_iter()
            .map(|(mut doc, cv_hidden)| {
                if details >= 0 || !cv_hidden.is_empty() {
                    let locked = doc.locked(&cv_hidden, details, summary, open);
                    doc.layout.retain(|l| !locked[l.node]);
                }
                doc
            })
            .collect();
        Snap { strings, docs }
    }

    /// The string at `i` ("" for none).
    pub fn at(&self, i: i64) -> &str {
        if i >= 0 {
            self.strings.get(i as usize).map_or("", String::as_str)
        } else {
            ""
        }
    }

    /// Whether a node inside a closed shadow root is in the snapshot (the
    /// page's scripts can't reach one).
    pub fn has_closed_shadow_root(&self) -> bool {
        self.docs.iter().any(|d| d.shadow_root_type.iter().any(|&(_, v)| self.at(v) == "closed"))
    }

    /// The main document with `ops` applied: each subtree an op names is
    /// read from the page again, the rest kept. An error when an op names
    /// a node the snapshot doesn't hold, or the ops overlap; the caller
    /// then captures the page whole.
    pub fn patch(self, ops: Vec<Op>) -> Result<Snap, &'static str> {
        self.patch_with(ops, true)
    }

    /// [`Snap::patch`]; `bulk`: a subtree no op reaches is copied as one
    /// run of nodes (each subtree is one run when the document is in
    /// preorder, as a capture and a patch are; else node by node).
    fn patch_with(mut self, ops: Vec<Op>, bulk: bool) -> Result<Snap, &'static str> {
        if self.docs.is_empty() {
            return Err("no document");
        }
        let mut old = std::mem::take(&mut self.docs[0]);
        let n = old.parent.len();
        let kids = Kids::of(&old.parent)?;
        let preorder = bulk && is_preorder(&old.parent);
        // Nodes by backend id: only the ones the ops name, found in one pass.
        let mut by_backend: FxMap<i64, usize> = FxMap::default();
        for op in &ops {
            match op {
                Op::Replace { backend, .. } | Op::Clickable { backend, .. } => {
                    by_backend.insert(*backend, usize::MAX);
                }
                Op::Children { backend, children } => {
                    by_backend.insert(*backend, usize::MAX);
                    for c in children {
                        if let Child::Keep(b) = c {
                            by_backend.insert(*b, usize::MAX);
                        }
                    }
                }
            }
        }
        for (i, b) in old.backend.iter().enumerate() {
            if let Some(slot) = by_backend.get_mut(b) {
                // The last node with the id, as a map of every node keeps.
                *slot = i;
            }
        }
        by_backend.retain(|_, i| *i != usize::MAX);
        // Sparse columns by node, for copying.
        let column = |pairs: &[(usize, i64)]| {
            let mut v = vec![None; n];
            for &(i, x) in pairs {
                if i < n {
                    v[i] = Some(x);
                }
            }
            v
        };
        let flags = |list: &[usize]| {
            let mut v = vec![false; n];
            for &i in list {
                if i < n {
                    v[i] = true;
                }
            }
            v
        };
        let shadow = column(&old.shadow_root_type);
        let input_value = column(&old.input_value);
        let text_value = column(&old.text_value);
        let pseudo = column(&old.pseudo_type);
        let content_document = column(&old.content_document);
        let checked = flags(&old.input_checked);
        let selected = flags(&old.option_selected);
        let clickable = flags(&old.clickable);
        // Layout objects by node (most nodes have none or one).
        let mut first_layout = vec![usize::MAX; n];
        let mut more_layout: FxMap<usize, Vec<usize>> = FxMap::default();
        for (k, l) in old.layout.iter().enumerate() {
            if l.node >= n {
                continue;
            }
            if first_layout[l.node] == usize::MAX {
                first_layout[l.node] = k;
            } else {
                more_layout.entry(l.node).or_default().push(k);
            }
        }
        let mut facts = Facts {
            shadow,
            input_value,
            text_value,
            pseudo,
            content_document,
            checked,
            selected,
            clickable,
            first_layout,
            more_layout,
        };
        let mut replace: FxMap<usize, Vec<NewNode>> = FxMap::default();
        let mut children: FxMap<usize, Vec<Child>> = FxMap::default();
        for op in ops {
            match op {
                Op::Replace { backend, nodes } => {
                    let i = *by_backend.get(&backend).ok_or("replaced node not in the snapshot")?;
                    if nodes.is_empty() || replace.insert(i, nodes).is_some() {
                        return Err("overlapping ops");
                    }
                }
                Op::Children { backend, children: list } => {
                    let i = *by_backend.get(&backend).ok_or("parent not in the snapshot")?;
                    if children.insert(i, list).is_some() {
                        return Err("overlapping ops");
                    }
                }
                Op::Clickable { backend, on } => {
                    let i = *by_backend.get(&backend).ok_or("listener node not in the snapshot")?;
                    facts.clickable[i] = on;
                }
            }
        }
        let (ops_replace, ops_children) = (replace.len(), children.len());
        // What a node's subtree holds that an op names: such a subtree is
        // walked node by node, any other copied as one run.
        let mut dirty = vec![false; if preorder { n } else { 0 }];
        for &t in replace.keys().chain(children.keys()).filter(|_| preorder) {
            let mut at = t as i64;
            while at >= 0 && !dirty[at as usize] {
                dirty[at as usize] = true;
                at = old.parent[at as usize];
            }
        }
        let mut out = SnapDoc { url: old.url, base: old.base, title: old.title, ..SnapDoc::default() };
        if preorder {
            out.parent.reserve(n);
            out.ty.reserve(n);
            out.name.reserve(n);
            out.value.reserve(n);
            out.backend.reserve(n);
            out.attrs.reserve(n);
            out.layout.reserve(old.layout.len());
        }
        let mut interned: HashMap<String, i64> = HashMap::new();
        let strings = &mut self.strings;
        let mut intern = |s: &str| -> i64 {
            if let Some(&i) = interned.get(s) {
                return i;
            }
            let i = strings.len() as i64;
            strings.push(s.to_string());
            interned.insert(s.to_string(), i);
            i
        };
        enum Item {
            Old(usize, i64),
            New(Vec<NewNode>, i64),
        }
        let mut emitted = vec![false; n];
        let (mut applied_replace, mut applied_children) = (0, 0);
        let mut stack = vec![Item::Old(0, -1)];
        while let Some(item) = stack.pop() {
            match item {
                Item::Old(i, parent) if preorder && i < n && !dirty[i] => {
                    let end = kids.end(i);
                    if emitted[i..end].iter().any(|&e| e) {
                        return Err("a node twice");
                    }
                    emitted[i..end].fill(true);
                    let at = out.parent.len();
                    let offset = at as i64 - i as i64;
                    out.parent.push(parent);
                    out.parent.extend(old.parent[i + 1..end].iter().map(|&p| p + offset));
                    out.ty.extend_from_slice(&old.ty[i..end]);
                    out.name.extend_from_slice(&old.name[i..end]);
                    out.value.extend((i..end).map(|k| old.value.get(k).copied().unwrap_or(-1)));
                    out.backend.extend_from_slice(&old.backend[i..end]);
                    out.attrs.extend((i..end).map(|k| old.attrs.get_mut(k).map(std::mem::take).unwrap_or_default()));
                    for k in i..end {
                        facts.copy(k, (k as i64 + offset) as usize, &old.layout, &mut out);
                    }
                }
                Item::Old(i, parent) => {
                    if i >= n || std::mem::replace(&mut emitted[i], true) {
                        return Err("a node twice");
                    }
                    if let Some(nodes) = replace.remove(&i) {
                        applied_replace += 1;
                        stack.push(Item::New(nodes, parent));
                        // Its old subtree is gone with it.
                        kids.mark_subtree(i, &mut emitted);
                        continue;
                    }
                    let at = out.parent.len();
                    out.parent.push(parent);
                    out.ty.push(old.ty[i]);
                    out.name.push(old.name[i]);
                    out.value.push(old.value.get(i).copied().unwrap_or(-1));
                    out.backend.push(old.backend[i]);
                    out.attrs.push(old.attrs.get_mut(i).map(std::mem::take).unwrap_or_default());
                    facts.copy(i, at, &old.layout, &mut out);
                    let at = at as i64;
                    match children.remove(&i) {
                        Some(list) => {
                            applied_children += 1;
                            // The element's own pseudo-elements stay first.
                            let mut items: Vec<Item> = kids
                                .of_node(i)
                                .iter()
                                .filter(|&&k| facts.pseudo[k].is_some())
                                .map(|&k| Item::Old(k, at))
                                .collect();
                            for child in list {
                                items.push(match child {
                                    Child::Keep(b) => {
                                        Item::Old(*by_backend.get(&b).ok_or("kept child not in the snapshot")?, at)
                                    }
                                    Child::New(nodes) => Item::New(nodes, at),
                                });
                            }
                            // Old children not kept are gone.
                            let kept: FxSet<usize> = items
                                .iter()
                                .filter_map(|x| match x {
                                    Item::Old(j, _) => Some(*j),
                                    Item::New(..) => None,
                                })
                                .collect();
                            for &k in kids.of_node(i) {
                                if facts.pseudo[k].is_none() && !kept.contains(&k) {
                                    if emitted[k] {
                                        return Err("a node twice");
                                    }
                                    emitted[k] = true;
                                    kids.mark_subtree(k, &mut emitted);
                                }
                            }
                            stack.extend(items.into_iter().rev());
                        }
                        None => stack.extend(kids.of_node(i).iter().rev().map(|&k| Item::Old(k, at))),
                    }
                }
                Item::New(nodes, parent) => {
                    let base = out.parent.len();
                    for (k, node) in nodes.into_iter().enumerate() {
                        let at = base + k;
                        let p = if node.parent == usize::MAX {
                            parent
                        } else if node.parent < k {
                            (base + node.parent) as i64
                        } else {
                            return Err("new nodes out of order");
                        };
                        out.parent.push(p);
                        out.ty.push(node.ty);
                        out.name.push(intern(&node.name));
                        out.value.push(node.value.as_deref().map_or(-1, &mut intern));
                        out.backend.push(node.backend);
                        out.attrs.push(node.attrs.iter().flat_map(|(k, v)| [intern(k), intern(v)]).collect());
                        if let Some(v) = &node.pseudo {
                            out.pseudo_type.push((at, intern(v)));
                        }
                        if let Some(v) = &node.input_value {
                            out.input_value.push((at, intern(v)));
                        }
                        if let Some(v) = &node.text_value {
                            out.text_value.push((at, intern(v)));
                        }
                        if node.checked {
                            out.input_checked.push(at);
                        }
                        if node.selected {
                            out.option_selected.push(at);
                        }
                        if node.clickable {
                            out.clickable.push(at);
                        }
                        if let Some(l) = &node.layout {
                            let mut styles = [-1; STYLES];
                            for (s, v) in styles.iter_mut().zip(&l.styles) {
                                *s = intern(v);
                            }
                            out.layout.push(Layout {
                                node: at,
                                text: l.text.as_deref().map_or(-1, &mut intern),
                                styles,
                                bounds: l.bounds,
                            });
                        }
                    }
                }
            }
        }
        if applied_replace != ops_replace || applied_children != ops_children {
            return Err("an op's node is not in the tree");
        }
        self.docs[0] = out;
        Ok(self)
    }
}

/// A document's per-node facts besides its columns, by node index.
struct Facts {
    shadow: Vec<Option<i64>>,
    input_value: Vec<Option<i64>>,
    text_value: Vec<Option<i64>>,
    pseudo: Vec<Option<i64>>,
    content_document: Vec<Option<i64>>,
    checked: Vec<bool>,
    selected: Vec<bool>,
    clickable: Vec<bool>,
    /// Layout objects by node (most nodes have none or one).
    first_layout: Vec<usize>,
    more_layout: FxMap<usize, Vec<usize>>,
}

impl Facts {
    /// Old node `i`'s facts, as new node `at`'s.
    fn copy(&self, i: usize, at: usize, layout: &[Layout], out: &mut SnapDoc) {
        if let Some(v) = self.shadow[i] {
            out.shadow_root_type.push((at, v));
        }
        if let Some(v) = self.input_value[i] {
            out.input_value.push((at, v));
        }
        if let Some(v) = self.text_value[i] {
            out.text_value.push((at, v));
        }
        if let Some(v) = self.pseudo[i] {
            out.pseudo_type.push((at, v));
        }
        if let Some(v) = self.content_document[i] {
            out.content_document.push((at, v));
        }
        if self.checked[i] {
            out.input_checked.push(at);
        }
        if self.selected[i] {
            out.option_selected.push(at);
        }
        if self.clickable[i] {
            out.clickable.push(at);
        }
        if self.first_layout[i] != usize::MAX {
            let mut l = layout[self.first_layout[i]].clone();
            l.node = at;
            out.layout.push(l);
            for &k in self.more_layout.get(&i).into_iter().flatten() {
                let mut l = layout[k].clone();
                l.node = at;
                out.layout.push(l);
            }
        }
    }
}

/// Whether parent indices list the tree in preorder: each node's parent
/// is the nearest node before it still open, so every subtree is one run
/// of indices, its children in index order.
fn is_preorder(parent: &[i64]) -> bool {
    let mut open: Vec<i64> = Vec::new();
    for (i, &p) in parent.iter().enumerate() {
        if p < 0 {
            if i > 0 {
                return false;
            }
        } else {
            while open.last().is_some_and(|&top| top != p) {
                open.pop();
            }
            if open.is_empty() {
                return false;
            }
        }
        open.push(i as i64);
    }
    true
}

/// Each node's children, in order, in one array.
struct Kids {
    start: Vec<usize>,
    list: Vec<usize>,
}

impl Kids {
    /// From parent indices in tree order (a parent before its children).
    fn of(parent: &[i64]) -> Result<Kids, &'static str> {
        let n = parent.len();
        let mut start = vec![0usize; n + 1];
        for (i, &p) in parent.iter().enumerate() {
            if p >= 0 {
                if p as usize >= i {
                    return Err("not in tree order");
                }
                start[p as usize + 1] += 1;
            }
        }
        for i in 0..n {
            start[i + 1] += start[i];
        }
        let mut fill = start.clone();
        let mut list = vec![0usize; start[n]];
        for (i, &p) in parent.iter().enumerate() {
            if p >= 0 {
                list[fill[p as usize]] = i;
                fill[p as usize] += 1;
            }
        }
        Ok(Kids { start, list })
    }

    fn of_node(&self, i: usize) -> &[usize] {
        &self.list[self.start[i]..self.start[i + 1]]
    }

    /// One past the last node of `i`'s subtree, in a preorder document.
    fn end(&self, mut i: usize) -> usize {
        while let Some(&last) = self.of_node(i).last() {
            i = last;
        }
        i + 1
    }

    /// Mark the subtree under `i` (not `i`) as done with.
    fn mark_subtree(&self, i: usize, emitted: &mut [bool]) {
        let mut stack: Vec<usize> = self.of_node(i).to_vec();
        while let Some(k) = stack.pop() {
            emitted[k] = true;
            stack.extend_from_slice(self.of_node(k));
        }
    }
}

impl SnapDoc {
    /// The DOM children of node `i`, in order: not its pseudo-elements.
    /// None for a shadow host, whose shadow tree the snapshot lists among
    /// them.
    pub fn dom_children(&self, i: usize) -> Option<Vec<usize>> {
        let marked = |list: &[(usize, i64)], k: usize| list.binary_search_by_key(&k, |&(n, _)| n).is_ok();
        let mut out = Vec::new();
        for k in i + 1..self.parent.len() {
            if self.parent[k] != i as i64 {
                continue;
            }
            if marked(&self.shadow_root_type, k) {
                return None;
            }
            if !marked(&self.pseudo_type, k) {
                out.push(k);
            }
        }
        Some(out)
    }

    /// Which nodes a display lock skips (see [`Snap::from_value`]), given
    /// the elements with `content-visibility: hidden` and the string
    /// indices of "DETAILS", "SUMMARY" and "open" (-1 for none).
    fn locked(&self, cv_hidden: &[usize], details: i64, summary: i64, open: i64) -> Vec<bool> {
        let n = self.parent.len();
        let mut hides = vec![false; n];
        for &i in cv_hidden {
            if i < n {
                hides[i] = true;
            }
        }
        let mut pseudo = vec![false; n];
        for &(i, _) in &self.pseudo_type {
            if i < n {
                pseudo[i] = true;
            }
        }
        // Whether a <details> is closed, and the closed ones whose summary
        // was seen: only the first summary child is shown.
        let mut closed: FxMap<usize, bool> = FxMap::default();
        let mut summarized: FxSet<usize> = FxSet::default();
        let mut locked = vec![false; n];
        for i in 0..n {
            let p = self.parent[i];
            if p < 0 || p as usize >= i {
                continue;
            }
            let p = p as usize;
            if locked[p] || hides[p] {
                locked[i] = true;
                continue;
            }
            if details < 0 || self.name[p] != details || pseudo[i] {
                continue;
            }
            let shut = *closed.entry(p).or_insert_with(|| !self.attrs[p].chunks(2).any(|a| a[0] == open));
            if !shut {
                continue;
            }
            let is_summary = summary >= 0 && self.ty[i] == 1 && self.name[i] == summary;
            if !(is_summary && summarized.insert(p)) {
                locked[i] = true;
            }
        }
        locked
    }

    /// [`SnapDoc::from_value`] from the typed answer: the same columns,
    /// moved instead of copied.
    fn from_wire(doc: wire::Doc, hidden: i64) -> (SnapDoc, Vec<usize>) {
        let wire::Doc { document_url, base_url, title, nodes: n, layout: l } = doc;
        let count = n.parent_index.len();
        let mut attrs = n.attributes;
        attrs.resize_with(count, Vec::new);
        let mut cv_hidden = Vec::new();
        let (styles, bounds) = (l.styles.0, l.bounds.0);
        let layout = l
            .node_index
            .into_iter()
            .enumerate()
            .filter(|(_, node)| *node >= 0)
            .map(|(k, node)| {
                let mut st = [-1; STYLES];
                if let Some(s) = styles.get(k) {
                    st.copy_from_slice(&s[..STYLES]);
                    if hidden >= 0 && s[STYLES] == hidden {
                        cv_hidden.push(node as usize);
                    }
                }
                let b = bounds.get(k).copied().unwrap_or([0.0; 4]);
                Layout { node: node as usize, text: l.text.get(k).copied().unwrap_or(-1), styles: st, bounds: b }
            })
            .collect();
        let doc = SnapDoc {
            url: document_url,
            base: base_url,
            title,
            ty: n.node_type,
            name: n.node_name,
            value: n.node_value,
            backend: n.backend_node_id,
            attrs,
            shadow_root_type: n.shadow_root_type.pairs(),
            input_value: n.input_value.pairs(),
            text_value: n.text_value.pairs(),
            pseudo_type: n.pseudo_type.pairs(),
            content_document: n.content_document_index.pairs(),
            input_checked: n.input_checked.nodes(),
            option_selected: n.option_selected.nodes(),
            clickable: n.is_clickable.nodes(),
            layout,
            parent: n.parent_index,
        };
        (doc, cv_hidden)
    }

    /// The document's columns, and the elements whose `content-visibility`
    /// (the style after [`STYLES`]) is the string `hidden`.
    fn from_value(doc: &Value, hidden: i64) -> (SnapDoc, Vec<usize>) {
        let n = &doc["nodes"];
        let parent = ints(n, "parentIndex");
        let count = parent.len();
        let attrs = match n.get("attributes").and_then(Value::as_array) {
            Some(all) => (0..count)
                .map(|i| {
                    all.get(i)
                        .and_then(Value::as_array)
                        .map(|a| a.iter().map(|x| x.as_i64().unwrap_or(-1)).collect())
                        .unwrap_or_default()
                })
                .collect(),
            None => vec![Vec::new(); count],
        };
        let l = &doc["layout"];
        let styles = l.get("styles").and_then(Value::as_array);
        let bounds = l.get("bounds").and_then(Value::as_array);
        let text = ints(l, "text");
        let mut cv_hidden = Vec::new();
        let layout = ints(l, "nodeIndex")
            .into_iter()
            .enumerate()
            .filter(|(_, node)| *node >= 0)
            .map(|(k, node)| {
                let mut st = [-1; STYLES];
                if let Some(s) = styles.and_then(|s| s.get(k)).and_then(Value::as_array) {
                    for (slot, v) in st.iter_mut().zip(s) {
                        *slot = v.as_i64().unwrap_or(-1);
                    }
                    if hidden >= 0 && s.get(STYLES).and_then(Value::as_i64) == Some(hidden) {
                        cv_hidden.push(node as usize);
                    }
                }
                let mut b = [0.0; 4];
                if let Some(r) = bounds.and_then(|s| s.get(k)).and_then(Value::as_array) {
                    for (slot, v) in b.iter_mut().zip(r) {
                        *slot = v.as_f64().unwrap_or(0.0);
                    }
                }
                Layout { node: node as usize, text: text.get(k).copied().unwrap_or(-1), styles: st, bounds: b }
            })
            .collect();
        let index = |key: &str| doc.get(key).and_then(Value::as_i64).unwrap_or(-1);
        let doc = SnapDoc {
            url: index("documentURL"),
            base: index("baseURL"),
            title: index("title"),
            ty: ints(n, "nodeType"),
            name: ints(n, "nodeName"),
            value: ints(n, "nodeValue"),
            backend: ints(n, "backendNodeId"),
            attrs,
            shadow_root_type: rare(n, "shadowRootType"),
            input_value: rare(n, "inputValue"),
            text_value: rare(n, "textValue"),
            pseudo_type: rare(n, "pseudoType"),
            content_document: rare(n, "contentDocumentIndex"),
            input_checked: rare_bool(n, "inputChecked"),
            option_selected: rare_bool(n, "optionSelected"),
            clickable: rare_bool(n, "isClickable"),
            layout,
            parent,
        };
        (doc, cv_hidden)
    }
}

/// A node read from the page for a patch: its parent is an earlier node
/// of the same list, or `usize::MAX` for the node the list replaces or
/// adds (whose parent the op gives).
#[derive(Clone, Debug, Default)]
pub struct NewNode {
    pub parent: usize,
    pub ty: i64,
    pub name: String,
    pub value: Option<String>,
    pub backend: i64,
    pub attrs: Vec<(String, String)>,
    pub pseudo: Option<String>,
    pub input_value: Option<String>,
    pub text_value: Option<String>,
    pub checked: bool,
    pub selected: bool,
    pub clickable: bool,
    pub layout: Option<NewLayout>,
}

#[derive(Clone, Debug, Default)]
pub struct NewLayout {
    pub text: Option<String>,
    pub styles: [String; STYLES],
    pub bounds: [f64; 4],
}

/// What changed in the main document since the snapshot.
#[derive(Debug)]
pub enum Op {
    /// The node and its subtree, read again (the list's first node is it).
    Replace { backend: i64, nodes: Vec<NewNode> },
    /// The node's children are now these (its pseudo-elements stay).
    Children { backend: i64, children: Vec<Child> },
    /// A listener for clicks was added to the node, or the last one taken off.
    Clickable { backend: i64, on: bool },
}

#[derive(Debug)]
pub enum Child {
    /// A child the snapshot holds, with its subtree.
    Keep(i64),
    /// A new child, its subtree in preorder.
    New(Vec<NewNode>),
}

/// The parts of a `DOMSnapshot.captureSnapshot` answer [`Snap`] keeps,
/// typed, so the JSON text is read straight into them: as JSON values, a
/// 10,000-row table's 7 MB took ~50 ms to read and as much again to copy
/// out of. What isn't named here (text boxes) is skipped unread. A field
/// that isn't there is empty, as [`Snap::from_value`] reads it; a value of
/// another type fails the read (the caller then reads values).
mod wire {
    use serde::de::{Deserializer, IgnoredAny, SeqAccess, Visitor};
    use serde::Deserialize;

    use super::STYLES;

    #[derive(Deserialize)]
    pub struct Capture {
        pub documents: Vec<Doc>,
        pub strings: Vec<String>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Doc {
        #[serde(rename = "documentURL", default = "none")]
        pub document_url: i64,
        #[serde(rename = "baseURL", default = "none")]
        pub base_url: i64,
        #[serde(default = "none")]
        pub title: i64,
        #[serde(default)]
        pub nodes: Nodes,
        #[serde(default)]
        pub layout: Layout,
    }

    fn none() -> i64 {
        -1
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub struct Nodes {
        pub parent_index: Vec<i64>,
        pub node_type: Vec<i64>,
        pub node_name: Vec<i64>,
        pub node_value: Vec<i64>,
        pub backend_node_id: Vec<i64>,
        pub attributes: Vec<Vec<i64>>,
        pub shadow_root_type: Rare,
        pub input_value: Rare,
        pub text_value: Rare,
        pub pseudo_type: Rare,
        pub content_document_index: Rare,
        pub input_checked: Rare,
        pub option_selected: Rare,
        pub is_clickable: Rare,
    }

    /// Sparse data: the nodes that have it, and (not for flags) its value.
    #[derive(Deserialize, Default)]
    #[serde(default)]
    pub struct Rare {
        index: Vec<i64>,
        value: Vec<i64>,
    }

    impl Rare {
        pub fn pairs(self) -> Vec<(usize, i64)> {
            self.index.into_iter().zip(self.value).filter(|(i, _)| *i >= 0).map(|(i, v)| (i as usize, v)).collect()
        }

        pub fn nodes(self) -> Vec<usize> {
            self.index.into_iter().filter(|i| *i >= 0).map(|i| i as usize).collect()
        }
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub struct Layout {
        pub node_index: Vec<i64>,
        pub styles: Rows<i64, { STYLES + 1 }>,
        pub bounds: Rows<f64, 4>,
        pub text: Vec<i64>,
    }

    /// What a row's missing places hold.
    pub trait Fill: Copy {
        const FILL: Self;
    }

    impl Fill for i64 {
        const FILL: i64 = -1;
    }

    impl Fill for f64 {
        const FILL: f64 = 0.0;
    }

    /// A list of short lists, each held as a fixed row: no allocation per
    /// row. Places past `W` are skipped, missing ones hold [`Fill::FILL`].
    #[derive(Default)]
    pub struct Rows<T, const W: usize>(pub Vec<[T; W]>);

    impl<'de, T: Fill + Deserialize<'de>, const W: usize> Deserialize<'de> for Rows<T, W> {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct Outer<T, const W: usize>(std::marker::PhantomData<T>);
            struct Row<T, const W: usize>(std::marker::PhantomData<T>);
            struct Cells<T, const W: usize>([T; W]);
            impl<'de, T: Fill + Deserialize<'de>, const W: usize> Visitor<'de> for Row<T, W> {
                type Value = Cells<T, W>;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("a list")
                }
                fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                    let mut row = [T::FILL; W];
                    for slot in row.iter_mut() {
                        match seq.next_element()? {
                            Some(v) => *slot = v,
                            None => return Ok(Cells(row)),
                        }
                    }
                    while seq.next_element::<IgnoredAny>()?.is_some() {}
                    Ok(Cells(row))
                }
            }
            impl<'de, T: Fill + Deserialize<'de>, const W: usize> Deserialize<'de> for Cells<T, W> {
                fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                    d.deserialize_seq(Row::<T, W>(std::marker::PhantomData))
                }
            }
            impl<'de, T: Fill + Deserialize<'de>, const W: usize> Visitor<'de> for Outer<T, W> {
                type Value = Rows<T, W>;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("a list of lists")
                }
                fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                    let mut rows = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                    while let Some(Cells(row)) = seq.next_element::<Cells<T, W>>()? {
                        rows.push(row);
                    }
                    Ok(Rows(rows))
                }
            }
            d.deserialize_seq(Outer::<T, W>(std::marker::PhantomData))
        }
    }
}

#[cfg(test)]
mod tests {
    /// A patch copying untouched subtrees whole is node for node the one
    /// walking every node, errors included: random ops on every snapshot
    /// in `NAVIGERA_SNAP_DIR`.
    #[test]
    #[ignore]
    fn bulk_patch_is_the_node_walk() {
        use super::*;
        let dir = std::env::var("NAVIGERA_SNAP_DIR").expect("NAVIGERA_SNAP_DIR");
        let mut files: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
        files.sort();
        let (mut same, mut errs) = (0, 0);
        for f in files {
            let value: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
            let snap = Snap::from_value(&value).unwrap();
            let d = &snap.docs[0];
            let n = d.parent.len();
            assert!(is_preorder(&d.parent), "{}: capture not in preorder", f.display());
            for seed in 0..40u64 {
                let kids = Kids::of(&d.parent).unwrap();
                let text = |b: i64| NewNode {
                    parent: usize::MAX,
                    ty: 3,
                    name: "#text".into(),
                    value: Some(format!("t{b}")),
                    backend: b,
                    ..Default::default()
                };
                let make = |seed: u64| -> Vec<Op> {
                    let mut r = seed.wrapping_add(1);
                    let mut next = |m: usize| {
                        r = r.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                        ((r >> 33) as usize) % m.max(1)
                    };
                    let mut ops = Vec::new();
                    for _ in 0..1 + next(4) {
                        let i = next(n);
                        let b = d.backend[i];
                        match next(3) {
                            0 => ops.push(Op::Replace { backend: b, nodes: vec![text(b)] }),
                            1 => {
                                let mut list: Vec<Child> = kids
                                    .of_node(i)
                                    .iter()
                                    .filter(|&&k| !d.pseudo_type.iter().any(|&(p, _)| p == k))
                                    .map(|&k| Child::Keep(d.backend[k]))
                                    .collect();
                                if next(2) == 0 {
                                    list.reverse();
                                }
                                if !list.is_empty() && next(3) == 0 {
                                    list.remove(next(list.len()));
                                }
                                let at = next(list.len() + 1);
                                list.insert(at, Child::New(vec![text(-7 - i as i64)]));
                                ops.push(Op::Children { backend: b, children: list });
                            }
                            _ => ops.push(Op::Clickable { backend: b, on: next(2) == 0 }),
                        }
                    }
                    ops
                };
                let (ops, ops2) = (make(seed), make(seed));
                let bulk = snap.clone().patch_with(ops, true);
                let walk = snap.clone().patch_with(ops2, false);
                match (&bulk, &walk) {
                    (Ok(a), Ok(b)) => {
                        assert!(format!("{a:?}") == format!("{b:?}"), "{}: seed {seed} differs", f.display());
                        assert!(is_preorder(&a.docs[0].parent), "{}: patched not in preorder", f.display());
                        same += 1;
                    }
                    (Err(a), Err(b)) => {
                        assert_eq!(a, b, "{}: seed {seed}", f.display());
                        errs += 1;
                    }
                    _ => panic!("{}: seed {seed}: {:?} vs {:?}", f.display(), bulk.is_ok(), walk.is_ok()),
                }
            }
        }
        eprintln!("{same} patches equal, {errs} errors equal");
    }

    use super::*;
    use serde_json::json;

    /// html > body > [div#a > "x", p > "y"]
    /// The snapshot read from values and from its text, which must agree.
    fn read_both(v: &Value) -> Snap {
        let from_value = Snap::from_value(v).unwrap();
        let from_json = Snap::from_json(&v.to_string()).expect("typed read");
        assert_eq!(format!("{from_json:?}"), format!("{from_value:?}"), "typed read differs");
        from_json
    }

    /// What the typed read fills in or skips, as the value read does: no
    /// attributes or layout, short and long style rows, a missing row.
    #[test]
    fn the_typed_read_takes_what_the_value_read_takes() {
        let v = json!({
            "strings": ["#document", "HTML", "hidden", "x"],
            "documents": [{
                "nodes": {
                    "parentIndex": [-1, 0, 1],
                    "nodeType": [9, 1, 1],
                    "nodeName": [0, 1, 1],
                    "backendNodeId": [1, 2, 3],
                    "attributes": [[], [3, 3]],
                    "inputChecked": { "index": [2] },
                    "textValue": { "index": [1, -1], "value": [3, 3] }
                },
                "layout": {
                    "nodeIndex": [0, 1, -1, 2],
                    "styles": [[0], [0, 1, 2, 3, 0, 1, 2, 9, 9], [], [3, 3, 3, 3, 3, 3, 2]],
                    "bounds": [[1, 2.5, 3, 4, 5]],
                    "text": [3]
                },
                "textBoxes": { "layoutIndex": [0], "bounds": [[0, 0, 1, 1]], "start": [0], "length": [1] }
            }, { "documentURL": 3 }]
        });
        let s = read_both(&v);
        // Node 1's `content-visibility` is "hidden": node 2's layout goes.
        assert_eq!(s.docs[0].layout.iter().map(|l| l.node).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(s.docs[0].layout[0].bounds, [1.0, 2.5, 3.0, 4.0]);
        assert_eq!(s.docs[0].attrs, [vec![], vec![3, 3], vec![]]);
        assert_eq!(s.docs[1].url, 3);
        // Not a capture's shape: the typed read declines.
        assert!(Snap::from_json(r#"{"strings": ["a"], "documents": [{"nodes": {"parentIndex": ["x"]}}]}"#).is_none());
    }

    fn base() -> Snap {
        let v = json!({
            "strings": ["#document", "HTML", "BODY", "DIV", "#text", "x", "P", "y", "id", "a", "block", "auto"],
            "documents": [{
                "documentURL": -1, "baseURL": -1, "title": -1,
                "nodes": {
                    "parentIndex": [-1, 0, 1, 2, 3, 2, 5],
                    "nodeType": [9, 1, 1, 1, 3, 1, 3],
                    "nodeName": [0, 1, 2, 3, 4, 6, 4],
                    "nodeValue": [-1, -1, -1, -1, 5, -1, 7],
                    "backendNodeId": [1, 2, 3, 4, 5, 6, 7],
                    "attributes": [[], [], [], [8, 9], [], [], []],
                    "isClickable": { "index": [5] }
                },
                "layout": {
                    "nodeIndex": [0, 1, 2, 3, 4, 5, 6],
                    "styles": [[], [], [], [11, 11, 11, 11, 10, 11], [], [], []],
                    "text": [-1, -1, -1, -1, 5, -1, 7],
                    "bounds": [[0, 0, 1, 1], [0, 0, 1, 1], [0, 0, 1, 1], [0, 0, 1, 1], [0, 0, 1, 1], [0, 0, 1, 1], [0, 0, 1, 1]]
                }
            }]
        });
        read_both(&v)
    }

    fn text(parent: usize, backend: i64, value: &str) -> NewNode {
        NewNode {
            parent,
            ty: 3,
            name: "#text".into(),
            value: Some(value.into()),
            backend,
            layout: Some(NewLayout { text: Some(value.into()), ..Default::default() }),
            ..Default::default()
        }
    }

    fn texts(s: &Snap) -> Vec<String> {
        let d = &s.docs[0];
        (0..d.parent.len()).filter(|&i| d.ty[i] == 3).map(|i| s.at(d.value[i]).to_string()).collect()
    }

    #[test]
    fn a_replaced_text_node_reads_its_new_text() {
        let s = base().patch(vec![Op::Replace { backend: 5, nodes: vec![text(usize::MAX, 5, "x2")] }]).unwrap();
        assert_eq!(texts(&s), ["x2", "y"]);
        let d = &s.docs[0];
        assert_eq!(d.parent, [-1, 0, 1, 2, 3, 2, 5]);
        assert_eq!(d.layout.len(), 7);
        assert_eq!(d.clickable, [5]);
    }

    #[test]
    fn children_keep_their_subtrees_and_new_ones_go_where_listed() {
        let div = NewNode { parent: usize::MAX, ty: 1, name: "DIV".into(), backend: 9, ..Default::default() };
        let s = base()
            .patch(vec![Op::Children {
                backend: 3,
                children: vec![Child::Keep(6), Child::New(vec![div, text(0, 10, "new")])],
            }])
            .unwrap();
        // div#a is gone; p kept before the new div.
        assert_eq!(texts(&s), ["y", "new"]);
        let d = &s.docs[0];
        assert_eq!(d.backend, [1, 2, 3, 6, 7, 9, 10]);
        assert_eq!(d.parent, [-1, 0, 1, 2, 3, 2, 5]);
        // p's layout and its click flag moved with it.
        assert_eq!(d.clickable, [3]);
        assert!(d.layout.iter().any(|l| l.node == 3));
        assert!(!d.layout.iter().any(|l| l.node == 5));
    }

    #[test]
    fn ops_on_nodes_the_snapshot_lacks_fail() {
        assert!(base().patch(vec![Op::Replace { backend: 99, nodes: vec![text(usize::MAX, 99, "z")] }]).is_err());
        assert!(base().patch(vec![Op::Children { backend: 3, children: vec![Child::Keep(99)] }]).is_err());
        // A node kept twice.
        assert!(base()
            .patch(vec![Op::Children { backend: 3, children: vec![Child::Keep(6), Child::Keep(6)] }])
            .is_err());
        // An op inside a replaced subtree never applies.
        assert!(base()
            .patch(vec![
                Op::Replace { backend: 4, nodes: vec![text(usize::MAX, 4, "z")] },
                Op::Replace { backend: 5, nodes: vec![text(usize::MAX, 5, "z")] },
            ])
            .is_err());
    }

    /// html > body > [details > [summary > "s", p > "b", summary, ::after],
    /// div > "d"], every node laid out (Chrome keeps the boxes of content
    /// it rendered before a lock), the div's `content-visibility` hidden.
    fn locked(open: bool) -> Snap {
        let attrs = if open { json!([12, 13]) } else { json!([]) };
        let v = json!({
            "strings": ["#document", "HTML", "BODY", "DETAILS", "SUMMARY", "#text", "s", "P", "b", "::after",
                        "DIV", "d", "open", "", "hidden", "visible", "after"],
            "documents": [{
                "documentURL": -1, "baseURL": -1, "title": -1,
                "nodes": {
                    "parentIndex": [-1, 0, 1, 2, 3, 4, 3, 6, 3, 3, 2, 10],
                    "nodeType": [9, 1, 1, 1, 1, 3, 1, 3, 1, 1, 1, 3],
                    "nodeName": [0, 1, 2, 3, 4, 5, 7, 5, 4, 9, 10, 5],
                    "nodeValue": [-1, -1, -1, -1, -1, 6, -1, 8, -1, -1, -1, 11],
                    "backendNodeId": [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
                    "attributes": [[], [], [], attrs, [], [], [], [], [], [], [], []],
                    "pseudoType": { "index": [9], "value": [16] }
                },
                "layout": {
                    "nodeIndex": [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
                    "styles": (0..12).map(|i| {
                        let cv = if i == 10 { 14 } else { 15 };
                        json!([-1, -1, -1, -1, -1, -1, cv])
                    }).collect::<Vec<_>>(),
                    "text": [-1, -1, -1, -1, -1, 6, -1, 8, -1, -1, -1, 11],
                    "bounds": (0..12).map(|_| json!([0, 0, 1, 1])).collect::<Vec<_>>()
                }
            }]
        });
        read_both(&v)
    }

    #[test]
    fn what_a_display_lock_skips_has_no_layout() {
        let laid = |s: &Snap| s.docs[0].layout.iter().map(|l| l.node).collect::<Vec<_>>();
        // Closed: the first summary and the details' own ::after stay; its
        // body and a second summary go; so does what the hidden div holds.
        assert_eq!(laid(&locked(false)), [0, 1, 2, 3, 4, 5, 9, 10]);
        assert_eq!(laid(&locked(true)), [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    }

    #[test]
    fn a_listener_flag_moves_with_its_node() {
        let s = base()
            .patch(vec![Op::Clickable { backend: 4, on: true }, Op::Clickable { backend: 6, on: false }])
            .unwrap();
        assert_eq!(s.docs[0].clickable, [3]);
    }
}
