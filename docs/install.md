# Install

## Platforms

Linux, macOS and Windows are supported. On Windows, the session socket is
a loopback TCP port; clients must present a random token from the session
file in your temp directory. CDP runs over a DevTools WebSocket, and
Chrome sits in a kill-on-close job object, so Chrome exits when navigera
does.

## Which browser navigera uses

navigera finds Chrome or Chromium on its own: a system install,
`$CHROME_BIN`, or a Playwright/Puppeteer browser cache. It uses a system
Chrome first; with none installed it picks chrome-headless-shell (or
Chromium) from a Playwright or Puppeteer cache. Set `$CHROME_BIN` to choose
explicitly.

For the fastest cold start, use
[chrome-headless-shell](https://developer.chrome.com/blog/chrome-headless-shell),
the same Chrome without its browser UI layer:

```sh
navigera install-browser chrome-headless-shell   # from Google's Chrome for Testing, used from then on
```

What it saves, measured in CI, is the chrome-headless-shell line of the
[benchmarks](benchmarks.md).
