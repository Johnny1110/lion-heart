//! Round-trip latency (RTL) measurement over a physical loopback.
//!
//! The output emits a short 1 kHz burst every `interval_ms`; the input runs a
//! threshold detector after a noise-floor calibration phase. Both callbacks
//! timestamp their event with `Instant`, compensated by the position of the
//! sample inside its buffer, so the wall-clock difference is the full round
//! trip including driver, converter, and buffering stages. The median over
//! several trials suppresses callback-scheduling jitter.
//!
//! Requires the interface output to be cabled back into the measured input
//! (or the interface's loopback mode).

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use cpal::traits::StreamTrait;

use crate::stats::{Snapshot, Stats};
use crate::stream::{self, DuplexSpec};
use crate::{DEFAULT_SAMPLE_RATE, IoError};

/// Output silence before the first burst: covers detector calibration plus margin.
const WARMUP_SECONDS: f64 = 1.0;
/// A detection is paired with an emission at most this far in the past.
const MATCH_WINDOW: Duration = Duration::from_millis(250);
const CLICK_MILLIS: f64 = 2.0;
const CLICK_HZ: f64 = 1_000.0;

#[derive(Debug, Clone)]
pub struct LatencyOpts {
    pub input: Option<String>,
    pub output: Option<String>,
    pub sample_rate: u32,
    pub buffer: Option<u32>,
    pub in_channel: u16,
    pub trials: u32,
    pub interval_ms: u32,
    pub amplitude: f32,
}

impl Default for LatencyOpts {
    fn default() -> Self {
        Self {
            input: None,
            output: None,
            sample_rate: DEFAULT_SAMPLE_RATE,
            buffer: Some(64),
            in_channel: 1,
            trials: 10,
            interval_ms: 300,
            amplitude: 0.5,
        }
    }
}

#[derive(Debug)]
pub struct LatencyReport {
    pub trials_ms: Vec<f64>,
    pub median_ms: f64,
    pub min_ms: f64,
    pub max_ms: f64,
    /// Emissions that never produced a detection within the match window.
    pub missed: u32,
    pub noise_floor: f32,
    pub threshold: f32,
    pub sample_rate: u32,
    pub requested_buffer: Option<u32>,
    /// Frames per callback the backend actually granted (input, output).
    pub actual_buffer: (Option<u32>, Option<u32>),
    pub description: String,
    pub xruns: Snapshot,
}

impl LatencyReport {
    pub fn to_markdown(&self) -> String {
        let buffer = self
            .requested_buffer
            .filter(|&n| n > 0)
            .map(|n| n.to_string())
            .unwrap_or_else(|| "default".into());
        let fmt_actual = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_else(|| "?".into());
        format!(
            "### RTL measurement\n\n\
             ```\n{}\n```\n\n\
             - sample rate: {} Hz, requested buffer: {} frames (actual in/out: {}/{})\n\
             - **RTL median: {:.2} ms** (min {:.2} / max {:.2}, {} trials, {} missed)\n\
             - noise floor: {:.4} (detector threshold {:.4})\n\
             - stream errors during test: {}\n",
            self.description,
            self.sample_rate,
            buffer,
            fmt_actual(self.actual_buffer.0),
            fmt_actual(self.actual_buffer.1),
            self.median_ms,
            self.min_ms,
            self.max_ms,
            self.trials_ms.len(),
            self.missed,
            self.noise_floor,
            self.threshold,
            self.xruns.stream_errors,
        )
    }
}

