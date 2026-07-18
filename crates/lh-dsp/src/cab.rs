//! Cabinet simulator: zero-latency partitioned FFT convolution with a loaded
//! impulse response. The IR is prepared off-thread (see `lh-assets`) and
//! swapped in through an [`AssetSlot`].

use fft_convolver::FFTConvolver;
use lh_core::{EffectDesc, FamilyDesc, ParamDesc, Range, db_to_lin};

use crate::Effect;
use crate::blocks::smooth::Smoothed;
use crate::blocks::swap::{AssetHandle, AssetSlot, asset_channel};

static PARAMS: [ParamDesc; 1] = [ParamDesc {
    key: "level",
    name: "Level",
    unit: "dB",
    range: Range::Linear {
        min: -12.0,
        max: 12.0,
    },
    default: 0.0,
    smoothing_ms: 20.0,
}];

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

/// Ready-to-run convolvers (one per channel, same IR), built and `init`-ed
/// off the audio thread.
pub struct IrAsset {
    pub left: FFTConvolver<f32>,
    pub right: FFTConvolver<f32>,
}

const SCRATCH: usize = 1024;

pub struct CabIr {
    slot: AssetSlot<IrAsset>,
    level: Smoothed,
    scratch: Vec<f32>,
}

impl CabIr {
    pub fn new() -> (Self, AssetHandle<IrAsset>) {
        let (slot, handle) = asset_channel();
        (
            Self {
                slot,
                level: Smoothed::new(db_to_lin(PARAMS[0].default)),
                scratch: Vec::new(),
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
        self.scratch = vec![0.0; SCRATCH];
    }

    fn reset(&mut self) {
        if let Some(asset) = self.slot.get_mut() {
            asset.left.reset();
            asset.right.reset();
        }
    }

    fn set_param(&mut self, index: usize, normalized: f32) {
        if index == 0 {
            self.level
                .set_target(db_to_lin(PARAMS[0].range.to_real(normalized)));
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.slot.tick();
        if let Some(asset) = self.slot.get_mut() {
            for (lc, rc) in left.chunks_mut(SCRATCH).zip(right.chunks_mut(SCRATCH)) {
                for (chunk, convolver) in [(lc, &mut asset.left), (rc, &mut asset.right)] {
                    let dry = &mut self.scratch[..chunk.len()];
                    dry.copy_from_slice(chunk);
                    if convolver.process(dry, chunk).is_err() {
                        // Fail safe: an unusable convolver must not mute the rig.
                        chunk.copy_from_slice(dry);
                    }
                }
            }
        }
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let g = self.level.tick();
            *l *= g;
            *r *= g;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{assert_finite, impulse, peak};

    fn asset_from_ir(ir: &[f32]) -> Box<IrAsset> {
        let build = || {
            let mut convolver = FFTConvolver::<f32>::default();
            convolver.init(128, ir).unwrap();
            convolver
        };
        Box::new(IrAsset {
            left: build(),
            right: build(),
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
}
