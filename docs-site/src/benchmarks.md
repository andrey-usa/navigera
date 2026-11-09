# Benchmarks and agent eval

## Benchmarks

Same scripted session (launch → goto listing → title/count/extract evals →
fill+click filter → visible-count eval → screenshot → new tab → goto detail →
title/row-count evals → close) driven against the same headless browser by
each driver. Fixtures are deterministic and local (500-card listing,
1000-row detail). Run in CI via `.github/workflows/bench.yml`.

| contender | session wall (best of 3) | warm eval mean (200×) | cold start (best of 5) | driver CPU (own) | driver peak RSS (own) | browser memory (PSS) |
|---|---|---|---|---|---|---|
| `nv-shell` (this repo on chrome-headless-shell) | **0.34s** | **0.42 ms** | **0.08s** | **<10 ms** | **6 MB** | 225 MB |
| `nv-brave` (this repo on Brave) | 0.59s | 0.53 ms | 0.31s | **<10 ms** | **6 MB** | 528 MB |
| `nv-serve` (this repo on Chrome) | 0.60s | 0.59 ms | 0.28s | **<10 ms** | **6 MB** | 370 MB |
| `nv-edge` (this repo on Edge) | 0.71s | 0.55 ms | 0.34s | **<10 ms** | **6 MB** | 763 MB |
| `gorod` (go-rod 0.116.2, Go) | 0.71s | 0.57 ms | 0.29s | **20 ms** | 16 MB | 391 MB |
| `chromiumoxide` 0.7 (Rust) | 0.82s | 1.00 ms | 0.32s | **20 ms** | 9 MB | 426 MB |
| `chromey` 2.x (Rust, maintained chromiumoxide fork) | 0.94s | 1.08 ms | 0.35s | 50 ms | 21 MB | 472 MB |
| `chromedp` 0.19.1 (Go) | 1.02s | 0.61 ms | 0.47s | **40 ms** | 13 MB | 479 MB |
| `puppeteer-core` (Node) | 1.03s | 0.83 ms | 0.63s | 0.39 s | 81 MB | 420 MB |
| `playwright-core` (Node) | 1.31s | 1.36 ms | 0.77s | 0.71 s | 150 MB | 418 MB |
| `nv-lightpanda` (this repo on Lightpanda, 1 tab) | 0.18s | 0.39 ms | 0.11s | **<10 ms** | **6 MB** | 14 MB |

Measured 2026-10-21 on GitHub-hosted `ubuntu-latest` (AMD EPYC 7763 64-Core Processor — 4 vCPU · Chrome 155.0.8059.39).
Every driver produced byte-identical extracted data and counts (correctness
gate); wall is best-of-N.
The ladder reruns weekly ([`bench.yml`](.github/workflows/bench.yml)) against
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

**Reading the table.** Session wall order: nv-shell 0.34s, nv-brave 0.59s, nv-serve 0.60s, nv-edge 0.71s, gorod 0.71s, chromiumoxide 0.82s, chromey 0.94s, chromedp 1.02s, puppeteer 1.03s, playwright 1.31s. navigera's best
(nv-shell) ranks #1 of 10; on regular Chrome, nv-serve (0.60s) is ahead of the fastest other driver, gorod (0.71s). All native drivers (navigera,
go-rod, chromiumoxide, chromedp) spend tens of milliseconds of their own CPU
or less; the Node drivers spend hundreds and carry 80–150 MB of their own RSS.

### Real-world (public internet, best of 3, same run)

example.com goto → title/h1: 10 of 10 pass; nv-shell 0.20s, gorod 0.41s, nv-serve 0.45s, chromey 0.45s, nv-brave 0.47s, chromiumoxide 0.48s, nv-edge 0.51s, chromedp 0.57s, puppeteer 0.75s, playwright 0.87s.

