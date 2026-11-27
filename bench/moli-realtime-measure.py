#!/usr/bin/env python3
"""Measure one Moli launch variant for navigera #27 (measurement-only).

Times, per variant, on CI:
  - the Moli server process's CPU over 10 s idle (3 windows), page loaded
  - goto, click, text, ax, eval (median of 5 reps) on:
      page A: the #27 example (button #b, script-appended #late)
      page B: a ~4,500-node page (generated, count asserted)
Also records the layout-probe verdict ([layout] live vs kept until a
screenshot) from the session log, and asserts the #27 answers
(click lands, rect follows the scroll) hold for the variant.

Usage:
  python3 bench/moli-realtime-measure.py --binary <navigera> \
      --moli-bin <moli> --label <realtime|layout> --out <results.json>
The caller builds the binary from the tree under test; the label is only
a tag. Moli 1.1.28 is required for --realtime; the layout variant is the
same binary built from a tree without the flag.
"""
import argparse
import json
import os
import re
import statistics
import subprocess
import sys
import time
import urllib.parse
from pathlib import Path

PAGE_A_HTML = (
    '<!doctype html><html><head><style>body{margin:0}'
    'button{position:absolute}</style></head><body>'
    '<button id="b" style="left:100px;top:200px">Press</button>'
    '<div style="position:absolute;left:0;top:2500px;width:10px;'
    'height:10px"></div></body></html>'
)
PAGE_A_URL = "data:text/html," + urllib.parse.quote(PAGE_A_HTML)
APPEND_LATE = (
    "const b=document.createElement('button');b.id='late';"
    "b.style.cssText='position:absolute;left:400px;top:400px;"
    "width:80px;height:30px';b.textContent='Later';"
    "b.onclick=()=>{document.title='clicked'};"
    "document.body.appendChild(b);'appended'"
)
LATE_RECT = "JSON.stringify(document.getElementById('late').getBoundingClientRect())"
BUY_RECT = "JSON.stringify(document.getElementById('buy').getBoundingClientRect())"
N_REPS = 10
IDLE_WINDOWS = 3
IDLE_SECS = 10
HTTP_PORT = 8931


def gen_page_b(path: Path) -> None:
    rows = "".join(
        f"<tr><td>SKU-{i}</td><td>Widget {i}</td>"
        f"<td>${i % 500}.00</td><td>vendor-{i % 5}</td></tr>"
        for i in range(900)
    )
    path.write_text(
        "<!doctype html><html><head><meta charset='utf-8'>"
        "<title>Parts — 900 rows</title></head><body>"
        "<h1>Parts</h1>"
        "<button id='buy' onclick=\"document.title='bought'\">Buy</button>"
        f"<table><tbody>{rows}</tbody></table></body></html>",
        encoding="utf-8",
    )


def run(cmd, env, timeout=120):
    start = time.monotonic()
    proc = subprocess.run(
        cmd, env=env, capture_output=True, text=True, timeout=timeout
    )
    return (time.monotonic() - start) * 1000.0, proc


