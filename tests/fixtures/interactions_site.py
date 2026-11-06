#!/usr/bin/env python3
"""Fixture for the interactions that real practice sites showed navigera
getting wrong, one page each, built the way those sites behave:

/hidden-layers  a second click on a button now covered by another (the
                page notes a click a user couldn't have made)
/iframe-form    a form inside a same-origin iframe
/frameset       nested framesets with text in each frame
/srcdoc         a button inside nested srcdoc iframes
/multi-select   a <select multiple>
/nbsp-select    options whose labels have non-breaking spaces
/slow?ms=N      a button that appears after N ms
/download       a file sent as an attachment, and one made from a Blob
/shadow-css     icon-only buttons inside (nested) open shadow roots
/entry-modal    a modal that slides in and wires its Close button only
                once it has finished
/overlapped     an input whose centre is under a panel until the box
                scrolls; typing into a covered field clears it
/right-click    a button that reports right and double clicks
/wizard         a step that saves with fetch, then reloads into the next
/card-radios    radio cards whose real inputs are visually hidden
/combobox       a react-select-style dropdown: ArrowDown opens a listbox
                rendered at the end of <body>
/typed-only     an input whose app keeps only what was typed
/lazy-box       a scrollable box that loads more when scrolled to its end
/honesty        a decoy copy of an id, disabled and read-only controls
/leader         two tabs of one site: the one opened with ?leader tells the
                other on beforeunload, which takes over (a tab closed
                without its unload path leaves the follower waiting)
/poll           a page that polls one URL every 300 ms, forever
/long-poll      a chat-style long poll: one request held open (?hold=ms), reconnected
/beacons        analytics POSTs at irregular 100-900 ms intervals, answered 204
/one-fetch      "Loading..." until one fetch, which the server holds 400 ms, brings a list
/click-fetch    a button that starts a 200 ms fetch, then brings a list
/rerender       a list whose rows and Save button are drawn again (new elements)
                by Redraw; Add row adds a fourth, Route pushes a new URL
/legal          a 1,500-clause agreement (~130 KB of text), every 10th
                clause long enough to be cut in a snapshot
/two-fetch      a list in two halves: one fetch, a 400 ms gap, a second fetch
/big-table      a 2,000-row table, a button and a link in each row
/clicks         a 2,000-row page with a button per kind of change a click makes
                (a cell, a row added or removed, a menu a sibling rule shows, a
                dialog, a popover, a list drawn again, a listener, a value, a
                fade, a numbered list), and a closed <details>
/journal        a 1,300-row page with fields whose typing changes nothing
                else, a live count, a tip a style shows on focus, a hint a
                :has() rule hides once the field is filled, a field whose
                typing rewrites a style rule, a focus tip from an @imported
                sheet; ?closed: a field that changes a closed shadow root
/stale-ref      a button and a bare draggable=true handle (no handlers)
/drag-native    a list of draggable items that reorders on drop (reads dataTransfer)
/drag-pointer   the same list, each item made draggable on pointerdown (as
                React Aria does)
/drag-position  a drop box that reports where in it the drop (and the press) landed
/drag-slider    a knob that follows a held mouse (no drag starts)
/never-stops    a button that never stops drifting: the click still
                happens once the settle budget runs out
/moving-target  a button that glides to a stop over its first frames
                (no CSS animation), reporting where it was clicked
/div-cells      3,000 cells, each a <div> of its own text, in 300 <div> rows
/long-text      a <div> and a <p> of 1,120 characters each
/cookie-banner  a button under a banner with Accept and Reject that stays
/loading-cover  a button under an overlay with a Cancel button, removed when
                a 2.5 s fetch returns
/wired-late     a button wired only after load: by a timer (?ms=N), by a
                script the page loads then (?script=N), by a listener its
                container gets (?root=N) or the document gets (?doc=N);
                ?analytics: a document listener from the start; ?replay=N:
                clicks recorded at the document before Go is wired, replayed
                once it is (&late: 50 ms after; &invisible: the replayed
                click shows nothing); ?quiet: a button that works
                from the start and shows nothing, beside a late listener
/hover-cover    a button whose first mouse move opens an overlay over it, with
                a Subscribe button, for 300 ms: the press that follows the
                move lands on the overlay
/click-off      a button whose page answers the press itself, so the click
                event goes to their shared parent, #w: ?replace re-renders
                the button on mousedown, ?banner puts a banner in above it
                on pointerdown; neither: a still button
/xo-parent      a card form in a cross-site iframe (localhost in a 127.0.0.1
                page: its own process); ?same-site: cross-origin, same process

Prints "LISTENING http://127.0.0.1:<port>" and serves until killed."""
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

HEAD = "<!doctype html><html><head><meta charset=utf-8><title>{title}</title><style>{style}</style></head><body>"

PAGES = {}



PAGES["/never-stops"] = ("never stops", "", """
<p id=status>waiting</p>
<button id=drifter style="position:absolute;left:200px;top:60px;width:60px;height:30px"
  onclick="document.getElementById('status').textContent='ok '+event.clientX+' '+event.clientY">D</button>
<script>
// A perpetual small drift, no CSS animation: the box never reads the same
// twice, so only the settle budget can end the wait - then the click must
// still land on the button (the drift is smaller than the button).
let dx = 0, dir = 1;
const drift = () => {
  dx += dir * 3;
  if (Math.abs(dx) >= 12) dir = -dir;
  document.getElementById('drifter').style.left = (200 + dx) + 'px';
  requestAnimationFrame(drift);
};
requestAnimationFrame(drift);
</script>""")

PAGES["/div-cells"] = ("cells", "", "".join(
    "<div>" + "".join(f"<div>r{i}c{j}</div>" for j in range(10)) + "</div>" for i in range(300)
) + "<button>Next</button>")

LONG = "Lorem ipsum dolor sit amet. " * 40
PAGES["/long-text"] = ("long", "", f"<div>{LONG}</div><p>{LONG}</p>")


def clause(n):
    text = f"{n // 50 + 1}.{n % 50 + 1} The Supplier shall keep obligation {n} in force for the term of this Agreement."
    if n % 10 == 0:
        text += " It survives termination, applies to every affiliate and subcontractor, and is not limited by any" \
            " other clause: notice of a breach is given in writing within five business days, and the cure period" \
            f" is thirty days from that notice. Penalty schedule {n} applies to any breach not cured in time."
    return f"<p>{text}</p>"


PAGES["/rerender"] = ("rerender", "", """
<p id=status>idle</p>
<div id=app></div>
<button id=redraw>Redraw</button> <button id=add>Add row</button> <button id=route>Route</button>
<script>
let rows = ['alpha', 'beta', 'gamma'];
function draw() {
  const app = document.getElementById('app');
  app.innerHTML = '';
  for (const [i, r] of rows.entries()) {
    const li = document.createElement('div');
    li.textContent = r + ' ';
    const b = document.createElement('button');
    b.textContent = 'Edit';
    b.onclick = () => { document.getElementById('status').textContent = 'edit ' + r; };
    li.appendChild(b);
    app.appendChild(li);
  }
  const save = document.createElement('button');
  save.textContent = 'Save';
  save.onclick = () => { document.getElementById('status').textContent = 'saved'; };
  app.appendChild(save);
}
draw();
document.getElementById('redraw').onclick = draw;
document.getElementById('add').onclick = () => { rows.push('delta'); draw(); };
document.getElementById('route').onclick = () => { history.pushState({}, '', '/rerender?view=2'); draw(); };
</script>""")