/// Run the measurement. Blocks the calling thread; `progress` is invoked once
/// per successful trial with (trial number, RTL in ms).
pub fn measure(
    opts: &LatencyOpts,
    progress: &mut dyn FnMut(usize, f64),
) -> Result<LatencyReport, IoError> {
    let setup = stream::resolve(&DuplexSpec {
        input: opts.input.clone(),
        output: opts.output.clone(),
        sample_rate: opts.sample_rate,
        buffer: opts.buffer,
        in_channel: opts.in_channel,
    })?;

    let sr = opts.sample_rate;
    let sr_f = sr as f64;
    let trials = opts.trials.max(1);
    // Keep emissions far enough apart that the detector's refractory period
    // and the match window cannot bleed across trials.
    let interval_ms = opts.interval_ms.max(150);
    let interval_samples = (sr_f * interval_ms as f64 / 1e3) as u64;

    let stats = Arc::new(Stats::default());
    let (mut emit_tx, mut emit_rx) = rtrb::RingBuffer::<Instant>::new(64);
    let (mut det_tx, mut det_rx) = rtrb::RingBuffer::<Instant>::new(64);
    let noise_bits = Arc::new(AtomicU32::new(0));
    let threshold_bits = Arc::new(AtomicU32::new(0));

    let in_channels = setup.in_channels;
    let tap = setup.tap;
    let data_in = {
        let stats = Arc::clone(&stats);
        let noise_bits = Arc::clone(&noise_bits);
        let threshold_bits = Arc::clone(&threshold_bits);
        let mut detector = Detector::new(sr);
        let mut calibration_stored = false;
        move |data: &[f32], _info: &cpal::InputCallbackInfo| {
            let now = Instant::now();
            stats.in_callbacks.fetch_add(1, Ordering::Relaxed);
            let frames = data.len() / in_channels;
            for (i, frame) in data.chunks_exact(in_channels).enumerate() {
                if detector.feed(frame[tap]) {
                    // Sample i was captured (frames - i) samples before "now".
                    let age = Duration::from_secs_f64((frames - i) as f64 / sr_f);
                    let _ = det_tx.push(now - age);
                }
            }
            if !calibration_stored && detector.calibrated() {
                noise_bits.store(detector.noise_floor().to_bits(), Ordering::Relaxed);
                threshold_bits.store(detector.threshold().to_bits(), Ordering::Relaxed);
                calibration_stored = true;
            }
            stats.in_frames.fetch_add(frames as u64, Ordering::Relaxed);
            stats.record_callback_nanos(now.elapsed().as_nanos() as u64);
        }
    };

    let out_channels = setup.out_channels;
    let click = click_signal(sr, opts.amplitude);
    let data_out = {
        let stats = Arc::clone(&stats);
        let mut wait = (sr_f * WARMUP_SECONDS) as u64;
        let mut playing = false;
        let mut pos = 0usize;
        let mut sent = 0u32;
        move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
            let now = Instant::now();
            stats.out_callbacks.fetch_add(1, Ordering::Relaxed);
            for (i, frame) in data.chunks_exact_mut(out_channels).enumerate() {
                if !playing {
                    if wait == 0 && sent < trials {
                        playing = true;
                        pos = 0;
                        sent += 1;
                        // Frame i of this buffer plays i samples after frame 0.
                        let _ = emit_tx.push(now + Duration::from_secs_f64(i as f64 / sr_f));
                    } else {
                        wait = wait.saturating_sub(1);
                    }
                }
                let value = if playing {
                    let s = click[pos];
                    pos += 1;
                    if pos == click.len() {
                        playing = false;
                        wait = interval_samples;
                    }
                    s
                } else {
                    0.0
                };
                for out in frame {
                    *out = value;
                }
            }
            let frames = (data.len() / out_channels) as u64;
            stats.out_frames.fetch_add(frames, Ordering::Relaxed);
            stats.record_callback_nanos(now.elapsed().as_nanos() as u64);
        }
    };

    let (in_stream, out_stream) = stream::build_pair(&setup, &stats, data_in, data_out)?;
    in_stream.play()?;
    out_stream.play()?;
    let actual_buffer = (in_stream.buffer_size().ok(), out_stream.buffer_size().ok());

    let deadline = Duration::from_secs_f64(WARMUP_SECONDS)
        + Duration::from_millis(trials as u64 * interval_ms as u64 + 3_000);
    let started = Instant::now();
    let mut pending: VecDeque<Instant> = VecDeque::new();
    let mut results: Vec<f64> = Vec::new();
    let mut missed = 0u32;

    while results.len() < trials as usize && started.elapsed() < deadline {
        std::thread::sleep(Duration::from_millis(5));
        while let Ok(at) = emit_rx.pop() {
            pending.push_back(at);
        }
        while pending
            .front()
            .is_some_and(|emit| emit.elapsed() > MATCH_WINDOW)
        {
            pending.pop_front();
            missed += 1;
        }
        while let Ok(at) = det_rx.pop() {
            if let Some(ms) = match_detect(&mut pending, at, MATCH_WINDOW) {
                results.push(ms);
                progress(results.len(), ms);
            }
        }
    }

    drop(in_stream);
    drop(out_stream);

    let noise_floor = f32::from_bits(noise_bits.load(Ordering::Relaxed));
    let threshold = f32::from_bits(threshold_bits.load(Ordering::Relaxed));
    if results.is_empty() {
        return Err(IoError::NoLoopbackSignal {
            noise: noise_floor,
            threshold,
        });
    }

    let mut sorted = results.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let median_ms = sorted[sorted.len() / 2];

    Ok(LatencyReport {
        median_ms,
        min_ms: sorted[0],
        max_ms: sorted[sorted.len() - 1],
        missed,
        noise_floor,
        threshold,
        sample_rate: sr,
        requested_buffer: opts.buffer,
        actual_buffer,
        description: setup.describe(),
        xruns: stats.snapshot(),
        trials_ms: results,
    })
}

/// Pair a detection with the oldest pending emission. Detections that precede
/// every pending emission (startup clicks, cable noise) are ignored; stale
/// emissions are pruned by the caller.
fn match_detect(pending: &mut VecDeque<Instant>, detect: Instant, window: Duration) -> Option<f64> {
    let emit = *pending.front()?;
    let delta = detect.checked_duration_since(emit)?;
    if delta <= window {
        pending.pop_front();
        Some(delta.as_secs_f64() * 1e3)
    } else {
        None
    }
}

