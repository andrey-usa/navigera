"""Write the benchmark docs from a ladder publish.json.

Usage: gen_readme.py README.md publish.json ["what the nv-baseline A/B changed"]
                     [--run ID] [--page PATH [--losses PATH]]

Numbers in the docs come from a run's publish.json, never retyped. Each
generated table's heading is followed by a link to the run: ID from --run,
else the run_id the ladder recorded in publish.json.

With --page (docs/benchmarks.md), every table goes to that page, and the
measures where another tool beat navigera in the run to a section closing
it, or to the page --losses names (docs/losses.md); README.md's `## Benchmarks` gets the headline: one table of
the main drivers, the per-call time of the agent tools, the same list of
losses, and a link to the page. Without it, every table goes to README.md
(how runs before the docs site were written)."""
import json
import re
import sys

args = sys.argv[1:]
opts = {}
for flag in ("--run", "--page", "--losses"):
    if flag in args:
        i = args.index(flag)
        opts[flag] = args[i + 1]
        del args[i:i + 2]
run_arg, page_path, losses_path = opts.get("--run"), opts.get("--page"), opts.get("--losses")
readme_path, pub_path = args[0], args[1]
change_note = args[2].strip() if len(args) > 2 else ""
doc = json.load(open(pub_path))
run_id = run_arg or doc.get("run_id")
if not run_id:
    sys.exit("publish.json records no run_id: pass --run <id> so each table can link its run")
numbers = (f"Numbers: [bench run {run_id}]"
           f"(https://github.com/andrey-usa/navigera/actions/runs/{run_id})")
all_rows = doc["contenders"]
baseline = next((r for r in all_rows if r["name"] == "nv-baseline"), None)
ws = next((r for r in all_rows if r["name"] == "nv-ws"), None)
# A/B rows (same build over another transport, the previous build) get a
# sentence each, not a row in the ranking.
rows = [r for r in all_rows if r["name"] not in ("nv-baseline", "nv-ws")]
versions = doc.get("versions") or {}


def ver(key, fallback):
    """A contender's version as the run recorded it (older runs: none)."""
    return versions.get(key, fallback).lstrip("v")


NAMES = {
    "nv-serve": "`nv-serve` (this repo on Chrome)",
    "nv-shell": "`nv-shell` (this repo on chrome-headless-shell)",
    "nv-brave": "`nv-brave` (this repo on Brave)",
    "nv-edge": "`nv-edge` (this repo on Edge)",
    "nv-lightpanda": "`nv-lightpanda` (this repo on Lightpanda, 1 tab)",
    "gorod": f"`gorod` (go-rod {ver('go-rod', '0.116.2')}, Go)",
    "chromedp": f"`chromedp` {ver('chromedp', '0.19.1')} (Go)",
    "chromiumoxide": f"`chromiumoxide` {ver('chromiumoxide', '0.7')} (Rust)",
    "chromey": f"`chromey` {ver('chromey', '2.x')} (Rust, maintained chromiumoxide fork)",
    "playwright-bun": f"`playwright-core` {ver('playwright-core', '')} (Bun {ver('bun', '')})".replace("  ", " ").replace("(Bun )", "(Bun)"),
    "puppeteer": f"`puppeteer-core` {ver('puppeteer-core', '')} (Node)".replace("  ", " "),
    "playwright": f"`playwright-core` {ver('playwright-core', '')} (Node)".replace("  ", " "),
}


def cpu(v):
    return "<10 ms" if v < 0.005 else (f"{v * 1000:.0f} ms" if v < 0.1 else f"{v:.2f} s")


chrome = sorted([r for r in rows if not r["experimental"]], key=lambda r: r["session"]["wall_s"])
exp = [r for r in rows if r["experimental"]]
best = lambda key, rs: min(rs, key=key)
fast_wall = chrome[0]["session"]["wall_s"]
fast_eval = min(r["eval"]["mean_ms"] for r in chrome if r.get("eval"))