PAGES["/legal"] = ("agreement", "", "<main><h1>Master Services Agreement</h1>" + "".join(
    (f"<h2>Section {n // 50 + 1}</h2>" if n % 50 == 0 else "") + clause(n) for n in range(1500)
) + "</main>")

PAGES["/cookie-banner"] = ("cookie banner", "", """
<p id=status>waiting</p>
<button id=buy onclick="document.getElementById('status').textContent='bought'">Buy</button>
<div id=consent style="position:fixed;inset:0;background:rgba(0,0,0,.6);z-index:9">
  <div style="background:#fff;margin:30vh auto;width:300px;padding:12px">
    <p>We use cookies.</p>
    <button onclick="document.getElementById('consent').remove()">Accept</button>
    <button onclick="document.getElementById('consent').remove()">Reject</button>
  </div>
</div>""")

PAGES["/loading-cover"] = ("loading cover", "", """
<p id=status>waiting</p>
<button id=save onclick="document.getElementById('status').textContent='saved'">Save</button>
<div id=cover style="position:fixed;inset:0;background:rgba(255,255,255,.8);z-index:9">
  <p>Loading...</p><button>Cancel</button>
</div>
<script>
fetch('/hold?ms=2500').then(() => document.getElementById('cover').remove());
</script>""")

PAGES["/moving-target"] = ("moving target", "", """
<p id=status>waiting</p>
<button id=mover style="position:absolute;left:60px;top:60px;width:34px;height:22px"
  onclick="document.getElementById('status').textContent='ok '+event.clientX+' '+event.clientY">M</button>
<script>
// No CSS animation: a plain rAF glide, the shape a deferred page commit
// takes. 40px a frame for 12 frames, then still - a one-read click lands
// on empty page where the button used to be.
let x = 60, frames = 0;
const glide = () => {
  if (frames++ >= 12) return;
  x += 40;
  document.getElementById('mover').style.left = x + 'px';
  requestAnimationFrame(glide);
};
requestAnimationFrame(glide);
</script>""")

PAGES["/leader"] = ("leader", "", """
<p id=role>waiting</p>
<script>
const bc = new BroadcastChannel('nv-leader');
const role = document.getElementById('role');
if (location.search.includes('leader')) {
  role.textContent = 'leader';
  addEventListener('beforeunload', () => bc.postMessage('bye'));
} else {
  role.textContent = 'follower';
  bc.onmessage = (e) => { if (e.data === 'bye') role.textContent = 'took over'; };
}
</script>""")

PAGES["/poll"] = ("poll", "", """
<p id=status>idle</p><button id=go>Go</button>
<script>
let n = 0;
setInterval(() => fetch('/slow-echo?ms=5').then(() => { document.getElementById('status').textContent = 'polled ' + (++n); }), 300);
</script>""")

PAGES["/long-poll"] = ("long poll", "", """
<p id=status>connecting</p>
<script>
// A chat-style long poll: one request held open, reconnected when it ends.
let n = 0;
const poll = () => fetch('/hold?ms=' + (new URLSearchParams(location.search).get('hold') || '2000'))
  .then(() => { document.getElementById('status').textContent = 'reconnected ' + (++n); poll(); });
poll();
</script>""")

PAGES["/beacons"] = ("beacons", "", """
<p id=status>tracking</p>
<script>
// Analytics beacons at irregular intervals; the server answers 204.
const beacon = () => {
  fetch('/collect', { method: 'POST', body: 'e=' + Date.now() });
  setTimeout(beacon, 100 + Math.random() * 800);
};
beacon();
</script>""")

PAGES["/one-fetch"] = ("one fetch", "", """
<ul id=list><li>Loading...</li></ul>
<script>
fetch('/slow-echo?ms=400').then(() => {
  document.getElementById('list').innerHTML = '<li>Alpha</li><li>Beta</li>';
});
</script>""")

PAGES["/click-fetch"] = ("click fetch", "", """
<button id=go>Go</button><ul id=list><li>Loading...</li></ul>
<script>
document.getElementById('go').onclick = () => {
  fetch('/slow-echo?ms=200').then(() => {
    document.getElementById('list').innerHTML = '<li>Alpha</li><li>Beta</li>';
  });
};
</script>""")

PAGES["/nav-intent"] = ("nav intent", "", """
<div id=out>idle</div>
<button id=plain>Plain</button>
<form id=f1><input name=x value=1><button>Handled submit</button></form>
<form id=f2 action=/posted><input name=y value=2><button>Real submit</button></form>
<form id=f3 action=/posted><input name=z required><button>Invalid submit</button></form>
<form id=f4 action=/posted><input name=w value=4><button>Script submit</button></form>
<a id=routed href=/posted>Routed link</a> <a id=real href=/posted>Real link</a>
<input id=q aria-label=Query>
<button id=save>Save later</button>
<script>
const out = document.getElementById('out');
document.getElementById('plain').onclick = () => { out.textContent = 'plain ' + Date.now(); };
f1.onsubmit = (e) => { e.preventDefault(); out.textContent = 'handled ' + Date.now(); };
f4.onsubmit = (e) => { e.preventDefault(); f4.submit(); };
routed.onclick = (e) => { e.preventDefault(); history.pushState({}, '', '/routed'); out.textContent = 'routed'; };
// A placeholder at once, the request 30 ms later.
document.getElementById('save').onclick = () => {
  out.textContent = 'Saving...';
  setTimeout(() => fetch('/slow-echo?ms=150').then(() => { out.textContent = 'saved'; }), 30);
};
// Typing asks for suggestions a timer turn later, after 20 ms of work,
// with nothing shown first.
q.oninput = () => setTimeout(() => {
  const end = performance.now() + 20;
  while (performance.now() < end);
  fetch('/slow-echo?ms=150').then(() => { out.textContent = 'suggested ' + q.value; });
}, 0);
</script>""")

PAGES["/two-fetch"] = ("two fetch", "", """
<ul id=list><li>Loading...</li></ul>
<script>
// Half a list, then a 400 ms gap with no request, then the other half:
// a quiet window shorter than the gap reads only the first half.
fetch('/slow-echo?ms=100').then(() => {
  document.getElementById('list').innerHTML = '<li>Alpha</li>';
  return new Promise(r => setTimeout(r, 400));
}).then(() => fetch('/slow-echo?ms=100')).then(() => {
  document.getElementById('list').innerHTML = '<li>Alpha</li><li>Beta</li>';
});
</script>""")

