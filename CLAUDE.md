# CLAUDE.md

## Project

Lion-Heart: an open-source guitar amp & multi-effects processor for macOS, written in Rust. Standalone app first (recording + live use), CLAP/VST3 plugin later. Tone core = NAM captures (via `nam-rs`) + cabinet IR convolution; every other effect is hand-written DSP.

**Authoritative plan:** `docs/white-paper.md` (Traditional Chinese) — vision, requirements, architecture, tech decisions, milestones. Deltas against it are recorded as ADRs in `docs/adr/`. If code and white paper disagree, flag it — never silently diverge.

## Communication

- Reply to the user in **Traditional Chinese (zh-TW)**.
- Code, comments, commit messages, and repo docs are **English** (exceptions: the white paper and any doc explicitly written in Chinese).
- The user is a Java/Go backend engineer and electric guitarist: fluent in systems and backend concepts, learning audio DSP as the project progresses. Explain DSP theory when it drives a decision; skip backend basics.

## Current phase

**Released: v0.1.0.** The nine-feature roadmap (PRDs 001–019, ADRs 001–026) is
complete and committed — engine, session, plugin, GUI, and the full effect
families (dynamics / drive / eq / modulation / filter / pitch / time / cab /
power + practice tools + recorder/re-amp + setlists/leveling).

> **For the detailed build history, read `git log` + `docs/PRD/` (001–021) +
> `docs/adr/` (001–029) — not this file.** Those are authoritative for what
> landed and why. This section only tracks the **current direction**.

**Since v0.1.0** — two open lines from the white paper:

1. **Deep-water research line** (white paper §6 — WDF white-box circuit
   modelling). Proved the WDF approach on single pedals:
   - **#1 WDF Tube Screamer clipping stage** — PRD 020 / ADR 028, **committed**
     (`9a6de75`). New reusable `lh_dsp::blocks::wdf` (bilinear `Capacitor`,
     antiparallel `DiodePair` root via warm-started damped Newton in f64,
     `parallel_root` adaptor); new drive pedal `screamer` (shunt RC-diode clipper).
   - **#2 WDF feedback overdrive + asymmetric clipping** — PRD 021 / ADR 029.
     `blocks::wdf` grew `AsymDiode` + `parallel_root_with_source`; new drive pedal
     `sd1` (Boss SD-1, diodes in the feedback loop, ideal-op-amp virtual short).
   - **angry-charlie-v2** drive pedal (routine append-only, no PRD/ADR).
   - Drive family is now **14 pedals** (`MODELS` / `DRIVE_PEDALS`, append-only).
2. **Cross-platform port** — ADR 027, **Windows-first**. Portable `~/.lion-heart`
   path resolution landed; Windows CI first-green + WASAPI hardware verification
   deferred (needs a Windows runner).

### Next version — Tone Revolution (drive + tone-stack overhaul)

