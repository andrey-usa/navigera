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
            ("navigera -s me start --auto-connect", "your running browser, in a window of the agent's own behind yours"),
            ("navigera -s me start --auto-connect --tab current", "your running browser, in the tab you are on"),
        ],
        &[
            ("--own-browser", "a browser for this session alone (default: one shared browser, a context each)"),
            ("--idle <s>", "end the session after this long unused (default 600; 0 = never)"),
            ("--parent <pid>", "end the session when that process exits ($NAVIGERA_PARENT_PID)"),
            ("--attach [<where>]", "use a running browser; it stays open after quit (chrome, edge, a port, a profile dir)"),
            ("--auto-connect", "--attach to whichever of your browsers serves remote debugging ($NAVIGERA_AUTO_CONNECT=1)"),
            ("--tab current", "with --attach: start in the tab you are on, not a window of the agent's own"),
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
            ("navigera -s s ax --grep \"number\"", "only the lines that mention it, with their refs"),
        ],
        &[
            ("<ref>", "scope the tree to the element with this ref"),
            ("--selector <css>", "scope the tree to the first CSS match"),
            ("--text <text>", "scope the tree to the element with this text"),
            ("<locator>", "scope via --role/--name/--label/--testid/--near/--nth"),
            ("--limit <lines>", "most lines to print; default 2000"),
            ("--grep <words>", "only the lines that mention the words, under their containers (--limit: matching lines, 30)"),
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
            ("navigera -s s click --text \"Load more\" --until-gone", "the whole list, in one call"),
        ],
        &[
            ("<ref>", "the element's ref from ax"),
            ("--selector <css>", "the element, by CSS"),
            ("--text <visible text>", "the element, by its visible text"),
            ("--frame <css>", "look only inside this iframe"),
            ("--button left|right|middle", "which mouse button; default left"),
            ("--force", "click even if another element covers it"),
            ("--until-gone", "click again until it is gone, hidden or disabled, or a click changes nothing"),
            ("--max <n>", "with --until-gone: at most this many clicks (50)"),
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
        &[
            ("navigera -s s text --selector \"h3\"", ""),
            ("navigera -s s text", "the whole page, as it reads"),
            ("navigera -s s text --grep \"free shipping\"", "one fact from a long page: the passages that mention it"),
        ],
        &[
            ("--selector <css>", "the element whose text to print, as it reads (none: the whole page)"),
            ("--grep <words>", "only the passages that mention the words (letter case aside; all the words in any order when the phrase is nowhere)"),
            ("--context <lines>", "with --grep: lines shown around each match (1)"),
            ("--limit <passages>", "with --grep: passages shown (5)"),
        ],
    ),
    ("title", &[("navigera -s s title", "")], &[]),
    (
        "passkey",
        &[
            ("navigera -s s passkey on", "before the page asks to create or use a passkey"),
            ("navigera -s s passkey list", "the passkeys the page made, without their private keys"),
            ("navigera -s s state save auth.json", "keeps them too: `state load auth.json` signs in again later"),
            ("navigera -s s passkey verify off", "the next ceremony fails as a wrong fingerprint would"),
        ],
        &[
            ("on", "a virtual authenticator in every tab and in tabs opened later; a phone's or laptop's own (internal) with discoverable passkeys and user verification"),
            ("--authenticator usb|nfc|ble|hybrid", "on: a security key (usb, nfc, ble) or a phone (hybrid) instead of the device's own"),
            ("--no-user-verification", "on: an authenticator with no fingerprint or PIN"),
            ("--no-resident-key", "on: passkeys the site must name (no username-less sign-in)"),
            ("load <file.json>", "add passkeys: one, a list, or a state file's credentials (Playwright's shape works)"),
            ("remove <id> | clear", "one passkey, or all of them"),
            ("off", "no authenticator, and its passkeys gone"),
        ],
    ),
    (
        "a11y",
        &[
            ("navigera -s s a11y", "the whole page"),
            ("navigera -s s a11y --selector form --ignore color-contrast", "one part, a rule left out"),
        ],
        &[
            ("--selector <css>", "only this part of the page"),
            ("--tags <list>", "axe tags: wcag2a, wcag2aa, wcag21aa, wcag22aa, best-practice (default: WCAG 2.0-2.2 A and AA)"),
            ("--ignore <list>", "rules to leave out, by id (color-contrast, region, …)"),
        ],
    ),
    (
        "vitals",
        &[
            ("navigera -s s vitals", "after the page loaded, and again after the clicks whose INP you want"),
            ("navigera -s s set cpu 4", "first, to measure as a mid-tier phone would"),
        ],
        &[],
    ),
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
        "http",
        &[
            ("navigera -s s http https://api.example.com/v1/me", ""),
            ("navigera -s s http POST https://api.example.com/hooks --json '{\"n\": 1}'", ""),
            ("navigera -s s http PUT https://example.com/profile --form name=Ada", ""),
        ],
        &[
            ("GET|POST|PUT|PATCH|DELETE|HEAD", "the method; GET, or POST with a body"),
            ("--json <body>", "a JSON body, sent as application/json"),
            ("--body <text>", "a body as given"),
            ("--form <name=value>", "a form field (repeat for more)"),
            ("--header 'Name: value'", "a request header (repeat for more)"),
            ("--max <bytes>", "the most of the answer's body to show; 16384"),
            ("--insecure", "accept any certificate"),
            ("--as-json", "the answer as JSON: status, body, cookies"),
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
            ("--values", "show HttpOnly cookies' values too (hidden by default)"),
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
            ("permissions grant|deny|reset <name>…", "geolocation, notifications, camera, microphone, clipboard (and midi, sensors, background-sync)"),
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
        &[
            ("navigera -s s route", ""),
            ("navigera -s s route \"*://*.example.com/*\" --abort", ""),
            ("navigera -s s route \"*/api/*\" --har api.har --not-found continue", ""),
            ("navigera -s s route \"*/api/me\" --modify --replace free pro", ""),
        ],
        &[
            ("<url glob>", "requests to match, with * and ? wildcards"),
            ("--abort", "fail matching requests (the default with no flag)"),
            ("--status <n>", "answer matching requests with this status"),
            ("--body <text>", "the body for --status answers"),
            ("--body-file <f>", "read the --status body from this file"),
            ("--content-type <t>", "the Content-Type for --status answers"),
            ("--header 'Name: value'", "an extra header for --status answers"),
            ("--har <f>", "answer from a HAR file (record one with network --har): same method and URL, entries in order, then the last"),
            ("--not-found abort|continue", "--har: what requests the HAR has no entry for do (default abort)"),
            ("--modify", "let matching requests reach the server and change their responses"),
            ("--set-header 'Name: value'", "--modify: set a response header ('Name:' removes it)"),
            ("--replace <from> <to>", "--modify: replace text in the response body (repeatable)"),
        ],
    ),
    (
        "unroute",
        &[("navigera -s s unroute \"*://*.example.com/*\"", ""), ("navigera -s s unroute", "")],
        &[("<url glob>", "the rule to remove; none removes all rules")],
    ),
    (
        "clock",
        &[
            ("navigera -s s clock install --time \"2024-02-02T08:00:00\" --paused", ""),
            ("navigera -s s clock run-for 30m", ""),
            ("navigera -s s clock pause-at \"2024-02-02T10:00:00\"", ""),
            ("navigera -s s clock resume", ""),
        ],
        &[
            ("install", "fake Date, performance.now and timers from now on, in every document of every tab"),
            ("--time <t>", "install: where the clock starts (2024-02-02T08:00:00, or epoch ms; default now)"),
            ("--paused", "install: the clock stands still until run-for, fast-forward or resume"),
            ("set <t>", "the date jumps to t; timers keep their delays and none fire"),
            ("run-for <length>", "advance (30s, 5m, 1h, mm:ss), firing each timer due on the way at its time, intervals as often as they come"),
            ("fast-forward <length>", "jump ahead, firing each timer that came due once"),
            ("pause-at <t>", "run to t, firing the timers on the way, and stand still there"),
            ("resume", "the clock flows with real time again"),
            ("uninstall", "documents opened from now on keep real time"),
        ],
    ),
    (
        "screencast",
        &[
            ("navigera -s s screencast start ./rec", ""),
            ("navigera -s s screencast stop", ""),
        ],
        &[
            ("start <dir>", "record from now on: a JPEG each time the page's pixels change, with its time"),
            ("--quality <0-100>", "JPEG quality (default 60)"),
            ("--size <w>x<h>", "the largest frame; the page is scaled down into it (default 1280x720)"),
            ("stop", "end it and write <dir>/index.html, which plays the frames at their pace"),
        ],
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
        &[("navigera -s s check --selector \"input[type=checkbox]\"", "")],
        &[("<target>", "the checkbox/radio/switch, by ref, --selector, …")],
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
    ("webmcp-list", &[("navigera -s s webmcp-list", "the tools the page offers an agent")], &[]),
    (
        "webmcp-call",
        &[
            ("navigera -s s webmcp-call search_flights --params '{\"to\":\"LIS\"}'", "a read-only tool"),
            ("navigera -s s webmcp-call add_to_cart --params '{\"sku\":\"A1\"}' --confirm", "one that changes things"),
        ],
        &[
            ("<name>", "the tool, as webmcp-list names it"),
            ("--params '{json}'", "its input, a JSON object (default {})"),
            ("--confirm", "needed for a tool the page doesn't mark read-only"),
        ],
    ),
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
    (
        "tab-list",
        &[("navigera -s s tab-list", "the session's tabs; in your own browser, your other tabs too (user_tabs)")],
        &[],
    ),
    (
        "show",
        &[
            ("navigera -s s show", ""),
            ("navigera -s me show", "in your own browser: the agent's tab, in front of you"),
        ],
        &[],
    ),
    ("hide", &[("navigera -s s hide", "")], &[]),
    (
        "tab-select",
        &[
            ("navigera -s s tab-select 0", ""),
            ("navigera -s me tab-select current", "the tab you are on: in your own browser, one of yours"),
        ],
        &[
            ("<index>", "the tab's index from tab-list"),
            ("<target id>", "the tab's id from tab-list (one of user_tabs too)"),
            ("current", "the tab you are on; quit leaves a tab of yours open"),
        ],
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
        &[
            ("--close-browser", "also close an attached/--profile browser (a shared one stays for its other sessions)"),
            ("--keep-tabs", "attached: leave navigera's own tabs open too (by default only yours stay)"),
        ],
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
            ("--auto-connect", "against whichever of your browsers serves remote debugging"),
            ("--tab current", "with --attach: in the tab you are on"),
        ],
    ),
    ("profiles", &[("navigera profiles", "")], &[]),
    (
        "totp",
        &[
            ("navigera totp SHOP_TOTP", "the code to type now; SHOP_TOTP holds the key or otpauth:// URI"),
            ("navigera -s s fill --label \"Authentication code\" \"$(navigera totp SHOP_TOTP)\"", "a sign-in's second step"),
        ],
        &[("<ENV_NAME>", "the environment variable with the secret: never the secret itself, which would land in the shell history")],
    ),
    (
        "batch",
        &[
            ("navigera -s s batch steps.txt", "goto, fill, click … in one call"),
            ("navigera -s s --raw batch -", "steps on stdin; print only what they read"),
            ("navigera -s s batch flow.txt --keep-going", "run every step even after a failure"),
        ],
        &[
            ("<file> | -", "the steps: one command per line, or JSON ops; - (or nothing) reads stdin"),
            ("--keep-going", "run the remaining steps after a failure"),
        ],
    ),
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
