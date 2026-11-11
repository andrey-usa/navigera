#!/usr/bin/env python3
"""Ops stage runner: one matched operation per call, warm session, median of N.

One invocation drives ONE tool through every op, N reps each, in a single
warm session, and prints one JSON line + writes a transcript JSON file.

Ops (all on the ladder fixtures, page_a unless noted):
  goto          goto page_a (warm session, page already loaded)
  fill          fill #q with "widget"
  click         click #search (client-side filter, no navigation)
  click-submit  click #place (form submit, handled by the page script)
  read-text     eval the h1 text ("Search results")
  snapshot      accessibility snapshot of page_a (default output)
  screenshot    screenshot to a file
  form-batch    fill #cust "ada", fill #qty "2", click #place, eval #status
                — one call for tools with a batch command (navigera batch,
                agent-browser batch --bail), four calls otherwise.

Timing is the wall of the call itself: one CLI process (or one MCP
tools/call) per op, against the warm session — the way an agent's tool
calls drive a browser. Session start, state resets between reps and
verification reads are untimed setup, reported separately in the transcript.

Each op is matched: same page, same element, same expected value, for every
tool. A rep whose verification fails is recorded failed (with the reason),
not silently dropped. A tool that cannot perform an op at all records the op
as not run, with the reason — never as a zero.

Output mode: every tool is measured in its DEFAULT output mode (navigera
compact, agent-browser text, playwright-cli text, MCP over JSON-RPC which
is its only interface, browser-use exec). Where a default is a plain value
it is parsed as printed (a JSON scalar when it parses, else the raw text);
where playwright-cli wraps the value in a "### Result" block, the block is
extracted. No tool is switched into a machine mode for any op, so no
per-op footnote is needed. NAVIGERA_OUTPUT is stripped from this process
so a stray NAVIGERA_OUTPUT=json in the parent cannot switch navigera out
of its default mode. Verification reads are untimed, outside the timed
call, for every tool including browser-use. Snapshot checks assert the
page's known heading ("Search results") and the Search button are present;
screenshot checks fully decode the PNG (chunk CRCs, IDAT decompression,
exact raster size, every scanline filter byte 0-4) and assert its dimensions equal the session's live
viewport, read untimed.

Preflight (bench_harness_checks): the fixture pages are fetched over HTTP
before any tool runs; a 404 or an empty served directory aborts the run.

Usage: ops_agent.py --tool {nv,ab,pw,pwmcp,cdmcp,bu} --reps 10 --out DIR
  --session NAME [--navigera PATH] [--chrome PATH] [--browser-use PATH]
  [--own-browser]

Env: LADDER_BASE (fixture server base URL), CHROME_BIN, NAVIGERA.
"""
import argparse
import json
import os
import re
import struct
import subprocess
import sys
import tempfile
import time
import zlib
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from cli_agent import Mcp, mcp_json, ref_for, run, uid_for  # noqa: E402

# navigera is measured in its default (compact) output.
# Strip NAVIGERA_OUTPUT so a stray NAVIGERA_OUTPUT=json in the parent
# process cannot switch it into JSON mode. cli_agent.py sets the same
# default at import (for the other scenarios); the pop here, after the
# imports, wins for this process.
os.environ.pop("NAVIGERA_OUTPUT", None)


def parse_scalar(text: str):
    """Parse a tool's printed scalar value: JSON when it parses, else raw text.

    navigera compact eval prints the bare value ("Search results", "250");
    agent-browser default eval prints a JSON scalar ('"Search results"').
    Both reduce to the same Python value here.
    """
    t = text.strip()
    try:
        return json.loads(t)
    except Exception:
        return t


def parse_pw_result(out: str):
    """Extract playwright-cli's default eval value from its "### Result" block."""
    lines = out.splitlines()
    try:
        i = lines.index("### Result")
    except ValueError:
        raise RuntimeError(
            f"playwright-cli eval: no '### Result' block in {out[:300]!r}")
    buf = []
    for ln in lines[i + 1:]:
        if ln.startswith("###"):
            break
        buf.append(ln)
    return parse_scalar("\n".join(buf))


# PNG colour types -> channels; valid bit depths per type (RFC 2083).
_PNG_CHANNELS = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}
_PNG_DEPTHS = {0: (1, 2, 4, 8, 16), 2: (8, 16), 3: (1, 2, 4, 8),
               4: (8, 16), 6: (8, 16)}
# Adam7 passes: (x0, y0, dx, dy).
_PNG_ADAM7 = ((0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4),
              (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2))


def _png_raster_bytes(width, height, bit_depth, color_type, interlace):
    """Exact decompressed IDAT length for a PNG (one filter byte per row)."""
    bpp = _PNG_CHANNELS[color_type] * bit_depth  # bits per pixel

    def row_bytes(w):
        return (w * bpp + 7) // 8

    if interlace == 0:
        return height * (1 + row_bytes(width))
    total = 0
    for x0, y0, dx, dy in _PNG_ADAM7:
        pw = (width - x0 + dx - 1) // dx if width > x0 else 0
        ph = (height - y0 + dy - 1) // dy if height > y0 else 0
        total += ph * (1 + row_bytes(pw))
    return total


def _png_validate_filters(raw, width, height, bit_depth, color_type, interlace):
    """Every scanline's filter byte must be 0-4 (RFC 2083).

    Walks the decompressed raster with the same row layout as
    _png_raster_bytes, so the offsets are only valid after the length
    check has passed. Raises ValueError naming the offending byte.
    """
    bpp = _PNG_CHANNELS[color_type] * bit_depth

    def row_bytes(w):
        return (w * bpp + 7) // 8

    def check_rows(offset, rows, rb):
        for i in range(rows):
            f = raw[offset + i * (1 + rb)]
            if f > 4:
                raise ValueError(
                    f"has an invalid row-filter byte {f} "
                    f"(valid filters are 0-4)")
        return offset + rows * (1 + rb)

    if interlace == 0:
        check_rows(0, height, row_bytes(width))
        return
    offset = 0
    for x0, y0, dx, dy in _PNG_ADAM7:
        pw = (width - x0 + dx - 1) // dx if width > x0 else 0
        ph = (height - y0 + dy - 1) // dy if height > y0 else 0
        offset = check_rows(offset, ph, row_bytes(pw))


