# Agile delivery plan

The project uses small, evidence-driven slices. A story should produce one
observable capability or one durable engineering contract.

## Working cadence

- One-week discovery/build slices until the first vertical demo.
- One issue per slice; keep implementation pull requests narrow.
- Review the provider evidence and quality artifacts at the end of each
  slice.
- Do not create speculative adapter work without a verified source.

## Proposed increments

### Increment 0 — foundation

- Freeze the product vocabulary and snapshot contract.
- Create the local daemon/shell boundary.
- Add CI, lint, type checks, and fixture-test scaffolding.
- Decide whether the first shell is a taskbar pill, tray icon, or both.

### Increment 1 — first vertical slice

- Implement one provider adapter with recorded fixtures.
- Implement the cache/freshness policy.
- Render a compact status and a detailed card.
- Demonstrate live, cached, stale, unavailable, and not-configured states.

### Increment 2 — provider expansion

- Add hosted providers only after their evidence spikes pass.
- Add account/provider configuration and per-provider opt-out for hosted
  providers.

### Issue #10 — hosted provider configuration and registry bootstrap

Issue #10 is intentionally narrower than a general plugin system. The goal is
to make the verified, compiled hosted adapters configurable without putting
provider registration in the Windows shell or CLI entrypoints.

#### In scope

- A versioned, per-user JSON configuration file containing provider enablement.
- One shared bootstrap path for the shell and CLI.
- Codex and Grok consumer enabled by default; missing local sessions remain
  `not_configured`, while disabling either provider persists across restarts.
- Disabled providers are not scheduled, rendered, or surfaced from the cache.
- `not_configured` remains distinct from an explicit user opt-out.
- Deterministic config, registry, cache-filtering, and redaction tests.

#### Explicitly out of scope

- Kimi implementation; keep the evidence spike in #3 and the adapter in #17.
- Ollama Pro/cloud adapter behavior is tracked separately in #9; this registry
  slice only owns its compiled registration and opt-in enablement.
- Runtime DLL/plugin loading. Adding a new adapter still changes the compiled
  provider factory, but it must not require edits to the shell UI.
- Credentials, cookies, or access tokens in the configuration file.

#### Acceptance boundary

The slice is complete when a fresh install can run with the defaults, a user
can disable Codex or Grok by editing the config file and restart without that
provider being called or shown, and both entrypoints produce the same provider
set from the same configuration. Unknown or unsupported providers are ignored
  without being treated as usage data.

### Increment 3 — hardening

- Add invariant, property-based, mutation, security, packaging, and UI tests.
- Add quality evidence artifacts and a coverage ratchet.
- Exercise the selected self-hosted or GitHub-hosted runner path.

### Increment 4 — release candidate

- Package and install on a clean Windows machine.
- Validate uninstall, upgrades, startup behavior, and offline recovery.
- Document known provider limitations and support boundaries.

## Definition of Ready

A story is ready when:

- the user-visible outcome is stated;
- the provider/source assumptions are recorded;
- acceptance examples exist;
- secrets and privacy impact are known;
- dependencies and out-of-scope work are listed;
- the planned test level is named.

## Definition of Done

A story is done when:

- implementation and tests are merged;
- the relevant quality gates pass;
- documentation and known limitations are updated;
- generated or recorded fixtures are redacted and reproducible;
- no credentials or raw authenticated payloads enter the repository;
- the issue links the validation evidence and is closed only after review.

## Backlog shape

The initial GitHub issues are intentionally grouped into:

- product and data contracts;
- desktop shell and daemon;
- provider evidence and adapters;
- quality/test infrastructure;
- security, packaging, and release operations.

Dependencies are written in issue bodies so the backlog can be worked in
vertical slices instead of completing an entire layer before demonstrating
value.

## Initial GitHub backlog

The first backlog is live in the private repository:

| Slice | Issues |
| --- | --- |
| Contract and evidence | [#1](https://github.com/ferminquant/ai-usage-bar/issues/1), [#2](https://github.com/ferminquant/ai-usage-bar/issues/2), [#3](https://github.com/ferminquant/ai-usage-bar/issues/3), [#4](https://github.com/ferminquant/ai-usage-bar/issues/4), [#5](https://github.com/ferminquant/ai-usage-bar/issues/5) |
| Vertical slice | [#6](https://github.com/ferminquant/ai-usage-bar/issues/6), [#7](https://github.com/ferminquant/ai-usage-bar/issues/7), [#8](https://github.com/ferminquant/ai-usage-bar/issues/8) |
| Provider expansion | [#9](https://github.com/ferminquant/ai-usage-bar/issues/9), [#10](https://github.com/ferminquant/ai-usage-bar/issues/10), [#17](https://github.com/ferminquant/ai-usage-bar/issues/17), [#18](https://github.com/ferminquant/ai-usage-bar/issues/18) |
| Quality and safety | [#11](https://github.com/ferminquant/ai-usage-bar/issues/11), [#12](https://github.com/ferminquant/ai-usage-bar/issues/12), [#13](https://github.com/ferminquant/ai-usage-bar/issues/13), [#14](https://github.com/ferminquant/ai-usage-bar/issues/14), [#15](https://github.com/ferminquant/ai-usage-bar/issues/15) |
| Release | [#16](https://github.com/ferminquant/ai-usage-bar/issues/16) |

The first recommended slice is #1, then the four provider evidence spikes
can run in parallel. Implementation should not begin for a provider until
its spike records a supported source and redacted fixture.
