# Benchmarks

Numbers: [bench run 43114462641](https://github.com/andrey-usa/navigera/actions/runs/43114462641).

Same scripted session (launch → goto listing → title/count/extract evals →
fill+click filter → visible-count eval → screenshot → new tab → goto detail →
title/row-count evals → close) driven against the same headless browser by
each driver. Fixtures are deterministic and local (500-card listing,
1000-row detail). navigera is the static Linux binary the releases ship.
Reps are interleaved across contenders.

| contender | session wall, best (median) of 3 | warm eval mean (200×) | cold start, median (best) of 15 | driver CPU (own) | driver peak RSS (own) | browser memory (PSS) |
|---|---|---|---|---|---|---|
| `nv-shell` (this repo on chrome-headless-shell) | **0.36s** (0.36) | 0.43 ms | **0.11s** (0.10) | **<10 ms** | **7 MB** | 244 MB |
| `gorod-shell` (go-rod 0.116.2, Go, on chrome-headless-shell) | 0.41s (0.42) | **0.40 ms** | 0.11s (0.10) | **20 ms** | 12 MB | 248 MB |
| `nv-serve` (this repo on Chrome) | 0.60s (0.61) | 0.50 ms | 0.27s (0.26) | **<10 ms** | **7 MB** | 347 MB |
| `nv-brave` (this repo on Brave) | 0.64s (0.67) | 0.47 ms | 0.33s (0.32) | **<10 ms** | **7 MB** | 655 MB |
| `nv-edge` (this repo on Edge) | 0.71s (0.73) | 0.45 ms | 0.38s (0.38) | **10 ms** | **7 MB** | 770 MB |
| `gorod` (go-rod 0.116.2, Go) | 0.73s (0.74) | 0.58 ms | 0.32s (0.31) | **20 ms** | 12 MB | 364 MB |
| `chromiumoxide` 0.7.0 (Rust) | 0.89s (0.91) | 0.98 ms | 0.39s (0.37) | **20 ms** | 9 MB | 463 MB |
| `puppeteer-core` 25.16.0 (Node) | 0.92s (0.92) | 0.71 ms | 0.55s (0.53) | 0.36 s | 80 MB | 375 MB |
| `chromey` 2.58.2 (Rust, maintained chromiumoxide fork) | 1.01s (1.07) | 1.03 ms | 0.38s (0.34) | 60 ms | 21 MB | 453 MB |
| `chromedp` 0.19.1 (Go) | 1.07s (1.07) | 0.60 ms | 0.53s (0.49) | **40 ms** | 8 MB | 463 MB |
| `playwright-core` 1.65.0 (Bun 1.4.4) | 1.18s (1.19) | 1.23 ms | 0.69s (0.66) | 0.56 s | 104 MB | 405 MB |
| `playwright-core` 1.65.0 (Node) | 1.35s (1.36) | 1.39 ms | 0.80s (0.76) | 0.65 s | 142 MB | 404 MB |
| `nv-lightpanda` (this repo on Lightpanda, 1 tab) | 0.19s (0.20) | 0.48 ms | 0.11s (0.11) | **<10 ms** | 8 MB | 15 MB |

Measured 2026-11-11 on GitHub-hosted `ubuntu-24.04, image 20261101.360.1` (AMD EPYC 7763 64-Core Processor — 4 vCPU · Chrome 156.0.8078.79).
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

**Reading the table**: every driver but `nv-shell`, `gorod-shell`, `nv-edge` and `nv-brave` runs the same Google Chrome; on it, nv-serve (0.60s) is ahead of the fastest other driver, gorod (0.73s). `nv-shell`'s lead is partly chrome-headless-shell's, a lighter build; its pair is `gorod-shell` (0.41s), go-rod on the same shell.

## Real-world (public internet, median of 9, same run)

example.com goto → title check: 12 of 12 pass; nv-shell 0.18s, gorod-shell 0.19s, nv-serve 0.36s, gorod 0.42s, nv-brave 0.44s, nv-edge 0.49s, chromey 0.50s, chromiumoxide 0.52s, puppeteer 0.65s, chromedp 0.66s, playwright-bun 0.80s, playwright 0.89s.

GitHub browse (awesome-list scroll + trending click-through): 12 of 12 pass; gorod-shell 3.27s, nv-shell 3.56s, gorod 3.73s, puppeteer 3.91s, chromiumoxide 3.91s, chromey 3.93s, nv-brave 3.95s, nv-serve 4.01s, playwright-bun 4.23s, nv-edge 4.35s, playwright 4.39s, chromedp 4.45s.
navigera's click returns once the new page has settled; the other drivers'
clicks return at once and they then poll the URL, so the browse rows do not
time the same readiness. The example.com check is the title; h1 is read, not checked.
Live pages change between runs, so these are informational, not part of the
correctness gate.

**CDP transport A/B** (same build and run): `--remote-debugging-pipe` (default on Linux/macOS)
vs a DevTools WebSocket port: cold start 0.27s vs 0.27s (medians),
session 0.60s vs 0.58s. No speed difference: the WebSocket handshake itself is ~10 ms. The pipe is the default for two other reasons:
it opens no TCP port that another local process could attach to, and Chrome exits when
navigera dies (EOF on its command pipe), so a killed agent leaks no browser
(`tests/edge_cases.rs`; on Windows, which has no pipe transport here, a
kill-on-close job object gives the same guarantee).

**This build vs navigera 0.8.0, same machine and run** (`nv-baseline`,
built the same way by the ladder's `baseline_ref` A/B):
session 0.69s → 0.60s (-13%), cold start 0.32s → 0.27s median (-15%), warm eval 0.60 ms → 0.50 ms (-18%), browser CPU per session 1.35s → 0.86s.

Where navigera's cold start goes (`NAVIGERA_TIMINGS`, best run): browser up
166 ms, first page 83 ms, close 2 ms.

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
| `navigera` (this repo) | CLI | 14 | 0.70s | 32 ms | 32 ms | 34 KB | 77 ms | 41 KB | 77 ms | ✓ |
| `navigera` on chrome-headless-shell | CLI | 14 | 0.44s | 26 ms | 32 ms | 34 KB | 81 ms | 41 KB | 40 ms | ✓ |
| `navigera`, known steps in `batch` | CLI | 6 | 0.68s | 94 ms | 35 ms | 34 KB | 86 ms | 41 KB | — | ✓ |
| `agent-browser` 0.42.0 (Vercel Labs, Rust) | CLI | 13 | 1.37s | 105 ms | 157 ms | 44 KB | 402 ms | 116 KB | — | ✓ |
| `playwright-cli` 0.1.24 (Microsoft) | CLI | 13 | 10.07s | 801 ms | 450 ms | 51 KB | 545 ms | 144 KB | — | ✓ |
| Playwright MCP 0.0.85 (Microsoft) | MCP | 13 | 5.38s | 408 ms | 73 ms | 51 KB | 192 ms | 144 KB | — | ✓ |
| Chrome DevTools MCP 1.13.0 (Google) | MCP | 13 | 3.38s | 207 ms | 101 ms | 34 KB | 359 ms | 78 KB | — | ✓ |

Per-call time includes process start (CLI) or JSON-RPC (MCP), the hop to
the daemon, and the CDP work. Playwright's tool backend, shared by
playwright-cli and Playwright MCP, waits a fixed 500 ms after every action;
playwright-cli also starts a Node process per step. `batch` saves calls, an
agent's turns, not time.

## Matched operations (one call per op, warm session, median of 10)

Numbers: [bench run 43114462641](https://github.com/andrey-usa/navigera/actions/runs/43114462641).

One CLI process (or one MCP `tools/call`) per op, against a warm session,
on the same fixture page. `goto` re-navigates to the listing; `fill` fills
the search box; `click` clicks Search (client-side filter, no navigation);
`click-submit` clicks the order form's submit, which a script handles (no
navigation); `read-text` reads the heading; `snapshot` is each tool's default
accessibility output; `screenshot` writes a PNG; `form-batch` fills two fields,
clicks submit and reads the status — one call for tools with a batch command,
four calls otherwise. Two rounds of 5 reps: rounds are the outer loop and tools
the inner loop, with the second round in reversed tool order. Each tool runs in
its default output mode (MCP tools over JSON-RPC, their only interface); no tool
is switched into a machine mode. A cell is complete only with all 10 reps and no
round error; a partial cell is shown as `incomplete (n/N)` and left out of the
per-op losses below.

| op | `navigera` (this repo) | `navigera` (baseline build) | `agent-browser` 0.42.0 (Vercel Labs, Rust) | `playwright-cli` 0.1.24 (Microsoft) | Playwright MCP 0.0.85 (Microsoft) | Chrome DevTools MCP 1.13.0 (Google) | `browser-use` 1.0.2 (exec model) |
|---|---|---|---|---|---|---|---|
| `goto` | 46.1 ms (1 call) | 58.7 ms (1 call) | 59.1 ms (1 call) | 524.0 ms (1 call) | 142.9 ms (1 call) | 142.2 ms (1 call) | 213.7 ms (1 call) |
| `fill` | 7.8 ms (1 call) | 6.6 ms (1 call) | 5.6 ms (1 call) | 383.4 ms (1 call) | 12.5 ms (1 call) | 232.1 ms (1 call) | 209.8 ms (1 call) |
| `click` | 23.2 ms (1 call) | 36.6 ms (1 call) | 24.8 ms (1 call) | 938.7 ms (1 call) | 573.1 ms (1 call) | 244.7 ms (1 call) | 314.5 ms (1 call) |
| `click-submit` | 20.8 ms (1 call) | 187.1 ms (1 call) | 18.0 ms (1 call) | 942.8 ms (1 call) | 561.2 ms (1 call) | 237.5 ms (1 call) | 247.9 ms (1 call) |
| `read-text` | 2.8 ms (1 call) | 2.4 ms (1 call) | 2.8 ms (1 call) | 877.4 ms (1 call) | 504.7 ms (1 call) | 204.4 ms (1 call) | 188.4 ms (1 call) |
| `snapshot` | 16.5 ms (1 call) | 20.4 ms (1 call) | 48.4 ms (1 call) | 416.0 ms (1 call) | 36.9 ms (1 call) | 36.6 ms (1 call) | 241.4 ms (1 call) |
| `screenshot` | 49.3 ms (1 call) | 50.7 ms (1 call) | 37.7 ms (1 call) | 407.3 ms (1 call) | 49.5 ms (1 call) | 71.7 ms (1 call) | 237.8 ms (1 call) |
| `form-batch` | 75.7 ms (1 call) | 194.9 ms (1 call) | 26.8 ms (1 call) | 2562.2 ms (4 calls) | 1101.8 ms (4 calls) | 868.6 ms (4 calls) | 276.9 ms (1 call) |

Snapshot outputs differ in form (each tool's default tree), so that row compares
call latency, not equivalent work. The browser-use snapshot comes from its
documented raw `cdp()` route; it has no built-in snapshot command. Per-call time
includes process start (CLI) or JSON-RPC (MCP) and the daemon hop. An `incomplete
(n/N)` cell has only n of the 10 expected reps; its median is partial evidence.

## Parallel agents (memory, same run)

N agents, each with a session of its own, all open at once on the listing
after a snapshot. Memory is every process the tool started (browsers,
daemons, servers), summed as PSS.
| tool | 1 agent | 4 agents | 8 agents | per extra agent |
|---|---|---|---|---|
| `navigera` (sessions share one browser) | 354 MB | 496 MB | 682 MB | 47 MB |
| `navigera --own-browser` (a browser each) | 352 MB | 780 MB | 1308 MB | 137 MB |
| `agent-browser` 0.42.0 (Vercel Labs, Rust) | 513 MB | 1317 MB | 2339 MB | 261 MB |
| `playwright-cli` 0.1.24 (Microsoft) | 539 MB | 1349 MB | 2390 MB | 264 MB |
| Playwright MCP 0.0.85 (Microsoft) | 537 MB | 1357 MB | 2412 MB | 268 MB |
| Chrome DevTools MCP 1.13.0 (Google) | 564 MB | 1450 MB | 2598 MB | 291 MB |

**Lightpanda** (experimental) passes the same correctness gate in
0.19s with ~15 MB of browser memory. It has no
rendering engine and one tab, so it is listed apart from the Chrome ranking.
