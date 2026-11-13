# Benchmarks

Numbers: [bench run 43491069940](https://github.com/andrey-usa/navigera/actions/runs/43491069940).

Same scripted session (launch → goto listing → title/count/extract evals →
fill+click filter → visible-count eval → screenshot → new tab → goto detail →
title/row-count evals → close) driven against the same headless browser by
each driver. Fixtures are deterministic and local (500-card listing,
1000-row detail). navigera is the static Linux binary the releases ship.
Reps are interleaved across contenders.

| contender | session wall, best (median) of 5 | warm eval mean (200×) | cold start, median (best) of 15 | driver CPU (own) | driver peak RSS (own) | browser memory (PSS) |
|---|---|---|---|---|---|---|
| `nv-shell` (this repo on chrome-headless-shell) | **0.35s** (0.38) | 0.41 ms | **0.10s** (0.10) | **<10 ms** | 9 MB | 237 MB |
| `gorod-shell` (go-rod 0.116.2, Go, on chrome-headless-shell) | 0.40s (0.41) | **0.38 ms** | 0.11s (0.10) | **10 ms** | 12 MB | 246 MB |
| `nv-serve` (this repo on Chrome) | 0.57s (0.58) | 0.46 ms | 0.26s (0.25) | **<10 ms** | 9 MB | 346 MB |
| `nv-brave` (this repo on Brave) | 0.65s (0.66) | 0.48 ms | 0.33s (0.30) | **10 ms** | 9 MB | 668 MB |
| `gorod` (go-rod 0.116.2, Go) | 0.66s (0.72) | 0.46 ms | 0.30s (0.28) | **10 ms** | 12 MB | 380 MB |
| `nv-edge` (this repo on Edge) | 0.71s (0.72) | 0.48 ms | 0.37s (0.35) | **<10 ms** | 9 MB | 774 MB |
| `chromiumoxide` 0.7.0 (Rust) | 0.79s (0.82) | 0.91 ms | 0.37s (0.34) | **30 ms** | 9 MB | 414 MB |
| `puppeteer-core` 25.16.0 (Node) | 0.91s (0.92) | 0.68 ms | 0.54s (0.53) | 0.36 s | 79 MB | 377 MB |
| `chromedp` 0.19.1 (Go) | 0.92s (0.97) | 0.51 ms | 0.48s (0.44) | **40 ms** | **8 MB** | 458 MB |
| `chromey` 2.58.2 (Rust, maintained chromiumoxide fork) | 0.93s (1.01) | 0.84 ms | 0.36s (0.33) | 50 ms | 21 MB | 447 MB |
| `playwright-core` 1.65.0 (Bun 1.4.4) | 1.10s (1.13) | 0.96 ms | 0.65s (0.63) | 0.54 s | 104 MB | 382 MB |
| `playwright-core` 1.65.0 (Node) | 1.18s (1.23) | 1.13 ms | 0.75s (0.74) | 0.61 s | 140 MB | 400 MB |
| `nv-lightpanda` (this repo on Lightpanda, 1 tab) | 0.19s (0.19) | 0.49 ms | 0.12s (0.11) | **<10 ms** | 10 MB | 15 MB |

Measured 2026-11-13 on GitHub-hosted `ubuntu-24.04, image 20261111.370.1` (AMD EPYC 7763 64-Core Processor — 4 vCPU · Chrome 157.0.8103.50).
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

**Reading the table**: every driver but `nv-shell`, `gorod-shell`, `nv-edge` and `nv-brave` runs the same Google Chrome; on it, nv-serve (0.57s) is ahead of the fastest other driver, gorod (0.66s). `nv-shell`'s lead is partly chrome-headless-shell's, a lighter build; its pair is `gorod-shell` (0.40s), go-rod on the same shell.

## Real-world (public internet, median of 9, same run)

example.com goto → title check: 12 of 12 pass; nv-shell 0.23s, gorod-shell 0.25s, nv-serve 0.39s, gorod 0.43s, nv-brave 0.47s, chromey 0.49s, nv-edge 0.50s, chromiumoxide 0.51s, chromedp 0.64s, puppeteer 0.66s, playwright-bun 0.78s, playwright 0.87s.

GitHub browse (awesome-list scroll + trending click-through): 12 of 12 pass; gorod-shell 3.46s, nv-shell 3.54s, gorod 3.74s, nv-serve 3.83s, chromey 3.95s, chromiumoxide 3.97s, nv-brave 4.03s, puppeteer 4.03s, nv-edge 4.33s, playwright-bun 4.34s, playwright 4.43s, chromedp 4.44s.
navigera's click returns once the new page has settled; the other drivers'
clicks return at once and they then poll the URL, so the browse rows do not
time the same readiness. The example.com check is the title; h1 is read, not checked.
Live pages change between runs, so these are informational, not part of the
correctness gate.

**CDP transport A/B** (same build and run): `--remote-debugging-pipe` (default on Linux/macOS)
vs a DevTools WebSocket port: cold start 0.26s vs 0.27s (medians),
session 0.57s vs 0.58s. No speed difference: the WebSocket handshake itself is ~10 ms. The pipe is the default for two other reasons:
it opens no TCP port that another local process could attach to, and Chrome exits when
navigera dies (EOF on its command pipe), so a killed agent leaks no browser
(`tests/edge_cases.rs`; on Windows, which has no pipe transport here, a
kill-on-close job object gives the same guarantee).

**This build vs navigera 0.12.0 at a4f596e, same machine and run** (`nv-baseline`,
built the same way by the ladder's `baseline_ref` A/B):
session 0.54s → 0.57s (+5%), cold start 0.27s → 0.26s median (-1%), warm eval 0.50 ms → 0.46 ms (-7%), browser CPU per session 0.79s → 0.74s. Between them: fewer round trips, page scripts kept as functions.

Where navigera's cold start goes (`NAVIGERA_TIMINGS`, best run): browser up
164 ms, first page 76 ms, close 4 ms.

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
| `navigera` (this repo) | CLI | 14 | 0.65s | 31 ms | 37 ms | 34 KB | 72 ms | 41 KB | 85 ms | ✓ |
| `navigera` on chrome-headless-shell | CLI | 14 | 0.42s | 25 ms | 30 ms | 34 KB | 80 ms | 41 KB | 47 ms | ✓ |
| `navigera`, known steps in `batch` | CLI | 6 | 0.65s | 86 ms | 36 ms | 34 KB | 86 ms | 41 KB | — | ✓ |
| `agent-browser` 0.43.0 (Vercel Labs, Rust) | CLI | 13 | 1.35s | 103 ms | 100 ms | 44 KB | 356 ms | 116 KB | — | ✓ |
| `playwright-cli` 0.1.24 (Microsoft) | CLI | 13 | 9.92s | 790 ms | 439 ms | 51 KB | 536 ms | 144 KB | — | ✓ |
| Playwright MCP 0.0.85 (Microsoft) | MCP | 13 | 5.31s | 402 ms | 69 ms | 51 KB | 192 ms | 144 KB | — | ✓ |
| Chrome DevTools MCP 1.13.0 (Google) | MCP | 13 | 3.35s | 205 ms | 109 ms | 34 KB | 338 ms | 78 KB | — | ✓ |

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
| `navigera` (sessions share one browser) | 358 MB | 496 MB | 680 MB | 46 MB |
| `navigera --own-browser` (a browser each) | 354 MB | 770 MB | 1294 MB | 134 MB |
| `agent-browser` 0.43.0 (Vercel Labs, Rust) | 496 MB | 1241 MB | 2200 MB | 243 MB |
| `playwright-cli` 0.1.24 (Microsoft) | 523 MB | 1292 MB | 2278 MB | 251 MB |
| Playwright MCP 0.0.85 (Microsoft) | 522 MB | 1300 MB | 2294 MB | 253 MB |
| Chrome DevTools MCP 1.13.0 (Google) | 567 MB | 1441 MB | 2585 MB | 288 MB |

**Lightpanda** (experimental) passes the same correctness gate in
0.19s with ~15 MB of browser memory. It has no
rendering engine and one tab, so it is listed apart from the Chrome ranking.
