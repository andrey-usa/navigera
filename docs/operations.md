# Matched operations (one call per op, warm session, median of 10)

Numbers: [bench run 46044180770](https://github.com/andrey-usa/navigera/actions/runs/46044180770).

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

Each cell gives the median of the 10 reps (the mean too where it ranks; every
rep's time is in publish.json) and its calls where there were more than one. Chrome answers a
capture on its 60 Hz frame grid, in one frame (~17 ms) or two (~31 ms) by the
moment the request lands, so each rest before a timed call is lengthened by an
offset in [0, 1/60 s), one seeded sequence that every tool shares
(seed 46044180770, in publish.json): every tool meets the same moments. A
screenshot cell adds its one-frame share (reps under 24 ms), and `screenshot-720`
is ranked by its mean, the time expected over the frame grid; the other ops by
the median.

| op | `navigera` (this repo) | `agent-browser` 0.45.0 (Vercel Labs, Rust) | `playwright-cli` 0.1.25 (Microsoft) | Playwright MCP 0.0.86 (Microsoft) | Chrome DevTools MCP 1.14.0 (Google) | `browser-use` 1.0.9 (exec model) | navigera Firefox | Playwright Firefox Node API | Playwright Firefox Bun API | Puppeteer Firefox BiDi API | WebdriverIO Firefox BiDi API | Vibium Firefox API |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `goto` | 23.3 ms | 32.3 ms | 354.4 ms | 95.2 ms | 131.9 ms | 164.7 ms | 47.3 ms | 27.9 ms | 31.1 ms | 38.5 ms | 36.1 ms | 30.2 ms |
| `fill` | 5.8 ms | 6.5 ms | 263.6 ms | 11.5 ms | 223.7 ms | 159.8 ms | 5.2 ms | 9.0 ms | 9.8 ms | 34.9 ms | 14.2 ms | 8.8 ms |
| `click` | 8.8 ms | 6.5 ms | 811.4 ms | 554.7 ms | 238.5 ms | 230.9 ms | 10.2 ms | 41.7 ms | 38.9 ms | 35.5 ms | 63.0 ms | 63.8 ms |
| `click-submit` | 5.4 ms | 5.5 ms | 803.8 ms | 548.9 ms | 241.6 ms | 181.4 ms | 8.0 ms | 38.4 ms | 35.9 ms | 25.8 ms | 61.2 ms | 61.7 ms |
| `read-text` | 1.3 ms | 2.1 ms | 756.5 ms | 503.9 ms | 205.2 ms | 139.5 ms | 1.6 ms | 1.4 ms | 1.2 ms | 1.1 ms | 1.8 ms | 1.5 ms |
| `snapshot` | 6.9 ms | 26.1 ms | 280.3 ms | 26.0 ms | 24.6 ms | 179.4 ms | 11.2 ms | 60.5 ms | 58.5 ms | not run: Puppeteer BiDi has no accessibility snapshot API | not run: WebdriverIO has no Firefox accessibility snapshot API | 9.4 ms |
| `screenshot` | 32.3 ms (1440x813, 57 KB, one frame 0/10) | 32.4 ms (1280x633, 30 KB, one frame 0/10) | 284.1 ms (1280x720, 32 KB, one frame 0/10) | 30.4 ms (1280x720, 32 KB, one frame 0/10) | 49.4 ms (1905x2053, 157 KB, one frame 0/10) | 183.3 ms (780x437/780x493, 21 KB, one frame 0/10) | 11.1 ms (1366x682, 47 KB, one frame 10/10) | 22.9 ms (1440x900, 63 KB, one frame 8/10) | 22.5 ms (1440x900, 63 KB, one frame 8/10) | 14.0 ms (1440x900, 63 KB, one frame 10/10) | 14.3 ms (1440x814, 56 KB, one frame 10/10) | 12.0 ms (1366x682, 47 KB, one frame 10/10) |
| `screenshot-720` | 22.3 ms, mean 22.0 (1280x720, 49 KB, one frame 6/10) | 30.3 ms, mean 30.3 (1280x720, 32 KB, one frame 1/10) | 286.4 ms, mean 285.5 (1280x720, 32 KB, one frame 0/10) | 32.0 ms, mean 31.9 (1280x720, 32 KB, one frame 0/10) | 22.6 ms, mean 22.0 (1280x720, 49 KB, one frame 6/10) | 171.8 ms, mean 173.9 (1280x720, 33 KB, one frame 0/10) | 12.8 ms, mean 13.1 (1280x720, 48 KB, one frame 10/10) | 21.0 ms, mean 23.7 (1280x720, 48 KB, one frame 7/10) | 21.9 ms, mean 23.7 (1280x720, 48 KB, one frame 7/10) | 12.5 ms, mean 12.8 (1280x720, 48 KB, one frame 10/10) | 14.2 ms, mean 14.3 (1280x720, 48 KB, one frame 10/10) | 14.0 ms, mean 14.0 (1280x720, 48 KB, one frame 10/10) |
| `form-batch` | 23.5 ms | 20.6 ms | 2095.0 ms (4 calls) | 1083.3 ms (4 calls) | 891.1 ms (4 calls) | 204.3 ms | 11.1 ms | 72.2 ms (4 calls) | 67.3 ms (4 calls) | 64.3 ms (4 calls) | 86.8 ms (4 calls) | 72.2 ms (4 calls) |
| `click-large` | 46.1 ms | 82.3 ms | 2209.7 ms | 1954.2 ms | 318.4 ms | 14526.2 ms | 12.6 ms | 83.8 ms | 81.9 ms | 30.8 ms | 68.0 ms | 65.7 ms |
| `click-large-first` | 64.4 ms | 90.3 ms | 2427.2 ms | 1998.4 ms | 335.3 ms | 15002.2 ms | 15.2 ms | 132.7 ms | 130.4 ms | 36.2 ms | 70.7 ms | 68.5 ms |

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
