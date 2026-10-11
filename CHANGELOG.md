# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

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

[0.4.0]: https://github.com/andrey-usa/navigera/releases/tag/v0.4.0
