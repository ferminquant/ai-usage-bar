# OpenCode Go usage spike (#33)

Status: complete. Evidence was re-checked on 2026-08-14 against the current
OpenCode Go, provider, and CLI documentation, the OpenCode Console usage guide,
the deployed Go usage endpoint, and live Windows checks using the local
OpenCode installation. No credential, cookie, account identifier, or raw
authenticated response is stored in this repository.

The issue's “OpenCode Co” wording refers to the official product name,
**OpenCode Go**.

## Recommendation

| Surface | Decision |
| --- | --- |
| OpenCode Go plan limits | Record as product semantics only. The official docs describe dollar-weighted five-hour, weekly, and monthly limits, but these values may change. |
| `opencode` CLI and local database | A local estimate remains available when no Go key is present: filter `opencode-go` messages, apply the model-specific Go quota weighting, and calculate each window. It is not account-authoritative and is labeled inferred. |
| OpenCode Console usage export | Reject as the Go quota source. It is a service-account-only historical CSV export with bounded UTC ranges, not remaining allowance or reset metadata. |
| `GET /zen/go/v1/usage` | **Use as the preferred account source.** A live authenticated check on 2026-08-14 returned rolling, weekly, and monthly percentages plus `resetsAt` timestamps. |
| Browser automation | Not required for a local estimate. The authenticated dashboard remains the exact manual fallback; do not scrape HTML or copy browser credentials. |
| Issue #34 adapter | The adapter prefers the exact individual-key usage endpoint and reads the existing `opencode-go` key in memory only. The local estimator remains the no-key fallback; reset-anchor editing is retained for that fallback. |

## Product semantics

