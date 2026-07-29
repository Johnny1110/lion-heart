//! **fuzz-face** — Dallas Arbiter Fuzz Face-style germanium fuzz. Two
//! directly-coupled PNP transistors in a shunt-series feedback pair (the
//! classic 2.2 µF in, 33 kΩ / 8.2 kΩ collectors, 100 kΩ bias feedback, Fuzz
//! at Q2's emitter, Volume off Q2's collector). Almost nothing in common with
//! the op-amp/diode pedals in this family — its whole voice comes from how
//! two transistors misbehave.
//!
//! Three behaviors define it, and this model chases all three:
//! - **Asymmetric clipping.** Q1 biases near 1.3 V, nowhere near mid-supply,
//!   so one polarity hits mushy saturation while the other swings all the way
//!   to cutoff. Modelled as a soft `tanh` on the way up and a **hard flat
//!   clamp** on the way down, plus a fixed bias offset: a fat stack of even
//!   *and* odd harmonics, splatty on the attack — not the tidy odd-only of a
//!   symmetric clipper.
//! - **Gating / splutter on the decay.** The germanium Fuzz Face's signature:
//!   a plucked note fuzzes, sustains, then cuts off with a "velcro" splutter
//!   instead of fading smoothly. Its cause in hardware is *blocking
//!   distortion* — the input coupling cap charges on peaks and bleeds slowly,
//!   biasing the stage toward cutoff once the note falls below where it was.
//!   The audible result is that the note gates when its level drops well below
//!   its own recent peak. We reproduce exactly that: a fast envelope measured
//!   against a slow peak-hold, and when the ratio collapses the output gates.
//!   Because it keys on the *ratio*, not an absolute level, a steady quiet
//!   signal (envelope ≈ its own peak) never gates — it just plays.
//! - **Cleans up with the input.** A real Fuzz Face's ~10 kΩ input impedance
//!   makes it a slave to the guitar's volume pot. We don't have the pickup's
//!   source impedance here, but the sonic result — heavy fuzz that dissolves
//!   to near-clean as the input drops — is inherent to a very-high-gain
//!   clipper and comes for free (and the ratio gate leaves it alone).
//!
//! Two knobs, like the hardware: **Fuzz** (gain — no clean floor, it fuzzes
//! all the way down) and **Volume**. Voiced dark and thick (a pre-clip
//! high-cut for the woolly germanium top); no tone control, by design.
//!
//! # Type: germanium or silicon (PRD 034)
//!
//! A third control, which the hardware does not have because the hardware is
//! two different pedals. From 1968 Dallas Arbiter fitted silicon BC108/BC183s
//! instead of NKT275 germaniums, and nobody who has played both thinks they are
//! the same box. Every difference in the [`VOICES`] table follows from the
//! device:
//!
//! - **more gain** — silicon β runs 3–5× germanium's, so both ends of the Fuzz
//!   sweep move up;
//! - **less asymmetry** — silicon's `Vbe` is predictable and its leakage
//!   negligible, so Q1 biases close to where the resistors say and the two
//!   clipping thresholds converge. The duty cycle straightens out and the even
//!   harmonics thin;
//! - **no gate** — the velcro splutter is *blocking distortion*, driven by
//!   germanium's leaky junction and 0.2 V drop. Silicon Fuzz Faces sustain
//!   smoothly and famously do not do it;
//! - **brighter** — nothing woolly about a BC108.
//!
//! This stays a **behavioural** model, deliberately: phase 05 looked at the
//! nodal state-space route and found the reference implementation's
//! coefficients come out of a private generator, so building one from the
//! literature is a framework's worth of work. It is recorded as future research
//! in ADR 035 rather than half-done here.

use lh_core::{EffectDesc, ParamDesc, Range, db_to_lin};

use crate::blocks::waveshaper::{Adaa1, tanh_f1};

use super::{Circuit, OnePole, Ramp, knob, lp_coeff};

static TYPE_LABELS: [&str; 2] = ["Germanium", "Silicon"];
static TYPE_RANGE: Range = Range::Stepped {
    labels: &TYPE_LABELS,
};

static PARAMS: [ParamDesc; 3] = [
    knob("fuzz", "Fuzz", 5.0, 20.0),
    ParamDesc {
        key: "type",
        name: "Type",
        unit: "",
        range: TYPE_RANGE,
        default: 0.0,
        smoothing_ms: 0.0,
    },
    knob("volume", "Volume", 6.0, 20.0),
];

