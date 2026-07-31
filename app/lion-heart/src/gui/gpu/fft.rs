//! GPU FFT compute pipeline: replaces realfft's CPU-only spectrum analyzer path with a
//! wgpu compute stage for magnitude extraction. The GUI thread still owns all
//! analysis work and no audio-thread code is touched.

use std::mem::size_of;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, mpsc};

use pollster::block_on;
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
const SAMPLE_BYTES: u64 = (FFT_LEN * size_of::<f32>()) as u64;
const MAG_BYTES: u64 = (COMPLEX_BINS * size_of::<f32>()) as u64;
const WORKGROUP_SIZE: u32 = 64;

struct GpuState {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    input_buffer: wgpu::Buffer,
    output_buffer: wgpu::Buffer,
    readback_buffer: wgpu::Buffer,
    compute_pipeline: wgpu::ComputePipeline,
    bind_group: wgpu::BindGroup,
}

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
    bins: Vec<f32>,

    gpu: Option<GpuState>,
    gpu_input_bytes: Vec<u8>,
    fft_magnitudes: Vec<f32>,
}

impl GpuFft {
    /// Create a GPU-preferred analyzer. If no compatible GPU adapter/device can be
    /// acquired, construction still succeeds and the analyzer falls back to CPU
    /// magnitude computation.
    pub fn new(sample_rate: u32) -> Self {
        let sample_rate = sample_rate as f32;
        let mut analyzer = Self::new_cpu(sample_rate);
        if let Some((device, queue)) = Self::default_wgpu_device() {
            analyzer.gpu = Self::init_gpu_state(&device, &queue);
        }
        analyzer
    }

    /// Create a GPU-preferred analyzer from an external wgpu context. If the
    /// provided context is unusable, this also falls back to CPU-only mode.
    pub fn new_with_context(ctx: &GpuContext, sample_rate: u32) -> Self {
        let sample_rate = sample_rate as f32;
        let mut analyzer = Self::new_cpu(sample_rate);
        analyzer.gpu = Self::init_gpu_state(&ctx.device, &ctx.queue);
        analyzer
    }