PAGES["/action-chain"] = ("action chain", "", """
<button id=go>Load</button><div id=out>idle</div>
<script>
// A click whose response starts a second fetch at once: the report must
// show what the second fetch rendered, not the first's.
go.onclick = () => {
  fetch('/slow-echo?ms=150').then(() => {
    out.textContent = 'phase one';
    fetch('/slow-echo?ms=150').then(() => { out.textContent = 'chain complete'; });
  });
};
</script>""")

PAGES["/action-chain-timer"] = ("action chain timer", "", """
<button id=go>Load</button><div id=out>idle</div>
<script>
// As /action-chain, with the second fetch deferred 80 ms: inside the
// after-action quiet window, after a frame and a task.
go.onclick = () => {
  fetch('/slow-echo?ms=150').then((r) => r.text()).then(() => {
    out.textContent = 'phase one';
    setTimeout(() => fetch('/slow-echo?ms=150').then((r) => r.text()).then(() => { out.textContent = 'chain complete'; }), 80);
  });
};
</script>""")

PAGES["/big-table"] = ("big table", "", """
<table><tbody id=rows></tbody></table><p id=out></p>
<script>
rows.innerHTML = Array.from({length: 2000}, (_, i) =>
  `<tr><td>Item ${i}</td><td><button onclick="out.textContent='edit ${i}'">Edit ${i}</button> <a href="#r${i}">View ${i}</a></td></tr>`).join('');
</script>""")

PAGES["/stale-ref"] = ("stale ref", "", """
<button id=b>Go</button>
<div id=h draggable="true" style="width:80px;height:20px">Handle</div>
""")
PAGES["/drag-native"] = ("drag native", "", """
<style>li{padding:8px;margin:2px;border:1px solid #888;width:200px;list-style:none;background:#eee}</style>
<ul id=list><li id=a>Alpha</li><li id=b>Beta</li><li id=c>Gamma</li></ul>
<p id=log></p>
<script>
const log = [];
const note = (t) => { log.push(t); document.getElementById('log').textContent = log.join(' '); };
let moving = null;
for (const li of document.querySelectorAll('li')) {
  li.draggable = true;
  li.addEventListener('dragstart', (e) => { moving = li; e.dataTransfer.setData('text/plain', li.id); e.dataTransfer.effectAllowed = 'move'; note('start'); });
  li.addEventListener('dragenter', () => note('enter'));
  li.addEventListener('dragover', (e) => e.preventDefault());
  li.addEventListener('drop', (e) => {
    e.preventDefault();
    const id = e.dataTransfer.getData('text/plain');
    const src = document.getElementById(id);
    note('drop:' + id);
    if (src && src !== li) li.after(src);
  });
  li.addEventListener('dragend', () => { note('end');  });
}
</script>""")

PAGES["/drag-pointer"] = ("drag pointer", "", """
<style>li{padding:8px;margin:2px;border:1px solid #888;width:200px;list-style:none;background:#eee}</style>
<ul id=list><li id=a>Alpha</li><li id=b>Beta</li><li id=c>Gamma</li></ul>
<p id=log></p>
<script>
const log = [];
const note = (t) => { log.push(t); document.getElementById('log').textContent = log.join(' '); };
let moving = null;
for (const li of document.querySelectorAll('li')) {
  li.addEventListener('pointerdown', () => { li.draggable = true; });
  li.addEventListener('dragstart', (e) => { moving = li; e.dataTransfer.setData('text/plain', li.id); e.dataTransfer.effectAllowed = 'move'; note('start'); });
  li.addEventListener('dragenter', () => note('enter'));
  li.addEventListener('dragover', (e) => e.preventDefault());
  li.addEventListener('drop', (e) => {
    e.preventDefault();
    const id = e.dataTransfer.getData('text/plain');
    const src = document.getElementById(id);
    note('drop:' + id);
    if (src && src !== li) li.after(src);
  });
  li.addEventListener('dragend', () => { note('end'); li.draggable = false; });
}
</script>""")

PAGES["/drag-position"] = ("drag position", "#src{width:80px;height:30px;background:#cde}#dst{width:200px;height:100px;background:#eee;margin-top:40px}", """
<div id=src draggable=true>drag me</div>
<div id=dst>drop here</div>
<p id=log></p>
<script>
const dst = document.getElementById('dst');
document.getElementById('src').addEventListener('dragstart', (e) => {
  e.dataTransfer.setData('text/plain', 'x');
  const r = e.target.getBoundingClientRect();
  document.getElementById('log').dataset.press = Math.round(e.clientX - r.left) + ',' + Math.round(e.clientY - r.top);
});
dst.addEventListener('dragover', (e) => e.preventDefault());
dst.addEventListener('drop', (e) => {
  e.preventDefault();
  const r = dst.getBoundingClientRect();
  document.getElementById('log').textContent = Math.round(e.clientX - r.left) + ',' + Math.round(e.clientY - r.top);
});
</script>""")

PAGES["/drag-slider"] = ("drag slider", "", """
<div id=track style="position:relative;width:300px;height:20px;background:#ccc;margin:20px">
  <div id=knob style="position:absolute;left:0;top:0;width:20px;height:20px;background:#33a"></div>
</div>
<p id=out>0</p>
<script>
const knob = document.getElementById('knob'), track = document.getElementById('track');
let held = false;
knob.addEventListener('mousedown', (e) => { held = true; e.preventDefault(); });
addEventListener('mouseup', () => { held = false; });
addEventListener('mousemove', (e) => {
  if (!held) return;
  const x = Math.max(0, Math.min(280, e.clientX - track.getBoundingClientRect().left - 10));
  knob.style.left = x + 'px';
  document.getElementById('out').textContent = Math.round(x);
});
</script>""")

PAGES["/hidden-layers"] = ("hidden layers", "", """
<div id=spot style="position:relative;height:60px">
  <button id=greenButton style="position:absolute;left:0;top:0;width:120px;height:40px;background:#3a3">Button</button>
</div>
<div id=log></div>
<script>
const green = document.getElementById('greenButton');
green.addEventListener('click', () => {
  if (document.getElementById('blueButton')) {
    const p = document.createElement('p');
    p.className = 'bg-warning';
    p.textContent = 'green was clicked while covered';
    document.getElementById('log').appendChild(p);
    return;
  }
  const b = document.createElement('button');
  b.id = 'blueButton';
  b.textContent = 'Button';
  b.style.cssText = 'position:absolute;left:0;top:0;width:120px;height:40px;background:#33a;z-index:2';
  green.parentElement.appendChild(b);
});
</script>""")

PAGES["/iframe-form"] = ("iframe form", "iframe{width:420px;height:160px;border:1px solid #888}", """
<h1>Newsletter</h1>
<iframe id=email-subscribe src="/iframe-inner"></iframe>""")