def mid(stage):
    """The wall that ranks a cold or live-site row: its median (runs from
    before medians were recorded: the best run)."""
    m = (stage or {}).get("median_wall_s")
    return m if m is not None else (stage or {}).get("wall_s")


def reps_of(key, fallback):
    return max((len(r[key].get("runs_wall_s") or []) for r in rows if r.get(key)), default=0) or fallback


fast_cold = min(mid(r["cold"]) for r in chrome if r.get("cold"))
cold_reps = reps_of("cold", 5)
low_cpu = min(r["session"]["driver_cpu_s"] for r in chrome)
low_rss = min(r["session"]["driver_rss_mb"] for r in chrome)


def mem(v):
    # 0 = the run ended between PSS samples (fixed in the ladder since).
    return f"{v:.0f} MB" if v else "n/a"


def b(text, cond):
    return f"**{text}**" if cond else text


def med(stage):
    """' (median)' when the run recorded one."""
    m = (stage or {}).get("median_wall_s")
    return f" ({m:.2f})" if m is not None else ""


def best_of(stage):
    """' (best)' after a median."""
    w = (stage or {}).get("wall_s")
    return f" ({w:.2f})" if w is not None else ""


lines = [
    "## Benchmarks",
    "",
    numbers + ".",
    "",
    "Same scripted session (launch → goto listing → title/count/extract evals →",
    "fill+click filter → visible-count eval → screenshot → new tab → goto detail →",
    "title/row-count evals → close) driven against the same headless browser by",
    "each driver. Fixtures are deterministic and local (500-card listing,",
    "1000-row detail). navigera is the static Linux binary the releases ship.",
    "Reps are interleaved across contenders. Run in CI via `.github/workflows/bench.yml`.",
    "",
    f"| contender | session wall, best (median) of {doc['reps']} | warm eval mean (200×) | cold start, median (best) of {cold_reps} | driver CPU (own) | driver peak RSS (own) | browser memory (PSS) |",
    "|---|---|---|---|---|---|---|",
]
for r in chrome + exp:
    s = r["session"]
    ev = r.get("eval", {}).get("mean_ms")
    cold = mid(r.get("cold"))
    lines.append(
        f"| {NAMES.get(r['name'], r['name'])} | {b(f'{s['wall_s']:.2f}s', s['wall_s'] == fast_wall)}{med(s)} | "
        f"{b(f'{ev:.2f} ms', ev == fast_eval and not r['experimental']) if ev is not None else '—'} | "
        f"{b(f'{cold:.2f}s', cold == fast_cold and not r['experimental']) + best_of(r.get('cold')) if cold is not None else '—'} | "
        f"{b(cpu(s['driver_cpu_s']), s['driver_cpu_s'] < 0.05)} | "
        f"{b(f'{s['driver_rss_mb']:.0f} MB', round(s['driver_rss_mb']) == round(low_rss))} | "
        f"{mem(s['browser_pss_mb'])} |"
    )
