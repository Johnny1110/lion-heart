# Lion-Heart

**An open-source guitar amp & multi-effects processor for macOS, written in Rust.**

Plug your guitar into an audio interface, shape your tone in software — noise gate to high-gain amp stack to ambient delays — and send it back out. Built for two jobs: recording guitars, and replacing the floor modeler on stage.

> **Status: M1 — first pedals (pre-alpha).** A chain of hand-written DSP — noise gate →
> drive (4× oversampled waveshaper) → delay — runs in real time under `lion-heart jam`,
> with live click-free parameter control from a REPL and a zero-allocation audio thread
> (enforced by `assert_no_alloc` in debug builds). M0's latency tooling included.
> The complete technical plan lives in the [white paper](docs/white-paper.md)
> (Traditional Chinese / 繁體中文).

## Why

- **Own your rig.** Commercial amp-sim software is closed; hardware modelers are expensive and fixed-function. Lion-Heart is a rig you can read, patch, and extend.
- **Stand on the NAM ecosystem.** [Neural Amp Modeler](https://github.com/sdatkinson/NeuralAmpModelerCore) captures and cabinet IRs provide world-class tones from day one, so Lion-Heart's own effort goes into the engine, the pedals around the amp, and the playing experience.
- **Latency-obsessed.** Built for live playing: target ≤ 10 ms round-trip at 48 kHz / 64-sample buffers, with measurement tooling built first (M0) so the number is proven, not assumed.
- **A learning vehicle.** Every pedal is hand-written DSP, developed together with the research notes behind it.

## Planned features (MVP scope)

- **Amp & cab** — load `.nam` captures (WaveNet / LSTM) and cabinet IRs (partitioned FFT convolution)
- **Hand-built pedals** — noise gate, compressor, drive/boost (oversampled waveshaping + tone stack), EQ, chorus / flanger / phaser / tremolo, delay, reverb
- **Utilities** — tuner, input/output metering, output limiter (speaker & ear safety)
- **Chain** — reorderable signal chain, per-slot bypass, glitch-free preset switching
- **Presets** — versioned JSON, referencing NAM/IR assets by path + content hash
- **Later** — MIDI foot control (program change / CC / expression), CLAP + VST3 plugin builds

## Signal path

```
guitar ─▶ interface ─▶ [ gate → comp → drive → NAM amp → EQ → mod → delay → reverb → cab IR → limiter ] ─▶ interface ─▶ monitors / FOH
```

## Architecture in five lines

1. The real-time audio thread runs the DSP chain inside the CoreAudio (cpal) callback — it never allocates, locks, or blocks.
2. The UI and asset workers talk to it only through lock-free queues, triple buffers, and atomic pointer swaps.
3. Heavy work (parsing `.nam`, building convolvers) happens on worker threads; finished objects are swapped in atomically and retired objects are dropped off-thread.
4. Parameters are ID-addressed, normalized, and smoothed per-sample on the audio thread.
5. The engine is UI-agnostic: engine crates never depend on the GUI, so the interface can evolve (or be replaced) without touching the sound.

## Tech stack

| Area                  | Choice                                                        | Notes                                                          |
| --------------------- | ------------------------------------------------------------- | -------------------------------------------------------------- |
| Language              | Rust                                                          | see white paper §5.1 for the Rust-vs-C++ decision record        |
| Audio I/O             | [cpal](https://github.com/RustAudio/cpal) (CoreAudio backend) | escape hatch: `coreaudio-rs` for macOS-specific control         |
| NAM inference         | [nam-rs](https://lib.rs/crates/nam-rs)                        | pure-Rust, RT-safe; fallback: FFI to NeuralAmpModelerCore (C++) |
| IR convolution        | [fft-convolver](https://github.com/neodsp/fft-convolver)      | uniform-partitioned FFT, zero latency, RT-safe                  |
| GUI                   | [iced](https://iced.rs) (primary candidate) / [vizia](https://github.com/vizia/vizia) | decided by a spike in M4; `egui` allowed for internal dev tools |
| Plugin export (later) | [nih-plug](https://github.com/robbert-vdh/nih-plug)           | CLAP + VST3 (note: VST3 builds are GPLv3)                       |
| MIDI (later)          | midir / coremidi                                              | foot controller: program change, CC, expression                 |

## Roadmap

Milestones are **completion units, not dates** (this is a burst-driven side project). Each one ends with something playable.

| Milestone | Name             | Exit criteria                                                              |
| --------- | ---------------- | --------------------------------------------------------------------------- |
| M0 ✅     | First sound      | Duplex passthrough; measured round-trip latency report; xrun counter        |
| M1 ✅     | First pedal      | Gate + drive (oversampled) + basic delay; glitch-free param changes; offline test harness |
| M2        | The amp          | `.nam` loading + IR cab + gain staging + safety limiter — a record-worthy tone |
| M3        | Chain & memory   | Reorder/bypass chain; JSON presets; click-free preset switching             |
| M4        | The face         | Product-grade GUI (iced-vs-vizia spike first); tuner; metering              |
| M5        | Full pedalboard  | Modulation family, reverb (FDN), compressor, EQ                             |
| M6        | On stage         | MIDI foot control; live view; 32-sample-buffer performance hardening        |
| M7        | Plugin & release | CLAP/VST3 via nih-plug; codesign + notarization; CI releases; v0.1          |
| M8+       | Deep water       | WDF circuit modeling research, convolution reverb, Windows/Linux ports      |

## Non-goals (for now)

- Windows / Linux at MVP (the design stays portable — cpal/iced/nam-rs are cross-platform — but ports come after M7)
- AU and AAX plugin formats; mobile
- Building our own capture-training UI (use the upstream NAM trainer)

## Repository layout (planned)

```
crates/
  lh-core      # param IDs, chain model, preset schema — no I/O, no threads
  lh-dsp       # hand-written effects; offline-testable, RT-safe
  lh-engine    # RT graph runner, node lifecycle, lock-free plumbing
  lh-nam       # AmpModel trait + nam-rs integration
  lh-io        # cpal device management, latency measurement
  lh-assets    # worker-side loading: .nam, IR wav, convolver building
app/
  lion-heart   # the standalone GUI application
docs/
  white-paper.md   # the plan (zh-TW) — authoritative
  adr/             # decision records for deltas against the white paper
```

## Documentation

- [White paper / 白皮書](docs/white-paper.md) — vision, requirements, architecture, tech choices, milestones (Traditional Chinese)
- [CLAUDE.md](CLAUDE.md) — engineering conventions, including the non-negotiable real-time audio rules
- `docs/adr/` — architecture decision records (created as decisions happen)

## Building & running (M0)

Requires stable Rust (macOS; Linux also builds, given `libasound2-dev` + `pkg-config`).

```sh
cargo build --release

# list audio devices and their capabilities
cargo run -p lion-heart -- devices

# duplex passthrough: guitar in → guitar out (Ctrl-C to stop)
cargo run -p lion-heart --release -- run --buffer 64

# play through the pedalboard (gate → drive → delay) with a live control REPL
cargo run -p lion-heart --release -- jam --buffer 64
#   > set drive.drive 24        # dB, smoothed — no clicks
#   > set delay.time 500        # ms, tape-style pitch slew
#   > off gate / on gate        # crossfaded bypass

# measure round-trip latency (needs a loopback cable: interface out → in)
cargo run -p lion-heart --release -- latency --buffer 64 --markdown

# per-block DSP cost (criterion)
cargo bench -p lh-dsp --bench effects
```

Pick devices with `--input/--output` (index or name substring), e.g.
`--input scarlett --output scarlett`. Measured RTL numbers are logged in
[docs/latency.md](docs/latency.md).

### Troubleshooting

- **"does not support 48000 Hz"** — the *system default* device is probably not your
  interface: Bluetooth/continuity microphones only run at 16–24 kHz, HDMI outputs are
  often 44.1 kHz-only. Run `lion-heart devices`, then select your interface for **both**
  sides: `--input <name> --output <name>`. `--sample-rate 0` follows the device default.
- **Periodic clicks when input and output are different hardware** — two devices means
  two clocks, and they drift. Use the same interface for both sides (the normal rig).

## License

Application code: **MIT OR Apache-2.0** (dual). Future VST3-bundled builds will be distributed under **GPLv3** as required by the VST3 SDK licensing; the CLAP build and the standalone app are unaffected.

## Acknowledgments

- [Steven Atkinson](https://github.com/sdatkinson) and the Neural Amp Modeler community
- [HiFi-LoFi FFTConvolver](https://github.com/HiFi-LoFi/FFTConvolver) and its Rust port by neodsp
- [robbert-vdh](https://github.com/robbert-vdh/nih-plug) for nih-plug, and the RustAudio community
