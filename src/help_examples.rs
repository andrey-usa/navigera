//! `help <command>`: examples that run as shown and a line per flag.
//!
//! Each example was run against the-internet.herokuapp.com or example.com
//! (the page it needs open first); `navigera` in one stands for the program
//! name help is printed under.

/// A command, its examples (`(command line, comment)`) and its flags
/// (`(flag, what it does)`).
pub type Help = (&'static str, &'static [(&'static str, &'static str)], &'static [(&'static str, &'static str)]);

pub const EXAMPLES: &[Help] = &[
    (
        "start",
        &[
            ("navigera -s s start", ""),
            ("navigera -s s start --natural", "browser that looks less headless"),
            ("navigera -s s start --parent 4242", "the session ends when process 4242 (the agent) exits"),
            ("navigera -s app start --electron ./my-app", "an Electron app; its windows are the tabs"),
        ],
        &[
            ("--own-browser", "a browser for this session alone (default: one shared browser, a context each)"),
            ("--idle <s>", "end the session after this long unused (default 600; 0 = never)"),
            ("--parent <pid>", "end the session when that process exits ($NAVIGERA_PARENT_PID)"),
            ("--attach [<where>]", "use a running browser; it stays open after quit"),
            ("--profile <name>", "a persistent navigera browser, kept between sessions"),
            ("--from <browser>", "seed the profile with a copy of a browser's sign-ins"),
            ("--from-profile <p>", "which profile --from copies (dir, name or email)"),
            ("--headless", "hide a --profile browser (default: visible)"),
            ("--electron <app>", "launch an Electron app (its directory, main script or executable); quit ends it"),
            ("--electron-arg <arg>", "an argument for the app (repeatable)"),
            ("--natural", "no HeadlessChrome user agent, a real screen size"),
            ("--download-dir <dir>", "where this session saves downloads"),
        ],
    ),
    (
        "goto",
        &[
            ("navigera -s s goto https://example.com", ""),
            ("navigera -s s goto https://example.com --wait domcontentloaded", ""),
        ],
        &[("--wait load|domcontentloaded|commit", "how much of the load to wait for; default load")],
    ),
    (
        "ax",
        &[
            ("navigera -s s ax", ""),
            ("navigera -s s ax --limit 20", ""),
            ("navigera -s s ax --selector \"input[type=number]\"", ""),
        ],
        &[
            ("<ref>", "scope the tree to the element with this ref"),
            ("--selector <css>", "scope the tree to the first CSS match"),
            ("--text <text>", "scope the tree to the element with this text"),
            ("<locator>", "scope via --role/--name/--label/--testid/--near/--nth"),
            ("--limit <lines>", "most lines to print; default 2000"),
            ("--max-depth <n>", "stop the tree at this depth"),
            ("--all", "every node of the accessibility tree, as a JSON list"),
            ("--refs all", "put [ref=N] on every node, not just actionable ones"),
            ("--format json", "print the tree as JSON instead of text"),
            ("--no-wait", "skip the wait for the network to go quiet (up to 3 s after the last fetch/XHR, so a list still loading has landed); steady polls never hold it; a successful `wait` also skips the next snapshot's wait while no request happens"),
        ],
    ),
    (
        "click",
        &[
            ("navigera -s s click --selector \"input[type=checkbox]\"", ""),
            ("navigera -s s click --text \"Add Element\"", "on /add_remove_elements/"),
            ("navigera -s s click --button right --selector \"input[type=checkbox]\"", ""),
        ],
        &[
            ("<ref>", "the element's ref from ax"),
            ("--selector <css>", "the element, by CSS"),
            ("--text <visible text>", "the element, by its visible text"),
            ("--frame <css>", "look only inside this iframe"),
            ("--button left|right|middle", "which mouse button; default left"),
            ("--force", "click even if another element covers it"),
        ],
    ),
    (
        "fill",
        &[
            ("navigera -s s fill --selector \"input[type=number]\" --value 42", ""),
            ("navigera -s s fill --selector \"input[type=number]\" --value 7 --keys", ""),
        ],
        &[
            ("<ref>", "the input's ref from ax"),
            ("<value>", "the value to set"),
            ("--selector <css>", "the input, by CSS"),
            ("--value <v>", "the value to set (with --selector)"),
            ("--frame <css>", "look only inside this iframe"),
            ("--keys", "clear it and type key by key instead of setting"),
        ],
    ),
    (
        "type",
        &[("navigera -s s type 99 --selector \"input[type=number]\"", "")],
        &[
            ("<text>", "the text to type, key by key"),
            ("--selector <css>", "the element to type into, by CSS"),
            ("--ref <n>", "the element to type into, by ref"),
        ],
    ),
    (
        "press",
        &[("navigera -s s press Tab", ""), ("navigera -s s press Enter --selector \"input[type=number]\"", "")],
        &[
            ("<key>", "the key or chord: Enter, Tab, Escape, Control+a"),
            ("--selector <css>", "the element to press it in, by CSS"),
            ("--ref <n>", "the element to press it in, by ref"),
        ],
    ),
    (
        "select",
        &[
            ("navigera -s s select --selector \"#dropdown\" --value 1", ""),
            ("navigera -s s select --selector \"#dropdown\" --value \"Option 2\"", ""),
        ],
        &[
            ("<ref>", "the <select>'s ref from ax"),
            ("<option>...", "option(s) by value or label"),
            ("--selector <css>", "the <select>, by CSS"),
            ("--value <option>", "the option by value or label (with --selector)"),
            ("--add", "keep the options already chosen"),
        ],
    ),
    (
        "hover",
        &[("navigera -s s hover --selector \".figure\"", ""), ("navigera -s s hover --selector \".figure img\"", "")],
        &[
            ("<ref>", "the element's ref from ax"),
            ("--selector <css>", "the element, by CSS"),
            ("--text <t>", "the element, by its visible text"),
        ],
    ),
    (
        "mouse",
        &[
            ("navigera -s s mouse move 100 100", ""),
            ("navigera -s s mouse click 100 100", ""),
            ("navigera -s s mouse wheel 0 100", ""),
        ],
        &[
            ("move <x> <y>", "move the pointer to viewport CSS pixels"),
            ("down", "press the held mouse button down"),
            ("up", "release the held mouse button"),
            ("click <x> <y>", "click at viewport CSS pixels"),
            ("wheel <dx> <dy>", "scroll by this many pixels"),
            ("--steps n", "split a move into this many steps"),
            ("--count 2", "how many clicks a mouse click makes; default 1"),
            ("--button left|right|middle", "which mouse button; default left"),
            ("<target>", "measure from this element's top-left, not the viewport"),
        ],
    ),
    (
        "scroll",
        &[("navigera -s s scroll --by 500", ""), ("navigera -s s scroll --to bottom", "")],
        &[
            ("--by <px>", "scroll down this many pixels"),
            ("--to top|bottom", "scroll to the top or bottom of the page"),
            ("<ref>", "bring this element into view, by ref"),
            ("--selector <css>", "scroll inside this element, or bring it into view"),
        ],
    ),
    (
        "upload",
        &[("navigera -s s upload --selector \"#file-upload\" --file /tmp/upload.txt", "")],
        &[
            ("<ref>", "the file input's ref from ax"),
            ("<file>...", "the file(s) to set"),
            ("--selector <css>", "the file input, by CSS"),
            ("--file <path>", "the file to set (with --selector)"),
        ],
    ),
    (
        "wait",
        &[
            ("navigera -s s wait --text \"Inputs\"", ""),
            ("navigera -s s wait --ms 500", ""),
            ("navigera -s s wait --selector \"input[type=number]\"", ""),
        ],
        &[
            ("--selector <css>", "wait for an element matching this CSS"),
            ("--text <t>", "wait for this text to appear"),
            ("--url <part>", "wait for the URL to contain this"),
            ("--gone <css>", "wait for this CSS to disappear"),
            ("--js <expr>", "wait until this JS expression is truthy"),
            ("--frame <css>", "look only inside this iframe"),
            ("--idle", "wait for no fetch/XHR for 500 ms"),
            ("--load networkidle", "wait until the network is idle"),
            ("--download [<glob>]", "wait for a finished download, optionally matching"),
            ("--ms <n>", "wait exactly this many milliseconds"),
        ],
    ),
    (
        "downloads",
        &[
            ("navigera -s s downloads", ""),
            ("navigera -s s downloads --json", ""),
            ("navigera -s s downloads --dir /tmp/nav-dl", ""),
        ],
        &[
            ("--clear", "forget the downloads listed so far"),
            ("--json", "print the list as JSON instead of lines"),
            ("--dir <path>", "save this session's downloads here from now on"),
        ],
    ),
    (
        "eval",
        &[
            ("navigera -s s eval \"() => document.title\"", ""),
            ("navigera -s s eval \"() => document.links.length\"", ""),
            ("navigera -s app eval --main \"({ app }) => app.getName()\"", "Electron: the main process"),
        ],
        &[
            ("<js>", "the JavaScript to run; arrow functions are called"),
            ("--file <path.js>", "read the script from this file"),
            ("-", "read the script from stdin"),
            ("--main", "Electron: run in the app's main process; a function gets the electron module"),
        ],
    ),
    (
        "text",
        &[("navigera -s s text --selector \"h3\"", ""), ("navigera -s s text", "the whole page, as it reads")],
        &[("--selector <css>", "the element whose textContent to print (none: the page's innerText)")],
    ),
    (
        "find",
        &[("navigera -s s find \"the number input\"", "needs a decision model")],
        &[
            ("<description>", "the element, described in words; needs a decision model"),
            ("--no-wait", "skip the wait for the network to go quiet (up to 3 s after the last fetch/XHR, so a list still loading has landed); steady polls never hold it"),
        ],
    ),
    ("title", &[("navigera -s s title", "")], &[]),
    (
        "console",
        &[("navigera -s s console", ""), ("navigera -s s console --level error", "")],
        &[
            ("--level error|warn|info|log|debug", "only messages at this level"),
            ("--limit <n>", "most messages to print"),
            ("--clear", "clear the console buffer after printing"),
            ("--json", "print messages as JSON instead of lines"),
        ],
    ),
    (
        "errors",
        &[("navigera -s s errors", ""), ("navigera -s s errors --limit 5", "")],
        &[
            ("--limit <n>", "most errors to print"),
            ("--clear", "clear the error buffer after printing"),
            ("--json", "print errors as JSON instead of lines"),
        ],
    ),
    (
        "network",
        &[
            ("navigera -s s network", ""),
            ("navigera -s s network --failed", ""),
            ("navigera -s s network --limit 5", ""),
        ],
        &[
            ("<n>", "show request #n in full, with its response body"),
            ("--filter <text>", "only requests whose URL contains this"),
            ("--type fetch|xhr|document|…", "only requests of this type"),
            ("--failed", "only requests that failed"),
            ("--since <n>", "only requests after #n"),
            ("--limit <n>", "most requests to print"),
            ("--clear", "clear the request log after printing"),
            ("--har <file>", "write the requests as a HAR file"),
            ("--body-file <file>", "write request #n's body to this file"),
        ],
    ),
    (
        "cookies",
        &[
            ("navigera -s s cookies", ""),
            ("navigera -s s cookies set demo hello", ""),
            ("navigera -s s cookies delete demo", ""),
        ],
        &[
            ("set <name> <value>", "set a cookie on the current page"),
            ("delete <name>", "delete a cookie from the current page"),
            ("clear", "delete all of the current page's cookies"),
            ("--url <u>", "the URL the cookie belongs to"),
            ("--domain <d>", "the domain the cookie belongs to"),
            ("--path <p>", "the path the cookie belongs to; default /"),
            ("--expires <unix s>", "when the cookie expires, as a unix time"),
            ("--http-only", "mark the cookie http-only"),
            ("--secure", "mark the cookie secure"),
            ("--same-site Lax|Strict|None", "the cookie's SameSite value"),
            ("--all", "list every cookie, not just this page's"),
            ("--json", "print cookies as JSON instead of lines"),
        ],
    ),
    (
        "pdf",
        &[
            ("navigera -s s pdf --path /tmp/nav-page.pdf", ""),
            ("navigera -s s pdf --path /tmp/nav-page.pdf --landscape", ""),
        ],
        &[
            ("--path <f.pdf>", "where to write the PDF"),
            ("--landscape", "landscape pages instead of portrait"),
            ("--background", "include the page's background graphics"),
        ],
    ),
    (
        "storage",
        &[
            ("navigera -s s storage set demo hello", ""),
            ("navigera -s s storage get demo", ""),
            ("navigera -s s storage remove demo", ""),
        ],
        &[
            ("local|session", "localStorage (default) or sessionStorage"),
            ("get [<key>]", "print one value, or all of it with no key"),
            ("set <key> <value>", "store a value"),
            ("remove <key>", "delete one key"),
            ("clear", "delete everything"),
        ],
    ),
    (
        "state",
        &[("navigera -s s state save /tmp/nav-state.json", ""), ("navigera -s s state load /tmp/nav-state.json", "")],
        &[
            ("save <file.json>", "write cookies + localStorage to this file"),
            ("load <file.json>", "restore cookies + localStorage from this file"),
        ],
    ),
    (
        "set",
        &[
            ("navigera -s s set viewport 1280 720", ""),
            ("navigera -s s set media dark", ""),
            ("navigera -s s set device \"iPhone 15\"", ""),
        ],
        &[
            ("viewport <w> <h>", "the viewport size in CSS pixels"),
            ("--scale <dpr>", "device pixel ratio for a viewport; default 1"),
            ("--mobile", "emulate a mobile viewport with touch"),
            ("device <name>", "a named device's viewport, scale and touch"),
            ("media dark|light|print|reduced-motion", "the emulated media feature"),
            ("geo <lat> <lon>", "the emulated geolocation"),
            ("offline on|off", "cut or restore the network"),
            ("headers <Name: value>…", "extra HTTP headers on every request"),
            ("ua <string>", "the user agent string"),
            ("locale <tag>", "the emulated locale, e.g. en-US"),
            ("timezone <zone>", "the emulated timezone, e.g. Europe/London"),
            ("credentials <user> <password>", "HTTP basic-auth credentials (`off` resets)"),
            ("network-quiet <ms>", "this session's snapshot network-quiet window (default 500; 0 skips the window itself, pending requests still wait)"),
        ],
    ),
    (
        "route",
        &[("navigera -s s route", ""), ("navigera -s s route \"*://*.example.com/*\" --abort", "")],
        &[
            ("<url glob>", "requests to match, with * and ? wildcards"),
            ("--abort", "fail matching requests (the default with no flag)"),
            ("--status <n>", "answer matching requests with this status"),
            ("--body <text>", "the body for --status answers"),
            ("--body-file <f>", "read the --status body from this file"),
            ("--content-type <t>", "the Content-Type for --status answers"),
            ("--header 'Name: value'", "an extra header for --status answers"),
        ],
    ),
    (
        "unroute",
        &[("navigera -s s unroute \"*://*.example.com/*\"", ""), ("navigera -s s unroute", "")],
        &[("<url glob>", "the rule to remove; none removes all rules")],
    ),
    (
        "init-script",
        &[("navigera -s s init-script \"window.demo = 42\"", ""), ("navigera -s s init-script --clear", "")],
        &[
            ("<js>", "the script to run before every page's own scripts"),
            ("--file <path.js>", "read the script from this file"),
            ("--clear", "remove all init scripts"),
        ],
    ),
    (
        "get",
        &[
            ("navigera -s s get text --selector \"h3\"", ""),
            ("navigera -s s get value --selector \"input[type=number]\"", ""),
            ("navigera -s s get count \"a\"", ""),
        ],
        &[
            ("text", "the element's textContent"),
            ("value", "the input's value"),
            ("html", "the element's innerHTML"),
            ("outer", "the element's outerHTML"),
            ("box", "the element's bounding box"),
            ("checked", "whether a checkbox/radio is checked"),
            ("attr <name>", "one attribute's value"),
            ("style <prop>", "one computed CSS property"),
            ("<target>", "ref, --selector, --text, --role/--name, …"),
            ("count <css>", "how many elements match this CSS"),
            ("title", "the page title"),
            ("url", "the active tab state {tab, tabs, url, title}"),
        ],
    ),
    (
        "is",
        &[
            ("navigera -s s is visible --selector \"h3\"", ""),
            ("navigera -s s is enabled --selector \"input[type=number]\"", ""),
            ("navigera -s s is checked --selector \"input[type=checkbox]\"", ""),
        ],
        &[
            ("visible", "true if the element is visible"),
            ("hidden", "true if the element is hidden"),
            ("enabled", "true if the element is enabled"),
            ("disabled", "true if the element is disabled"),
            ("checked", "true if a checkbox/radio is checked"),
            ("editable", "true if the element can be edited"),
            ("focused", "true if the element has focus"),
            ("<target>", "ref, --selector, --text, --role/--name, …"),
        ],
    ),
    (
        "dblclick",
        &[("navigera -s s dblclick --selector \"input[type=number]\"", "")],
        &[("<target>", "ref, --selector, --text, --role/--name, …")],
    ),
    (
        "drag",
        &[
            ("navigera -s s drag --from-text \"A\" --to-text \"B\"", ""),
            ("navigera -s s drag \"#column-a\" \"#column-b\"", ""),
            ("navigera -s s drag 12 14 --to-at 20,5", "drop near the top edge (before it, in a list)"),
        ],
        &[
            ("<from>", "the source: a ref or CSS selector"),
            ("<to>", "the target: a ref or CSS selector"),
            ("--from-text <t>", "the source, by its visible text"),
            ("--to-text <t>", "the target, by its visible text"),
            ("--from-at <x>,<y>", "press here, in CSS px from the source's top-left (default: center)"),
            ("--to-at <x>,<y>", "drop here, in CSS px from the target's top-left (default: center)"),
        ],
    ),
    (
        "focus",
        &[("navigera -s s focus --selector \"input[type=number]\"", "")],
        &[("<target>", "ref, --selector, --text, --role/--name, …")],
    ),
    (
        "check",
        &[
            ("navigera -s s check --selector \"input[type=checkbox]\"", ""),
            ("navigera -s s check \"Is a box checked?\"", "a yes/no question: needs a decision model"),
        ],
        &[
            ("<target>", "the checkbox/radio/switch, by ref, --selector, …"),
            ("<yes/no question>", "ask the decision model; needs NAVIGERA_DECIDER_KEY"),
            ("--no-wait", "with a question: skip the wait for the network to go quiet (see `ax --no-wait`)"),
        ],
    ),
    (
        "uncheck",
        &[("navigera -s s uncheck --selector \"input[type=checkbox]\"", "")],
        &[("<target>", "the checkbox/switch, by ref, --selector, …")],
    ),
    ("keydown", &[("navigera -s s keydown Shift", "")], &[("<key>", "the key to hold: Shift, Control, a character")]),
    ("keyup", &[("navigera -s s keyup Shift", "")], &[("<key>", "the held key to release")]),
    (
        "insert-text",
        &[("navigera -s s insert-text 99 --selector \"input[type=number]\"", "")],
        &[
            ("<text>", "the text to insert at the caret, in one go"),
            ("<target>", "ref, --selector, --text, --role/--name, …"),
        ],
    ),
    ("url", &[("navigera -s s url", "")], &[]),
    ("status", &[("navigera -s s status", "")], &[]),
    (
        "locate",
        &[("navigera -s s locate --selector \"input[type=number]\"", "")],
        &[("<target>", "ref, --selector, --text, --role/--name, …")],
    ),
    (
        "history",
        &[("navigera -s s history", ""), ("navigera -s s history --clear", "")],
        &[("--clear", "forget the actions run so far")],
    ),
    ("back", &[("navigera -s s back", "")], &[]),
    ("forward", &[("navigera -s s forward", "")], &[]),
    ("reload", &[("navigera -s s reload", "")], &[]),
    (
        "screenshot",
        &[
            ("navigera -s s screenshot --path /tmp/nav-shot.png", ""),
            ("navigera -s s screenshot --path /tmp/nav-shot.png --full-page", ""),
        ],
        &[
            ("--path <f.png>", "where to write the PNG"),
            ("--full-page", "capture the whole page, not just the viewport"),
            ("--no-wait", "skip the wait for the network to go quiet (up to 3 s after the last fetch/XHR, so a list still loading has landed); steady polls never hold it"),
        ],
    ),
    (
        "reset",
        &[("navigera -s s reset", ""), ("navigera -s s reset --all", "")],
        &[("--all", "also drop set/routes/credentials/init scripts")],
    ),
    (
        "tab-new",
        &[("navigera -s s tab-new https://example.com", ""), ("navigera -s s tab-new", "")],
        &[("<url>", "the URL for the new tab; none opens a blank tab")],
    ),
    ("tab-list", &[("navigera -s s tab-list", "")], &[]),
    ("show", &[("navigera -s s show", "")], &[]),
    ("hide", &[("navigera -s s hide", "")], &[]),
    (
        "tab-select",
        &[("navigera -s s tab-select 0", ""), ("navigera -s s tab-select 1", "")],
        &[("<index>", "the tab's index from tab-list"), ("<target id>", "the tab's id from tab-list")],
    ),
    (
        "tab-close",
        &[("navigera -s s tab-close", ""), ("navigera -s s tab-close 0", "")],
        &[("<index>", "the tab to close; none closes the active tab")],
    ),
    (
        "dialog",
        &[("navigera -s s dialog --accept", ""), ("navigera -s s dialog --dismiss", "")],
        &[
            ("--accept", "answer later dialogs by accepting (the default)"),
            ("--prompt-text <t>", "the text to answer a prompt dialog with"),
            ("--dismiss", "answer later dialogs by dismissing"),
        ],
    ),
    (
        "quit",
        &[("navigera -s s quit", ""), ("navigera -s s quit --close-browser", "")],
        &[("--close-browser", "also close an attached/--profile browser (a shared one stays for its other sessions)")],
    ),
    (
        "serve",
        &[
            ("navigera serve", "JSON commands on stdin, answers on stdout"),
            ("echo '{\"id\":1,\"op\":\"title\"}' | navigera serve", "one command; EOF ends it"),
        ],
        &[],
    ),
    (
        "mcp",
        &[
            ("navigera mcp", "serve the Model Context Protocol on stdio"),
            ("navigera mcp --headed", "visible browser for the MCP tools"),
        ],
        &[
            ("--headed", "run the browser visibly instead of headless"),
            ("--profile <name>", "serve tools against this persistent browser"),
            ("--attach [<where>]", "serve tools against a running browser"),
        ],
    ),
    ("profiles", &[("navigera profiles", "")], &[]),
    (
        "sessions",
        &[
            ("navigera sessions", "name, pid, idle time, browser, memory of each"),
            ("navigera sessions --clean", "quit the ones idle 5 min or more"),
            ("navigera sessions --clean --all", "quit every session; say how much memory that freed"),
        ],
        &[
            ("--clean", "quit idle sessions, drop dead ones, close unused shared browsers"),
            ("--idle <s>", "with --clean: idle for this long or more (default 300)"),
            ("--all", "with --clean: every session"),
        ],
    ),
    ("skill", &[("navigera skill", "")], &[]),
    (
        "install-browser",
        &[
            ("navigera install-browser", "Google's headless Chrome, into navigera's cache"),
            ("navigera install-browser --where", "print the browser directory only"),
        ],
        &[
            ("chrome-headless-shell|chrome", "which Chrome build; default chrome-headless-shell"),
            ("--channel stable|beta|dev|canary", "the Chrome channel; default stable"),
            ("--where", "print the directory instead of downloading"),
        ],
    ),
    (
        "install-skill",
        &[
            ("navigera install-skill", "into ./.agents/skills"),
            ("navigera install-skill --claude", "into ./.claude/skills"),
        ],
        &[
            ("--dir <skills dir>", "where to install; default ./.agents/skills (wins over --claude)"),
            ("--claude", "install under ./.claude/skills instead"),
            ("--continue", "install under ./.continue/skills instead"),
            ("--global", "install for all projects, not this one"),
        ],
    ),
];
