// Where does a Page.captureScreenshot call's time go, at 1280x720 vs
// 1440x813, with the default PNG encoder vs `optimizeForSpeed`?
// Raw CDP against Chrome on the runner (modelled on shot_probe.mjs):
//   sizes: 1280x720 and 1440x813 via Emulation.setDeviceMetricsOverride,
//          plus 1440x813 unemulated (Browser.setContentsSize)
//   arms: each size x optimizeForSpeed false/true, 20 interleaved captures
//   per arm (rounds x per-round, arm order rotated each round)
//   per capture: Tracing.start (devtools,viz,gpu,toplevel,cc,
//          transferMode ReturnAsStream), the capture, Tracing.end; the
//          trace is read back over IO and reduced to the encode event
//          (EncodeBitmapAsPngFast/Slow), copy-output/readback events, and
//          the longest events, plus the call wall and the PNG bytes.
// Tracing is on for every arm, so its overhead is common to all of them;
// the wall is timed around Page.captureScreenshot only.
//   node trace_probe.mjs CHROME URL [ROUNDS] [PER_ROUND] [OUTDIR]
// One notice line per arm, one for the environment; per-capture extracts
// and the first full trace of each arm go to OUTDIR as JSON.
import { spawn, execSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir, cpus } from "node:os";
import { join } from "node:path";

const [chrome, url, rounds = "5", per = "4", outdir = "out"] = process.argv.slice(2);
if (!chrome || !url) { console.error("usage: node trace_probe.mjs CHROME URL [ROUNDS] [PER_ROUND] [OUTDIR]"); process.exit(2); }
mkdirSync(outdir, { recursive: true });
const profile = mkdtempSync(join(tmpdir(), "trace-probe-"));
const proc = spawn(chrome, ["--headless=new", "--no-sandbox", "--remote-debugging-port=0", "--window-size=1440,813",
  `--user-data-dir=${profile}`, "about:blank"], { stdio: ["ignore", "ignore", "pipe"] });
const wsUrl = await new Promise((ok, fail) => {
  let b = "";
  const t = setTimeout(() => fail(new Error("no DevTools ws url in 30 s")), 30000);
  proc.stderr.on("data", (d) => { b += d; const m = b.match(/ws:\/\/\S+/); if (m) { clearTimeout(t); ok(m[0]); } });
});
const ws = new WebSocket(wsUrl);
await new Promise((ok) => { ws.onopen = ok; });
let id = 0;
const wait = new Map();
const events = []; // notifications, in arrival order
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.id && wait.has(m.id)) { wait.get(m.id)(m); wait.delete(m.id); }
  else if (m.method) events.push(m);
};
const send = (method, params = {}, sessionId) => new Promise((ok, rej) => {
  const i = ++id; wait.set(i, ok); ws.send(JSON.stringify({ id: i, method, params, sessionId }));
  setTimeout(() => { if (wait.has(i)) { wait.delete(i); rej(new Error(`timeout ${method}`)); } }, 60000);
});
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const page = (await send("Target.getTargets")).result.targetInfos.find((t) => t.type === "page");
const s = (await send("Target.attachToTarget", { targetId: page.targetId, flatten: true })).result.sessionId;
await send("Page.enable", {}, s);
await send("IO.enable", {}, s); // the Tracing stream is read back on this session
await send("Page.navigate", { url }, s);
await sleep(1500);
const { windowId } = (await send("Browser.getWindowForTarget", { targetId: page.targetId })).result;

