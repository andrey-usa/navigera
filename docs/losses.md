# Where navigera is not ahead

Numbers: [bench run 45950542045](https://github.com/andrey-usa/navigera/actions/runs/45950542045).

Each measure of the benchmarks on which another tool beat navigera in this
run by more than 2%, from its publish.json: navigera on Chrome against every
other driver on the same Chrome, navigera on chrome-headless-shell against
go-rod on it, navigera against every other agent tool, and each op
against the tools on the same browser (Chrome, or Firefox for navigera
Firefox). Closer
than 2% is a tie. Driver CPU is left out: the kernel counts it in 10 ms
ticks, so every native driver ties.

- Warm eval: go-rod 0.116.2 (Go) 0.43 ms, navigera 0.50 ms.
- Warm eval on chrome-headless-shell: go-rod 0.116.2 (Go, chrome-headless-shell) 0.39 ms, navigera 0.43 ms.
- Click per call: agent-browser 0.45.0 7.0 ms, navigera 9.4 ms.
- Click-submit per call: agent-browser 0.45.0 5.7 ms, navigera 6.3 ms.
- Screenshot-720 per call: Chrome DevTools MCP 1.14.0 21.2 ms (49 KB PNG), navigera 30.9 ms (49 KB PNG).
- Form-batch per call: agent-browser 0.45.0 27.2 ms, navigera 28.9 ms.
- Goto per call on Firefox: Vibium Firefox API 41.0 ms, navigera Firefox 50.0 ms.
- Read-text per call on Firefox: Puppeteer Firefox BiDi API 0.8 ms, navigera Firefox 1.8 ms.
- Snapshot per call on Firefox: Vibium Firefox API 14.1 ms, navigera Firefox 14.5 ms.
- Screenshot-720 per call on Firefox: Puppeteer Firefox BiDi API 14.7 ms (48 KB PNG), navigera Firefox 15.5 ms (48 KB PNG).
