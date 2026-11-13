# navigera plugin

Browser automation for agents, driven from the shell. This plugin adds one
skill, `navigera`, that teaches an agent to use the
[navigera](https://github.com/andrey-usa/navigera) command-line tool: a
single native binary (Rust, no Node) that drives Chrome and Electron apps
over the DevTools Protocol.

What an agent gets:

- **Pages it can read cheaply.** A compact accessibility tree with
  `[ref=N]` handles instead of HTML or screenshots.
- **One command per step.** `click`, `fill`, `select`, `press`, `wait`,
  `screenshot`, tabs, iframes, shadow DOM and dialogs, against a warm
  session that keeps cookies between calls.
- **What changed, without asking.** Each action prints the lines of the
  page that changed, so the next read is usually not needed.
- **Known steps in one call.** `batch` runs several commands in one turn
  and stops at the first failure.

## Measured

Six browser tasks, eight runs each, the same model (DeepSeek-V4.1-Flash)
driving each tool through its own skill
([run 43061276529](https://github.com/andrey-usa/navigera/actions/runs/43061276529)):

| tool | tasks solved | median turns | median tokens | median time |
|---|---|---|---|---|
| navigera | 48/48 | 8 | 31.7K | 19 s |
| playwright-cli 0.1.24 | 48/48 | 12 | 75.7K | 28 s |
| agent-browser 0.42.0 | 48/48 | 14 | 96.3K | 28 s |

Per-operation timings, the method, and the operations where another tool
is faster are published with every release:
[Benchmarks](https://andrey-usa.github.io/navigera/benchmarks/results.html).

## Install

In Claude Code:

```
/plugin install navigera --marketplace andrey-usa/navigera
```

The skill needs the `navigera` binary on the machine. Its first step
checks for it and, when it is missing, installs the latest release from
this repository:

```sh
curl -fsSL https://raw.githubusercontent.com/andrey-usa/navigera/master/install.sh | sh
```

Also on crates.io, npm and PyPI as `navigera`.

## What this plugin runs and fetches

- The plugin itself is text: `SKILL.md` and its reference pages. It has no
  hooks, no MCP server and no scripts.
- The skill tells the agent to run `navigera` commands in the shell. The
  install script above downloads the release binary from GitHub and checks
  its checksum.
- navigera starts a Chrome it finds on the machine, or one fetched with
  `navigera install-browser` when asked. It sends nothing anywhere itself:
  network traffic is the browser loading the pages the agent opens.

## More

- [Documentation](https://andrey-usa.github.io/navigera/)
- [Skill, as a page](https://andrey-usa.github.io/navigera/agents/skill.html)
- [MCP server](https://andrey-usa.github.io/navigera/agents/mcp.html), for
  clients without a shell
- [Issues](https://github.com/andrey-usa/navigera/issues)

MIT licensed.
