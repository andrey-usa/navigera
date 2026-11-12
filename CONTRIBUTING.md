# Contributing to navigera

Bug reports and pull requests are welcome. For anything larger than a fix,
open an issue first so we can agree on the approach.

## Build and test

```sh
cargo build
cargo test                                   # unit + browser e2e tests
cargo clippy --all-targets -- -D warnings    # lint gate
cargo fmt --check
```

The e2e tests drive a real Chrome. navigera finds `$CHROME_BIN`, a system
Chrome/Chromium/Edge, or one `navigera install-browser` installed; without
any of them the browser tests are skipped (CI sets `NAVIGERA_REQUIRE_BROWSER=1`
so they can't be skipped there). The Acme Supply scenario in
`tests/site_scenarios.rs` takes about 10 s.

For manual testing, run the local shop and drive it with a session:

```sh
python3 bench/site/server.py --port 8765
navigera -s dev start
navigera -s dev goto localhost:8765
navigera -s dev ax
```

### Offline builds

`vendor.yml` publishes the vendored dependencies to the git ref
`refs/cache/vendor` whenever `Cargo.lock` changes, for machines that can reach
GitHub but not crates.io:

```sh
git fetch origin refs/cache/vendor:refs/cache/vendor
mkdir -p ../nv-vendor && git archive refs/cache/vendor | tar -x -C ../nv-vendor
mkdir -p ~/.cargo && printf '[source.crates-io]\nreplace-with = "v"\n[source.v]\ndirectory = "%s"\n' \
    "$(cd ../nv-vendor && pwd)/vendor" >> ~/.cargo/config.toml
cargo test --offline
```

## CI

| workflow | runs | what |
|---|---|---|
| `ci.yml` | push, PR, weekly | tests, lint, Windows, install scripts, Chrome Stable + three milestones back, chrome-headless-shell, Beta |
| `agent-check.yml` | push, PR, nightly | natural purchase checks with a real agent on Linux and Windows |
| `agent-eval.yml` | manual, weekly | Gemini CLI doing the Acme tasks with navigera, playwright-cli and agent-browser |
| `bench.yml` | manual | the driver ladder: navigera against Playwright, Puppeteer, go-rod, chromedp, chromiumoxide |
| `chrome-bisect.yml` | `.github/bisect.env` on a branch | one Chrome milestone under several flag/transport variants |
| `gate.yml` | manual | timing gate: the candidate against the last release, fixed probes |
| `release.yml` | manual, `v*` tags | binaries, GitHub release, registries |

Every workflow reports what you need to act on as check annotations (failing
test output, the CDP methods each Chrome build accepted, bench tables, eval
results), so the checks API is enough to read a run:

```sh
gh api repos/andrey-usa/navigera/check-runs/<job-id>/annotations -q '.[] | "[\(.title)] \(.message)"'
```

Before using a new CDP method or parameter, check that the oldest supported
milestone has it: `bash tools/protocol_dump.sh <chrome> protocol.json`.

### Windows

Windows differs in three places: CDP runs over a WebSocket (no pipe), sessions
use loopback TCP plus a token file (`src/session.rs`, `endpoint`), and Chrome
runs in a kill-on-close job object (`src/cdp/procjob.rs`). The `windows` job in
`ci.yml` builds, lints and runs every test on the runner's Chrome.

## Performance changes

Runners differ between runs by more than most changes are worth, so judge a
change with an A/B inside one run: `baseline_ref` builds a second navigera from
any ref and runs it beside your build.

```sh
gh workflow run bench.yml --ref my-branch -f reps=3 \
    -f only=nv-serve,nv-baseline,gorod -f scenarios=eval,cold -f baseline_ref=master
```

Measurement rules the ladder follows:

- navigera is measured as released: the static musl build (both sides of an
  A/B, each built from its own tree, since `.cargo/config.toml` is part of
  the build).
- Reps are interleaved across contenders, and `publish.json` keeps every
  run, the median and each contender's version beside the best.
- Driver CPU and memory are the driver's own (`utime+stime` from the zombie's
  `/proc/<pid>/stat`, VmHWM), never `wait4` totals, which include every child
  the driver reaped.
- Browser numbers cover the whole process tree as PSS.
- Every benchmark number in the docs cites the run it came from, and none is
  typed by hand: `bench/ladder/fetch_publish.py <run id>` writes the run's
  `publish.json` from its notices (no token, no artifact download), then
  `bench/ladder/gen_readme.py README.md publish.json "<what changed>"
  --page docs/benchmarks.md --losses docs/losses.md --ops docs/operations.md` writes every table to
  the docs site's Benchmarks pages, the measures another tool won to its
  losses page, and the headline to the README.

## Keeping docs in step

When ops or flags change, update `skills/navigera/SKILL.md` (compiled into the
binary as `navigera skill`), `docs/navigera.md` and the `OPS` table in
`src/protocol.rs` together.

