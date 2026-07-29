# Lion-Heart — Improvement Backlog

Generated 2026-07-29 from parallel scout analysis + reference project survey.
**26 items implemented across 18 PRs.** Remaining items are backlog.

---

## Status Legend

- [x] Done — implemented, tested, PR created
- [ ] Open — not yet started

---

## P0 — High Severity — ALL DONE (7/7)

### [x] P0-1: Audio-thread order Vec allocation — PR #3
### [x] P0-2: No FTZ/DAZ setup on audio threads — PR #3
### [x] P0-3: Plugin default-chain active-state divergence — PR #3
### [x] P0-4: QueueFull leaves control shadow inconsistent — PR #3
### [x] P0-5: Biquad accepts unstable parameters — PR #4
### [x] P0-6: Base-rate nonlinear stages alias — PR #4
### [x] P0-7: SetOrder trusts unvalidated indices — PR #3

---

## P1 — Medium Severity (12/14 done)

### [x] P1-7: No end-to-end production-chain test — PR #5
### [x] P1-8: Allocation testing only covers Drive — PR #5
### [x] P1-9: Plugin formats never validated in CI — PR #6
### [ ] P1-10: No CI coverage reporting
### [ ] P1-11: No golden audio / regression comparison
### [x] P1-12: EQ coefficient smoothing endpoint jumps — PR #7
### [x] P1-13: Dynamics parameters not smoothed — PR #8
### [x] P1-14: Reverb stereo collapse — PR #9 (documented as intentional)
### [ ] P1-15: Oversampler hot path is scalar with Vec copies
### [ ] P1-16: Modulation/delay recompute transcendentals per sample
### [x] P1-17: Duplicated default-chain factory — PR #13
### [ ] P1-18: No feature flags
### [x] P1-20: lh-assets/lh-nam leak third-party types — PR #14
### [ ] P1-21: session.rs is a ~3,573-line monolith
### [x] P1-22: Documentation status drift — PR #12
### [x] P1-23: Release workflow is macOS-only — PR #10
### [x] P1-24: No cargo-deny supply-chain checks — PR #11

---

## P2 — Lower Severity (7/11 done)

### [x] P2-25: No rust-toolchain.toml / rust-version pin — PR #15
### [x] P2-26: No project lint configuration — PR #15
### [x] P2-27: Makefile bench target / no --locked — PR #15
### [x] P2-28: Criterion bench macro allocates inside b.iter — PR #16
### [ ] P2-29: No proptest/quickcheck property tests
### [x] P2-30: Missing LICENSE files — PR #17
### [ ] P2-31: 11 drive models lack model-local tests
### [ ] P2-32: Modulation chorus/flanger/phaser lack dedicated tests
### [ ] P2-33: No 192 kHz test coverage
### [x] P2-34: Plugin preset IntParam hardcodes max 99 — PR #16
### [x] P2-35: codesign-notarize.sh lacks cleanup trap — PR #16

---

## Extra (not in original analysis)

### [x] VST3 MIT license update + logo — PR #18
Steinberg relicensed VST3 SDK to MIT (SDK 3.8.x). Plugin license changed
from GPL-3.0-or-later to MIT OR Apache-2.0. VST3 Compatible logo added.

---

## Remaining Backlog (7 items)

| # | Item | Effort |
|---|------|--------|
| P1-10 | CI coverage reporting | Medium — llvm-cov + Codecov |
| P1-11 | Golden audio / regression comparison | Medium — snapshot fixtures |
| P1-15 | Oversampler SIMD | High — rewrite hot path |
| P1-16 | Transcendental caching | Medium — sin_cos recurrence |
| P1-18 | Feature flags | High — multi-crate refactor |
| P1-21 | Split session.rs | High — 3,573-line monolith |
| P2-29 | Proptest property tests | Medium — new test framework |
| P2-31 | Drive model-local tests | Low — 11 inline test modules |
| P2-32 | Modulation behavioral tests | Low — 3 inline test modules |
| P2-33 | 192 kHz test coverage | Low — add rate to arrays |

---

## Ideas from Reference Projects

| Idea | Source | Relevance |
|------|--------|-----------|
| Two-stage FFT convolver | FFTConvolver-non-uniform | Cab IR latency reduction |
| NAM prewarm() API | NeuralAmpModelerCore | Avoid startup transient |
| Tone3000 API client | tone3000-rs | In-app NAM/IR browsing & download |
| TOML presets | rusty-amp | Preset format alternative |
| Analysis examples | rusty-amp | DSP verification tooling |
| cliff.toml changelog | rustortion | Release automation |
| env_logger + log | rustortion | Debugging without println! |
| Dev profile opt-level=1 | rustortion | Faster debug iteration |
| criterion html_reports | rustortion | Benchmark UX |
| tempfile in dev-deps | rustortion | Test hygiene |
| nextest.toml | rusty-amp | Test UX and CI speed |
| Plugin hosting (CLAP/AU) | rusty-amp | Extensibility |