// Wait for a Tracing.tracingComplete notification (arrives after Tracing.end
// when transferMode is ReturnAsStream); returns its stream handle, or null.
async function tracingStream(timeoutMs = 30000) {
  const start = Date.now();
  for (;;) {
    const i = events.findIndex((e) => e.method === "Tracing.tracingComplete");
    if (i >= 0) return events.splice(i, 1)[0].params.stream || null;
    if (Date.now() - start > timeoutMs) return null;
    await sleep(50);
  }
}
async function readStream(handle, sessionId) {
  let text = "";
  for (;;) {
    const r = await send("IO.read", { handle, size: 1048576 }, sessionId);
    if (r.error) throw new Error(`IO.read: ${JSON.stringify(r.error)}`);
    text += r.result.base64Encoded ? Buffer.from(r.result.data, "base64").toString("utf8") : r.result.data;
    if (r.result.eof) break;
  }
  await send("IO.close", { handle }, sessionId).catch(() => {});
  return text;
}
const INTEREST = /encode|copy|readback|screenshot|capture|beginframe|draw|swap|commit|present|raster|gpu/i;
function reduce(traceText) {
  let evs = [];
  try { evs = JSON.parse(traceText).traceEvents || []; } catch { /* fall through to regex scan */ }
  const enc = evs.filter((e) => /^EncodeBitmapAsPng/.test(e.name || ""));
  const hits = evs.filter((e) => INTEREST.test(e.name || "") && !/^EncodeBitmapAsPng/.test(e.name || ""));
  const top = [...evs].filter((e) => typeof e.dur === "number")
    .sort((a, b) => b.dur - a.dur).slice(0, 8)
    .map((e) => ({ name: e.name, cat: e.cat, dur_us: e.dur, pid: e.pid, tid: e.tid }));
  const names = [...new Set(evs.map((e) => e.name))].sort();
  return { n_events: evs.length, encode: enc.map((e) => ({ name: e.name, dur_us: e.dur })),
    hits: hits.map((e) => ({ name: e.name, dur_us: e.dur })), top, names,
    raw_has_encode: /EncodeBitmapAsPng/.test(traceText) };
}

