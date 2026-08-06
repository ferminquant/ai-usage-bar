# Ollama Pro/cloud usage spike (#4)

## Decision status

**Totals source found; reset metadata remains an upstream dependency.**
The Ollama Pro account API exposes the two percentages this product needs
without scraping the settings page. Reset timestamps are still absent from the
API response, so the adapter must not invent them. The runtime uses the API as
the primary totals source and the shell offers a link to the settings page in
the user's normal browser. This spike records the live sources and the
authentication boundary; it does not add model-specific detail or a browser
scraper.

## Product scope

The first Ollama slice is intentionally narrow:

- total session usage and total weekly usage;
- session usage is the default compact window because it is the practical
  short-term limiter;
- weekly usage is selectable from the detail/context-menu path;
- provider plan/authentication state, reset metadata when Ollama exposes it,
  stale data, timeout, malformed response, and unavailable states;
- model-specific request usage is deferred until a separate decision and issue.

The provider identifier remains `ollama_cloud` in the normalized model, with
the user-facing name Ollama.

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
Ollama issue [#12532](https://github.com/ollama/ollama/issues/12532) remains the
upstream tracking point for exposing account usage in a supported, digestible
API shape; local issue [#35](https://github.com/ferminquant/ai-usage-bar/issues/35)
tracks when reset metadata becomes available. The live endpoint now provides
totals, but still omits reset timestamps and reset headers. The settings page is
therefore a useful manual fallback only; the application does not scrape it or
copy browser credentials.

On Windows, `ollama` may be a Windows client talking to an Ollama daemon that
is actually running in WSL. Those environments have separate key files even
though the local daemon reports the signed-in account. The adapter first uses
the native Windows key and, after an upstream authentication rejection, retries
with the default WSL key through `wsl.exe`; it does not sign out, copy the key,
or interrupt the daemon's active sessions. An explicit `OLLAMA_ID` or
`OLLAMA_HOME` remains authoritative and disables that automatic fallback.

## Candidate source decision

| Source | Provides totals/reset | Auth boundary | Decision |
| --- | --- | --- | --- |
| Account settings page | Yes, visibly shows session and weekly totals plus countdowns | User's normal browser session; no app-side cookie or HTML access | Open from the Ollama context menu as a manual fallback |
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
response contract. `resets_at` remains absent until Ollama exposes a supported
machine-readable reset field. The adapter must not infer a session reset from
the 5-hour description or a weekly reset from a guessed calendar boundary.

The runtime adapter and shell/view-model implementation make `session` the
default focused window and expose `weekly` through the context menu without
aggregating the two windows.

## Adapter admission gate

Issue #9 is complete for totals. Reset enrichment remains deferred until all of
the following are true:

1. the live `GET /api/usage` response is treated as an explicitly reviewed,
   non-scraping account source, even though it is not in the public API docs;
2. the self-signed Ollama key boundary is documented and the key never enters
   logs or fixtures;
3. Ollama documents a stable reset timestamp or reset-header contract;
4. redacted fixtures or deterministic mocks cover both API windows and the
   reset metadata;
5. mappings cover plan/auth state, timeout, stale cache, malformed data, and
   signed-out/unavailable results;
6. missing reset timestamps are represented honestly, rather than inferred;
7. model-specific request detail stays out of the initial UI.

The totals implementation is complete. Until the upstream contract changes,
the shell's **Open Ollama usage page** action is the supported reset-time
workaround; no extension, cookie import, or HTML parser is part of the product.
