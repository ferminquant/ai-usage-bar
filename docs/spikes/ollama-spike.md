# Ollama Pro/cloud usage spike (#4)

## Decision status

**Totals source found; a hybrid adapter is now unblocked for the first slice.**
The Ollama Pro account API exposes the two percentages this product needs
without scraping the settings page. Reset timestamps are still absent from the
API response, so the adapter must not invent them. The implementation plan is
to use the API as the primary totals source and make an authenticated settings
fetch an optional reset-time enrichment. This spike records the live sources,
their authentication boundaries, and the fallback behavior; it does not add
runtime code or model-specific detail.

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
The settings page remains useful for validating semantics and is the only
observed source for exact reset timestamps. A current independent implementation
extracts ISO timestamps from `data-time` attributes on the reset elements; see
[its parser](https://github.com/steipete/CodexBar/blob/main/Sources/CodexBarCore/Providers/Ollama/OllamaUsageParser.swift)
and [provider notes](https://github.com/steipete/CodexBar/blob/main/docs/ollama.md).
That is evidence of a workable HTML contract, not an Ollama-supported API
guarantee. The implementation should make one authenticated page request and
parse the machine-readable attribute, not crawl the site or parse the rounded
countdown text.

## Candidate source decision

| Source | Provides totals/reset | Auth boundary | Decision |
| --- | --- | --- | --- |
| Account settings page | Yes, visibly shows session and weekly totals plus countdowns; current HTML carries reset timestamps in `data-time` attributes | Browser session/cookies; HTML can change and requires a parser | Use only as optional reset-time enrichment; preserve totals when it is unavailable |
| `ollama launch` CLI | No usage command | CLI integration setup only | Reject as a usage source; use the API directly |
| `GET https://ollama.com/api/usage` | Yes, session and weekly percentages; no reset timestamps | Ollama self-signed key from the signed-in CLI | Primary totals source; treat the undocumented response as a guarded contract |
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
response contract. An optional authenticated settings fetch may fill
`resets_at` from the page's ISO `data-time` attribute. If that fetch is
unavailable or the HTML contract changes, the adapter keeps the live API totals
and leaves `resets_at` absent. It must not infer a session reset from the
5-hour description or a weekly reset from a guessed calendar boundary.

The runtime adapter and shell/view-model implementation make `session` the
default focused window and expose `weekly` through the context menu without
aggregating the two windows.

## Adapter admission gate

Issue #9 can proceed with totals and optional reset enrichment once all of the
following are covered:

1. the live `GET /api/usage` response is treated as an explicitly reviewed,
   non-scraping account source, even though it is not in the public API docs;
2. the self-signed Ollama key boundary is documented and the key never enters
   logs or fixtures;
3. the settings reset parser is limited to the authenticated settings page,
   extracts `data-time` timestamps rather than display text, and never logs or
   stores the session cookie;
4. redacted fixtures or deterministic mocks cover both API windows and the
   reset-enrichment states;
5. mappings cover plan/auth state, timeout, stale cache, malformed data, and
   signed-out/unavailable results;
6. missing reset timestamps are represented honestly, rather than inferred;
7. model-specific request detail stays out of the initial UI.

The totals and reset-enrichment design are now implemented in issue #9. The
spike remains the source and contract record.

The runtime bridge accepts the browser cookie explicitly through the local
`OLLAMA_SESSION_COOKIE` environment variable. It makes one settings-page
request when present, never persists or logs the cookie, and treats missing or
expired cookies as a normal totals-only fallback. Automatic browser-cookie
discovery can be added later without changing the API-primary contract.
