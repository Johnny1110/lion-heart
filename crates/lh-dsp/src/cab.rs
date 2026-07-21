//! Cabinet simulator: zero-latency partitioned FFT convolution with a loaded
//! impulse response. The IR is prepared off-thread (see `lh-assets`) and
//! swapped in through an [`AssetSlot`].
//!
//! **Dual IR / mic blend** (ADR 015): a cab can hold two IRs — a primary `a`
//! and an optional blend partner `b` (a second mic / cabinet) — and the
//! `blend` knob crossfades between them (0 = all `a`, 1 = all `b`). The two
//! IRs are correlated (same source), so the crossfade is **linear**: low end
//! stays put while the mic difference (mostly the top and the comb between
//! them) sweeps. Both convolvers live in one [`IrAsset`], swapped atomically;
//! the control side composes it (it owns both IR files — see `lh-assets` /
//! the session), so this stays one asset handle and one hot-swap.

use fft_convolver::FFTConvolver;
use lh_core::{EffectDesc, FamilyDesc, ParamDesc, Range, db_to_lin};

use crate::Effect;
use crate::blocks::smooth::Smoothed;
use crate::blocks::swap::{AssetHandle, AssetSlot, asset_channel};

static PARAMS: [ParamDesc; 2] = [
    ParamDesc {
        key: "level",
        name: "Level",
        unit: "dB",
        range: Range::Linear {
            min: -12.0,
            max: 12.0,
        },
        default: 0.0,
        smoothing_ms: 20.0,
    },
    // Mic blend a⇄b (ADR 015); a no-op when no second IR is loaded. Appended
    // after `level` so old cab presets (one param) migrate untouched.
    ParamDesc {
        key: "blend",
        name: "Blend",
        unit: "",
        range: Range::Linear { min: 0.0, max: 1.0 },
        default: 0.0, // all `a`
        smoothing_ms: 20.0,
    },
];

pub static DESC: EffectDesc = EffectDesc {
    key: "cab",
    name: "Cab IR",
    params: &PARAMS,
};

/// Single-pedal family: the pedal key doubles as the family key.
pub static FAMILY: FamilyDesc = FamilyDesc {
    key: "cab",
    name: "Cab IR",
    pedals: &[&DESC],
};

/// One IR's stereo convolver pair (one per channel of the bus, same IR),
/// built and `init`-ed off the audio thread.
pub struct IrPair {
    pub left: FFTConvolver<f32>,
    pub right: FFTConvolver<f32>,
}

impl IrPair {
    pub fn reset(&mut self) {
        self.left.reset();
        self.right.reset();
    }
}

/// A ready-to-run cabinet: the primary IR `a` and an optional blend partner
/// `b` (a second mic). Composed on a worker thread and swapped in atomically.
pub struct IrAsset {
    pub a: IrPair,
    pub b: Option<IrPair>,
}

const SCRATCH: usize = 1024;

pub struct CabIr {
    slot: AssetSlot<IrAsset>,
    level: Smoothed,
    blend: Smoothed,
    // Per-block scratch (allocated in `prepare`, never on the audio thread):
    // one dry input copy, the two wet IR outputs, and the smoothed control
    // trajectories shared by both channels.
    dry: Vec<f32>,
    wet_a: Vec<f32>,
    wet_b: Vec<f32>,
    blend_buf: Vec<f32>,
    level_buf: Vec<f32>,
}

impl CabIr {
    pub fn new() -> (Self, AssetHandle<IrAsset>) {
        let (slot, handle) = asset_channel();
        (
            Self {
                slot,
                level: Smoothed::new(db_to_lin(PARAMS[0].default)),
                blend: Smoothed::new(PARAMS[1].default),
                dry: Vec::new(),
                wet_a: Vec::new(),
                wet_b: Vec::new(),
                blend_buf: Vec::new(),
                level_buf: Vec::new(),
            },
            handle,
        )
    }
}

