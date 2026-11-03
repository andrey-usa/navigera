#!/usr/bin/env python3
"""Parallel agents: N sessions of one tool open at once, for memory.

Each agent has a session of its own, the way parallel agents use a tool:
navigera a named session each (`-s`; one shared browser, a context each, or
`--own-browser`), agent-browser and playwright-cli a session each, the MCP
servers one server process each. Every session opens the listing page and
takes a snapshot. Then this prints one JSON line {"ready": true, "up_s",
"session_ms": [...]} and holds everything open until a line arrives on
stdin, while the ladder sums the memory of every process the tool started;
then it closes them all and prints {"closed": true}.

    parallel.py --tool nv|nv-own|ab|pw|pwmcp|cdmcp --n 4

Env: LADDER_BASE, CHROME_BIN, NAVIGERA, AGENT_BROWSER, PLAYWRIGHT_CLI,
PLAYWRIGHT_MCP, CHROME_DEVTOOLS_MCP (as cli_agent.py).
"""
import argparse
import json
import os
import re
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import cli_agent  # noqa: E402  (Mcp client and run(), same directory)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--tool", choices=["nv", "nv-own", "ab", "pw", "pwmcp", "cdmcp"], required=True)
    ap.add_argument("--n", type=int, required=True)
    args = ap.parse_args()
    base = os.environ["LADDER_BASE"]
    chrome = os.environ["CHROME_BIN"]
    env = dict(os.environ)
    url = f"{base}/page_a.html"
    tag = f"par{os.getpid()}"
    names = [f"{tag}-{i}" for i in range(args.n)]
    closers = []
    session_ms = []

    def run(argv, cwd=None):
        return cli_agent.run(argv, env, cwd)

    t0 = time.perf_counter()
    if args.tool in ("nv", "nv-own"):
        exe = os.environ["NAVIGERA"]
        own = ["--own-browser"] if args.tool == "nv-own" else []
        for s in names:
            t = time.perf_counter()
            run([exe, "--session", s, "start", "--chromium", chrome, *own])
            run([exe, "--session", s, "goto", url])
            run([exe, "--session", s, "ax"])
            session_ms.append((time.perf_counter() - t) * 1000)
            closers.append(lambda s=s: run([exe, "--session", s, "quit"]))
    elif args.tool == "ab":
        exe = os.environ.get("AGENT_BROWSER", "agent-browser")
        env.setdefault("AGENT_BROWSER_EXECUTABLE_PATH", chrome)
        env.setdefault("AGENT_BROWSER_ARGS", "--no-sandbox")
        for s in names:
            t = time.perf_counter()
            run([exe, "--session", s, "open", url])
            run([exe, "--session", s, "snapshot"])
            session_ms.append((time.perf_counter() - t) * 1000)
            closers.append(lambda s=s: run([exe, "--session", s, "close"]))
    elif args.tool == "pw":
        exe = os.environ.get("PLAYWRIGHT_CLI", "playwright-cli")
        cwd = tempfile.mkdtemp(prefix="pwpar-")
        os.makedirs(os.path.join(cwd, ".playwright"))
        with open(os.path.join(cwd, ".playwright", "cli.config.json"), "w") as f:
            json.dump({"browser": {"browserName": "chromium", "launchOptions": {
                "executablePath": chrome, "headless": True, "chromiumSandbox": False}},
                "outputDir": cwd}, f)
        for s in names:
            t = time.perf_counter()
            run([exe, f"-s={s}", "open", url], cwd)
            run([exe, f"-s={s}", "snapshot"], cwd)
            session_ms.append((time.perf_counter() - t) * 1000)
            closers.append(lambda s=s: run([exe, f"-s={s}", "close"], cwd))
    else:
        for s in names:
            cwd = tempfile.mkdtemp(prefix=f"{args.tool}-")
            t = time.perf_counter()
            if args.tool == "pwmcp":
                exe = os.environ.get("PLAYWRIGHT_MCP", "playwright-mcp")
                mcp = cli_agent.Mcp([exe, "--headless", "--browser", "chromium", "--executable-path", chrome,
                                     "--isolated", "--no-sandbox", "--output-dir", cwd], env, cwd)
                mcp.tool("browser_navigate", {"url": url})
                mcp.tool("browser_snapshot", {})
            else:
                exe = os.environ.get("CHROME_DEVTOOLS_MCP", "chrome-devtools-mcp")
                mcp = cli_agent.Mcp([exe, "--headless", "--executable-path", chrome, "--isolated",
                                     "--chrome-arg=--no-sandbox", "--no-usage-statistics"], env, cwd)
                page = {}
                if "pageId" in mcp.schemas.get("navigate_page", {}):
                    _, text = mcp.tool("list_pages", {})
                    m = re.search(r"^(\d+): .*\[selected\]", text, re.M)
                    if m:
                        page = {"pageId": int(m.group(1))}
                mcp.tool("navigate_page", {"type": "url", "url": url, **page})
                mcp.tool("take_snapshot", page)
            session_ms.append((time.perf_counter() - t) * 1000)
            closers.append(mcp.close)
    print(json.dumps({"ready": True, "up_s": time.perf_counter() - t0,
                      "session_ms": [round(x, 1) for x in session_ms]}), flush=True)
    sys.stdin.readline()
    for close in closers:
        try:
            close()
        except Exception as e:  # measured already; a failed close is noted, not fatal
            print(f"close failed: {e}", file=sys.stderr)
    print(json.dumps({"closed": True}), flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
