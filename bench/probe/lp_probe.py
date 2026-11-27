#!/usr/bin/env python3
"""Lightpanda's own CDP commands (the LP domain) next to navigera's ops on
the same pages, on the same Lightpanda release.

Three arms per measured pair, interleaved (the arm order rotates each rep):

  nv    the navigera op through its CLI on `--engine lightpanda`
        (`ax`, `text`, `click <ref>`, `fill <ref> <v>`, `ax --selector form`)
  nvlp  the LP command through the same session (`navigera cdp LP.x {...}`):
        the same process start and session hop as `nv`, so the difference
        is what the op itself costs inside navigera
  raw   the LP command over a WebSocket of our own to a `lightpanda serve`
        of our own (same flags navigera starts it with): the command alone

For each pair: correctness against navigera's result on the same page
(same elements, same text, same fields; every difference listed), the
median of N wall times per arm, the bytes of what an agent reads, and the
errors. For the page-status cases, what navigera says next to what
LP.getPageStatus says. navigera's CDP round trips per call are counted
from its NAVIGERA_CDP_TRACE file.

Usage: lp_probe.py --navigera BIN --lightpanda BIN --out DIR [--reps 20]
Writes DIR/results.json and DIR/summary.md (also to $GITHUB_STEP_SUMMARY).
Standard library only.
"""
import argparse
import base64
import collections
import http.server
import json
import os
import re
import shutil
import socket
import socketserver
import statistics
import struct
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "bench" / "ladder" / "fixtures"
# The flags navigera starts `lightpanda serve` with (src/cdp/browser.rs).
SERVE_FLAGS = ["--load-resources", "iframe", "--load-resources", "stylesheet"]


# ---------------------------------------------------------------- WebSocket


