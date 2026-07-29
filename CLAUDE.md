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

**Phases 01, 02 and 06 landed** (2026-07-27) — the three quick wins of the
revised order:

- **01** Wright Omega closed-form diode root (PRD 022).
- **02** Tone-stack framework (PRD 023 / **ADR 030**): `eq::tonestack` turns a
  netlist into a state space and Tustin-discretises it at block rate;
  `bassman`/`jcm800`/`big-muff` models; the five FMV-voiced drives migrated off
  the old additive 3-band (**voicing change**, character tests re-pinned); a
  standalone `tonestack` pedal appended to the `eq` family. **Makeup recalibrated
  2026-07-29 (ADR 037)** after ear acceptance failed on LF overflow: band-average
  unity had lifted the noon low shelf +4–5 dB *absolute* (measured against a
  pre-02 build; the netlists were right, only the calibration point was wrong).
  Makeup now pins the noon **ceiling** to unity (bassman +1.63 / jcm800 +0.98 /
  big-muff +4.05), so noon sits ≤ 0 dB everywhere like the passive network
  itself.
- **06** Waveshaper + ADAA (PRD 024 / **ADR 031**): `blocks::waveshaper` gives
  first/second-order antiderivative anti-aliasing plus a 12-curve bank; **every
  memoryless clipper in the drive family** now runs through it (alias floor −29
  → −38…−87 dB, character pins unchanged); new `waveshaper` drive pedal.
  Drive family is now **15 pedals**.

- **03** WDF composable adaptor framework (PRD 025 / **ADR 032**), landed
  2026-07-28 — the architectural one. `blocks::wdf` split into five files;
  owned generic trees (`Parallel<A, B>` / `Series<A, B>` / `PolarityInverter`)
  with root-driven waves and post-order `calc_impedance`; `RType<N, M, P>` whose
  **scattering matrix is built numerically from a junction netlist at knob rate**
  (not offline symbolic codegen — the plan's fallback promoted to primary, see
  ADR 032); finite-gain op-amp as three junction elements. `screamer`/`sd1`
  rewritten onto it, ~1e-8 relative against the pre-rewrite commit — **no tonal
  change, no new pedals**.

