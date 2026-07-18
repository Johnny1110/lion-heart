//! Global output parametric EQ (PRD 003): 8 bands of RBJ biquads on the
//! engine's **output stage** — after the chain, before the safety limiter.
//! Not a chain slot (the `eq` pedal stays tone shaping); this is
//! environment correction, driven by [`lh_core::global_eq`] state.
//!
//! Click-freeness: freq (log-domain), gain, and Q ride smoothers with
//! block-rate coefficient rebuilds (the chain EQ's proven pattern), and
//! every band has a wet crossfade so enabling/disabling engages smoothly —
//! for cut filters too, where there is no gain to ramp. A master wet does
//! the same for the whole stage. With everything settled off, `process`
//! early-outs and the stage is bit-transparent.

use lh_core::global_eq::{Band, BandKind, GlobalEqState, Q_MIN};

use crate::blocks::biquad::Biquad;
use crate::blocks::smooth::Smoothed;

/// Engage/disengage crossfade per band and for the master toggle.
const WET_MS: f32 = 15.0;
/// Control smoothing for freq/gain/Q moves (block-rate rebuilds).
const CTRL_MS: f32 = 30.0;

/// Configure `filter`'s coefficients for `band` (state preserved).
fn set_coeffs(filter: &mut Biquad, sample_rate: f32, kind: BandKind, freq: f32, gain: f32, q: f32) {
    match kind {
        BandKind::LowCut => filter.set_highpass(sample_rate, freq, q),
        BandKind::LowShelf => filter.set_low_shelf(sample_rate, freq, gain),
        BandKind::Bell => filter.set_peaking(sample_rate, freq, gain, q),
        BandKind::HighShelf => filter.set_high_shelf(sample_rate, freq, gain),
        BandKind::HighCut => filter.set_lowpass(sample_rate, freq, q),
    }
}

/// Combined response of `state` at `freq` in dB — computed from the same
/// RBJ math the audio path runs, so the GUI curve *is* the truth.
pub fn response_db(state: &GlobalEqState, sample_rate: f32, freq: f32) -> f32 {
    if !state.enabled {
        return 0.0;
    }
    let mut total = 0.0;
    let mut probe = Biquad::default();
    for band in &state.bands {
        if !band.enabled {
            continue;
        }
        let b = band.clamped();
        set_coeffs(&mut probe, sample_rate, b.kind, b.freq, b.gain_db, b.q);
        total += probe.magnitude_db(sample_rate, freq);
    }
    total
}

struct BandRuntime {
    /// Target state (kind and enabled are authoritative here; the smoothed
    /// values below chase freq/gain/q).
    band: Band,
    /// 0..1 engage crossfade.
    wet: Smoothed,
    /// Smoothed in the log domain so drags glide musically.
    freq_log2: Smoothed,
    gain_db: Smoothed,
    q: Smoothed,
    filters: [Biquad; 2],
}

impl BandRuntime {
    fn new(band: Band) -> Self {
        Self {
            band,
            wet: Smoothed::new(if band.enabled { 1.0 } else { 0.0 }),
            freq_log2: Smoothed::new(band.freq.log2()),
            gain_db: Smoothed::new(band.gain_db),
            q: Smoothed::new(band.q),
            filters: [Biquad::default(); 2],
        }
    }

    /// Still audible: engaged, or fading out.
    fn engaged(&self) -> bool {
        !(self.wet.is_settled() && self.wet.target() == 0.0)
    }
}

pub struct GlobalEq {
    sample_rate: f32,
    /// Master engage crossfade (0 = stage bypassed).
    master: Smoothed,
    bands: Vec<BandRuntime>,
}

impl Default for GlobalEq {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalEq {
    pub fn new() -> Self {
        let state = GlobalEqState::default();
        Self {
            sample_rate: 48_000.0,
            master: Smoothed::new(if state.enabled { 1.0 } else { 0.0 }),
            bands: state.bands.iter().copied().map(BandRuntime::new).collect(),
        }
    }

    /// Off the audio thread: configure smoothers and snap to targets.
    pub fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate as f32;
        self.master.configure(WET_MS, sample_rate);
        self.master.snap_to_target();
        for band in &mut self.bands {
            band.wet.configure(WET_MS, sample_rate);
            band.freq_log2.configure(CTRL_MS, sample_rate);
            band.gain_db.configure(CTRL_MS, sample_rate);
            band.q.configure(CTRL_MS, sample_rate);
            band.wet.snap_to_target();
            band.freq_log2.snap_to_target();
            band.gain_db.snap_to_target();
            band.q.snap_to_target();
            for filter in &mut band.filters {
                filter.reset();
            }
        }
        self.rebuild_coeffs(0);
    }

    /// Clear filter memories. RT-safe.
    pub fn reset(&mut self) {
        for band in &mut self.bands {
            for filter in &mut band.filters {
                filter.reset();
            }
        }
    }

