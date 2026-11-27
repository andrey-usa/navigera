# Matched operations (one call per op, warm session, median of 10)

Numbers: [bench run 45918635350](https://github.com/andrey-usa/navigera/actions/runs/45918635350).

One CLI process (or one MCP `tools/call`) per op, against a warm session,
on the same fixture page. `goto` re-navigates to the listing; `fill` fills
the search box; `click` clicks Search (client-side filter, no navigation);
`click-submit` clicks the order form's submit, which a script handles (no
navigation); `read-text` reads the heading; `snapshot` is each tool's default
accessibility output; `screenshot` writes a PNG at the tool's own default
viewport (the PNG's size in the cell); `screenshot-720` is the same capture after
the tool's own resize command set a 1280x720 viewport (untimed, undone after), so
every tool captures the same pixels; `form-batch` fills two fields,
clicks submit and reads the status — one call for tools with a batch command,
four calls otherwise. Two rounds of 5 reps: rounds are the outer loop and tools
the inner loop, with the second round in reversed tool order. Each tool runs in
its default output mode (MCP tools over JSON-RPC, their only interface); no tool
is switched into a machine mode. A cell is complete only with all 10 reps and no
round error; a partial cell is shown as `incomplete (n/N)` and left out of the
per-op losses, which rank `screenshot-720`, not the default-viewport row.

| op | `navigera` (this repo) | `agent-browser` 0.45.0 (Vercel Labs, Rust) | `playwright-cli` 0.1.25 (Microsoft) | Playwright MCP 0.0.86 (Microsoft) | Chrome DevTools MCP 1.14.0 (Google) | `browser-use` 1.0.9 (exec model) | navigera Firefox | Playwright Firefox Node API | Playwright Firefox Bun API | Puppeteer Firefox BiDi API | WebdriverIO Firefox BiDi API | Vibium Firefox API |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `goto` | 24.6 ms (1 call) | 31.4 ms (1 call) | 351.0 ms (1 call) | 97.0 ms (1 call) | 132.7 ms (1 call) | 456.3 ms (1 call) | 31.8 ms (1 call) | 34.2 ms (1 call) | 31.8 ms (1 call) | 24.3 ms (1 call) | 49.5 ms (1 call) | 30.4 ms (1 call) |
| `fill` | 5.4 ms (1 call) | 6.6 ms (1 call) | 264.4 ms (1 call) | 11.6 ms (1 call) | 231.5 ms (1 call) | 160.9 ms (1 call) | 11.4 ms (1 call) | 9.4 ms (1 call) | 10.1 ms (1 call) | 36.2 ms (1 call) | 14.1 ms (1 call) | 15.3 ms (1 call) |
| `click` | 9.8 ms (1 call) | 6.3 ms (1 call) | 813.6 ms (1 call) | 557.2 ms (1 call) | 238.8 ms (1 call) | 232.2 ms (1 call) | 9.9 ms (1 call) | 46.7 ms (1 call) | 45.1 ms (1 call) | 41.0 ms (1 call) | 63.0 ms (1 call) | 63.1 ms (1 call) |
| `click-submit` | 6.1 ms (1 call) | 5.5 ms (1 call) | 806.6 ms (1 call) | 551.7 ms (1 call) | 235.1 ms (1 call) | 174.9 ms (1 call) | 8.6 ms (1 call) | 29.1 ms (1 call) | 29.2 ms (1 call) | 30.0 ms (1 call) | 61.1 ms (1 call) | 60.9 ms (1 call) |
| `read-text` | 1.0 ms (1 call) | 1.9 ms (1 call) | 760.5 ms (1 call) | 504.2 ms (1 call) | 204.6 ms (1 call) | 134.3 ms (1 call) | 1.3 ms (1 call) | 0.8 ms (1 call) | 0.8 ms (1 call) | 0.7 ms (1 call) | 1.3 ms (1 call) | 0.8 ms (1 call) |
| `snapshot` | 6.8 ms (1 call) | 26.0 ms (1 call) | 281.1 ms (1 call) | 23.9 ms (1 call) | 23.7 ms (1 call) | 176.1 ms (1 call) | 11.1 ms (1 call) | 64.9 ms (1 call) | 62.9 ms (1 call) | not run: Puppeteer BiDi has no accessibility snapshot API | not run: WebdriverIO has no Firefox accessibility snapshot API | 9.1 ms (1 call) |
| `screenshot` | 43.1 ms (1 call, 1440x813) | 44.0 ms (1 call, 1280x633) | 286.7 ms (1 call, 1280x720) | 25.8 ms (1 call, 1280x720) | 45.4 ms (1 call, 1905x2053) | 165.6 ms (1 call, 780x437/780x493) | 10.4 ms (1 call, 1366x682) | 21.7 ms (1 call, 1440x900) | 21.8 ms (1 call, 1440x900) | 14.1 ms (1 call, 1440x900) | 14.1 ms (1 call, 1440x814) | 11.6 ms (1 call, 1366x682) |
| `screenshot-720` | 17.4 ms (1 call, 1280x720) | 31.9 ms (1 call, 1280x720) | 281.8 ms (1 call, 1280x720) | 29.5 ms (1 call, 1280x720) | 18.7 ms (1 call, 1280x720) | 171.7 ms (1 call, 1280x720) | 12.6 ms (1 call, 1280x720) | 21.9 ms (1 call, 1280x720) | 22.8 ms (1 call, 1280x720) | 12.1 ms (1 call, 1280x720) | 13.2 ms (1 call, 1280x720) | 13.5 ms (1 call, 1280x720) |
| `form-batch` | 29.5 ms (1 call) | 28.3 ms (1 call) | 2076.2 ms (4 calls) | 1085.2 ms (4 calls) | 889.9 ms (4 calls) | 203.1 ms (1 call) | 11.4 ms (1 call) | 66.8 ms (4 calls) | 66.2 ms (4 calls) | 64.8 ms (4 calls) | 83.8 ms (4 calls) | 72.5 ms (4 calls) |

Firefox API arms use persistent library sessions, with the JSON-line control pipe
included in wall time. They are not CLI-per-op timings. Playwright Node/Bun use
its bundled patched Firefox; Puppeteer/WebdriverIO use the runner Firefox;
Vibium uses its managed Firefox. Browser builds are recorded in publish.json.
Puppeteer disables its extra bounding-box stability wait on static fixtures;
visibility/enabled checks remain. API form-batch has four calls in one request.

Snapshot outputs differ in form (each tool's default tree), so that row compares
call latency, not equivalent work. The browser-use snapshot comes from its
documented raw `cdp()` route; it has no built-in snapshot command. Per-call time
includes process start (CLI) or JSON-RPC (MCP) and the daemon hop. An `incomplete
(n/N)` cell has only n of the 10 expected reps; its median is partial evidence.
