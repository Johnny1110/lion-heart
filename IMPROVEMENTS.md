# Lion-Heart — Improvement Backlog

Generated 2026-07-29 from parallel scout analysis + reference project survey.
Items P0-1 through P0-7, P1-7 through P1-9, P1-12 through P1-14, P1-17,
P1-22, P1-23, and P1-24 are **implemented**. Remaining items are backlog.

---

## Status Legend

- [x] Done — implemented, tested, PR created
- [ ] Open — not yet started
- [~] Partial — some work done, more remaining

---

## P0 — High Severity (correctness, RT safety) — ALL DONE

### [x] P0-1: Audio-thread order Vec allocation — PR #3
### [x] P0-2: No FTZ/DAZ setup on audio threads — PR #3
### [x] P0-3: Plugin default-chain active-state divergence — PR #3
### [x] P0-4: QueueFull leaves control shadow / audio state inconsistent — PR #3
### [x] P0-5: Biquad accepts unstable parameters — PR #4
### [x] P0-6: Base-rate nonlinear stages alias without oversampling — PR #4
### [x] P0-7: SetOrder audio handler trusts unvalidated indices — PR #3

---

## P1 — Medium Severity

### [x] P1-7: No end-to-end production-chain test — PR #5
### [x] P1-8: Allocation testing only covers Drive models — PR #5
### [x] P1-9: Plugin formats never validated in CI — PR #6
### [ ] P1-10: No CI coverage reporting
### [ ] P1-11: No golden audio / regression comparison
### [x] P1-12: EQ coefficient smoothing endpoint jumps — PR #7
### [x] P1-13: Dynamics parameters not smoothed — PR #8
### [x] P1-14: Reverb collapses stereo to mono in wet path — PR #9 (documented as intentional, test added)
### [ ] P1-15: Oversampler hot path is scalar with Vec copies
### [ ] P1-16: Modulation/delay recompute transcendentals per sample
### [x] P1-17: Duplicated default-chain factory — PR #13
### [ ] P1-18: No feature flags — everything compiles unconditionally
### [ ] P1-20: lh-assets/lh-nam leak third-party types across boundaries
### [ ] P1-21: app/lion-heart/src/session.rs is a ~3,573-line monolith
### [x] P1-22: Documentation status drift — PR #12
### [x] P1-23: Release workflow is macOS-only — PR #10
### [x] P1-24: No cargo-audit / cargo-deny / supply-chain checks — PR #11

---

## P2 — Lower Severity (polish, ergonomics)

### [ ] P2-25: No rust-toolchain.toml / rust-version pin
### [ ] P2-26: No project lint configuration
### [ ] P2-27: Makefile bench target only runs lh-dsp; no --locked anywhere
### [ ] P2-28: Criterion bench macro allocates inside b.iter
### [ ] P2-29: No proptest/quickcheck property tests
### [ ] P2-30: Missing LICENSE files
### [ ] P2-31: 11 drive models lack model-local tests
### [ ] P2-32: Modulation chorus/flanger/phaser lack dedicated behavioral tests
### [ ] P2-33: No 192 kHz test coverage
### [ ] P2-34: Plugin preset IntParam hardcodes max 99
### [ ] P2-35: codesign-notarize.sh lacks cleanup trap

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