def png_dimensions(path: Path) -> tuple[int, int]:
    """Decode a PNG for real and return its dimensions (stdlib only).

    Verifies the signature, walks every chunk checking its CRC, requires
    IHDR first and at least one IDAT, then zlib-decompresses the IDAT
    stream and checks the decompressed raster is exactly the size IHDR
    implies, with every scanline's filter byte in 0-4. A header-only
    fake (signature + IHDR + IEND, no image data), a truncated file,
    garbage IDAT data, or a raster with an invalid filter byte fails
    here; a screenshot that is not a decodable image of the size it
    claims cannot pass. No Pillow dependency, so the check runs the same
    on CI runners.
    """
    data = path.read_bytes()
    if len(data) < 8 or data[:8] != b"\x89PNG\r\n\x1a\n":
        raise RuntimeError(f"screenshot: {path} is not a PNG")
    pos = 8
    width = height = bit_depth = color_type = interlace = None
    idat = bytearray()
    seen_plte = seen_iend = False
    first = True
    while pos + 8 <= len(data):
        length, ctype = struct.unpack(">I4s", data[pos:pos + 8])
        end = pos + 12 + length
        if end > len(data):
            raise RuntimeError(f"screenshot: {path} has a truncated {ctype!r} chunk")
        chunk = data[pos + 8:pos + 8 + length]
        crc, = struct.unpack(">I", data[pos + 8 + length:end])
        if zlib.crc32(ctype + chunk) & 0xFFFFFFFF != crc:
            raise RuntimeError(f"screenshot: {path} has a bad CRC in its {ctype!r} chunk")
        if first and ctype != b"IHDR":
            raise RuntimeError(f"screenshot: {path} has no IHDR chunk first")
        first = False
        if ctype == b"IHDR":
            (width, height, bit_depth, color_type, comp, filt,
             interlace) = struct.unpack(">IIBBBBB", chunk)
            if comp != 0 or filt != 0 or interlace not in (0, 1):
                raise RuntimeError(f"screenshot: {path} has an unsupported IHDR")
            if (color_type not in _PNG_DEPTHS
                    or bit_depth not in _PNG_DEPTHS[color_type]):
                raise RuntimeError(
                    f"screenshot: {path} has an invalid colour type/depth "
                    f"{color_type}/{bit_depth}")
            if width <= 0 or height <= 0:
                raise RuntimeError(f"screenshot: {path} has bad dimensions {width}x{height}")
        elif ctype == b"PLTE":
            seen_plte = True
        elif ctype == b"IDAT":
            idat += chunk
        elif ctype == b"IEND":
            seen_iend = True
            pos = end
            break
        pos = end
    if width is None:
        raise RuntimeError(f"screenshot: {path} has no IHDR chunk")
    if not seen_iend:
        raise RuntimeError(f"screenshot: {path} is truncated (no IEND trailer)")
    if pos != len(data):
        raise RuntimeError(f"screenshot: {path} has trailing data after IEND")
    if color_type == 3 and not seen_plte:
        raise RuntimeError(f"screenshot: {path} is a palette image with no PLTE")
    if not idat:
        raise RuntimeError(f"screenshot: {path} has no IDAT chunk (no image data)")
    dec = zlib.decompressobj()
    try:
        raw = dec.decompress(bytes(idat))
        raw += dec.flush()
    except zlib.error as e:
        raise RuntimeError(f"screenshot: {path} has undecodable IDAT data ({e})")
    if not dec.eof or dec.unused_data:
        raise RuntimeError(f"screenshot: {path} has a truncated or padded zlib stream")
    expected = _png_raster_bytes(width, height, bit_depth, color_type, interlace)
    if len(raw) != expected:
        raise RuntimeError(
            f"screenshot: {path} decodes to {len(raw)} raster bytes, "
            f"expected {expected} for {width}x{height}")
    try:
        _png_validate_filters(raw, width, height, bit_depth, color_type, interlace)
    except ValueError as e:
        raise RuntimeError(f"screenshot: {path} {e}")
    return width, height


def check_screenshot_png(path: Path, expected_w: int, expected_h: int) -> tuple[int, int]:
    """Decode the screenshot PNG and assert it is the expected size."""
    w, h = png_dimensions(path)
    if (w, h) != (expected_w, expected_h):
        raise RuntimeError(
            f"screenshot: {path} is {w}x{h}, expected {expected_w}x{expected_h}")
    return w, h


def assert_snapshot_known_elements(text: str):
    """The snapshot must name the page's known heading and the Search button."""
    if "Search results" not in text:
        raise RuntimeError("snapshot: heading 'Search results' not present")
    if not re.search(r'button\s+"?Search"?', text):
        raise RuntimeError("snapshot: button 'Search' not present")

FIXTURES = HERE.parent / "fixtures"
OPS = ("goto", "fill", "click", "click-submit", "read-text", "snapshot",
       "screenshot", "form-batch")

TITLE = "(() => document.title)()"
HEADING = "(() => document.querySelector('h1').textContent)()"
STATUS = "(() => document.getElementById('status').textContent)()"
Q_VALUE = "(() => document.getElementById('q').value)()"
VISIBLE = "(() => document.querySelectorAll('.card:not(.hidden)').length)()"
PLACED = "(() => window.__placed || 0)()"
URL = "(() => location.href)()"

# The browser-use harness's documented AX-click workflow (from its skill):
# AX tree -> DOM box model -> click_at_xy. Reused for its click ops.
BU_CLICK = '''
def _nm(n):
    v = n.get("name")
    return v.get("value") if isinstance(v, dict) else v
def _rl(n):
    v = n.get("role")
    return v.get("value") if isinstance(v, dict) else v
def click_named(name, role="button"):
    nodes = cdp("Accessibility.getFullAXTree")["nodes"]
    hits = [n for n in nodes if _nm(n) == name and _rl(n) == role]
    assert hits, "no AX node " + name
    q = cdp("DOM.getBoxModel", backendNodeId=hits[0]["backendDOMNodeId"])["model"]["content"]
    click_at_xy(sum(q[0::2]) / 4, sum(q[1::2]) / 4)
'''


def preflight(base: str, expected: dict) -> None:
    """Fetch each fixture page over HTTP before any tool runs."""
    for page in ("page_a.html", "page_b.html"):
        url = f"{base}/{page}"
        try:
            with urllib.request.urlopen(url, timeout=10) as r:
                body = r.read()
        except Exception as e:
            raise RuntimeError(f"preflight: {url} did not serve: {e}")
        if r.status != 200:
            raise RuntimeError(f"preflight: {url} returned {r.status}")
        if not body:
            raise RuntimeError(f"preflight: {url} served an empty page")
    print(f"preflight: {base}/page_a.html and page_b.html serve over HTTP", flush=True)