/// 2 ms of 1 kHz sine. Starts at a zero crossing (no DC thump) yet exceeds a
/// low detector threshold within a sample or two, keeping onset bias ≪ 0.1 ms.
fn click_signal(sample_rate: u32, amplitude: f32) -> Vec<f32> {
    let len = ((sample_rate as f64) * CLICK_MILLIS / 1e3).round() as usize;
    (0..len)
        .map(|n| {
            let phase = 2.0 * std::f64::consts::PI * CLICK_HZ * n as f64 / sample_rate as f64;
            (phase.sin() * amplitude as f64) as f32
        })
        .collect()
}

/// Threshold detector with a noise-calibration phase and a refractory period.
/// Pure sample-in/bool-out so it can be tested without any audio device.
struct Detector {
    calib_remaining: u32,
    calib_total: u32,
    sum_squares: f64,
    noise_rms: f32,
    threshold: f32,
    refractory: u32,
    cooldown: u32,
}

impl Detector {
    fn new(sample_rate: u32) -> Self {
        Self {
            calib_remaining: sample_rate / 2,
            calib_total: sample_rate / 2,
            sum_squares: 0.0,
            noise_rms: 0.0,
            threshold: 0.0,
            refractory: sample_rate / 10,
            cooldown: 0,
        }
    }

    /// Feed one sample; true = onset detected at this sample.
    fn feed(&mut self, sample: f32) -> bool {
        if self.calib_remaining > 0 {
            self.sum_squares += f64::from(sample) * f64::from(sample);
            self.calib_remaining -= 1;
            if self.calib_remaining == 0 {
                self.noise_rms = (self.sum_squares / f64::from(self.calib_total)).sqrt() as f32;
                self.threshold = (self.noise_rms * 8.0).max(0.02);
            }
            return false;
        }
        if self.cooldown > 0 {
            self.cooldown -= 1;
            return false;
        }
        if sample.abs() >= self.threshold {
            self.cooldown = self.refractory;
            return true;
        }
        false
    }

    fn calibrated(&self) -> bool {
        self.calib_remaining == 0
    }

    fn noise_floor(&self) -> f32 {
        self.noise_rms
    }

    fn threshold(&self) -> f32 {
        self.threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    fn calibrated_detector(noise: f32) -> Detector {
        let mut det = Detector::new(SR);
        let mut sign = 1.0f32;
        for _ in 0..SR / 2 {
            det.feed(noise * sign);
            sign = -sign;
        }
        assert!(det.calibrated());
        det
    }

    #[test]
    fn detector_fires_once_then_respects_refractory() {
        let mut det = calibrated_detector(0.0);
        assert!((det.threshold() - 0.02).abs() < 1e-6, "floor threshold");

        assert!(det.feed(0.5), "loud sample must fire");
        for _ in 0..SR / 10 {
            assert!(!det.feed(0.5), "refractory must suppress re-fires");
        }
        assert!(det.feed(0.5), "fires again after refractory");
    }

    #[test]
    fn detector_threshold_scales_with_noise_floor() {
        let mut det = calibrated_detector(0.01);
        assert!((det.noise_floor() - 0.01).abs() < 1e-4);
        assert!((det.threshold() - 0.08).abs() < 1e-4);
        assert!(!det.feed(0.05), "below threshold");
        assert!(det.feed(0.09), "above threshold");
    }

    #[test]
    fn detector_stays_quiet_during_calibration() {
        let mut det = Detector::new(SR);
        for _ in 0..100 {
            assert!(!det.feed(0.9), "no detections while calibrating");
        }
    }

    #[test]
    fn click_starts_at_zero_crossing_and_peaks_at_amplitude() {
        let click = click_signal(SR, 0.5);
        assert_eq!(click.len(), 96); // 2 ms at 48 kHz
        assert!(click[0].abs() < 1e-6);
        assert!(
            click[1] > 0.02,
            "must clear the floor threshold immediately"
        );
        let peak = click.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((peak - 0.5).abs() < 0.01);
    }

    #[test]
    fn match_detect_pairs_prunes_and_ignores() {
        let window = Duration::from_millis(250);
        let base = Instant::now();
        let ms = |n: u64| Duration::from_millis(n);

        // Normal pairing.
        let mut pending = VecDeque::from([base]);
        let got = match_detect(&mut pending, base + ms(6), window).unwrap();
        assert!((got - 6.0).abs() < 0.5);
        assert!(pending.is_empty());

        // Detection earlier than every emission → spurious, keep the emission.
        let mut pending = VecDeque::from([base + ms(10)]);
        assert!(match_detect(&mut pending, base + ms(5), window).is_none());
        assert_eq!(pending.len(), 1);

        // Detection far past the window → no match, emission left for pruning.
        let mut pending = VecDeque::from([base]);
        assert!(match_detect(&mut pending, base + ms(400), window).is_none());
        assert_eq!(pending.len(), 1);

        // No pending emissions → ignored.
        let mut pending = VecDeque::new();
        assert!(match_detect(&mut pending, base, window).is_none());
    }
}