    /// Update one band from a control message. RT-safe.
    pub fn set_band(&mut self, index: usize, band: Band) {
        let Some(rt) = self.bands.get_mut(index) else {
            return;
        };
        let band = band.clamped();
        let was_off = rt.wet.is_settled() && rt.wet.target() == 0.0;
        rt.band = band;
        rt.freq_log2.set_target(band.freq.log2());
        rt.gain_db.set_target(band.gain_db);
        rt.q.set_target(band.q);
        rt.wet.set_target(if band.enabled { 1.0 } else { 0.0 });
        if band.enabled && was_off {
            // Fresh engage: clean filters at the target values — no sweep
            // from stale settings, no stale ringing; the wet ramp does the
            // fade-in.
            rt.freq_log2.snap_to_target();
            rt.gain_db.snap_to_target();
            rt.q.snap_to_target();
            for filter in &mut rt.filters {
                filter.reset();
            }
        }
    }

    /// Master toggle (crossfaded). RT-safe.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.master.set_target(if enabled { 1.0 } else { 0.0 });
    }

    /// Advance control smoothers by `n` samples and rebuild coefficients
    /// for engaged bands (block rate, like the chain EQ slot).
    fn rebuild_coeffs(&mut self, n: usize) {
        let sample_rate = self.sample_rate;
        for band in &mut self.bands {
            if !band.engaged() {
                continue;
            }
            for _ in 0..n {
                band.freq_log2.tick();
                band.gain_db.tick();
                band.q.tick();
            }
            let freq = band.freq_log2.current().exp2();
            let gain = band.gain_db.current();
            let q = band.q.current().max(Q_MIN);
            for filter in &mut band.filters {
                set_coeffs(filter, sample_rate, band.band.kind, freq, gain, q);
            }
        }
    }

    /// In-place stereo processing. RT-safe; bit-transparent when settled off.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        // A fade-out below -60 dB is inaudible: snap it so the transparent
        // fast-paths engage instead of blending forever (engine pattern).
        if self.master.target() == 0.0 && self.master.current() <= 1e-3 {
            self.master.snap_to_target();
        }
        for band in &mut self.bands {
            if band.wet.target() == 0.0 && band.wet.current() <= 1e-3 {
                band.wet.snap_to_target();
            }
        }

        let master_off = self.master.is_settled() && self.master.target() == 0.0;
        let any_band = self.bands.iter().any(|b| b.engaged());
        if master_off || (self.master.is_settled() && !any_band) {
            return;
        }

        self.rebuild_coeffs(left.len());

        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (dry_l, dry_r) = (*l, *r);
            let (mut wl, mut wr) = (dry_l, dry_r);
            for band in &mut self.bands {
                if !band.engaged() {
                    continue;
                }
                let w = band.wet.tick();
                let fl = band.filters[0].process_sample(wl);
                let fr = band.filters[1].process_sample(wr);
                wl += w * (fl - wl);
                wr += w * (fr - wr);
            }
            let m = self.master.tick();
            *l = dry_l + m * (wl - dry_l);
            *r = dry_r + m * (wr - dry_r);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{assert_finite, rms, sine};

    const SR: u32 = 48_000;

    fn prepared(state: &GlobalEqState) -> GlobalEq {
        let mut eq = GlobalEq::new();
        eq.prepare(SR);
        eq.set_enabled(state.enabled);
        for (i, band) in state.bands.iter().enumerate() {
            eq.set_band(i, *band);
        }
        eq
    }

    fn enabled_bell(freq: f32, gain_db: f32, q: f32) -> GlobalEqState {
        let mut state = GlobalEqState::default();
        state.bands[2] = Band {
            enabled: true,
            kind: BandKind::Bell,
            freq,
            gain_db,
            q,
        };
        state
    }

    /// Steady-state gain (dB) at `freq` through the whole stage.
    fn gain_at(eq: &mut GlobalEq, freq: f32) -> f32 {
        let x = sine(SR, freq, SR as usize / 2);
        let mut l = x.clone();
        let mut r = x.clone();
        for (cl, cr) in l.chunks_mut(64).zip(r.chunks_mut(64)) {
            eq.process(cl, cr);
        }
        assert_finite("eq output", &l);
        let n = l.len();
        lh_core::lin_to_db(rms(&l[n / 2..]) / rms(&x[n / 2..]))
    }

    #[test]
    fn all_disabled_is_bit_transparent() {
        let mut eq = prepared(&GlobalEqState::default());
        let x = sine(SR, 220.0, 4_096);
        let mut l = x.clone();
        let mut r = x.clone();
        eq.process(&mut l, &mut r);
        assert_eq!(x, l, "disabled bands must not touch the signal");
        assert_eq!(x, r);
    }

    #[test]
    fn master_off_is_bit_transparent_after_settling() {
        let mut state = enabled_bell(1_000.0, 12.0, 1.0);
        state.enabled = false;
        let mut eq = prepared(&state);
        // Let the master crossfade settle (prepare snapped it, but the
        // set_enabled(false) after a default-on new() needs to decay).
        let warm = sine(SR, 220.0, SR as usize / 4);
        let mut wl = warm.clone();
        let mut wr = warm.clone();
        eq.process(&mut wl, &mut wr);

        let x = sine(SR, 220.0, 4_096);
        let mut l = x.clone();
        let mut r = x.clone();
        eq.process(&mut l, &mut r);
        assert_eq!(x, l, "master off must be bit-exact");
    }

    #[test]
    fn bell_boosts_at_center_only() {
        let mut eq = prepared(&enabled_bell(800.0, 9.0, 0.9));
        assert!((gain_at(&mut eq, 800.0) - 9.0).abs() < 0.5);
        eq.reset();
        assert!(gain_at(&mut eq, 60.0).abs() < 1.0);
        eq.reset();
        assert!(gain_at(&mut eq, 10_000.0).abs() < 1.0);
    }

    #[test]
    fn every_kind_shapes_its_range() {
        let case = |kind, freq, gain_db, q, probe: f32, expect_db: f32, tol: f32| {
            let mut state = GlobalEqState::default();
            state.bands[3] = Band {
                enabled: true,
                kind,
                freq,
                gain_db,
                q,
            };
            let mut eq = prepared(&state);
            let got = gain_at(&mut eq, probe);
            assert!(
                (got - expect_db).abs() < tol,
                "{kind:?} at {probe} Hz: expected {expect_db} dB, got {got:.2}"
            );
        };
        case(BandKind::LowCut, 200.0, 0.0, 0.707, 50.0, -24.0, 4.0);
        case(BandKind::LowShelf, 150.0, 8.0, 0.707, 40.0, 8.0, 1.0);
        case(
            BandKind::HighShelf,
            3_000.0,
            -9.0,
            0.707,
            12_000.0,
            -9.0,
            1.5,
        );
        // Two octaves above a 12 dB/oct lowpass ≈ -24 dB, a touch more
        // from the bilinear warp toward Nyquist.
        case(BandKind::HighCut, 2_000.0, 0.0, 0.707, 8_000.0, -26.0, 6.0);
    }

    #[test]
    fn response_curve_matches_rendered_audio() {
        let state = enabled_bell(1_200.0, -7.0, 1.4);
        let mut eq = prepared(&state);
        for freq in [200.0, 1_200.0, 6_000.0] {
            let analytic = response_db(&state, SR as f32, freq);
            let rendered = gain_at(&mut eq, freq);
            eq.reset();
            assert!(
                (analytic - rendered).abs() < 0.5,
                "{freq} Hz: curve {analytic:.2} vs audio {rendered:.2}"
            );
        }
    }

    #[test]
    fn engage_and_disengage_are_click_free() {
        let mut state = GlobalEqState::default();
        let mut eq = prepared(&state);
        let x = sine(SR, 220.0, SR as usize);
        let mut l = x.clone();
        let mut r = x.clone();
        for (i, (cl, cr)) in l.chunks_mut(64).zip(r.chunks_mut(64)).enumerate() {
            if i == 100 {
                state.bands[2] = Band {
                    enabled: true,
                    kind: BandKind::Bell,
                    freq: 500.0,
                    gain_db: 15.0,
                    q: 2.0,
                };
                eq.set_band(2, state.bands[2]);
            }
            if i == 400 {
                state.bands[2].enabled = false;
                eq.set_band(2, state.bands[2]);
            }
            eq.process(cl, cr);
        }
        assert_finite("engage sweep", &l);
        let max_step = l
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(max_step < 0.25, "click detected, step {max_step}");
    }

    #[test]
    fn silence_in_silence_out_and_studio_rates() {
        for sr in [44_100u32, 48_000, 96_000] {
            let mut eq = GlobalEq::new();
            eq.prepare(sr);
            eq.set_band(
                2,
                Band {
                    enabled: true,
                    kind: BandKind::Bell,
                    freq: 1_000.0,
                    gain_db: 6.0,
                    q: 1.0,
                },
            );
            for chunk in [32usize, 483, 1_024] {
                let x = sine(sr, 440.0, 4_096);
                let mut l = x.clone();
                let mut r = x.clone();
                for (cl, cr) in l.chunks_mut(chunk).zip(r.chunks_mut(chunk)) {
                    eq.process(cl, cr);
                }
                assert_finite("eq multirate", &l);
            }
            // Silence-in → silence-out after reset (the sine run above
            // left a legitimate IIR tail in the filters).
            eq.reset();
            let mut l = vec![0.0f32; 8_192];
            let mut r = vec![0.0f32; 8_192];
            for (cl, cr) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
                eq.process(cl, cr);
            }
            assert!(rms(&l) == 0.0 && rms(&r) == 0.0);
        }
    }
}
