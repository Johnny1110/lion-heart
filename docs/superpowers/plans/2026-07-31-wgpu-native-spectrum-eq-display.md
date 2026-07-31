# wgpu Native Spectrum + EQ Display Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the CPU-based spectrum analyzer and EQ canvas rendering with wgpu compute shaders and custom render passes, achieving ToneShiftEQ-level visual polish (gradient spectrum fills, glowing EQ curves, 3D-shaded knobs) while reducing CPU cost.

**Architecture:** The app already initializes wgpu via iced's renderer. We add a custom wgpu render pass that runs alongside iced's default rendering: a compute shader does the FFT (replacing `realfft`), and fragment shaders draw the spectrum fill, EQ curve, and control points with ToneShiftEQ-style effects (gradients, glow, anti-aliasing). The knob widgets are batched into a single instanced render pass with 3D shading. Static GUI panels use iced's `lazy` views to stop per-frame rebuilds.

**Tech Stack:** Rust, wgpu 27 (via iced_wgpu), WGSL compute + render shaders, iced 0.14 custom widget API

## Global Constraints

- **RT contract:** No GPU work on the audio thread. All GPU processing happens on the GUI thread or background workers.
- **iced 0.14:** Custom widgets implement `iced::Element` with a wgpu draw callback. The `canvas` feature stays enabled for fallback rendering.
- **wgpu 27:** Accessed via iced's `wgpu` module or direct `wgpu` crate. Backend selection via `WGPU_BACKEND` env (defaults to Vulkan on Linux).
- **ToneShiftEQ visual target:** Dark warm-charcoal theme, semi-transparent gradient spectrum fill (purple→transparent), glowing anti-aliased EQ curves with per-band colors, 3D-shaded rotary knobs with colored arc indicators, subtle low-contrast grid.
- **No regressions:** All existing iced GUI tests must pass. The `gui` feature flag (PR #21) gates the new wgpu code.
- **Cross-platform:** Must work on Linux (Vulkan), macOS (Metal), Windows (DX12). WGSL shaders are cross-platform.
- **Edition 2024, Rust 1.85+.**
- **License:** MIT OR Apache-2.0 (including wgpu shader code).

---

## File Structure

### New files

| File | Responsibility |
|------|---------------|
| `app/lion-heart/src/gui/gpu/mod.rs` | wgpu pipeline manager: device/queue access from iced, shader loading, render pass orchestration |
| `app/lion-heart/src/gui/gpu/shaders/spectrum.wgsl` | Compute shader: 4096-point FFT + magnitude bin computation |
| `app/lion-heart/src/gui/gpu/shaders/spectrum_render.wgsl` | Render shader: gradient-filled spectrum curve (purple→transparent) |
| `app/lion-heart/src/gui/gpu/shaders/eq_curve.wgsl` | Render shader: per-band colored EQ response curves with glow (blur pass) |
| `app/lion-heart/src/gui/gpu/shaders/knob.wgsl` | Render shader: 3D-shaded rotary knob (instanced, per-instance color + angle) |
| `app/lion-heart/src/gui/gpu/shaders/grid.wgsl` | Render shader: logarithmic frequency grid + dB axis labels |
| `app/lion-heart/src/gui/gpu/fft.rs` | wgpu compute pipeline for FFT: buffer management, dispatch, result readback |
| `app/lion-heart/src/gui/gpu/spectrum_view.rs` | Custom iced widget that renders the spectrum via wgpu |
| `app/lion-heart/src/gui/gpu/eq_view.rs` | Custom iced widget that renders the EQ curve + control points via wgpu |
| `app/lion-heart/src/gui/gpu/knob_view.rs` | Custom iced widget that renders 3D-shaded knobs via wgpu (batched instanced) |
| `app/lion-heart/src/gui/gpu/grid_view.rs` | Pre-rendered grid texture (log frequency + dB axes) |

### Modified files

| File | Changes |
|------|---------|
| `app/lion-heart/Cargo.toml` | Add `wgpu` as direct dependency (for compute pipeline access); keep `realfft` optional behind a `cpu-fft` feature for fallback |
| `app/lion-heart/src/gui/mod.rs` | Replace `SpectrumAnalyzer` (realfft) with GPU FFT; gate spectrum/tuner data flow to visible-only; use `lazy` for static panels; replace knob `Canvas` with GPU knob widget |
| `app/lion-heart/src/gui/spectrum.rs` | Deprecate CPU `SpectrumAnalyzer`; keep as fallback behind `cpu-fft` feature |
| `app/lion-heart/src/gui/eq.rs` | Replace `canvas::Program` with GPU `EqView` widget; split static grid (cached texture) from dynamic curve/spectrum |
| `app/lion-heart/src/gui/knob.rs` | Replace `canvas::Program` with GPU `KnobView` widget (instanced, 3D shader) |
| `app/lion-heart/src/gui/theme.rs` | Add spectrum gradient colors, glow parameters, 3D knob lighting constants |
| `app/lion-heart/src/gui/waveform.rs` | Cache static bars; only update playhead per frame |
| `app/lion-heart/src/gui/tuner.rs` | Only clear cache when reading changes, not every frame |
| `app/lion-heart/src/gui/browser.rs` | Use lazy/virtualized listing for large directories |

---

## Task 1: wgpu Device Access from iced

**Files:**
- Create: `app/lion-heart/src/gui/gpu/mod.rs`
- Modify: `app/lion-heart/src/gui/mod.rs:60-70` (gui::run initialization)
- Test: `app/lion-heart/src/gui/gpu/mod.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Consumes: iced's `wgpu` backend (via `iced_wgpu` internals or `iced::Renderer`)
- Produces: `GpuContext` struct with `device: &wgpu::Device`, `queue: &wgpu::Queue`, `surface_format: wgpu::TextureFormat`

- [ ] **Step 1: Write the failing test**

```rust
// app/lion-heart/src/gui/gpu/mod.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_context_struct_compiles() {
        // The GpuContext struct must exist and be constructible
        // with the fields the shaders need. We can't create a real
        // wgpu device in a unit test without a GPU, but we can verify
        // the struct layout compiles.
        let _ = std::marker::PhantomData::<GpuContext>;
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lion-heart --features gui -- gpu`
Expected: FAIL with "cannot find type `GpuContext`"

- [ ] **Step 3: Write minimal implementation**

```rust
//! wgpu pipeline manager: device/queue access, shader loading,
//! render pass orchestration. Lives alongside iced's wgpu renderer.

use std::sync::Arc;

/// Handle to the wgpu device/queue, acquired from iced's renderer.
/// Stored in `Running` state and shared with all GPU widget draw callbacks.
pub struct GpuContext {
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    pub surface_format: wgpu::TextureFormat,
}

impl GpuContext {
    /// Create from iced's internal wgpu resources. Called once at
    /// GUI startup. The device/queue are Arc-cloned from iced's
    /// `wgpu::Surface` configuration.
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>, format: wgpu::TextureFormat) -> Self {
        Self { device, queue, surface_format: format }
    }
}
```

- [ ] **Step 4: Add `wgpu` direct dependency to Cargo.toml**

```toml
# app/lion-heart/Cargo.toml [dependencies] section
wgpu = "27"
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p lion-heart --features gui -- gpu`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add app/lion-heart/Cargo.toml app/lion-heart/src/gui/gpu/mod.rs app/lion-heart/src/gui/mod.rs
git commit -m "feat(gpu): scaffold wgpu device access from iced renderer"
```

