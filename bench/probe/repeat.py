#!/usr/bin/env python3
"""Run one test command N times and keep what each run leaves behind.

    repeat.py --arm NAME --n 30 --out DIR [--hang-after S] -- cargo test ...

Each repeat gets a directory of its own (DIR/NAME/rep-NN) and these in its
environment: NAVIGERA_CDP_TRACE (the commands sent, with times),
NAVIGERA_CHROME_LOG (the browser's stderr), NV_KEEP_DIR (a test's session
logs) and NV_SIGNALS_DIR (what a page saw). Crash reports the OS wrote
during the repeat (macOS: DiagnosticReports) are copied there, with their
top frames. A repeat still running after --hang-after seconds gets a
process table and, on Linux, the stacks of navigera and the browser.

Writes DIR/NAME.jsonl (one row per repeat) and DIR/NAME.md (the summary).
"""
import argparse
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

CRASH_DIRS = [Path.home() / "Library/Logs/DiagnosticReports", Path("/Library/Logs/DiagnosticReports")]


def crash_files():
    out = {}
    for d in CRASH_DIRS:
        if d.is_dir():
            for p in d.rglob("*"):
                if p.is_file() and p.suffix in (".ips", ".crash", ".diag"):
                    out[str(p)] = p.stat().st_mtime
    return out


def ips_top(path, n=8):
    """Process, exception and the crashed thread's top frames of a crash report."""
    text = Path(path).read_text(errors="replace")
    first, _, rest = text.partition("\n")
    try:
        hdr, body = json.loads(first), json.loads(rest)
    except ValueError:
        exc = re.search(r"^Exception Type:\s*(.+)$", text, re.M)
        return {"file": Path(path).name, "exception": exc.group(1) if exc else "?", "frames": []}
    imgs = body.get("usedImages", [])
    ft = body.get("faultingThread", 0)
    threads = body.get("threads", [])
    th = threads[ft] if ft < len(threads) else {}
    frames = []
    for f in th.get("frames", [])[:n]:
        i = f.get("imageIndex")
        img = imgs[i].get("name", "?") if isinstance(i, int) and i < len(imgs) else "?"
        if "symbol" in f:
            frames.append(f"{img}!{f['symbol']}+{f.get('symbolLocation', 0)}")
        else:
            frames.append(f"{img}+0x{f.get('imageOffset', 0):x}")
    exc = body.get("exception", {})
    term = body.get("termination", {})
    return {
        "file": Path(path).name,
        "process": body.get("procName") or hdr.get("app_name"),
        "version": hdr.get("app_version"),
        "exception": " ".join(str(x) for x in (exc.get("type"), exc.get("signal"), exc.get("subtype")) if x),
        "termination": term.get("indicator"),
        "thread": f"{ft} {th.get('name') or th.get('queue') or ''}".strip(),
        "frames": frames,
    }


