# Benchmarks

Numbers: [bench run 42952776646](https://github.com/andrey-usa/navigera/actions/runs/42952776646).

Same scripted session (launch → goto listing → title/count/extract evals →
fill+click filter → visible-count eval → screenshot → new tab → goto detail →
title/row-count evals → close) driven against the same headless browser by
each driver. Fixtures are deterministic and local (500-card listing,
1000-row detail). navigera is the static Linux binary the releases ship.
Reps are interleaved across contenders.

| contender | session wall, best (median) of 5 | warm eval mean (200×) | cold start, median (best) of 15 | driver CPU (own) | driver peak RSS (own) | browser memory (PSS) |
|---|---|---|---|---|---|---|
| `nv-shell` (this repo on chrome-headless-shell) | **0.29s** (0.31) | 0.31 ms | **0.09s** (0.08) | **<10 ms** | **7 MB** | 224 MB |
| `gorod-shell` (go-rod 0.116.2, Go, on chrome-headless-shell) | 0.36s (0.37) | **0.26 ms** | 0.10s (0.09) | **10 ms** | 12 MB | 248 MB |
| `nv-serve` (this repo on Chrome) | 0.48s (0.49) | 0.35 ms | 0.23s (0.22) | **<10 ms** | **7 MB** | 362 MB |
| `nv-brave` (this repo on Brave) | 0.53s (0.53) | 0.35 ms | 0.28s (0.26) | **<10 ms** | **7 MB** | 610 MB |
| `nv-edge` (this repo on Edge) | 0.57s (0.58) | 0.32 ms | 0.33s (0.29) | **<10 ms** | **7 MB** | 709 MB |
| `gorod` (go-rod 0.116.2, Go) | 0.63s (0.65) | 0.46 ms | 0.27s (0.24) | **10 ms** | 12 MB | 383 MB |
| `chromiumoxide` 0.7.0 (Rust) | 0.72s (0.75) | 0.88 ms | 0.34s (0.32) | **20 ms** | 9 MB | 418 MB |
| `puppeteer-core` 25.16.0 (Node) | 0.79s (0.85) | 0.45 ms | 0.53s (0.44) | 0.29 s | 79 MB | 376 MB |
| `chromey` 2.58.2 (Rust, maintained chromiumoxide fork) | 0.88s (0.89) | 0.81 ms | 0.32s (0.30) | **40 ms** | 16 MB | 454 MB |
| `chromedp` 0.19.1 (Go) | 0.88s (0.90) | 0.52 ms | 0.48s (0.40) | **30 ms** | 8 MB | 427 MB |
| `playwright-core` 1.65.0 (Bun 1.4.4) | 0.95s (1.00) | 0.89 ms | 0.60s (0.55) | 0.42 s | 102 MB | 398 MB |
| `playwright-core` 1.65.0 (Node) | 1.02s (1.06) | 1.19 ms | 0.68s (0.62) | 0.51 s | 139 MB | 383 MB |
| `nv-lightpanda` (this repo on Lightpanda, 1 tab) | 0.17s (0.17) | 0.32 ms | 0.11s (0.11) | **<10 ms** | 8 MB | 15 MB |

Measured 2026-11-11 on GitHub-hosted `ubuntu-24.04, image 20261101.360.1` (AMD EPYC 9V74 80-Core Processor — 4 vCPU · Chrome 156.0.8078.79).
Every driver produced byte-identical extracted data and counts (the
correctness gate). Session wall is the best of N, the median in brackets;
cold start and the live-site rows rank by the median, the best run in brackets.
The ladder reruns weekly ([`bench.yml`](https://github.com/andrey-usa/navigera/blob/master/.github/workflows/bench.yml)) against
the latest contender releases; runners vary, so compare rows within one run.

**How driver CPU/RSS are measured.** Each driver's *own* `utime+stime`,
read from its zombie's `/proc/<pid>/stat` before reaping (`waitid` with
`WNOWAIT`), and its own VmHWM; the kernel's 10 ms ticks make anything under
~50 ms a tie (`wait4` would add in every reaped child, the artifact earlier
versions of this table reported). Browser memory is the whole process tree,
sampled as summed PSS every 250 ms (shared pages counted once); the median
of the reps' peaks, since one short session's peak is caught or missed by phase.

**Reading the table**: every driver but `nv-shell`, `gorod-shell`, `nv-edge` and `nv-brave` runs the same Google Chrome; on it, nv-serve (0.48s) is ahead of the fastest other driver, gorod (0.63s). `nv-shell`'s lead is partly chrome-headless-shell's, a lighter build; its pair is `gorod-shell` (0.36s), go-rod on the same shell.

## Real-world (public internet, median of 9, same run)

example.com goto → title check: 12 of 12 pass; nv-shell 0.17s, gorod-shell 0.18s, nv-serve 0.31s, gorod 0.36s, nv-brave 0.37s, nv-edge 0.41s, chromiumoxide 0.42s, chromey 0.42s, puppeteer 0.53s, chromedp 0.56s, playwright-bun 0.67s, playwright 0.75s.

GitHub browse (awesome-list scroll + trending click-through): 12 of 12 pass; gorod-shell 2.96s, nv-shell 3.00s, nv-serve 3.23s, gorod 3.26s, puppeteer 3.40s, chromiumoxide 3.40s, nv-brave 3.46s, chromey 3.48s, nv-edge 3.55s, playwright-bun 3.65s, chromedp 3.70s, playwright 3.71s.
navigera's click returns once the new page has settled; the other drivers'
clicks return at once and they then poll the URL, so the browse rows do not
time the same readiness. The example.com check is the title; h1 is read, not checked.
Live pages change between runs, so these are informational, not part of the
correctness gate.

**CDP transport A/B** (same build and run): `--remote-debugging-pipe` (default on Linux/macOS)
vs a DevTools WebSocket port: cold start 0.23s vs 0.24s (medians),
session 0.48s vs 0.49s. No speed difference: the WebSocket handshake itself is ~10 ms. The pipe is the default for two other reasons:
it opens no TCP port that another local process could attach to, and Chrome exits when
navigera dies (EOF on its command pipe), so a killed agent leaks no browser
(`tests/edge_cases.rs`; on Windows, which has no pipe transport here, a
kill-on-close job object gives the same guarantee).

**This build vs navigera 0.11.0, same machine and run** (`nv-baseline`,
built the same way by the ladder's `baseline_ref` A/B):
session 0.46s → 0.48s (+5%), cold start 0.23s → 0.23s median (+3%), warm eval 0.36 ms → 0.35 ms (-1%), browser CPU per session 0.67s → 0.64s.

Where navigera's cold start goes (`NAVIGERA_TIMINGS`, best run): browser up
140 ms, first page 67 ms, close 2 ms.

## Agent tools (scripted, warm session, best of 3)

The same canonical session plus one page snapshot, driven as an agent's
tool calls: CLI tools run one process per step, MCP servers get one
`tools/call` per step over a persistent connection.
Snapshot sizes are each tool's default output, what the agent reads.
Outside the session's total, each tool also snapshots the 1000-row page,
and navigera starts a second agent's session while the first one's browser
is up.

| tool | kind | calls | total wall | per call | snapshot | its size | 1000-row snapshot | its size | 2nd session start | gate |
|---|---|---|---|---|---|---|---|---|---|---|
| `navigera` (this repo) | CLI | 14 | 0.56s | 26 ms | 29 ms | 34 KB | 52 ms | 41 KB | 66 ms | ✓ |
| `navigera` on chrome-headless-shell | CLI | 14 | 0.37s | 22 ms | 26 ms | 34 KB | 60 ms | 41 KB | 34 ms | ✓ |
| `navigera`, known steps in `batch` | CLI | 6 | 0.54s | 74 ms | 28 ms | 34 KB | 58 ms | 41 KB | — | ✓ |
| `agent-browser` 0.42.0 (Vercel Labs, Rust) | CLI | 13 | 1.21s | 92 ms | 123 ms | 44 KB | 308 ms | 116 KB | — | ✓ |
| `playwright-cli` 0.1.24 (Microsoft) | CLI | 13 | 8.61s | 687 ms | 361 ms | 50 KB | 429 ms | 144 KB | — | ✓ |
| Playwright MCP 0.0.85 (Microsoft) | MCP | 13 | 4.97s | 384 ms | 52 ms | 50 KB | 147 ms | 144 KB | — | ✓ |
| Chrome DevTools MCP 1.12.0 (Google) | MCP | 13 | 3.04s | 194 ms | 85 ms | 34 KB | 262 ms | 78 KB | — | ✓ |

Per-call time includes process start (CLI) or JSON-RPC (MCP), the hop to
the daemon, and the CDP work. Playwright's tool backend, shared by
playwright-cli and Playwright MCP, waits a fixed 500 ms after every action;
playwright-cli also starts a Node process per step. `batch` saves calls, an
agent's turns, not time.

## Parallel agents (memory, same run)

N agents, each with a session of its own, all open at once on the listing
after a snapshot. Memory is every process the tool started (browsers,
daemons, servers), summed as PSS.
| tool | 1 agent | 4 agents | 8 agents | per extra agent |
|---|---|---|---|---|
| `navigera` (sessions share one browser) | 355 MB | 495 MB | 681 MB | 47 MB |
| `navigera --own-browser` (a browser each) | 352 MB | 778 MB | 1316 MB | 138 MB |
| `agent-browser` 0.42.0 (Vercel Labs, Rust) | 508 MB | 1313 MB | 2343 MB | 262 MB |
| `playwright-cli` 0.1.24 (Microsoft) | 536 MB | 1351 MB | 2396 MB | 266 MB |
| Playwright MCP 0.0.85 (Microsoft) | 542 MB | 1353 MB | 2414 MB | 267 MB |
| Chrome DevTools MCP 1.12.0 (Google) | 554 MB | 1412 MB | 2518 MB | 281 MB |

**Lightpanda** (experimental) passes the same correctness gate in
0.17s with ~15 MB of browser memory. It has no
rendering engine and one tab, so it is listed apart from the Chrome ranking.
