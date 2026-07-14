# DSP benchmarks

Per-block processing cost at the live target format: **48 kHz, 64-frame blocks**,
deadline **1,333 µs** per block (white paper §3.2). Run with:

```sh
cargo bench -p lh-dsp --bench effects
```

## 2026-07-14 — Linux container (aarch64, Docker on Apple Silicon) — indicative only

| Bench                       | Median      | % of 64-frame deadline |
| --------------------------- | ----------- | ---------------------- |
| gate                        | ~409 ns     | 0.03 %                 |
| drive (4× oversampled)      | ~4.99 µs    | 0.37 %                 |
| delay                       | ~388 ns     | 0.03 %                 |
| chain: gate → drive → delay | ~5.43 µs    | **0.41 %**             |

Drive dominates, as expected — the cost is the four half-band FIR passes plus tanh at
4× rate. Headroom for the M2 amp block (NAM ≈ 1.9 µs/sample ⇒ ~122 µs/block ≈ 9 %) and
cab IR convolution remains enormous.

_Add rows measured on real hardware (Apple Silicon, `cargo bench` on macOS) as they come._
