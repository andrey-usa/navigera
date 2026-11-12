"""Write the benchmark docs from a ladder publish.json.

Usage: gen_readme.py README.md publish.json ["what the nv-baseline A/B changed"]
                     [--run ID] [--page PATH [--losses PATH] [--ops PATH]]

Numbers in the docs come from a run's publish.json, never retyped. Every
generated page opens with a link to the run (all its tables are that run's):
ID from --run, else the run_id the ladder recorded in publish.json.

With --page (docs/benchmarks.md), every table goes to that page, and the
measures where another tool beat navigera in the run to a section closing
it, or to the page --losses names (docs/losses.md), and the matched-operations
table to the page --ops names (docs/operations.md: the results page stays
under the site's 1,500 words); README.md's `## Benchmarks` gets the headline: one table of
the main drivers, the per-call time of the agent tools, the same list of
losses, and a link to the page. Without it, every table goes to README.md
(how runs before the docs site were written)."""
import json
import re
import sys

args = sys.argv[1:]
opts = {}
for flag in ("--run", "--page", "--losses", "--ops"):
    if flag in args:
        i = args.index(flag)
        opts[flag] = args[i + 1]
        del args[i:i + 2]
run_arg, page_path, losses_path, ops_path = opts.get("--run"), opts.get("--page"), opts.get("--losses"), opts.get("--ops")
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
    "gorod-shell": f"`gorod-shell` (go-rod {ver('go-rod', '0.116.2')}, Go, on chrome-headless-shell)",
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
    "Reps are interleaved across contenders.",
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
# Runs from before the image was recorded ran on the moving label.
runner = doc.get("runner") or "ubuntu-latest"
env = doc["env"].replace("**", "").replace("Env: ", "").replace("Chrome: Google Chrome ", "Chrome ")
lines += [
    "",
    # No link to the run: the ladder may run in a fork or a private mirror,
    # where the link would not resolve for readers.
    f"Measured {date} on GitHub-hosted `{runner}` ({env}).",
    "Every driver produced byte-identical extracted data and counts (the",
    "correctness gate). Session wall is the best of N, the median in brackets;",
    "cold start and the live-site rows rank by the median, the best run in brackets.",
    "The ladder reruns weekly ([`bench.yml`](https://github.com/andrey-usa/navigera/blob/master/.github/workflows/bench.yml)) against",
    "the latest contender releases; runners vary, so compare rows within one run.",
    "",
    "**How driver CPU/RSS are measured.** Each driver's *own* `utime+stime`,",
    "read from its zombie's `/proc/<pid>/stat` before reaping (`waitid` with",
    "`WNOWAIT`), and its own VmHWM; the kernel's 10 ms ticks make anything under",
    "~50 ms a tie (`wait4` would add in every reaped child, the artifact earlier",
    "versions of this table reported). Browser memory is the whole process tree,",
    "sampled as summed PSS every 250 ms (shared pages counted once); the median",
    "of the reps' peaks, since one short session's peak is caught or missed by phase.",
    "",
]
nv_best = next(r for r in chrome if r["name"].startswith("nv-"))
serve_row = next((r for r in chrome if r["name"] == "nv-serve"), None)
# Rows on chrome-headless-shell: compared with each other, not with Chrome.
SHELL_ROWS = ("nv-shell", "gorod-shell")
other = next((r for r in chrome if not r["name"].startswith("nv-") and r["name"] not in SHELL_ROWS), None)
gorod_shell = next((r for r in chrome if r["name"] == "gorod-shell"), None)
rank = chrome.index(nv_best) + 1
shell_row = next((r for r in chrome if r["name"] == "nv-shell"), None)
# Rows that run a browser other than the regular Google Chrome.
off = [f"`{n}`" for n in ("nv-shell", "gorod-shell", "nv-edge", "nv-brave") if any(r["name"] == n for r in chrome)]
off_text = off[0] if len(off) == 1 else ", ".join(off[:-1]) + " and " + off[-1] if off else ""
lines += [
    "**Reading the table**:"
    + (f" every driver{' but ' + off_text if off else ''} runs the same Google Chrome; on it, nv-serve ({serve_row['session']['wall_s']:.2f}s) is "
       + ("ahead of" if serve_row["session"]["wall_s"] < other["session"]["wall_s"] else "behind")
       + f" the fastest other driver, {other['name']} ({other['session']['wall_s']:.2f}s)"
       if serve_row and other else "")
    + (f". `nv-shell`'s lead is partly chrome-headless-shell's, a lighter build"
       + (f"; its pair is `gorod-shell` ({gorod_shell['session']['wall_s']:.2f}s), go-rod on the same"
          " shell" if gorod_shell else " no other row runs")
       if shell_row else "")
    + ".",
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
        f"example.com goto → title check: {rw_ok} of {len(rw)} pass; " + live(rw, "realworld") + ".",
        "",
        f"GitHub browse (awesome-list scroll + trending click-through): {br_ok} of {len(br)} pass; "
        + live(br, "browse") + ".",
        "navigera's click returns once the new page has settled; the other drivers'",
        "clicks return at once and they then poll the URL, so the browse rows do not",
        "time the same readiness. The example.com check is the title; h1 is read, not checked.",
        "Live pages change between runs, so these are informational, not part of the",
        "correctness gate.",
        "",
    ]