GitHub browse (awesome-list scroll + trending click-through): 10 of 10 pass; nv-shell 3.46s, gorod 3.63s, chromiumoxide 3.88s, nv-brave 3.90s, chromey 3.92s, puppeteer 4.22s, nv-edge 4.28s, nv-serve 4.37s, playwright 4.48s, chromedp 4.53s.
Live pages change between runs, so these are informational, not part of the
correctness gate. (The scroll check used to fail at random for every driver:
GitHub sets CSS `scroll-behavior: smooth`, so `scrollY` was read mid-animation;
contenders now scroll with `behavior: 'instant'`.)

**chrome-headless-shell** (the same Chrome build without its browser UI
layer): cold start 0.28s → 0.08s, session 0.60s → 0.34s, browser memory 370 MB → 225 MB.

**CDP transport A/B** (same build and run): `--remote-debugging-pipe` (default on Linux/macOS)
vs a DevTools WebSocket port: cold start 0.28s vs 0.30s,
session 0.60s vs 0.63s. No speed difference: the WebSocket handshake itself is ~10 ms. The pipe is the default for two other reasons:
it opens no TCP port that another local process could attach to, and Chrome exits when
navigera dies (EOF on its command pipe), so a killed agent leaks no browser
(`tests/edge_cases.rs`: over a WebSocket port the browser outlives its driver).
Windows has no pipe transport here and uses the WebSocket, with a kill-on-close
job object giving the same guarantee (`killed_session_server_takes_its_browser_down`
runs there too).

