# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.10.0] - 2026-11-07

### Added

- `--auto-connect` (also `--autoconnect`, and a bare `--attach`) works in
  whichever of your browsers serves remote debugging: Chrome, Chrome Beta,
  Dev and Canary, Chromium, Edge and Brave by their default profiles, then
  a browser started with `--remote-debugging-port` 9222 or 9229.
  `NAVIGERA_AUTO_CONNECT=1` does the same for every `start` and `mcp`.
  `start` says which browser it found and how many of your tabs are open.
- In your own browser `tab-list` lists your other tabs too, under
  `user_tabs`, and marks the tab you are on `current: true`: the one whose
  window has your keyboard, else the one that came on screen last.
  Windows another navigera session on the same machine opened there are
  not listed as yours.
- `tab-select current` works in the tab you are on, and `tab-select` takes
  a target id from `user_tabs`. The tab stays yours: `quit` and `reset`
  leave it open. `start --attach --tab current` starts there and opens no
  window of navigera's own. MCP: `browser_tabs` `select` takes `target`
  (an id, or `current`).
- `quit --keep-tabs` leaves navigera's own tabs open in a browser it
  attached to, the ones `reset` opened as well.

### Behaviour changes

- In a browser navigera attached to, `quit` closes the tabs navigera
  opened and leaves yours open, where it left them all. An idle or
  `--parent` shutdown does the same. The browser's last tab is never
  closed.
- navigera's window in your own browser opens behind yours, so the window
  you type in keeps the keyboard. `tab-new` there opens another such
  window, where it opened a tab in the window you used last. `tab-select`
  and `reset` there no longer bring a tab to the front: the tab you look at
  stays in front, and a tab behind it still reads, takes clicks and typing,
  and gives screenshots.
- `show` in your own browser brings the session's tab to the front for
  you and hands it to you (`quit` leaves it open), where it was refused.
- When the agent moves to one of your tabs, navigera's own window closes
  if it was never used.
- A bare `--attach` looks for any of your browsers (as `--auto-connect`),
  where it meant Chrome only; `--attach chrome` still means Chrome.

### Changed

- An action whose requests fetch data reports about 50 ms sooner. Once
  those requests are done, the wait for the page to settle ends as soon
  as the page has changed, drawn a frame and run a task with no request
  or short timer (up to 100 ms) of its own still pending; it used to wait
  out 100 ms of network quiet every time. A fetch the page chains from
  the response, at once or by such a timer, is still waited for, and one
  deferred past 100 ms still isn't, as before. In a tab that is not on
  screen (no frames there) it waits out the 100 ms as before.
- After a look, `select`, `check` and `uncheck` print only the changed
  lines when those lines already show the choice or the flipped box. They
  used to print `{"selected":["Tokyo"]}` or `{"changed":true,"checked":true}`
  above lines that said the same.

### Fixed

- In a tab that is not on screen in your own browser, an action no longer
  waits about a second before it runs. Your browser runs timers there
  about once a second, and navigera's wait for a frame before a click
  fell back to such a timer; it now waits one task of the page there.
  A click in such a tab takes about 230 ms where it took 1 s.
- `tab-list` in the default compact output printed only `{"active": n}`
  once the tab list had been printed before; it now always lists the tabs.
  MCP: `browser_tabs` `list` returned the page line without the tabs; it
  now lists them, a line each.

## [0.9.0] - 2026-11-06

### Behaviour changes

- `navigera --help` (or no arguments) now fits on one screen: 1.9 KB
  where it was 19.5 KB. It lists the commands an agent needs first, one
  line each, then the names of the rest and the flags to start with.
  `navigera help <command>` still gives a command's details, and
  `navigera help --all` the full list.
- MCP: the action tools (`browser_click`, `browser_fill`, `browser_type`,
  `browser_press_key`, `browser_select_option`, `browser_file_upload`,
  `browser_run`) return what the action changed on the page, as the
  CLI's compact output does, where they returned the full page state.
  Pass `state: true` to get the full state back. `browser_hover`,
  `browser_mouse` and `browser_scroll` still return the full state,
  because navigera doesn't track what they change.
- MCP: the default tool list has 13 tools and takes about 8 KB, down from
  16.9 KB. The 13 tools agents rarely need (hover, tabs, screenshot, file
  upload, dialog, WebMCP list and call, mouse, handoff, history, console,
  network, resize) are listed when the agent calls `browser_more`. With a
  decision model the list also has `browser_find` and `browser_check`.
- The skill is split into a 5 KB core (`SKILL.md`, 18.9 KB before) and
  eight reference files that the agent opens when it needs them: acting,
  reading, inspecting, sessions, Electron, the decision model, setup and
  troubleshooting. `install-skill` installs all of them.

### Added

- `start --allow-domains <list>` (also `$NAVIGERA_ALLOW_DOMAINS` and
  `mcp --allow-domains`) gives a session a domain allowlist. navigera
  refuses navigation and requests to any other host at the Fetch request
  stage, before any `route` rule, on every redirect hop. Entries are
  hostnames, and `*.example.com` covers the subdomains but not the domain
  itself. An empty or malformed list fails at startup. A refused
  navigation's error names the denied host and the flag, and `status`
  lists the hosts it refused. Requests are paused at the browser too, so
  a popup (`window.open`, `target=_blank`) is refused before its first
  request rather than once navigera attaches to it, and in a browser the
  session owns, a service worker's fetches are checked as well (strict
  mode). `--allow-domains-docs-only` checks top-level navigation only:
  it limits where the agent goes, and it doesn't stop data leaving the
  page. Measured limits: the policy is by hostname, not origin, and
  Chrome's Fetch doesn't intercept WebSocket handshakes, so neither mode
  stops data sent over a WebSocket, or through a service worker in a
  shared browser. `reset` keeps the policy and `reset --all` drops it.
- MCP: two tools, `browser_webmcp_list` and `browser_webmcp_call`, listed
  through `browser_more`, expose the WebMCP tools a page provides. The
  page writes those tools, so they are untrusted: calling one the page
  doesn't mark read-only needs `confirm: true`.

### Changed

- Typing into a field on a large page no longer reads the whole page to
  report. A `fill`, `type` or `insert-text` that changed nothing else the
  tree shows answers `ok` straight away (10,000 table rows: 574 to 27 ms;
  1,000 rows: 63 to 12 ms, local, Chrome 153). To know that nothing else
  changed, navigera keeps a journal in the page from the last read. It
  covers the DOM of every document and open shadow root, form values a
  script set, popovers, a script writing a style rule during the entry,
  and the style rules that depend on an element's state (`:focus`,
  `:invalid`, `:has()` and the like) in every sheet and the sheets it
  `@import`s, each checked against what it matched before the entry. When
  the journal can't rule a change out (it saw one, a style sheet comes
  from another origin, a shadow root is closed), the page is read as
  before. The report is the same either way; `NAVIGERA_CHANGE_JOURNAL=0`
  reads after every entry, to compare.
- `check`, `uncheck` and `select` on a large page skip the page read the
  same way. When the journal shows the action changed only checkboxes,
  radios or single selects, navigera rewrites just their lines as a read
  would print them (`checked`, a select's `value=` and the option marked
  `(selected)`), a radio switch and a "select all" included (10,000 table
  rows: `check` 478 to 49 ms, `select` 447 to 34 ms; 1,000 rows: 60 to
  22 ms and 46 to 9 ms; medians of 10 interleaved, local, Chrome 153).
  The page is still read as before for a select inside another control's
  label, a table cell or anything else named by its text, an option the
  line doesn't list in full, and a required radio group.
