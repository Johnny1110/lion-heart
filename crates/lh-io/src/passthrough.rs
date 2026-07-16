//! Duplex streaming: input device → lock-free ring → processor → output.
//!
//! [`DuplexRunner`] owns the stream pair and calls a caller-supplied mono
//! processor inside the output callback — this is where the effect chain
//! plugs in. [`Passthrough`] is the identity-processor special case used by
//! the `run` diagnostic command.
//!
//! The bus is stereo (M7): one mono input channel is tapped, duplicated onto
//! an L/R pair for the processor, and interleaved to the output device (even
//! channels take L, odd take R). Output starts with a soft-start gain ramp so
//! a hot signal never hits the monitors instantly (white paper §3.3). In debug builds the
//! processor runs under `assert_no_alloc`: any allocation on the audio thread
//! aborts loudly (CLAUDE.md real-time rules).

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use cpal::traits::StreamTrait;

use crate::stats::{Snapshot, Stats};
use crate::stream::{self, DuplexSpec};
use crate::{DEFAULT_SAMPLE_RATE, IoError};

/// Startup window during which ring under/overruns are expected (streams are
/// still settling) and therefore not counted.
const GRACE_SECONDS: f64 = 0.25;
/// Soft-start ramp length.
const RAMP_SECONDS: f64 = 0.1;
/// Mono scratch handed to the processor; larger callbacks are sub-chunked.
const SCRATCH_FRAMES: usize = 8_192;

#[derive(Debug, Clone)]
pub struct RunnerOpts {
    pub input: Option<String>,
    pub output: Option<String>,
    pub sample_rate: u32,
    /// Requested buffer size in frames; `None` = device default.
    pub buffer: Option<u32>,
    /// Input channel to tap, 1-based.
    pub in_channel: u16,
    /// Output gain applied after the processor and the ramp, in dB.
    pub gain_db: f32,
    /// Blocks of silence pre-filled into the ring. Each block adds one buffer
    /// period of latency but absorbs callback jitter between the two streams.
    pub prefill_blocks: u32,
}

impl Default for RunnerOpts {
    fn default() -> Self {
        Self {
            input: None,
            output: None,
            sample_rate: DEFAULT_SAMPLE_RATE,
            buffer: Some(64),
            in_channel: 1,
            gain_db: 0.0,
            prefill_blocks: 1,
        }
    }
}

/// Back-compat name: the pure-passthrough `run` command shares these options.
pub type PassthroughOpts = RunnerOpts;

/// Resolved stream facts, handed to the processor factory before the streams
/// start — the one chance to `prepare()` DSP at the real sample rate.
pub struct StreamInfo {
    pub sample_rate: u32,
    pub description: String,
}

/// A running duplex stream pair. Dropping it stops the audio.
pub struct DuplexRunner {
    _in_stream: cpal::Stream,
    _out_stream: cpal::Stream,
    stats: Arc<Stats>,
    pub description: String,
    pub sample_rate: u32,
}

