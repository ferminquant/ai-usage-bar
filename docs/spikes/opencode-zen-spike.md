# OpenCode Zen credits/balance spike (#86)

Status: complete. Evidence was re-checked on **2026-08-14** against the
official OpenCode Zen documentation (`opencode.ai/docs/zen`), the provider
documentation, the upstream Zen balance API request, and public live probes of
the production hosts. No credential, cookie, account identifier, or raw
authenticated response is stored in this repository, and no authenticated
request was made for this spike: the local OpenCode installation has no Zen
key configured, and the probes below are deliberately unauthenticated public
behavior only.

## Recommendation

| Surface | Decision |
| --- | --- |
| Zen pay-as-you-go **balance** (USD credits) | **Defer / dashboard-only.** No released, documented, individual-account balance endpoint or CLI command exists as of 2026-08-14. The only upstream proposal (`GET /zen/v1/balance`, issue #10448) returns **HTTP 404** on the production host. The authenticated dashboard remains the manual fallback. |
| Proposed `/zen/v1/balance` and `/zen/v1/workspace/usage` | Do not implement against them. Both returned HTTP 404 on 2026-08-14; the feature issue is still open. |
| Local `opencode stats` ledger / local spend | **Reject as a balance source.** It is local-device session cost/tokens and is explicitly not account-authoritative. Promoting it to "balance" is prohibited by this spike's acceptance criteria. |
| `GET /zen/go/v1/usage` | Not a Zen balance source. This is the **OpenCode Go plan quota** route (upstream PR #16513 is now merged; the route responds 401 behind key auth on 2026-08-14) and reports Go rolling/weekly/monthly windows. Keep it in the Go domain. |
| Console service-account usage export | Reject as Zen balance. Service-account-only historical CSV; a different auth boundary and product surface. |
| Browser automation / dashboard scraping | Out of scope. Never scrape dashboard HTML or import browser cookies. |

**Explicit admission decision:** no production adapter for Zen balance/credits
should be admitted at this time. There is no supported individual-account
balance source, so the follow-up implementation issue (#87) should close as
**deferred** with the dashboard-only fallback documented. The deterministic
fixtures in `docs/fixtures/opencode_zen/` are test-planning artifacts for the
day a supported source is released; they are not a reason to ship an adapter
now.

## Product semantics (official docs)

The official [OpenCode Zen documentation](https://opencode.ai/docs/zen/)
(checked 2026-08-14, HTTP 200) describes Zen as:

- an optional **AI gateway**: "OpenCode Zen is a list of tested and verified
  models provided by the OpenCode team… you login to OpenCode Zen and get your
  API key";
- **pay-as-you-go billing**: "You are charged per request and you can add
  credits to your account"; billing details are added when signing in at
  `opencode.ai/auth` and the API key is copied from there;
- **auto-reload**: "If your balance goes below $5, Zen will automatically
  reload $20. You can change the auto-reload amount. You can also disable
  auto-reload entirely.";
- **monthly limits**: workspace-wide and per-member monthly usage limits set
  by admins ("you can set a monthly usage limit for the entire workspace and
  for each member of your team"); auto-reload can exceed a monthly limit when
  the balance drops below $5;
- **workspaces and roles**: Admin "Manage models, members, API keys, and
  billing"; Member "Manage only their own API keys";
- **documented endpoints** (inference only): `https://opencode.ai/zen/v1/responses`,
  `https://opencode.ai/zen/v1/messages`, `https://opencode.ai/zen/v1/chat/completions`,
  `https://opencode.ai/zen/v1/models`, and per-model endpoints. **No balance,
  credits, or usage endpoint is documented.**

The docs mention a dashboard "usage history" (low-cost models such as Haiku,
Nano, or Flash appear there), but no programmatic surface for it.

## Source matrix

| Source | Auth boundary | What it provides | Scope / freshness | Decision |
| --- | --- | --- | --- | --- |
| [Zen docs](https://opencode.ai/docs/zen/) | Public | Product semantics: pay-as-you-go, auto-reload ($5 / $20), monthly limits, workspace roles | No machine-readable balance response is documented | Product evidence only |
| [Provider docs](https://opencode.ai/docs/providers) | Public | Zen `/connect` flow: sign in, add billing details, copy API key; key stored in the local OpenCode credential file | No quota/balance command or response is documented | Auth/config evidence only |
| `GET https://opencode.ai/zen/v1/models` | Public (no key required; live probe HTTP 200) | Model list with metadata | No balance/credits fields | Inference/config probe only |
| Proposed `GET https://opencode.ai/zen/v1/balance` ([#10448](https://github.com/anomalyco/opencode/issues/10448)) | Would be the Zen API key (Bearer) | Hypothetical `balance`, `currency`, `auto_reload` fields | **Live probe 2026-08-14: HTTP 404 text/html — not deployed**; issue open | Track upstream; do not implement |
| Proposed `GET /zen/v1/usage`, `/zen/v1/workspace/usage` (issue comments) | Would be the Zen API key | Hypothetical balance + monthly usage shapes | **Live probe 2026-08-14: HTTP 404 text/html — not deployed** | Do not implement |
| `GET https://opencode.ai/zen/go/v1/usage` | Go API key (Bearer) | **Go plan** rolling/weekly/monthly usage (PR #16513, now merged); live probe 2026-08-14: **HTTP 401 JSON** `{"type":"error","error":{"type":"AuthError","message":"Missing API key."}}` — route exists behind auth | Not Zen balance; Go domain only; still not documented as a released contract in the Go docs | Keep in the Go spike/domain |
| `opencode` CLI (`stats`, `providers`, `db`) | Local provider credential file / session DB | `stats` = local-device session cost/tokens; `providers` = credential management; **no `zen`, `usage`, or `balance` command** | Local-only; cannot see other devices/workspaces; not account-authoritative | Reject as balance source |
| [Console usage export](https://console.opencode.ai/guides/usage) | Console service-account key (`oc_sk_…`); user session tokens rejected | Historical CSV (tokens, `cost_micro_cents`, `created_at`); ranges `24h`/`7d`/`30d` | Service-account scope; not Zen balance or Go allowance | Reject as Zen balance |
| Dashboard `https://opencode.ai/workspace/<id>/…` (billing/usage pages) | Web OAuth session + workspace actor | Balance, auto-reload, monthly limits, usage history (manual) | Human-visual only; HTML not a CLI API | Manual fallback only; never scrape or copy cookies |

### Live checks (2026-08-14, public/unauthenticated)

All probes below were unauthenticated GET requests to the production host.
No key, cookie, or account identifier was used or stored. Response headers
dated **Fri, 14 Aug 2026 19:47 UTC**.

```text
GET https://opencode.ai/zen/v1/models            -> HTTP 200 application/json (model list, no balance fields)
GET https://opencode.ai/zen/v1/balance           -> HTTP 404 text/html (issue #10448 proposal: not deployed)
GET https://opencode.ai/zen/v1/usage             -> HTTP 404 text/html
GET https://opencode.ai/zen/v1/workspace/usage   -> HTTP 404 text/html
GET https://opencode.ai/zen/go/v1/usage          -> HTTP 401 application/json (route exists behind key auth)
GET https://opencode.ai/zen/go/v1/models         -> HTTP 200 application/json (Go model list)
GET https://opencode.ai/zen/v1/nonexistent-xyz   -> HTTP 404 text/html
GET https://opencode.ai/zen/go/v1/nonexistent-xyz-> HTTP 404 text/html
```

Two findings matter:

1. **Unknown paths return the site's HTML 404 page.** The
   `/zen/v1/balance`, `/zen/v1/usage`, and `/zen/v1/workspace/usage` 404s are
   therefore genuine "route not registered" results, not a generic gateway
   auth response.
2. **`/zen/go/v1/usage` returns a JSON 401**, stable across repeated probes,
   with body `{"type":"error","error":{"type":"AuthError","message":"Missing API key."}}`.
   This differs from the previous spike's 2026-08-06 observation (HTTP 404)
   and matches upstream PR #16513, which is now **merged**: the Go plan quota
   route is deployed behind key auth. This is Go quota evidence, not Zen
   balance evidence, and even a successful response would report Go
   rolling/weekly/monthly percentages — never the Zen pay-as-you-go balance.

A later probe must be repeated if upstream issue #10448 changes state; do not
treat a transient 404 page, an HTML dashboard, or the Go usage route as a Zen
balance source.

## Zen balance vs OpenCode Go quota vs Console export

These are three deliberately separate product surfaces:

- **Zen pay-as-you-go balance**: USD credits added to the account and spent
  per request (`/zen/v1/*` inference endpoints, `Use balance` fallback for
  Go). Auto-reload threshold ($5) and amount ($20) and workspace monthly
  limits are Zen billing settings, not quota windows. There is currently no
  released machine-readable source for the current balance.
- **OpenCode Go plan quota**: dollar-weighted **rolling (5-hour, $12)**,
  **weekly ($30)**, and **monthly ($60)** allowance windows tracked
  server-side and now exposed (behind key auth) at `/zen/go/v1/usage`. The Go
  docs explicitly frame Zen balance as the fallback spend source: "If you
  also have credits on your Zen balance, you can enable the **Use balance**
  option in the console. When enabled, Go will fall back to your Zen balance
  after you've reached your usage limits instead of blocking requests."
- **Console service-account export**: historical CSV for organizations /
  members / service accounts / models via `console.opencode.ai` with a
  service-account key (`oc_sk_…`). It records inference cost, not a remaining
  balance.

An adapter must never merge these: a Zen balance snapshot is not a Go window,
and a Go window or Console row must never be rendered as "Zen balance".

## Auth boundary

- The Zen **API key** (created at `opencode.ai/auth`, pasted via `/connect`
  in the TUI, stored in the local OpenCode credential file
  `~/.local/share/opencode/auth.json`) is the only credential that would
  authorize a future Zen balance endpoint. The current local installation has
  **no Zen key configured** (`opencode providers list` shows only OpenCode
  Go), so this spike made no authenticated request.
- The **dashboard** uses a separate web OAuth session plus a workspace actor;
  the Go inference key does not authorize the dashboard pages (already
  verified in the OpenCode Go spike). The product must never copy browser
  cookies or scrape dashboard HTML.
- The **Console** requires a service-account key; a personal Zen or Go key
  cannot be silently substituted.

## Fields / units / currency (hypothetical contract, not admitted)

Because no source is released, field semantics below are the **proposed**
shape from upstream issue #10448 and its comments, recorded for the future
admission gate — not a released contract:

| Proposed field | Type | Meaning | Adapter note (if released) |
| --- | --- | --- | --- |
| `balance` | number | Remaining Zen balance, USD | Map to `remaining`; unit `usd`; `used` absent unless the provider also reports spend (never derive `used = limit − balance`) |
| `currency` | string | e.g. `USD` | Stored in `unit`; do not invent conversions |
| `auto_reload.enabled` | bool | Auto-reload on/off | Billing metadata, not a quota; render-only in details, never a window |
| `auto_reload.threshold` / `amount` | number | $5 threshold, $20 reload | Billing settings; the widget must never imply it can change them |
| `balance.percentOfLastTopUp` / `lastTopUp` | number/object | Issue-comment proposal | Optional detail; `null` when no top-up anchor exists — do not fake a percentage |
| monthly usage `used`/`limit`/`remaining`/`resetsAt` | numbers | Workspace/member monthly usage limit | Separate `window_kind=monthly` quota if admitted; distinct from balance |

A released balance response would map to the provider-neutral contract as
`metric_kind=credits`, `unit="usd"`, `window_kind=none`, `remaining=<balance>`,
`used`/`limit` absent, `resets_at` absent (a balance does not "reset"),
`confidence=exact` (provider-reported) and `freshness=live`. **Missing**
balance stays absent — it is never converted to zero; **zero** balance is a
real `remaining=0.0`. Auto-reload and monthly-limit fields are billing
metadata that do not fit the snapshot value fields and must be documented as
dashboard-only, not rendered as a second quota.

## Failure behavior (for a future admitted source)

Until a supported endpoint exists, the failure surface is "unavailable /
deferred": no adapter is registered, and the dashboard is the manual fallback.
If upstream #10448 is eventually released, the deterministic fixtures under
`docs/fixtures/opencode_zen/` map the expected behavior:

| Condition | Expected behavior |
| --- | --- |
| No Zen key configured locally | `not_configured` (registry decision), never zero balance |
| 401 / invalid or expired key | `unavailable` + `auth_expired`; never log the key |
| Timeout / network error | preserve last good cache as stale; else `unavailable` + `timeout` |
| Rate limit (HTTP 429) | `unavailable` + `rate_limited`; back off, keep stale cache |
| Malformed success-shaped body | `unavailable` + `schema_drift` |
| Missing balance field | absent, not zero, not unlimited, not unavailable |
| Zero balance | real `remaining=0.0` (exhausted), distinct from missing |
| Current 404 (endpoint not deployed) | `unavailable`/deferred; dashboard-only fallback; do not register a provider from an HTML page |

## Security notes

- The Zen API key stays in the local OpenCode credential file. The adapter
  must never print, log, or persist it; diagnostics and fixtures contain no
  keys, cookies, emails, account ids, or raw authenticated payloads.
- Dashboard scraping and browser-cookie import are explicitly out of scope
  (repository security policy and this issue's non-goals).
- Local `opencode stats` spend is a local-device ledger, not an account
  balance; it must never be labeled `exact` or rendered as Zen balance.
- Any future adapter must keep Zen `credits` identity separate from OpenCode
  Go quota in provider identity, cache keys, window selection, UI, and JSON
  (issue #87 requirement), and must never participate in quota percentage
  comparisons or cross-provider totals.

## Deterministic fixtures and failure mapping

Synthetic, redacted fixtures under
[`docs/fixtures/opencode_zen/`](../fixtures/opencode_zen/). They are
**hypothetical** shapes for a source that is not released; every file carries
`_status: "unreleased"` and `_decision: "defer"` metadata, and no file
contains a real credential.

| Fixture | Scenario | Behavior if the source were released |
| --- | --- | --- |
| `positive_balance.json` | Hypothetical positive balance, USD, auto-reload on | `credits` snapshot, `remaining=balance`, `used`/`limit` absent |
| `zero_balance.json` | Exhausted balance (`balance: 0`) | Real zero `remaining=0.0`; never treated as missing |
| `missing_fields.json` | Balance field absent; `currency`/`auto_reload` also absent | Missing balance stays absent (never zero, never unlimited); no unit/currency or quota shape is invented |
| `malformed.json` | Success-shaped body with wrong types | `schema_drift`/unavailable |
| `auth_failure.json` | 401 gateway error shape (same shape as the observed `/zen/go/v1/usage` 401) | `auth_expired`/unavailable; no key logged |
| `timeout.json` | Network deadline | preserve stale cache; else `timeout` |
| `rate_limited.json` | HTTP 429 | `rate_limited`/unavailable; keep stale cache |
| `unavailable.json` | **Live probe record:** `GET /zen/v1/balance` → 404 text/html on 2026-08-14 | Deferred; dashboard-only; do not register a provider |

## Admission decision

**Defer.** As of 2026-08-14 there is **no supported individual-account Zen
balance source**:

1. The official docs document no balance/credits endpoint; only inference
   endpoints (`/zen/v1/responses`, `/messages`, `/chat/completions`,
   `/models`).
2. The upstream request for `GET /zen/v1/balance` (#10448) is open, and the
   live probe returns HTTP 404.
3. The CLI has no balance/usage command, and `opencode stats` is local-device
   spend, not account balance.
4. `/zen/go/v1/usage` exists behind auth but is the **Go plan quota** surface,
   not Zen balance.

No production adapter should be admitted. Issue #87 should close as deferred
with the dashboard-only fallback documented, and this spike remains the
admission gate: re-probe the upstream issue and the documented endpoint list
before any implementation work. A future adapter, if a supported endpoint is
released, must use `MetricKind::Credits` with `unit="usd"`, preserve
provider-reported balance/zero/missing distinctions, keep Zen identity
separate from OpenCode Go quota and Console exports, and never scrape the
dashboard or import browser cookies.
