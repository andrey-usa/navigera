# Benchmarks

Numbers: [bench run 43398451080](https://github.com/andrey-usa/navigera/actions/runs/43398451080).

Same scripted session (launch → goto listing → title/count/extract evals →
fill+click filter → visible-count eval → screenshot → new tab → goto detail →
title/row-count evals → close) driven against the same headless browser by
each driver. Fixtures are deterministic and local (500-card listing,
1000-row detail). navigera is the static Linux binary the releases ship.
Reps are interleaved across contenders.

| contender | session wall, best (median) of 3 | warm eval mean (200×) | cold start, median (best) of 15 | driver CPU (own) | driver peak RSS (own) | browser memory (PSS) |
|---|---|---|---|---|---|---|
| `nv-shell` (this repo on chrome-headless-shell) | **0.36s** (0.36) | 0.45 ms | **0.11s** (0.09) | **<10 ms** | 9 MB | 235 MB |
| `gorod-shell` (go-rod 0.116.2, Go, on chrome-headless-shell) | 0.43s (0.44) | **0.39 ms** | 0.11s (0.10) | **20 ms** | 12 MB | 276 MB |
| `nv-serve` (this repo on Chrome) | 0.60s (0.61) | 0.48 ms | 0.29s (0.27) | **<10 ms** | 9 MB | 360 MB |
| `nv-brave` (this repo on Brave) | 0.67s (0.69) | 0.52 ms | 0.34s (0.32) | **<10 ms** | 9 MB | 653 MB |
| `nv-edge` (this repo on Edge) | 0.73s (0.75) | 0.46 ms | 0.41s (0.38) | **<10 ms** | 9 MB | 768 MB |
| `gorod` (go-rod 0.116.2, Go) | 0.74s (0.74) | 0.46 ms | 0.32s (0.31) | **20 ms** | 12 MB | 381 MB |
| `chromiumoxide` 0.7.0 (Rust) | 0.82s (0.84) | 0.89 ms | 0.42s (0.37) | **20 ms** | 9 MB | 409 MB |
| `puppeteer-core` 25.16.0 (Node) | 0.94s (0.97) | 0.66 ms | 0.58s (0.56) | 0.37 s | 79 MB | 371 MB |
| `chromedp` 0.19.1 (Go) | 0.95s (0.99) | 0.56 ms | 0.55s (0.49) | **40 ms** | **8 MB** | 438 MB |
| `chromey` 2.58.2 (Rust, maintained chromiumoxide fork) | 1.00s (1.04) | 0.87 ms | 0.41s (0.34) | 50 ms | 21 MB | 447 MB |
| `playwright-core` 1.65.0 (Bun 1.4.4) | 1.19s (1.19) | 0.99 ms | 0.70s (0.68) | 0.59 s | 104 MB | 411 MB |
| `playwright-core` 1.65.0 (Node) | 1.25s (1.25) | 1.11 ms | 0.80s (0.78) | 0.64 s | 140 MB | 391 MB |
| `nv-lightpanda` (this repo on Lightpanda, 1 tab) | 0.19s (0.20) | 0.49 ms | 0.12s (0.11) | **<10 ms** | 10 MB | 15 MB |

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

**Reading the table**: every driver but `nv-shell`, `gorod-shell`, `nv-edge` and `nv-brave` runs the same Google Chrome; on it, nv-serve (0.60s) is ahead of the fastest other driver, gorod (0.74s). `nv-shell`'s lead is partly chrome-headless-shell's, a lighter build; its pair is `gorod-shell` (0.43s), go-rod on the same shell.

## Real-world (public internet, median of 9, same run)

example.com goto → title check: 12 of 12 pass; nv-shell 0.19s, gorod-shell 0.20s, nv-serve 0.37s, gorod 0.41s, nv-brave 0.46s, chromey 0.48s, nv-edge 0.50s, chromiumoxide 0.51s, chromedp 0.65s, puppeteer 0.66s, playwright-bun 0.80s, playwright 0.89s.

GitHub browse (awesome-list scroll + trending click-through): 12 of 12 pass; gorod-shell 3.30s, nv-shell 3.55s, gorod 3.68s, nv-serve 3.84s, puppeteer 3.90s, chromey 4.00s, nv-brave 4.02s, chromiumoxide 4.03s, chromedp 4.04s, nv-edge 4.28s, playwright-bun 4.37s, playwright 4.41s.
navigera's click returns once the new page has settled; the other drivers'
clicks return at once and they then poll the URL, so the browse rows do not
time the same readiness. The example.com check is the title; h1 is read, not checked.
Live pages change between runs, so these are informational, not part of the
correctness gate.

**CDP transport A/B** (same build and run): `--remote-debugging-pipe` (default on Linux/macOS)
vs a DevTools WebSocket port: cold start 0.29s vs 0.29s (medians),
session 0.60s vs 0.63s, within run-to-run noise (the pipe's 3 sessions took 0.60–0.93s): the WebSocket handshake itself is ~10 ms. The pipe is the default for two other reasons:
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
| `navigera` (this repo) | CLI | 14 | 0.71s | 32 ms | 33 ms | 34 KB | 72 ms | 41 KB | 83 ms | ✓ |
| `navigera` on chrome-headless-shell | CLI | 14 | 0.44s | 26 ms | 35 ms | 34 KB | 84 ms | 41 KB | 44 ms | ✓ |
| `navigera`, known steps in `batch` | CLI | 6 | 0.68s | 94 ms | 39 ms | 34 KB | 77 ms | 41 KB | — | ✓ |
| `agent-browser` 0.43.0 (Vercel Labs, Rust) | CLI | 13 | 1.35s | 103 ms | 120 ms | 44 KB | 357 ms | 116 KB | — | ✓ |
| `playwright-cli` 0.1.24 (Microsoft) | CLI | 13 | 10.01s | 797 ms | 426 ms | 51 KB | 544 ms | 144 KB | — | ✓ |
| Playwright MCP 0.0.85 (Microsoft) | MCP | 13 | 5.32s | 403 ms | 70 ms | 51 KB | 180 ms | 144 KB | — | ✓ |
| Chrome DevTools MCP 1.13.0 (Google) | MCP | 13 | 3.39s | 206 ms | 103 ms | 34 KB | 337 ms | 78 KB | — | ✓ |

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
| `navigera` (sessions share one browser) | 357 MB | 496 MB | 676 MB | 46 MB |
| `navigera --own-browser` (a browser each) | 354 MB | 769 MB | 1301 MB | 135 MB |
| `agent-browser` 0.43.0 (Vercel Labs, Rust) | 496 MB | 1244 MB | 2195 MB | 243 MB |
| `playwright-cli` 0.1.24 (Microsoft) | 521 MB | 1289 MB | 2278 MB | 251 MB |
| Playwright MCP 0.0.85 (Microsoft) | 523 MB | 1295 MB | 2290 MB | 252 MB |
| Chrome DevTools MCP 1.13.0 (Google) | 562 MB | 1437 MB | 2582 MB | 289 MB |

**Lightpanda** (experimental) passes the same correctness gate in
0.19s with ~15 MB of browser memory. It has no
rendering engine and one tab, so it is listed apart from the Chrome ranking.
