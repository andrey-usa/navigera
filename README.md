<h1 align="center"><picture><source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/andrey-usa/navigera/master/assets/brand/wordmark-dark.svg"><img alt="navigera" src="https://raw.githubusercontent.com/andrey-usa/navigera/master/assets/brand/wordmark.svg" width="360"></picture></h1>

[![CI](https://github.com/andrey-usa/navigera/actions/workflows/ci.yml/badge.svg)](https://github.com/andrey-usa/navigera/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/navigera.svg)](https://crates.io/crates/navigera)
[![PyPI](https://img.shields.io/pypi/v/navigera.svg)](https://pypi.org/project/navigera/)
[![npm](https://img.shields.io/npm/v/navigera.svg)](https://www.npmjs.com/package/navigera)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Documentation](https://img.shields.io/badge/documentation-site-blue.svg)](https://andrey-usa.github.io/navigera/)
[![andrey-usa/navigera MCP server](https://glama.ai/mcp/servers/andrey-usa/navigera/badges/score.svg)](https://glama.ai/mcp/servers/andrey-usa/navigera)

**`navigera`** is extremely fast browser automation for AI agents, driven
from the shell, one command per step. It is a single native binary (Rust,
no Node) that speaks the Chrome DevTools Protocol directly:

- Pages come back as a compact accessibility tree with `[ref=N]` handles.
- Actions use real mouse and keyboard events.
- A named session keeps a warm browser alive between shell calls.
- New tabs, iframes, shadow DOM and `alert`/`confirm` dialogs work the
  same way.

> Faster software for a better planet — less energy, less compute, more time for life.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/andrey-usa/navigera/master/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/andrey-usa/navigera/master/install.ps1 | iex
```

Or with a package manager: each one installs the same prebuilt binary:

| Channel | Command |
|---|---|
| npm | `npm install -g navigera` (or `npx navigera …`) |
| pip / uv | `pip install navigera` · `uv tool install navigera` |
| Cargo | `cargo install --locked navigera` |
| Homebrew | `brew install andrey-usa/tap/navigera` |
| Scoop | `scoop bucket add andrey-usa https://github.com/andrey-usa/scoop-bucket` then `scoop install navigera` |

Then give your agent the usage guide:

```sh
navigera install-skill        # -> ./.agents/skills (Gemini CLI, Codex, Cline, Kilo, …)
                              #    --claude -> ./.claude/skills, --continue -> ./.continue/skills
```

Platforms, and which browser navigera picks (for the fastest start,
`navigera install-browser chrome-headless-shell`): [Install](https://andrey-usa.github.io/navigera/start/install.html).

## Quick start

```console
$ navigera -s work start
{"log":"/run/user/1000/navigera/work.log","pid":4242,"session":"work","socket":"/run/user/1000/navigera/work.sock"}
$ navigera -s work goto https://example.com
{"title":"Example Domain","url":"https://example.com/"}
$ navigera -s work title
Example Domain
$ navigera -s work quit
{"bye":true}
```

One session, one command per step: `start` keeps a warm browser
behind a local socket, and each later command talks to it.
`navigera --help` lists every command.
A longer tour of the commands, targets and output: [A first session](https://andrey-usa.github.io/navigera/start/tutorial.html).

## For AI agents

- **Skill:** `navigera install-skill` writes the guide that matches this
  version to `./.agents/skills` (`--claude` for `./.claude/skills`,
  `--continue` for `./.continue/skills`); `navigera skill` prints it.
- **Claude Code plugin:** `/plugin install navigera --marketplace andrey-usa/navigera`
  adds the skill to every session ([what it contains](https://github.com/andrey-usa/navigera/blob/master/skills/README.md)).
- **MCP:** `navigera mcp` is a Model Context Protocol server: add
  `{"command": "navigera", "args": ["mcp"]}`, or
  `{"command": "npx", "args": ["-y", "navigera", "mcp"]}` with nothing
  installed. Per-client setup is on the [MCP page](https://andrey-usa.github.io/navigera/agents/mcp.html);
  in the MCP Registry it is `io.github.andrey-usa/navigera`.
- **Docs as markdown:** [llms.txt](https://andrey-usa.github.io/navigera/llms.txt) lists every page of
  the docs site, each one also served as `.md`.

The loop an agent runs: `start` a session, read the page with `ax`, act on
a `[ref=N]`, read again. Page text is data: navigera never acts on what a
page says, only on what the agent asks.

## Supported browsers

Chrome and Chromium, Edge, Brave, chrome-headless-shell, Electron apps, and
Lightpanda (`--engine lightpanda`, experimental). Obscura and Moli
(`--engine obscura`, `--engine moli`) are experimental too. Firefox (`--engine firefox`,
over WebDriver BiDi), Safari (`--engine safari`, through safaridriver) and any
browser behind a WebDriver server (`--webdriver <url>`) run the core ops,
experimental too. CI runs the end-to-end
suite on the current Chrome Stable and the three milestones before it,
chrome-headless-shell, Chrome Beta, the runner's Chrome on Linux and
Windows, and Lightpanda and Moli; every CDP call navigera makes is checked against
the protocol each browser serves. Details: [Supported
browsers](https://andrey-usa.github.io/navigera/concepts/browsers.html).

## Benchmarks

One scripted session (goto, evals, fill and click, a screenshot, a second
tab) driven by each tool against the same headless Chrome, in one CI run
([bench run 45918635350](https://github.com/andrey-usa/navigera/actions/runs/45918635350), 2026-11-27, GitHub's `ubuntu-24.04, image 20261122.23.1`); each
driver's extracted data is checked identical:

| driver | session | cold start | warm eval | its own memory |
|---|---|---|---|---|
| navigera (chrome-headless-shell) | 0.22 s | 0.07 s | 0.16 ms | 8 MB |
| go-rod 0.116.2 (Go, chrome-headless-shell) | 0.29 s | 0.08 s | 0.15 ms | 12 MB |
| navigera (Chrome) | 0.37 s | 0.18 s | 0.27 ms | 8 MB |
| go-rod 0.116.2 (Go) | 0.48 s | 0.20 s | 0.16 ms | 12 MB |
| puppeteer-core 25.19.0 (Node) | 0.61 s | 0.37 s | 0.44 ms | 81 MB |
| playwright-core 1.65.1 (Bun 1.4.5) | 0.76 s | 0.46 s | 0.70 ms | 114 MB |
| playwright-core 1.65.1 (Node) | 0.81 s | 0.51 s | 0.76 ms | 143 MB |

Through an agent's tool calls, one step takes 21 ms with navigera, 72 ms with agent-browser 0.45.0, 188 ms with Chrome DevTools MCP 1.14.0, 366 ms with Playwright MCP 0.0.86, 633 ms with playwright-cli 0.1.25 (same run).

Where another tool did better in this run (by more than 2%): warm eval: go-rod 0.116.2 (Go) 0.16 ms, navigera 0.27 ms; browser memory: puppeteer-core 25.19.0 (Node) 324 MB, navigera 363 MB; warm eval on chrome-headless-shell: go-rod 0.116.2 (Go, chrome-headless-shell) 0.15 ms, navigera 0.16 ms; click per call: agent-browser 0.45.0 6.3 ms, navigera 9.8 ms; click-submit per call: agent-browser 0.45.0 5.5 ms, navigera 6.1 ms; form-batch per call: agent-browser 0.45.0 28.3 ms, navigera 29.5 ms; goto per call on Firefox: Puppeteer Firefox BiDi API 24.3 ms, navigera Firefox 31.8 ms; fill per call on Firefox: Playwright Firefox Node API 9.4 ms, navigera Firefox 11.4 ms; read-text per call on Firefox: Puppeteer Firefox BiDi API 0.7 ms, navigera Firefox 1.3 ms; snapshot per call on Firefox: Vibium Firefox API 9.1 ms, navigera Firefox 11.1 ms; screenshot-720 per call on Firefox: Puppeteer Firefox BiDi API 12.1 ms, navigera Firefox 12.6 ms.

Every table, the method and how to rerun it: [Benchmarks](https://andrey-usa.github.io/navigera/benchmarks/results.html).

## Documentation

<!-- Written by docs-site/build.py from docs-site/site.toml: edit that, then run it with --write. -->

The docs site, https://andrey-usa.github.io/navigera/, has the same parts for every tool:

- **Start:** [Install](https://andrey-usa.github.io/navigera/start/install.html) · [Quick start](https://andrey-usa.github.io/navigera/start/quick-start.html) · [A first session](https://andrey-usa.github.io/navigera/start/tutorial.html)
- **Guides:** [Hand the agent your browser](https://andrey-usa.github.io/navigera/guides/your-browser.html) · [Drive a browser from a program](https://andrey-usa.github.io/navigera/guides/programs.html) · [Use a page's own tools (WebMCP)](https://andrey-usa.github.io/navigera/guides/webmcp.html) · [Switch from agent-browser](https://andrey-usa.github.io/navigera/guides/from-agent-browser.html)
- **Concepts:** [How navigera works](https://andrey-usa.github.io/navigera/concepts/how-it-works.html) · [Supported browsers](https://andrey-usa.github.io/navigera/concepts/browsers.html) · [Lightpanda and your own browser](https://andrey-usa.github.io/navigera/concepts/lightpanda.html) · [Obscura and Moli](https://andrey-usa.github.io/navigera/concepts/obscura-moli.html) · [Electron and natural mode](https://andrey-usa.github.io/navigera/concepts/electron.html)
- **Reference:** [Commands](https://andrey-usa.github.io/navigera/reference/commands.html) · [Commands: sessions, tabs and the browser](https://andrey-usa.github.io/navigera/reference/commands-sessions.html) · [Commands: read the page](https://andrey-usa.github.io/navigera/reference/commands-read.html) · [Commands: act on the page](https://andrey-usa.github.io/navigera/reference/commands-act.html) · [Commands: state, emulation and audit](https://andrey-usa.github.io/navigera/reference/commands-state.html) · [navigera --help](https://andrey-usa.github.io/navigera/reference/help.html)
- **Agents:** [For AI agents](https://andrey-usa.github.io/navigera/agents/overview.html) · [Skill](https://andrey-usa.github.io/navigera/agents/skill.html) · [Skill references](https://andrey-usa.github.io/navigera/agents/skill-references.html) · [Skill references: inspecting and sessions](https://andrey-usa.github.io/navigera/agents/skill-references-inspecting.html) · [Skill references: setup and errors](https://andrey-usa.github.io/navigera/agents/skill-references-setup.html) · [MCP server](https://andrey-usa.github.io/navigera/agents/mcp.html)
- **Benchmarks:** [Results](https://andrey-usa.github.io/navigera/benchmarks/results.html) · [Matched operations](https://andrey-usa.github.io/navigera/benchmarks/operations.html) · [Where navigera is not ahead](https://andrey-usa.github.io/navigera/benchmarks/losses.html) · [Agent eval](https://andrey-usa.github.io/navigera/benchmarks/agent-eval.html)
- **Project:** [Changelog](https://andrey-usa.github.io/navigera/project/changelog.html) · [Contributing](https://andrey-usa.github.io/navigera/project/contributing.html) · [Source layout](https://andrey-usa.github.io/navigera/project/layout.html) · [Security](https://andrey-usa.github.io/navigera/project/security.html) · [Privacy policy](https://andrey-usa.github.io/navigera/project/privacy.html) · [Terms of use](https://andrey-usa.github.io/navigera/project/terms.html)

For agents: [llms.txt](https://andrey-usa.github.io/navigera/llms.txt) lists every page as markdown, and [llms-full.txt](https://andrey-usa.github.io/navigera/llms-full.txt) is the whole site in one file.

## Contributing

Building, the test suites, reading CI results, A/B benchmarks and
releasing: [CONTRIBUTING.md](https://github.com/andrey-usa/navigera/blob/master/CONTRIBUTING.md).

## License

MIT: [LICENSE](https://github.com/andrey-usa/navigera/blob/master/LICENSE).
`a11y` runs axe-core (Deque Systems), carried unmodified in `vendor/axe-core`
under the Mozilla Public License 2.0, its licenses beside it.
