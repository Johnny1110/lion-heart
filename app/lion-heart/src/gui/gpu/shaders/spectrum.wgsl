@group(0) @binding(0)
var<storage, read> input_signal: array<f32>;

@group(0) @binding(1)
var<storage, read_write> output_mag: array<f32>;

const FFT_LEN: f32 = 4096.0;
const COMPLEX_BINS: u32 = 2049u;
const TAU: f32 = 6.283185307179586;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let idx = global_id.x;
    if (idx >= COMPLEX_BINS) {
        return;
    }

    let k = f32(idx);
    var re: f32 = 0.0;
    var im: f32 = 0.0;
    for (var n: u32 = 0u; n < u32(FFT_LEN); n = n + 1u) {
        let sample = input_signal[n];
        let phase = TAU * k * f32(n) / FFT_LEN;
        re += sample * cos(phase);
        im += sample * sin(phase);
    }

    output_mag[idx] = re * re + im * im;
}