def snapshot(rep):
    """A repeat that runs long: the process table, and on Linux the stacks."""
    ps = ["ps", "-ww", "-eo", "pid,ppid,pgid,stat,etime,time,args"]
    if sys.platform.startswith("linux"):
        ps = ["ps", "-ww", "-eo", "pid,ppid,pgid,stat,etimes,time,wchan:32,args", "--forest"]
    table = subprocess.run(ps, capture_output=True, text=True).stdout
    (rep / "hang-ps.txt").write_text(table)
    if not sys.platform.startswith("linux"):
        return
    for line in table.splitlines()[1:]:
        cols = line.split(None, 7)
        if len(cols) < 8:
            continue
        pid, args = cols[0], cols[7]
        main_browser = "--nv-network-sandbox-test" in args and "--type=" not in args
        ours = "target/debug/navigera" in args or "/deps/navigera" in args
        if not (main_browser or ours):
            continue
        out = rep / f"hang-stack-{pid}.txt"
        r = subprocess.run(
            ["sudo", "-n", "timeout", "40", "gdb", "-p", pid, "-batch", "-ex", "thread apply all bt 30"],
            capture_output=True, text=True)
        k = subprocess.run(["sudo", "-n", "sh", "-c", f"for t in /proc/{pid}/task/*; do echo \"== $t $(cat $t/comm) $(cat $t/wchan)\"; cat $t/stack; done"],
                           capture_output=True, text=True)
        out.write_text(f"$ {args}\n\n{r.stdout}\n{r.stderr[-2000:]}\n---- kernel stacks\n{k.stdout}\n{k.stderr[-500:]}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--arm", required=True)
    ap.add_argument("--n", type=int, default=30)
    ap.add_argument("--out", required=True)
    ap.add_argument("--hang-after", type=float, default=0)
    ap.add_argument("--timeout", type=float, default=300)
    ap.add_argument("cmd", nargs=argparse.REMAINDER)
    a = ap.parse_args()
    cmd = a.cmd[1:] if a.cmd and a.cmd[0] == "--" else a.cmd
    out = Path(a.out)
    arm_dir = out / a.arm
    arm_dir.mkdir(parents=True, exist_ok=True)
    seen = crash_files()
    rows = []
    step_start = time.time()
    for i in range(1, a.n + 1):
        rep = arm_dir / f"rep-{i:02d}"
        rep.mkdir(exist_ok=True)
        env = dict(os.environ,
                   NAVIGERA_CDP_TRACE=str(rep / "cdp-trace.jsonl"),
                   NAVIGERA_CHROME_LOG=str(rep / "browser-stderr.log"),
                   NV_KEEP_DIR=str(rep), NV_SIGNALS_DIR=str(rep))
        t0 = time.time()
        with open(rep / "test.log", "wb") as log:
            p = subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT, env=env, start_new_session=True)
            hung = timed_out = False
            try:
                p.wait(timeout=a.hang_after or a.timeout)
            except subprocess.TimeoutExpired:
                if a.hang_after:
                    hung = True
                    snapshot(rep)
                try:
                    p.wait(timeout=max(1, a.timeout - (time.time() - t0)))
                except subprocess.TimeoutExpired:
                    timed_out = True
                    os.killpg(p.pid, signal.SIGKILL)
                    p.wait()
        secs = time.time() - t0
        rc = p.returncode
        if rc != 0:
            time.sleep(6)  # ReportCrash writes the report a few seconds after
        crashes = []
        now = crash_files()
        for path, mtime in sorted(now.items(), key=lambda kv: kv[1]):
            if path in seen:
                continue
            seen[path] = mtime
            (rep / "crash").mkdir(exist_ok=True)
            shutil.copy(path, rep / "crash" / Path(path).name)
            try:
                crashes.append(ips_top(path))
            except Exception as e:  # a report we can't parse is still kept
                crashes.append({"file": Path(path).name, "exception": f"(unparsed: {e})", "frames": []})
        text = (rep / "test.log").read_text(errors="replace")
        failed = re.findall(r"^test (\S+) \.\.\. FAILED", text, re.M)
        passed = re.findall(r"^test (\S+) \.\.\. ok", text, re.M)
        m = re.search(r"panicked at ([^\n]+)\n([^\n]*)", text)
        first = f"{m.group(1)} {m.group(2)}"[:600] if m else ""
        row = {"arm": a.arm, "rep": i, "rc": rc, "secs": round(secs, 1), "start_offset_s": round(t0 - step_start, 1),
               "hung_past": a.hang_after if hung else None, "timed_out": timed_out, "failed": failed,
               "passed": len(passed), "first": first, "crashes": crashes}
        rows.append(row)
        print(json.dumps(row)[:400], flush=True)
    with open(out / f"{a.arm}.jsonl", "w") as f:
        for r in rows:
            f.write(json.dumps(r) + "\n")
    write_summary(out / f"{a.arm}.md", a.arm, rows, cmd)


def write_summary(path, arm, rows, cmd):
    bad = [r for r in rows if r["rc"] != 0]
    tests = {}
    for r in rows:
        for t in r["failed"]:
            tests[t] = tests.get(t, 0) + 1
    with_crash = [r for r in rows if r["crashes"]]
    lines = [f"{arm}: {len(bad)}/{len(rows)} repeats failed; first failing repeat "
             f"{bad[0]['rep'] if bad else '-'}; crash reports in {len(with_crash)} repeats "
             f"({sum(1 for r in with_crash if r['rc'] == 0)} of them passed); "
             f"median {sorted(r['secs'] for r in rows)[len(rows) // 2]} s; `{' '.join(cmd)}`"]
    if tests:
        lines.append("failed tests: " + ", ".join(f"{t} x{n}" for t, n in sorted(tests.items())))
    for r in rows:
        if r["rc"] == 0 and not r["crashes"]:
            continue
        s = f"rep {r['rep']} rc {r['rc']} {r['secs']} s"
        if r["hung_past"]:
            s += f" (still running at {r['hung_past']} s)"
        if r["first"]:
            s += ": " + r["first"][:300]
        for c in r["crashes"][:2]:
            s += f" | {c.get('process')} {c.get('exception')} thread {c.get('thread', '?')}: " + " < ".join(c["frames"][:5])
        lines.append(s)
    Path(path).write_text("\n".join(lines)[:28000] + "\n")


if __name__ == "__main__":
    main()
