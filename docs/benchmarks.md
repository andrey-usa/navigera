# Benchmarks

Numbers: [bench run 45918635350](https://github.com/andrey-usa/navigera/actions/runs/45918635350).

Same scripted session (launch → goto listing → title/count/extract evals →
fill+click filter → visible-count eval → screenshot → new tab → goto detail →
title/row-count evals → close) driven against the same headless browser by
each driver. Fixtures are deterministic and local (500-card listing,
1000-row detail). navigera is the static Linux binary the releases ship.
Reps are interleaved across contenders.

| contender | session wall, best (median) of 3 | warm eval mean (200×) | cold start, median (best) of 15 | driver CPU (own) | driver peak RSS (own) | browser memory (PSS) |
|---|---|---|---|---|---|---|
| `nv-shell` (this repo on chrome-headless-shell) | **0.22s** (0.23) | 0.16 ms | **0.07s** (0.06) | **<10 ms** | **8 MB** | 230 MB |
| `gorod-shell` (go-rod 0.116.2, Go, on chrome-headless-shell) | 0.29s (0.30) | **0.15 ms** | 0.08s (0.07) | **<10 ms** | 12 MB | 252 MB |
| `nv-serve` (this repo on Chrome) | 0.37s (0.39) | 0.27 ms | 0.18s (0.17) | **<10 ms** | **8 MB** | 363 MB |
| `nv-brave` (this repo on Brave) | 0.38s (0.40) | 0.28 ms | 0.21s (0.20) | **<10 ms** | **8 MB** | 587 MB |
| `nv-edge` (this repo on Edge) | 0.42s (0.43) | 0.29 ms | 0.23s (0.22) | **<10 ms** | **8 MB** | 736 MB |
| `gorod` (go-rod 0.116.2, Go) | 0.48s (0.49) | 0.16 ms | 0.20s (0.18) | **<10 ms** | 12 MB | 386 MB |
| `chromiumoxide` 0.7.0 (Rust) | 0.55s (0.56) | 0.41 ms | 0.25s (0.23) | **10 ms** | 10 MB | 423 MB |
| `chromedp` 0.19.1 (Go) | 0.60s (0.60) | 0.27 ms | 0.32s (0.30) | **20 ms** | **8 MB** | 420 MB |
| `puppeteer-core` 25.19.0 (Node) | 0.61s (0.64) | 0.44 ms | 0.37s (0.36) | 0.25 s | 81 MB | 324 MB |
| `chromey` 2.58.2 (Rust, maintained chromiumoxide fork) | 0.71s (0.73) | 0.33 ms | 0.24s (0.23) | **30 ms** | 22 MB | 445 MB |
| `playwright-core` 1.65.1 (Bun 1.4.5) | 0.76s (0.83) | 0.70 ms | 0.46s (0.45) | 0.38 s | 114 MB | 394 MB |
| `playwright-core` 1.65.1 (Node) | 0.81s (0.82) | 0.76 ms | 0.51s (0.51) | 0.44 s | 143 MB | 387 MB |
| `nv-lightpanda` (this repo on Lightpanda, 1 tab) | 0.16s (0.16) | 0.28 ms | 0.11s (0.11) | **<10 ms** | 9 MB | 15 MB |

Measured 2026-11-27 on GitHub-hosted `ubuntu-24.04, image 20261122.23.1` (Intel(R) Xeon(R) 6975P-C — 4 vCPU · Chrome 158.0.8127.36).
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

**Reading the table**: every driver but `nv-shell`, `gorod-shell`, `nv-edge` and `nv-brave` runs the same Google Chrome; on it, nv-serve (0.37s) is ahead of the fastest other driver, gorod (0.48s). `nv-shell`'s lead is partly chrome-headless-shell's, a lighter build; its pair is `gorod-shell` (0.29s), go-rod on the same shell.

## Real-world (public internet, median of 9, same run)

example.com goto → title check: 12 of 12 pass; nv-shell 0.14s, gorod-shell 0.14s, nv-serve 0.24s, gorod 0.27s, nv-brave 0.28s, chromiumoxide 0.30s, nv-edge 0.31s, chromey 0.32s, chromedp 0.42s, puppeteer 0.43s, playwright-bun 0.54s, playwright 0.59s.

GitHub browse (awesome-list scroll + trending click-through): 12 of 12 pass; nv-shell 2.57s, gorod-shell 2.59s, nv-serve 2.77s, gorod 2.81s, nv-brave 2.86s, puppeteer 2.87s, nv-edge 2.94s, chromiumoxide 2.95s, chromedp 3.00s, chromey 3.03s, playwright-bun 3.11s, playwright 3.19s.
navigera's click returns once the new page has settled; the other drivers'
clicks return at once and they then poll the URL, so the browse rows do not
time the same readiness. The example.com check is the title; h1 is read, not checked.
Live pages change between runs, so these are informational, not part of the
correctness gate.

**CDP transport A/B** (same build and run): `--remote-debugging-pipe` (default on Linux/macOS)
vs a DevTools WebSocket port: cold start 0.18s vs 0.18s (medians),
session 0.37s vs 0.38s. No speed difference: the WebSocket handshake itself is ~10 ms. The pipe is the default for two other reasons:
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
| `navigera` (this repo) | CLI | 14 | 0.44s | 21 ms | 21 ms | 34 KB | 40 ms | 41 KB | 52 ms | ✓ |
| `navigera` on chrome-headless-shell | CLI | 14 | 0.26s | 16 ms | 25 ms | 34 KB | 41 ms | 41 KB | 28 ms | ✓ |
| `navigera`, known steps in `batch` | CLI | 6 | 0.42s | 60 ms | 22 ms | 34 KB | 39 ms | 41 KB | — | ✓ |
| `agent-browser` 0.45.0 (Vercel Labs, Rust) | CLI | 13 | 0.97s | 72 ms | 84 ms | 44 KB | 219 ms | 116 KB | — | ✓ |
| `playwright-cli` 0.1.25 (Microsoft) | CLI | 13 | 7.90s | 633 ms | 298 ms | 51 KB | 370 ms | 144 KB | — | ✓ |
| Playwright MCP 0.0.86 (Microsoft) | MCP | 13 | 4.70s | 366 ms | 44 ms | 51 KB | 122 ms | 144 KB | — | ✓ |
| Chrome DevTools MCP 1.14.0 (Google) | MCP | 13 | 2.89s | 188 ms | 76 ms | 34 KB | 193 ms | 78 KB | — | ✓ |

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
| `navigera` (sessions share one browser) | 332 MB | 463 MB | 641 MB | 44 MB |
| `navigera --own-browser` (a browser each) | 331 MB | 690 MB | 1150 MB | 117 MB |
| `agent-browser` 0.45.0 (Vercel Labs, Rust) | 496 MB | 1258 MB | 2248 MB | 250 MB |
| `playwright-cli` 0.1.25 (Microsoft) | 523 MB | 1302 MB | 2298 MB | 254 MB |
| Playwright MCP 0.0.86 (Microsoft) | 528 MB | 1306 MB | 2314 MB | 255 MB |
| Chrome DevTools MCP 1.14.0 (Google) | 553 MB | 1388 MB | 2488 MB | 276 MB |

**Lightpanda** (experimental) passes the same correctness gate in
0.16s with ~15 MB of browser memory. It has no
rendering engine and one tab, so it is listed apart from the Chrome ranking.
