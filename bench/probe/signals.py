#!/usr/bin/env python3
"""What natural mode's page saw in each repeat (signals.json beside each
rep-NN directory): how often each automation sign fired, and the window
sizes of the repeats where one did."""
import json
import sys
from pathlib import Path

arm = Path(sys.argv[1])
counts, lines, n = {}, [], 0
for rep in sorted(arm.glob("rep-*")):
    f = rep / "signals.json"
    if not f.is_file():
        lines.append(f"{rep.name}: no signals.json (the test ended before natural mode's page)")
        continue
    n += 1
    after = json.loads(f.read_text())["after"]
    fired = [k for k, v in after.items() if not k.startswith("info_") and v is True]
    for k in fired:
        counts[k] = counts.get(k, 0) + 1
    if fired:
        lines.append(f"{rep.name}: {fired} size {after.get('info_size')} outer at check {after.get('info_outer_at_check')}")
sizes = {}
for rep in sorted(arm.glob("rep-*")):
    f = rep / "signals.json"
    if f.is_file():
        at = json.loads(f.read_text())["after"].get("info_outer_at_check") or [None, None, None]
        key = f"{at[0]}x{at[1]}"
        sizes[key] = sizes.get(key, 0) + 1
print(f"signals ({n} repeats reached natural mode's page): fired {counts or 'none'}; outer size at the check: {sizes}")
for line in lines:
    print(line)
