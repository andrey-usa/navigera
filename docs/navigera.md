# `navigera` reference

navigera drives a real Chrome over the DevTools Protocol from one native
binary. There are three ways to run it:

| mode | how | for |
|---|---|---|
| **session** | `navigera -s <name> start`, then one command per process, then `quit` | agents' shell tools, people at a terminal |
| **serve** | `navigera serve`; JSON commands on stdin, one JSON response per line | programs holding a pipe open |
| **one-shot** | `navigera <command> …` | a single action (launches and closes a browser) |

The agent-oriented guide is [the skill](../skills/navigera/SKILL.md),
also printed by `navigera skill`. This page is the full reference.

## CLI shape

```text
navigera [GLOBAL FLAGS] <command> [ARGS] [GLOBAL FLAGS]
```

| global flag | meaning |
|---|---|
| `-s, --session <name\|path>` | named session. A bare name lives in a directory only you can enter: `$XDG_RUNTIME_DIR/navigera/<name>.sock`, else `$TMPDIR/navigera-<uid>/<name>.sock` (mode 0700; navigera refuses one that is a symlink, someone else's or open to others), `%TEMP%\navigera\` on Windows; a value with a path separator is used as the path. `$NAVIGERA_SESSION` sets a default for client calls |
| `--raw` | print only the result (strings unquoted, e.g. the `ax` tree). Errors go to stderr as `error: …`, notes (dialogs, new tabs, an exception the page threw, an engine difference) as `note: …` |
| `--pretty` | pretty-print the JSON |
| `--timeout-ms <ms>` | before the command: navigation/command timeout (default 35000). After an element command: that op's element wait (default 5000) |
| `--chromium <path>` | browser binary (also `$NAVIGERA_CHROMIUM`, `$CHROME_BIN`) |
| `--engine chrome\|lightpanda` (`$NAVIGERA_ENGINE`), `--headed` | engine; visible window |
| `--idle-timeout-s <s>` | a session server shuts down after this idle time (default 1800, 0 = never) |
| `-V, --version`, `-h, --help`, `help <command>` | info |

Every command prints one JSON object: `{"id":…, "ok":true, "result":…,
"elapsed_ms":…}` or `{"ok":false, "error":"…"}`. Two optional fields report
side effects: `dialogs` (dialogs the page opened and how they were answered)
and `new_tabs` (tabs the page opened, now in the tab list). The exit code is
0 on success, 1 on a failed op and 2 on bad arguments.

**Targets.** Element commands take exactly one target:

- a ref from `ax` as the first argument (`click 12`; `@12`, `e12` and
  `ref=12` also work) or `--ref 12`;
- `--selector <css>`: the first match;
- `--text "<visible text>"`: the best match by accessible label, text or
  value. An exact match beats a partial one, actionable elements beat plain
  ones, and a `<label>` stands for its control.

- `--role <role> [--name <accessible name>]`, `--label <form label>`,
  `--placeholder <text>`, `--testid <data-testid>`: semantic locators
  (case-insensitive, exact before contains, visible first, open shadow
  roots included); `--nth <i>` picks among their matches or `--selector`'s
  (0-based, `-1` = the last).

Selector, text and locator targets auto-wait (default 5 s) for the element
to appear.

## Commands

| command | args | result |
|---|---|---|
| `start` | (with `-s`) | `{session, socket, log, pid}`; detached server + browser; `already_running` if it exists |
| `goto` | `<url>` or `--url`; `--wait load\|domcontentloaded\|commit`; `--timeout-ms` | state `{tab, tabs, url, title}`. `example.com` → `https://`, `localhost:3000` → `http://` |
| `ax` (`snapshot`) | `[<ref> \| --selector <css> \| --text t]` scope; `--limit <lines>` (2000); `--refs all`; `--format json` (old flat list); `--all` | text tree (below) |
| `click` | target | state; `synthetic_click` note if a DOM click had to be used |
| `fill` | target, `<value>` or `--value` | state |
| `type` | `<text>` or `--value`; optional `--selector/--ref/--text` to focus first | state |
| `press` | `<key>` or `--key`; optional target to focus first | state |
| `select` | target, option label(s)/value(s) (positional or repeated `--value`) | state + `selected` |
| `hover` | target | state; `synthetic_hover` note if the element could not be hovered |
| `scroll` | `--by <px>` (default 800) or `--to top\|bottom`; or a target to scroll into view (`--by` scrolls inside it) | `{scrollY, scrollHeight, viewport}` |
| `upload` | target, file path(s) (positional or `--file`) | `{files}` |
| `wait` | any of `--selector <css>` (visible), `--text <t>`, `--url <part>`, `--gone <css>`, `--js <expr>`, `--idle` (no fetch/XHR for 500 ms; also `--load networkidle`), `--ms <n>`; `--timeout-ms` | state + `waited_ms` |
| `eval` (`evaluate`) | `<js>`, `--expression <js>`, `--file <path.js>`, or `-` (read the script from stdin) | the JSON value (an arrow function is called; promises are awaited) |
| `text` | `--selector <css>` | `textContent` or `null` |
| `find` | `<description>` | `{ref, element, in, p, confidence, others, sure}`: the element a decision model chose ([below](#decisions-find-and-check)) |
| `check` | `<yes/no question>` (no target given) | `{question, yes, answer}`: the probability the answer is yes |
| `check` / `uncheck` | target (a ref or a targeting flag) | state + `checked`, `changed`: clicks a checkbox/radio/switch only when its state differs, fails if the click didn't change it |
| `get` | `text\|value\|html\|outer\|box\|checked` target; `attr <name>` / `style <prop>` target; `count <css>`; `title`; `url` | the value |
| `is` | `visible\|hidden\|enabled\|disabled\|checked\|editable\|focused` target | `true`/`false` (a missing element is not visible, after 500 ms) |
| `dblclick` / `focus` | target | state / `{focused}` |
| `drag` | `<from> <to>` (each a ref or CSS selector), or `--from-text <t> --to-text <t>` | state and `drag`: `{html5: true, dropped}` for a `draggable` source (the drag events with one DataTransfer, which headless Chrome will not start from synthetic input; `dropped` is false when the target does not accept it), else the two points a held mouse moved between (sliders, pointer-driven sortable lists) |
| `keydown` / `keyup` | `<key>` | holds / releases a key; held modifiers apply to later `press`es |
| `insert-text` | `<text>`, optional target to focus first | state: text inserted at the caret in one go (paste, IME), no key events |
| `console` | `--level error\|warn\|info\|log\|debug`, `--limit <n>` (200), `--clear`, `--json` | `[level] text  (url:line)` per line: console calls, uncaught exceptions (`[exception]`), failed loads and CSP reports |
| `errors` | `--limit`, `--clear`, `--json` | `console --level error` |
| `network` | `--filter <text>`, `--type fetch\|xhr\|document\|…`, `--failed`, `--limit <n>` (100), `--clear`; `--har <file>` | `#n METHOD STATUS type url size time` per line (same-origin URLs as paths); `--har` writes HAR 1.2 |
| `network <n>` | `--body-file <file>` | one request in full: headers, post data, response body (8 KB shown; the file gets all of it) |
| `cookies` | `[--all] [--url <u>] [--json]`; `set <name> <value> [--url <u> \| --domain <d>] [--path] [--expires <unix s>] [--http-only] [--secure] [--same-site Lax\|Strict\|None]`; `delete <name>`; `clear` | one cookie per line (`--all`: every cookie in the browser) |
| `storage` | `[local\|session] [get [<key>] \| set <key> <value> \| remove <key> \| clear]` | the value, or the whole store as an object |
| `state` | `save\|load <file.json>` | every cookie (HttpOnly too) and the open tabs' localStorage to a file, or back: `{saved\|loaded, cookies, origins}`. The file is Playwright's `storageState` format, so either tool reads the other's. `load` sets the cookies at once and gives each origin's keys to documents of that origin that lack them |
| `pdf` | `[<path>]` or `--path`; `--landscape`; `--background` | `{path, bytes}` |
| `set` | `viewport <w> <h> [--scale <dpr>] [--mobile]`, `device <name>` (iPhone 15, iPhone 15 Pro Max, iPhone SE, Pixel 8, Galaxy S24, iPad Mini, iPad Pro 11, Laptop, Desktop), `media dark\|light\|print\|reduced-motion\|forced-colors`, `geo <lat> <lon> [--accuracy <m>]`, `offline on\|off`, `headers 'Name: value'…` (or a JSON object), `ua <string>`, `locale <tag>`, `timezone <zone>`, `credentials <user> <password>`; `off` resets one | what was set. Applies to every tab and to tabs opened later |
| `route` | `<url glob>` (`*`, `?` over the whole URL), then `--abort` (default) or `--status <n> [--body <text> \| --body-file <f>] [--content-type <t>] [--header 'Name: value']…`; no argument lists the rules with their hit counts | `{route, action}`. Only matching URLs are intercepted |
| `unroute` | `[<url glob>]` | `{removed}` (all rules without a glob) |
| `init-script` | `<js>`, `--file <path.js>`, `--clear`; no argument lists them | runs before the page's own scripts in every document of every tab |
| `status` | — | `{engine, executable, endpoint, attached, version}` + state |
| `title` / `url` | — | string / state |
| `back` / `forward` / `reload` | — | state |
| `screenshot` | `[<path>]` or `--path`; `--full-page` | `{path, bytes}` |
| `tab-new` | `[<url>]` | `{tabs:[{index,target,active}], active, url}` |
| `tab-list` | — | same |
| `tab-select` | `<index>` | state |
| `tab-close` | `[<index>]` (default: active) | state |
| `dialog` | `--accept [--prompt-text t]` or `--dismiss` | `{dialogs: "accept"\|"dismiss"}` |
| `quit` (`close`) | — | `{bye: true}`; a session server and its browser exit |
| `skill` | — | prints the agent guide (SKILL.md) |
| `install-skill` | `[--dir <skills dir>] [--claude \| --continue] [--global]` | writes `<dir>/navigera/SKILL.md` (default `./.agents/skills`, read by Cline and Kilo; `--claude`: `.claude/skills`; `--continue`: `.continue/skills`; `--global`: under your home directory) |

### The `ax` tree

```text
page "Cart — Acme Supply" http://127.0.0.1:8765/cart (tab 0 of 2)
- main:
  - heading "Your cart" [level=1]
  - table:
    - row:
      - cell "Brass Sprocket"
      - cell "Qty 2":
        - spinbutton "Qty" [ref=146] value="2"
      - cell "Remove Brass Sprocket": button [ref=193]
  - textbox "Coupon code" [ref=148]
  - button "Apply coupon" [ref=212]
```

- The tree comes from one `Accessibility.getFullAXTree` call per frame.
  Same-origin and in-process iframes are spliced in under their `Iframe`
  node, and open shadow roots are included.
- Wrapper nodes (`generic`, label boxes) are collapsed, and text that repeats
  an ancestor's name or a neighbouring control's label is dropped.
- A named node with one same-named actionable child is printed on one line,
  for example `heading "X": link [ref=7]`.
- `[ref=N]` (a backend DOM node id) appears on actionable nodes: interactive
  roles and focusable elements. Use `--refs all` to get one on every node.
- States are shown in brackets: `checked`, `disabled`, `expanded` or
  `collapsed`, `selected`, `required`, `invalid`, `level=N`.
- Inputs show `value="…"`, links show `url=` (shortened to the path on the
  same origin), and `<select>` lists its `options:` inline.
- Collapsed content (closed `<details>`, menus that open on hover) is not in
  the tree until it is opened.

## Behaviour

- **Snapshots wait for fetched data.** Before `ax` and `screenshot`,
  navigera waits until the tab has had no `fetch`/XHR in flight for 500 ms
  (Playwright's `networkidle` window), at most 3 s; requests open longer
  than 5 s (long polls, streams) don't count. A page that has been quiet —
  usual when an agent has been thinking — is read at once. So
  `goto … && … ax` shows a single-page app's list, not its "Loading…"
  placeholder.
- **Clicks** are trusted mouse events at the element's centre after
  scrolling it into view. navigera checks that the press reached the page:
  Chrome occasionally acknowledges mouse events it never delivers (seen
  right after a tab switch), and then the click is sent once more, or as
  DOM events if it still doesn't arrive (`synthetic_click` says so). Same-origin iframe offsets are added in the page;
  inside a cross-origin iframe (refs only) the box comes from
  `DOM.getContentQuads`.
  - A styled checkbox or radio (the `<input>` hidden or covered by its own
    label's box) is clicked through its label, as a user would.
  - If something else covers the element (a toast, an overlay), the click
    retries for up to 3 s.
  - After that it dispatches DOM mouse events and says so
    (`synthetic_click`).
  - If the click starts a navigation, the command returns once the new
    document is parsed.
- **Page errors are reported.** An uncaught exception while a command ran
  adds one line to its `notes` (`the page threw TypeError: …; \`errors\`
  lists them`), so a click whose handler crashed doesn't look like a click
  that did nothing. `console`, `errors` and `network` keep the last 1000
  entries per tab.
- **Navigation settling:** every command that reads or acts on the page
  first waits for any navigation an earlier action started, such as a JS
  redirect, so it never reads the previous page. The wait is bounded by the
  op's own `timeout_ms` and never fails the op. Once the new document has
  committed, it waits at most 5 s for `DOMContentLoaded`: a page that keeps
  streaming is then used as it is, and the response says `"loading": true`.
- **Paths** (`upload`, `screenshot`) given to a session client are resolved
  in the caller's working directory, not the session server's.
- **New tabs:** pages opened with `target=_blank` or `window.open` are
  adopted into the tab list and become active. Tabs the page closes are
  dropped.
- **Dialogs** are answered the moment they open: accept by default, or
  dismiss after `dialog --dismiss`. A `beforeunload` dialog is always
  accepted. Each dialog is reported in the response.
- **No `Runtime.enable`:** it is the best-known automation fingerprint, and
  nothing here needs it.
- **Logs:** launch details go to stderr only with `NAVIGERA_VERBOSE=1`.

## Decisions: `find` and `check`

Two small decisions an agent otherwise spends a page snapshot and an LLM turn
on can go to a System One model instead: TypeSafe's
[Jev](https://docs.typesafe.ai), which answers typed questions (a choice
among options, a yes/no probability) in a few hundred milliseconds and bills
input tokens only. Opt-in: nothing is sent anywhere unless one of these is set
where the session (or `navigera mcp`) starts:

| variable | |
|---|---|
| `TYPESAFE_API_KEY` | TypeSafe's API, `https://api.typesafe.ai/v1/systemone` |
| `NAVIGERA_DECIDER_URL` | an endpoint with the same API (a LiteLLM proxy's `/typesafe/v1/systemone`, a mock); `http://` directly, `https://` through `curl` |
| `NAVIGERA_DECIDER_MODEL` | model id (default `jev-latest`) |
| `NAVIGERA_FIND_MIN` | probability `--find` needs before it acts (default 0.5) |

`start` reports `"decider": "<model>"` when one is configured, so an agent can
tell without looking at the variables.

- `find "add the brass valve to the cart"` returns the chosen element's ref,
  its line, the probability and the three runners-up. Every element with a
  ref is an option, described by its line and the containers around it (the
  card's heading, a dialog's name), so twelve "Add to cart" buttons are told
  apart. Large pages are split into choices of 200 and the winners asked
  again.
- Element ops take `--find "<description>"` in place of a ref (`click --find
  "accept the cookies"`, `fill --find "email field" a@b.c`): the op runs on
  the chosen element and its result carries `found`. Below `NAVIGERA_FIND_MIN`
  the op fails and lists the candidates, so a guess is never acted on.
- `check "Did the order go through?"` returns the probability of yes. The
  page goes as its text tree without refs; near 0.5 means unsure.
- A plain `ax` of a tree over 16 KB (`NAVIGERA_AX_PREVIEW_BYTES`; `0` turns
  it off) shows its first 40 lines, ends with `... N more lines not shown`,
  and notes that `--find` and `check` act on the whole page; a tree over 60
  lines but under that size is shown whole, with the note; `ax --limit <n>`, `--max-depth`, `--all` or a scope
  (`--selector`, a ref) returns what was asked for, with the note. Without a
  decider, `ax` is unchanged.

What leaves the machine: the page's tree, with what was typed into text
fields replaced by `(filled)` (password fields are masked by the browser
already). MCP clients see `browser_find`, `browser_check` and a `find` target
only when a decider is configured. `docs/multi-model.md` is the design and the
measurements.

## Serve protocol (programs)

`ax` without `format` returns the flat JSON list (`[{ref, role, name,
value?, url?}]`, `url` on links; main frame only) that serve clients have always received; send
`"format":"text"` for the indented tree the CLI prints.

```json
{"id":1,"op":"goto","url":"https://example.com"}
{"id":2,"op":"ax","format":"text"}
{"id":3,"op":"click","ref":12}
{"id":4,"op":"fill","selector":"#q","value":"widget","timeout_ms":3000}
{"id":5,"op":"select","text":"Country","value":["Canada"]}
{"id":6,"op":"wait","text":"Saved"}
{"id":7,"op":"eval","expression":"() => document.title"}
{"id":8,"op":"quit"}
```

Each op's fields are the long flag names in snake case: `selector`, `ref`,
`text`, `value`, `timeout_ms`, `wait`, `format`, `limit`, `by`, `to`,
`files`, `ms`, `js`, `accept`, `prompt_text` and so on. Malformed lines and
unknown ops return `ok:false` and leave the session running. EOF or `quit`
closes the browser. The session socket speaks exactly this protocol, one
connection at a time.

A line holding a JSON **array** of commands is a batch: each runs in order
and gets its own response line (a failure doesn't stop the rest; `quit`
does). Use it where the shell can't send newlines: PowerShell's `echo`
passes `\n` through literally, so `echo '{…}\n{…}' | navigera serve` is one
bad line, while `echo '[{…},{…}]' | navigera serve` works. A leading UTF-8
BOM (PowerShell 5 adds one) is ignored.

**Driving serve from a program.** Read one response line per command line
(a batch: one per element), match responses by `id`, and give every read a
timeout. Read or discard stderr (`stderr=DEVNULL` in Python): a full stderr
pipe stalls the server. Decode stdout as UTF-8 (`encoding="utf-8"` in
Python; Windows' default code page can't decode page text). A minimal Python
client:

```python
import json, subprocess
nv = subprocess.Popen(["navigera", "serve"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                      stderr=subprocess.DEVNULL, text=True, encoding="utf-8")
def call(op, _id=[0], **fields):
    _id[0] += 1
    nv.stdin.write(json.dumps({"id": _id[0], "op": op, **fields}) + "\n"); nv.stdin.flush()
    reply = json.loads(nv.stdout.readline())
    assert reply["id"] == _id[0], reply
    return reply
call("goto", url="https://example.com")
print(call("ax", format="text")["result"])
call("quit"); nv.wait()
```

An agent working from a shell needs none of this: use a session
(`navigera -s work start`, then one command per call). Per call, a direct
session command costs about 3 ms more than a line on a `serve` pipe (Linux,
warm session: `title` 3.3 vs 0.5 ms, `ax` 14.7 vs 11.1 ms), far less than one
model turn. A wrapper pays off only for long runs with no decision between
steps, written by a program that already exists; a script that launches its
own `serve` pays a browser cold start and loses cookies on every run.

## MCP server (IDE agents)

`navigera mcp` serves the [Model Context Protocol](https://modelcontextprotocol.io)
on stdio, for agents that take their tools over MCP: Cline, Roo Code, Kilo
Code, Continue, VS Code, Claude Desktop, Cursor. Each tool is one serve op, so
it behaves exactly like the CLI. The browser starts on the first tool call.
`--headed`, `--profile <name>` (a persistent, signed-in browser) and
`--attach` work as they do for sessions.

| tool | does |
|---|---|
| `browser_navigate` | `goto` |
| `browser_snapshot` | `ax`: the tree with `[ref=N]` (optional `ref`/`selector` scope, `limit`) |
| `browser_click`, `browser_fill`, `browser_type`, `browser_press_key`, `browser_select_option`, `browser_hover`, `browser_scroll`, `browser_file_upload` | the element ops, by `ref`, `selector` or `text` |
| `browser_wait_for`, `browser_evaluate`, `browser_screenshot` (returns the image), `browser_tabs`, `browser_history`, `browser_handle_dialog` | as the CLI |
| `browser_console_messages`, `browser_network_requests` | `console`, `network` |
| `browser_resize` | `set viewport` |
| `browser_run` | several ops in one call (`[{"op":"fill","ref":21,"value":"Ada"}, {"op":"click","ref":30}]`), stopping at the first failure, ending with a snapshot |
| `browser_close` | close the browser; the next call starts a fresh one |

Every action takes `snapshot: true` to return the updated tree in the same
call. The whole tool list (21 tools) is about 11.6 KB (~2,900 tokens), against
~20 KB for Playwright MCP 0.0.83 and ~26 KB for Chrome DevTools MCP 1.10.1.

Client configuration (all run the same command):

```jsonc
// Cline (cline_mcp_settings.json), Roo Code (.roo/mcp.json), Claude Desktop, Cursor
{ "mcpServers": { "navigera": { "command": "navigera", "args": ["mcp"] } } }

// Kilo Code (kilo.json)
{ "mcp": { "navigera": { "type": "local", "command": ["navigera", "mcp"] } } }

// VS Code (.vscode/mcp.json)
{ "servers": { "navigera": { "type": "stdio", "command": "navigera", "args": ["mcp"] } } }
```

```yaml
# Continue (config.yaml)
mcpServers:
  - name: navigera
    command: navigera
    args: [mcp]
```

From a shell: `cline mcp install navigera -- navigera mcp`, `claude mcp add navigera -- navigera mcp`.
Without a local install, use `"command": "npx", "args": ["-y", "navigera", "mcp"]`
(the npm package carries the binary). The server is published to the MCP
Registry as `io.github.andrey-usa/navigera` (`server.json`).

## Browsers and engines

- **Chrome-family (default).** navigera looks for a browser in this
  order:
  1. `--chromium`;
  2. `$NAVIGERA_CHROMIUM`, `$CHROME_BIN`;
  3. system Chrome, Chromium or Edge;
  4. Playwright and Puppeteer browser caches. For headless runs the cache
     search prefers chrome-headless-shell.

  Brave and Edge work via `--chromium`. chrome-headless-shell starts about
  3× faster than full Chrome (cold start 0.32 s → 0.11 s in bench run
  37498679296).
- **CDP transport.** `--remote-debugging-pipe` by default on Linux/macOS:
  no TCP port, and the browser exits when navigera dies. `--transport ws`
  (or `$NAVIGERA_CDP_TRANSPORT=ws`) opens a DevTools WebSocket port instead, for
  attaching other tools. Windows always uses `ws`; Chrome runs in a
  kill-on-close job object there, so it still exits with navigera.
- **Windows sessions.** The session file holds `127.0.0.1:<port> <token>`;
  the server listens on that loopback port and drops any client whose first
  line isn't the token (a web page posting to localhost can't drive it).
  `start` detaches the server from the console, the caller's job and its
  stdio handles, so the shell that ran `start` returns at once. It returns
  once the browser is up (no need to sleep after it); the first Chrome
  launch on a fresh Windows machine takes 4–6 s, later ones under 1 s.
- **A session that stopped.** A client call to a session that isn't running
  quotes the end of its server's log (`<socket>.log`, kept after the server
  exits), which says why it stopped: `idle for 1800s`, a browser crash, or
  `quit`.
- **`--engine lightpanda`** (experimental; tested on Lightpanda 1.0.1 and the
  nightly) starts `lightpanda serve` with iframes and stylesheets loaded and
  telemetry off (`LIGHTPANDA_DISABLE_TELEMETRY`). The binary comes from
  `$LIGHTPANDA_BIN` or `PATH`, or from `--chromium`; no X server is needed.
  `goto` waits for load as on Chrome. Differences from Chrome:
  - one tab: a page opened with `target=_blank` or `window.open` is loaded in
    the current tab, and the response says so in `notes`; `tab-new` fails.
  - dialogs are answered from a response armed before they open: one per
    dialog, re-armed after each, so a script that opens two dialogs in a row
    gets the policy for the first and a dismissal for the second (`dialogs`
    in the response reports what the page got).
  - no layout: clicks go straight to DOM events where the hit test can't
    work (inside shadow roots), without the settle wait and without the
    "covered" warning.
  - shadow roots: the engine's accessibility tree stops at one and gives the
    nodes inside no ids, so `ax` lists what an open shadow root holds after
    the tree, under `- shadow root of <tag>`, without refs; target those
    elements with `--text`.
  - password fields: the engine reports `*****` for every one, filled or
    not; `ax` shows a dot per character, as on Chrome, and no value for an
    empty field.
  - infinite scroll: with no layout, an `IntersectionObserver` sees every
    target as on screen, so a page that loads more as you scroll loads all
    of it (60 products where Chrome shows 12), and `ax` is that much longer.
  - `status` reports the engine of a running session; `start` on a session
    that runs the other engine fails instead of reusing it.

**Your own browser: `--attach` and `--profile`** (with `start`, or `serve`):

| | what it connects to | closes the browser? |
|---|---|---|
| `--attach` / `--attach chrome` | your running Chrome (also `beta`, `dev`, `canary`, `chromium`, `edge`): Chrome 144+ serves remote debugging for your default profile once you turn it on at `chrome://inspect/#remote-debugging`, and asks you to Allow each connection. navigera reads the endpoint from `DevToolsActivePort` in that browser's user data directory | never |
| `--attach <dir>` | a Chrome started with `--remote-debugging-port` and `--user-data-dir=<dir>` | never |
| `--attach ws://…` / `http://host:port` / `<port>` | that DevTools endpoint | never |
| `--profile <name>` | a visible Chrome on its own persistent profile (`%LOCALAPPDATA%\navigera\profiles\<name>`, `~/Library/Application Support/navigera/profiles/<name>`, `~/.local/share/navigera/profiles/<name>`; a path works too), started when it isn't running and reused when it is. Sign in once; cookies, settings and extensions stay. `--headless` hides it | only with `quit --close-browser` |

The agent gets a window of its own; your tabs are never touched, and only
popups from navigera's own tabs are adopted. `quit` disconnects and leaves
every window open. Because Chrome asks to Allow every new connection, these
modes need a session (one connection for all commands): a one-shot command
with `--attach` is refused. Chrome 136–143 can't serve its default profile
at all (the `--remote-debugging-port` switch is ignored for it, and the
inspect toggle arrives in 144): use `--profile` there, which works on any
version.

**Diagnostics:**

- `NAVIGERA_TIMINGS=1` prints launch phases at shutdown.
- `NAVIGERA_CDP_TRACE=<file>` records every CDP command sent; CI checks it with
  `tools/cdp_check.py`.
- `NAVIGERA_VERBOSE=1` turns on launch logs.

## Playwright CLI / agent-browser → navigera

| playwright-cli | agent-browser | navigera |
|---|---|---|
| `open <url>` / `goto <url>` | `open <url>` | `start` + `goto <url>` |
| `snapshot` | `snapshot` | `ax` (or `snapshot`) |
| `click e15` | `click @e2` | `click 15` (also `@e15`, `e15`) |
| `fill e3 "x"` | `fill @e3 "x"` | `fill 3 "x"` |
| `type`, `press Enter`, `select`, `hover` | same | same |
| `eval "document.title"` | `eval "document.title"` | `eval "document.title"` |
| `tab-new`, `tab-list`, `tab-select`, `tab-close` | `tab new`, `tab`, … | `tab-new`, `tab-list`, `tab-select`, `tab-close` |
| `close` | `close` | `quit` (or `close`) |
| `-s=name` | `--session name` | `-s name` / `-s=name` |
| `--raw` | `--json` (default text) | `--raw` (default JSON) |
