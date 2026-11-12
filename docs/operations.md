# Matched operations (one call per op, warm session, median of 10)

Numbers: [bench run 43114462641](https://github.com/andrey-usa/navigera/actions/runs/43114462641).

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

| op | `navigera` (this repo) | `navigera` (baseline build) | `agent-browser` 0.42.0 (Vercel Labs, Rust) | `playwright-cli` 0.1.24 (Microsoft) | Playwright MCP 0.0.85 (Microsoft) | Chrome DevTools MCP 1.13.0 (Google) | `browser-use` 1.0.2 (exec model) |
|---|---|---|---|---|---|---|---|
| `goto` | 46.1 ms (1 call) | 58.7 ms (1 call) | 59.1 ms (1 call) | 524.0 ms (1 call) | 142.9 ms (1 call) | 142.2 ms (1 call) | 213.7 ms (1 call) |
| `fill` | 7.8 ms (1 call) | 6.6 ms (1 call) | 5.6 ms (1 call) | 383.4 ms (1 call) | 12.5 ms (1 call) | 232.1 ms (1 call) | 209.8 ms (1 call) |
| `click` | 23.2 ms (1 call) | 36.6 ms (1 call) | 24.8 ms (1 call) | 938.7 ms (1 call) | 573.1 ms (1 call) | 244.7 ms (1 call) | 314.5 ms (1 call) |
| `click-submit` | 20.8 ms (1 call) | 187.1 ms (1 call) | 18.0 ms (1 call) | 942.8 ms (1 call) | 561.2 ms (1 call) | 237.5 ms (1 call) | 247.9 ms (1 call) |
| `read-text` | 2.8 ms (1 call) | 2.4 ms (1 call) | 2.8 ms (1 call) | 877.4 ms (1 call) | 504.7 ms (1 call) | 204.4 ms (1 call) | 188.4 ms (1 call) |
| `snapshot` | 16.5 ms (1 call) | 20.4 ms (1 call) | 48.4 ms (1 call) | 416.0 ms (1 call) | 36.9 ms (1 call) | 36.6 ms (1 call) | 241.4 ms (1 call) |
| `screenshot` | 49.3 ms (1 call) | 50.7 ms (1 call) | 37.7 ms (1 call) | 407.3 ms (1 call) | 49.5 ms (1 call) | 71.7 ms (1 call) | 237.8 ms (1 call) |
| `form-batch` | 75.7 ms (1 call) | 194.9 ms (1 call) | 26.8 ms (1 call) | 2562.2 ms (4 calls) | 1101.8 ms (4 calls) | 868.6 ms (4 calls) | 276.9 ms (1 call) |

Snapshot outputs differ in form (each tool's default tree), so that row compares
call latency, not equivalent work. The browser-use snapshot comes from its
documented raw `cdp()` route; it has no built-in snapshot command. Per-call time
includes process start (CLI) or JSON-RPC (MCP) and the daemon hop. An `incomplete
(n/N)` cell has only n of the 10 expected reps; its median is partial evidence.