PAGES["/iframe-inner"] = ("inner", "", """
<label>Email <input id=email type=email></label>
<button id=btn-subscribe type=button>Subscribe</button>
<p id=msg></p>
<script>
document.getElementById('btn-subscribe').addEventListener('click', () => {
  document.getElementById('msg').textContent = 'Thanks ' + document.getElementById('email').value;
});
</script>""")

FRAMESET = """<!doctype html><html><head><title>frames</title></head>
<frameset rows="50%,50%">
  <frame name=frame-top src="/frame-top">
  <frame name=frame-bottom src="/frame-text?t=BOTTOM">
</frameset></html>"""

FRAME_TOP = """<!doctype html><html><head><title>top</title></head>
<frameset cols="33%,33%,33%">
  <frame name=frame-left src="/frame-text?t=LEFT">
  <frame name=frame-middle src="/frame-text?t=MIDDLE">
  <frame name=frame-right src="/frame-text?t=RIGHT">
</frameset></html>"""

PAGES["/srcdoc"] = ("srcdoc", "iframe{width:420px;height:150px}", """
<p id=out>nothing yet</p>
<div id=host></div>
<script>
const inner = '<button id=deep onclick="parent.parent.document.getElementById(\\'out\\').textContent = \\'deep clicked\\'">Deep button</button>';
const outer = document.createElement('iframe');
outer.srcdoc = '<p>outer frame</p><iframe id=innerframe></iframe>';
outer.addEventListener('load', () => { outer.contentDocument.getElementById('innerframe').srcdoc = inner; });
document.getElementById('host').appendChild(outer);
</script>""")

PAGES["/multi-select"] = ("multi select", "", """
<label for=cars>Cars</label>
<select id=cars multiple size=4>
  <option value=volvo>Volvo</option><option value=saab>Saab</option>
  <option value=opel>Opel</option><option value=audi>Audi</option>
</select>
<p id=picked></p>
<script>
const cars = document.getElementById('cars');
cars.addEventListener('change', () => {
  document.getElementById('picked').textContent = [...cars.selectedOptions].map(o => o.value).join(',');
});
</script>""")

PAGES["/nbsp-select"] = ("nbsp select", "", """
<select id=city>
  <option value=pick>Pick one</option>
  <option value=ny>New&nbsp;York</option>
  <option value=sf>San&nbsp;Francisco</option>
</select>""")

PAGES["/canvas"] = ("canvas", "body{margin:0;height:3000px}canvas{position:absolute;left:50px;top:40px;width:300px;height:200px;background:#eee}", """
<canvas id=c width=300 height=200></canvas>
<script>
window.log = [];
// Strokes drawn, and context losses (macOS drops a canvas's GPU context
// under load, and the bitmap with it), apart from the event log.
window.drawn = 0;
window.lost = [];
c.addEventListener('contextlost', () => lost.push('contextlost'));
const g = c.getContext('2d');
let last = null;
const at = (e) => `${Math.round(e.offsetX)} ${Math.round(e.offsetY)}`;
for (const t of ['mousedown', 'mouseup', 'click', 'dblclick', 'contextmenu']) {
  c.addEventListener(t, (e) => { log.push(`${t} ${at(e)} ${e.button} ${e.buttons}`); if (t === 'contextmenu') e.preventDefault(); });
}
c.addEventListener('mousemove', (e) => {
  log.push(`move ${at(e)} ${e.buttons}`);
  if (e.buttons & 1 && last) { g.beginPath(); g.lineWidth = 4; g.moveTo(last[0], last[1]); g.lineTo(e.offsetX, e.offsetY); g.stroke(); drawn++; }
  last = [e.offsetX, e.offsetY];
});
c.addEventListener('wheel', (e) => { log.push(`wheel ${Math.round(e.deltaX)} ${Math.round(e.deltaY)}`); e.preventDefault(); }, { passive: false });
window.ink = (x, y) => g.getImageData(x, y, 1, 1).data[3];
</script>
""")

# What a form POST to /posted shows, and what reopening it with GET shows.
PAGES["/posted"] = ("posted", "", """<p>posted</p>""")

PAGES["/download"] = ("download", "", """
<a id=dl href="/file.csv">Download CSV</a>
<button id=blob type=button>Save notes</button>
<script>
document.getElementById('blob').addEventListener('click', () => {
  const a = document.createElement('a');
  a.href = URL.createObjectURL(new Blob(['notes from a blob\\n'], { type: 'text/plain' }));
  a.download = 'notes.txt';
  document.body.appendChild(a);
  a.click();
  a.remove();
});
</script>""")

PAGES["/shadow-css"] = ("shadow css", "", """
<guid-generator></guid-generator>
<outer-box></outer-box>
<p id=nested-out>untouched</p>
<script>
customElements.define('guid-generator', class extends HTMLElement {
  connectedCallback() {
    const root = this.attachShadow({ mode: 'open' });
    root.innerHTML = '<input id=editField><button id=buttonGenerate><svg width=16 height=16><circle cx=8 cy=8 r=6/></svg></button>';
    root.getElementById('buttonGenerate').addEventListener('click', () => {
      root.getElementById('editField').value = 'guid-' + Math.random().toString(16).slice(2, 10);
    });
  }
});
customElements.define('inner-box', class extends HTMLElement {
  connectedCallback() {
    const root = this.attachShadow({ mode: 'open' });
    root.innerHTML = '<button id=nested><svg width=16 height=16><rect width=12 height=12/></svg></button>';
    root.getElementById('nested').addEventListener('click', () => { document.getElementById('nested-out').textContent = 'nested clicked'; });
  }
});
customElements.define('outer-box', class extends HTMLElement {
  connectedCallback() { this.attachShadow({ mode: 'open' }).innerHTML = '<div><inner-box></inner-box></div>'; }
});
</script>""")

PAGES["/entry-modal"] = ("entry modal", """
.modal{position:fixed;left:20%;top:15%;width:50%;padding:20px;background:#fff;border:1px solid #333;
  opacity:0;transform:translateY(-60px);transition:opacity .7s ease-out, transform .7s ease-out;display:none}
.modal.show{opacity:1;transform:none}""", """
<p>Some page</p>
<div class=modal id=modal role=dialog aria-label="This is a modal window">
  <h3>This is a modal window</h3>
  <p>It's commonly used to encourage a user to take an action.</p>
  <button id=close type=button>Close</button>
</div>
<script>
const modal = document.getElementById('modal');
setTimeout(() => {
  modal.style.display = 'block';
  void modal.offsetWidth;
  modal.classList.add('show');
  // The page's script wires the button once the modal has finished opening.
  modal.addEventListener('transitionend', () => {
    window.wiredAt = performance.now();
    document.getElementById('close').onclick = () => { modal.classList.remove('show'); modal.style.display = 'none'; };
  }, { once: true });
  document.addEventListener('pointerdown', () => { window.pressedAt = window.pressedAt || performance.now(); }, true);
}, 100);
</script>""")

