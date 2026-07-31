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
Codex + Kimi + Ollama + Grok, Windows-first scope:

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
- [Ollama pricing and cloud limits](https://ollama.com/pricing)
- [Ollama usage API](https://docs.ollama.com/api/usage)
- [Ollama cloud](https://docs.ollama.com/cloud)
- [Grok FAQ](https://docs.x.ai/grok/faq)
- [Grok overview](https://docs.x.ai/grok/overview)
- [xAI API rate limits](https://docs.x.ai/developers/rate-limits)
- [xAI console usage](https://docs.x.ai/console/usage)

Provider interfaces and plan semantics can change. The links are evidence
starting points, not a guarantee of a stable integration contract.
