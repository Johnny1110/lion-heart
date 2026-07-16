# CLAUDE.md

## Project

Lion-Heart: an open-source guitar amp & multi-effects processor for macOS, written in Rust. Standalone app first (recording + live use), CLAP/VST3 plugin later. Tone core = NAM captures (via `nam-rs`) + cabinet IR convolution; every other effect is hand-written DSP.

**Authoritative plan:** `docs/white-paper.md` (Traditional Chinese) — vision, requirements, architecture, tech decisions, milestones. Deltas against it are recorded as ADRs in `docs/adr/`. If code and white paper disagree, flag it — never silently diverge.

## Communication

- Reply to the user in **Traditional Chinese (zh-TW)**.
- Code, comments, commit messages, and repo docs are **English** (exceptions: the white paper and any doc explicitly written in Chinese).
- The user is a Java/Go backend engineer and electric guitarist: fluent in systems and backend concepts, learning audio DSP as the project progresses. Explain DSP theory when it drives a decision; skip backend basics.

## Current phase

**M7 (plugin & release) — code landed.** Two parts:

1. **Stereo bus** — `Effect::process(left, right)` end to end; gate/comp/limiter
   linked detectors, drive/EQ dual channel state, modulation true stereo
   (quadrature LFO; tremolo auto-pans), reverb dual ±1 Hadamard tap mixes off
   the shared FDN, NAM mono-sums, cab dual convolvers; duplex runner duplicates
   the mono input to L/R and interleaves out (even device channels L, odd R).
   ADR 002 marked implemented.
2. **Plugin** — `plugin/lion-heart-plugin` wraps the same chain via nih-plug
   (git dep, **pinned rev** in the workspace `Cargo.toml`): manual
   `unsafe impl Params` built from the effect descriptors (every param + per-slot
   bypass host-automatable, real units, stepped params show labels), a
   **Preset (assets)** IntParam loads NAM/IR from `~/.lion-heart/presets/` on
   nih-plug's background thread through the same `AssetHandle` hot-swap seam.
   Chain order fixed in the plugin (no editor yet). **Passes clap-validator
   (16/16 applicable)**. `cargo xtask bundle lion-heart-plugin --release` makes
   `target/bundled/Lion-Heart.{clap,vst3}`; VST3 builds are GPLv3 (crate license
   differs from the workspace on purpose). Release pipeline:
   `.github/workflows/release.yml` (tag `v*` → macOS build → draft release),
   `scripts/codesign-notarize.sh` gated on Apple secrets — see `docs/release.md`.

Pending user verification on the Mac: stereo width by ear (chorus/reverb/
tremolo), plugin in a real host (Reaper/Bitwig/Live: insert, pick a preset,
automate knobs), foot controller end-to-end, `--buffer 32` on hardware, RTL
numbers into `docs/latency.md`. **v0.1 tagging is the user's call** after that
verification (`git tag v0.1.0 && git push origin v0.1.0` drafts the release).

- `lh-midi` (new crate): PC/CC parsing and a JSON mapping
  (`~/.lion-heart/midi.json`: `input` port, `channel` filter, `pc_presets` names,
  `cc` → `"slot.param"` or bare `"slot"` for bypass). Zero-config default: connect
  the first input port, **PC n loads the n-th preset (sorted)**. midir events are
  forwarded over `mpsc` to the control thread — the engine queue stays SPSC; MIDI
  never touches the audio thread. Connection failure is never fatal.
- Session drains MIDI in the control loop (`drain_midi()` → applied-action lines);
  jam prints them, the GUI shows them in the status line and re-syncs its state.
  `--midi <port>` overrides on both; `lion-heart devices` lists MIDI inputs too.
- GUI **live view** ("live" chip): big preset name, prev/next preset buttons, big
  meters, mini tuner readout, chain summary — stage mode.
- GUI **settings panel** ("settings" chip): input/output device, input channel,
  and buffer size at runtime. Apply restarts the stream via
  `Session::carry_over()`/`resume()` (chain state + assets survive; on failure it
  rolls back to the previous configuration). Applied choices persist in
  `~/.lion-heart/config.json` and fill in whatever the CLI left unspecified —
  explicit flags still win (`--buffer`/`--in-channel` are now optional args,
  defaults unchanged). `devices::select` prefers exact name matches over
  substrings so full-name GUI picks are unambiguous.
- **Drive model registry — ADR 003.** The drive slot has a stepped `model`
  param (`ts9`, `blues driver`, `classic`) + three pedal-style position knobs
  0–10; each model is a `Circuit` impl (nonlinear `shape` at 4× OS rate,
  linear `post` at base rate) registered in `lh_dsp::drive::MODELS` — append
  a `ModelDef` (label, knob captions, builder) and the GUI dropdown + knob
  captions ("Gain" on blues driver), REPL labels, MIDI, plugin param pick it
  up. Preset **schema v2**: v1 drive values (dB/Hz) migrate through
  `lh_core::drive_law` inverses onto `model=classic` — old presets sound
  identical. Stepped params render as dropdowns in the GUI (mod type too).
- 32-frame target verified: the 8-pedal hand-written chain is ~4 µs per 32-frame
  block (0.6 % of the 667 µs deadline, linear scaling — see `docs/benchmarks.md`),
  null-device run at `--buffer 32` clean under assert_no_alloc.

M5 recap: ten-slot chain gate→comp→drive→amp→eq→mod→delay→reverb→cab→limiter;
`Range::Stepped { labels }` for the mod-type param (labels work in REPL/UI);
8-line Householder FDN reverb, **mono — ADR 002** defers stereo to the M7 bus.
Old presets load forward-compatibly; the limiter is always moved back to last.

Pending user verification on the Mac: foot controller end-to-end (PC preset
switch, CC expression → param, CC bypass), live view at stage distance,
`--buffer 32` xruns on real hardware, settings panel against real devices
(switch Scarlett ↔ built-in mid-jam, buffer change, unplug rollback), drive
models by ear (ts9 mid-hump vs blues driver openness, knob tapers, model
switch mid-note, an old preset still sounding right), plus the standing items
(M5 pedals by ear, tuner sanity, RTL numbers into `docs/latency.md`).

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
| `lh-dsp`         | Effects (gate, drive, delay, …). Offline-testable, RT-safe        | `lh-core`     |
| `lh-engine`      | RT graph runner, node lifecycle, lock-free plumbing               | core, dsp     |
| `lh-nam`         | `NamAmp` effect + `.nam` loading/validation (nam-rs seam)         | core, dsp     |
| `lh-io`          | cpal device management, duplex runner, latency measurement        | core          |
| `lh-midi`        | MIDI foot control: PC/CC parsing, mapping, midir input            | —             |
| `lh-assets`      | IR WAV loading: decode, sinc-resample, normalize, build convolver | dsp           |
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
