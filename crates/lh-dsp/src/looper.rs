//! Looper: a single-pedal chain-slot family (PRD 013). Record a phrase,
//! layer overdubs on top, undo the last layer, play it back reversed or at
//! half speed. Being a chain slot, *where* you place it is the feature —
//! drop it before the drive to loop a clean DI, after the cab to loop the
//! fully-processed tone (PRD 002 gives that for free; the looper adds no new
//! engine mechanism).
//!
//! **Transport is momentary params** (PRD 013 / the `tempo.tap` idiom): a
//! `rec`/`undo`/`clear` press is a value crossing 0.5, edge-detected here in
//! [`Effect::set_param`]; the state change is pure O(1) bookkeeping, so it is
//! RT-safe to do inline. `reverse`/`half` are stepped toggles; `level`/`mix`
//! are the smoothed continuous knobs. The engine and session are untouched —
//! everything rides the existing param path.
//!
//! **State machine** (one-button, the classic single-switch looper):
//! `Empty →[rec] Recording →[rec] Playing →[rec] Overdubbing →[rec] Playing …`
//! The first recording pass defines the loop length (v1 is free length; tempo
//! quantize is a v2 item). `clear` returns to `Empty`.
//!
//! **Real-time invariants** (white paper §3.1):
//! - The two 60-second stereo banks are allocated once, in [`prepare`]. That
//!   is ~46 MB at 48 kHz — the price of a hardware-looper-grade buffer.
//! - `clear`/`reset` are **logical**: they zero `loop_len`, not the buffer.
//!   A bulk `memset` of tens of MB on the audio thread would blow the block
//!   budget; instead we only ever read inside `[0, loop_len)`, and recording
//!   overwrites from index 0, so stale samples past the (new) loop end are
//!   never audible.
//! - Overdub sums **in place** with a `tanh` soft clip (unity small-signal,
//!   bounded ceiling), so stacking layers forever can never run away (rule 7).
//! - **One-level undo/redo** is a bank-index swap — no audio-thread copy of
//!   the whole loop. The undo snapshot is filled lazily during an overdub's
//!   first pass (copy-*before*-sum, so it captures the pre-overdub loop), and
//!   becomes valid only once that pass completes.
//! - The loop seam is smoothed by a short **boundary fade** (the loop dips to
//!   zero across a few ms at the wrap), so a recording whose start and end
//!   don't line up can't click. A single read tap keeps playback faithful —
//!   no granular blending that would smear the loop.
//!
//! Not in the plugin: a DAW owns looping/freezing, and the plugin chain is
//! host-driven (same reasoning as spillover, PRD 010/013).

use lh_core::{EffectDesc, FamilyDesc, ParamDesc, Range};

use crate::Effect;
use crate::blocks::smooth::Smoothed;

/// Longest loop any single take can hold. 60 s of stereo at 48 kHz is ~23 MB
/// per bank; two banks (loop + undo snapshot) ~46 MB, allocated in `prepare`.
const MAX_LOOP_SECONDS: f32 = 60.0;

/// Boundary fade half-width target: the loop ramps to zero over this long at
/// each end of the wrap, clamped so it never eats more than a quarter of a
/// short loop. Kept short enough to be a subtle "breath," long enough that a
/// full-scale seam discontinuity can't click.
const SEAM_FADE_MS: f32 = 6.0;

/// Overdub soft-clip drive: `tanh(drive·x)/drive`. Below unity so medium
/// signals pass near-linearly (an overdub of silence barely touches the loop)
/// while the `1/drive` ceiling keeps infinite stacking bounded.
const OVERDUB_DRIVE: f32 = 0.7;

static PARAMS: [ParamDesc; 7] = [
    // Momentary transport: real value crosses 0.5 to fire (edge-detected).
    momentary("rec", "Rec"),
    momentary("undo", "Undo"),
    momentary("clear", "Clear"),
    toggle("reverse", "Reverse"),
    toggle("half", "Half"),
    ParamDesc {
        key: "level",
        name: "Level",
        unit: "",
        range: Range::Linear { min: 0.0, max: 1.5 },
        default: 1.0,
        smoothing_ms: 30.0,
    },
    ParamDesc {
        key: "mix",
        name: "Mix",
        unit: "",
        range: Range::Linear { min: 0.0, max: 1.0 },
        default: 1.0,
        smoothing_ms: 20.0,
    },
];

