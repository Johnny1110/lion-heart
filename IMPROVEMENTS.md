# Lion-Heart — Improvement Backlog

Generated 2026-07-29 from parallel scout analysis of the codebase + reference
project survey. Items P0-1 through P0-7 and P1-7 through P1-9 are
**implemented and verified** (648 tests pass). Remaining items are backlog.

---

## Status Legend

- [x] Done — implemented, tested, verified
- [ ] Open — not yet started
- [~] Partial — some work done, more remaining

---

## P0 — High Severity (correctness, RT safety)

### [x] P0-1: Audio-thread order Vec allocation
**File:** `crates/lh-engine/src/lib.rs` ~1704

`build_chain` created `order` with only `count` capacity. A later `SetOrder`
with more slots could grow the Vec inside `Chain::process` — an allocation on
the audio thread.

**Fix:** Pre-reserve `MAX_SLOTS` capacity: `Vec::with_capacity(MAX_SLOTS)`.

### [x] P0-2: No FTZ/DAZ setup on audio threads
**Files:** entire `lh-dsp` crate, `lh-io`, `plugin`

No MXCSR/FPCR setup anywhere. Denormals in biquad states, modulation allpass
histories, delay rings, and reverb buffers could cause CPU spikes.

**Fix:** New `crates/lh-core/src/rt.rs` with `flush_denormals_to_zero()` using
inline asm (x86/x86_64: MXCSR bits 15+6; aarch64: FPCR bit 24). Called at the
top of the cpal output callback and plugin `process`. Also added per-component
denormal flush in `Biquad::process_sample`.

### [x] P0-3: Plugin default-chain active-state divergence
**File:** `plugin/lion-heart-plugin/src/lib.rs` ~165

Plugin built all effects with `active=true`, then `last_active` was initialized
from host `BoolParams` defaults (`default_active` → filter/power=false). `process`
only sends `SetActive` on *changes*, so bypassed slots stayed active.

**Fix:** `initialize` now explicitly pushes all bypass defaults to
`ChainHandle` before the first `process` call.

### [x] P0-4: QueueFull leaves control shadow / audio state inconsistent
**File:** `crates/lh-engine/src/lib.rs` ~1264-1366

`ChainHandle` methods mutated shadow state *before* `rtrb` pushes. On
`QueueFull` the mirror diverged from audio state.

**Fix:** `set_order`, `move_position`, `remove_slot`, `spill_slot` now build
the new order locally, check queue capacity, push all messages, then update
shadow state. `QueueFull` leaves state unchanged.

### [x] P0-5: Biquad accepts unstable parameters
**File:** `crates/lh-dsp/src/blocks/biquad.rs` ~45-160

Setters accepted `fc >= Nyquist`, `q <= 0`, nonfinite values. Low host sample
rate + GlobalEq max 20k could produce unstable poles.

**Fix:** Added `sanitize()` that clamps `fc` to (10 Hz, Nyquist×0.999), `q` to
(0.1, 100). `set()` falls back to unity on invalid `a0`. All 8 setters call
`sanitize` first. 5 new tests.

### [x] P0-6: Base-rate nonlinear stages alias without oversampling
**Files:** `power.rs`, `time/delay/mod.rs`, `time/reverb/mod.rs`, `dynamics/limiter.rs`

Delay tape/vintage feedback `soft_clip`, reverb spring/shimmer/magneto clips,
and power transformer `tanh` all operated at base rate without ADAA or
oversampling.

**Fix:** Replaced `soft_clip` and transformer `tanh` with first-order ADAA
using the existing `Adaa1` + `tanh_f1` from `waveshaper.rs`. Added `Adaa1`
state to `DelayChannel`, `Reverb`, and `PowerAmp`. Reset on pedal switch and
`reset()`. Removed now-unused `soft_clip` functions. Limiter hard clamp kept
as-is (safety guarantee, not tonal — test contract requires unity below
ceiling).

### [x] P0-7 (was #19): SetOrder audio handler trusts unvalidated indices
**File:** `crates/lh-engine/src/lib.rs` ~720

