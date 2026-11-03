#!/usr/bin/env python3
"""Browser-driver ladder: same scripted session, CDP drivers, one table.

Contenders (each drives the same headless Chrome on the same fixture pages):
  nv-serve        navigera serve on Chrome (this repo: from-scratch Rust CDP engine;
                  bench.yml builds the static musl binary the releases ship)
  nv-baseline     the same, another build for an A/B ($NAVIGERA_BASELINE)
  nv-edge         navigera serve on Microsoft Edge
  nv-brave        navigera serve on Brave
  nv-lightpanda   navigera serve on Lightpanda (headless Chromium for AI agents)
  playwright      playwright-core + channel:chrome (Node)
  puppeteer       puppeteer-core (Node)
  chromiumoxide   chromiumoxide 0.7 (native Rust CDP)
  chromedp        chromedp v0.19.1 (native Go CDP)
  gorod           go-rod v0.116.2 (native Go CDP)

Engine variants (nv-edge/nv-brave/nv-lightpanda) are skipped gracefully when
their binary is not installed (env EDGE_BIN / BRAVE_BIN / lightpanda shim).

Three measurements:
  session   full 12-op scripted session, best-of-N wall
  eval      warm eval round-trip micro (200x `() => document.title`)
  cold      cold start: launch -> new page -> one eval -> close, best-of-5

Accounting (house style, cf. gruppera bench-200m.yml):
  * wall     perf_counter around the run (best of N)
  * driver CPU: the driver process's OWN utime+stime, read from
    /proc/<pid>/stat while it is a zombie (waitid WNOWAIT), before reaping.
    NOT wait4's rusage: that is RUSAGE_BOTH, i.e. it also contains every
    descendant the driver reaped. Drivers that wait() on their Chrome child
    (navigera, chromedp, puppeteer, playwright) were charged Chrome's CPU
    and peak RSS that way; drivers that leave Chrome to be reaped elsewhere
    (go-rod's leakless helper, chromiumoxide) were not. The reaped-children
    share is still recorded as `cpu_reaped_s` for transparency.
  * driver peak RSS: VmHWM of the driver process itself, sampled every
    20 ms from /proc (VmHWM is monotonic, so the last sample is the peak up
    to <=20 ms before exit). For nv-* the driver is the `navigera` child
    of contenders/nv_serve.py, measured the same way there.
  * browser CPU / memory: the browser process tree below each driver run
    (found by parent links, so helpers like go-rod's leakless are crossed),
    sampled every 50 ms (PSS every 250 ms): summed CPU deltas and peak summed
    PSS (shared pages split, not double counted). Approximate; labeled as such.

Correctness: every contender must produce identical normalized
`extract_a.json` and identical `counts` (titles, card counts, filter
result, row count). The ladder aborts the table on mismatch.

Env:
  NAVIGERA   path to the release navigera binary (required)
  CHROME_BIN     chrome executable (required)
  LADDER_BASE    fixture server base URL (set by this script itself)
  LADDER_REPS    session repetitions (default 3)

Outputs (in --out-dir): results.json, table.md, publish.json (every run, the
median, and the version of every contender and browser), per-contender
artifacts.
"""
import argparse
import http.server
import json
import math
import os
import socketserver
import statistics
import subprocess
import sys
import threading
import time
from pathlib import Path

# navigera prints compact output by default; this reads its JSON lines.
os.environ.setdefault("NAVIGERA_OUTPUT", "json")

HERE = Path(__file__).resolve().parent
CONTENDERS = HERE / "contenders"
FIXTURES = HERE / "fixtures"

# navigera engine variants populated by main(): name -> extra nv_serve.py args.
# e.g. {"nv-serve": [], "nv-edge": ["--engine", "chrome", "--chromium", "/path"]}
NAVIGERA_ENGINE_ARGS: dict[str, list[str]] = {}

# navigera binary per nv-* contender (default $NAVIGERA). The A/B
# contender `nv-baseline` runs a second build ($NAVIGERA_BASELINE, e.g.
# master) on the same machine in the same run — the only fair way to judge a
# perf change, since GitHub runners vary between runs.
NAVIGERA_BINARY: dict[str, str] = {}


def parse_phases(text: str) -> dict:
    """navigera's `NAVIGERA_TIMINGS {...}` shutdown line, if present."""
    for line in text.splitlines():
        if line.startswith("NAVIGERA_TIMINGS "):
            try:
                return json.loads(line[len("NAVIGERA_TIMINGS "):])
            except ValueError:
                return {}
    return {}

# Contenders that did not run, with the reason — printed in table.md so a
# missing row is never silent.
SKIPPED: dict[str, str] = {}

# Experimental contenders: a failure in any stage, or a correctness mismatch,
# is recorded in the table instead of aborting the ladder. Lightpanda is a
# single-tab engine and runs the session's second page in the same tab.
EXPERIMENTAL = {"nv-lightpanda"}
NOTES: dict[str, list[str]] = {}

# Process name the sampler should attribute to each contender's browser
# (Edge/Brave run under their own binary names, not Chrome's).
BROWSER_TAGS = {"nv-edge": "msedge", "nv-brave": "brave", "nv-lightpanda": "lightpanda",
                "nv-shell": "chrome-headless-shell"}

OPTIONAL_SCENARIOS = ("eval", "cold", "realworld", "browse", "agent", "parallel")

# Parallel agents: how many sessions are held open at once.
PARALLEL_NS = (1, 4, 8)


def is_nv(name: str) -> bool:
    """True for navigera contenders (nv-serve, nv-edge, ...)."""
    return name in NAVIGERA_ENGINE_ARGS

CLK_TCK = os.sysconf("SC_CLK_TCK")


def read_proc_stat(pid: int):
    """(utime+stime in seconds, rss_kb) for pid, or None."""
    try:
        with open(f"/proc/{pid}/stat", "rb") as f:
            parts = f.read().rsplit(b")", 1)[1].split()
        utime = int(parts[11])
        stime = int(parts[12])
        with open(f"/proc/{pid}/statm", "rb") as f:
            rss_pages = int(f.read().split()[1])
        page_kb = os.sysconf("SC_PAGE_SIZE") // 1024
        return (utime + stime) / CLK_TCK, rss_pages * page_kb
    except (FileNotFoundError, ProcessLookupError, IndexError, ValueError):
        return None


def chrome_pids(chrome_tag: str) -> list[int]:
    pids = []
    for pid in os.listdir("/proc"):
        if not pid.isdigit():
            continue
        try:
            with open(f"/proc/{pid}/cmdline", "rb") as f:
                cmd = f.read().replace(b"\0", b" ").decode(errors="replace")
        except (FileNotFoundError, ProcessLookupError):
            continue
        # Match the browser binary itself (all contenders launch the same
        # chrome); renderer/utility children carry the same basename.
        # Contenders launch with --remote-debugging-pipe or
        # --remote-debugging-port, so don't filter on either.
        argv0 = cmd.split(" ", 1)[0]
        if argv0 == chrome_tag or argv0.endswith("/" + chrome_tag):
            pids.append(int(pid))
    return pids


def kill_stray_chrome(chrome_tag: str) -> None:
    """Best-effort cleanup so one contender's browser can't pollute the next."""
    strays = set(chrome_pids(chrome_tag))
    strays.update(pid for pid, (_, name) in proc_table().items()
                  if name.startswith(BROWSER_PREFIXES) and pid != os.getpid())
    for pid in strays:
        try:
            os.kill(pid, 15)
        except (ProcessLookupError, PermissionError):
            pass
    # It runs before every rep: wait only when something was left, and only
    # until it has gone.
    deadline = time.monotonic() + 1.0
    while strays and time.monotonic() < deadline:
        strays = {p for p in strays if Path(f"/proc/{p}").exists() and not is_zombie(p)}
        time.sleep(0.05)


def is_zombie(pid: int) -> bool:
    try:
        with open(f"/proc/{pid}/stat", "rb") as f:
            return f.read().rsplit(b")", 1)[1].split()[0] == b"Z"
    except (OSError, IndexError):
        return True


# Process names (comm) that belong to a browser: Chrome's processes are
# "chrome" (crashpad "chrome_crashpad"), Edge "msedge", Brave "brave".
BROWSER_PREFIXES = ("chrome", "google-chrome", "msedge", "brave", "lightpanda")


