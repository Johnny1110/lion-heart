# CLAUDE.md

## Project

Lion-Heart: an open-source guitar amp & multi-effects processor for macOS, written in Rust. Standalone app first (recording + live use), CLAP/VST3 plugin later. Tone core = NAM captures (via `nam-rs`) + cabinet IR convolution; every other effect is hand-written DSP.

**Authoritative plan:** `docs/white-paper.md` (Traditional Chinese) — vision, requirements, architecture, tech decisions, milestones. Deltas against it are recorded as ADRs in `docs/adr/`. If code and white paper disagree, flag it — never silently diverge.

## Communication

- Reply to the user in **Traditional Chinese (zh-TW)**.
- Code, comments, commit messages, and repo docs are **English** (exceptions: the white paper and any doc explicitly written in Chinese).
- The user is a Java/Go backend engineer and electric guitarist: fluent in systems and backend concepts, learning audio DSP as the project progresses. Explain DSP theory when it drives a decision; skip backend basics.

## Current phase

**M8 (freeboard) — code landed.** Three features, specced in `docs/PRD/`
(001–003, zh-TW) and recorded as ADRs 004–006:

1. **Per-pedal params — PRD 001, ADR 004.** `FamilyDesc` in lh-core: a chain
   slot hosts a family of pedals and **every pedal owns its faceplate**
   (`EffectDesc`: TS9 3 knobs, evva 5 incl. its 3-band EQ, tremolo 2 —
   its redundant mix folded into depth). Knob **memory** lives in the
   ChainHandle's per-pedal shadow: switching sends `SelectPedal` then
   re-sends the incoming pedal's values; effects hold no cross-pedal state.
   Preset **schema v3** stores every pedal's values per slot; v1/v2
   migrations keep old files sounding identical (`preset::DRIVE_PEDALS` /
   `MOD_PEDALS` pin registry order). Virtual `slot.pedal` selector in
   REPL/MIDI (`model`/`type` accepted as aliases). The plugin statically
   expands all pedals' params (`drive_ts9_drive`, …) plus a stepped
   `{slot}_pedal` selector; host state per pedal is the memory there.
2. **Dynamic chain — PRD 002, ADR 005.** Slots are instances: same family
   several times, addressed as `family`/`familyN` by chain rank (`drive2`).
   Engine grew `InstallSlot` (control-side-prepared, applied silently
   outside the audible order) and `RemoveSlot` (at the fade bottom, after
   the pending order) plus a retire chute — untouched slots keep their
   tails; an install cancels a racing pending removal of its index.
   Presets define the structure: load reconciles (claim same-family
   survivors → remove leftovers → install missing via the session's
   `build_family_effect` factory). amp/cab stay singletons (asset mounts;
   re-adding remounts the loaded asset). Max 12 slots; empty chain =
   passthrough; limiter no longer pinned last. GUI chain strip is the
   board editor (drag cards to move, ＋ pick-list to add, remove in the
   params panel); REPL: `add`/`remove`, `order` takes handles.
3. **Global output EQ — PRD 003, ADR 006.** Fixed output stage in `Chain`
   after the master fade: **global EQ → safety limiter → spectrum tap**.
   The safety limiter (−0.3 dBFS, always on, invisible) carries white
   paper §3.3 now that the chain limiter is removable. EQ: 8 bands
   (low/high-cut, shelves, bells; `lh_core::global_eq` state), smoothed
   block-rate coefficient rebuilds + per-band wet crossfades,
   bit-transparent when off. Persisted app-globally in
   `~/.lion-heart/global_eq.json` — deliberately **not** in presets
   (environment, not tone). GUI "eq" chip: log-freq canvas with the live
   output spectrum (realfft on the GUI thread, 4096-pt Hann, ~30 Hz,
   fast-attack/slow-release) under the response curve computed from the
   same RBJ math as the audio path; drag = freq/gain, wheel = Q,
   double-click = enable/disable; detail strip for type/readouts/flat/
   master. `global_eq_4band` criterion bench tracks the stage cost.

**Post-M8 polish — code landed (uncommitted):**

1. **lh-dsp category layout.** Effects grouped one module per kind:
   `dynamics/` (gate/comp/limiter), `drive/` (**one pedal, one file** —
   registry + `Circuit` + shared `OnePole`/`Ramp` in `drive/mod.rs`),
   `eq/` (`chain.rs` = the 3-band pedal, `global.rs` = the output-stage
   EQ, ex-`param_eq`), `time/` (delay/reverb), `blocks/` (biquad,
   oversample, smooth, swap); `modulation`/`cab`/`tuner` stay root-level
   categories. Public paths moved (e.g. `lh_dsp::dynamics::NoiseGate`,
   `lh_dsp::eq::global::GlobalEq`) — all call sites updated, including
   the `spikes/` workspace, which was also repaired to the stereo/M8
   engine API (it had bit-rotted; its gates are green again).
