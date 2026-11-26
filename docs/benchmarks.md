# Benchmarks

Numbers: [bench run 45774240157](https://github.com/andrey-usa/navigera/actions/runs/45774240157).

Same scripted session (launch → goto listing → title/count/extract evals →
fill+click filter → visible-count eval → screenshot → new tab → goto detail →
title/row-count evals → close) driven against the same headless browser by
each driver. Fixtures are deterministic and local (500-card listing,
1000-row detail). navigera is the static Linux binary the releases ship.
Reps are interleaved across contenders.

| contender | session wall, best (median) of 3 | warm eval mean (200×) | cold start, median (best) of 15 | driver CPU (own) | driver peak RSS (own) | browser memory (PSS) |
|---|---|---|---|---|---|---|
| `nv-shell` (this repo on chrome-headless-shell) | **0.28s** (0.28) | 0.30 ms | **0.08s** (0.08) | **<10 ms** | 10 MB | 203 MB |
| `gorod-shell` (go-rod 0.116.2, Go, on chrome-headless-shell) | 0.35s (0.37) | **0.28 ms** | 0.10s (0.09) | **10 ms** | 12 MB | 246 MB |
| `nv-serve` (this repo on Chrome) | 0.46s (0.47) | 0.35 ms | 0.22s (0.21) | **<10 ms** | 10 MB | 361 MB |
| `nv-brave` (this repo on Brave) | 0.50s (0.51) | 0.32 ms | 0.26s (0.24) | **10 ms** | 10 MB | 585 MB |
| `nv-edge` (this repo on Edge) | 0.55s (0.55) | 0.35 ms | 0.28s (0.28) | **<10 ms** | 10 MB | 694 MB |
| `gorod` (go-rod 0.116.2, Go) | 0.59s (0.62) | 0.32 ms | 0.26s (0.24) | **20 ms** | 12 MB | 399 MB |
| `chromiumoxide` 0.7.0 (Rust) | 0.69s (0.70) | 0.72 ms | 0.34s (0.31) | **10 ms** | 9 MB | 430 MB |
| `puppeteer-core` 25.19.0 (Node) | 0.78s (0.80) | 0.50 ms | 0.46s (0.43) | 0.29 s | 80 MB | 369 MB |
| `chromedp` 0.19.1 (Go) | 0.81s (0.84) | 0.37 ms | 0.41s (0.38) | **30 ms** | **8 MB** | 421 MB |
| `chromey` 2.58.2 (Rust, maintained chromiumoxide fork) | 0.87s (0.89) | 0.70 ms | 0.32s (0.28) | **40 ms** | 16 MB | 446 MB |
| `playwright-core` 1.65.1 (Bun 1.4.5) | 0.95s (1.17) | 0.80 ms | 0.58s (0.55) | 0.46 s | 113 MB | 390 MB |
| `playwright-core` 1.65.1 (Node) | 1.02s (1.04) | 0.85 ms | 0.64s (0.61) | 0.53 s | 142 MB | 401 MB |
| `nv-lightpanda` (this repo on Lightpanda, 1 tab) | 0.17s (0.18) | 0.34 ms | 0.11s (0.11) | **<10 ms** | 11 MB | 15 MB |

Measured 2026-11-26 on GitHub-hosted `ubuntu-24.04, image 20261116.376.1` (AMD EPYC 9V74 80-Core Processor — 4 vCPU · Chrome 157.0.8103.78).
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

**Reading the table**: every driver but `nv-shell`, `gorod-shell`, `nv-edge` and `nv-brave` runs the same Google Chrome; on it, nv-serve (0.46s) is ahead of the fastest other driver, gorod (0.59s). `nv-shell`'s lead is partly chrome-headless-shell's, a lighter build; its pair is `gorod-shell` (0.35s), go-rod on the same shell.

## Real-world (public internet, median of 9, same run)

example.com goto → title check: 12 of 12 pass; nv-shell 0.16s, gorod-shell 0.18s, nv-serve 0.29s, nv-brave 0.33s, gorod 0.34s, nv-edge 0.36s, chromey 0.39s, chromiumoxide 0.41s, chromedp 0.54s, puppeteer 0.54s, playwright-bun 0.64s, playwright 0.71s.

GitHub browse (awesome-list scroll + trending click-through): 12 of 12 pass; nv-shell 2.70s, gorod-shell 2.70s, nv-serve 2.93s, gorod 2.96s, nv-brave 3.04s, puppeteer 3.14s, nv-edge 3.18s, chromiumoxide 3.22s, chromey 3.22s, chromedp 3.25s, playwright-bun 3.33s, playwright 3.46s.
navigera's click returns once the new page has settled; the other drivers'
clicks return at once and they then poll the URL, so the browse rows do not
time the same readiness. The example.com check is the title; h1 is read, not checked.
Live pages change between runs, so these are informational, not part of the
correctness gate.

**CDP transport A/B** (same build and run): `--remote-debugging-pipe` (default on Linux/macOS)
vs a DevTools WebSocket port: cold start 0.22s vs 0.23s (medians),
session 0.46s vs 0.48s, within run-to-run noise (the pipe's 3 sessions took 0.46–1.02s): the WebSocket handshake itself is ~10 ms. The pipe is the default for two other reasons:
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
| `navigera` (this repo) | CLI | 14 | 0.52s | 24 ms | 28 ms | 34 KB | 53 ms | 41 KB | 65 ms | ✓ |
| `navigera` on chrome-headless-shell | CLI | 14 | 0.36s | 22 ms | 24 ms | 34 KB | 49 ms | 41 KB | 35 ms | ✓ |
| `navigera`, known steps in `batch` | CLI | 6 | 0.52s | 76 ms | 37 ms | 34 KB | 51 ms | 41 KB | — | ✓ |
| `agent-browser` 0.45.0 (Vercel Labs, Rust) | CLI | 13 | 1.20s | 91 ms | 83 ms | 44 KB | 272 ms | 116 KB | — | ✓ |
| `playwright-cli` 0.1.25 (Microsoft) | CLI | 13 | 8.56s | 684 ms | 330 ms | 51 KB | 423 ms | 144 KB | — | ✓ |
| Playwright MCP 0.0.86 (Microsoft) | MCP | 13 | 4.94s | 380 ms | 51 ms | 51 KB | 143 ms | 144 KB | — | ✓ |
| Chrome DevTools MCP 1.14.0 (Google) | MCP | 13 | 3.12s | 198 ms | 88 ms | 34 KB | 259 ms | 78 KB | — | ✓ |

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
| `navigera` (sessions share one browser) | 335 MB | 478 MB | 659 MB | 46 MB |
| `navigera --own-browser` (a browser each) | 338 MB | 709 MB | 1163 MB | 118 MB |
| `agent-browser` 0.45.0 (Vercel Labs, Rust) | 496 MB | 1252 MB | 2228 MB | 247 MB |
| `playwright-cli` 0.1.25 (Microsoft) | 524 MB | 1292 MB | 2282 MB | 251 MB |
| Playwright MCP 0.0.86 (Microsoft) | 524 MB | 1296 MB | 2299 MB | 254 MB |
| Chrome DevTools MCP 1.14.0 (Google) | 556 MB | 1397 MB | 2499 MB | 277 MB |

**Lightpanda** (experimental) passes the same correctness gate in
0.17s with ~15 MB of browser memory. It has no
rendering engine and one tab, so it is listed apart from the Chrome ranking.
