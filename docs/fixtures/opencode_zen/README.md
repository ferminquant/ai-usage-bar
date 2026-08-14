# OpenCode Zen balance spike fixtures (#86)

These are deterministic, synthetic fixtures for issue #86. They are
**hypothetical** shapes for a Zen balance source that is **not released**:
the upstream request
[opencode/opencode#10448](https://github.com/anomalyco/opencode/issues/10448)
proposes `GET /zen/v1/balance`, and a live unauthenticated probe on
2026-08-14 returned HTTP 404 for that route on the production host.

Every file carries `_status: "unreleased"` and `_decision: "defer"` metadata,
and the `_source` field names the proposed or observed surface. The error
fixtures model the gateway's observed error shape
(`{"type":"error","error":{"type":"AuthError","message":"..."}}`, seen at
`/zen/go/v1/usage` on 2026-08-14) and the failure states an eventual adapter
must handle if a supported source is ever released.

No file contains an API key, cookie, email, account id, or raw browser
response. These fixtures are test-planning artifacts; they are not a reason
to ship a production Zen balance adapter (the spike decision is **defer**).