- A click on a large page reports in about the time it takes to read
  what changed, without capturing the whole page. navigera starts from
  the last read's snapshot and reads again only the subtrees the change
  journal names (10,000 table rows: a cell edited 588 to 159 ms, a row
  added 1,071 to 315 ms; 1,000 rows: 74 to 42 ms; medians of 10 and 8
  interleaved, local, Chrome 153). The journal names each element whose
  attributes, text or children changed; each control a script set; each
  element that a state-dependent rule (`:hover`, `:focus`, ...) or a rule
  reaching past its subtree (a sibling, a child's place, `:has()`) now
  styles differently; each popover, dialog and `<details>` that opened or
  closed; each element animated in what the tree reads; and each element
  that gained or lost a click listener. The page is still captured whole,
  as before, after a new style sheet, a style rule a script wrote, a
  container query, a first-letter rule, a frame or shadow root, a change
  in `<head>`, or a change bigger than an eighth of the page. The report
  is the same either way; `NAVIGERA_DELTA_READ=0` captures every time, to
  compare.
- A plain `ax` after a read or an action is patched the same way (10,000
  rows, the `ax` after a click: 652 to 58 ms; medians of 15, interleaved,
  local); `NAVIGERA_REST_PRIME=0` turns that off. Pages whose style rules
  far outnumber their elements are the exception. On a site that ships
  its whole CSS bundle, as GitHub does (26,000 to 42,000 rules), walking
  the rules for the next read costs more than the capture it saves, so a
  read leaves them to the first action, as before (github.com, first `ax`
  193 to 72 ms; medians of 5, interleaved, live). The walk also stops as
  soon as it finds more state-dependent rules than the journal can track.
- The first `ax` after a `goto` on a large page is patched too when the
  client pauses between commands, as an agent does while it reads what
  `goto` printed. During the pause the session captures the page once its
  network is quiet and keeps the snapshot, unprinted, as a read would
  (10,000 rows, first `ax` 776 to 130 ms; 1,000 rows 93 to 34 ms; medians
  of 10, interleaved, local). It does this only for a client that paused
  1 s or more before the `goto` and before most of its last three
  commands, and only for a page whose capture fits in half of the
  shortest of those pauses: Chrome can't cut a capture short, so a
  command that arrives during one waits for it. A `start` and a first
  `goto` sent together don't count against an agent's own first `goto`.
  When a page read ahead is left unread because another `goto` came
  first, reading ahead stops until an `ax` follows a `goto` again.
  `NAVIGERA_PRE_READ=0` turns it off.
- `navigera mcp` reads large pages the way the CLI does. The MCP server
  kept no change journal, so every `browser_snapshot` on a large page
  captured the whole page. Now a snapshot after an action, after another
  snapshot, or after a navigation the agent paused on is patched with
  what changed, and between calls the server reads ahead a page that a
  navigation loaded (10,000 rows: first snapshot after `browser_navigate`
  839 to 137 ms, snapshot after a click 780 to 97 ms; 1,000 rows 85 to
  30 ms; medians of 10, interleaved, client pausing 1.5 s). Tool results
  read the same as before. Between calls the server also saves the
  storage snapshot that a shared browser's crash recovery restores, which
  it never did over MCP.
- navigera's own share of building a large page's tree is smaller (10,000
  rows, from a saved snapshot: tree 85 to 51 ms, text 49 to 26 ms,
  clickable elements 34 to 20 ms). It reads the snapshot into columns
  once, indexes per-node facts by node, and uses a faster hash for maps
  keyed by node number. Most of a full read is still Chrome's capture.
- A read of a large page builds only the part of the tree it prints.
  navigera used to build the whole tree from the DOM snapshot before
  printing it, though the printer stops one line past its limit (2,000 by
  default), so on a 10,000-row table most nodes were built, named and
  indexed for nothing. Nodes are now built as the printer reaches them
  (10,000 rows, from a saved snapshot: tree and text 147 to 31 ms; a
  click's report, a cell edited 292 to 219 ms, a row added 375 to 308 ms;
  `ax` 938 to 883 ms; 1,000 rows: a click 74 to 60 ms; medians of 10
  interleaved, local, Chrome 153). The text is the same;
  `NAVIGERA_LAZY_BUILD=0` builds the whole tree first, to compare.
  Finding the clickable elements, which runs alongside, went from 37 to
  8 ms on the same snapshot (the same elements on 103 saved pages). A
  click's patch of the kept snapshot copies the subtrees it doesn't touch
  as runs of nodes: 19 to 9 ms for an edited cell.
- Actions no longer sit out fixed waits. Before saying what an action
  changed, navigera waited 50 ms for a request the action might have
  started; one turn of the page's event loop now shows whether it started
  one. A click on a link or a submit button waits for a new document only
  when the page is loading one. It used to wait up to 150 ms when a
  router or a script cancelled the navigation (single-page apps, forms a
  script submits) or the form failed validation. A click's mouse move,
  press and release go out together, with one wait for the three. What
  an action reports, and where it leaves the page, are unchanged.
- When an action puts up a placeholder ("Loading…", "Saving", a progress
  bar) for work the page starts a moment later (on a timer, the next
  frame, an effect after paint), the report waits for the page's next
  change and reads again, for up to 1.5 s. Before, the report could show
  just the placeholder, and the agent spent a turn looking again.

### Fixed

- An Electron session no longer switches to a window the app opens on its
  own right after `start`. When a second start-up window showed up late
  (on a slow machine), the first command after `start` took it as a new
  tab and made it the active one, so `ax` read that window instead of the
  main one (seen on Windows and macOS CI, about 1 run in 20). Until the
  agent acts, navigates or picks a tab, navigera now chooses among the
  app's windows as `start` does (the largest one it shows). A window the
  app opens after that still becomes the new active tab.
- `start` right after a session ended because its shared browser crashed
  a second time now makes a new session. The ending session sent its
  answer first and released its socket afterwards, so a `start` in
  between reached it on its way out, said `already_running`, and the next
  command found no session (seen on Windows, about 1 run in 30). The
  socket now goes before that answer, and a `start` that reaches a server
  leaving without an answer waits for it to go, then starts a new
  session. The same holds when the command arrived before the shared
  browser's host had dropped its connections: the host refuses requests
  to a dead browser at once, and that refusal alone ends the session.
- `ax` no longer shows content the page has hidden again behind a display
  lock: the body of a `<details>` that was opened and closed, or content
  under `content-visibility: hidden` or `hidden=until-found` after it had
  been visible. Chrome keeps the boxes of such content and its DOM
  snapshot lists them, so the tree showed text and links nobody could
  see, and a click that closed a `<details>` reported no change. Reads
  are no slower: a 10,000-row read is within 1% (interleaved, local).
- An action's report no longer lists lines that only moved across the
  line limit at the end of a cut tree. On a page longer than `ax` shows,
  opening a section above the cut reported the rows it pushed past the
  cut as removed, and closing it reported the rows it brought back as
  added.
- A click no longer removes a form `submit` or clipboard method the page
  installed while handling it. navigera removes the hooks it installs to
  watch a click only if they are still in place.
- A click no longer presses whatever took its element's place between the
  hit test and the press, such as an overlay that a mouse move or a timer
  opened just then, or an element that moves every frame. navigera keeps
  the whole press (down, up, click) from the page and presses again where
  the element is now. If it keeps missing, the click fails and says what
  it would have hit, and nothing is clicked. Before, the press clicked the
  other element and the report showed that element's change as the
  click's (3 of 40 clicks under an overlay that opened 4 to 37 ms after
  load pressed the overlay's button; 0 now). Clicks on a still page take
  as long as before.
- When the page answers a press by moving or replacing the element (a
  banner inserted above it on pointerdown, a button rendered again on
  mousedown), the click now says so in `click_missed`. The click event
  went to the parent the press and the release share, or the browser sent
  none, so the element's own handler never ran. Before, this read as a
  plain success (compact output printed `ok` for the replaced button).
  The page saw the press, so navigera doesn't press again. The check
  rides on the press-arrival check, with no extra round trip.
- A text entry on a large page reported `ok` when a script had set a
  checkbox, a radio, a select or a field's value (no DOM change, no
  event) between the `ax` and the entry, though a read showed the change.
  Form states are now recorded with the read, and if one has changed
  since, the page is read again.
- A `batch` says what its actions changed, as each action does on its
  own. With `fill` and `click` in one batch it printed only the page
  state, so the agent had to read the page again to see how the batch
  ended. Steps that changed nothing print nothing, and a batch that
  changed nothing prints `ok`. In the LLM bench the model still looks once
  more before it answers (search and form, 5 of 6 sessions each), so its
  token use didn't change (8,716 vs 8,764 per session, 6 reps, same
  window).
- `text --selector <css>` leaves out the scripts and styles inside the
  element. `text --selector body` on a page with an inline script printed
  the script's code after the text (565 characters for 78 of text on the
  bench's order form).
- A click that arrives before the page has wired up its element (a script
  that clicks right after `goto`, hydration, a handler set by a timer or
  by a script loaded after load) is sent again once the page wires it,
  where it used to report `no_effect`. On a page loaded less than 10 s
  ago, a click that changed nothing and found no click, mouse, pointer or
  touch listener on the element, its ancestors, its document or window
  (so no script of the page ran for it) waits while the page loads
  scripts or data, and up to 500 ms after the click, for a listener to
  appear, then clicks once more (`clicked_again` says how long the page
  took). A click that any listener heard is never sent again, because a
  script that recorded it may replay it later, out of sight. Goto and
  click in one batch, 3 reps each of 7 late-wiring pages (local, release,
  Chrome 153): clicked once in 0/21 runs before, 21/21 now; React 18
  `hydrateRoot` and Vue 3 SSR mounts loaded after load: clicked once. A
  handled click costs the same (40 in a batch: 16.8 vs 16.6 ms each). A
  dead button on a page that just loaded reports `no_effect` in 521 ms
  instead of 221 ms. `NAVIGERA_CLICK_AGAIN=0` turns it off.
- A wait on something only a property shows (`wait --js` on a field's
  value, focus or a checkbox, and assertions built on the same wait) ends
  when the page fires `input`, `change`, `focusin` or `focusout` for the
  change, where it used to end at the next check, up to 100 ms later. A
  wait for a field's value after a `fill` ended 97 ms after the input
  event and now ends in under 1 ms (median of 20, timed in the page,
  Chrome 156). A change a script makes without an event
  (`el.checked = true` alone) is still seen at the next check.
- A click waits for a slide that starts while it checks that the element
  holds still. A dialog that appeared during those frames and slid in
  could pass as still: a slide's first frame hasn't moved yet, and near
  the end of an ease-out slide the box moves less than a pixel a frame.
  The press then went out before the page's `transitionend` handler had
  wired the button. The click now looks again for a running animation on
  the element and its ancestors, waits for it and its end event, and
  reads again. In the sliding-modal test with six browsers at once
  (4 cores), the button was pressed before it was wired in 30 of 660 runs
  before, and in 0 of 660 now.