/// A momentary "button" parameter: linear 0..1, default off, not smoothed —
/// the press is a rising edge through 0.5, handled in `set_param`.
const fn momentary(key: &'static str, name: &'static str) -> ParamDesc {
    ParamDesc {
        key,
        name,
        unit: "",
        range: Range::Linear { min: 0.0, max: 1.0 },
        default: 0.0,
        smoothing_ms: 0.0,
    }
}

/// A stepped off/on toggle (reverse, half): default off.
const fn toggle(key: &'static str, name: &'static str) -> ParamDesc {
    ParamDesc {
        key,
        name,
        unit: "",
        range: Range::Stepped {
            labels: &["off", "on"],
        },
        default: 0.0,
        smoothing_ms: 0.0,
    }
}

pub static DESC: EffectDesc = EffectDesc {
    key: "looper",
    name: "Looper",
    params: &PARAMS,
};

/// Single-pedal family: the pedal key doubles as the family key.
pub static FAMILY: FamilyDesc = FamilyDesc {
    key: "looper",
    name: "Looper",
    pedals: &[&DESC],
};

/// Param positions (single pedal — match on the index directly, like `gate`).
const P_REC: usize = 0;
const P_UNDO: usize = 1;
const P_CLEAR: usize = 2;
const P_REVERSE: usize = 3;
const P_HALF: usize = 4;
const P_LEVEL: usize = 5;
const P_MIX: usize = 6;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Empty,
    Recording,
    Playing,
    Overdubbing,
}

/// Unity small-signal, bounded loud — the overdub-stack limiter (RT rule 7).
#[inline]
fn soft_clip(x: f32) -> f32 {
    (x * OVERDUB_DRIVE).tanh() / OVERDUB_DRIVE
}

/// One interpolated read tap into a channel's loop buffer, wrapping at
/// `loop_len`. A single tap keeps the loop faithful (no granular smear); the
/// seam is smoothed separately by [`seam_gain`].
#[inline]
fn read_tap(ch: &[f32], head: f32, loop_len: usize) -> f32 {
    let base = head.floor();
    let frac = head - base;
    let i0 = (base as usize) % loop_len;
    let i1 = if i0 + 1 == loop_len { 0 } else { i0 + 1 };
    ch[i0] + frac * (ch[i1] - ch[i0])
}

/// Boundary fade: the loop playback ramps to zero over `fade` samples at each
/// end of the wrap, so a start/end mismatch can't click. Smoothstep (C¹, no
/// trig) keeps the ramp itself click-free.
#[inline]
fn seam_gain(head: f32, loop_len: usize, fade: f32) -> f32 {
    if fade < 1.0 {
        return 1.0;
    }
    let e = head.min(loop_len as f32 - head);
    if e >= fade {
        1.0
    } else {
        let u = (e / fade).clamp(0.0, 1.0);
        u * u * (3.0 - 2.0 * u)
    }
}

pub struct Looper {
    sample_rate: u32,
    /// Two banks: `banks[play]` is the live loop, `banks[1 - play]` the undo
    /// snapshot. Each bank is `[left, right]`, preallocated in `prepare`.
    banks: [[Vec<f32>; 2]; 2],
    play: usize,
    max_len: usize,

    state: State,
    loop_len: usize,
    /// Fractional read/record head. Integer-valued (frac 0) while recording or
    /// overdubbing; fractional under reverse/half playback.
    head: f32,
    fade: f32,

    reverse: bool,
    half: bool,
    level: Smoothed,
    mix: Smoothed,

    // One-level undo/redo bookkeeping.
    dub_first_pass: bool,
    dub_covered: usize,
    undo_available: bool,
    redo_available: bool,

    // Momentary edge memory (rising through 0.5 fires).
    prev_rec: f32,
    prev_undo: f32,
    prev_clear: f32,
}

impl Default for Looper {
    fn default() -> Self {
        Self::new()
    }
}