date = doc["generated_utc"][:10]
env = doc["env"].replace("**", "").replace("Env: ", "").replace("Chrome: Google Chrome ", "Chrome ")
lines += [
    "",
    # No link to the run: the ladder may run in a fork or a private mirror,
    # where the link would not resolve for readers.
    f"Measured {date} on GitHub-hosted `ubuntu-latest` ({env}).",
    "Every driver produced byte-identical extracted data and counts (correctness",
    "gate). Session wall is best-of-N (median in brackets); cold start and the",
    "live-site rows are ranked by their median, with the best run in brackets.",
    "The ladder reruns weekly ([`bench.yml`](https://github.com/andrey-usa/navigera/blob/master/.github/workflows/bench.yml)) against",
    "the latest contender releases. GitHub runners vary between runs, so compare",
    "rows within one run.",
    "",
    "**How driver CPU/RSS are measured.** Each driver's *own* `utime+stime`,",
    "read from its zombie's `/proc/<pid>/stat` before reaping (`waitid` with",
    "`WNOWAIT`), and its own VmHWM. The kernel counts CPU in 10 ms ticks, so",
    "values under ~50 ms are a tie. Not `wait4`: its rusage is RUSAGE_BOTH and",
    "adds in every child the driver reaped. navigera, chromedp, puppeteer",
    "and playwright `wait()` on their Chrome, so `wait4` charged them Chrome's",
    "CPU and RSS; go-rod (its leakless helper reaps Chrome) and chromiumoxide",
    "were never charged. Earlier versions of this table reported exactly that",
    "artifact (\"0.82s vs 0.03s\"). Browser memory is the whole browser process",
    "tree (every renderer/GPU/utility process), sampled as summed PSS every",
    "250 ms (shared pages counted once).",
    "",
]
order = ", ".join(f"{r['name']} {r['session']['wall_s']:.2f}s" for r in chrome)
nv_best = next(r for r in chrome if r["name"].startswith("nv-"))
serve_row = next((r for r in chrome if r["name"] == "nv-serve"), None)
other = next((r for r in chrome if not r["name"].startswith("nv-")), None)
rank = chrome.index(nv_best) + 1
shell_row = next((r for r in chrome if r["name"] == "nv-shell"), None)
# Rows that run a browser other than the regular Google Chrome.
off = [f"`{n}`" for n in ("nv-shell", "nv-edge", "nv-brave") if any(r["name"] == n for r in chrome)]
off_text = off[0] if len(off) == 1 else ", ".join(off[:-1]) + " and " + off[-1] if off else ""
lines += [
    f"**Reading the table.** Session wall order: {order}"
    + (f". Every driver{' but ' + off_text if off else ''} runs the same Google Chrome; on it, nv-serve ({serve_row['session']['wall_s']:.2f}s) is "
       + ("ahead of" if serve_row["session"]["wall_s"] < other["session"]["wall_s"] else "behind")
       + f" the fastest other driver, {other['name']} ({other['session']['wall_s']:.2f}s)"
       if serve_row and other else "")
    + (f". `nv-shell` ({shell_row['session']['wall_s']:.2f}s) is navigera on chrome-headless-shell, a"
       " lighter build of the same browser that no other row runs, so its lead is partly the"
       " browser's" if shell_row else "")
    + ". All native drivers (navigera,",
    "go-rod, chromiumoxide, chromedp) spend tens of milliseconds of their own CPU",
    "or less; the Node drivers spend hundreds and carry 80–150 MB of their own RSS.",
    "",
]
rw = [r for r in chrome if r.get("realworld")]
br = [r for r in chrome if r.get("browse")]
if rw and br:
    rw_ok = sum(1 for r in rw if r["realworld"]["ok"])
    br_ok = sum(1 for r in br if r["browse"]["ok"])
    live_reps = reps_of("realworld", 3)
    live_how = "median" if any(r["realworld"].get("median_wall_s") is not None for r in rw) else "best"

    def live(rs, key):
        ranked = sorted((r for r in rs if mid(r[key]) is not None), key=lambda r: mid(r[key]))
        return ", ".join(f"{r['name']} {mid(r[key]):.2f}s" for r in ranked)
    lines += [
        f"### Real-world (public internet, {live_how} of {live_reps}, same run)",
        "",
        numbers + ".",
        "",
        f"example.com goto → title check: {rw_ok} of {len(rw)} pass; " + live(rw, "realworld") + ".",
        "",
        f"GitHub browse (awesome-list scroll + trending click-through): {br_ok} of {len(br)} pass; "
        + live(br, "browse") + ".",
        "navigera's click returns once the new page has settled; the other drivers'",
        "clicks return at once and they then poll the URL, so the browse rows do not",
        "time the same readiness. The example.com check is the title; h1 is read, not checked.",
        "Live pages change between runs, so these are informational, not part of the",
        "correctness gate. (The scroll check used to fail at random for every driver:",
        "GitHub sets CSS `scroll-behavior: smooth`, so `scrollY` was read mid-animation;",
        "contenders now scroll with `behavior: 'instant'`.)",
        "",
    ]