## [0.8.0] - 2026-11-03

### Behaviour changes

- An action says what it changed on the page, so an agent needs no look
  after it. After an `ax`, a click, fill, press, select, check, drag or
  upload that stays on the same document prints the lines of that tree
  that are new (`+`), gone (`-`) or read differently (`~`), with refs,
  read again the way that `ax` read it and after the data the action
  fetched has arrived (at most 40 lines, then a count). `ok` now means
  nothing visible changed, except that after a fill a line that newly
  shows the text just typed (the field itself, a live preview) is left
  out; before, a click that loaded results printed `ok` while the page
  was still fetching them. Compact output only:
  `--output json`, `serve` and the MCP server are unchanged.
- The tree is shorter on lists: a list item's bullet is left out and its
  text goes on its line (`- listitem: Blue Lamp`, one line where there
  were three), and a list item carries no `level` (its indentation is
  its level). Numbered markers (`1.`) stay.

### Changed

- MCP: seven tool descriptions say when to use the tool and what it
  costs (`browser_snapshot`, `browser_type`, `browser_press_key`,
  `browser_wait_for`, `browser_file_upload`, `browser_run`,
  `browser_resize`), and `glama.json` names the maintainer for the Glama
  listing.

### Fixed

- npm: `npm install -g navigera` and `npx navigera` failed on Linux and
  macOS from 0.6.5 to 0.7.1 (EACCES): the platform packages shipped the
  binary without its executable bit. The release now refuses a package
  whose binary isn't executable, and the launcher repairs one that lost
  the bit.
- A snapshot after a "Load more" waits for the page it loads again. Three
  requests to one path at a steady pace counted as polling and were not
  waited for, and an agent clicking "Load more" every few seconds is that
  pace: from the third page on, `ax` showed the "Loading…" button. A
  request that starts within a second of an action is now the action's
  own.
- The static Linux binaries (what `install.sh`, npm, pip and Homebrew
  install) peaked at 19 MB in a scripted session, against 6 MB for the
  other builds, since 0.7.1: mimalloc committed its whole arena up front
  and transparent huge pages backed it with 2 MiB pages. On a machine whose
  memory is fragmented, the first snapshot of a large page also took three
  times as long. It now commits on demand: 6 MB. Warm snapshots of very
  large pages (2,000 rows, 13,000 elements) take 3-6% longer than with
  0.7.1's build (timing gate, median of 10).

## [0.7.1] - 2026-11-02

### Behaviour changes

What a script or an agent relying on the 0.7.0 output or timing should
know:

- **The CLI prints compact output by default**, not one JSON line per
  command: an action that left the page as it was prints `ok`, one that
  moved it only its own fields and what changed, a read only its value;
  errors go to stderr as `error: …` (exit code 1), notes as `note: …`. A
  script that parses the JSON line adds `--output json`, or sets
  `NAVIGERA_OUTPUT=json` for the whole shell. `serve`, MCP and the library
  are unchanged.
- `ax` may add a line under the page line when it cut text
  (`… N texts cut short here …`), and prints long texts in full when the
  whole snapshot stays under 32,000 characters.
- A snapshot right after an action whose requests have started waits for
  them and 100 ms, not 500 ms of network quiet.
- An op on a ref whose element was redrawn now acts on the redrawn element
  (with a note) instead of failing, when that element is certain.
- More notes: another process using the session, a redrawn ref, an
  Electron window still starting.
- An icon-only control that printed as a bare `button [ref=N]` may now
  carry `icon="svg"`, `img="inline"` or an image's file name.
- `text` with no selector is the page's whole text (it was an error).
- The browsers navigera starts have WebMCP on (`--enable-features=WebMCP`),
  and the `--enable-features` / `--disable-features` lists from
  `NAVIGERA_EXTRA_FLAGS` are merged into navigera's own instead of replacing
  them (Chrome keeps only the last of each flag).
- `network --har` includes the response bodies the browser still holds,
  so the file is larger.
- A command sent just after the shared browser died is run on a relaunched
  browser instead of failing as in flight: the host watches the browser's
  process (its pipe can outlive it while the browser's children hold it
  open) and refuses later requests as never reached, and the session runs
  those again. A command that may have reached the browser is still never
  repeated. Killed browser, command 30 ms later: 20 of 20 recovered (was 0
  of 10); 100 ms later: 10 of 10 (was 6 of 10).
- A session on the shared browser no longer ends when that browser dies: its
  next command relaunches or joins one, reopens its tabs and carries a note
  (and a `recovered` field) naming what was lost. A command running at the
  crash still fails, and is not retried.

### Added

- The agent skill (`navigera skill`, `install-skill`) starts with the
  defaults an agent should take: keep the compact output, known steps in
  one `batch`, narrow reads, a new look only when the outcome is uncertain,
  page content as data.
- `webmcp-list` and `webmcp-call <name> [--params '{json}'] [--confirm]`:
  the tools a page registers with WebMCP (`document.modelContext`), listed
  and run through Chrome's WebMCP DevTools domain in the frame that
  registered them. Every answer says the tools are the page's and
  untrusted; a tool not marked read-only needs `--confirm`. MCP clients
  run them as `browser_run` steps. Chrome 153 and later, https or
  localhost pages.
- `set device` knows 55 devices (recent iPhones, Pixels, Galaxy phones and
  tablets, iPads, two desktop sizes).
- `set permissions grant|deny|reset <name>…` sets geolocation,
  notifications, camera, microphone, clipboard, midi, sensors and
  background-sync for the session, so a page sees them as granted, denied
  or still to be asked.
- `--output compact|raw|json` (and `NAVIGERA_OUTPUT` for a whole shell);
  compact is the default, `--compact` and `--raw` name the first two. Ten
  steps of a form, a table read and a heading: 827 characters of output
  compact, 2,756 raw, 3,246 as JSON, no slower (fill median 8.9 vs 10.0
  ms, interleaved).
- `batch [<file> | -] [--keep-going]` runs several steps in one call over
  one connection: one command per line, written as on the command line, or
  JSON ops. Every line is parsed before anything runs; the first failure
  stops the rest. It prints one response: what the steps read, the page
  state once, notes tagged with their step. A six-step form flow is one
  call instead of six (590 characters of output instead of 1,764; 62 with
  `--raw`).
- `route --har <file>` answers requests from a recorded HAR: the first
  unused entry with the request's method and URL, then the last one again
  (a page that polls keeps getting the final answer). Requests the HAR has
  no entry for fail, or reach the network with `--not-found continue`. A
  glob before `--har` limits it to those URLs.
- `route <glob> --modify` lets the request reach the server and changes
  its response: `--set-header 'Name: value'` (`'Name:'` removes one) and
  `--replace <from> <to>` in the body, repeatable.
- `screenshot --timeout-ms <n>` bounds the capture (default: the session
  timeout).
- `start --electron <app>` drives an Electron desktop app: navigera
  launches it with a DevTools port, waits for its first window and adopts
  its windows as tabs, the ones it opens later included (they have no
  opener, unlike a popup). `<app>` is the app's directory or main script
  (run with the Electron in its `node_modules`), a packaged app's
  executable, a macOS `.app` bundle or a command on `PATH`;
  `--electron-arg <arg>` passes arguments to the app. `quit` ends the app.
  `tab-new`, `reset`, `show` and `hide` say why they don't apply. `navigera
  mcp --electron <app>` gives an IDE agent the same.
- `start --electron` waits for a window the app shows to have something to
  read or use, starts on the largest window that isn't hidden (an app's
  hidden helper windows are tabs, not the active one, also after a window
  closes), and reports `electron: {version, main_process}`. An `ax` of a
  window that still shows nothing says the app may be starting. Found on
  real apps: one with six windows started on a hidden dropdown; others
  read an empty shell for up to 20 s. An app that sets its own user agent
  gets its version from its main process, or null when that is out of
  reach. A single-instance app that exits because a copy of it is already
  running fails `start` saying that may be why.
- `start --attach <port|ws://…>` on an Electron app that is already
  running with a DevTools port drives it as `start --electron` does: its
  windows are the tabs (new ones too, though no page opened them) and the
  session starts in the largest one it shows, instead of opening a blank
  tab the app has no use for. `quit` leaves the app running; `quit
  --close-browser` ends it.
- `tab-list` (and `tab-new`, `reset`) give each tab's `url` and `title`,
  so a tab can be chosen without selecting each one to look: an Electron
  app's windows, a popup, the tabs of an attached browser.
- An Electron app's `<webview>`s are tabs: `tab-list` marks them
  `"webview": true`, `tab-select` goes into one to read and act in it, and
  the session never moves into one by itself (it is part of a window).
