# Codex usage spike (#2)

Status: complete. Evidence captured from the Codex CLI app-server protocol.

## Source

The Codex CLI ships an **app-server** that speaks JSON-RPC over stdio. It
reuses the user's existing local ChatGPT/Codex login session from
`~/.codex/auth.json`. No API key is required and no credentials are sent in
the request body.

- CLI path (this machine): `%LOCALAPPDATA%\OpenAI\Codex\bin\codex.exe`
- Command: `codex app-server --listen stdio://`
- Protocol: JSON-RPC 2.0 over stdio (newline-delimited)
- Auth: existing local session (`~/.codex/auth.json`); the server handles
  token refresh internally.
- Schema generation: `codex app-server generate-json-schema --out <dir>`
  produces the full v1/v2 protocol schema without touching credentials.

## Verified methods

| Method | Params | Returns | Notes |
| --- | --- | --- | --- |
| `initialize` | `{clientInfo:{name,version}}` | `{codexHome,platformFamily,platformOs,userAgent}` | Required handshake before any other call. |
| `account/read` | `{refreshToken?:bool}` | `{account?,requiresOpenaiAuth}` | Returns account type, email, planType. `requiresOpenaiAuth=true` means a live session exists. |
| `account/rateLimits/read` | `{}` | `{rateLimits, rateLimitsByLimitId?}` | The primary usage surface. |

## Push notifications

| Method | Payload | Notes |
| --- | --- | --- |
| `account/rateLimits/updated` | `{rateLimits: RateLimitSnapshot}` | Pushed when limits change during a session. |
| `account/updated` | account snapshot | Pushed when account info changes. |
| `thread/tokenUsage/updated` | per-thread token breakdown | Per-turn token counts; not a quota. |

## RateLimitSnapshot shape (verified)

~~~json
{
  "limitId": "codex",
  "limitName": null,
  "primary": {
    "usedPercent": 40,
    "windowDurationMins": 10080,
    "resetsAt": 1786036566
  },
  "secondary": null,
  "credits": {
    "hasCredits": false,
    "unlimited": false,
    "balance": "0"
  },
  "planType": "plus",
  "rateLimitReachedType": null
}
~~~

### Field semantics

| Field | Type | Meaning | Notes |
| --- | --- | --- | --- |
| `limitId` | string\|null | Metered limit identifier (e.g. `codex`) | Used as the key in `rateLimitsByLimitId`. |
| `limitName` | string\|null | Human-readable limit name | Often null. |
| `primary.usedPercent` | int32 | Percent of the primary window used | **Required.** 0-100. This is a percentage, not a raw count. |
| `primary.windowDurationMins` | int64\|null | Window duration in minutes | 10080 = 7 days (weekly). |
| `primary.resetsAt` | int64\|null | Unix epoch seconds when the window resets | Can be null if not reported. |
| `secondary` | RateLimitWindow\|null | A secondary window | Same shape as `primary`. Observed null in this capture. |
| `credits.hasCredits` | bool | Whether the account has credits | Required. |
| `credits.unlimited` | bool | Whether credits are unlimited | Required. Distinct from a missing balance. |
| `credits.balance` | string\|null | Credit balance as a string | Null means not reported; "0" is a real zero. |
| `planType` | enum | ChatGPT plan | free, go, plus, pro, prolite, team, business, enterprise, edu, unknown. |
| `rateLimitReachedType` | enum\|null | Why the limit was reached, if applicable | null when not reached. |

### Key observations

1. **Usage is a percentage, not a raw count.** The protocol exposes
   `usedPercent` (0-100), not `used`/`limit` counts. This means our snapshot
   contract must represent Codex as a percentage-based quota where `used`
   and `limit` are absent and the shell computes the percentage from
   `usedPercent` directly, OR we store `usedPercent` as `used` with
   `limit=100` and `unit="percent"`. The latter fits the contract better
   and lets the shell render it without special-casing.

2. **Primary and secondary windows.** The schema supports two windows
   (`primary` and `secondary`). Each becomes a separate snapshot with a
   distinct `window_label`. In this capture, `secondary` was null.

3. **Window duration is in minutes.** 10080 = weekly. We derive
   `window_kind` from this: 1440=daily, 10080=weekly, 43200=monthly,
   other=rolling.

4. **`resetsAt` is Unix epoch seconds**, not ISO 8601. The adapter must
   convert to ISO 8601 for the snapshot contract.

5. **Credits are separate from quota.** `credits` is a distinct metric
   (`metric_kind=credits`), not a quota window. `unlimited=true` maps
   directly to the contract's `unlimited` field.

6. **`rateLimitsByLimitId`** is the multi-bucket view. Today it has one
   key (`codex`), but the schema supports multiple metered limits. The
   adapter should iterate this map when present, falling back to the flat
   `rateLimits` field.

7. **Account email is in `account/read`.** It must be redacted in
   fixtures. The `account_id` in our snapshot should be a stable hash or
   the plan type, never the raw email.

## Mapping to the snapshot contract

| Codex field | Snapshot field | Value |
| --- | --- | --- |
| `limitId` | `provider` | `codex` |
| account email (redacted) | `account_id` | derived stable id, not raw email |
| `primary.usedPercent` | `used` | the percent value; `limit=100`; `unit="percent"` |
| `primary.windowDurationMins` | `window_kind` | derived: 10080→weekly, 1440→daily, etc. |
| `primary.resetsAt` | `resets_at` | converted from epoch seconds to ISO 8601 |
| `primary` | `window_label` | `"primary"` |
| `secondary` | `window_label` | `"secondary"` (separate snapshot) |
| `credits.hasCredits` + `credits.balance` | separate snapshot | `metric_kind=credits` |
| `credits.unlimited` | `unlimited` | on the credits snapshot |
| `planType` | not in snapshot | account metadata; could be in diagnostics |
| `rateLimitReachedType` | `error.code` | when not null, maps to a redacted error code |

## Recommendation

**Implement.** The app-server protocol is stable, scriptable, uses the
existing local session, and exposes exactly the usage surfaces the product
needs: primary/secondary windows, reset times, credits, and plan type. No
browser bridge is required for Codex.

## Security notes

- The adapter connects to a **fresh** app-server process over stdio. It does
  not attach to the running desktop app's app-server.
- No tokens, cookies, or auth headers appear in the request body or the
  response. The server handles auth internally using `~/.codex/auth.json`.
- The account email in `account/read` must be redacted before any fixture
  or diagnostic is persisted.
- `requiresOpenaiAuth=true` in the account response means a live session
  exists. If false, the adapter returns `freshness=not_configured`.
- The config warning observed (`unknown variant max`) is a local config
  issue and does not affect usage data. The adapter should ignore
  `configWarning` notifications for usage purposes.

## Fixtures

Redacted fixtures are in `docs/fixtures/codex/`:

- `normal.json` - successful response with primary window
- `multiple_windows.json` - primary + secondary windows
- `unlimited_or_missing.json` - unlimited credits, null secondary
- `auth_failure.json` - `requiresOpenaiAuth=false`
- `timeout.json` - no response within the deadline
- `malformed.json` - schema-drift response

All fixtures are redacted: email replaced with `redacted@example.com`,
account_id is a stable placeholder.