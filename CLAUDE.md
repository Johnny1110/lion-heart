# CLAUDE.md

## Project

Lion-Heart: an open-source guitar amp & multi-effects processor for macOS, written in Rust. Standalone app first (recording + live use), CLAP/VST3 plugin later. Tone core = NAM captures (via `nam-rs`) + cabinet IR convolution; every other effect is hand-written DSP.

**Authoritative plan:** `docs/white-paper.md` (Traditional Chinese) — vision, requirements, architecture, tech decisions, milestones. Deltas against it are recorded as ADRs in `docs/adr/`. If code and white paper disagree, flag it — never silently diverge.

## Communication

- Reply to the user in **Traditional Chinese (zh-TW)**.
- Code, comments, commit messages, and repo docs are **English** (exceptions: the white paper and any doc explicitly written in Chinese).
- The user is a Java/Go backend engineer and electric guitarist: fluent in systems and backend concepts, learning audio DSP as the project progresses. Explain DSP theory when it drives a decision; skip backend basics.

## Current phase

**M4 (the face) — code landed.** `lion-heart` with **no subcommand opens the iced
GUI** (framework decided by the spike in `spikes/` — see
`docs/adr/001-gui-framework.md`; vizia lost on the skia-safe C++ dependency,
deprecated-OpenGL macOS backend, and its 0.4 state rewrite). The GUI: chain strip
(select / bypass / ◀▶ reorder, limiter pinned last), per-param canvas knobs, NAM/IR
file browsers (last directory per kind persisted in `config.json`), preset browser
(save / click-to-load), header peak meters, and a **tuner** — YIN pitch detection in
`lh_dsp::tuner` (±2-cent tests at 44.1/48/96 kHz) fed by a raw-input rtrb tap
installed via `Chain::set_input_tap` (lock-free chunk write, drop-on-full).
`app/.../session.rs` owns chain build + stream startup + asset/preset/config logic,
shared by the GUI and the jam REPL. Everything rides `window::frames()`: telemetry
poll + meter ballistics per frame, asset GC every 12 frames, tuner estimate at
~15 Hz. Audio startup failure renders an in-window error screen (device fixes
included). The engine↔UI contract: the UI thread owns `ChainHandle`/asset handles,
mutates only in `update()`, and never blocks the audio thread.

Pending user verification on the Mac: GUI holds 60 fps and xruns don't increase
with the UI open (footer shows both), tuner sanity-check against a reference,
RTL numbers into `docs/latency.md`, by-ear click tests.

M3 recap: runtime chain reorder rides a ~4 ms master fade through silence; versioned
JSON presets reference assets by **path + SHA-256** with same-name relocation;
`~/.lion-heart/{config.json, presets/}` persists state; last preset auto-loads. The
NAM fixture in `crates/lh-nam/tests/fixtures/` is vendored from nam-rs (MIT).

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
```

The GUI spike workspace has its own gates (run from `spikes/`):
`cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.

CI (`.github/workflows/ci.yml`) runs fmt/clippy/test/build on macOS and Ubuntu
(root workspace only; `spikes/` is excluded).

## Workspace layout

| Crate            | Responsibility                                                    | May depend on |
| ---------------- | ----------------------------------------------------------------- | ------------- |
| `lh-core`        | Param IDs & ranges, chain model, preset schema. No I/O, no threads | —             |
| `lh-dsp`         | Effects (gate, drive, delay, …). Offline-testable, RT-safe        | `lh-core`     |
| `lh-engine`      | RT graph runner, node lifecycle, lock-free plumbing               | core, dsp     |
| `lh-nam`         | `NamAmp` effect + `.nam` loading/validation (nam-rs seam)         | core, dsp     |
| `lh-io`          | cpal device management, duplex runner, latency measurement        | core          |
| `lh-assets`      | IR WAV loading: decode, sinc-resample, normalize, build convolver | dsp           |
| `app/lion-heart` | Standalone GUI application (iced)                                 | everything    |

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
