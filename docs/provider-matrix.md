# Provider matrix

This matrix records current planning evidence, not a promise that an
undocumented endpoint will remain available. Adapter work must verify the
surface again before implementation.

| Provider | Relevant usage shape | Candidate source | Main risk | First acceptance slice |
| --- | --- | --- | --- | --- |
| Codex | Plan-dependent shared agentic pool with primary/secondary windows and reset times | Codex CLI app-server (`codex app-server --listen stdio://`), JSON-RPC methods `account/rateLimits/read` and `account/read`; reuses `~/.codex/auth.json` local session | Schema and plan behavior can change; multiple windows must remain separate; `usedPercent` is a percentage not a raw count; `resetsAt` is epoch seconds not ISO 8601 | Parse a recorded response into exact snapshots, preserve reset times, and show stale/error states. **Verified** — see [codex-spike.md](spikes/codex-spike.md) |
| Kimi | Rolling 5-hour and weekly limits, membership/credit cycle, and possible extra usage | Kimi Code CLI /usage, Console, or another explicitly supported surface | No stable public API may exist for every metric; browser automation can be brittle | Start with a CLI/Console evidence spike; no hidden endpoint assumption |
| Ollama Pro/cloud | Hosted compute quota windows and account usage | Supported cloud/API/dashboard surface | Programmatic quota surface is not yet established | Defer until a supported hosted source is verified |
| Grok consumer | Shared weekly SuperGrok pool (`creditUsagePercent` 0–100), weekly period start/end, optional product breakdown and prepaid credits | Grok Build CLI local OIDC session (`~/.grok/auth.json`) + `GET https://cli-chat-proxy.grok.com/v1/billing?format=credits` (same path as official `/usage` / `x.ai/billing`) | Short-lived access tokens need refresh; schema has legacy monthly-cent fields and newer percent fields; product rows must not become a second fake quota | **Verified implement** — see [grok-spike.md](spikes/grok-spike.md); redacted fixtures in `docs/fixtures/grok_consumer/` |
| Grok API | Per-model RPS/TPM rate limits by spend tier; console usage explorer for API spend/tokens | xAI API key + Console rate-limits / usage explorer | Must not be conflated with SuperGrok weekly pool | **Defer** as separate `grok_api` adapter; not the consumer remaining-usage bar |

## Adapter admission rules

An adapter may enter implementation only when its issue includes:

1. a source URL or local command that the user is authorized to use;
2. a captured, redacted fixture or deterministic mock;
3. field semantics and unit definitions;
4. refresh, timeout, and stale-cache behavior;
5. a security review of tokens, cookies, and logs;
6. an explicit fallback when the surface disappears.

If those conditions cannot be met, the provider remains “planned” rather than
being represented by guessed data.
