# Ollama Pro/cloud usage spike (#4)

## Decision status

**Deferred pending a supported account-usage source.** The Ollama Pro account
settings page exposes the two totals this product needs, but the documented
CLI and public API do not currently expose those account-level windows. This
spike records the evidence and the adapter admission gate; it does not add a
dashboard scraper or an undocumented authentication flow.

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

The installed Ollama CLI was inspected with:

```text
ollama --help
ollama launch --help
```

`ollama launch` is an integration launcher (for example, Codex, ChatGPT,
Kimi, and other tools). It has no usage/quota command and does not document a
way to read the account session or weekly totals.

The [documented API](https://docs.ollama.com/api) covers model operations and
request responses, not account-level usage windows. The upstream Ollama issue
[#12532](https://github.com/ollama/ollama/issues/12532) specifically requests
making the settings usage stats available through `/api/me`. A newer upstream
request, [#15663](https://github.com/ollama/ollama/issues/15663), likewise
describes account quota/usage as unavailable from the Cloud API.

The current evidence is therefore insufficient to admit a non-scraping
adapter. The settings page is evidence of the product semantics, not an
approved implementation source for this repository.

## Candidate source decision

| Source | Provides totals/reset | Auth boundary | Decision |
| --- | --- | --- | --- |
| Account settings page | Yes, visibly shows session and weekly totals | Browser session/cookies; would require page parsing | Defer; not an approved source for this slice |
| `ollama launch` CLI | No documented usage command | CLI integration setup only | Reject as a usage source |
| Public Cloud/API endpoints | No documented account-usage response | API key or account auth | Defer until Ollama documents a usage endpoint |
| Model request responses | Per-request token counters | Request authentication | Insufficient for account windows and reset times |

## Contract mapping when a source exists

The adapter should emit two `UsageSnapshot` values for the same hosted account:

| Window | `metric_kind` | `window_kind` | `window_label` | `unit` |
| --- | --- | --- | --- | --- |
| Session | `quota` | `rolling` | `session` | `percent` |
| Weekly | `quota` | `weekly` | `weekly` | `percent` |

For each snapshot, the provider-reported percentage maps to `used` with
`limit=100`; `remaining` may be derived as `100 - used` only when the source
contract guarantees that interpretation. An absolute reset timestamp is
preferred. If the source only reports a countdown, the adapter must preserve
that limitation in `confidence` and derive no more precision than the source
supports.

The shell/view-model work in the implementation story should make `session`
the default focused window and expose `weekly` through the existing provider
menu/detail path without aggregating them.

## Adapter admission gate

Issue #9 remains blocked until all of the following are available:

1. an official CLI command or documented endpoint for both totals;
2. an explicit authentication and privacy boundary that does not require
   scraping a signed-in page;
3. a redacted fixture or deterministic mock containing both windows and reset
   semantics;
4. mappings for plan/auth state, timeout, stale cache, malformed data, and
   signed-out/unavailable results;
5. a test plan that keeps model-specific request detail out of the initial UI.

Until then, the correct product behavior is to keep Ollama Pro/cloud planned
and avoid fabricating usage from per-request counters.
