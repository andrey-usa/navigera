# Where navigera is not ahead

Numbers: [bench run 43114462641](https://github.com/andrey-usa/navigera/actions/runs/43114462641).

Each measure of the benchmarks on which another tool beat navigera in this
run by more than 2%, from its publish.json: navigera on Chrome against every
other driver on the same Chrome, navigera on chrome-headless-shell against
go-rod on it, and navigera against every other agent tool. Closer
than 2% is a tie. Driver CPU is left out: the kernel counts it in 10 ms
ticks, so every native driver ties.

- Warm eval on chrome-headless-shell: go-rod 0.116.2 (Go, chrome-headless-shell) 0.40 ms, navigera 0.43 ms.
- Fill per call: agent-browser 0.42.0 5.6 ms, navigera 7.8 ms.
- Click-submit per call: agent-browser 0.42.0 18.0 ms, navigera 20.8 ms.
- Screenshot per call: agent-browser 0.42.0 37.7 ms, navigera 49.3 ms.
- Form-batch per call: agent-browser 0.42.0 26.8 ms, navigera 75.7 ms.