pub(super) static DESC: EffectDesc = EffectDesc {
    key: "fuzz-face",
    name: "Fuzz Face",
    params: &PARAMS,
};

/// One transistor pair's voicing — everything that changes between a 1966
/// germanium Fuzz Face and a 1968 silicon one.
struct Voice {
    /// Soft "mushy saturation" knee (the swing toward saturation).
    knee_pos: f32,
    /// Hard "cutoff" clamp (the swing toward the off transistor), *flat* below
    /// it. Sits below `knee_pos` by however far off mid-supply the pair biases.
    knee_neg: f32,
    /// Fixed *pre-gain* bias: how far off mid-supply Q1 sits. Offsetting the
    /// signal before the huge gain shifts the clip's zero-crossing, so the
    /// flat-topped square comes out with an asymmetric duty cycle — the even
    /// harmonics survive the DC blocker (a post-clip offset would just be a DC
    /// level the blocker erases, leaving a symmetric square).
    pre_bias: f32,
    /// The note gates once its envelope falls below this fraction of its
    /// slowly-bleeding recent peak — the blocking-distortion cutoff, as a ratio
    /// so it fires on a fading note but never on a merely-quiet one. **Zero
    /// disables it**, which is what silicon does.
    gate_frac: f32,
    /// Pre-clip high-cut feeding the clipper.
    dark_hz: f32,
    /// Fuzz sweep: floor and span, in dB.
    gain_db: (f32, f32),
    /// Calibrated per voice with `default_level_survey`, so changing type is a
    /// change of character and not of loudness.
    makeup: f32,
}

/// Indexed by the Type selector; **append-only** (presets store the index).
static VOICES: [Voice; 2] = [
    Voice {
        knee_pos: 0.9,
        knee_neg: 0.5,
        pre_bias: 0.02,
        gate_frac: 0.25,
        dark_hz: 5_500.0,
        gain_db: (20.0, 35.0),
        makeup: 0.13,
    },
    Voice {
        // Silicon: a matched, predictable pair biased close to where the
        // resistors put it, so the two thresholds close up and both sit lower —
        // harder, squarer, and much less even-harmonic content.
        knee_pos: 0.75,
        knee_neg: 0.68,
        pre_bias: 0.006,
        gate_frac: 0.0,
        dark_hz: 9_000.0,
        gain_db: (26.0, 38.0),
        makeup: 0.125,
    },
];

/// The pair's curve: mushy `tanh` saturation pushing up, a hard flat cutoff
/// pulling down — the transistor asymmetry.
#[inline]
fn germ_clip(v: f64, pos: f64, neg: f64) -> f64 {
    if v >= 0.0 {
        pos * (v / pos).tanh()
    } else {
        v.max(-neg)
    }
}

/// Its antiderivative, normalised to `F₁(0) = 0` (PRD 024). Both branches
/// vanish at the origin, so the halves join without a step.
#[inline]
fn germ_clip_f1(v: f64, pos: f64, neg: f64) -> f64 {
    if v >= 0.0 {
        pos * pos * tanh_f1(v / pos)
    } else if v >= -neg {
        v * v / 2.0
    } else {
        -neg * v - neg * neg / 2.0
    }
}

pub(super) struct FuzzFace {
    /// Anti-aliased clipping (PRD 024): the hard negative floor is exactly the
    /// corner ADAA exists for.
    clip: Adaa1,
    hp_in: OnePole,
    dark: OnePole,
    dc_os: OnePole,
    /// Gate key: a fast envelope of the input, a slow-bleeding peak-hold of
    /// that envelope, and the smoothed gate gain riding their ratio.
    env: f32,
    peak: f32,
    gate: f32,
    /// Index into [`VOICES`] — the Type selector.
    voice: usize,
    os_rate: f32,
    c_sub: f32,
    c_dark: f32,
    c_dc: f32,
    c_env: f32,
    c_peak: f32,
    c_gate: f32,
}

impl FuzzFace {
    pub(super) fn new() -> Self {
        Self {
            clip: Adaa1::new(),
            hp_in: OnePole::default(),
            dark: OnePole::default(),
            dc_os: OnePole::default(),
            env: 0.0,
            peak: 0.0,
            gate: 0.0,
            voice: 0,
            os_rate: 4.0 * 48_000.0,
            c_sub: 0.0,
            c_dark: 0.0,
            c_dc: 0.0,
            c_env: 0.0,
            c_peak: 0.0,
            c_gate: 0.0,
        }
    }
}

