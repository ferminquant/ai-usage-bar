# Usage snapshot contract

This is the provider-neutral contract every adapter, the cache, and the
shell consume. It is the first concrete step on
[issue #1](https://github.com/ferminquant/ai-usage-bar/issues/1).

The contract is language-neutral on purpose: the implementation language is
not locked yet. The shape is defined as JSON Schema plus prose so it can be
ported to a typed record, struct, or interface in any stack, and so contract
fixtures can be validated before any adapter exists.

This document refines the conceptual sketch in
[architecture.md](architecture.md). Where the two disagree, this file is
authoritative.

## Status

Revised after the Codex evidence spike (#2). The Codex-specific mapping is
now backed by a real response. Field names and value semantics are frozen
for adapter work. The Grok consumer mapping is also verified; Kimi remains a
planning note, while the hosted Ollama Pro totals source is now verified in
spike #4.

## Guiding rules

1. Every value is evidence, not a promise. Each snapshot carries its source,
   observed time, and confidence.
2. "Not reported" is not zero, not unlimited, and not unavailable. Each is a
   distinct state.
3. Providers are never combined into one aggregate percentage in the model.
   Aggregation is a shell rendering choice and is forbidden by invariant for
   incompatible metric kinds.
4. Credentials and raw authenticated payloads never appear in a snapshot, a
   fixture, a log, or a diagnostic. The `error` field is always redacted.
5. A provider failure never deletes the last good snapshot. It marks the
   state stale or unavailable and keeps the prior observation.

## Snapshot shape

~~~json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "UsageSnapshot",
  "type": "object",
  "additionalProperties": false,
  "required": [
    "provider",
    "account_id",
    "metric_kind",
    "window_kind",
    "unit",
    "observed_at",
    "source",
    "freshness",
    "confidence"
  ],
  "properties": {
    "provider": {
      "type": "string",
      "description": "Stable provider identifier, e.g. codex, kimi, ollama_cloud, grok_consumer, grok_api."
    },
    "account_id": {
      "type": "string",
      "description": "Stable local identifier for the account/runtime. Never a secret, token, cookie, or raw account email. Derived or redacted."
    },
    "metric_kind": {
      "enum": ["quota", "credits", "spend", "tokens", "requests", "health"]
    },
    "window_kind": {
      "enum": ["rolling", "daily", "weekly", "monthly", "session", "none"]
    },
    "used": {
      "description": "Consumed amount in the current window. Absent means not reported.",
      "type": ["number", "null"],
      "minimum": 0
    },
    "remaining": {
      "description": "Amount left in the current window. Absent means not reported. null is not the same as 0 or unlimited.",
      "type": ["number", "null"],
      "minimum": 0
    },
    "limit": {
      "description": "Total amount for the window. Absent means not reported. Unlimited must be represented by setting unlimited=true, not by a sentinel number.",
      "type": ["number", "null"],
      "minimum": 0
    },
    "unlimited": {
      "type": "boolean",
      "default": false,
      "description": "True when the provider reports no cap for this metric/window. Distinct from a missing limit."
    },
    "unit": {
      "type": "string",
      "description": "Human-readable unit, e.g. requests, tokens, usd, credits, seconds, health. Free-form but stable per provider+metric_kind."
    },
    "resets_at": {
      "type": ["string", "null"],
      "format": "date-time",
      "description": "When the window resets. null when the provider does not report a reset or window_kind is none."
    },
    "observed_at": {
      "type": "string",
      "format": "date-time",
      "description": "When the adapter observed this value. Never mutated by the cache; the cache preserves the original."
    },
    "source": {
      "enum": ["api", "cli", "local_api", "browser", "fixture", "system"],
      "description": "Where the value came from. system is reserved for registry-generated state when no provider call was made."
    },
    "freshness": {
      "enum": ["live", "cached", "stale", "unavailable", "not_configured", "not_applicable"]
    },
    "confidence": {
      "enum": ["exact", "reported_estimate", "inferred", "unknown"]
    },
    "window_label": {
      "type": ["string", "null"],
      "description": "Optional provider-native window name, e.g. primary, secondary, weekly_pool. Lets multiple windows for one provider render distinctly without the shell guessing."
    },
    "error": {
      "type": ["object", "null"],
      "additionalProperties": false,
      "required": ["code"],
      "properties": {
        "code": {
          "type": "string",
          "description": "Stable, redacted error code, e.g. auth_expired, timeout, schema_drift, rate_limited, network, unknown."
        },
        "message": {
          "type": ["string", "null"],
          "description": "Optional redacted, non-sensitive human hint. Must not contain tokens, cookies, headers, or raw response text."
        }
      },
      "description": "Present only when freshness is unavailable. Must be redacted. Never present on a live or cached success."
    }
  }
}
~~~

## State semantics

These are the only valid combinations and the meaning of each `freshness`
value. The cache and the shell must preserve these distinctions.

| freshness | meaning | used/remaining/limit | error | when set |
| --- | --- | --- | --- | --- |
| live | a fresh successful observation in this refresh cycle | populated where the provider reports them | absent | adapter success in the current refresh |
| cached | a prior live observation still within the freshness policy | preserved from the original live snapshot | absent | cache hit within policy |
| stale | the last good observation is older than the policy allows | preserved with the original observed_at and resets_at | absent | cache hit past policy, or a refresh failed but a prior snapshot exists |
| unavailable | configured, but the most recent provider call failed and no prior snapshot exists | absent | present, redacted | adapter failure with no cache |
| not_configured | the user has not enabled or authenticated the provider | absent | absent | registry decides this, not the adapter |
| not_applicable | the provider does not expose this metric | absent | absent | adapter declares this metric unsupported |

Rules:

- `unavailable` requires a redacted `error`. `stale` does not carry an
  error; it carries the last good value and is distinguished from `cached`
  only by age.
- `not_configured` is set by the provider registry, never by an adapter. An
  adapter is only invoked for configured providers.
- `not_applicable` is the only state that may be returned without a network
  or local call. It is a stable declaration that a metric does not exist for
  this provider.
- A snapshot never carries a token, cookie, authorization header, or raw
  response body. The `error.message` is the only human-readable text and it
  must be redacted.

## Value semantics

- Absent `used`, `remaining`, or `limit` means "not reported". It is not 0,
  not unlimited, and not unavailable.
- `unlimited=true` means the provider explicitly reports no cap. It is
  distinct from a missing `limit`.
- `remaining=0` is a real zero, not "unknown".
- Percentages are not stored. The shell computes a percentage only when
  `used` and `limit` are both present, `unlimited` is false, and the metric
  kind is quota, credits, or requests. The model stores raw counts and the
  unit; rendering is a shell concern.
- `resets_at` in the past is allowed and means the window has already
  rolled; the adapter should ideally report the next reset, but the model
  does not silently correct it.
- `observed_at` is immutable once set. The cache must not bump it on a
  cache hit.
- Every adapter result is validated **before** message redaction and before it
  enters the cache. Validation rejects:
  - non-finite or negative `used` / `remaining` / `limit`;
  - out-of-range percentages (`unit=percent` values outside 0–100);
  - `used > limit` when `unlimited` is false;
  - unsafe `account_id` (empty/whitespace or control characters);
  - empty `unit`;
  - `freshness=unavailable` carrying metric values or missing `error`;
  - `error` present on live/cached/stale/not_configured/not_applicable;
- Invalid windows become a redacted `schema_drift` outcome (or the previous
  stale cache entry for that window key when still within `stale_after`).
  Sibling windows from the same refresh are validated independently and are
  not dropped because another window failed.

## Multiple windows

A provider may emit multiple snapshots per refresh, one per window. They are
distinguished by `window_kind` and the optional `window_label`. The shell
renders each independently. Examples:

- Codex primary and secondary windows: two snapshots, same `provider` and
  `account_id`, different `window_label`.
- Kimi rolling 5-hour and weekly limits: two snapshots, different
  `window_kind` (rolling, weekly).

## Per-provider notes

These are planning notes, not fixtures. Real field semantics are recorded by
the evidence spikes (#2-#5) and then folded back here.

- **Codex**: plan-dependent shared agentic pool. The verified source is
  the Codex CLI app-server (`codex app-server --listen stdio://`), JSON-RPC
  method `account/rateLimits/read`. It exposes `usedPercent` (0-100), not
  raw counts, so the adapter stores `used=<percent>`, `limit=100`,
  `unit="percent"`. Multiple windows (`primary`, `secondary`) become
  separate snapshots with distinct `window_label`. `resetsAt` is Unix
  epoch seconds and must be converted to ISO 8601. `windowDurationMins`
  (10080=weekly, 1440=daily) is used to derive `window_kind`. Credits are
  a separate snapshot with `metric_kind=credits`. See
  [codex-spike.md](spikes/codex-spike.md) for the full evidence.
- **Kimi**: rolling 5-hour and weekly limits plus membership/credit cycle.
  Expect `metric_kind=quota` and `metric_kind=credits` snapshots. The
  evidence spike (#3) must confirm which surface exposes each.
- **Ollama Pro/cloud**: authenticated `GET /api/usage` is the primary source
  for separate session and weekly hosted quota fractions. Map them to separate
  `quota` snapshots (`rolling`/`session` and `weekly`/`weekly`) for one
  provider/account, scaling the reported fraction by 100. An optional
  authenticated settings fetch may enrich those snapshots with `resets_at`
  from the page's ISO `data-time` attributes. If that enrichment is unavailable,
  keep the API totals live and leave `resets_at` absent; never infer it from the
  documented five-hour/seven-day durations. Session is the default UI focus;
  model-level request rows are detail-only and deferred. See
  [ollama-spike.md](spikes/ollama-spike.md).
- **Grok consumer**: shared weekly SuperGrok pool across products.
  `metric_kind=quota`, `window_kind=weekly`, `unit="percent"`, store
  `creditUsagePercent` as `used` with `limit=100`. Source is Grok Build CLI
  auth + `cli-chat-proxy` billing (`format=credits`), not a browser scrape.
  See [grok-spike.md](spikes/grok-spike.md). Optional product breakdown rows
  are detail only; prepaid top-up cents are a separate credits snapshot.
- **Grok API**: separate adapter/account type from consumer. API RPS/TPM and
  console spend are not conflated with the consumer weekly pool; deferred
  until a dedicated API-key story.

## Fixture shape

Fixtures are redacted JSON files, one provider per directory, one file per
state. The evidence spikes (#2-#5) must produce at least:

- `normal.json` - a successful response with all reported fields;
- `multiple_windows.json` - two or more windows for the same provider;
- `unlimited_or_missing.json` - unlimited or not-reported values;
- `auth_failure.json` - `freshness=unavailable`, `error.code=auth_expired`;
- `timeout.json` - `freshness=unavailable`, `error.code=timeout`;
- `malformed.json` - `freshness=unavailable`, `error.code=schema_drift`.

Every fixture must pass the JSON Schema above and must not contain a token,
cookie, authorization header, account email, or raw response body. A secret
scan check will reject any fixture that contains known secret patterns.

## Test cases

These are the executable constraints the contract must be validated against.
They are derived from [testing-strategy.md](testing-strategy.md) and are the
acceptance tests for this contract.

### Unit

- Each `freshness` value round-trips through serialization.
- A missing `used` is not coerced to 0.
- `unlimited=true` is not coerced to a sentinel number.
- `remaining=0` is preserved as a real zero.
- `observed_at` is not mutated by a cache round trip.
- A past `resets_at` is preserved, not silently corrected.
- `error` is absent on live and cached snapshots.
- `error.code` is one of the stable redacted codes.

### Property-based

- For any snapshot, `used <= limit` when both are present and
  `unlimited=false`. Violations are rejected, not clamped.
- For any snapshot, `freshness=unavailable` implies `error` is present and
  `used`/`remaining`/`limit` are absent.
- For any snapshot, `freshness=not_configured` implies no adapter was
  invoked and no `error` is present.
- For any snapshot, `freshness=not_applicable` implies no network/local call
  was made.
- For any two snapshots from the same provider+account+metric+window, the
  one with the later `observed_at` wins. An older concurrent response cannot
  overwrite a newer one.
- Percentages computed by the shell are bounded to [0,100] or reported as
  unknown; they are never negative or over 100.

### Invariant

- The compact view never aggregates incompatible metric kinds. A combined
  percentage across quota, spend, and tokens is rejected.
- A provider failure preserves the last good snapshot and marks it stale or
  unavailable; it never deletes it.
- Cache keys cannot mix provider, account, metric, or window.
- Redaction removes tokens, cookies, authorization headers, and sensitive
  query values from every snapshot, log, and diagnostic.
- Disabling a provider removes it from refresh scheduling and the UI.
- A missing provider source produces not_configured or unavailable, never
  zero.

### Contract

- Every redacted fixture passes the JSON Schema above.
- Every fixture is free of known secret patterns.
- Each provider has at least the six fixture states listed in
  "Fixture shape".

## Acceptance for issue #1

This draft is the first step on #1, not the close. #1 closes when:

- [ ] The contract is linked from architecture.md and README.md.
- [ ] The state and value semantics above are reviewed.
- [ ] At least one redacted fixture shape is specified per planned provider
      (done above as a required list; actual fixtures come from #2-#5).
- [ ] Unit, property-based, and invariant test cases are listed before
      implementation (done above).
- [x] The contract is revised once after the Codex evidence spike (#2)
      captures the first real response, then frozen for adapter work.

The Codex revision is complete. The Grok consumer surface is verified in
[grok-spike.md](spikes/grok-spike.md). Kimi remains planning; hosted Ollama Pro
totals are verified in [ollama-spike.md](spikes/ollama-spike.md), with reset
timestamps still absent from the current API response.
~~~
