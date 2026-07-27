//! **red-charlie** — Marshall JCM800 (2203)-style distortion, wearing the
//! house nickname. Two cascaded gain stages are the sound: a warm first
//! triode that clips softly, then the famous cold-biased clipper that
//! shears the negative swing far earlier than the positive — that
//! asymmetric cascade is the crunch. A 470 pF bright cap across the preamp
//! pot keeps low-gain settings cutting (its emphasis fades as the pot
//! rises and shorts it out); the first stage's cathode network and the
//! interstage coupling both thin the lows *before* the hot stage, which is
//! why palm mutes stay tight instead of flubbing out. Bass/Middle/Treble run
//! the amp's own **JCM800 passive tone stack** (PRD 023) — 33k slope, 470 pF
//! treble cap, 220k/1M/22k pots — so the three knobs interact and noon keeps
//! the Marshall mid scoop instead of sitting flat; Master is the output pot.

use lh_core::{EffectDesc, ParamDesc, db_to_lin};

use crate::blocks::waveshaper::{Adaa1, asym_tanh, asym_tanh_f1};
use crate::eq::tonestack::kind;

use super::{Circuit, OnePole, Ramp, ToneStack, knob, lp_coeff};

static PARAMS: [ParamDesc; 5] = [
    knob("gain", "Gain", 5.0, 20.0),
    knob("bass", "Bass", 5.0, 30.0),
    knob("middle", "Middle", 5.0, 30.0),
    knob("treble", "Treble", 5.0, 30.0),
    knob("master", "Master", 6.0, 20.0),
];

pub(super) static DESC: EffectDesc = EffectDesc {
    key: "red-charlie",
    name: "Red Charlie",
    params: &PARAMS,
};

/// Stage-1 knees: a warm triode — grid conduction clips the top a little
/// sooner than the bottom.
const STAGE1_KNEE_POS: f32 = 0.7;
const STAGE1_KNEE_NEG: f32 = 1.0;
/// Stage-2 knees: the 2203's cold clipper — biased near cutoff, the
/// negative swing shears far earlier than the positive.
const STAGE2_KNEE_POS: f32 = 1.0;
const STAGE2_KNEE_NEG: f32 = 0.4;
/// Fixed second-stage gain (+12 dB) on top of the swept first stage.
const STAGE2_GAIN: f32 = 4.0;
/// Bright-cap treble emphasis above 1.6 kHz with the pot at 0; the pot
/// progressively shorts the cap, so the emphasis is gone at 10.
const BRIGHT_MAX: f32 = 0.9;
/// The V1a cathode network: lows below ~100 Hz see ~8 dB less stage-1 gain.
const LOW_TRIM: f32 = 0.6;
/// Calibrated with `modelled_pedals_sit_near_unity_at_default_knobs`
/// (re-trimmed after the solo-pot gain extension made noon hotter).
const MAKEUP: f32 = 0.18;

/// Anti-aliased clipping (PRD 024). A `tanh` knee aliases far less than a hard
/// corner, but at high gain it approaches one — first-order ADAA, since
/// `tanh`'s second antiderivative is not elementary.
pub(super) struct RedCharlie {
    stage1: Adaa1,
    stage2: Adaa1,
    hp_in: OnePole,
    lf_trim: OnePole,
    bright: OnePole,
    couple: OnePole,
    dc_os: OnePole,
    stack: ToneStack,
    c35: f32,
    c100: f32,
    c1600: f32,
    c120: f32,
    c12: f32,
}

impl RedCharlie {
    pub(super) fn new() -> Self {
        Self {
            stage1: Adaa1::new(),
            stage2: Adaa1::new(),
            hp_in: OnePole::default(),
            lf_trim: OnePole::default(),
            bright: OnePole::default(),
            couple: OnePole::default(),
            dc_os: OnePole::default(),
            stack: ToneStack::new(kind::JCM800),
            c35: 0.0,
            c100: 0.0,
            c1600: 0.0,
            c120: 0.0,
            c12: 0.0,
        }
    }

    /// Asymmetric tanh clipper with independent knees per polarity, run
    /// through the given stage's ADAA state.
    #[inline]
    fn clip(adaa: &mut Adaa1, v: f32, knee_pos: f32, knee_neg: f32) -> f32 {
        adaa.process(
            v,
            |u| asym_tanh(u, knee_pos as f64, knee_neg as f64),
            |u| asym_tanh_f1(u, knee_pos as f64, knee_neg as f64),
        )
    }
}

impl Circuit for RedCharlie {
    fn prepare(&mut self, base_rate: f32, os_rate: f32) {
        self.c35 = lp_coeff(35.0, os_rate);
        self.c100 = lp_coeff(100.0, os_rate);
        self.c1600 = lp_coeff(1_600.0, os_rate);
        self.c120 = lp_coeff(120.0, os_rate);
        self.c12 = lp_coeff(12.0, os_rate);
        self.stack.prepare(base_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.stage1.reset();
        self.stage2.reset();
        self.hp_in.reset();
        self.lf_trim.reset();
        self.bright.reset();
        self.couple.reset();
        self.dc_os.reset();
        self.stack.reset();
    }

    fn shape(&mut self, block: &mut [f32], drive: &[f32]) {
        // Preamp volume: +8 dB (edge of breakup) to +56 dB — the top ~12 dB
        // beyond the stock pot is the "screamer in front" solo reach, by
        // request; the audio taper keeps positions 0..4 in stock crunch
        // territory. powf twice per chunk, ramped per sample.
        let mut gain = Ramp::over(drive, |d| db_to_lin(8.0 + 48.0 * (d * 0.1).powf(1.5)));
        for (s, d) in block.iter_mut().zip(drive) {
            let x = *s;
            // Subsonic block at 35 Hz.
            let x = x - self.hp_in.lp(x, self.c35);
            // V1a cathode network: shelve the lows down before any gain.
            let x = x - LOW_TRIM * self.lf_trim.lp(x, self.c100);
            // Bright cap across the preamp pot — strongest with the pot down.
            let bright = BRIGHT_MAX * (1.0 - d * 0.1);
            let x = x + bright * (x - self.bright.lp(x, self.c1600));
            // Stage 1: warm asymmetric soft clip.
            let s1 = Self::clip(
                &mut self.stage1,
                gain.tick() * x,
                STAGE1_KNEE_POS,
                STAGE1_KNEE_NEG,
            );
            // Interstage coupling thins the lows into the hot stage.
            let s1 = s1 - self.couple.lp(s1, self.c120);
            // Stage 2: the cold clipper (the real stage inverts; opposite
            // knee polarity models the flip).
            let s2 = Self::clip(
                &mut self.stage2,
                STAGE2_GAIN * s1,
                STAGE2_KNEE_POS,
                STAGE2_KNEE_NEG,
            );
            *s = s2 - self.dc_os.lp(s2, self.c12);
        }
    }

    fn post(&mut self, block: &mut [f32], _tone: &[f32]) {
        // No single tone knob — tone shaping lives in `eq`; `post` only
        // applies the output makeup.
        for s in block.iter_mut() {
            *s *= MAKEUP;
        }
    }

    fn eq(&mut self, block: &mut [f32], low: &[f32], mid: &[f32], high: &[f32]) {
        // The 2203's own passive stack.
        self.stack.process(block, low, mid, high);
    }
}