PAGES["/overlapped"] = ("overlapped", """
#wrap{position:relative;width:300px;height:120px}
#scroller{height:120px;overflow-y:scroll;border:1px solid #999}
#panel{position:absolute;left:0;right:16px;top:45px;bottom:0;background:#eee;border-top:1px solid #999}
input{display:block;margin:10px}""", """
<div id=wrap>
  <div id=scroller>
    <input id=id placeholder=Id>
    <input id=name placeholder=Name>
    <div style="height:300px"></div>
  </div>
  <div id=panel>Subscribe for news</div>
</div>
<p id=status>ok</p>
<script>
const name = document.getElementById('name');
name.addEventListener('input', () => {
  const r = name.getBoundingClientRect();
  const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
  if (hit !== name) { name.value = ''; document.getElementById('status').textContent = 'cleared: covered while typing'; }
});
</script>""")

PAGES["/right-click"] = ("right click", "", """
<button id=rightClickBtn type=button>Right Click Me</button>
<button id=doubleClickBtn type=button>Double Click Me</button>
<p id=msg></p>
<script>
document.getElementById('rightClickBtn').addEventListener('contextmenu', (e) => {
  e.preventDefault();
  document.getElementById('msg').textContent = 'You have done a right click';
});
document.getElementById('rightClickBtn').addEventListener('click', () => {
  document.getElementById('msg').textContent = 'That was a left click';
});
document.getElementById('doubleClickBtn').addEventListener('dblclick', () => {
  document.getElementById('msg').textContent = 'You have done a double click';
});
</script>""")


PAGES["/wizard"] = ("wizard", "", """
<script>
const step = new URLSearchParams(location.search).get('step') || '1';
document.write(step === '1'
  ? '<label>Name <input id=name></label> <button id=next type=button>Next</button>'
  : step === '2'
  ? '<label>Email <input id=email></label> <button id=finish type=button>Finish</button>'
  : '<p id=done>Saved ' + sessionStorage.getItem('name') + ' / ' + sessionStorage.getItem('email') + '</p>');
const save = (k, v, next) => fetch('/slow-echo?ms=150').then(() => { sessionStorage.setItem(k, v); location.href = '/wizard?step=' + next; });
if (step === '1') document.getElementById('next').onclick = () => save('name', document.getElementById('name').value, 2);
if (step === '2') document.getElementById('finish').onclick = () => save('email', document.getElementById('email').value, 3);
</script>""")

PAGES["/card-radios"] = ("card radios", ".sr{position:absolute;width:1px;height:1px;overflow:hidden;clip:rect(0 0 0 0)}.card{border:1px solid #888;padding:10px;margin:4px;cursor:pointer;width:200px}", """
<div class=card data-for=s role=radio aria-checked=false>Small</div>
<div class=card data-for=m role=radio aria-checked=false>Medium</div>
<input type=radio name=size value=s id=in-s class=sr><input type=radio name=size value=m id=in-m class=sr>
<p id=picked>none</p>
<script>
document.querySelectorAll('.card').forEach((c) => c.addEventListener('click', () => {
  document.querySelectorAll('.card').forEach((x) => x.setAttribute('aria-checked', 'false'));
  c.setAttribute('aria-checked', 'true');
  document.getElementById('in-' + c.dataset.for).checked = true;
  document.getElementById('picked').textContent = c.dataset.for;
}));
</script>""")

PAGES["/combobox"] = ("combobox", ".opt{padding:4px 8px}.opt.hl{background:#bde}", """
<div id=cb role=combobox aria-expanded=false aria-haspopup=listbox style="border:1px solid #888;padding:6px;width:220px">
  <span id=cb-value>Select…</span> <input id=cb-input aria-label=Fruit autocomplete=off style="border:0;width:8em">
</div>
<p id=committed>none</p>
<script>
const OPTIONS = ['Apple', 'Apricot', 'Banana', 'Blueberry', 'Cherry'];
const cb = document.getElementById('cb'), input = document.getElementById('cb-input');
let open = false, list = OPTIONS;
function render() {
  document.querySelectorAll('.portal').forEach((e) => e.remove());
  if (!open) return;
  const r = cb.getBoundingClientRect(), d = document.createElement('div');
  d.className = 'portal'; d.setAttribute('role', 'listbox');
  d.style.cssText = `position:absolute;top:${r.bottom + scrollY}px;left:${r.left}px;border:1px solid #888;background:#fff`;
  list.slice(0, 3).forEach((o) => {
    const e = document.createElement('div'); e.className = 'opt'; e.setAttribute('role', 'option'); e.textContent = o;
    e.addEventListener('mousedown', (ev) => { ev.preventDefault(); choose(o); });
    d.appendChild(e);
  });
  document.body.appendChild(d);
}
function choose(o) { document.getElementById('cb-value').textContent = o; document.getElementById('committed').textContent = o; input.value = ''; list = OPTIONS; open = false; cb.setAttribute('aria-expanded', 'false'); render(); }
input.addEventListener('keydown', (ev) => { if (ev.key === 'ArrowDown') { ev.preventDefault(); open = true; cb.setAttribute('aria-expanded', 'true'); render(); } });
input.addEventListener('input', () => { list = OPTIONS.filter((o) => o.toLowerCase().includes(input.value.toLowerCase())); render(); });
// Only three options show at a time: Cherry needs typing to appear.
</script>""")

PAGES["/typed-only"] = ("typed only", "", """
<label>Amount <input id=amt></label> <button id=go type=button>Commit</button>
<p id=model>model=</p>
<script>
let model = '';
const amt = document.getElementById('amt');
amt.addEventListener('keydown', (e) => { if (!e.isTrusted) return;
  if (e.key.length === 1) model += e.key; else if (e.key === 'Backspace') model = amt.selectionStart === 0 && amt.selectionEnd === amt.value.length ? '' : model.slice(0, -1); });
amt.addEventListener('blur', () => { amt.value = model; });
document.getElementById('go').onclick = () => { document.getElementById('model').textContent = 'model=' + model; };
</script>""")

PAGES["/lazy-box"] = ("lazy box", "", """
<div id=list style="height:200px;overflow:auto;border:1px solid #888"></div>
<script>
let n = 0, busy = false;
const list = document.getElementById('list');
function more() { for (let i = 0; i < 20; i++) { n++; const d = document.createElement('div'); d.style.padding = '8px'; d.textContent = 'Item ' + n; list.appendChild(d); } }
more();
list.addEventListener('scroll', () => {
  if (!busy && list.scrollTop + list.clientHeight >= list.scrollHeight - 10 && n < 60) { busy = true; setTimeout(() => { more(); busy = false; }, 100); }
});
</script>""")


PAGES["/honesty"] = ("honesty", "", """
<button id=dup style="position:absolute;left:-9999px" onclick="out('decoy')">Decoy</button>
<button id=dup onclick="out('real')">Real action</button>
<button id=save disabled onclick="out('saved')">Save</button>
<button id=unlock-later disabled onclick="out('unlocked')">Unlocks soon</button>
<input id=ro readonly value=fixed> <input id=off disabled>
<article style="width:300px;padding:40px;border:1px solid #888"><h3>Carbon Gear</h3><div>$34.82</div>
<button aria-label="Add Carbon Gear to cart" onclick="out('added')">Add to cart</button></article>
<p id=out>none</p>
<script>
function out(t) { document.getElementById('out').textContent = t; }
setTimeout(() => { document.getElementById('unlock-later').disabled = false; }, 600);
</script>""")


