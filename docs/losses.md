# Where navigera is not ahead

Numbers: [bench run 42759659475](https://github.com/andrey-usa/navigera/actions/runs/42759659475).

Each measure of the benchmarks on which another tool beat navigera in this
run, from its publish.json: navigera on Chrome against every other driver on
the same Chrome, and against every other agent tool. Driver CPU is left out:
the kernel counts it in 10 ms ticks, so every native driver ties.

- Warm eval: go-rod 0.116.2 (Go) 0.24 ms, navigera 0.35 ms.
- Browser memory: puppeteer-core 25.16.0 (Node) 365 MB, navigera 399 MB.
