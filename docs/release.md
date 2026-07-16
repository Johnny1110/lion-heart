# Cutting a release

## What ships

- `lion-heart` — the standalone app (macOS binary)
- `Lion-Heart.clap` / `Lion-Heart.vst3` — plugin bundles built with
  `cargo xtask bundle lion-heart-plugin --release`
  (VST3 bundles are **GPLv3** — vst3-sys licensing; the CLAP bundle and the
  standalone app stay MIT OR Apache-2.0)

## How

1. Verify on real hardware (play through it; check the footer for xruns).
2. Update the version in the workspace `Cargo.toml`, commit.
3. Tag and push:

   ```sh
   git tag v0.1.0 && git push origin v0.1.0
   ```

4. The `release` workflow builds everything on a macOS runner and opens a
   **draft** GitHub release with the artifacts attached — review and publish.

## Codesigning & notarization (optional but recommended)

Unsigned artifacts work after `xattr -dr com.apple.quarantine <file>`, but
Gatekeeper warns. To sign + notarize automatically, add these repository
secrets (Settings → Secrets → Actions); the workflow picks them up when
present:

| Secret | Contents |
| --- | --- |
| `MACOS_CERTIFICATE` | base64 of a **Developer ID Application** `.p12` export |
| `MACOS_CERTIFICATE_PASSWORD` | the `.p12` password |
| `APPLE_ID` | the Apple ID owning the certificate |
| `APPLE_TEAM_ID` | its team id (10 chars) |
| `APPLE_APP_PASSWORD` | an app-specific password (appleid.apple.com) for `notarytool` |

This needs a paid Apple Developer membership. The same script runs locally:
export the five variables and run `./scripts/codesign-notarize.sh` after
building.

## Plugin sanity check

`clap-validator validate target/bundled/Lion-Heart.clap` — the CLAP
conformance suite (16 applicable tests) must pass. Install it with
`cargo install --git https://github.com/free-audio/clap-validator`.

## Plugin v1 limitations (documented on purpose)

- No custom editor yet: parameters appear in the host's generic UI.
- The **Preset (assets)** parameter loads the NAM capture + cab IR from a
  `~/.lion-heart/presets/` preset (sorted order, 1-based; 0 = none). Knob
  values stay host-owned — dial tones in the standalone app, save, select
  here, automate in the host.
- Chain order is fixed at the default; reordering is a standalone feature
  until the plugin grows an editor.
- NAM captures are rate-locked (usually 48 kHz): run the host at the
  capture's rate, or the amp slot logs a refusal and stays passthrough.
