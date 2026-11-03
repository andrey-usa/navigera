#!/usr/bin/env python3
"""LLM-in-the-loop browser tool bench, v2.

One model drives one browser tool through one function
(`run_shell_command`) on a local site, and each session records
success, model turns, prompt/completion/cached tokens, USD cost and
wall time.

What v2 changes, and why:

- The site is served by this driver, not from files: there is nothing
  on disk for a page to be missing from, and the server records what
  a user's action did (a search run, an order placed). The driver
  checks every page before the first session and stops if one fails.
- Success is judged from the site's recorded state and the model's
  final answer, never from a string appearing in some tool output.
  The table's row count and last value and the catalog's size are
  drawn per session, so no file anywhere holds an answer.
- CLI arms run without a shell, from an empty directory, with no
  secrets in their environment: the first word must be the arm's
  tool, one command per call (a heredoc only for navigera `batch`).
  MCP arms accept only `mcp <tool> <json-args>` lines, as before.
- Every arm starts with its browser open on the home page.

Arms:
  nav-default          navigera CLI (compact output, its default)
  nav-batch            navigera CLI; a whole flow may go in one `batch`
  agent-browser        agent-browser CLI
  playwright-cli       playwright-cli CLI (-s session)
  playwright-mcp       Playwright MCP server, `mcp <tool> <json>` lines
  chrome-devtools-mcp  Chrome DevTools MCP server, same lines

Provider: SiliconFlow's OpenAI-compatible API, DeepSeek-V4.1-Flash;
key from SILICONFLOW_API_KEY (an Actions secret, never in a file).
List prices $0.15/M input, $0.003/M cached input, $0.60/M output.
One Budget caps total spend for the run.

Usage: python3 bench/llm/llm_bench.py --arms nav-default --reps 3 --out results
Env: SILICONFLOW_API_KEY, NAV_BIN, TOOLS_DIR, CHROME_PATH.
"""
import argparse
import glob
import json
import os
import random
import shlex
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
import zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

API = "https://api.siliconflow.com/v1/chat/completions"
MODEL_DEFAULT = "deepseek-ai/DeepSeek-V4.1-Flash"
PRICE_IN, PRICE_IN_CACHED, PRICE_OUT = 0.15, 0.003, 0.60  # USD / 1M
MAX_TOKENS = 1024
TOOL_DEF_OVERHEAD_TOKENS = 250
MIN_RESERVE_USD = 0.005
MAX_TURNS = 40
CMD_TIMEOUT = 120
SESSION_TIMEOUT = 1200
OUTPUT_TAIL = 12000
REPEAT_LIMIT = 3
TRANSCRIPT_ARG_MAX = 300
TRANSCRIPT_RESULT_MAX = 500
PORT = 8901
BASE = f"http://127.0.0.1:{PORT}"

ALL_ARMS = ["nav-default", "nav-batch", "agent-browser", "playwright-cli",
            "playwright-mcp", "chrome-devtools-mcp"]
ALL_TASKS = ["search", "form", "table", "catalog"]
CLI_TOOL = {"nav-default": "navigera", "nav-batch": "navigera",
            "agent-browser": "agent-browser",
            "playwright-cli": "playwright-cli"}


# ---------------------------------------------------------------- site

class Site:
    """Per-session page data and the record of what was done."""

    def __init__(self):
        self.lock = threading.Lock()
        self.reset(0)

    def reset(self, seed):
        rng = random.Random(seed)
        with self.lock:
            self.rows = rng.randint(14, 29)
            self.prices = [rng.randint(3, 97) * 10 for _ in range(self.rows)]
            self.items = rng.randint(23, 38)
            self.names = [f"{rng.choice(ADJ)} {rng.choice(NOUN)} {i + 1}"
                          for i in range(self.items)]
            self.events = []

    def record(self, kind, **data):
        with self.lock:
            self.events.append({"kind": kind, **data})

    def saw(self, kind, pred):
        with self.lock:
            return any(e["kind"] == kind and pred(e) for e in self.events)


