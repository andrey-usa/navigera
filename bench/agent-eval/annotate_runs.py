#!/usr/bin/env python3
"""Publish an agent-eval summary JSON as check-run annotations.

GitHub keeps at most 10 notices per step and ~3.5 KB fits per notice, so
the per-run record is trimmed (no model text; passing runs lose their
command lists first, failed runs keep their last commands, failing steps
and `in_flight`) until the whole summary fits in 10 chunks; a run of very
many sessions keeps one line of numbers for each.

  annotate_runs.py out/gemini/summary-skilled.json "agent eval json skilled"
"""
import json
import os
import subprocess
import sys

CHUNK, MAX_NOTICES = 3500, 10


def main() -> int:
    path, title = sys.argv[1], sys.argv[2]
    if not os.path.exists(path):
        print(f"{path}: no summary (mode not run)")
        return 0
    doc = json.load(open(path, encoding="utf-8"))
    runs = doc.get("runs", [])
    full = {id(r): (list(r.get("shell_commands", [])), list(r.get("shell_steps", []))) for r in runs}
    # Passing runs give up their detail first; a failed run keeps its
    # commands, failing steps and the command it was stuck in as long as
    # anything fits: that is what explains it.
    for keep_ok, keep_failed in ((12, 12), (6, 12), (3, 12), (0, 12), (0, 6), (0, 3), (0, 0)):
        for run in runs:
            keep = keep_failed if not run.get("ok") else keep_ok
            commands, steps = full[id(run)]
            run["shell_commands"] = commands[-keep:] if keep and not run.get("ok") else commands[:keep]
            # Output tails: keep the failed steps first, they explain the run.
            steps = [x for x in steps if x.get("status") != "success" or "rror" in x.get("tail", "")] + \
                    [x for x in steps if x.get("status") == "success" and "rror" not in x.get("tail", "")]
            run["shell_steps"] = [dict(x, tail=x.get("tail", "")[-160:]) for x in steps[:keep]]
            run.pop("text", None)
            run.pop("tools_used", None)
        # One value per line: annotate.py chunks on line boundaries.
        text = json.dumps(doc, indent=0)
        if len(text) <= CHUNK * MAX_NOTICES * 0.9:
            break
    else:
        # Many runs (8 reps of 4 tasks on 3 tools is 96): each keeps the
        # numbers the tables are made of, one run to a line; the artifact
        # has the rest. Still too long: the per-tool summary alone.
        keep = ("tool", "task", "rep", "os", "ok", "infra", "timed_out", "requests", "shell_count", "tokens_in",
                "tokens_cached", "tokens_out", "wall_s")
        said = lambda v: not (v is None or v is False or v == "linux")
        lines = [json.dumps({k: run[k] for k in keep if said(run.get(k))}, separators=(",", ":")) for run in runs]
        head = {k: v for k, v in doc.items() if k != "runs"}
        text = json.dumps(head, indent=0)[:-2] + ',\n"runs": [\n' + ",\n".join(lines) + "\n]\n}"
        if len(text) > CHUNK * MAX_NOTICES * 0.9:
            text = json.dumps(dict(head, runs_left_out=len(runs)), indent=0)
    tmp = path + ".annot"

    with open(tmp, "w", encoding="utf-8") as f:
        f.write(text)
    here = os.path.dirname(os.path.abspath(__file__))
    annotate = os.path.join(here, "..", "ladder", "annotate.py")
    return subprocess.call([sys.executable, annotate, tmp, title])


if __name__ == "__main__":
    sys.exit(main())
