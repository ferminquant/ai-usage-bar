# OpenCode Zen credits/balance decision record (#86)

Status: **decision recorded; deferred.** Evidence date: **2026-08-14**.

This record is the admission gate for any future OpenCode Zen balance work. It
is the concise decision record for #86; speculative endpoint contracts,
detailed unauthenticated probe transcripts, and synthetic fixture planning
are not maintained while no supported source exists.

## Conclusion

As of 2026-08-14 there is **no released or documented individual-account
balance API or CLI command** for OpenCode Zen:

1. The official [Zen documentation](https://opencode.ai/docs/zen/) documents
   inference endpoints only (`/zen/v1/responses`, `/messages`,
   `/chat/completions`, `/models`); no balance, credits, or usage endpoint.
2. The proposed upstream request
   [anomalyco/opencode#10448](https://github.com/anomalyco/opencode/issues/10448)
   (`GET /zen/v1/balance`) is still open; a public probe of the production
   host returned HTTP 404 on the evidence date.
3. The CLI has no Zen `usage`/`balance` command. `opencode stats` reports
   local-device session cost/tokens, which is explicitly **not** an account
   balance and must never be promoted to one or inferred from.
4. `GET /zen/go/v1/usage` is a **separate surface**: OpenCode Go plan quota
   (upstream PR #16513), not Zen credits. Even a successful response reports
   Go rolling/weekly/monthly windows, never the Zen pay-as-you-go balance.
5. The dashboard (workspace billing/usage pages) is the only place a balance
   is visible today. It is a human-visual **manual fallback, not an API**: do
   not scrape its HTML, copy its cookies, or treat it as a programmatic
   source.

This states what the *documented and released* surface shows as of the
evidence date. It is **not** a claim that a hidden or internal endpoint can
never exist; if one appears, it must still pass the admission gate below.

## Source matrix (checked 2026-08-14)

| Source | What it provides | Decision |
| --- | --- | --- |
| [Zen docs](https://opencode.ai/docs/zen/) | Product semantics: pay-as-you-go, auto-reload ($5 / $20), workspace monthly limits | Product evidence only; no balance response documented |
| [Provider docs](https://opencode.ai/docs/providers) | `/connect` flow and local key storage | Auth/config evidence only |
| Upstream [#10448](https://github.com/anomalyco/opencode/issues/10448) | Proposed `GET /zen/v1/balance` | Open; probe returned HTTP 404 on 2026-08-14; track, do not implement |
| `GET /zen/go/v1/usage` | **Go plan** quota surface, not Zen credits | Not Zen balance; keep in the Go domain |
| `opencode stats` / local ledger | Local-device spend/tokens | Reject as a balance source |
| [Console usage export](https://console.opencode.ai/guides/usage) | Service-account-only historical CSV | Reject as Zen balance |
| Dashboard billing/usage pages | Manual visual balance and billing settings | Manual fallback only; never scrape or import cookies |

## Non-goals (explicit)

- No production adapter for Zen balance/credits is admitted now.
- No scraping of dashboard HTML and no browser-cookie import, ever.
- No inference of an account balance from local `opencode stats` or any local
  ledger; local spend is not a balance.
- No implementation against #10448's proposed shape until it is released,
  documented, and re-probed.
- No speculative endpoint contracts, probe transcripts, or fixture planning
  are maintained in this repository while no supported source exists.
- `/zen/go/v1/usage` is Go quota evidence and stays out of the Zen balance
  domain.

## Decision

**Defer / dashboard-only.** The follow-up story (#87) stays explicitly
deferred until a **supported, documented individual-account Zen balance
source** exists — a released API or CLI command. Until then the dashboard is
the documented manual fallback, and no Zen provider is registered from
guessed or scraped data.

## Wait condition

Re-open #87 only when one of the following is true, and re-verify each on the
probe date before any implementation:

1. The official Zen docs document a balance/credits endpoint or CLI command.
2. Upstream #10448 is closed/implemented, or a successor PR is merged, the
   route responds to a live probe, and the response shape is documented.
3. OpenCode ships a supported, documented individual-account balance surface
   elsewhere.

Any future adapter must preserve provider-reported balance/zero/missing
distinctions, keep Zen `credits` identity separate from OpenCode Go quota and
Console exports, and never scrape the dashboard or import cookies.
