# navigera

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![CI](https://github.com/andrey-usa/navigera/actions/workflows/ci.yml/badge.svg)](https://github.com/andrey-usa/navigera/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/navigera.svg)](https://crates.io/crates/navigera)
[![PyPI](https://img.shields.io/pypi/v/navigera.svg)](https://pypi.org/project/navigera/)
[![npm](https://img.shields.io/npm/v/navigera.svg)](https://www.npmjs.com/package/navigera)

**`navigera`** — extremely fast browser automation for AI agents, from the
shell, one command per step.
It is a single native binary (Rust, no Node) that speaks the Chrome DevTools
Protocol directly:

- It reads pages as a compact accessibility tree with `[ref=N]` handles.
- It acts with real mouse and keyboard events.
- A warm browser behind a named session survives between shell calls.
- New tabs, iframes, shadow DOM and `alert`/`confirm` dialogs are handled
  for you.

> Faster software for a better planet — less energy, less compute, more time for life.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/andrey-usa/navigera/master/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/andrey-usa/navigera/master/install.ps1 | iex
```

Or with a package manager — each one installs the same prebuilt binary:

| | |
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

Linux, macOS and Windows are supported. On Windows the session socket is a
loopback TCP port that only accepts clients presenting a random token from
the session file in your temp directory, CDP runs over a DevTools
WebSocket, and Chrome sits in a kill-on-close job object, so it still exits
when navigera dies.

navigera finds Chrome or Chromium on its own: a system install,
`$CHROME_BIN`, or a Playwright/Puppeteer browser cache. For the fastest
cold start, point it at
[chrome-headless-shell](https://developer.chrome.com/blog/chrome-headless-shell):
`npx @puppeteer/browsers install chrome-headless-shell@stable` gives the same
Chrome without its browser UI layer. In the benchmark it cut cold start from
0.32 s to 0.11 s and the scripted session from 0.66 s to 0.32 s.
navigera uses a system Chrome first; with none installed it picks
chrome-headless-shell (or Chromium) from a Playwright or Puppeteer cache.
Set `$CHROME_BIN` to choose explicitly.

## Use (agents and people)

```sh
navigera -s work start                      # warm headless browser behind a local socket
navigera -s work goto example.com
navigera -s work --raw ax                   # page as an indented tree, [ref=N] on actionable nodes
navigera -s work click 12                   # act on a ref
navigera -s work fill 31 "hello" && navigera -s work press Enter
navigera -s work wait --text "Saved"
navigera -s work quit
```

```text
page "Checkout — Acme" http://shop.test/checkout
- main:
  - heading "Checkout" [level=1]
  - textbox "Full name" [ref=21]
  - combobox "Country" [ref=22] value="Select…" options: "Canada", "Mexico", …
  - radio "Express (1–2 days)" [checked, ref=25]
  - Iframe "Card payment":
    - document "Card details":
      - textbox "Card number" [ref=41]
  - button "Place order" [ref=30]
```

The commands are `goto`, `ax`, `click`, `fill`, `type`, `press`, `select`,
`hover`, `drag`, `scroll`, `upload`, `wait`, `eval`, `back`/`forward`/`reload`,
`screenshot`, `pdf`, the `tab-*` commands, `dialog` and `quit`, with `get`
and `is` for one value, `console` and `network` for what the page logged and
fetched, `cookies`, `storage` and `state save|load` (Playwright's
`storageState` format) for sign-ins, `set` for viewport, device, media, geo
and the like, and `route` to fail or answer requests.

- **Targets:** elements can be named by ref, `--selector <css>`,
  `--text "<visible text>"`, `--role/--name`, `--label`, `--placeholder` or
  `--testid`.
- **Output:** every command prints one JSON line, or just the result with
  `--raw`.
- **Help:** `navigera help <command>`.
- **Agent guide:** the version-matched guide is
  [`skills/navigera/SKILL.md`](skills/navigera/SKILL.md),
  also printed by `navigera skill`.
- **Reference:** [docs/navigera.md](docs/navigera.md) has the full
  reference, and [llms.txt](llms.txt) is an index for LLMs.

**Hand the agent your browser.** Three ways, from most direct to most
portable:

| | the agent gets | needs |
|---|---|---|
| `start --attach` | your running Chrome itself, in a window of its own: your logins, settings, extensions, live | Chrome 144+, remote debugging turned on once at `chrome://inspect/#remote-debugging`, and a click on Allow |
| `start --profile agent --from chrome` | a browser of its own, seeded with a copy of your Chrome profile's sign-ins (cookies, site storage, saved logins); it can run `--headless` while you keep working | any Chrome; same machine and user. On Windows close Chrome for the copy (it locks its cookies) |
| `state save` / `state load` | the cookies and site storage of a session, as a file, to any other machine or CI job | nothing; Playwright's `storageState` format |

The other way round, `show` puts a headless session on the person's screen
with the same tabs and sign-ins, for the step only they can do (a login, a
2FA prompt, a CAPTCHA), and `hide` takes it back; MCP clients get the same as
`browser_handoff`.

`navigera profiles` lists what can be handed over: each installed browser's
profiles (name, signed-in email), whether `--attach` reaches it right now,
and navigera's own `--profile` browsers. `--profile work` without `--from` is
a persistent browser you sign in to yourself. Either way navigera works in windows of its own, `quit` only
disconnects, and `quit --close-browser` waits until the profile is free.

**Natural mode.** `--natural` makes a headless browser look like the browser
it is: no `HeadlessChrome` in the user agent (page, workers, request
headers), and a real screen size. `navigator.webdriver` is false in every
mode, and navigera never passes `--enable-automation`. It is not a CAPTCHA
solver and fakes no fingerprint: it is a regular Chrome, run for you.
Details: [docs/navigera.md](docs/navigera.md#browsers-and-engines).

Programs can keep one browser on a pipe instead: `navigera serve` reads
one JSON command per stdin line, for example
`{"id":1,"op":"goto","url":"https://example.com"}`. The session socket speaks
the same protocol.

Full command documentation is in the [reference](reference.md); the
agent-oriented quick guide is the [skill page](skill.md).
