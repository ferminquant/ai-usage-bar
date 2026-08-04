# Grok consumer / API usage spike (#5)

Status: complete. Evidence captured from official Grok Build CLI auth and the
CLI chat-proxy billing endpoint used by `/usage` in the open-source
`xai-org/grok-build` client (verified live on 2026-08-04).

## Recommendation

| Surface | Provider id | Decision |
| --- | --- | --- |
| SuperGrok / X Premium+ **consumer** weekly pool (Build, Chat, Imagine, …) | `grok_consumer` | **Implement** |
| xAI **API** team rate limits (RPS/TPM) and console spend | `grok_api` | **Defer** (separate product; console/API-key path, not the same remaining-usage bar) |
| Browser bridge to grok.com Settings → Usage | n/a | **Not required** for consumer remaining usage when Grok Build is logged in |

## Why consumer is implementable without a browser bridge

The official Grok Build CLI (`grok`, this machine: `%USERPROFILE%\.grok\bin\grok.exe`
v0.2.118) already:

1. Signs in via OIDC (`grok login` / `auth.x.ai`) into `~/.grok/auth.json`.
2. Calls the same authorized proxy the TUI uses for `/usage`:

   ~~~text
   GET https://cli-chat-proxy.grok.com/v1/billing?format=credits
   Authorization: Bearer <session key from ~/.grok/auth.json>
   X-XAI-Token-Auth: xai-grok-cli
   x-userid: <user_id from auth.json>
   x-grok-client-version: <cli version>
   ~~~

3. Surfaces that response through the agent extension method `x.ai/billing`
   (open source: `crates/codegen/xai-grok-shell/src/extensions/billing.rs`).

That is a **user-authorized, first-party client surface** — not a hidden
scrape or cookie bypass. An adapter should reuse the same URL, headers, and
local session store the CLI uses.

## Consumer product semantics (public docs)

