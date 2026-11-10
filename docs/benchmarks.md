# Benchmarks

Numbers: [bench run 42863598577](https://github.com/andrey-usa/navigera/actions/runs/42863598577).

Same scripted session (launch → goto listing → title/count/extract evals →
fill+click filter → visible-count eval → screenshot → new tab → goto detail →
title/row-count evals → close) driven against the same headless browser by
each driver. Fixtures are deterministic and local (500-card listing,
1000-row detail). navigera is the static Linux binary the releases ship.
Reps are interleaved across contenders. Run in CI via `.github/workflows/bench.yml`.

| contender | session wall, best (median) of 3 | warm eval mean (200×) | cold start, median (best) of 15 | driver CPU (own) | driver peak RSS (own) | browser memory (PSS) |
|---|---|---|---|---|---|---|
| `nv-shell` (this repo on chrome-headless-shell) | **0.26s** (0.26) | 0.28 ms | **0.08s** (0.07) | **<10 ms** | **7 MB** | 231 MB |
| `nv-serve` (this repo on Chrome) | 0.41s (0.43) | 0.34 ms | 0.21s (0.19) | **<10 ms** | **7 MB** | 400 MB |
| `nv-brave` (this repo on Brave) | 0.45s (0.47) | 0.31 ms | 0.24s (0.22) | **<10 ms** | **7 MB** | 652 MB |
| `nv-edge` (this repo on Edge) | 0.50s (0.51) | 0.28 ms | 0.27s (0.25) | **<10 ms** | **7 MB** | 662 MB |
| `gorod` (go-rod 0.116.2, Go) | 0.55s (0.56) | **0.25 ms** | 0.23s (0.22) | **10 ms** | 12 MB | 417 MB |
| `puppeteer-core` 25.16.0 (Node) | 0.64s (0.66) | 0.42 ms | 0.40s (0.38) | 0.23 s | 79 MB | 368 MB |
| `chromiumoxide` 0.7.0 (Rust) | 0.65s (0.67) | 0.76 ms | 0.30s (0.25) | **20 ms** | 9 MB | 454 MB |
| `chromey` 2.58.2 (Rust, maintained chromiumoxide fork) | 0.77s (0.81) | 0.42 ms | 0.30s (0.26) | **40 ms** | 21 MB | 474 MB |
| `chromedp` 0.19.1 (Go) | 0.79s (0.80) | 0.48 ms | 0.40s (0.37) | **20 ms** | 8 MB | 436 MB |
| `playwright-core` 1.65.0 (Bun 1.4.4) | 0.82s (0.84) | 0.77 ms | 0.51s (0.49) | 0.35 s | 103 MB | 416 MB |
| `playwright-core` 1.65.0 (Node) | 0.87s (0.93) | 0.89 ms | 0.55s (0.52) | 0.40 s | 140 MB | 417 MB |
| `nv-lightpanda` (this repo on Lightpanda, 1 tab) | 0.15s (0.16) | 0.22 ms | 0.11s (0.11) | **<10 ms** | 8 MB | 15 MB |

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

**Reading the table.** Session wall order: nv-shell 0.26s, nv-serve 0.41s, nv-brave 0.45s, nv-edge 0.50s, gorod 0.55s, puppeteer 0.64s, chromiumoxide 0.65s, chromey 0.77s, chromedp 0.79s, playwright-bun 0.82s, playwright 0.87s. Every driver but `nv-shell`, `nv-edge` and `nv-brave` runs the same Google Chrome; on it, nv-serve (0.41s) is ahead of the fastest other driver, gorod (0.55s). `nv-shell` (0.26s) is navigera on chrome-headless-shell, a lighter build of the same browser that no other row runs, so its lead is partly the browser's. All native drivers (navigera,
go-rod, chromiumoxide, chromedp) spend tens of milliseconds of their own CPU
or less; the Node drivers spend hundreds and carry 80–150 MB of their own RSS.

## Real-world (public internet, median of 9, same run)

