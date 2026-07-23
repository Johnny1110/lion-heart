# ADR 027: Cross-platform port — Windows & Linux (Windows-first)

Status: **accepted — code-portability landed; Windows CI + hardware verification in progress**
Date: 2026-07-23
Relates to: white paper §2 (可攜性紀律 — the portability discipline this executes)
and §7 M8+ (the deferred "Windows (WASAPI)/Linux (PipeWire/JACK) 移植"), ADR 001
(iced — chosen partly for cross-platform reach), the `~/.lion-heart` disk layout
in `lh-assets` (a cross-binary contract shared by app and plugin)

## Context

The 2026-07-20 nine-feature roadmap is complete and all 19 PRDs are implemented.
The next direction (user-picked) is the white paper's M8+ **portability** item,
and the user chose **Windows-first** — tackle the platform that is genuinely
unverified rather than the one that is nearly free.

A code audit says the discipline held and the port is far more "started" than a
from-scratch effort:

- **Zero platform code in the core.** The whole workspace has exactly one
  `#[cfg(...)]`, and it is `not(debug_assertions)` — nothing keys on
  `target_os`. "CoreAudio" appears only in explanatory comments in `lh-io`.
- **All deps are cross-platform** (cpal, midir, symphonia, hound, iced, ctrlc —
  `ctrlc` already handles Windows via `SetConsoleCtrlHandler`). The GUI file
  browser is iced-drawn, so there is **no native-dialog** portability surface.
- **Linux already builds and tests in CI** (Ubuntu, with ALSA headers), and its
  `$HOME`-based `~/.lion-heart` layout resolves correctly — Linux is effectively
  ready at the code level.
- **The one concrete Windows blocker** is path resolution: `lh_assets::app_dir()`
  read `std::env::var_os("HOME")`, and native Windows has no `$HOME` (it has
  `%USERPROFILE%`). That single chokepoint fans out to presets/config/midi/
  recordings, so on Windows the entire disk layout failed to resolve.
- **Windows was not in CI at all**, so its build status was unknown.

## Decision

Phase the port **Linux (nearly free) → Windows (the real work)**, but do the
Windows code-portability fixes first per the user's choice. This ADR lands the
sandbox-verifiable code changes; the hardware/runner-dependent parts are
explicitly deferred (below).

### Home-directory / disk-layout strategy

**Keep the single `~/.lion-heart` dotfolder on every platform** — do *not* move
to per-OS config dirs (`%APPDATA%`, `~/Library/Application Support`,
`~/.config`). Rationale:

- The `~/.lion-heart` preset list is a **cross-binary contract** (MIDI PC *n*
  and the plugin's preset parameter both index the same sorted list). One layout
  on all platforms keeps that contract and the codebase's entire `~/.lion-heart`
  vocabulary literally true.
- Existing macOS/Linux installs must **never move**.

Resolve `~` **from the environment, dep-free** rather than pulling the `dirs`
crate: a new `lh_assets::home_dir()` returns `$HOME` when set (Unix — byte-
identical to every prior release), else `%USERPROFILE%` (Windows), else `None`
(a stripped/service environment — callers already degrade gracefully). Checking
`$HOME` first means macOS/Linux behaviour is provably unchanged; the precedence
is pinned by a pure `resolve_home` unit test. `app_dir()` now composes on
`home_dir()`, and the GUI browser's start-dir fallback reuses it (with a `.`
last resort instead of the old Unix-only `/`).

Trade-off recorded: a Windows shell that *does* set `$HOME` to an MSYS-style
Unix path (Git Bash) would win over `%USERPROFILE%`; a native GUI/`cmd`/
PowerShell launch — the real target — has only `%USERPROFILE%` and resolves
correctly. Acceptable; `dirs::home_dir()`'s known-folder API is the upgrade path
if that corner ever bites.

### Audio & MIDI backends

- cpal's `default_host()` already selects the right backend per OS: **WASAPI**
  on Windows, **ALSA** on Linux, CoreAudio on macOS. No code change.
- **JACK/PipeWire on Linux** (the white paper's low-latency target) is a future
  cpal `jack` feature flag — PipeWire is reachable through its JACK/ALSA compat.
  Not enabled yet; ALSA is the v1 Linux path.
- **ASIO on Windows is deferred.** It needs the Steinberg ASIO SDK (licensing +
  a non-vendorable header) behind cpal's `asio` feature; **WASAPI shared/
  exclusive is the v1 answer**. Exclusive-mode buffer-size control is exactly
  the "abstraction leak" the white paper §5.4 flagged — verify on hardware.
- MIDI (midir) is cross-platform (WinMM/WinRT on Windows); no change.

### CI

- Add `windows-latest` to the `check` matrix (`fail-fast: false` is already set,
  so macOS/Linux still report if Windows fails).
- Guard the `Format` step to non-Windows: `cargo fmt` is OS-independent, and
  checking it on the Windows runner would false-positive on CRLF-vs-LF.

### Landed in this ADR (sandbox-verified: fmt/clippy clean, 412 tests green)

`lh_assets::home_dir()` + `resolve_home` (+ test) + portable `app_dir()`; the
GUI browser start-dir; user-facing `$HOME` → "home directory" error strings
across session/render/level; `windows-latest` in CI with the guarded fmt step.
**No preset schema bump, no plugin param-id change, zero RT/DSP cost.**

### Deferred (runner/hardware-dependent — not doable in the Linux sandbox)

1. **Windows CI first-green** — the fix makes it *possible*; only the runner can
   surface any remaining Windows-specific compile/test issues.
2. **WASAPI device/buffer verification** on real hardware; RTL numbers per OS
   into `docs/latency.md` (the user's, like the Mac verification).
3. **Windows/Linux release artifacts** — `release.yml` builds macOS only. A
   `release-windows`/`release-linux` job (`.exe`/tarball + CLAP/VST3 bundles;
   optional Authenticode) is Phase-2 follow-up.
4. **JACK/PipeWire feature**, **ASIO**, and a `.gitattributes` LF policy if
   Windows contributors ever appear.

## Consequences

- **Linux is usable at the code level today**; Windows is unblocked where it was
  hard-blocked (paths). The macOS/Linux experience is provably unchanged —
  `$HOME` still wins, every existing path resolves identically, all tests green.
- The remaining Windows risk is concentrated in what only a real runner/interface
  can exercise (WASAPI buffer control, first CI compile). That is surfaced, not
  hidden: Windows is now in CI so the unknowns become visible failures.
- Release/signing for non-macOS is not yet built — v0.1 can still ship macOS-only
  and add Windows/Linux binaries once their CI is green and hardware-verified.
