# Where navigera is not ahead

Numbers: [bench run 46044180770](https://github.com/andrey-usa/navigera/actions/runs/46044180770).

Each measure of the benchmarks on which another tool beat navigera in this
run by more than 2%, from its publish.json: navigera on Chrome against every
other driver on the same Chrome, navigera on chrome-headless-shell against
go-rod on it, navigera against every other agent tool, and each op
against the tools on the same browser (Chrome, or Firefox for navigera
Firefox). Closer
than 2% is a tie. Driver CPU is left out: the kernel counts it in 10 ms
ticks, so every native driver ties.

- Warm eval: go-rod 0.116.2 (Go) 0.18 ms, navigera 0.31 ms.
- Browser memory: puppeteer-core 25.19.0 (Node) 323 MB, navigera 359 MB.
- Warm eval on chrome-headless-shell: go-rod 0.116.2 (Go, chrome-headless-shell) 0.15 ms, navigera 0.18 ms.
- Click per call: agent-browser 0.45.0 6.5 ms, navigera 8.8 ms.
- Form-batch per call: agent-browser 0.45.0 20.6 ms, navigera 23.5 ms.
- Goto per call on Firefox: Playwright Firefox Node API 27.9 ms, navigera Firefox 47.3 ms.
- Read-text per call on Firefox: Puppeteer Firefox BiDi API 1.1 ms, navigera Firefox 1.6 ms.
- Snapshot per call on Firefox: Vibium Firefox API 9.4 ms, navigera Firefox 11.2 ms.
