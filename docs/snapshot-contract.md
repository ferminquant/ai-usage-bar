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

Draft. Field names and value semantics are frozen enough to unblock the
provider evidence spikes (#2-#5) and the daemon (#6). They are expected to
be revised once when the first real provider response (Codex) is captured,
and only then. Adapter implementation must not start before that revision.

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
      "description": "Stable provider identifier, e.g. codex, kimi, ollama_local, ollama_cloud, grok_consumer, grok_api."
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
      "enum": ["api", "cli", "local_api", "browser", "fixture"]
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
| not_applicable | the provider does not expose this metric (e.g. Ollama local has no hosted quota) | absent | absent | adapter declares this metric unsupported |

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

## Multiple windows

A provider may emit multiple snapshots per refresh, one per window. They are
distinguished by `window_kind` and the optional `window_label`. The shell
renders each independently. Examples:

- Codex primary and secondary windows: two snapshots, same `provider` and
  `account_id`, different `window_label`.
- Kimi rolling 5-hour and weekly limits: two snapshots, different
  `window_kind` (rolling, weekly).
- Ollama local telemetry: one snapshot with `metric_kind=health` or
  `metric_kind=tokens`, `window_kind=session` or `none`, and
  `freshness=not_applicable` for any hosted-quota metric the adapter is
  asked for.

## Per-provider notes

These are planning notes, not fixtures. Real field semantics are recorded by
the evidence spikes (#2-#5) and then folded back here.

- **Codex**: plan-dependent shared agentic pool. Expect multiple windows
  with reset times. `metric_kind=quota`, `source=cli` or `source=api`.
  Reset times are required for a live snapshot.
- **Kimi**: rolling 5-hour and weekly limits plus membership/credit cycle.
  Expect `metric_kind=quota` and `metric_kind=credits` snapshots. The
  evidence spike (#3) must confirm which surface exposes each.
- **Ollama local**: local runtime telemetry, not a hosted quota.
  `metric_kind=tokens` or `metric_kind=health`, `window_kind=session` or
  `none`. Any hosted-quota metric requested for Ollama local must return
  `not_applicable`, never a fake percentage.
- **Ollama cloud**: separate account from local. Cloud quota windows, if a
  supported surface exists, are a separate `account_id`. The evidence spike
  (#4) decides whether cloud is implement or defer.
- **Grok consumer**: shared weekly pool across supported products.
  `metric_kind=quota`, `window_kind=weekly`. The evidence spike (#5) decides
  whether this is a browser bridge or deferred.
- **Grok API**: separate adapter/account type from consumer. API rate
  limits and spend are not conflated with consumer subscription usage.

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

- Local Ollama telemetry is never classified as a hosted quota. A snapshot
  with `provider=ollama_local` and `metric_kind=quota` is rejected.
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
- [ ] The contract is revised once after the Codex evidence spike (#2)
      captures the first real response, then frozen for adapter work.

The final checkbox is intentionally not checked. The contract is expected
to change once when real Codex data lands, and only then.
~~~