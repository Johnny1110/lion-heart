# ADR 025: The comp slot becomes a three-pedal family (VCA / Opto / FET)

Status: **accepted — implemented**
Date: 2026-07-22
Relates to: PRD 015 (`docs/PRD/015-comp-family.md`), ADR 004 (per-pedal
params), ADR 007 (delay: the single-pedal→family + migration precedent),
ADR 008 (reverb family), ADR 017 (eq family-ization), white paper §4.2

## Context

The chain `comp` slot is a single transparent digital VCA compressor
(`dynamics::comp`, threshold/ratio/attack/release/makeup). The user wants
more than one compression *character* — the three classic hardware
topologies each feel different under the fingers: the clean VCA leveler, the
slow round LA-2A opto, and the fast biting 1176 FET.

## Decision

Grow `comp` from one pedal to a **three-pedal family** (`vca`/`opto`/`fet`),
following the delay (ADR 007) and reverb (ADR 008) path: one shared engine
that reads per-voice `VoiceDef` constants in the hot loop, one file per pedal
under `dynamics/comp/`.

- **One detector → gain-computer engine.** A linked peak follower feeds a
  dB-domain gain computer; `VoiceDef` switches the knee, the ratio law, and
  the release law. No per-sample vtable (the delay/modulation pattern).
- **vca** — the pre-family compressor, verbatim: full Threshold/Ratio/
  Attack/Release, hard knee. At its defaults the audio path is **bit-exact**
  to the old compressor, save one deliberate change: a denormal flush on the
  detector envelope (RT rule 7 hygiene the old code lacked, inaudible below
  −400 dB).
- **opto** (LA-2A) — fixed slow attack (10 ms), **program-dependent** release
  (a two-stage recovery, 100 ms → 1.5 s, sliding by current gain-reduction
  depth: the deeper it compresses the slower it lets go), a **rising** ratio
  (2.5:1 → 8:1 as the signal pushes further over threshold: leveling →
  limiting), and a soft 12 dB knee. Faceplate is just Peak Reduction / Gain.
- **fet** (1176) — microsecond attack (20–800 µs), hard knee, and a
  **stepped** ratio (4/8/12/20/**All**); the all-buttons-in step drops the
  threshold 8 dB under the 20:1 curve for the aggressive pump.
- **Two shared knobs** on every voice (PRD 015): `blend` (parallel/New-York
  compression, `dry·(1−blend) + compressed·blend`, default 1.0 = fully
  compressed) and `sc_hpf` (a high-pass on the **sidechain detector** only,
  20–300 Hz, so bass stops ducking the mix; bypassed at its 20 Hz floor).
- **Schema v7 → v8** (`migrate_v7_comp_pedal`): the old `comp` pedal key is
  renamed to `vca` (the delay `delay`→`digital` move). `COMP_PEDALS` pins the
  registry order; a sparse `comp` slot falls back to index 0 = `vca`.
- **Plugin: pre-v0.1 param-id break** (M9 precedent). comp goes single →
  multi, so `comp_threshold` → `comp_vca_threshold`, `comp_opto_*`/
  `comp_fet_*` appear, and the stepped `comp_pedal` selector is added.
  Re-run clap-validator.

## Deltas from PRD 015

- **Schema bump is 7 → 8, not the PRD's "6 → 7".** The PRD (2026-07-20)
  predates dual-IR cab (ADR 015) and snapshots landing on v6/v7; the working
  tree was already at v7. The migration function is therefore
  `migrate_v7_comp_pedal`, gated `< 8`, and runs for any file below v8 (the
  intervening bumps added only `#[serde(default)]` fields, so a `comp`-keyed
  slot from *any* older version lands correctly on `vca`).
- **fet keeps a `threshold` knob** (a 7-knob face, not the bare
  attack/ratio/makeup the PRD prose sketched). A real 1176 has no threshold
  knob — you drive INPUT into a fixed threshold — but a compressor with no
  threshold *or* input control is nearly undialable, so `threshold` models
  that INPUT-drive. Same structure as vca; the FET character lives in the
  microsecond attack, hard knee, and stepped/all-buttons ratio.
- **Migration exactness comes from the two shared defaults**: `blend` ships
  at 1.0 (fully compressed = old behavior) and `sc_hpf` at its 20 Hz floor
  (detector reads raw, full-band). Both are what let the vca voice reproduce
  the pre-family sound sample-for-sample.

## Consequences

- **Engine, session, plugin: zero code changes.** The multi-pedal path
  (SelectPedal, per-pedal shadow memory, generic plugin param expansion via
  `from_families`) already covered a growing family; the session registry
  entry still builds `Compressor::new()`, now voice-aware. `default_active
  ("comp")` stays true — comp still ships engaged on the default board.
- **DEFAULT_CHAIN unchanged**: comp stays where it was (after gate/filter,
  before drive). No board reshuffle.
- **Cost** (Apple Silicon, 64-frame block): `comp_vca` ~0.56 µs, `comp_fet`
  ~0.56 µs, `comp_opto` ~1.23 µs (its program release + rising ratio spend a
  second `log10` per sample). Worst case 0.092 % of the deadline — under the
  PRD's 0.15 % bar. `comp_{vca,opto,fet}` benches added.
- **Theme**: opto wears warm optical-tube gold, fet a fast steel grey; vca
  keeps the family blue. Added to the distinct-livery pin (8 families now).
- Old presets: a `comp` slot loads as `vca` with its threshold/ratio/attack/
  release/makeup intact — audibly unchanged. Non-target compression tastes
  (opto/fet) are one pedal-switch away with their own knob memory.

## Alternatives considered

- **Grow the single VCA with a "mode" knob.** Rejected: the three topologies
  have genuinely different faceplates (opto has no attack/ratio; fet's ratio
  is stepped) — a shared knob row would misrepresent all three. The family
  model (each pedal owns its faceplate, ADR 004) fits exactly.
- **WDF / circuit-level opto & FET models.** Out of scope (PRD §3): this is
  behavioral hand-written DSP — topology *feel*, not SPICE.
