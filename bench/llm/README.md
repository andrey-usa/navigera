# Model benchmark usage records

Run `python3 -m unittest discover -s bench/llm -p test_usage.py` for offline
fixtures. These tests make no provider calls and launch no browser.

## Record shape, version 1

Each session writes `<arm>-<task>-<rep>.usage.jsonl` alongside its JSON.
One line records every attempted model call, including HTTP errors and
timeouts. It contains the exact raw `usage` object (or null), model and
response model/request id, timestamp, HTTP status, retry count (zero: no
retries in this driver), elapsed ms, request byte/message counts, context
strategy label, and normalized fields. Authorization headers, API keys
and prompt bodies are not recorded in this usage journal. Existing
transcript logging is unchanged.

`tokens.input_total` includes `input_cached`. Do not add cached input
again. `input_uncached = input_total - input_cached` only when both are
valid and the difference is nonnegative. Output tokens are separate.
Cached output has a numeric value only if the provider actually returns
`completion_tokens_details.cached_tokens`; otherwise its value is null
and its status is `unsupported`. A reported positive cached-output value
without a verified rate makes the total estimate unknown, not free.

Every token field has a status. Missing usage is null/`missing`; explicit
zero remains zero/`reported`; unsupported is not zero. No usage is inferred
from estimated costs, character counts, or request bytes. Session totals
are null if any constituent call is unknown for that field. Known
subtotals are labeled as subtotals, never as full totals.

`rates` records currency, token unit, model, source URL and date read.
The default model rate card is from https://www.siliconflow.com/pricing,
read 2026-11-05: USD per million, uncached input 0.15, cached input 0.003,
output 0.60. Other models have unknown rates until explicitly configured.
Rates are not automatically refreshed or claimed to be contractual prices.
`estimated_cost_components_usd` multiplies disjoint token categories by
their rates. `estimated_cost_usd` is their sum when complete. Calculations
use decimal arithmetic and do not round each call before aggregation.
These are harness-calculated estimates, not actual provider charges.

`provider_charge_usd` is null/`not_reported` for this provider adapter.
There is no verified billing field populated by this driver. A future
adapter may fill a charge only from a documented provider charge field,
with currency/unit provenance. Verified billing requires separate provider
reconciliation. No cached-output charge or rate is invented.

Failed/timeout calls are `cost_unknown`, not zero dollars. An unknown
usage response also retains its reservation for admission control.
`unknown_exposure_bound_usd` is a conditional reservation, not measured
spend or a verified maximum. `admission_control_consumed_usd` in the run
summary can include these reservations and must not be labeled as cost.
The input reservation is a conditional ceiling over the full serialized
request body (body bytes + per-message and per-request template slack),
NOT a verified spending cap: the provider's tokenizer behavior and the
template slack are unverified.

The summary retains expected and observed session counts and a coverage
flag. Completeness must be checked from the full artifacts, not a subset
of published notices. Per arm/task tables show medians of input (including
cache), cached input, output, estimated cost, model attempts and wall time.
Cost per success = sum of estimated costs of ALL task attempts / successful
tasks, including failed-task cost in the numerator; it is unknown if any
cost is missing or there are no successes. Do not sum medians into a run
total. Model attempts and browser steps are different quantities, especially
for a batch arm. Context strategy is a label, not proof of a causal benefit.

This shape is intended for later IDE/agent benchmark adapters as well.
Prior per-call raw usage was not persisted and cannot be reconstructed;
older session JSON may still contain separate prompt/completion totals.
