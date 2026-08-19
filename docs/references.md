# References and prior art

## Design inspiration

- [Codex usage widget example](https://x.com/i/status/2083054528522268756) —
  compact glanceable usage pill on a desktop taskbar.
- [Quality-constraints reference post](https://x.com/i/status/2080257779395154409)
  — the requested reference for surrounding agent-generated work with tests,
  acceptance criteria, QA procedures, metrics, mutation testing, and
  coverage.

## Related projects

These projects are useful prior art, but none currently covers the complete
Codex + Kimi + hosted Ollama + Grok, Windows-first scope:

- [hohieuu/ai-usage-bar](https://github.com/hohieuu/ai-usage-bar) —
  macOS SwiftBar integration for Claude Code and Cursor.
- [crearo/ai-usage-bar](https://github.com/crearo/ai-usage-bar) —
  macOS SwiftBar integration for Claude and Codex through ccusage.
- [jhartzell/ai-usage-bar](https://github.com/jhartzell/ai-usage-bar) —
  Linux Waybar widget for Claude, Codex, and OpenRouter with caching and a
  detail popup.

## Provider documentation to re-check before adapter work

- [OpenAI Codex usage with ChatGPT plans](https://help.openai.com/en/articles/11369540-using-codex-with-your-chatgpt-plan)
- [Kimi Code membership and usage](https://www.kimi.com/code/docs/en/kimi-code/membership.html)
- [Kimi Code slash commands](https://www.kimi.com/code/docs/en/kimi-code-cli/reference/slash-commands.html)
- [Kimi Code error reference](https://www.kimi.com/code/docs/en/kimi-code/error-reference.html)
- [MoonshotAI/kimi-code](https://github.com/MoonshotAI/kimi-code) (official
  open-source CLI; managed usage endpoint in `packages/oauth/src/managed-usage.ts`)
- [Kimi usage spike](spikes/kimi-spike.md) — verified `/coding/v1/usages`
  surface, OAuth session reuse, and the implement decision.
- [Ollama pricing and cloud limits](https://ollama.com/pricing)
- [Ollama cloud](https://docs.ollama.com/cloud) (future hosted-provider
  evidence only)
- [Ollama usage spike](spikes/ollama-spike.md) — session/weekly semantics,
  authenticated `/api/usage` evidence, and the deferred reset-metadata plan.
- [OpenCode Go documentation](https://opencode.ai/docs/go/) — plan windows,
  dollar-weighted semantics, Go gateway endpoints, and the console link.
- [OpenCode provider documentation](https://opencode.ai/docs/providers) —
  `/connect` flow and local `auth.json` credential boundary.
- [OpenCode CLI documentation](https://dev.opencode.ai/docs/cli/) — provider
  auth commands and local session `stats` (not subscription quota).
- [OpenCode Console usage guide](https://console.opencode.ai/guides/usage) —
  service-account-only historical CSV export; not Go remaining allowance.
- [OpenCode Go usage spike](spikes/opencode-go-spike.md) — issue #33 evidence,
  upstream API tracking, admission gate, and redacted deterministic fixtures.
- [OpenCode Zen documentation](https://opencode.ai/docs/zen/) — pay-as-you-go
  gateway, auto-reload ($5 / $20), workspace monthly limits; documents
  inference endpoints only, no balance API.
- [Upstream Zen balance request #10448](https://github.com/anomalyco/opencode/issues/10448)
  — open; proposes `GET /zen/v1/balance`; the live probe returned HTTP 404 on
  2026-08-14 (see [Zen tracking record](spikes/opencode-zen-spike.md) — #86
  stays open with a periodic recheck plan while upstream is pending; the
  dashboard is the manual fallback).
- [Grok FAQ](https://docs.x.ai/grok/faq) (weekly SuperGrok pool semantics)
- [Grok overview](https://docs.x.ai/grok/overview)
- [Grok Build CLI](https://x.ai/news/grok-build-cli) / [xai-org/grok-build](https://github.com/xai-org/grok-build) (open-source client; billing path)
- [xAI API rate limits](https://docs.x.ai/developers/rate-limits)
- [xAI console usage](https://docs.x.ai/console/usage)
- Spike evidence: [grok-spike.md](spikes/grok-spike.md)

Provider interfaces and plan semantics can change. The links are evidence
starting points, not a guarantee of a stable integration contract.
