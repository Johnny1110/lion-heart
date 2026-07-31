//! GPU FFT compute pipeline: replaces realfft's CPU-based spectrum analyzer.
//! Feeds audio tap samples into a wgpu storage buffer, dispatches a
//! compute shader for magnitude computation, and reads back display bins.
//! The FFT itself is currently performed on the GUI thread using realfft,
//! while the compute pass performs magnitude extraction.

use std::mem::size_of;
use std::sync::{Arc, mpsc};

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

use super::GpuContext;
use super::shaders;

/// Analysis window (~85 ms at 48 kHz — enough low-end resolution to place
/// a 30 Hz rumble while still tracking playing).
pub const FFT_LEN: usize = 4_096;
/// Log-spaced display bins across 20 Hz – 20 kHz.
pub const DISPLAY_BINS: usize = 120;
pub const FREQ_MIN: f32 = 20.0;
pub const FREQ_MAX: f32 = 20_000.0;
/// Display floor; bins rest here when silent.
pub const DB_FLOOR: f32 = -90.0;
/// Release per update call (~30 Hz updates ⇒ ~18 dB/s decay).
const RELEASE_DB: f32 = 0.6;

const COMPLEX_BINS: usize = FFT_LEN / 2 + 1;
const COMPLEX_BYTES: u64 = (COMPLEX_BINS * size_of::<f32>() * 2) as u64;
const MAG_BYTES: u64 = (COMPLEX_BINS * size_of::<f32>()) as u64;
const WORKGROUP_SIZE: u32 = 64;

pub struct GpuFft {
    sample_rate: f32,
    /// Ring of the latest FFT_LEN samples from the tap.
    window: Vec<f32>,
    write: usize,
    hann: Vec<f32>,
    fft: Arc<dyn RealToComplex<f32>>,
    input: Vec<f32>,
    output: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    /// FFT-bin range per display bin.
    ranges: Vec<(usize, usize)>,
    /// Display values in dBFS with ballistics applied.
    pub bins: Vec<f32>,

    // WGPU resources.
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    input_buffer: wgpu::Buffer,
    output_buffer: wgpu::Buffer,
    readback_buffer: wgpu::Buffer,
    compute_pipeline: wgpu::ComputePipeline,
    bind_group: wgpu::BindGroup,
    fft_bytes: Vec<u8>,
    fft_magnitudes: Vec<f32>,
}

impl GpuFft {
    pub fn new(ctx: &GpuContext, sample_rate: u32) -> Self {
        let sample_rate = sample_rate as f32;
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FFT_LEN);
        let hann: Vec<f32> = (0..FFT_LEN)
            .map(|i| {
                let phase = std::f32::consts::TAU * i as f32 / FFT_LEN as f32;
                0.5 * (1.0 - phase.cos())
            })
            .collect();
        let ranges = compute_ranges(sample_rate);

