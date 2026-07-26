# Packaging

How to produce an installable build, and what is deliberately not done yet.

## Building

```bash
npm install                    # Tauri CLI
npm --prefix frontend install
npm run build
```

Artifacts land in `target/release/bundle/`:

| platform | output |
|---|---|
| Windows | `nsis/Nexus_<version>_x64-setup.exe`, `msi/Nexus_<version>_x64_en-US.msi` |
| macOS | `dmg/Nexus_<version>_x64.dmg`, `macos/Nexus.app` |
| Linux | `deb/`, `appimage/`, `rpm/` |

Tauri only builds for the host platform. Cross-compiling needs the target
toolchain and, for macOS, a Mac — there is no way around the second one.

### Measured (Windows x64, v0.1.0)

| artifact | size |
|---|---|
| NSIS installer | 4.5 MB |
| MSI installer | 6.0 MB |
| unpacked binary | 13 MB |

An Electron app of comparable scope ships an 80–150 MB installer, because it
carries its own Chromium. Tauri uses the webview already on the machine. That
gap is the product argument, not a detail — it is why this can be a download
someone tries on a whim.

The release profile is tuned for it: `lto = true`, `codegen-units = 1`,
`panic = "abort"`, `strip = true`. Dropping the ADK dependency earlier removed
391 of 755 crates, which is also visible here.

## Not signed

Builds are unsigned. What that means for whoever installs it:

- **Windows** — SmartScreen shows "Windows protected your PC" and the app runs
  only after *More info → Run anyway*. Signing needs an OV or EV certificate
  from a CA, typically a few hundred dollars a year, and an EV cert needs
  hardware.
- **macOS** — Gatekeeper refuses to open it at all on first launch. The user
  has to right-click → Open, or clear the quarantine attribute. Signing needs
  a paid Apple Developer account, and distribution outside the App Store also
  needs notarisation.
- **Linux** — no equivalent barrier.

This is worth knowing before sharing a build: an unsigned installer asks a
stranger to click past a security warning, which is exactly the instinct you
do not want to train. For a handful of people who know you, fine. For anything
wider, sign it.

### When you do sign

Windows, once you hold a certificate:

```jsonc
// tauri.conf.json → bundle
"windows": {
  "certificateThumbprint": "<thumbprint>",
  "digestAlgorithm": "sha256",
  "timestampUrl": "http://timestamp.digicert.com"
}
```

A timestamp URL matters: without it, signatures stop validating when the
certificate expires rather than when it was signed.

macOS uses `APPLE_CERTIFICATE`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, and
`APPLE_TEAM_ID` in the environment, plus `"macOS": { "signingIdentity": … }`.

Never commit a certificate or its password. These belong in CI secrets or a
local environment, the same rule the app follows for API keys.

## No auto-update

The updater is not configured. It needs a signing keypair (`tauri signer
generate`), a hosted `latest.json` manifest, and somewhere to serve the
artifacts. Adding it before there are users to update would be building a
release pipeline for an audience of one.

## Content Security Policy

The webview runs under a restrictive CSP. Worth knowing why each entry exists,
because loosening one by reflex is how these become meaningless:

- `script-src 'self'` — no inline or remote scripts. Model output is rendered
  as Markdown with raw HTML disabled, so nothing a model emits can execute.
- `style-src 'self' 'unsafe-inline'` — Shiki emits inline `style` attributes
  for syntax colours. This is the one real concession.
- `connect-src` — IPC only. The webview makes no network requests of its own;
  every provider call happens in Rust.

The dev CSP additionally allows `unsafe-eval` and the Vite HMR websocket,
which is why the two are configured separately rather than one relaxed policy
covering both.

## Before sharing a build

- [ ] Launch from the installed location, not `target/`, so a missing runtime
      dependency actually shows up
- [ ] Check the app data directory is created under the real identifier
- [ ] Confirm a provider can be added and a key stored in the OS keychain
- [ ] Send one message end to end
- [ ] Time a cold start