The official [OpenCode Go documentation](https://opencode.ai/docs/go/) currently
describes:

- a five-hour limit of **$12**;
- a weekly limit of **$30**; and
- a monthly limit of **$60**.

These are dollar-weighted allowances. The request count varies by model and
token mix, so the adapter must not turn model request estimates into a fake
token quota. The same page publishes a model-specific **Usage** tier (for
example, $60 or $15 of included usage) and explains that Go applies a
model-dependent multiplier. That makes a local estimate possible when the
local session ledger contains the model and raw cost. The estimate must still
be marked inferred because the model table and multipliers can change, and the
local ledger cannot see usage from another device or workspace. The plan is
separate from OpenCode Zen's pay-as-you-go balance; the Go docs describe
Zen-balance fallback as a separate setting rather than another Go window.

## Source matrix

| Source | Auth boundary | What it provides | Reset/freshness behavior | Decision |
| --- | --- | --- | --- | --- |
| [Go plan docs](https://opencode.ai/docs/go/) | Go API key created through `/connect` at [opencode.ai/auth](https://opencode.ai/auth) | Plan limits, model list, Go inference endpoints, dollar-weighted semantics | The general plan page documents the limits, while the authenticated gateway supplies current usage | Product semantics and auth evidence |
| [Provider docs](https://opencode.ai/docs/providers) | `/connect` credentials are stored in `~/.local/share/opencode/auth.json` | Confirms the OpenCode Go provider and API-key flow | The usage route is a gateway surface rather than an `opencode` CLI command | Auth/config evidence |
| [CLI docs](https://dev.opencode.ai/docs/cli/) | Local provider credential file and OpenCode session database | `opencode providers list` lists configured credentials; `opencode stats` reports per-session/per-model token and raw cost statistics | No `usage`/`quota` command; model-weighted windows are an inferred local estimate and do not include other devices/workspaces | Candidate estimate source, not exact account source |
| `GET https://opencode.ai/zen/go/v1/models` | Bearer Go key | Live model discovery; confirms the Go gateway key and host work | No plan usage or reset fields | Inference/config probe only |
| `GET https://opencode.ai/zen/go/v1/usage` | Bearer Go key | Account-authoritative rolling, weekly, and monthly percentages plus reset timestamps | Individual-key route is not described in the general CLI docs; preserve a stale-cache fallback if it changes | **Preferred account source** |
| [Console usage export](https://console.opencode.ai/guides/usage) | Requires a Console service-account key (`oc_sk_...`); user session tokens are rejected | Historical CSV records with token fields, billing source, `cost_micro_cents`, and `created_at`; scopes are organization/member/service-account/model | `range` is only `24h`, `7d`, or `30d`, starting at midnight UTC; no remaining allowance or reset timestamps | Reject as Go quota source; keep separate from Zen/API billing |
| [Upstream feature request #16017](https://github.com/anomalyco/opencode/issues/16017) | Historical request for an individual Go API-key surface | Documents the original gap between the dashboard and CLI | The deployed route now supplies the requested fields | Historical context |
| [Upstream PR #16513](https://github.com/anomalyco/opencode/pull/16513) | Proposed bearer API key | Documents the route shape that is now live | Re-check the response shape if OpenCode changes the gateway | Implementation evidence |

### Live checks

The local OpenCode credential was read only to perform a redacted, in-memory
probe. The command output contained no key or payload:

```text
opencode providers list
  OpenCode Go  api

GET https://opencode.ai/zen/go/v1/models   -> HTTP 200 (model list)
GET https://opencode.ai/zen/go/v1/usage    -> HTTP 200 (rolling/weekly/monthly + resetsAt)
```

The usage endpoint is now live, but it is still a provider gateway surface
rather than a documented `opencode usage` CLI command. The adapter validates
the three windows and their reset timestamps, never stores the key, and keeps
the local ledger estimator only as a no-key fallback.

### Workspace dashboard route

The authenticated dashboard route has the form
`https://opencode.ai/workspace/<workspace-id>/go`. Fetching the user's exact
route with curl returned an HTTP redirect to `auth.opencode.ai/authorize`.
Supplying the Go inference key as `Authorization: Bearer` did not change that
result: the page requires the web login/session and workspace actor, not the
provider key used by the Go model gateway.

This is useful as a manual browser fallback, but it is not a CLI API. The
workspace id, browser cookie, and rendered HTML are intentionally not stored
in the repository. The product must not copy those cookies or scrape the page
to manufacture a quota source.

## CLI and local auth findings

The installed CLI exposes provider credential management and local statistics:

```text
opencode providers list
opencode providers login
opencode providers logout
opencode stats
```

The documented credential path is `~/.local/share/opencode/auth.json`. The
current Windows installation resolves this to the user's Windows home
directory. The file contains an `opencode-go` provider entry with a bearer key;
the spike read only the provider name and non-secret metadata during the live
check.
The adapter must never print or persist that key, and it must not initiate a
new `/connect` or browser login flow on the user's behalf.

`opencode stats` is useful for local session token/cost history and model
breakdowns. Its cost is the raw model cost, not the Go quota cost. The server
tracks Go quota by multiplying that cost by the model's `costMultiplier` before
updating the rolling, weekly, and monthly counters; the published Go model
table exposes the corresponding Usage tiers. Therefore the local ledger can
produce an **inferred** percentage when it is filtered to `opencode-go` and
weighted with the current model table. It remains non-authoritative: it cannot
see usage from another device/workspace, and it cannot guarantee the server's
current multiplier table or subscription anniversary.

For the current published table, models with a $60 Usage tier have a 1x
weight, while models with a $15 Usage tier have a 4x weight against Go's $60
monthly allowance. A Windows smoke check showed that the local ledger can omit
usage visible in the dashboard (for example, other devices or workspaces), so
its weighted estimate did not match the account windows. This keeps the
estimator useful as a no-key fallback, but it is precisely why the adapter now
prefers the authenticated account endpoint.

## Console export boundary

The Console guide documents:

```text
GET https://console.opencode.ai/api/v1/usage/export
Authorization: Bearer <service-account-key>
scope=organization|member|service_account|model
range=24h|7d|30d
Accept: text/csv
```

The response is a historical CSV. `cost_micro_cents` is Console-managed
inference cost (100,000,000 microcents per USD), not a Go subscription balance.
The documented `401`/`403` errors concern missing, expired, revoked, or
unauthorized **service-account** keys. This boundary means a personal Go key
cannot be silently substituted for a Console service key, and an export row
cannot be converted into a remaining percentage or reset timestamp.

## Authoritative usage contract

The deployed [gateway route](https://opencode.ai/zen/go/v1/usage) now returns
the account-authoritative shape used by the adapter:

```json
{
  "usage": {
    "rolling": { "percent": 22, "resetsAt": "2026-08-14T17:43:38.318Z" },
    "weekly": { "percent": 83, "resetsAt": "2026-08-17T00:00:00.318Z" },
    "monthly": { "percent": 60, "resetsAt": "2026-09-05T14:03:20.318Z" }
  }
}
```

The response is fetched with the existing `opencode-go` bearer key, and the
adapter preserves the provider-reported percentages and RFC3339 reset times
when present. It validates all three windows and treats missing, malformed,
out-of-range, or non-finite usage values (and malformed reset timestamps) as
schema drift; a missing reset field remains explicitly unknown. The older open [PR
#16513](https://github.com/anomalyco/opencode/pull/16513) remains useful as
implementation history, but the running service response—not that proposal—
is the contract this repository consumes. The deterministic fixtures under
[`docs/fixtures/opencode/`](../fixtures/opencode/) remain synthetic planning
artifacts and are not captured account payloads.

## Deterministic fixtures and failure mapping

The fixtures are synthetic and contain no personal data. They cover the
proposed upstream response and the source failures that an eventual adapter
must handle:

| Fixture | Scenario | Required behavior for #34 |
| --- | --- | --- |
| `normal.json` | All three proposed windows with reset seconds | Keep rolling, weekly, and monthly windows separate; preserve provider reset metadata |
| `missing_rolling.json`, `missing_weekly.json`, `missing_monthly.json` | Each sibling window omitted in turn | Keep valid siblings; do not infer the missing window or turn it into zero |
| `auth_failure.json` | 401/missing or expired Go key | `not_configured` for no key; otherwise `auth_expired`/unavailable; never log the key |
| `malformed.json` | Success-shaped response with wrong types | `schema_drift`/unavailable; retain last good cache only as stale |
| `unavailable.json` | Gateway route is unavailable or changes status | Preserve a recent cache as stale; otherwise report the provider as unavailable |
| `reset_unknown.json` | Usage exists but reset metadata is absent | Preserve usage if a supported source supplies it; leave `resets_at` empty and label the result honestly |
| `timeout.json` | Network deadline exceeded | Preserve stale cache when available; otherwise unavailable with `timeout` |

No fixture contains a real `Authorization` value, cookie, email, account id,
or raw authenticated dashboard payload.

## Admission decision for #34

There are now two deliberately separate implementation paths:

1. **Local estimate (fallback).**
   Read the local OpenCode session ledger, keep only `opencode-go` messages,
   apply the current model-specific weights, and calculate rolling, weekly,
   and monthly percentages. Mark every snapshot `source=local_api` and
   `confidence=inferred`; do not claim account-wide scope. A rolling reset can
   be approximated from local timestamps. The weekly boundary is derivable,
   but the monthly subscription anniversary and any activity from another
   device are not authoritative locally. The shell lets the user set the next
   weekly and monthly anchors; clearing them returns to the built-in
   Monday/first-of-month defaults. The rolling reset is estimated from the
   latest local Go event.
2. **Exact account adapter (implemented and preferred).** The live
   `/zen/go/v1/usage` route accepts the existing individual Go key, and a
   Windows smoke test confirms all three windows plus reset metadata. The
   adapter uses it whenever that key is available; the local estimator is
   retained only for installations where no key can be found.

In either path, preserve separate `rolling`, `weekly`, and `monthly` identity,
keep Go allowance separate from Zen balance and Console service-account
exports, and never scrape the dashboard or import browser cookies. The
provider API key is the correct credential for the Go gateway; the browser
OAuth session is a separate dashboard auth boundary.