impl DuplexRunner {
    /// Start streaming. `make_processor` runs once, off the audio thread,
    /// with the resolved stream info; the closure it returns runs on the
    /// audio thread for every stereo block pair (the mono input arrives
    /// duplicated on both channels) and must obey real-time rules.
    pub fn start<F>(opts: &RunnerOpts, make_processor: F) -> Result<Self, IoError>
    where
        F: FnOnce(&StreamInfo) -> Box<dyn FnMut(&mut [f32], &mut [f32]) + Send>,
    {
        let setup = stream::resolve(&DuplexSpec {
            input: opts.input.clone(),
            output: opts.output.clone(),
            sample_rate: opts.sample_rate,
            buffer: opts.buffer,
            in_channel: opts.in_channel,
        })?;

        // Effective rate: resolve() may have substituted the device default.
        let sr = setup.in_config.sample_rate;
        let block = opts.buffer.filter(|&n| n > 0).unwrap_or(256) as usize;
        let grace_frames = (sr as f64 * GRACE_SECONDS) as u64;

        // Capacity is headroom against transient stalls; only occupancy
        // (the prefill) adds latency.
        let capacity = (block * 16).max(4096);
        let (mut producer, mut consumer) = rtrb::RingBuffer::<f32>::new(capacity);
        let prefill = block * opts.prefill_blocks as usize;
        for _ in 0..prefill.min(capacity) {
            let _ = producer.push(0.0);
        }

        let stats = Arc::new(Stats::default());

        let info = StreamInfo {
            sample_rate: sr,
            description: setup.describe(),
        };
        let mut processor = make_processor(&info);

        let in_channels = setup.in_channels;
        let tap = setup.tap;
        let data_in = {
            let stats = Arc::clone(&stats);
            move |data: &[f32], _info: &cpal::InputCallbackInfo| {
                let t0 = Instant::now();
                stats.in_callbacks.fetch_add(1, Ordering::Relaxed);
                let mut dropped = 0u64;
                for frame in data.chunks_exact(in_channels) {
                    if producer.push(frame[tap]).is_err() {
                        dropped += 1;
                    }
                }
                let frames = (data.len() / in_channels) as u64;
                let total = stats.in_frames.fetch_add(frames, Ordering::Relaxed) + frames;
                if dropped > 0 && total > grace_frames {
                    stats.overrun_frames.fetch_add(dropped, Ordering::Relaxed);
                    stats.overrun_events.fetch_add(1, Ordering::Relaxed);
                }
                stats.record_callback_nanos(t0.elapsed().as_nanos() as u64);
            }
        };

        let out_channels = setup.out_channels;
        let target_gain = db_to_lin(opts.gain_db);
        let ramp_step = target_gain / (sr as f64 * RAMP_SECONDS) as f32;
        let data_out = {
            let stats = Arc::clone(&stats);
            let mut gain = 0.0f32;
            let mut scratch_l = vec![0.0f32; SCRATCH_FRAMES];
            let mut scratch_r = vec![0.0f32; SCRATCH_FRAMES];
            move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                let t0 = Instant::now();
                stats.out_callbacks.fetch_add(1, Ordering::Relaxed);
                let frames = data.len() / out_channels;
                let mut missing = 0u64;
                let mut done = 0usize;
                while done < frames {
                    let n = (frames - done).min(SCRATCH_FRAMES);
                    let left = &mut scratch_l[..n];
                    let right = &mut scratch_r[..n];
                    for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                        let s = consumer.pop().unwrap_or_else(|_| {
                            missing += 1;
                            0.0
                        });
                        // Mono guitar in, duplicated onto the stereo bus.
                        *l = s;
                        *r = s;
                    }

                    #[cfg(debug_assertions)]
                    assert_no_alloc::assert_no_alloc(|| processor(left, right));
                    #[cfg(not(debug_assertions))]
                    processor(left, right);

                    let out = &mut data[done * out_channels..(done + n) * out_channels];
                    for (frame, (l, r)) in out
                        .chunks_exact_mut(out_channels)
                        .zip(left.iter().zip(right.iter()))
                    {
                        if gain < target_gain {
                            gain = (gain + ramp_step).min(target_gain);
                        }
                        // Even device channels get L, odd get R: stereo on a
                        // normal pair, and nothing is ever silent on >2-ch
                        // interfaces.
                        for (ch, o) in frame.iter_mut().enumerate() {
                            *o = if ch % 2 == 0 { l * gain } else { r * gain };
                        }
                    }
                    done += n;
                }
                let total =
                    stats.out_frames.fetch_add(frames as u64, Ordering::Relaxed) + frames as u64;
                if missing > 0 && total > grace_frames {
                    stats.underrun_frames.fetch_add(missing, Ordering::Relaxed);
                    stats.underrun_events.fetch_add(1, Ordering::Relaxed);
                }
                stats.record_callback_nanos(t0.elapsed().as_nanos() as u64);
            }
        };

        let (in_stream, out_stream) = stream::build_pair(&setup, &stats, data_in, data_out)?;
        // Input first: the ring should be filling before the output drains it.
        in_stream.play()?;
        out_stream.play()?;

        let mut description = info.description;
        description.push_str(&format!(
            "\nring  : prefill {} block(s) = {:.2} ms added latency\nactual: {} / {} frames per callback (in/out)",
            opts.prefill_blocks,
            prefill as f64 / sr as f64 * 1e3,
            describe_actual_buffer(&in_stream),
            describe_actual_buffer(&out_stream),
        ));

        Ok(Self {
            _in_stream: in_stream,
            _out_stream: out_stream,
            stats,
            description,
            sample_rate: sr,
        })
    }

    pub fn stats(&self) -> Snapshot {
        self.stats.snapshot()
    }
}

/// Identity-processor duplex: the `run` diagnostic command.
pub struct Passthrough {
    inner: DuplexRunner,
    pub description: String,
}

impl Passthrough {
    pub fn start(opts: &PassthroughOpts) -> Result<Self, IoError> {
        let inner = DuplexRunner::start(opts, |_| Box::new(|_l: &mut [f32], _r: &mut [f32]| {}))?;
        let description = inner.description.clone();
        Ok(Self { inner, description })
    }

    pub fn stats(&self) -> Snapshot {
        self.inner.stats()
    }
}

fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// The buffer size the backend actually granted, when it can report one.
fn describe_actual_buffer(stream: &cpal::Stream) -> String {
    stream
        .buffer_size()
        .map(|n| n.to_string())
        .unwrap_or_else(|_| "?".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_to_lin_reference_points() {
        assert!((db_to_lin(0.0) - 1.0).abs() < 1e-6);
        assert!((db_to_lin(-6.0) - 0.5012).abs() < 1e-3);
        assert!((db_to_lin(-60.0) - 0.001).abs() < 1e-6);
        assert!((db_to_lin(6.0) - 1.9953).abs() < 1e-3);
    }
}
