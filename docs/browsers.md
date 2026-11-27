# Supported browsers

CI runs the full end-to-end suite on:

- the current **Chrome Stable** and **the three milestones before it**
  (about four months of releases, covering Extended Stable), all from Chrome
  for Testing;
- **chrome-headless-shell**;
- **Chrome Beta**, as a non-blocking early warning;
- the runner's own Chrome, on Linux, **macOS** and **Windows**.

The suite is weekly as well as on every push, so a new Chrome release is
caught before agents hit it. In its first runs the matrix caught two real
bugs that the runner's Chrome alone missed: Chrome 151 segfaulted with one of
the automation flags (`--disable-features=OptimizationHints`, bisected with
`chrome-bisect.yml`), and on Chrome 153 an Enter-submitted form returned
before its redirect landed. Both are fixed.

Each run also checks **CDP protocol correctness**. navigera records
every command it sends (`NAVIGERA_CDP_TRACE`), and
[`tools/cdp_check.py`](../tools/cdp_check.py) validates every method, parameter
and enum value against the `/json/protocol` that *that* browser serves.
Deprecated or unknown usage fails the build. Edge and Brave are covered by
the benchmark.

**Firefox, Safari and WebDriver servers** (experimental) run through the
two W3C WebDriver protocols: `--engine firefox` starts Firefox and drives it
over WebDriver BiDi (`--attach firefox` works in one you started yourself
with `firefox --remote-debugging-port`, in a window of its own, and leaves
it running), `--engine safari` (or `safari-preview`) starts
safaridriver on macOS, and `--webdriver <url>` drives whatever browser a
WebDriver server serves: geckodriver, chromedriver, msedgedriver, a Selenium
Grid, a Mac's safaridriver. `--capabilities <json>` adds to the session's
capabilities (Safari on the iOS Simulator:
`{"platformName":"iOS","safari:useSimulator":true}`), and `--bidi` runs a
server's session over its BiDi endpoint. `ax` reads the page in the page's
own JavaScript and prints the same tree as on Chrome; locators, `fill` and
`select` run the same page code. The core ops work: goto, ax, click, fill,
select, check, press, type, text, eval, wait, screenshot, pdf, tabs, cookies,
console and dialogs, and upload, drag, mouse, keydown, keyup, storage and
`set viewport`. Over BiDi also `init-script`, and `route` and `unroute`
(answer, abort or replay a request from a HAR; `--modify` is Chrome's),
`downloads` and `wait --download`, `network` (the requests of every tab
of the session from its first `network` on, as a browser listened to
slows every load: run `network` once before the load you want to see;
the list, a response body, `--har` with the bodies the browser kept, up
to 10 MiB each; chromedriver 157 sends a request body that is not UTF-8
as its base64 labelled as text, which no client can tell from a text body
that reads the same, so navigera shows that text; Firefox labels it
base64),
`screencast` (screenshots of the
viewport on the session's connection while commands run and between them,
a frame for each change, at most every 100 ms and one a second on a still
page),
and `set locale`, `timezone`, `media dark|light`, `ua`, `offline`, `geo`
and `device` for every tab of the session (Firefox 160 emulates no other
media feature; `device` is the viewport, pixel ratio and user agent, no
touch; `geo` grants the permission to each origin the tab visits, and takes
it back at `set geo off`).
An op only Chrome has (`cdp`, `passkey`, throttling; `network` over
classic WebDriver) says to
use `--engine chrome`; `screencast` over classic WebDriver says to add
`--bidi`. Safari's driver has no
headless mode, runs one session per machine and blocks user input while it
runs (Apple's limits). The `webdriver` workflow runs one scenario on Firefox
(Linux, macOS, Windows), Safari (macOS) and chromedriver over both protocols.

**Lightpanda** 1.0 (`--engine lightpanda`) runs its own end-to-end tests and
its leg of the commands suite in CI on every push, on its newest stable
release (never the nightly). It is a different engine (one tab, no layout),
so navigera fills in what its CDP leaves out: shadow roots and password
fields in `ax`, `confirm()`/`prompt()` answers, navigations a click starts.
The differences that remain are listed in
[docs/navigera.md](navigera.md#lightpanda-and-your-own-browser).

**Obscura** (`--engine obscura`) and **Moli** (`--engine moli`), both
experimental, are newer headless engines that are their own CDP server,
like Lightpanda; navigera starts each as `<engine> serve`. Unlike
Lightpanda they open tabs and lay pages out, so they take Chrome's paths.
[`tests/cdp_servers.rs`](../tests/cdp_servers.rs) checks each one:
navigation, the accessibility tree and acting on its refs, fill, a form
submitted with the keyboard (the server checks the sign-in), a checkbox
click, a select, tabs, a screenshot, and the engine's server gone after
`quit`. CI runs it on Moli's newest stable release on every push (1.1.28
at this writing); on Obscura it last ran on 0.2.7
([run 45312041662](https://github.com/andrey-usa/navigera/actions/runs/45312041662)).
Run with either
one as the default engine (`NAVIGERA_ENGINE`), the rest of the suite shows
how far each is from Chrome; the differences agents meet are listed in
[docs/navigera.md](navigera.md#obscura-and-moli). Moli's layout now stays
current (`--realtime`, 1.1.28+), so content a page's scripts render later
has a box to click at once; 1.1.28 is the floor for live layout, and older
Moli still works — navigera starts it with `--layout` only and refreshes
layout with a 1x1 screenshot. What remains is mostly surface: refs are
Moli's large node ids, a `<select>`'s value in `ax` is the selected
option's value rather than its label, and a few roles differ from
Chrome's.