PAGES["/fillwait"] = ("fillwait", "", """
<input id=hidden style="display:none"> <input id=vis style="visibility:hidden">
<input id=later style="display:none"> <input id=off disabled> <input id=ro readonly value=fixed>
<input id=zero style="opacity:0">
<script>
setTimeout(() => { document.getElementById('later').style.display = ''; }, 400);
setTimeout(() => { document.getElementById('off').disabled = false; }, 400);
</script>""")


PAGES["/effects"] = ("effects", "#strip{width:200px;overflow-x:auto;white-space:nowrap}#strip span{display:inline-block;width:150px}", """
<button id=late>Close</button>
<button id=noop>Does nothing</button>
<label><input type=checkbox id=agree> Agree</label>
<input id=q value=typed> <button id=clear>Clear</button>
<button id=send>Send</button>
<button id=later>Later</button>
<div id=strip><span>a</span><span>b</span><span>c</span></div> <button id=next>Next</button>
<a id=self href="#">Top</a> <button id=copy>Copy</button> <button id=clip>Copy link</button>
<button id=save>Save</button>
<x-panel id=panel></x-panel> <x-badge id=badge></x-badge>
<p id=out>none</p>
<script>
const out = (t) => { document.getElementById('out').textContent = t; };
// Close is wired only once the page has "loaded its scripts": here when
// the test says so (`wire()`), on a real page a moment after load.
function wire() { document.getElementById('late').onclick = () => out('closed'); }
// Clear sets the field from script: no DOM change, no event.
document.getElementById('clear').onclick = () => { document.getElementById('q').value = ''; };
// Send only starts a request.
document.getElementById('send').onclick = () => { fetch('/slow-echo?ms=400'); };
// Copy puts text on the clipboard and shows nothing.
document.addEventListener('copy', (e) => { e.clipboardData.setData('text/plain', 'ref 42'); e.preventDefault(); });
document.getElementById('copy').onclick = () => { document.execCommand('copy'); };
document.getElementById('clip').onclick = () => { navigator.clipboard.writeText(location.href).catch(() => {}); };
// Save shows its result only once the server answers (400 ms).
document.getElementById('save').onclick = () => fetch('/slow-echo?ms=400').then(() => out('saved'));
// `?telemetry`: every click is reported, as analytics do.
if (location.search.includes('telemetry')) document.addEventListener('click', (e) => { fetch('/event/click?on=' + e.target.id); }, true);
document.getElementById('later').onclick = () => setTimeout(() => out('later'), 80);
document.getElementById('next').onclick = () => { document.getElementById('strip').scrollLeft += 150; };
// A button in one shadow root changes text in another one.
customElements.define('x-badge', class extends HTMLElement {
  constructor() { super(); this.attachShadow({ mode: 'open' }).innerHTML = '<b>0</b>'; }
  set count(n) { this.shadowRoot.querySelector('b').textContent = n; }
});
customElements.define('x-panel', class extends HTMLElement {
  constructor() { super(); this.attachShadow({ mode: 'open' }).innerHTML = '<button id=bump>Bump</button>'; let n = 0;
    this.shadowRoot.querySelector('button').onclick = () => { document.getElementById('badge').count = ++n; }; }
});
</script>""")


PAGES["/toggles"] = ("toggles", "#tip{inset:auto;top:240px;left:20px;margin:0}#dlg{top:160px}", """
<button id=open-tip>Open tip</button> <button id=close-tip>Close tip</button>
<button id=late-tip>Late tip</button> <button id=late-dlg>Late dialog</button> <button id=noop>Does nothing</button>
<div id=tip popover=manual>Tip</div>
<dialog id=dlg>Dialog</dialog>
<script>
const tip = document.getElementById('tip'), dlg = document.getElementById('dlg');
// A popover opening or closing changes no attribute: only its toggle says so.
document.getElementById('open-tip').onclick = () => tip.showPopover();
document.getElementById('close-tip').onclick = () => tip.hidePopover();
// show() and showPopover() queue their toggle as a later task, which can
// run after the next click has started: these stand in for that late event.
const late = (n) => n.dispatchEvent(new ToggleEvent('toggle', { oldState: 'closed', newState: 'open' }));
document.getElementById('late-tip').onclick = () => late(tip);
document.getElementById('late-dlg').onclick = () => late(dlg);
dlg.show();
</script>""")


PAGES["/hover-cover"] = ("hover cover", "#cover{position:fixed;inset:0;background:#fff} #cover button{width:100%;height:100%}", """
<button id=go onclick="document.getElementById('out').textContent = 'clicked ' + (++window.k)">Go</button>
<span id=out>idle</span>
<div id=cover hidden><button onclick="document.getElementById('out').textContent = 'subscribed'">Subscribe</button></div>
<script>
window.k = 0;
addEventListener('pointermove', () => {
  const c = document.getElementById('cover');
  c.hidden = false;
  setTimeout(() => { c.hidden = true; }, 300);
}, { once: true });
</script>
""")


PAGES["/click-off"] = ("click off", "#go{padding:10px}", """
<div id=w><button id=go>Go</button></div>
<p id=out>idle</p>
<script>
// A press the page reacts to before the release: a framework that renders
// the pressed state anew (?replace), a banner that slides the button down
// (?banner). The release lands on another element than the press, so the
// click goes to the parent they share and Go's own handler never runs.
const q = new URLSearchParams(location.search);
const went = () => { document.getElementById('out').textContent = 'went'; };
const go = document.getElementById('go');
go.onclick = went;
if (q.has('replace')) go.onmousedown = function () {
  const n = this.cloneNode(true);
  n.onclick = went;
  this.replaceWith(n);
};
if (q.has('banner')) go.onpointerdown = () => {
  document.getElementById('w').insertAdjacentHTML('afterbegin', '<div style="height:80px">banner</div>');
};
</script>""")