- `eval --main <js>` in an Electron session runs JavaScript in the app's
  main process (Node's inspector, `--inspect=0` at launch): an expression,
  or a function called with the `electron` module, awaited. MCP's
  `browser_evaluate` takes `main: true`.
- CI runs an Electron end-to-end test on Linux, macOS and Windows: a
  fixture app that saves a form through its main process and opens a
  second window.
- `screenshot --quality <1-100>` writes a JPEG at that quality instead of a PNG (a
  default `shot-<time>.jpg` name), for traces and reports that keep many.

### Changed

- If the shared browser crashes or is killed, each session's next command
  relaunches it (one relaunch when several recover at once, under the host's
  lock) instead of ending the session. The session's tabs reopen at their last
  URLs (a tab whose last load was a form POST reopens with GET, and the note
  says so), the cookies and localStorage come back from a snapshot kept in
  memory (at most 500 ms behind, retaken when the session is idle), `set`
  emulation, init scripts and the download directory are re-applied, and the
  note and the `recovered` field list what could not come back (history,
  scroll, typed text, sessionStorage, refs, logs). A second crash within 10 s
  of a recovery ends the session, as before.

- Two processes on one session name are told: a command from another
  process than the session's last one notes it, and the first process's
  next command says its tab was moved, from where to where (before, only a
  command that arrived while another ran got a note, and the process whose
  page was replaced learned nothing). On Linux the process is the agent,
  not the shell it runs each command in; `NAVIGERA_CALLER` names it
  anywhere.
- A ref outlives a re-render of its element: an op whose element the page
  drew again (a list redrawn, a button replaced after a state change) is
  done on the same element, found by the role, name and place the snapshot
  printed, and the note says so (`ref 17 was re-rendered: this acted on
  button "Save", now ref 48`). When that is not certain (another element of
  that role and name, another URL, a navigation) the op fails as before.
  Remembering what the refs were costs about 5 ms on a 2,000-row snapshot.
- Long text in snapshots: a snapshot that cut anything says so under its
  first line, with how many texts and characters and how to read them
  (`… 150 texts cut short here (11,553 chars left out): …`). When the
  whole snapshot, every text in full, stays under 32,000 characters,
  nothing is cut. `ax --full-text` (MCP `browser_snapshot` `full_text`)
  prints every word, and `text` with no selector prints the page's whole
  text as it reads. A 1,500-clause agreement: every clause in the default
  snapshot, the 150 long ones cut and counted on the second line; all
  172,000 characters with `--full-text` or `text`, in about 45 ms.
- A snapshot right after an action whose own requests have started (a
  click that fetches) waits for those requests and 100 ms more, not the
  whole 500 ms quiet window: a click that loads data for 200 ms is read
  about 300 ms later instead of about 750, with the data in it. Traffic
  that hasn't started yet (a page's data after it loads) still gets the
  whole window, and a window set with `set network-quiet` holds after
  actions too.
- Snapshots of large pages cost well under half as much on Linux: the
  static Linux build allocates with mimalloc (musl's allocator was several
  times slower on the small allocations a snapshot makes), the snapshot is
  no longer copied while the tree is built and printed, and the tree built
  from the DOM snapshot is made of typed nodes instead of a JSON map per
  node, built while the clickable elements are read from the same
  snapshot. 13,000 nested divs: 545 to 191 ms (Playwright's ariaSnapshot:
  159 ms on that machine); a header over a 2,000-row table: 563 to 193 ms
  (static Linux build, median of 32, interleaved, warm). Most of what
  remains is Chrome's capture. macOS and Windows gain all but the
  allocator.
- Pages of 800 to 3,000 DOM nodes are read from the DOM snapshot too, not
  Chrome's accessibility tree: about five times faster (a 1,500-element
  page: 166-201 ms to 29-40 ms), the same text on 54 of 56 fixture and
  benchmark pages. Smaller pages keep Chrome's tree. A `<canvas>` has
  Chrome's `Canvas` role in the snapshot tree too.

### Fixed

