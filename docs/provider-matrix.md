# Provider matrix

This matrix records current planning evidence, not a promise that an
undocumented endpoint will remain available. Adapter work must verify the
surface again before implementation.

| Provider | Relevant usage shape | Candidate source | Main risk | First acceptance slice |
| --- | --- | --- | --- | --- |
| Codex | Plan-dependent shared agentic pool with primary/secondary windows and reset times | Local Codex app-server/account usage surface; existing authenticated local session | Schema and plan behavior can change; multiple windows must remain separate | Parse a recorded response into exact snapshots, preserve reset times, and show stale/error states |
| Kimi | Rolling 5-hour and weekly limits, membership/credit cycle, and possible extra usage | Kimi Code CLI /usage, Console, or another explicitly supported surface | No stable public API may exist for every metric; browser automation can be brittle | Start with a CLI/Console evidence spike; no hidden endpoint assumption |
| Ollama local | Local model request/token/timing telemetry; no hosted subscription quota | Ollama local API usage fields | Local usage is not a quota; units differ by model and request | Show local health and token counts as telemetry, never as “quota remaining” |
| Ollama cloud | Cloud compute quota windows and account usage | Supported cloud/API/dashboard surface | Programmatic quota surface is not yet established | Keep local and cloud accounts separate; defer cloud quota until evidence exists |
| Grok consumer | Shared weekly pool across supported products with reset/extra-usage semantics | User-authorized usage UI or supported product surface | Consumer usage may not have a stable public API; authentication/privacy | Document a read-only browser-bridge spike or defer |
| Grok API | API rate limits and spend/usage explorer are separate from consumer subscription usage | xAI API/console for API users | Consumer and API metrics must not be conflated | Treat API as a separate adapter/account type |

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
