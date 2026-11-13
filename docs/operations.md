# Matched operations (one call per op, warm session, median of 10)

Numbers: [bench run 43491069940](https://github.com/andrey-usa/navigera/actions/runs/43491069940).

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

| op | `navigera` (this repo) | `navigera` (baseline build) | `agent-browser` 0.43.0 (Vercel Labs, Rust) | `playwright-cli` 0.1.24 (Microsoft) | Playwright MCP 0.0.85 (Microsoft) | Chrome DevTools MCP 1.13.0 (Google) | `browser-use` 1.0.3 (exec model) |
|---|---|---|---|---|---|---|---|
| `goto` | 45.5 ms (1 call) | 42.5 ms (1 call) | 55.8 ms (1 call) | 513.5 ms (1 call) | 147.3 ms (1 call) | 144.6 ms (1 call) | 377.4 ms (1 call) |
| `fill` | 6.9 ms (1 call) | 7.1 ms (1 call) | 4.7 ms (1 call) | 379.1 ms (1 call) | 12.9 ms (1 call) | 231.6 ms (1 call) | 207.0 ms (1 call) |
| `click` | 13.2 ms (1 call) | 13.9 ms (1 call) | 7.4 ms (1 call) | 939.0 ms (1 call) | 572.4 ms (1 call) | 243.5 ms (1 call) | 319.3 ms (1 call) |
| `click-submit` | 10.5 ms (1 call) | 13.1 ms (1 call) | 6.1 ms (1 call) | 925.4 ms (1 call) | 562.2 ms (1 call) | 233.6 ms (1 call) | 240.2 ms (1 call) |
| `read-text` | 2.4 ms (1 call) | 2.7 ms (1 call) | 2.7 ms (1 call) | 868.6 ms (1 call) | 504.0 ms (1 call) | 204.0 ms (1 call) | 183.1 ms (1 call) |
| `snapshot` | 11.6 ms (1 call) | 12.3 ms (1 call) | 40.5 ms (1 call) | 403.6 ms (1 call) | 36.5 ms (1 call) | 36.8 ms (1 call) | 242.3 ms (1 call) |
| `screenshot` | 47.3 ms (1 call) | 42.1 ms (1 call) | 37.8 ms (1 call) | 411.4 ms (1 call) | 40.4 ms (1 call) | 71.5 ms (1 call) | 218.9 ms (1 call) |
| `form-batch` | 26.3 ms (1 call) | 39.4 ms (1 call) | 26.7 ms (1 call) | 2567.9 ms (4 calls) | 1104.5 ms (4 calls) | 874.2 ms (4 calls) | 270.8 ms (1 call) |

Snapshot outputs differ in form (each tool's default tree), so that row compares
call latency, not equivalent work. The browser-use snapshot comes from its
documented raw `cdp()` route; it has no built-in snapshot command. Per-call time
includes process start (CLI) or JSON-RPC (MCP) and the daemon hop. An `incomplete
(n/N)` cell has only n of the 10 expected reps; its median is partial evidence.