- **04** op-amp overdrive family (**in progress, 1 of 6**) — first pedal
  **`ts-wdf`** landed 2026-07-28 (PRD 026 / **ADR 033**): the whole TS clipping
  amplifier as one WDF tree on the phase-03 framework, with an optional-diode
  faceplate (Diode selector + continuous Count). Two policy decisions in ADR 033
  bind every remaining pedal in the family — **op-amp `Ag`/`Ri`/`Ro` come from
  the part's datasheet, not from BYOD** (BYOD's `Ag=100` measurably suppresses
  the drive sweep's top end and the `C4` treble cut the phase's own acceptance
  criteria require), and **diode menus carry `(Is, n)` per device, not `Is`
  alone** (BYOD's `1N34 → 200 pA` makes germanium clip *higher* than silicon).
  **`zendrive`** landed the same day (PRD 027 / **ADR 034**) — the *same
  junction* as `ts-wdf` with different parts, which is why the shared layout now
  lives in the framework as `blocks::wdf::{NON_INVERTING_NODES,
  NON_INVERTING_PORTS, non_inverting_els}`. Its clipper (1N4148 + diode-connected
  2N7002 per branch) was fitted here to that device's curve. ADR 034 also
  **corrects the phase plan**: BYOD's ZenDrive diode params were *not* distorted
  by its P1/P3 wiring bug (they were fitted offline against a standalone LTspice
  clipper) and the ~3× thermal voltage is *not* compensation — it is two devices
  in series. The wiring bug is real and is fixed here; the params are refitted
  for a different, measurable reason (fitted `Is·sinh`, evaluated `2·Is·sinh`).
  **`mxr-dist`** followed (PRD 028, no new ADR) — the first of the family to clip
  *shunt to ground at the output* rather than inside the feedback loop, so the
  same junction is adapted at a different port. `blocks::wdf` gained
  `NON_INVERTING_OUT_PORTS` alongside `NON_INVERTING_PORTS`, sharing
  `non_inverting_els`; the rule is **the up port goes where the nonlinearity
  is**. Then **`rat`** (PRD 029 — loop gain below one, on purpose), the platform
  piece **`diode-clipper`** (PRD 030 — one diode wired four ways; added
  `Ctl::Mode`), and **`king-of-tone`** (PRD 031 — two stages, two roots, and the
  family's only soft feedback clipper). **Phase 04 is complete at six pedals;
  the drive family is now 21.** Five of them share one junction across two
  adapted-port layouts, and no scattering matrix is written down anywhere.

- **05** fuzz / transistor / booster (PRD 032–034 / **ADR 035**), landed
  2026-07-29 — the phase that stops using WDF, on purpose. New module
  **`blocks::transistor`**, because a WDF root is by definition a *single-port*
  nonlinearity and this family breaks that in two different ways: `big-muff`'s
  amplifier is a common-emitter stage linearised to `A = −Rc/Re`, so it has no
  input/output impedance an R-type junction could use (and ADR 033 forbids
  inventing them); a BJT is a *two-port* nonlinearity, so it can never be a root
  at all. **`big-muff`** (PRD 032) is two `ShuntFeedbackStage`s — the same
  mechanism class as `sd1`, solved as one node equation by damped Newton — into
  the Phase 02 `big-muff` tone stack, the first pedal to use it.
  **`rangemaster`** (PRD 033) is a three-node Ebers–Moll solve over
  `blocks::transistor::Bjt`, its DC operating point solved once at `prepare`
  (never on the audio thread — `reset()` runs there) and pinned against four
  lines of hand arithmetic. **`fuzz-face`** (PRD 034) gained a
  Germanium/Silicon selector and stays behavioural: the reference NDK
  coefficients come from a private generator, so **NDK is recorded as future
  research** (ADR 035 §4). Two corrections to the reference implementation are in
  ADR 035 §3, both found by re-deriving the equations here — the Big Muff
  feedback current is injected across the **AC** Thévenin resistance `R19‖R20`
  (9.09 kΩ), not the DC one (`R20`, 100 kΩ), which is 6.5× of gain per stage; and
  the Rangemaster's `*1e16 − 5e5 − 1.0` output scaling is a *plotting* leftover
  from feeding NPN equations to a PNP. Device parameters are germanium's, not the
  reference's silicon — ADR 033's diode policy, one device class along. **Drive
  family is now 23.** Cost note: `rangemaster` is the most expensive pedal in the
  family (~4.6× `screamer`, ~12 % of the deadline); it converges in 2 Newton
  steps, so that is the price of the device model, not slack.

- **08** self-R&D platform (PRD 035–036 / **ADR 036**), landed 2026-07-29 — the
  plan's closing phase, and **the one whose plan drifted furthest**: it was
  written before phase 03, and phase 03 replaced its foundation. Both of its
  toolchains are void — §2.1's R-Solver scattering-matrix codegen has **nothing
  to generate** (ADR 032 made the matrix a run-time numerical construction from
  a junction netlist that *is* Rust, so `tools/wdf_codegen/` and
  `tools/netlists/` do not and will not exist), and §2.2's SPICE flow needed
  ngspice, which is absent and which phase 02 had already replaced with its own
  nodal oracle. What the platform actually lacked was a way to know a tree is
  right, so the phase delivers **a second solver**: `testutil::netlist`, an
  independent modified-nodal-analysis reference sharing no code, formula or
  constant with `blocks::wdf`. Both discretise capacitors trapezoidally, so they
  are two views of *one* discrete system and the residual is attributable —
  series/parallel tree 2.5e-7 V, **R-type junction 3.2e-5 relative** (that is
  `f32` in a scattering solve conditioned at a few hundred, and it is the floor
  for any comparison), and **the closed-form root's error is 1400× the tree's**,
  pinned as its own test. Alongside it: `testutil::whitebox` (the discrimination
  kit — `memory` reads 3.4e-6 for a curve and 1.52 for a circuit),
  `tests/whitebox.rs`, `docs/tone_revolution/cookbook.md`, and
  `tools/fit_device.py` (device-level `(Is, n)` fitting, no simulator). The
  example pedal is **`mane`** (PRD 036), specified by the user as *clipping
  **and** voicing*: Focus sweeps the gain leg's capacitor **inside** the feedback
  loop (it decides which frequencies break up — the same low E goes from 0.275
  THD to 0.004, 70×), Bass/Mid/Treble run the phase-02 passive JCM800 network
  **after** it. Hand-solved AC analysis to **0.22 %**, and **no new junction, no
  new adaptor, no new root** — which is the phase's actual claim. ADR 036 §3 adds
  one policy: datasheet is the source for op-amp parameters, but **numerical
  conditioning is a constraint** — a JFET's 1e12 Ω `Ri` is indistinguishable from
  1e9 Ω in this junction and 1e12 wrecks the `f32` solve. **Drive family is
  now 24.** Cost note: `mane` is 2.47× `screamer`, and the benchmark attributes
  it — not the R-type junction (1.30×) and not the tone stack (free), but the
  **asymmetric Newton root**, which has no closed form where the symmetric pair
  has PRD 022's. That is now a priced work item, not a wish.

The full phase map and each phase's acceptance criteria live in the docs linked
above. Four plan docs carry correction boxes at the top: phase 02's makeup
calibration was re-done after ear acceptance (ADR 037, see the 02 bullet),
phase 04's still says the scattering matrix comes from **R-Solver** and asks
for `tools/netlists/` (ADR 032 superseded both), phase 05's ADR number, module
placement, "scalar" Newton claim and cost estimate were all revised on landing
(ADR 035), and phase 08's two toolchains, its optional tweakable-component mode
and its ZenDrive re-fit acceptance item were all dropped (ADR 036).

**Only phase 07 (neural / tube, optional) is left unscheduled.**

**Licensing red line:** lion-heart is **MIT OR Apache-2.0** (including VST3
builds — the VST3 SDK is now MIT-licensed as of SDK 3.8.x). **BYOD is GPL-3 —
never copy its code.** Port algorithms from `chowdsp_wdf` (BSD) / `omega.h`
(MIT); take circuit topologies + component values + diode SPICE params as
*facts*. **R-Type scattering matrices are never transcribed from anywhere** —
since ADR 032 they are derived numerically from our own junction netlist at run
time (the plan's original "regenerate with R-Solver" route was dropped; the
reasoning is in that ADR). Formalized into the main sequence these are
**PRD 022+ / ADR 030+**.

### Adding a pedal

`docs/tone_revolution/cookbook.md` is the step-by-step (zh-TW): which solver a
circuit belongs to, where component parameters come from, the three verification
layers, and the append-only registry rules. `crates/lh-dsp/src/drive/mane.rs` is
the worked example that follows it end to end.

### Operational notes

Debug builds install `assert_no_alloc::AllocDisabler` (app `main.rs`) and wrap the audio
processor: **an allocation on the audio thread aborts with SIGABRT (exit 134)** — treat
that as a real-time violation to fix, never a crash to paper over. It already caught an
undersized oversampler scratch buffer that offline tests missed.

Since PRD 026 the same check also runs **offline** for the drive family:
`crates/lh-dsp/tests/alloc.rs` is a separate test binary that installs
`AllocDisabler` as its own global allocator and sweeps every pedal's knobs under
`assert_no_alloc`. Put new RT-path coverage there rather than in the library's
unit tests — `#[global_allocator]` is crate-wide.

Hardware verification outstanding (macOS + interface): record RTL numbers in
`docs/latency.md`; play through `jam` sweeping params by ear to confirm no clicks.

Note for sandboxed/Linux dev environments: everything compiles and unit-tests without
audio hardware; the ALSA "null" device (usually index 0) exercises the stream pipeline
(including assert_no_alloc) but has no real clock, so its xrun counts are meaningless.

### Commands

```sh
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

## Workspace layout

| Crate            | Responsibility                                                    | May depend on |
| ---------------- | ----------------------------------------------------------------- | ------------- |
| `lh-core`        | Param IDs & ranges, chain model, preset schema. No I/O, no threads | —             |
| `lh-dsp`         | Effects, one module per family over shared `blocks/`; plus non-slot modules `practice` (aux monitor), `loudness`, `tuner`. Offline-testable, RT-safe | `lh-core`     |
| `lh-engine`      | RT graph runner, node lifecycle, lock-free plumbing               | core, dsp     |
| `lh-nam`         | `NamAmp` effect + `.nam` loading/validation (nam-rs seam)         | core, dsp     |
| `lh-io`          | cpal device management, duplex runner, latency measurement        | core          |
| `lh-midi`        | MIDI foot control: PC/CC parsing, mapping, midir input            | —             |
| `lh-assets`      | IR WAV loading (decode, sinc-resample, normalize, build convolver), general WAV read/write (`wav`, PRD 014) + the `~/.lion-heart` disk layout shared by app & plugin | dsp           |
| `app/lion-heart` | Standalone GUI application (iced)                                 | everything    |
| `plugin/…`       | CLAP/VST3 wrapper via nih-plug (MIT/Apache-2.0)                   | core→assets   |

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
