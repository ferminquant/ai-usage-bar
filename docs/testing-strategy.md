# Testing strategy

The test suite should follow the same risk boundaries as the architecture.
Provider tests must be deterministic and offline by default; live accounts
are opt-in diagnostics, never the only proof of correctness.

## Test layers

### Unit tests

Cover pure normalization, range validation, reset-time parsing, freshness
classification, redaction, provider configuration, and view-model formatting.

### Provider contract tests

Each adapter gets redacted fixtures for:

- a normal response;
- multiple windows;
- no remaining value or an unlimited value;
- a reset time in the past or missing;
- malformed units and out-of-range percentages;
- rate-limit, auth, timeout, and schema-change failures.

### Invariant tests

The highest-risk cross-layer truths should be expressed as invariants:

- every normalized percentage is bounded or explicitly unknown;
- a provider failure preserves the last good snapshot and marks it stale or
  unavailable;
- an older concurrent response cannot overwrite a newer observation;
- cache keys cannot mix provider, account, metric, or window;
- local Ollama telemetry is never classified as hosted quota;
- the compact view never aggregates incompatible percentages;
- reset timestamps and observed timestamps survive cache round trips;
- redaction removes tokens, cookies, authorization headers, and sensitive
  query values from logs and diagnostics;
- disabling a provider removes it from refresh scheduling and the UI;
- a missing provider source produces not-configured/unavailable, not zero.

### Acceptance tests

Use Gherkin-style scenarios for user behavior, for example:

~~~gherkin
Scenario: A stale provider value is visible as stale
  Given the last successful snapshot is older than the freshness policy
  When the desktop bar refreshes
  Then the provider card shows the last value with a stale label
  And the reset time remains the provider-reported reset time
  And the card explains when the value was observed
~~~

~~~gherkin
Scenario: Local Ollama telemetry is not a subscription quota
  Given Ollama is running locally and reports token counts
  When the usage bar renders the Ollama card
  Then it shows local runtime telemetry
  And it does not show a hosted quota percentage or fake reset time
~~~

### Integration tests

Use local fake servers, CLI subprocess doubles, and deterministic clocks.
Live provider checks may be a separately marked diagnostic suite and must
never require a personal account for ordinary pull requests.

### UI and packaging tests

The release path needs clean-machine smoke tests for install, startup,
refresh, detail view, offline behavior, upgrade, and uninstall. UI tests
should assert accessible names and state labels, not screenshot pixels alone.

### Security tests

Add secret-pattern fixtures, log-capture tests, permission checks for browser
bridges, dependency audits, and a check that test artifacts contain no raw
provider payloads.

### Mutation and property-based tests

Mutation testing should initially target normalization, freshness, cache
ordering, and aggregation policy. Property-based tests should generate
windows, units, timestamps, missing fields, and provider error combinations.

## Planned markers and commands

Rust integration-test binaries now provide the first executable marker
boundary. Test names use the same prefixes so local filtering remains clear,
and the files can be run independently:

- `tests/contract.rs` — `contract_*` tests for serialization and value/state
  semantics;
- `tests/invariants.rs` — `invariant_*` tests for cross-layer policy,
  redaction, cache identity, and property-based generators;
- `tests/acceptance.rs` — `scenario_*` tests written as Given/When/Then
  user behavior against fake adapters (stale-after-failure, first-failure
  unavailable, disabled provider, local Ollama is not hosted quota).

The remaining markers stay reserved for the corresponding future suites:

- unit
- contract
- invariant
- integration
- ui
- packaging
- security
- diagnostic

The deterministic CI job runs the three implemented integration binaries on
every pull request:

~~~text
cargo test --test contract --all-features
cargo test --test invariants --all-features
cargo test --test acceptance --all-features
~~~

The broader Linux and Windows jobs continue to run all targets. Integration,
UI, packaging, security, diagnostic, and mutation jobs remain split by risk
and runtime as those surfaces are implemented.