def cpu_times(pid: int):
    stat = Path(f"/proc/{pid}/stat").read_text()
    fields = stat[stat.rfind(")") + 2 :].split()
    # fields[11] = utime, fields[12] = stime (fields are 3.. after comm)
    return int(fields[11]) + int(fields[12])


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--binary", required=True)
    ap.add_argument("--moli-bin", required=True)
    ap.add_argument("--label", required=True)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    nv = str(Path(args.binary).resolve())
    env = dict(os.environ)
    env["MOLI_BIN"] = str(Path(args.moli_bin).resolve())
    env["NAVIGERA_TIMINGS"] = "1"
    env["NAVIGERA_VERBOSE"] = "1"
    session = f"meas-{args.label}"
    errors = []
    result = {
        "label": args.label,
        "navigera": subprocess.run(
            [nv, "--version"], capture_output=True, text=True
        ).stdout.strip(),
        "moli": subprocess.run(
            [env["MOLI_BIN"], "--version"], capture_output=True, text=True
        ).stdout.strip(),
    }

    page_b = Path("/tmp/moli-page-b.html")
    gen_page_b(page_b)
    server = subprocess.Popen(
        [sys.executable, "-m", "http.server", str(HTTP_PORT),
         "--bind", "127.0.0.1", "--directory", "/tmp"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    page_b_url = f"http://127.0.0.1:{HTTP_PORT}/moli-page-b.html"
    try:
        ms, proc = run(
            [nv, "--engine", "moli", "--session", session, "start"], env
        )
        if proc.returncode != 0:
            print(proc.stdout + proc.stderr, file=sys.stderr)
            return 1
        info = json.loads(proc.stdout)
        log_path = Path(info["log"])

        def nv_call(*a):
            return run([nv, "--session", session, *a], env)

        # Page A loaded, then idle: the Moli server's CPU over 10 s.
        ms, proc = nv_call("goto", PAGE_A_URL)
        if proc.returncode != 0:
            errors.append(f"goto page A: {proc.stdout} {proc.stderr}")
        time.sleep(2)
        pids = subprocess.run(
            ["pgrep", "-f", "moli serve"], capture_output=True, text=True
        ).stdout.split()
        if len(pids) != 1:
            errors.append(f"moli serve pids: {pids}")
            idle = []
        else:
            pid = int(pids[0])
            result["moli_pid"] = pid
            clk = int(subprocess.run(
                ["getconf", "CLK_TCK"], capture_output=True, text=True
            ).stdout.strip())
            idle = []
            for _ in range(IDLE_WINDOWS):
                before = cpu_times(pid)
                time.sleep(IDLE_SECS)
                after = cpu_times(pid)
                idle.append(round((after - before) / clk / IDLE_SECS * 100, 3))
            rss = Path(f"/proc/{pid}/status").read_text()
            m = re.search(r"VmRSS:\s+(\d+) kB", rss)
            result["moli_rss_kb"] = int(m.group(1)) if m else None
        result["idle_cpu_pct_10s"] = idle

        # Correctness once per variant: the #27 answers.
        nv_call("eval", APPEND_LATE)
        ms, proc = nv_call("eval", LATE_RECT)
        rect = json.loads(proc.stdout) if proc.returncode == 0 else {}
        if (rect.get("x"), rect.get("y"), rect.get("width"),
                rect.get("height")) != (400, 400, 80, 30):
            errors.append(f"late rect: {proc.stdout.strip()}")
        ms, proc = nv_call("click", "--text", "Later")
        ms, proc = nv_call("eval", "document.title")
        if proc.stdout.strip() != "clicked":
            errors.append(f"click title: {proc.stdout.strip()}")
        ms, proc = nv_call("eval", "window.scrollTo(0,1059);window.scrollY")
        ms, proc = nv_call("eval", LATE_RECT)
        rect = json.loads(proc.stdout) if proc.returncode == 0 else {}
        if rect.get("top") != 400 - 1059:
            errors.append(f"late rect after scroll: {proc.stdout.strip()}")

        # Op timings: median of 5 reps, fresh page each rep.
        ops = {}
        for name, page, steps in (
            ("page_a", PAGE_A_URL, ("eval_rect", "click", "text", "ax")),
            ("page_b", page_b_url, ("eval_rect", "click", "text", "ax")),
        ):
            times = {op: [] for op in ("goto", *steps)}
            counts = {}
            for _ in range(N_REPS):
                ms, proc = nv_call("goto", page)
                times["goto"].append(round(ms, 2))
                if proc.returncode != 0:
                    errors.append(f"goto {name}: {proc.stderr.strip()[:200]}")
                    continue
                if name == "page_a":
                    nv_call("eval", APPEND_LATE)
                    expr, click = LATE_RECT, ["click", "--text", "Later"]
                else:
                    expr, click = BUY_RECT, ["click", "--text", "Buy"]
                    if not counts:
                        ms, proc = nv_call(
                            "eval", "document.querySelectorAll('*').length"
                        )
                        counts["nodes"] = proc.stdout.strip()
                ms, proc = nv_call("eval", expr)
                times["eval_rect"].append(round(ms, 2))
                ms, proc = nv_call(*click)
                times["click"].append(round(ms, 2))
                ms, proc = nv_call("text")
                times["text"].append(round(ms, 2))
                ms, proc = nv_call("ax")
                times["ax"].append(round(ms, 2))
            ops[name] = {
                op: round(statistics.median(v), 2) if v else None
                for op, v in times.items()
            }
            ops[name]["reps"] = {op: v for op, v in times.items()}
            if counts:
                ops[name]["nodes"] = counts["nodes"]
                n = int(counts["nodes"])
                if not 4400 <= n <= 4600:
                    errors.append(f"page B nodes: {n} (want ~4500)")
        result["ops_ms_median"] = ops

        log = log_path.read_text(errors="ignore") if log_path.exists() else ""
        m = re.search(r"\[layout\] ([^\n]+)", log)
        result["layout_probe"] = m.group(1) if m else None
        result["layout_refresh_phases"] = log.count("layout_refresh")
        nv_call("quit")
    finally:
        server.terminate()
    result["errors"] = errors
    Path(args.out).write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