serve = next((r for r in rows if r["name"] == "nv-serve"), None)
shell = next((r for r in rows if r["name"] == "nv-shell"), None)
if shell and serve and shell.get("cold") and serve.get("cold"):
    lines += [
        "**chrome-headless-shell** (the same Chrome build without its browser UI",
        f"layer): cold start {mid(serve['cold']):.2f}s → {mid(shell['cold']):.2f}s (medians), session "
        f"{serve['session']['wall_s']:.2f}s → {shell['session']['wall_s']:.2f}s, browser memory "
        f"{mem(serve['session']['browser_pss_mb'])} → {mem(shell['session']['browser_pss_mb'])}.",
        "",
    ]
if ws and serve and ws.get("cold") and serve.get("cold"):
    spread = serve["session"].get("runs_wall_s") or []
    lines += [
        "**CDP transport A/B** (same build and run): `--remote-debugging-pipe` (default on Linux/macOS)",
        f"vs a DevTools WebSocket port: cold start {mid(serve['cold']):.2f}s vs {mid(ws['cold']):.2f}s (medians),",
        f"session {serve['session']['wall_s']:.2f}s vs {ws['session']['wall_s']:.2f}s"
        + (". No speed difference"
           if abs(serve['session']['wall_s'] / ws['session']['wall_s'] - 1) < 0.05
           else (f", within run-to-run noise (the pipe's {len(spread)} sessions took "
                 f"{min(spread):.2f}–{max(spread):.2f}s)" if spread else ", within run-to-run noise"))
        + ": the WebSocket handshake itself is ~10 ms. The pipe is the default for two other reasons:",
        "it opens no TCP port that another local process could attach to, and Chrome exits when",
        "navigera dies (EOF on its command pipe), so a killed agent leaks no browser",
        "(`tests/edge_cases.rs`: over a WebSocket port the browser outlives its driver).",
        "Windows has no pipe transport here and uses the WebSocket, with a kill-on-close",
        "job object giving the same guarantee (`killed_session_server_takes_its_browser_down`",
        "runs there too).",
        "",
    ]
if baseline and serve:
    def pct(new, old):
        return f"{100 * (new / old - 1):+.0f}%"
    ev_old = (baseline.get("eval") or {}).get("mean_ms")
    ev_new = (serve.get("eval") or {}).get("mean_ms")
    lines += [
        f"**This build vs {versions.get('nv-baseline', 'the previous one')}, same machine and run** (`nv-baseline`,",
        "built the same way by the ladder's `baseline_ref` A/B):",
        f"session {baseline['session']['wall_s']:.2f}s → {serve['session']['wall_s']:.2f}s "
        f"({pct(serve['session']['wall_s'], baseline['session']['wall_s'])}), "
        f"cold start {mid(baseline['cold']):.2f}s → {mid(serve['cold']):.2f}s median "
        f"({pct(mid(serve['cold']), mid(baseline['cold']))}), "
        + (f"warm eval {ev_old:.2f} ms → {ev_new:.2f} ms ({pct(ev_new, ev_old)}), " if ev_old and ev_new else "")
        + f"browser CPU per session {baseline['session']['browser_cpu_s']:.2f}s → "
        f"{serve['session']['browser_cpu_s']:.2f}s." + (f" {change_note}" if change_note else ""),
        "",
    ]
    ph = (serve.get("cold") or {}).get("phases_ms") or {}
    if ph:
        lines += [
            "Where navigera's cold start goes (`NAVIGERA_TIMINGS`, best run): browser up",
            f"{ph.get('browser_up', 0):.0f} ms, first page {ph.get('first_page', 0):.0f} ms, "
            f"close {ph.get('close', 0):.0f} ms.",
            "",
        ]
