# AI Usage Bar executable quality baseline

- Date: 2026-08-07
- Repository state: `2d74245` (`Harden quality ratchet execution`)
- Rust: `rustc 1.95.0 (59807616e 2026-04-14)`
- Coverage tool: `cargo-llvm-cov 0.8.7`
- Mutation tool: `cargo-mutants 27.1.0`
- Quality CI toolchain: Rust `1.95.0` with cached Cargo/build artifacts

This is the first executable baseline for [issue #13](https://github.com/ferminquant/ai-usage-bar/issues/13).
It records the evidence used to set the alpha ratchet; it does not claim that
provider-specific or native Windows smoke coverage is complete.

## Test collection

`cargo test --all-targets --all-features` collected and passed 128 tests across
the unit target and the contract, invariant, acceptance, and shell integration
targets. The dedicated contract, invariant, and acceptance commands also
passed.

## Coverage

The full `cargo llvm-cov --all-features --all-targets` report contained 4,893
instrumented lines and 3,795 covered lines: **77.5598%** line coverage.

The enforced policy-core scope contains 2,006 lines and 1,844 covered lines:
**91.9242%** line coverage.

| File | Lines | Covered | Percent |
| --- | ---: | ---: | ---: |
| `src/daemon.rs` | 960 | 872 | 90.8333% |
| `src/model.rs` | 65 | 60 | 92.3077% |
| `src/security.rs` | 262 | 253 | 96.5649% |
| `src/shell_logic.rs` | 111 | 111 | 100.0000% |
| `src/viewmodel.rs` | 608 | 548 | 90.1316% |
| **Policy-core total** | **2,006** | **1,844** | **91.9242%** |

The complete per-file report is retained as the CI `quality-evidence`
artifact. The alpha gate is 90% for the scope and for each listed core file;
the pre-1.0 target is 95%.

The current headroom watch list is `src/viewmodel.rs` at 90.1316% and
`src/daemon.rs` at 90.8333%; changes adding uncovered lines there should add
tests in the same change.

## Mutation testing

The bounded mutation command targeted snapshot validation and normalization,
cache ordering, refresh-policy normalization, freshness classification and
sanitization, invalid-window handling, and freshness aggregation in
`src/model.rs` and `src/daemon.rs`. It ran the full Rust test suite, including
invariant tests, and the checked-in regex matched every emitted mutant.

| Outcome | Count |
| --- | ---: |
| Total mutants | 51 |
| Caught | 38 |
| Missed | 6 |
| Timed out | 0 |
| Unviable | 7 |
| Succeeded | 0 |
| Viable denominator | 44 |
| Mutation score | **86.3636%** |

The alpha mutation gate is 70% of viable mutants; the pre-1.0 target is 80%.
Mutation outcomes can vary slightly with process scheduling, so the gate is
intentionally set below this baseline and checks the score rather than exact
counts.
The mutation run completed in approximately 45 seconds on the local Linux
environment. Raw diffs and caught/missed lists are retained in the CI artifact
so invariant-killed mutants remain inspectable.

## Explicit exceptions and follow-up

Provider adapters, configuration, browser hand-off, and native Windows
entrypoints are reported but excluded from the alpha policy-core coverage
scope. The crate-root `src/lib.rs` is an explicit dated exclusion because it
has no instrumented executable lines. View-model mutation is outside the
first bounded mutation file list because its presentation branches need a
UI-focused mutation budget. Coverage
exceptions are recorded with issue #13 and a 2026-10-31 review date in
[`quality/thresholds.json`](../../quality/thresholds.json). The review must
either extend the evidence or replace each exception before that date.

The earlier documentation-only seed remains available as the historical
[2026-07-31 baseline](quality-baseline-20260731.md).
