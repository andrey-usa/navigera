// Page.captureScreenshot of a 1280x720 view, three ways of getting that
// view and with and without `optimizeForSpeed` (Chrome DevTools MCP passes
// it; its `resize_page` is Browser.setContentsSize, navigera's
// `set viewport` is Emulation.setDeviceMetricsOverride):
//   plain     the window's own 1400x900 view, no override (the reference)
//   emulated  a 1280x720 device metrics override in that window
//   contents  the window's content area resized to 1280x720
// Arms interleaved in rounds; one notice line.
//   node shot_probe.mjs CHROME URL [ROUNDS] [PER_ROUND]
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const [chrome, url, rounds = "10", per = "6"] = process.argv.slice(2);
const profile = mkdtempSync(join(tmpdir(), "shot-probe-"));
const proc = spawn(chrome, ["--headless=new", "--no-sandbox", "--remote-debugging-port=0", "--window-size=1400,900",
  `--user-data-dir=${profile}`, "about:blank"], { stdio: ["ignore", "ignore", "pipe"] });
const wsUrl = await new Promise((ok) => {
  let b = "";
  proc.stderr.on("data", (d) => { b += d; const m = b.match(/ws:\/\/\S+/); if (m) ok(m[0]); });
});
const ws = new WebSocket(wsUrl);
await new Promise((ok) => { ws.onopen = ok; });
let id = 0;
const wait = new Map();
ws.onmessage = (e) => { const m = JSON.parse(e.data); if (m.id && wait.has(m.id)) { wait.get(m.id)(m); wait.delete(m.id); } };
const send = (method, params = {}, sessionId) => new Promise((ok) => {
  const i = ++id; wait.set(i, ok); ws.send(JSON.stringify({ id: i, method, params, sessionId }));
});
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const page = (await send("Target.getTargets")).result.targetInfos.find((t) => t.type === "page");
const s = (await send("Target.attachToTarget", { targetId: page.targetId, flatten: true })).result.sessionId;
await send("Page.enable", {}, s);
await send("Page.navigate", { url }, s);
await sleep(1500);
const { windowId } = (await send("Browser.getWindowForTarget", { targetId: page.targetId })).result;
const view = { plain: [1400, 900], emulated: [1280, 720], contents: [1280, 720] };
const setup = {
  plain: async () => {
    await send("Emulation.clearDeviceMetricsOverride", {}, s);
    await send("Browser.setContentsSize", { windowId, width: 1400, height: 900 });
  },
  emulated: async () => {
    await send("Browser.setContentsSize", { windowId, width: 1400, height: 900 });
    await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 720, deviceScaleFactor: 1, mobile: false }, s);
  },
  contents: async () => {
    await send("Emulation.clearDeviceMetricsOverride", {}, s);
    const r = await send("Browser.setContentsSize", { windowId, width: 1280, height: 720 });
    if (r.error) console.log(`::warning::Browser.setContentsSize: ${JSON.stringify(r.error)}`);
  },
};
const arms = Object.keys(setup).flatMap((k) => [k, `${k}-fast`]);
const got = Object.fromEntries(arms.map((a) => [a, { ms: [], bytes: [], seen: "" }]));
for (let r = 0; r < Number(rounds); r++) {
  const order = arms.slice(r % arms.length).concat(arms.slice(0, r % arms.length));
  for (const a of order) {
    const base = a.replace(/-fast$/, "");
    await setup[base]();
    await sleep(300);
    got[a].seen = (await send("Runtime.evaluate", { expression: "innerWidth + 'x' + innerHeight", returnByValue: true }, s)).result.result.value;
    for (let i = 0; i < Number(per); i++) {
      const t = performance.now();
      const res = await send("Page.captureScreenshot", { format: "png", optimizeForSpeed: a.endsWith("-fast") }, s);
      got[a].ms.push(performance.now() - t);
      got[a].bytes.push(Buffer.from(res.result.data, "base64").length);
      await sleep(50);
    }
  }
}
const q = (xs, p) => { const b = [...xs].sort((x, y) => x - y); return b[Math.min(b.length - 1, Math.floor(b.length * p))]; };
const version = (await send("Browser.getVersion")).result.product;
console.log(`::notice title=captureScreenshot median [IQR] ms, ${rounds}x${per} per arm, ${version}::` + arms.map((a) => {
  const g = got[a];
  return `${a} (${g.seen}, want ${view[a.replace(/-fast$/, "")].join("x")}) ${q(g.ms, 0.5).toFixed(2)} [${q(g.ms, 0.25).toFixed(2)}-${q(g.ms, 0.75).toFixed(2)}] ${Math.round(q(g.bytes, 0.5) / 1024)} KiB`;
}).join(" · "));
ws.close();
proc.kill();
await sleep(500);
rmSync(profile, { recursive: true, force: true });
