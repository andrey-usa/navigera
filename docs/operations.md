# Matched operations (one call per op, warm session, median of 10)

Numbers: [bench run 45950542045](https://github.com/andrey-usa/navigera/actions/runs/45950542045).

One CLI process (or one MCP `tools/call`) per op, against a warm session,
on the same fixture page. `goto` re-navigates to the listing; `fill` fills
the search box; `click` clicks Search (client-side filter, no navigation);
`click-submit` clicks the order form's submit, which a script handles (no
navigation); `click-large` clicks a button on a large page (~60,000 elements,
40 open shadow roots; no navigation) that was loaded and clicked once before
(untimed), and `click-large-first` is the same click as the first action on the
freshly loaded page: a cost that grows with the page shows there;
`read-text` reads the heading; `snapshot` is each tool's default
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
| `goto` | 41.6 ms (1 call) | 55.0 ms (1 call) | 493.4 ms (1 call) | 143.1 ms (1 call) | 143.6 ms (1 call) | 511.2 ms (1 call) | 50.0 ms (1 call) | 49.7 ms (1 call) | 45.8 ms (1 call) | 41.6 ms (1 call) | 48.2 ms (1 call) | 41.0 ms (1 call) |
| `fill` | 5.4 ms (1 call) | 7.6 ms (1 call) | 365.2 ms (1 call) | 12.1 ms (1 call) | 232.1 ms (1 call) | 208.7 ms (1 call) | 9.0 ms (1 call) | 9.3 ms (1 call) | 8.9 ms (1 call) | 56.9 ms (1 call) | 24.0 ms (1 call) | 24.1 ms (1 call) |
| `click` | 9.4 ms (1 call) | 7.0 ms (1 call) | 921.9 ms (1 call) | 565.8 ms (1 call) | 241.8 ms (1 call) | 312.0 ms (1 call) | 9.5 ms (1 call) | 42.7 ms (1 call) | 43.9 ms (1 call) | 38.2 ms (1 call) | 64.8 ms (1 call) | 61.5 ms (1 call) |
| `click-submit` | 6.3 ms (1 call) | 5.7 ms (1 call) | 914.5 ms (1 call) | 557.4 ms (1 call) | 233.8 ms (1 call) | 237.1 ms (1 call) | 7.1 ms (1 call) | 28.8 ms (1 call) | 28.9 ms (1 call) | 28.4 ms (1 call) | 59.6 ms (1 call) | 59.0 ms (1 call) |
| `read-text` | 1.5 ms (1 call) | 2.7 ms (1 call) | 853.5 ms (1 call) | 504.1 ms (1 call) | 203.5 ms (1 call) | 182.2 ms (1 call) | 1.8 ms (1 call) | 0.9 ms (1 call) | 0.9 ms (1 call) | 0.8 ms (1 call) | 1.3 ms (1 call) | 0.9 ms (1 call) |
| `snapshot` | 9.3 ms (1 call) | 39.8 ms (1 call) | 388.5 ms (1 call) | 35.6 ms (1 call) | 32.6 ms (1 call) | 234.9 ms (1 call) | 14.5 ms (1 call) | 116.5 ms (1 call) | 100.2 ms (1 call) | not run: Puppeteer BiDi has no accessibility snapshot API | not run: WebdriverIO has no Firefox accessibility snapshot API | 14.1 ms (1 call) |
| `screenshot` | 41.5 ms (1 call, 1440x813, 57 KB) | 39.2 ms (1 call, 1280x633, 30 KB) | 400.1 ms (1 call, 1280x720, 32 KB) | 36.5 ms (1 call, 1280x720, 32 KB) | 72.6 ms (1 call, 1905x2053, 157 KB) | 219.8 ms (1 call, 780x437/780x493, 22 KB) | 15.6 ms (1 call, 1366x682, 47 KB) | 30.7 ms (1 call, 1440x900, 63 KB) | 30.1 ms (1 call, 1440x900, 63 KB) | 21.5 ms (1 call, 1440x900, 63 KB) | 18.9 ms (1 call, 1440x814, 56 KB) | 16.9 ms (1 call, 1366x682, 47 KB) |
| `screenshot-720` | 30.9 ms (1 call, 1280x720, 49 KB) | 40.4 ms (1 call, 1280x720, 32 KB) | 402.4 ms (1 call, 1280x720, 32 KB) | 40.7 ms (1 call, 1280x720, 32 KB) | 21.2 ms (1 call, 1280x720, 49 KB) | 223.4 ms (1 call, 1280x720, 33 KB) | 15.5 ms (1 call, 1280x720, 48 KB) | 27.0 ms (1 call, 1280x720, 48 KB) | 27.5 ms (1 call, 1280x720, 48 KB) | 14.7 ms (1 call, 1280x720, 48 KB) | 16.2 ms (1 call, 1280x720, 48 KB) | 17.3 ms (1 call, 1280x720, 48 KB) |
| `form-batch` | 28.9 ms (1 call) | 27.2 ms (1 call) | 2495.3 ms (4 calls) | 1098.2 ms (4 calls) | 879.8 ms (4 calls) | 265.1 ms (1 call) | 10.0 ms (1 call) | 81.1 ms (4 calls) | 70.1 ms (4 calls) | 68.7 ms (4 calls) | 88.4 ms (4 calls) | 70.5 ms (4 calls) |
| `click-large` | 67.2 ms (1 call) | 91.8 ms (1 call) | 2708.7 ms (1 call) | 2344.2 ms (1 call) | 331.2 ms (1 call) | 21117.5 ms (1 call) | 11.3 ms (1 call) | 97.3 ms (1 call) | 97.1 ms (1 call) | 31.4 ms (1 call) | 65.0 ms (1 call) | 62.9 ms (1 call) |
| `click-large-first` | 92.8 ms (1 call) | 98.4 ms (1 call) | 3159.6 ms (1 call) | 2425.7 ms (1 call) | 340.3 ms (1 call) | 21946.3 ms (1 call) | 14.8 ms (1 call) | 172.4 ms (1 call) | 171.3 ms (1 call) | 42.2 ms (1 call) | 69.7 ms (1 call) | 65.9 ms (1 call) |

The KB in a screenshot cell is the median PNG file size over its reps (1 KB =
1,024 bytes): a tool may trade bytes for time, a faster encoding writing a larger file.

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
