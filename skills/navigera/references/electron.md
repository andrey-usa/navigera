# Electron apps

```bash
navigera -s app start --electron ./my-app   # app directory or main script (Electron from its node_modules), a packaged app's executable, or a .app bundle
navigera -s app ax                          # the window it opened first, read like a page
navigera -s app tab-list                    # each window is a tab (a <webview> too, "webview": true); one the app opens becomes the active tab
navigera -s app eval --main "({ app }) => app.getName()"   # the app's main process (a function gets the electron module)
navigera -s app quit                        # ends the app
```

`--electron-arg <arg>` (repeatable) passes arguments to the app. Every other
command works as on a page. `tab-new`, `reset`, `show` and `hide` don't: the
app owns its windows and storage. `start` waits for a window the app shows to
have content and says `electron: {version, main_process}`; an app that is
still starting may need `wait --text <word>` before `ax`. On a Linux server,
run the whole session inside one `xvfb-run` (one per command kills the app).
An app already running with `--remote-debugging-port=9222`: `start --attach
9222` drives its windows the same way; `quit` leaves it running.
