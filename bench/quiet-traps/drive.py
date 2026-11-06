#!/usr/bin/env python3
"""Final-state rate per trap, per navigera build, N reps each.

Usage: python3 drive.py --bin /path/navigera --label next --reps 5 --port 8973
Writes results-<label>.jsonl: one row per rep (trap, final-state flag,
click wall ms, rc, report tail).
"""
import argparse
import json
import re
import subprocess
import time

PROMPT_WALL_MS = 1500
LONGPOLL_WALL_MS = 5000


def run(bin_, session, args, timeout=30):
    t0 = time.monotonic()
    p = subprocess.run([bin_, "-s", session] + args, capture_output=True, text=True, timeout=timeout)
    wall = (time.monotonic() - t0) * 1000
    return p.stdout + p.stderr, wall, p.returncode


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True)
    ap.add_argument("--label", required=True)
    ap.add_argument("--reps", type=int, default=5)
    ap.add_argument("--port", type=int, default=8973)
    ap.add_argument("--traps", default="")
    a = ap.parse_args()

    import server
    traps = a.traps.split(",") if a.traps else server.TRAPS
    base = f"http://127.0.0.1:{a.port}"
    rows = []
    for trap in traps:
        for rep in range(a.reps):
            s = f"qt-{a.label}-{trap}-{rep}"
            try:
                run(a.bin, s, ["start"], timeout=60)
                run(a.bin, s, ["goto", f"{base}/trap/{trap}"], timeout=60)
                time.sleep(0.6)
                tree, _, rc = run(a.bin, s, ["ax"], timeout=60)
                m = re.search(r'button "go" \[ref=(\d+)\]', tree)
                if not m:
                    rows.append({"trap": trap, "rep": rep, "final": False, "wall_ms": -1, "rc": rc, "out": "no button ref: " + tree[-400:]})
                    continue
                try:
                    out, wall, rc = run(a.bin, s, ["click", m.group(1)], timeout=45)
                except subprocess.TimeoutExpired:
                    out, wall, rc = "", 45000, -1
                if trap == "analytics":
                    ok = rc == 0 and wall < PROMPT_WALL_MS
                elif trap == "longpoll":
                    ok = rc == 0 and "listening" in out and wall < LONGPOLL_WALL_MS
                else:
                    ok = f"final: {trap}" in out
                rows.append({"trap": trap, "rep": rep, "final": ok, "wall_ms": round(wall, 1), "rc": rc, "out": out[-800:]})
            except Exception as e:
                rows.append({"trap": trap, "rep": rep, "final": False, "wall_ms": -1, "rc": -2, "out": f"driver: {e}"})
            finally:
                subprocess.run([a.bin, "-s", s, "quit"], capture_output=True, timeout=30)
            print(json.dumps({k: rows[-1][k] for k in ("trap", "rep", "final", "wall_ms", "rc")}), flush=True)
    with open(f"results-{a.label}.jsonl", "w") as f:
        for r in rows:
            f.write(json.dumps(r) + "\n")


if __name__ == "__main__":
    main()
