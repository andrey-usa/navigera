#!/usr/bin/env python3
"""Echo a file as GitHub `::notice` annotations.

Step summaries and artifacts are only readable through the web UI / blob
storage; annotations come back through the plain checks API
(`gh api repos/<o>/<r>/check-runs/<job>/annotations`), so a remote agent or
script can read the ladder table without downloading anything.

GitHub keeps 10 notices per step, so a file that would need more is an
error, never a silently cut tail. A `.json` file is re-serialised compact
(no indent, no spaces) first; joining its notices in order and parsing the
result gives the same document back.
"""
import json
import sys
from pathlib import Path

MAX_NOTICES = 10
CHUNK = 3500

# Tables carry ✓/✗/⚠: write UTF-8 even where the console code page isn't.
sys.stdout.reconfigure(encoding="utf-8")
path = Path(sys.argv[1] if len(sys.argv) > 1 else "bench/ladder/out/table.md")
title = sys.argv[2] if len(sys.argv) > 2 else "ladder table"
if not path.exists():
    print(f"::warning title={title}::{path} not found")
    sys.exit(0)
text = path.read_text(encoding="utf-8")
if path.suffix == ".json":
    try:
        text = json.dumps(json.loads(text), separators=(",", ":"), ensure_ascii=False)
    except ValueError as e:
        print(f"::error title={title}::{path} is not valid JSON: {e}")
        sys.exit(1)
    # One line, so cut by length; lines stay whole only for text files.
    # No piece begins or ends with a space, so whatever trims a notice's
    # edges on its way changes nothing: compact JSON has spaces only inside
    # its strings, and a lost one would still parse.
    chunks, i = [], 0
    while i < len(text):
        j = min(i + CHUNK, len(text))
        while i + 1 < j < len(text) and (text[j - 1].isspace() or text[j].isspace()):
            j -= 1
        chunks.append(text[i:j])
        i = j
else:
    chunks, cur = [], ""
    for line in text.splitlines():
        if cur and len(cur) + len(line) > CHUNK:
            chunks.append(cur)
            cur = ""
        cur += line + "\n"
    if cur:
        chunks.append(cur)
if len(chunks) > MAX_NOTICES:
    print(f"::error title={title}::{path} needs {len(chunks)} notices, GitHub keeps "
          f"{MAX_NOTICES} per step: shorten the file or split the step")
    sys.exit(1)
for i, chunk in enumerate(chunks, 1):
    enc = chunk.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
    print(f"::notice title={title} {i}/{len(chunks)}::{enc}")
