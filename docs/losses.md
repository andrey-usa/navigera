# Where navigera is not ahead

Numbers: [bench run 45918635350](https://github.com/andrey-usa/navigera/actions/runs/45918635350).

Each measure of the benchmarks on which another tool beat navigera in this
run by more than 2%, from its publish.json: navigera on Chrome against every
other driver on the same Chrome, navigera on chrome-headless-shell against
go-rod on it, navigera against every other agent tool, and each op
against the tools on the same browser (Chrome, or Firefox for navigera
Firefox). Closer
than 2% is a tie. Driver CPU is left out: the kernel counts it in 10 ms
ticks, so every native driver ties.

- Warm eval: go-rod 0.116.2 (Go) 0.16 ms, navigera 0.27 ms.
- Browser memory: puppeteer-core 25.19.0 (Node) 324 MB, navigera 363 MB.
- Warm eval on chrome-headless-shell: go-rod 0.116.2 (Go, chrome-headless-shell) 0.15 ms, navigera 0.16 ms.
- Click per call: agent-browser 0.45.0 6.3 ms, navigera 9.8 ms.
- Click-submit per call: agent-browser 0.45.0 5.5 ms, navigera 6.1 ms.
- Form-batch per call: agent-browser 0.45.0 28.3 ms, navigera 29.5 ms.
- Goto per call on Firefox: Puppeteer Firefox BiDi API 24.3 ms, navigera Firefox 31.8 ms.
- Fill per call on Firefox: Playwright Firefox Node API 9.4 ms, navigera Firefox 11.4 ms.
- Read-text per call on Firefox: Puppeteer Firefox BiDi API 0.7 ms, navigera Firefox 1.3 ms.
- Snapshot per call on Firefox: Vibium Firefox API 9.1 ms, navigera Firefox 11.1 ms.
- Screenshot-720 per call on Firefox: Puppeteer Firefox BiDi API 12.1 ms, navigera Firefox 12.6 ms.