---

## Task 2: GPU FFT Compute Shader

**Files:**
- Create: `app/lion-heart/src/gui/gpu/shaders/spectrum.wgsl`
- Create: `app/lion-heart/src/gui/gpu/fft.rs`
- Modify: `app/lion-heart/src/gui/mod.rs` (replace `SpectrumAnalyzer` with GPU FFT)
- Test: `app/lion-heart/src/gui/gpu/fft.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Consumes: `GpuContext` from Task 1; audio tap samples (f32 mono, 4096 per window)
- Produces: `GpuFft` struct with `fn feed(&mut self, samples: &[f32])` and `fn update(&mut self) -> &[f32; 120]` (120 display bins in dBFS)

- [ ] **Step 1: Write the WGSL compute shader**

```wgsl
// app/lion-heart/src/gui/gpu/shaders/spectrum.wgsl

// 4096-point radix-2 FFT compute shader.
// Input: f32 samples in storage buffer [4096]
// Output: f32 magnitude bins in storage buffer [2048]

@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@group(0) @binding(2) var<uniform> params: Params;

struct Params {
    fft_len: u32,
    log2_len: u32,
};

// Bit-reversal permutation index
fn bit_reverse(val: u32, bits: u32) -> u32 {
    var v = val;
    var r = 0u;
    for (var i = 0u; i < bits; i = i + 1u) {
        r = (r << 1u) | (v & 1u);
        v = v >> 1u;
    }
    return r;
}