label = {
    "navigera": "`navigera` (this repo)",
    "navigera (batch)": "`navigera`, known steps in `batch`",
    "navigera (headless shell)": "`navigera` on chrome-headless-shell",
    "agent-browser": f"`agent-browser` {ver('agent-browser', '')} (Vercel Labs, Rust)".replace("  ", " "),
    "playwright-cli": f"`playwright-cli` {ver('@playwright/cli', '')} (Microsoft)".replace("  ", " "),
    "Playwright MCP": f"Playwright MCP {ver('@playwright/mcp', '')} (Microsoft)".replace("  ", " "),
    "Chrome DevTools MCP": f"Chrome DevTools MCP {ver('chrome-devtools-mcp', '')} (Google)".replace("  ", " "),
}
# Diagnostic agent rows (the A/B baseline build, a browser of its own) stay
# out of the README.
agent = [a for a in doc.get("agent_cli") or []
         if a["name"] not in ("navigera (baseline)", "navigera (own browser)")]
if agent:
    lines += [
        "### Agent tools (scripted, warm session, best of 3)",
        "",
        numbers + ".",
        "",
        "How an agent's tool calls drive a browser: CLI tools run one process per",
        "step (a shell tool), MCP servers get one `tools/call` per step over a",
        "persistent connection. Same canonical session plus one page snapshot.",
        "Snapshot sizes are each tool's default output, what the agent reads.",
        "Outside the session's total, each tool also snapshots the 1000-row page,",
        "and navigera starts a second agent's session while the first one's browser",
        "is up (sessions share one browser, a context each).",
        "",
        "| tool | kind | calls | total wall | per call | snapshot | its size | 1000-row snapshot | its size | 2nd session start | gate |",
        "|---|---|---|---|---|---|---|---|---|---|---|",
    ]
    for a in agent:
        kind = "MCP" if "MCP" in a["name"] else "CLI"
        if not a.get("wall_s"):
            lines.append(f"| {label.get(a['name'], a['name'])} | {kind} | — | failed | — | — | — | — | — | — | ✗ |")
            continue
        steps = {s_['op']: s_['ms'] for s_ in a.get("steps") or []}
        big = steps.get("snapshot b")
        warm = steps.get("start warm")
        lines.append(
            f"| {label.get(a['name'], a['name'])} | {kind} | {a.get('calls') or '—'} | {a['wall_s']:.2f}s | "
            f"{a['op_mean_ms']:.0f} ms | {steps.get('snapshot', 0):.0f} ms | {a['snapshot_bytes'] / 1024:.0f} KB | "
            + (f"{big:.0f} ms | {(a.get('snapshot_b_bytes') or 0) / 1024:.0f} KB | " if big is not None else "— | — | ")
            + (f"{warm:.0f} ms | " if warm is not None else "— | ")
            + f"{'✓' if a['ok'] else '✗'} |")
    lines += [
        "",
        "Per-call time includes process start (CLI) or JSON-RPC (MCP), the hop to",
        "the daemon, and the CDP work. playwright-cli and Playwright MCP share Playwright's tool",
        "backend, which waits a fixed 500 ms after every action (`timeouts.settle`); playwright-cli",
        "also starts a Node process per step. `batch` saves calls, which are an agent's turns,",
        "not time: its row runs the same steps in fewer, longer calls.",
        "",
    ]
