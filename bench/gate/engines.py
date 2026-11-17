#!/usr/bin/env python3
"""Engine timings: a release candidate against the release before it, on
the engines the timing gate (gate.py, Chrome only) leaves out, and on the
text search.

Runs the same ops with both binaries on this machine, one `serve` process
per sample set, interleaved (base, candidate, base, candidate), and prints
the median of each probe's `elapsed_ms`: the op's whole time as navigera
answers it, page round trips included. It reports, it does not gate: the
WebDriver engines are experimental and their drivers vary by machine.

    python3 bench/gate/engines.py --base <old navigera> --candidate <new navigera> \
        [--runs 10] [--summary out.md] [--only <text>]

Engines, each only when found: Firefox ($NAVIGERA_FIREFOX, over WebDriver
BiDi), chromedriver ($CHROMEDRIVER, its Chrome headless; $CHROME_BIN when
set) over classic WebDriver and over BiDi, and Chrome itself for the text
search. The pages: the per-op bench's page_a (bench/ladder/gen_fixtures.py:
500 cards, about 3,000 nodes in `ax`), a page whose link opens a tab, and a
2.8 MB text of 20,000 lines (plain, and with accented letters on every
line). Exit 0: every probe ran; 2: one could not run (the table says why).
"""
import argparse
import json
import os
import socket
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "ladder"))
from gate import serve_dir  # noqa: E402
from gen_fixtures import gen_page_a  # noqa: E402

GREP_LINES = 20_000


def write_pages(dir_: Path) -> None:
    (dir_ / "page_a.html").write_text(gen_page_a(), encoding="utf-8")
    (dir_ / "tabs.html").write_text(
        "<!doctype html><title>tabs</title><a id=open href=other.html target=_blank>open a tab</a> "
        "<button id=count onclick=\"this.dataset.n = (+this.dataset.n || 0) + 1\">count</button>"
    )
    (dir_ / "other.html").write_text("<!doctype html><title>other</title><p>another tab")
    for name, words in (("text", "the quick brown fox jumps over the lazy dog"),
                        ("text-accented", "le café crème brûlée à côté de la forêt")):
        lines = []
        for i in range(GREP_LINES):
            # Every 2,000th line has the word searched for: ten passages.
            mark = " zebra" if i % 2000 == 7 else ""
            lines.append(f"<div>Line {i:05}: {words}, {words}, and so on to the end{mark}.</div>")
        (dir_ / f"{name}.html").write_text(
            f"<!doctype html><meta charset=utf-8><title>{name}</title>" + "".join(lines), encoding="utf-8")


