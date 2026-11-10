#!/usr/bin/env python3
"""Write a bench run's publish.json from its `ladder json i/N` notices.

Usage: fetch_publish.py RUN_ID [out.json] [--repo owner/name]

annotate.py prints publish.json as notices, so a run's numbers can be read
through the checks API with no token and no artifact download. This joins
them back; `gen_readme.py README.md out.json` then rewrites the README's
Benchmarks from that run."""
import json
import re
import sys
import urllib.request

args = sys.argv[1:]
repo = "andrey-usa/navigera"
if "--repo" in args:
    i = args.index("--repo")
    repo = args[i + 1]
    del args[i:i + 2]
if not args:
    sys.exit(__doc__)
run, out = args[0], args[1] if len(args) > 1 else "publish.json"
api = f"https://api.github.com/repos/{repo}"


def get(url):
    with urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent": "fetch_publish"})) as r:
        return json.load(r)


parts, total = {}, 0
for job in get(f"{api}/actions/runs/{run}/jobs?per_page=100")["jobs"]:
    page = 1
    while True:
        notices = get(f"{api}/check-runs/{job['id']}/annotations?per_page=100&page={page}")
        for n in notices:
            m = re.fullmatch(r"ladder json (\d+)/(\d+)", n.get("title") or "")
            if m:
                parts[int(m.group(1))] = n["message"]
                total = int(m.group(2))
        if len(notices) < 100:
            break
        page += 1
if not total or sorted(parts) != list(range(1, total + 1)):
    sys.exit(f"run {run}: {len(parts)} of {total or '?'} `ladder json` notices found")
pieces = [parts[i] for i in sorted(parts)]
try:
    # Compact JSON cut by length: the pieces join as they are.
    doc = json.loads("".join(pieces))
except ValueError:
    # Runs from before that: indented JSON cut at line ends, each notice's
    # last newline dropped on the way.
    doc = json.loads("\n".join(p.rstrip("\n") for p in pieces))
with open(out, "w", encoding="utf-8") as f:
    json.dump(doc, f, indent=1, ensure_ascii=False)
    f.write("\n")
print(f"{out}: run {doc.get('run_id', run)}, generated {doc.get('generated_utc')}, {total} notices")
