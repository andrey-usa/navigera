# Acting

| command | what it does |
|---|---|
| `goto <url> [--wait load\|domcontentloaded\|commit]` | navigate (`example.com` gets `https://`) |
| `click <ref>` / `--selector <css>` / `--text "<visible text>"` | real mouse click; waits for the element and for any page load it triggers |
| `click <ref> --until-gone [--max 50]` | a list behind "Load more": clicks until the button is gone or disabled, each page loaded first; says how many clicks |
| `fill <ref> "<value>"` | set an input/textarea value (replaces existing text) |
| `type "<text>" [--ref N]` | type key by key (autocomplete, key listeners) |
| `press <key> [--ref N]` | `Enter`, `Tab`, `Escape`, `ArrowDown`, `Control+a`, … |
| `select <ref> "<option>"` | pick an option by label or value; custom (react-select style) dropdowns too |
| `hover <ref>` | move the mouse over an element (hover menus) |
| `scroll [--by 800 \| --to bottom] [<ref>]` | scroll the page (infinite lists load more) or bring a ref into view; `--selector "#list"` scrolls inside a box |
| `upload <ref> <file>…` | set an `<input type=file>` |
| `wait --text "Saved" \| --selector <css> \| --url <part> \| --gone <css> \| --js "<expr>" \| --ms 500` | wait for a condition (up to 5 s, or `--wait-ms`) |
| `eval "<js>"` | run JavaScript in the page (an expression, an arrow function, or statements with `await` and `return`), result as JSON; `--file f.js` or `-` (stdin) |
| `back` / `forward` / `reload` | history |
| `screenshot [file.png] [--full-page]` | PNG of the viewport or the whole page |
| `tab-new [url]`, `tab-list`, `tab-select <i or id>`, `tab-close [<i>]` | tabs; an id from `tab-list` stays right as tabs open and close |
| `reset` | start clean in the same browser: no cookies, storage or tabs (signs you out) |
| `dialog --accept\|--dismiss [--prompt-text t]` | how later `alert`/`confirm`/`prompt` dialogs are answered |

Element commands wait up to 5 s for the element (`--timeout-ms <ms>` for
longer, or `start --wait-ms <ms>` once for a slow app). Besides a ref,
`--selector` and `--text`, they take `--role button --name "Save"`,
`--label "Email"`, `--placeholder …`, `--testid …` and `--nth <i>` (`-1` =
last). Frames are searched for you; when two frames hold a match, `--frame
"#billing"` (the iframe's CSS; `"#outer >> #inner"` for nested ones) looks
only there.

## What the tool handles for you

- **New tabs:** a `target=_blank` link or `window.open` opens a tab the
  session adopts and switches to; the response says `"new_tabs":[1]`.
  `tab-select 0` goes back.
- **Dialogs:** `alert`, `confirm` and `prompt` are accepted and reported as
  `"dialogs":[{type, message, accepted}]`. Run `dialog --dismiss` first for
  "Cancel".
- **Navigation:** a click that loads a page returns once it is ready; the
  next command never reads the old page. A page that never finishes loading
  is used as it is after about 5 s, with `"loading": true`: `wait --text …`
  for what you need.
- **Moving and covered elements:** a click waits for the element to stop
  moving and for what covers it (a toast, a sticky bar) to go. If it is still
  covered, the click fails and names the cover: close that first (a cookie
  banner needs its own click); `click --force` sends DOM events anyway.
  Styled checkboxes and radios are clicked through their label; a hidden
  input with no label fails at once and names what to click instead.
- **Clicks that do nothing:** if nothing changed after a click (DOM, focus,
  value, URL, request, dialog, tab; analytics don't count), the response
  carries `"no_effect"`. The page is often not wired up yet, or slow: `wait`
  for what the click should show, and click again only if it never comes.
  Don't assume the click worked. A click that came before a page that just
  loaded wired the element up is sent again by navigera once it does
  (`"clicked_again"`): don't click it again yourself.
- **Iframes and shadow DOM:** refs work in any iframe. `--selector`, `--text`
  and the locators also search iframes (cross-origin ones included) and open
  shadow roots when the page itself has no match. `--selector "#app >>> x-card
  >>> button"` names a path through shadow hosts (what `locate` writes for
  an icon button in a web component).
- **Files:** `upload ./doc.txt` and `screenshot shot.png` take paths relative
  to your current directory.