- `start --electron` could start on an app's hidden helper window: a
  window made with `show: false` says it is visible on its first load
  (Electron's paintWhenInitiallyHidden), a shown one under Xvfb says it is
  hidden, and one whose page could not answer yet counted as shown. The
  main process now says which windows are visible (without it, the largest
  window is taken), a window that can't answer yet counts as unknown, and
  the choice waits until the app has stopped opening windows for a moment.

- `screenshot --no-wait` right after a click that confirm()s, posts and
  reloads the page could go unanswered for the whole 35 s timeout: Chrome
  loses a capture asked for while the document is being replaced, and the
  next one works at once. With no answer after 400 ms the capture is asked
  for again and the first answer is taken: about 0.43 s there, and a
  capture that is only slow still answers in its own time.
- An icon-only button with nothing to name it by now says what kind of icon
  it is: an inline style's background image (`img="bell.svg"`), an icon
  component's test id (`icon="DeleteIcon"`), and, failing all,
  `icon="svg"` or `img="inline"` (a `data:` image), so it no longer reads
  as a bare `button [ref=N]`.
- A scoped `ax` (`--selector`, a ref) on a large page builds only the
  scope's tree again, instead of the whole page's and then cutting it:
  a header on a 2,000-row page 336 -> 166 ms, on 4,000 rows 605 -> 360 ms
  (0.6.5: about 214 ms on 2,000 rows). A large scope stays on the fast
  path (the 2,000-row table itself: 299 ms; Chrome's tree takes 1.4 s).

## [0.7.0] - 2026-11-01

### Changed

- `ax` on a large page (over 3,000 DOM nodes) builds the tree from the DOM
  snapshot it already takes for clickable elements, instead of reading
  Chrome's accessibility tree, which costs about a millisecond per table
  row. The output is the same (roles, names, states, values, refs): on 22
  fixture pages and on Wikipedia, Python's and MDN's docs, GitHub, BBC,
  httpbin and four QA practice sites the two trees print the same but for a
  handful of lines (a time input's inner fields, very long cell names that
  Chrome cuts). Table of 1,000 / 2,000 / 4,000 rows, whole tree: 730 /
  1,789 / 4,671 ms before, 155 / 325 / 681 ms now (Playwright's
  `ariaSnapshot`: 238 / 476 / 975 ms); with the default limit 149 / 317 /
  604 ms. `NAVIGERA_AX_SOURCE=chrome|dom` picks the source.
- Adjacent text in a snapshot shares lines of up to 300 characters instead
  of taking a line per element: a page of 3,000 `<div>` cells went from
  3,001 lines (39 KB) to 51 lines (15 KB). A control or any other element
  between texts still starts a new line, and no text is cut to pack a line.
- A text, name or value cut in a snapshot now says how much it left out,
  and a cut text says which ref reads all of it:
  `… (+1979 chars: get text 8)`. A name's cut is noted outside its quotes.
- A control with no name or text (an icon button) shows what the page
  marks it with next to its `#id` or `.class`: a tooltip attribute
  (`data-tooltip`, `title`, …), its icon (an icon font's class, an SVG
  sprite's symbol, `data-icon`), its image's file name, or the
  `aria-describedby` text: `button .icon-btn icon="fa-trash" [ref=7]`.
  Two icon buttons of one class no longer read the same.
- `quit --close-browser` on a session in the shared browser closes the
  browser with it when no other session uses it, and otherwise says how
  many do. It used to leave the browser to close after its grace period.

### Fixed

- A click on an element that never stops moving (a perpetual drift) took
  the whole element wait, 5 s, before clicking where it last was; it now
  waits a second for it to hold still (about 1 s on such a button).
- A click on an element under a cover that stays put with controls of its
  own (a cookie banner, a dialog) fails after about a second instead of
  5 s, naming the cover and its controls:
  `covered by div#consent "We use cookies. Accept Reject" … Close it with
  one of its controls ("Accept", "Reject")`. While the page still fetches,
  a cover is waited for as before: a loading overlay with a Cancel button
  goes when its data arrives, and the click then lands.
- A session under a session directory with a long path (a deep
  `$XDG_RUNTIME_DIR` or `$TMPDIR`) failed to start: the shared browser's
  socket path did not fit a socket address. Such a session now takes a
  browser of its own, and its log says why.
- A long poll (a chat or notification request held open, reconnected when
  it ends) no longer holds snapshots for up to 3 s each cycle: a fetch/XHR
  open past 1 s stops holding a snapshot, and its URL's later requests
  never hold one. On a page holding a 20 s long poll: 3,022 / 1,976 / 7 ms
  for three snapshots in 0.6.4, 1,002 / 7 / 7 now.
- Analytics beacons at irregular intervals no longer hold the first
  snapshot: a request the server answers 202 or 204 (no content) is
  background traffic, and that method and URL's requests, the ones still
  out included, don't count, in whatever order their events arrive (first
  snapshot on such a page: 505 to 8 ms). The end of a long poll no longer
  restarts the quiet window either.
- `ax` on a large page through the shared browser lost up to a quarter of
  its speed in 0.6.5: each of the snapshot's thousands of tree reads crossed
  four threads of the browser's host. The host now routes a message on the
  thread that reads it, and renumbers and routes answers and events by
  their envelope alone, without parsing them. A 2,000-row table: 1,563 to
  about 1,400 ms (0.6.4 with a browser of its own: about 1,265).
- `ax` with the default limit reads a page the cheaper way for that page.
  The read that stops at the limit, node by node, now gives up for one full
  read when it projects more node reads than a quarter of the page's DOM
  nodes, as on markup where every printed line sits under many wrappers. A
  2,000-row list nested in divs: 5.1 s in 0.6.4, 3.1 s now; a 1,000-row
  table is read in one call (about 900 ms); 2,000- and 4,000-row tables keep
  stopping at the limit. A snapshot that gave up reads the page's frames and
  clickable elements once, not twice.

## [0.6.5] - 2026-10-31

### Added

- `drag --to-at <x>,<y>` drops at that point of the target (CSS pixels from
  its top-left corner; default its center), `--from-at` presses at that
  point of the source, as Playwright's `targetPosition`/`sourcePosition`:
  a drop near a list row's top edge lands before it.
- Named sessions share one browser, each in a browser context of its own
  (tabs, cookies, storage, cache, downloads, permissions apart), so N
  parallel agents cost one browser plus a renderer each instead of N
  browsers. A small host process keeps the browser on the private pipe (no
  DevTools port), gives each session a browser-level CDP session of its own,
  disposes a session's contexts when it goes (killed or not) and closes the
  browser 30 s after the last session (`NAVIGERA_SHARED_GRACE_S`). `navigera
  mcp` shares the same way. `start --own-browser` (or
  `NAVIGERA_SHARED_BROWSER=0`) keeps a browser per session; `--attach`,
  `--profile` and Lightpanda keep theirs. `status` says `browser: shared (N
  sessions)` or `own`; if the shared browser dies, each session's next
  command says so and `start` relaunches it.
- `navigera sessions` lists the named sessions (pid, age, idle time,
  browser, URL, memory) and the shared browsers; `sessions --clean` quits the
  ones idle 5 min or more (`--idle <s>`), removes dead socket files and closes
  a shared browser left unused, and says how much memory that freed;
  `--clean --all` quits every session.
- `start --parent <pid>` (or `NAVIGERA_PARENT_PID`): the session quits when
  that process exits, so a crashed agent leaves no browser behind.
- A command that came in while another client's command was running on the
  same session gets a note: a session name is shared by every process that
  uses it.

### Changed

- An unused session ends after 10 min, not 30 (`start --idle <s>`, or
  `--idle-timeout-s`). `status` and `sessions` don't count as use.
- Chrome starts with `--disable-field-trial-config`, as Playwright's and
  Puppeteer's do: Chromium and Chrome for Testing builds no longer turn on
  the experiments of Chrome's field-trial testing config, which cost about
  19 MiB per page (8 sessions: 894 to 716 MiB) and differed from build to
  build.
- A ref whose element has left the page fails at once ("that ref is stale ... run `ax` for fresh
  refs", no `--force` hint) instead of after the 5 s wait for a box. An element with
  `draggable="true"` gets a ref in `ax` even with no handler on it.
- The decision model's key is `NAVIGERA_DECIDER_KEY`, one key for whichever
  model `NAVIGERA_DECIDER_MODEL` names (Jev, Kev, ...). `TYPESAFE_API_KEY`
  is still read when it is unset.
- `ax` on a page whose tree is about as long as the output limit or shorter
  reads it in one call instead of node by node (a 250-row table: 342 to
  163 ms), and no longer copies the reply. A page with more than 4 DOM nodes
  per line of limit still reads lazily, stopping at the limit.
- A snapshot right after a `wait` that succeeded (text, selector, url,
  gone, js, idle) skips the network-quiet wait when no request has started
  since (`ax` after `wait --text`: 561 to 56 ms). A snapshot that did wait
  says so (`waited 412 ms for network quiet`). `set network-quiet <ms>`
  sets the quiet window per session (default 500 ms; 250 ms reads a list
  that arrives in two fetches 400 ms apart too early), and `wait --idle`
  uses it.

### Fixed

- `drag` to a target that has to scroll into view drags the source. The
  target used to be scrolled into view before the press, which moved the
  source away: the press landed on whatever had scrolled under it (often
  the target itself), so the wrong element was dragged or no drag started.
  As in Playwright, the source is now pressed and the drag started first,
  and the target is scrolled into view while it is held.
- A snapshot cut by `--limit` that was read in one call said "1 more lines"
  whatever its length; it now says "more lines", as the lazy read does.
- `start --profile` opens one window, not two: the profile browser starts
  with `--no-startup-window`, as navigera's own launches do, so there is no
  blank startup window and no "unsupported command-line flag" bar.

## [0.6.4] - 2026-10-31

Fixes found by running a real app's end-to-end tests on navigera: snapshots
no longer stall on pages that poll, drag and drop runs a real browser drag,
clicks wait for a moving element to settle, and headless Chrome reports a
mouse, as Playwright's does.

### Added

- `ax`, `screenshot`, `find`, `check <question>` and `--find` take
  `--no-wait` to skip the wait for fetched data; their help now describes
  the wait.
- `fill --force` skips the new field checks (below).

### Changed

- The wait for fetched data before a snapshot ignores polls: a fetch/XHR to
  a URL (query string aside) already requested twice or more at a steady
  interval no longer counts as pending. `ax` on a page polling every 300 ms:
  3011 ms -> 2 ms; a list that arrives from one held fetch is still waited
  for. Requests are forgotten when a new document starts.
- `drag` runs a real browser drag when the page starts one
  (`Input.setInterceptDrags`, then `dragEnter`/`dragOver` along the path and
  a drop at the target), so HTML5 drag-and-drop lists and boards accept it.
  Without a drag the held mouse is released at the target as before. The
  result gains `"path": "drag"`, `"mouse"` or `"synthetic"`. An engine that
  never reports a drag (Lightpanda) gives a `draggable` source the
  page-side drag.
- `click` and `dblclick` wait until the target's box is the same in two
  reads with a frame drawn between them before pressing, so an element
  that is still moving without a CSS animation (a deferred commit, a
  script-driven slide) is clicked where it settles, not where it started.
  `--force` skips the wait; a target that never stops is clicked when the
  settle budget ends.
- `fill` and `fill --keys` wait, within the command timeout, until the field
  is visible, enabled and editable, and the timeout error names the check
  that failed. Before, a hidden or disabled field was filled without a
  word. `--force` keeps the old behaviour.
- `check` re-reads the state every 50 ms until it matches or the element
  budget runs out, so a controlled input that commits its state after the
  click returns (React `onChange`) is no longer reported as a failed click.
- Waits end at the page's change instead of a frame later: on a page that
  keeps changing, frames lag; on a page writing to the DOM every 4 ms, a wait
  for `enabled`, a URL or text returns 3-7 ms sooner (median). Wakes are at least 8 ms apart, so a page
  that changes constantly is not re-checked on every write.
- Headless launches report a mouse (`hover: hover`, `pointer: fine`), as
  Playwright's do. Before, an app that turns its mouse handlers on only for
  a hover-capable pointer treated headless Chrome as a touch screen. A touch
  device from `set device` still reports touch.
- `ax` asks Chrome for less layout data for its clickable-element marks:
  2,000 rows with `--limit 50`: 618 -> 378 ms. The output is unchanged.

- `wait --frame` is woken by the frame's changes too, as the other waits
  are, not only looked at every 100 ms. An element added to a same-origin
  frame 100-400 ms after the wait starts: seen 4 ms after it appears, not
  65 (`--selector`, median; `--text` 42 -> 4). A frame of another origin is
  out of the observer's reach and is still looked at every 100 ms.
- `tab-close` closes a tab the way a user does, with `Page.close` on the
  page's own session, so the page runs its unload path (`beforeunload`,
  `pagehide`) and a tab that hands work to another tab from it is heard at
  once. `Target.closeTarget` skipped `beforeunload`. It stays as the
  fallback when `Page.close` is refused or the tab is still there after 1.5 s.

### Fixed

- `set device Desktop` and `set device Laptop` failed after setting the
  viewport on Chrome 155, which refuses to turn touch off with a touch-point
  count of 0. A desktop device now turns touch off as `set device off` does.

## [0.6.3] - 2026-10-30

Refs on what a page made clickable: a `<div>` row with a click handler, a card
with `cursor: pointer`, an icon `<span>`, which agents had no ref for and dug
through the DOM with `eval` to reach. Waits woken by the page's changes
instead of a timer, `ax` and the locators linear on long pages, and errors
that say what happened and what to do next.

### Security

- A file navigera writes where you name it (`network --body-file`, `--har`,
  `screenshot`, `pdf`, `state save`, `install-skill`) no longer follows a
  link already at that path: a link planted there (in a cloned project, or
  in a shared directory) is replaced by the file instead of having its
  target overwritten.

### Added

- `ax` gives a ref to what a user could click that the accessibility tree
  doesn't call actionable: elements with a click, mousedown or mouseup
  handler of their own and the outermost element of a `cursor: pointer`
  area. A `<div>` row or card, or an icon `<span>`, prints as
  `clickable [ref=N]: <text>`; an actionable node with no name or text
  shows its `#id` or `.class`, so two blank buttons can be told apart. A
  list or table listening for its items' clicks gives each item or row the
  ref (one ref on the list would click whichever item is in the middle).
  Only what a user's click can land on and tell apart is marked: not an
  empty box, `pointer-events: none`, something fully transparent (an ad's
  click guard), `visibility: hidden` (a control shown on hover), under
  `aria-hidden` or `inert`, inside another site's frame, inside a link or
  button (including `role=button` and `role=link`; they have the ref and
  get the click), or with no name, text, `#id` or `.class` (a hidden
  menu's 287 blank tiles on one shop's home page). A label whose control
  is hidden is what a user clicks, so it gets the ref. One `DOMSnapshot`
  call, Chrome only.
- The skill says what a `clickable` line is and to click it by ref
  before reaching for `eval`.
- `eval` runs statements too, with `await` and `return` at the top level
  (`const r = await fetch('/api/cart'); r.status`, or a body that ends in
  `return`): what was one expression or an arrow function before; a probe
  written either way works the first time.

### Changed

- `--help` and `help <command>` fit 80 columns, list `-s`, `--wait-ms` and
  `--version`, and show `check` once with both of its forms. A usage error
  prints its usage once; an unknown command is matched only when it is
  close (`goot` → `goto`, not `bogus` → `focus`).
- Errors say what happened and what to do next, in plain words:
  - a failed `goto` says what Chrome's code means (the host name did not
    resolve, nothing answered on that port, the server answered 404 with no
    page and the tab now shows Chrome's error page; chrome-headless-shell
    shows a blank page there instead, and `goto` succeeds) and keeps the
    code at the end;
  - a click on a word that is not a ref (`click abc`) says it was read as
    CSS and that refs are the numbers in `ax`; `ref12` now reads as ref 12;
  - a stale ref, a covered or hidden element, and a browser that went away
    are described without DevTools terms (no "remote object", "CDP command",
    "DOM events");
  - `wait` timeouts name `--timeout-ms`, `tab-select` says which tab numbers
    there are, `set viewport` says what was missing;
  - flags that can't go together (`--text` with `--nth`, a ref with
    `--selector`, `--near` with no locator) and a `set` of an unknown
    setting are refused when the command is read, with exit 2 and before
    any browser; `--nth` with `--text` says how to count instead;
  - a missing session says to start it the way navigera was called
    (`primavera navigera -s <name> start` from inside primavera);
  - a selector that isn't CSS at all (`35.75`, `Total:`) is said at once,
    with `--text` to use instead, not as the browser's `querySelector`
    error after the wait;
  - the op is named once (`click selector "abc": …`, not
    `click selector "abc": click: …`), and the `(os error 2)` suffix is gone.
