# Matched operations (one call per op, warm session, median of 10)

Numbers: [bench run 45774240157](https://github.com/andrey-usa/navigera/actions/runs/45774240157).

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

| op | `navigera` (this repo) | `agent-browser` 0.45.0 (Vercel Labs, Rust) | `playwright-cli` 0.1.25 (Microsoft) | Playwright MCP 0.0.86 (Microsoft) | Chrome DevTools MCP 1.14.0 (Google) | `browser-use` 1.0.8 (exec model) | navigera Firefox | Playwright Firefox Node API | Playwright Firefox Bun API | Puppeteer Firefox BiDi API | WebdriverIO Firefox BiDi API | Vibium Firefox API |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `goto` | 33.7 ms (1 call) | 42.0 ms (1 call) | 398.1 ms (1 call) | 109.6 ms (1 call) | 135.7 ms (1 call) | 337.8 ms (1 call) | 46.0 ms (1 call) | 34.5 ms (1 call) | 42.9 ms (1 call) | 35.2 ms (1 call) | 44.0 ms (1 call) | 34.7 ms (1 call) |
| `fill` | 6.7 ms (1 call) | 6.7 ms (1 call) | 295.0 ms (1 call) | 10.8 ms (1 call) | 226.9 ms (1 call) | 171.6 ms (1 call) | 8.2 ms (1 call) | 9.0 ms (1 call) | 8.9 ms (1 call) | 45.7 ms (1 call) | 26.7 ms (1 call) | 17.9 ms (1 call) |
| `click` | 12.2 ms (1 call) | 6.5 ms (1 call) | 833.4 ms (1 call) | 559.7 ms (1 call) | 246.7 ms (1 call) | 262.4 ms (1 call) | 11.8 ms (1 call) | 33.5 ms (1 call) | 35.2 ms (1 call) | 41.6 ms (1 call) | 63.3 ms (1 call) | 62.6 ms (1 call) |
| `click-submit` | 8.6 ms (1 call) | 5.4 ms (1 call) | 833.5 ms (1 call) | 552.1 ms (1 call) | 238.5 ms (1 call) | 199.1 ms (1 call) | 8.8 ms (1 call) | 29.2 ms (1 call) | 29.7 ms (1 call) | 13.7 ms (1 call) | 59.9 ms (1 call) | 59.3 ms (1 call) |
| `read-text` | 1.6 ms (1 call) | 2.2 ms (1 call) | 784.7 ms (1 call) | 504.1 ms (1 call) | 204.1 ms (1 call) | 158.6 ms (1 call) | 1.9 ms (1 call) | 0.8 ms (1 call) | 0.8 ms (1 call) | 0.6 ms (1 call) | 1.2 ms (1 call) | 0.7 ms (1 call) |
| `snapshot` | 10.7 ms (1 call) | 31.4 ms (1 call) | 307.5 ms (1 call) | 30.5 ms (1 call) | 27.4 ms (1 call) | 199.9 ms (1 call) | 17.6 ms (1 call) | 78.0 ms (1 call) | 76.6 ms (1 call) | not run: Puppeteer BiDi has no accessibility snapshot API | not run: WebdriverIO has no Firefox accessibility snapshot API | 11.7 ms (1 call) |
| `screenshot` | 36.0 ms (1 call, 1440x813) | 41.1 ms (1 call, 1280x633) | 316.4 ms (1 call, 1280x720) | 38.6 ms (1 call, 1280x720) | 62.9 ms (1 call, 1905x2053) | 277.8 ms (1 call, 780x437/780x493) | 15.1 ms (1 call, 1366x682) | 27.3 ms (1 call, 1440x900) | 26.7 ms (1 call, 1440x900) | 16.7 ms (1 call, 1440x900) | 16.8 ms (1 call, 1440x814) | 14.1 ms (1 call, 1366x682) |
| `screenshot-720` | 34.3 ms (1 call, 1280x720) | 27.7 ms (1 call, 1280x720) | 316.2 ms (1 call, 1280x720) | 30.6 ms (1 call, 1280x720) | 20.7 ms (1 call, 1280x720) | 180.6 ms (1 call, 1280x720) | 16.2 ms (1 call, 1280x720) | 22.9 ms (1 call, 1280x720) | 24.8 ms (1 call, 1280x720) | 13.2 ms (1 call, 1280x720) | 14.4 ms (1 call, 1280x720) | 15.0 ms (1 call, 1280x720) |
| `form-batch` | 26.9 ms (1 call) | 28.2 ms (1 call) | 2225.3 ms (4 calls) | 1092.5 ms (4 calls) | 886.7 ms (4 calls) | 220.4 ms (1 call) | 12.2 ms (1 call) | 65.1 ms (4 calls) | 65.3 ms (4 calls) | 55.6 ms (4 calls) | 82.9 ms (4 calls) | 69.6 ms (4 calls) |

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
