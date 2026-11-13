# Matched operations (one call per op, warm session, median of 10)

Numbers: [bench run 43398451080](https://github.com/andrey-usa/navigera/actions/runs/43398451080).

One CLI process (or one MCP `tools/call`) per op, against a warm session,
on the same fixture page. `goto` re-navigates to the listing; `fill` fills
the search box; `click` clicks Search (client-side filter, no navigation);
`click-submit` clicks the order form's submit, which a script handles (no
navigation); `read-text` reads the heading; `snapshot` is each tool's default
accessibility output; `screenshot` writes a PNG; `form-batch` fills two fields,
clicks submit and reads the status — one call for tools with a batch command,
four calls otherwise. Two rounds of 5 reps: rounds are the outer loop and tools
the inner loop, with the second round in reversed tool order. Each tool runs in
its default output mode (MCP tools over JSON-RPC, their only interface); no tool
is switched into a machine mode. A cell is complete only with all 10 reps and no
round error; a partial cell is shown as `incomplete (n/N)` and left out of the
per-op losses below.

| op | `navigera` (this repo) | `agent-browser` 0.43.0 (Vercel Labs, Rust) | `playwright-cli` 0.1.24 (Microsoft) | Playwright MCP 0.0.85 (Microsoft) | Chrome DevTools MCP 1.13.0 (Google) | `browser-use` 1.0.3 (exec model) |
|---|---|---|---|---|---|---|
| `goto` | 46.9 ms (1 call) | 56.8 ms (1 call) | 519.6 ms (1 call) | 150.1 ms (1 call) | 146.7 ms (1 call) | 508.4 ms (1 call) |
| `fill` | 7.8 ms (1 call) | 5.6 ms (1 call) | 381.0 ms (1 call) | 14.0 ms (1 call) | 233.3 ms (1 call) | 218.5 ms (1 call) |
| `click` | 15.2 ms (1 call) | 8.4 ms (1 call) | 943.8 ms (1 call) | 567.9 ms (1 call) | 247.1 ms (1 call) | 333.8 ms (1 call) |
| `click-submit` | 14.3 ms (1 call) | 6.7 ms (1 call) | 940.1 ms (1 call) | 562.1 ms (1 call) | 243.8 ms (1 call) | 252.0 ms (1 call) |
| `read-text` | 2.1 ms (1 call) | 2.7 ms (1 call) | 874.6 ms (1 call) | 504.9 ms (1 call) | 204.1 ms (1 call) | 195.1 ms (1 call) |
| `snapshot` | 14.8 ms (1 call) | 42.9 ms (1 call) | 407.5 ms (1 call) | 37.2 ms (1 call) | 37.1 ms (1 call) | 245.7 ms (1 call) |
| `screenshot` | 46.7 ms (1 call) | 38.5 ms (1 call) | 413.8 ms (1 call) | 50.5 ms (1 call) | 72.2 ms (1 call) | 221.8 ms (1 call) |
| `form-batch` | 42.5 ms (1 call) | 27.1 ms (1 call) | 2578.2 ms (4 calls) | 1108.6 ms (4 calls) | 877.6 ms (4 calls) | 277.1 ms (1 call) |

Snapshot outputs differ in form (each tool's default tree), so that row compares
call latency, not equivalent work. The browser-use snapshot comes from its
documented raw `cdp()` route; it has no built-in snapshot command. Per-call time
includes process start (CLI) or JSON-RPC (MCP) and the daemon hop. An `incomplete
(n/N)` cell has only n of the 10 expected reps; its median is partial evidence.