PAGES["/wired-late"] = ("wired late", "", """
<button id=go>Go</button>
<div id=root><span><button id=inner>Inner</button></span></div>
<p id=out>idle</p>
<script>
// What a page that wires its controls after load does (hydration, a
// widget that starts later): until then a click on Go reaches nothing.
window.hits = 0;
const q = new URLSearchParams(location.search);
const out = (t) => { document.getElementById('out').textContent = t; };
function wireGo() { document.getElementById('go').onclick = () => { hits++; out('went ' + hits); }; }
addEventListener('load', () => {
  if (q.has('ms')) setTimeout(wireGo, +q.get('ms'));
  if (q.has('script')) setTimeout(() => {
    const s = document.createElement('script');
    s.src = '/late-script?ms=' + q.get('script');
    document.head.appendChild(s);
  }, 0);
  if (q.has('root')) setTimeout(() => document.getElementById('root').addEventListener('click', (e) => {
    if (e.target.closest('#inner')) { hits++; out('inner ' + hits); }
  }), +q.get('root'));
  // A framework that hydrates the whole document listens there.
  if (q.has('doc')) setTimeout(() => document.addEventListener('click', (e) => {
    if (e.target.closest('#go')) { hits++; out('went ' + hits); }
  }), +q.get('doc'));
});
// Analytics listen on the document from the start.
if (q.has('analytics')) document.addEventListener('click', () => { window.beacons = (window.beacons || 0) + 1; }, true);
// An early event contract: clicks on Go before it is wired are recorded,
// and replayed once it is (the click counts, later).
if (q.has('replay')) {
  const early = [];
  const contract = (e) => { if (e.target.closest('#go')) early.push(e); };
  document.addEventListener('click', contract, true);
  addEventListener('load', () => setTimeout(() => {
    // &invisible: the replayed click counts and shows nothing.
    if (q.has('invisible')) document.getElementById('go').onclick = () => { hits++; };
    else wireGo();
    document.removeEventListener('click', contract, true);
    const replay = () => { for (const e of early.splice(0)) document.getElementById('go').onclick(e); };
    if (q.has('late')) setTimeout(replay, 50); else replay();
  }, +q.get('replay')));
}
// Go counts from the start but shows nothing; a widget then listens on the
// body. The click was heard, so it must not be sent again.
if (q.has('quiet')) {
  document.getElementById('go').onclick = () => { hits++; };
  addEventListener('load', () => setTimeout(() => document.body.addEventListener('pointerdown', () => {}), 300));
}
</script>""")


def twin_frames(port):
    """Frames that hold the same ids: two of this origin, two of another
    (localhost in a 127.0.0.1 page), and one inside another."""
    form = lambda who: ("<label>Name <input id=name></label><button id=save type=button "
                        f"onclick=\"parent.document.getElementById('log').textContent += '{who}:' + document.getElementById('name').value + ' '\">Save</button>")
    outer = "<p>outer</p><iframe id=inner srcdoc='<button type=button onclick=top.deep()>Deep</button>'></iframe>"
    return ("twin frames", "iframe{width:420px;height:120px;border:1px solid #888;display:block}", f"""
<p id=log></p>
<iframe id=shipping srcdoc="{form('shipping').replace('"', '&quot;')}"></iframe>
<iframe id=billing srcdoc="{form('billing').replace('"', '&quot;')}"></iframe>
<iframe id=card src="http://localhost:{port}/xo-child"></iframe>
<iframe id=card2 src="http://localhost:{port}/xo-child"></iframe>
<iframe id=outer srcdoc="{outer.replace('"', '&quot;')}"></iframe>
<script>
window.deep = () => {{ document.getElementById('log').textContent += 'deep '; }};
addEventListener('message', (e) => {{
  if (!e.data || !e.data.paid) return;
  const who = e.source === document.getElementById('card2').contentWindow ? 'card2' : 'card';
  document.getElementById('log').textContent += who + ':' + e.data.paid + ' ';
}});
</script>""")


def xo_parent(port, child_host):
    return ("xo parent", "iframe{width:420px;height:170px;border:1px solid #888}", f"""
<h1>Checkout</h1>
<h2>Payment details</h2>
<p id=paid>unpaid</p>
<iframe id=card title="Secure card input" src="http://{child_host}:{port}/xo-child"></iframe>
<script>
addEventListener('message', (e) => {{ if (e.data && e.data.paid) document.getElementById('paid').textContent = 'paid ' + e.data.paid; }});
</script>""")


PAGES["/journal"] = ("journal", ".tip{display:none} #tipped:focus ~ .tip{display:inline} .hint{display:none} "
    "form:has(#code:placeholder-shown) .hint{display:block}", """
<input id=name aria-label=Name>
<textarea id=note aria-label=Note></textarea>
<input id=count aria-label=Count oninput="document.getElementById('n').textContent = this.value.length + ' chars'">
<span id=n>0 chars</span>
<p><input id=tipped aria-label=Tipped><span class=tip>Use your full name</span></p>
<form><input id=code aria-label=Code placeholder=code><span class=hint>Type a code</span></form>
<style id=rules>.later{display:none}</style>
<p><input id=restyle aria-label=Restyle oninput="document.getElementById('rules').sheet.cssRules[0].style.display = 'inline'">
<span class=later>Restyled by a rule</span></p>
<style>@import url('/journal-import.css');</style>
<p><input id=imported aria-label=Imported><span class=itip>Imported tip</span></p>
<div id=host></div>
<input id=inclosed aria-label=Closed oninput="if (window.cs) window.cs.querySelector('span').textContent = 'closed changed'">
<label><input type=checkbox id=agree> Agree</label>
<label>Size <select id=size><option>Small</option><option>Medium</option><option>Large</option></select></label>
<input type=radio name=ship id=ground aria-label=Ground checked><input type=radio name=ship id=air aria-label=Air>
<label><input type=checkbox id=later> Later</label>
<button id=go onclick="document.getElementById('out').textContent = 'clicked ' + (++window.k)">Go</button>
<span id=out>idle</span>
<script>
window.k = 0;
// ?closed: a closed shadow root, which only the snapshot reads.
if (location.search.includes('closed')) {
  window.cs = document.getElementById('host').attachShadow({ mode: 'closed' });
  window.cs.innerHTML = '<span>closed idle</span>';
}
</script>
""" + "".join(f"<p>row {i} of the long page</p>" for i in range(1300)))


# Many more style rules than elements (a site's whole CSS bundle on a small
# page): a read leaves walking them to the first action.
PAGES["/many-rules"] = ("many rules",
    "".join(f".p{i}{{color:#{i % 4096:03x}}}" for i in range(12000)) + " .row:hover{cursor:pointer}", """
<button id=add onclick="const li = document.createElement('li'); li.className = 'row'; li.textContent = 'Added row'; rows.prepend(li)">Add</button>
<ul id=rows></ul>
<script>
rows.innerHTML = Array.from({length: 2000}, (_, i) => `<li class=row>Row ${i}</li>`).join('');
</script>""")

