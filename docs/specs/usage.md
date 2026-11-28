# Usage and limits of the model in use

Status: v0.0.3, stages U1 and U2. Plan: `plans/provider-usage.md` (local, not committed). A **limit** is one line: a name, a
bar, "used / total" or a percentage, and when it resets. A provider has zero or more.

## 1. What is built, and what is not

Built: the counters (U1); limits from response headers, from the body of a 429 and from a known limit the owner types (U2);
the ring, the card, the bar per provider, the Usage block, the 90 % line, the tool `usage`. Not built: quota endpoints per
template (U4), skipping to the fallback when a limit is out (U3), the reserve for unattended work, reports, rotation. A
limit that is out is shown and told to the model; the loop does not change its behaviour because of it.

## 2. Counters (U1)

`<data>/usage.sqlite` (`config.md` section 15). Every response that ends well is counted: one request, its input and output
tokens (`ModelResponse.usage`, as the four adapters fill it). A failed request is not counted. The count is kept per provider,
model and **source** in three windows: a minute (`YYYY-MM-DDTHH:MM`), a day (`YYYY-MM-DD`) and a month (`YYYY-MM`), in the
machine's local time. Rows older than 13 months are deleted at start and once a day.

Source is the small enum `chat`, `voice`, `subagent`, `task`, `other`. The loop sets it from the turn's `Via` in a task-local
(`kagent_core::model::CALL_SOURCE`): text, Telegram, the screen watch and notes are `chat`, voice is `voice`, a subagent is
`subagent`, a scheduled task is `task`; a call outside a turn (the provider test, a title, a summary made outside a turn) is
`other`. The live voice socket does not go through the providers client, so a live model is not counted (stated in the UI as
"live voice is not counted").

`kagent_providers::usage::UsageSink` is the seam: the client calls `counted(provider, model, usage)` after a response and
`observed(provider, observation)` for every response, success or failure; the daemon's `usage` module implements it.

## 3. Limits (U2)

```
Limit {id, name, used, total, unit, resetsAt, source, level}
```

`used` and `total` are numbers, or `null` for a limit that is known to be out but whose size is not (a bare `retry-after`).
`unit` is `requests`, `tokens` or `usd` (reserved). `resetsAt` is an RFC 3339 time or `null`. `source` is `provider` (a header
or an error body) or `kagent` (KAgent's own count against a known limit). `level` is `fine` (under 70 %), `close` (70 to 90 %
inclusive), `nearlyOut` (over 90 %) or `out` (100 %, or a 429 that names a limit). `used / total` is capped at 1.

**A missing or malformed header yields no limit, never a wrong one**: a limit needs a limit and a remaining value that are
whole numbers with `0 < limit` and `remaining <= limit`; a reset that is present and cannot be read drops that family.
A limit carries when it was seen; it is dropped when its reset has passed, or, with no reset, 10 minutes after it was seen
without a newer answer replacing it. A newer answer replaces the limit with the same `id`.

### 3.1 Headers (`kagent-providers/src/usage/headers.rs`)

Parsed on every response, success or failure, whatever the API family; the names are lower case.

| Family | Headers | Limits (`id`, name) |
|---|---|---|
| Anthropic | `anthropic-ratelimit-{requests,tokens,input-tokens,output-tokens}-{limit,remaining,reset}`; reset is an RFC 3339 time | `requests` "Requests", `tokens` "Tokens", `input-tokens` "Input tokens", `output-tokens` "Output tokens" |
| OpenAI style (OpenAI, Groq, Mistral, many compatible servers) | `x-ratelimit-{limit,remaining,reset}-{requests,tokens}` | `requests`, `tokens` |

A reset is read as: a duration (`1s`, `6m0s`, `2m59.56s`, `20ms`, `1h2m3.5s`); a number (below 1e9 seconds from now, below
1e12 an epoch in seconds, else an epoch in milliseconds); an RFC 3339 time. Anything else is malformed. `retry-after`
(seconds, or an HTTP date) is read only on a 429 or 503.

### 3.2 The body of a 429 (`.../usage/errors.rs`)