def median(xs: list[float]) -> float:
    s = sorted(xs)
    n = len(s)
    return s[n // 2] if n % 2 else (s[n // 2 - 1] + s[n // 2]) / 2


class Tool:
    """One agent tool's warm session: start, one call per op, close."""

    def __init__(self, args, base: str, expected: dict, out_dir: Path):
        self.name = args.tool
        self.tag = args.tag or args.tool
        self.args = args
        self.base = base
        self.expected = expected
        self.out_dir = out_dir
        self.session = args.session
        self.calls: list[dict] = []  # transcript: every call, timed or not
        self.mcp: Mcp | None = None
        self.cwd: str | None = None
        self._refs: dict[str, str] = {}  # pwmcp: element -> ref
        self._uids: dict[str, str] = {}  # cdmcp: element -> uid
        self._bu_chrome: subprocess.Popen | None = None
        self._bu_port: int | None = None

    # -- generic plumbing -------------------------------------------------
    def log(self, kind: str, op: str, ms: float | None, ok: bool, detail: str = ""):
        self.calls.append({"kind": kind, "op": op, "ms": ms, "ok": ok,
                           "detail": detail[:500]})

    def cli(self, argv: list[str], env: dict, op: str, stdin: str | None = None,
            cwd: str | None = None) -> tuple[float, str]:
        """One timed CLI call; raises RuntimeError (via run) on failure."""
        try:
            ms, out = run(argv, env, cwd, stdin)
        except Exception as e:
            self.log("timed" if op in OPS else op, op, None, False, str(e))
            raise
        self.log("timed" if op in OPS else op, op, ms, True)
        return ms, out

    # -- session lifecycle -------------------------------------------------
    def start(self):
        if self.name in ("nv",):
            exe = self.args.navigera or os.environ["NAVIGERA"]
            chrome = self.args.chrome or os.environ["CHROME_BIN"]
            argv = [exe, "--session", self.session, "start", "--chromium", chrome]
            if self.args.own_browser:
                argv.append("--own-browser")
            ms, _ = self.cli(argv, dict(os.environ), "start")
            self.log("setup", "start", ms, True, "navigera session start")
        elif self.name == "ab":
            exe = os.environ.get("AGENT_BROWSER", "agent-browser")
            env = dict(os.environ)
            env.setdefault("AGENT_BROWSER_EXECUTABLE_PATH",
                           self.args.chrome or os.environ.get("CHROME_BIN", ""))
            env.setdefault("AGENT_BROWSER_ARGS", "--no-sandbox")
            ms, _ = self.cli([exe, "--session", self.session, "--json",
                              "open", f"{self.base}/page_a.html"], env, "start")
            self.log("setup", "start", ms, True, "agent-browser open")
        elif self.name == "pw":
            exe = os.environ.get("PLAYWRIGHT_CLI", "playwright-cli")
            self.cwd = tempfile.mkdtemp(prefix="pwops-")
            os.makedirs(os.path.join(self.cwd, ".playwright"))
            with open(os.path.join(self.cwd, ".playwright", "cli.config.json"), "w") as f:
                json.dump({"browser": {"browserName": "chromium", "launchOptions": {
                    "executablePath": self.args.chrome or os.environ["CHROME_BIN"],
                    "headless": True, "chromiumSandbox": False}},
                    "outputDir": self.cwd}, f)
            ms, _ = self.cli([exe, f"-s={self.session}", "open",
                              f"{self.base}/page_a.html"], dict(os.environ),
                             "start", cwd=self.cwd)
            self.log("setup", "start", ms, True, "playwright-cli open")
        elif self.name in ("pwmcp", "cdmcp"):
            chrome = self.args.chrome or os.environ["CHROME_BIN"]
            cwd = tempfile.mkdtemp(prefix=f"{self.name}-ops-")
            self.cwd = cwd
            if self.name == "pwmcp":
                exe = os.environ.get("PLAYWRIGHT_MCP", "playwright-mcp")
                argv = [exe, "--headless", "--browser", "chromium",
                        "--executable-path", chrome, "--isolated", "--no-sandbox",
                        "--output-dir", cwd]
            else:
                exe = os.environ.get("CHROME_DEVTOOLS_MCP", "chrome-devtools-mcp")
                # Filesystem tools are restricted to the OS temp dir unless
                # a workspace is configured; take_screenshot writes its file
                # into the ops out dir, so that dir must be a workspace root.
                # Absolute: the server resolves paths against its own cwd
                # (the temp dir below), so a relative out dir named a root
                # that does not exist, and the screenshot went under the
                # temp dir while the check looked here (run 43068761390,
                # 0/10 "missing or empty").
                argv = [exe, "--headless", "--executable-path", chrome, "--isolated",
                        "--chrome-arg=--no-sandbox", "--no-usage-statistics",
                        "--workspace", str(self.out_dir.resolve())]
            t0 = time.perf_counter()
            self.mcp = Mcp(argv, dict(os.environ), cwd)
            self.log("setup", "start", (time.perf_counter() - t0) * 1000, True,
                     f"{self.name} MCP server start")
            self._mcp_goto()
            self._mcp_resolve()
        elif self.name == "bu":
            self._bu_start()
        else:
            raise RuntimeError(f"unknown tool {self.name}")

    def close(self):
        try:
            if self.name == "nv":
                exe = self.args.navigera or os.environ["NAVIGERA"]
                ms, _ = self.cli([exe, "--session", self.session, "quit"],
                                 dict(os.environ), "close")
                self.log("setup", "close", ms, True, "")
            elif self.name == "ab":
                exe = os.environ.get("AGENT_BROWSER", "agent-browser")
                ms, _ = self.cli([exe, "--session", self.session, "--json", "close"],
                                 dict(os.environ), "close")
                self.log("setup", "close", ms, True, "")
            elif self.name == "pw":
                exe = os.environ.get("PLAYWRIGHT_CLI", "playwright-cli")
                ms, _ = self.cli([exe, f"-s={self.session}", "close"],
                                 dict(os.environ), "close", cwd=self.cwd)
                self.log("setup", "close", ms, True, "")
            elif self.name == "pwmcp":
                assert self.mcp
                t0 = time.perf_counter()
                self._mcp_call("browser_close", {}, "close")
                self.log("setup", "close", (time.perf_counter() - t0) * 1000, True, "")
                self.mcp.close()
            elif self.name == "cdmcp":
                assert self.mcp
                t0 = time.perf_counter()
                self.mcp.close()
                self.log("setup", "close", (time.perf_counter() - t0) * 1000, True, "")
            elif self.name == "bu":
                self._bu_stop()
        except Exception as e:
            self.log("setup", "close", None, False, str(e))

    # -- browser-use (harness CLI) session --------------------------------
    def _bu_start(self):
        chrome = self.args.chrome or os.environ["CHROME_BIN"]
        exe = self.args.browser_use or os.environ.get("BROWSER_USE") or "browser-use"
        import socket
        s = socket.socket()
        s.bind(("127.0.0.1", 0))
        self._bu_port = s.getsockname()[1]
        s.close()
        user_data = tempfile.mkdtemp(prefix="buops-")
        self._bu_chrome = subprocess.Popen(
            [chrome, "--headless=new", "--no-sandbox", "--disable-gpu",
             f"--remote-debugging-port={self._bu_port}",
             f"--user-data-dir={user_data}", "about:blank"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            try:
                with urllib.request.urlopen(
                        f"http://127.0.0.1:{self._bu_port}/json/version", timeout=2) as r:
                    if r.status == 200:
                        break
            except Exception:
                time.sleep(0.2)
        else:
            raise RuntimeError("browser-use: throwaway Chrome did not expose CDP in 30s")
        env = dict(os.environ, BU_CDP_URL=f"http://127.0.0.1:{self._bu_port}",
                   BU_NAME=f"ops-{os.getpid()}", BH_TAB_MARKER="0")
        self._bu_env = env
        self._bu_exe = exe
        # First navigation is new_tab (the harness skill's rule), then the
        # daemon keeps the tab across invocations.
        ms, out = self._bu_exec(
            f'new_tab("{self.base}/page_a.html")\nwait_for_load()\n'
            f'print(js("document.title"))\n', "start")
        if self.expected["title_a"] not in out:
            raise RuntimeError(f"browser-use: first navigation did not land: {out[:300]}")
        self.log("setup", "start", ms, True, "browser-use new_tab + attach")

    def _bu_stop(self):
        if self._bu_chrome:
            self._bu_chrome.terminate()
            try:
                self._bu_chrome.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self._bu_chrome.kill()
            self._bu_chrome = None

    def _bu_exec(self, code: str, op: str) -> tuple[float, str]:
        t0 = time.perf_counter()
        p = subprocess.run([self._bu_exe], input=code, capture_output=True,
                           text=True, timeout=120, env=self._bu_env)
        ms = (time.perf_counter() - t0) * 1000.0
        out = (p.stdout + p.stderr).strip()
        if p.returncode != 0:
            self.log("timed" if op in OPS else op, op, ms, False,
                     f"rc={p.returncode}: {out[-800:]}")
            raise RuntimeError(f"browser-use exec rc={p.returncode}: {out[-800:]}")
        self.log("timed" if op in OPS else op, op, ms, True)
        return ms, out

    # -- MCP helpers ---------------------------------------------------------
    def _mcp_goto(self):
        assert self.mcp
        if self.name == "pwmcp":
            self._mcp_call("browser_navigate", {"url": f"{self.base}/page_a.html"}, "start")
        else:
            self._cd_goto(f"{self.base}/page_a.html")

    def _cd_goto(self, url: str):
        """chrome-devtools-mcp navigate, with its --page-id-routing."""
        assert self.mcp
        if "pageId" in self.mcp.schemas.get("navigate_page", {}):
            text = self.mcp.tool("list_pages", {})[1]
            m = re.search(r"^(\d+): .*\[selected\]", text, re.M)
            if m:
                self._cd_page = int(m.group(1))
        args = {"type": "url", "url": url}
        if "pageId" in self.mcp.schemas.get("navigate_page", {}) and getattr(self, "_cd_page", None) is not None:
            args["pageId"] = self._cd_page
        self._mcp_call("navigate_page", args, "start")

    def _mcp_call(self, tool_name: str, tool_args: dict, op: str) -> tuple[float, str]:
        """One timed MCP tools/call (or a setup/verify call when op says so)."""
        assert self.mcp
        try:
            ms, text = self.mcp.tool(tool_name, tool_args)
        except Exception as e:
            self.log("timed" if op in OPS else op, op, None, False, str(e))
            raise
        self.log("timed" if op in OPS else op, op, ms, True)
        return ms, text

    def _mcp_snapshot_text(self, op: str = "verify") -> str:
        assert self.mcp
        if self.name == "pwmcp":
            return self._mcp_call("browser_snapshot", {}, op)[1]
        return self._cd_call("take_snapshot", {}, op)[1]

    def _cd_call(self, tool_name: str, tool_args: dict, op: str = "verify") -> tuple[float, str]:
        """chrome-devtools-mcp tool call, adding pageId when the schema wants it.

        Returns (ms, text), like _mcp_call. The text is scanned for the
        "[selected]" page marker so the page id stays current.
        """
        assert self.mcp
        if "pageId" in self.mcp.schemas.get(tool_name, {}) and getattr(self, "_cd_page", None) is not None:
            tool_args = {**tool_args, "pageId": self._cd_page}
        ms, text = self._mcp_call(tool_name, tool_args, op)
        m = re.search(r"^(\d+): .*\[selected\]", text, re.M)
        if m:
            self._cd_page = int(m.group(1))
        return ms, text

    def _mcp_resolve(self):
        """Resolve the form elements' refs/uids once, from one snapshot.

        The page structure does not change across reps (the filter only
        toggles .hidden on cards; the submit handler only rewrites #status
        text), so refs/uids resolved here stay valid for the session.
        """
        snap = self._mcp_snapshot_text()
        if self.name == "pwmcp":
            if "target" in self.mcp.schemas.get("browser_type", {}):
                return  # selector targets: no snapshot refs needed
            self._refs["q"] = first_textbox_ref(snap)
            self._refs["search"] = ref_for(snap, r'button "Search')
            self._refs["cust"] = ref_for(snap, r'textbox "Name"')
            self._refs["qty"] = ref_for(snap, r'textbox "Qty"')
            self._refs["place"] = ref_for(snap, r'button "Place order"')
        else:
            self._uids["q"] = first_textbox_uid(snap)
            self._uids["search"] = uid_for(snap, r'button "Search')
            self._uids["cust"] = uid_for(snap, r'textbox "Name"')
            self._uids["qty"] = uid_for(snap, r'textbox "Qty"')
            self._uids["place"] = uid_for(snap, r'button "Place order"')

    # Snapshot patterns for the form elements (cdmcp uid / pwmcp ref).
    # #q is the first textbox (no accessible name); the rest match by name.
    MCP_PATTERNS = {"search": r'button "Search', "cust": r'textbox "Name"',
                    "qty": r'textbox "Qty"', "place": r'button "Place order"'}

    def _mcp_resolve_one(self, element: str):
        """Resolve one element's ref/uid from a fresh snapshot (untimed setup).

        chrome-devtools-mcp uids (and Playwright MCP refs on releases without
        selector targets) are per-snapshot: a navigation, or another snapshot,
        invalidates them. Resolving right before the timed call is the tool's
        own workflow (snapshot, then act) — the snapshot itself is setup, not
        part of the timed op.
        """
        snap = self._mcp_snapshot_text("setup")
        if self.name == "pwmcp":
            if element == "q":
                self._refs["q"] = first_textbox_ref(snap)
            else:
                self._refs[element] = ref_for(snap, self.MCP_PATTERNS[element])
        else:
            if element == "q":
                self._uids["q"] = first_textbox_uid(snap)
            else:
                self._uids[element] = uid_for(snap, self.MCP_PATTERNS[element])

    def _mcp_eval(self, expr: str, op: str = "verify"):
        assert self.mcp
        fn = f"() => {expr}"
        if self.name == "pwmcp":
            _, text = self._mcp_call("browser_evaluate", {"function": fn}, op)
        else:
            text = self._cd_call("evaluate_script", {"function": fn}, op)[1]
        return mcp_json(text)

    def _mcp_fill(self, element: str, value: str, op: str) -> float:
        assert self.mcp
        if self.name == "pwmcp":
            if "target" in self.mcp.schemas.get("browser_type", {}):
                ms, _ = self._mcp_call("browser_type",
                                       {"target": f"#{element}", "text": value}, op)
            else:
                self._mcp_resolve_one(element)
                ms, _ = self._mcp_call("browser_type",
                                       {"element": element, "ref": self._refs[element],
                                        "text": value}, op)
        else:
            self._mcp_resolve_one(element)
            ms, _ = self._cd_call("fill", {"uid": self._uids[element], "value": value}, op)
        return ms

    def _mcp_click(self, element: str, op: str) -> float:
        assert self.mcp
        if self.name == "pwmcp":
            if "target" in self.mcp.schemas.get("browser_type", {}):
                ms, _ = self._mcp_call("browser_click", {"target": f"#{element}"}, op)
            else:
                self._mcp_resolve_one(element)
                name = {"search": "Search button", "place": "Place order button"}[element]
                ms, _ = self._mcp_call("browser_click",
                                       {"element": name, "ref": self._refs[element]}, op)
        else:
            self._mcp_resolve_one(element)
            ms, _ = self._cd_call("click", {"uid": self._uids[element]}, op)
        return ms

    # -- untimed reads (verification + setup) --------------------------------
    def eval_value(self, expr: str):
        """Untimed eval returning the value (or raw text for bu).

        Each tool is read in its default output mode, like the timed ops:
        navigera compact prints the bare value, agent-browser prints a JSON
        scalar, playwright-cli prints a "### Result" block.
        """
        if self.name == "nv":
            exe = self.args.navigera or os.environ["NAVIGERA"]
            _, out = self.cli([exe, "--session", self.session, "eval", expr],
                              dict(os.environ), "verify")
            return parse_scalar(out)
        if self.name == "ab":
            exe = os.environ.get("AGENT_BROWSER", "agent-browser")
            _, out = self.cli([exe, "--session", self.session,
                               "eval", expr], dict(os.environ), "verify")
            return parse_scalar(out)
        if self.name == "pw":
            exe = os.environ.get("PLAYWRIGHT_CLI", "playwright-cli")
            _, out = self.cli([exe, f"-s={self.session}", "eval", expr],
                              dict(os.environ), "verify", cwd=self.cwd)
            return parse_pw_result(out)
        if self.name in ("pwmcp", "cdmcp"):
            return self._mcp_eval(expr)
        # bu: js() returns the value; print it. expr is an IIFE
        # "(() => <inner>)()": strip the 7-char prefix "(() => " and the
        # 3-char suffix ")()". Parse the printed line as a scalar, like
        # the other tools' evals: a count prints as "250" but compares
        # against the expected int 250, so the string must become a number.
        _, out = self._bu_exec(f'print(js("{expr[7:-3]}"))\n', "verify")
        return parse_scalar(out.strip().splitlines()[-1]) if out.strip() else ""

    def setup(self, code: str):
        """Untimed state reset between reps (documented setup, not timed)."""
        if self.name in ("pwmcp", "cdmcp"):
            self._mcp_eval(f"(() => {{ {code}; return 1 }})()")
        elif self.name == "bu":
            self._bu_exec(f'js("{code}")\n', "setup")
        else:
            self.eval_value(f"(() => {{ {code}; return 1 }})()")

    # -- the ops ---------------------------------------------------------------
    def op_goto(self, rep: int):
        url = f"{self.base}/page_a.html"
        if self.name == "nv":
            exe = self.args.navigera or os.environ["NAVIGERA"]
            ms, _ = self.cli([exe, "--session", self.session, "goto", url],
                             dict(os.environ), "goto")
        elif self.name == "ab":
            exe = os.environ.get("AGENT_BROWSER", "agent-browser")
            ms, _ = self.cli([exe, "--session", self.session,
                              "open", url], dict(os.environ), "goto")
        elif self.name == "pw":
            exe = os.environ.get("PLAYWRIGHT_CLI", "playwright-cli")
            ms, out = self.cli([exe, f"-s={self.session}", "goto", url],
                               dict(os.environ), "goto", cwd=self.cwd)
        elif self.name in ("pwmcp", "cdmcp"):
            t0 = time.perf_counter()
            if self.name == "pwmcp":
                assert self.mcp
                self.mcp.tool("browser_navigate", {"url": url})
            else:
                self._cd_goto(url)
            ms = (time.perf_counter() - t0) * 1000
        else:  # bu
            # Timed work is the navigation itself; the title check is the
            # shared untimed verification below, like every other tool.
            ms, _ = self._bu_exec(f'goto_url("{url}")\nwait_for_load()\n', "goto")
        title = self.eval_value(TITLE)
        if title != self.expected["title_a"]:
            raise RuntimeError(f"goto verify: title {title!r} != {self.expected['title_a']!r}")
        return ms, 1, ""

    def op_fill(self, rep: int):
        # Same starting state every rep: clear the box first (untimed setup).
        self.setup("document.getElementById('q').value=''")
        if self.name == "nv":
            exe = self.args.navigera or os.environ["NAVIGERA"]
            ms, _ = self.cli([exe, "--session", self.session, "fill",
                              "--selector", "#q", "--value", "widget"],
                             dict(os.environ), "fill")
        elif self.name == "ab":
            exe = os.environ.get("AGENT_BROWSER", "agent-browser")
            ms, _ = self.cli([exe, "--session", self.session,
                              "fill", "#q", "widget"], dict(os.environ), "fill")
        elif self.name == "pw":
            exe = os.environ.get("PLAYWRIGHT_CLI", "playwright-cli")
            ms, out = self.cli([exe, f"-s={self.session}", "fill", "#q", "widget"],
                               dict(os.environ), "fill", cwd=self.cwd)
        elif self.name in ("pwmcp", "cdmcp"):
            ms = self._mcp_fill("q", "widget", "fill")
        else:  # bu
            # Timed work is the fill only; the value read is the shared
            # untimed verification below, like every other tool.
            ms, _ = self._bu_exec('fill_input("#q", "widget")\n', "fill")
        value = self.eval_value(Q_VALUE)
        if value != "widget":
            raise RuntimeError(f"fill verify: value {value!r} != 'widget'")
        return ms, 1, ""

    def op_click(self, rep: int):
        # Same starting state every rep: the box holds "widget" (untimed setup).
        self.setup("document.getElementById('q').value='widget'")
        if self.name == "nv":
            exe = self.args.navigera or os.environ["NAVIGERA"]
            ms, _ = self.cli([exe, "--session", self.session, "click",
                              "--selector", "#search"], dict(os.environ), "click")
        elif self.name == "ab":
            exe = os.environ.get("AGENT_BROWSER", "agent-browser")
            ms, _ = self.cli([exe, "--session", self.session,
                              "click", "#search"], dict(os.environ), "click")
        elif self.name == "pw":
            exe = os.environ.get("PLAYWRIGHT_CLI", "playwright-cli")
            ms, out = self.cli([exe, f"-s={self.session}", "click", "#search"],
                               dict(os.environ), "click", cwd=self.cwd)
        elif self.name in ("pwmcp", "cdmcp"):
            ms = self._mcp_click("search", "click")
        else:  # bu
            # Timed work is the click only; the visible-count read is the
            # shared untimed verification below, like every other tool.
            ms, _ = self._bu_exec(BU_CLICK + 'click_named("Search")\n', "click")
        visible = self.eval_value(VISIBLE)
        if visible != self.expected["visible_after_filter_widget"]:
            raise RuntimeError(
                f"click verify: visible {visible} != {self.expected['visible_after_filter_widget']}")
        return ms, 1, ""

    def op_click_submit(self, rep: int):
        # Same starting state every rep: clear the status + counter (untimed).
        self.setup("document.getElementById('status').textContent=''; window.__placed=0")
        if self.name == "nv":
            exe = self.args.navigera or os.environ["NAVIGERA"]
            ms, _ = self.cli([exe, "--session", self.session, "click",
                              "--selector", "#place"], dict(os.environ), "click-submit")
        elif self.name == "ab":
            exe = os.environ.get("AGENT_BROWSER", "agent-browser")
            ms, _ = self.cli([exe, "--session", self.session,
                              "click", "#place"], dict(os.environ), "click-submit")
        elif self.name == "pw":
            exe = os.environ.get("PLAYWRIGHT_CLI", "playwright-cli")
            ms, out = self.cli([exe, f"-s={self.session}", "click", "#place"],
                               dict(os.environ), "click-submit", cwd=self.cwd)
        elif self.name in ("pwmcp", "cdmcp"):
            ms = self._mcp_click("place", "click-submit")
        else:  # bu
            # Timed work is the click only; the placed/status reads are the
            # shared untimed verification below, like every other tool.
            ms, _ = self._bu_exec(BU_CLICK + 'click_named("Place order")\n',
                                  "click-submit")
        placed = self.eval_value(PLACED)
        status = self.eval_value(STATUS)
        url = self.eval_value(URL)
        if placed != 1:
            raise RuntimeError(f"click-submit verify: __placed {placed} != 1")
        if not isinstance(status, str) or not status.startswith("placed:"):
            raise RuntimeError(f"click-submit verify: status {status!r} has no 'placed:'")
        if not url.startswith(self.base):
            raise RuntimeError(f"click-submit verify: navigation happened: {url}")
        return ms, 1, ""

    def op_read_text(self, rep: int):
        # The timed call's own output IS the value, in each tool's default mode.
        if self.name == "nv":
            exe = self.args.navigera or os.environ["NAVIGERA"]
            ms, out = self.cli([exe, "--session", self.session, "eval", HEADING],
                               dict(os.environ), "read-text")
            value = parse_scalar(out)
        elif self.name == "ab":
            exe = os.environ.get("AGENT_BROWSER", "agent-browser")
            ms, out = self.cli([exe, "--session", self.session,
                                "eval", HEADING], dict(os.environ), "read-text")
            value = parse_scalar(out)
        elif self.name == "pw":
            exe = os.environ.get("PLAYWRIGHT_CLI", "playwright-cli")
            ms, out = self.cli([exe, f"-s={self.session}", "eval", HEADING],
                               dict(os.environ), "read-text", cwd=self.cwd)
            value = parse_pw_result(out)
        elif self.name in ("pwmcp", "cdmcp"):
            t0 = time.perf_counter()
            value = self._mcp_eval(HEADING, "read-text")
            ms = (time.perf_counter() - t0) * 1000
        else:  # bu
            ms, out = self._bu_exec(
                'print(js("document.querySelector(\'h1\').textContent"))\n', "read-text")
            value = out.strip().splitlines()[-1] if out.strip() else ""
        if value != self.expected["heading_a"]:
            raise RuntimeError(f"read-text verify: {value!r} != {self.expected['heading_a']!r}")
        return ms, 1, ""

    def op_snapshot(self, rep: int):
        if self.name == "nv":
            exe = self.args.navigera or os.environ["NAVIGERA"]
            # Default (compact) output: what an agent reads.
            snap_env = {k: v for k, v in os.environ.items() if k != "NAVIGERA_OUTPUT"}
            ms, out = self.cli([exe, "--session", self.session, "ax"],
                               snap_env, "snapshot")
            nbytes = len(out.encode())
        elif self.name == "ab":
            exe = os.environ.get("AGENT_BROWSER", "agent-browser")
            ms, out = self.cli([exe, "--session", self.session, "snapshot"],
                               dict(os.environ), "snapshot")
            nbytes = len(out.encode())
        elif self.name == "pw":
            exe = os.environ.get("PLAYWRIGHT_CLI", "playwright-cli")
            ms, out = self.cli([exe, f"-s={self.session}", "snapshot"],
                               dict(os.environ), "snapshot", cwd=self.cwd)
            nbytes = len(out.encode())
        elif self.name in ("pwmcp", "cdmcp"):
            t0 = time.perf_counter()
            text = self._mcp_snapshot_text("snapshot")
            ms = (time.perf_counter() - t0) * 1000
            nbytes = len(text.encode())
        else:  # bu: no built-in snapshot; the documented raw cdp() route.
            ms, out = self._bu_exec(
                'nodes = cdp("Accessibility.getFullAXTree")["nodes"]\n'
                'def _nm(n):\n    v = n.get("name")\n    return v.get("value") if isinstance(v, dict) else v\n'
                'def _rl(n):\n    v = n.get("role")\n    return v.get("value") if isinstance(v, dict) else v\n'
                'print("\\n".join(f"{_rl(n)} {_nm(n) or \'\'}" for n in nodes))\n',
                "snapshot")
            nbytes = len(out.encode())
        if nbytes < 100:
            raise RuntimeError(f"snapshot verify: only {nbytes} bytes")
        # a size check is not a content check. The snapshot
        # must name the page's known heading and the Search button. For the
        # MCP tools the text is the snapshot text; for the rest it is the
        # timed call's output.
        snap_text = text if self.name in ("pwmcp", "cdmcp") else out
        assert_snapshot_known_elements(snap_text)
        return ms, 1, f"{nbytes} bytes"

    def op_screenshot(self, rep: int):
        shot = self.out_dir / f"ops-shot-{self.tag}-r{rep}.png"
        if self.name == "nv":
            exe = self.args.navigera or os.environ["NAVIGERA"]
            ms, out = self.cli([exe, "--session", self.session, "screenshot",
                                "--path", str(shot)], dict(os.environ), "screenshot")
        elif self.name == "ab":
            exe = os.environ.get("AGENT_BROWSER", "agent-browser")
            ms, out = self.cli([exe, "--session", self.session,
                                "screenshot", str(shot)], dict(os.environ), "screenshot")
        elif self.name == "pw":
            exe = os.environ.get("PLAYWRIGHT_CLI", "playwright-cli")
            # playwright-cli writes the shot into its own outputDir (cwd).
            ms, out = self.cli([exe, f"-s={self.session}", "screenshot",
                                f"--filename={shot.name}"], dict(os.environ),
                               "screenshot", cwd=self.cwd)
            shot = Path(self.cwd) / shot.name
        elif self.name in ("pwmcp", "cdmcp"):
            assert self.mcp
            t0 = time.perf_counter()
            if self.name == "pwmcp":
                self._mcp_call("browser_take_screenshot", {"filename": shot.name}, "screenshot")
                shot = Path(self.cwd) / shot.name
            else:
                self._cd_call("take_screenshot", {"filePath": str(shot.resolve())}, "screenshot")
            ms = (time.perf_counter() - t0) * 1000
        else:  # bu
            ms, out = self._bu_exec(
                f'print(capture_screenshot("{shot}"))\n', "screenshot")
        if not shot.exists() or shot.stat().st_size == 0:
            raise RuntimeError(f"screenshot verify: {shot} missing or empty")
        # existence and nonzero size are not a content
        # check. Decode the PNG for real and assert it is exactly the
        # session's live viewport (times devicePixelRatio), read untimed.
        # The tools do not share one viewport here (run6: navigera
        # 1440x900, agent-browser and playwright-cli 1280x720, browser-use
        # and Chrome DevTools MCP 800x600), so a single hardcoded size
        # would fail honest screenshots; the live viewport is the expected
        # size for the session that took the shot.
        viewport = self.eval_value(
            "(() => window.innerWidth + 'x' + window.innerHeight + '@' "
            "+ window.devicePixelRatio)()")
        m = re.fullmatch(r"(\d+)x(\d+)@([0-9.]+)", str(viewport).strip())
        if not m:
            raise RuntimeError(f"screenshot verify: unreadable viewport {viewport!r}")
        ew = round(int(m.group(1)) * float(m.group(3)))
        eh = round(int(m.group(2)) * float(m.group(3)))
        w, h = check_screenshot_png(shot, ew, eh)
        return ms, 1, f"{shot.stat().st_size} bytes, {w}x{h} == viewport {ew}x{eh}"

    def op_form_batch(self, rep: int):
        # Same starting state every rep: clear the status + counter (untimed).
        self.setup("document.getElementById('status').textContent=''; window.__placed=0")
        if self.name == "nv":
            exe = self.args.navigera or os.environ["NAVIGERA"]
            lines = "\n".join([
                "fill --selector #cust --value ada",
                "fill --selector #qty --value 2",
                "click --selector #place",
                'eval "(() => document.getElementById(\'status\').textContent)()"',
            ]) + "\n"
            # Compact (default) output: a failed step exits nonzero, which
            # run() raises on; the status is verified untimed below.
            ms, _ = self.cli([exe, "--session", self.session, "batch", "-"],
                             dict(os.environ), "form-batch", stdin=lines)
            calls = 1
        elif self.name == "ab":
            exe = os.environ.get("AGENT_BROWSER", "agent-browser")
            # NOTE: batch splits each command string into tokens and strips
            # quotes, so an eval with a quoted string breaks inside batch
            # (getElementById('status') becomes getElementById(status) ->
            # null). The 4th step therefore reads the status with the tool's
            # own `get text` command, which takes no quotes.
            ms, out = self.cli(
                [exe, "--session", self.session, "batch", "--bail",
                 "fill #cust ada", "fill #qty 2", "click #place",
                 "get text #status"],
                dict(os.environ), "form-batch")
            if "\u2717" in out:
                raise RuntimeError(f"agent-browser batch step failed: {out[-500:]}")
            calls = 1
        else:
            # No batch command: the same 4 steps as 4 calls; the cell times
            # the whole form, and records the call count.
            total = 0.0
            if self.name == "pw":
                exe = os.environ.get("PLAYWRIGHT_CLI", "playwright-cli")
                for argv in ([exe, f"-s={self.session}", "fill", "#cust", "ada"],
                            [exe, f"-s={self.session}", "fill", "#qty", "2"],
                            [exe, f"-s={self.session}", "click", "#place"]):
                    m, _ = self.cli(argv, dict(os.environ), "form-batch", cwd=self.cwd)
                    total += m
                m, out = self.cli([exe, f"-s={self.session}", "eval", STATUS],
                                  dict(os.environ), "form-batch", cwd=self.cwd)
                total += m
            elif self.name in ("pwmcp", "cdmcp"):
                total += self._mcp_fill("cust", "ada", "form-batch")
                total += self._mcp_fill("qty", "2", "form-batch")
                total += self._mcp_click("place", "form-batch")
                t0 = time.perf_counter()
                self._mcp_eval(STATUS, "form-batch")
                total += (time.perf_counter() - t0) * 1000
            else:  # bu: one exec block = the batch, by the tool's own model
                # The 4th step prints the status, like the other tools' timed
                # status eval; it is not checked here — the shared untimed
                # verification below does the checking, like every other tool.
                total, _ = self._bu_exec(
                    'fill_input("#cust", "ada")\nfill_input("#qty", "2")\n'
                    + BU_CLICK + 'click_named("Place order")\n'
                    'print(js("document.getElementById(\'status\').textContent"))\n',
                    "form-batch")
            # Four calls for pw/MCP; one exec block for bu.
            ms, calls = total, 1 if self.name == "bu" else 4
        status = self.eval_value(STATUS)
        if status != self.expected["status_after_batch"]:
            raise RuntimeError(
                f"form-batch verify: status {status!r} != {self.expected['status_after_batch']!r}")
        return ms, calls, ""

    OP_METHODS = {
        "goto": op_goto, "fill": op_fill, "click": op_click,
        "click-submit": op_click_submit, "read-text": op_read_text,
        "snapshot": op_snapshot, "screenshot": op_screenshot,
        "form-batch": op_form_batch,
    }

    def run_op(self, op: str, reps: int) -> dict:
        method = self.OP_METHODS[op]
        runs: list[float] = []
        errors: list[str] = []
        calls = 1
        extra: dict = {}
        for rep in range(reps):
            try:
                ms, calls, detail = method(self, rep)
                runs.append(ms)
                if detail:
                    extra["detail"] = detail
            except Exception as e:
                errors.append(f"rep {rep}: {' '.join(str(e).split())[:300]}")
                print(f"  [ops] {self.name} {op} rep {rep + 1}/{reps}: FAILED ({e})",
                      flush=True)
        ok = len(runs) == reps and not errors
        print(f"  [ops] {self.name} {op}: "
              + (f"median {median(runs):.1f} ms over {len(runs)} reps, {calls} call(s)"
                 if runs else "no passing reps"), flush=True)
        return {"runs_ms": [round(x, 2) for x in runs],
                "median_ms": round(median(runs), 2) if runs else None,
                "calls": calls, "ok": ok,
                **({"errors": errors} if errors else {}),
                **extra}


def first_textbox_ref(snapshot: str) -> str:
    """Ref of the first textbox in a Playwright MCP snapshot (#q: no name)."""
    for line in snapshot.splitlines():
        m = re.search(r'- textbox(?: "([^"]*)")? \[ref=([^\]]+)\]', line)
        if m and m.group(1) is None:
            return m.group(2)
    raise RuntimeError("no unnamed textbox (#q) in snapshot")


def first_textbox_uid(snapshot: str) -> str:
    """Uid of the first textbox line in a chrome-devtools-mcp snapshot (#q)."""
    for line in snapshot.splitlines():
        if re.search(r"textbox", line):
            m = re.search(r"uid=(\S+)", line)
            if m:
                return m.group(1)
    raise RuntimeError("no textbox uid (#q) in snapshot")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--tool", choices=["nv", "ab", "pw", "pwmcp", "cdmcp", "bu"],
                    required=True)
    ap.add_argument("--reps", type=int, default=10)
    ap.add_argument("--out", required=True)
    ap.add_argument("--session", default=f"ops-{os.getpid()}")
    ap.add_argument("--tag", default="",
                    help="transcript/screenshot tag; defaults to --tool. The "
                         "ladder passes a per-label slug so two tools sharing "
                         "a --tool code (navigera vs its baseline) do not "
                         "overwrite each other's transcript in one round dir.")
    ap.add_argument("--navigera", default="")
    ap.add_argument("--chrome", default="")
    ap.add_argument("--browser-use", default="")
    ap.add_argument("--own-browser", action="store_true")
    args = ap.parse_args()

    base = os.environ["LADDER_BASE"]
    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    with open(FIXTURES / "expected.json", encoding="utf-8") as f:
        expected = json.load(f)

    preflight(base, expected)
    tool = Tool(args, base, expected, out_dir)
    results: dict[str, dict] = {}
    error: str | None = None
    try:
        tool.start()
        for op in OPS:
            results[op] = tool.run_op(op, args.reps)
    except Exception as e:
        error = " ".join(str(e).split())
        print(f"[ops] {args.tool}: ABORTED: {error}", flush=True)
    finally:
        tool.close()
    doc = {"tool": args.tool, "tag": tool.tag, "reps": args.reps, "ops": results,
           "calls": tool.calls, **({"error": error} if error else {})}
    (out_dir / f"ops-{tool.tag}.json").write_text(json.dumps(doc, indent=2) + "\n",
                                                encoding="utf-8")
    print(json.dumps({"tool": args.tool, "reps": args.reps,
                      "ops": {op: {"median_ms": r.get("median_ms"), "calls": r["calls"],
                                   "ok": r["ok"]} for op, r in results.items()},
                      **({"error": error} if error else {})}))
    return 0 if not error else 1


if __name__ == "__main__":
    sys.exit(main())