The docs site (https://andrey-usa.github.io/navigera/) is written by
`docs-site/build.py` from `docs-site/site.toml`, which says which page comes
from which source: README sections, `docs/`, the skill, CHANGELOG.md and the
binary's `--help`. Nothing under `docs-site/src` is edited by hand. The build
stops on a link to a missing file or heading and on a source section that is
on no page; `--write` also rewrites the README's Documentation section and
`llms.txt`, which CI's docs job checks:

```sh
cargo build --release
python3 docs-site/build.py --write                    # pages, README's Documentation, llms.txt
python3 docs-site/build.py --mdbook mdbook            # and the HTML, with every link checked
```

## Releasing

1. Actions → gate → Run workflow on the candidate branch: the same probes
   (snapshots of large and div-heavy pages, scoped snapshots, waits around
   long polls, beacons and clicks that fetch, a covered click, a big select)
   on the last release and the candidate, one runner; it fails when the
   candidate is more than 25% (and 20 ms) slower on any of them. Both are
   the static musl build a release ships. Locally:
   `cargo build --release --target x86_64-unknown-linux-musl`, then
   `python3 bench/gate/gate.py --base <old navigera> --candidate target/x86_64-unknown-linux-musl/release/navigera`.
2. Bump `version` in `Cargo.toml` and add a `CHANGELOG.md` entry on `master`,
   with a "Behaviour changes" section for what a script or an agent relying
   on the old output or timing should know.
3. Actions → release → Run workflow with that version. The run checks the
   version, builds every target, tags `v<version>`, creates the GitHub release
   and publishes to each registry that is switched on (see below).

Registries are switched on per repository with Actions variables, once the
account and trusted publisher exist:

| variable | publishes to | trusted publisher to configure |
|---|---|---|
| `PUBLISH_CRATES=true` | crates.io | crate `navigera`, workflow `release.yml` |
| `PUBLISH_NPM=true` | npm (`navigera` + one package per platform) | each package, workflow `release.yml` |
| `PUBLISH_PYPI=true` | PyPI (platform wheels) | project `navigera`, workflow `release.yml`, environment `release` |
| `HOMEBREW_TAP=owner/homebrew-tap` | Homebrew formula | secret `TAP_TOKEN` with write access to the tap |
| `SCOOP_BUCKET=owner/scoop-bucket` | Scoop manifest | the same `TAP_TOKEN` |

Re-running the workflow for a version that already exists rebuilds its files
and publishes it to any registry that doesn't have it yet.

### Glama

Glama ([glama.ai/mcp/servers/andrey-usa/navigera](https://glama.ai/mcp/servers/andrey-usa/navigera))
builds its own image of `navigera mcp` from what is already published, and
needs two files in this repository, used by nothing else:

| file | what Glama does with it |
|---|---|
| `glama.json` (repository root, where Glama looks) | names the maintainers, for the listing |
| `packaging/glama/build.sh` | its build steps: the system libraries Chrome links against, `uv tool install navigera==<version>` from PyPI, `navigera install-browser chrome-headless-shell`, a check that the browser's libraries load, and one page read through `navigera mcp` with no browser variable set, so a missing library or a server that needs a setting fails the build instead of the first call |

Neither goes into a release: the crate is an allowlist (`include` in
`Cargo.toml`, checked by the "Glama files stay out of the packages" step in
`ci.yml`), and npm, PyPI, Homebrew, Scoop and the archives carry the binary
alone. Glama's admin form holds the rest: the build step is
`sh packaging/glama/build.sh`, run in Glama's copy of this repository (the
build's working directory), the command is
`["mcp-proxy", "--", "/root/.local/bin/navigera", "mcp"]` (Glama's proxy in
front of navigera's stdio server), and it offers users **no settings**:
the environment-variable schema in that form is empty. The server finds the
browser the build installed by itself, and needs no key. A field left there
(a browser path, a key) is asked of everyone who tries the server, and a
wrong browser path breaks it.

A navigera release is a Glama release under the same number: auto-release
names it after the tag without the `v` (`v0.8.0` is `0.8.0`). Glama builds
from its copy of the repository, not from the tag, and `build.sh` takes the
version from that copy's `Cargo.toml`. So after the release merge reaches
`master`, sync Glama's copy (the link next to its head commit) before the
build. A stale copy fails the build, or builds the version before. After it,
check the listing in Glama's "try it": it asks for no settings, `tools/list` gives the 13 default tools, and
`browser_navigate` to https://example.com followed by `browser_snapshot`
returns the page's tree. Glama can't rebuild a version it has released, so a
fix to Glama's image alone would need a number of its own. Wait for the next
navigera release instead: a Glama-only number reads like a navigera release
that doesn't exist.