par = doc.get("parallel") or []
if par:
    ns = [r_["n"] for r_ in par[0]["runs"]]
    plabel = {**label, "navigera": "`navigera` (sessions share one browser)",
              "navigera (own browser)": "`navigera --own-browser` (a browser each)"}
    lines += [
        "### Parallel agents (memory, same run)",
        "",
        numbers + ".",
        "",
        "N agents, each with a session of its own (navigera `-s <name>`, agent-browser and",
        "playwright-cli sessions, one MCP server each), all open at once on the listing",
        "after a snapshot. Memory is every process the tool started (browsers, daemons,",
        "servers), summed as PSS.",
        "",
        "| tool | " + " | ".join(f"{n} agent{'s' if n > 1 else ''}" for n in ns) + " | per extra agent |",
        "|---|" + "---|" * (len(ns) + 1),
    ]
    for p_ in par:
        ok = [r_ for r_ in p_["runs"] if "pss_mb" in r_]
        cells = [f"{r_['pss_mb']:.0f} MB" if "pss_mb" in r_ else "failed" for r_ in p_["runs"]]
        per = (f"{(ok[-1]['pss_mb'] - ok[0]['pss_mb']) / (ok[-1]['n'] - ok[0]['n']):.0f} MB"
               if len(ok) > 1 else "—")
        lines.append(f"| {plabel.get(p_['name'], p_['name'])} | " + " | ".join(cells) + f" | {per} |")
    lines.append("")
for r in exp:
    s = r["session"]
    lines += [
        f"**Lightpanda** (experimental) passes the same correctness gate in",
        f"{s['wall_s']:.2f}s" + (f" with ~{s['browser_pss_mb']:.0f} MB of browser memory" if s['browser_pss_mb'] else "") + ". It has no",
        "rendering engine and one tab (the session opens the second page in the",
        "same tab), so it is listed apart from the Chrome-family ranking.",
        "",
    ]



def short(r):
    """A driver's name for the headline: the tool, its version, and what runs it."""
    fixed = {"nv-serve": "navigera (Chrome)", "nv-shell": "navigera (chrome-headless-shell)",
             "gorod": f"go-rod {ver('go-rod', '')} (Go)", "chromey": f"chromey {ver('chromey', '')} (Rust)"}
    return fixed.get(r["name"]) or NAMES.get(r["name"], r["name"]).replace("`", "")


def tool(a):
    """An agent tool's name and version, without who makes it."""
    return re.sub(r" \(.*\)$", "", label.get(a["name"], a["name"])).replace("`", "")


# A difference within 2% is a tie, not a loss: about what one run varies by.
TIE = 0.02
# What another tool did better than navigera in this run, measure by measure:
# navigera on Chrome against every other driver on the same Chrome, and
# navigera against every other agent tool. Lower is better for each.
losses = []
others = [r for r in chrome if not r["name"].startswith("nv-")]
if serve:
    measures = [
        ("session wall", lambda r: r["session"]["wall_s"], lambda v: f"{v:.2f} s"),
        ("cold start (median)", lambda r: mid(r.get("cold")), lambda v: f"{v:.2f} s"),
        ("warm eval", lambda r: (r.get("eval") or {}).get("mean_ms"), lambda v: f"{v:.2f} ms"),
        ("the driver's own memory", lambda r: r["session"]["driver_rss_mb"], lambda v: f"{v:.0f} MB"),
        ("browser memory", lambda r: r["session"]["browser_pss_mb"] or None, lambda v: f"{v:.0f} MB"),
    ]
    # Driver CPU is left out: the kernel counts it in 10 ms ticks, so every
    # native driver's tens of milliseconds are a tie.
    for what, get, fmt in measures:
        mine = get(serve)
        rivals = [(get(r), r) for r in others if get(r) is not None]
        if mine is None or not rivals:
            continue
        v, r = min(rivals, key=lambda t: t[0])
        if v < mine * (1 - TIE):
            losses.append(f"{what}: {short(r)} {fmt(v)}, navigera {fmt(mine)}")
