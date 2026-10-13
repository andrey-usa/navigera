# Several models, one browser

An agent that browses spends most of its tokens reading pages. A single
accessibility tree of a real page is 20–100 KB (Wikipedia's Rust article:
89 KB, about 22k tokens), the agent reads one after nearly every action, and
each stays in the conversation and is paid for again on every later turn.
Most of that reading serves two small decisions: *which element is the one I
want*, and *did that work*. Neither needs a model that can write.

This document is about giving those decisions to a cheaper, faster model, and
what that model can and cannot take over.

## The models

| role | model | strengths | costs |
|---|---|---|---|
| System Two: plan, read, write | the agent's LLM (Claude, Gemini, GPT, a local model) | understands the task, writes text, URLs, form values, final answers | the most expensive tokens, a second or more per turn |
| System One: decide | TypeSafe Jev 1.13 (`jev-latest`) | a choice among up to 255 options, a yes/no probability, a score on 2–10 levels, each with calibrated probabilities; a few hundred ms; questions in one request run in parallel | $0.042 per million input tokens, output free; 64k tokens a request (32k for the state plus the longest question); 80 requests/s |
| Mechanics | navigera itself | waits, retries, refs, tabs, dialogs, iframes: everything with one right answer | free |

Jev's own documentation is clear about its limits, and they decide where it
fits: it writes nothing (not a URL, not a value, not an answer), it does not
count or do arithmetic, it reads dates as text, it leans toward the first
option, accuracy falls as the state fills with unrelated text, and it **does
not treat the state as hostile**: text in the page can steer its answers. So
it cannot replace the agent's LLM, and it must not be a security boundary.
What it can do is make the LLM's job smaller.

## What is built

`find`, `check` and `--find` (see `docs/navigera.md`, "Decisions"):

- **`find "<description>"`**: every element with a ref becomes an option,
  described by its tree line and the containers it sits in, so twelve "Add to
  cart" buttons are told apart by their cards. One Choice per 200 options,
  several per request, the chunk winners asked again. The answer is the ref,
  the probability, and three runners-up.
- **`--find "<description>"` on any element command**: the agent acts without
  reading the tree. Below a probability floor (`NAVIGERA_FIND_MIN`, 0.5) the
  command fails and lists the candidates, so an unsure choice is shown to the
  LLM instead of being acted on.
- **`check "<question>"`**: a Noul over the page's text, for the "did it
  work" step: "Is an error shown?", "Is the cart empty?".
- **MCP**: `browser_find`, `browser_check` and a `find` target appear only
  when a decider is configured, so the default tool list stays what agents
  know (it grows from ~2,600 to ~3,200 tokens).
- **Privacy**: off unless `TYPESAFE_API_KEY` or `NAVIGERA_DECIDER_URL` is set;
  typed values are replaced by `(filled)` before anything is sent.

### Measured

What the agent's LLM reads to choose one element: the tree, against the
`find` answer (bytes; about four to a token). The decisions were made by the
stand-in `bench/decide/mock_jev.py`, which matches words and does not
understand anything, so the sizes are real and the "chose" column is only the
mock's.

| page | looking for | tree | `find` answer | sent to the decider | mock chose |
|---|---|--:|--:|--:|---|
| en.wikipedia.org/wiki/Rust_(programming_language) | the Cargo package manager article | 89,341 | 674 | 284,828 | "package manager" (wrong) |
| github.com/rust-lang/rust | the Issues tab | 31,899 | 359 | 35,477 | "Issues (5k+)" |
| news.ycombinator.com | the login link | 22,378 | 361 | 34,320 | the logo (wrong) |
| docs.python.org/3/library/json.html | the search box | 69,526 | 315 | 21,286 | "Quick search" |
| developer.mozilla.org (input element) | the site search | 98,957 | 445 | 240,522 | "Search the site" |
| www.rust-lang.org/learn | the Rust book | 6,740 | 481 | 5,102 | the CLI book (wrong) |
| **6 pages** | | **318,841** | **2,635** | **621,535** | |