serve = next((r for r in rows if r["name"] == "nv-serve"), None)
shell = next((r for r in rows if r["name"] == "nv-shell"), None)
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
        "(`tests/edge_cases.rs`; on Windows, which has no pipe transport here, a",
        "kill-on-close job object gives the same guarantee).",
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
        "The same canonical session plus one page snapshot, driven as an agent's",
        "tool calls: CLI tools run one process per step, MCP servers get one",
        "`tools/call` per step over a persistent connection.",
        "Snapshot sizes are each tool's default output, what the agent reads.",
        "Outside the session's total, each tool also snapshots the 1000-row page,",
        "and navigera starts a second agent's session while the first one's browser",
        "is up.",
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
        "the daemon, and the CDP work. Playwright's tool backend, shared by",
        "playwright-cli and Playwright MCP, waits a fixed 500 ms after every action;",
        "playwright-cli also starts a Node process per step. `batch` saves calls, an",
        "agent's turns, not time.",
        "",
    ]
ops = doc.get("ops") or []
if ops:
    ops_order = ["navigera", "navigera (baseline)", "agent-browser",
                 "playwright-cli", "Playwright MCP", "Chrome DevTools MCP",
                 "browser-use"]
    ops_cols = [o for o in ops if o["name"] in ops_order]
    ops_cols.sort(key=lambda o: ops_order.index(o["name"]))
    ops_cols += [o for o in ops if o["name"] not in ops_order]
    op_names = ("goto", "fill", "click", "click-submit", "read-text", "snapshot",
                "screenshot", "form-batch")
    ops_label = {**label,
                 "navigera (baseline)": "`navigera` (baseline build)",
                 "browser-use": f"`browser-use` {ver('browser-use', '')} (exec model)".replace("  ", " ")}
    ops_lines = [
        "### Matched operations (one call per op, warm session, median of 10)",
        "",
        numbers + ".",
        "",
        "One CLI process (or one MCP `tools/call`) per op, against a warm session,",
        "on the same fixture page. `goto` re-navigates to the listing; `fill` fills",
        "the search box; `click` clicks Search (client-side filter, no navigation);",
        "`click-submit` clicks the order form's submit, which a script handles (no",
        "navigation); `read-text` reads the heading; `snapshot` is each tool's default",
        "accessibility output; `screenshot` writes a PNG; `form-batch` fills two fields,",
        "clicks submit and reads the status — one call for tools with a batch command,",
        "four calls otherwise. Two rounds of 5 reps: rounds are the outer loop and tools",
        "the inner loop, with the second round in reversed tool order. Each tool runs in",
        "its default output mode (MCP tools over JSON-RPC, their only interface); no tool",
        "is switched into a machine mode. A cell is complete only with all 10 reps and no",
        "round error; a partial cell is shown as `incomplete (n/N)` and left out of the",
        "per-op losses below.",
        "",
        "| op | " + " | ".join(ops_label.get(o["name"], o["name"]) for o in ops_cols) + " |",
        "|---|" + "---|" * len(ops_cols),
    ]
    for op in op_names:
        row = [f"`{op}`"]
        for o in ops_cols:
            cell = (o.get("cells") or {}).get(op) or {}
            if cell.get("median_ms") is None:
                row.append("failed")
            elif cell.get("incomplete"):
                n = cell.get("reps", 0)
                n_expected = cell.get("expected_reps", 10)
                row.append(f"incomplete ({n}/{n_expected}) {cell['median_ms']:.1f} ms "
                           f"({cell['calls']} call{'s' if cell['calls'] != 1 else ''})")
            else:
                row.append(f"{cell['median_ms']:.1f} ms ({cell['calls']} call"
                           f"{'s' if cell['calls'] != 1 else ''})")
        ops_lines.append("| " + " | ".join(row) + " |")
    ops_lines += [
        "",
        "Snapshot outputs differ in form (each tool's default tree), so that row compares",
        "call latency, not equivalent work. The browser-use snapshot comes from its",
        "documented raw `cdp()` route; it has no built-in snapshot command. Per-call time",
        "includes process start (CLI) or JSON-RPC (MCP) and the daemon hop. An `incomplete",
        "(n/N)` cell has only n of the 10 expected reps; its median is partial evidence.",
        "",
    ]
    if ops_path:
        # Its own page: the heading becomes the title, and the results page
        # points at it (the site holds every page under 1,500 words).
        ops_page = ["# " + ops_lines[0][4:]] + ops_lines[1:]
        open(ops_path, "w").write("\n".join(ops_page))
        lines += [
            "Per op: [Matched operations](https://andrey-usa.github.io/navigera/benchmarks/operations.html).",
            "",
        ]
    else:
        lines += ops_lines
