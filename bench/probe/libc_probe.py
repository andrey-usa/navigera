#!/usr/bin/env python3
"""What the static musl build costs a call, and where.

    libc_probe.py time BASE_URL OUT_DIR
    libc_probe.py profile ARM BASE_URL OUT_DIR
    libc_probe.py cli ARM BASE_URL OUT_DIR
    libc_probe.py split BASE_URL OUT_DIR

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


if MODE == "split":
    split_mode(sys.argv[2], sys.argv[3])
elif MODE == "cli":
    cli_mode(sys.argv[2], sys.argv[3], sys.argv[4])
elif MODE == "time":
    time_mode(sys.argv[2], sys.argv[3])
elif MODE == "profile":
    profile_mode(sys.argv[2], sys.argv[3], sys.argv[4])
else:
    raise SystemExit(__doc__)
