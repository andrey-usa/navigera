# Hand the agent your browser

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
Details: [docs/navigera.md](../navigera.md#browsers-and-engines).