- Every MCP tool parameter has a description.
- `help <command>` explains each flag in a line and shows examples that run
  as shown (each was run against a public practice site). `help status`
  names each field of what `status` prints, and `help ax` says a target (a
  ref or any locator) scopes the tree to that element.

- Waits see a change within a frame of it. The `wait` command checked every
  100 ms and the element waits under `click`, `fill`, `get` and the
  locators every 50 ms (100 for `--text`), so they answered 25-50 ms late
  on average; the page's changes now wake them (a MutationObserver), and
  they still look at the old interval for what no observer sees (a
  stylesheet rule starting to match, a hover, a background tab). An element
  added 100-400 ms after the wait starts: seen 6 ms after it appears, not
  58 (`wait --selector`, median), 5 not 34 (`--role --name`).

- `click` waits for an element that is there but not shown yet, as long as
  the element wait, as it already did for one under a closing overlay or
  disabled: a dialog's button shown by a class after a second, a control
  made visible (`visibility`) or clickable (`pointer-events`), one sliding
  in from off-screen. It failed after 300 ms with "it has no box on
  screen". An input hidden on purpose behind its label or card (a styled
  radio) still fails at once, naming what to click instead.

### Fixed

- `click` on a table row whose middle falls between two cells clicks a
  cell of the row, instead of refusing the row as hidden behind its table.
- `ax` read the page's whole accessibility tree to print at most its
  limit, and Chrome's full-tree call grows faster than the page: 8000 list
  items took 13 s and 32000 timed out. The tree is now fetched from the
  root down, children on demand, and the walk stops at the limit (2000
  items 1.24 s -> 0.78 s, 8000 13.0 s -> 2.3 s, 32000 6.8 s, on Chromium
  141). Output under the limit is byte-identical; over it, the last line
  says more lines are not shown, without a count the walk no longer
  learns. `--limit 0`, `--format json` and `--max-depth` read the whole
  tree as before, and a browser without the calls falls back to it.
- A locator by role and name (`--role button --name …`), by label, or by
  text read every candidate's `el.labels`, which scans the whole document
  on its first read for each element: quadratic. On a page of 8000
  buttons the first such call took 4.4 s (12000: about 9 s); it now takes
  under 50 ms, from one pass over the page's labels.
- `wait --text … --wait-ms <ms>` waits that long; `--wait-ms` was accepted
  and ignored on `wait`, which then waited its default 5 s.
- `click --force` on a covered element said the element had "no visible
  box"; the result now says the click was forced.
- A bare target with `--nth`, `--near` or `--frame` (`click .row --nth 2`)
  was refused as an unexpected argument; it is the target.
- `set offline` failed on Chrome before the rule-based network calls
  (Chromium 141: `'Network.emulateNetworkConditionsByRule' wasn't
  found`). Such a browser now gets `Network.emulateNetworkConditions`,
  which also tells the page it is offline; newer browsers are unchanged.

## [0.6.2] - 2026-10-26

A security fix to 0.6.1's MCP file limits: a link inside the working
directory no longer leads a file out of it. The npm platform packages move
to the `@navigera` scope; `npm install navigera` is unchanged.

### Changed

- npm: the platform packages are `@navigera/<os>-<cpu>` (`@navigera/linux-x64`
  and so on); npm refused the unscoped names. Install as before:
  `npm install -g navigera` or `npx navigera`.

### Security

- MCP: a file a `browser_run` step names could still land outside the
  working directory through a link inside it: `link/../x`, with `link`
  pointing elsewhere, was checked as `x` in the working directory and
  written beside the link's target (`state save`, `pdf`, `downloads --dir`;
  0.6.1). Each part of the path is now followed to where it leads before
  the next `..`, as the system does. Found by red-teaming the 0.6.1 fix.

## [0.6.1] - 2026-10-26

A security fix for the MCP server: every file a `browser_run` step names
stays inside the working directory. Also `install-browser`, which fetches
Chrome for Testing into navigera's own cache; `locate` and `history`, which
`primavera from-session` builds tests from; and numbers from the page that
keep their exact value.

### Added

- `install-browser [chrome-headless-shell | chrome] [--channel stable|beta|dev|canary]`:
  downloads Chrome from Google's Chrome for Testing into navigera's own cache
  (`--where` prints it; `$NAVIGERA_BROWSERS` moves it), each file checked
  against the archive's checksum. Headless sessions use an installed
  chrome-headless-shell first (about 3x faster to start); a Chrome installed
  this way is the fallback where the system has none.

- `--near <text>` with a locator: the match whose own part of the page
  (its card, row or section) shows the text. `locate` writes it when a
  role and name match more than one element, before falling back to a CSS
  path.
- `locate <target>`: the locators that find an element again, most stable
  first (test id, role and name, label, placeholder, text, CSS), each kept
  only when the finder an op would use picks that element on the page;
  `--frame` added inside a frame (a cross-origin one named from the page
  that holds it); `secret: true` for a password field. An element in a
  shadow root gets its hosts' path: `--selector "#app >>> x-card >>> button"`.
- `--selector "host >>> inner"`: `inner` inside the open shadow root of an
  element `host` matches (or a shadow root under it), for every op,
  `wait --selector` and `get count`.
- `history [--clear]`: the actions a session ran and its waits, oldest first; one on a
  ref carries its element's `locate` from just before it ran, and each
  what the page said after it (title, heading, status messages). What
  `primavera from-session` turns into a test.

### Fixed

- Numbers from the page keep their exact value: `eval` and the JSON
  output printed about one double in five slightly off in its last digit
  (`9480.300552478155` as `9480.300552478157`). Found by fuzzing.
