#!/usr/bin/env python3
"""What the static musl build costs a call, and where.

    libc_probe.py time BASE_URL OUT_DIR
    libc_probe.py profile ARM BASE_URL OUT_DIR
    libc_probe.py cli ARM BASE_URL OUT_DIR
    libc_probe.py split BASE_URL OUT_DIR
    libc_probe.py rest BASE_URL OUT_DIR
    libc_probe.py sched ARM BASE_URL OUT_DIR

Builds, from $NAVIGERA_BUILDS (`name=path,name=path,...`): the release's
musl + mimalloc, musl with musl's own allocator, glibc with glibc's, glibc
with mimalloc. One more arm runs the released build with mimalloc's purge
off (MIMALLOC_PURGE_DELAY=-1): freed pages kept instead of handed back
10 ms later. agent-browser is the control for the runner's drift.

`time`: every navigera arm on a browser of its own (NAVIGERA_SHARED_BROWSER=0:
the session server talks to Chrome itself, so the server is all of
navigera's side but the CLI), three ops (the ladder's click with its setup
and 0.3 s rest, `eval 1`, `ax`), rounds in a Latin square so each arm is
in each position once. One notice per op.

`profile ARM`: one fresh session of that arm; its session server under
`perf stat` (task clock, context switches, migrations, page faults, cycles
and instructions where the runner counts them), `strace -c`, and
`perf record`, each over 45 clicks with their setup, and over 45 setups
alone (the difference is the clicks'). Notices per click; full output to
OUT_DIR.

`rest`: the click after its setup with the ladder's 0.3 s rest and with
0.02 s, every arm and both rests interleaved: a cost that grows with the
idle time before the click (a purge, a cold cache, a deep CPU sleep
state) shows as a gap at 0.3 s that closes at 0.02 s.

`sched ARM`: the off-CPU view of one arm's clicks. (a) `perf sched record`
of the whole machine over the clicks: per click, from the scheduler's own
switch and wakeup events, the session server's threads' run time,
scheduling delay (woken but not yet on a CPU) and wakeups, its Chrome's
processes' (renderer, GPU, browser, utility), and how idle the CPUs were;
only the timed click's window counts, not its setup. (b) `perf record`
of the server's switch-outs with DWARF stacks: where its threads block
or are preempted during the clicks, folded and counted.
"""
import json
import os
import re
import signal
import statistics
import subprocess
import sys
import time

MODE = sys.argv[1]
BUILDS = dict(x.split("=", 1) for x in os.environ["NAVIGERA_BUILDS"].split(","))
AB = os.environ.get("AGENT_BROWSER", "agent-browser")
CHROME = os.environ["CHROME_BIN"]
ROUNDS, REPS, PROF_N = int(os.environ.get("ROUNDS", "6")), int(os.environ.get("REPS", "15")), int(os.environ.get("PROF_N", "45"))
SETUP = ("(() => { document.getElementById('q').value='widget'; "
         "for (const c of document.querySelectorAll('.card.hidden')) c.classList.remove('hidden'); return 1 })()")
ENV_AB = dict(os.environ, AGENT_BROWSER_EXECUTABLE_PATH=CHROME, AGENT_BROWSER_ARGS="--no-sandbox")


def arms():
    out = {}
    for name, path in BUILDS.items():
        out[name] = (path, {})
    # NAVIGERA_ENV_ARMS: `build:suffix:VAR=value;...`, the build run with
    # that environment (inherited by its session server).
    for spec in filter(None, os.environ.get("NAVIGERA_ENV_ARMS", "").split(";")):
        build, suffix, kv = spec.split(":", 2)
        k, v = kv.split("=", 1)
        out[f"{build}-{suffix}"] = (BUILDS[build], {k: v})
    return out


ARMS = arms()


def run(argv, env=None):
    t = time.perf_counter()
    p = subprocess.run(argv, capture_output=True, text=True, env=env, timeout=60)
    ms = (time.perf_counter() - t) * 1000
    if p.returncode != 0:
        raise RuntimeError(f"{argv}: {p.returncode} {p.stdout[-300:]} {p.stderr[-300:]}")
    return ms, p.stdout


def nav_env(extra):
    return dict(os.environ, NAVIGERA_SHARED_BROWSER="0", **extra)