impl Effect for CabIr {
    fn family(&self) -> &'static FamilyDesc {
        &FAMILY
    }

    fn prepare(&mut self, sample_rate: u32) {
        self.level.configure(PARAMS[0].smoothing_ms, sample_rate);
        self.level.snap_to_target();
        self.blend.configure(PARAMS[1].smoothing_ms, sample_rate);
        self.blend.snap_to_target();
        self.dry = vec![0.0; SCRATCH];
        self.wet_a = vec![0.0; SCRATCH];
        self.wet_b = vec![0.0; SCRATCH];
        self.blend_buf = vec![0.0; SCRATCH];
        self.level_buf = vec![0.0; SCRATCH];
    }

    fn reset(&mut self) {
        if let Some(asset) = self.slot.get_mut() {
            asset.a.reset();
            if let Some(b) = &mut asset.b {
                b.reset();
            }
        }
    }

    fn set_param(&mut self, index: usize, normalized: f32) {
        match index {
            0 => self
                .level
                .set_target(db_to_lin(PARAMS[0].range.to_real(normalized))),
            1 => self.blend.set_target(PARAMS[1].range.to_real(normalized)),
            _ => {}
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.slot.tick();
        let Some(asset) = self.slot.get_mut() else {
            // No IR: still advance the smoothers so a later install picks up
            // the current knob positions, but pass the signal untouched.
            for _ in left.iter() {
                self.level.tick();
                self.blend.tick();
            }
            return;
        };
        for (lc, rc) in left.chunks_mut(SCRATCH).zip(right.chunks_mut(SCRATCH)) {
            let len = lc.len();
            // Snapshot the control trajectories once so both channels move in
            // lockstep (a per-channel re-tick would desync L and R).
            for i in 0..len {
                self.blend_buf[i] = self.blend.tick();
                self.level_buf[i] = self.level.tick();
            }
            let blend = &self.blend_buf[..len];
            let level = &self.level_buf[..len];
            // Each channel: convolve through `a` (and `b`), crossfade, scale.
            // Done as two sequential calls (not an array of both) so `asset.b`
            // is borrowed once at a time.
            convolve_channel(
                lc,
                &mut asset.a.left,
                asset.b.as_mut().map(|p| &mut p.left),
                &mut self.dry,
                &mut self.wet_a,
                &mut self.wet_b,
                blend,
                level,
            );
            convolve_channel(
                rc,
                &mut asset.a.right,
                asset.b.as_mut().map(|p| &mut p.right),
                &mut self.dry,
                &mut self.wet_a,
                &mut self.wet_b,
                blend,
                level,
            );
        }
    }
}