par = doc.get("parallel") or []
if par:
    ns = [r_["n"] for r_ in par[0]["runs"]]
    plabel = {**label, "navigera": "`navigera` (sessions share one browser)",
              "navigera (own browser)": "`navigera --own-browser` (a browser each)"}
    lines += [
        "### Parallel agents (memory, same run)",
        "",
        "N agents, each with a session of its own, all open at once on the listing",
        "after a snapshot. Memory is every process the tool started (browsers,",
        "daemons, servers), summed as PSS."
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
        "rendering engine and one tab, so it is listed apart from the Chrome ranking.",
        "",
    ]



def short(r):
    """A driver's name for the headline: the tool, its version, and what runs it."""
    fixed = {"nv-serve": "navigera (Chrome)", "nv-shell": "navigera (chrome-headless-shell)",
             "gorod": f"go-rod {ver('go-rod', '')} (Go)",
             "gorod-shell": f"go-rod {ver('go-rod', '')} (Go, chrome-headless-shell)", "chromey": f"chromey {ver('chromey', '')} (Rust)"}
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
others = [r for r in chrome if not r["name"].startswith("nv-") and r["name"] not in SHELL_ROWS]
measures = []
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
# The same measures on chrome-headless-shell: navigera against go-rod there.
if shell and gorod_shell:
    for what, get, fmt in measures:
        mine, v = get(shell), get(gorod_shell)
        if mine is not None and v is not None and v < mine * (1 - TIE):
            losses.append(f"{what} on chrome-headless-shell: {short(gorod_shell)} {fmt(v)}, navigera {fmt(mine)}")
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
nav_ops = next((o for o in ops if o["name"] == "navigera"), None)
if nav_ops:
    # Per-op losses: only against tools that took the same number of calls
    # (the form-batch cell is 1 call for batch tools, 4 for the rest).
    # Incomplete cells are left out entirely: a partial median
    # is not a ranking, for navigera or for a rival.
    for op in ("goto", "fill", "click", "click-submit", "read-text", "snapshot",
               "screenshot", "form-batch"):
        mine = (nav_ops.get("cells") or {}).get(op) or {}
        mine_ms, mine_calls = mine.get("median_ms"), mine.get("calls")
        if mine_ms is None or mine.get("incomplete"):
            continue
        rivals = []
        for o in ops:
            if o["name"].startswith("navigera") or not o.get("cells"):
                continue
            cell = o["cells"].get(op) or {}
            if (cell.get("median_ms") is not None and not cell.get("incomplete")
                    and cell.get("calls") == mine_calls):
                rivals.append((cell["median_ms"], o))
        if not rivals:
            continue
        v, o = min(rivals, key=lambda t: t[0])
        if v < mine_ms * (1 - TIE):
            losses.append(f"{op} per call: {tool(o)} {v:.1f} ms, navigera {mine_ms:.1f} ms")
loss_text = ("Where another tool did better in this run (by more than 2%): " + "; ".join(losses) + "."
             if losses else "No other tool did better than navigera by more than 2% on any measure in this run.")

if page_path:
    page = ["# Benchmarks", ""] + [("## " + x[4:]) if x.startswith("### ") else x for x in lines[2:]]
    lose = [("# " if losses_path else "## ") + "Where navigera is not ahead", "", numbers + ".", "",
            "Each measure of the benchmarks on which another tool beat navigera in this",
            "run by more than 2%, from its publish.json: navigera on Chrome against every",
            "other driver on the same Chrome, navigera on chrome-headless-shell against",
            "go-rod on it, and navigera against every other agent tool. Closer",
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
            f"({numbers[len('Numbers: '):]}, {date}, GitHub's `{runner}`); each",
            "driver's extracted data is checked identical:", "",
            "| driver | session | cold start | warm eval | its own memory |",
            "|---|---|---|---|---|"]
    picks = [r for r in chrome if r["name"] in ("nv-serve", "nv-shell")]
    picks += [r for r in chrome if r["name"] in ("gorod", "gorod-shell", "puppeteer", "playwright", "playwright-bun")]
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
print("README" + "".join(f", {p}" for p in (page_path, losses_path, ops_path) if p) + " updated from run", run_id)