impl Circuit for FuzzFace {
    fn prepare(&mut self, _base_rate: f32, os_rate: f32) {
        // Everything runs at the oversampled rate: the clip is brutal and the
        // gating envelope shares its clock.
        self.os_rate = os_rate;
        self.c_sub = lp_coeff(50.0, os_rate);
        self.c_dark = lp_coeff(VOICES[self.voice].dark_hz, os_rate);
        self.c_dc = lp_coeff(10.0, os_rate);
        // Envelope ~4 ms, gate smoothing ~3 ms (declicked), peak-hold bleed
        // ~0.6 s — slower than the note so the ratio falls as it decays, fast
        // enough that the body still plays before the tail gates.
        self.c_env = lp_coeff(40.0, os_rate);
        self.c_peak = lp_coeff(0.27, os_rate);
        self.c_gate = lp_coeff(50.0, os_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.clip.reset();
        self.hp_in.reset();
        self.dark.reset();
        self.dc_os.reset();
        self.env = 0.0;
        self.peak = 0.0;
        self.gate = 0.0;
    }

    fn set_shape(&mut self, index: usize) {
        let index = index.min(VOICES.len() - 1);
        if index != self.voice {
            self.voice = index;
            // The pre-clip high-cut is part of the voice, so it is rebuilt
            // here; everything else the voice touches is read per sample.
            self.c_dark = lp_coeff(VOICES[index].dark_hz, self.os_rate);
            // A different pair of transistors is a different pedal, not a knob
            // move: start it clean rather than reinterpreting the old one's
            // clipper and gate state.
            self.reset();
        }
    }

    fn shape(&mut self, block: &mut [f32], drive: &[f32]) {
        // A fuzz is high gain top to bottom (Fuzz down is still dirty — it
        // cleans up from the *guitar*, not this knob). Audio taper, powf twice
        // per chunk, ramped per sample; the floor and span are the voice's.
        let v = &VOICES[self.voice];
        let (floor, span) = v.gain_db;
        let (knee_pos, knee_neg) = (f64::from(v.knee_pos), f64::from(v.knee_neg));
        let (pre_bias, gate_frac) = (v.pre_bias, v.gate_frac);
        let mut gain = Ramp::over(drive, |d| db_to_lin(floor + span * (d * 0.1).powf(1.5)));
        for s in block.iter_mut() {
            let x = *s;
            // Tightening high-pass at 50 Hz (a fuzz's huge gain turns any
            // sub-bass into flub, worst when a boost is stacked in front),
            // then the woolly germanium high-cut feeding the clipper — dark
            // in, so the fuzz is smooth, not fizzy.
            let x = x - self.hp_in.lp(x, self.c_sub);

            // Gate key on the (clean, pre-gain) input: fast envelope vs a
            // slow-bleeding peak-hold. When the note fades well below its
            // recent peak the ratio collapses and the gate shuts — the germ
            // splutter. A steady signal keeps env ≈ peak, so it never gates.
            self.env += self.c_env * (x.abs() - self.env);
            if self.env > self.peak {
                self.peak = self.env;
            } else {
                self.peak -= self.c_peak * self.peak;
            }
            if self.peak < 1e-20 {
                self.peak = 0.0;
            }
            // A zero threshold (silicon) means *no gate*: held open, including
            // from the first sample, rather than merely never tripping.
            let target = if gate_frac <= 0.0 || self.env > gate_frac * self.peak {
                1.0
            } else {
                0.0
            };
            self.gate += self.c_gate * (target - self.gate);

            let xd = self.dark.lp(x, self.c_dark);
            let v = gain.tick() * (xd + pre_bias);
            // Asymmetric clip: mushy soft saturation pushing up, a hard flat
            // cutoff pulling down.
            let clipped = self.clip.process(
                v,
                |u| germ_clip(u, knee_pos, knee_neg),
                |u| germ_clip_f1(u, knee_pos, knee_neg),
            );
            *s = (clipped - self.dc_os.lp(clipped, self.c_dc)) * self.gate;
        }
    }

    fn post(&mut self, block: &mut [f32], _tone: &[f32]) {
        // No tone knob — just the voice's output makeup.
        let makeup = VOICES[self.voice].makeup;
        for s in block.iter_mut() {
            *s *= makeup;
        }
    }
}