- `scroll --to bottom` (the page's, or inside a target) at the end already
  waits up to 2 s for a list that loads more to grow, then goes to the new
  end: two in a row, as a recorded session replays them, reached only the
  first page of a lazy list.
- `history` marks a click that changed nothing (`no_effect: true`), so a
  test writer can drop it when the same click follows.
- A `goto` to a server that takes the connection and never answers fails
  at its timeout and cancels that navigation: the tab answers at once again
  (it had answered nothing while the dead load was pending, so the next
  `url` or `ax`, and a test runner's failure screenshot, each waited their
  whole timeout too: one failed step cost 105 s).
- A control inside its `<label>` (a select, a textarea) is named by the
  label's own words for `--label`/`--name`, not with its options' text.
- `get box` of an element inside an iframe is in top-level viewport pixels
  (each frame's offset added), as `mouse` takes them; it was relative to
  the element's own frame.
- `--natural` with chrome-headless-shell as the browser (`--chromium`,
  `$CHROME_BIN`) is refused with a message instead of starting a browser
  that still tells pages it is headless.
- `--from` copies Chrome 156's local storage, which is one `LocalStorage`
  SQLite file there (with its journal) instead of a `Local Storage`
  directory: sign-ins kept in local storage came over empty.
- A `--profile` browser falls back to no sandbox the way navigera's own
  launches do (as root, with `NAVIGERA_NO_SANDBOX`, or after a first start
  that failed for want of one); it had exited and the profile could not
  be used on such a machine.

### Changed

- Browser discovery no longer looks in other tools' browser caches: it takes
  `--chromium`, `$CHROME_BIN`, a system Chrome, Chromium or Edge, or what
  `install-browser` installed. Point `$CHROME_BIN` at a browser elsewhere.

### Security

- tokio 1.54 (RUSTSEC-2026-0386).
- MCP: `browser_run` steps could read and write files outside the working
  directory (a route's `body_file` served any local file to the page;
  `state save`, `network --har`/`--body-file`, `pdf` and `downloads --dir`
  wrote anywhere). Every file a step names is now held to the working
  directory, as uploads were, and the browser's own pages (`chrome://`,
  `devtools://`) are refused; `NAVIGERA_MCP_ALLOW_FILES=1` lifts both.
- The default download directory is now inside `navigera-<uid>` in the
  temp directory (mode 0700), not a world-readable `navigera-downloads-<pid>`.

## [0.6.0] - 2026-10-24

The browser handed over to a person and back, downloads, frames of any
origin and custom dropdowns; clicks that wait for what moves or covers
their element and say when they changed nothing; and `reset` and `fork`,
so a test runner gets a clean, separate session in about 40 ms.

### Added

- Element ops reach into same-origin iframes, framesets and `srcdoc`
  frames, and `--selector` into open shadow roots, when the page itself
  has no match (role/name and text locators already reached shadow roots).
  `wait --selector/--text/--gone`, `get count` and `text` look there too.
- `--wait-ms <ms>` (on `start`, `serve`, `mcp` or one command;
  `NAVIGERA_WAIT_MS`): how long element ops wait for their element when they
  name no timeout. A test suite against a slow app sets it once.
- Downloads: files a page downloads are saved under `--download-dir`
  (`NAVIGERA_DOWNLOAD_DIR`; default a directory of the session's own) with
  the name the page gave them. `downloads` lists them; `wait --download
  "*.csv"` waits for one and returns its path. `downloads --dir <path>`
  gives one session a directory of its own (kept across `reset`; forks
  and other sessions keep theirs), so a test runner can give each test
  one and file names don't carry over (`report (2).csv`) between tests.
- `click --button right|middle` (a native context-menu click), `click
  --force` (DOM events on the element, covered or not), `select --add`
  (add to a multiple select's choice), `tab-select <target id>`,
  `network --since <n>`.
- `select` on custom dropdowns (ARIA comboboxes such as react-select): it
  opens them the way a user does (ArrowDown, else a click), finds the
  option in the listbox that opened (portals included), types to filter
  when it isn't listed, and clicks it.
- `fill --keys`: clear the field and type the value key by key, for forms
  whose app keeps only what was typed.
- `scroll --to top|bottom` with a target scrolls inside it.
- Selectors, text and locators reach into cross-origin iframes (a payment
  provider's card form, a sign-in frame) when the page itself has no match:
  navigera looks there from an isolated world of its own, which the page
  can't see. A single `wait --text`/`--selector` does too. With `--attach`,
  frames in another process stay ref-only.
- Library: `Driver::fork` (and `BrowserSession::fork`), another driver on
  the same browser in a browser context of its own, movable to another
  thread: several sessions at once without a browser each (a test runner's
  workers). Each takes only the tabs its own tabs open, and routes,
  credentials, the dialog answer and the downloads list are each
  session's own.
- `reset`: a clean slate in the same browser, a new browser context (no
  cookies, storage or cache from before) with one blank tab, in about 40 ms
  instead of a new browser's few hundred. For test runners that isolate
  scenarios, and agents that want to start over signed out. `set`, routes,
  init scripts and the dialog answer stay; `reset --all` drops them too.
- `--frame <css>` on element targets and `wait`: look only inside that
  iframe or frame (`"#outer >> #inner"` for nested ones), of this origin
  or another, when two frames hold the same element (the first frame the
  CSS matches; when it matches none, the error says so). Without it,
  frames are searched as before.
- `mouse move|down|up|click|wheel`: raw mouse input at a point, for what
  has no element to aim at (a canvas, a map, a game): viewport CSS pixels,
  or from a target's top-left corner (`mouse click 30 40 --selector
  canvas`). The pointer stays where it went and held buttons stay held, so
  `mouse down`, moves (`--steps` for a smooth path), `mouse up` drags or
  draws; `--count 2` double-clicks; `--button right|middle`. A target
  out of view is scrolled into view first, as for a click; `down`, `up`
  and `wheel` with a target go to its center; a click outside the
  viewport is refused instead of landing on nothing. MCP:
  `browser_mouse`. (An agent working through WebGames couldn't play its
  canvas games without it.)
- `--attach ws://host:port/devtools/page/<id>`: work in that one tab of a
  running browser as it is, not in a window of its own (a tab a paused
  test run left open for a look). `BrowserSession::page_endpoint` gives a
  tab's.
- A click that changes nothing says so (`no_effect` in its result): no DOM
  change, focus, form value, URL, scroll, clipboard write, request, dialog,
  tab or download within 200 ms. Requests that are the page reporting the
  click (beacons, analytics services, first-party `/event/…`-style paths
  whose answer changes nothing) don't count, and the note names them;
  nor does what the browser or page loads on its own meanwhile (the tab's
  icon, lazy images, fonts).
  Typically a button the page hasn't wired up yet. A click that does
  something costs nothing extra; one whose effect comes a moment later now
  returns once it has.

- `start --profile <name> --from chrome`: the agent's own browser, seeded
  with a copy of one of your Chrome profiles' sign-ins -- cookies (HttpOnly
  too), Local Storage, IndexedDB, saved logins -- so it starts signed in
  without touching your running browser, on any Chrome version (attaching
  needs 144+). `--from` takes `chrome`, `beta`, `dev`, `canary`,
  `chromium`, `edge` or a user data directory; `--from-profile` picks a
  profile by directory, name or email. The copy replaces only a profile
  navigera made, and is refused while that profile's browser runs.
- `show` / `hide` (MCP: `browser_handoff`): the session's browser on the
  person's screen with the same tabs and sign-ins, for a step only they can
  do -- a login, a 2FA prompt, a CAPTCHA -- and back off it. A `--profile`
  browser restarts on its own profile; one navigera launched hands its
  cookies and storage to its replacement.
- `navigera profiles`: the browsers and profiles a session can be handed
  (name, signed-in email, whether `--attach` reaches the browser now), and
  navigera's own `--profile` browsers.
- `--natural` (`NAVIGERA_NATURAL=1`): a headless browser that shows pages no
  sign of being headless -- the user agent the same Chrome sends with a
  window (page, workers, request headers) and a real screen size -- checked
  in CI against a page that runs the checks anti-bot scripts make.

### Changed

- A click that may navigate waits for this tab's navigation only when one
  can come: not for a `download` link, a link or form that opens another
  tab or window, or a link or submit inside a frame (which navigates the
  frame), and the wait ends as soon as the click opened a dialog, a tab or
  a download instead. Each of those clicks had cost 150-200 ms (a
  download link 170 ms, now about 20).

- A click waits for its element to stop moving: CSS animations and
  transitions on it or an ancestor (a modal sliding in) finish first, since
  pages often wire a dialog's buttons only once it has opened. Pages with
  nothing animating pay nothing for this.
- A script sent to a page that a navigation then replaced no longer waits
  out the whole timeout when the old page never answers (seen on macOS):
  a second after the new page commits, the op looks again there.
- `--text` prefers an exact match inside a cross-origin frame (the "Pay"
  button of a payment frame) over page text that merely contains it (a
  "Payment details" heading), and never picks an iframe by its title.
- `--text` matches what an element shows as well as what names it: a
  button showing "Add to cart" whose `aria-label` is "Add Carbon Gear to
  cart" is the match, not the product card around it (a click on the card
  did nothing; `no_effect` caught it in the benchmark).
- A click on an element that stays covered by another fails and names the
  covering element, instead of sending DOM events to an element no user
  could click. The element is first tried at other scroll positions, which
  is what uncovers it from a sticky header; `fill` does the same before
  typing into a field under a panel.
- `select` matches option labels with whitespace collapsed (non-breaking
  spaces count as spaces).
- `type` with a target fails when the target doesn't take focus, instead of
  typing into whatever has it.
- A disabled control is waited for (a Submit that unlocks once the form is
  valid) and otherwise refused: `click` and `fill` no longer report
  success on a disabled button, or a disabled or read-only field.
- When a selector's first match is out of a user's reach (a decoy copy of
  an id parked off the page) and another match isn't, the other one is
  used.
- A click on an element a user can't see where it is (zero size, clipped,
  the hidden input of a styled radio card) fails at once and names what
  stands for it, instead of waiting and then sending DOM events.
- An element op whose page is replaced by a navigation while it looks for
  its element (a step that saves with a request and then reloads) looks
  again on the new page instead of failing with "Inspected target
  navigated or closed".
- "not found" errors name the wait asked for (5000 ms, not 4750), and page
  errors drop the redundant `Error:` prefix.

- A `--profile` browser starts with `navigator.webdriver` false, as
  navigera's own launches always have, and `quit --close-browser` returns
  once the profile is free, so a `start` right after it finds no browser on
  its way out.

- With a decision model on, a plain `ax` of a tree over 16 KB
  (`NAVIGERA_AX_PREVIEW_BYTES`) shows its first 40 lines and how many more
  there are, with a note that `--find` and
  `check` act on the whole page without reading it and that `ax --limit
  <n>` reads it all. The note alone came after the whole tree, once the
  agent had read it: in the 0.5.0 eval the agent reached for `--find` or
  `check` unasked in 3 of 12 runs. `ax` with a limit, a depth, `--all` or a
  scope is unchanged, and so is `ax` without a decider.

### Fixed

- A page that stops answering (a script holding its thread, a renderer
  hung under load) cost one full command timeout per command after it:
  a click's press timed out, then its clean-up and checks each waited the
  whole 35 s again (105 s for one click on a loaded macOS runner). Now,
  once a command on a tab has gone unanswered, the next ones wait 3 s
  until the tab answers again: the click fails about 35 s in and says it
  timed out, and the next op fails in seconds rather than another 35.
- A session's only tab closing (the user closed it in an `--attach`ed
  browser, or the page closed itself) left every op failing with Chrome's
  "Session with given id not found", while `status` and `tab-list` still
  showed the tab. Now ops say the tab is closed and that `tab-new`
  opens another; `status` and the tab in `tab-list` carry `closed: true`;
  and `tab-new` (or `reset`) replaces the closed tab instead of listing
  it beside the new one. An op still waiting on a tab when it closes (an
  `eval` awaiting a promise, a `goto` waiting for a slow page) ends then
  and says so, instead of after its 35 s timeout; with other tabs open,
  the session goes on in one of them.
