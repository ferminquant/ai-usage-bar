# Quality engineering contract

This repository is documentation-only at creation time. No implementation
coverage or test pass rate is claimed yet. The following is the proposed
quality contract to be implemented through the GitHub backlog.

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
| Line coverage, overall | >= 90% | >= 95% | coverage.json plus ratchet config |
| Adapter normalization/core coverage | >= 95% | >= 95% | Per-file thresholds |
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
baseline, an issue explaining the risk, and a ratchet plan. A high percentage
does not excuse missing behavior or weak provider semantics.

## Required evidence artifacts

The CI workflow should eventually publish or retain:

- a machine-readable quality-summary.json;
- coverage and per-file threshold data;
- mutation-test summary for the policy core;
- test counts by marker;
- dependency and secret-scan summaries;
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
unknown; it is not silently treated as passing.