Malformed `EngineMsg` (len > MAX_SLOTS, duplicate indices, unoccupied slots)
could panic the audio callback.

**Fix:** Validate len, index bounds, slot occupancy, and uniqueness before
applying. Malformed messages are dropped silently.

---

## P1 — Medium Severity

### [x] P1-7: No end-to-end production-chain test
**Files:** `crates/lh-engine/tests/chain.rs`, `app/lion-heart/src/render.rs`

`full_chain_renders_finite_audio` only tests Gate→Drive→Delay. The real
12-stage default chain (gate, filter, comp, drive, NAM, power, EQ, mod, delay,
reverb, cab, limiter) is never tested as a whole.

**Fix:** Added 4 tests: `default_chain_renders_finite_audio` (full 12-stage
chain), `default_chain_output_is_bounded` (hot signal, limiter ceiling),
`default_chain_block_partition_equivalent` (32 vs 1024 frames), and
`default_chain_survives_all_sample_rates` (44.1/48/96 kHz). PR #5.

### [x] P1-8: Allocation testing only covers Drive models
**File:** `crates/lh-dsp/tests/alloc.rs`

`alloc.rs` iterates only the 24 `Drive` models. No no-allocation assertions
for Modulation, Reverb, Delay, EQ, Filter, Compressor, PowerAmp, Pitch,
Looper, Cab, NAM, or `Chain::process`/tap/spill paths.

**Fix:** Extended from 1 test to 11 — added modulation (8 voices), delay (3),
reverb (12), EQ, filter, compressor (3 voices), noise gate, limiter, power
amp, and cab IR. All verified allocation-free under parameter sweeps. PR #5.

### [x] P1-9: Plugin formats never validated in CI
**Files:** `.github/workflows/ci.yml`, `.github/workflows/release.yml`

CI runs fmt/clippy/test/build only. No `clap-validator`, VST3 validator, or
`pluginval`. The release workflow bundles but never validates.

**Fix:** Added `plugin-validation` CI job: installs `clap-validator`, bundles
the plugin via `cargo xtask bundle --release`, validates CLAP bundle. Runs on
Linux. PR #6.

### [ ] P1-10: No CI coverage reporting
No `llvm-cov`/`grcov`/tarpaulin/Codecov anywhere.

**Plan:** Add LLVM source coverage + upload + modest ratchet threshold.

### [ ] P1-11: No golden audio / regression comparison
No committed WAV/checksum/spectral snapshots. Tests assert broad properties
but not stable output.

**Plan:** Add deterministic golden vectors or compact FFT/impulse-response
metric snapshots per effect family + full-chain fixture. Provide a
regeneration command/review path.

### [ ] P1-12: EQ coefficient smoothing endpoint jumps
**Files:** `eq/chain.rs` ~108-135, `eq/tonestack.rs` ~866-889, GlobalEq ~179-200

Tone smoothers tick `n` samples then set coefficients once per block. With
`MAX_BLOCK=1024` at 48k, that's ~21ms endpoint jumps. Drive's ToneStack
already uses `EQ_REBUILD=64` sub-blocks — apply the same pattern to chain
tone, global EQ, and tone-stack.

### [ ] P1-13: Dynamics parameters not smoothed
**Files:** `dynamics/gate.rs` ~10-33, `dynamics/comp/mod.rs` ~72-98, `dynamics/limiter.rs` ~9-32

Gate threshold/release, compressor threshold/ratio/attack/release/sidechain
HPF, and limiter ceiling/release all have `smoothing_ms=0` and feed the gain
computer directly. Automation produces clicks.

**Plan:** Smooth detector/gain parameters or crossfade. Add ramp tests.

### [ ] P1-14: Reverb collapses stereo to mono in wet path
**File:** `crates/lh-dsp/src/time/reverb/mod.rs` ~1016-1019

`dry = 0.5*(L+R)` feeds a mono wet tank, collapsing inter-channel content.

**Plan:** Use dual/M-S/decorrelated core, or document the mono-fed limitation
and test stereo separation.

### [ ] P1-15: Oversampler hot path is scalar with Vec copies
**File:** `crates/lh-dsp/src/blocks/oversample.rs` ~64-137