ADJ = ["Red", "Blue", "Quiet", "Swift", "Bright", "Small", "Large", "Smart"]
NOUN = ["Lamp", "Kettle", "Chair", "Desk", "Fan", "Clock", "Mug", "Shelf"]
PAGE = 10

HOME = """<!doctype html><html><head><title>Bench Home</title></head><body>
<h1>Bench Home</h1><p>Welcome to the local shop.</p>
<nav><a href="/form.html">Order form</a> | <a href="/table.html">Price table</a>
 | <a href="/catalog.html">Catalog</a></nav>
<label>Search products <input id="search"></label>
<button id="go">Search</button><p id="out" role="status"></p>
<script>
document.getElementById('go').onclick = async () => {
  const q = document.getElementById('search').value;
  document.getElementById('out').textContent = 'Searching...';
  const r = await fetch('/api/search?q=' + encodeURIComponent(q));
  const j = await r.json();
  document.getElementById('out').textContent = 'Results for: ' + j.q + ' (' + j.n + ' products)';
};
</script></body></html>"""

FORM = """<!doctype html><html><head><title>Bench Form</title></head><body>
<h1>Order form</h1>
<form id="order"><label>Name <input id="name" name="name"></label><br>
<label>City <select id="city" name="city"><option>Paris</option><option>Berlin</option>
<option>Tokyo</option><option>Lima</option></select></label><br>
<label><input type="checkbox" id="agree" name="agree"> I agree to the terms</label><br>
<button type="submit">Submit order</button></form><p id="done" role="status"></p>
<script>
document.getElementById('order').onsubmit = async e => {
  e.preventDefault();
  const body = JSON.stringify({name: document.getElementById('name').value,
    city: document.getElementById('city').value,
    agree: document.getElementById('agree').checked});
  const r = await fetch('/api/order', {method: 'POST', body});
  const j = await r.json();
  document.getElementById('done').textContent = j.ok
    ? 'Order placed for ' + j.name + ' in ' + j.city
    : 'Error: ' + j.error;
};
</script></body></html>"""

CATALOG = """<!doctype html><html><head><title>Catalog</title></head><body>
<h1>Catalog</h1><ul id="items"></ul>
<button id="more">Load more</button>
<script>
let page = 0;
async function load() {
  const b = document.getElementById('more');
  b.disabled = true; b.textContent = 'Loading...';
  const r = await fetch('/api/items?page=' + page);
  const j = await r.json();
  for (const n of j.items) {
    const li = document.createElement('li'); li.textContent = n;
    document.getElementById('items').appendChild(li);
  }
  page += 1;
  if (j.more) { b.disabled = false; b.textContent = 'Load more'; }
  else { b.remove(); }
}
document.getElementById('more').onclick = load;
load();
</script></body></html>"""


def table_page(site):
    rows = "".join(f"<tr><td>Item {i + 1}</td><td>{p}</td></tr>"
                   for i, p in enumerate(site.prices))
    return ("<!doctype html><html><head><title>Bench Table</title></head>"
            "<body><h1>Price table</h1><table id=prices><thead><tr>"
            "<th>Item</th><th>Price</th></tr></thead><tbody>"
            f"{rows}</tbody></table></body></html>")


