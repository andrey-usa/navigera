#!/usr/bin/env python3
"""`navigera text` end to end, two builds on the same machine.

    text_ab.py run --a <navigera> --b <navigera> --label-a X --label-b Y \
        --chrome <bin> --lightpanda <bin> --out <dir> [--rounds 3] [--calls 21]
    text_ab.py report --out <dir> --group chrome|lightpanda|checks

`run` serves bench/ladder/fixtures (page_b, page_c) from a stdlib server
(a thread per connection, TCP_NODELAY), then for each round, each engine
and each build in turn (A B, A B, ...): starts one navigera session,
goes to each page once and times `navigera text` --calls times, each a
fresh CLI process as an agent runs it. Every call is a line of
<out>/calls.jsonl; the pages' text from each session is kept for the
output checks (<out>/checks.json), which compare B's text with A's.

`report` prints a group's numbers as ::notice lines (10 at most) and a
table to $GITHUB_STEP_SUMMARY. Call 1 is reported apart; median and p90
are of the calls after it.
"""
import argparse
import hashlib
import http.server
import json
import math
import os
import platform
import shutil
import socket
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "bench" / "ladder" / "fixtures"
PAGES = ("page_b", "page_c")
ENGINES = ("chrome", "lightpanda")

# Hosts of open shadow roots and the elements in the document (page_c: 40, 60,051).
COUNT_JS = (
    "(() => { let roots = 0, els = 0; for (const e of document.getElementsByTagName('*'))"
    " { els++; if (e.shadowRoot) roots++; } return roots + ' ' + els; })()"
)
# What the shadow roots hold, as one line per root (their first child's innerText).
SHADOW_JS = (
    "JSON.stringify([...document.getElementsByTagName('x-card')]"
    ".map(e => e.shadowRoot && e.shadowRoot.firstElementChild ? e.shadowRoot.firstElementChild.innerText : null))"
)


class Handler(http.server.SimpleHTTPRequestHandler):
    # Without it each response waits out the client's delayed ack (~40 ms).
    disable_nagle_algorithm = True
    protocol_version = "HTTP/1.1"

    def __init__(self, *a, **kw):
        super().__init__(*a, directory=str(FIXTURES), **kw)

    def log_message(self, *args):
        pass


class Server(http.server.ThreadingHTTPServer):
    daemon_threads = True

    def server_bind(self):
        # HTTPServer.server_bind looks the host name up (slow on some runners).
        self.socket.bind(self.server_address)
        self.server_address = self.socket.getsockname()  # the port bound when 0 was asked
        self.server_name, self.server_port = self.server_address[:2]


