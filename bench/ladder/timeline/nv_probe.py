#!/usr/bin/env python3
"""Probe A : `navigera serve` timeline on https://example.com.

One warm serve process: goto https://example.com, one eval, quit.
Client-side wall for each op (stdin/stdout protocol included), the eval's
engine-side elapsed_ms, and the NAVIGERA_TIMINGS launch phases. Prints one
JSON line on stdout.
"""
import argparse
import json
import os
import subprocess
import time


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--navigera", required=True)
    ap.add_argument("--chrome", default="")
    args = ap.parse_args()

    argv = [args.navigera, "serve"]
    if args.chrome:
        argv += ["--chromium", args.chrome]
    stderr_path = f"/tmp/nv-timeline-{os.getpid()}.stderr"
    env = dict(os.environ, NAVIGERA_TIMINGS="1", NAVIGERA_OUTPUT="json")
    with open(stderr_path, "w") as se:
        proc = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                stderr=se, text=True, bufsize=1, env=env)

        def cmd(op: dict) -> dict:
            proc.stdin.write(json.dumps(op) + "\n")
            proc.stdin.flush()
            line = proc.stdout.readline()
            if not line:
                raise RuntimeError("serve closed stdout")
            resp = json.loads(line)
            if not resp.get("ok"):
                raise RuntimeError(f"op failed: {resp.get('error')}")
            return resp

        t0 = time.perf_counter()
        cmd({"id": 1, "op": "goto", "url": "https://example.com"})
        t1 = time.perf_counter()
        resp = cmd({"id": 2, "op": "eval", "expression": "() => document.title"})
        t2 = time.perf_counter()
        cmd({"id": 3, "op": "quit"})
        proc.stdin.close()
        proc.wait(timeout=60)

    phases = {}
    with open(stderr_path, encoding="utf-8", errors="replace") as f:
        for line in f:
            if line.startswith("NAVIGERA_TIMINGS "):
                phases = json.loads(line[len("NAVIGERA_TIMINGS "):])

    print(json.dumps({
        "tool": "navigera",
        "goto_client_ms": round((t1 - t0) * 1000, 3),
        "eval_client_ms": round((t2 - t1) * 1000, 3),
        "eval_engine_ms": resp.get("elapsed_ms"),
        "title": resp.get("result"),
        "phases": phases,
    }))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
