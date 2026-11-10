# Source layout

- `src/protocol.rs`: CLI parsing, the JSON-lines protocol and `Driver` (the `OPS` table is `--help`)
- `src/browser.rs`: `BrowserSession`: browser discovery (incl. chrome-headless-shell), launch flags, tabs
- `src/cdp/`: from-scratch CDP engine: pipe/WebSocket transport, JSON-RPC client (`NAVIGERA_CDP_TRACE`), page ops, `ax.rs` (snapshot tree), `events.rs` (dialogs, popups, navigation), `procjob.rs` (Windows job object)
- `src/session.rs`: named sessions: socket server (Unix socket; loopback TCP + token on Windows), client, detached `start`
- `src/cdp/attach.rs`: `--attach` (your running browser) and `--profile` (persistent navigera browser)
- `src/mcp.rs`: `navigera mcp`, the Model Context Protocol server on stdio
- `src/decide.rs`: `find` / `check`: the decision model's client and how page elements become options
- `src/timing.rs`: `NAVIGERA_TIMINGS=1` launch/close phase timings on stderr
- `tests/`: browser e2e: serve protocol, sessions, and the Acme Supply scenario
- `bench/site/server.py`: Acme Supply, a deterministic local shop (SPA, iframes, shadow DOM, dialogs, popups, login, upload)
- `bench/ladder/`: driver ladder and agent-tool benchmark (`publish.json` per run, `gen_readme.py`)
- `bench/agent-eval/`: Gemini CLI agent eval across navigera, playwright-cli and agent-browser
- `bench/decide/`: decision benchmark (`compare.py`), TypeSafe relay (`relay.py`) and a stand-in for runs without a key (`mock_jev.py`)
- `tools/`: `cdp_check.py` (protocol correctness), `protocol_dump.sh`, `cft_matrix.py` (Chrome version matrix)
- `skills/navigera/SKILL.md`: the agent guide (compiled into the binary: `navigera skill`)
- `vendor/axe-core/`: axe-core 4.14.0 (Deque Systems, MPL-2.0, unmodified, with its licenses), compiled in deflated for `a11y`
- `.github/workflows/`: `ci.yml` (tests, Chrome matrix, protocol check), `bench.yml`, `agent-eval.yml`, `release.yml`, `chrome-bisect.yml`, `vendor.yml`
