(max) => {
    // The page read as DOMSnapshot.captureSnapshot's columns, so the tree
    // builder and the printer that read Chrome's snapshot read this one the
    // same way. Node ids come from a page-side table (kept per window):
    // short integers that later ops look up again (see REF_EL).
    const T = window[Symbol.for('navigera.ids')] || (window[Symbol.for('navigera.ids')] = { next: 1, byNode: new WeakMap(), byId: new Map() });
    const idOf = (n) => {
        let id = T.byNode.get(n);
        if (id === undefined) { id = T.next++; T.byNode.set(n, id); T.byId.set(id, new WeakRef(n)); }
        return id;
    };
    const listening = window[Symbol.for('navigera.listen')];
    const strings = [], index = new Map();
    const str = (s) => {
        if (s === null || s === undefined) return -1;
        s = String(s);
        let i = index.get(s);
        if (i === undefined) { i = strings.length; strings.push(s); index.set(s, i); }
        return i;
    };
    const STYLES = ['cursor', 'pointer-events', 'opacity', 'visibility', 'display', 'content', 'content-visibility'];
    const documents = [];
    let count = 0, truncated = false;
    const rare = () => ({ index: [], value: [] });
    const flags = () => ({ index: [] });
    const walkDoc = (doc) => {
        const d = {
            documentURL: str(doc.URL), baseURL: str(doc.baseURI), title: str(doc.title),
            nodes: {
                parentIndex: [], nodeType: [], nodeName: [], nodeValue: [], backendNodeId: [], attributes: [],
                shadowRootType: rare(), inputValue: rare(), textValue: rare(), contentDocumentIndex: rare(),
                inputChecked: flags(), optionSelected: flags(), isClickable: flags(),
            },
            layout: { nodeIndex: [], styles: [], bounds: [], text: [] },
        };
        documents.push(d);
        const N = d.nodes, L = d.layout;
        const view = doc.defaultView || window;
        const sx = view.scrollX || 0, sy = view.scrollY || 0;
        const frames = [];
        // (node, parent index, rendered): a stack, so a deep page can't
        // overflow the call stack.
        const stack = [[doc, -1, true]];
        while (stack.length) {
            const [n, parent, shown] = stack.pop();
            if (count >= max) { truncated = true; break; }
            count++;
            const i = N.parentIndex.length;
            N.parentIndex.push(parent);
            N.nodeType.push(n.nodeType);
            N.nodeName.push(str(n.nodeName));
            N.nodeValue.push(str(n.nodeType === 1 ? '' : n.nodeValue));
            N.backendNodeId.push(idOf(n));
            const attrs = [];
            let rendered = false;
            if (n.nodeType === 1) {
                for (const a of n.attributes) attrs.push(str(a.name), str(a.value));
                const tag = n.tagName;
                if (tag === 'INPUT') {
                    N.inputValue.index.push(i); N.inputValue.value.push(str(n.value));
                    if (n.checked) N.inputChecked.index.push(i);
                } else if (tag === 'TEXTAREA') {
                    N.textValue.index.push(i); N.textValue.value.push(str(n.value));
                } else if (tag === 'OPTION') {
                    if (n.selected) N.optionSelected.index.push(i);
                }
                if ((listening && listening.has(n)) || n.hasAttribute('onclick') || n.hasAttribute('onmousedown')
                    || (tag === 'A' && n.hasAttribute('href'))) N.isClickable.index.push(i);
                if (shown) {
                    const cs = view.getComputedStyle(n);
                    const display = cs.display;
                    rendered = display !== 'none';
                    if (rendered && display !== 'contents') {
                        const r = n.getBoundingClientRect();
                        L.nodeIndex.push(i);
                        L.styles.push(STYLES.map((p) => str(cs.getPropertyValue(p))));
                        L.bounds.push([r.left + sx, r.top + sy, r.width, r.height]);
                        L.text.push(-1);
                    }
                    // A <select>'s options draw in its own box: no layout of their own.
                    if (tag === 'SELECT') rendered = false;
                }
                if ((tag === 'IFRAME' || tag === 'FRAME')) {
                    let cd = null;
                    try { cd = n.contentDocument; } catch (_) {}
                    if (cd && cd.documentElement) frames.push([i, cd]);
                }
            } else if (n.nodeType === 3) {
                if (shown && n.data.trim()) {
                    const p = n.parentElement;
                    const tt = p ? view.getComputedStyle(p).textTransform : 'none';
                    let t = n.data;
                    if (tt === 'uppercase') t = t.toUpperCase();
                    else if (tt === 'lowercase') t = t.toLowerCase();
                    else if (tt === 'capitalize') t = t.replace(/(^|\s)(\S)/g, (_, a, b) => a + b.toUpperCase());
                    L.nodeIndex.push(i);
                    L.styles.push([]);
                    L.bounds.push([0, 0, 0, 0]);
                    L.text.push(str(t));
                }
            } else if (n.nodeType === 9 || n.nodeType === 11) {
                rendered = shown;
            }
            N.attributes.push(attrs);
            if (n.nodeType === 11 && n.host) {
                N.shadowRootType.index.push(i); N.shadowRootType.value.push(str(n.mode || 'open'));
            }
            // Children in order: pushed last to first. A shadow root comes
            // before the light children it slots.
            const kids = n.childNodes;
            for (let k = kids.length - 1; k >= 0; k--) {
                const c = kids[k];
                if (c.nodeType === 1 || c.nodeType === 3 || c.nodeType === 10) stack.push([c, i, rendered]);
            }
            if (n.nodeType === 1 && n.shadowRoot) stack.push([n.shadowRoot, i, rendered]);
        }
        return frames;
    };
    // Same-origin frames follow the page, as Chrome's snapshot lists them.
    const queue = [[null, document]];
    while (queue.length && !truncated) {
        const [owner, doc] = queue.shift();
        const di = documents.length;
        if (owner) { owner[0].contentDocumentIndex.index.push(owner[1]); owner[0].contentDocumentIndex.value.push(di); }
        const frames = walkDoc(doc);
        const nodes = documents[di].nodes;
        for (const [i, cd] of frames) queue.push([[nodes, i], cd]);
    }
    return { documents, strings, truncated };
}
