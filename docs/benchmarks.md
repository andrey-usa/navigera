# Benchmarks

Numbers: [bench run 42759659475](https://github.com/andrey-usa/navigera/actions/runs/42759659475).

Same scripted session (launch → goto listing → title/count/extract evals →
fill+click filter → visible-count eval → screenshot → new tab → goto detail →
title/row-count evals → close) driven against the same headless browser by
each driver. Fixtures are deterministic and local (500-card listing,
1000-row detail). navigera is the static Linux binary the releases ship.
Reps are interleaved across contenders. Run in CI via `.github/workflows/bench.yml`.

| contender | session wall, best (median) of 3 | warm eval mean (200×) | cold start, median (best) of 15 | driver CPU (own) | driver peak RSS (own) | browser memory (PSS) |
|---|---|---|---|---|---|---|
| `nv-shell` (this repo on chrome-headless-shell) | **0.25s** (0.26) | 0.32 ms | **0.07s** (0.06) | **<10 ms** | **7 MB** | 247 MB |
| `nv-serve` (this repo on Chrome) | 0.41s (0.43) | 0.35 ms | 0.19s (0.18) | **<10 ms** | **7 MB** | 399 MB |
| `nv-brave` (this repo on Brave) | 0.42s (0.46) | 0.34 ms | 0.22s (0.21) | **<10 ms** | **7 MB** | 599 MB |
| `nv-edge` (this repo on Edge) | 0.50s (0.51) | 0.27 ms | 0.26s (0.25) | **<10 ms** | **7 MB** | 673 MB |
| `gorod` (go-rod 0.116.2, Go) | 0.51s (0.52) | **0.24 ms** | 0.22s (0.21) | **10 ms** | 12 MB | 425 MB |
| `chromiumoxide` 0.7.0 (Rust) | 0.60s (0.61) | 0.74 ms | 0.27s (0.25) | **10 ms** | 9 MB | 451 MB |
| `puppeteer-core` 25.16.0 (Node) | 0.61s (0.62) | 0.41 ms | 0.37s (0.35) | 0.21 s | 78 MB | 365 MB |
| `chromedp` 0.19.1 (Go) | 0.75s (0.76) | 0.41 ms | 0.37s (0.35) | **20 ms** | 8 MB | 441 MB |
| `chromey` 2.58.2 (Rust, maintained chromiumoxide fork) | 0.76s (0.80) | 0.69 ms | 0.26s (0.24) | **30 ms** | 16 MB | 494 MB |
| `playwright-core` 1.65.0 (Bun 1.4.4) | 0.80s (0.82) | 0.73 ms | 0.47s (0.45) | 0.33 s | 104 MB | 420 MB |
| `playwright-core` 1.65.0 (Node) | 0.89s (0.91) | 0.89 ms | 0.51s (0.49) | 0.39 s | 139 MB | 419 MB |
| `nv-lightpanda` (this repo on Lightpanda, 1 tab) | 0.15s (0.16) | 0.23 ms | 0.11s (0.11) | **<10 ms** | 8 MB | 15 MB |

Measured 2026-11-10 on GitHub-hosted `ubuntu-latest` (AMD EPYC 9V45 96-Core Processor — 4 vCPU · Chrome 156.0.8078.79).
Every driver produced byte-identical extracted data and counts (correctness
gate). Session wall is best-of-N (median in brackets); cold start and the
live-site rows are ranked by their median, with the best run in brackets.
The ladder reruns weekly ([`bench.yml`](https://github.com/andrey-usa/navigera/blob/master/.github/workflows/bench.yml)) against
the latest contender releases. GitHub runners vary between runs, so compare
rows within one run.

**How driver CPU/RSS are measured.** Each driver's *own* `utime+stime`,
read from its zombie's `/proc/<pid>/stat` before reaping (`waitid` with
`WNOWAIT`), and its own VmHWM. The kernel counts CPU in 10 ms ticks, so
values under ~50 ms are a tie. Not `wait4`: its rusage is RUSAGE_BOTH and
adds in every child the driver reaped. navigera, chromedp, puppeteer
and playwright `wait()` on their Chrome, so `wait4` charged them Chrome's
CPU and RSS; go-rod (its leakless helper reaps Chrome) and chromiumoxide
were never charged. Earlier versions of this table reported exactly that
artifact ("0.82s vs 0.03s"). Browser memory is the whole browser process
tree (every renderer/GPU/utility process), sampled as summed PSS every
250 ms (shared pages counted once).

**Reading the table.** Session wall order: nv-shell 0.25s, nv-serve 0.41s, nv-brave 0.42s, nv-edge 0.50s, gorod 0.51s, chromiumoxide 0.60s, puppeteer 0.61s, chromedp 0.75s, chromey 0.76s, playwright-bun 0.80s, playwright 0.89s. Every driver but `nv-shell`, `nv-edge` and `nv-brave` runs the same Google Chrome; on it, nv-serve (0.41s) is ahead of the fastest other driver, gorod (0.51s). `nv-shell` (0.25s) is navigera on chrome-headless-shell, a lighter build of the same browser that no other row runs, so its lead is partly the browser's. All native drivers (navigera,
go-rod, chromiumoxide, chromedp) spend tens of milliseconds of their own CPU
or less; the Node drivers spend hundreds and carry 80–150 MB of their own RSS.

## Real-world (public internet, median of 9, same run)

Numbers: [bench run 42759659475](https://github.com/andrey-usa/navigera/actions/runs/42759659475).

example.com goto → title check: 11 of 11 pass; nv-shell 0.18s, nv-serve 0.28s, gorod 0.33s, nv-brave 0.34s, chromiumoxide 0.34s, chromey 0.35s, nv-edge 0.36s, puppeteer 0.47s, chromedp 0.52s, playwright-bun 0.58s, playwright 0.63s.

GitHub browse (awesome-list scroll + trending click-through): 11 of 11 pass; nv-shell 2.84s, gorod 3.10s, nv-serve 3.12s, puppeteer 3.17s, nv-brave 3.22s, chromiumoxide 3.24s, chromey 3.29s, chromedp 3.29s, nv-edge 3.30s, playwright-bun 3.38s, playwright 3.43s.
navigera's click returns once the new page has settled; the other drivers'
clicks return at once and they then poll the URL, so the browse rows do not
time the same readiness. The example.com check is the title; h1 is read, not checked.
Live pages change between runs, so these are informational, not part of the
correctness gate. (The scroll check used to fail at random for every driver:
GitHub sets CSS `scroll-behavior: smooth`, so `scrollY` was read mid-animation;
contenders now scroll with `behavior: 'instant'`.)

**chrome-headless-shell** (the same Chrome build without its browser UI
layer): cold start 0.19s → 0.07s (medians), session 0.41s → 0.25s, browser memory 399 MB → 247 MB.

**CDP transport A/B** (same build and run): `--remote-debugging-pipe` (default on Linux/macOS)
vs a DevTools WebSocket port: cold start 0.19s vs 0.20s (medians),
session 0.41s vs 0.42s. No speed difference: the WebSocket handshake itself is ~10 ms. The pipe is the default for two other reasons:
it opens no TCP port that another local process could attach to, and Chrome exits when
navigera dies (EOF on its command pipe), so a killed agent leaks no browser
(`tests/edge_cases.rs`: over a WebSocket port the browser outlives its driver).
Windows has no pipe transport here and uses the WebSocket, with a kill-on-close
job object giving the same guarantee (`killed_session_server_takes_its_browser_down`
runs there too).

**This build vs navigera 0.10.1, same machine and run** (`nv-baseline`,
built the same way by the ladder's `baseline_ref` A/B):
session 0.42s → 0.41s (-1%), cold start 0.19s → 0.19s median (-1%), warm eval 0.63 ms → 0.35 ms (-44%), browser CPU per session 0.59s → 0.52s. This build is next since 0.10.1: a11y, vitals, passkey, element screenshots, totp and set cpu, and eval answering in one awaited round trip again.

Where navigera's cold start goes (`NAVIGERA_TIMINGS`, best run): browser up
121 ms, first page 49 ms, close 3 ms.

## Agent tools (scripted, warm session, best of 3)

Numbers: [bench run 42759659475](https://github.com/andrey-usa/navigera/actions/runs/42759659475).

How an agent's tool calls drive a browser: CLI tools run one process per
step (a shell tool), MCP servers get one `tools/call` per step over a
persistent connection. Same canonical session plus one page snapshot.
Snapshot sizes are each tool's default output, what the agent reads.
Outside the session's total, each tool also snapshots the 1000-row page,
and navigera starts a second agent's session while the first one's browser
is up (sessions share one browser, a context each).

| tool | kind | calls | total wall | per call | snapshot | its size | 1000-row snapshot | its size | 2nd session start | gate |
|---|---|---|---|---|---|---|---|---|---|---|
| `navigera` (this repo) | CLI | 14 | 0.45s | 21 ms | 21 ms | 34 KB | 47 ms | 41 KB | 70 ms | ✓ |
| `navigera` on chrome-headless-shell | CLI | 14 | 0.30s | 18 ms | 19 ms | 34 KB | 44 ms | 41 KB | 33 ms | ✓ |
| `navigera`, known steps in `batch` | CLI | 6 | 0.45s | 58 ms | 24 ms | 34 KB | 41 ms | 41 KB | — | ✓ |
| `agent-browser` 0.42.0 (Vercel Labs, Rust) | CLI | 13 | 1.10s | 80 ms | 88 ms | 44 KB | 243 ms | 116 KB | — | ✓ |
| `playwright-cli` 0.1.24 (Microsoft) | CLI | 13 | 7.33s | 588 ms | 261 ms | 50 KB | 311 ms | 144 KB | — | ✓ |
| Playwright MCP 0.0.85 (Microsoft) | MCP | 13 | 4.64s | 363 ms | 40 ms | 50 KB | 101 ms | 144 KB | — | ✓ |
| Chrome DevTools MCP 1.12.0 (Google) | MCP | 13 | 2.77s | 181 ms | 59 ms | 34 KB | 194 ms | 78 KB | — | ✓ |

Per-call time includes process start (CLI) or JSON-RPC (MCP), the hop to
the daemon, and the CDP work. playwright-cli and Playwright MCP share Playwright's tool
backend, which waits a fixed 500 ms after every action (`timeouts.settle`); playwright-cli
also starts a Node process per step. `batch` saves calls, which are an agent's turns,
not time: its row runs the same steps in fewer, longer calls.

## Parallel agents (memory, same run)

Numbers: [bench run 42759659475](https://github.com/andrey-usa/navigera/actions/runs/42759659475).

N agents, each with a session of its own (navigera `-s <name>`, agent-browser and
playwright-cli sessions, one MCP server each), all open at once on the listing
after a snapshot. Memory is every process the tool started (browsers, daemons,
servers), summed as PSS.

| tool | 1 agent | 4 agents | 8 agents | per extra agent |
|---|---|---|---|---|
| `navigera` (sessions share one browser) | 360 MB | 500 MB | 687 MB | 47 MB |
| `navigera --own-browser` (a browser each) | 358 MB | 790 MB | 1340 MB | 140 MB |
| `agent-browser` 0.42.0 (Vercel Labs, Rust) | 514 MB | 1318 MB | 2384 MB | 267 MB |
| `playwright-cli` 0.1.24 (Microsoft) | 542 MB | 1361 MB | 2425 MB | 269 MB |
| Playwright MCP 0.0.85 (Microsoft) | 542 MB | 1367 MB | 2442 MB | 271 MB |
| Chrome DevTools MCP 1.12.0 (Google) | 558 MB | 1425 MB | 2553 MB | 285 MB |

**Lightpanda** (experimental) passes the same correctness gate in
0.15s with ~15 MB of browser memory. It has no
rendering engine and one tab (the session opens the second page in the
same tab), so it is listed apart from the Chrome-family ranking.
