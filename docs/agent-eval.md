# Agent eval: how a generic agent copes with each tool

Benchmarks time scripted steps. The agent eval asks the question that
matters for agents: can a generic coding agent, given only the tool and its
own documentation, finish real tasks? [`agent-eval.yml`](../.github/workflows/agent-eval.yml)
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

### One model, six tools

DeepSeek-V4.1-Flash drives each tool through one shell function on a local
shop the harness serves: a search, an order form, a price table and a
catalog that loads in pages. Success is judged from what the site recorded
(the search run, the order placed) and from the final answer, never from
the agent's claim; the table and the catalog are drawn per session, so no
file holds an answer. CLI tools run without a shell (the command must start
with the tool, one per call), MCP servers get one `tools/call` per call,
and every tool starts on the home page. Four tasks, 12 reps each, the tools
at their latest npm releases; medians over each tool's 48 sessions
([`llm-bench.yml`](https://github.com/andrey-usa/navigera/blob/master/.github/workflows/llm-bench.yml),
run [42228800128](https://github.com/andrey-usa/navigera/actions/runs/42228800128)).
Cost is the harness's estimate at SiliconFlow's list prices (per million
tokens: $0.15 uncached input, $0.003 cached input, $0.60 output), not a bill:

| tool | success | median turns | median tokens per session [IQR] | mean tokens | estimated $ for the 48 sessions |
|---|---|---|---|---|---|
| `navigera`, `batch` allowed | 48/48 | **5** | **5,094** [3,910–7,996] | **8,626** | **$0.0351** |
| `navigera` | 48/48 | 6.5 | 7,906 [5,590–9,261] | 9,543 | $0.0373 |
| `agent-browser` 0.41.2 | 48/48 | 8 | 8,035 [6,780–11,787] | 11,543 | $0.0393 |
| `playwright-cli` 0.1.23 | 48/48 | 7 | 9,682 [5,681–11,286] | 13,207 | $0.0460 |
| Playwright MCP 0.0.84 | 48/48 | 9 | 13,706 [7,759–20,145] | 15,515 | $0.0535 |
| Chrome DevTools MCP 1.12.0 | 48/48 | 10 | 13,790 [9,665–19,193] | 14,747 | $0.0506 |

Per task, the lowest median tokens: `navigera` with `batch` in all four (search
3.9K, form 5.1K, table 4.5K, catalog 12.7K). The best other tool is
agent-browser on search (6.2K) and form (8.1K, under `navigera` without
`batch` at 8.4K), playwright-cli on the table (5.4K; `navigera` without
`batch` 5.9K) and Playwright MCP on the catalog (22.0K; `navigera` without
`batch` 13.5K).

What navigera lost: without `batch` its median is only 1.6% under
agent-browser's (7,906 against 8,035, inside each other's IQR), it is behind
agent-browser on the order form, and playwright-cli and Playwright MCP
finished the table in fewer turns (median 4 against 5). A tool's catalog
cost varies two to six times between its reps (playwright-cli 12.2K–73.3K,
`navigera` 6.6K–31.9K), so close rows can swap places between runs; the
6-rep run of the same build (run
[42177759665](https://github.com/andrey-usa/navigera/actions/runs/42177759665))
had playwright-cli ahead of agent-browser by mean, and the 12 reps put it
behind. Earlier runs of the same harness: 8,705 tokens a
session without `batch` and 11,689 with it on 2026-11-03 (run
[41579693623](https://github.com/andrey-usa/navigera/actions/runs/41579693623));
16,661 and 12,755 against navigera 0.7.1 (run
[41568863979](https://github.com/andrey-usa/navigera/actions/runs/41568863979)).
Since 0.7.1 an action says what it changed on the page, so the agent sees
the result of a click without another look, and a list item is one line of
the tree instead of three.

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

**Natural checks** ([`agent-check.yml`](../.github/workflows/agent-check.yml),
every master push, PRs and nightly): Gemini is told in plain words to buy
something with navigera and its skill. On the local shop, once on Linux
and once on Windows through PowerShell (real checks, judged from the
recorded order), and once on Sauce Labs' public demo
shop [saucedemo.com](https://www.saucedemo.com), which exists for automation
practice and takes no real payment (informational, judged from the
confirmation and the order total).
