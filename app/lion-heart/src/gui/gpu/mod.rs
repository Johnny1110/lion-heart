//! wgpu pipeline manager module for GUI-side compute paths.
//! Keeps GPU resource wrappers available from the renderer entrypoint.
pub mod fft;
pub mod shaders;

// Module doc comments previously appeared after `mod` declarations; keep
// GPU context definitions grouped with a single top-level module preface.
// The GPU context is consumed by analyzer/effect widgets when needed.

use std::sync::Arc;

/// Handle to the wgpu device/queue, acquired from iced's renderer.
/// Stored in `Running` state and shared with all GPU widget draw callbacks.
#[allow(dead_code)]
pub struct GpuContext {
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    pub surface_format: wgpu::TextureFormat,
}

impl GpuContext {
    /// Create from iced's internal wgpu resources. Called once at
    /// GUI startup.
    #[allow(dead_code)]
    pub fn new(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        Self {
            device,
            queue,
            surface_format,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_context_struct_compiles() {
        // Verify the struct is constructible (can't create a real
        // wgpu device in a unit test without a GPU, but we can verify
        // the struct layout compiles).
        fn _check(_ctx: GpuContext) {}
    }
}