impl Looper {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000,
            banks: [[Vec::new(), Vec::new()], [Vec::new(), Vec::new()]],
            play: 0,
            max_len: 0,
            state: State::Empty,
            loop_len: 0,
            head: 0.0,
            fade: 0.0,
            reverse: false,
            half: false,
            level: Smoothed::new(PARAMS[P_LEVEL].default),
            mix: Smoothed::new(PARAMS[P_MIX].default),
            dub_first_pass: false,
            dub_covered: 0,
            undo_available: false,
            redo_available: false,
            prev_rec: 0.0,
            prev_undo: 0.0,
            prev_clear: 0.0,
        }
    }

    /// The current transport state, for the GUI LED mirror (control side reads
    /// this only in offline tests; the live GUI mirrors it, PRD 013).
    #[cfg(test)]
    fn state(&self) -> State {
        self.state
    }

    /// Logical clear: forget the loop without a bulk `memset` (RT-safe). Stale
    /// samples past `loop_len` are never read.
    fn clear_loop(&mut self) {
        self.state = State::Empty;
        self.loop_len = 0;
        self.head = 0.0;
        self.dub_first_pass = false;
        self.dub_covered = 0;
        self.undo_available = false;
        self.redo_available = false;
    }

    /// Advance the one-button state machine on a `rec` press.
    fn on_rec(&mut self) {
        match self.state {
            State::Empty => {
                // Start a fresh take from the top of the play bank.
                self.head = 0.0;
                self.state = State::Recording;
            }
            State::Recording => {
                // First pass defines the loop length.
                let len = self.head as usize;
                if len >= 2 {
                    self.loop_len = len;
                    self.state = State::Playing;
                    self.head = 0.0;
                    // A brand-new loop has no undo history yet.
                    self.undo_available = false;
                    self.redo_available = false;
                } else {
                    // A double-tap with nothing recorded: back to empty.
                    self.clear_loop();
                }
            }
            State::Playing => {
                if self.loop_len >= 2 {
                    // Align the overdub head to an integer sample so writes
                    // stay contiguous and the snapshot copy is exact.
                    self.head = self.head.round() % self.loop_len as f32;
                    self.state = State::Overdubbing;
                    self.dub_first_pass = true;
                    self.dub_covered = 0;
                }
            }
            State::Overdubbing => {
                self.state = State::Playing;
            }
        }
    }

    /// Undo/redo: swap the play bank with the snapshot bank. Only meaningful in
    /// `Playing` (undoing mid-overdub is ill-defined for one level; PRD 013).
    fn on_undo(&mut self) {
        if self.state != State::Playing {
            return;
        }
        if self.undo_available {
            self.play ^= 1;
            self.undo_available = false;
            self.redo_available = true;
        } else if self.redo_available {
            self.play ^= 1;
            self.redo_available = false;
            self.undo_available = true;
        }
    }
}

