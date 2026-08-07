# OpenCode Go usage spike (#33)

Status: complete. Evidence was re-checked on 2026-08-06 against the current
OpenCode Go, provider, and CLI documentation, the OpenCode Console usage guide,
the upstream usage API request, and live Windows checks using the local
OpenCode installation. No credential, cookie, account identifier, or raw
authenticated response is stored in this repository.

The issue's “OpenCode Co” wording refers to the official product name,
**OpenCode Go**.

## Recommendation

| Surface | Decision |
| --- | --- |
| OpenCode Go plan limits | Record as product semantics only. The official docs describe dollar-weighted five-hour, weekly, and monthly limits, but these values may change. |
| `opencode` CLI and local database | A local estimate is possible: filter `opencode-go` messages, apply the model-specific Go quota weighting, and calculate each window. This is not account-authoritative and must be labeled inferred. |
| OpenCode Console usage export | Reject as the Go quota source. It is a service-account-only historical CSV export with bounded UTC ranges, not remaining allowance or reset metadata. |
| Proposed `GET /zen/go/v1/usage` | Monitor, but do not implement against it yet. The upstream request is still open and the live endpoint returned HTTP 404 on 2026-08-06. |
| Browser automation | Not required for a local estimate. The authenticated dashboard remains the exact manual fallback; do not scrape HTML or copy browser credentials. |
| Issue #34 adapter | Split the decision: an explicitly labeled local estimate can be implemented now; an exact account adapter still waits for a released, documented individual-key usage endpoint. |

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
| [Go plan docs](https://opencode.ai/docs/go/) | Go API key created through `/connect` at [opencode.ai/auth](https://opencode.ai/auth) | Plan limits, model list, Go inference endpoints, dollar-weighted semantics | No machine-readable remaining-usage response is documented; limits may change | Product evidence only |
| [Provider docs](https://opencode.ai/docs/providers) | `/connect` credentials are stored in `~/.local/share/opencode/auth.json` | Confirms the OpenCode Go provider and API-key flow | No quota command or response is documented | Auth/config evidence only |
| [CLI docs](https://dev.opencode.ai/docs/cli/) | Local provider credential file and OpenCode session database | `opencode providers list` lists configured credentials; `opencode stats` reports per-session/per-model token and raw cost statistics | No `usage`/`quota` command; model-weighted windows are an inferred local estimate and do not include other devices/workspaces | Candidate estimate source, not exact account source |
| `GET https://opencode.ai/zen/go/v1/models` | Bearer Go key | Live model discovery; confirms the Go gateway key and host work | No plan usage or reset fields | Inference/config probe only |
| [Console usage export](https://console.opencode.ai/guides/usage) | Requires a Console service-account key (`oc_sk_...`); user session tokens are rejected | Historical CSV records with token fields, billing source, `cost_micro_cents`, and `created_at`; scopes are organization/member/service-account/model | `range` is only `24h`, `7d`, or `30d`, starting at midnight UTC; no remaining allowance or reset timestamps | Reject as Go quota source; keep separate from Zen/API billing |
| [Upstream feature request #16017](https://github.com/anomalyco/opencode/issues/16017) | Requests an individual Go API-key surface | Confirms that dashboard rolling/weekly/monthly usage and reset timers currently lack a public API | Still open | Track upstream |
| [Upstream PR #16513](https://github.com/anomalyco/opencode/pull/16513) | Proposed bearer API key | Proposes `GET /zen/go/v1/usage` returning rolling, weekly, and monthly usage objects | PR is open and unmerged; live authenticated probe returned HTTP 404 on 2026-08-06 | Do not depend on it yet |

### Live checks

The local OpenCode credential was read only to perform a redacted, in-memory
probe. The command output contained no key or payload:

```text
opencode providers list
  OpenCode Go  api

GET https://opencode.ai/zen/go/v1/models   -> HTTP 200 (model list)
GET https://opencode.ai/zen/go/v1/usage    -> HTTP 404 (not deployed)
```

The 404 is important: the endpoint name is present in an upstream PR, but it
is not a supported production contract today. A later probe must be repeated
after the upstream issue/PR changes state; do not treat a transient 404 page,
an HTML dashboard, or a model response as quota data.

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
monthly allowance. Applying those weights to the local Go model costs in a
Windows smoke check reproduced the dashboard's weekly and monthly percentages
to within one percentage point. This validates the estimator as a useful
fallback; it does not turn it into an exact account API.

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

## Proposed upstream usage contract (not admitted)

The open [PR #16513](https://github.com/anomalyco/opencode/pull/16513) adds a
route at `/zen/go/v1/usage` and calls the server's rolling, weekly, and monthly
usage analyzers. Its proposed response names are `useBalance`, `rollingUsage`,
`weeklyUsage`, and `monthlyUsage`; each usage object contains a status,
`usagePercent`, and `resetInSec`.

That shape is useful evidence, but it is not an official released contract:
the PR remains open, the feature issue remains open, and the live endpoint
returned 404. The deterministic fixtures under
[`docs/fixtures/opencode/`](../fixtures/opencode/) therefore label this shape
as **unreleased**. The proposed route reports `usagePercent` and a countdown,
not account-specific dollars used/remaining. If it is eventually released, an
adapter must preserve that provider-reported percentage rather than inventing
USD usage from model prices or from the Console export. The fixtures are test
planning artifacts, not a reason to ship an adapter now.

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
| `unavailable.json` | Current production endpoint returns 404 | Dashboard/manual fallback; do not register a provider from an unrecognized response |
| `reset_unknown.json` | Usage exists but reset metadata is absent | Preserve usage if a supported source supplies it; leave `resets_at` empty and label the result honestly |
| `timeout.json` | Network deadline exceeded | Preserve stale cache when available; otherwise unavailable with `timeout` |

No fixture contains a real `Authorization` value, cookie, email, account id,
or raw authenticated dashboard payload.

## Admission decision for #34

There are now two deliberately separate implementation paths:

1. **Local estimate (available now, if the product accepts inferred data).**
   Read the local OpenCode session ledger, keep only `opencode-go` messages,
   apply the current model-specific weights, and calculate rolling, weekly,
   and monthly percentages. Mark every snapshot `source=local_api` and
   `confidence=inferred`; do not claim account-wide scope. A rolling reset can
   be approximated from local timestamps. The weekly boundary is derivable,
   but the monthly subscription anniversary and any activity from another
   device are not authoritative locally, so those reset fields must be labeled
   accordingly or left empty.
2. **Exact account adapter (preferred).** OpenCode merges and documents an
   individual-key endpoint equivalent to the proposed `/zen/go/v1/usage`, and
   a live Windows smoke test confirms all three windows plus reset metadata; or
   OpenCode documents another supported individual-subscriber endpoint with
   the same semantics.

In either path, preserve separate `rolling`, `weekly`, and `monthly` identity,
keep Go allowance separate from Zen balance and Console service-account
exports, and never scrape the dashboard or import browser cookies. The
provider API key is the correct credential for the Go gateway; the browser
OAuth session is a separate dashboard auth boundary.
