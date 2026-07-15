# Test fixtures

- `reference.nam` — a small real WaveNet capture (131 weights, 48 kHz,
  loudness −20.02 dB). Taken from the [nam-rs](https://github.com/OpenSauce/nam-rs)
  test fixtures (MIT), which in turn vendored it from NAM Core's
  [`example_models/wavenet.nam`](https://github.com/sdatkinson/NeuralAmpModelerCore)
  (MIT). Used to test loading, rate validation, loudness normalization, and
  real-time processing.