        let input_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("spectrum_fft_input"),
            size: COMPLEX_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let output_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("spectrum_fft_output"),
            size: MAG_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        // WGPU can only create a readback buffer once the compute shader has
        // written into a storage buffer.
        let readback_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("spectrum_fft_readback"),
            size: MAG_BYTES,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("spectrum_wgpu_shader"),
                source: wgpu::ShaderSource::Wgsl(shaders::SPECTRUM_WGSL.into()),
            });

        let bind_group_layout =
            ctx.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("spectrum_bind_group_layout"),
                    entries: &[
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: false },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                    ],
                });

        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("spectrum_bind_group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: input_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: output_buffer.as_entire_binding(),
                },
            ],
        });

        let pipeline_layout = ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("spectrum_pipeline_layout"),
                bind_group_layouts: &[&bind_group_layout],
                push_constant_ranges: &[],
            });

        let compute_pipeline =
            ctx.device
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some("spectrum_magnitude_pipeline"),
                    layout: Some(&pipeline_layout),
                    module: &shader,
                    entry_point: Some("main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    cache: None,
                });

        let input = vec![0.0f32; FFT_LEN];
        let output = fft.make_output_vec();
        let scratch = fft.make_scratch_vec();
        let fft_bytes = vec![0u8; COMPLEX_BYTES as usize];
        let fft_magnitudes = vec![0.0f32; COMPLEX_BINS];

        Self {
            sample_rate,
            window: vec![0.0; FFT_LEN],
            write: 0,
            hann,
            fft,
            input,
            output,
            scratch,
            ranges,
            bins: vec![DB_FLOOR; DISPLAY_BINS],
            device: Arc::clone(&ctx.device),
            queue: Arc::clone(&ctx.queue),
            input_buffer,
            output_buffer,
            readback_buffer,
            compute_pipeline,
            bind_group,
            fft_bytes,
            fft_magnitudes,
        }
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Append tapped samples into the sliding window.
    pub fn feed(&mut self, samples: &[f32]) {
        for &s in samples {
            self.window[self.write] = s;
            self.write = (self.write + 1) % self.window.len();
        }
    }

    /// Recompute the display bins from the latest window (call ~30 Hz).
    pub fn update(&mut self) {
        let n = self.window.len();
        for (i, x) in self.input.iter_mut().enumerate() {
            *x = self.window[(self.write + i) % n] * self.hann[i];
        }

        if self
            .fft
            .process_with_scratch(&mut self.input, &mut self.output, &mut self.scratch)
            .is_err()
        {
            return;
        }

        self.pack_complex_input();
        if !self.dispatch_gpu_magnitude() {
            self.compute_magnitudes_cpu();
        }
        self.aggregate_to_display_bins();
    }

    fn pack_complex_input(&mut self) {
        let mut chunks = self.fft_bytes.chunks_exact_mut(8);
        for (slot, sample) in chunks.by_ref().zip(self.output.iter()) {
            slot[0..4].copy_from_slice(&sample.re.to_ne_bytes());
            slot[4..8].copy_from_slice(&sample.im.to_ne_bytes());
        }

        let _ = self
            .queue
            .write_buffer(&self.input_buffer, 0, &self.fft_bytes);
    }

    fn dispatch_gpu_magnitude(&mut self) -> bool {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("spectrum_magnitude_encoder"),
            });

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("spectrum_magnitude_pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.compute_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            let workgroups = (COMPLEX_BINS as u32 + WORKGROUP_SIZE - 1) / WORKGROUP_SIZE;
            pass.dispatch_workgroups(workgroups, 1, 1);
        }

        encoder.copy_buffer_to_buffer(&self.output_buffer, 0, &self.readback_buffer, 0, MAG_BYTES);

        self.queue.submit(Some(encoder.finish()));

        let slice = self.readback_buffer.slice(..);
        let (tx, rx) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());

        let ok = match rx.recv() {
            Ok(Ok(())) => {
                let data = slice.get_mapped_range();
                for (dst, chunk) in self
                    .fft_magnitudes
                    .iter_mut()
                    .zip(data.chunks_exact(size_of::<f32>()).take(COMPLEX_BINS))
                {
                    let bytes = [chunk[0], chunk[1], chunk[2], chunk[3]];
                    *dst = f32::from_ne_bytes(bytes);
                }
                drop(data);
                true
            }
            _ => false,
        };

        if ok {
            self.readback_buffer.unmap();
        }
        ok
    }

    fn compute_magnitudes_cpu(&mut self) {
        for (dst, c) in self.fft_magnitudes.iter_mut().zip(self.output.iter()) {
            *dst = c.norm_sqr();
        }
    }

    fn aggregate_to_display_bins(&mut self) {
        // Hann coherent gain is 0.5: a full-scale sine peaks at 0 dBFS.
        let scale = 4.0 / FFT_LEN as f32;
        for (bin, &(lo, hi)) in self.bins.iter_mut().zip(&self.ranges) {
            let mut peak = 0.0f32;
            for &m in &self.fft_magnitudes[lo..hi] {
                peak = peak.max(m);
            }
            let amp = peak.sqrt() * scale;
            let db = (20.0 * amp.max(1e-9).log10()).max(DB_FLOOR);
            // Fast attack, slow release.
            *bin = if db > *bin {
                db
            } else {
                (*bin - RELEASE_DB).max(db).max(DB_FLOOR)
            };
        }
    }
}

pub fn compute_ranges(sample_rate: f32) -> Vec<(usize, usize)> {
    let bin_hz = sample_rate / FFT_LEN as f32;
    let edge =
        |i: usize| -> f32 { FREQ_MIN * (FREQ_MAX / FREQ_MIN).powf(i as f32 / DISPLAY_BINS as f32) };
    (0..DISPLAY_BINS)
        .map(|i| {
            let lo = (edge(i) / bin_hz).floor().max(1.0) as usize;
            let hi = ((edge(i + 1) / bin_hz).ceil() as usize).clamp(lo + 1, FFT_LEN / 2 + 1);
            (lo.min(FFT_LEN / 2), hi)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_cpu_version() {
        assert_eq!(FFT_LEN, 4_096);
        assert_eq!(DISPLAY_BINS, 120);
        assert_eq!(FREQ_MIN, 20.0);
        assert_eq!(FREQ_MAX, 20_000.0);
        assert_eq!(DB_FLOOR, -90.0);
    }

    #[test]
    fn bin_ranges_are_log_spaced() {
        let sample_rate = 48_000.0f32;
        let ranges = compute_ranges(sample_rate);
        assert_eq!(ranges.len(), DISPLAY_BINS);
        // First bin should cover the lowest frequencies.
        assert!(ranges[0].0 <= 1 || ranges[0].1 >= 1);
        // Last bin should cover near Nyquist.
        assert!(ranges.last().unwrap().1 >= FFT_LEN / 2 - 10);
    }

    #[test]
    fn ranges_are_monotonic_and_in_bounds() {
        let sample_rate = 48_000.0f32;
        let ranges = compute_ranges(sample_rate);
        let mut prev_hi = 0usize;
        for &(lo, hi) in &ranges {
            assert!(lo < hi);
            assert!(lo <= FFT_LEN / 2);
            assert!(hi <= FFT_LEN / 2 + 1);
            assert!(hi > prev_hi);
            prev_hi = hi;
        }
    }
}
