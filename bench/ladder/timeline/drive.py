#!/usr/bin/env python3
"""Driver : interleaved navigera vs go-rod timeline probes.

Runs REPS of probe A (navigera serve, nv_probe.py) and probe B (gorod
timeline mode), alternating A,B,A,B,... so network drift hits both tools
equally. Appends raw JSON lines to the results file, then writes a
markdown summary with medians and IQRs per phase.
"""
import argparse
import json
import os
import statistics
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))


def run(cmd, env) -> dict:
    p = subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=300)
    if p.returncode != 0:
        raise RuntimeError(f"probe failed: {' '.join(cmd)}\n{p.stderr[-1500:]}")
    for line in p.stdout.splitlines():
        line = line.strip()
        if line.startswith("{"):
            return json.loads(line)
    raise RuntimeError(f"no JSON line from {' '.join(cmd)}\n{p.stdout[-500:]}\n{p.stderr[-500:]}")


def med_iqr(vals, allow_neg=False):
    vals = sorted(v for v in vals if v is not None and (allow_neg or v >= 0))
    if not vals:
        return None, None, None
    n = len(vals)
    med = statistics.median(vals)
    q1 = statistics.median(vals[: n // 2]) if n > 1 else vals[0]
    q3 = statistics.median(vals[(n + 1) // 2:]) if n > 1 else vals[-1]
    return med, q1, q3


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--reps", type=int, default=9)
    ap.add_argument("--navigera", required=True)
    ap.add_argument("--gorod", required=True)
    ap.add_argument("--chrome", required=True)
    ap.add_argument("--out", required=True, help="results JSONL (appended)")
    ap.add_argument("--summary", required=True, help="markdown summary out")
    ap.add_argument("--run-id", default="", help="workflow run id for the header")
    args = ap.parse_args()

    rows = []
    for i in range(args.reps):
        for tool in ("navigera", "gorod"):
            if tool == "navigera":
                env = dict(os.environ)
                r = run([sys.executable, os.path.join(HERE, "nv_probe.py"),
                         "--navigera", args.navigera, "--chrome", args.chrome], env)
            else:
                env = dict(os.environ, CHROME_BIN=args.chrome)
                r = run([args.gorod, "timeline"], env)
                r["tool"] = "gorod"
            r["run"] = i + 1
            rows.append(r)
            with open(args.out, "a", encoding="utf-8") as f:
                f.write(json.dumps(r) + "\n")
            print(f"[{i+1}/{args.reps}] {tool} done", file=sys.stderr, flush=True)

    nv = [r for r in rows if r["tool"] == "navigera"]
    gr = [r for r in rows if r["tool"] == "gorod"]

    def g(row, *path):
        cur = row
        for k in path:
            cur = (cur or {}).get(k)
            if cur is None:
                return None
        return cur

    def fnum(x):
        return f"{x:.1f}" if isinstance(x, (int, float)) else "-"

    def line(label, nv_vals, gr_vals, allow_neg=False):
        nm, n1, n3 = med_iqr(nv_vals, allow_neg)
        gm, g1, g3 = med_iqr(gr_vals, allow_neg)
        diff = (nm - gm) if (nm is not None and gm is not None) else None
        return (f"| {label} | {fnum(nm)} ({fnum(n1)}-{fnum(n3)}) | "
                f"{fnum(gm)} ({fnum(g1)}-{fnum(g3)}) | {fnum(diff)} |")

    lines = []
    header = f"# Timeline probe: navigera serve vs go-rod, {args.reps} interleaved runs each"
    if args.run_id:
        header += f" (run {args.run_id})"
    lines.append(header)
    lines.append("")
    lines.append("All values ms (median, IQR bounds). `net goto` = the goto window without browser launch.")
    lines.append("")
    lines.append("| phase | navigera | go-rod | nv - rod |")
    lines.append("|---|---|---|---|")
    lines.append(line("launch (nv launch_total / rod browser launch)",
                      [g(r, "phases", "launch_total") for r in nv],
                      [g(r, "timeline", "launch_ms") for r in gr]))
    lines.append(line("goto client total (incl. launch)",
                      [g(r, "goto_client_ms") for r in nv],
                      [g(r, "timeline", "goto_ms") for r in gr]))
    # rod's goto_ms starts at nav start (launch already done); navigera's
    # serve launches lazily inside the goto op, so subtract launch_total.
    lines.append(line("net goto (nv: total - launch; rod: goto_ms)",
                      [(g(r, "goto_client_ms") or 0) - (g(r, "phases", "launch_total") or 0) for r in nv],
                      [g(r, "timeline", "goto_ms") for r in gr]))
    lines.append(line("  Navigate command reply (rod client)",
                      [None for _ in nv],
                      [g(r, "timeline", "nav_reply_ms") for r in gr]))
    lines.append(line("  nav_start -> responseReceived (client clock)",
                      [None for _ in nv],
                      [g(r, "timeline", "resp_ms") for r in gr]))
    lines.append(line("  responseReceived -> loadEventFired (client)",
                      [None for _ in nv],
                      [(g(r, "timeline", "load_ms") or -1) - (g(r, "timeline", "resp_ms") or 0)
                       if g(r, "timeline", "load_ms") is not None and g(r, "timeline", "load_ms") >= 0 else None for r in gr]))
    # rod's WaitLoad is an in-page JS promise; the CDP Page.loadEventFired
    # notification can arrive later. Positive = event seen after WaitLoad.
    lines.append(line("  CDP loadEventFired receipt - WaitLoad return",
                      [None for _ in nv],
                      [(g(r, "timeline", "load_ms") or 0) - (g(r, "timeline", "goto_ms") or 0)
                       if g(r, "timeline", "load_ms") is not None and g(r, "timeline", "load_ms") >= 0 else None for r in gr],
                      allow_neg=True))
    lines.append(line("first eval (client)",
                      [g(r, "eval_client_ms") for r in nv],
                      [g(r, "timeline", "eval_ms") for r in gr]))
    lines.append(line("first eval (engine/CDP only)",
                      [g(r, "eval_engine_ms") for r in nv],
                      [None for _ in gr]))
    lines.append("")
    lines.append("go-rod network detail (from Network.responseReceived timing):")
    lines.append("")
    lines.append("| segment | median (IQR) |")
    lines.append("|---|---|")
    for key, label in [("dns_ms", "DNS"), ("connect_ms", "connect (TCP)"),
                       ("ssl_ms", "TLS"), ("send_ms", "send"),
                       ("ttfb_ms", "request start -> first byte (receiveHeadersEnd)")]:
        vals = [g(r, "timeline", "timing", key) for r in gr]
        m, q1, q3 = med_iqr(vals)
        lines.append(f"| {label} | {fnum(m)} ({fnum(q1)}-{fnum(q3)}) |")
    lines.append("")
    # CDP-clock phase split for go-rod, used as the shared network reference.
    lines.append("go-rod CDP-clock splits (event monotonic timestamps):")
    lines.append("")
    lines.append("| segment | median (IQR) |")
    lines.append("|---|---|")
    for label, vals in [
        ("requestWillBeSent -> responseReceived",
         [((g(r, "timeline", "resp_cdp_s") or 0) - (g(r, "timeline", "req_cdp_s") or 0)) * 1000
          if g(r, "timeline", "resp_cdp_s") is not None and g(r, "timeline", "resp_cdp_s") >= 0 else None for r in gr]),
        ("responseReceived -> loadEventFired",
         [((g(r, "timeline", "load_cdp_s") or 0) - (g(r, "timeline", "resp_cdp_s") or 0)) * 1000
          if g(r, "timeline", "load_cdp_s") is not None and g(r, "timeline", "load_cdp_s") >= 0 else None for r in gr]),
    ]:
        m, q1, q3 = med_iqr(vals)
        lines.append(f"| {label} | {fnum(m)} ({fnum(q1)}-{fnum(q3)}) |")
    lines.append("")

    with open(args.summary, "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")
    print("\n".join(lines))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