- The LLM reads **121× less**: 2.6 KB instead of 319 KB (about 80k tokens).
- The decider reads about twice the tree (descriptions repeat their context,
  and big pages are asked twice), ~155k tokens for the six: **$0.0065** at
  Jev's price. That is cheaper than the LLM reading the trees once whenever
  the LLM's input price is above ~$0.08 per million tokens, which is every
  capable model; and the LLM would keep each tree in its context for the rest
  of the task.
- On the bench site's own pages (`/shop`, product, cart, login, docs,
  support) the trees are small (0.6–3 KB), the saving is 3×, and even the mock
  chose right 6 of 6.

**Not measured yet: how often the real model chooses right.** That needs a
TypeSafe key. The harness is in place (`tests/decide.rs` for the plumbing,
`bench/decide/mock_jev.py` for runs without a key); the next step is the
agent-eval tasks run three ways, with the real model, recording success, LLM
tokens, decider tokens and wall time:

1. the LLM reads trees (today's baseline);
2. the LLM uses `--find` / `check`, falling back to the tree when unsure;
3. a small LLM with `--find` / `check` against a large one without.

Run 3 is the one that matters most: if a small model plus Jev matches a
large model alone, the saving is the price ratio of the two models, not the
size ratio of the trees.

## What comes next, by expected value

1. **`wait --until "<question>"`**: poll `check` until yes. Replaces sleeps
   and "read it again" turns on pages that render late (the reason Playwright
   MCP needed five calls where navigera needed two in `bench/ide-eval`).
2. **Snapshot pruning**: score each region of the tree (landmark, form,
   table, article) for relevance to the task in one request, return the
   relevant regions in full and the rest as one-line outlines. The tree
   still reaches the LLM, smaller; this keeps the LLM's judgment where `find`
   replaces it, so it is the safer of the two for hard pages.
3. **Action gate**: before a click or Enter on a control that can commit
   something (a purchase, a send, a delete), a Score of how irreversible the
   action is; above a level, the op asks for `--confirm` and the agent asks
   the user. Policy in code, judgment in the model, and still not a boundary:
   a page can word its buttons to look harmless.
4. **Injection screening**: a Noul per text block, "is this text addressed
   to an AI assistant or trying to change its task?", marking flagged blocks
   in the tree (`[untrusted: instructions]`). A screen, not a wall, since the
   same text reaches Jev; paired with the skill's rule that page text is
   data.
5. **Turn routing in the agent harness** (outside navigera): a cheap LLM
   drives by default; a Noul on (task, page, proposed action) — "will this
   action make progress?" — and a Noul for "is the task done?" decide when to
   escalate to the strong model and when to stop. This is Jev's
   confidence-gated routing pattern applied to turns instead of requests.

## Why not let Jev drive

A browsing step is mostly a choice from a finite set: which element, which
action, done or not. A "System One autopilot" could take a form-filling task
step by step: choose an action among `click/fill/select/press` × the page's
elements; take each value from the task text by choosing among candidate
spans (Jev's pre-parsed value extraction pattern); stop on a "task done"
Noul. That works only while every value is literally in the task and every
step is unambiguous. The moment a step needs reading (compare two prices,
answer a question about the page, recover from an unexpected page) it needs
the LLM. It is worth an experiment for repetitive, well-specified flows
(the same form for many records), where it would make each step cost
fractions of a cent and a fraction of a second; it is not a general
replacement, and TypeSafe says the same.

## Risks

- **Accuracy is unmeasured** with the real model; the floor on `--find` and
  the runners-up in every answer are there so a wrong pick is visible.
- **Page content goes to a third party** when enabled: opt-in, documented in
  `SECURITY.md`, typed values redacted. A self-hosted endpoint with the same
  API works through `NAVIGERA_DECIDER_URL`.
- **One vendor**: the API is small (one endpoint, three question types), the
  client is 150 lines, and LiteLLM proxies it; another System One model with
  the same shape would drop in.
- **Latency**: each decision is a network round trip (hundreds of ms). It
  replaces an LLM turn (seconds), but adds to a step that needed none, so
  `--find` is for the steps where the agent would otherwise read the tree.