def make_handler(site):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def send(self, code, body, ctype="text/html; charset=utf-8"):
            data = body.encode()
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(data)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            try:
                self.wfile.write(data)
            except OSError:
                pass  # the page navigated away mid-request

        def do_GET(self):
            u = urlparse(self.path)
            q = parse_qs(u.query)
            if u.path in ("/", "/index.html"):
                return self.send(200, HOME)
            if u.path == "/form.html":
                return self.send(200, FORM)
            if u.path == "/table.html":
                return self.send(200, table_page(site))
            if u.path == "/catalog.html":
                return self.send(200, CATALOG)
            if u.path == "/api/search":
                term = q.get("q", [""])[0].strip()
                time.sleep(0.3)
                site.record("search", q=term)
                return self.send(200, json.dumps({"q": term, "n": 3}),
                                 "application/json")
            if u.path == "/api/items":
                page = int(q.get("page", ["0"])[0] or 0)
                time.sleep(0.4)
                chunk = site.names[page * PAGE:(page + 1) * PAGE]
                site.record("items", page=page)
                more = (page + 1) * PAGE < len(site.names)
                return self.send(200, json.dumps({"items": chunk,
                                                  "more": more}),
                                 "application/json")
            return self.send(404, "<h1>Not found</h1>")

        def do_POST(self):
            u = urlparse(self.path)
            n = int(self.headers.get("Content-Length") or 0)
            raw = self.rfile.read(n).decode(errors="replace")
            if u.path == "/api/order":
                try:
                    d = json.loads(raw)
                except Exception:
                    d = {}
                name = str(d.get("name", "")).strip()
                city = str(d.get("city", "")).strip()
                agree = bool(d.get("agree"))
                if not agree:
                    return self.send(200, json.dumps(
                        {"ok": False, "error": "accept the terms"}),
                        "application/json")
                site.record("order", name=name, city=city)
                return self.send(200, json.dumps(
                    {"ok": True, "name": name, "city": city}),
                    "application/json")
            return self.send(404, "{}", "application/json")
    return Handler


def task_def(task, site):
    """(goal, judge): judge(final_answer) -> bool, from the site's state."""
    if task == "search":
        return ("On the home page, search for 'headphones' and report the "
                "results line exactly as the page shows it.",
                lambda a: site.saw("search",
                                   lambda e: e["q"].lower() == "headphones")
                and "results for: headphones" in a.lower())
    if task == "form":
        return ("On the order form, place an order for name Alex, city "
                "Tokyo, with the terms accepted, and report the "
                "confirmation line.",
                lambda a: site.saw("order", lambda e: e["name"] == "Alex"
                                   and e["city"] == "Tokyo")
                and "order placed for alex in tokyo" in a.lower())
    if task == "table":
        want = f"{site.rows} rows, last={site.prices[-1]}"
        return ("On the price table, report how many rows the table body "
                "has and the last row's price, in the form "
                "'N rows, last=P'.",
                lambda a: want in a.replace("  ", " "))
    if task == "catalog":
        n, last = len(site.names), site.names[-1]
        return ("On the catalog page, load every item (the list loads in "
                "pages), then report how many items there are in total "
                "and the name of the last item, in the form "
                "'N items, last=NAME'.",
                lambda a: f"{n} items" in a and last.lower() in a.lower())
    raise ValueError(task)


def check_site():
    """Every page answers 200 with its marker, and the order API
    records: a broken site stops the run before any model call."""
    marks = {"/": "Bench Home", "/form.html": "Order form",
             "/table.html": "Price table", "/catalog.html": "Catalog"}
    for path, mark in marks.items():
        with urllib.request.urlopen(BASE + path, timeout=10) as r:
            body = r.read().decode()
            if r.status != 200 or mark not in body:
                raise SystemExit(f"site check failed: {path}")
    req = urllib.request.Request(BASE + "/api/order", method="POST",
                                 data=b'{"name":"x","city":"y","agree":true}')
    with urllib.request.urlopen(req, timeout=10) as r:
        if not json.loads(r.read()).get("ok"):
            raise SystemExit("site check failed: /api/order")


# ---------------------------------------------------------------- model

class BudgetStop(RuntimeError):
    pass