def session(arm):
    """Session name, argv prefix and environment for an arm's calls."""
    if arm == "ab":
        return "ab", [AB, "--session", "ab"], ENV_AB
    path, extra = ARMS[arm]
    name = re.sub(r"[^a-z0-9]", "", arm)
    return name, [path, "--session", name], nav_env(extra)


def start(arm, base, page="page_a.html", suffix=""):
    name, pre, env = session(arm)
    pre = pre[:2] + [pre[2] + suffix]
    if arm == "ab":
        run(pre + ["--json", "open", f"{base}/{page}"], env)
        return None
    _, out = run(pre + ["start", "--chromium", CHROME], env)
    run(pre + ["goto", f"{base}/{page}"], env)
    m = re.search(r'"?pid"?\s*[:=]\s*(\d+)', out)
    return int(m.group(1)) if m else None


def op_argv(arm, op):
    _, pre, env = session(arm)
    if op == "click":
        click = pre + (["click", "#search"] if arm == "ab" else ["click", "--selector", "#search"])
        return pre + ["eval", SETUP], click, env
    if op == "eval":
        return None, pre + ["eval", "1"], env
    if op == "ax":
        return None, pre + (["snapshot"] if arm == "ab" else ["ax"]), env
    if op == "version":
        return None, [pre[0], "--version"], env
    if op == "axbig":
        # A session of its own on the 2,000-row page: the click session never reads a tree.
        big = pre[:2] + [pre[2] + "big"]
        return None, big + (["snapshot"] if arm == "ab" else ["ax", "--limit", "0"]), env
    raise ValueError(op)


def timed(arm, op, reps):
    setup, call, env = op_argv(arm, op)
    out = []
    for _ in range(reps):
        if setup:
            run(setup, env)
            time.sleep(0.3)
        out.append(run(call, env)[0])
    return out


def summary(xs):
    q = statistics.quantiles(xs, n=4)
    return f"{statistics.median(xs):.2f} [{q[0]:.2f}-{q[2]:.2f}]"


def clean():
    for path in set(BUILDS.values()):
        subprocess.run([path, "sessions", "--clean", "--all"], capture_output=True)
    subprocess.run([AB, "close", "--all"], capture_output=True, env=ENV_AB)


def time_mode(base, out_dir):
    names = list(ARMS) + ["ab"]
    for a in names:
        start(a, base)
        start(a, base, "big.html", "big")
    ops = ["click", "eval", "version", "axbig"]
    got = {op: {a: [] for a in names} for op in ops}
    for r in range(ROUNDS):
        order = names[r % len(names):] + names[:r % len(names)]
        for a in order:
            for op in ops:
                try:
                    got[op][a] += timed(a, op, REPS)
                except RuntimeError as e:
                    print(f"::warning::{a} {op}: {str(e)[:200]}")
    for op in ops:
        parts = [f"{a} {summary(got[op][a])}" for a in names if len(got[op][a]) >= 4]
        print(f"::notice title={op} median [IQR] ms, {ROUNDS}x{REPS} per arm::" + " · ".join(parts))
    json.dump(got, open(os.path.join(out_dir, "times.json"), "w"))
    clean()


def tids(pid):
    return sorted(int(t) for t in os.listdir(f"/proc/{pid}/task"))


