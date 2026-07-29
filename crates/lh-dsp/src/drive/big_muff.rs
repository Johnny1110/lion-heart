//! **big-muff** — the Electro-Harmonix Big Muff Pi's drive section as a
//! white-box model: **two cascaded transistor clipping stages** with the diodes
//! inside each stage's feedback loop, into the Muff's own tone stack. The first
//! pedal of Tone Revolution phase 05 (PRD 032), and the first in the family
//! whose amplifier is a *transistor* rather than an op-amp.
//!
//! # Why it is not a WDF
//!
//! Every drive since phase 03 has been a wave digital filter, so the exception
//! is worth stating. A Big Muff clipping stage is a common-emitter amplifier
//! with `R17 ‖ C12 ‖ diodes` wrapped from its collector back to its base — the
//! same *mechanism* as [`super::sd1`]'s feedback overdrive. But the amplifier
//! is not an op-amp: linearised to `A = −Rc/Re`, it is a bare voltage gain with
//! no input or output impedance an R-type junction could be built around.
//! Claiming one would be inventing numbers. So the stage is solved as what it
//! is — one node equation, one damped Newton — in
//! [`crate::blocks::transistor::ShuntFeedbackStage`].
//!
//! # What makes it a Muff and not a Screamer
//!
//! - **Two stages, in series.** Stage 1 clips; stage 2 is then driven ~25× past
//!   its own knee, so its output is a near-square wave. That is the wall of
//!   sustain, and it is why the Muff compresses long after a one-stage
//!   overdrive has stopped changing.
//! - **`C12` (470 pF) across a 470 kΩ feedback resistor** puts the stage's
//!   corner at 720 Hz. *Below* the knee the stage is a 6 dB/oct lowpass;
//!   *above* it the conducting diodes drop the feedback impedance to tens of
//!   ohms and the corner runs off to megahertz. So the pedal filters what it
//!   does not clip and passes what it does — the Muff's smooth, un-fizzy top,
//!   grown from one capacitor.
//! - **The tone stack is a notch, not a tilt.** Bass and treble arrive on two
//!   separate paths that are *summed*, so at noon they partly cancel: the
//!   famous mid scoop, sliding as the knob turns. It is the `big-muff` model in
//!   [`crate::eq::tonestack`], from phase 02 — this pedal is the first to use
//!   it in anger.
//!
//! Faceplate: **Sustain / Tone / Volume**, like the hardware.

use lh_core::{EffectDesc, ParamDesc, db_to_lin};

use crate::blocks::transistor::ShuntFeedbackStage;
use crate::eq::tonestack::kind;

use super::{Circuit, OnePole, Ramp, ToneStack, knob, lp_coeff};

static PARAMS: [ParamDesc; 3] = [
    knob("sustain", "Sustain", 5.0, 20.0),
    knob("tone", "Tone", 5.0, 30.0),
    knob("volume", "Volume", 6.0, 20.0),
];

pub(super) static DESC: EffectDesc = EffectDesc {
    key: "big-muff",
    name: "Big Muff",
    params: &PARAMS,
};

// --- the clipping stage's netlist (values off the schematic) ---

/// Input coupling capacitor.
const C5: f32 = 100e-9;
/// Series input resistor, source side of the summing node.
const R19: f32 = 10e3;
/// Bias resistor from the summing node to (AC) ground.
const R20: f32 = 100e3;
/// Collector and emitter resistors: the linearised common-emitter gain is
/// `−Rc/Re`. Unbypassed, which is what keeps it a modest 67 rather than the
/// hundreds a bypassed stage would give.
const RC: f32 = 10e3;
const RE: f32 = 150.0;
/// Feedback resistor — the diodes clip *across* it.
const R17: f32 = 470e3;
/// The smoothing capacitor across the feedback resistor. 470 pF against 470 kΩ
/// is a 720 Hz corner: the single component most responsible for the Muff being
/// dark rather than buzzy.
const C12: f32 = 470e-12;

