# When `start` prints `"decider"`

A decision model is on: describe elements instead of reading the tree.
`click --find "the Sign in link"`, `fill --find "the email field" "ada@example.com"`
(every element command takes `--find` in place of a ref; the result names
the element it chose), `find "…"` for the ref only, and `check "Is the user
signed in?"` → `{"yes":0.97,"answer":true}` (near 0.5, look yourself). When
`--find` is unsure it lists the candidates: then read `ax`. Read `ax` or
`eval` only for text you must report (a price, an order number). With a
decision model on, a plain `ax` of a tree over 16 KB shows its first 40
lines; `ax --limit 2000` or `ax --selector main` gives the text. Without one,
`ax` is always the whole tree.
