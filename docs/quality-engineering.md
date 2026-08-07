# Quality engineering contract

This document records the quality contract and its executable gates. The
coverage and mutation ratchet is intentionally bounded: the alpha gate
measures the provider-neutral policy core, while provider I/O and native
Windows entrypoints remain explicit, dated exceptions until their dedicated
fixture and smoke suites are complete.

The guiding reference is
[the requested Uncle Bob post](https://x.com/i/status/2080257779395154409),
which argues for surrounding agent-generated code with executable constraints:
unit tests, acceptance tests, QA procedures, quality metrics, mutation
testing, and coverage. The project adopts the engineering principle without
treating any single metric as proof of correctness.

## Metrics and proposed gates

| Area | Alpha gate | Pre-1.0 target | Evidence |
| --- | ---: | ---: | --- |
| Test pass rate | 100% selected CI tests | 100% | CI job summary |
| Line coverage, enforced policy scope | >= 90% | >= 95% | coverage.json plus ratchet config |
| Policy-core file coverage | >= 90% | >= 95% | Per-file thresholds |
| Provider adapter/entrypoint coverage | Reported with dated exceptions | >= 95% | quality-summary.json and issue-linked exclusions |
| Mutation score for policy/normalization core | >= 70% | >= 80% | Mutation report |
| Contract fixture pass rate | 100% | 100% | Versioned redacted fixtures |
| Invariant pass rate | 100% | 100% | Dedicated invariant job |
| Secret scan findings | 0 high-confidence secrets | 0 | Secret scanner report |
| Type/lint findings | 0 blocking findings | 0 | CI output |
| Dependency vulnerabilities | 0 critical/high exploitable findings | 0 critical/high | Dependency audit |
| Unclassified flaky tests | 0 | 0 | Flake ledger/issue |
| UI smoke failures | 0 | 0 | Clean-install smoke artifact |
| Quality job runtime | <= 20 minutes on the selected runner | <= 15 minutes where practical | CI timing artifact |

Thresholds are starting points. A threshold can be changed only with a dated
baseline, an issue explaining the risk, and a ratchet plan. Every excluded
file must carry an issue number, a reason, and a review date in
[`quality/thresholds.json`](../quality/thresholds.json); an expired exception
fails the quality job. A high percentage does not excuse missing behavior or
weak provider semantics.

## Executable alpha gate

The `quality` CI job installs `cargo-llvm-cov` and `cargo-mutants`, then emits
`quality-output/quality-summary.json` and the raw reports as a short-lived
artifact. Coverage is calculated for `src/model.rs`, `src/daemon.rs`,
`src/security.rs`, `src/shell_logic.rs`, and `src/viewmodel.rs`. This scope is
the shared policy and presentation boundary; adapter and native-entrypoint
files are still reported and listed as exceptions.

Mutation testing is deliberately bounded to the snapshot contract, cache
ordering, freshness classification, sanitization, invalid-window handling,
and freshness aggregation functions in `src/model.rs` and `src/daemon.rs`.
The mutation command runs the full Rust test set, including the invariant
tests, so the report shows which policy mutants those tests kill. The alpha
gate is 70% of viable mutants (unviable mutants are reported separately); the
pre-1.0 target is 80%.

## Required evidence artifacts

The CI workflow publishes or retains:

- a machine-readable quality-summary.json;
- coverage and per-file threshold data;
- mutation-test summary for the policy core;
- test counts by marker;
- dependency and secret-scan summaries (the `security` CI job);
- UI/package smoke results;
- job duration and cache/freshness fixture evidence;
- a dated Markdown baseline under docs/generated/.

Artifacts must be redacted and must never contain access tokens, cookies, or
raw authenticated provider responses.

## Quality gates by change type

| Change | Minimum validation |
| --- | --- |
| Snapshot model/policy | Unit, property-based, invariant, mutation |
| Provider adapter | Contract fixtures, parser edge cases, redaction test, adapter integration |
| Cache/scheduler | Concurrency and stale-data invariants, deterministic clock tests |
| Windows shell | View-model unit tests, accessibility checks, UI smoke |
| Browser bridge | Permission/auth tests, fixture replay, no-secret-log test |
| Packaging/startup | Clean-machine install, upgrade, uninstall, startup smoke |
| CI/workflow | Actionlint, workflow-structure test, dry-run or smoke job |

## Baseline policy

The first implementation issue must record:

- commit and environment;
- test collection counts;
- coverage by package/file;
- mutation scope and score;
- skipped tests and why;
- observed CI runtime;
- known gaps and their issue links.

Future baselines compare against that record. Missing evidence is reported as
unknown; it is not silently treated as passing. The current executable
baseline is recorded in
[docs/generated/quality-baseline-20260807.md](generated/quality-baseline-20260807.md).

The starting point is recorded in
[docs/generated/quality-baseline-20260731.md](generated/quality-baseline-20260731.md).
