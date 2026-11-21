#!/usr/bin/env python3
"""Experiment: what a goto of a page of 300 no-store images pays for the
request log over WebDriver BiDi. bench/gate/engines.py's image server
(TCP_NODELAY), a fresh `serve` session per sample (repeated loads in one
tab drift on some Chromes), variants alternated per round, order rotated.

    goto_variants.py --engine firefox|bidi|classic --rounds 10 --gotos 3 \
        [--env label:KEY=VAL ...] label=binary ...

Prints one JSON line per session (each goto's elapsed_ms, navigera's own
CPU in ms) and a ::notice line per variant with the medians.
"""
import argparse, json, os, resource, statistics, subprocess, sys, time
from pathlib import Path

ap = argparse.ArgumentParser()
ap.add_argument("--engine", default="bidi")
ap.add_argument("--rounds", type=int, default=10)
ap.add_argument("--gotos", type=int, default=3)
ap.add_argument("--out", default="")
ap.add_argument("--env", action="append", default=[])
ap.add_argument("bins", nargs="+")
a = ap.parse_args()

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "gate"))
import engines  # noqa: E402

images = engines.serve_images()
proc = None
if a.engine == "firefox":
    args = ["--engine", "firefox"]
else:
    proc, url, caps = engines.chromedriver()
    args = ["--webdriver", url, "--capabilities", caps] + (["--bidi"] if a.engine == "bidi" else [])
bins = [b.split("=", 1) for b in a.bins]
envs = {}
for e in a.env:
    label, kv = e.split(":", 1)
    k, v = kv.split("=", 1)
    envs.setdefault(label, {})[k] = v
res = {label: [] for label, _ in bins}
cpu = {label: [] for label, _ in bins}
out = open(a.out, "a") if a.out else None
try:
    for r in range(a.rounds):
        order = bins[r % len(bins):] + bins[:r % len(bins)]
        for label, binary in order:
            ops = [{"op": "goto", "url": images} for _ in range(a.gotos)]
            lines = [json.dumps({"id": i, **op}) for i, op in enumerate(ops)]
            lines.append(json.dumps({"id": len(ops), "op": "quit"}))
            env = dict(os.environ, **envs.get(label, {}))
            ru0 = resource.getrusage(resource.RUSAGE_CHILDREN)
            t = time.monotonic()
            p = subprocess.run([binary, *args, "serve"], input="\n".join(lines) + "\n",
                               capture_output=True, text=True, timeout=600, env=env)
            wall = time.monotonic() - t
            ru1 = resource.getrusage(resource.RUSAGE_CHILDREN)
            # Children reaped by navigera count too (Firefox, on that engine).
            c = round((ru1.ru_utime - ru0.ru_utime + ru1.ru_stime - ru0.ru_stime) * 1000)
            ms = []
            for line in p.stdout.splitlines():
                try:
                    o = json.loads(line)
                except ValueError:
                    continue
                if isinstance(o, dict) and o.get("id", -1) < a.gotos:
                    ms.append(o.get("elapsed_ms") if o.get("ok") else None)
            res[label].append(ms)
            cpu[label].append(c)
            rec = {"round": r, "label": label, "engine": a.engine, "ms": ms, "wall_s": round(wall, 2), "cpu_ms": c}
            if len(ms) < a.gotos or None in ms:
                rec["stderr"] = p.stderr[-300:]
            print(json.dumps(rec), flush=True)
            if out:
                out.write(json.dumps(rec) + "\n")
finally:
    if proc:
        proc.kill()

for label, runs in res.items():
    cols = []
    for k in range(a.gotos):
        v = [x[k] for x in runs if len(x) > k and x[k] is not None]
        cols.append(f"goto{k+1} {statistics.median(v):.0f} ({min(v):.0f}-{max(v):.0f})" if v else f"goto{k+1} -")
    allv = [x for run in runs for x in run if x is not None]
    total = f"all {statistics.median(allv):.0f}" if allv else "all -"
    print(f"::notice::{a.engine} {label}: {total}; " + "; ".join(cols)
          + f"; cpu {statistics.median(cpu[label]):.0f} ms/session")
