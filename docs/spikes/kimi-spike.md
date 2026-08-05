# Kimi usage spike (#3)

Status: complete. Evidence captured from the official open-source Kimi Code
CLI (`MoonshotAI/kimi-code`), the Kimi Code membership docs, and a Kimi Code
team post on the official forum.

## Recommendation

| Surface | Provider id | Decision |
| --- | --- | --- |
| Kimi Code membership quota: weekly (7-day) plan window plus rolling 5-hour window | `kimi` | **Implement** |
| Extra Usage wallet (top-up balance, monthly spending cap) | `kimi` (separate credits snapshot) | **Implement** with the same `/usages` response |
| Kimi Code Console (`https://www.kimi.com/code/console`) | n/a | Manual fallback only; open in the user's OS browser, never scraped |
| Kimi Open Platform (`platform.moonshot.cn` / API-key billing balance) | n/a | **Defer / out of scope**; a separate pay-as-you-go product, not the membership remaining-usage bar |
| Browser bridge to kimi.com | n/a | **Not required** when the CLI session or an API key is present |

## Why Kimi is implementable without a browser bridge

The official Kimi Code CLI (`kimi`, open source at
[MoonshotAI/kimi-code](https://github.com/MoonshotAI/kimi-code)) already:

1. Signs in with a device-code OAuth flow (`kimi login` / `/login`) against
   `https://auth.kimi.com` and persists the token bundle under the CLI data
   root (Windows: `C:\Users\<name>\.kimi-code\credentials\kimi-code.json`).
2. Calls a **managed usage endpoint** on the same base URL the CLI uses for
   every request: `GET https://api.kimi.com/coding/v1/usages`, authenticated
   with `Authorization: Bearer <access_token>`.
3. Parses that response into the quota model rendered by the interactive
   `/usage` slash command (alias `/status`).

The adapter can reuse the CLI's local session and call the same endpoint. No
HTML scraping, cookie copying, or hidden endpoint is needed.

## Public product semantics (official docs)

From [Membership Benefits](https://www.kimi.com/code/docs/en/kimi-code/membership.html)
(checked 2026-08-05):

- Kimi Code is a benefit inside the Kimi membership; the same quota is shared
  with Kimi on the web/app and across all logged-in devices and API keys.
- The weekly quota **refreshes every 7 days from the subscription date**;
  unused quota does not roll over. The 5-hour rolling window is a rate
  window that recovers automatically when it rolls over.
- If the shared Kimi membership **monthly** quota is exhausted, Kimi Code is
  frozen until the monthly reset or an upgrade. This is a third, separate
  pool that can override the two Kimi Code windows.
- **Extra Usage** is a prepaid balance (shown in RMB/CNY) shared between Kimi
  web and Kimi Code. It is deducted last, once subscription quota runs out,
  has an optional **monthly spending cap**, and is usable after a
  subscription lapses. The balance never expires and stacks across top-ups.
- The CLI `/usage` command shows token usage, context consumption, and quota
  information; the Console shows remaining quota and rate-limit status.

Error semantics from the [Error Reference](https://www.kimi.com/code/docs/en/kimi-code/error-reference.html):

- `429 "You've reached your usage limit for this period"` — 5-hour rolling
  window reached; wait for the reset shown in the Console.
- `429 "You've reached kimi monthly usage limit"` — shared monthly quota
  exhausted; all Kimi benefits freeze.
- `403 "usage limit for this billing cycle"` — weekly quota fully used.

## Verified surface: managed `/usages` endpoint

Official source: `packages/oauth/src/managed-usage.ts` in
[MoonshotAI/kimi-code](https://github.com/MoonshotAI/kimi-code).

```text
GET https://api.kimi.com/coding/v1/usages
Authorization: Bearer <access_token>
Accept: application/json
```

The CLI's default timeout for this call is 8 seconds. The endpoint accepts
both OAuth bearer tokens (managed `kimi-code` session) and Console-created
API keys — the Kimi Code team demonstrated the API-key form publicly
([forum post #191](https://forum.moonshot.ai/t/error-code-429-were-receiving-too-many-requests-at-the-moment/191)).

Observed response shape (values illustrative/redacted; see fixtures):

```json
{
  "usage": {
    "used": "33",
    "limit": "100",
    "remaining": "67",
    "resetTime": "2026-08-10T09:20:45.248979Z"
  },
  "limits": [
    {
      "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" },
      "detail": {
        "used": "2",
        "limit": "100",
        "remaining": "98",
        "resetTime": "2026-08-05T11:20:45.248979Z"
      }
    }
  ],
  "boosterWallet": {
    "balance": { "type": "BOOSTER", "amount": 1500000000, "amountLeft": 1250000000 },
    "monthlyChargeLimit": { "priceInCents": 10000, "currency": "CNY" },
    "monthlyUsed": { "priceInCents": 4000, "currency": "CNY" },
    "monthlyChargeLimitEnabled": true
  }
}
```

### Field semantics (managed usage response)

| Field | Type | Meaning | Adapter notes |
| --- | --- | --- | --- |
| `usage.used` / `usage.limit` / `usage.remaining` | decimal string | Plan **weekly** allowance: used, total, left. The backend omits the window; the CLI synthesizes `window = 1 week`. Values are plan-normalized 0–100 units (the CLI renders them as `% left`). | Primary weekly snapshot: `used`, `limit=100`-shaped values, `unit=percent`. |
| `usage.resetTime` | ISO-8601 string | When the weekly window resets (7 days from subscription date) | `resets_at`; **do not** substitute a calendar-week boundary. |
| `limits[]` | array | Additional windows, e.g. the rolling 5-hour rate window | One snapshot per row. `window.duration`/`timeUnit` map to `window_kind` (`300 TIME_UNIT_MINUTE` → `rolling` 5h). |
| `limits[].detail.used/limit/remaining` | decimal string | Per-window usage | Same mapping as `usage`. |
| `limits[].detail.resetTime` | ISO-8601 string | Per-window reset | `resets_at`. |
| `boosterWallet.balance` | fixed-point ints | Extra Usage wallet; `amount`/`amountLeft` in **fixed-point cents** (`1_000_000` = 1 cent) | Separate `metric_kind=credits` snapshot, `unit=cents`, `currency` from `monthlyChargeLimit`/`monthlyUsed`. Absent wallet → no credits snapshot (not zero). |
| `boosterWallet.monthlyChargeLimit` | `{priceInCents, currency}` | Monthly spending cap; 0 = unlimited | Diagnostics; report only if present. |
| `boosterWallet.monthlyUsed` | `{priceInCents, currency}` | Spend this month under the cap | Diagnostics; report only if present. |
| `boosterWallet.monthlyChargeLimitEnabled` | bool | Whether the cap toggle is on | Diagnostics. |
| `user.membership.level` | string | Plan tier (e.g. `LEVEL_INTERMEDIATE`) | Diagnostics; do not expose as a quota. |

The official parser treats any row without a numeric `used`/`limit` as absent,
and a 401 as "Authorization failed … try /login". The same response may also
carry the shared **monthly** membership pool when the backend reports it;
treat it as another `limits[]`-style row if present, never merge windows.

## Auth and session details

- **Login**: RFC 8628 device-code flow against `https://auth.kimi.com`
  (`POST /api/oauth/device_authorization`, `POST /api/oauth/token`). The CLI
  uses client id `17e5f671-d194-4dfb-9706-5516cb48c098` and `X-Msh-*` device
  headers during the flow. **This project must not initiate new device flows
  under the CLI's client id**; it only reuses a session the user already
  created with `kimi login` / `/login`.
- **Token store** (Windows): `C:\Users\<name>\.kimi-code\credentials\kimi-code.json`
  (data root `~/.kimi-code/`, dir `0700` / file `0600`, relocatable via
  `KIMI_CODE_HOME`). Wire format is snake_case:
  `{access_token, refresh_token, expires_at, scope, token_type, expires_in}`.
- **Refresh**: `POST https://auth.kimi.com/api/oauth/token` with
  `grant_type=refresh_token`, `client_id`, and the stored `refresh_token`.
  Access tokens are short-lived (`expires_in` ≈ 3600). The CLI treats
  401/403/`invalid_grant` as unrecoverable (user must `/login` again) and
  retries 429/5xx with backoff.
- **Usage call headers**: only `Authorization: Bearer` and `Accept`; no
  `X-Msh-*` headers are needed for `/usages`.
- Missing/empty credential file → `freshness=not_configured`, never zero
  usage. `KIMI_CODE_BASE_URL` can override the API base URL; the managed
  `/usages` path stays under it.

## Mapping to the snapshot contract

| Source | Snapshot field | Value |
| --- | --- | --- |
| (fixed) | `provider` | `kimi` |
| redacted credential owner | `account_id` | stable local hash; never email |
| `usage.*` | weekly snapshot | `metric_kind=quota`, `window_kind=weekly`, `window_label=primary`, `unit=percent`, `used`/`limit` from the response, `resets_at=usage.resetTime` |
| `limits[]` | window snapshots | one per row; `window_kind=rolling` (300 min → 5h) or derived from `timeUnit`/`duration`; `window_label` = row `name` when present |
| `boosterWallet` | credits snapshot | `metric_kind=credits`, `unit=cents`, `used=amountLeft`/`total=amount` equivalents with `currency`; absent wallet → no snapshot |
| monthly membership pool | optional snapshot | if the backend reports it, keep it separate (`window_kind=monthly`); never merge with weekly/5h |
| successful fetch | `freshness` | `live` |
| (fixed) | `source` | `cli` when reusing the CLI session; `api` when using a Console API key |

The weekly and 5-hour snapshots carry raw provider counts/percent units as
reported; the shell never derives one window from the other.

## Console and browser bridge

The [Kimi Code Console](https://www.kimi.com/code/console) shows remaining
quota, rate-limit status, API Keys, and devices. It is the manual fallback:
the shell can open it in the user's OS browser (same pattern as the Ollama
settings link). No extension, cookie import, or HTML parser is part of the
product.

The **Kimi Open Platform** (`platform.moonshot.cn`, API-key billing) is a
separate pay-as-you-go product with its own balance endpoint. It must not be
conflated with the Kimi Code membership quota; a future API-key adapter
would be a separate provider id.

## Failure modes (planned tests)

| Condition | Expected snapshot |
| --- | --- |
| No credential file / logged out | `not_configured` |
| Token expired and refresh fails (401/403/`invalid_grant`) | `unavailable` + `auth_expired` |
| Network / proxy timeout | preserve cache as stale; else `unavailable` + `timeout` |
| HTTP 4xx/5xx from `/usages` | redacted error; `schema_drift` only for unparseable success-shaped junk |
| Missing `usage` and empty `limits` | `schema_drift` on the primary window; sibling rows still parse independently |
| Non-numeric `used`/`limit`/`remaining` | row skipped; `schema_drift` if no row remains |
| `used`/`remaining` outside 0–100 percent semantics | reject via contract validation |
| `boosterWallet` absent | no credits snapshot (not zero, not unlimited) |
| `/usages` disappears (404) | last good cache → stale; else `unavailable` + manual Console fallback |

## Security notes

- Reuse only the same host, path, and Bearer auth the official CLI uses
  (`api.kimi.com/coding/v1/usages`).
- Never log `Authorization`, `access_token`, `refresh_token`, cookies, or
  raw authenticated responses.
- Redact email and any user identity in fixtures; `account_id` is a stable
  hash only.
- Read the credential file without modifying it. If the adapter refreshes
  tokens, it writes back through the same OAuth token contract the CLI uses
  (atomic `0600` write), keeping the CLI session healthy.
- Do not scrape kimi.com HTML or copy browser cookies; do not initiate
  device-code sign-in under the CLI's client id from this project. Signed-out
  users are told to run `kimi login` / `/login`.
- If the endpoint or auth path disappears, fall back to
  dashboard-only/unavailable rather than inventing data.

## Open-source evidence pointers

- Usage fetch/parse:
  [`packages/oauth/src/managed-usage.ts`](https://github.com/MoonshotAI/kimi-code/blob/main/packages/oauth/src/managed-usage.ts)
- Device flow + refresh:
  [`packages/oauth/src/oauth.ts`](https://github.com/MoonshotAI/kimi-code/blob/main/packages/oauth/src/oauth.ts)
- Token persistence:
  [`packages/oauth/src/storage.ts`](https://github.com/MoonshotAI/kimi-code/blob/main/packages/oauth/src/storage.ts)
- Client id / OAuth host:
  [`packages/oauth/src/constants.ts`](https://github.com/MoonshotAI/kimi-code/blob/main/packages/oauth/src/constants.ts)
- Token wire format:
  [`packages/oauth/src/types.ts`](https://github.com/MoonshotAI/kimi-code/blob/main/packages/oauth/src/types.ts)
- Data locations (credential path):
  [`docs/en/configuration/data-locations.md`](https://github.com/MoonshotAI/kimi-code/blob/main/docs/en/configuration/data-locations.md)
- Official docs: [Membership Benefits](https://www.kimi.com/code/docs/en/kimi-code/membership.html),
  [Slash Commands](https://www.kimi.com/code/docs/en/kimi-code-cli/reference/slash-commands.html),
  [Error Reference](https://www.kimi.com/code/docs/en/kimi-code/error-reference.html)
- Kimi Code team forum post demonstrating `/usages` with a bearer API key:
  [forum.moonshot.ai #191](https://forum.moonshot.ai/t/error-code-429-were-receiving-too-many-requests-at-the-moment/191)
- Upstream, still open: non-interactive `kimi usage` command request
  [MoonshotAI/kimi-cli#2169](https://github.com/MoonshotAI/kimi-cli/issues/2169)
  and PR [MoonshotAI/kimi-cli#2301](https://github.com/MoonshotAI/kimi-cli/pull/2301)
  — the current kimi-code CLI has no `usage` subcommand, so this adapter
  calls the endpoint directly instead of shelling out.

## Fixtures

Redacted fixtures: `docs/fixtures/kimi/`.

## Follow-up

- Story [#17](https://github.com/ferminquant/ai-usage-bar/issues/17) should
  implement the `kimi` adapter against this surface and fixtures: weekly +
  5-hour quota snapshots, optional credits snapshot, refresh/stale/auth
  handling, and the Console link as the manual fallback.