def proc_table() -> dict[int, tuple[int, str]]:
    """pid -> (ppid, comm) for every visible process.

    comm (the executable's name, kernel-maintained) rather than argv[0]:
    Chromium rewrites its child processes' command lines into one
    space-joined string, so an argv[0] basename can come out as a fragment
    of some later --flag=/path argument (Edge's tree went missing that way)."""
    table = {}
    for d in os.listdir("/proc"):
        if not d.isdigit():
            continue
        try:
            with open(f"/proc/{d}/stat", "rb") as f:
                ppid = int(f.read().rsplit(b")", 1)[1].split()[1])
            with open(f"/proc/{d}/comm", "rb") as f:
                comm = f.read().strip().decode(errors="replace")
        except (OSError, ValueError, IndexError):
            continue
        table[int(d)] = (ppid, comm)
    return table


def browser_descendants(root: int, table: dict[int, tuple[int, str]]) -> list[int]:
    """Browser processes anywhere below `root` (through helpers like go-rod's
    leakless or nv_serve.py -> navigera)."""
    kids: dict[int, list[int]] = {}
    for pid, (ppid, _) in table.items():
        kids.setdefault(ppid, []).append(pid)
    found, stack = [], [root]
    while stack:
        for child in kids.get(stack.pop(), []):
            stack.append(child)
            if table[child][1].startswith(BROWSER_PREFIXES):
                found.append(child)
    return found


def browser_pids(root: int) -> list[int]:
    """Every browser process belonging to the current run.

    Descendants of the driver, plus any browser process not under it: some
    launchers (the Edge stub) exit and leave the real browser reparented to
    init, outside the driver's tree. Contenders run one at a time and
    kill_stray_chrome() clears leftovers before each one, so every browser
    process alive during a run is that run's."""
    table = proc_table()
    pids = set(browser_descendants(root, table))
    pids.update(pid for pid, (_, name) in table.items() if name.startswith(BROWSER_PREFIXES))
    return sorted(pids)


def pss_kb(pid: int) -> int | None:
    """Proportional set size: shared pages split across the processes that
    map them, so summing a process tree doesn't double count (RSS does)."""
    try:
        with open(f"/proc/{pid}/smaps_rollup", "rb") as f:
            for line in f:
                if line.startswith(b"Pss:"):
                    return int(line.split()[1])
    except (OSError, ValueError, IndexError):
        pass
    sample = read_proc_stat(pid)
    return sample[1] if sample else None


class BrowserSampler(threading.Thread):
    """Samples every browser process of one driver run (see browser_pids):
    summed CPU deltas and peak summed PSS, every 50 ms. Same rule for every
    contender (the old name-matching sampler saw only Chrome's main process,
    but every msedge/brave process, with RSS double counting)."""

    def __init__(self, root_pid: int, interval: float = 0.05):
        super().__init__(daemon=True)
        self.root_pid = root_pid
        self.interval = interval
        self.cpu_s = 0.0
        self.peak_pss_kb = 0
        self._stop_event = threading.Event()
        self._prev: dict[int, float] = {}
        self._pss: dict[int, int] = {}  # last PSS reading per pid

    def run(self):
        tick = 0
        while not self._stop_event.is_set():
            # CPU (/proc/<pid>/stat) is cheap; PSS (smaps_rollup) walks the
            # target's mappings under its mmap lock, so read it every 5th
            # tick (250 ms) to keep the observer effect off the browser.
            with_pss = tick % 5 == 0
            tick += 1
            pids = browser_pids(self.root_pid)
            for pid in pids:
                sample = read_proc_stat(pid)
                if sample is None:
                    continue
                cpu, _ = sample
                prev = self._prev.get(pid)
                if prev is not None and cpu >= prev:
                    self.cpu_s += cpu - prev
                elif prev is None:
                    self.cpu_s += cpu  # first sight: count what it already used
                self._prev[pid] = cpu
                # Read PSS on the slow tick, and at first sight of a process
                # so short runs (e.g. Lightpanda's ~0.2 s session) still get
                # a reading; between readings each pid keeps its last value.
                if with_pss or pid not in self._pss:
                    v = pss_kb(pid)
                    if v is not None:
                        self._pss[pid] = v
            live = set(pids)
            total = sum(v for p, v in self._pss.items() if p in live)
            self.peak_pss_kb = max(self.peak_pss_kb, total)
            self._stop_event.wait(self.interval)

    def stop(self):
        self._stop_event.set()
        self.join()


def zombie_cpu_s(pid: int) -> tuple[float, float] | None:
    """(own utime+stime, reaped-children cutime+cstime) for an exited, not yet
    reaped pid. /proc/<pid>/stat of a zombie carries the final totals."""
    try:
        with open(f"/proc/{pid}/stat", "rb") as f:
            parts = f.read().rsplit(b")", 1)[1].split()
        own = (int(parts[11]) + int(parts[12])) / CLK_TCK
        reaped = (int(parts[13]) + int(parts[14])) / CLK_TCK
        return own, reaped
    except (FileNotFoundError, ProcessLookupError, IndexError, ValueError):
        return None


def vm_hwm_kb(pid: int) -> int | None:
    """Peak RSS (VmHWM) of pid itself, or None (gone / zombie)."""
    try:
        with open(f"/proc/{pid}/status", "rb") as f:
            for line in f:
                if line.startswith(b"VmHWM:"):
                    return int(line.split()[1])
    except (FileNotFoundError, ProcessLookupError, ValueError, IndexError):
        pass
    return None


class HwmSampler(threading.Thread):
    """Tracks one process's own VmHWM until stopped."""

    def __init__(self, pid: int, interval: float = 0.02):
        super().__init__(daemon=True)
        self.pid = pid
        self.interval = interval
        self.peak_kb = 0
        self._stop_event = threading.Event()

    def run(self):
        while not self._stop_event.is_set():
            v = vm_hwm_kb(self.pid)
            if v and v > self.peak_kb:
                self.peak_kb = v
            self._stop_event.wait(self.interval)

    def stop(self):
        self._stop_event.set()
        self.join()


