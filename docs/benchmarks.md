# Benchmarks

Numbers: [bench run 45950542045](https://github.com/andrey-usa/navigera/actions/runs/45950542045).

Same scripted session (launch → goto listing → title/count/extract evals →
fill+click filter → visible-count eval → screenshot → new tab → goto detail →
title/row-count evals → close) driven against the same headless browser by
each driver. Fixtures are deterministic and local (500-card listing,
1000-row detail). navigera is the static Linux binary the releases ship.
Reps are interleaved across contenders.

| contender | session wall, best (median) of 3 | warm eval mean (200×) | cold start, median (best) of 15 | driver CPU (own) | driver peak RSS (own) | browser memory (PSS) |
|---|---|---|---|---|---|---|
| `nv-shell` (this repo on chrome-headless-shell) | **0.30s** (0.32) | 0.43 ms | **0.09s** (0.09) | **<10 ms** | **8 MB** | 239 MB |
| `gorod-shell` (go-rod 0.116.2, Go, on chrome-headless-shell) | 0.42s (0.42) | **0.39 ms** | 0.11s (0.10) | **20 ms** | 12 MB | 276 MB |
| `nv-serve` (this repo on Chrome) | 0.54s (0.56) | 0.50 ms | 0.25s (0.23) | **<10 ms** | **8 MB** | 338 MB |
| `nv-brave` (this repo on Brave) | 0.57s (0.58) | 0.42 ms | 0.30s (0.29) | **10 ms** | **8 MB** | 551 MB |
| `nv-edge` (this repo on Edge) | 0.63s (0.63) | 0.46 ms | 0.35s (0.34) | **<10 ms** | **8 MB** | 640 MB |
| `gorod` (go-rod 0.116.2, Go) | 0.68s (0.68) | 0.43 ms | 0.29s (0.28) | **10 ms** | 12 MB | 394 MB |
| `chromiumoxide` 0.7.0 (Rust) | 0.75s (0.76) | 0.75 ms | 0.38s (0.32) | **20 ms** | 9 MB | 401 MB |
| `puppeteer-core` 25.19.0 (Node) | 0.89s (0.89) | 0.66 ms | 0.54s (0.52) | 0.35 s | 81 MB | 366 MB |
| `chromedp` 0.19.1 (Go) | 0.92s (0.94) | 0.52 ms | 0.49s (0.41) | **30 ms** | **8 MB** | 418 MB |
| `chromey` 2.58.2 (Rust, maintained chromiumoxide fork) | 0.96s (0.97) | 0.87 ms | 0.36s (0.33) | 50 ms | 22 MB | 446 MB |
| `playwright-core` 1.65.1 (Bun 1.4.5) | 1.09s (1.10) | 0.94 ms | 0.64s (0.63) | 0.52 s | 112 MB | 381 MB |
| `playwright-core` 1.65.1 (Node) | 1.21s (1.49) | 1.09 ms | 0.74s (0.72) | 0.60 s | 142 MB | 394 MB |
| `nv-lightpanda` (this repo on Lightpanda, 1 tab) | 0.19s (0.19) | 0.48 ms | 0.11s (0.11) | **<10 ms** | 9 MB | 15 MB |

Measured 2026-11-27 on GitHub-hosted `ubuntu-24.04, image 20261122.23.1` (AMD EPYC 7763 64-Core Processor — 4 vCPU · Chrome 158.0.8127.36).
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

**Reading the table**: every driver but `nv-shell`, `gorod-shell`, `nv-edge` and `nv-brave` runs the same Google Chrome; on it, nv-serve (0.54s) is ahead of the fastest other driver, gorod (0.68s). `nv-shell`'s lead is partly chrome-headless-shell's, a lighter build; its pair is `gorod-shell` (0.42s), go-rod on the same shell.

## Real-world (public internet, median of 9, same run)

example.com goto → title check: 12 of 12 pass; nv-shell 0.16s, gorod-shell 0.18s, nv-serve 0.31s, gorod 0.38s, nv-brave 0.38s, nv-edge 0.45s, chromey 0.45s, chromiumoxide 0.47s, chromedp 0.60s, puppeteer 0.62s, playwright-bun 0.75s, playwright 0.83s.

GitHub browse (awesome-list scroll + trending click-through): 12 of 12 pass; gorod-shell 2.85s, nv-shell 2.90s, gorod 3.21s, nv-serve 3.24s, nv-brave 3.38s, puppeteer 3.38s, chromiumoxide 3.50s, chromey 3.53s, playwright-bun 3.65s, nv-edge 3.65s, chromedp 3.76s, playwright 3.78s.
navigera's click returns once the new page has settled; the other drivers'
clicks return at once and they then poll the URL, so the browse rows do not
time the same readiness. The example.com check is the title; h1 is read, not checked.
Live pages change between runs, so these are informational, not part of the
correctness gate.

**CDP transport A/B** (same build and run): `--remote-debugging-pipe` (default on Linux/macOS)
vs a DevTools WebSocket port: cold start 0.25s vs 0.25s (medians),
session 0.54s vs 0.55s. No speed difference: the WebSocket handshake itself is ~10 ms. The pipe is the default for two other reasons:
it opens no TCP port that another local process could attach to, and Chrome exits when
navigera dies (EOF on its command pipe), so a killed agent leaks no browser
(`tests/edge_cases.rs`; on Windows, which has no pipe transport here, a
kill-on-close job object gives the same guarantee).

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
| `navigera` (this repo) | CLI | 14 | 0.56s | 27 ms | 37 ms | 34 KB | 55 ms | 41 KB | 73 ms | ✓ |
| `navigera` on chrome-headless-shell | CLI | 14 | 0.38s | 22 ms | 31 ms | 34 KB | 56 ms | 41 KB | 35 ms | ✓ |
| `navigera`, known steps in `batch` | CLI | 6 | 0.59s | 79 ms | 32 ms | 34 KB | 57 ms | 41 KB | — | ✓ |
| `agent-browser` 0.45.0 (Vercel Labs, Rust) | CLI | 13 | 1.26s | 95 ms | 117 ms | 44 KB | 338 ms | 116 KB | — | ✓ |
| `playwright-cli` 0.1.25 (Microsoft) | CLI | 13 | 9.67s | 770 ms | 421 ms | 51 KB | 538 ms | 144 KB | — | ✓ |
| Playwright MCP 0.0.86 (Microsoft) | MCP | 13 | 5.21s | 397 ms | 69 ms | 51 KB | 172 ms | 144 KB | — | ✓ |
| Chrome DevTools MCP 1.14.0 (Google) | MCP | 13 | 3.25s | 202 ms | 96 ms | 34 KB | 295 ms | 78 KB | — | ✓ |

Per-call time includes process start (CLI) or JSON-RPC (MCP), the hop to
the daemon, and the CDP work. Playwright's tool backend, shared by
playwright-cli and Playwright MCP, waits a fixed 500 ms after every action;
playwright-cli also starts a Node process per step. `batch` saves calls, an
agent's turns, not time.

Per op: [Matched operations](https://andrey-usa.github.io/navigera/benchmarks/operations.html).

## Parallel agents (memory, same run)

N agents, each with a session of its own, all open at once on the listing
after a snapshot. Memory is every process the tool started (browsers,
daemons, servers), summed as PSS.
| tool | 1 agent | 4 agents | 8 agents | per extra agent |
|---|---|---|---|---|
| `navigera` (sessions share one browser) | 332 MB | 466 MB | 640 MB | 44 MB |
| `navigera --own-browser` (a browser each) | 332 MB | 694 MB | 1150 MB | 117 MB |
| `agent-browser` 0.45.0 (Vercel Labs, Rust) | 496 MB | 1254 MB | 2237 MB | 249 MB |
| `playwright-cli` 0.1.25 (Microsoft) | 520 MB | 1298 MB | 2290 MB | 253 MB |
| Playwright MCP 0.0.86 (Microsoft) | 524 MB | 1298 MB | 2304 MB | 254 MB |
| Chrome DevTools MCP 1.14.0 (Google) | 548 MB | 1390 MB | 2484 MB | 276 MB |

**Lightpanda** (experimental) passes the same correctness gate in
0.19s with ~15 MB of browser memory. It has no
rendering engine and one tab, so it is listed apart from the Chrome ranking.