Numbers: [bench run 42863598577](https://github.com/andrey-usa/navigera/actions/runs/42863598577).

example.com goto → title check: 11 of 11 pass; nv-shell 0.19s, nv-serve 0.29s, gorod 0.34s, chromey 0.35s, nv-brave 0.36s, chromiumoxide 0.37s, nv-edge 0.37s, puppeteer 0.48s, chromedp 0.53s, playwright-bun 0.62s, playwright 0.65s.

GitHub browse (awesome-list scroll + trending click-through): 11 of 11 pass; nv-shell 2.97s, gorod 3.11s, nv-serve 3.18s, puppeteer 3.22s, chromiumoxide 3.24s, nv-brave 3.32s, chromey 3.33s, chromedp 3.47s, nv-edge 3.49s, playwright-bun 3.50s, playwright 3.61s.
navigera's click returns once the new page has settled; the other drivers'
clicks return at once and they then poll the URL, so the browse rows do not
time the same readiness. The example.com check is the title; h1 is read, not checked.
Live pages change between runs, so these are informational, not part of the
correctness gate. (The scroll check used to fail at random for every driver:
GitHub sets CSS `scroll-behavior: smooth`, so `scrollY` was read mid-animation;
contenders now scroll with `behavior: 'instant'`.)

**chrome-headless-shell** (the same Chrome build without its browser UI
layer): cold start 0.21s → 0.08s (medians), session 0.41s → 0.26s, browser memory 400 MB → 231 MB.

**CDP transport A/B** (same build and run): `--remote-debugging-pipe` (default on Linux/macOS)
vs a DevTools WebSocket port: cold start 0.21s vs 0.21s (medians),
session 0.41s vs 0.41s. No speed difference: the WebSocket handshake itself is ~10 ms. The pipe is the default for two other reasons:
it opens no TCP port that another local process could attach to, and Chrome exits when
navigera dies (EOF on its command pipe), so a killed agent leaks no browser
(`tests/edge_cases.rs`: over a WebSocket port the browser outlives its driver).
Windows has no pipe transport here and uses the WebSocket, with a kill-on-close
job object giving the same guarantee (`killed_session_server_takes_its_browser_down`
runs there too).

**This build vs navigera 0.10.1, same machine and run** (`nv-baseline`,
built the same way by the ladder's `baseline_ref` A/B):
session 0.42s → 0.41s (-1%), cold start 0.21s → 0.21s median (-1%), warm eval 0.61 ms → 0.34 ms (-44%), browser CPU per session 0.48s → 0.52s. This build is next since 0.10.1: a11y, vitals, passkey, element screenshots, totp and set cpu; eval answering in one awaited round trip again, its arm compiled once per document, and the pipe's replies found by their braces and with memchr.

Where navigera's cold start goes (`NAVIGERA_TIMINGS`, best run): browser up
123 ms, first page 57 ms, close 3 ms.

## Agent tools (scripted, warm session, best of 3)

Numbers: [bench run 42863598577](https://github.com/andrey-usa/navigera/actions/runs/42863598577).

How an agent's tool calls drive a browser: CLI tools run one process per
step (a shell tool), MCP servers get one `tools/call` per step over a
persistent connection. Same canonical session plus one page snapshot.
Snapshot sizes are each tool's default output, what the agent reads.
Outside the session's total, each tool also snapshots the 1000-row page,
and navigera starts a second agent's session while the first one's browser
is up (sessions share one browser, a context each).

| tool | kind | calls | total wall | per call | snapshot | its size | 1000-row snapshot | its size | 2nd session start | gate |
|---|---|---|---|---|---|---|---|---|---|---|
| `navigera` (this repo) | CLI | 14 | 0.50s | 24 ms | 27 ms | 34 KB | 40 ms | 41 KB | 62 ms | ✓ |
| `navigera` on chrome-headless-shell | CLI | 14 | 0.32s | 19 ms | 19 ms | 34 KB | 49 ms | 41 KB | 34 ms | ✓ |
| `navigera`, known steps in `batch` | CLI | 6 | 0.50s | 71 ms | 39 ms | 34 KB | 43 ms | 41 KB | — | ✓ |
| `agent-browser` 0.42.0 (Vercel Labs, Rust) | CLI | 13 | 1.13s | 85 ms | 86 ms | 44 KB | 255 ms | 116 KB | — | ✓ |
| `playwright-cli` 0.1.24 (Microsoft) | CLI | 13 | 7.70s | 616 ms | 272 ms | 50 KB | 343 ms | 144 KB | — | ✓ |
| Playwright MCP 0.0.85 (Microsoft) | MCP | 13 | 4.74s | 369 ms | 39 ms | 50 KB | 109 ms | 144 KB | — | ✓ |
| Chrome DevTools MCP 1.12.0 (Google) | MCP | 13 | 2.90s | 188 ms | 67 ms | 34 KB | 224 ms | 78 KB | — | ✓ |

Per-call time includes process start (CLI) or JSON-RPC (MCP), the hop to
the daemon, and the CDP work. playwright-cli and Playwright MCP share Playwright's tool
backend, which waits a fixed 500 ms after every action (`timeouts.settle`); playwright-cli
also starts a Node process per step. `batch` saves calls, which are an agent's turns,
not time: its row runs the same steps in fewer, longer calls.

## Parallel agents (memory, same run)

Numbers: [bench run 42863598577](https://github.com/andrey-usa/navigera/actions/runs/42863598577).

N agents, each with a session of its own (navigera `-s <name>`, agent-browser and
playwright-cli sessions, one MCP server each), all open at once on the listing
after a snapshot. Memory is every process the tool started (browsers, daemons,
servers), summed as PSS.

| tool | 1 agent | 4 agents | 8 agents | per extra agent |
|---|---|---|---|---|
| `navigera` (sessions share one browser) | 367 MB | 506 MB | 694 MB | 47 MB |
| `navigera --own-browser` (a browser each) | 366 MB | 787 MB | 1350 MB | 141 MB |
| `agent-browser` 0.42.0 (Vercel Labs, Rust) | 523 MB | 1334 MB | 2384 MB | 266 MB |
| `playwright-cli` 0.1.24 (Microsoft) | 548 MB | 1375 MB | 2430 MB | 269 MB |
| Playwright MCP 0.0.85 (Microsoft) | 551 MB | 1380 MB | 2452 MB | 272 MB |
| Chrome DevTools MCP 1.12.0 (Google) | 566 MB | 1434 MB | 2578 MB | 287 MB |

**Lightpanda** (experimental) passes the same correctness gate in
0.15s with ~15 MB of browser memory. It has no
rendering engine and one tab (the session opens the second page in the
same tab), so it is listed apart from the Chrome-family ranking.
