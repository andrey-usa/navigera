# navigera

[![CI](https://github.com/andrey-usa/navigera/actions/workflows/ci.yml/badge.svg)](https://github.com/andrey-usa/navigera/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/navigera.svg)](https://crates.io/crates/navigera)
[![PyPI](https://img.shields.io/pypi/v/navigera.svg)](https://pypi.org/project/navigera/)
[![npm](https://img.shields.io/npm/v/navigera.svg)](https://www.npmjs.com/package/navigera)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Documentation](https://img.shields.io/badge/documentation-site-blue.svg)](https://andrey-usa.github.io/navigera/)

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
Lightpanda (`--engine lightpanda`, experimental). CI runs the end-to-end
suite on the current Chrome Stable and the three milestones before it,
chrome-headless-shell, Chrome Beta, the runner's Chrome on Linux and
Windows, and Lightpanda; every CDP call navigera makes is checked against
the protocol each browser serves. Details: [Supported
browsers](https://andrey-usa.github.io/navigera/concepts/browsers.html).

## Benchmarks

One scripted session (goto, evals, fill and click, a screenshot, a second
tab) driven by each tool against the same headless Chrome, in one CI run
([bench run 42759659475](https://github.com/andrey-usa/navigera/actions/runs/42759659475), 2026-11-10, GitHub's `ubuntu-latest`); each
driver's extracted data is checked identical:

| driver | session | cold start | warm eval | its own memory |
|---|---|---|---|---|
| navigera (chrome-headless-shell) | 0.25 s | 0.07 s | 0.32 ms | 7 MB |
| navigera (Chrome) | 0.41 s | 0.19 s | 0.35 ms | 7 MB |
| go-rod 0.116.2 (Go) | 0.51 s | 0.22 s | 0.24 ms | 12 MB |
| puppeteer-core 25.16.0 (Node) | 0.61 s | 0.37 s | 0.41 ms | 78 MB |
| playwright-core 1.65.0 (Bun 1.4.4) | 0.80 s | 0.47 s | 0.73 ms | 104 MB |
| playwright-core 1.65.0 (Node) | 0.89 s | 0.51 s | 0.89 ms | 139 MB |

Through an agent's tool calls, one step takes 21 ms with navigera, 80 ms with agent-browser 0.42.0, 181 ms with Chrome DevTools MCP 1.12.0, 363 ms with Playwright MCP 0.0.85, 588 ms with playwright-cli 0.1.24 (same run).

Where another tool did better in this run: warm eval: go-rod 0.116.2 (Go) 0.24 ms, navigera 0.35 ms; browser memory: puppeteer-core 25.16.0 (Node) 365 MB, navigera 399 MB.

Every table, the method and how to rerun it: [Benchmarks](https://andrey-usa.github.io/navigera/benchmarks/results.html).

## Documentation

<!-- Written by docs-site/build.py from docs-site/site.toml: edit that, then run it with --write. -->

The docs site, https://andrey-usa.github.io/navigera/, has the same parts for every tool:

- **Start:** [Install](https://andrey-usa.github.io/navigera/start/install.html) · [Quick start](https://andrey-usa.github.io/navigera/start/quick-start.html) · [A first session](https://andrey-usa.github.io/navigera/start/tutorial.html)
- **Guides:** [Hand the agent your browser](https://andrey-usa.github.io/navigera/guides/your-browser.html) · [Drive a browser from a program](https://andrey-usa.github.io/navigera/guides/programs.html) · [Let a fast model pick the element](https://andrey-usa.github.io/navigera/guides/decider.html) · [Use a page's own tools (WebMCP)](https://andrey-usa.github.io/navigera/guides/webmcp.html) · [Switch from agent-browser](https://andrey-usa.github.io/navigera/guides/from-agent-browser.html)
- **Concepts:** [How navigera works](https://andrey-usa.github.io/navigera/concepts/how-it-works.html) · [Supported browsers](https://andrey-usa.github.io/navigera/concepts/browsers.html) · [Several models, one browser](https://andrey-usa.github.io/navigera/concepts/multi-model.html)
- **Reference:** [Commands](https://andrey-usa.github.io/navigera/reference/commands.html) · [navigera --help](https://andrey-usa.github.io/navigera/reference/help.html)
- **Agents:** [For AI agents](https://andrey-usa.github.io/navigera/agents/overview.html) · [Skill](https://andrey-usa.github.io/navigera/agents/skill.html) · [Skill references](https://andrey-usa.github.io/navigera/agents/skill-references.html) · [MCP server](https://andrey-usa.github.io/navigera/agents/mcp.html)
- **Benchmarks:** [Results](https://andrey-usa.github.io/navigera/benchmarks/results.html) · [Where navigera is not ahead](https://andrey-usa.github.io/navigera/benchmarks/losses.html) · [Agent eval](https://andrey-usa.github.io/navigera/benchmarks/agent-eval.html)
- **Project:** [Changelog](https://andrey-usa.github.io/navigera/project/changelog.html) · [Contributing](https://andrey-usa.github.io/navigera/project/contributing.html) · [Source layout](https://andrey-usa.github.io/navigera/project/layout.html) · [Security](https://andrey-usa.github.io/navigera/project/security.html)

For agents: [llms.txt](https://andrey-usa.github.io/navigera/llms.txt) lists every page as markdown, and [llms-full.txt](https://andrey-usa.github.io/navigera/llms-full.txt) is the whole site in one file.

## Contributing

Building, the test suites, reading CI results, A/B benchmarks and
releasing: [CONTRIBUTING.md](https://github.com/andrey-usa/navigera/blob/master/CONTRIBUTING.md).

## License

MIT: [LICENSE](https://github.com/andrey-usa/navigera/blob/master/LICENSE).
`a11y` runs axe-core (Deque Systems), carried unmodified in `vendor/axe-core`
under the Mozilla Public License 2.0, its licenses beside it.
