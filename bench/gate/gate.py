#!/usr/bin/env python3
"""Timing gate: a release candidate against the release before it.

Runs the same fixed probes with both binaries on this machine (one `serve`
process each, one browser, warm), takes the median of each probe, and fails
when the candidate is slower than the base by more than the threshold on
any probe (and by more than a small absolute floor, so a 3 ms probe can't
fail on noise).

    python3 bench/gate/gate.py --base <old navigera> --candidate <new navigera> \
        [--runs 5] [--threshold 0.25] [--summary out.md]

The pages come from tests/fixtures/interactions_site.py plus a few written
here (a header over a 2,000-row table, 13,000 nested divs, a 2,000-option
select). Exit 0: no regression; 1: a regression (the table says which);
2: a probe failed to run.
"""
import argparse
import json
import os
import statistics
import subprocess
import sys
import tempfile
import threading
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FLOOR_MS = 20.0


def write_pages(dir_: Path) -> None:
    rows = "".join(
        f'<tr><td>Item {i}</td><td><button>Edit {i}</button></td><td><a href="#r{i}">View {i}</a></td></tr>'
        for i in range(2000)
    )
    (dir_ / "header-table.html").write_text(
        "<!doctype html><title>rows</title><header><nav aria-label=Main><a href=/>Home</a> <a href=/o>Orders</a></nav>"
        "<h1>Invoices</h1><form role=search><label>Search <input name=q></label> <button>Go</button></form></header>"
        f"<main><table><tbody>{rows}</tbody></table></main>"
    )
    cards = "".join(
        f'<div class=card><div class=head><div>Order {i}</div><div>Customer {i}</div></div>'
        f'<div class=body><div class=row><div>Qty</div><div>{i % 7 + 1}</div></div>'
        f'<div class=row><div>Status</div><div>Shipped</div></div></div><div class=foot><div>Total</div></div></div>'
        for i in range(1000)
    )
    (dir_ / "divs.html").write_text(f"<!doctype html><title>divs</title><div id=app>{cards}</div>")
    options = "".join(f'<option value="c{i}">Country {i}</option>' for i in range(2000))
    (dir_ / "select.html").write_text(
        f"<!doctype html><title>sel</title><label>Country <select id=c>{options}</select></label>"
    )


class Quiet(SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass


def serve_dir(dir_: Path) -> str:
    srv = ThreadingHTTPServer(("127.0.0.1", 0), partial(Quiet, directory=str(dir_)))
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return f"http://127.0.0.1:{srv.server_address[1]}"


def start_site() -> tuple[subprocess.Popen, str]:
    site = subprocess.Popen(
        [sys.executable, str(ROOT / "tests/fixtures/interactions_site.py")],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    line = site.stdout.readline()
    return site, line.split()[1]


def probes(site: str, pages: str, runs: int) -> list[tuple[str, list[dict], set[int]]]:
    """(name, ops, ids of the ops whose time counts). Each probe's ops run
    `runs` times; for a probe that reads a fresh load, the load is repeated."""

    def repeat(name, setup, measured, fresh):
        ops, timed = [], set()
        if not fresh:
            ops += setup
        for _ in range(runs + 1):  # one warm-up
            if fresh:
                ops += setup
            ops.append(dict(measured))
            if _ > 0:
                timed.add(len(ops) - 1)
        return name, ops, timed

    ax = {"op": "ax", "format": "text"}
    return [
        repeat("ax 2,000 rows, whole tree", [{"op": "goto", "url": f"{site}/big-table"}], {**ax, "limit": 0}, False),
        repeat("ax 2,000 rows, default limit", [{"op": "goto", "url": f"{site}/big-table"}], ax, False),
        repeat("ax scoped to a header (2,000 rows)", [{"op": "goto", "url": f"{pages}/header-table.html"}],
               {**ax, "selector": "header"}, False),
        repeat("ax 13,000 divs", [{"op": "goto", "url": f"{pages}/divs.html"}], ax, False),
        repeat("first ax on a long-poll page", [{"op": "goto", "url": f"{site}/long-poll"}], ax, True),
        repeat("first ax on a beacon page", [{"op": "goto", "url": f"{site}/beacons"}], ax, True),
        repeat("ax after a click that fetches 200 ms",
               [{"op": "goto", "url": f"{site}/click-fetch"}, {"op": "click", "role": "button", "name": "Go"}],
               ax, True),
        repeat("click under a cookie banner (fails)", [{"op": "goto", "url": f"{site}/cookie-banner"}],
               {"op": "click", "selector": "#buy"}, True),
        repeat("select on 2,000 options", [{"op": "goto", "url": f"{pages}/select.html"}],
               {"op": "select", "selector": "#c", "value": ["Country 1500"]}, False),
    ]


def run_probe(binary: str, ops: list[dict], timed: set[int]) -> list[float]:
    lines = [json.dumps({"id": i, **op}) for i, op in enumerate(ops)]
    lines.append(json.dumps({"id": len(ops), "op": "quit"}))
    env = dict(os.environ)
    out = subprocess.run(
        [binary, "serve"], input="\n".join(lines) + "\n", capture_output=True, text=True, env=env, timeout=600
    ).stdout
    times = []
    for line in out.splitlines():
        try:
            r = json.loads(line)
        except ValueError:
            continue
        if r.get("id") in timed:
            times.append(float(r["elapsed_ms"]))
    if len(times) != len(timed):
        raise RuntimeError(f"{binary}: {len(times)} of {len(timed)} timed answers")
    return times


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--base", required=True)
    ap.add_argument("--candidate", required=True)
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--threshold", type=float, default=0.25)
    ap.add_argument("--summary")
    args = ap.parse_args()

    site, site_url = start_site()
    with tempfile.TemporaryDirectory() as tmp:
        write_pages(Path(tmp))
        pages = serve_dir(Path(tmp))
        rows, failed, broken = [], [], []
        try:
            for name, ops, timed in probes(site_url, pages, args.runs):
                try:
                    # Interleaved: base, candidate, base, candidate (drift hits both).
                    b1 = run_probe(args.base, ops, timed)
                    c1 = run_probe(args.candidate, ops, timed)
                    b = statistics.median(b1 + run_probe(args.base, ops, timed))
                    c = statistics.median(c1 + run_probe(args.candidate, ops, timed))
                except Exception as e:  # noqa: BLE001 - reported in the table
                    broken.append(name)
                    rows.append(f"| {name} | - | - | - | could not run: {str(e)[:80]} |")
                    continue
                ratio = c / b if b else 1.0
                worse = c > b * (1 + args.threshold) and c - b > FLOOR_MS
                if worse:
                    failed.append(name)
                verdict = "SLOWER" if worse else ("faster" if ratio < 0.9 else "same")
                rows.append(f"| {name} | {b:.0f} | {c:.0f} | {ratio:.2f}x | {verdict} |")
        finally:
            site.kill()

    table = "\n".join(
        [
            f"Timing gate: candidate vs base, median of {2 * args.runs} per probe (ms), "
            f"fails above +{args.threshold:.0%} and +{FLOOR_MS:.0f} ms",
            "",
            "| probe | base | candidate | ratio | |",
            "|---|---:|---:|---:|---|",
            *rows,
            "",
            ("FAIL: " + ", ".join(failed)) if failed else ("could not run: " + ", ".join(broken)) if broken else "ok",
        ]
    )
    print(table)
    if args.summary:
        Path(args.summary).write_text(table + "\n")
    return 1 if failed else 2 if broken else 0


if __name__ == "__main__":
    sys.exit(main())
