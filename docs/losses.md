# Where navigera is not ahead

Numbers: [bench run 45774240157](https://github.com/andrey-usa/navigera/actions/runs/45774240157).

Each measure of the benchmarks on which another tool beat navigera in this
run by more than 2%, from its publish.json: navigera on Chrome against every
other driver on the same Chrome, navigera on chrome-headless-shell against
go-rod on it, and navigera against every other agent tool. Closer
than 2% is a tie. Driver CPU is left out: the kernel counts it in 10 ms
ticks, so every native driver ties.

- Warm eval: go-rod 0.116.2 (Go) 0.32 ms, navigera 0.35 ms.
- The driver's own memory: chromedp 0.19.1 (Go) 8 MB, navigera 10 MB.
- Warm eval on chrome-headless-shell: go-rod 0.116.2 (Go, chrome-headless-shell) 0.28 ms, navigera 0.30 ms.
- Click per call: agent-browser 0.45.0 6.5 ms, navigera 12.2 ms.
- Click-submit per call: agent-browser 0.45.0 5.4 ms, navigera 8.6 ms.
- Read-text per call: Puppeteer Firefox BiDi API 0.6 ms, navigera 1.6 ms.
- Screenshot-720 per call: Puppeteer Firefox BiDi API 13.2 ms, navigera 34.3 ms.