/// 1N4148 SPICE-representative junction parameters — the family's silicon, and
/// per ADR 033 a device carries its ideality as well as its `Is`.
const IS: f32 = 2.52e-9;
const N: f32 = 1.75;
const VT: f32 = 25.85e-3;

/// Open-loop gain of one stage.
const A: f32 = -RC / RE;
/// The **AC** Thévenin resistance at the summing node. `R19 ‖ R20`, not `R20`:
/// above the input network's 14.5 Hz corner the source is a short through
/// `C5`, so the feedback current sees the two in parallel. Getting this wrong
/// costs a factor of ~6 in the stage's gain, which is exactly what
/// `the_linear_response_matches_hand_solved_ac_analysis` exists to catch.
const R_TH: f32 = R19 * R20 / (R19 + R20);
/// Open-circuit divider of the same network, above its corner.
const IN_DIV: f32 = R20 / (R19 + R20);
/// …and its corner, 14.5 Hz.
const F_IN: f32 = 1.0 / (std::f32::consts::TAU * C5 * (R19 + R20));

/// Calibrated with `default_level_survey`.
const MAKEUP: f32 = 0.195;

/// Sustain is the input gain ahead of the first clipping stage — the pedal's
/// own input booster and its Sustain pot, folded into one control. A stage
/// breaks up at ~14 mV in, so sustain 0 sits right on the knee (the Muff is
/// never truly clean) and sustain 10 slams both stages by three decades.
#[inline]
fn sustain_gain(pos: f32) -> f32 {
    db_to_lin(-20.0 + 54.0 * (pos * 0.1).powf(1.5))
}

pub(super) struct BigMuff {
    stages: [ShuntFeedbackStage; 2],
    /// Each stage's input network, as the high-pass its Thévenin voltage is.
    hp: [OnePole; 2],
    c_in: f32,
    tone: ToneStack,
    dc: OnePole,
    c_dc: f32,
}

impl BigMuff {
    pub(super) fn new() -> Self {
        let stage = || ShuntFeedbackStage::new(A, R_TH, R17, C12, IS, N, VT);
        Self {
            stages: [stage(), stage()],
            hp: [OnePole::default(), OnePole::default()],
            c_in: 0.0,
            tone: ToneStack::new(kind::BIG_MUFF),
            dc: OnePole::default(),
            c_dc: 0.0,
        }
    }

    /// The clipping section: two identical stages, each fed the Thévenin
    /// voltage of its own input network. Both invert, so the pair is in phase
    /// with the input.
    #[inline]
    fn core(&mut self, x: f32) -> f32 {
        let mut v = x;
        for k in 0..2 {
            let u = IN_DIV * (v - self.hp[k].lp(v, self.c_in));
            v = self.stages[k].process(u);
        }
        v
    }
}

impl Circuit for BigMuff {
    fn prepare(&mut self, base_rate: f32, os_rate: f32) {
        self.c_in = lp_coeff(F_IN, os_rate);
        self.c_dc = lp_coeff(16.0, base_rate);
        for s in &mut self.stages {
            s.prepare(os_rate);
        }
        self.tone.prepare(base_rate);
        self.reset();
    }

    fn reset(&mut self) {
        for s in &mut self.stages {
            s.reset();
        }
        for h in &mut self.hp {
            h.reset();
        }
        self.tone.reset();
        self.dc.reset();
    }

    fn shape(&mut self, block: &mut [f32], drive: &[f32]) {
        let mut gain = Ramp::over(drive, sustain_gain);
        for s in block.iter_mut() {
            *s = self.core(gain.tick() * *s);
        }
    }

