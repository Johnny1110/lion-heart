//! Shared engine driver for the M4 GUI framework spike.
//!
//! Both spike UIs (iced / vizia) bind to the exact plumbing the product UI
//! will use — `ChainHandle` messages in, `Telemetry` atomics out — but the
//! audio callback is replaced by a worker thread pacing the real chain at
//! 48 kHz over a synthetic guitar pluck, so the spike needs no audio device
//! and runs identically in a container and on a Mac.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use lh_core::{ParamDesc, lin_to_db};
use lh_dsp::delay::Delay;
use lh_dsp::drive::Drive;
use lh_dsp::gate::NoiseGate;
use lh_engine::{ChainHandle, build_chain};

pub const SAMPLE_RATE: u32 = 48_000;
const BLOCK: usize = 64;
/// Blocks per worker wakeup: 12 × 64 / 48 kHz = 16 ms — one UI frame's worth.
const BLOCKS_PER_TICK: usize = 12;

/// Meter floor; peaks below this render as an empty bar.
pub const METER_FLOOR_DB: f32 = -60.0;
/// Display fall rate in dB per UI frame (~30 dB/s at 60 fps), instant attack.
const METER_FALL_DB_PER_FRAME: f32 = 0.5;

/// A looping low-E guitar pluck: harmonic stack with an exponential decay,
/// retriggered every 1.5 s. The attack transient at retrigger is the point —
/// it exercises the gate, drive and meters the way a real pick attack would.
struct Pluck {
    sample_rate: f32,
    /// Samples since the last trigger.
    t: usize,
    /// Samples between triggers.
    period: usize,
}

impl Pluck {
    fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: sample_rate as f32,
            t: 0,
            period: (sample_rate as f32 * 1.5) as usize,
        }
    }

    fn fill(&mut self, block: &mut [f32]) {
        const F0: f32 = 82.41; // low E
        for x in block.iter_mut() {
            let secs = self.t as f32 / self.sample_rate;
            let env = (-2.5 * secs).exp();
            let ph = std::f32::consts::TAU * F0 * secs;
            let s = ph.sin()
                + 0.5 * (2.0 * ph).sin()
                + 0.33 * (3.0 * ph + 0.3).sin()
                + 0.2 * (4.0 * ph + 0.7).sin();
            *x = 0.28 * env * s;
            self.t += 1;
            if self.t >= self.period {
                self.t = 0;
            }
        }
    }
}

/// The real chain on a paced worker thread, plus the control-side handle.
pub struct SpikeEngine {
    handle: ChainHandle,
    drive: &'static ParamDesc,
    drive_norm: f32,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl SpikeEngine {
    pub fn start() -> Self {
        let (mut chain, handle) = build_chain(vec![
            Box::new(NoiseGate::new()),
            Box::new(Drive::new()),
            Box::new(Delay::new()),
        ]);
        chain.prepare(SAMPLE_RATE);

        let drive = handle
            .descriptors()
            .iter()
            .find(|d| d.key == "drive")
            .expect("drive slot")
            .params
            .iter()
            .find(|p| p.key == "drive")
            .expect("drive param");

        let running = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&running);
        let worker = std::thread::Builder::new()
            .name("spike-engine".into())
            .spawn(move || {
                let mut synth = Pluck::new(SAMPLE_RATE);
                let mut block = [0.0f32; BLOCK];
                let tick = Duration::from_micros(
                    (BLOCKS_PER_TICK * BLOCK) as u64 * 1_000_000 / SAMPLE_RATE as u64,
                );
                let mut next = Instant::now();
                while flag.load(Ordering::Relaxed) {
                    for _ in 0..BLOCKS_PER_TICK {
                        synth.fill(&mut block);
                        chain.process(&mut block);
                    }
                    next += tick;
                    let now = Instant::now();
                    if next > now {
                        std::thread::sleep(next - now);
                    } else {
                        next = now; // fell behind (e.g. laptop sleep) — resync
                    }
                }
            })
            .expect("spawn spike engine worker");

        let mut engine = Self {
            handle,
            drive,
            drive_norm: drive.range.to_norm(drive.default),
            running,
            worker: Some(worker),
        };
        // Push the default through the queue so handle state and display agree.
        engine.set_drive_norm(engine.drive_norm);
        engine
    }