class Budget:
    def __init__(self, total):
        self.total, self.spent, self.reserved = total, 0.0, 0.0

    def reserve(self, amount):
        if self.spent + self.reserved + amount > self.total:
            raise BudgetStop(f"budget: spent {self.spent:.4f} + "
                             f"{amount:.4f} > {self.total}")
        self.reserved += amount

    def settle(self, amount, cost):
        self.reserved = max(0.0, self.reserved - amount)
        self.spent += cost


def reserve_for(messages):
    n = TOOL_DEF_OVERHEAD_TOKENS
    for m in messages:
        c = m.get("content") or ""
        n += len(c) // 4 + 8
        for call in (m.get("tool_calls") or []):
            n += len(json.dumps(call)) // 4 + 8
    return max(MIN_RESERVE_USD, (n * PRICE_IN + MAX_TOKENS * PRICE_OUT) / 1e6)


TOOLS = [{"type": "function", "function": {
    "name": "run_shell_command",
    "description": "Run one command and return its output.",
    "parameters": {"type": "object",
                   "properties": {"command": {"type": "string"}},
                   "required": ["command"]}}}]


def chat(key, model, messages, budget):
    amount = reserve_for(messages)
    budget.reserve(amount)
    body = json.dumps({"model": model, "messages": messages,
                       "temperature": 0, "max_tokens": MAX_TOKENS,
                       "tools": TOOLS})
    req = urllib.request.Request(API, data=body.encode(), headers={
        "Authorization": f"Bearer {key}",
        "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=120) as r:
            resp = json.loads(r.read())
    except Exception as e:
        budget.settle(amount, 0.0)
        raise RuntimeError(f"model request failed: {e}")
    u = resp.get("usage", {})
    p, c = u.get("prompt_tokens", 0), u.get("completion_tokens", 0)
    cached = (u.get("prompt_tokens_details") or {}).get("cached_tokens", 0)
    cost = ((p - cached) * PRICE_IN + cached * PRICE_IN_CACHED
            + c * PRICE_OUT) / 1e6
    budget.settle(amount, cost)
    return resp["choices"][0]["message"], {"prompt": p, "completion": c,
                                           "cached": cached, "cost": cost}


# ---------------------------------------------------------------- tools

def chrome_path():
    env = os.environ.get("CHROME_PATH")
    if env:
        return env
    hits = sorted(glob.glob(os.path.expanduser(
        "~/.cache/ms-playwright/chromium-*/chrome-linux64/chrome")))
    return hits[-1] if hits else "chrome"


def tool_env():
    """What a tool's process sees: no API key, no runner tokens."""
    keep = ("PATH", "HOME", "LANG", "TMPDIR", "CHROME_BIN",
            "PLAYWRIGHT_BROWSERS_PATH", "XDG_RUNTIME_DIR")
    env = {k: v for k, v in os.environ.items() if k in keep}
    env["CHROME_PATH"] = chrome_path()
    return env


class McpServer:
    def __init__(self, cmd, env_extra):
        env = tool_env()
        env.update(env_extra)
        self.p = subprocess.Popen(cmd, stdin=subprocess.PIPE,
                                  stdout=subprocess.PIPE, text=True,
                                  bufsize=1, env=env)
        self.rid = 0
        self.rpc("initialize", {"protocolVersion": "2024-11-05",
                                "capabilities": {},
                                "clientInfo": {"name": "llm-bench",
                                               "version": "2"}})
        self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        tools = self.rpc("tools/list", {})
        self.tool_names = [t.get("name") for t in
                           tools.get("result", {}).get("tools", [])]

    def _send(self, obj):
        self.p.stdin.write(json.dumps(obj) + "\n")
        self.p.stdin.flush()

    def rpc(self, method, params):
        self.rid += 1
        rid = self.rid
        self._send({"jsonrpc": "2.0", "id": rid, "method": method,
                    "params": params})
        while True:
            line = self.p.stdout.readline()
            if not line:
                return {}
            d = json.loads(line)
            if d.get("id") == rid:
                return d

    def call(self, name, args):
        return self.rpc("tools/call", {"name": name, "arguments": args})

    def close(self):
        try:
            self.p.stdin.close()
            self.p.wait(timeout=10)
        except Exception:
            self.p.kill()


def bins(tools_dir, nav_bin):
    nm = os.path.join(tools_dir, "node_modules", ".bin")
    return {"navigera": nav_bin,
            "agent-browser": os.path.join(nm, "agent-browser"),
            "playwright-cli": os.path.join(nm, "playwright-cli")}


def vocab(arm, sess):
    """The arm's prompt block: the same shape and length for every tool."""
    if arm.startswith("nav-"):
        p = f"navigera -s {sess}"
        extra = ""
        if arm == "nav-batch":
            extra = (f" Several steps may go in ONE call: `{p} batch "
                     f"<<'EOF'` with one navigera command per line, then "
                     f"EOF; it stops at the first failing line.")
        return (f"Browser tool: the navigera CLI. Commands: `{p} goto "
                f"<url>`, `{p} ax` (the page as a tree with [ref=N]), "
                f"`{p} click <ref>`, `{p} fill <ref> <text>`, `{p} select "
                f"<ref> <value>`, `{p} check <ref>`, `{p} press <key>`, "
                f"`{p} text --selector <css>`, `{p} eval <js>`; "
                f"`navigera --help` lists more.{extra}")
    if arm == "agent-browser":
        p = "agent-browser"
        return (f"Browser tool: the agent-browser CLI. Commands: `{p} open "
                f"<url>`, `{p} snapshot` (the page as a tree with refs), "
                f"`{p} click @<ref>`, `{p} fill @<ref> <text>`, `{p} select "
                f"@<ref> <value>`, `{p} check @<ref>`, `{p} press <key>`, "
                f"`{p} get text <selector>`, `{p} eval <js>`; "
                f"`agent-browser --help` lists more.")
    if arm == "playwright-cli":
        p = f"playwright-cli -s {sess}"
        return (f"Browser tool: the playwright-cli CLI. Commands: `{p} goto "
                f"<url>`, `{p} snapshot` (the page as a tree with refs), "
                f"`{p} click <ref>`, `{p} fill <ref> <text>`, `{p} select "
                f"<ref> <value>`, `{p} check <ref>`, `{p} press <key>`, "
                f"`{p} eval <js>`; `playwright-cli --help` lists more.")
    if arm == "playwright-mcp":
        return ("Browser tool: the Playwright MCP server (running). Send "
                "`mcp <tool> <json-args>` lines, e.g. `mcp browser_snapshot "
                "{}`, `mcp browser_click {\"element\": \"...\", \"ref\": "
                "\"...\"}`. Only `mcp` lines are accepted.")
    if arm == "chrome-devtools-mcp":
        return ("Browser tool: the Chrome DevTools MCP server (running). "
                "Send `mcp <tool> <json-args>` lines, e.g. `mcp "
                "take_snapshot {}`, `mcp click {\"uid\": \"...\"}`. Only "
                "`mcp` lines are accepted.")
    raise ValueError(arm)


def split_heredoc(command):
    """`cmd <<'EOF'\\nbody\\nEOF` -> (cmd, body); else (command, None)."""
    first, nl, rest = command.partition("\n")
    if "<<" not in first:
        return command, None
    head, _, marker = first.partition("<<")
    marker = marker.strip().strip("'\"")
    if not marker or not nl:
        return command, None
    lines = rest.split("\n")
    body = []
    for line in lines:
        if line.strip() == marker:
            break
        body.append(line)
    return head.strip(), "\n".join(body) + "\n"


def gate_cli(arm, command, sess, b):
    """argv for an allowed command, or an error string."""
    tool = CLI_TOOL[arm]
    cmd, stdin = split_heredoc(command.strip())
    if stdin is not None and arm != "nav-batch":
        return None, None, "error: heredocs are only for navigera batch"
    lex = shlex.shlex(cmd, posix=True, punctuation_chars=True)
    lex.whitespace_split = True
    try:
        argv = list(lex)
    except ValueError as e:
        return None, None, f"error: cannot parse the command: {e}"
    if not argv:
        return None, None, "error: empty command"
    if any(t and all(ch in "();<>|&" for ch in t) for t in argv):
        return None, None, (f"error: one `{tool}` command per call: no "
                            f"pipes, redirects, `&&` or `;`")
    if os.path.basename(argv[0]) != tool:
        return None, None, (f"error: this arm runs only `{tool}` "
                            f"commands")
    argv[0] = b[tool]
    return argv, stdin, None


def run_cli(argv, stdin, cwd):
    try:
        r = subprocess.run(argv, input=stdin, capture_output=True,
                           stdin=None if stdin is not None
                           else subprocess.DEVNULL,
                           text=True, timeout=CMD_TIMEOUT, cwd=cwd,
                           env=tool_env())
        return (r.stdout + r.stderr)[-OUTPUT_TAIL:] or "(no output)"
    except subprocess.TimeoutExpired:
        return "error: command timed out"
    except OSError as e:
        return f"error: {e}"


def setup_arm(arm, sess, b, cwd):
    """Start the tool with its browser open on the home page."""
    home = BASE + "/"
    if arm.startswith("nav-"):
        run_cli([b["navigera"], "-s", sess, "start", "--headless"], None, cwd)
        run_cli([b["navigera"], "-s", sess, "goto", home], None, cwd)
        return None
    if arm == "agent-browser":
        run_cli([b["agent-browser"], "open", home], None, cwd)
        return None
    if arm == "playwright-cli":
        run_cli([b["playwright-cli"], "-s", sess, "open", home], None, cwd)
        return None
    tools_dir = os.environ.get("TOOLS_DIR", "tools-npm")
    if arm == "playwright-mcp":
        cfg = {"browser": {"browserName": "chromium", "launchOptions": {
            "executablePath": chrome_path(), "chromiumSandbox": False,
            "args": ["--no-sandbox"]}}}
        cfg_path = os.path.join(cwd, "mcp-pw.config.json")
        with open(cfg_path, "w") as f:
            json.dump(cfg, f)
        mcp = McpServer(["node", os.path.join(
            tools_dir, "node_modules", "@playwright", "mcp", "cli.js"),
            "--headless", "--config", cfg_path], {})
        mcp.call("browser_navigate", {"url": home})
        return mcp
    if arm == "chrome-devtools-mcp":
        mcp = McpServer(["node", os.path.join(
            tools_dir, "node_modules", "chrome-devtools-mcp", "build", "src",
            "bin", "chrome-devtools-mcp.js"), "--headless",
            "--no-usage-statistics", "--isolated",
            "--executablePath", chrome_path(),
            "--chrome-arg=--no-sandbox",
            "--chrome-arg=--disable-setuid-sandbox"], {})
        mcp.call("list_pages", {})
        mcp.call("navigate_page", {"url": home})
        return mcp
    raise ValueError(arm)


def cleanup(arm, sess, b, cwd, mcp):
    if mcp is not None:
        mcp.close()
        return
    if arm.startswith("nav-"):
        run_cli([b["navigera"], "-s", sess, "quit"], None, cwd)
    elif arm == "agent-browser":
        run_cli([b["agent-browser"], "close"], None, cwd)
    elif arm == "playwright-cli":
        run_cli([b["playwright-cli"], "-s", sess, "close"], None, cwd)


def exec_command(command, arm, sess, b, cwd, mcp):
    if mcp is not None:
        s = command.strip()
        if not s.startswith("mcp "):
            return "error: this arm accepts only `mcp <tool> <json-args>` lines"
        name, _, argstr = s[4:].strip().partition(" ")
        try:
            args = json.loads(argstr) if argstr.strip() else {}
        except Exception as e:
            return f"error: bad json args: {e}"
        try:
            d = mcp.call(name, args)
        except Exception as e:
            return f"error: mcp call failed: {e}"
        res = d.get("result") or d.get("error") or {}
        parts = [c.get("text", "") for c in res.get("content", [])
                 if c.get("type") == "text"]
        text = "\n".join(parts) if parts else json.dumps(res)
        if res.get("isError"):
            text = "error: " + text
        return text[-OUTPUT_TAIL:]
    argv, stdin, err = gate_cli(arm, command, sess, b)
    if err:
        return err
    return run_cli(argv, stdin, cwd)


# ---------------------------------------------------------------- loop

def run_session(key, model, arm, task, rep, b, site, budget, out_dir):
    sess = f"{arm}-{task}-{rep}"
    seed = zlib.crc32(f"{task}-{rep}".encode())  # same page data for every arm
    site.reset(seed)
    goal, judge = task_def(task, site)
    cwd = tempfile.mkdtemp(prefix="llmb-")
    mcp = None
    t0 = time.monotonic()
    turns = prompt_t = completion_t = cached_t = 0
    cost = 0.0
    success = False
    answer = ""
    stop = "max-turns"
    transcript = []
    try:
        mcp = setup_arm(arm, sess, b, cwd)
        block = vocab(arm, sess)
        if mcp is not None:
            block += " Tools: " + ", ".join(mcp.tool_names) + "."
        prompt = (f"Task: {goal} Site: {BASE}/ (home), {BASE}/form.html, "
                  f"{BASE}/table.html, {BASE}/catalog.html. The browser is "
                  f"already open on the home page. {block} Use only "
                  f"run_shell_command, one browser command per call. When "
                  f"you have the answer, reply `DONE: <answer>` and stop.")
        messages = [{"role": "user", "content": prompt}]
        last = []
        while turns < MAX_TURNS:
            if time.monotonic() - t0 > SESSION_TIMEOUT:
                stop = "timeout"
                break
            msg, u = chat(key, model, messages, budget)
            turns += 1
            prompt_t += u["prompt"]
            completion_t += u["completion"]
            cached_t += u["cached"]
            cost += u["cost"]
            messages.append(msg)
            calls = msg.get("tool_calls") or []
            if not calls:
                text = (msg.get("content") or "").strip()
                if "DONE" in text:
                    answer = text[text.index("DONE"):]
                    success = bool(judge(answer))
                    stop = "answered"
                    transcript.append(f"T{turns:02d} answer -> {answer[:300]}")
                    break
                messages.append({"role": "user", "content":
                                 "Continue with run_shell_command, or "
                                 "reply `DONE: <answer>`."})
                continue
            for call in calls:
                try:
                    args = json.loads(call["function"].get("arguments") or "{}")
                except Exception:
                    args = {}
                command = str(args.get("command", ""))
                out = exec_command(command, arm, sess, b, cwd, mcp)
                transcript.append(
                    f"T{turns:02d} "
                    + " ".join(command.split())[:TRANSCRIPT_ARG_MAX]
                    + " -> "
                    + " ".join(out.split())[:TRANSCRIPT_RESULT_MAX])
                messages.append({"role": "tool", "tool_call_id": call["id"],
                                 "content": out})
                last.append(command)
                if len(last) >= REPEAT_LIMIT and \
                        len(set(last[-REPEAT_LIMIT:])) == 1:
                    messages.append({"role": "user", "content":
                                     "That command ran three times the "
                                     "same way; try something else."})
    except BudgetStop as e:
        stop = f"budget ({e})"
    except Exception as e:
        stop = f"error ({e})"
    finally:
        try:
            cleanup(arm, sess, b, cwd, mcp)
        finally:
            shutil.rmtree(cwd, ignore_errors=True)
    wall_ms = round((time.monotonic() - t0) * 1000)
    rec = {"arm": arm, "task": task, "rep": rep, "success": success,
           "turns": turns, "prompt_tokens": prompt_t,
           "completion_tokens": completion_t, "cached_tokens": cached_t,
           "cost_usd": round(cost, 6), "wall_ms": wall_ms, "stop": stop}
    with open(os.path.join(out_dir, f"{sess}.json"), "w") as f:
        json.dump(rec, f, indent=2)
    line = (f"RESULT arm={arm} task={task} rep={rep} success={int(success)} "
            f"turns={turns} tokens={prompt_t + completion_t} "
            f"cached={cached_t} cost_usd={cost:.6f} wall_ms={wall_ms} "
            f"stop={stop.split(' ')[0]}")
    print(line, flush=True)
    print(f"::notice::{line}", flush=True)
    for t in transcript:
        print(f"  {t}", flush=True)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a") as f:
            f.write(f"\n#### {arm} / {task} / rep {rep}: success="
                    f"{int(success)}, turns={turns}, stop={stop}\n\n```\n")
            f.write("\n".join(transcript) + "\n```\n")
    return rec


def med(xs):
    xs = sorted(xs)
    return xs[len(xs) // 2] if xs else 0


def table(records, arms, tasks):
    out = ["| arm | task | success | median turns | median tokens | "
           "median $ | median wall s |", "|---|---|---|---|---|---|---|"]
    for arm in arms:
        for task in tasks:
            rs = [r for r in records if r["arm"] == arm and r["task"] == task]
            if not rs:
                continue
            out.append(
                f"| {arm} | {task} | {sum(r['success'] for r in rs)}/"
                f"{len(rs)} | {med([r['turns'] for r in rs])} | "
                f"{med([r['prompt_tokens'] + r['completion_tokens'] for r in rs])} | "
                f"{med([r['cost_usd'] for r in rs]):.5f} | "
                f"{med([r['wall_ms'] for r in rs]) / 1000:.1f} |")
    return "\n".join(out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--arms", default=",".join(ALL_ARMS))
    ap.add_argument("--tasks", default=",".join(ALL_TASKS))
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--model", default=MODEL_DEFAULT)
    ap.add_argument("--out", default="results")
    ap.add_argument("--budget", type=float, default=1.00)
    ap.add_argument("--site-only", action="store_true",
                    help="serve the site and check it, then exit")
    args = ap.parse_args()
    site = Site()
    server = ThreadingHTTPServer(("127.0.0.1", PORT), make_handler(site))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    check_site()
    print("site check: ok", flush=True)
    if args.site_only:
        return 0
    key = os.environ.get("SILICONFLOW_API_KEY")
    if not key:
        print("SILICONFLOW_API_KEY is not set", file=sys.stderr)
        return 2
    b = bins(os.environ.get("TOOLS_DIR", "tools-npm"),
             os.environ.get("NAV_BIN", "navigera"))
    os.makedirs(args.out, exist_ok=True)
    arms = [a for a in args.arms.split(",") if a]
    tasks = [t for t in args.tasks.split(",") if t]
    budget = Budget(args.budget)
    records = []
    for rep in range(1, args.reps + 1):
        for arm in arms:
            for task in tasks:
                print(f"--- {arm} / {task} / rep {rep} ---", flush=True)
                records.append(run_session(key, args.model, arm, task, rep,
                                           b, site, budget, args.out))
    server.shutdown()
    with open(os.path.join(args.out, "summary.json"), "w") as f:
        json.dump({"model": args.model, "records": records,
                   "total_cost_usd": round(budget.spent, 6)}, f, indent=2)
    t = table(records, arms, tasks)
    print(t, flush=True)
    for row in t.split("\n")[2:]:
        print(f"::notice::{row}", flush=True)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a") as f:
            f.write(f"\n## Summary\n\nModel {args.model}, spend "
                    f"${budget.spent:.4f}\n\n{t}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