From [Grok FAQ — Usage & Limits](https://docs.x.ai/grok/faq) (checked 2026-08-04):

- Paid SuperGrok / product access uses **one shared weekly usage pool** across
  Grok products (Chat, Imagine, Voice, Build, …).
- Usage is shown as a **percentage used**, with product breakdown and a
  weekly reset time (Settings → Usage on web/mobile).
- Hitting the weekly limit pauses paid features until reset; Extra Usage
  Credits / auto top-up / plan upgrade are separate options.
- Free-tier Chat/Voice limits are **separate** from the weekly pool.

## Live capture (redacted)

Date: **2026-08-04**. Auth: existing `grok login` OIDC session in
`~/.grok/auth.json`. Request as above; HTTP **200**.

Observed body shape (values illustrative / redacted; see fixtures):

~~~json
{
  "config": {
    "currentPeriod": {
      "type": "USAGE_PERIOD_TYPE_WEEKLY",
      "start": "2026-08-04T13:28:26.395580+00:00",
      "end": "2026-08-11T13:28:26.395580+00:00"
    },
    "creditUsagePercent": 8.0,
    "onDemandCap": { "val": 0 },
    "onDemandUsed": { "val": 0 },
    "productUsage": [
      { "product": "GrokBuild", "usagePercent": 8.0 }
    ],
    "isUnifiedBillingUser": true,
    "prepaidBalance": { "val": 0 },
    "topUpMethod": "TOP_UP_METHOD_SAVED_PAYMENT_METHOD",
    "billingPeriodStart": "2026-08-04T13:28:26.395580+00:00",
    "billingPeriodEnd": "2026-08-11T13:28:26.395580+00:00"
  }
}
~~~

### Field semantics (consumer billing response)

| Field | Type | Meaning | Adapter notes |
| --- | --- | --- | --- |
| `config.creditUsagePercent` | f64 | Included allowance used, **0–100** | Primary compact metric. Map to `used` with `limit=100`, `unit="percent"`. |
| `config.currentPeriod.type` | string | e.g. `USAGE_PERIOD_TYPE_WEEKLY` | Derive `window_kind=weekly` when weekly. |
| `config.currentPeriod.start` / `end` | RFC 3339 | Window bounds | Prefer `end` as `resets_at`. Fall back to deprecated `billingPeriodEnd`. |
| `config.productUsage[]` | array | Per-product percent of pool | Detail-only rows; **do not** sum into a second quota icon. Optional secondary snapshots with `window_label` = product name. |
| `config.prepaidBalance.val` | i64 | Extra / top-up credits in **USD cents** | Separate `metric_kind=credits` snapshot if present and meaningful; never merge into the weekly % pool. |
| `config.onDemandUsed` / `onDemandCap` | Cent | On-demand spend vs cap (cents) | Detail/diagnostics; not the primary weekly bar. |
| `config.isUnifiedBillingUser` | bool | Shared weekly/monthly pool user | Expect `true` for modern SuperGrok weekly pool. |
| `subscriptionTier` / remote settings | string | Tier display name | Optional diagnostics; may be filled by CLI from remote settings, not always in this JSON. |

Open-source `BillingConfig` also documents **legacy** fields (`monthlyLimit`,
`used` as cents) for older `GetGrokBuildBillingConfig` responses. Prefer
`creditUsagePercent` + `currentPeriod`; fall back to legacy only when the
new fields are absent.

## Mapping to the snapshot contract

| Source | Snapshot field | Value |
| --- | --- | --- |
| (fixed) | `provider` | `grok_consumer` |
| redacted user/team id | `account_id` | stable hash of `user_id` or `team_id`, never email |
| `creditUsagePercent` | `used` | percent used |
| (derived) | `limit` | `100` |
| (derived) | `remaining` | `100 - used` when finite |
| (fixed) | `unit` | `percent` |
| (fixed) | `metric_kind` | `quota` |
| period type weekly | `window_kind` | `weekly` |
| `currentPeriod.end` | `resets_at` | ISO-8601 |
| (fixed) | `window_label` | `primary` for the shared pool |
| product rows | optional extra snapshots | `window_label` = product id; still `metric_kind=quota` only if product % is a share of the same pool (detail), not a second independent limit |
| `prepaidBalance` | optional credits snapshot | `metric_kind=credits`, unit `cents` or `usd` after conversion |
| (fixed) | `source` | e.g. `cli` / dedicated Grok source once enum allows; use closest existing (`Cli` or document new) |
| successful fetch | `freshness` | `live` |

## Auth and session details

- Store: `~/.grok/auth.json` (Windows: `%USERPROFILE%\.grok\auth.json`).
- Shape: map keyed by `https://auth.x.ai::<oidc_client_id>` → session object.
- Relevant fields: `key` (Bearer), `refresh_token`, `expires_at`, `user_id`,
  `email`, `team_id`, `auth_mode` (`oidc`).
- Access tokens are **short-lived** (capture saw ~6h). Adapter must refresh
  via the same OIDC refresh path the CLI uses before calling billing, or
  surface `auth_expired` when refresh fails.
- Missing/empty auth file → `freshness=not_configured`, not zero usage.

## API surface (separate; defer for remaining-usage bar)

Documented xAI **API** usage is a different account type:

| Concern | Source | Shape |
| --- | --- | --- |
| Rate limits | [Rate limits](https://docs.x.ai/developers/rate-limits), Console Rate Limits page | Per-model **RPS** and **TPM** by spend tier — not a SuperGrok weekly % |
| Spend / tokens | [Usage explorer](https://docs.x.ai/console/usage) | Team admin dashboards; credit consumption, filters by API key/model |
| Auth | API key (`XAI_API_KEY` / console keys) | Not `~/.grok/auth.json` OIDC consumer session |

**Do not** show API RPS/TPM as “remaining SuperGrok usage.” If an API adapter
is desired later, use provider id `grok_api`, API-key config, and metrics that
match rate-limit or spend semantics — not the consumer weekly pool.

## Browser bridge

**Not recommended for the first Grok consumer adapter.**

If a future path needs web-only fields the billing endpoint does not expose:

- **Opt-in only** in config (default off).
- Read-only access to user-authorized session (no password storage in this
  project).
- Least privilege: usage page only; no write actions.
- Redact cookies/tokens from all logs and fixtures.
- Fallback: last good cache → stale; else unavailable with redacted error.
- Prefer extending the official CLI/proxy path over scraping HTML.

## Session `/usage` vs account billing

Inside Grok Build, `/usage` (alias `/cost`) shows **session** token/cost
totals (`x.ai/session/usage`) and, for consumer accounts, can open billing
management. Session token totals are **not** the subscription remaining pool.
The adapter for the usage bar must call **account billing**
(`…/billing?format=credits` / `x.ai/billing`), not session ledger usage.

## Failure modes (planned tests)

| Condition | Expected snapshot |
| --- | --- |
| No `auth.json` / logged out | `not_configured` |
| Token expired and refresh fails | `unavailable` + `auth_expired` |
| Network / proxy timeout | preserve cache as stale; else `unavailable` + `timeout` |
| HTTP 4xx/5xx from billing | redacted error; `schema_drift` only if body is unparseable success-shaped junk |
| Missing `creditUsagePercent` and no legacy fallback | `schema_drift` |
| Percent outside 0–100 | reject via contract validation |

## Security notes

- Reuse only the **same** host and auth headers the official CLI uses
  (`cli-chat-proxy.grok.com`, `X-XAI-Token-Auth: xai-grok-cli`).
- Never log `Authorization`, `key`, `refresh_token`, cookies, or raw
  `auth.json`.
- Redact `email`, raw `user_id`, and `team_id` in fixtures (stable hashed
  `account_id` only).
- Do not propose reverse-engineered private endpoints beyond this
  first-party CLI surface; if the path disappears, fall back to
  dashboard-only / unavailable.

## Open-source evidence pointers

- CLI billing extension:
  [`billing.rs`](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-shell/src/extensions/billing.rs)
- Slash `/usage`:
  [`usage.rs`](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/src/slash/commands/usage.rs)
- Public install / CLI: https://x.ai/news/grok-build-cli , https://github.com/xai-org/grok-build

## Fixtures

Redacted fixtures: `docs/fixtures/grok_consumer/`.

## Follow-up

- Story **#18** should implement `grok_consumer` against this surface and
  fixtures; keep `grok_api` out of scope until a separate API-key spike/story
  is filed if needed.