    fn new_cpu(sample_rate: f32) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_LEN);
        let hann: Vec<f32> = (0..FFT_LEN)
            .map(|i| {
                let phase = std::f32::consts::TAU * i as f32 / FFT_LEN as f32;
                0.5 * (1.0 - phase.cos())
            })
            .collect();

        let output = fft.make_output_vec();
        let scratch = fft.make_scratch_vec();
        Self {
            sample_rate,
            window: vec![0.0; FFT_LEN],
            write: 0,
            hann,
            fft,
            input: vec![0.0f32; FFT_LEN],
            output,
            scratch,
            ranges: compute_ranges(sample_rate),
            bins: vec![DB_FLOOR; DISPLAY_BINS],
            gpu: None,
            gpu_input_bytes: vec![0u8; SAMPLE_BYTES as usize],
            fft_magnitudes: vec![0.0f32; COMPLEX_BINS],
        }
    }

    fn default_wgpu_device() -> Option<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = instance
            .enumerate_adapters(wgpu::Backends::all())
            .into_iter()
            .find(|a| a.get_info().device_type != wgpu::DeviceType::Cpu)?;

        let result = catch_unwind(AssertUnwindSafe(|| {
            block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("lion-heart-spectrum-analyzer"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                trace: wgpu::Trace::Off,
            }))
        }))
        .ok()?;

        match result {
            Ok((device, queue)) => Some((Arc::new(device), Arc::new(queue))),
            Err(_) => None,
        }
    }

    fn init_gpu_state(device: &Arc<wgpu::Device>, queue: &Arc<wgpu::Queue>) -> Option<GpuState> {
        let state = catch_unwind(AssertUnwindSafe(|| {
            let input_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("spectrum_fft_input"),
                size: SAMPLE_BYTES,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let output_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("spectrum_fft_output"),
                size: MAG_BYTES,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            // WGPU can only create a readback buffer once the compute shader has
            // written into a storage buffer.
            let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("spectrum_fft_readback"),
                size: MAG_BYTES,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });

            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("spectrum_wgpu_shader"),
                source: wgpu::ShaderSource::Wgsl(shaders::SPECTRUM_WGSL.into()),
            });

            let bind_group_layout =
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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

            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
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

            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("spectrum_pipeline_layout"),
                bind_group_layouts: &[&bind_group_layout],
                push_constant_ranges: &[],
            });

            let compute_pipeline =
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some("spectrum_dft_pipeline"),
                    layout: Some(&pipeline_layout),
                    module: &shader,
                    entry_point: Some("main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    cache: None,
                });

            GpuState {
                device: Arc::clone(device),
                queue: Arc::clone(queue),
                input_buffer,
                output_buffer,
                readback_buffer,
                compute_pipeline,
                bind_group,
            }
        }));

        state.ok()
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Display values in dBFS with ballistics applied.
    pub fn bins(&self) -> &[f32] {
        &self.bins
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

        let used_gpu = if self.gpu.is_some() {
            self.pack_window();
            self.dispatch_gpu_magnitude()
        } else {
            false
        };

        if !used_gpu {
            if self
                .fft
                .process_with_scratch(&mut self.input, &mut self.output, &mut self.scratch)
                .is_err()
            {
                return;
            }
            self.compute_magnitudes_cpu();
        }

        self.aggregate_to_display_bins();
    }

    fn pack_window(&mut self) {
        for (slot, sample) in self
            .gpu_input_bytes
            .chunks_exact_mut(size_of::<f32>())
            .zip(self.input.iter())
        {
            slot.copy_from_slice(&sample.to_ne_bytes());
        }

        if let Some(gpu) = self.gpu.as_ref() {
            let _ = gpu
                .queue
                .write_buffer(&gpu.input_buffer, 0, &self.gpu_input_bytes);
        }
    }

    fn dispatch_gpu_magnitude(&mut self) -> bool {
        let Some(gpu) = self.gpu.as_ref() else {
            return false;
        };

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("spectrum_magnitude_encoder"),
            });

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("spectrum_magnitude_pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&gpu.compute_pipeline);
            pass.set_bind_group(0, &gpu.bind_group, &[]);
            let workgroups = (COMPLEX_BINS as u32 + WORKGROUP_SIZE - 1) / WORKGROUP_SIZE;
            pass.dispatch_workgroups(workgroups, 1, 1);
        }

        encoder.copy_buffer_to_buffer(&gpu.output_buffer, 0, &gpu.readback_buffer, 0, MAG_BYTES);

        gpu.queue.submit(Some(encoder.finish()));

        let slice = gpu.readback_buffer.slice(..);
        let (tx, rx) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());

        let ok = match rx.recv() {
            Ok(Ok(())) => {
                let data = slice.get_mapped_range();
                for (dst, chunk) in self
                    .fft_magnitudes
                    .iter_mut()
                    .zip(data.chunks_exact(size_of::<f32>()).take(COMPLEX_BINS))
                {
                    *dst = f32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                }
                drop(data);
                true
            }
            _ => false,
        };

        if ok {
            gpu.readback_buffer.unmap();
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
    fn bin_ranges_cover_expected_band() {
        let sample_rate = 48_000.0f32;
        let ranges = compute_ranges(sample_rate);
        let bin_hz = sample_rate / FFT_LEN as f32;
        let freq_max_bin = ((FREQ_MAX / bin_hz).ceil() as usize).clamp(1, FFT_LEN / 2);

        assert_eq!(ranges.len(), DISPLAY_BINS);
        assert!(ranges[0].0 <= 1 || ranges[0].1 >= 1);
        assert!(ranges.iter().all(|&(_lo, hi)| _lo < hi));
        assert!(ranges.iter().all(|&(_lo, _hi)| _lo <= FFT_LEN / 2));
        assert!(ranges.iter().all(|&(_lo, hi)| hi <= FFT_LEN / 2 + 1));
        assert!(ranges.last().unwrap().1 >= freq_max_bin);
    }
}
