# Round-trip latency (RTL) measurements

M0 exit criterion: **measured RTL ≤ 10 ms at 48 kHz / 64-frame buffers** (white paper §3.2).

## How to measure

1. Connect the interface's output back into input 1 with a short cable
   (or enable the interface's hardware loopback/mix mode).
2. Set the input gain to a moderate level; disconnect or mute everything else.
3. Run:

   ```sh
   cargo run -p lion-heart --release -- latency --buffer 64 --markdown
   ```

4. Paste the emitted markdown under **Results** below, newest first, adding the
   date and interface model.

The tool plays ten 1 kHz bursts, detects them on the input after a noise-floor
calibration phase, and reports the median wall-clock round trip — including
ADC/DAC conversion, driver, and all buffer stages. Accuracy is roughly ±0.5 ms
(bounded by callback-scheduling jitter); the min/max spread shows the jitter.

Sweep a few buffer sizes to build the latency/stability curve for the interface:

```sh
for b in 32 64 128 256; do
  cargo run -q -p lion-heart --release -- latency --buffer $b
done
```

## Results

_No measurements yet — run the tool on real hardware (macOS + audio interface)._