**This is the next version's headline work.** Full plan:
**`docs/tone_revolution/overview.md`** + `docs/tone_revolution/phase/01..08-*.md`
(zh-TW, PRD-style). It scales the deep-water WDF line (#1/#2 above) from
one-off pedals into a **framework + the full named-pedal roster**, born from a
study of `/mnt/BYOD` (Build-Your-Own-Distortion, ChowDSP, **GPL-3**) and
`/mnt/chowdsp_wdf` (the WDF library, **BSD-3** — the mature upstream of our
`blocks::wdf`). Three goals:

1. **A real interactive tone stack.** Today `drive::ToneStack` is three
   independent additive one-pole bands = a graphic EQ; a real Fender/Marshall
   FMV/TMB stack is a *coupled* passive network with knob interaction + an
   intrinsic mid-scoop at noon. This also fixes the FMV-voiced drives that bake
   it in (red-charlie / monster5150 / angry-charlie / evva).
2. **Port the whole named-pedal drive roster** as WDF white-box (op-amp
   overdrives + fuzz/transistor) — "I want all the drives."
3. **Make `blocks::wdf` a platform for the user's own pedal R&D.**

**Phase map** (each = a `phase/NN-*.md` with concrete work + acceptance):

1. **Wright Omega** closed-form diode solver — replaces the f64 Newton
   (~68 µs → ~10 µs). Highest-ROI, most-independent step; **do first**.
2. **Tone-stack framework** — analytic coupled FMV/Baxandall transfer function
   (cheap, exact, linear); fixes the headline complaint. **New ADR 030.**
3. **WDF composable adaptor framework** — Series/Parallel + **R-Type** adaptor +
   op-amp model (the substrate for the whole op-amp family). **New ADR 031.**
4. **op-amp overdrive family** — TS (faithful) / ZenDrive / King of Tone /
   MXR Distortion+ / RAT + selectable diodes.
5. **fuzz / transistor / booster** — Big Muff / Fuzz Face / Rangemaster.
6. **waveshaper bank + ADAA** anti-aliasing (also de-fizzes existing memoryless
   drives).
7. **neural / tube** (Centaur / GuitarML / triode) — heaviest, **optional/
   deferred** (model-weight licensing).
8. **self-R&D platform** — netlist → R-Solver → codegen, SPICE-fit workflow,
   "add-a-WDF-pedal" cookbook.

**Licensing red line:** lion-heart is **MIT OR Apache-2.0**. **BYOD is GPL-3 —
never copy its code.** Port algorithms from `chowdsp_wdf` (BSD) / `omega.h`
(MIT); take circuit topologies + component values + diode SPICE params as
*facts*; regenerate R-Type scattering matrices with **R-Solver** (don't paste
BYOD's). Formalized into the main sequence these are **PRD 022+ / ADR 030+**.

### Operational notes

Debug builds install `assert_no_alloc::AllocDisabler` (app `main.rs`) and wrap the audio
processor: **an allocation on the audio thread aborts with SIGABRT (exit 134)** — treat
that as a real-time violation to fix, never a crash to paper over. It already caught an
undersized oversampler scratch buffer that offline tests missed.

Hardware verification outstanding (macOS + interface): record RTL numbers in
`docs/latency.md`; play through `jam` sweeping params by ear to confirm no clicks.

Note for sandboxed/Linux dev environments: everything compiles and unit-tests without
audio hardware; the ALSA "null" device (usually index 0) exercises the stream pipeline
(including assert_no_alloc) but has no real clock, so its xrun counts are meaningless.

### Commands

```sh
cargo build                                    # debug build
cargo fmt --check                              # formatting gate
cargo clippy --all-targets -- -D warnings      # lint gate
cargo test                                     # all tests run offline, no device needed
cargo bench -p lh-dsp --bench effects          # per-block DSP cost (criterion)
cargo run -p lion-heart --release              # the GUI (no subcommand)
cargo run -p lion-heart -- devices             # list devices
cargo run -p lion-heart --release -- run       # passthrough (Ctrl-C to stop)
cargo run -p lion-heart --release -- jam       # pedalboard + control REPL
cargo run -p lion-heart --release -- latency   # RTL measurement (loopback cable)
cargo run -p lion-heart --release -- render di.wav --preset lead  # offline re-amp (PRD 014)
```

Plugin bundling: `cargo xtask bundle lion-heart-plugin --release` →
`target/bundled/Lion-Heart.{clap,vst3}`; conformance:
`clap-validator validate target/bundled/Lion-Heart.clap`.

The GUI spike workspace has its own gates (run from `spikes/`):
`cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.

CI (`.github/workflows/ci.yml`) runs fmt/clippy/test/build on macOS and Ubuntu
(root workspace only; `spikes/` is excluded).

## Workspace layout

| Crate            | Responsibility                                                    | May depend on |
| ---------------- | ----------------------------------------------------------------- | ------------- |
| `lh-core`        | Param IDs & ranges, chain model, preset schema. No I/O, no threads | —             |
| `lh-dsp`         | Effects, one module per family (dynamics, filter, drive, power, eq, modulation, time, cab, pitch, looper, acoustic) over shared `blocks/`; plus non-slot modules `practice` (aux monitor: metronome/drums/song), `loudness` (LUFS), `tuner`. Offline-testable, RT-safe | `lh-core`     |
| `lh-engine`      | RT graph runner, node lifecycle, lock-free plumbing               | core, dsp     |
| `lh-nam`         | `NamAmp` effect + `.nam` loading/validation (nam-rs seam)         | core, dsp     |
| `lh-io`          | cpal device management, duplex runner, latency measurement        | core          |
| `lh-midi`        | MIDI foot control: PC/CC parsing, mapping, midir input            | —             |
| `lh-assets`      | IR WAV loading (decode, sinc-resample, normalize, build convolver), general WAV read/write (`wav`, PRD 014) + the `~/.lion-heart` disk layout shared by app & plugin | dsp           |
| `app/lion-heart` | Standalone GUI application (iced)                                 | everything    |
| `plugin/…`       | CLAP/VST3 wrapper via nih-plug (GPLv3 for VST3 builds)            | core→assets   |

GUI code is never imported by `lh-*` crates — the engine must build and test without any UI.

## Real-time audio rules (non-negotiable)

Applies to all code reachable from the audio callback (`lh-engine`, `lh-dsp`, RT paths of `lh-nam`):

1. **No heap allocation or deallocation.** No `Box::new`, no `Vec` growth beyond preallocated capacity, no `format!`, no cloning heap types.
2. **No locks** (`Mutex`, `RwLock`), no blocking channels, no `async`.
3. **No syscalls**: no file/network I/O, no `println!`/`log` macros. Debug via a lock-free ring buffer drained by another thread.
4. Cross-thread communication only via **`rtrb` SPSC rings, `triple_buffer`, atomics, or `arc-swap`** pointer swaps.
5. Objects are **built on worker threads**, swapped in atomically; retired objects are sent back to a worker for dropping — never dropped on the RT thread.
6. Parameter changes go through the **smoothing layer**; never hard-jump a value that reaches the signal path.
7. **Denormals**: enable flush-to-zero in the callback; feedback paths must not sustain denormals. No NaN may escape a node — debug builds assert on non-finite output.
8. Debug builds wrap the callback in **`assert_no_alloc`**.

## DSP conventions

- `f32` samples. Mono chain by default; stereo only where inherent (reverb/modulation outputs onward).
- Engine canonical sample rate is **48 kHz** (NAM models are rate-locked — white paper §5.3). Device rate mismatches are handled at the I/O boundary, never inside effects.
- Every effect implements the common `Effect` trait (process block, reset, apply params) and must run offline: pure buffer-in/buffer-out, no device, no threads.
- Tests: golden/null tests against fixtures with an explicit tolerance; property tests (no NaN/inf, bounded output, silence-in → silence-out after reset); `criterion` benches report per-block cost at 48 kHz / 64 samples.
- Rate-dependent code is tested at 44.1/48/96 kHz and block sizes 32–1024.

## Dependency policy

- RT-path dependencies get their process-path code read for allocations/locks **before** adoption.
- Pin `nam-rs` to a minor version; treat its parity fixtures as part of our CI expectations.
- No C++/FFI unless the pure-Rust path is proven insufficient; the sanctioned fallback is NeuralAmpModelerCore behind the same `AmpModel` trait, and it requires an ADR.

## Unsafe policy

`unsafe` only at FFI boundaries or in proven-hot SIMD kernels, each with a `// SAFETY:` invariant comment and a covering test. Prefer safe SIMD (`wide`, portable-simd) before intrinsics.

## Workflow

- Before commit: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
- Commits: imperative subject, prefixed with the milestone when applicable (`M2: add IR convolver node`).
- Irreversible or architectural decisions → `docs/adr/NNN-short-title.md` (context / decision / consequences).
</content>