Every chunk: `Vec::extend`/`clear`/copies + scalar 17/16/33-tap FIR loops.

**Plan:** Replace with ring/fixed history buffer and SIMD/polyphase kernels.

### [ ] P1-16: Modulation/delay recompute transcendentals per sample
**Files:** `modulation.rs` ~356-430, `time/delay/mod.rs` ~424-469

Phaser/univibe recompute `tan` per stage per sample. Delay does 4 `sin`/sample
+ `powf`/`exp` on tone movement.

**Plan:** Use `sin_cos` recurrence, sub-block coefficient caching, and LFO
approximation.

### [ ] P1-17: Duplicated default-chain factory: plugin vs app
**Files:** `plugin/.../lib.rs` ~79-97 vs `app/.../session.rs` ~912-1030

Two independent runtime chain constructors that can drift.

**Plan:** Extract a shared `EffectFactory`/default-chain builder crate or
module that both consume.

### [ ] P1-18: No feature flags — everything compiles unconditionally
**Files:** `Cargo.toml`, `crates/lh-dsp/Cargo.toml`, `app/lion-heart/Cargo.toml`

`lh-dsp` always compiles looper/pitch/acoustic/practice. App always pulls
`iced`/`symphonia`/`realfft`/`hound`.

**Plan:** Add cargo feature gates (`gui`, `midi`, `nam`, `practice`,
`offline`, `testutil`) so engine-only iteration skips GUI compile cost.
*(Reference: rusty-amp gates `clap`/`au` hosting behind features.)*

### [ ] P1-20: lh-assets/lh-nam leak third-party types across boundaries
`lh-assets` exposes `fft_convolver::FFTConvolver` via `IrPair`/`IrAsset` and
`hound::Error` in `AssetError`. `lh-nam` exposes `nam_rs::Model` in `NamAsset`.

**Plan:** Make assets opaque; wrap third-party errors to preserve crate
boundaries.

### [ ] P1-21: app/lion-heart/src/session.rs is a ~3,573-line monolith
Split into board/preset/settings/practice controllers or view models with a
`Session` façade. GUI `mod.rs` is also a giant controller.

### [ ] P1-22: Documentation status drift
**Files:** `CLAUDE.md`, `README.md`, `docs/install.md`, `docs/release.md`

`CLAUDE.md` says v0.1.0 released + phases landed; `README.md` says M7
pre-alpha + lists Windows/Linux as non-goals; `docs/install.md` says no
prebuilt release + Linux/Windows "planned".

**Plan:** Choose canonical status and update all four together.

### [ ] P1-23: Release workflow is macOS-only
**File:** `.github/workflows/release.yml` ~19-46

Despite CI running on 3 OSes, release artifacts are macOS-only. No
Linux/Windows release tarballs.

**Plan:** Add matrix package jobs or explicitly document mac-only. Add
SHA256SUMS, license/readme, and generated changelog to release assets.

### [ ] P1-24: No cargo-audit / cargo-deny / supply-chain checks
No `deny.toml`, no advisory/license/ban checks.

**Plan:** Add `cargo-deny` config + scheduled job. GPL exception should be
scoped to plugin/VST3 only.

---

## P2 — Lower Severity (polish, ergonomics)

### [ ] P2-25: No rust-toolchain.toml / rust-version pin
Docs claim 1.85+, CI floats stable. Add workspace `rust-version=1.85` +
`rust-toolchain.toml` with exact tested toolchain + MSRV CI job.

### [ ] P2-26: No project lint configuration
No `.rustfmt.toml` or `clippy.toml`. *(Reference: rusty-amp has a thorough
`clippy.toml` — cognitive-complexity-threshold=15, missing-docs-in-crate-items=true,
too-many-lines-threshold=200, max-struct-bools=3, too-many-arguments-threshold=7.)*

### [ ] P2-27: Makefile bench target only runs lh-dsp; no --locked anywhere
`make bench` only runs `lh-dsp --bench effects`. CI/Makefile commands omit
`--locked`. Add `--locked` everywhere + `.cargo/config.toml` alias. Add
`bench-all` target covering engine + NAM.

