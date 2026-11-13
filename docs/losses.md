# Where navigera is not ahead

Numbers: [bench run 43398451080](https://github.com/andrey-usa/navigera/actions/runs/43398451080).

Each measure of the benchmarks on which another tool beat navigera in this
run by more than 2%, from its publish.json: navigera on Chrome against every
other driver on the same Chrome, navigera on chrome-headless-shell against
go-rod on it, and navigera against every other agent tool. Closer
than 2% is a tie. Driver CPU is left out: the kernel counts it in 10 ms
ticks, so every native driver ties.

- Warm eval: go-rod 0.116.2 (Go) 0.46 ms, navigera 0.48 ms.
- The driver's own memory: chromedp 0.19.1 (Go) 8 MB, navigera 9 MB.
- Warm eval on chrome-headless-shell: go-rod 0.116.2 (Go, chrome-headless-shell) 0.39 ms, navigera 0.45 ms.
- Fill per call: agent-browser 0.43.0 5.6 ms, navigera 7.8 ms.
- Click per call: agent-browser 0.43.0 8.4 ms, navigera 15.2 ms.
- Click-submit per call: agent-browser 0.43.0 6.7 ms, navigera 14.3 ms.
- Screenshot per call: agent-browser 0.43.0 38.5 ms, navigera 46.7 ms.
- Form-batch per call: agent-browser 0.43.0 27.1 ms, navigera 42.5 ms.
