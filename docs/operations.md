# Matched operations (one call per op, warm session, median of 10, mean beside it)

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

Each cell gives the median of the 10 reps, then their mean. Chrome answers a
capture on its 60 Hz frame grid, in one frame (~17 ms) or two (~31 ms) by the
moment the request lands, so each rest before a timed call is lengthened by an
offset in [0, 1/60 s), one seeded sequence that every tool shares
(seed 46044180770, in publish.json): every tool meets the same moments. A
screenshot cell adds its one-frame share (reps under 24 ms), and `screenshot-720`
is ranked by its mean, the time expected over the frame grid; the other ops by
the median.

| op | `navigera` (this repo) | `agent-browser` 0.45.0 (Vercel Labs, Rust) | `playwright-cli` 0.1.25 (Microsoft) | Playwright MCP 0.0.86 (Microsoft) | Chrome DevTools MCP 1.14.0 (Google) | `browser-use` 1.0.9 (exec model) | navigera Firefox | Playwright Firefox Node API | Playwright Firefox Bun API | Puppeteer Firefox BiDi API | WebdriverIO Firefox BiDi API | Vibium Firefox API |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `goto` | 23.3 ms, mean 26.2 (1 call) | 32.3 ms, mean 34.0 (1 call) | 354.4 ms, mean 356.9 (1 call) | 95.2 ms, mean 102.1 (1 call) | 131.9 ms, mean 132.0 (1 call) | 164.7 ms, mean 279.4 (1 call) | 47.3 ms, mean 55.5 (1 call) | 27.9 ms, mean 31.2 (1 call) | 31.1 ms, mean 39.7 (1 call) | 38.5 ms, mean 46.9 (1 call) | 36.1 ms, mean 37.8 (1 call) | 30.2 ms, mean 34.7 (1 call) |
| `fill` | 5.8 ms, mean 6.7 (1 call) | 6.5 ms, mean 9.1 (1 call) | 263.6 ms, mean 267.0 (1 call) | 11.5 ms, mean 14.6 (1 call) | 223.7 ms, mean 227.7 (1 call) | 159.8 ms, mean 163.8 (1 call) | 5.2 ms, mean 7.3 (1 call) | 9.0 ms, mean 17.9 (1 call) | 9.8 ms, mean 20.5 (1 call) | 34.9 ms, mean 48.7 (1 call) | 14.2 ms, mean 32.4 (1 call) | 8.8 ms, mean 19.5 (1 call) |
| `click` | 8.8 ms, mean 9.7 (1 call) | 6.5 ms, mean 7.0 (1 call) | 811.4 ms, mean 811.1 (1 call) | 554.7 ms, mean 554.6 (1 call) | 238.5 ms, mean 242.0 (1 call) | 230.9 ms, mean 232.1 (1 call) | 10.2 ms, mean 13.3 (1 call) | 41.7 ms, mean 41.0 (1 call) | 38.9 ms, mean 40.4 (1 call) | 35.5 ms, mean 34.4 (1 call) | 63.0 ms, mean 65.2 (1 call) | 63.8 ms, mean 66.8 (1 call) |
| `click-submit` | 5.4 ms, mean 5.8 (1 call) | 5.5 ms, mean 5.7 (1 call) | 803.8 ms, mean 805.0 (1 call) | 548.9 ms, mean 548.1 (1 call) | 241.6 ms, mean 240.2 (1 call) | 181.4 ms, mean 181.9 (1 call) | 8.0 ms, mean 9.3 (1 call) | 38.4 ms, mean 36.3 (1 call) | 35.9 ms, mean 34.8 (1 call) | 25.8 ms, mean 27.0 (1 call) | 61.2 ms, mean 61.3 (1 call) | 61.7 ms, mean 61.6 (1 call) |
| `read-text` | 1.3 ms, mean 1.3 (1 call) | 2.1 ms, mean 2.1 (1 call) | 756.5 ms, mean 757.1 (1 call) | 503.9 ms, mean 504.1 (1 call) | 205.2 ms, mean 205.0 (1 call) | 139.5 ms, mean 139.9 (1 call) | 1.6 ms, mean 1.6 (1 call) | 1.4 ms, mean 1.4 (1 call) | 1.2 ms, mean 1.4 (1 call) | 1.1 ms, mean 1.2 (1 call) | 1.8 ms, mean 1.9 (1 call) | 1.5 ms, mean 1.4 (1 call) |
| `snapshot` | 6.9 ms, mean 8.4 (1 call) | 26.1 ms, mean 27.2 (1 call) | 280.3 ms, mean 279.9 (1 call) | 26.0 ms, mean 26.4 (1 call) | 24.6 ms, mean 24.4 (1 call) | 179.4 ms, mean 179.7 (1 call) | 11.2 ms, mean 13.2 (1 call) | 60.5 ms, mean 63.0 (1 call) | 58.5 ms, mean 61.0 (1 call) | not run: Puppeteer BiDi has no accessibility snapshot API | not run: WebdriverIO has no Firefox accessibility snapshot API | 9.4 ms, mean 9.8 (1 call) |
| `screenshot` | 32.3 ms, mean 33.7 (1 call, 1440x813, 57 KB, one frame 0/10) | 32.4 ms, mean 34.4 (1 call, 1280x633, 30 KB, one frame 0/10) | 284.1 ms, mean 283.9 (1 call, 1280x720, 32 KB, one frame 0/10) | 30.4 ms, mean 32.7 (1 call, 1280x720, 32 KB, one frame 0/10) | 49.4 ms, mean 51.0 (1 call, 1905x2053, 157 KB, one frame 0/10) | 183.3 ms, mean 2989.9 (1 call, 780x437/780x493, 21 KB, one frame 0/10) | 11.1 ms, mean 11.8 (1 call, 1366x682, 47 KB, one frame 10/10) | 22.9 ms, mean 22.9 (1 call, 1440x900, 63 KB, one frame 8/10) | 22.5 ms, mean 22.7 (1 call, 1440x900, 63 KB, one frame 8/10) | 14.0 ms, mean 15.1 (1 call, 1440x900, 63 KB, one frame 10/10) | 14.3 ms, mean 15.0 (1 call, 1440x814, 56 KB, one frame 10/10) | 12.0 ms, mean 12.9 (1 call, 1366x682, 47 KB, one frame 10/10) |
| `screenshot-720` | 22.3 ms, mean 22.0 (1 call, 1280x720, 49 KB, one frame 6/10) | 30.3 ms, mean 30.3 (1 call, 1280x720, 32 KB, one frame 1/10) | 286.4 ms, mean 285.5 (1 call, 1280x720, 32 KB, one frame 0/10) | 32.0 ms, mean 31.9 (1 call, 1280x720, 32 KB, one frame 0/10) | 22.6 ms, mean 22.0 (1 call, 1280x720, 49 KB, one frame 6/10) | 171.8 ms, mean 173.9 (1 call, 1280x720, 33 KB, one frame 0/10) | 12.8 ms, mean 13.1 (1 call, 1280x720, 48 KB, one frame 10/10) | 21.0 ms, mean 23.7 (1 call, 1280x720, 48 KB, one frame 7/10) | 21.9 ms, mean 23.7 (1 call, 1280x720, 48 KB, one frame 7/10) | 12.5 ms, mean 12.8 (1 call, 1280x720, 48 KB, one frame 10/10) | 14.2 ms, mean 14.3 (1 call, 1280x720, 48 KB, one frame 10/10) | 14.0 ms, mean 14.0 (1 call, 1280x720, 48 KB, one frame 10/10) |
| `form-batch` | 23.5 ms, mean 24.6 (1 call) | 20.6 ms, mean 22.0 (1 call) | 2095.0 ms, mean 2094.5 (4 calls) | 1083.3 ms, mean 1085.3 (4 calls) | 891.1 ms, mean 892.9 (4 calls) | 204.3 ms, mean 204.5 (1 call) | 11.1 ms, mean 11.9 (1 call) | 72.2 ms, mean 75.7 (4 calls) | 67.3 ms, mean 69.5 (4 calls) | 64.3 ms, mean 63.0 (4 calls) | 86.8 ms, mean 87.2 (4 calls) | 72.2 ms, mean 72.6 (4 calls) |
| `click-large` | 46.1 ms, mean 46.6 (1 call) | 82.3 ms, mean 83.7 (1 call) | 2209.7 ms, mean 2215.2 (1 call) | 1954.2 ms, mean 1946.2 (1 call) | 318.4 ms, mean 319.0 (1 call) | 14526.2 ms, mean 14596.9 (1 call) | 12.6 ms, mean 12.6 (1 call) | 83.8 ms, mean 82.6 (1 call) | 81.9 ms, mean 82.0 (1 call) | 30.8 ms, mean 30.1 (1 call) | 68.0 ms, mean 68.3 (1 call) | 65.7 ms, mean 65.8 (1 call) |
| `click-large-first` | 64.4 ms, mean 67.7 (1 call) | 90.3 ms, mean 93.8 (1 call) | 2427.2 ms, mean 2453.6 (1 call) | 1998.4 ms, mean 2018.6 (1 call) | 335.3 ms, mean 336.4 (1 call) | 15002.2 ms, mean 14952.7 (1 call) | 15.2 ms, mean 17.2 (1 call) | 132.7 ms, mean 160.1 (1 call) | 130.4 ms, mean 144.2 (1 call) | 36.2 ms, mean 37.7 (1 call) | 70.7 ms, mean 71.3 (1 call) | 68.5 ms, mean 69.0 (1 call) |

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