def under(tool_argv, setup, call, env, n, with_click):
    """Run n (setup [+ rest + click]) under a tool attached to the server."""
    p = subprocess.Popen(tool_argv, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    time.sleep(1.0)
    ms = []
    for _ in range(n):
        run(setup, env)
        time.sleep(0.3)
        if with_click:
            ms.append(run(call, env)[0])
    time.sleep(0.2)
    p.send_signal(signal.SIGINT)
    try:
        _, err = p.communicate(timeout=60)
    except subprocess.TimeoutExpired:
        p.kill()
        _, err = p.communicate()
    return ms, err


def perf_stat_file(path):
    vals = {}
    for line in open(path):
        f = line.strip().split(",")
        if len(f) < 3 or line.startswith("#"):
            continue
        try:
            vals[f[2]] = float(f[0])
        except ValueError:
            vals[f[2]] = None
    return vals


def strace_file(path):
    rows = {}
    for line in open(path):
        f = line.split()
        if len(f) >= 5 and f[0][0].isdigit() and f[-1] != "total":
            calls = int(f[3])
            errors = int(f[4]) if len(f) == 6 else 0
            rows[f[-1]] = (calls, float(f[1]), errors)
    return rows


def profile_mode(arm, base, out_dir):
    """Each tool on its own: one that fails leaves the others' numbers."""
    pid = start(arm, base)
    if not pid:
        raise SystemExit(f"{arm}: no session server pid in start's answer")
    setup, call, env = op_argv(arm, "click")
    for _ in range(5):
        run(setup, env)
        run(call, env)
    plist = ",".join(map(str, tids(pid)))
    for part in (stat_part, strace_part, record_part):
        try:
            part(arm, pid, plist, setup, call, env, out_dir)
        except Exception as e:  # noqa: BLE001 - report and go on
            print(f"::warning title={arm} {part.__name__}::{type(e).__name__}: {str(e)[:300]}")
    clean()


def stat_part(arm, pid, plist, setup, call, env, out_dir):
    events = "task-clock,context-switches,cpu-migrations,page-faults,cycles,instructions"
    res = {}
    for label, click in (("clicks", True), ("setups", False)):
        f = os.path.join(out_dir, f"{arm}-stat-{label}.csv")
        ms, err = under(["perf", "stat", "-x,", "-o", f, "-e", events, "-p", str(pid)], setup, call, env, PROF_N, click)
        res[label] = perf_stat_file(f)
        if click:
            res["ms"] = ms
    per = {}
    for k, v in res["clicks"].items():
        b = res["setups"].get(k)
        if v is not None and b is not None:
            per[k] = (v - b) / PROF_N
    shown = []
    for k, unit, scale in (("task-clock", "ms cpu", 1.0), ("context-switches", "cs", 1), ("cpu-migrations", "migr", 1),
                           ("page-faults", "faults", 1), ("instructions", "M instr", 1e-6), ("cycles", "M cyc", 1e-6)):
        if k in per:
            shown.append(f"{per[k] * scale:.2f} {unit}")
    print(f"::notice title={arm} perf stat per click (clicks minus setups, {PROF_N} each)::"
          + " · ".join(shown) + f" · wall {summary(res['ms'])} ms")


def strace_part(arm, pid, plist, setup, call, env, out_dir):
    st = {}
    for label, click in (("clicks", True), ("setups", False)):
        f = os.path.join(out_dir, f"{arm}-strace-{label}.txt")
        argv = ["strace", "-c", "-f", "-o", f] + sum((["-p", t] for t in plist.split(",")), [])
        under(argv, setup, call, env, PROF_N, click)
        st[label] = strace_file(f)
    diff = []
    for sc, (calls, secs, errs) in st["clicks"].items():
        b = st["setups"].get(sc, (0, 0.0, 0))
        diff.append((calls - b[0], sc, errs - b[2]))
    diff.sort(reverse=True)
    print(f"::notice title={arm} syscalls per click (strace -c, clicks minus setups)::"
          + " · ".join(f"{sc} {c / PROF_N:.1f}" + (f" ({e / PROF_N:.1f} err)" if e > 0 else "") for c, sc, e in diff[:10]))


def record_part(arm, pid, plist, setup, call, env, out_dir):
    data = os.path.join(out_dir, f"{arm}-perf.data")
    under(["perf", "record", "-F", "2999", "-o", data, "-p", str(pid)], setup, call, env, PROF_N, True)
    rep = subprocess.run(["perf", "report", "-i", data, "--stdio", "--no-children", "--sort", "dso,sym", "-q",
                          "--percent-limit", "0.5"], capture_output=True, text=True)
    open(os.path.join(out_dir, f"{arm}-perf-report.txt"), "w").write(rep.stdout + rep.stderr)
    top = []
    for line in rep.stdout.splitlines():
        m = re.match(r"\s*([\d.]+)%\s+(\S+)\s+\[[^\]]*\]\s+(.*)", line)
        if m:
            top.append(f"{m.group(1)}% {os.path.basename(m.group(2))}:{m.group(3).strip()[:60]}")
    print(f"::notice title={arm} perf record self time (whole server, {PROF_N} setup+click)::" + " · ".join(top[:12]))


def cli_mode(arm, base, out_dir):
    """The CLI process itself, per call: perf stat and strace -c -f of
    each `eval 1` and each click (with its untimed setup)."""
    start(arm, base)
    for op in ("eval", "click"):
        setup, call, env = op_argv(arm, op)
        for _ in range(3):
            if setup:
                run(setup, env)
            run(call, env)
        stat = os.path.join(out_dir, f"{arm}-cli-{op}-stat.csv")
        tr = os.path.join(out_dir, f"{arm}-cli-{op}-strace")
        open(stat, "w").close()
        sums, calls, n = {}, {}, 15
        for i in range(n):
            if setup:
                run(setup, env)
                time.sleep(0.3)
            run(["perf", "stat", "-x,", "--append", "-o", stat, "-e",
                 "task-clock,page-faults,context-switches,cpu-migrations", "--"] + call, env)
        for line in open(stat):
            f = line.strip().split(",")
            if len(f) >= 3 and not line.startswith("#"):
                try:
                    sums[f[2]] = sums.get(f[2], 0.0) + float(f[0])
                except ValueError:
                    pass
        for i in range(n):
            if setup:
                run(setup, env)
                time.sleep(0.3)
            f = f"{tr}-{i}.txt"
            run(["strace", "-f", "-c", "-o", f, "--"] + call, env)
            for sc, (c, _, _) in strace_file(f).items():
                calls[sc] = calls.get(sc, 0) + c
        top = sorted(((c, sc) for sc, c in calls.items()), reverse=True)[:12]
        print(f"::notice title={arm} CLI {op}, per call (perf stat; strace -c -f, {n} calls each)::"
              + " · ".join(f"{k} {v / n:.2f}" for k, v in sums.items())
              + " | " + " · ".join(f"{sc} {c / n:.1f}" for c, sc in top))
    clean()


def split_mode(base, out_dir):
    """Where a click's wall time goes, per build: every session server
    traced (NAVIGERA_CDP_TRACE: wall-clock microseconds per CDP command
    sent and answered, the request read, the answer written), clicks of
    all arms interleaved. Phases: CLI start to the request read; to the
    first mouse event sent (prep); the three mouse events' answers; to
    the answer written; to the CLI's exit."""
    names = [a for a in ARMS]
    traces = {}
    for a in names:
        traces[a] = os.path.join(out_dir, f"{a}-split.jsonl")
        ARMS[a] = (ARMS[a][0], dict(ARMS[a][1], NAVIGERA_CDP_TRACE=traces[a]))
        start(a, base)
    got = {a: [] for a in names}
    reps = int(os.environ.get("SPLIT_REPS", "10"))
    for r in range(ROUNDS):
        order = names[r % len(names):] + names[:r % len(names)]
        for a in order:
            setup, call, env = op_argv(a, "click")
            for _ in range(reps):
                run(setup, env)
                time.sleep(0.3)
                t0 = time.time() * 1e6
                run(call, env)
                t1 = time.time() * 1e6
                got[a].append((t0, t1))
    rows_by = {}
    for a in names:
        rows = [json.loads(x) for x in open(traces[a])]
        rows_by[a] = rows
        ph = {k: [] for k in ("start", "prep", "m1", "m2", "m3", "after", "exit", "total")}
        for t0, t1 in got[a]:
            rs = [x for x in rows if t0 <= x["t"] <= t1]
            req = next((x["t"] for x in rs if x["method"] == "REQ"), None)
            ans = next((x["t"] for x in rs if x["method"] == "ANS"), None)
            sends = [x["t"] for x in rs if x["method"] == "Input.dispatchMouseEvent"]
            dones = [x["t"] for x in rs if x["method"] == "DONE Input.dispatchMouseEvent"]
            if None in (req, ans) or len(sends) < 3 or len(dones) < 3:
                continue
            ph["start"].append((req - t0) / 1000)
            ph["prep"].append((sends[0] - req) / 1000)
            for i in range(3):
                ph[f"m{i + 1}"].append((dones[i] - sends[0]) / 1000)
            ph["after"].append((ans - dones[2]) / 1000)
            ph["exit"].append((t1 - ans) / 1000)
            ph["total"].append((t1 - t0) / 1000)
        n = len(ph["total"])
        print(f"::notice title={a} click split, median ms ({n} of {len(got[a])} clicks)::"
              + " · ".join(f"{k} {statistics.median(v):.2f}" for k, v in ph.items() if v))
    json.dump({"got": got}, open(os.path.join(out_dir, "split.json"), "w"))
    clean()


def mono_us():
    return time.clock_gettime(time.CLOCK_MONOTONIC) * 1e6


def clicks_in_windows(arm, n, rest=0.3):
    """n (setup, rest, click) of a started arm; the clicks' monotonic windows."""
    setup, call, env = op_argv(arm, "click")
    wins, ms = [], []
    for _ in range(n):
        run(setup, env)
        time.sleep(rest)
        t0 = mono_us()
        ms.append(run(call, env)[0])
        wins.append((t0, mono_us()))
    return wins, ms


def family(pid):
    """Every process descended from pid (the server's Chrome), by kind."""
    kids = {}
    for d in os.listdir("/proc"):
        if not d.isdigit():
            continue
        try:
            stat = open(f"/proc/{d}/stat").read()
            ppid = int(stat.rsplit(")", 1)[1].split()[1])
            # Chrome rewrites its children's command lines into one string.
            args = open(f"/proc/{d}/cmdline", "rb").read().replace(b"\0", b" ").split()
        except (OSError, IndexError, ValueError):
            continue
        kids.setdefault(ppid, []).append((int(d), args))
    out, todo = {}, [pid]
    while todo:
        p = todo.pop()
        for c, args in kids.get(p, []):
            kind = next((a.split(b"=", 1)[1].decode() for a in args if a.startswith(b"--type=")), "browser")
            out[c] = kind
            todo.append(c)
    return out


def all_tids(pids):
    got = {}
    for p, kind in pids.items():
        try:
            for t in os.listdir(f"/proc/{p}/task"):
                got[int(t)] = kind
        except OSError:
            pass
    return got


# sched_switch as the raw fields or as perf's own print of them
# ("comm:tid [prio] state ==> comm:tid [prio]"); a comm may hold ':'.
SW = re.compile(r"prev_pid=(\d+) .*?prev_state=(\S+) ==> .*?next_pid=(\d+)"
                r"|:(\d+) \[-?\d+\] (\S+) ==> .*:(\d+) \[-?\d+\]\s*$")
WK = re.compile(r"\bpid=(\d+)")


def sched_part(arm, pid, n, out_dir):
    who = {t: "server" for t in tids(pid)}
    who.update({t: f"chrome-{k}" for t, k in all_tids(family(pid)).items()})
    data = os.path.join(out_dir, f"{arm}-sched.data")
    p = subprocess.Popen(["sudo", "perf", "sched", "record", "-k", "CLOCK_MONOTONIC", "-a", "-o", data],
                         stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    time.sleep(1.5)
    wins, ms = clicks_in_windows(arm, n)
    time.sleep(0.2)
    subprocess.run(["sudo", "kill", "-INT", str(p.pid)])
    p.communicate(timeout=120)
    # perf sched record runs `perf record` as its child under sudo: the
    # data file is written once that child has gone.
    subprocess.run(["sudo", "chmod", "a+r", data])
    txt = subprocess.run(["sudo", "perf", "script", "-i", data, "-F", "cpu,time,event,trace", "--ns"],
                         capture_output=True, text=True).stdout
    open(os.path.join(out_dir, f"{arm}-sched.txt"), "w").write(txt)
    ncpu = os.cpu_count() or 1
    per = sched_replay(txt, who, wins, ncpu)
    report_sched(arm, per, wins, ncpu, out_dir)


def sched_replay(txt, who, wins, ncpu):
    """Per click window: run time, scheduling delay (wakeup to on-CPU),
    wakeups and preemptions per group of tids, and the CPUs' idle share,
    from `perf script -F cpu,time,event,trace` of sched_switch/wakeup."""
    # Replay the scheduler: per tid, when it was woken and switched in.
    woke, on, idle_since = {}, {}, {}
    ev = []  # (t_us, kind, group, value)
    for line in txt.splitlines():
        m = re.match(r"\s*\[(\d+)\]\s+([\d.]+):\s+(\S+):\s+(.*)", line)
        if not m:
            continue
        cpu, t, name, trace = int(m.group(1)), float(m.group(2)) * 1e6, m.group(3), m.group(4)
        if name.startswith("sched:sched_wakeup"):
            w = WK.search(trace)
            if w:
                woke[int(w.group(1))] = t
        elif name == "sched:sched_switch":
            s_ = SW.search(trace)
            if not s_:
                continue
            g3 = s_.groups()[:3] if s_.group(1) else s_.groups()[3:]
            prev, state, nxt = int(g3[0]), g3[1], int(g3[2])
            if prev in on:
                g = who.get(prev)
                if g:
                    ev.append((on.pop(prev), t, g, "run"))
                    if state.startswith("R"):
                        ev.append((t, t, g, "preempt"))
            if prev == 0 and cpu in idle_since:
                ev.append((idle_since.pop(cpu), t, "cpu", "idle"))
            if nxt == 0:
                idle_since[cpu] = t
            else:
                on[nxt] = t
                g = who.get(nxt)
                if g:
                    w = woke.pop(nxt, None)
                    if w is not None and w <= t:
                        ev.append((w, t, g, "delay"))
                        ev.append((t, t, g, "wake"))
    # Close what is still running or idle when the trace ends.
    end = max((e[1] for e in ev), default=0.0)
    for tid, t in on.items():
        if who.get(tid):
            ev.append((t, end, who[tid], "run"))
    for cpu, t in idle_since.items():
        ev.append((t, end, "cpu", "idle"))
    per = []
    for t0, t1 in wins:
        acc = {}
        for a, b, g, k in ev:
            if b < t0 or a > t1:
                continue
            key = (g, k)
            if k in ("wake", "preempt"):
                acc[key] = acc.get(key, 0) + 1
            else:
                acc[key] = acc.get(key, 0) + (min(b, t1) - max(a, t0)) / 1000
                if k == "delay":
                    acc[(g, "maxdelay")] = max(acc.get((g, "maxdelay"), 0), (b - a) / 1000)
        acc[("cpu", "idlepct")] = 100 * acc.get(("cpu", "idle"), 0) / (ncpu * (t1 - t0) / 1000)
        acc[("click", "ms")] = (t1 - t0) / 1000
        per.append(acc)
    return per


def report_sched(arm, per, wins, ncpu, out_dir):
    keys = sorted({k for a in per for k in a})
    med = {k: statistics.median(a.get(k, 0) for a in per) for k in keys}
    p90 = {k: statistics.quantiles([a.get(k, 0) for a in per], n=10)[-1] for k in keys}
    json.dump({"windows": wins, "per_click": [{f"{g}.{k}": v for (g, k), v in a.items()} for a in per]},
              open(os.path.join(out_dir, f"{arm}-sched.json"), "w"))
    groups = [g for g in ("server", "chrome-renderer", "chrome-browser", "chrome-gpu-process", "chrome-utility")
              if any(k[0] == g for k in keys)]
    parts = [f"click {med[('click', 'ms')]:.2f} (p90 {p90[('click', 'ms')]:.2f})",
             f"cpus idle {med[('cpu', 'idlepct')]:.0f}%"]
    for g in groups:
        parts.append(f"{g}: run {med.get((g, 'run'), 0):.2f} · delay {med.get((g, 'delay'), 0):.3f}"
                     f" (p90 {p90.get((g, 'delay'), 0):.3f}, max/click {med.get((g, 'maxdelay'), 0):.3f})"
                     f" · wakes {med.get((g, 'wake'), 0):.0f} · preempt {med.get((g, 'preempt'), 0):.0f}")
    print(f"::notice title={arm} sched per click, median ms ({len(per)} clicks, {ncpu} cpus)::" + " | ".join(parts))


def stacks_part(arm, pid, n, out_dir):
    data = os.path.join(out_dir, f"{arm}-switch.data")
    p = subprocess.Popen(["sudo", "perf", "record", "-e", "sched:sched_switch", "--call-graph", "dwarf,16384",
                          "-k", "CLOCK_MONOTONIC", "-p", str(pid), "-o", data],
                         stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    time.sleep(1.5)
    wins, _ = clicks_in_windows(arm, n)
    time.sleep(0.2)
    subprocess.run(["sudo", "kill", "-INT", str(p.pid)])
    p.communicate(timeout=120)
    subprocess.run(["sudo", "chmod", "a+r", data])
    txt = subprocess.run(["sudo", "perf", "script", "-i", data, "-F", "time,trace,ip,sym", "--ns"],
                         capture_output=True, text=True).stdout
    open(os.path.join(out_dir, f"{arm}-switch.txt"), "w").write(txt)
    counts, cur = {}, None

    def flush():
        if cur and any(a <= cur[0] <= b for a, b in wins):
            # The scheduler's own frames say nothing; the syscall's do,
            # then the user stack from the innermost frame out.
            kern = [x for x in cur[2] if not re.match(r"(__)?schedule|preempt_schedule", x)]
            key = f"{cur[1]} " + ";".join(kern[:3] + ["|"] + cur[3][:8])
            counts[key] = counts.get(key, 0) + 1

    for line in txt.splitlines():
        m = re.match(r"\s*([\d.]+):\s+.*(?:prev_state=|\[-?\d+\] )(\S+) ==> ", line)
        if m:
            flush()
            cur = (float(m.group(1)) * 1e6, m.group(2), [], [])
        elif cur and line.strip():
            f = line.split(None, 1)
            if len(f) == 2:
                sym = re.sub(r"\s*\(.*\)$", "", f[1].strip())
                sym = re.sub(r"\+0x[0-9a-f]+$", "", sym)
                sym = re.sub(r"::h[0-9a-f]{16}$", "", sym)[:70]
                frames = cur[2] if f[0].startswith("ffff") else cur[3]
                if not frames or frames[-1] != sym:
                    frames.append(sym)
    flush()
    top = sorted(counts.items(), key=lambda kv: -kv[1])[:8]
    open(os.path.join(out_dir, f"{arm}-switch-folded.txt"), "w").write(
        "".join(f"{c} {k}\n" for k, c in sorted(counts.items(), key=lambda kv: -kv[1])))
    print(f"::notice title={arm} server switch-outs in {n} clicks ({sum(counts.values())} in all), top stacks::"
          + " ‖ ".join(f"{c}x {k[:300]}" for k, c in top))


def sched_mode(arm, base, out_dir):
    pid = start(arm, base)
    if not pid:
        raise SystemExit(f"{arm}: no session server pid in start's answer")
    setup, call, env = op_argv(arm, "click")
    for _ in range(5):
        run(setup, env)
        run(call, env)
    print(f"::notice title={arm} threads::server {len(tids(pid))} · chrome processes "
          + ", ".join(f"{k} {v}" for k, v in sorted(_count(family(pid).values()).items())))
    n = int(os.environ.get("SCHED_N", "30"))
    for part in (sched_part, stacks_part):
        try:
            part(arm, pid, n, out_dir)
        except Exception as e:  # noqa: BLE001 - report and go on
            print(f"::warning title={arm} {part.__name__}::{type(e).__name__}: {str(e)[:300]}")
    clean()


def _count(xs):
    out = {}
    for x in xs:
        out[x] = out.get(x, 0) + 1
    return out


def rest_mode(base, out_dir):
    names = list(ARMS)
    for a in names:
        start(a, base)
    got = {(a, r): [] for a in names for r in (0.3, 0.02)}
    reps = int(os.environ.get("REST_REPS", "5"))
    for rnd in range(ROUNDS):
        order = names[rnd % len(names):] + names[:rnd % len(names)]
        for a in order:
            for r in ((0.3, 0.02) if rnd % 2 == 0 else (0.02, 0.3)):
                got[(a, r)] += clicks_in_windows(a, reps, r)[1]
    print(f"::notice title=click by rest before it, median [IQR] ms, {ROUNDS}x{reps} each::"
          + " · ".join(f"{a} rest {r:g}s {summary(got[(a, r)])}" for a in names for r in (0.3, 0.02)))
    json.dump({f"{a}@{r}": v for (a, r), v in got.items()}, open(os.path.join(out_dir, "rest.json"), "w"))
    clean()


if MODE == "split":
    split_mode(sys.argv[2], sys.argv[3])
elif MODE == "cli":
    cli_mode(sys.argv[2], sys.argv[3], sys.argv[4])
elif MODE == "time":
    time_mode(sys.argv[2], sys.argv[3])
elif MODE == "profile":
    profile_mode(sys.argv[2], sys.argv[3], sys.argv[4])
elif MODE == "rest":
    rest_mode(sys.argv[2], sys.argv[3])
elif MODE == "sched":
    sched_mode(sys.argv[2], sys.argv[3], sys.argv[4])
else:
    raise SystemExit(__doc__)
