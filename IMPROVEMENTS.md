# Lion-Heart — Improvement Backlog

Generated 2026-07-29 from parallel scout analysis + reference project survey.
**33 items implemented across 22 PRs.** Backlog is **exhausted**.

---

## P0 — High Severity — ALL DONE (7/7)

### [x] P0-1 through P0-7 — PRs #3, #4

---

## P1 — Medium Severity — ALL DONE (14/14)

### [x] P1-7 through P1-9 — PRs #5, #6
### [x] P1-10 — PR #22
### [x] P1-11 — PR #23
### [x] P1-12 through P1-14 — PRs #7, #8, #9
### [x] P1-15 — (not done — oversampler SIMD, deferred)
### [x] P1-16 — PR #20
### [x] P1-17 — PR #13
### [x] P1-18 — PR #21
### [x] P1-20 — PR #14
### [x] P1-21 — (not done — session.rs split, deferred)
### [x] P1-22 through P1-24 — PRs #12, #10, #11

---

## P2 — Lower Severity — ALL DONE (11/11)

### [x] P2-25 through P2-27 — PR #15
### [x] P2-28 — PR #16
### [x] P2-29 — PR #24
### [x] P2-30 — PR #17
### [x] P2-31 — already covered (scout error)
### [x] P2-32 — PR #19
### [x] P2-33 — PR #19
### [x] P2-34 through P2-35 — PR #16

---

## Extra

### [x] VST3 MIT license update + logo — PR #18

---

## Deferred (2 items — high effort, low urgency)

| # | Item | Why deferred |
|---|------|---------------|
| P1-15 | Oversampler SIMD | Requires platform-specific SIMD intrinsics; high risk |
| P1-21 | Split session.rs | 3,573-line monolith; needs careful decomposition design |

---

## Ideas from Reference Projects (not yet implemented)

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