PAGES["/clicks"] = ("clicks", ".menu{display:none} [aria-expanded=true] + .menu{display:block} .hidden{display:none} "
    ".fade{opacity:0;transition:opacity 100ms} .fade.in{opacity:1} .arrow::before{content:\"\\2192  \"}", """
<button id=edit onclick="document.querySelector('#r5 td:nth-child(2)').textContent = 'edited ' + (++window.k)">Edit</button>
<button id=add onclick="const tr = document.createElement('tr'); tr.innerHTML = '<td>NEW</td><td>Inserted</td><td><button>Act</button></td>'; document.querySelector('#rows tbody').insertBefore(tr, document.getElementById('r3'))">Add</button>
<button id=remove onclick="document.getElementById('r4').remove()">Remove</button>
<nav><button id=menu aria-expanded=false onclick="this.setAttribute('aria-expanded', 'true')">Menu</button><ul class=menu><li><a href=#a>Alpha</a></li></ul></nav>
<button id=show onclick="document.getElementById('box').classList.remove('hidden')">Show</button>
<div id=box class=hidden><h2>Hidden section</h2><a href=#h>hidden link</a></div>
<details id=more><summary>More</summary><p>Details body</p></details>
<button id=det onclick="const d = document.getElementById('more'); d.open = !d.open">Toggle details</button>
<button id=dlg onclick="document.getElementById('dialog').show()">Dialog</button>
<dialog id=dialog><p>Dialog text</p></dialog>
<button id=pop onclick="document.getElementById('popover').showPopover()">Popover</button>
<div id=popover popover><p>Popover body</p></div>
<button id=redraw onclick="document.getElementById('list').innerHTML = Array.from({length: 20}, (_, i) => '<p class=arrow>Item ' + i + ' <a href=#' + i + '>open</a></p>').join('')">Redraw</button>
<div id=list></div>
<button id=wire onclick="document.getElementById('wired').addEventListener('click', () => {})">Wire</button>
<div id=wired>Wire me</div>
<button id=val onclick="document.getElementById('field').value = 'set by script'">Value</button>
<input id=field aria-label=Field>
<button id=fade onclick="document.getElementById('faded').classList.add('in')">Fade</button>
<div id=faded class=fade><button>Faded</button></div>
<button id=ol onclick="const li = document.createElement('li'); li.textContent = 'zeroth'; document.getElementById('steps').prepend(li)">List</button>
<ol id=steps><li>first</li><li>second</li></ol>
<script>window.k = 0;</script>
<table id=rows><tbody>
""" + "\n".join(f"<tr id=r{i}><td>SKU-{i:05d}</td><td>Item {i}</td><td>{i % 97}.99</td></tr>" for i in range(2000)) + "</tbody></table>")


PAGES["/xo-child"] = ("card frame", "", """
<label>Card number <input id=cardnumber autocomplete=off></label>
<label><input type=checkbox id=save> Save card</label>
<select id=exp><option>01/27</option><option>02/28</option></select>
<button id=pay type=button>Pay</button>
<p id=status>idle</p>
<script>
document.getElementById('pay').onclick = () => {
  const n = document.getElementById('cardnumber').value;
  document.getElementById('status').textContent = 'sent ' + n;
  parent.postMessage({ paid: n.slice(-4) + (document.getElementById('save').checked ? ' saved' : '') + ' ' + document.getElementById('exp').value }, '*');
};
</script>""")


def slow_page(ms):
    return ("slow", "", f"""
<p>Loading…</p>
<script>
setTimeout(() => {{
  const b = document.createElement('button');
  b.id = 'late';
  b.textContent = 'Late button';
  b.onclick = () => {{ document.querySelector('p').textContent = 'late clicked'; }};
  document.body.appendChild(b);
}}, {ms});
</script>""")


BUMPS = 0
BUMPS_LOCK = __import__("threading").Lock()


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def bump(self):
        # An effect outside the browser: no relaunch or storage snapshot
        # can mask it, so a replayed command shows here.
        with BUMPS_LOCK:
            global BUMPS
            BUMPS += 1
            n = BUMPS
        self.send(str(n), ctype="text/plain")

    def send(self, body, ctype="text/html; charset=utf-8", extra=()):
        data = body.encode()
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        for k, v in extra:
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(data)

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        self.rfile.read(length)
        if urlparse(self.path).path == "/bump":
            self.bump()
            return
        if urlparse(self.path).path == "/collect":
            self.send_response(204)
            self.end_headers()
            return
        # A page a form POST loads (an empty 404 would become Chrome's own
        # error page, which has no path of its own).
        if urlparse(self.path).path == "/posted":
            body = b"<!doctype html><title>posted</title><p>posted</p>"
            self.send_response(200)
            self.send_header("Content-Type", "text/html")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.send_response(404)
        self.end_headers()

    def do_GET(self):
        if urlparse(self.path).path == "/bumps":
            with BUMPS_LOCK:
                n = BUMPS
            self.send(str(n), ctype="text/plain")
            return
        u = urlparse(self.path)
        q = parse_qs(u.query, keep_blank_values=True)
        if u.path == "/frameset":
            return self.send(FRAMESET)
        if u.path == "/frame-top":
            return self.send(FRAME_TOP)
        if u.path == "/frame-text":
            t = q.get("t", ["?"])[0]
            return self.send(f"<!doctype html><html><body><div id=content>{t}</div></body></html>")
        if u.path.startswith("/event/"):
            return self.send("ok", "text/plain")
        if u.path == "/hold":
            import time
            time.sleep(int(q.get("ms", ["2000"])[0]) / 1000)
            return self.send("{}", "application/json")
        if u.path == "/journal-import.css":
            return self.send(".itip{display:none} #imported:focus ~ .itip{display:inline}", "text/css")
        if u.path == "/late-script":
            import time
            time.sleep(int(q.get("ms", ["300"])[0]) / 1000)
            return self.send("wireGo();", "text/javascript")
        if u.path == "/slow-echo":
            import time
            time.sleep(int(q.get("ms", ["100"])[0]) / 1000)
            return self.send("ok", "text/plain")
        if u.path == "/file.csv":
            return self.send("id,name\n1,Ada\n2,Grace\n", "text/csv",
                             [("Content-Disposition", 'attachment; filename="report.csv"')])
        if u.path == "/no-content":
            self.send_response(204)
            self.end_headers()
            return
        if u.path == "/slow":
            page = slow_page(int(q.get("ms", ["2000"])[0]))
        elif u.path == "/twin-frames":
            page = twin_frames(self.server.server_address[1])
        elif u.path == "/xo-parent":
            # localhost and 127.0.0.1 are different sites: the frame is
            # cross-site, so Chrome runs it in a process of its own.
            # `?same-site` keeps it cross-origin but in the same process.
            port = self.server.server_address[1]
            page = xo_parent(port, "127.0.0.1" if "same-site" in q else "localhost")
            if "same-site" in q:
                page = (page[0], page[1], page[2].replace(f"http://127.0.0.1:{port}/xo-child", f"http://127.0.0.1:{XO_PORT[0]}/xo-child"))
        elif u.path in PAGES:
            page = PAGES[u.path]
        else:
            self.send_response(404)
            self.end_headers()
            return
        title, style, body = page
        self.send(HEAD.format(title=title, style=style) + body + "</body></html>")


XO_PORT = [0]

if __name__ == "__main__":
    # A second port: another origin on the same site (127.0.0.1).
    xo = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    XO_PORT[0] = xo.server_address[1]
    import threading
    threading.Thread(target=xo.serve_forever, daemon=True).start()
    srv = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    print(f"LISTENING http://127.0.0.1:{srv.server_address[1]}", flush=True)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass
    sys.exit(0)
