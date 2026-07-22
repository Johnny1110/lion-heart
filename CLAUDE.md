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
2c. **jan-ray drive pedal** (Vemuram Jan Ray — a lightly-modified Paul
   Cochrane *Timmy*): op-amp soft clip with **two 1N4148 in series each
   way** (~1.2 V knee → clips late and stays dynamic/uncompressed, the
   reputed '63 Fender Deluxe chime). Tall, mildly-asymmetric knees
   (0.95/0.75) give the internal bias trim's gentle even harmonic (pinned
   below the evva's strong 2nd); **full lows** — only a 70 Hz subsonic trim
   sits ahead of the clip (vs the ts9's 720 Hz scoop, so a low note breaks
   up with the mids: amp-in-a-box, tested as a lower mid/low distortion
   ratio than the ts9); a fixed bright pre-emphasis (+~2.6 dB above 2.2 kHz)
   is the sparkle; +2..+30 dB low/medium gain (transparent boost → medium
   breakup, dynamic-at-low-gain test). Fender-style **four-knob** face
   (Gain/Bass/Treble/Volume — no mid): a custom two-band tone (120 Hz bass
   low-shelf + a 2.8 kHz treble high-shelf, the Jan Ray's own darker corner
   vs the Timmy's ~10 kHz), the Timmy's pre-clip bass idealized as the fixed
   trim + a post-clip shelf. Livery: warm bronze. `DRIVE_PEDALS` is now 9
   (append-only: angry-charlie 8, jan-ray 9); plugin params expand
   automatically (`drive_jan-ray_*` appear + the `drive_pedal` selector
   gains a step — additive, re-run clap-validator). ~10 µs per block.
2d. **fuzz-face drive pedal** (Dallas Arbiter Fuzz Face — germanium): nothing
   like the op-amp/diode pedals — a two-transistor feedback-pair fuzz.
   Three signatures modelled: (1) **asymmetric clip** — soft `tanh` up, a
   *hard flat clamp* down (KNEE 0.9/0.5), plus a **pre-gain** bias offset
   (Q1 ≈ 1.3 V, off mid-supply) that shifts the clip's zero-crossing so the
   flat-topped square keeps an asymmetric *duty cycle* → strong even
   harmonics that survive the DC blocker (a post-gain offset would just be a
   DC level the blocker erases → symmetric square, no evens — a bug caught in
   testing); (2) **gated/spluttering decay** — the blocking-distortion
   cutoff, reproduced as a **ratio gate**: a fast input envelope vs a slow
   (~0.6 s) peak-hold, gate shuts when env falls below ¼ of the recent peak,
   so a fading note gates but a merely-quiet *steady* one never does; (3)
   **cleans up from the input** — inherent to the very high gain (+20..+55 dB,
   no clean floor) and left alone by the ratio gate. Dark/thick voicing
   (5.5 kHz pre-clip high-cut = the woolly germanium top); **two knobs** Fuzz
   / Volume (family's smallest face, `controls [Drive, Level]`). Character
   tests: gates-the-decay (tail/body ≪ the ts9's), strongly-asymmetric
   (h2 ≫ ts9), no-clean-floor, cleans-up-at-low-input. Livery: Dallas Arbiter
   turquoise. `DRIVE_PEDALS` is now 10; plugin ids `drive_fuzz-face_*` appear
   (additive, re-run clap-validator). ~11 µs per block.
2e. **Drive-stacking low-end fix.** Reported: a Centaur (drive 10 %, output
   100 %) boosting an Angry Charlie booms/farts on the lows. Diagnosis — the
   engine cascades slots raw (no coupling-cap high-pass between stages, unlike
   real gear), and the two loosest links were stacked: the Centaur's clean
   path was a *flat, full-range* boost, and the Angry Charlie / Fuzz Face pass
   full lows into a hard clip. Measured with a 112+176 Hz low chord's 48 Hz
   odd-order intermod (2·112−176, absent from the input = pure clipping boom):
   stacking lifted it ~25 % over the Angry Charlie alone. Fixes (voicing, all
   above ~130 Hz so single-pedal tone is untouched — the 220 Hz character
   tests never move): Centaur clean path gained a ~−6 dB low-shelf below
   130 Hz (the real Klon is a **mid-forward** boost that tightens lows, not a
   flat lift); Angry Charlie's subsonic HP 25→50 Hz and the Fuzz Face's 20→50
   Hz (shed sub-bass flab, keep the body). After: the stacked 48 Hz intermod
   sits *below* solo (no added boom). Pinned by `centaur_boost_is_mid_forward`
   (80 vs 800 Hz tilt) and the integration guard `drive_stacking_stays_tight`.
   Most of the family already tightens pre-gain (TS9 720 Hz, red-charlie /
   monster5150 carve, jan-ray 70 Hz) — this brings the deliberate full-range
   outliers into line for stacking without gutting their solo voice.
3. **GUI v2.** Header = view tabs (board · tuner · eq · live) with
   settings set apart top-right; a **persistent preset bar** (◀ picker ▶,
   save-as field — replaces the presets overlay, rescans the dir ~1 Hz);
   window 960×640. Bug fixed: clicking/dragging a chain card now always
   returns to the board view with that slot's faceplate open
   (`select_position` sets `View::Board`) — it used to do nothing while
   tuner/eq/live/settings was up.
3a. **Recoverable no-audio startup.** `App::Failed` (dead end when the
   saved interface is unplugged) became `App::Setup`: the window opens
   anyway with the failure reason, the shared device/channel/buffer picker
   (`draft_controls`, reused by the settings panel), "start audio", and
   "rescan devices" (auto-rescan after a failed try). Unresolvable saved
   specs preselect *system default* — recovery is one click. A recovery
   start deliberately does **not** write config.json: the usual interface
   wins again next launch; persistent changes stay in the settings panel.
   Both-configs-lost during a settings apply lands here too (the active
   preset reload rebuilds the board). `lh_io::devices::resolve_name` maps
   a `select` spec onto an `enumerate` snapshot (same index/exact/substring
   semantics, unit-tested) so the UI can preselect what a spec refers to.
3b. **GUI v3 — the "Backline" redesign** (view layer only; no engine or
   Message changes beyond additive knob interactions). Design system in
   `gui/theme.rs`: warm-charcoal palette, one tube-amber accent, and a
   **signature color per pedal** (TS9 green, BD-2 blue, Centaur gold, evva
   crimson-pink, red-charlie crimson, monster5150 ivory, angry-charlie
   scarlet, jan-ray bronze, fuzz-face turquoise, delay voices
   digital/tape/vintage) that runs through the chain
   card stripe, the faceplate identity rule, and the knob arcs — a theme
   test pins that every selectable pedal has a distinct livery. Chain cards
   have LED bypass dots and signal-flow chevrons; knobs grew a tick ring,
   glow arc, and recessed readout, plus **wheel = nudge, double-click =
   default**; meters are three-zone (green/amber/red) with peak-hold;
   asset rows render as inset "LCD" wells; header tabs are one segmented
   group; footer xruns only turn red when nonzero; window 1120×700.
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

**M10 (reverb family) — code landed (uncommitted).** Specced in PRD 005,
recorded as ADR 008. The `reverb` slot became a **twelve-machine family**
(BigSky-inspired: `hall`/`room`/`plate`/`spring`/`swell`/`bloom`/`cloud`/
`chorale`/`shimmer`/`magneto`/`nonlinear`/`reflections`, family key
unchanged, hall first = the M5 FDN voicing and migration target), one shared
engine in `time/reverb/` (one file per pedal, like drive/delay). The engine
is the M5 Householder tank grown structural switches per `VoiceDef`:
interpolated line reads (`size` scale — hall's noon is exactly 1.0 — and a
`mod` LFO distributed over the 8 lines by phase rotation, 1 sin_cos/sample),
a `Kind` enum (Tank / Magneto multi-head echo→tank / feedback-free burst for
nonlinear's gate-reverse-swoosh window and reflections' 3 ER tap tables),
and in-tank inserts (shimmer's soft-clipped granular pitch loop — bounded
drone, never runaway; chorale's out-of-loop vowel bandpasses; spring's
chirp-allpass banks + dwell soft clip; swell's onset-retriggered rise ramp;
bloom's ≤0.85-gain regen diffusion loop). Shared knobs Decay/Predelay/Mix/
Tone(Hz, the v4 key)/Mod + ≤2 signature knobs each; `lowend` is loss-only
in-loop below neutral, input-shelf boost above (stability by construction).
`blocks::biquad` gained RBJ bandpass + 2nd-order allpass. Preset **schema
v5** (`migrate_v4_reverb_pedal` renames `reverb`→`hall`, values verbatim,
`REVERB_PEDALS` pins the registry; sparse slots land on hall). Engine/
session/plugin: **zero code changes**; theme gained 12 reverb liveries
(hall = sky blue) under the pinned distinct-livery test. 27 new tests
(hall-keeps-M5 suite, per-voice character probes — tail-based, not
steady-state single tones, which read FDN comb interference — and
family-wide invariants incl. an every-knob-sweep fuzz). 1.97–4.43 µs per
voice per 64-frame block (worst 0.33 % of deadline; hall ~3.7× the old
fixed-read FDN — accepted, see benchmarks.md). Plugin param ids expand
(`reverb_hall_decay`, …, `reverb_pedal`) — **pre-v0.1 break**, re-run
clap-validator.

**M11 (mod family expansion) — code landed (uncommitted).** Specced in
PRD 006, recorded as ADR 009. The "tremolo is barely audible" report traced
to three design causes: a hardwired half-cycle stereo offset (auto-pan whose
L+R cancels in a room), a linear-amplitude depth law (−6 dB at noon, −2.5 dB
after the v2 fold), sine-only. Tremolo rebuilt: **dB-linear depth**
(`exp(−60 dB·depth·w)`, peaks pinned at unity), `wave`
(sine/triangle/chop, gain slewed ~1.2 ms — declicked chop, depth-0
bit-exact), `spread` knob (R-phase 0..180°, **default 0 = in phase**; full
spread is hard ping-pong — the dB law's convexity means L+R is not
conserved, tested as envelope anti-correlation). Old presets get audibly
stronger tremolo — that is the request. **Four pedals appended** (family
now 8; `MOD_PEDALS` still pins only the v2 first four): **vibrato** (wet
swept-delay pitch, L==R coherent by test), **harmonic** (700 Hz
complementary split, counter-phase band gains, <1e-7 depth returns input
verbatim — float re-sum isn't bit-exact), **rotary** (800 Hz split; horn
and drum rotors with own doppler/AM/pan and own inertia — horn 0.9 s, drum
3.2 s `Smoothed` rate targets; select_pedal starts rotors at slow so a fast
preset arrival spins up; equal-power balance), **univibe** (staggered
allpass corners 78/210/620/1750 Hz, lamp-skewed LFO, fixed 50/50 blend,
pinned ≠ phaser). Param positions route through a per-pedal `Ctl` table
(ADR 008 pattern) — no schema bump (append-only params/pedals). 8 mod
liveries in the theme. 0.77–2.85 µs per block (univibe's four per-sample
`tan`s = 0.21 % deadline, cache rejected). Plugin ids: tremolo
wave/spread + four pedals' params appear — **pre-v0.1 break**, re-run
clap-validator.

**M12 (filter family: autowah) — code landed (uncommitted).** Specced in
PRD 007, recorded as ADR 010. New chain family `filter` (key broader than
"wah" for growth: LFO wah/S&H/formant later), first pedal **autowah**:
asymmetric envelope follower (2 ms attack, `decay`-knob release 60–600 ms,
`sens` +30 dB max pre-gain, mono-summed — both channels share one sweep, a
quack is one event) → geometric sweep 180 Hz–2.4 kHz (`direction` flips) →
per-channel Chamberlin SVF (`mode` LP/BP/HP free from one structure; band
state tanh-clipped per sample so Q 12 saturates instead of diverging).
**DEFAULT_CHAIN is now 11 slots** (cap 12): `gate → filter → comp → …` —
filter sits before comp because the follower eats the dynamics comp
removes. New shared flag **`lh_core::default_active(key)`**: filters have
no transparent setting, so the slot ships **bypassed** on the app default
board (Session::start) and the plugin's `Active` param defaults off — one
flag, pinned by the plugin chain test. No preset schema bump (new family =
vocabulary; old presets reconcile the slot away). Chamberlin BP is
constant-skirt — Q shows at the peak (≈Q), not the skirts; the character
test probes resonance with a small signal below the soft-clip knee.
~1.23 µs/block (0.09 %). Registry/plugin grew one entry each; theme adds
the acid-lime `filter` family color. Plugin ids: `filter_*` appear,
`filter_active` defaults off — **pre-v0.1 break**, re-run clap-validator.

**M13 (expression pipeline) — code landed (uncommitted).** Specced in
PRD 008, recorded as ADR 011; closes white-paper M6's "CC 映射、expression
（wah/volume）" leftovers. Four pieces, **engine untouched** (everything is
control-side or a new pedal): (1) **manual `wah`** joins the filter family
(now 2 pedals; `filter.rs` → `filter/` with delay-style per-pedal `Ctl`
tables — autowah's math unchanged, ~1.20 µs): smoothed `pos` (25 ms —
absorbs 7-bit CC staircases, pinned by a declick test) sweeps 350 Hz–
2.2 kHz geometrically into the shared soft-clipped SVF; faceplate
pos/q/mode/mix (q default 6, lowpass, full wet), **no direction knob** —
a reversed pedal is the mapping's job; theme: wah wears chrome, the filter
family joined the distinct-livery test; plugin auto-expands `filter_wah_*`
(**pre-v0.1 id addition**, re-run clap-validator); ~1.15 µs. (2) **CC
shaping** (lh-midi): a `cc` entry is now the legacy string *or*
`{target, min, max, curve, pickup}` (serde-untagged — old `midi.json`
parses unchanged; min > max inverts; curve `linear`|`audio` = x²).
(3) **Soft-takeover** in the session: per-controller `PickupState`,
desynced by preset load / that slot's pedal switch / manual (GUI or REPL)
moves of the same param; re-engages when the shaped CC crosses the current
value (`ChainHandle::param_norm` reads the shadow) or lands within ±0.02;
silent until then. (4) **MIDI learn**: right-click a knob arms it (toggle
to cancel), the next on-channel CC binds in the string form and persists
to `midi.json` (input/channel/pc_presets preserved, displaced target
reported); bound knobs wear an amber CC badge, learning shows a "?" ring
plus a panel banner (cancel / clear CC n); REPL grew
`learn`/`unlearn <slot.param>`. No preset schema bump. Learn/pickup are
deliberately absent from the plugin — host automation is that answer.

**M13 (snapshots / scenes) — code landed (uncommitted).** Specced in
PRD 009, recorded as ADR 012; lands the white-paper M8+ "snapshot morphing"
item. Up to **four scenes (A–D) per preset**, each a value+bypass overlay
on the *one* board — never structure or pedal selection (two drive tones =
two drive slots). **Engine untouched again**: switching diffs current-vs-
target and emits the existing `SetParam`/`SetActive`, so delay/reverb tails
ring straight through a scene change and every move is declicked by the
effect's own smoother. **Morph** (`morph_ms` in config.json, 0–2000,
default 0): above 0 the session interpolates each changed param's
*normalized* value over the window on the control-loop tick (`tick_morph`),
a scene change becomes an audible sweep; norm-space keeps log ranges
musical; `active` flips at morph start (engine crossfades it). The morph
math (`Morph::build`/`at` — drop no-ops, lerp, clamp) is pure and unit-
tested; `ChainHandle::capture_scene()` reads the active pedal's shadow into
a `lh_core::preset::Snapshot`. **Schema v6**: `Preset` gains
`snapshots: BTreeMap<"A".."D", Snapshot>` + `active_snapshot`, both
`#[serde(default)]` — a v5 file is a v6 with no scenes (loads identical);
version bumped so an old build rejects a scene-bearing file rather than
silently dropping scenes. Scenes ride a device-restart via `CarryOver`.
GUI: four A–D chips in the preset bar (click a populated one switches /
morphs, an empty one captures; ⤓ re-captures the active; active glows,
populated solid / empty dim, `*` = drifted-unsaved). MIDI: virtual
`snapshot.select` target (CC value quartered → A–D, switch-on-change only);
REPL `snapshot <A-D>` / `snapshot save <A-D>` / `morph <ms>`. A switch
`midi_desync_all`s (pickup re-engages, PRD 008). **Plugin has no scenes in
v1** — host automation lanes are the DAW's scene mechanism (ADR 012).

**M13 (spillover) — code landed (uncommitted).** Specced in PRD 010,
recorded as ADR 013; lands white-paper M6's stretch "delay/reverb
spillover". First change to the **RT lifecycle** since M5. The engine grew
**4 spill lanes** (`SPILL_LANES`, preallocated), processed after the master
fade and before the output stage — so a structure-change fade can't mute a
tail and the safety limiter/EQ still cover the sum. New
**`EngineMsg::SpillSlot { index }`** takes the slot **immediately** (pointer
move into a free lane, no alloc; the main loop skips the emptied slot while
its index still sits in the order) and the lane rings the tail out on
silence. Exits: output < −80 dBFS for 250 ms retires down the existing
chute; after an 8 s grace the lane force-decays at −12 dB/s (bounded-drone
insurance for a self-oscillating delay; gain floored to 0 below 1e-7 for
denormals); a spill into full lanes evicts the oldest (hard cut). New
**`Effect::tail_seconds()`** (default 0; delay 8, reverb 12) is a static
hint cached in `SlotShadow.tail_secs` so the control side chooses
spill-vs-remove without touching the audio effect. `apply_preset_chain`
took a `spillover: bool`: pass-1 refuses to *claim* a tailed survivor (no
delay-time-glide artifact) and pass-2 **spills** unclaimed tailed slots
(fresh instances built in pass-3), else removes. `ChainHandle::spill_slot`
mirrors `remove_slot`; the session's `remove_slot` and preset reconcile pick
spill when `config.spillover && tail`. **`AppConfig.spillover`** (config.json,
default **on**, manual `Default` impl so a fresh config and an absent field
agree); REPL `spillover on|off`. ~7.6 µs / block for 4 hall lanes (0.57 %;
worst voice ×4 ≈ 18 µs) — `spillover_worst` bench in a new lh-engine
criterion harness. Engine tail suite (rings-then-evicts, hard-cut contrast,
forced-decay cap, lane exhaustion) uses a deterministic feedback-resonator
test effect. **Plugin unchanged** — hosts own their tail handling.

**Global tempo & note-division sync — code landed (uncommitted).** Recorded as
**ADR 014 (sync target) + ADR 018 (BPM source)**, specced in PRD 012. Two
parallel designs (one per machine) were reconciled at merge time (2026-07-21)
into one combined feature; the two ADRs carry matching merge-reconciliation
notes. **BPM source (ADR 018):** one app-global BPM (`AppConfig.tempo_bpm`,
default 120, clamp 30–300, config.json — environment, not preset), fed by three
sources into a session-owned **`TempoState`** (tap history + MIDI-clock
accumulators): tap (footer **♩ BPM chip**, REPL `tap`/`tempo`), MIDI clock, and
(plugin) host transport. `lh-midi` parses realtime bytes (`Ignore::None` — midir
filters them by default): `MidiEvent::Clock { stamp_us }` / `Start` / `Stop`,
`stamp_us` the driver's own timestamp (tick *intervals* carry the tempo). The
session takes the **median** of the last 48 tick intervals (mean would let one
USB hiccup bend it) with a plausibility gate (4–120 ms/tick, restarts on a gap)
and a <0.5% hysteresis so a steady clock does not repaint the bar or requeue
chain messages; tap/typed tempo persists, a MIDI-clock tempo is applied live but
not persisted. **Sync target (ADR 014):** a per-pedal stepped **`sync`** param
(`Free · 1/1 · 1/2 · 1/4. · 1/4 · 1/8. · 1/8T · 1/8 · 1/16` from
`lh_core::tempo::SYNC_DIVISIONS`, default Free) on the 3 delay voices + tremolo —
append-only, **no preset schema bump** (old files lack it → Free → identical),
rides presets/MIDI/plugin for free, a **control-side no-op** in the DSP like
delay's `subdivision` (`Ctl::Sync`). Pure math in `lh_core::tempo`
(`synced_time_ms`/`synced_rate_hz`). **`ChainHandle::apply_tempo_sync(bpm) ->
bool`** (lh-engine) locks a slot's `time` (delay) or `rate` (tremolo) when its
`sync` ≠ Free via the normal `set_param` smoother; idempotent, returns whether
anything moved. `Session::tick_tempo` delegates each control tick after
`tick_morph`; `apply_tempo_now` is the non-persisting apply the MIDI-clock path
shares. Per-slot **tap + `subdivision` stay** — a *Free* delay's TAP still sets
its own `time` from tap × subdivision. GUI: the `sync` division renders as a
stepped dropdown in the selector row (not a boolean chip); the footer ♩ BPM
chip is the always-in-view tap, beside a **typed-BPM field** (`set` / Enter →
`set_tempo_bpm`, digits-only draft) for exact tempi — the tap chip was
tap-only before. MIDI: virtual `tempo.tap` target (`SetParam` gated on
`norm >= 0.5` — the press, PRD 008's `snapshot.select` pattern). **Plugin:**
`apply_tempo_sync` runs once per block; while the active delay pedal's `sync` ≠
Free, `context.transport().tempo` (via `lh_core::tempo::synced_time_ms`, split
out and unit-tested) overrides `time` and the host's own `time` automation is
ignored until `sync` returns to Free (the host param is never touched, so no
restore logic). Plugin ids gain `delay_{digital,tape,vintage}_sync` +
`mod_tremolo_sync` (**pre-v0.1 additive break** — no renames; re-run
clap-validator); the plugin drives delay `time` only (tremolo `rate` sync is
standalone-only for now). **Zero DSP/RT cost** (no audio-thread change). Engine
test `apply_tempo_sync` locks delay time + tremolo rate; lh-core math tests;
session tap/clock median tests.

**Dual-IR cab / mic blend — code landed (uncommitted).** Recorded as
ADR 015; deepens the NAM+IR tone core. The cab convolves a **primary IR `a`
plus an optional blend IR `b`** (a second mic) with a new **`blend` knob**
(0 = all A, 1 = all B) — a **linear** crossfade (the two mics are correlated,
so level stays put while the top-end/comb difference sweeps; identical mics
sum to unity). `IrAsset` grew `{ a: IrPair, b: Option<IrPair> }`; the blend +
level trajectories are snapshotted once per block and shared L/R. **One asset
handle** kept (`CabIr::new()` unchanged — the family-builder signature is
shared by all 11 families): the control side owns both files and composes the
combined asset — `lh_assets::load_ir_pair` decodes one IR; the session's
`rebuild_cab` reloads whichever of `ir_ref`/`ir_b_ref` are set and installs
them together (changing one re-decodes both — cheap/rare). Preset **schema v7**
(`assets.ir_b`, `#[serde(default)]`; a v6 file is a single-mic v7, sounds
identical; bumped so an old build rejects a dual-IR preset); `ir_b` rides
`CarryOver`. Surfaces: REPL `load ir_b`/`unload ir_b`; GUI cab faceplate shows
**MIC A**/**MIC B** rows (new `AssetKind::IrB` routes browser/unload) around
the auto-rendered `blend` knob; blend IR needs a primary; unloading the
primary clears both. Plugin loads both from a preset + exposes `cab_blend`
(**pre-v0.1 additive break, re-run clap-validator**); it can't load a blend IR
interactively (assets come from presets there). ~2× cab CPU only when a blend
IR is loaded (~7 µs, 0.5 %); single-mic cabs pay nothing. RT-safe (all scratch
in `prepare`). Tests: `blend_crossfades_between_the_two_irs`,
`blend_is_inert_without_a_second_ir`, preset `ir_b` round-trip.

**Pitch family (octaver) — code landed (uncommitted).** Recorded as ADR 016;
fills the one missing effect category (octave/pitch). New chain family
**`pitch`** (key broad for a future harmonizer/whammy/detuner), first pedal
**`octaver`**: a POG-style granular doubler — a stereo **Dry** path mixed with a
**Sub** (−1 oct) and **Oct** (+1 oct) voice taken off the *mono sum* (fat,
centered octaves), a shared **Tone** lowpass (600 Hz–9 kHz) taming the granular
fizz. Built on a new shared primitive **`blocks::grain::GrainShift`** (the
no-FFT two-tap "doppler" shifter promoted from the shimmer reverb's math;
reverb keeps its private copy — dedup deferred to a health pass to avoid
pre-v0.1 churn). Feed-forward, so **no tail** (`tail_seconds()` 0, no spill) and
bounded by construction. **Opt-in, not on the default board**: `pitch` is
registered in `FAMILY_REGISTRY` (so ＋ menu / REPL `add pitch` reach it) but
**absent from `DEFAULT_CHAIN`**, so the board stays 11/12 with a free slot. This
decoupled the registry from `DEFAULT_CHAIN` for the first time — the default
board is now an in-order **subsequence** of the registry (pinning test relaxed
accordingly; the standalone-only `looper` is the other off-board family).
**No preset schema bump** (new family =
vocabulary; old presets reconcile the absent slot away). Livery: orchid magenta,
joined the distinct-livery test. Faceplate auto-renders (generic knobs); GUI/
session/engine needed no per-family code beyond the registry entry + theme
color. ~1.05 µs/block (0.08 %; `pitch_octaver` bench). Granular ⇒ polyphonic
but warbly — *not* an OC-2 mono divider (accepted, ADR 016). **Plugin
unchanged**: its fixed chain is `DEFAULT_CHAIN`, which did not move, so no new
param ids and clap-validator is unaffected — plugin pitch is a follow-up. 12
new tests (octave up/down land on the Goertzel bin, dry-path transparent,
Tone darkens the up-voice, bounded/finite fuzz, silence→silence, plus
`GrainShift`'s own).

**Parametric EQ pedal — code landed (uncommitted).** Specced in
PRD 011, recorded as **ADR 017**. First item of the 2026-07-20 nine-feature
roadmap (user-picked: recorder/re-amp, looper, global tempo sync, comp
family, power-amp sim, practice tools, setlists+leveling, pitch family,
this). The `eq` slot became a **two-pedal family** (3-band keeps key
`eq`; `parametric` appended — no schema bump). The parametric **is**
`GlobalEq` reused as a core behind a 40-param façade (`b1_on`..`b8_q`,
kinds pinned to `BandKind::ALL`, defaults = the global layout, all-off =
bit-transparent fast path); slot bypass stays the engine crossfade (core
master pinned 1.0). The 3-band demoted to `eq::chain::Tone` (inherent
methods); **`eq::Eq` is now the family dispatcher** — session/plugin
imports unchanged; engine/session/preset needed zero code. GUI: `EqPanel`
gained `EqTarget::{Global, Slot}` — the parametric faceplate on the board
renders the same canvas + detail strip (type/on-off/readout/flat), edits
diff-and-send through the slot param path (shadow-live, MIDI-learnable via
REPL `learn eq.b3_freq`, scene-morphable), spectrum overlay = the output
tap tagged "OUT" (no per-slot taps); the FFT also runs while a parametric
faceplate is showing. Livery: analyzer ice; eq joined the distinct-livery
pin. Plugin: **pre-v0.1 id break** — eq now qualifies ids
with the pedal key (`eq_low` → `eq_eq_low`), `eq_parametric_*` +
`eq_pedal` appear; re-run clap-validator. Bench `eq_parametric_4band` ≈
`global_eq_4band` (same-box parity ~1.45 µs — settled-skip inherited).

**Looper — code landed (uncommitted).** Specced in PRD 013, recorded as
**ADR 019**. First item of the 2026-07-20 nine-feature roadmap after tempo. New
single-pedal family `looper` — an **add-only family**: buildable +
offered in the "＋" menu but **not** in `DEFAULT_CHAIN` and **not** in the
plugin (standalone-only, ADR 013's reasoning). The registry↔default-chain
test was relaxed from "equal" to "default chain is an in-order **subsequence**
of the registry; the rest are add-only" (`pitch` ships off-board too).
**Engine and session message set untouched**: transport
(`rec`/`undo`/`clear`) are momentary linear params, rising-edge-through-0.5 in
`set_param` (the `tempo.tap` idiom); the GUI/REPL/MIDI fire a **1.0→0.0
pulse** (the FIFO ring keeps both edges; the shadow settles at 0 so a preset
never stores a held button → no re-trigger on load). One-button state machine
`Empty→Recording→Playing→Overdubbing→Playing…`. Two 60 s stereo banks
preallocated in `prepare` (~46 MB @ 48 k, ~92 MB @ 96 k; the alloc rides
`install_slot` on the control thread). `clear`/`reset` are **logical** (reset
`loop_len`, never `memset` the multi-MB banks on the RT thread — reads stay in
`[0,loop_len)`, recording overwrites from 0). **One-level undo/redo = a
bank-index swap** (no audio-thread copy); the undo snapshot is filled by
copy-*before*-sum during an overdub's first pass, valid once that pass
completes; undo gated to `Playing`. Overdub sums in place with a `tanh` soft
clip (bounded infinite stacking). Playback is a **single interpolated tap +
smoothstep boundary fade** (~6 ms dip at the wrap kills seam clicks without
the two-grain smear a single stored loop would suffer); `reverse`/`half` are
`Playing`-only read modifiers, record/overdub run forward at an integer head.
Faceplate: REC/UNDO/CLEAR buttons + reverse/half chips + level/mix knobs + a
state LED (red/green/amber) driven by a **control-side session mirror** (the
effect's state isn't tapped out of the engine; the mirror advances on the same
presses the session forwards — best-effort, only ever mistints an LED).
Livery: orchid/magenta. REPL `looper <slot> rec|undo|clear`. **No preset
schema bump** (new vocabulary; transport stored as 0), **no plugin id change**.
15 DSP tests (state machine, undo/redo swap, clear, reverse/half read,
overdub soft-clip bound, seam delta bound, mix-0 bit-exact, 60 s @ 96 k cap,
multi-rate). ~0.65–1.03 µs/block record/play/overdub (`looper_*` benches,
≤0.08 % deadline). v2: tempo-quantized length, multi-undo, loop→WAV export.

**Practice tools — metronome (Phase 1) — code landed (uncommitted).** Specced
in PRD 019 (three phases: metronome → drum groove → song player), recorded as
**ADR 020**. Phase 1 = the metronome **plus the shared aux-monitor foundation**
the later phases reuse. First **monitor/aux path** in the engine: the `Chain`
grew an optional interleaved-stereo **aux input ring**
(`Chain::set_aux_input`), drained and summed onto the bus **after** the output
stage (global EQ → safety limiter → spectrum tap) and before `peak_out` — so
the click bypasses the amp tone and is *not* limited by the guitar's safety
limiter (a monitor mix, not tone), the **spectrum tap stays guitar-only**
(read pre-aux), and `peak_out` reflects the true device sum. Drop-on-empty
(underrun = a brief gap, never a stall); bit-transparent with the click off
(the drain early-returns on an empty ring). **Synthesis is off the audio
thread**: a dedicated `lh-aux-player` thread renders the metronome into the
ring keeping ~50 ms buffered, so the audio callback only does a lock-free ring
read + a per-sample stereo add — `assert_no_alloc`-clean (validated by a
null-device jam with the click on, exit 0). The metronome is
`lh_dsp::practice::Metronome` — a **generator, not an `Effect`** (new
`practice/` module, the future home of the drum player + WSOLA): a mono
enveloped-sine click (1500 Hz accent / 1000 Hz beat, ~50 ms), beat-1 accent,
`beats_per_bar` (1–16), volume, `restart`/count-in, driven by an internal
sample clock so it is pure/offline-testable and cannot drift vs the audio it
mixes into. **BPM follows the global tempo** (ADR 014) — the session pushes
`config.tempo_bpm` into the shared control state on every tempo move, so
tapping the footer BPM chip re-times the click. Cross-thread control is an
`Arc<MetronomeShared>` of atomics (enabled/bpm/vol/beats/accent/restart-gen/
run flag, `Relaxed`); the player thread is **joined on `Session` drop**, and
the metronome's runtime state rides a device restart via `CarryOver` (BPM
re-reads from the persisted config). **App-global environment, not tone** — no
preset schema bump, not in presets; **plugin unchanged** (hosts own their
metronome, the spillover/ADR 013 precedent). Surfaces: footer `click` toggle
(lit amber = running) + a **click-level slider** (0–100 %, beside the chip) + a
stepped time-sig chip + the shared BPM chip (tap **or** a typed-BPM field); REPL
`metronome on|off` / `click <0-100>` / `timesig <n>` / `countin`. Click level
kept moderate (default accent ≈ −9 dBFS) since the aux sums post-limiter — a
coincident full-scale guitar+click peak can clip (accepted for a practice
monitor; a summed-output brickwall is the noted future mitigation). ~218 ns
worst-case click block (`metronome_click` bench, **off the RT budget** — player
thread); the aux sum is below a bench's noise floor. 7 metronome DSP tests
(beat phase/accent/meter/volume/count-in restart/tempo tracking/multi-rate
finite) + 4 engine aux tests (empty = bit-transparent, stereo sum, underrun
gap, tap excludes aux). **Phase 3 (song player: `symphonia` decode, A-B loop,
WSOLA varispeed, ±semitone) is deferred** — it renders into the same aux ring
from the same player thread.

**Practice tools — drum groove (Phase 2) — code landed (uncommitted).** Specced
in PRD 019 Phase 2, recorded as **ADR 021**. A **procedural drum machine**
(`lh_dsp::practice::DrumMachine`) that synthesises the beat **at the exact global
BPM** rather than stretching a sample — a deliberate deviation from the PRD's
sample+WSOLA sketch (ADR 021): can't ship quality drum-recording binaries, and
synth-at-target-tempo locks tighter than any stretched loop with zero assets. A
5-voice analog-style kit (kick = pitch-swept sine + beater click; snare = tone +
HP noise; closed/open hi-hat = HP noise; tom = pitch-swept sine), clocked off an
internal 16th-note counter like the metronome. **Deterministic** (seeded
xorshift noise → reproducible bars, the tests lean on it). 4 built-in patterns
(`rock`/`funk`/`metal`/`ballad`, append-only velocity tables on a 16-step grid)
+ a one-bar tom-roll **fill** armed on the next downbeat. **Reuses Phase 1's aux
lane + player thread**: the player now renders metronome *and* drums and sums
both into the aux ring (audio thread unchanged — still just reads + adds); both
track the one global tempo. A second `GrooveShared` atomic block (enabled/
pattern/volume/fill-gen/restart-gen) carries the control and rides a device
restart via `CarryOver`. **Standalone-only, app-global, no preset/plugin change**
(ADR 020's reasoning). **WSOLA deferred to Phase 3** (procedural drums need no
stretch; its real consumer is the song player). Surfaces: footer `drums` toggle
(lit amber = playing) + a pattern-cycle chip; REPL `groove <name>|on|off` /
`groove vol <n>` / `fill`. Same post-limiter placement as the click, so the
groove stays polite (conservative level). ~0.82 µs to render the busiest bar
(`drum_groove_funk` bench, **off the RT budget** — player thread);
`assert_no_alloc`-clean with drums + click both on (null-device jam, exit 0). 7
DrumMachine DSP tests (grid timing, pattern-energy differences, tempo scaling,
volume/silence, determinism, fill-adds-energy, bounded/finite multi-rate).

**Practice tools — song player (Phase 3) — code landed (uncommitted).** Specced
in PRD 019 Phase 3, recorded as **ADR 022** — completes the practice tools.
Load a backing track (WAV/MP3) and practice against it: slow a solo without
dropping pitch, transpose to your key, loop a hard bar. Three pieces:
(1) **`lh_dsp::practice::Wsola`** — a hand-written **WSOLA time-stretch**
(varispeed, pitch-preserving): overlap-add of Hann grains, each placed by a
normalised cross-correlation search so the pitch period survives (plain OLA
smears it); **stereo-linked** (offset chosen from L, applied to both).
(2) **`lh_dsp::practice::SongPlayer`** — pipeline `source → WSOLA(tempo) →
GrainShift(±semitone) → mix`, reusing the octaver's `blocks::grain::GrainShift`
(ADR 016) for transpose; two independent granular stages rather than one
combined stretch (each testable). A-B loop by cursor reset (seam, no crossfade
yet); stop at end with no loop. (3) **`symphonia` decode** in the **app crate
only** (kept out of the plugin) — a **background loader thread** decodes +
`lh_assets::resample_sinc`s to engine rate, hands an `Arc<SongBuffer>` to the
player thread; the RT path only reads the immutable buffer. It's a **third aux
source** on the existing player thread (renders stereo song alongside mono
metronome+drums, sums into the aux ring — audio thread unchanged, ADR 020). A
`SongShared` atomic block carries the transport (play/speed/semitones/mix/A-B/
seek) and publishes the play position back for the GUI; the song runs on its
**own transport**, not the global tempo. GUI: a dedicated **`song` view tab** —
draw-only waveform Canvas (peak envelope + playhead + shaded loop region), a
seek slider, A/B/clear-loop buttons, speed/transpose/level sliders; the file
browser gained `AssetKind::Song` (.wav/.mp3). REPL `song load|play|stop|speed|
pitch|mix|seek|loop`. **Standalone-only, no preset/plugin change** (symphonia
never enters the plugin build). **Not carried across a device restart** (large
buffer on the player thread — reload after a device change; metronome/drums are
carried, the song isn't). ~38 µs to render a 64-frame block at 75 % speed +2 st
(`song_player_stretch_shift` bench, **off the RT budget** — player thread, WSOLA
correlation is the cost; ~1.2 ms compute per ~3 ms fill, no underruns);
`assert_no_alloc`-clean with song+drums+click all mixing (null-device jam, exit
0). 14 tests: WSOLA (5 — pitch preserved at 0.5×/2×, neutral transparent, cursor
advance, bounded), SongPlayer (8 — plays/stopped/transpose-octave/half-speed-
keeps-pitch/A-B-loop/stop-at-end/mix/peaks), symphonia loader (1 — WAV round
trip). **All three practice-tools phases are complete** (metronome ADR 020,
drums ADR 021, song player ADR 022) — one aux lane, one player thread.

**M17 (recorder + re-amp) — code landed (uncommitted).** Specced in PRD 014,
recorded as **ADR 023**. First item of the 2026-07-20 roadmap not yet built;
lands white-paper success metric #1 ("record a guitar track"). Two features on
one offline base. (1) **Live DI + wet recording.** The engine grew **two
recording taps** (`Chain::set_record_taps`, same install pattern as the PRD 003
spectrum tap, no new `EngineMsg`): a **DI tap** at chain entry (raw input,
before any slot) and a **wet tap** after the output stage (global EQ → safety
limiter) but **before** the aux mix — so a take carries no metronome/backing
bleed (`wet_tap` is guitar-only, like the spectrum tap). Both are stereo
interleaved `StereoTap`s over `rtrb` producers, sharing an
`Arc<RecordTapState>` (`armed: AtomicBool` + `dropped: AtomicU64`): **disarmed =
one relaxed load/block** (bit-transparent, ring untouched), armed = interleave +
count the shortfall when the disk falls behind (drops = a defect, surfaced, not
silent). A `Recorder` (app) owns the tap consumers between takes; `start()`
drains stale ring contents, opens two `hound` WAVs
(`~/.lion-heart/recordings/<UTC>-di.wav`/`-wet.wav`, timestamped control-side),
spawns a **`lh-recorder` disk thread** that streams both rings to disk, then
arms; `stop()` disarms, joins (final drain + finalize), reclaims the rings;
`Recorder::drop` finalizes any in-progress take. **App-global environment**
(`AppConfig.recordings_dir` + `record_bits` 16/24/32f, default 24) — no preset
schema bump; a take **does not** survive a device restart (fresh session = fresh
recorder; the old one finalizes on drop). (2) **Offline re-amp** — new
`lion-heart render <di.wav> --preset <name> [-o out] [--tail secs]`. Reuses the
**exact live chain** (`build_chain` + `ChainHandle::apply_preset_chain` + the
same `lh-assets`/`lh-nam` loaders), driven by hand: empty chain → reconcile to
the preset → mount assets → pump silent warm-up blocks (drain the control
messages, settle fades) → feed the DI → `--tail` of silence so delay/reverb
tails finish. **Reproducible**: global EQ (environment) is left flat, so a DI
renders identically anywhere; the always-on safety limiter still applies; the DI
must be 48 kHz (mismatch = hard error, matching NAM rate-lock — no offline
resample). New shared **`lh_assets::wav`** (read/write-all + a streaming
`WavStream` + `WavBits`) is the one WAV path, CI-tested (bit-exact float
round-trip). GUI: a header **● REC** button (red + elapsed while recording, ⚠ on
dropped frames); REPL `record start|stop`. **Plugin unchanged** — a DAW is the
recorder/re-amp host there (ADR 013/020 precedent), so no param-id change,
clap-validator unaffected. Tests: `lh-assets::wav` round-trip/interleave/stream
(5), engine tap correctness + drop-counting + disarmed-transparency (2), offline
render processed-output/rate-mismatch/tail/passthrough (4), recorder
timestamp/civil-date/ring-capacity (3). Zero DSP/RT cost when idle; the disk
thread does all I/O. **Unblocks PRD 016** (LUFS leveling reuses `render`).

**M20 (power amp) — code landed (uncommitted).** Specced in PRD 017, recorded as
**ADR 024**. 7th item of the 2026-07-20 roadmap; supplies the post-preamp feel
the (preamp-only) NAM captures miss. New **single-pedal family `power`** in
`lh-dsp/src/power.rs`, placed in `DEFAULT_CHAIN` **after `amp`, before `cab`**
(board **11→12**). Hand-written, 4× oversampled (drive family's `Oversampler4x`)
per channel: presence high-shelf pre-emphasis → **push-pull asymmetric
waveshaper** `supply·(tanh(g·x/supply + BIAS) − tanh(BIAS))` (fixed BIAS →
even harmonics; the `supply` divide/multiply is the **sag** — a drooping rail
clips earlier and lowers the ceiling → dynamic compression + "give") →
output transformer (≈35 Hz low-cut doubling as DC blocker + gentle `tanh` core)
→ depth low-shelf resonance → master (`drive_law` level). **One linked sag
detector** feeds both channels (shared supply, gate/comp precedent); stereo
otherwise independent. Faceplate 5 knobs `drive/sag/presence/depth/master`
(all 0..10; presence/depth **boost-only**). **Ships bypassed**
(`default_active("power")==false`, 2nd family after filter — a full-amp capture
already has a power stage; the engine skips a settled-bypassed slot's `process`,
so it's **zero CPU / zero latency** until engaged, ~24-sample OS latency when
on, like drive). **`MAX_SLOTS` 12→16** (board fills 12/12; headroom for `add` +
off-board pitch/looper/acoustic — engine fixed arrays just grow, no logic
change). **No preset schema bump** (new family = vocabulary; old presets
reconcile the absent slot away). `FAMILY_REGISTRY` 14→15 (power between amp/eq),
plugin fixed chain grows one slot, theme adds a valve-glow orange livery.
Deltas from PRD (ADR 024): **depth is post-waveshaper** (resonance is a
post-tube/transformer/speaker phenomenon; presence stays pre = NFB high-end
lift), presence/depth **boost-only 0..10** (real amp panel, not ±dB). **Plugin
pre-v0.1 additive id break**: `power_{drive,sag,presence,depth,master}` +
`power_active` (default off) appear, no renames — re-run clap-validator. ~12.5 µs
/ block driven (0.94 %, on par with the drive 4× pedals; `power_4x_oversampled`
bench). 11 DSP tests (drive-harmonics, bias even-harmonics, sag compresses loud
> quiet, presence/depth shelves, oversample alias floor, DC-block, slammed
bounded/finite, multi-rate/block, near-unity defaults, click-free sweep, ships
bypassed) + existing registry/plugin/theme pins stay green (381 total).

**M18 (comp family) — code landed (uncommitted).** Specced in PRD 015, recorded
as **ADR 025**. 5th item of the 2026-07-20 roadmap. The `comp` slot became a
**three-pedal family** (`vca`/`opto`/`fet`, family key unchanged, one file per
pedal under `dynamics/comp/`, delay/reverb pattern): one shared detector →
gain-computer engine reading a per-voice `VoiceDef` (knee, ratio law, release
law) in the hot loop, no per-sample vtable. **vca** = the old digital VCA
verbatim (full Threshold/Ratio/Attack/Release, hard knee — **bit-exact at its
defaults** bar a new denormal flush on the detector envelope, RT-hygiene the old
code lacked); **opto** (LA-2A) = fixed 10 ms attack + **program-dependent**
two-stage release (100 ms→1.5 s, sliding by GR depth) + **rising** ratio
(2.5→8:1, leveling→limiting) + soft 12 dB knee, face = Peak Reduction/Gain only;
**fet** (1176) = microsecond attack (20–800 µs) + hard knee + **stepped** ratio
(4/8/12/20/**All**, all-buttons drops threshold −8 dB for the pump). Two **shared
knobs** on every voice: `blend` (parallel/NY comp, `dry·(1−b)+comp·b`, default
1.0) and `sc_hpf` (sidechain-**detector**-only high-pass 20–300 Hz, bypassed at
the 20 Hz floor). Preset **schema v7→v8** (`migrate_v7_comp_pedal` renames the
old `comp` pedal → `vca`, gated `<8` so a `comp` slot from any older version
lands right; `COMP_PEDALS` pins the registry; sparse slots → vca). **DEFAULT_CHAIN
unchanged** (comp stays after gate/filter, before drive; `default_active("comp")`
still true = ships engaged). Deltas from PRD (ADR 025): schema is **7→8 not the
PRD's 6→7** (dual-IR/snapshots already took v6/v7); **fet keeps a `threshold`
knob** (models the 1176's INPUT-into-fixed-threshold — a compressor with no
threshold is undialable). Engine/session/**plugin zero code changes** (multi-pedal
path + generic `from_families` expansion cover it); theme gained opto gold / fet
steel liveries (vca keeps family blue), distinct-livery pin now 8 families.
**Plugin pre-v0.1 id break**: `comp_threshold`→`comp_vca_threshold`,
`comp_opto_*`/`comp_fet_*` + `comp_pedal` selector appear — re-run clap-validator.
~0.56 µs vca/fet, ~1.23 µs opto per block (worst 0.092 %, under the 0.15 % bar;
`comp_{vca,opto,fet}` benches). 14 DSP tests (registry pin, the 4 preserved vca
curve/unity/ratio/makeup tests = migration parity, blend-0 bit-transparent,
sc_hpf spares lows, fet-faster-than-opto attack, opto program-dependent release,
fet all-buttons slams+bounded, topologies distinct, every-voice fuzz +
silence-in-out, pedal-switch finite, multi-rate) + 4 lh-core migration tests.

Pending user verification on the Mac: **the three compressors by ear** (vca
transparent leveling unchanged vs old comp presets; opto slow/round/sticky with
the program-dependent release breathing on a fading note; fet fast attack biting
transients, all-buttons-in pumping; `blend` parallel comp keeping the pick
transient while squashing under it; `sc_hpf` up — a bass note stops pumping the
whole mix; an old `comp` preset saved/reloaded still sounds the same as vca;
plugin ids re-checked — `comp_threshold`→`comp_vca_threshold` rename plus
`comp_opto_*`/`comp_fet_*` + `comp_pedal`, pre-v0.1 break),
pedal switching by ear (per-pedal
values restored, faceplates correct), **red-charlie by ear** (crunch vs
the other drives, bright low-gain edge, B/M/T reach, unity at defaults),
**monster5150 by ear** (chug tightness, sustain, fizz level, Low-knob
post-distortion thickness, no-clean floor acceptable),
**jan-ray by ear** (transparent/dynamic clean-up rolling back the guitar
volume, Fender chime/sparkle, full un-scooped lows breaking up with the
mids, Bass/Treble reach, gentle amp-like warmth, unity at defaults),
**fuzz-face by ear** (splatty asymmetric attack, the gated/velcro decay on
a held note, cleans up rolling the guitar volume back, no-tone-knob thick
germanium voice, Fuzz/Volume interaction; and confirm it does *not* gate a
deliberately-quiet steady signal — that should stay clean, not stutter),
**drive stacking by ear** (Centaur drive-low/output-high boosting an Angry
Charlie or Fuzz Face — low chords should stay tight, no bass fart/boom; and
check the Centaur still sounds full, not thin, on its own),
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
preset** loaded (delay → digital, still rings), **the twelve reverbs by ear**
(hall unchanged vs pre-M10 presets; spring drip + dwell bite; shimmer
+octave climb staying a bounded drone at max; nonlinear gate cutting like an
'80s drum room; magneto head rhythms; swell/bloom/cloud pad behavior against
real playing; chorale vowels; reflections placing the amp in a room),
a reverb-heavy board saved/reloaded and an **old v4 preset** loaded
(reverb → hall, same sound), the plugin re-checked with the reverb voices
expanded (param-id break again), **the reworked tremolo by ear** (default
depth unmissable, chop wave, spread sweep mono→ping-pong; old presets now
audibly throb — intended), **the four new mod pedals by ear** (vibrato
seasick at full depth, harmonic's brownface seesaw on clean, rotary
spin-up/down when flipping speed mid-chord, univibe's Machine Gun throb
with drive in front), **the autowah by ear** (clean funk light/hard
picking tracks the quack; direction down + high q; drag it post-drive for
the synth squelch; the default board's filter LED starts dark and the slot
is silent-neutral until engaged), **the manual wah with the expression
pedal** (right-click the pos knob → learn → sweep: smooth, no staircase,
heel/toe reach the throat's ends; q up for the sharper vowel), **a volume
pedal via CC shaping** (hand-edit `curve: "audio"` — taper feels right),
**pickup** (load a preset, move the pedal: silent until it crosses the
stored value, then seamless), **learn UX end-to-end** (badge shows the CC
number, banner cancel/clear work, `midi.json` survives a restart with
input/channel/pc_presets intact), **snapshots by ear** (store verse/chorus/
solo scenes A–C, switch mid-playing: delay/reverb tails ring through, no
click; set `morph 1000` and hear a filter/mix sweep between scenes;
⤓ re-capture; `*` dirty dot; a scene-bearing preset saved/reloaded keeps
its scenes and the active one; MIDI `snapshot.select` on a pedal steps
A–D), **spillover by ear** (delay/reverb-heavy preset A, hold a chord and
switch to B: the A tail rings out while B is instantly playable; pull a
ringing reverb off the board — tail continues; rapid A/B between two
space-heavy presets doesn't click; `spillover off` cuts tails immediately
for contrast; **and confirm `assert_no_alloc` stays quiet** in a debug
build while spilling — a preset switch mid-note must not SIGABRT),
**the parametric EQ pedal** (switch the eq slot to Parametric, drag a bell
while playing — curve, spectrum, and ear agree, no zipper; wheel = Q,
double-click = band on/off, flat button; move the slot pre-drive vs
post-cab and hear the position; `add eq` for a second instance with its
own memory; save/reload a board with parametric values; an old preset
still loads as the 3-band untouched; plugin ids re-checked — `eq_eq_*`
rename plus `eq_parametric_*`),
**global tempo & sync** (tap the footer BPM chip a few times — it locks in;
set a delay's `sync` division to dotted-1/8 and its echo snaps to the beat,
a synced tremolo pulses in time; re-tap or `tempo <bpm>` re-locks, `sync`
back to *Free* returns the Time/Rate knob with no click as the smoother
glides; feed MIDI clock from a DAW/drum machine — the chip tracks it, stop
the clock and a manual tap takes back over; `tempo.tap` bound to a momentary
footswitch taps on the press, not the release; a synced delay saved/reloaded
stays synced; plugin in a host — turn a delay pedal's `sync` off *Free*,
confirm `time` locks to the project tempo and the host's own `time`
automation goes inert, then back to *Free* and `time` snaps to whatever the
host stored; plugin ids re-checked — `delay_*_sync` + `mod_tremolo_sync`
appeared),
**dual-IR cab by ear** (load MIC A, then MIC B — a different cab/mic; sweep the
`blend` knob A⇄B and hear the mic mix / comb; the level shouldn't jump across
the sweep; a dual-IR preset saved/reloaded keeps both mics + the blend; unload
MIC B returns to A-only; the plugin loads both from that preset and `cab_blend`
automates),
**the octaver by ear** (`add pitch` from the ＋ menu — it starts off the default
board; Sub for a fat synth-bass under a riff, Oct for the 12-string/organ
shimmer, roll Tone back to tuck the up-octave fizz; check chord tracking and the
granular warble on held notes vs the tighter dry; drag it *before* the drive for
cleaner tracking; confirm Dry-only is transparent),
**the looper by ear** (`add looper`; REC records a phrase, REC again plays
it, REC again overdubs a layer, UNDO drops the layer and UNDO again brings it
back, CLEAR empties it; drag the looper before the drive to loop a clean DI
vs after the cab for the full wet tone; reverse plays it backward, half drops
it an octave; the seam doesn't click; a foot CC on `looper.rec` punches the
loop and the faceplate LED tracks it — red recording / green playing / amber
overdub; **confirm `assert_no_alloc` stays quiet** in a debug build while
recording/overdubbing/undoing — a transport press mid-note must not SIGABRT),
**the metronome by ear** (footer `click` toggle starts the tick on beat 1, lit
amber; it locks to the global tempo — tap the BPM chip / `tempo 90` and the
click follows; the time-sig chip moves the accent's bar length; `countin`
restarts on 1; `click <0-100>` sets a comfortable monitor level that sits under
the guitar without clipping on loud transients; **the click bypasses the amp**
— it stays clean regardless of drive/EQ, and does not duck under a loud chord;
the spectrum analyzer/EQ does *not* show the click; a device/buffer change in
settings keeps the click running; **confirm `assert_no_alloc` stays quiet**
with the click on while switching presets/pedals),
**the drum groove by ear** (footer `drums` toggle starts a beat locked to the
tempo; the pattern chip steps rock → funk → metal → ballad; `fill` drops a
one-bar tom roll on the next downbeat; tap the BPM chip / `tempo` and the beat
follows; play along and confirm it stays *in time* on the click — the synth kit
is a drum-machine voice, not sampled acoustic drums, which is expected; drums +
metronome together sit under the guitar without clipping; REPL `groove funk`
starts playing, `groove vol 60`, `fill`; **confirm `assert_no_alloc` stays
quiet** with drums + click on while editing the board),
**the song player by ear** (song tab → load a backing track WAV/MP3; play — it
mixes under the guitar; drag `speed` to 60–70 % and confirm a solo slows down
**without dropping pitch** (WSOLA); `transpose` ±a few semitones lands in a new
key without changing tempo; set A then B to loop a hard bar (the playhead wraps
A→B — a small seam is expected); the `level` slider balances it against the
guitar; the song bypasses the amp (clean backing regardless of drive/EQ);
granular warble at extreme speed/transpose is acceptable for practice; an MP3
loads as well as a WAV; REPL `song load|play|speed|pitch|loop`; **confirm
`assert_no_alloc` stays quiet** with song + drums + click all playing while
editing the board; a device/buffer change drops the song by design — reload it),
**the recorder by ear** (header ● REC starts a take — DI + wet WAVs land in
`~/.lion-heart/recordings/`; play a phrase, stop; the **wet** plays back = what
you heard, the **DI** = dry input; with the metronome/drums on, confirm the
**wet track has no click/drum bleed** — the wet tap is pre-aux; a long take keeps
the header dropped-frames indicator at 0 (⚠ appears only if the disk stalls);
REPL `record start|stop`; **confirm `assert_no_alloc` stays quiet** on a debug
build while `record start`/`record stop` mid-note under a null-device jam — a
take must not SIGABRT; a device/buffer change ends the take (WAV finalized) by
design),
**offline re-amp** (`lion-heart render <old-di.wav> --preset <name>` produces a
re-processed WAV — the tone matches loading that preset live, delay/reverb tails
complete within `--tail`; render the same DI through two presets and hear the
difference; a non-48 kHz DI errors clearly; a missing preset/file errors
cleanly; confirm the render is amp-tone only — the machine's global EQ does not
color it),
**the power amp by ear** (it starts on the default board but **bypassed** — a
full-amp NAM capture should sound unchanged until you light the LED; load a
**preamp-only** capture and engage power: it should "come alive" — sag give on
hard picking, push-pull fatness, presence adding top-end bite and depth adding
low-end thump/resonance; roll the guitar volume back and feel the sag breathe;
crank drive for power-amp saturation without fizz/aliasing; confirm double-
stacking on a full-amp capture sounds worse — the reason it ships off; drag it
before/after in the chain if you `add` a second one; plugin re-check — `power_*`
params + `power_active` (off) appeared, pre-v0.1 additive id break),
plus the standing M7 items
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
| `lh-dsp`         | Effects, one module per category (dynamics, drive, eq, modulation, pitch, time, cab) over shared `blocks/`. Offline-testable, RT-safe | `lh-core`     |
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
