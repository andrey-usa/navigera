# Shell commands, not a wrapper script

Don't write a Python/Node wrapper around `navigera serve`: a direct call
costs ~3 ms more than a pipe, a wrapper costs model turns to write and debug,
and a script that starts its own `serve` relaunches the browser (losing its
cookies) on every run. Data from many elements: one `eval` returning JSON.
The same commands over a list of URLs: a shell loop. `navigera serve` (JSON
lines on stdin, docs/navigera.md) is for hundreds of steps with no decision
between them, from a program you are already writing. If a command fails,
read its error and fix the command.

## The user's own browser (their logins and settings)

```bash
navigera profiles                     # the user's browsers and profiles that can be handed over
navigera -s me start --auto-connect   # their running browser (Chrome 144+: they turn on chrome://inspect/#remote-debugging once, then click Allow); same as --attach
navigera -s me tab-list               # your tabs, then theirs under user_tabs ("current": true is the one they are on)
navigera -s me tab-select current     # work in the tab they are on (or a target id from user_tabs)
navigera -s me start --auto-connect --tab current   # start in their tab, with no window of your own
navigera -s me start --profile agent --from chrome   # a browser of your own, starting with a copy of their sign-ins
navigera -s me start --profile work   # a separate persistent browser: sign in once, it stays open between sessions
navigera -s me show                   # bring your tab to the front for them: it is theirs now, and quit leaves it open
navigera -s me quit                   # closes your own tabs, leaves theirs (--keep-tabs leaves yours too; --close-browser closes the browser)
```

In their browser navigera opens a window of its own behind theirs, and
never brings a tab forward by itself, so their keyboard and view stay where
they are; a tab behind another still reads and takes clicks and typing.
When they say "this page" or "my tab", `tab-select current` works in the tab
they are on: it stays theirs, and `quit` leaves it open. To hand them
something you opened, `show` it. `start --natural` helps when a site treats headless browsers
differently. A step only the person can do (a login, 2FA, a CAPTCHA):
`navigera -s me show` puts the browser on their screen with its tabs and
sign-ins; tell them what to do, `wait --url /dashboard --timeout-ms
300000`, then `hide` and carry on.

## A browser in the cloud

Its service hands out a `wss://` endpoint, usually with the key in the
query. Have it in `$NAVIGERA_ATTACH` (the user's environment or the MCP
config's `env`), then `navigera -s me start`: the key stays off command
lines and out of your context (navigera prints it as `***`). Never echo the
variable or paste the key into a command. `$HTTPS_PROXY` is used, as by curl.

## Sessions

- The browser, its tabs and cookies persist between commands of a session.
  `-s` is short for `--session`; `$NAVIGERA_SESSION` sets a default. An idle
  session shuts down after 10 min (`start --idle <s>`); `start --parent <pid>`
  ends it when that process (your agent) exits.
- Several agents at once: give each its own `-s` name. Sessions share one
  browser, each in its own context (cookies, storage, tabs apart); two
  agents on one name would drive one browser. `navigera sessions` lists them
  with their memory; `sessions --clean` quits the idle ones.
