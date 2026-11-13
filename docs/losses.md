# Where navigera is not ahead

Numbers: [bench run 43491069940](https://github.com/andrey-usa/navigera/actions/runs/43491069940).

Each measure of the benchmarks on which another tool beat navigera in this
run by more than 2%, from its publish.json: navigera on Chrome against every
other driver on the same Chrome, navigera on chrome-headless-shell against
go-rod on it, and navigera against every other agent tool. Closer
than 2% is a tie. Driver CPU is left out: the kernel counts it in 10 ms
ticks, so every native driver ties.

- The driver's own memory: chromedp 0.19.1 (Go) 8 MB, navigera 9 MB.
- Warm eval on chrome-headless-shell: go-rod 0.116.2 (Go, chrome-headless-shell) 0.38 ms, navigera 0.41 ms.
- Fill per call: agent-browser 0.43.0 4.7 ms, navigera 6.9 ms.
- Click per call: agent-browser 0.43.0 7.4 ms, navigera 13.2 ms.
- Click-submit per call: agent-browser 0.43.0 6.1 ms, navigera 10.5 ms.
- Screenshot per call: agent-browser 0.43.0 37.8 ms, navigera 47.3 ms.
