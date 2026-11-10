# Supported browsers

CI runs the full end-to-end suite on:

- the current **Chrome Stable** and **the three milestones before it**
  (about four months of releases, covering Extended Stable), all from Chrome
  for Testing;
- **chrome-headless-shell**;
- **Chrome Beta**, as a non-blocking early warning;
- the runner's own Chrome, on Linux, **macOS** and **Windows**.

The suite is weekly as well as on every push, so a new Chrome release is
caught before agents hit it. In its first runs the matrix caught two real
bugs that the runner's Chrome alone missed: Chrome 151 segfaulted with one of
the automation flags (`--disable-features=OptimizationHints`, bisected with
`chrome-bisect.yml`), and on Chrome 153 an Enter-submitted form returned
before its redirect landed. Both are fixed.

Each run also checks **CDP protocol correctness**. navigera records
every command it sends (`NAVIGERA_CDP_TRACE`), and
[`tools/cdp_check.py`](../tools/cdp_check.py) validates every method, parameter
and enum value against the `/json/protocol` that *that* browser serves.
Deprecated or unknown usage fails the build. Edge and Brave are covered by
the benchmark.

**Lightpanda** 1.0 (`--engine lightpanda`) runs the same end-to-end suite in
CI, on 1.0.1 and the nightly. It is a different engine (one tab, no layout),
so navigera fills in what its CDP leaves out: shadow roots and password
fields in `ax`, `confirm()`/`prompt()` answers, navigations a click starts.
The differences that remain are listed in
[docs/navigera.md](navigera.md#lightpanda-and-your-own-browser).
