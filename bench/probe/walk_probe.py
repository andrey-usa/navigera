#!/usr/bin/env python3
"""Ways to find a page's open shadow roots, timed in the page, per engine.

    walk_probe.py --navigera <bin> --chrome <bin> --lightpanda <bin> [--reps 15]

Serves bench/ladder/fixtures, starts one navigera session per engine, goes
to page_c (60,051 elements, 40 open roots) and page_b (2,000, none), then
runs every variant once per round (order rotated each round), `reps`
rounds. Each variant returns [ms by performance.now(), hosts found]; a
variant that finds a different host count than the TreeWalker is flagged.
Prints ::notice lines (median and min per variant) and writes JSON lines
to $RUNNER_TEMP/walk.jsonl.
"""
import argparse, http.server, json, os, statistics, subprocess, tempfile, threading
from pathlib import Path

FIXTURES = Path(__file__).resolve().parents[2] / "bench" / "ladder" / "fixtures"

WRAP = "(() => {{ const t0 = performance.now(); let h = 0; {body} return JSON.stringify([performance.now() - t0, h]); }})()"
VARIANTS = {
    # what FLAT_TEXT does (one scope: page_c's roots hold no hosts)
    "treewalker": "const w = document.createTreeWalker(document.body, 1); for (let n = w.nextNode(); n; n = w.nextNode()) if (n.shadowRoot) h++;",
    "treewalker-no-check": "const w = document.createTreeWalker(document.body, 1); for (let n = w.nextNode(); n; n = w.nextNode()) h++; h = -1;",
    "gebtn-index": "const a = document.body.getElementsByTagName('*'); for (let i = 0, n = a.length; i < n; i++) if (a[i].shadowRoot) h++;",
    "gebtn-forof": "for (const e of document.body.getElementsByTagName('*')) if (e.shadowRoot) h++;",
    "qsa-forof": "for (const e of document.body.querySelectorAll('*')) if (e.shadowRoot) h++;",
    "qsa-index": "const a = document.body.querySelectorAll('*'); for (let i = 0, n = a.length; i < n; i++) if (a[i].shadowRoot) h++;",
    "sibling-loop": "let n = document.body.firstElementChild; const st = []; while (n) { if (n.shadowRoot) h++; const c = n.firstElementChild; if (c) { const s = n.nextElementSibling; if (s) st.push(s); n = c; } else n = n.nextElementSibling || st.pop() || null; }",
    "children-stack": "const st = [document.body]; while (st.length) { const c = st.pop().children; for (let i = 0; i < c.length; i++) { const e = c[i]; if (e.shadowRoot) h++; if (e.firstElementChild) st.push(e); } }",
    # candidates by tag: custom elements plus the 18 tags that may host a root
    "qsa-may-host": "const q = 'article,aside,blockquote,div,footer,h1,h2,h3,h4,h5,h6,header,main,nav,p,section,span'; for (const e of document.body.querySelectorAll(q)) if (e.shadowRoot) h++; for (const e of document.body.getElementsByTagName('*')) if (e.localName.includes('-') && e.shadowRoot) h++;",
}


class Handler(http.server.SimpleHTTPRequestHandler):
    disable_nagle_algorithm = True
    protocol_version = "HTTP/1.1"

    def __init__(self, *a, **kw):
        super().__init__(*a, directory=str(FIXTURES), **kw)

    def log_message(self, *args):
        pass


class Server(http.server.ThreadingHTTPServer):
    daemon_threads = True

    def server_bind(self):
        self.socket.bind(self.server_address)
        self.server_address = self.socket.getsockname()
        self.server_name, self.server_port = self.server_address[:2]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--navigera", required=True)
    ap.add_argument("--chrome", required=True)
    ap.add_argument("--lightpanda", required=True)
    ap.add_argument("--reps", type=int, default=15)
    args = ap.parse_args()
    srv = Server(("127.0.0.1", 0), Handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{srv.server_port}"
    out = open(os.path.join(os.environ.get("RUNNER_TEMP", tempfile.gettempdir()), "walk.jsonl"), "w")
    notices = []
    for engine in ("chrome", "lightpanda"):
        sock = os.path.join(tempfile.gettempdir(), f"nv-walk-{engine}-{os.getpid()}.sock")
        env = dict(os.environ, NAVIGERA_SHARED_BROWSER="0")
        eng = ["--engine", "lightpanda", "--chromium", args.lightpanda] if engine == "lightpanda" else ["--chromium", args.chrome]
        run = lambda *a: subprocess.run([args.navigera, "--session", sock, *a], env=env, capture_output=True, text=True, timeout=180)
        r = run(*eng, "start", "--idle-timeout-s", "900")
        if r.returncode:
            print(f"::error title=start {engine}::{r.stderr[:500]}")
            return 1
        try:
            for page in ("page_c", "page_b"):
                assert run("goto", f"{base}/{page}.html").returncode == 0
                names = list(VARIANTS)
                got = {k: [] for k in names}
                hosts = {}
                for rep in range(args.reps):
                    for k in names[rep % len(names):] + names[:rep % len(names)]:
                        r = run("eval", WRAP.format(body=VARIANTS[k]))
                        if r.returncode:
                            print(f"::warning title={engine} {page} {k}::{r.stderr[:300]}")
                            continue
                        v = r.stdout.strip()
                        try:
                            v = json.loads(v)
                            if isinstance(v, str):
                                v = json.loads(v)
                        except ValueError:
                            print(f"::warning title={engine} {page} {k}::unparsed {v[:200]}")
                            continue
                        got[k].append(v[0])
                        hosts.setdefault(k, set()).add(v[1])
                        out.write(json.dumps({"engine": engine, "page": page, "variant": k, "rep": rep, "ms": v[0], "hosts": v[1]}) + "\n")
                ref = hosts.get("treewalker")
                parts = []
                for k in names:
                    if not got[k]:
                        parts.append(f"{k} -")
                        continue
                    flag = "" if hosts[k] == ref or k == "treewalker-no-check" else f" HOSTS {sorted(hosts[k])}"
                    parts.append(f"{k} {statistics.median(got[k]):.2f}/{min(got[k]):.2f}{flag}")
                notices.append((f"{engine} {page} (median/min ms, n={args.reps}, hosts {sorted(ref or [])})", " · ".join(parts)))
        finally:
            run("quit")
    for title, msg in notices:
        print(f"::notice title={title}::{msg}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