nav_agent = next((a for a in agent if a["name"] == "navigera" and a.get("wall_s")), None)
if nav_agent:
    def kb(key):
        return lambda a: (a.get(key) or 0) / 1024 or None
    for what, get, fmt in [
        ("time per agent tool call", lambda a: a.get("op_mean_ms"), lambda v: f"{v:.0f} ms"),
        ("snapshot size", kb("snapshot_bytes"), lambda v: f"{v:.0f} KB"),
        ("1000-row snapshot size", kb("snapshot_b_bytes"), lambda v: f"{v:.0f} KB"),
    ]:
        mine = get(nav_agent)
        rivals = [(get(a), a) for a in agent if not a["name"].startswith("navigera") and a.get("wall_s") and get(a)]
        if mine is None or not rivals:
            continue
        v, a = min(rivals, key=lambda t: t[0])
        if v < mine * (1 - TIE):
            losses.append(f"{what}: {tool(a)} {fmt(v)}, navigera {fmt(mine)}")
loss_text = ("Where another tool did better in this run (by more than 2%): " + "; ".join(losses) + "."
             if losses else "No other tool did better than navigera by more than 2% on any measure in this run.")

if page_path:
    page = ["# Benchmarks", ""] + [("## " + x[4:]) if x.startswith("### ") else x for x in lines[2:]]
    lose = [("# " if losses_path else "## ") + "Where navigera is not ahead", "", numbers + ".", "",
            "Each measure of the benchmarks on which another tool beat navigera in this",
            "run by more than 2%, from its publish.json: navigera on Chrome against every",
            "other driver on the same Chrome, and against every other agent tool. Closer",
            "than 2% is a tie. Driver CPU is left out: the kernel counts it in 10 ms",
            "ticks, so every native driver ties.", ""]
    lose += [f"- {x[0].upper() + x[1:]}." for x in losses] or ["- None in this run."]
    lose.append("")
    if losses_path:
        open(losses_path, "w").write("\n".join(lose))
    else:
        page += lose
    open(page_path, "w").write("\n".join(page))
    site = "https://andrey-usa.github.io/navigera/benchmarks/results.html"
    head = ["## Benchmarks", "",
            f"One scripted session (goto, evals, fill and click, a screenshot, a second",
            f"tab) driven by each tool against the same headless Chrome, in one CI run",
            f"({numbers[len('Numbers: '):]}, {date}, GitHub's `ubuntu-latest`); each",
            "driver's extracted data is checked identical:", "",
            "| driver | session | cold start | warm eval | its own memory |",
            "|---|---|---|---|---|"]
    picks = [r for r in chrome if r["name"] in ("nv-serve", "nv-shell")]
    picks += [r for r in others if r["name"] in ("gorod", "puppeteer", "playwright", "playwright-bun")]
    for r in sorted(picks, key=lambda r: r["session"]["wall_s"]):
        ev = (r.get("eval") or {}).get("mean_ms")
        head.append(f"| {short(r)} | {r['session']['wall_s']:.2f} s | {mid(r.get('cold')):.2f} s | "
                    + (f"{ev:.2f} ms" if ev is not None else "—") + f" | {r['session']['driver_rss_mb']:.0f} MB |")
    head.append("")
    calls = [a for a in agent if a.get("wall_s") and a["name"] in
             ("navigera", "agent-browser", "Playwright MCP", "Chrome DevTools MCP", "playwright-cli")]
    if calls:
        calls.sort(key=lambda a: a["op_mean_ms"])
        head += ["Through an agent's tool calls, one step takes "
                 + ", ".join(f"{a['op_mean_ms']:.0f} ms with {tool(a)}" for a in calls) + " (same run).", ""]
    head += [loss_text, "",
             f"Every table, the method and how to rerun it: [Benchmarks]({site}).", ""]
    lines = head

text = open(readme_path).read()
start = text.index("## Benchmarks")
rest = text.find("\n## ", start + 1)
if not page_path:
    rest = text.index("## Agent eval") - 1 if "## Agent eval" in text else text.index("## Working on this repo") - 1
end = rest + 1 if rest >= 0 else len(text)
text = text[:start] + "\n".join(lines) + "\n" + text[end:]
open(readme_path, "w").write(text)
print("README" + "".join(f", {p}" for p in (page_path, losses_path) if p) + " updated from run", run_id)
