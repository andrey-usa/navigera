# Command list (`navigera --help`)

Verbatim output of `navigera --help` from the current build:

```text
navigera 0.5.0 — drive headless Chrome over CDP, one shell command per step.

AGENT LOOP:
  navigera --session s start
  navigera --session s goto https://example.com
  navigera --session s ax                     # read: tree with [ref=N]
  navigera --session s click 12               # act on a ref
  navigera --session s fill 31 "hello"
  navigera --session s quit

COMMANDS:
  start         [--attach [<where>] | --profile <name> [--from <browser> [--from-profile <p>]] [--headless]] [--natural]
                spawn a detached session server + headless browser (needs --session); --attach/--profile use a browser that stays open; --from seeds the profile with a copy of your browser's sign-ins
  goto          <url> [--wait load|domcontentloaded|commit]
                navigate the active tab (default: wait for load)
  ax            [--selector <css> | <ref>] [--limit <lines>] [--refs all] [--format json]
                accessibility snapshot: indented tree, [ref=N] on actionable nodes
  click         <ref> | --selector <css> | --text <visible text> [--button right|middle] [--force]
                trusted mouse click once the element stops moving; refuses an element covered by another (--force: DOM events anyway); waits for any navigation it starts
  fill          <ref> <value> | --selector <css> --value <v> [--keys]
                set an input/textarea's value; --keys clears it and types the value key by key (for apps that ignore a value set directly)
  type          <text> [--selector <css> | --ref <n>]
                type key by key into the target or the focused element
  press         <key> [--selector <css> | --ref <n>]
                press a key or chord: Enter, Tab, Escape, ArrowDown, Control+a
  select        <ref> <option>... | --selector <css> --value <option> [--add]
                choose <select> option(s) by value or label (whitespace-insensitive); --add keeps the options already chosen
  hover         <ref> | --selector <css> | --text <t>
                move the mouse over an element (hover menus)
  mouse         move <x> <y> [--steps n] | down | up | click <x> <y> [--count 2] | wheel <dx> <dy>  [--button right|middle] [<target>]
                raw mouse at a point, for what has no element to aim at (a canvas, a map, a game): viewport CSS pixels (as in a screenshot), or from the target's top-left corner; the pointer stays where it went and held buttons stay held, so down, moves, up drags or draws
  scroll        [--by <px> | --to top|bottom] [<ref> | --selector <css>]
                scroll the page, or an element into view
  upload        <ref> <file>... | --selector <css> --file <path>
                set the files of an <input type=file>
  wait          [--selector <css>] [--text <t>] [--url <part>] [--gone <css>] [--js <expr>] [--idle] [--download [<glob>]] [--ms <n>]
                wait until every condition holds (default up to 5 s; --timeout-ms); --idle: no fetch/XHR for 500 ms; --download: a finished download
  downloads     [--clear] [--json]
                files the pages downloaded: #n state bytes path (url)
  eval          <js> | --file <path.js> | -
                evaluate JS (arrow functions are called); prints the JSON result; `-` reads the script from stdin
  text          --selector <css>
                textContent of the first match
  find          <description>
                the element a description means, chosen by a decision model (TYPESAFE_API_KEY); element ops also take --find <description>
  check         <yes/no question>
                probability that the answer about the page is yes, from a decision model (TYPESAFE_API_KEY)
  title         
                page title
  console       [--level error|warn|info|log|debug] [--limit <n>] [--clear] [--json]
                console messages, uncaught exceptions and browser log entries of the active tab
  errors        [--limit <n>] [--clear] [--json]
                only the errors: exceptions, console.error, failed loads
  network       [<n>] [--filter <text>] [--type fetch|xhr|document|…] [--failed] [--since <n>] [--limit <n>] [--clear] [--har <file>] [--body-file <file>]
                the active tab's requests, one line each (#n METHOD STATUS type url size time); `network <n>` shows one in full with its response body
  cookies       [set <name> <value> [--url <u> | --domain <d>] [--path <p>] [--expires <unix s>] [--http-only] [--secure] [--same-site Lax|Strict|None] | delete <name> [--url <u>] | clear] [--all] [--json]
                the current page's cookies (--all: every cookie), or set, delete or clear them
  pdf           [--path <f.pdf>] [--landscape] [--background]
                the page as a PDF file
  storage       [local|session] [get [<key>] | set <key> <value> | remove <key> | clear]
                the current page's localStorage (or sessionStorage)
  state         save|load <file.json>
                every cookie and the open tabs' localStorage to a file, or back (Playwright storageState format): sign in once, reuse it in later sessions
  set           viewport <w> <h> [--scale <dpr>] [--mobile] | device <name> | media dark|light|print|reduced-motion | geo <lat> <lon> | offline on|off | headers <Name: value>… | ua <string> | locale <tag> | timezone <zone> | credentials <user> <password>  (`off` resets)
                emulate a device or network condition in every tab (and in tabs opened later)
  route         [<url glob> [--abort | --status <n> [--body <text> | --body-file <f>] [--content-type <t>] [--header 'Name: value']]]
                fail or answer requests whose URL matches (* and ? globs); no flags = fail; no pattern = list the rules
  unroute       [<url glob>]
                remove one route rule, or all
  init-script   [<js> | --file <path.js> | --clear]
                run a script before the page's own in every document of every tab; no argument lists them
  get           text|value|html|outer|box|attr <name>|style <prop> <target> | count <css>
                read one property of an element (target: ref, --selector, --text, --role/--name, --label, --placeholder, --testid, --nth; --frame <css> to look only inside that frame)
  is            visible|hidden|enabled|disabled|checked|editable|focused <target>
                true/false; a missing element is not visible
  dblclick      <target>
                trusted double click
  drag          <from> <to> | --from-text <t> --to-text <t>
                drag one element onto another (each a ref or CSS selector): HTML5 drag and drop for a draggable source, a held mouse moved across otherwise
  focus         <target>
                focus an element
  check         <target> | <yes/no question>
                check a checkbox/radio/switch (clicks only if needed); with a question instead of a target, the decision model's yes/no
  uncheck       <target>
                uncheck a checkbox/switch
  keydown       <key>
                hold a key (Shift, Control, a character) until keyup
  keyup         <key>
                release a held key
  insert-text   <text> [<target>]
                insert text at the caret in one go (paste/IME), no key events
  url           
                active tab state {tab, tabs, url, title}
  status        
                the session's engine, browser executable, CDP endpoint and tab state
  back          
                history back
  forward       
                history forward
  reload        
                reload the page
  screenshot    [--path <f.png>] [--full-page]
                PNG of the viewport (or whole page)
  reset         [--all]
                a clean slate in the same browser: a new browser context (no cookies, storage or cache from before) with one blank tab; the old tabs close (`set`, routes, init scripts and the dialog answer stay; --all drops them too)
  tab-new       [<url>]
                open a tab and make it active
  tab-list      
                list tabs
  show          
                put the browser on screen with the same tabs and sign-ins, to hand the session to a person (a login, 2FA, a CAPTCHA); `hide` takes it back off
  hide          
                take the browser off screen again (headless), with the same tabs and sign-ins
  tab-select    <index> | <target id>
                switch the active tab (a target id from tab-list stays right when tabs open and close)
  tab-close     [<index>]
                close a tab (default: active)
  dialog        --accept [--prompt-text <t>] | --dismiss
                how alert/confirm/prompt are answered (default: accept; every dialog is reported)
  quit          [--close-browser]
                close the browser and the session server; attached (--attach/--profile): only disconnect, unless --close-browser
  mcp           [--headed] [--profile <name> | --attach [<where>]]
                serve the Model Context Protocol on stdio (for Cline, Roo, Kilo, Continue, VS Code, Claude Desktop, Cursor); the browser starts on the first tool call
  profiles      
                list what a session can be handed: your browsers' profiles (name, email, whether --attach can reach them now) and navigera's own --profile browsers
  skill         
                print the agent skill (usage guide) for this version
  install-skill [--dir <skills dir>] [--claude | --continue] [--global]
                install the skill for agents (default ./.agents/skills, read by Cline and Kilo; --claude: ./.claude/skills; --continue: ./.continue/skills)

GLOBAL FLAGS (before or after the command):
  --session <name>       warm browser behind a local socket ($NAVIGERA_SESSION)
  --raw                  print only the result (strings unquoted); errors on stderr
  --timeout-ms <ms>      navigation/command timeout (default 35000); per op: element wait (default 5000)
  --chromium <path>      browser binary ($CHROME_BIN); --engine lightpanda ($NAVIGERA_ENGINE); --headed
  --transport ws         CDP over a DevTools port instead of a private pipe (to attach DevTools; Windows default)
  --idle-timeout-s <s>   session server idle shutdown (default 1800, 0 = never)
  --attach [chrome|edge|<dir>|<port>]  with `start`: use your running browser (Chrome 144+: chrome://inspect/#remote-debugging); never closes it
  --profile <name>       with `start`: persistent visible navigera browser, kept open between sessions (--headless to hide)
  --from <browser|dir>   with --profile: first copy your own browser's sign-ins into it (cookies, site storage, saved logins); --from-profile <dir|name|email>
  --natural              look like the browser it is: no HeadlessChrome user agent, a real screen size ($NAVIGERA_NATURAL=1)
  --pretty               pretty-print JSON

Output: one JSON object per command, {"ok":true,"result":…} or {"ok":false,"error":…}; exit 1 on error.
Without --session each command launches and closes its own browser (one-shot).
Programs: `navigera serve` reads JSON commands on stdin, e.g. {"id":1,"op":"goto","url":"…"}.
More: `navigera help <command>`, `navigera skill`.
```
