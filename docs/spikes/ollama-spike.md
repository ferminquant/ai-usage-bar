# Ollama Pro/cloud usage spike (#4)

## Decision status

**Totals source found; adapter implementation is now unblocked for the first
slice.** The Ollama Pro account API exposes the two percentages this product
needs without scraping the settings page. Reset timestamps are still absent
from the API response, so the adapter must not invent them. This spike records
the live source, its authentication boundary, and the remaining reset-metadata
gap; it does not add model-specific detail or dashboard scraping.

## Product scope

The first Ollama slice is intentionally narrow:

- total session usage and total weekly usage;
- session usage is the default compact window because it is the practical
  short-term limiter;
- weekly usage is selectable from the detail/context-menu path;
- provider plan/authentication state, reset timestamps, stale data, timeout,
  malformed response, and unavailable states;
- model-specific request usage is deferred until a separate decision and issue.

The provider identifier remains `ollama_cloud` in the normalized model, with
the user-facing name Ollama Pro/cloud.

## Observed account surface

The user-provided Ollama Pro settings view shows a `Cloud usage` panel with:

- a Pro plan badge;
- `Session usage`, a percentage used and a reset countdown;
- `Weekly usage`, a percentage used and a reset countdown;
- a separate list of models used during the week, with request counts.

The model/request list is useful diagnostic detail, but it is not part of the
first usage-bar contract. The totals are the source of truth for the first
adapter slice.

## Official usage semantics

Ollama's [pricing page](https://ollama.com/pricing) states that individual
plans have session limits that reset every five hours and weekly limits that
reset every seven days. It also says usage depends on the model and on input,
cached-input, and output tokens rather than one fixed token allowance.

That means the usage bar should preserve the provider-reported percentages;
it must not infer a token limit from the model breakdown or pretend the two
windows are one combined quota.

## CLI and API investigation

The installed Ollama CLI (0.32.5) was inspected with:

```text
ollama --help
ollama launch --help
```

`ollama launch` is an integration launcher (for example, Codex, ChatGPT,
Kimi, and other tools). There is still no `ollama usage`, `ollama quota`, or
equivalent top-level command. That is a CLI-surface gap, not an API absence.

The deeper investigation traced the authentication used by the official
client and queried the account endpoint directly with the signed-in CLI key:

```text
GET https://ollama.com/api/usage
Authorization: <Ollama self-signed key token>
```

The live response contains:

```json
{
  "limits": {
    "session": { "usage": 1.0, "models": [] },
    "weekly": { "usage": 0.184, "models": [] }
  }
}
```

`usage` is a fraction in the range 0..1, so the adapter multiplies it by 100
to obtain the provider-neutral percentage. The endpoint also includes model
request counts; those are deliberately ignored for this slice. The response
does not include reset timestamps, reset countdowns, or rate-limit reset
headers.

The [documented API](https://docs.ollama.com/api) covers model operations and
request responses. Its [Usage page](https://docs.ollama.com/api/usage) documents
per-request timing/token metrics, not this account-level `/api/usage` response.
The upstream requests [#12532](https://github.com/ollama/ollama/issues/12532)
and [#15663](https://github.com/ollama/ollama/issues/15663) are historical
evidence that this surface was missing or undiscoverable; the live endpoint
now exists even though the CLI and public API documentation have not caught up.
The settings page remains useful for validating semantics, but is not an
implementation source.

## Candidate source decision

| Source | Provides totals/reset | Auth boundary | Decision |
| --- | --- | --- | --- |
| Account settings page | Yes, visibly shows session and weekly totals plus countdowns | Browser session/cookies; would require page parsing | Defer; not an approved source for this slice |
| `ollama launch` CLI | No usage command | CLI integration setup only | Reject as a usage source; use the API directly |
| `GET https://ollama.com/api/usage` | Yes, session and weekly percentages; no reset timestamps | Ollama self-signed key from the signed-in CLI | Use for totals; treat the undocumented response as a guarded contract |
| Model request responses | Per-request token counters | Request authentication | Insufficient for account windows and reset times |

## Contract mapping when a source exists

The adapter should emit two `UsageSnapshot` values for the same hosted account:

| Window | `metric_kind` | `window_kind` | `window_label` | `unit` |
| --- | --- | --- | --- | --- |
| Session | `quota` | `rolling` | `session` | `percent` |
| Weekly | `quota` | `weekly` | `weekly` | `percent` |

For each snapshot, the API fraction maps to `used = fraction * 100` with
`limit=100` and `remaining = 100 - used`. The API's `usage` field is the
provider-reported utilization, so this conversion is exact for the current
response contract. `resets_at` remains `None` until Ollama exposes a timestamp;
the adapter must not infer a session reset from the 5-hour description or a
weekly reset from a guessed calendar boundary.

The shell/view-model work in the implementation story should make `session`
the default focused window and expose `weekly` through the existing provider
menu/detail path without aggregating them.

## Adapter admission gate

Issue #9 can proceed with totals once all of the following are covered:

1. the live `GET /api/usage` response is treated as an explicitly reviewed,
   non-scraping account source, even though it is not in the public API docs;
2. the self-signed Ollama key boundary is documented and the key never enters
   logs or fixtures;
3. a redacted fixture or deterministic mock contains both windows;
4. mappings cover plan/auth state, timeout, stale cache, malformed data, and
   signed-out/unavailable results;
5. missing reset timestamps are represented honestly, rather than inferred;
6. model-specific request detail stays out of the initial UI.

The totals are now implementable. Reset countdowns and model detail remain
separate follow-up work.