impl Effect for Looper {
    fn family(&self) -> &'static FamilyDesc {
        &FAMILY
    }

    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate;
        self.max_len = (MAX_LOOP_SECONDS * sample_rate as f32) as usize + 1;
        for bank in &mut self.banks {
            for ch in bank {
                *ch = vec![0.0; self.max_len];
            }
        }
        self.fade = (SEAM_FADE_MS * 1e-3 * sample_rate as f32).max(1.0);
        self.level
            .configure(PARAMS[P_LEVEL].smoothing_ms, sample_rate);
        self.mix.configure(PARAMS[P_MIX].smoothing_ms, sample_rate);
        self.level.snap_to_target();
        self.mix.snap_to_target();
        self.reset();
    }

    fn reset(&mut self) {
        // Logical clear only — never memset the multi-MB banks on the RT path.
        self.play = 0;
        self.clear_loop();
    }

    fn set_param(&mut self, index: usize, normalized: f32) {
        match index {
            P_REC => {
                let fired = self.prev_rec < 0.5 && normalized >= 0.5;
                self.prev_rec = normalized;
                if fired {
                    self.on_rec();
                }
            }
            P_UNDO => {
                let fired = self.prev_undo < 0.5 && normalized >= 0.5;
                self.prev_undo = normalized;
                if fired {
                    self.on_undo();
                }
            }
            P_CLEAR => {
                let fired = self.prev_clear < 0.5 && normalized >= 0.5;
                self.prev_clear = normalized;
                if fired {
                    self.clear_loop();
                }
            }
            P_REVERSE => self.reverse = normalized >= 0.5,
            P_HALF => self.half = normalized >= 0.5,
            P_LEVEL => self
                .level
                .set_target(PARAMS[P_LEVEL].range.to_real(normalized)),
            P_MIX => self.mix.set_target(PARAMS[P_MIX].range.to_real(normalized)),
            _ => {}
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.max_len == 0 {
            return; // prepare() not called yet
        }
        let play = self.play;
        let max_len = self.max_len;
        let fade = self.fade;
        // Borrow both banks at once (field-split borrow); `play`/snapshot
        // indices are constant across a block (undo swaps happen in set_param).
        let (first, second) = self.banks.split_at_mut(1);
        let (play_bank, snap_bank) = if play == 0 {
            (&mut first[0], &mut second[0])
        } else {
            (&mut second[0], &mut first[0])
        };

        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let dry_l = *l;
            let dry_r = *r;
            let level = self.level.tick();
            let mix = self.mix.tick();
            let mut loop_l = 0.0;
            let mut loop_r = 0.0;

            match self.state {
                State::Empty => {}
                State::Recording => {
                    let i = self.head as usize;
                    if i < max_len {
                        play_bank[0][i] = dry_l;
                        play_bank[1][i] = dry_r;
                        self.head += 1.0;
                        if self.head as usize >= max_len {
                            // Hit the ceiling: auto-commit the maximal loop.
                            self.loop_len = max_len;
                            self.state = State::Playing;
                            self.head = 0.0;
                        }
                    }
                }
                State::Playing => {
                    let len = self.loop_len;
                    let g = seam_gain(self.head, len, fade);
                    loop_l = read_tap(&play_bank[0], self.head, len) * g;
                    loop_r = read_tap(&play_bank[1], self.head, len) * g;
                    let step = if self.half { 0.5 } else { 1.0 };
                    if self.reverse {
                        self.head -= step;
                        if self.head < 0.0 {
                            self.head += len as f32;
                        }
                    } else {
                        self.head += step;
                        if self.head >= len as f32 {
                            self.head -= len as f32;
                        }
                    }
                }
                State::Overdubbing => {
                    let len = self.loop_len;
                    let i = self.head as usize;
                    let old_l = play_bank[0][i];
                    let old_r = play_bank[1][i];
                    // First pass fills the undo snapshot with the pre-overdub
                    // loop (copy before summing), one sample per visited index.
                    if self.dub_first_pass {
                        snap_bank[0][i] = old_l;
                        snap_bank[1][i] = old_r;
                        self.dub_covered += 1;
                        if self.dub_covered >= len {
                            self.dub_first_pass = false;
                            self.undo_available = true;
                            self.redo_available = false;
                        }
                    }
                    // Play the existing loop (seam-faded); the new layer is
                    // heard live via dry and joins the loop next pass.
                    let g = seam_gain(self.head, len, fade);
                    loop_l = old_l * g;
                    loop_r = old_r * g;
                    play_bank[0][i] = soft_clip(old_l + dry_l);
                    play_bank[1][i] = soft_clip(old_r + dry_r);
                    self.head += 1.0;
                    if self.head >= len as f32 {
                        self.head -= len as f32;
                    }
                }
            }

            // Dry always passes at unity — a looper must never duck live
            // playing; the loop sums on top at `level × mix` (mix 0 = bit-exact
            // dry).
            let loop_gain = level * mix;
            *l = dry_l + loop_l * loop_gain;
            *r = dry_r + loop_r * loop_gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{assert_finite, peak, rms, silence, sine};

    const SR: u32 = 48_000;

    fn prepared() -> Looper {
        let mut lp = Looper::new();
        lp.prepare(SR);
        lp
    }

    /// A momentary press: pulse the param high then low (the GUI/session sends
    /// exactly this, so the next press edges again).
    fn press(lp: &mut Looper, index: usize) {
        lp.set_param(index, 1.0);
        lp.set_param(index, 0.0);
    }

    /// Process one owned buffer in place, returning (L, R).
    fn run(lp: &mut Looper, input: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let mut l = input.to_vec();
        let mut r = input.to_vec();
        lp.process(&mut l, &mut r);
        (l, r)
    }

    fn set_real(lp: &mut Looper, index: usize, real: f32) {
        lp.set_param(index, PARAMS[index].range.to_norm(real));
    }

    #[test]
    fn registry_is_consistent() {
        assert_eq!(FAMILY.key, "looper");
        assert_eq!(FAMILY.pedals.len(), 1);
        assert!(std::ptr::eq(FAMILY.pedals[0], &DESC));
        let keys: Vec<&str> = DESC.params.iter().map(|p| p.key).collect();
        assert_eq!(
            keys,
            ["rec", "undo", "clear", "reverse", "half", "level", "mix"]
        );
        // A looper isn't in the default chain; when added it engages actively.
        assert!(lh_core::default_active("looper"));
    }

    #[test]
    fn state_machine_walks_empty_record_play_overdub_play() {
        let mut lp = prepared();
        assert_eq!(lp.state(), State::Empty);
        press(&mut lp, P_REC);
        assert_eq!(lp.state(), State::Recording);
        // Record ~50 ms so the loop has a real length.
        let x = sine(SR, 220.0, SR as usize / 20);
        run(&mut lp, &x);
        press(&mut lp, P_REC);
        assert_eq!(lp.state(), State::Playing);
        assert!(lp.loop_len >= 2, "loop length was captured");
        press(&mut lp, P_REC);
        assert_eq!(lp.state(), State::Overdubbing);
        // Overdub a full pass so the snapshot completes.
        let dub = sine(SR, 330.0, lp.loop_len + 64);
        run(&mut lp, &dub);
        press(&mut lp, P_REC);
        assert_eq!(lp.state(), State::Playing);
    }

    #[test]
    fn recorded_loop_plays_back_in_silence() {
        let mut lp = prepared();
        press(&mut lp, P_REC);
        let x = sine(SR, 220.0, SR as usize / 10);
        run(&mut lp, &x);
        press(&mut lp, P_REC); // -> Playing
        // Feed silence: the loop must ring out (dry is silent, loop is not).
        let (l, _) = run(&mut lp, &silence(SR as usize / 2));
        assert_finite("looper playback", &l);
        assert!(
            rms(&l) > 0.1,
            "loop must play under silent input: {}",
            rms(&l)
        );
    }

    #[test]
    fn empty_is_silent_and_bit_transparent() {
        let mut lp = prepared();
        let x = sine(SR, 200.0, 4_096);
        let (l, r) = run(&mut lp, &x);
        assert_eq!(l, x, "empty looper passes dry L untouched");
        assert_eq!(r, x, "empty looper passes dry R untouched");
        // And clearing an empty looper stays silent.
        press(&mut lp, P_CLEAR);
        let (l2, _) = run(&mut lp, &silence(2_048));
        assert!(rms(&l2) == 0.0);
    }

    #[test]
    fn mix_zero_is_bit_exact_dry_even_with_a_loop() {
        let mut lp = prepared();
        press(&mut lp, P_REC);
        run(&mut lp, &sine(SR, 220.0, SR as usize / 10));
        press(&mut lp, P_REC); // Playing, loop full
        set_real(&mut lp, P_MIX, 0.0);
        // Warm a full second so the 20 ms mix smoother snaps to exactly 0,
        // then assert bit-exact dry.
        run(&mut lp, &sine(SR, 220.0, SR as usize));
        let x = sine(SR, 330.0, 8_192);
        let (l, r) = run(&mut lp, &x);
        assert_eq!(l, x, "mix 0 must pass dry exactly (L)");
        assert_eq!(r, x, "mix 0 must pass dry exactly (R)");
    }

    #[test]
    fn clear_returns_to_empty_and_silences_the_loop() {
        let mut lp = prepared();
        press(&mut lp, P_REC);
        run(&mut lp, &sine(SR, 220.0, SR as usize / 10));
        press(&mut lp, P_REC); // Playing
        run(&mut lp, &silence(1_024)); // it's ringing
        press(&mut lp, P_CLEAR);
        assert_eq!(lp.state(), State::Empty);
        let (l, _) = run(&mut lp, &silence(SR as usize / 4));
        assert!(rms(&l) == 0.0, "cleared loop must be silent: {}", rms(&l));
    }

    #[test]
    fn undo_swaps_back_to_the_pre_overdub_loop() {
        // Record a loud tone, overdub a full pass, then undo: the playback
        // energy must drop back toward the pre-overdub level.
        let mut lp = prepared();
        press(&mut lp, P_REC);
        let base: Vec<f32> = sine(SR, 220.0, SR as usize / 10)
            .iter()
            .map(|s| s * 0.5)
            .collect();
        run(&mut lp, &base);
        press(&mut lp, P_REC); // Playing
        let len = lp.loop_len;

        // Measure the pre-overdub playback level (a full loop under silence).
        let (pre, _) = run(&mut lp, &silence(len + 128));
        let pre_rms = rms(&pre[64..len]);

        // Overdub a loud layer for two full passes.
        press(&mut lp, P_REC); // Overdubbing
        let dub: Vec<f32> = sine(SR, 440.0, len * 2 + 128)
            .iter()
            .map(|s| s * 0.5)
            .collect();
        run(&mut lp, &dub);
        press(&mut lp, P_REC); // Playing (overdub committed)

        let (dubbed, _) = run(&mut lp, &silence(len + 128));
        let dubbed_rms = rms(&dubbed[64..len]);
        assert!(
            dubbed_rms > 1.2 * pre_rms,
            "overdub must add energy: {pre_rms:.4} -> {dubbed_rms:.4}"
        );

        // Undo: back to the pre-overdub loop.
        press(&mut lp, P_UNDO);
        let (undone, _) = run(&mut lp, &silence(len + 128));
        let undone_rms = rms(&undone[64..len]);
        assert!(
            undone_rms < 0.6 * dubbed_rms,
            "undo must drop the overdub layer: {dubbed_rms:.4} -> {undone_rms:.4}"
        );

        // Redo: the overdub returns.
        press(&mut lp, P_UNDO);
        let (redone, _) = run(&mut lp, &silence(len + 128));
        assert!(
            rms(&redone[64..len]) > 1.2 * undone_rms,
            "redo must restore the overdub"
        );
    }

    #[test]
    fn overdub_soft_clip_stays_bounded_under_infinite_stacking() {
        let mut lp = prepared();
        press(&mut lp, P_REC);
        run(&mut lp, &sine(SR, 110.0, SR as usize / 20));
        press(&mut lp, P_REC); // Playing
        let len = lp.loop_len;
        press(&mut lp, P_REC); // Overdubbing
        // Stack 200 loud passes: the tanh ceiling must hold.
        let loud: Vec<f32> = sine(SR, 110.0, len * 200).iter().map(|s| s * 0.9).collect();
        let (l, r) = run(&mut lp, &loud);
        assert_finite("looper overdub stack L", &l);
        assert_finite("looper overdub stack R", &r);
        // Loop content is tanh-bounded to 1/drive; dry adds < 1 on top.
        assert!(
            peak(&l) < 3.0,
            "overdub stack must stay bounded: {}",
            peak(&l)
        );
    }

    #[test]
    fn reverse_mirrors_the_loop_position() {
        // Record a loop that is silent except for a marker burst in its first
        // third. Forward playback places the marker early; reverse places it
        // late (mirrored about the loop).
        let mut lp = prepared();
        let len = SR as usize / 5; // 200 ms
        let mut mark = silence(len);
        let pos = len / 6;
        for s in &mut mark[pos..pos + 200] {
            *s = 0.8;
        }
        press(&mut lp, P_REC);
        run(&mut lp, &mark);
        press(&mut lp, P_REC); // Playing forward

        let find_peak = |buf: &[f32]| {
            buf.iter()
                .enumerate()
                .max_by(|a, b| a.1.abs().partial_cmp(&b.1.abs()).unwrap())
                .map(|(i, _)| i)
                .unwrap()
        };
        let (fwd, _) = run(&mut lp, &silence(len));
        let fwd_pos = find_peak(&fwd);
        assert!(fwd_pos < len / 2, "forward marker sits early: {fwd_pos}");

        // Restart at the loop top, engage reverse.
        press(&mut lp, P_CLEAR);
        press(&mut lp, P_REC);
        run(&mut lp, &mark);
        press(&mut lp, P_REC);
        set_real(&mut lp, P_REVERSE, 1.0);
        let (rev, _) = run(&mut lp, &silence(len));
        let rev_pos = find_peak(&rev);
        assert!(rev_pos > len / 2, "reverse marker sits late: {rev_pos}");
    }

    #[test]
    fn half_speed_doubles_the_loop_period() {
        // A marker burst recurs every loop_len forward, every 2×loop_len at
        // half speed.
        let mut lp = prepared();
        let len = SR as usize / 5;
        let mut mark = silence(len);
        for s in &mut mark[100..300] {
            *s = 0.8;
        }
        press(&mut lp, P_REC);
        run(&mut lp, &mark);
        press(&mut lp, P_REC);
        set_real(&mut lp, P_HALF, 1.0);
        // Over 2×loop_len the marker should appear once (its second hit lands
        // right at the end), vs twice at full speed.
        let (out, _) = run(&mut lp, &silence(2 * len));
        let hits = out
            .windows(1)
            .enumerate()
            .filter(|(_, w)| w[0].abs() > 0.4)
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        // Group contiguous hits into events.
        let mut events = 0;
        let mut last = None;
        for i in hits {
            if last.is_none_or(|p| i > p + 50) {
                events += 1;
            }
            last = Some(i);
        }
        assert_eq!(
            events, 1,
            "half speed must stretch the period (one hit in 2×len)"
        );
    }

    #[test]
    fn loop_seam_does_not_click() {
        // Record a loop that ends far from where it starts (a ramp): the raw
        // wrap would be a full-scale jump. The boundary fade must bound the
        // sample-to-sample delta well under that.
        let mut lp = prepared();
        let len = SR as usize / 10;
        let ramp: Vec<f32> = (0..len)
            .map(|i| -0.9 + 1.8 * i as f32 / len as f32)
            .collect();
        press(&mut lp, P_REC);
        run(&mut lp, &ramp);
        press(&mut lp, P_REC); // Playing
        set_real(&mut lp, P_MIX, 1.0);
        // Play several loops under silence so only the loop drives the output.
        let (l, _) = run(&mut lp, &silence(len * 3));
        let max_delta = l
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_delta < 0.2,
            "seam fade must bound the wrap delta, got {max_delta:.3}"
        );
    }

    #[test]
    fn level_scales_loop_playback() {
        let loop_rms = |level: f32| {
            let mut lp = prepared();
            press(&mut lp, P_REC);
            run(&mut lp, &sine(SR, 220.0, SR as usize / 10));
            press(&mut lp, P_REC);
            set_real(&mut lp, P_LEVEL, level);
            run(&mut lp, &silence(2_048)); // settle the level smoother
            let (l, _) = run(&mut lp, &silence(SR as usize / 4));
            rms(&l)
        };
        let low = loop_rms(0.5);
        let high = loop_rms(1.5);
        assert!(
            high > 2.5 * low,
            "level must scale the loop: {low:.4} vs {high:.4}"
        );
    }

    #[test]
    fn stored_momentary_value_does_not_retrigger_on_resend() {
        // Re-sending a settled rec=0 (preset load / shadow re-send) must not
        // fire; only a genuine rising edge does.
        let mut lp = prepared();
        lp.set_param(P_REC, 0.0);
        lp.set_param(P_REC, 0.0);
        assert_eq!(lp.state(), State::Empty, "resent 0 never records");
        lp.set_param(P_REC, 1.0);
        assert_eq!(lp.state(), State::Recording, "the rising edge records");
    }

    #[test]
    fn buffer_holds_sixty_seconds_at_ninety_six_k() {
        // prepare at 96 k, record past 60 s: it must auto-commit at the ceiling
        // without panicking or overflowing.
        let mut lp = Looper::new();
        lp.prepare(96_000);
        press(&mut lp, P_REC);
        // 61 s of input in chunks; recording auto-commits at 60 s.
        for _ in 0..61 {
            run(&mut lp, &sine(96_000, 220.0, 96_000));
        }
        assert_eq!(lp.state(), State::Playing, "auto-committed at the ceiling");
        assert_eq!(lp.loop_len, lp.max_len, "loop capped at 60 s");
        let (l, _) = run(&mut lp, &silence(4_096));
        assert_finite("looper 60s cap", &l);
    }

    #[test]
    fn survives_all_rates_and_block_sizes() {
        for sr in [44_100u32, 48_000, 96_000] {
            let mut lp = Looper::new();
            lp.prepare(sr);
            press(&mut lp, P_REC);
            // Record, play, overdub across odd block sizes.
            let x = sine(sr, 330.0, sr as usize / 8);
            for chunk in x.chunks(97) {
                let mut l = chunk.to_vec();
                let mut r = chunk.to_vec();
                lp.process(&mut l, &mut r);
            }
            press(&mut lp, P_REC);
            press(&mut lp, P_REC); // overdub
            for chunk in [32usize, 483, 1_024] {
                let x = sine(sr, 440.0, 4_096);
                for c in x.chunks(chunk) {
                    let mut l = c.to_vec();
                    let mut r = c.to_vec();
                    lp.process(&mut l, &mut r);
                    assert_finite("looper multirate", &l);
                }
            }
        }
    }
}
