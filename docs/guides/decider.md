# Let a fast model pick the element

Two small decisions an agent otherwise spends a page tree and an LLM turn on,
*which element* and *did it work*, can go to a System One model:
TypeSafe's [Jev](https://docs.typesafe.ai) when `TYPESAFE_API_KEY` is set, or
any endpoint with the same API through `NAVIGERA_DECIDER_URL`.

```bash
navigera -s work click --find "accept the cookies"   # acts on the element the model picks
navigera -s work check "Is an error message shown?"  # {"yes":0.02,"answer":false}
```

`find` returns the ref, its probability and the runners-up; below 0.5,
`--find` refuses and lists the candidates instead of acting. `start` reports
`"decider"` when one is configured, and MCP clients get `browser_find` and
`browser_check` only then. Nothing leaves the machine otherwise; typed values
are sent as `(filled)`.

**Measured (2026-10-14).** On 39 decisions with known answers, Jev picked
25/25 elements and answered 14/14 yes/no questions (151 ms median; $0.0086
for all 39). Gemini 3.5 Flash-Lite reading the same tree got 23/25 and 14/14
(724 ms median) and once chose the button that text planted in the page
pointed at. In the agent eval, Gemini CLI told to prefer `--find` on two large
reference pages (56–95 KB trees) used about half the Gemini tokens of the runs
that read the tree (114–119K against a median of 250K; small samples).
Unprompted, it read the tree and never used the decider. Details and caveats:
[docs/multi-model.md](../multi-model.md).

**Measured again (2026-10-21, run 39530836547).** On 78 decisions (50
elements, 28 yes/no questions, public pages included), Jev got 50/50 and
27/28 at an 80 ms median, $0.017 for all of them; DeepSeek-V4.1-Flash
reading the same tree got the same 50/50 and 27/28 at a 2.0 s median.
Neither followed text planted in the page. Since `start` and long trees
tell the agent how to use the decider, Gemini CLI used it unprompted in 3 of
15 runs and in 14 of 15 when the prompt asked (run 39561380436, three runs
of five tasks per arm); median tokens per run were 199K without the
decider, 172K with it and 190K when told, a spread within the run-to-run
noise at this sample size.

**Measured (2026-11-11, runs [42978184533](https://github.com/andrey-usa/navigera/actions/runs/42978184533) and
[42978367965](https://github.com/andrey-usa/navigera/actions/runs/42978367965)).** Since `ax --grep` and `text --grep`, an
agent that knows a word of what it is looking for needs no decision model
to skip reading a large page, and the model no longer pays for itself
there: on two large docs pages DeepSeek-V4.1-Flash took 6 and 7 turns
without the decider and 6 and 6 with it, at an estimated $0.0011 a session
without and $0.0014 with (Jev's tokens included); told to prefer the
decider, $0.0019 to $0.0026. GPT-5.6 Terra, at 13 times the token price,
took 4 turns either way and cost the same within 4%. What is left for the
decider is the pick an agent cannot name a word for. On 39 decisions with
known answers Jev picked 23/25 elements and answered 14/14 questions at a
72 ms median (run [42978205179](https://github.com/andrey-usa/navigera/actions/runs/42978205179)); a link repeated on a
page is now asked about once, 11% fewer tokens over those decisions.