**This build vs the previous one, same machine and run** (`nv-baseline`,
built from the previous master by the ladder's `baseline_ref` A/B):
session 0.62s → 0.60s (-3%), cold start 0.30s → 0.28s (-5%), browser CPU per session 1.14s → 1.16s. The session now also records what pages log and fetch (`console`, `network`).

Where navigera's cold start goes (`NAVIGERA_TIMINGS`, best run): browser up
159 ms, first page 110 ms, close 2 ms.

### Agent tools (scripted, warm session, best of 3)

How an agent's tool calls drive a browser: CLI tools run one process per
step (a shell tool), MCP servers get one `tools/call` per step over a
persistent connection. Same canonical session plus one page snapshot.

| tool | kind | total wall | mean per step | snapshot | snapshot size | gate |
|---|---|---|---|---|---|---|
| `navigera` (this repo) | CLI | 0.84s | 43 ms | 190 ms | 36 KB | ✓ |
| `navigera` on chrome-headless-shell | CLI | 0.50s | 32 ms | 160 ms | 36 KB | ✓ |
| `agent-browser` (Vercel Labs, Rust) | CLI | 1.24s | 89 ms | 207 ms | 73 KB | ✓ |
| `playwright-cli` (Microsoft) | CLI | 10.07s | 801 ms | 430 ms | 50 KB | ✓ |
| Playwright MCP (Microsoft) | MCP | 5.32s | 403 ms | 76 ms | 50 KB | ✓ |
| Chrome DevTools MCP (Google) | MCP | 3.35s | 200 ms | 121 ms | 34 KB | ✓ |

Per-step time includes process start (CLI) or JSON-RPC (MCP), the hop to
the daemon, and the CDP work. playwright-cli and Playwright MCP share Playwright's tool
backend, which waits a fixed 500 ms after every action (`timeouts.settle`); playwright-cli
also starts a Node process per step.

**Lightpanda** (experimental) passes the same correctness gate in
0.18s with ~14 MB of browser memory. It has no
rendering engine and one tab (the session opens the second page in the
same tab), so it is listed apart from the Chrome-family ranking.

## Agent eval: how a generic agent copes with each tool

Benchmarks time scripted steps. The agent eval asks the question that
matters for agents: can a generic coding agent, given only the tool and its
own documentation, finish real tasks? [`agent-eval.yml`](.github/workflows/agent-eval.yml)
runs [Gemini CLI](https://github.com/google-gemini/gemini-cli) headless on a
free-tier API key against the local Acme Supply site, with **navigera**,
**playwright-cli** and **agent-browser** in turn:

- **skilled:** the tool is installed and its vendor's own Agent Skill is in
  the workspace (`.agents/skills/`);
- **onboard:** the agent gets only the tool's name and repository URL and
  must install it from its docs, then do a short task.

The six tasks are a price lookup, a purchase with a coupon and an iframe
payment form, a login plus a paginated order table, docs in a new tab with a
collapsed FAQ, a support form inside an iframe, and four named changes on a
170-line settings form that must leave everything else as it was. Success is judged from
the site's recorded state and the final answer, never from the agent's
claim. Every run also records model requests, tokens, shell commands and
off-tool workarounds (curl, ad-hoc scripts). Model, prompt template, Chrome
and site are identical across tools. `os: windows` (or `both`) runs the same
jobs on Windows runners, where the agent's shell is PowerShell.

The three tools run as parallel jobs. Keys live in the `main` environment,
one slot per tool, because Gemini's free quota is per Google Cloud project:
`GEMINI_API_KEY` (navigera), `GEMINI_API_KEY_2` (agent-browser) and
`GEMINI_API_KEY_3` (playwright-cli). An empty slot falls back to
`GEMINI_API_KEY`. Without any key only the scripted baseline runs: it checks
the site, the success checks and the harness with known-good navigera
command plans. Runs that die on a model quota or outage are reported as
**⚠ infra**, not counted as a tool failure.

`agent: openai` runs the same tasks, prompt, turn limit and timeout with
any OpenAI-compatible model instead of Gemini CLI (DeepSeek-V4.1-Flash
through SiliconFlow by default; `OPENAI_BASE_URL`, `SILICONFLOW_API_KEY`), so
a result can be checked against a second model family.

`navigera+jev` runs navigera with the decider configured, and
`navigera+jev-told` also asks for `--find` / `check` in the prompt; the
TypeSafe key (`TYPESAFE_API_KEY` in `main`) stays in a local relay that the
agent's shell never sees, and each run records the decider's calls and
tokens. `decisions: true` adds the Jev-against-Gemini decision comparison
above (`bench/decide/compare.py`).

**0.5.0 (2026-10-22)**, skilled, the six tasks twice per tool:

- DeepSeek-V4.1-Flash (run 39699303497): navigera passed 12/12 at a median
  50K tokens and 9 model turns, and 12/12 on Lightpanda (121K);
  playwright-cli 11/12 (96K), agent-browser 12/12 (153K). With the decider
  on, navigera passed 12/12 at 43K and the agent used it in 3 of 12 runs
  unprompted; asked to, in 11 of 12 (12/12, 62K).
- Gemini 3.5 Flash-Lite (run 39691538974): navigera 12/12 at 127K, and
  12/12 on Lightpanda (167K).

**0.4.1 (2026-10-14)**, Gemini 3.5 Flash-Lite, skilled: navigera passed 10/10
runs of the five tasks (2 each), median 7 model turns and 103K tokens.

**0.3.0 vs 0.2.1, same model, launched together** (purchase + account, 2
runs each, all passed): `ax` now waits for data a page is still fetching,
and the skill teaches chaining sure steps in one shell call. Median model
turns 18 → 10, median tokens 288K → 167K; every 0.3.0 run used fewer tokens
than every 0.2.1 run of the same task.
The scripted session didn't slow down: 0.51 s vs 0.49 s, cold start 0.23 s
both.

**Natural checks** ([`agent-check.yml`](.github/workflows/agent-check.yml),
every master push, PRs and nightly): Gemini is told in plain words to buy
something with navigera and its skill. On the local shop, once on Linux
and once on Windows through PowerShell (real checks, judged from the
recorded order), and once on Sauce Labs' public demo
shop [saucedemo.com](https://www.saucedemo.com), which exists for automation
practice and takes no real payment (informational, judged from the
confirmation and the order total).
