# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- `status`: the session's engine, browser executable, CDP endpoint and tab
  state.
- `notes` in responses: what the engine did differently from what was asked
  (e.g. a Lightpanda popup loaded in the current tab).

### Fixed

- Lightpanda 1.0.x (`--engine lightpanda`): `goto` waits for the load event
  (it returned at commit and gave up after 500 ms); confirm()/prompt() get
  the dialog policy (they returned false/null while reported accepted);
  `target=_blank` pages are followed in the one tab; iframes and stylesheets
  load; table cells keep their text in `ax`; shadow-DOM clicks no longer wait
  3 s; no `xvfb-run` wrapper; telemetry off through
  `LIGHTPANDA_DISABLE_TELEMETRY`.
- `start` on a running session with a different engine fails instead of
  silently reusing it.
- Dialogs, popups and JS navigations reported just before a command's
  response are no longer missed by the op that caused them.
- Lightpanda: a navigation a click handler starts (`location.href = ...`)
  is waited for. The engine reports it only after the click's reply, so the
  click returned the old page with `loading: true`; it now returns the page
  the click led to.
- Lightpanda: `ax` shows what open shadow roots hold. The engine's tree
  stops at a shadow root and gives the nodes inside no ids, so a coupon
  field in a web component was invisible to an agent reading the snapshot;
  it is now listed under the root, without refs, with `--text` as the way to
  target it.

## [0.4.1] - 2026-10-14

### Added

- `find <description>`, `check <question>` and `--find <description>` on
  element commands: element choice and yes/no questions about the page,
  answered by a System One decision model (TypeSafe Jev) when
  `TYPESAFE_API_KEY` or `NAVIGERA_DECIDER_URL` is set. MCP gains
  `browser_find`, `browser_check` and a `find` target in that case only.
  Typed values are redacted from what is sent. `start` reports `"decider"`
  with the model id, so an agent can tell without reading the key.
- `server.json` for the MCP Registry (`io.github.andrey-usa/navigera`; the npm
  package carries the matching `mcpName`), published by the release workflow
  when `PUBLISH_MCP_REGISTRY` is set.
- `install-skill --continue` (`.continue/skills`); `--global` finds the home
  directory on Windows too.
- `navigera mcp`: a Model Context Protocol server on stdio for IDE agents
  (Cline, Roo Code, Kilo Code, Continue, VS Code, Claude Desktop, Cursor).
  18 tools named after Playwright MCP's, `snapshot: true` on every action,
  `browser_run` for several steps in one call; the browser starts on the first
  tool call.

### Security

- Chrome's renderer sandbox is on by default. `--no-sandbox` is added only as
  root, with `NAVIGERA_NO_SANDBOX=1`, or when Chrome reports that the machine
  has no usable sandbox (with a warning on stderr). That last case is
  remembered per browser binary until it changes or the machine reboots, so
  only the first launch pays for the failed attempt (about 350 ms with Chrome
  for Testing on a GitHub runner).
- Named sessions live in a private per-user directory
  (`$XDG_RUNTIME_DIR/navigera` or `$TMPDIR/navigera-<uid>`, mode 0700) instead
  of `/tmp/navigera-<name>.sock`, so another user can't squat a session's
  socket; the log is 0600 and doesn't follow symlinks.
- `install.sh` and `install.ps1` verify the release's SHA-256 checksum.
- The skill tells agents to treat page text as data, not instructions.

## [0.4.0] - 2026-10-09

First public release.

### Added

- Named sessions: a warm browser behind a local socket, one shell command per
  step (`navigera -s <name> start`, then `goto`, `ax`, `click`, …, `quit`).
- `ax`: the page as a compact accessibility tree with `[ref=N]` handles,
  including iframes (cross-origin too) and open shadow DOM; links carry their
  `url`.
- Element commands by ref, CSS selector or visible text, with auto-wait;
  trusted mouse and keyboard input; tabs, dialogs, popups, uploads,
  screenshots.
- `serve`: a JSON-lines protocol on stdin/stdout; a line holding a JSON array
  runs as a batch.
- `eval --file <path>` and `eval -` (script from stdin).
- `--attach` (your running Chrome 144+) and `--profile` (a persistent
  navigera browser).
- An agent skill compiled into the binary (`navigera skill`,
  `navigera install-skill`).
- Prebuilt binaries for Linux (x86_64, aarch64; static), macOS (arm64,
  x86_64) and Windows (x64); install scripts for sh and PowerShell; packages
  for crates.io, npm, PyPI, Homebrew and Scoop.

[0.4.1]: https://github.com/andrey-usa/navigera/releases/tag/v0.4.1
[0.4.0]: https://github.com/andrey-usa/navigera/releases/tag/v0.4.0