    fn post(&mut self, block: &mut [f32], tone: &[f32]) {
        // The Muff's stack has one control, wired to the family's treble knob
        // (`knob_mask`), so the same trajectory goes to all three slots — the
        // two it does not use never move relative to it, which keeps the
        // settled-skip working.
        self.tone.process(block, tone, tone, tone);
        for s in block.iter_mut() {
            let y = *s * MAKEUP;
            *s = y - self.dc.lp(y, self.c_dc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OS: f32 = 4.0 * 48_000.0;

    fn prepared() -> BigMuff {
        let mut p = BigMuff::new();
        p.prepare(48_000.0, OS);
        p
    }

    /// Run a sine through the clipping section only (no sustain gain, no tone
    /// stack), returning the settled second half.
    fn run(p: &mut BigMuff, amp: f32, f: f32, n: usize) -> Vec<f32> {
        let mut buf: Vec<f32> = (0..n)
            .map(|k| amp * (std::f32::consts::TAU * f * k as f32 / OS).sin())
            .collect();
        for s in buf.iter_mut() {
            *s = p.core(*s);
        }
        buf.split_off(n / 2)
    }

    fn mag_at(buf: &[f32], f: f32) -> f64 {
        let n = buf.len() as f64;
        let (mut cs, mut cc) = (0.0f64, 0.0f64);
        for (i, s) in buf.iter().enumerate() {
            let ph = 2.0 * std::f64::consts::PI * f64::from(f) * i as f64 / f64::from(OS);
            cs += f64::from(*s) * ph.sin();
            cc += f64::from(*s) * ph.cos();
        }
        2.0 * (cs * cs + cc * cc).sqrt() / n
    }

    /// Fraction of a buffer's energy that is *not* at the fundamental.
    fn harmonic_frac(buf: &[f32], f: f32) -> f64 {
        let n = buf.len() as f64;
        let fund = mag_at(buf, f) / 2f64.sqrt();
        let total = (buf.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / n).sqrt();
        (total.powi(2) - fund.powi(2)).max(0.0).sqrt() / total
    }

    // --- the analog reference ---

    type C = (f64, f64);
    fn cdiv(a: C, b: C) -> C {
        let d = b.0 * b.0 + b.1 * b.1;
        ((a.0 * b.0 + a.1 * b.1) / d, (a.1 * b.0 - a.0 * b.1) / d)
    }
    fn cabs(a: C) -> f64 {
        a.0.hypot(a.1)
    }

    fn prewarp(f: f32) -> f64 {
        2.0 * f64::from(OS) * (std::f64::consts::PI * f64::from(f) / f64::from(OS)).tan()
    }

    /// |H(jω)| of **one** analog clipping stage, hand-solved from the netlist.
    ///
    /// Below the knee the diodes are not gone — an antiparallel pair has a
    /// finite zero-bias resistance `nVt/2Is` (9 MΩ here) that sits across the
    /// 470 kΩ and takes 3 % off the gain. Everything else is the textbook
    /// finite-gain inverting amplifier:
    ///
    /// ```text
    /// H_in(jω) = jωC5·R20 / (1 + jωC5(R19+R20))      the input network
    /// Y_f      = 1/R17 + 1/R_d + jωC12               the feedback admittance
    /// G(jω)    = 1 / (1/A − κ·R_th·Y_f),   κ = 1−1/A
    /// ```
    ///
    /// The whole pedal is two of these in cascade. Nothing here is shared with
    /// the implementation but the component values.
    fn analog_stage(w: f64) -> C {
        let (r19, r20, c5) = (f64::from(R19), f64::from(R20), f64::from(C5));
        let h_in = cdiv((0.0, w * c5 * r20), (1.0, w * c5 * (r19 + r20)));

        let r_d = f64::from(N * VT) / (2.0 * f64::from(IS));
        let y_f = (1.0 / f64::from(R17) + 1.0 / r_d, w * f64::from(C12));
        let a = f64::from(A);
        let kappa = 1.0 - 1.0 / a;
        let r_th = f64::from(R_TH);
        let denom = (1.0 / a - kappa * r_th * y_f.0, -kappa * r_th * y_f.1);
        let g = cdiv((1.0, 0.0), denom);
        (h_in.0 * g.0 - h_in.1 * g.1, h_in.0 * g.1 + h_in.1 * g.0)
    }

    /// **The independent check on the whole circuit.** Below the diodes' knee
    /// both stages are linear, so the measured response must match hand-solved
    /// AC analysis of the same netlist, squared.
    ///
    /// This is also what pins the correction this pedal makes to its reference
    /// implementation: injecting the feedback current across `R20` (100 kΩ, the
    /// *DC* Thévenin resistance) instead of `R19 ‖ R20` (9.1 kΩ, the AC one)
    /// would land 6× low here and nowhere near the tolerance.
    #[test]
    fn the_linear_response_matches_hand_solved_ac_analysis() {
        // Small enough that even the second stage's output stays two decades
        // under the knee, so nothing has bent.
        const AMP: f32 = 1e-5;
        for f in [40.0f32, 120.0, 400.0, 1_000.0, 4_000.0, 10_000.0] {
            let mut p = prepared();
            let y = run(&mut p, AMP, f, 1 << 17);
            let got = mag_at(&y, f) / f64::from(AMP);
            let one = cabs(analog_stage(prewarp(f)));
            let want = one * one;
            let err = (got - want).abs() / want;
            assert!(
                err < 0.03,
                "{f} Hz: model {got:.4} vs analog {want:.4} ({:.2} %)",
                err * 100.0
            );
        }
    }

    /// The gain the two-stage cascade actually delivers, stated once so a
    /// component-value slip shows up as a number and not as a mood. ~650×
    /// (56 dB) is why a Muff needs no booster in front of it.
    #[test]
    fn the_cascade_gain_is_what_the_components_say() {
        let one = cabs(analog_stage(prewarp(300.0)));
        assert!(
            (600.0..700.0).contains(&(one * one)),
            "two stages give {:.0}×",
            one * one
        );
    }

    /// **The white-box discriminator.** `C12` sits across the feedback resistor,
    /// so the stage's gain rolls off above 720 Hz *while the diodes are off* —
    /// and once they conduct the feedback impedance collapses and the roll-off
    /// leaves. The audible consequence is that at one input level a low note is
    /// driven into the diodes and a high note is filtered before it gets there.
    /// A memoryless clipper cannot do this in either direction.
    #[test]
    fn the_smoothing_cap_makes_break_up_frequency_dependent() {
        // One amplitude, two frequencies either side of the 720 Hz corner.
        let low = harmonic_frac(&run(&mut prepared(), 3e-3, 120.0, 1 << 15), 120.0);
        let high = harmonic_frac(&run(&mut prepared(), 3e-3, 4_000.0, 1 << 15), 4_000.0);
        assert!(
            low > high * 1.2,
            "C12 must protect the highs: 120 Hz {low:.3} vs 4 kHz {high:.3}"
        );
    }

    /// Two stages, not one: the second is driven ~25× past its own knee, so the
    /// pair compresses far harder than a single stage does. Measured as the
    /// output level's refusal to follow a 12 dB input cut.
    #[test]
    fn the_second_stage_is_what_makes_it_a_wall() {
        let loud = run(&mut prepared(), 0.1, 220.0, 1 << 14);
        let quiet = run(&mut prepared(), 0.025, 220.0, 1 << 14);
        let ratio = f64::from(crate::testutil::rms(&quiet) / crate::testutil::rms(&loud));
        assert!(
            ratio > 0.85,
            "12 dB in must barely move the output (linear is 0.25), got {ratio:.3}"
        );
    }

    /// Silence in, exact silence out: `y = 0` is the fixed point of both stages'
    /// node equations, and no bias offset is modelled to spoil it.
    #[test]
    fn core_silence_in_silence_out() {
        let mut p = prepared();
        for _ in 0..1000 {
            assert_eq!(p.core(0.0), 0.0);
        }
    }

    /// RT rule 7 at the pedal: a slammed, alternating input stays finite and
    /// clamped by the diodes, cold.
    #[test]
    fn core_bounded_when_slammed() {
        let mut p = prepared();
        for k in 0..2000 {
            let x = if k % 2 == 0 { 1.0e6 } else { -1.0e6 };
            let y = p.core(x);
            assert!(y.is_finite() && y.abs() < 2.0, "k={k}: y={y}");
        }
    }
}