/// One channel through the primary IR (and, if present, the blend IR),
/// crossfaded by `blend` and scaled by `level` (both per-sample, shared across
/// the stereo pair). All buffers are caller-owned scratch — no allocation.
#[allow(clippy::too_many_arguments)]
fn convolve_channel(
    chunk: &mut [f32],
    a_conv: &mut FFTConvolver<f32>,
    b_conv: Option<&mut FFTConvolver<f32>>,
    dry: &mut [f32],
    wet_a: &mut [f32],
    wet_b: &mut [f32],
    blend: &[f32],
    level: &[f32],
) {
    let len = chunk.len();
    let dry = &mut dry[..len];
    dry.copy_from_slice(chunk);
    let wet_a = &mut wet_a[..len];
    if a_conv.process(dry, wet_a).is_err() {
        // Fail safe: an unusable convolver must not mute the rig.
        wet_a.copy_from_slice(dry);
    }
    match b_conv {
        Some(b_conv) => {
            let wet_b = &mut wet_b[..len];
            if b_conv.process(dry, wet_b).is_err() {
                wet_b.copy_from_slice(dry);
            }
            for i in 0..len {
                chunk[i] = (wet_a[i] * (1.0 - blend[i]) + wet_b[i] * blend[i]) * level[i];
            }
        }
        None => {
            for i in 0..len {
                chunk[i] = wet_a[i] * level[i];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{assert_finite, impulse, peak, rms};

    fn pair_from_ir(ir: &[f32]) -> IrPair {
        let build = || {
            let mut convolver = FFTConvolver::<f32>::default();
            convolver.init(128, ir).unwrap();
            convolver
        };
        IrPair {
            left: build(),
            right: build(),
        }
    }

    fn asset_from_ir(ir: &[f32]) -> Box<IrAsset> {
        Box::new(IrAsset {
            a: pair_from_ir(ir),
            b: None,
        })
    }

    fn dual_asset(a: &[f32], b: &[f32]) -> Box<IrAsset> {
        Box::new(IrAsset {
            a: pair_from_ir(a),
            b: Some(pair_from_ir(b)),
        })
    }

    #[test]
    fn unloaded_cab_is_passthrough() {
        let (mut cab, _handle) = CabIr::new();
        cab.prepare(48_000);
        let x = impulse(512, 100);
        let mut y = x.clone();
        let mut yr = x.clone();
        cab.process(&mut y, &mut yr);
        assert_eq!(x, y);
        assert_eq!(x, yr);
    }

    #[test]
    fn delta_ir_is_identity() {
        let (mut cab, mut handle) = CabIr::new();
        cab.prepare(48_000);
        handle.install(asset_from_ir(&[1.0])).unwrap();

        let x = crate::testutil::sine(48_000, 440.0, 512);
        let mut y = x.clone();
        let mut yr = x.clone();
        cab.process(&mut y, &mut yr);
        assert_eq!(y, yr, "identical channels through identical IRs");
        assert_finite("cab output", &y);
        let max_err = x
            .iter()
            .zip(&y)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(max_err < 1e-4, "delta IR must be identity, err {max_err}");
    }

    #[test]
    fn shifted_delta_delays_the_signal() {
        let (mut cab, mut handle) = CabIr::new();
        cab.prepare(48_000);
        let mut ir = vec![0.0f32; 64];
        ir[40] = 1.0;
        handle.install(asset_from_ir(&ir)).unwrap();

        let x = impulse(512, 10);
        let mut y = x.clone();
        let mut yr = x.clone();
        cab.process(&mut y, &mut yr);
        let (argmax, p) = y.iter().enumerate().fold((0, 0.0f32), |(bi, bv), (i, v)| {
            if v.abs() > bv { (i, v.abs()) } else { (bi, bv) }
        });
        assert_eq!(argmax, 50, "impulse must arrive shifted by the IR delay");
        assert!((p - 1.0).abs() < 1e-3);
    }

    #[test]
    fn level_scales_output() {
        let (mut cab, mut handle) = CabIr::new();
        cab.prepare(48_000);
        handle.install(asset_from_ir(&[1.0])).unwrap();
        cab.set_param(0, 0.0); // -12 dB
        let mut y = crate::testutil::sine(48_000, 440.0, 48_000 / 4);
        let mut yr = y.clone();
        cab.process(&mut y, &mut yr);
        let tail = &y[y.len() - 1_000..];
        assert!((peak(tail) - db_to_lin(-12.0)).abs() < 0.02);
    }

    /// The blend knob crossfades between the two IRs: at 0 the output equals
    /// `a` alone, at 1 it equals `b` alone. Two distinct delta IRs make the
    /// two extremes land at different sample positions.
    #[test]
    fn blend_crossfades_between_the_two_irs() {
        // a = 5-sample delay, b = 30-sample delay: an impulse comes out at 5
        // (all a), 30 (all b), or both (mid blend).
        let mut ir_a = vec![0.0f32; 64];
        ir_a[5] = 1.0;
        let mut ir_b = vec![0.0f32; 64];
        ir_b[30] = 1.0;

        let render = |blend_norm: f32| -> Vec<f32> {
            let (mut cab, mut handle) = CabIr::new();
            cab.prepare(48_000);
            handle.install(dual_asset(&ir_a, &ir_b)).unwrap();
            cab.set_param(1, blend_norm);
            // Settle the blend smoother fully before measuring (the 20 ms
            // one-pole needs several time constants).
            let mut warm = crate::testutil::silence(12_000);
            let mut warm_r = warm.clone();
            cab.process(&mut warm, &mut warm_r);
            let x = impulse(256, 10);
            let mut y = x.clone();
            let mut yr = x.clone();
            cab.process(&mut y, &mut yr);
            y
        };

        // blend 0 → only the a-tap (at 10+5=15) fires; the b-tap (10+30=40) is silent.
        let all_a = render(0.0);
        assert!(all_a[15].abs() > 0.5, "a present at 0: {}", all_a[15]);
        assert!(all_a[40].abs() < 1e-3, "b absent at 0: {}", all_a[40]);

        // blend 1 → only the b-tap fires.
        let all_b = render(1.0);
        assert!(all_b[15].abs() < 1e-3, "a absent at 1: {}", all_b[15]);
        assert!(all_b[40].abs() > 0.5, "b present at 1: {}", all_b[40]);

        // blend 0.5 → both taps present, each about half.
        let mid = render(0.5);
        assert!((mid[15].abs() - 0.5).abs() < 0.05, "half a: {}", mid[15]);
        assert!((mid[40].abs() - 0.5).abs() < 0.05, "half b: {}", mid[40]);
    }

    /// With no second IR, the blend knob does nothing — the cab stays `a`.
    #[test]
    fn blend_is_inert_without_a_second_ir() {
        let render = |blend_norm: f32| -> Vec<f32> {
            let (mut cab, mut handle) = CabIr::new();
            cab.prepare(48_000);
            handle.install(asset_from_ir(&[1.0])).unwrap();
            cab.set_param(1, blend_norm);
            let mut warm = crate::testutil::silence(2_048);
            let mut warm_r = warm.clone();
            cab.process(&mut warm, &mut warm_r);
            let x = crate::testutil::sine(48_000, 440.0, 512);
            let mut y = x.clone();
            let mut yr = x.clone();
            cab.process(&mut y, &mut yr);
            y
        };
        let full_a = render(0.0);
        let full_b = render(1.0);
        let err = full_a
            .iter()
            .zip(&full_b)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(err < 1e-6, "blend must be a no-op with one IR, err {err}");
        assert!(rms(&full_a) > 0.1, "signal still passes");
    }
}