### [ ] P2-28: Criterion bench macro allocates inside b.iter
**File:** `crates/lh-dsp/benches/effects.rs` — `bench_stereo!`

Calls `signal()` (Vec allocation) inside every Criterion iteration,
contaminating process timings. Preallocate immutable input buffers outside
`b.iter`.

### [ ] P2-29: No proptest/quickcheck property tests
Add property tests for: block partition invariance, reset idempotence,
boundedness/finite output for generated parameter vectors, dry-path identity,
migration serialize→deserialize stability. Start with pure helpers/WDF roots.

### [ ] P2-30: Missing LICENSE files
No `LICENSE-MIT`/`LICENSE-APACHE` files despite manifest
`license = "MIT OR Apache-2.0"`. Add license texts + VST3 GPL notice.

### [ ] P2-31: 11 drive models lack model-local tests
`ts9`, `bd2`, `classic`, `centaur`, `evva`, `red_charlie`, `monster5150`,
`angry_charlie`, `jan_ray`, `fuzz_face`, `overdrive` have no `#[cfg(test)]`
module. Family-wide loops cover finiteness/DC/silence but not model-specific
transfer/voicing/topology regression.

### [ ] P2-32: Modulation chorus/flanger/phaser lack dedicated behavioral tests
Only generic loops cover these; tremolo/vibrato/harmonic/rotary/univibe have
named character tests. Add per-voice impulse/comb-notch or phase/feedback
tests.

### [ ] P2-33: No 192 kHz test coverage
Family rate matrices stop at 96 kHz. Add 44.1/48/96/192 kHz × irregular-block
(1, 32, 483, 1024) smoke test for all family registries.

### [ ] P2-34: Plugin preset IntParam hardcodes max 99
**File:** `plugin/lion-heart-plugin/src/lib.rs` ~403-425

Hardcoded max 99 presets despite unlimited `list_presets`. Host can't select
>99. Use dynamic count or documented hard cap.

### [ ] P2-35: codesign-notarize.sh lacks cleanup trap
**File:** `scripts/codesign-notarize.sh` ~20-27

Changes default keychain with no `trap` to restore/delete on failure. Temp
cert cleanup only runs on success. Add cleanup trap + unique tempdir + restore
default keychain.

---

## Ideas from Reference Projects

Surveyed 201 reference projects in `/home/shawn/workspace2/guitar-rig/reference/`.
Ideas not yet in lion-heart:

| Idea | Source | Relevance |
|------|--------|-----------|
| **Two-stage FFT convolver** — short head block + long tail block + background thread for low-latency cab IR | `FFTConvolver-non-uniform` | Cab IR latency reduction |
| **NAM `prewarm()` API** — settle model initial conditions before audio starts | `NeuralAmpModelerCore` | Avoid startup transient with NAM models |
| **Tone3000 API client** — OAuth/PKCE model marketplace download | `tone3000-rs` | In-app NAM/IR browsing & download |
| **TOML presets** — human-readable, version-controllable | `rusty-amp` | Preset format alternative/complement |
| **Analysis examples** (`di_compare`, `cab_spectrum`, `rig_loudness`, `knob_match`) | `rusty-amp` | Developer tooling for DSP verification |
| **`cliff.toml`** — automated conventional-commit changelog generation | `rustortion` | Release workflow automation |
| **`env_logger` + `log`** — structured runtime logging | `rustortion` | Debugging without `println!` |
| **Dev profile `opt-level=1`** — keep IR/convolver code fast in debug builds | `rustortion` | Faster debug iteration |
| **`criterion` `html_reports`** — browsable benchmark dashboards | `rustortion` | Benchmark UX |
| **`tempfile` in dev-deps** — clean test fixture management | `rustortion` | Test hygiene |
| **`nextest.toml`** — faster, parallel test runner with retry isolation | `rusty-amp` | Test UX and CI speed |
| **Plugin hosting (CLAP/AU as inserts)** — host other plugins inside the chain | `rusty-amp` | Extensibility |