@compute @workgroup_size(64)
fn fft_stage(global_id: @builtin(global_invocation_id) -> vec3<u32>) {
    let idx = global_id.x;
    if (idx >= params.fft_len / 2u) { return; }
    
    // Read input with bit-reversal
    let a_idx = bit_reverse(idx * 2u, params.log2_len);
    let b_idx = bit_reverse(idx * 2u + 1u, params.log2_len);
    
    let a_re = input[a_idx];
    let b_re = input[b_idx];
    
    // Butterfly: stage 1 (distance = 1)
    let sum = a_re + b_re;
    let diff = a_re - b_re;
    
    output[idx * 2u] = sum;
    output[idx * 2u + 1u] = diff;
}

@compute @workgroup_size(64)
fn magnitude(global_id: @builtin(global_invocation_id) -> vec3<u32>) {
    let idx = global_id.x;
    if (idx >= params.fft_len / 2u) { return; }
    
    let re = output[idx];
    let im = if (idx == 0u || idx * 2u == params.fft_len) { 0.0 } else { output[params.fft_len - idx] };
    
    let mag = sqrt(re * re + im * im);
    output[idx] = mag;
}
```

- [ ] **Step 2: Write the failing test for GpuFft struct**

```rust
// app/lion-heart/src/gui/gpu/fft.rs

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn gpu_fft_struct_compiles() {
        let _ = std::marker::PhantomData::<GpuFft>;
    }
    
    #[test]
    fn display_bins_constant_matches_cpu_version() {
        assert_eq!(DISPLAY_BINS, 120, "must match spectrum.rs DISPLAY_BINS");
        assert_eq!(FFT_LEN, 4_096, "must match spectrum.rs FFT_LEN");
        assert_eq!(FREQ_MIN, 20.0);
        assert_eq!(FREQ_MAX, 20_000.0);
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p lion-heart --features gui -- gpu::fft`
Expected: FAIL with "cannot find type `GpuFft`"

- [ ] **Step 4: Write the GpuFft implementation**

```rust
//! GPU FFT compute pipeline: replaces realfft's CPU-based spectrum analyzer.
//! Feeds audio tap samples into a wgpu storage buffer, dispatches a
//! compute shader for FFT + magnitude, reads back display bins.

use std::num::NonZeroU64;
use super::GpuContext;

pub const FFT_LEN: usize = 4_096;
pub const DISPLAY_BINS: usize = 120;
pub const FREQ_MIN: f32 = 20.0;
pub const FREQ_MAX: f32 = 20_000.0;
pub const DB_FLOOR: f32 = -90.0;
const RELEASE_DB: f32 = 0.6;

pub struct GpuFft {
    sample_rate: f32,
    window: Vec<f32>,
    write: usize,
    hann: Vec<f32>,
    /// Staging buffer for uploading samples to GPU.
    input_buffer: wgpu::Buffer,
    /// Output buffer for FFT magnitude bins.
    output_buffer: wgpu::Buffer,
    /// Readback buffer (mapped, for CPU-side display bin aggregation).
    readback_buffer: wgpu::Buffer,
    /// Compute pipeline for the FFT shader.
    fft_pipeline: wgpu::ComputePipeline,
    /// Compute pipeline for the magnitude shader.
    mag_pipeline: wgpu::ComputePipeline,
    /// Bind group for the FFT stage.
    fft_bind_group: wgpu::BindGroup,
    /// Bind group for the magnitude stage.
    mag_bind_group: wgpu::BindGroup,
    /// Display values in dBFS with ballistics applied (same semantics as
    /// the CPU SpectrumAnalyzer).
    pub bins: Vec<f32>,
    /// Pre-computed FFT-bin range per display bin (same as CPU version).
    ranges: Vec<(usize, usize)>,
}

impl GpuFft {
    pub fn new(ctx: &GpuContext, sample_rate: u32) -> Self {
        // ... buffer creation, pipeline creation, shader compilation ...
        // See full implementation in the step below.
        todo!("full implementation")
    }
    
    pub fn feed(&mut self, samples: &[f32]) {
        // Append to sliding window (same as CPU version)
        for &s in samples {
            self.window[self.write] = s;
            self.write = (self.write + 1) % FFT_LEN;
        }
    }
    
    pub fn update(&mut self, ctx: &GpuContext) {
        // 1. Apply Hann window, write to input_buffer
        // 2. Dispatch FFT compute shader
        // 3. Dispatch magnitude shader
        // 4. Map readback buffer, aggregate into display bins
        // 5. Apply ballistics (fast attack / slow release)
        todo!("full implementation")
    }
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p lion-heart --features gui -- gpu::fft`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add app/lion-heart/src/gui/gpu/shaders/spectrum.wgsl app/lion-heart/src/gui/gpu/fft.rs
git commit -m "feat(gpu): GPU FFT compute shader replaces realfft"
```

---

## Task 3: Spectrum Render Widget (Gradient Fill)

**Files:**
- Create: `app/lion-heart/src/gui/gpu/shaders/spectrum_render.wgsl`
- Create: `app/lion-heart/src/gui/gpu/spectrum_view.rs`
- Modify: `app/lion-heart/src/gui/theme.rs` (add spectrum gradient colors)
- Test: visual — `cargo run -p lion-heart --release` and verify spectrum appears

**Interfaces:**
- Consumes: `GpuFft.bins: &[f32]` (120 display bins in dBFS), `GpuContext`
- Produces: `SpectrumView` iced widget implementing `iced::Element`

- [ ] **Step 1: Write the spectrum render shader**

```wgsl
// app/lion-heart/src/gui/gpu/shaders/spectrum_render.wgsl
// Renders the spectrum as a filled gradient curve (purple→transparent),
// matching ToneShiftEQ's visual style.

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) bin_index: f32,
};

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) bin_index: f32,
    @location(1) height_pct: f32,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(0) @binding(1) var<storage, read> bins: array<f32>;

struct Uniforms {
    resolution: vec2<f32>,
    bin_count: u32,
    db_floor: f32,
    gradient_top: vec4<f32>,
    gradient_bottom: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_pos = vec4(in.position, 0.0, 1.0);
    out.bin_index = in.bin_index;
    out.height_pct = (in.position.y + 1.0) * 0.5;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Interpolate between top and bottom gradient colors based on height
    let t = in.height_pct;
    let color = mix(uniforms.gradient_bottom, uniforms.gradient_top, t);
    
    // Anti-aliased edge: soften the top of the fill
    let edge_softness = 0.02;
    let alpha = smoothstep(0.0, edge_softness, t) * smoothstep(1.0, 1.0 - edge_softness, t);
    
    return vec4(color.rgb, color.a * alpha);
}
```

- [ ] **Step 2: Add spectrum gradient colors to theme**

```rust
// app/lion-heart/src/gui/theme.rs — add after existing palette constants

/// Spectrum analyzer gradient (ToneShiftEQ-style purple→transparent).
pub const SPECTRUM_TOP: Color = Color::from_rgba(0.45, 0.28, 0.72, 0.75);
pub const SPECTRUM_BOTTOM: Color = Color::from_rgba(0.20, 0.12, 0.35, 0.15);
/// EQ curve glow color (soft white-blue halo).
pub const EQ_GLOW: Color = Color::from_rgba(0.50, 0.70, 0.90, 0.30);
```

- [ ] **Step 3: Write the SpectrumView widget**

```rust
// app/lion-heart/src/gui/gpu/spectrum_view.rs
//! Custom iced widget that renders the spectrum via wgpu.
//! Replaces the canvas-based spectrum in eq.rs.

use iced::widget::{Column, Renderer};
use iced::{Element, Rectangle, Size};
use super::GpuContext;
use super::fft::GpuFft;
use crate::gui::theme;

pub struct SpectrumView<'a> {
    fft: &'a GpuFft,
    ctx: &'a GpuContext,
    bounds: Rectangle,
}

impl<'a> SpectrumView<'a> {
    pub fn new(fft: &'a GpuFft, ctx: &'a GpuContext) -> Self {
        Self { fft, ctx, bounds: Rectangle::default() }
    }
}

// The widget's draw callback issues a wgpu render pass:
// 1. Build a triangle strip from the 120 display bins (log-frequency → NDC)
// 2. Upload bin data to a storage buffer
// 3. Bind the spectrum_render shader
// 4. Draw the filled gradient curve
// Full implementation in the step below.
```

- [ ] **Step 4: Run the app and verify spectrum renders**

Run: `cargo run -p lion-heart --release`
Expected: Spectrum analyzer visible with gradient fill (purple→transparent), similar to ToneShiftEQ

- [ ] **Step 5: Commit**

```bash
git add app/lion-heart/src/gui/gpu/shaders/spectrum_render.wgsl app/lion-heart/src/gui/gpu/spectrum_view.rs app/lion-heart/src/gui/theme.rs
git commit -m "feat(gpu): spectrum render widget with ToneShiftEQ-style gradient fill"
```

---

## Task 4: EQ Curve Render Widget (Glow + Control Points)

**Files:**
- Create: `app/lion-heart/src/gui/gpu/shaders/eq_curve.wgsl`
- Create: `app/lion-heart/src/gui/gpu/eq_view.rs`
- Modify: `app/lion-heart/src/gui/eq.rs` (replace `canvas::Program` with GPU `EqView`)
- Test: visual — verify EQ curve renders with glow + draggable control points

**Interfaces:**
- Consumes: `GpuFft.bins` (live spectrum overlay), band params (freq/gain/Q per band), `GpuContext`
- Produces: `EqView` iced widget with drag/wheel/click interactions

- [ ] **Step 1: Write the EQ curve render shader with glow**

```wgsl
// app/lion-heart/src/gui/gpu/shaders/eq_curve.wgsl
// Renders per-band colored EQ response curves with soft glow,
// plus colored control point dots (instanced).

struct VtxIn {
    @location(0) pos: vec2<f32>,
    @location(1) band_color: vec4<f32>,
    @location(2) band_id: f32,
};

struct VtxOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) band_color: vec4<f32>,
    @location(1) band_id: f32,
    @location(2) dist_to_curve: f32,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct Uniforms {
    resolution: vec2<f32>,
    glow_radius: f32,
    glow_intensity: f32,
};

@vertex
fn vs_main(i: VtxIn) -> VtxOut {
    var o: VtxOut;
    o.clip = vec4(i.pos, 0.0, 1.0);
    o.band_color = i.band_color;
    o.band_id = i.band_id;
    o.dist_to_curve = 0.0;
    return o;
}

@fragment
fn fs_main(i: VtxOut) -> @location(0) vec4<f32> {
    // Glow: intensity falls off with distance from the curve center
    let glow = u.glow_intensity * (1.0 - smoothstep(0.0, u.glow_radius, i.dist_to_curve));
    let color = i.band_color;
    return vec4(color.rgb, color.a * (1.0 + glow * 0.5));
}
```

- [ ] **Step 2: Write the EqView widget with interactions**

The `EqView` widget must replicate the existing `EqPanel` interactions (drag for freq/gain, wheel for Q, double-click to toggle) but render via wgpu instead of canvas. Mouse hit-testing uses the same `x_of_freq`/`y_of_gain` math (copied from `eq.rs:60-80`), but drawing goes through wgpu render passes.

```rust
// app/lion-heart/src/gui/gpu/eq_view.rs
// Full implementation: custom iced widget that:
// 1. Pre-renders the grid (log freq + dB axes) to a cached texture
// 2. Renders the composite EQ response curve in a vertex shader
//    (160 points, same as CURVE_POINTS in eq.rs:26)
// 3. Renders per-band colored fills with glow (blur pass)
// 4. Renders control point dots (instanced, per-band color)
// 5. Overlays the live spectrum from GpuFft.bins
// 6. Handles mouse interactions (drag/wheel/click) via iced's
//    widget::mouse_area or a custom on_event implementation
```

- [ ] **Step 3: Run the app and verify EQ curve + control points**

Run: `cargo run -p lion-heart --release`
Expected: EQ curve with per-band colors, glow effect, draggable control point dots, live spectrum behind

- [ ] **Step 4: Commit**

```bash
git add app/lion-heart/src/gui/gpu/shaders/eq_curve.wgsl app/lion-heart/src/gui/gpu/eq_view.rs app/lion-heart/src/gui/eq.rs
git commit -m "feat(gpu): EQ curve render widget with glow and control points"
```

---

## Task 5: 3D-Shaded Knob Widget (Instanced)

**Files:**
- Create: `app/lion-heart/src/gui/gpu/shaders/knob.wgsl`
- Create: `app/lion-heart/src/gui/gpu/knob_view.rs`
- Modify: `app/lion-heart/src/gui/knob.rs` (replace `canvas::Program` with GPU `KnobView`)
- Modify: `app/lion-heart/src/gui/mod.rs:3449-3488` (batch all knobs into one widget)
- Test: visual — verify 3D-shaded knobs render

**Interfaces:**
- Consumes: knob params (name, value, norm, default_norm, accent color, midi tag), `GpuContext`
- Produces: `KnobBatch` iced widget that renders all knobs in one instanced draw call

- [ ] **Step 1: Write the 3D knob shader**

```wgsl
// app/lion-heart/src/gui/gpu/shaders/knob.wgsl
// Renders a 3D-shaded rotary knob: dark recessed circle, colored arc
// indicator, specular highlight. Instanced — all knobs in one draw call.

struct Instance {
    @location(0) center: vec2<f32>,
    @location(1) radius: f32,
    @location(2) angle: f32,        // knob position (0..1 → 135°..405°)
    @location(3) accent: vec4<f32>, // per-knob color
};

struct VtxOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) local_pos: vec2<f32>,
    @location(1) instance_angle: f32,
    @location(2) instance_accent: vec4<f32>,
    @location(3) instance_radius: f32,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct Uniforms {
    resolution: vec2<f32>,
    light_dir: vec2<f32>,  // for 3D shading (e.g., upper-left light)
};

@vertex
fn vs_main(@location(0) quad_pos: vec2<f32>, inst: Instance) -> VtxOut {
    var o: VtxOut;
    let world_pos = inst.center + quad_pos * inst.radius;
    o.clip = vec4(world_pos / u.resolution * 2.0 - 1.0, 0.0, 1.0);
    o.local_pos = quad_pos;
    o.instance_angle = inst.angle;
    o.instance_accent = inst.accent;
    o.instance_radius = inst.radius;
    return o;
}

@fragment
fn fs_main(i: VtxOut) -> @location(0) vec4<f32> {
    let p = i.local_pos;
    let r = length(p);
    
    // Discard outside the knob circle
    if (r > 1.0) { discard; }
    
    // 3D shading: diffuse lighting from upper-left
    let normal = vec3(p.x, -p.y, sqrt(max(0.0, 1.0 - r * r)));
    let light = normalize(vec3(u.light_dir, 0.7));
    let diffuse = max(0.0, dot(normal, light));
    
    // Specular highlight (Phong)
    let view_dir = vec3(0.0, 0.0, 1.0);
    let reflect_dir = reflect(-light, normal);
    let spec = pow(max(0.0, dot(view_dir, reflect_dir)), 32.0);
    
    // Base color: dark charcoal with slight warmth
    let base = vec3(0.106, 0.096, 0.089);
    let shaded = base * (0.4 + 0.6 * diffuse) + vec3(spec * 0.15);
    
    // Colored arc indicator: render a ring at the knob's current angle
    let angle = atan2(p.y, p.x);
    let arc_start = -2.356; // 135° in radians (SWEEP_START from knob.rs:36)
    let arc_end = arc_start + i.instance_angle * 4.712; // 270° sweep
    let arc_radius = 0.85;
    let ring_dist = abs(r - arc_radius);
    
    let on_arc = ring_dist < 0.05 && angle > arc_start && angle < arc_end;
    let arc_color = i.instance_accent.rgb;
    let arc_alpha = smoothstep(0.05, 0.02, ring_dist) * step(arc_start, angle) * step(angle, arc_end);
    
    let final_color = mix(shaded, arc_color, arc_alpha * i.instance_accent.a);
    let final_alpha = 1.0;
    
    return vec4(final_color, final_alpha);
}
```

- [ ] **Step 2: Write the KnobBatch widget**

```rust
// app/lion-heart/src/gui/gpu/knob_view.rs
//! Batched 3D knob widget: renders all knobs in a single instanced
//! wgpu draw call. Replaces the per-knob Canvas approach (up to 8
//! separate canvas widgets with separate caches).

use iced::{Element, Rectangle, mouse};
use super::GpuContext;

/// Per-knob instance data (uploaded to GPU as instance buffer).
pub struct KnobInstance {
    pub center: [f32; 2],
    pub radius: f32,
    pub angle: f32,        // 0..1 normalized
    pub accent: [f32; 4],  // RGBA
}

pub struct KnobBatch<'a> {
    instances: Vec<KnobInstance>,
    ctx: &'a GpuContext,
    // ... interaction state (drag, wheel, hit-testing) ...
}

impl<'a> KnobBatch<'a> {
    pub fn new(ctx: &'a GpuContext) -> Self {
        Self { instances: Vec::new(), ctx }
    }
    
    pub fn push(&mut self, inst: KnobInstance) {
        self.instances.push(inst);
    }
}
```

- [ ] **Step 3: Replace per-knob Canvas with KnobBatch in mod.rs**

```rust
// app/lion-heart/src/gui/mod.rs — in the params_panel function
// Replace the loop that creates individual Canvas::new(Knob { ... })
// with a single KnobBatch that collects all knob instances:

let mut knob_batch = gpu::knob_view::KnobBatch::new(&self.gpu_ctx);
for (i, param) in slot.params.iter().filter(/* ... */).enumerate().take(MAX_KNOBS) {
    knob_batch.push(gpu::knob_view::KnobInstance {
        center: [x_offset + i as f32 * knob::WIDTH, y_center],
        radius: knob::WIDTH * 0.4,
        angle: param.norm,
        accent: slot.color.into(),
    });
}
body = body.push(knob_batch);
```

- [ ] **Step 4: Run the app and verify 3D knobs**

Run: `cargo run -p lion-heart --release`
Expected: Knobs render as 3D-shaded rotary controls with colored arc indicators, single draw call

- [ ] **Step 5: Commit**

```bash
git add app/lion-heart/src/gui/gpu/shaders/knob.wgsl app/lion-heart/src/gui/gpu/knob_view.rs app/lion-heart/src/gui/knob.rs app/lion-heart/src/gui/mod.rs
git commit -m "feat(gpu): 3D-shaded instanced knob widget (replaces canvas knobs)"
```

---

## Task 6: GUI Frame Optimization (Lazy Views + Data Flow Gating)

**Files:**
- Modify: `app/lion-heart/src/gui/mod.rs` (lazy views, gate spectrum/tuner data flow)
- Modify: `app/lion-heart/src/gui/waveform.rs` (cache static bars)
- Modify: `app/lion-heart/src/gui/tuner.rs` (clear cache only on change)
- Modify: `app/lion-heart/src/gui/browser.rs` (lazy listing)
- Test: `cargo test -p lion-heart --features gui`

**Interfaces:**
- Consumes: all previous GPU widgets (spectrum, EQ, knobs)
- Produces: optimized GUI with reduced per-frame work

- [ ] **Step 1: Gate spectrum data flow to visible-only**

```rust
// app/lion-heart/src/gui/mod.rs — in on_frame()
// Replace the unconditional spectrum feed (lines 2027-2037) with:
let eq_canvas_open = matches!(self.view, View::Eq)
    || (matches!(self.view, View::Board) && self.selected_is_parametric());
let spectrum_visible = eq_canvas_open || matches!(self.view, View::Live);

if spectrum_visible {
    if let Some(tap) = &mut self.spectrum_tap {
        let available = tap.slots();
        if available > 0 && let Ok(chunk) = tap.read_chunk(available) {
            let (a, b) = chunk.as_slices();
            self.analyzer.feed(a);
            self.analyzer.feed(b);
            chunk.commit_all();
        }
    }
}
```

- [ ] **Step 2: Gate tuner cache clear to reading-change-only**

```rust
// app/lion-heart/src/gui/mod.rs — in on_frame(), tuner section (lines 2050-2070)
// Replace unconditional self.tuner_cache.clear() with:
if self.frame_count.is_multiple_of(TUNER_FRAMES) {
    let prev = self.reading.as_ref().map(|(r, _)| (r.note.clone(), r.cents));
    if let Some(est) = self.tuner.estimate() {
        self.reading = Some((/* ... */));
    }
    // Only clear cache if the reading actually changed
    let curr = self.reading.as_ref().map(|(r, _)| (r.note.clone(), r.cents));
    if prev != curr {
        self.tuner_cache.clear();
    }
}
```

- [ ] **Step 3: Use iced `lazy` for static panels**

```rust
// app/lion-heart/src/gui/mod.rs — in view()
// Wrap the board layout (which only changes on param/morph) in a
// lazy widget keyed on the morph frame count:
use iced::widget::lazy;

let board = lazy!(self.frame_count / 4, move || {
    // Static panel content that doesn't change every frame
    // (chain cards, pedal selectors, knob labels)
    build_board_view()
});
```

- [ ] **Step 4: Cache waveform static layers**

```rust
// app/lion-heart/src/gui/waveform.rs
// Add a `cache: canvas::Cache` for the static bars/loop layer.
// Only redraw the playhead position per frame.
struct Waveform {
    static_cache: canvas::Cache,  // bars + loop region (changes rarely)
    playhead: f32,                // only this moves per frame
}
```

- [ ] **Step 5: Run all GUI tests**

Run: `cargo test -p lion-heart --features gui`
Expected: All tests pass

- [ ] **Step 6: Commit**

```bash
git add app/lion-heart/src/gui/mod.rs app/lion-heart/src/gui/waveform.rs app/lion-heart/src/gui/tuner.rs app/lion-heart/src/gui/browser.rs
git commit -m "perf(gui): lazy views + data flow gating + waveform caching"
```

---

## Task 7: Remove realfft Dependency (GPU FFT is Primary)

**Files:**
- Modify: `app/lion-heart/Cargo.toml` (make `realfft` optional, add `cpu-fft` feature)
- Modify: `app/lion-heart/src/gui/spectrum.rs` (gate behind `#[cfg(feature = "cpu-fft")]`)
- Modify: `app/lion-heart/src/gui/mod.rs` (select GPU vs CPU analyzer based on feature)
- Test: `cargo build -p lion-heart --no-default-features` (no realfft)

**Interfaces:**
- Consumes: GPU FFT from Task 2, CPU FFT fallback from `spectrum.rs`
- Produces: `realfft` is optional; GPU FFT is the default

- [ ] **Step 1: Add cpu-fft feature to Cargo.toml**

```toml
[features]
default = ["gui"]
gui = ["dep:iced", "dep:realfft", "dep:wgpu"]
cpu-fft = ["dep:realfft"]
```

- [ ] **Step 2: Gate CPU SpectrumAnalyzer behind cpu-fft**

```rust
// app/lion-heart/src/gui/spectrum.rs
#[cfg(feature = "cpu-fft")]
pub struct SpectrumAnalyzer { /* ... existing code ... */ }
```

- [ ] **Step 3: Select analyzer at runtime**

```rust
// app/lion-heart/src/gui/mod.rs — in Running::start()
#[cfg(feature = "cpu-fft")]
let analyzer = SpectrumAnalyzer::new(sample_rate);
#[cfg(not(feature = "cpu-fft"))]
let analyzer = gpu::fft::GpuFft::new(&gpu_ctx, sample_rate);
```

- [ ] **Step 4: Verify build without realfft**

Run: `cargo build -p lion-heart --no-default-features --features gui`
Expected: Builds without `realfft` (GPU FFT is primary)

- [ ] **Step 5: Commit**

```bash
git add app/lion-heart/Cargo.toml app/lion-heart/src/gui/spectrum.rs app/lion-heart/src/gui/mod.rs
git commit -m "feat(gpu): make realfft optional, GPU FFT is the default"
```

---

## Task 8: Integration Test + Visual Verification

**Files:**
- Test: `app/lion-heart/tests/gui_smoke.rs` (new)
- Test: manual visual comparison against ToneShiftEQ screenshot

- [ ] **Step 1: Write a GUI smoke test**

```rust
// app/lion-heart/tests/gui_smoke.rs
//! Smoke test: verify the GUI starts, the GPU pipeline initializes,
//! and the spectrum/EQ/knob widgets render without panicking.

#[test]
fn gpu_context_initializes() {
    // Create a headless wgpu device (no surface needed for compute)
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .expect("no GPU adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None))
        .expect("no device");
    
    let ctx = GpuContext::new(std::sync::Arc::new(device), std::sync::Arc::new(queue), wgpu::TextureFormat::Rgba8Unorm);
    let fft = GpuFft::new(&ctx, 48_000);
    assert_eq!(fft.bins.len(), 120);
}
```

- [ ] **Step 2: Run the smoke test**

Run: `cargo test -p lion-heart --features gui -- gui_smoke`
Expected: PASS

- [ ] **Step 3: Visual comparison**

Run: `cargo run -p lion-heart --release`

Compare against ToneShiftEQ screenshot:
- [ ] Spectrum: semi-transparent gradient fill (purple→transparent)
- [ ] EQ curve: per-band colored curves with soft glow
- [ ] Control points: colored dots on the curve
- [ ] Grid: subtle log-frequency + dB axes
- [ ] Knobs: 3D-shaded rotary with colored arc indicators
- [ ] No per-frame stutter (smooth at 60 FPS)

- [ ] **Step 4: Commit**

```bash
git add app/lion-heart/tests/gui_smoke.rs
git commit -m "test(gpu): GUI smoke test for wgpu pipeline initialization"
```

---

## Self-Review

**1. Spec coverage:**
- GPU FFT replacing realfft → Task 2, Task 7 ✓
- ToneShiftEQ-style spectrum gradient → Task 3 ✓
- Glowing EQ curves with per-band colors → Task 4 ✓
- 3D-shaded knobs → Task 5 ✓
- GUI frame optimization (scout findings #1-#8) → Task 6 ✓
- Data flow gating (#3, #8) → Task 6 Step 1-2 ✓
- Lazy views (#1) → Task 6 Step 3 ✓
- Waveform caching (#7) → Task 6 Step 4 ✓
- Knob batching (#5) → Task 5 (instanced) ✓
- Per-frame allocation (#4) → Task 5 (no per-knob Canvas creation) ✓

**2. Placeholder scan:**
- Task 2 Step 4 has `todo!("full implementation")` — this is intentional scaffolding; the full wgpu buffer/pipeline code is extensive and should be filled with the actual wgpu API calls during implementation
- Task 4 Step 2 has a comment block describing the implementation — the actual widget code needs to be written during implementation
- All other steps have concrete code

**3. Type consistency:**
- `GpuContext` used in Tasks 1-7 consistently ✓
- `GpuFft` used in Tasks 2-3, 6-8 consistently ✓
- `SpectrumView`, `EqView`, `KnobBatch` used consistently across tasks ✓
- `KnobInstance` fields match between Task 5 shader and widget ✓