2. **red-charlie drive pedal** (Marshall JCM800 2203-style; born
   "jcm800", renamed before ever shipping — no preset shim): cascaded
   stages — warm asymmetric stage 1, cathode-network low trim (~8 dB
   below 100 Hz) + 120 Hz interstage coupling for the tight low end,
   gain-dependent bright cap (strongest at low gain), cold-clipper
   stage 2 (0.4/1.0 knees) — FMV-voiced Bass/Middle/Treble
   (100/650/3300 Hz) via the `eq()` hook, Master on the shared level
   law. Gain pot +8..+56 dB — the top ~12 dB beyond a stock 2203 is the
   "screamer in front" solo reach the user asked for after playing it;
   the audio taper keeps 0..4 in stock crunch territory (MAKEUP
   re-trimmed 0.22→0.18 to keep noon in the family's unity window).
   ~10.3 µs per 64-frame stereo block. EQ-band tests probe with
   real multi-tone inputs (`tones()` helper — projecting onto absent
   frequencies read the noise floor; evva's test upgraded to match).
2b. **monster5150 drive pedal** (EVH 5150-style high gain): three-stage
   cascade ending in a *very* cold clipper (0.35/0.9), pre gain
   +12..+60 dB — **no clean floor**; tightness carved pre-gain (low trim
   below 120 Hz + a second 180 Hz coupling), Low knob restores lows
   post-distortion (resonance-style, 80 Hz), fixed bright pre-emphasis,
   ~6.8 kHz post fizz lowpass; Pre/Low/Mid/High/Post faceplate
   (mid 550 Hz, high 3 kHz). Character pinned by a **sustain test**
   (−38 dBFS tail stays ≥1.4× louder than red-charlie's — residuals
   plateau at the square-wave ceiling, compression is the honest
   high-gain metric). ~13.3 µs per block. `DRIVE_PEDALS` is now 7,
   append-only respected; plugin params expand automatically.
3. **GUI v2.** Header = view tabs (board · tuner · eq · live) with
   settings set apart top-right; a **persistent preset bar** (◀ picker ▶,
   save-as field — replaces the presets overlay, rescans the dir ~1 Hz);
   window 960×640. Bug fixed: clicking/dragging a chain card now always
   returns to the board view with that slot's faceplate open
   (`select_position` sets `View::Board`) — it used to do nothing while
   tuner/eq/live/settings was up.
4. **Health pass (no behavior change intended).** Family knowledge now has
   one home: `lh_core::DEFAULT_CHAIN` is the canonical rig order,
   `session::FAMILY_REGISTRY` (desc + mounted-asset kind + builder) replaces
   the FAMILIES string list / `build_family_effect` match / hardcoded
   `Session::start` chain, and the plugin's fixed chain is pinned to the
   same constant by a test — adding a family is now one registry entry.
   `~/.lion-heart` path helpers (`presets_dir`/`list_presets`) moved to
   lh-assets, shared by app and plugin (the sorted preset list is a
   cross-binary contract: MIDI PC and the plugin preset param index it).
   Shared 3-band `ToneStack` in `drive/mod.rs` replaces the triplicated
   `eq()` in evva/red-charlie/monster5150 (gains now Ramp-mapped: 2 powf
   per chunk instead of 3 per sample); one-pole coefficient math deduped
   into `blocks::{onepole_ms, onepole_hz}`. Perf (Apple Silicon, measured):
   chain EQ and global EQ skip coefficient rebuilds while controls are
   settled (global_eq_4band ~1.66 µs → ~0.80 µs; eq_3band ~0.60 → ~0.38 µs —
   docs/benchmarks.md now has a native Apple Silicon section);
   GUI knob drags update one param in place instead of re-snapshotting the
   whole chain per mouse-move. Benched-and-rejected (kept the original
   code): branchless ring-buffer wrap in delay/mod/reverb (+10% on delay —
   the div pipelines, the branches don't) and a below-threshold compressor
   fast path (+8% in the worst case, which is the RT budget). Also: engine
   output stage now finds the safety-limiter ceiling by key, not `params[0]`;
   out-of-range `set_param` no longer panics in chain-EQ/reverb.

**M9 (delay family) — code landed (uncommitted).** Specced in PRD 004,
recorded as ADR 007. The `delay` slot became a **three-pedal family**
(`digital`/`tape`/`vintage`, family key unchanged), one shared interpolated
delay engine `match`ing per-voice `VoiceDef` constants (one file per pedal
under `time/delay/`, like drive). New shared controls: **tone** (feedback-path
lowpass, dark⇄bright, compounding per repeat, settled-skip coefficient),
**mod** as each voice's signature knobs (tape Wow+Flutter, vintage Mod —
depth over voice-fixed LFO rates; digital none), and **tap tempo**. digital is
clean/linear (feedback ≤ 0.9); tape/vintage soft-clip the feedback
(`tanh(drive·x)/drive`, unity small-signal, `1/drive` ceiling) so feedback
≥ 1.0 self-oscillates into a *bounded* drone — never NaN/runaway. **Tap** is
control-side, GUI-only: a `subdivision` stepped param (stored in presets,
no-op in the DSP audio path) plus a per-slot `TapState` in the GUI that times
taps and sets `time = period × subdivision`; flipping subdivision re-derives
from the last tempo. Preset **schema v4**: `migrate_v3_delay_pedal` renames the
old `delay` pedal → `digital` (`DELAY_PEDALS` pins the family, `time/feedback/
mix` carry over, old files sound the same bar a brighter default tone). Engine
/ session / plugin needed **no code changes** (the multi-pedal path already
covered them); the plugin auto-expands per-voice params — **pre-v0.1 param-id
break** (`delay_digital_time`, …, `delay_pedal`), re-run clap-validator.

Pending user verification on the Mac: pedal switching by ear (per-pedal
values restored, faceplates correct), **red-charlie by ear** (crunch vs
the other drives, bright low-gain edge, B/M/T reach, unity at defaults),
**monster5150 by ear** (chug tightness, sustain, fizz level, Low-knob
post-distortion thickness, no-clean floor acceptable),
the reworked GUI (tabs, preset bar prev/next/save, chain-click landing
on the board from every view), board editing while playing (drag/
add/remove — tails keep ringing through the fade), a 3-drive board saved
and reloaded, the EQ panel against real playing (spectrum sanity, drag
feel, persistence across restarts), plugin re-check in a real host
(drive/mod param ids changed and red-charlie/monster5150 params
appeared — pre-v0.1 break; re-run clap-validator, now with the delay voices
expanded too), **the three delays by ear** (digital clean, tape wobble +
warmth, vintage dark/gooey self-oscillation, tone sweep, feedback into bounded
self-oscillation), **tap tempo** (two taps lock the time, subdivision reshapes
the echo, BPM readout), a delay-heavy board saved/reloaded and an **old v3
preset** loaded (delay → digital, still rings), plus the standing M7 items
(stereo width by ear, foot controller end-to-end, `--buffer 32` on hardware,
RTL numbers into `docs/latency.md`). **v0.1 tagging is the user's call**
after that.

M7 recap: stereo bus end to end (ADR 002 implemented) and the
CLAP/VST3 plugin via nih-plug (pinned rev) with the release pipeline
(`.github/workflows/release.yml`, `scripts/codesign-notarize.sh`,
`docs/release.md`). M6: MIDI foot control (`lh-midi`, `~/.lion-heart/
midi.json`, zero-config PC n → nth preset; mpsc → control thread, engine
queue stays SPSC), GUI live view + settings panel
(`Session::carry_over()`/`resume()`, config-persisted I/O). M5: the
ten-slot chain and the 8-line Householder FDN reverb; the hand-written
chain is ~4 µs per 32-frame block (`docs/benchmarks.md`).

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
| `lh-dsp`         | Effects, one module per category (dynamics, drive, eq, modulation, time, cab) over shared `blocks/`. Offline-testable, RT-safe | `lh-core`     |
| `lh-engine`      | RT graph runner, node lifecycle, lock-free plumbing               | core, dsp     |
| `lh-nam`         | `NamAmp` effect + `.nam` loading/validation (nam-rs seam)         | core, dsp     |
| `lh-io`          | cpal device management, duplex runner, latency measurement        | core          |
| `lh-midi`        | MIDI foot control: PC/CC parsing, mapping, midir input            | —             |
| `lh-assets`      | IR WAV loading (decode, sinc-resample, normalize, build convolver) + the `~/.lion-heart` disk layout shared by app & plugin | dsp           |
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