    pub fn drive_param(&self) -> &'static ParamDesc {
        self.drive
    }

    pub fn drive_norm(&self) -> f32 {
        self.drive_norm
    }

    /// Set the drive from a normalized 0..1 value; returns the real dB applied.
    pub fn set_drive_norm(&mut self, norm: f32) -> f32 {
        let norm = norm.clamp(0.0, 1.0);
        let real = self.drive.range.to_real(norm);
        let applied = self
            .handle
            .set_param("drive", "drive", real)
            .expect("drive param exists");
        self.drive_norm = norm;
        applied.real
    }

    /// Human-readable current value, e.g. `"24.0 dB"`.
    pub fn drive_display(&self) -> String {
        format!(
            "{:.1} {}",
            self.drive.range.to_real(self.drive_norm),
            self.drive.unit
        )
    }

    /// Latest per-block linear peaks (input, output) from the audio side.
    pub fn peaks(&self) -> (f32, f32) {
        let t = self.handle.telemetry();
        (t.peak_in(), t.peak_out())
    }
}

impl Drop for SpikeEngine {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Peak-meter display ballistics: instant attack, constant-rate fall.
/// Shared so both spike UIs render identical meter motion.
pub struct MeterBallistics {
    in_db: f32,
    out_db: f32,
}

impl MeterBallistics {
    pub fn new() -> Self {
        Self {
            in_db: METER_FLOOR_DB,
            out_db: METER_FLOOR_DB,
        }
    }

    /// Feed the latest linear peaks (once per UI frame); returns display dB.
    pub fn tick(&mut self, peaks: (f32, f32)) -> (f32, f32) {
        let feed = |shown: &mut f32, peak: f32| {
            let db = lin_to_db(peak.max(1e-6)).max(METER_FLOOR_DB);
            *shown = if db >= *shown {
                db
            } else {
                (*shown - METER_FALL_DB_PER_FRAME).max(db)
            };
        };
        feed(&mut self.in_db, peaks.0);
        feed(&mut self.out_db, peaks.1);
        (self.in_db, self.out_db)
    }

    /// Current display values as 0..1 bar lengths (floor..0 dBFS).
    pub fn norms(&self) -> (f32, f32) {
        let n = |db: f32| ((db - METER_FLOOR_DB) / -METER_FLOOR_DB).clamp(0.0, 1.0);
        (n(self.in_db), n(self.out_db))
    }
}

impl Default for MeterBallistics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_runs_and_reports_peaks() {
        let mut engine = SpikeEngine::start();
        engine.set_drive_norm(0.75);
        std::thread::sleep(Duration::from_millis(120));
        let (peak_in, peak_out) = engine.peaks();
        assert!(peak_in > 0.0 && peak_in.is_finite(), "input peak {peak_in}");
        assert!(
            peak_out > 0.0 && peak_out.is_finite(),
            "output peak {peak_out}"
        );
    }

    #[test]
    fn drive_norm_round_trips_through_range() {
        let mut engine = SpikeEngine::start();
        let real = engine.set_drive_norm(0.5);
        assert!((real - 20.0).abs() < 1e-3, "mid of 0..40 dB, got {real}");
        assert_eq!(engine.drive_display(), "20.0 dB");
    }

    #[test]
    fn ballistics_attack_is_instant_and_fall_is_bounded() {
        let mut m = MeterBallistics::new();
        let (i, _) = m.tick((1.0, 0.0));
        assert!(i.abs() < 1e-3, "0 dBFS shows immediately, got {i}");
        let (i2, _) = m.tick((0.0, 0.0));
        assert!(
            (i - i2 - METER_FALL_DB_PER_FRAME).abs() < 1e-3,
            "falls one step per frame"
        );
    }
}