def serve() -> str:
    srv = Server(("127.0.0.1", 0), Handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return f"http://127.0.0.1:{srv.server_port}"


class Session:
    """One navigera session, one CLI process per step."""

    def __init__(self, exe: str, engine: str, args, name: str):
        self.exe = exe
        self.sock = os.path.join(tempfile.gettempdir(), f"nv-text-ab-{name}-{os.getpid()}.sock")
        env = dict(os.environ)
        env.pop("NAVIGERA_OUTPUT", None)  # the default output, as an agent reads it
        env["NAVIGERA_SHARED_BROWSER"] = "0"  # each build its own browser
        if engine == "lightpanda":
            env.pop("DISPLAY", None)
            self.engine_args = ["--engine", "lightpanda", "--chromium", args.lightpanda]
        else:
            self.engine_args = ["--chromium", args.chrome]
        self.env = env
        ok, out, err, ms = self.run(*self.engine_args, "start", "--idle-timeout-s", "3600", timeout=180)
        if not ok:
            raise RuntimeError(f"start failed ({engine}, {exe}): {out[:300]!r} {err[:500]}")
        self.start_ms = ms

    def run(self, *args, timeout=180):
        t0 = time.perf_counter()
        r = subprocess.run([self.exe, "--session", self.sock, *args], env=self.env, capture_output=True,
                           stdin=subprocess.DEVNULL, timeout=timeout)
        ms = (time.perf_counter() - t0) * 1000.0
        return r.returncode == 0, r.stdout, r.stderr.decode(errors="replace"), ms

    def quit(self):
        try:
            self.run("quit", timeout=60)
        except subprocess.TimeoutExpired:
            pass


def sha(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()[:16]


def cmd_run(args) -> int:
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    texts = out / "texts"
    texts.mkdir(exist_ok=True)
    base = serve()
    builds = (("A", args.label_a, args.a), ("B", args.label_b, args.b))
    meta = {
        "cpu": next((ln.split(":", 1)[1].strip() for ln in open("/proc/cpuinfo") if ln.startswith("model name")), "?"),
        "nproc": os.cpu_count(),
        "kernel": platform.release(),
        "builds": {k: {"label": lab, "version": subprocess.run([exe, "--version"], capture_output=True, text=True).stdout.strip()}
                   for k, lab, exe in builds},
        "chrome": subprocess.run([args.chrome, "--version"], capture_output=True, text=True).stdout.strip(),
        "lightpanda": subprocess.run([args.lightpanda, "version"], capture_output=True, text=True).stdout.strip(),
        "lightpanda_release": os.environ.get("LIGHTPANDA_RELEASE", "?"),
        "rounds": args.rounds,
        "calls": args.calls,
        "order": "each round: chrome A, chrome B, lightpanda A, lightpanda B",
    }
    failures = 0
    probes = []
    with open(out / "calls.jsonl", "w") as log:
        for rnd in range(1, args.rounds + 1):
            for engine in ENGINES:
                for key, label, exe in builds:
                    name = f"{engine}-{key}-r{rnd}"
                    try:
                        s = Session(exe, engine, args, name)
                    except Exception as e:  # noqa: BLE001 - recorded, the run goes on
                        print(f"::error title=start {name}::{e}"[:900])
                        failures += 1
                        continue
                    try:
                        for page in PAGES:
                            ok, o, err, goto_ms = s.run("goto", f"{base}/{page}.html")
                            if not ok:
                                print(f"::error title=goto {name} {page}::{err[:500]}")
                                failures += 1
                                continue
                            probe = {"round": rnd, "engine": engine, "build": key, "page": page,
                                     "start_ms": round(s.start_ms, 1), "goto_ms": round(goto_ms, 1)}
                            if page == "page_c":
                                for what, js in (("count", COUNT_JS), ("shadow", SHADOW_JS)):
                                    ok, o, err, _ = s.run("eval", js)
                                    probe[what] = o.decode(errors="replace").strip()[:400] if ok else "error: " + err[:300]
                            hashes = []
                            for call in range(1, args.calls + 1):
                                ok, o, err, ms = s.run("text")
                                h = sha(o)
                                hashes.append(h)
                                rec = {"round": rnd, "engine": engine, "build": key, "label": label, "page": page,
                                       "call": call, "ms": round(ms, 3), "ok": ok, "bytes": len(o), "sha": h}
                                if not ok:
                                    rec["err"] = err[:300]
                                    failures += 1
                                log.write(json.dumps(rec) + "\n")
                                if call == 1:
                                    (texts / f"{name}-{page}.txt").write_bytes(o)
                            probe["distinct_texts"] = len(set(hashes))
                            probes.append(probe)
                            log.flush()
                    finally:
                        s.quit()
                    print(f"{name}: done", flush=True)
    meta["probes"] = probes
    meta["checks"] = output_checks(texts, args.rounds)
    meta["failures"] = failures
    (out / "meta.json").write_text(json.dumps(meta, indent=2))
    print(json.dumps(meta["checks"], indent=1))
    return 1 if failures else 0


def supersequence(a: list, b: list):
    """Greedy: is `a` a subsequence of `b`? The lines of `b` left over are the inserted ones."""
    i, inserted = 0, []
    for line in b:
        if i < len(a) and line == a[i]:
            i += 1
        else:
            inserted.append(line)
    return len(a) - i, inserted


def output_checks(texts: Path, rounds: int) -> dict:
    checks = {}
    for engine in ENGINES:
        for page in PAGES:
            got = {}
            for key in ("A", "B"):
                files = [texts / f"{engine}-{key}-r{r}-{page}.txt" for r in range(1, rounds + 1)]
                blobs = [f.read_bytes() for f in files if f.exists()]
                got[key] = blobs
            c = {"rounds_same_A": len({sha(x) for x in got["A"]}) == 1 if got["A"] else None,
                 "rounds_same_B": len({sha(x) for x in got["B"]}) == 1 if got["B"] else None}
            if got["A"] and got["B"]:
                a = got["A"][0].decode(errors="replace")
                b = got["B"][0].decode(errors="replace")
                al, bl = a.split("\n"), b.split("\n")
                missing, inserted = supersequence(al, bl)
                distinct = {}
                for ln in inserted:
                    distinct[ln[:40]] = distinct.get(ln[:40], 0) + 1
                c.update({"equal": a == b, "chars_A": len(a), "chars_B": len(b), "lines_A": len(al),
                          "lines_B": len(bl), "A_lines_missing_from_B": missing, "inserted_lines": len(inserted),
                          "distinct_inserted": dict(sorted(distinct.items(), key=lambda kv: -kv[1])[:6])})
            checks[f"{engine}/{page}"] = c
    return checks


def pctl(xs, p):
    xs = sorted(xs)
    return xs[max(0, math.ceil(p * len(xs)) - 1)]


def cells(out: Path):
    rows = [json.loads(ln) for ln in open(out / "calls.jsonl")]
    by = {}
    for r in rows:
        by.setdefault((r["engine"], r["page"], r["build"], r["round"]), []).append(r)
    return by


def fmt(x):
    return f"{x:.1f}"


def cmd_report(args) -> int:
    out = Path(args.out)
    meta = json.loads((out / "meta.json").read_text())
    lab = {k: v["label"] for k, v in meta["builds"].items()}
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    md = []
    if args.group == "checks":
        notes = [
            ("machine", f"{meta['cpu']} · {meta['nproc']} cpus · kernel {meta['kernel']}"),
            ("builds", f"A {lab['A']} = {meta['builds']['A']['version']} · B {lab['B']} = {meta['builds']['B']['version']}"),
            ("engines", f"{meta['chrome']} · Lightpanda {meta['lightpanda_release']} ({meta['lightpanda']})"),
        ]
        for eng in ENGINES:
            pc = [p for p in meta["probes"] if p["engine"] == eng and p["page"] == "page_c"]
            counts = sorted({p.get("count", "?") for p in pc})
            shadow = sorted({p.get("shadow", "?")[:60] for p in pc})
            distinct = max((p["distinct_texts"] for p in meta["probes"] if p["engine"] == eng), default=0)
            notes.append((f"page_c {eng}", f"shadow roots + elements: {', '.join(counts)} · shadow text: {' | '.join(shadow)}"
                          f" · most distinct texts in one session's {meta['calls']} calls: {distinct}"))
        for key, c in meta["checks"].items():
            if "equal" not in c:
                notes.append((f"check {key}", f"missing text: {c}"))
                continue
            notes.append((f"check {key}", (
                f"B==A {c['equal']} · chars {c['chars_A']} -> {c['chars_B']} · lines {c['lines_A']} -> {c['lines_B']}"
                f" · A lines not in B {c['A_lines_missing_from_B']} · inserted {c['inserted_lines']}"
                f" {json.dumps(c['distinct_inserted'])} · same over rounds A {c['rounds_same_A']} B {c['rounds_same_B']}")))
        notes.append(("failures", str(meta["failures"])))
        for title, text in notes[:10]:
            print(f"::notice title={title}::{text}")
        md.append("### Setup and output checks\n")
        md += [f"- **{t}**: {x}" for t, x in notes]
    else:
        eng = args.group
        by = cells(out)
        md.append(f"### navigera text on {eng}: wall ms per CLI call (call 1 apart; median, p90 of calls 2..{meta['calls']})\n")
        md.append("| page | build | round | call 1 | median | p90 | min | max |")
        md.append("|---|---|---|---|---|---|---|---|")
        lines = []
        pooled = {}
        for page in PAGES:
            for key in ("A", "B"):
                parts, rest_all = [], []
                for rnd in range(1, meta["rounds"] + 1):
                    rs = sorted(by.get((eng, page, key, rnd), []), key=lambda r: r["call"])
                    if not rs:
                        parts.append(f"r{rnd} -")
                        continue
                    first = rs[0]["ms"]
                    rest = [r["ms"] for r in rs[1:]]
                    rest_all += rest
                    med, p90 = statistics.median(rest), pctl(rest, 0.9)
                    parts.append(f"r{rnd} {fmt(first)}/{fmt(med)}/{fmt(p90)}")
                    md.append(f"| {page} | {key} {lab[key]} | {rnd} | {fmt(first)} | {fmt(med)} | {fmt(p90)} | "
                              f"{fmt(min(rest))} | {fmt(max(rest))} |")
                if rest_all:
                    pooled[(page, key)] = statistics.median(rest_all)
                    parts.append(f"all {fmt(statistics.median(rest_all))}/{fmt(pctl(rest_all, 0.9))}")
                lines.append((f"{eng} {page} {key} {lab[key]}", "first/median/p90 ms: " + " · ".join(parts)))
        for page in PAGES:
            if (page, "A") in pooled and (page, "B") in pooled:
                a, b = pooled[(page, "A")], pooled[(page, "B")]
                lines.append((f"{eng} {page} B/A", f"median of all rounds {fmt(a)} -> {fmt(b)} ms ({(b / a - 1) * 100:+.1f}%)"))
        for title, text in lines[:10]:
            print(f"::notice title={title}::{text}")
        md.append("")
        md += [f"- **{t}**: {x}" for t, x in lines]
    if summary:
        with open(summary, "a") as f:
            f.write("\n".join(md) + "\n\n")
    else:
        print("\n".join(md))
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run")
    r.add_argument("--a", required=True)
    r.add_argument("--b", required=True)
    r.add_argument("--label-a", default="A")
    r.add_argument("--label-b", default="B")
    r.add_argument("--chrome", default=os.environ.get("CHROME_BIN") or shutil.which("google-chrome") or "")
    r.add_argument("--lightpanda", default=os.environ.get("LIGHTPANDA_BIN", ""))
    r.add_argument("--out", required=True)
    r.add_argument("--rounds", type=int, default=3)
    r.add_argument("--calls", type=int, default=21)
    p = sub.add_parser("report")
    p.add_argument("--out", required=True)
    p.add_argument("--group", required=True, choices=["chrome", "lightpanda", "checks"])
    args = ap.parse_args()
    return cmd_run(args) if args.cmd == "run" else cmd_report(args)


if __name__ == "__main__":
    sys.exit(main())