- `--attach <port>` and `--attach http://host:port` sent Chrome's
  `/json/version` request without the port in `Host`, got back a WebSocket
  URL without one, and failed to connect.
- `goto` and `reload` try once more when Chrome answers "Not attached to
  an active page" (the tab's target was swapped mid-navigation), instead of
  failing the step.
- `goto` no longer retries a navigation that ended in `ERR_ABORTED`
  because it was an answer: a URL that downloads a file was fetched four
  times (four copies on disk); now the file is saved once and `goto`
  succeeds with `download: <name>`, the page staying where it was. A 204 or
  205 answer fails at once and says so, instead of after three retries.
- `goto` and `reload` no longer wait out their whole timeout when the
  load event is lost on its way (seen as rare 35 s stalls on slow macOS
  machines): every 2 s they also look at the page itself, and return once
  its document is the new one and has loaded.

## [0.5.0] - 2026-10-22

Lightpanda 1.0 as a second engine, the inspection and state commands agents
reach for in Playwright, and a decision model the agent is told how to use.

### Added

- Lightpanda 1.0 (`--engine lightpanda`, or `NAVIGERA_ENGINE=lightpanda` for
  every command of a shell): the same commands and `ax` tree as on Chrome,
  with the engine's gaps covered (see Fixed). CI runs the end-to-end suite
  on Lightpanda 1.0.1 and the nightly.
- `drag <from> <to>`: HTML5 drag and drop for a `draggable` source (the drag
  events with one DataTransfer, since headless Chrome will not start a native
  drag from synthetic input, and on Lightpanda too), a held mouse moved
  across for anything else; the result says whether the drop was accepted.
- `state save|load <file>`: every cookie and the open tabs' localStorage to
  a file and back, in Playwright's `storageState` format, so a signed-in
  session can be reused, by navigera or Playwright.
- `status`: the session's engine, browser executable, CDP endpoint and tab
  state.
- `console`, `errors`, `network` (one request in full with `network <n>`,
  HAR with `--har`): what the page logged and fetched; an uncaught exception
  during a command is reported in its `notes`.
- `cookies` (list/set/delete/clear), `storage` (local/session), `pdf`.
- `set viewport|device|media|geo|offline|headers|ua|locale|timezone|credentials`:
  emulation for every tab, replayed on new tabs.
- `route` / `unroute`: fail or answer requests by URL glob.
- `init-script`: code that runs before the page's own in every document.
- Locators on every element command: `--role/--name`, `--label`,
  `--placeholder`, `--testid`, `--nth`.
- `get`, `is`, `dblclick`, `focus`, `check`/`uncheck <target>`,
  `keydown`/`keyup`, `insert-text`, `wait --idle`.
- MCP: `browser_console_messages`, `browser_network_requests`,
  `browser_resize`.
- `notes` in responses: what the engine did differently from what was asked
  (e.g. a Lightpanda popup loaded in the current tab).
- Agent eval: any OpenAI-compatible model as the agent (`--agent openai`,
  e.g. DeepSeek-V4.1-Flash through SiliconFlow) on the same tasks, prompt,
  turn limit and timeout as Gemini CLI; the decision benchmark takes such a
  model as its baseline; an OpenRouter head-to-head of tool variants
  (`openrouter-eval.yml`); a WebArena-Verified pilot (`bench/webarena`,
  `webarena-eval.yml`).

### Changed

- With a decision model on, the agent is told how to use it where it
  decides what to do next: `start` adds a note (act with `--find`, verify
  with `check`, read `ax` only for text to report), a tree over 60 lines
  ends with a one-line note that acting needs no reading, and the skill's
  main loop shows the decider form next to the ax form. Before, `start` said
  only `"decider": "<model>"` and the usage sat in a later skill section: in
  eval run 39437548538 the agent saw the decider in 8 of 10 runs and used
  it in none; in run 39561380436 it used it in 3 of 15 unprompted.
- Agent eval: a `preferences` task (a 170-line settings form, four named
  changes, a confirmation to check; the rest must stay as it was), the task
  the decider is measured on, and the three-arm protocol for measuring it
  (`navigera`, `navigera+jev`, `navigera+jev-told`, one model key each).
- `start` on a running session with a different engine fails instead of
  silently reusing it.

### Fixed

- `reload` and `goto` try again after a moment when Chrome answers "Not
  attached to an active page" (the tab between two documents; nothing was
  done yet): seen once in a parallel test run. `tab-select` reads 4 or
  more hex characters as a target id even when they are all digits.
- A fork's `tab-new` opened its tab in the default browser context, with
  the owner's cookies and storage; it opens in the fork's own now. A
  download started by a subframe is listed for the session whose tab holds
  it (it went to the default list); `reset --all` no longer reports a
  dialog answered before it; a closed fork's routes, answers and downloads
  are let go.
- Sessions on one browser (`Driver::fork`) no longer share routes, HTTP
  credentials, the dialog answer or the download list: each is the
  session's own, keyed by its browser context. One worker's `route` mocked
  the others' requests, and `unroute` removed theirs.
- `--attach <port>` and `--attach http://host:port`: the DevTools URL asked
  for carries the port again (Chrome answered with a WebSocket URL without
  one, and the connection was refused).
- Lightpanda 1.0.x: `goto` waits for the load event (it returned at commit
  and gave up after 500 ms); confirm()/prompt() get the dialog policy (they
  returned false/null while reported accepted); `target=_blank` pages are
  followed in the one tab; iframes and stylesheets load; table cells keep
  their text in `ax`; shadow-DOM clicks no longer wait 3 s; no `xvfb-run`
  wrapper; telemetry off through `LIGHTPANDA_DISABLE_TELEMETRY`.
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
- Lightpanda: a settings form reads in `ax` as it does on Chrome. A
  checkbox showed its default value `"on"`, which reads like a state (in the
  DeepSeek eval, run 39530836547, both Lightpanda runs of the preferences
  task left a box unticked and failed); a `<select>` inside a `<label>` was
  named with every option ("Currency USD EUR GBP …") next to a copy of the
  label; an unnamed table cell took two lines per checkbox. The preferences
  page is 177 lines on Lightpanda now, 243 before, 169 on Chrome.
- Lightpanda: a checkbox or radio shows in `ax` the state a click left it
  in. The engine's tree keeps the `checked` attribute the page loaded with,
  so a box an agent had ticked read as unticked; in the DeepSeek eval (run
  39691526622) an agent clicked one back that way and ran out of turns.
- Lightpanda: an empty password field no longer reads as filled in `ax`.
  The engine gives every password field the value `*****`; the snapshot now
  asks the field and shows a dot per character, or nothing, as on Chrome.

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

[0.10.0]: https://github.com/andrey-usa/navigera/releases/tag/v0.10.0
[0.9.0]: https://github.com/andrey-usa/navigera/releases/tag/v0.9.0
[0.8.0]: https://github.com/andrey-usa/navigera/releases/tag/v0.8.0
[0.7.1]: https://github.com/andrey-usa/navigera/releases/tag/v0.7.1
[0.7.0]: https://github.com/andrey-usa/navigera/releases/tag/v0.7.0
[0.6.5]: https://github.com/andrey-usa/navigera/releases/tag/v0.6.5
[0.6.4]: https://github.com/andrey-usa/navigera/releases/tag/v0.6.4
[0.6.3]: https://github.com/andrey-usa/navigera/releases/tag/v0.6.3
[0.6.2]: https://github.com/andrey-usa/navigera/releases/tag/v0.6.2
[0.6.1]: https://github.com/andrey-usa/navigera/releases/tag/v0.6.1
[0.6.0]: https://github.com/andrey-usa/navigera/releases/tag/v0.6.0
[0.5.0]: https://github.com/andrey-usa/navigera/releases/tag/v0.5.0
[0.4.1]: https://github.com/andrey-usa/navigera/releases/tag/v0.4.1