const view = { "720": [1280, 720], "813emu": [1440, 813], "813plain": [1440, 813] };
const setup = {
  "720": async () => {
    await send("Browser.setContentsSize", { windowId, width: 1440, height: 813 });
    await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 720, deviceScaleFactor: 1, mobile: false }, s);
  },
  "813emu": async () => {
    await send("Browser.setContentsSize", { windowId, width: 1440, height: 813 });
    await send("Emulation.setDeviceMetricsOverride", { width: 1440, height: 813, deviceScaleFactor: 1, mobile: false }, s);
  },
  "813plain": async () => {
    await send("Emulation.clearDeviceMetricsOverride", {}, s);
    await send("Browser.setContentsSize", { windowId, width: 1440, height: 813 });
  },
};
const arms = Object.keys(setup).flatMap((k) => [k, `${k}-fast`]);
const got = Object.fromEntries(arms.map((a) => [a, { ms: [], bytes: [], enc_ms: [], enc_name: "", seen: "", rb_ms: [], failed: 0, trace_failed: 0, captures: [] }]));
const R = Number(rounds), P = Number(per);
for (let r = 0; r < R; r++) {
  const order = arms.slice(r % arms.length).concat(arms.slice(0, r % arms.length));
  for (const a of order) {
    const base = a.replace(/-fast$/, "");
    const g = got[a];
    try {
      await setup[base]();
      await sleep(300);
      g.seen = (await send("Runtime.evaluate", { expression: "innerWidth + 'x' + innerHeight", returnByValue: true }, s)).result.result.value;
      for (let i = 0; i < P; i++) {
        let cap, stream = null, red = null, wall = NaN;
        try {
          await send("Tracing.start", { categories: "devtools,viz,gpu,toplevel,cc", transferMode: "ReturnAsStream" }, s);
          const t = performance.now();
          cap = await send("Page.captureScreenshot", { format: "png", optimizeForSpeed: a.endsWith("-fast") }, s);
          wall = performance.now() - t;
          await send("Tracing.end", {}, s);
          stream = await tracingStream();
          if (stream) red = reduce(await readStream(stream, s));
          else g.trace_failed++;
        } catch (e) { console.log(`::warning::${a} capture ${r}x${i}: ${e.message}`); g.trace_failed++; }
        if (cap && !cap.error) {
          g.ms.push(wall);
          g.bytes.push(Buffer.from(cap.result.data, "base64").length);
          if (red) {
            if (red.encode.length) { g.enc_ms.push(red.encode[0].dur_us / 1000); g.enc_name = red.encode[0].name; }
            const rb = red.hits.filter((h) => /copy|readback/i.test(h.name)).reduce((m, h) => Math.max(m, (h.dur_us || 0) / 1000), 0);
            if (rb) g.rb_ms.push(rb);
            g.captures.push({ round: r, i, wall_ms: +wall.toFixed(3), png_bytes: g.bytes[g.bytes.length - 1], encode: red.encode, hits: red.hits, top: red.top });
            if (r === 0 && i === 0) writeFileSync(join(outdir, `trace-${a}.json`), JSON.stringify({ arm: a, trace_events: red.names, top: red.top }, null, 1));
          }
        } else g.failed++;
        await sleep(50);
      }
    } catch (e) { console.log(`::warning::${a} round ${r}: ${e.message}`); g.failed += P; }
  }
}
const q = (xs, p) => { if (!xs.length) return NaN; const b = [...xs].sort((x, y) => x - y); return b[Math.min(b.length - 1, Math.floor(b.length * p))]; };
const med = (xs) => q(xs, 0.5);
const version = (await send("Browser.getVersion")).result.product;
const cpu = execSync("grep -m1 'model name' /proc/cpuinfo | cut -d: -f2", { encoding: "utf8" }).trim();
console.log(`::notice title=env::${version} · ${process.env.ImageOS || "runner?"} ${process.env.ImageVersion || ""} · ${cpu} · ${cpus().length} cpus · kernel ${execSync("uname -r", { encoding: "utf8" }).trim()}`);
const summary = {};
for (const a of arms) {
  const g = got[a];
  const enc = med(g.enc_ms), wall = med(g.ms), rb = med(g.rb_ms);
  summary[a] = { seen: g.seen, n: g.ms.length, failed: g.failed, trace_failed: g.trace_failed,
    wall_ms_median: +wall.toFixed(2), wall_ms_iqr: [+q(g.ms, 0.25).toFixed(2), +q(g.ms, 0.75).toFixed(2)],
    png_kib_median: Math.round(med(g.bytes) / 1024),
    encode: g.enc_name, encode_ms_median: Number.isNaN(enc) ? null : +enc.toFixed(2),
    encode_share_pct: Number.isNaN(enc) ? null : +(100 * enc / wall).toFixed(1),
    readback_ms_median: Number.isNaN(rb) ? null : +rb.toFixed(2),
    readback_share_pct: Number.isNaN(rb) ? null : +(100 * rb / wall).toFixed(1) };
  console.log(`::notice title=${a} (${g.seen}, want ${view[a.replace(/-fast$/, "")].join("x")}, n=${g.ms.length})::` +
    `wall ${wall.toFixed(2)} [${q(g.ms, 0.25).toFixed(2)}-${q(g.ms, 0.75).toFixed(2)}] ms · ` +
    `${g.enc_name ? `${g.enc_name.replace("EncodeBitmapAsPng", "")} ${enc.toFixed(2)} ms (${(100 * enc / wall).toFixed(0)}% of wall)` : "NO encode event in trace"} · ` +
    `${Number.isNaN(rb) ? "no copy/readback event" : `copy/readback ${rb.toFixed(2)} ms (${(100 * rb / wall).toFixed(0)}%)`} · ${Math.round(med(g.bytes) / 1024)} KiB PNG` +
    (g.failed ? ` · ${g.failed} capture-failed` : "") + (g.trace_failed ? ` · ${g.trace_failed} trace-failed` : ""));
  writeFileSync(join(outdir, `captures-${a}.json`), JSON.stringify(g.captures, null, 1));
}
writeFileSync(join(outdir, "summary.json"), JSON.stringify({ chrome: version, cpu, rounds: R, per_round: P, url, arms: summary }, null, 1));
const anyEncode = arms.some((a) => got[a].enc_ms.length > 0);
if (!anyEncode) console.log("::warning::no EncodeBitmapAsPng event in any trace — the trace categories did not capture the encoder; check the per-arm trace-*.json event names");
ws.close();
proc.kill();
await sleep(500);
rmSync(profile, { recursive: true, force: true });
