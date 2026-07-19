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

Pending user verification on the Mac: pedal switching by ear (per-pedal
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
build while spilling — a preset switch mid-note must not SIGABRT), plus the
standing M7 items
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
