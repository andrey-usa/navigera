#!/usr/bin/env python3
"""Timing gate: a release candidate against the release before it.

Runs the same fixed probes with both binaries on this machine (one `serve`
process each, one browser, warm), takes the median of each probe, and fails
when the candidate is slower than the base by more than the threshold on
any probe (and by more than a small absolute floor, so a 3 ms probe can't
fail on noise). Most probes time one op by its own `elapsed_ms`; the eval
probe times 200 ops from this side of the pipe and sums them, so work done
before an op starts its clock or after it stops (a round trip before the
reply) counts too, and a sub-millisecond op clears the floor.

    python3 bench/gate/gate.py --base <old navigera> --candidate <new navigera> \
        [--runs 5] [--threshold 0.25] [--summary out.md]

The pages come from tests/fixtures/interactions_site.py plus a few written
here (a header over a 2,000-row table, 13,000 nested divs, a 200-row table
of about 1,400 elements, a 2,000-option select). Exit 0: no regression; 1: a regression (the table says which);
2: a probe failed to run.
"""
import argparse
import json
import os
import statistics
import subprocess
import socketserver
import sys
import tempfile
import threading
import time
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


class Server(ThreadingHTTPServer):
    # The local address is enough; reverse DNS can stall macOS runners.
    def server_bind(self):
        socketserver.TCPServer.server_bind(self)
        self.server_name, self.server_port = self.server_address[:2]


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
    mid = "".join(
        f'<tr><td>Item {i}</td><td>{i * 3} units</td><td><button>Edit {i}</button></td>'
        f'<td><a href="#r{i}">View {i}</a></td></tr>'
        for i in range(200)
    )
    (dir_ / "mid-table.html").write_text(
        "<!doctype html><title>mid</title><h1>Invoices</h1><form><label>Search <input name=q></label> "
        f"<button>Go</button></form><table><tbody>{mid}</tbody></table>"
    )
    options = "".join(f'<option value="c{i}">Country {i}</option>' for i in range(2000))
    (dir_ / "select.html").write_text(
        f"<!doctype html><title>sel</title><label>Country <select id=c>{options}</select></label>"
    )


class Quiet(SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass


def serve_dir(dir_: Path) -> str:
    srv = Server(("127.0.0.1", 0), partial(Quiet, directory=str(dir_)))
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


EVAL_BLOCK = 200


def probes(site: str, pages: str, runs: int) -> list[tuple]:
    """(name, ops, ids of the ops whose time counts[, block]). Each probe's
    ops run `runs` times; for a probe that reads a fresh load, the load is
    repeated. With a block, the timed ops are timed by the client in runs
    of `block` and each run's sum is one sample (see `run_streamed`)."""

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
        repeat("ax a 1,400-element page", [{"op": "goto", "url": f"{pages}/mid-table.html"}], ax, False),
        repeat("first ax on a long-poll page", [{"op": "goto", "url": f"{site}/long-poll"}], ax, True),
        repeat("first ax on a beacon page", [{"op": "goto", "url": f"{site}/beacons"}], ax, True),
        repeat("ax after a click that fetches 200 ms",
               [{"op": "goto", "url": f"{site}/click-fetch"}, {"op": "click", "role": "button", "name": "Go"}],
               ax, True),
        repeat("click under a cookie banner (fails)", [{"op": "goto", "url": f"{site}/cookie-banner"}],
               {"op": "click", "selector": "#buy"}, True),
        repeat("select on 2,000 options", [{"op": "goto", "url": f"{pages}/select.html"}],
               {"op": "select", "selector": "#c", "value": ["Country 1500"]}, False),
        evals(pages, runs, "() => document.title", "warm evals"),
        evals(pages, runs, "document.title = 'x'", "warm evals that write"),
    ]


def evals(pages: str, runs: int, expression: str, what: str) -> tuple:
    """200 warm `eval`s of `expression`, timed round trip by round trip from
    this side of the pipe: an eval is under a millisecond, so one more
    CDP round trip per call doubles it. A read is answered in one message;
    a write runs as an action after the read is refused."""
    ops = [{"op": "goto", "url": f"{pages}/mid-table.html"}]
    timed = set()
    for r in range(runs + 1):  # one warm-up block
        for _ in range(EVAL_BLOCK):
            ops.append({"op": "eval", "expression": expression})
            if r > 0:
                timed.add(len(ops) - 1)
    return f"{EVAL_BLOCK} {what}, answered one by one", ops, timed, EVAL_BLOCK


def run_once(binary: str, ops: list[dict], timed: set[int]) -> list[float]:
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


def run_streamed(binary: str, ops: list[dict], timed: set[int], block: int) -> list[float]:
    """Each op sent once the one before has answered, timed from the write
    to its answer; the timed ops' times summed per `block` of them."""
    p = subprocess.Popen([binary, "serve"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         stderr=subprocess.DEVNULL, text=True, env=dict(os.environ))
    times = []
    try:
        for i, op in enumerate(ops):
            t0 = time.perf_counter()
            p.stdin.write(json.dumps({"id": i, **op}) + "\n")
            p.stdin.flush()
            while True:
                line = p.stdout.readline()
                if not line:
                    raise RuntimeError(f"{binary}: serve ended before answering op {i}")
                try:
                    r = json.loads(line)
                except ValueError:
                    continue
                if r.get("id") == i:
                    break
            if not r.get("ok"):
                raise RuntimeError(f"{binary}: op {i} failed: {str(r.get('error'))[:80]}")
            if i in timed:
                times.append((time.perf_counter() - t0) * 1000.0)
        p.stdin.write(json.dumps({"id": len(ops), "op": "quit"}) + "\n")
        p.stdin.flush()
        p.wait(timeout=60)
    finally:
        if p.poll() is None:
            p.kill()
    return [sum(times[i:i + block]) for i in range(0, len(times), block)]


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
            for name, ops, timed, *block in probes(site_url, pages, args.runs):
                run_probe = partial(run_streamed, block=block[0]) if block else run_once
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
