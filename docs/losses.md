# Where navigera is not ahead

Numbers: [bench run 42863598577](https://github.com/andrey-usa/navigera/actions/runs/42863598577).

Each measure of the benchmarks on which another tool beat navigera in this
run by more than 2%, from its publish.json: navigera on Chrome against every
other driver on the same Chrome, navigera on chrome-headless-shell against
go-rod on it, and navigera against every other agent tool. Closer
than 2% is a tie. Driver CPU is left out: the kernel counts it in 10 ms
ticks, so every native driver ties.

- Warm eval: go-rod 0.116.2 (Go) 0.25 ms, navigera 0.34 ms.
- Browser memory: puppeteer-core 25.16.0 (Node) 368 MB, navigera 400 MB.