def run_once(argv, env, timeout_s, chrome_tag=None, detail=None) -> tuple[float, float, int, bytes]:
    """Run argv with a hard timeout; return (wall_s, cpu_s, maxrss_kb, stdout).

    cpu_s is the driver's OWN CPU (zombie /proc read, see module doc);
    maxrss_kb is its own sampled VmHWM. If `detail` is a dict it receives
    `cpu_reaped_s` (CPU of descendants the driver reaped) and the raw wait4
    numbers, so the old RUSAGE_BOTH figures stay inspectable.
    On timeout the child is killed and RuntimeError raised.
    """
    t0 = time.perf_counter()
    proc = subprocess.Popen(argv, env=env, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE)
    hwm = HwmSampler(proc.pid)
    hwm.start()
    browser = BrowserSampler(proc.pid)
    browser.start()
    # Drain pipes concurrently so a chatty driver can't block on a full pipe.
    bufs = {"out": b"", "err": b""}

    def _drain(stream, key):
        bufs[key] = stream.read()

    drains = [threading.Thread(target=_drain, args=(proc.stdout, "out"), daemon=True),
              threading.Thread(target=_drain, args=(proc.stderr, "err"), daemon=True)]
    for t in drains:
        t.start()
    while True:
        # WNOWAIT: observe the exit but leave the zombie in place so its
        # /proc/<pid>/stat (own vs reaped-children CPU) is still readable.
        info = os.waitid(os.P_PID, proc.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
        if info is not None and info.si_pid == proc.pid:
            break
        if time.perf_counter() - t0 > timeout_s:
            proc.kill()
            hwm.stop()
            browser.stop()
            try:
                os.wait4(proc.pid, 0)
            except ChildProcessError:
                pass
            if chrome_tag:
                kill_stray_chrome(chrome_tag)
            raise RuntimeError(f"{argv[:2]} timed out after {timeout_s}s")
        time.sleep(0.005)
    wall = time.perf_counter() - t0
    hwm.stop()
    browser.stop()
    cpu = zombie_cpu_s(proc.pid)
    _, status, ru = os.wait4(proc.pid, 0)
    proc.returncode = os.waitstatus_to_exitcode(status)
    for t in drains:
        t.join(timeout=30)  # a leaked grandchild holding the pipe can't hang us
    out = bufs["out"]
    err = bufs["err"].decode(errors="replace")[-20000:]
    code = proc.returncode
    if code != 0:
        out_tail = out.decode(errors="replace")[-20000:]
        raise RuntimeError(f"{argv[:2]} exited {code}\nSTDERR:\n{err}\nSTDOUT:\n{out_tail}")
    own_cpu, reaped_cpu = cpu if cpu else (ru.ru_utime + ru.ru_stime, float("nan"))
    if detail is not None:
        detail["phases"] = parse_phases(bufs["err"].decode(errors="replace"))
        detail["browser_cpu_s"] = browser.cpu_s
        detail["browser_pss_kb"] = browser.peak_pss_kb
        detail["cpu_reaped_s"] = reaped_cpu
        detail["wait4_cpu_s"] = ru.ru_utime + ru.ru_stime
        detail["wait4_maxrss_kb"] = ru.ru_maxrss
    return wall, own_cpu, hwm.peak_kb or ru.ru_maxrss, out


def percentile(xs: list[float], q: float) -> float:
    if not xs:
        return float("nan")
    s = sorted(xs)
    return s[min(len(s) - 1, int(len(s) * q))]


def bench_session(name: str, argv: list[str], env: dict, out_dir: Path,
                  reps, chrome_tag: str, timeout_s: int) -> dict:
    """`reps`: how many, or which rep numbers to run (the ladder interleaves
    contenders one rep at a time); likewise `rep_ids` below."""
    runs = []
    for rep in (range(reps) if isinstance(reps, int) else reps):
        extract = out_dir / f"extract_a-{name}-r{rep}.json"
        shot = out_dir / f"shot-{name}-r{rep}.png"
        stats = out_dir / f"stats-{name}-r{rep}.json"
        run_env = {
            **env,
            "LADDER_EXTRACT_OUT": str(extract),
            "LADDER_SHOT_OUT": str(shot),
        }
        if is_nv(name):
            run_env["LADDER_STATS_OUT"] = str(stats)
            cmd = [sys.executable, str(CONTENDERS / "nv_serve.py"),
                   "--mode", "session", "--stats-out", str(stats),
                   "--extract-out", str(extract), "--shot-out", str(shot),
                   *NAVIGERA_ENGINE_ARGS[name]]
        else:
            cmd = argv
        detail: dict = {}
        wall, cpu, rss, out = run_once(cmd, run_env, timeout_s, chrome_tag, detail)
        if is_nv(name):
            # navigera's own CPU/RSS come from nv_serve.py, which measures
            # its navigera child exactly like run_once measures drivers.
            with open(stats, encoding="utf-8") as f:
                bst = json.load(f)
            # Tool wall is the navigera process's own wall (launch ->
            # quit); the Python parent's spawn overhead is excluded, matching
            # the node contenders where wall == driver process wall.
            wall, cpu, rss = bst["wall_s"], bst["cpu_s"], bst["maxrss_kb"]
            detail["cpu_reaped_s"] = bst.get("cpu_reaped_s", float("nan"))
            detail["phases"] = bst.get("phases") or {}
            counts = bst["counts"]
            if os.environ.get("LADDER_PROFILE") == "1":
                print(out.decode(), flush=True)
        else:
            counts = json.loads(out.decode().splitlines()[0])["counts"]
        runs.append({
            "wall_s": wall, "cpu_s": cpu, "maxrss_kb": rss,
            "cpu_reaped_s": detail.get("cpu_reaped_s", float("nan")),
            "phases": detail.get("phases") or {},
            "browser_cpu_s": detail.get("browser_cpu_s", float("nan")),
            "browser_pss_kb": detail.get("browser_pss_kb", 0),
            "counts": counts, "extract": str(extract),
        })
        print(f"  [session] {name} rep {rep + 1}: "
              f"wall {wall:.2f}s driver-cpu {cpu:.2f}s", flush=True)
    best = min(runs, key=lambda r: r["wall_s"])
    return {"runs": runs, "best": best}


def bench_eval(name: str, argv: list[str], env: dict, out_dir: Path,
               chrome_tag: str, timeout_s: int, rep_ids=range(3)) -> dict:
    """Warm eval round-trip: 200 evals, report mean/p95 of the best session."""
    sessions = []
    for rep in rep_ids:
        stats = out_dir / f"evalstats-{name}-r{rep}.json"
        run_env = {**env, "LADDER_N_EVAL": "200"}
        if is_nv(name):
            run_env["LADDER_STATS_OUT"] = str(stats)
            cmd = [sys.executable, str(CONTENDERS / "nv_serve.py"),
                   "--mode", "eval", "--stats-out", str(stats), "--n-eval", "200",
                   *NAVIGERA_ENGINE_ARGS[name]]
        else:
            cmd = argv + ["eval"]
        wall, cpu, rss, out = run_once(cmd, run_env, timeout_s, chrome_tag)
        engine: list[float] = []
        if is_nv(name):
            with open(stats, encoding="utf-8") as f:
                st = json.load(f)
            lat = st["eval_ms"]
            engine = st.get("eval_engine_ms") or []
        else:
            lat = json.loads(out.decode().splitlines()[1])["eval_ms"]
        sessions.append({"mean_ms": sum(lat) / len(lat),
                         "p95_ms": percentile(lat, 0.95),
                         "wall_s": wall, "n": len(lat),
                         # navigera's own (engine-side) round trip, without
                         # the stdin/stdout hop to its client.
                         **({"engine_mean_ms": sum(engine) / len(engine),
                             "engine_p95_ms": percentile(engine, 0.95)} if engine else {})})
        print(f"  [eval] {name} rep {rep + 1}/3: "
              f"mean {sum(lat) / len(lat):.2f} ms p95 {percentile(lat, 0.95):.2f} ms",
              flush=True)
    best = min(sessions, key=lambda s: s["mean_ms"])
    return {"sessions": sessions, "best": best}


def bench_realworld(name: str, argv: list[str], env: dict, out_dir: Path,
                    chrome_tag: str, rep_ids=range(3)) -> dict:
    """Real-world scenario on https://example.com (all contenders).

    goto -> title/h1/paragraph evals -> quit. Checks the extracted facts and
    reports wall time. Best of 3. Network failures mark ok=False instead of
    failing the whole ladder (external dependency).
    """
    runs = []
    for rep in rep_ids:
        stats = out_dir / f"realworld-{name}-r{rep}.json"
        run_env = {**env, "LADDER_STATS_OUT": str(stats)}
        if is_nv(name):
            cmd = [sys.executable, str(CONTENDERS / "nv_serve.py"),
                   "--mode", "realworld", "--stats-out", str(stats),
                   *NAVIGERA_ENGINE_ARGS[name]]
        else:
            cmd = argv + ["realworld"]
        try:
            wall, cpu, rss, out = run_once(cmd, run_env, 120, chrome_tag)
        except RuntimeError as e:
            print(f"  [realworld] {name} rep {rep + 1}/3: FAILED ({e})", flush=True)
            runs.append({"wall_s": float("nan"), "cpu_s": float("nan"),
                         "maxrss_kb": 0, "ok": False, "title": None, "h1": None})
            continue
        if is_nv(name):
            with open(stats, encoding="utf-8") as f:
                doc = json.load(f)
            rw = doc["realworld"]
            # navigera's own wall, as the session row takes it: the Python
            # parent's start-up is not navigera's.
            wall, cpu, rss = doc["wall_s"], doc["cpu_s"], doc["maxrss_kb"]
        else:
            rw = json.loads(out.decode().splitlines()[0])["realworld"]
        # Title is the primary correctness signal; h1 may be missing if the
        # DOM isn't fully parsed yet when we eval (static page, no JS).
        ok = rw.get("title") == "Example Domain"
        runs.append({"wall_s": wall, "cpu_s": cpu, "maxrss_kb": rss,
                     "ok": ok, "title": rw.get("title"), "h1": rw.get("h1")})
        print(f"  [realworld] {name} rep {rep + 1}/3: wall {wall:.2f}s ok={ok}", flush=True)
    ok_runs = [r for r in runs if r["ok"]]
    best = min(ok_runs, key=lambda r: r["wall_s"]) if ok_runs else runs[0]
    return {"runs": runs, "best": best}


def bench_browse(name: str, argv: list[str], env: dict, out_dir: Path,
                 chrome_tag: str, rep_ids=range(3)) -> dict:
    """Real-world gentle browsing: GitHub awesome list + trending (all contenders).

    goto -> scroll in steps -> extract -> goto trending -> click-through.
    Correctness: awesome title contains "awesome", links > 100, scroll
    positions strictly increasing, trending has repo cards, top-3 names
    non-empty, click navigates to a github.com/<owner>/<repo> URL.
    Network failures mark ok=False instead of failing the ladder.
    Best of 3.
    """
    runs = []
    for rep in rep_ids:
        stats = out_dir / f"browse-{name}-r{rep}.json"
        run_env = {**env, "LADDER_STATS_OUT": str(stats)}
        if is_nv(name):
            cmd = [sys.executable, str(CONTENDERS / "nv_serve.py"),
                   "--mode", "browse", "--stats-out", str(stats),
                   *NAVIGERA_ENGINE_ARGS[name]]
        else:
            cmd = argv + ["browse"]
        try:
            wall, cpu, rss, out = run_once(cmd, run_env, 180, chrome_tag)
        except RuntimeError as e:
            print(f"  [browse] {name} rep {rep + 1}/3: FAILED ({e})", flush=True)
            runs.append({"wall_s": float("nan"), "cpu_s": float("nan"),
                         "maxrss_kb": 0, "ok": False})
            continue
        if is_nv(name):
            with open(stats, encoding="utf-8") as f:
                doc = json.load(f)
            facts = doc["realworld"]["facts"]
            # navigera's own wall, as the session row takes it.
            wall, cpu, rss = doc["wall_s"], doc["cpu_s"], doc["maxrss_kb"]
        else:
            facts = json.loads(out.decode().splitlines()[0])["realworld"]["facts"]
        reason = browse_failure(facts)
        ok = reason is None
        runs.append({"wall_s": wall, "cpu_s": cpu, "maxrss_kb": rss, "ok": ok,
                     "reason": reason,
                     "facts": {k: v for k, v in facts.items()
                               if k in ("awesome_title", "trending_title",
                                        "trending_top3", "trending_clicked_url",
                                        "awesome_scroll_ys")}})
        print(f"  [browse] {name} rep {rep + 1}/3: wall {wall:.2f}s ok={ok}"
              + (f" ({reason})" if reason else ""), flush=True)
    ok_runs = [r for r in runs if r["ok"]]
    best = min(ok_runs, key=lambda r: r["wall_s"]) if ok_runs else runs[0]
    return {"runs": runs, "best": best}


def browse_failure(f: dict) -> str | None:
    """Why the gentle-browsing facts fail validation (None = they pass).

    Returning the reason, not just a bool, is what lets a failing live-site
    run be diagnosed from the results alone."""
    try:
        if "awesome" not in (f.get("awesome_title") or "").lower():
            return f"awesome title {f.get('awesome_title')!r}"
        if not isinstance(f.get("awesome_links"), int) or f["awesome_links"] < 100:
            return f"awesome links {f.get('awesome_links')!r}"
        # Each contender scrolls with behavior:'instant'. GitHub sets CSS
        # scroll-behavior:smooth, so a plain scrollBy() is still animating
        # when scrollY is read ([0, 0, 724, ...]) and the check raced it.
        ys = f.get("awesome_scroll_ys") or []
        if len(ys) != 5 or not all(b > a for a, b in zip(ys, ys[1:])):
            return f"scroll ys not increasing {ys!r}"
        if not isinstance(f.get("trending_repos"), int) or f["trending_repos"] < 1:
            return f"trending repos {f.get('trending_repos')!r} (title {f.get('trending_title')!r})"
        top3 = f.get("trending_top3") or []
        if len(top3) != 3 or not all(isinstance(t, str) and "/" in t for t in top3):
            return f"trending top3 {top3!r}"
        url = f.get("trending_clicked_url") or ""
        if "github.com/" not in url or "trending" in url:
            return f"clicked url {url!r}"
        return None
    except Exception as e:
        return f"facts unreadable: {e}"


def check_browse_facts(f: dict) -> bool:
    """Validate the gentle-browsing scenario facts."""
    return browse_failure(f) is None


def bench_cold(name: str, argv: list[str], env: dict, chrome_tag: str,
               rep_ids=range(5)) -> dict:
    """Cold start: launch -> new page -> one eval -> close. Best of 5."""
    runs = []
    for rep in rep_ids:
        if is_nv(name):
            # one-shot mode: process start + browser launch + one eval + close
            # (nv_serve.py-only args like --navigera don't apply here)
            extra = [a for a in NAVIGERA_ENGINE_ARGS[name] if a not in ("--navigera", NAVIGERA_BINARY.get(name))]
            cmd = [NAVIGERA_BINARY.get(name, env["NAVIGERA"]), "eval", "--expression",
                   "() => 1 + 1", *extra]
            # nv-serve / nv-baseline (chrome) need --chromium; variants carry it
            if name in ("nv-serve", "nv-baseline"):
                cmd += ["--chromium", env["CHROME_BIN"]]
            run_env = {**env, "NAVIGERA_TIMINGS": "1"}
        else:
            cmd = argv + ["cold"]
            run_env = env
        detail: dict = {}
        wall, cpu, rss, _ = run_once(cmd, run_env, 120, chrome_tag, detail)
        runs.append({"wall_s": wall, "cpu_s": cpu, "maxrss_kb": rss,
                     "phases": detail.get("phases") or {}})
        print(f"  [cold] {name} rep {rep + 1}/5: wall {wall:.2f}s", flush=True)
    best = min(runs, key=lambda r: r["wall_s"])
    return {"runs": runs, "best": best}


AGENT_DAEMON_MARKERS = (b"agent-browser", b"playwright-cli", b"@playwright/cli",
                        b"playwright-mcp", b"@playwright/mcp", b"chrome-devtools-mcp",
                        # navigera's shared-browser host (0.6.5+) outlives a
                        # session's `quit` by its grace period, like the
                        # daemons above.
                        b"browser-host")

# Agent steps outside the canonical session (cli_agent.EXTRA_OPS): reported
# on their own, left out of the total and the per-step mean.
AGENT_EXTRA_OPS = ("snapshot b", "start warm", "quit warm")


def kill_agent_daemons() -> None:
    """agent-browser / playwright-cli leave their daemons idling after
    `close`, and navigera its shared browser's host; stop them (and any MCP
    server) so each rep starts cold."""
    me = os.getpid()
    for pid, (_, name) in proc_table().items():
        if pid == me:
            continue
        hit = name.startswith("agent-browser")
        if not hit:
            try:
                with open(f"/proc/{pid}/cmdline", "rb") as f:
                    cmd = f.read()
                hit = any(m in cmd for m in AGENT_DAEMON_MARKERS) and b"cli_agent.py" not in cmd
            except OSError:
                continue
        if hit:
            try:
                os.kill(pid, 9)
            except (ProcessLookupError, PermissionError):
                pass


def bench_agent(tools: dict[str, list[str]], env: dict, out_dir: Path,
                chrome_tag: str, expected: dict, base_extract: str | None) -> dict:
    """Agent-style CLI scenario (contenders/cli_agent.py): every step is its own
    process against a warm background session. Best of 3 by total wall."""
    # Interleaved like the ladder's reps: each tool's first run, then each
    # tool's second in the reverse order, then the third.
    runs_by: dict[str, list] = {name: [] for name in tools}
    for rep in range(3):
        for name in (list(tools) if rep % 2 == 0 else list(reversed(list(tools)))):
            extra = tools[name]
            runs = runs_by[name]
            kill_stray_chrome(chrome_tag)
            kill_agent_daemons()
            extract = out_dir / f"agent-extract-{name}-r{rep}.json"
            shot = out_dir / f"agent-shot-{name}-r{rep}.png"
            cmd = [sys.executable, str(CONTENDERS / "cli_agent.py"), *extra,
                   "--extract-out", str(extract), "--shot-out", str(shot)]
            try:
                _, _, _, out = run_once(cmd, env, 300, chrome_tag)
            except RuntimeError as e:
                print(f"  [agent] {name} rep {rep + 1}/3: FAILED ({e})", flush=True)
                runs.append({"ok": False, "error": " ".join(str(e).split())[-400:]})
                continue
            doc = json.loads(out.decode().strip().splitlines()[-1])
            counts = doc["counts"]
            exp = {"title_a": expected["title_a"], "cards_a": expected["cards_a"],
                   "visible": expected["visible_after_filter_widget"],
                   "title_b": expected["title_b"], "rows_b": expected["rows_b"]}
            ok = counts == exp and (base_extract is None
                                    or normalized_extract(str(extract)) == base_extract)
            ops = [s["ms"] for s in doc["steps"]
                   if s["op"] not in ("start", "close", "exit") + AGENT_EXTRA_OPS]
            runs.append({"ok": ok, "wall_s": doc["wall_s"], "steps": doc["steps"],
                         "total_wall_s": doc.get("total_wall_s"), "calls": doc.get("calls"),
                         "op_mean_ms": sum(ops) / len(ops),
                         "snapshot_bytes": doc["snapshot_bytes"],
                         "snapshot_b_bytes": doc.get("snapshot_b_bytes"), "counts": counts})
            print(f"  [agent] {name} rep {rep + 1}/3: wall {doc['wall_s']:.2f}s "
                  f"op mean {sum(ops) / len(ops):.1f} ms ok={ok}", flush=True)
    kill_agent_daemons()
    results = {}
    for name, runs in runs_by.items():
        good = [r for r in runs if r.get("ok")]
        best = min(good, key=lambda r: r["wall_s"]) if good else (runs[0] if runs else {})
        results[name] = {"runs": runs, "best": best}
    return results


def bench_parallel(tools: dict[str, str], env: dict, chrome_tag: str) -> dict:
    """Parallel agents (contenders/parallel.py): N sessions of one tool open
    at once, each on the listing page after a snapshot. Memory is every
    process the run started (browsers, daemons, MCP servers, session
    servers), summed as PSS so shared pages count once, at its peak over a
    second of sampling; the driver script itself is left out."""
    results: dict = {}
    for label, tool in tools.items():
        rows = []
        for n in PARALLEL_NS:
            kill_stray_chrome(chrome_tag)
            kill_agent_daemons()
            before = set(proc_table())
            proc = subprocess.Popen([sys.executable, str(CONTENDERS / "parallel.py"), "--tool", tool,
                                     "--n", str(n)], env=env, stdin=subprocess.PIPE,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            row = {"n": n}
            try:
                line = proc.stdout.readline()
                ready = json.loads(line) if line.strip() else {}
                if not ready.get("ready"):
                    raise RuntimeError(f"not ready: {line.strip()[:200]} {proc.stderr.read()[-600:]}")
                peak_kb, browsers = 0, 0
                for _ in range(5):
                    table = proc_table()
                    new = [pid for pid in table if pid not in before and pid != proc.pid]
                    peak_kb = max(peak_kb, sum(pss_kb(pid) or 0 for pid in new))
                    browsers = max(browsers, len({pid for pid in new
                                                  if table[pid][1].startswith(BROWSER_PREFIXES)}))
                    time.sleep(0.25)
                row.update({"pss_mb": round(peak_kb / 1024, 1), "browser_procs": browsers,
                            "up_s": round(ready["up_s"], 3), "session_ms": ready["session_ms"]})
                proc.stdin.write("\n")
                proc.stdin.flush()
                proc.wait(timeout=120)
            except Exception as e:
                proc.kill()
                proc.wait()
                row["error"] = " ".join(str(e).split())[-400:]
            print(f"  [parallel] {label} x{n}: " + (f"{row['pss_mb']:.0f} MB, up {row['up_s']:.2f}s"
                                                    if "pss_mb" in row else f"FAILED {row['error']}"), flush=True)
            rows.append(row)
        results[label] = rows
    kill_stray_chrome(chrome_tag)
    kill_agent_daemons()
    return results


def collect_versions() -> dict[str, str]:
    """The version of every contender, tool and browser this run measured.

    Contenders come from semver ranges or @latest (the weekly run measures
    the newest releases), so without this a change between two runs can't
    be traced to a release."""
    import re
    import shutil

    def first_line(argv: list[str]) -> str:
        try:
            r = subprocess.run(argv, capture_output=True, text=True, timeout=30)
        except Exception:
            return ""
        text = (r.stdout or r.stderr or "").strip()
        return text.splitlines()[0].strip() if text else ""

    v: dict[str, str] = {"navigera": first_line([os.environ["NAVIGERA"], "--version"])}
    if "nv-baseline" in NAVIGERA_ENGINE_ARGS:
        v["nv-baseline"] = first_line([NAVIGERA_BINARY["nv-baseline"], "--version"])
    for label, env_var in (("chrome", "CHROME_BIN"), ("edge", "EDGE_BIN"), ("brave", "BRAVE_BIN"),
                           ("chrome-headless-shell", "SHELL_BIN")):
        binary = os.environ.get(env_var)
        if binary and Path(binary).exists():
            v[label] = first_line([binary, "--version"])
    lp = shutil.which("lightpanda")
    if lp:
        v["lightpanda"] = first_line([lp, "version"])
    for pkg in ("playwright-core", "puppeteer-core"):
        try:
            v[pkg] = json.loads((HERE / "node_modules" / pkg / "package.json").read_text())["version"]
        except (OSError, ValueError, KeyError):
            pass
    for crate in ("chromiumoxide", "chromey"):
        try:
            lock = (CONTENDERS / crate / "Cargo.lock").read_text()
            m = re.search(r'\[\[package\]\]\nname = "' + crate + r'"\nversion = "([^"]+)"', lock)
            if m:
                v[crate] = m.group(1)
        except OSError:
            pass
    for label, module, d in (("chromedp", "github.com/chromedp/chromedp", "chromedp"),
                             ("go-rod", "github.com/go-rod/rod", "gorod")):
        try:
            m = re.search(re.escape(module) + r"\s+(v\S+)", (CONTENDERS / d / "go.mod").read_text())
            if m:
                v[label] = m.group(1)
        except OSError:
            pass
    npm = shutil.which("npm")
    if npm:
        try:
            r = subprocess.run([npm, "ls", "-g", "--depth=0", "--json"], capture_output=True,
                               text=True, timeout=60)
            deps = json.loads(r.stdout or "{}").get("dependencies") or {}
            for pkg in ("agent-browser", "@playwright/cli", "@playwright/mcp", "chrome-devtools-mcp"):
                if deps.get(pkg, {}).get("version"):
                    v[pkg] = deps[pkg]["version"]
        except Exception:
            pass
    return v


def normalized_extract(path: str) -> str:
    with open(path, encoding="utf-8") as f:
        data = json.load(f)
    return json.dumps(data, sort_keys=True)


def check_correctness(results: dict, expected: dict) -> None:
    """All contenders must agree with each other and with expected.json."""
    names = list(results)
    base_counts = results[names[0]]["session"]["best"]["counts"]
    for key in ("title_a", "cards_a", "visible", "title_b", "rows_b"):
        exp = expected[{"title_a": "title_a", "cards_a": "cards_a",
                        "visible": "visible_after_filter_widget",
                        "title_b": "title_b", "rows_b": "rows_b"}[key]]
        assert base_counts[key] == exp, f"{names[0]} {key}={base_counts[key]} != expected {exp}"
    base_extract = normalized_extract(results[names[0]]["session"]["best"]["extract"])
    for name in names[1:]:
        try:
            counts = results[name]["session"]["best"]["counts"]
            for key in ("title_a", "cards_a", "visible", "title_b", "rows_b"):
                assert counts[key] == base_counts[key], \
                    f"counts mismatch {name}.{key}: {counts[key]} != {base_counts[key]}"
            ext = normalized_extract(results[name]["session"]["best"]["extract"])
            assert ext == base_extract, f"extract_a.json mismatch for {name}"
            results[name]["correct"] = True
        except AssertionError as e:
            if name not in EXPERIMENTAL:
                raise
            results[name]["correct"] = False
            NOTES.setdefault(name, []).append(f"correctness gate failed: {e}")
    results[names[0]]["correct"] = True
    print("correctness: all gated contenders agree (counts + 500-card extract identical)")


class QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass


def write_publish_json(out_dir: Path, results: dict, contenders: dict,
                       reps: int, env_text: str, agent_results: dict | None = None,
                       versions: dict | None = None, parallel_results: dict | None = None) -> None:
    """Compact, chart-ready results (publish.json): what a results page or a
    remote agent needs, small enough to ship as check-run annotations."""
    def r3(x):
        return None if x is None or (isinstance(x, float) and math.isnan(x)) else round(x, 4)

    rows = []
    for name in contenders:
        r = results[name]
        sb = r["session"]["best"]
        row = {
            "name": name,
            "experimental": name in EXPERIMENTAL,
            "correct": r.get("correct", False),
            "session": {
                "wall_s": r3(sb["wall_s"]),
                "median_wall_s": r3(statistics.median(x["wall_s"] for x in r["session"]["runs"])),
                "runs_wall_s": [r3(x["wall_s"]) for x in r["session"]["runs"]],
                "driver_cpu_s": r3(sb["cpu_s"]),
                "reaped_child_cpu_s": r3(sb.get("cpu_reaped_s")),
                "driver_rss_mb": r3(sb["maxrss_kb"] / 1024),
                "browser_cpu_s": r3(sb["browser_cpu_s"]),
                "browser_pss_mb": r3(sb["browser_pss_kb"] / 1024),
            },
        }
        if sb.get("phases"):
            row["session"]["phases_ms"] = sb["phases"]
        if r.get("eval"):
            eb = r["eval"]["best"]
            row["eval"] = {"mean_ms": r3(eb["mean_ms"]), "p95_ms": r3(eb["p95_ms"]),
                           "runs_mean_ms": [r3(x["mean_ms"]) for x in r["eval"]["sessions"]]}
            if "engine_mean_ms" in eb:
                row["eval"]["engine_mean_ms"] = r3(eb["engine_mean_ms"])
                row["eval"]["engine_p95_ms"] = r3(eb["engine_p95_ms"])
        if r.get("cold"):
            cb = r["cold"]["best"]
            row["cold"] = {"wall_s": r3(cb["wall_s"]),
                           "median_wall_s": r3(statistics.median(x["wall_s"] for x in r["cold"]["runs"])),
                           "runs_wall_s": [r3(x["wall_s"]) for x in r["cold"]["runs"]]}
            if cb.get("phases"):
                row["cold"]["phases_ms"] = cb["phases"]
        for key in ("realworld", "browse"):
            if r.get(key):
                b = r[key]["best"]
                row[key] = {"wall_s": r3(b.get("wall_s")), "ok": bool(b.get("ok")),
                            "passed_reps": sum(1 for x in r[key]["runs"] if x.get("ok")),
                            "reps": len(r[key]["runs"])}
                reasons = [x.get("reason") for x in r[key]["runs"] if x.get("reason")]
                if reasons:
                    row[key]["fail_reasons"] = reasons
        rows.append(row)
    doc = {
        "generated_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "run_id": os.environ.get("GITHUB_RUN_ID"),
        "commit": os.environ.get("GITHUB_SHA"),
        "env": env_text.replace("**", ""),
        "reps": reps,
        "versions": versions or {},
        "contenders": rows,
        "notes": NOTES,
        "skipped": SKIPPED,
        "agent_cli": [
            {"name": name,
             "ok": bool(r["best"].get("ok")),
             "wall_s": r3(r["best"].get("wall_s")),
             "runs_wall_s": [r3(x.get("wall_s")) for x in r["runs"]],
             "op_mean_ms": r3(r["best"].get("op_mean_ms")),
             "total_wall_s": r3(r["best"].get("total_wall_s")),
             "calls": r["best"].get("calls"),
             "snapshot_bytes": r["best"].get("snapshot_bytes"),
             "snapshot_b_bytes": r["best"].get("snapshot_b_bytes"),
             "steps": r["best"].get("steps"),
             **({"errors": [x["error"] for x in r["runs"] if x.get("error")]}
                if any(x.get("error") for x in r["runs"]) else {})}
            for name, r in (agent_results or {}).items()
        ],
        "parallel": [{"name": name, "runs": rows} for name, rows in (parallel_results or {}).items()],
    }
    # indent=1 keeps lines short so annotate.py can chunk it.
    (out_dir / "publish.json").write_text(json.dumps(doc, indent=1) + "\n", encoding="utf-8")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out-dir", required=True)
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--only", default="",
                    help="comma-separated contenders to run (default: all), "
                         "e.g. nv-serve,gorod — for fast, narrow CI iterations")
    ap.add_argument("--scenarios", default=",".join(OPTIONAL_SCENARIOS),
                    help="optional stages besides the always-on session gate: "
                         + ",".join(OPTIONAL_SCENARIOS))
    args = ap.parse_args()
    only = {x.strip() for x in args.only.split(",") if x.strip()}
    scenarios = {x.strip() for x in args.scenarios.split(",") if x.strip()}
    unknown = scenarios - set(OPTIONAL_SCENARIOS)
    if unknown:
        ap.error(f"unknown scenarios: {sorted(unknown)}")

    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    navigera = os.environ["NAVIGERA"]
    chrome_bin = os.environ["CHROME_BIN"]
    chrome_tag = os.path.basename(chrome_bin)

    # Fixture server on 127.0.0.1, ephemeral port.
    # Threaded: a single-threaded server blocks on the first connection it
    # accepts, and Chrome opens connections it may not use yet (preconnect),
    # so a page's request could wait behind an idle socket until Chrome
    # dropped it.
    socketserver.ThreadingTCPServer.daemon_threads = True
    server = socketserver.ThreadingTCPServer(
        ("127.0.0.1", 0),
        lambda *a, **k: QuietHandler(*a, directory=str(FIXTURES), **k),
    )
    port = server.server_address[1]
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{port}"
    print(f"fixture server: {base}", flush=True)

    env = {
        **os.environ,
        "LADDER_BASE": base,
        "CHROME_BIN": chrome_bin,
        "NAVIGERA": navigera,
        "DISABLE_TELEMETRY": "1",
        "DO_NOT_TRACK": "1",
    }
    node_dir = CONTENDERS
    # navigera engine variants: name -> (engine flag, env var for binary).
    # Variants are skipped gracefully when their binary is unavailable.
    NAVIGERA_ENGINES = {
        "nv-serve": ("chrome", "CHROME_BIN"),
        "nv-baseline": ("chrome", "CHROME_BIN"),
        "nv-edge": ("chrome", "EDGE_BIN"),
        "nv-brave": ("chrome", "BRAVE_BIN"),
        "nv-shell": ("chrome", "SHELL_BIN"),
        # A/B of the CDP transport in one run: same build over a DevTools
        # WebSocket port instead of the default --remote-debugging-pipe.
        "nv-ws": ("chrome", "CHROME_BIN"),
        "nv-lightpanda": ("lightpanda", None),
    }
    contenders: dict[str, list[str]] = {}
    nv_engine_args: dict[str, list[str]] = {}
    for nv_name, (engine, bin_env) in NAVIGERA_ENGINES.items():
        NAVIGERA_BINARY[nv_name] = navigera
        if nv_name == "nv-baseline":
            baseline = os.environ.get("NAVIGERA_BASELINE")
            if not baseline or not Path(baseline).exists():
                continue  # A/B only on request; not "skipped"
            NAVIGERA_BINARY[nv_name] = baseline
            contenders[nv_name] = []
            NAVIGERA_ENGINE_ARGS[nv_name] = ["--navigera", baseline]
            continue
        if bin_env is not None:
            binary = os.environ.get(bin_env)
            if not binary or not Path(binary).exists():
                print(f"== {nv_name}: skipped (no binary: {bin_env}={binary})", flush=True)
                SKIPPED[nv_name] = f"no binary ({bin_env}={binary})"
                continue
            # nv-serve (chrome) uses CHROME_BIN via env; others pass explicitly
            if nv_name == "nv-serve":
                args_extra = []
            elif nv_name == "nv-ws":
                args_extra = ["--transport", "ws"]
            else:
                args_extra = ["--engine", engine, "--chromium", binary]
        else:
            # lightpanda: needs a working `lightpanda` binary; skip if absent
            # or broken (nightly builds can be flaky).
            import shutil
            import subprocess
            lp = shutil.which("lightpanda")
            lp_ok = False
            if lp:
                try:
                    r = subprocess.run([lp, "version"], capture_output=True,
                                       timeout=10)
                    lp_ok = r.returncode == 0
                except Exception:
                    lp_ok = False
            if not lp_ok:
                print("== nv-lightpanda: skipped (no working lightpanda binary)", flush=True)
                SKIPPED[nv_name] = "no working lightpanda binary"
                continue
            args_extra = ["--engine", engine]
        contenders[nv_name] = []
        NAVIGERA_ENGINE_ARGS[nv_name] = args_extra
    contenders.update({
        "playwright": ["node", str(node_dir / "contender_playwright.mjs")],
        "puppeteer": ["node", str(node_dir / "contender_puppeteer.mjs")],
        "chromiumoxide": [str(CONTENDERS / "chromiumoxide" / "target" / "release"
                              / "ladder-chromiumoxide")],
        "chromey": [str(CONTENDERS / "chromey" / "target" / "release" / "ladder-chromey")],
        "chromedp": [str(CONTENDERS / "chromedp" / "ladder-chromedp")],
        "gorod": [str(CONTENDERS / "gorod" / "ladder-gorod")],
    })

    with open(FIXTURES / "expected.json", encoding="utf-8") as f:
        expected = json.load(f)

    # Path-based contenders whose build step failed (optional ones are
    # continue-on-error) are reported, not fatal.
    for name, argv in list(contenders.items()):
        if argv and os.path.isabs(argv[0]) and not Path(argv[0]).exists():
            SKIPPED[name] = f"binary not built ({argv[0]})"
            contenders.pop(name)

    if only:
        missing = only - set(contenders)
        if missing:
            print(f"== --only names not available (skipped or unknown): {sorted(missing)}", flush=True)
        for name in list(contenders):
            if name not in only:
                contenders.pop(name)
                NAVIGERA_ENGINE_ARGS.pop(name, None)

    # Warm each browser binary once (page cache, font cache, first-run
    # profile work) so the first contender doesn't absorb a cold-start
    # penalty the others never pay (it showed up as an 8 s first rep).
    for bin_path in {chrome_bin, os.environ.get("EDGE_BIN"), os.environ.get("BRAVE_BIN"),
                     os.environ.get("SHELL_BIN")}:
        if bin_path and Path(bin_path).exists():
            try:
                subprocess.run([bin_path, "--headless=new", "--no-sandbox",
                                "--disable-gpu", "--dump-dom", "about:blank"],
                               capture_output=True, timeout=60)
                print(f"warmup: {os.path.basename(bin_path)} ok", flush=True)
            except Exception as e:  # warmup is best-effort
                print(f"warmup: {bin_path} failed: {e}", flush=True)

    # Reps are interleaved: every contender's first rep, then every
    # contender's second in the reverse order, and so on. A runner that
    # drifts during the run (a noisy neighbour, throttling) then shifts every
    # contender alike, instead of whichever ran during the slow minutes.
    def run_stage(key: str, name: str, argv: list[str], tag: str, rep: int) -> list:
        if key == "session":
            return bench_session(name, argv + (["session"] if not is_nv(name) else []),
                                 env, out_dir, [rep], tag, 180)["runs"]
        if key == "eval":
            return bench_eval(name, argv, env, out_dir, tag, 180, [rep])["sessions"]
        if key == "cold":
            return bench_cold(name, argv, env, tag, [rep])["runs"]
        if key == "realworld":
            return bench_realworld(name, argv, env, out_dir, tag, [rep])["runs"]
        return bench_browse(name, argv, env, out_dir, tag, [rep])["runs"]

    def finish(key: str, runs: list) -> dict:
        if key == "eval":
            return {"sessions": runs, "best": min(runs, key=lambda r: r["mean_ms"])}
        if key in ("realworld", "browse"):
            ok_runs = [r for r in runs if r["ok"]]
            return {"runs": runs,
                    "best": min(ok_runs, key=lambda r: r["wall_s"]) if ok_runs else runs[0]}
        return {"runs": runs, "best": min(runs, key=lambda r: r["wall_s"])}

    stages = [("session", "session", args.reps), ("eval", "eval micro", 3),
              ("cold", "cold start", 5), ("realworld", "realworld example.com", 3),
              ("browse", "browse github", 3)]
    parts: dict[str, dict[str, list]] = {name: {} for name in contenders}
    failed: set[tuple[str, str]] = set()
    for key, label, n in stages:
        if key != "session" and key not in scenarios:
            continue
        for rep in range(n):
            order = list(contenders) if rep % 2 == 0 else list(reversed(list(contenders)))
            print(f"== {label}: rep {rep + 1}/{n}: " + ", ".join(order), flush=True)
            for name in order:
                if name not in contenders or (name, key) in failed:
                    continue
                tag = BROWSER_TAGS.get(name, chrome_tag)
                kill_stray_chrome(chrome_tag)
                if tag != chrome_tag:
                    kill_stray_chrome(tag)
                try:
                    parts[name].setdefault(key, []).extend(
                        run_stage(key, name, contenders[name], tag, rep))
                except Exception as e:
                    # Experimental contenders: a failure is recorded instead
                    # of failing the whole ladder; a failed session drops it.
                    if name not in EXPERIMENTAL:
                        raise
                    if key == "session":
                        print(f"== {name}: SKIPPED (session failed: {e})", flush=True)
                        # Tail of the error: where navigera's own message lands.
                        SKIPPED[name] = " ".join(str(e).split())[-700:]
                        NAVIGERA_ENGINE_ARGS.pop(name, None)
                        contenders.pop(name)
                        parts.pop(name, None)
                        continue
                    NOTES.setdefault(name, []).append(
                        f"{key} failed: {' '.join(str(e).split())[-300:]}")
                    failed.add((name, key))
                    parts[name].pop(key, None)
    results: dict = {}
    for name in contenders:
        entry = {key: finish(key, runs) for key, runs in parts[name].items() if runs}
        # stash shot path for the report
        entry["session"]["best"]["shot"] = str(out_dir / f"shot-{name}-r0.png")
        results[name] = entry

    check_correctness(results, expected)

    agent_results: dict = {}
    if "agent" in scenarios:
        import shutil
        tools: dict[str, list[str]] = {}
        if "nv-serve" in contenders:
            tools["navigera"] = ["--tool", "nv"]
        # `--only` narrows the main contenders; the agent scenario always
        # runs every agent CLI that is installed.
        shell = os.environ.get("SHELL_BIN")
        if "nv-serve" in contenders and shell and Path(shell).exists():
            tools["navigera (headless shell)"] = ["--tool", "nv", "--chrome", shell]
        if "nv-serve" in contenders:
            # The skill's way (known steps in one `batch` call), and two
            # diagnostics kept out of the README: a browser of the session's
            # own instead of the shared one, and the A/B baseline build.
            tools["navigera (batch)"] = ["--tool", "nvb"]
            tools["navigera (own browser)"] = ["--tool", "nv", "--own-browser"]
            if "nv-baseline" in contenders:
                tools["navigera (baseline)"] = ["--tool", "nv", "--navigera",
                                                NAVIGERA_BINARY["nv-baseline"]]
        for label, tool, env_var, binary, hint in [
            ("agent-browser", "ab", "AGENT_BROWSER", "agent-browser", "npm i -g agent-browser"),
            ("playwright-cli", "pw", "PLAYWRIGHT_CLI", "playwright-cli", "npm i -g @playwright/cli"),
            ("Playwright MCP", "pwmcp", "PLAYWRIGHT_MCP", "playwright-mcp", "npm i -g @playwright/mcp"),
            ("Chrome DevTools MCP", "cdmcp", "CHROME_DEVTOOLS_MCP", "chrome-devtools-mcp",
             "npm i -g chrome-devtools-mcp"),
        ]:
            found = os.environ.get(env_var) or shutil.which(binary)
            if found:
                env[env_var] = found
                tools[label] = ["--tool", tool]
            else:
                SKIPPED[label] = f"not installed ({hint})"
        first = next(iter(results), None)
        base_extract = (normalized_extract(results[first]["session"]["best"]["extract"])
                        if first else None)
        print("== agent-style CLI scenario: " + ", ".join(tools), flush=True)
        agent_results = bench_agent(tools, env, out_dir, chrome_tag, expected, base_extract)

    parallel_results: dict = {}
    if "parallel" in scenarios:
        import shutil
        ptools: dict[str, str] = {}
        if "nv-serve" in contenders:
            ptools["navigera"] = "nv"
            ptools["navigera (own browser)"] = "nv-own"
        for label, tool, env_var, binary in [
            ("agent-browser", "ab", "AGENT_BROWSER", "agent-browser"),
            ("playwright-cli", "pw", "PLAYWRIGHT_CLI", "playwright-cli"),
            ("Playwright MCP", "pwmcp", "PLAYWRIGHT_MCP", "playwright-mcp"),
            ("Chrome DevTools MCP", "cdmcp", "CHROME_DEVTOOLS_MCP", "chrome-devtools-mcp"),
        ]:
            found = os.environ.get(env_var) or shutil.which(binary)
            if found:
                env[env_var] = found
                ptools[label] = tool
        print("== parallel agents: " + ", ".join(ptools), flush=True)
        parallel_results = bench_parallel(ptools, env, chrome_tag)

    summary = {
        "contenders": list(contenders),
        "reps": args.reps,
        "results": {
            name: {
                "session_best": {k: v for k, v in r["session"]["best"].items()
                                 if k != "counts"},
                "session_counts": r["session"]["best"]["counts"],
                "session_runs_wall": [round(x["wall_s"], 3) for x in r["session"]["runs"]],
                **({"eval_best": r["eval"]["best"]} if r.get("eval") else {}),
                **({"cold_best": r["cold"]["best"],
                    "cold_runs_wall": [round(x["wall_s"], 3) for x in r["cold"]["runs"]]}
                   if r.get("cold") else {}),
                "correct": r.get("correct", False),
                **({"realworld_best": r["realworld"]["best"],
                    "realworld_runs_wall": [round(x["wall_s"], 3) for x in r["realworld"]["runs"]]}
                   if r.get("realworld") else {}),
                **({"browse_best": r["browse"]["best"],
                    "browse_runs_wall": [round(x["wall_s"], 3) for x in r["browse"]["runs"]]}
                   if r.get("browse") else {}),
            }
            for name, r in results.items()
        },
    }
    with open(out_dir / "results.json", "w", encoding="utf-8") as f:
        json.dump(summary, f, indent=2)

    # Markdown table (also appended to $GITHUB_STEP_SUMMARY by the workflow).
    def env_line(caption: str) -> str:
        try:
            cpu = subprocess.run(
                ["sh", "-c", "grep 'model name' /proc/cpuinfo | head -1 | cut -d: -f2 | xargs"],
                capture_output=True, text=True, timeout=10).stdout.strip()
        except Exception:
            cpu = "unknown"
        try:
            chrome_v = subprocess.run(
                [os.environ.get("CHROME_BIN", "google-chrome"), "--version"],
                capture_output=True, text=True, timeout=10).stdout.strip()
        except Exception:
            chrome_v = "unknown"
        return (f"{caption}**Env:** {cpu or 'unknown'} — {os.cpu_count()} vCPU · "
                f"**Chrome:** {chrome_v}")
    lines = []
    lines.append("## browser-driver ladder")
    lines.append("")
    lines.append(env_line(""))
    lines.append("")
    lines.append("Full scripted session: launch → goto listing → title/count/extract "
                 "evals → fill+click filter → visible-count eval → screenshot → "
                 "new tab → goto detail → title/row-count evals → close. "
                 "Best-of-N wall; driver CPU = the driver process's own "
                 "utime+stime (zombie /proc read, excludes reaped Chrome); "
                 "driver RSS = its own VmHWM; browser CPU/PSS = the browser process "
                 "tree, sampled from /proc (approximate).")
    lines.append("")
    lines.append("| contender | wall (best of {}) | driver CPU (own) | reaped-child CPU† | "
                 "browser CPU* | driver peak RSS | browser PSS* | runs |".format(args.reps))
    lines.append("|---|---|---|---|---|---|---|---|")
    for name in contenders:
        b = results[name]["session"]["best"]
        runs = ",".join(f"{x['wall_s']:.2f}" for x in results[name]["session"]["runs"])
        label = f"`{name}`" + (" (experimental, one tab)" if name in EXPERIMENTAL else "")
        if not results[name].get("correct", False):
            label += " ✗ gate"
        lines.append(
            f"| {label} | {b['wall_s']:.2f}s | {b['cpu_s']:.3f}s | "
            f"{b.get('cpu_reaped_s', float('nan')):.2f}s | "
            f"{b['browser_cpu_s']:.2f}s | {b['maxrss_kb'] / 1024:.0f} MB | "
            f"{b['browser_pss_kb'] / 1024:.0f} MB | {runs}s |")
    lines.append("")
    lines.append("\\* browser numbers: whole browser process tree, /proc-sampled every 50 ms (approximate). "
                 "† CPU of descendants the driver itself waited for (its Chrome, "
                 "if it reaps it) — what wait4 used to add into \"driver CPU\".")
    lines.append("")
    lines.append("### warm eval round-trip (200× `() => document.title`, best session)")
    lines.append("")
    lines.append("| contender | mean | p95 |")
    lines.append("|---|---|---|")
    for name in contenders:
        if not results[name].get("eval"):
            continue
        b = results[name]["eval"]["best"]
        lines.append(f"| `{name}` | {b['mean_ms']:.2f} ms | {b['p95_ms']:.2f} ms |")
    lines.append("")
    lines.append("### cold start: launch → new page → one eval → close (best of 5)")
    lines.append("")
    lines.append("| contender | wall |")
    lines.append("|---|---|")
    for name in contenders:
        if not results[name].get("cold"):
            continue
        b = results[name]["cold"]["best"]
        lines.append(f"| `{name}` | {b['wall_s']:.2f}s |")
    lines.append("")
    phase_rows = [(n, results[n]["cold"]["best"].get("phases") or {}) for n in contenders
                  if results[n].get("cold") and (results[n]["cold"]["best"].get("phases"))]
    if phase_rows:
        keys = ["devtools_url", "ws_connect", "browser_up", "first_page", "launch_total", "close"]
        lines.append("### navigera cold-start phases (ms, best run; `NAVIGERA_TIMINGS`)")
        lines.append("")
        lines.append("| contender | " + " | ".join(keys) + " |")
        lines.append("|---|" + "---|" * len(keys))
        for name, ph in phase_rows:
            lines.append(f"| `{name}` | " + " | ".join(
                f"{ph[k]:.1f}" if isinstance(ph.get(k), (int, float)) else "—" for k in keys) + " |")
        lines.append("")
    rw_names = [n for n in contenders if results[n].get("realworld")]
    if rw_names:
        lines.append("### real-world: https://example.com goto → title/h1 evals (best of 3)")
        lines.append("")
        lines.append("| contender | wall | title | h1 |")
        lines.append("|---|---|---|---|")
        for name in rw_names:
            b = results[name]["realworld"]["best"]
            ok = "✓" if b.get("ok") else "✗"
            lines.append(f"| `{name}` | {b['wall_s']:.2f}s | {ok} {b.get('title','')} | {(b.get('h1') or '')[:20]} |")
        lines.append("")
    browse_names = [n for n in contenders if results[n].get("browse")]
    if browse_names:
        lines.append("### real-world browse: github awesome (scroll) + trending (click-through) (best of 3)")
        lines.append("")
        lines.append("| contender | wall | ok | trending top-3 | clicked |")
        lines.append("|---|---|---|---|---|")
        for name in browse_names:
            b = results[name]["browse"]["best"]
            ok = "✓" if b.get("ok") else "✗"
            f = b.get("facts") or {}
            top3 = ", ".join(f.get("trending_top3", []) or [])
            clicked = (f.get("trending_clicked_url") or "").replace("https://github.com/", "")
            lines.append(f"| `{name}` | {b['wall_s']:.2f}s | {ok} | {top3[:60]} | {clicked[:40]} |")
        lines.append("")
    if agent_results:
        lines.append("### agent tools: one CLI process (or one MCP tools/call) per step, warm session (best of 3)")
        lines.append("")
        lines.append("| tool | kind | calls | total wall | mean per call | snapshot | snapshot output | "
                     "1000-row snapshot | its output | 2nd session start | gate |")
        lines.append("|---|---|---|---|---|---|---|---|---|---|---|")
        for name, r in agent_results.items():
            b = r["best"]
            kind = "MCP" if "MCP" in name else "CLI"
            if not b.get("wall_s"):
                err = next((x.get("error", "") for x in r["runs"] if x.get("error")), "")
                lines.append(f"| `{name}` | {kind} | — | failed | — | — | — | — | — | — | ✗ {err[-160:].replace('|', '/')} |")
                continue
            step_ms = {st["op"]: st["ms"] for st in b.get("steps") or []}
            snap_b = step_ms.get("snapshot b")
            warm = step_ms.get("start warm")
            lines.append(f"| `{name}` | {kind} | {b.get('calls') or '—'} | {b['wall_s']:.2f}s | {b['op_mean_ms']:.1f} ms | "
                         f"{step_ms.get('snapshot', 0):.0f} ms | {b['snapshot_bytes'] / 1024:.1f} KB | "
                         + (f"{snap_b:.0f} ms | {(b.get('snapshot_b_bytes') or 0) / 1024:.1f} KB | "
                            if snap_b is not None else "— | — | ")
                         + (f"{warm:.0f} ms | " if warm is not None else "— | ")
                         + f"{'✓' if b['ok'] else '✗'} |")
        lines.append("")
        lines.append("Snapshot output is each tool's default (what an agent reads). The 1000-row snapshot "
                     "and the 2nd session start are extra steps, outside the total. `navigera (batch)` puts "
                     "the known steps in three `batch` calls (per step = per call).")
        lines.append("")
    if parallel_results:
        lines.append("### parallel agents: N sessions open at once, each after goto + snapshot "
                     "(PSS of every process the tool started, MB)")
        lines.append("")
        lines.append("| tool | " + " | ".join(f"{n} agent{'s' if n > 1 else ''}" for n in PARALLEL_NS)
                     + " | per extra agent | browser processes at max | up (max N) |")
        lines.append("|---|" + "---|" * (len(PARALLEL_NS) + 3))
        for label, rows in parallel_results.items():
            cells = [f"{r['pss_mb']:.0f}" if "pss_mb" in r else "✗" for r in rows]
            ok = [r for r in rows if "pss_mb" in r]
            per = (f"{(ok[-1]['pss_mb'] - ok[0]['pss_mb']) / (ok[-1]['n'] - ok[0]['n']):.0f}"
                   if len(ok) > 1 else "—")
            lines.append(f"| `{label}` | " + " | ".join(cells) + f" | {per} | "
                         + (f"{ok[-1]['browser_procs']} | {ok[-1]['up_s']:.2f}s |" if ok else "— | — |"))
        lines.append("")
    if NOTES:
        lines.append("### Notes (experimental contenders)")
        lines.append("")
        for name, notes in NOTES.items():
            for note in notes:
                lines.append(f"- `{name}`: {note.replace('|', '/')}")
        lines.append("")
    if SKIPPED:
        lines.append("### Skipped")
        lines.append("")
        for name, reason in SKIPPED.items():
            lines.append(f"- `{name}`: {reason.replace('|', '/')}")
        lines.append("")
    (out_dir / "table.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    versions = collect_versions()
    write_publish_json(out_dir, results, contenders, args.reps, env_line(""), agent_results, versions,
                       parallel_results)
    print("\n".join(lines), flush=True)

    server.shutdown()
    return 0


if __name__ == "__main__":
    sys.exit(main())