def chromedriver() -> tuple[subprocess.Popen, str, str] | None:
    exe = os.environ.get("CHROMEDRIVER")
    if not exe or not Path(exe).exists():
        return None
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        port = s.getsockname()[1]
    proc = subprocess.Popen([exe, f"--port={port}", "--disable-build-check"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    started = time.monotonic()
    while True:
        try:
            socket.create_connection(("127.0.0.1", port), timeout=1).close()
            break
        except OSError:
            if time.monotonic() - started > 20:
                proc.kill()
                raise RuntimeError("chromedriver did not listen")
            time.sleep(0.05)
    opts = {"args": ["--headless=new", "--no-sandbox", "--disable-gpu"]}
    if os.environ.get("CHROME_BIN"):
        opts["binary"] = os.environ["CHROME_BIN"]
    caps = json.dumps({"browserName": "chrome", "goog:chromeOptions": opts})
    return proc, f"http://127.0.0.1:{port}", caps


def samples(binary: str, args: list[str], setup: list[dict], measured: dict, runs: int) -> list[float]:
    """One `serve`: the setup, one warm-up of the measured op, then `runs`
    of it; each measured op's `elapsed_ms`."""
    ops = [*setup] + [dict(measured) for _ in range(runs + 1)]
    lines = [json.dumps({"id": i, **op}) for i, op in enumerate(ops)]
    lines.append(json.dumps({"id": len(ops), "op": "quit"}))
    p = subprocess.run([binary, *args, "serve"], input="\n".join(lines) + "\n",
                       capture_output=True, text=True, timeout=900)
    answers = {}
    for line in p.stdout.splitlines():
        try:
            r = json.loads(line)
        except ValueError:
            continue
        if isinstance(r, dict) and "id" in r:
            answers[r["id"]] = r
    first = len(setup) + 1
    out = []
    for i in range(len(ops)):
        r = answers.get(i)
        if r is None or not r.get("ok"):
            why = (r or {}).get("error") or (p.stderr.strip().splitlines() or ["no answer"])[-1]
            raise RuntimeError(f"op {i} ({ops[i]['op']}): {str(why)[:100]}")
        if i >= first:
            out.append(float(r["elapsed_ms"]))
    return out


def probes(pages: str) -> dict[str, list[tuple[str, list[dict], dict]]]:
    """(name, setup, measured op) per engine kind."""
    page_a = [{"op": "goto", "url": f"{pages}/page_a.html"}]
    tab = [{"op": "goto", "url": f"{pages}/tabs.html"}, {"op": "click", "selector": "#open"}]
    ax = ("ax on page_a", page_a, {"op": "ax", "format": "text"})
    click = ("click that stays on the page", page_a, {"op": "click", "selector": "#search"})
    after_tab = ("click after the page opened a tab", tab, {"op": "click", "selector": "#count"})
    grep = [(f"text --grep, 2.8 MB of {GREP_LINES:,} lines{what}",
             [{"op": "goto", "url": f"{pages}/{page}.html"}], {"op": "text", "grep": "zebra"})
            for page, what in (("text", ""), ("text-accented", ", accented"))]
    return {"firefox": [ax, click, after_tab], "webdriver": [ax, click, after_tab], "chrome": grep}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--base", required=True)
    ap.add_argument("--candidate", required=True)
    ap.add_argument("--runs", type=int, default=10)
    ap.add_argument("--summary")
    ap.add_argument("--only", default="",
                    help="run only the probes whose 'engine: probe' contains this text")
    args = ap.parse_args()

    rows, notes, broken = [], [], []
    driver = None
    with tempfile.TemporaryDirectory() as tmp:
        write_pages(Path(tmp))
        pages = serve_dir(Path(tmp))
        all_probes = probes(pages)
        engines = []
        if os.environ.get("NAVIGERA_FIREFOX"):
            engines.append(("Firefox (BiDi)", ["--engine", "firefox"], "firefox"))
        else:
            notes.append("Firefox: skipped, NAVIGERA_FIREFOX is not set")
        try:
            driver = chromedriver()
        except RuntimeError as e:
            notes.append(f"chromedriver: {e}")
        if driver:
            _, url, caps = driver
            engines.append(("chromedriver, classic", ["--webdriver", url, "--capabilities", caps], "webdriver"))
            engines.append(("chromedriver, BiDi", ["--webdriver", url, "--bidi", "--capabilities", caps], "webdriver"))
        elif not notes or not notes[-1].startswith("chromedriver"):
            notes.append("chromedriver: skipped, CHROMEDRIVER is not set")
        engines.append(("Chrome", [], "chrome"))
        try:
            for engine, eargs, kind in engines:
                for name, setup, op in all_probes[kind]:
                    if args.only not in f"{engine}: {name}":
                        continue
                    try:
                        # Interleaved: base, candidate, base, candidate (drift hits both).
                        b = samples(args.base, eargs, setup, op, args.runs)
                        c = samples(args.candidate, eargs, setup, op, args.runs)
                        b += samples(args.base, eargs, setup, op, args.runs)
                        c += samples(args.candidate, eargs, setup, op, args.runs)
                    except (RuntimeError, subprocess.TimeoutExpired) as e:
                        broken.append(f"{engine}: {name}")
                        rows.append(f"| {engine} | {name} | - | - | - | could not run: {str(e)[:100]} |")
                        continue
                    bm, cm = statistics.median(b), statistics.median(c)
                    spread = f"{min(c):.1f}-{max(c):.1f}"
                    rows.append(f"| {engine} | {name} | {bm:.1f} | {cm:.1f} | {cm / bm if bm else 1:.2f}x | {spread} |")
                    print(rows[-1], flush=True)
        finally:
            if driver:
                driver[0].kill()

    versions = []
    for label, binary in (("base", args.base), ("candidate", args.candidate)):
        v = subprocess.run([binary, "--version"], capture_output=True, text=True).stdout.strip()
        versions.append(f"{label} {v}")
    table = "\n".join([
        f"Engine timings: {', '.join(versions)}; median `elapsed_ms` of {2 * args.runs} per probe "
        "(two interleaved sets each), candidate's range last",
        "",
        "| engine | probe | base ms | candidate ms | ratio | candidate range |",
        "|---|---|---:|---:|---:|---|",
        *rows,
        "",
        *(f"- {n}" for n in notes),
        ("could not run: " + "; ".join(broken)) if broken else "ok",
    ])
    print(table)
    if args.summary:
        Path(args.summary).write_text(table + "\n")
    return 2 if broken else 0


if __name__ == "__main__":
    sys.exit(main())
