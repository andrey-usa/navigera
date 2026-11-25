#!/usr/bin/env python3
"""Where a ladder click's time goes, navigera against agent-browser.

    click_probe.py BASE_URL OUT_DIR

The ladder's `click` op (bench/ladder/contenders/ops_agent.py): the box
holds "widget", every card shows, then `click #search` filters the list.
Untimed setup and a 0.3 s rest before each timed call, as there.

Arms, interleaved per round: navigera on a shared browser (the default),
navigera with a browser of its own (NAVIGERA_SHARED_BROWSER=0), and
agent-browser. Then traced clicks: navigera's session server writes each
CDP command and its answer with a timestamp (NAVIGERA_CDP_TRACE, this
branch's patch), agent-browser goes through log_proxy.py in front of a
Chrome of its own. Prints `::notice::` lines; the traces go to OUT_DIR.
"""
import json
import os
import statistics
import subprocess
import sys
import time

BASE, OUT = sys.argv[1], sys.argv[2]
NAV = os.environ["NAVIGERA"]
AB = os.environ.get("AGENT_BROWSER", "agent-browser")
CHROME = os.environ["CHROME_BIN"]
ROUNDS, REPS = int(os.environ.get("ROUNDS", "3")), int(os.environ.get("REPS", "15"))
SETUP = ("(() => { document.getElementById('q').value='widget'; "
         "for (const c of document.querySelectorAll('.card.hidden')) c.classList.remove('hidden'); return 1 })()")
os.makedirs(OUT, exist_ok=True)


def run(argv, env=None):
    t = time.perf_counter()
    p = subprocess.run(argv, capture_output=True, text=True, env=env, timeout=60)
    ms = (time.perf_counter() - t) * 1000
    if p.returncode != 0:
        raise RuntimeError(f"{argv}: {p.returncode} {p.stdout[-300:]} {p.stderr[-300:]}")
    return ms, p.stdout


ENV_AB = dict(os.environ, AGENT_BROWSER_EXECUTABLE_PATH=CHROME, AGENT_BROWSER_ARGS="--no-sandbox")


def arm_cmds(name):
    if name.startswith("nv"):
        s = name
        return ([NAV, "--session", s, "eval", SETUP], [NAV, "--session", s, "click", "--selector", "#search"], None)
    return ([AB, "--session", name, "eval", SETUP], [AB, "--session", name, "click", "#search"], ENV_AB)


def visible(name):
    expr = "[...document.querySelectorAll('.card:not(.hidden)')].length"
    argv = [NAV, "--session", name, "eval", expr] if name.startswith("nv") else [AB, "--session", name, "eval", expr]
    return run(argv, None if name.startswith("nv") else ENV_AB)[1].strip()


def start():
    run([NAV, "--session", "nv", "start", "--chromium", CHROME])
    run([NAV, "--session", "nv", "goto", f"{BASE}/page_a.html"])
    run([NAV, "--session", "nvown", "start", "--chromium", CHROME], dict(os.environ, NAVIGERA_SHARED_BROWSER="0"))
    run([NAV, "--session", "nvown", "goto", f"{BASE}/page_a.html"])
    run([AB, "--session", "ab", "--json", "open", f"{BASE}/page_a.html"], ENV_AB)


def timed(name, reps):
    setup, click, env = arm_cmds(name)
    out = []
    for _ in range(reps):
        run(setup, env)
        time.sleep(0.3)
        out.append(run(click, env)[0])
    return out


def summary(xs):
    q = statistics.quantiles(xs, n=4)
    return f"median {statistics.median(xs):.2f} ms [IQR {q[0]:.2f}-{q[2]:.2f}] min {min(xs):.2f}"


def main():
    start()
    arms = ["nv", "nvown", "ab"]
    got = {a: [] for a in arms}
    for r in range(ROUNDS):
        for a in (arms if r % 2 == 0 else arms[::-1]):
            got[a] += timed(a, REPS)
    for a in arms:
        print(f"::notice::click {a}: {summary(got[a])} ({len(got[a])} clicks; visible after: {visible(a)})")
    json.dump(got, open(os.path.join(OUT, "times.json"), "w"))

    # navigera traced: a session server started with the trace on.
    trace = os.path.join(OUT, "nv-trace.jsonl")
    run([NAV, "--session", "nvt", "start", "--chromium", CHROME], dict(os.environ, NAVIGERA_CDP_TRACE=trace, NAVIGERA_SHARED_BROWSER="0"))
    run([NAV, "--session", "nvt", "goto", f"{BASE}/page_a.html"])
    setup, click, _ = arm_cmds("nvt")
    lines = []
    for i in range(6):
        run(setup)
        time.sleep(0.3)
        n0 = sum(1 for _ in open(trace))
        ms = run(click)[0]
        rows = [json.loads(x) for x in open(trace).read().splitlines()[n0:]]
        lines.append((ms, rows))
    lines = lines[1:]
    lines.sort(key=lambda x: x[0])
    ms, rows = lines[len(lines) // 2]
    t0 = rows[0]["t"] if rows else 0
    seq = []
    for d in rows:
        m = d["method"]
        tag = "<" if m.startswith("DONE ") else ">"
        m = m[5:] if tag == "<" else m
        short = m.split(".")[-1]
        seq.append(f"{(d['t'] - t0) / 1000:.2f}{tag}{short}")
    print(f"::notice::nv traced click (median of 5, {ms:.2f} ms wall): " + " ".join(seq))
    json.dump(lines, open(os.path.join(OUT, "nv-traced.json"), "w"))

    # agent-browser traced, through the logging proxy, on a Chrome of its own.
    plog = os.path.join(OUT, "ab-cdp.jsonl")
    prof = os.path.join(OUT, "ab-prof")
    chrome = subprocess.Popen([CHROME, "--headless", "--no-sandbox", "--remote-debugging-port=9333",
                               f"--user-data-dir={prof}", "about:blank"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    proxy = subprocess.Popen([sys.executable, "-I", os.path.join(os.path.dirname(__file__), "log_proxy.py"), "9334", "9333", plog])
    time.sleep(3)
    run([AB, "--session", "abp", "connect", "9334"], ENV_AB)
    run([AB, "--session", "abp", "open", f"{BASE}/page_a.html"], ENV_AB)
    setup, click, env = arm_cmds("abp")
    traced = []
    for i in range(6):
        run(setup, env)
        time.sleep(0.3)
        n0 = sum(1 for _ in open(plog))
        ms = run(click, env)[0]
        rows = [json.loads(x) for x in open(plog).read().splitlines()[n0:]]
        traced.append((ms, rows))
    traced = traced[1:]
    traced.sort(key=lambda x: x[0])
    ms, rows = traced[len(traced) // 2]
    t0 = rows[0]["t"] if rows else 0
    seq = [f"{d['t'] - t0:.2f}{'>' if d['d'] == 'send' else '<'}{(d.get('m') or '').split('.')[-1] or d['id']}"
           for d in rows if d["d"] != "evt"]
    print(f"::notice::ab traced click (median of 5, {ms:.2f} ms wall): " + " ".join(seq))
    json.dump(traced, open(os.path.join(OUT, "ab-traced.json"), "w"))
    proxy.kill()
    chrome.kill()

    # The CLI's own cost: a read with no page work.
    for name in ("nv", "ab"):
        argv = [NAV, "--session", "nv", "eval", "1"] if name == "nv" else [AB, "--session", "ab", "eval", "1"]
        xs = [run(argv, None if name == "nv" else ENV_AB)[0] for _ in range(15)]
        print(f"::notice::eval 1 {name}: {summary(xs)}")


main()
