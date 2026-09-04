# z.ai usage spike

## Scope

This spike covers the international z.ai GLM Coding Plan (`zai`), not the
domestic BigModel/Zhipu service. The adapter reports account quota only; it
does not make model requests or scrape the subscription dashboard.

## Evidence

The official z.ai coding-plan tooling publishes a usage-query plugin at
[`zai-org/zai-coding-plugins`](https://github.com/zai-org/zai-coding-plugins).
Its `usage-query` script calls:

```text
GET https://api.z.ai/api/monitor/usage/quota/limit
Authorization: <API key>
Accept: application/json
```

The public [GLM Coding Plan FAQ](https://docs.z.ai/devpack/faq) documents a
rolling five-hour quota and a seven-day weekly quota, and says that all
supported coding tools share those limits. The monitor response used by the
official plugin contains `data.limits[]` rows with `percentage`,
`currentValue`, `usage`, `remaining`, and epoch-millisecond `nextResetTime`.

## Response variants

Older plans return `TOKENS_LIMIT` rows. Newer credit-based plans return
`CREDIT_LIMIT` rows with the same `unit=3, number=5` (five-hour) and
`unit=6, number=1|7` (weekly) window markers. The adapter normalizes either
type to `metric_kind=quota`, `unit=percent`, and stable `5-hour`/`weekly`
labels so the compact pill and window selector can use them.

Some responses include a `TIME_LIMIT` row for the monthly MCP/tool allowance.
That row is kept separately as `metric_kind=requests`, `unit=requests`, and
is never combined with model quota. Unknown future limit types are ignored;
if no supported rows remain, the refresh is reported as schema drift.

## Credential boundary

The adapter reads `ZAI_API_KEY`, falling back to `GLM_API_KEY`. For a
Claude-compatible z.ai setup it accepts `ANTHROPIC_AUTH_TOKEN` only when
`ANTHROPIC_BASE_URL` contains `api.z.ai`. Keys are hashed into a stable local
account identifier and are never placed in snapshots, fixtures, or error
messages. The z.ai API key is not persisted by this application.

## Limitations and fallback

The monitor endpoint is used by z.ai's official tooling but is not listed in
the public OpenAPI catalog. It can change or disappear, and the adapter does
not infer missing reset times or quota values. A failed call becomes an
unavailable/stale refresh through the shared cache; the z.ai console remains
the manual fallback for subscription details.
