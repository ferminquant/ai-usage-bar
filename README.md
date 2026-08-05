# AI Usage Bar

AI Usage Bar is a desktop usage widget for people who use several online AI
subscriptions at once.

The first target is a small Windows taskbar/tray surface inspired by the
compact usage pill in [this Codex example](https://x.com/i/status/2083054528522268756).
The initial provider set is:

- Codex
- Kimi
- Ollama Pro/cloud (hosted)
- Grok

The MVP core is implemented: Codex, Grok consumer, and opt-in Ollama Pro/cloud
adapters, cached freshness states, contract/invariant tests, a Windows shell,
and hosted-provider configuration. Kimi remains a future evidence-driven
adapter.

Ollama reports hosted session (5-hour) and weekly (7-day) totals from its
authenticated cloud endpoint. The compact view defaults to the session quota;
the right-click menu can select the weekly quota. Ollama's usage response does
not currently include reset timestamps, so the Ollama context menu includes
**Open Ollama usage page** as the low-friction fallback. It opens the normal
OS browser at `https://ollama.com/settings`; no browser extension, cookie
copying, or manual setup is required. Reset metadata is tracked in
[issue #35](https://github.com/ferminquant/ai-usage-bar/issues/35) while
Ollama works on a supported API surface.

## Product direction

The widget should answer one question quickly: “What is the state of each AI
service I can use right now?”

It should:

- show each provider independently;
- show the relevant limit window(s), remaining/used values, and reset time;
- distinguish live, cached, stale, unavailable, and not-applicable data;
- keep credentials on the machine and avoid sending them to a project server;
- support providers whose limits are quotas, credits, or spend rather than
  pretending all usage is one comparable percentage;
- provide a compact glance view plus a detailed provider view.

It should not:

- scrape or bypass provider controls;
- combine unrelated percentages into a false “total quota”;
- invent hosted usage data for an unsupported provider.

## Current status

The current implementation is Windows-first. Provider adapters run locally and
use each service's existing session or credential surface; no provider
credentials are sent to a project server.

## Documentation

- [Product brief](docs/product-brief.md) — problem, users, goals, and
  non-goals.
- [Architecture](docs/architecture.md) — local daemon, adapters, cache, and
  desktop shell boundaries.
- [Snapshot contract](docs/snapshot-contract.md) — the provider-neutral
  usage snapshot model, state/value semantics, fixture requirements, and the
  unit/property/invariant test cases that adapters and the cache must
  satisfy.
- [Provider matrix](docs/provider-matrix.md) — what is known, what is
  uncertain, and the proposed evidence path for each provider.
- [Agile plan](docs/agile-plan.md) — increments, definition of ready/done,
  and the first backlog slices.
- [Configuration example](docs/config.example.json) — hosted-provider
  enablement settings; credentials remain in provider-owned local sessions.
- [Quality engineering](docs/quality-engineering.md) — measurable gates,
  evidence artifacts, and the “constraints around agent-generated code”
  approach.
- [Initial quality baseline](docs/generated/quality-baseline-20260731.md) —
  the honest docs-only starting point for future ratchets.
- [Testing strategy](docs/testing-strategy.md) — unit, contract, invariant,
  integration, UI, packaging, security, and mutation-test plans.
- [Runner strategy](docs/runner-strategy.md) — whether the existing Budget
  self-hosted runners can be reused safely.
- [References](docs/references.md) — design inspiration, prior art, and
  primary provider documentation.

On Windows, copy that example to %APPDATA%\AI Usage Bar\config.json and set
enabled to false for any hosted provider you want to opt out of. A missing
file uses the default Codex and Grok configuration; Ollama Pro/cloud is
registered but opt-in until enabled in that file.

## Proposed first release

The first useful slice is deliberately small:

1. a Windows tray/taskbar shell with a compact pill and a detail panel;
2. a provider-neutral usage snapshot model;
3. one or more reliable hosted-provider adapters;
4. cached data with explicit freshness and error states;
5. fixture-driven tests and a quality gate before more providers are added.

Kimi should be added only after its supported, user-authorized usage surface
has been verified. A browser/dashboard bridge is an option for surfaces
without a documented API, but it remains read-only and opt-in.

## Quality promise

The project follows the principle behind
[Robert Martin’s reference post](https://x.com/i/status/2080257779395154409):
agent-generated implementation is surrounded by strong, executable
constraints rather than trusted because it looks plausible. The planned
constraints include:

- deterministic unit and provider-contract tests;
- Gherkin-style acceptance scenarios for user-visible behavior;
- cross-layer invariants for snapshots, caching, freshness, and privacy;
- coverage with a ratcheting baseline;
- mutation testing for the normalization and policy core;
- lint, type, dependency, secret, packaging, and UI smoke checks;
- generated quality evidence attached to CI runs.

There is no source-code coverage baseline yet because the repository is still
documentation-only. The thresholds and artifacts are defined in
[Quality engineering](docs/quality-engineering.md), and implementation work
is tracked in the [GitHub issue backlog](https://github.com/ferminquant/ai-usage-bar/issues).

## Development principles

- Prefer a small adapter contract over provider-specific logic in the UI.
- Treat provider data as evidence with a source, timestamp, and confidence.
- Never log access tokens, cookies, or raw authenticated responses.
- Make offline and stale states first-class; do not silently display old data
  as current.
- Keep every change small enough to validate in one focused issue or pull
  request.

## License

License choice is deferred until the implementation and distribution model
are decided.
