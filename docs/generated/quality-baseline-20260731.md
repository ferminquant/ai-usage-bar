# AI Usage Bar quality baseline

- Date: 2026-07-31
- Repository state: planning/documentation seed
- Commit: d2b0d52
- Source implementation: not started

## Current evidence

| Surface | Result | Notes |
| --- | --- | --- |
| Documentation files | 9 | README plus architecture, product, provider, agile, quality, test, runner, reference, and baseline docs |
| Source files | 0 | No provider adapter, daemon, shell, or package exists yet |
| Automated tests | 0 | Test harness is planned in issues #11 and #12 |
| Line coverage | N/A | No executable source exists |
| Mutation score | N/A | Planned in issue #13 |
| Contract fixtures | 0 | Provider evidence and fixture work is planned in issues #2 through #5 |
| CI workflow | Not present | Planned in issues #11 and #15 |
| Secret scan | Not run | No source or fixture payloads exist; planned in issue #14 |
| Dependency audit | Not run | No runtime dependency manifest exists yet |
| UI/package smoke | Not run | Planned in issue #16 |
| Validation completed | Pass | Git whitespace check passed before the documentation commits |

## Initial targets

The proposed alpha and pre-1.0 thresholds live in
[quality-engineering.md](../quality-engineering.md):

- 90% overall line coverage in alpha, ratcheting to 95% pre-1.0;
- 95% for adapter normalization/core files;
- 70% mutation score in alpha, ratcheting to 80% pre-1.0;
- 100% pass rate for selected contract and invariant tests;
- zero high-confidence secret findings and blocking lint/type findings.

These are targets, not current results. The first implementation baseline must
record test collection, coverage, mutation scope, skips, runtime, and known
gaps before any threshold is enforced.

## Next evidence

- [Issue #1](https://github.com/ferminquant/ai-usage-bar/issues/1): snapshot contract.
- [Issue #11](https://github.com/ferminquant/ai-usage-bar/issues/11): CI quality evidence.
- [Issue #12](https://github.com/ferminquant/ai-usage-bar/issues/12): contract and invariant suite.
- [Issue #13](https://github.com/ferminquant/ai-usage-bar/issues/13): coverage and mutation ratchet.
