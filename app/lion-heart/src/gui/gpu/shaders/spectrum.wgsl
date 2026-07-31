@group(0) @binding(0)
var<storage, read> input_fft: array<vec2<f32>>;

@group(0) @binding(1)
var<storage, read_write> output_mag: array<f32>;

const COMPLEX_BINS: u32 = 2049u;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let idx = global_id.x;
    if (idx >= COMPLEX_BINS) {
        return;
    }

    let c = input_fft[idx];
    output_mag[idx] = c.x * c.x + c.y * c.y;
}
