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

- Add the remaining providers only after their evidence spikes pass.
- Keep local Ollama telemetry distinct from hosted quotas.
- Add account/provider configuration and per-provider opt-out.

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