class WS:
    """A minimal RFC 6455 client: text frames out (masked), frames in."""

    def __init__(self, url: str, timeout: float = 60.0):
        u = urllib.parse.urlparse(url)
        self.sock = socket.create_connection((u.hostname, u.port), timeout=timeout)
        self.sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        key = base64.b64encode(os.urandom(16)).decode()
        req = (
            f"GET {u.path or '/'} HTTP/1.1\r\nHost: {u.hostname}:{u.port}\r\n"
            f"Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\n"
            "Sec-WebSocket-Version: 13\r\n\r\n"
        )
        self.sock.sendall(req.encode())
        self.buf = bytearray()
        while b"\r\n\r\n" not in self.buf:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise EOFError("websocket handshake: connection closed")
            self.buf += chunk
        head, _, rest = bytes(self.buf).partition(b"\r\n\r\n")
        if b" 101 " not in head.split(b"\r\n", 1)[0] + b" ":
            raise RuntimeError(f"websocket handshake refused: {head[:200]!r}")
        self.buf = bytearray(rest)

    def send(self, text: str) -> None:
        data = text.encode()
        n = len(data)
        hdr = bytearray([0x81])
        if n < 126:
            hdr.append(0x80 | n)
        elif n < 65536:
            hdr.append(0x80 | 126)
            hdr += struct.pack(">H", n)
        else:
            hdr.append(0x80 | 127)
            hdr += struct.pack(">Q", n)
        mask = os.urandom(4)
        hdr += mask
        full = (mask * (n // 4 + 1))[:n]
        masked = (int.from_bytes(data, "big") ^ int.from_bytes(full, "big")).to_bytes(n, "big") if n else b""
        self.sock.sendall(bytes(hdr) + masked)

    def _read(self, n: int) -> bytes:
        while len(self.buf) < n:
            chunk = self.sock.recv(1 << 20)
            if not chunk:
                raise EOFError("websocket closed")
            self.buf += chunk
        out = bytes(self.buf[:n])
        del self.buf[:n]
        return out

    def recv(self) -> bytes:
        parts = []
        self.frames = []
        while True:
            b0, b1 = self._read(2)
            op, n = b0 & 0x0F, b1 & 0x7F
            if n == 126:
                n = struct.unpack(">H", self._read(2))[0]
            elif n == 127:
                n = struct.unpack(">Q", self._read(8))[0]
            mask = self._read(4) if b1 & 0x80 else None
            payload = self._read(n)
            self.frames.append(n)
            if mask:
                payload = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
            if op == 8:
                raise EOFError("websocket closed by the server")
            if op == 9:
                continue
            parts.append(payload)
            if b0 & 0x80:
                return b"".join(parts)

    def close(self) -> None:
        try:
            self.sock.close()
        except OSError:
            pass


class CDPError(Exception):
    pass


class CDP:
    """Raw CDP over one WebSocket, flattened sessions."""

    def __init__(self, ws_url: str):
        self.ws = WS(ws_url)
        self.next_id = 0
        self.events = []

    def call(self, method, params=None, session=None):
        """(result, wall ms up to the reply's bytes, reply bytes)."""
        self.next_id += 1
        mid = self.next_id
        msg = {"id": mid, "method": method, "params": params or {}}
        if session:
            msg["sessionId"] = session
        t0 = time.perf_counter()
        self.ws.send(json.dumps(msg))
        while True:
            raw = self.ws.recv()
            t1 = time.perf_counter()
            m = json.loads(raw)
            if m.get("id") == mid:
                if "error" in m:
                    raise CDPError(f"{m['error'].get('code')} {m['error'].get('message')}")
                return m.get("result", {}), (t1 - t0) * 1000, len(raw)
            self.events.append(m)

    def wait_event(self, name, session=None, timeout=30.0):
        end = time.time() + timeout
        while True:
            for i, e in enumerate(self.events):
                if e.get("method") == name and (session is None or e.get("sessionId") == session):
                    del self.events[: i + 1]
                    return e
            self.events.clear()
            if time.time() > end:
                raise TimeoutError(name)
            self.ws.sock.settimeout(max(0.05, end - time.time()))
            try:
                self.events.append(json.loads(self.ws.recv()))
            except socket.timeout:
                pass
            finally:
                self.ws.sock.settimeout(60)


class RawPanda:
    """Our own `lightpanda serve` and one page on it."""

    def __init__(self, bin_path: str, port: int):
        self.proc = subprocess.Popen(
            [bin_path, "serve", "--host", "127.0.0.1", "--port", str(port), *SERVE_FLAGS],
            stdout=subprocess.DEVNULL,
            stderr=open(os.environ.get("LP_RAW_LOG", os.devnull), "ab"),
        )
        self.port = port
        self.bin_path = bin_path
        self.connect()

    def connect(self):
        end = time.time() + 20
        ws_url = None
        while time.time() < end:
            try:
                with urllib.request.urlopen(f"http://127.0.0.1:{self.port}/json/version", timeout=2) as r:
                    ws_url = json.load(r)["webSocketDebuggerUrl"]
                break
            except Exception:
                time.sleep(0.1)
        if not ws_url:
            raise RuntimeError("lightpanda serve did not answer /json/version")
        self.cdp = CDP(ws_url)
        ctx, _, _ = self.cdp.call("Target.createBrowserContext")
        tgt, _, _ = self.cdp.call(
            "Target.createTarget", {"url": "about:blank", "browserContextId": ctx["browserContextId"]}
        )
        att, _, _ = self.cdp.call("Target.attachToTarget", {"targetId": tgt["targetId"], "flatten": True})
        self.session = att["sessionId"]
        for m in ("Page.enable", "Runtime.enable"):
            self.call(m)

    def alive(self) -> bool:
        return self.proc.poll() is None

    def restart(self):
        self.close()
        self.__init__(self.bin_path, self.port)

    def call(self, method, params=None):
        return self.cdp.call(method, params, self.session)

    def goto(self, url):
        self.cdp.events.clear()
        res, _, _ = self.call("Page.navigate", {"url": url})
        if res.get("errorText"):
            return res
        try:
            self.cdp.wait_event("Page.loadEventFired", self.session, timeout=20)
        except TimeoutError:
            pass
        return res

    def eval(self, expr):
        res, _, _ = self.call("Runtime.evaluate", {"expression": expr, "returnByValue": True, "awaitPromise": True})
        return res.get("result", {}).get("value")

    def close(self):
        try:
            self.cdp.ws.close()
        except Exception:
            pass
        self.proc.kill()
        self.proc.wait()


# ---------------------------------------------------------------- navigera


class Nav:
    """One navigera session on Lightpanda, driven one CLI call per step."""

    def __init__(self, nv: str, lp: str, out: Path):
        self.nv = nv
        self.trace = out / "nv-cdp-trace.jsonl"
        # A socket in the artifact directory would break its upload.
        self.session = os.path.join(tempfile.gettempdir(), f"nv-lp-probe-{os.getpid()}.sock")
        self.out = out
        env = dict(os.environ)
        env.pop("DISPLAY", None)
        env["NAVIGERA_CDP_TRACE"] = str(self.trace)
        self.env = env
        self.lp = lp
        self.start()

    def start(self):
        r = subprocess.run(
            [self.nv, "--session", self.session, "--engine", "lightpanda", "--chromium", self.lp,
             "start", "--idle-timeout-s", "3600"],
            env=self.env, capture_output=True, text=True, stdin=subprocess.DEVNULL,
        )
        if r.returncode != 0:
            raise RuntimeError(f"navigera start failed: {r.stdout} {r.stderr}")

    def trace_lines(self) -> int:
        try:
            with open(self.trace, "rb") as f:
                return sum(1 for _ in f)
        except FileNotFoundError:
            return 0

    def run(self, *args, timeout=120):
        """(ok, stdout bytes, stderr text, wall ms, CDP commands sent)."""
        before = self.trace_lines()
        t0 = time.perf_counter()
        r = subprocess.run(
            [self.nv, "--session", self.session, *args],
            env=self.env, capture_output=True, stdin=subprocess.DEVNULL, timeout=timeout,
        )
        ms = (time.perf_counter() - t0) * 1000
        return r.returncode == 0, r.stdout, r.stderr.decode(errors="replace"), ms, self.trace_lines() - before

    def cdp(self, method, params=None):
        args = ["cdp", method] + ([json.dumps(params)] if params else [])
        ok, out, err, ms, calls = self.run(*args)
        result = None
        if ok:
            try:
                result = json.loads(out).get("result")
            except ValueError:
                result = None
        return ok, out, err, ms, calls, result

    def eval(self, expr):
        ok, out, err, _, _ = self.run("eval", expr)
        if not ok:
            return {"error": err.strip()[:300]}
        text = out.decode(errors="replace").strip()
        try:
            return json.loads(text)
        except ValueError:
            return text

    def alive(self) -> bool:
        ok, _, _, _, _ = self.run("eval", "1")
        return ok

    @staticmethod
    def gone(err: str) -> bool:
        return "browser is gone" in err or "connection to the browser closed" in err

    def restart(self):
        self.run("quit")
        self.start()

    def quit(self):
        self.run("quit")
        log = Path(self.session).with_suffix(".log")
        if log.exists():
            shutil.copy(log, self.out / "nv-session.log")


# ---------------------------------------------------------------- fixtures

ADD_TO_CART = ("fetch('/api/cart', {method: 'POST', headers: {'Content-Type': 'application/json'}, "
               "body: JSON.stringify({id: 101})}).then(r => r.status)")


CASE_PAGES = {
    "/x/404": (404, {}, "<!doctype html><title>Not found</title><h1>Page not found</h1>"),
    "/x/500": (500, {}, "<!doctype html><title>Server error</title><h1>Something broke</h1>"),
    "/x/cf": (
        403,
        {"Server": "cloudflare", "cf-mitigated": "challenge"},
        "<!doctype html><title>Just a moment...</title><p>Checking your browser before accessing the site.</p>",
    ),
    "/x/empty": (200, {}, ""),
    "/x/jserr": (
        200,
        {},
        "<!doctype html><title>Broken script</title><h1>Welcome</h1><div id=app></div>"
        "<script>renderApp(document.getElementById('app'));</script>",
    ),
    "/x/xhr": (
        200,
        {},
        "<!doctype html><title>Feed</title><h1>Feed</h1><ul id=feed></ul><script>"
        "fetch('/x/api429').then(r => r.text()).catch(() => 0);"
        "fetch('/x/api403').then(r => r.text()).catch(() => 0);"
        "const x = new XMLHttpRequest(); x.open('GET', '/x/api500'); x.send();"
        "</script>",
    ),
    "/x/api429": (429, {"Content-Type": "application/json"}, '{"error":"rate limited"}'),
    "/x/api403": (403, {"Content-Type": "application/json"}, '{"error":"forbidden"}'),
    "/x/api500": (500, {"Content-Type": "application/json"}, '{"error":"oops"}'),
}


class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *a, **kw):
        super().__init__(*a, directory=str(FIXTURES), **kw)

    def log_message(self, *args):
        pass

    def do_GET(self):
        path = urllib.parse.urlparse(self.path).path
        if path in CASE_PAGES:
            status, headers, body = CASE_PAGES[path]
            data = body.encode()
            self.send_response(status)
            headers = {"Content-Type": "text/html; charset=utf-8", **headers}
            for k, v in headers.items():
                self.send_header(k, v)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return
        return super().do_GET()


class ThreadedServer(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True


def start_site(script: str, args):
    p = subprocess.Popen([sys.executable, str(ROOT / script), *args], stdout=subprocess.PIPE, text=True)
    line = p.stdout.readline()
    return p, line.split()[1]


def free_port() -> int:
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


# ---------------------------------------------------------------- parsing


AX_REF = re.compile(r"\[(?:[^\]]*?, )?ref=(\d+)\]")
FIELD_ROLES = {"textbox", "searchbox", "combobox", "listbox", "checkbox", "radio", "spinbutton", "slider",
               "switch", "file", "textarea"}


def ax_nodes(text: str):
    """navigera's tree: {ref: (role, name)} for every line with a ref, and
    the headings."""
    refs, heads = {}, []
    for line in text.splitlines():
        s = line.strip()
        if not s.startswith("- "):
            continue
        s = s[2:]
        if s.startswith("cell: "):
            s = s[6:]
        role = s.split(" ", 1)[0].rstrip(":")
        m = re.match(r'[^"]*?"((?:[^"\\]|\\.)*)"', s)
        name = json.loads(f'"{m.group(1)}"') if m else ""
        r = AX_REF.search(s)
        if r:
            refs[int(r.group(1))] = (role.lower(), name)
        if role == "heading":
            heads.append(name)
    return refs, heads


LP_LINE = re.compile(r"^\s*(\d+) (\[i\] )?(\S+)?(?: '(.*?)')?(?: value='.*?')?(?: \[\w+\])*\s*$")


def lp_nodes(text: str):
    """LP's text tree: {id: (role, name)} for the [i] lines, and headings."""
    inter, heads = {}, []
    for line in text.splitlines():
        m = re.match(r"^\s*(\d+) (\[i\] )?(.*)$", line)
        if not m:
            continue
        nid, interactive, rest = int(m.group(1)), bool(m.group(2)), m.group(3)
        if rest.startswith("'"):
            role, tail = "", rest
        else:
            role, _, tail = rest.partition(" ")
        nm = re.match(r"'(.*?)'(?: value=|\s*\[|\s*$)", tail)
        name = nm.group(1) if nm else ""
        if interactive:
            inter[nid] = (role.lower(), name)
        if role == "heading":
            heads.append(name)
    return inter, heads


def words(s: str):
    return collections.Counter(re.findall(r"\w+", s.lower()))


def diff_examples(a: collections.Counter, b: collections.Counter, k=8):
    only_a = a - b
    return sum(only_a.values()), [w for w, _ in only_a.most_common(k)]


def compare_trees(ax_text: str, lp_text: str):
    ax_refs, ax_heads = ax_nodes(ax_text)
    lp_int, lp_heads = lp_nodes(lp_text)
    only_ax = sorted(set(ax_refs) - set(lp_int))
    only_lp = sorted(set(lp_int) - set(ax_refs))
    mism = []
    for i in sorted(set(ax_refs) & set(lp_int)):
        (ar, an), (lr, ln) = ax_refs[i], lp_int[i]
        if ar != lr or an.strip() != ln.strip():
            mism.append(f"{i}: nv {ar} {an!r} / lp {lr} {ln!r}")
    hc_ax, hc_lp = collections.Counter(ax_heads), collections.Counter(lp_heads)
    return {
        "nv_refs": len(ax_refs),
        "lp_interactive": len(lp_int),
        "common": len(set(ax_refs) & set(lp_int)),
        "only_nv": len(only_ax),
        "only_nv_examples": [f"{i} {ax_refs[i][0]} {ax_refs[i][1]!r}" for i in only_ax[:8]],
        "only_lp": len(only_lp),
        "only_lp_examples": [f"{i} {lp_int[i][0]} {lp_int[i][1]!r}" for i in only_lp[:8]],
        "role_or_name_differs": len(mism),
        "differs_examples": mism[:8],
        "headings_nv": len(ax_heads),
        "headings_lp": len(lp_heads),
        "headings_only_nv": list((hc_ax - hc_lp).elements())[:5],
        "headings_only_lp": list((hc_lp - hc_ax).elements())[:5],
        "same": not only_ax and not only_lp and not mism and hc_ax == hc_lp,
    }


def compare_text(nv_text: str, md: str):
    # Link and image targets are URLs the page text doesn't show.
    md = re.sub(r"\]\([^)]*\)", "]", md)
    a, b = words(nv_text), words(md)
    n_a, ex_a = diff_examples(a, b)
    n_b, ex_b = diff_examples(b, a)
    return {
        "nv_words": sum(a.values()),
        "lp_words": sum(b.values()),
        "words_only_nv": n_a,
        "only_nv_examples": ex_a,
        "words_only_lp": n_b,
        "only_lp_examples": ex_b,
        "same_words": n_a == 0 and n_b == 0,
    }


def compare_forms(ax_text: str, forms):
    ax_refs, _ = ax_nodes(ax_text)
    nv_fields = {i: rn for i, rn in ax_refs.items() if rn[0] in FIELD_ROLES}
    lp_fields = {}
    for f in forms or []:
        for fld in f.get("fields", []):
            lp_fields[fld.get("backendNodeId")] = fld
    only_nv = sorted(set(nv_fields) - set(lp_fields))
    only_lp = sorted(set(lp_fields) - set(nv_fields))
    return {
        "lp_forms": len(forms or []),
        "nv_fields": len(nv_fields),
        "lp_fields": len(lp_fields),
        "only_nv": len(only_nv),
        "only_nv_examples": [f"{i} {nv_fields[i][0]} {nv_fields[i][1]!r}" for i in only_nv[:8]],
        "only_lp": len(only_lp),
        "only_lp_examples": [
            f"{i} {lp_fields[i].get('tagName')} {lp_fields[i].get('name') or lp_fields[i].get('inputType')}"
            for i in only_lp[:8]
        ],
        "lp_field_keys": sorted({k for f in lp_fields.values() for k in f}),
        "same_fields": not only_nv and not only_lp,
    }


# ---------------------------------------------------------------- stats


def med(xs):
    xs = [x for x in xs if x is not None]
    return round(statistics.median(xs), 2) if xs else None


def iqr(xs):
    xs = sorted(x for x in xs if x is not None)
    if len(xs) < 4:
        return None
    q = statistics.quantiles(xs, n=4)
    return [round(q[0], 2), round(q[2], 2)]


class Arm:
    def __init__(self, name):
        self.name = name
        self.ms, self.bytes, self.calls, self.errors, self.bad = [], [], [], [], 0
        self.stopped = None
        self.crashes = 0

    def fail(self, msg):
        """Record an error; the same error twice stops the arm (a missing
        element waits out its timeout every rep, a crash repeats)."""
        if self.errors and self.errors[-1] == msg and not msg.startswith("check:"):
            self.stopped = msg
        self.errors.append(msg)

    def summary(self):
        return {
            "median_ms": med(self.ms),
            "iqr_ms": iqr(self.ms),
            "n": len(self.ms),
            "bytes": med(self.bytes),
            "cdp_calls": med(self.calls) if self.calls else None,
            "errors": len(self.errors),
            "error_examples": self.errors[:3],
            "wrong": self.bad,
            "stopped": self.stopped,
            "crashes": self.crashes,
        }


def text_of(b: bytes) -> str:
    return b.decode(errors="replace")


# ---------------------------------------------------------------- the probe


class Probe:
    def __init__(self, args):
        self.args = args
        self.out = Path(args.out)
        self.out.mkdir(parents=True, exist_ok=True)
        self.reps = args.reps
        self.rows = []
        self.notes = []

    def log(self, msg):
        print(msg, flush=True)

    def setup(self):
        port = free_port()
        self.httpd = ThreadedServer(("127.0.0.1", port), Handler)
        threading.Thread(target=self.httpd.serve_forever, daemon=True).start()
        self.base = f"http://127.0.0.1:{port}"
        self.acme_p, self.acme = start_site("bench/site/server.py", ["--port", "0"])
        self.edge_p, self.edge = start_site("tests/fixtures/edge_site.py", ["0"])
        self.nav = Nav(self.args.navigera, self.args.lightpanda, self.out)
        os.environ["LP_RAW_LOG"] = str(self.out / "raw-lightpanda.log")
        self.raw = RawPanda(self.args.lightpanda, free_port())
        ok, out, _, _, _, res = self.nav.cdp("LP.version")
        self.lp_version = (res or {}).get("version")
        self.nv_version = text_of(subprocess.run([self.args.navigera, "--version"], capture_output=True).stdout).strip()
        self.log(f"navigera {self.nv_version} · Lightpanda {self.lp_version}")

    def teardown(self):
        for f in (self.nav.quit, self.raw.close, self.httpd.shutdown):
            try:
                f()
            except Exception:
                pass
        for p in (self.acme_p, self.edge_p):
            p.kill()

    # One page in both browsers, with an optional setup script run in each.
    def load(self, url, prep=None):
        before = None
        if isinstance(url, tuple):
            url, before = url
        if not self.nav.alive():
            self.notes.append(f"navigera's Lightpanda was gone before {url}: session restarted")
            self.nav.restart()
        if not self.raw.alive():
            self.notes.append(f"our Lightpanda was gone before {url}: restarted")
            self.raw.restart()
        if before:
            first, js = before
            self.nav.run("goto", first)
            self.nav.eval(js)
            self.raw.goto(first)
            self.raw.eval(js)
        self.nav.run("goto", url)
        self.raw.goto(url)
        if prep:
            self.nav.eval(prep)
            self.raw.eval(prep)

    def rotate(self, i, fns):
        order = fns[i % len(fns):] + fns[: i % len(fns)]
        for f in order:
            f()

    # ---- read pairs: snapshot, text, forms

    def read_pair(self, kind, page, url, nv_args, lp_method, lp_params, content_key, compare, prep=None,
                  full_compare=False):
        self.load(url, prep)
        nv, nvlp, raw = Arm("nv"), Arm("nvlp"), Arm("raw")
        sample = {}

        def recover():
            self.nav.restart()
            self.load(url, prep)

        def a_nv():
            if nv.stopped:
                return
            ok, out, err, ms, calls = self.nav.run(*nv_args)
            nv.calls.append(calls)
            if ok:
                nv.ms.append(ms)
                nv.bytes.append(len(out))
                sample.setdefault("nv", text_of(out))
            else:
                nv.fail(err.strip()[:300])
                if Nav.gone(err):
                    nv.crashes += 1
                    recover()

        def a_nvlp():
            if nvlp.stopped:
                return
            ok, out, err, ms, calls, res = self.nav.cdp(lp_method, lp_params)
            nvlp.calls.append(calls)
            if ok and res is not None:
                nvlp.ms.append(ms)
                content = res.get(content_key)
                nvlp.bytes.append(len(content if isinstance(content, str) else json.dumps(content)))
                sample.setdefault("nvlp", content)
            else:
                nvlp.fail(err.strip()[:300])
                if Nav.gone(err):
                    nvlp.crashes += 1
                    recover()

        def a_raw():
            if raw.stopped:
                return
            try:
                res, ms, _ = self.raw.call(lp_method, lp_params)
                raw.ms.append(ms)
                content = res.get(content_key)
                raw.bytes.append(len(content if isinstance(content, str) else json.dumps(content)))
                sample.setdefault("raw", content)
            except Exception as e:  # a crash or a CDP error is a result
                raw.fail(f"{type(e).__name__}: {e}"[:300])
                if not self.raw.alive():
                    raw.crashes += 1
                    self.raw.restart()
                    self.raw.goto(url if isinstance(url, str) else url[0])
                    if prep:
                        self.raw.eval(prep)

        for i in range(self.reps):
            self.rotate(i, [a_nv, a_nvlp, a_raw])
        cmp_res = cmp_full = None
        if "nv" in sample and "nvlp" in sample:
            cmp_res = compare(sample["nv"], sample["nvlp"])
        if full_compare and "nvlp" in sample:
            ok, out, _, _, _ = self.nav.run("ax", "--limit", "0")
            if ok:
                cmp_full = compare(text_of(out), sample["nvlp"])
        same_lp = None
        if "nvlp" in sample and "raw" in sample:
            same_lp = sample["nvlp"] == sample["raw"]
        name = re.sub(r"[^\w.-]+", "_", f"{kind}-{page}")
        for k, v in sample.items():
            (self.out / f"{name}.{k}.txt").write_text(v if isinstance(v, str) else json.dumps(v, indent=1))
        row = {
            "kind": kind, "page": page, "nv_op": " ".join(nv_args), "lp": f"{lp_method} {json.dumps(lp_params or {})}",
            "nv": nv.summary(), "nvlp": nvlp.summary(), "raw": raw.summary(),
            "compare": cmp_res, "compare_whole_ax": cmp_full, "lp_same_both_browsers": same_lp,
        }
        self.rows.append(row)
        self.log(json.dumps(row))

    # ---- action pairs: click, fill

    def act_pair(self, kind, page, url, find, nv_args_fn, lp_method, lp_params_fn, check, prep=None,
                 selector=None):
        """find(tree_text, is_lp) -> node id; check(eval_fn, state, arm) -> (ok, detail).
        Where navigera's tree can't be read (`selector` given), its op aims
        at the element by CSS instead of a ref."""
        self.load(url, prep)
        ok, out, err, _, _ = self.nav.run("ax", "--limit", "0")
        ref = find(text_of(out), False) if ok else None
        how = "ref"
        if ref is None and selector:
            if Nav.gone(err):
                self.notes.append(f"{kind} {page}: navigera's `ax` ended its Lightpanda; the op aims by --selector")
                self.nav.restart()
                self.load(url, prep)
            ref, how = selector, "selector"
        _, _, _, _, _, res = self.nav.cdp("LP.getSemanticTree", {"format": "text"})
        nid = find((res or {}).get("semanticTree", ""), True)
        try:
            rres, _, _ = self.raw.call("LP.getSemanticTree", {"format": "text"})
            rid = find(rres.get("semanticTree", ""), True)
        except Exception as e:
            rid = None
            self.notes.append(f"{kind} {page}: raw tree failed: {e}")
        nv, nvlp, raw = Arm("nv"), Arm("nvlp"), Arm("raw")
        state = {"nv": 0, "nvlp": 0, "raw": 0}
        sample = {}

        def a_nv():
            if ref is None:
                nv.errors.append("no ref found in navigera's tree")
                return
            args = nv_args_fn(ref) if how == "ref" else [nv_args_fn(ref)[0], "--selector", ref, *nv_args_fn(ref)[2:]]
            ok, out, err, ms, calls = self.nav.run(*args)
            nv.calls.append(calls)
            if ok:
                nv.ms.append(ms)
                nv.bytes.append(len(out))
                sample.setdefault("nv", text_of(out))
                good, detail = check(self.nav.eval, state, "nv")
                if not good:
                    nv.bad += 1
                    nv.errors.append(f"check: {detail}")
            else:
                nv.errors.append(err.strip()[:300])

        def a_nvlp():
            if nid is None:
                nvlp.errors.append("no node found in LP's tree")
                return
            ok, out, err, ms, calls, res = self.nav.cdp(lp_method, lp_params_fn(nid))
            nvlp.calls.append(calls)
            if ok:
                nvlp.ms.append(ms)
                nvlp.bytes.append(len(json.dumps(res)))
                sample.setdefault("nvlp", json.dumps(res))
                good, detail = check(self.nav.eval, state, "nvlp")
                if not good:
                    nvlp.bad += 1
                    nvlp.errors.append(f"check: {detail}")
            else:
                nvlp.errors.append(err.strip()[:300])

        def a_raw():
            if rid is None:
                raw.errors.append("no node found in LP's tree")
                return
            try:
                res, ms, _ = self.raw.call(lp_method, lp_params_fn(rid))
                raw.ms.append(ms)
                raw.bytes.append(len(json.dumps(res)))
                good, detail = check(self.raw.eval, state, "raw")
                if not good:
                    raw.bad += 1
                    raw.errors.append(f"check: {detail}")
            except Exception as e:
                raw.errors.append(f"{type(e).__name__}: {e}"[:300])

        for i in range(self.reps):
            self.rotate(i, [a_nv, a_nvlp, a_raw])
        row = {
            "kind": kind, "page": page, "nv_op": " ".join(nv_args_fn("<ref>")),
            "lp": f"{lp_method} {json.dumps(lp_params_fn('<id>'))}",
            "ids": {"nv_ref": ref, "nv_target": how, "nvlp": nid, "raw": rid},
            "nv": nv.summary(), "nvlp": nvlp.summary(), "raw": raw.summary(),
            "nv_sample": (sample.get("nv") or "")[:600],
        }
        self.rows.append(row)
        self.log(json.dumps(row))

    # ---- what ends Lightpanda on the large page

    def crash_check(self, url):
        """The calls navigera's `ax` sends, one by one on our own Lightpanda,
        on the large page: which one (if any) ends the process."""
        out = {}
        for method, params in [("LP.getSemanticTree", {"format": "text"}), ("Page.getFrameTree", None),
                               ("Accessibility.getFullAXTree", None)]:
            if not self.raw.alive():
                self.raw.restart()
            self.raw.goto(url)
            try:
                _, ms, n = self.raw.call(method, params)
                frames = self.raw.cdp.ws.frames
                out[method] = {"ok": True, "ms": round(ms, 2), "bytes": n, "frames": len(frames),
                               "largest_frame": max(frames)}
            except Exception as e:
                time.sleep(0.5)
                out[method] = {"ok": False, "error": f"{type(e).__name__}: {e}"[:200], "process_alive": self.raw.alive(),
                               "exit": self.raw.proc.poll()}
        if not self.raw.alive():
            self.raw.restart()
        row = {"kind": "crash-check", "page": "page_c", "calls": out}
        self.rows.append(row)
        self.log(json.dumps(row))

    # ---- what the trees say after an action

    def state_check(self, acme):
        """After a click on a checkbox and a fill of a password field: what
        LP.getSemanticTree, LP.detectForms and navigera's `ax` say about
        the element, next to the DOM's own property."""
        out = {}
        self.load(f"{acme}/preferences")
        tree = lambda: self.raw.call("LP.getSemanticTree", {"format": "text"})[0]["semanticTree"]
        line = lambda t, i: next((ln.strip() for ln in t.splitlines() if re.match(rf"^\s*{i} ", ln)), None)
        nid = next((i for i, (r, n) in lp_nodes(tree())[0].items() if n == "Price-drop alerts"), None)
        if nid:
            before = line(tree(), nid)
            self.raw.call("LP.clickNode", {"backendNodeId": nid})
            forms = self.raw.call("LP.detectForms")[0]["forms"]
            fld = next((f for fm in forms for f in fm["fields"] if f.get("backendNodeId") == nid), None)
            out["checkbox"] = {"dom_checked": self.raw.eval("document.querySelector('input[name=price_drop]').checked"),
                               "lp_tree_before": before, "lp_tree_after": line(tree(), nid),
                               "lp_detectForms_field": fld}
        ok, o, _, _, _ = self.nav.run("ax", "--limit", "0")
        ref = next((i for i, (r, n) in ax_nodes(text_of(o))[0].items() if n == "Price-drop alerts"), None)
        if ref:
            self.nav.run("click", str(ref))
            ok, o, _, _, _ = self.nav.run("ax", "--limit", "0")
            out["checkbox_nv"] = {"dom_checked": self.nav.eval("document.querySelector('input[name=price_drop]').checked"),
                                  "ax_after": next((ln.strip() for ln in text_of(o).splitlines()
                                                    if "Price-drop alerts" in ln), None)}
        self.load(f"{acme}/account")
        nid = next((i for i, (r, n) in lp_nodes(tree())[0].items() if n == "Password"), None)
        if nid:
            before = line(tree(), nid)
            self.raw.call("LP.fillNode", {"backendNodeId": nid, "text": "hunter2"})
            forms = self.raw.call("LP.detectForms")[0]["forms"]
            fld = next((f for fm in forms for f in fm["fields"] if f.get("backendNodeId") == nid), None)
            md = self.raw.call("LP.dump", {"format": "markdown"})[0]["content"]
            out["password"] = {"dom_value_len": self.raw.eval("document.getElementById('password').value.length"),
                               "lp_tree_before": before, "lp_tree_after": line(tree(), nid),
                               "lp_detectForms_value": (fld or {}).get("value"),
                               "markdown_has_secret": "hunter2" in md,
                               "html_has_secret": "hunter2" in self.raw.call("LP.dump", {"format": "html"})[0]["content"]}
        row = {"kind": "state-check", "page": "acme", "result": out}
        self.rows.append(row)
        self.log(json.dumps(row))

    # ---- page status

    def status_cases(self):
        cases = [
            ("ok", f"{self.base}/page_a.html"),
            ("http404", f"{self.base}/x/404"),
            ("http500", f"{self.base}/x/500"),
            ("cloudflare403", f"{self.base}/x/cf"),
            ("empty200", f"{self.base}/x/empty"),
            ("jserror", f"{self.base}/x/jserr"),
            ("subresource4xx5xx", f"{self.base}/x/xhr"),
            ("refused", "http://127.0.0.1:9/"),
            ("dns", "http://no-such-host.invalid/"),
        ]
        for name, url in cases:
            if not self.nav.alive():
                self.nav.restart()
            if not self.raw.alive():
                self.raw.restart()
            self.nav.run("errors", "--clear")
            ok, out, err, goto_ms, goto_calls = self.nav.run("goto", url)
            time.sleep(0.3)  # let the page's own fetches finish
            eok, eout, eerr, _, _ = self.nav.run("errors")
            nvlp, raw = Arm("nvlp"), Arm("raw")
            status_nv = status_raw = None
            try:
                self.raw.goto(url)
            except Exception as e:
                self.notes.append(f"status {name}: raw navigate: {e}")
            time.sleep(0.3)

            def a_nvlp():
                nonlocal status_nv
                ok2, out2, err2, ms, calls, res = self.nav.cdp("LP.getPageStatus")
                nvlp.calls.append(calls)
                if ok2:
                    nvlp.ms.append(ms)
                    nvlp.bytes.append(len(json.dumps(res)))
                    status_nv = res
                else:
                    nvlp.errors.append(err2.strip()[:300])

            def a_raw():
                nonlocal status_raw
                try:
                    res, ms, _ = self.raw.call("LP.getPageStatus")
                    raw.ms.append(ms)
                    raw.bytes.append(len(json.dumps(res)))
                    status_raw = res
                except Exception as e:
                    raw.errors.append(f"{type(e).__name__}: {e}"[:300])

            for i in range(self.reps):
                self.rotate(i, [a_nvlp, a_raw])
            row = {
                "kind": "page-status", "page": name, "url": url,
                "nv_goto": {"ok": ok, "ms": round(goto_ms, 2), "cdp_calls": goto_calls,
                            "stdout": text_of(out)[:400], "stderr": err.strip()[:600], "bytes": len(out) + len(err)},
                "nv_errors": {"ok": eok, "stdout": text_of(eout)[:600], "stderr": eerr.strip()[:300]},
                "lp_status_nv_session": status_nv, "lp_status_raw": status_raw,
                "nvlp": nvlp.summary(), "raw": raw.summary(),
            }
            self.rows.append(row)
            self.log(json.dumps(row))

    # ---- the plan

    def run(self):
        b, acme = self.base, self.acme
        tree = ("LP.getSemanticTree", {"format": "text"}, "semanticTree")
        pages = [
            ("page_a", f"{b}/page_a.html", None),
            ("page_b", f"{b}/page_b.html", None),
            ("page_c", f"{b}/page_c.html", None),
            ("acme-login", f"{acme}/account", None),
            ("acme-shop", f"{acme}/shop", None),
            ("acme-preferences", f"{acme}/preferences", None),
            ("acme-support", f"{acme}/support", None),
            # A cart with one item, so that checkout shows its form.
            ("acme-checkout", (f"{acme}/checkout", (f"{acme}/shop", ADD_TO_CART)), None),
            ("edge-slow", f"{self.edge}/slow?ms=0", None),
        ]
        self.crash_check(f"{b}/page_c.html")
        # Snapshot: navigera's default `ax` (what an agent reads) and the
        # whole tree (`--limit 0`, what the comparison needs) against LP's.
        for page, url, prep in pages:
            self.read_pair("snapshot", page, url, ["ax"], *tree, compare_trees, prep)
        for page, url, prep in pages:
            if page in ("page_c", "acme-preferences", "acme-support"):
                self.read_pair("snapshot-full", page, url, ["ax", "--limit", "0"], *tree, compare_trees, prep)
                self.read_pair("snapshot-interactive", page, url, ["ax", "--limit", "0"], "LP.getSemanticTree",
                               {"format": "text", "interactiveOnly": True}, "semanticTree", compare_trees, prep)
        for page, url, prep in pages:
            self.read_pair("text", page, url, ["text"], "LP.dump", {"format": "markdown"}, "content",
                           compare_text, prep)
        for page, url, prep in pages:
            if page in ("page_a", "acme-login", "acme-preferences", "acme-support", "acme-checkout"):
                self.read_pair("forms", page, url, ["ax", "--selector", "form", "--limit", "0"], "LP.detectForms",
                               None, "forms", compare_forms, prep, full_compare=True)

        def by(role, name):
            def find(text, is_lp):
                nodes = lp_nodes(text)[0] if is_lp else ax_nodes(text)[0]
                for i, (r, n) in nodes.items():
                    if r == role and n.strip() == name:
                        return i
                return None
            return find

        def first_textbox(text, is_lp):
            nodes = lp_nodes(text)[0] if is_lp else ax_nodes(text)[0]
            for i, (r, n) in sorted(nodes.items()):
                if r == "textbox":
                    return i
            return None

        # Clicks.
        def check_search(ev, st, arm):
            n = ev("document.querySelectorAll('.card:not(.hidden)').length")
            return n == 250, f"visible cards {n}, want 250"

        self.act_pair("click", "page_a-search", f"{b}/page_a.html", by("button", "Search"),
                      lambda r: ["click", str(r)], "LP.clickNode", lambda i: {"backendNodeId": i}, check_search,
                      prep="document.getElementById('q').value = 'widget'")

        def check_more(ev, st, arm):
            st[arm] += 1
            got = ev("document.getElementById('status').textContent")
            want = f"more: {ev('window.__more')}"
            return got == want and got != "more: undefined", f"status {got!r}"

        self.act_pair("click", "page_c-more", f"{b}/page_c.html", by("button", "More"),
                      lambda r: ["click", str(r)], "LP.clickNode", lambda i: {"backendNodeId": i}, check_more,
                      selector="#more")

        def check_place(ev, st, arm):
            got = ev("document.getElementById('status').textContent")
            return got == "placed: ada x 2", f"status {got!r}"

        self.act_pair("click", "page_a-submit", f"{b}/page_a.html", by("button", "Place order"),
                      lambda r: ["click", str(r)], "LP.clickNode", lambda i: {"backendNodeId": i}, check_place,
                      prep="document.getElementById('cust').value='ada'; document.getElementById('qty').value='2'")

        def check_toggle(ev, st, arm):
            # nv and nvlp drive the same page: one count per browser.
            key = "raw" if arm == "raw" else "nv"
            st[key] += 1
            got = ev("document.querySelector('input[name=price_drop]').checked")
            return got is (st[key] % 2 == 1), f"checked {got!r} after {st[key]} clicks"

        self.act_pair("click", "acme-checkbox", f"{acme}/preferences", by("checkbox", "Price-drop alerts"),
                      lambda r: ["click", str(r)], "LP.clickNode", lambda i: {"backendNodeId": i}, check_toggle)

        # Fills: a value and the events a page listens for.
        listen = ("window.__ev = []; for (const t of ['input', 'change']) "
                  "document.getElementById('q').addEventListener(t, () => window.__ev.push(t))")

        def check_fill(ev, st, arm):
            got = ev("[document.getElementById('q').value, window.__ev.splice(0).join(',')]")
            return isinstance(got, list) and got[0] == "brass sprocket" and "input" in got[1], f"value/events {got!r}"

        self.act_pair("fill", "page_a-q", f"{b}/page_a.html", first_textbox,
                      lambda r: ["fill", str(r), "brass sprocket"], "LP.fillNode",
                      lambda i: {"backendNodeId": i, "text": "brass sprocket"}, check_fill, prep=listen)

        def check_email(ev, st, arm):
            got = ev("document.getElementById('email').value")
            return got == "demo@acme.test", f"value {got!r}"

        self.act_pair("fill", "acme-login-email", f"{acme}/account", by("textbox", "Email"),
                      lambda r: ["fill", str(r), "demo@acme.test"], "LP.fillNode",
                      lambda i: {"backendNodeId": i, "text": "demo@acme.test"}, check_email)

        def check_password(ev, st, arm):
            got = ev("document.getElementById('password').value")
            return got == "hunter2", f"value {got!r}"

        self.act_pair("fill", "acme-login-password", f"{acme}/account", by("textbox", "Password"),
                      lambda r: ["fill", str(r), "hunter2"], "LP.fillNode",
                      lambda i: {"backendNodeId": i, "text": "hunter2"}, check_password)

        self.state_check(acme)
        self.status_cases()

    def write(self):
        res = {
            "navigera": self.nv_version, "lightpanda": self.lp_version, "reps": self.reps,
            "runner": {"cpu": cpu_model(), "nproc": os.cpu_count(), "image": os.environ.get("ImageVersion")},
            "rows": self.rows, "notes": self.notes,
        }
        (self.out / "results.json").write_text(json.dumps(res, indent=1))
        md = summary_md(res)
        (self.out / "summary.md").write_text(md)
        if os.environ.get("GITHUB_STEP_SUMMARY"):
            with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as f:
                f.write(md)
        print(md)


def cpu_model():
    try:
        for line in open("/proc/cpuinfo"):
            if line.startswith("model name"):
                return line.split(":", 1)[1].strip()
    except OSError:
        pass
    return None


def fmt(a):
    if not a or a.get("median_ms") is None:
        return f"err {a.get('errors') if a else '-'}"
    s = f"{a['median_ms']}"
    if a.get("cdp_calls") is not None:
        s += f" ({a['cdp_calls']:g} rt)"
    if a.get("wrong"):
        s += f" WRONG {a['wrong']}"
    if a.get("errors"):
        s += f" err {a['errors']}"
    return s


def summary_md(res):
    out = [f"## LP domain vs navigera on Lightpanda {res['lightpanda']} ({res['navigera']}, median of {res['reps']} ms)\n",
           f"runner: {res['runner']}\n",
           "| kind | page | nv op ms | nv bytes | LP via nv ms | LP raw ms | LP bytes | correctness |",
           "|---|---|---|---|---|---|---|---|"]
    for r in res["rows"]:
        if r["kind"] in ("page-status", "crash-check", "state-check"):
            continue
        c = r.get("compare") or r.get("compare_whole_ax") or {}
        if r["kind"].startswith("snapshot"):
            corr = (f"same" if c.get("same") else
                    f"refs nv {c.get('nv_refs')} lp {c.get('lp_interactive')}: only nv {c.get('only_nv')}, "
                    f"only lp {c.get('only_lp')}, differ {c.get('role_or_name_differs')}; headings "
                    f"{c.get('headings_nv')}/{c.get('headings_lp')}")
        elif r["kind"] == "text":
            corr = ("same words" if c.get("same_words") else
                    f"words nv {c.get('nv_words')} lp {c.get('lp_words')}: only nv {c.get('words_only_nv')}, "
                    f"only lp {c.get('words_only_lp')}")
        elif r["kind"] == "forms":
            corr = ("same fields" if c.get("same_fields") else
                    f"fields nv {c.get('nv_fields')} lp {c.get('lp_fields')}: only nv {c.get('only_nv')}, "
                    f"only lp {c.get('only_lp')}")
        else:
            corr = f"checks wrong: nv {r['nv']['wrong']} / LP {r['nvlp']['wrong']} / raw {r['raw']['wrong']}"
        out.append(f"| {r['kind']} | {r['page']} | {fmt(r['nv'])} | {r['nv']['bytes']} | {fmt(r['nvlp'])} | "
                   f"{fmt(r['raw'])} | {r['raw']['bytes'] or r['nvlp']['bytes']} | {corr} |")
    out += ["", "| status case | navigera goto | LP.getPageStatus | LP via nv ms | LP raw ms |", "|---|---|---|---|---|"]
    for r in res["rows"]:
        if r["kind"] != "page-status":
            continue
        g = r["nv_goto"]
        said = (g["stdout"] if g["ok"] else g["stderr"]).replace("\n", " ").replace("|", "/")[:160]
        st = json.dumps(r["lp_status_raw"] or r["lp_status_nv_session"])[:220].replace("|", "/")
        out.append(f"| {r['page']} | {'ok' if g['ok'] else 'ERR'}: {said} | {st} | {fmt(r['nvlp'])} | {fmt(r['raw'])} |")
    for r in res["rows"]:
        if r["kind"] == "crash-check":
            out += ["", f"crash check on {r['page']} (our own Lightpanda): `{json.dumps(r['calls'])}`"]
        if r["kind"] == "state-check":
            out += ["", f"state check: `{json.dumps(r['result'], ensure_ascii=False)}`"]
    if res["notes"]:
        out += ["", "Notes:"] + [f"- {n}" for n in res["notes"]]
    return "\n".join(out) + "\n"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--navigera", required=True)
    ap.add_argument("--lightpanda", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--reps", type=int, default=20)
    args = ap.parse_args()
    p = Probe(args)
    p.setup()
    try:
        p.run()
    finally:
        try:
            p.write()
        finally:
            p.teardown()


if __name__ == "__main__":
    main()
