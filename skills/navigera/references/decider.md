# When `start` prints `"decider"`

A decision model is on: name elements in words instead of reading the tree
for refs, so you can act on pages you have not seen. Put the steps you know
in one `batch`, across pages, and end with what you must report or verify:

```bash
navigera -s work batch <<'EOF'
goto https://docs.example.com
click --find "the link to the json module"
text --grep "indent"
EOF
```

- Every element command takes `--find "<what>"` in place of a ref (`fill
  --find "the email field" "ada@example.com"`); the result names the element
  it chose. `find "…"` gives the ref only.
- When `--find` is unsure it stops (a batch stops there too) and lists the
  candidates: pass one's ref, or read `ax`.
- `check "Is the user signed in?"` → `{"yes":0.97,"answer":true}` (near
  0.5, look yourself). For text you must report (a price, a default), `text
  --grep "<word>"` prints the passages; `check` only says yes or no.
- A plain `ax` of a tree over 16 KB shows its first 40 lines while the model
  is on; `ax --limit 2000` or `ax --selector main` gives all of it.