| Shape | Read | Limit |
|---|---|---|
| Google (`error.details[]`, also in an array of one; Gemini) | `QuotaFailure.violations[]`: `quotaId`, `quotaMetric`, `quotaValue`; `RetryInfo.retryDelay` | one per violation: id `quotaId`, name from the id ("Requests per day (free tier)"), `total` = `quotaValue`, `used` = `total`, level `out`, `resetsAt` = now + retryDelay (a daily quota with no better time: null) |
| OpenAI style message (`error.message`: "... on requests per min (RPM): Limit 3, Used 3, Requested 1. Please try again in 20s.") | the limit name, `Limit`, `Used`, the wait | id from the name, level by the numbers |
| OpenAI `insufficient_quota`, Anthropic `rate_limit_error`, any other 429 | `retry-after` | id `rate`, name "Rate limit", `used` and `total` null, level `out`, `resetsAt` from `retry-after` or null |

A 429 never invents a number it was not given.

### 3.3 A known limit (the owner's)

`settings.usage.knownLimits` (`config.md` section 15): per provider id a list of `{window: "minute"|"day"|"month", unit:
"requests"|"tokens", total}`. KAgent shows each as a limit with `source: "kagent"`, `used` from its own counters of the current
window (all models of that provider, all sources), `resetsAt` the end of the window, id `known-<window>-<unit>`. The card and
the tool say "counted by KAgent: other apps using this key are not included". It is optional and never prefilled.

A local provider (`local` true) has no limits: headers, bodies and known limits are ignored; counters only.

## 4. The state (`kagentd/src/usage/`)

`store.rs` the sqlite counters; `limits.rs` the limits per provider in memory (not saved: they are what the last answers said,
a restart starts empty); `mod.rs` the sink, the view and the events; `http.rs` the routes; `tool.rs` the tool; `tests.rs`.
The tightest limit of a provider is the one with the highest `used / total` (a limit with a null total counts as 100 %).

Events: `usageChanged {provider}` at once when the level of any limit of that provider changed or a limit appeared or went
away, and otherwise at most every 30 seconds per provider while responses keep arriving.

## 5. API (`api.md` section 11)

- `GET /api/usage` -> `{providers: [{id, name, local, limits: [Limit], tightest: Limit | null, today: Totals, month: Totals,
  models: [{model, today: Totals, month: Totals}], knownLimits: [...], checkedAt: string | null}]}`, with `Totals =
  {requests, inputTokens, outputTokens}`. Every provider, also those without a limit. `checkedAt` is when the last answer
  was seen. No key, no token, no header value other than the numbers above is in any answer (tested with a key in the store).
- `GET /api/providers` rows gain `local` and `tightestLimit` (the tightest limit or null).
- The known limits are saved with `PUT /api/settings {usage: {knownLimits: {<provider>: [...]}}}`.

## 6. The tool

`usage {provider?}` (group `agent`, read-only): per provider, its limits and today's and this month's counts, as text worded per
`agent-loop.md` section B: "Gemini: Requests per day 47 of 50 (94 %, nearly out), resets 09:00." An unknown provider is
`not_done("no provider named <x>", "call usage without a provider to list them")`. A provider with no limit says "no limit
known" and why (local, or the provider reports none). Counts are marked "through KAgent".

## 7. The UI

- `UsageRing` (`web/src/components/UsageRing.tsx`): a thin SVG ring; filled by the tightest limit's fraction, green under 70 %,
  amber from 70 % to 90 %, red above, red and full at out; nothing when there is no limit. On the composer's picker trigger
  (the provider of the answering model), on each provider in the chooser, on the widget's model chip (ring only).
- `UsageCard`: a click or hover on a ring opens it: each limit as a bar with "used / total" or its percentage, "resets at
  HH:MM", and its source ("from the provider" or "counted by KAgent"). The widget has no card.
- Settings -> Models: a thin bar with the percentage on each provider row; the provider card has a "Usage" block: its limits,
  today's and this month's requests and tokens per model, "last checked", and the known-limit fields (window, unit, total, with
  Save and Remove).
- A line above the composer, once per limit and level crossing, when a limit passes 90 %: "Gemini is at 94 % of Requests per day,
  resets 09:00". Dismissed with a button; shown again only after the limit has dropped below 90 % and passed it again.
- The page reads `GET /api/usage` at start, when the chooser or a card opens, and on `usageChanged`.

## 8. Not verified

The header and error samples in the tests are written from the providers' documentation, not recorded from real answers; each
fixture is marked so. They must be checked against real responses.
