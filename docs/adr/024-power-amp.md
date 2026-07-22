# ADR 024: Power Amp — a hand-written valve power stage (on the board, bypassed)

Status: **accepted — implemented**
Date: 2026-07-22
Relates to: PRD 017, white paper §5.3 (NAM tone core), ADR 003 (drive registry
+ 4× oversample precedent), ADR 010 (filter family — `default_active` precedent),
ADR 005 (dynamic chain / `MAX_SLOTS`)

## Context

The NAM ecosystem's captures are overwhelmingly **preamp-only** — they model
the preamp's voicing but not the push-pull power section, so they lack the
sag, the touch-dependent compression, and the output-transformer thump that
make a real amp *feel* alive. Competing products sell exactly this layer
(GENOME's TSM power amp, Tonocracy's speaker compression). Supplying it in
hand-written DSP is squarely on Lion-Heart's "everything outside the tone core
is hand-written" thesis. This is the 7th item of the 2026-07-20 roadmap (M20),
user-picked as the next feature after the recorder.

## Decision

- **New single-pedal family `power`**, placed in `DEFAULT_CHAIN` **after `amp`
  and before `cab`** — post-preamp, pre-speaker, exactly where a real power
  section lives. The board grows **11 → 12 slots**.
- **Ships bypassed** (`default_active("power") == false`, the second family to
  do so after `filter`): a *full-amp* capture already contains a power stage,
  so an always-on second one would double-colour. Preamp-only players light the
  LED deliberately. One flag, shared by the app's `Session::start` board and the
  plugin's `power_active` default (the ADR 010 mechanism). The engine already
  **skips `process` on a settled-bypassed slot**, so shipping it on-but-bypassed
  costs **zero CPU and adds zero latency** until engaged.
- **`MAX_SLOTS` 12 → 16.** The default board now fills 12 of 12, which would
  leave no room to `add` a second instance or the off-board families (pitch,
  looper, acoustic). The engine's fixed-capacity arrays (`[u8; MAX_SLOTS]`,
  etc.) simply grow; no logic depends on the exact value (the capacity test is
  written relative to `MAX_SLOTS`).
- **Hand-written DSP, 4× oversampled** (the drive family's `Oversampler4x`),
  one file `lh-dsp/src/power.rs`. Per channel:
  1. **presence** high-shelf pre-emphasis into the clipper;
  2. **push-pull asymmetric waveshaper** at 4×, `y = supply·(tanh(g·x/supply +
     BIAS) − tanh(BIAS))` — the fixed `BIAS` skews the class-AB transfer curve
     for **even harmonics**; dividing the drive by `supply` and multiplying the
     output by it is the **sag** (a drooping rail clips earlier *and* lowers the
     ceiling → dynamic compression + "give");
  3. **output transformer** — one-pole low-cut (≈ 35 Hz, doubles as the DC
     blocker for the shaper's asymmetric offset) + gentle `tanh` core softness;
  4. **depth** low-shelf resonance + **master** (the shared `drive_law` level).
- **One linked sag detector** drives both channels (a push-pull amp has one
  shared supply — the gate/comp linked-detector precedent). Stereo otherwise
  runs two independent filter/oversampler states.

### Deviations from PRD 017 (flagged per CLAUDE.md)

1. **No preset schema bump.** A new family is new vocabulary (like `filter`,
   `pitch`): old presets don't reference `power` and reconcile the absent slot
   away. (The PRD didn't ask for a bump; noting it is correct. Aside: PRD 015
   penciled schema v7 for `comp`, but v7 was since taken by the dual-IR cab —
   that reconciliation is `comp`'s to make, not this feature's.)
2. **`depth` is post-waveshaper, not pre.** The PRD sketch put presence *and*
   depth in the pre-shaping stage. Presence (a negative-feedback high-end lift)
   belongs before the clipper — pre-emphasis makes the top break up, the
   authentic behaviour. But depth/resonance is a **power-tube/transformer/
   speaker low-frequency resonance** that physically sits *after* the tubes, and
   pushing bass *into* a clipper only smears it. So presence is pre, depth is
   post.
3. **presence/depth are boost-only `0..10` knobs** (0 = flat, 10 = max boost),
   mirroring a real amp's front panel, not the PRD's bipolar `±dB`.

## Consequences

- **~12.5 µs / 64-frame block** driven into saturation (≈ 0.94 % of the 1333 µs
  deadline) — on par with the drive family's 4× pedals (ts9 ~11.5, monster5150
  ~12.5 µs; sandbox x86, the Apple-Silicon figures in `benchmarks.md` are
  lower). `power_4x_oversampled` bench added. Zero cost while bypassed.
- **~24-sample latency when engaged** (the shared oversampler's round trip,
  identical to any drive pedal); none while bypassed. Not separately reported to
  hosts, matching the existing drive/NAM behaviour.
- **RT-safe**: all scratch allocated in `prepare`; `process`/`reset`/`set_param`
  allocate nothing. `tanh` is bounded and `supply` is clamped `[0.4, 1.0]`, so
  the stage cannot run away or emit NaN even slammed (asserted).
- **Plugin: pre-v0.1 additive id change** — `power_drive`/`power_sag`/
  `power_presence`/`power_depth`/`power_master` + `power_active` (default off)
  appear; no renames. Re-run clap-validator. The fixed chain matches the new
  `DEFAULT_CHAIN` (pinned by test); the plugin drives it like any other slot.
- **Tested offline** (`power.rs`): drive adds harmonics (cranked ≫ clean), the
  bias makes a strong 2nd harmonic, sag compresses a loud note more than a quiet
  one (isolated against clipping-alone), presence/depth shelves shape the
  spectrum, 4× oversampling keeps the alias floor down, DC is blocked, output
  bounded/finite when slammed, multi-rate/blocksize, defaults near unity, knob
  sweeps click-free, ships bypassed. Ear verification (preamp-only NAM coming to
  life; full-amp capture confirmed neutral by default) is on the Mac.
- **v1 fixes one push-pull voice.** Tube-type selection (EL34/6L6/KT88),
  rectifier-sag flavours, and a WDF circuit model are explicitly out of scope —
  behavioural feel, not a circuit. A tube-type `VoiceDef`/`Ctl` family (the
  drive/delay pattern) is the natural v2.
