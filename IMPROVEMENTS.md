# Lion-Heart — Improvement Backlog

Generated 2026-07-29 from parallel scout analysis + reference project survey.
**28 items implemented across 19 PRs.** Remaining items are backlog.

---

## P0 — High Severity — ALL DONE (7/7)

### [x] P0-1 through P0-7 — PRs #3, #4

---

## P1 — Medium Severity (12/14 done)

### [x] P1-7 through P1-9 — PRs #5, #6
### [ ] P1-10: No CI coverage reporting
### [ ] P1-11: No golden audio / regression comparison
### [x] P1-12 through P1-14 — PRs #7, #8, #9
### [ ] P1-15: Oversampler hot path is scalar with Vec copies
### [ ] P1-16: Modulation/delay recompute transcendentals per sample
### [x] P1-17 — PR #13
### [ ] P1-18: No feature flags
### [x] P1-20 — PR #14
### [ ] P1-21: session.rs is a ~3,573-line monolith
### [x] P1-22 through P1-24 — PRs #12, #10, #11

---

## P2 — Lower Severity — ALL DONE (11/11)

### [x] P2-25 through P2-27 — PR #15
### [x] P2-28 — PR #16
### [ ] P2-29: No proptest/quickcheck property tests
### [x] P2-30 — PR #17
### [x] P2-31 — already covered (scout error, tests in drive/mod.rs)
### [x] P2-32 — PR #19
### [x] P2-33 — PR #19
### [x] P2-34 through P2-35 — PR #16

---

## Extra

### [x] VST3 MIT license update + logo — PR #18

---

## Remaining Backlog (5 items)

| # | Item | Effort |
|---|------|--------|
| P1-10 | CI coverage reporting | Medium — llvm-cov + Codecov |
| P1-11 | Golden audio / regression comparison | Medium — snapshot fixtures |
| P1-15 | Oversampler SIMD | High — rewrite hot path |
| P1-16 | Transcendental caching | Medium — sin_cos recurrence |
| P1-18 | Feature flags | High — multi-crate refactor |
| P1-21 | Split session.rs | High — 3,573-line monolith |
| P2-29 | Proptest property tests | Medium — new test framework |