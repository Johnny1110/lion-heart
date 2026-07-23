# Lion-Heart — Installation Guide

Lion-Heart is a guitar amp & multi-effects processor. This guide gets it running
on **macOS** and **Windows** — as a standalone app, and as a CLAP/VST3 plugin in
your DAW.

> **Today you install by building from source.** There are no pre-built
> downloads yet — the first tagged release (v0.1) is still pending hardware
> verification. Building is a single command once Rust is installed, and this
> guide walks through every step.
>
> **Windows support is brand new** (landed 2026-07-23,
> [ADR 027](adr/027-cross-platform-port.md)). It builds and its file paths are
> portable, but it has not yet been verified on a Windows CI runner or real
> audio hardware. If something doesn't work,
> [please open an issue](https://github.com/Johnny1110/lion-heart/issues).

## What you'll need

- An **audio interface** (USB or Thunderbolt). Lion-Heart runs at **48 kHz**;
  a laptop's built-in mic or a Bluetooth headset won't work for real playing.
- A **guitar** into the interface's instrument (Hi-Z) input.
- Headphones or monitors on the interface's output.
- A **NAM amp capture** (`.nam`, 48 kHz) and a **cabinet IR** (`.wav`) to load —
  Lion-Heart ships the engine, not the tones. Free ones are all over
  [ToneHunt](https://tonehunt.org) and the NAM community.

---

## Step 1 — Install Rust

Lion-Heart builds with the Rust toolchain (Rust edition 2024 → **Rust 1.85 or
newer**). The easiest installer is [rustup](https://rustup.rs).

### macOS

Install Apple's command-line tools (they provide the C linker Rust needs, plus
`git`), then Rust:

```sh
xcode-select --install
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Open a **new** terminal (so `~/.cargo/bin` is on your `PATH`) and confirm:

```sh
rustc --version   # 1.85.0 or newer
```

### Windows

1. Download and run **[`rustup-init.exe`](https://rustup.rs)**.
2. When it offers to install the Visual Studio C++ prerequisites, **accept** —
   Rust's default MSVC toolchain needs them. (Alternatively, install the
   "Desktop development with C++" workload from the
   [Visual Studio Build Tools](https://visualstudio.microsoft.com/downloads/)
   beforehand.)
3. Install [Git for Windows](https://git-scm.com/download/win) to fetch the
   source.
4. Reopen your terminal (PowerShell or Windows Terminal) and confirm:

   ```powershell
   rustc --version   # 1.85.0 or newer
   ```

---

## Step 2 — Get the source

```sh
git clone https://github.com/Johnny1110/lion-heart.git
cd lion-heart
```

---

## Step 3 — Build & run

The first build compiles every dependency and takes a few minutes; subsequent
builds are fast.

### macOS

The `Makefile` wraps the common flows:

```sh
make run          # build (release) and launch the GUI
```

…or drive cargo directly:

```sh
cargo run -p lion-heart --release
```

To put a short `lh` launcher on your `PATH` so you can start it from anywhere:

```sh
make install      # copies the release binary to ~/.cargo/bin/lh
lh                # …then run it
```

### Windows

There is no `make` on Windows by default, so use cargo directly:

```powershell
cargo run -p lion-heart --release
```

The built executable lands at `target\release\lion-heart.exe` — copy it anywhere
and double-click to launch once it's built.

Your presets and settings live in `%USERPROFILE%\.lion-heart\`
(e.g. `C:\Users\you\.lion-heart\`) — the same layout as macOS's `~/.lion-heart/`.

---

## Step 4 — First run

1. **Pick your interface.** Lion-Heart starts on the system default devices. If
   you see **"does not support 48000 Hz"**, the default isn't your interface
   (Bluetooth/continuity mics run at 16–24 kHz; HDMI outputs are often
   44.1 kHz-only). List devices and select yours for **both** sides:

   ```sh
   cargo run -p lion-heart -- devices
   cargo run -p lion-heart --release -- --input <name> --output <name>
   ```

   `<name>` is a device index or a name substring (e.g. `--input scarlett`). You
   can also choose devices in the GUI's **settings** panel — it remembers them,
   so you only do this once.

2. **Load a tone.** In the GUI, open the **amp** slot and browse for your `.nam`
   capture, then the **cab** slot for your `.wav` IR. (NAM captures are
   rate-locked — use 48 kHz models.)

3. **Play.** Watch the footer meters; the **`xruns`** counter should stay at `0`.
   If it climbs, raise the buffer size (settings panel, or `--buffer 128`).

> **Use one interface for both input and output.** Two different devices means
> two clocks that drift apart, which produces periodic clicks.

---

## Installing the plugin (CLAP / VST3)

Lion-Heart also builds as a plugin for your DAW:

```sh
cargo xtask bundle lion-heart-plugin --release
```

This produces `target/bundled/Lion-Heart.clap` and `target/bundled/Lion-Heart.vst3`.
Copy them into your system's plugin folders, then rescan in your DAW:

| Format | macOS | Windows |
| --- | --- | --- |
| CLAP | `~/Library/Audio/Plug-Ins/CLAP/` | `C:\Program Files\Common Files\CLAP\` |
| VST3 | `~/Library/Audio/Plug-Ins/VST3/` | `C:\Program Files\Common Files\VST3\` |

> The **VST3 build is GPLv3** (VST3 SDK licensing); the CLAP build and the
> standalone app stay MIT OR Apache-2.0. Plugin v1 has no custom editor
> (parameters show in the host's generic UI) and a fixed chain order — see
> [docs/release.md](release.md) for the current plugin limitations.

---

## Pre-built binaries (coming with v0.1)

Once v0.1 is tagged, macOS builds will appear on the
[Releases page](https://github.com/Johnny1110/lion-heart/releases). If a macOS
download is **unsigned**, Gatekeeper quarantines it — clear the flag once after
unpacking:

```sh
xattr -dr com.apple.quarantine <file>
```

Windows and Linux release binaries are planned but not built yet
([ADR 027](adr/027-cross-platform-port.md)) — build from source in the meantime.

---

## Updating

```sh
git pull
cargo run -p lion-heart --release   # macOS users can also just `make run`
```

---

## Uninstalling

- The app is self-contained in the repo's `target/` folder — delete the repo to
  remove it.
- If you ran `make install`: `make uninstall` (or delete `~/.cargo/bin/lh`).
- Presets/settings in `~/.lion-heart/` (macOS) / `%USERPROFILE%\.lion-heart\`
  (Windows) are left untouched — delete that folder too for a clean sweep.

---

## Linux (not a target platform, but it builds)

Install the ALSA development headers first, then follow the macOS cargo steps:

```sh
sudo apt install libasound2-dev pkg-config   # Debian/Ubuntu
cargo run -p lion-heart --release
```

Audio goes through ALSA. JACK/PipeWire and pre-built binaries are future work
([ADR 027](adr/027-cross-platform-port.md)).

---

## Troubleshooting

| Symptom | Fix |
| --- | --- |
| `error: package requires rustc 1.85` (or newer) | Update Rust: `rustup update stable`. |
| Windows build fails with a **linker error** | Install the Visual Studio C++ Build Tools ("Desktop development with C++"), then rebuild. |
| **"does not support 48000 Hz"** | The default device isn't your interface — select it explicitly for **both** sides (Step 4). `--sample-rate 0` follows the device default. |
| Periodic **clicks** | Use the *same* interface for input and output (two devices = two drifting clocks). |
| Crackle / **`xruns`** climbing | Raise the buffer size: `--buffer 128` (or `256`). |
| The **amp slot stays silent** / "passthrough" | The `.nam` capture's sample rate doesn't match the engine — use a **48 kHz** model. |

Still stuck? Open an issue at
<https://github.com/Johnny1110/lion-heart/issues>.
