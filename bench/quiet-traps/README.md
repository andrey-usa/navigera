# quiet-traps: chained work after an action's response

Trap pages for the wait after an action's own requests finish. A click
fetches `/first` (150 ms); on its response the page schedules a second
fetch (`/second`, 150 ms) or other work, per the trap (see `server.py`
for the list). The click's change report either shows the final state
(`final: <trap>`) or an in-between one.

Run:

    python3 server.py 8973 &                       # the trap pages
    python3 drive.py --bin target/release/navigera --label next --reps 5

`drive.py` writes `results-<label>.jsonl` (one row per rep: trap,
final-state flag, click wall ms). `final` for the fetch traps means the
report contained `final: <trap>`; for `analytics` it means the report
returned promptly (no DOM change to wait for); for `longpoll` it means
the report returned with the page's text well under the held request's
30 s.
