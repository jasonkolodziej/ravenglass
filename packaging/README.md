# Packaging and release

Everything needed to turn the workspace into installers and Relay artifacts. CI does it in
`.github/workflows/release.yml`; every step below also runs by hand.

| Artifact | Built by | Signing |
| --- | --- | --- |
| macOS `.app` + `.dmg`, universal (arm64 + x86_64 via `lipo`) | cargo-packager (`app`, `dmg`) | Developer ID + `notarytool` |
| Windows `.msi` (x64) | cargo-packager (`wix`, WiX 3) | Authenticode (`signtool`) |
| Linux `.deb` + AppImage (x86_64, aarch64) | cargo-packager (`deb`, `appimage`) | GPG signature on `SHA256SUMS` |
| Linux Flatpak | `flatpak/io.github.jasonkolodziej.Ravenglass.yml` (not built in CI) | Flathub signs |
| Relay static binary (x86_64 / aarch64 musl) | `cargo zigbuild` | GPG signature on `SHA256SUMS` |
| Relay OCI image (amd64 + arm64) | `relay/Dockerfile` | cosign |

## Layout

```
packaging/
  icons/            generated app icons (committed) and build.sh, which regenerates them
  linux/            .desktop template (mailto handler) and AppStream metainfo
  macos/            Info.plist additions (mailto) and the CI signing-identity helper
  windows/          WiX fragment registering Ravenglass as a mail client (Default apps)
  flatpak/          Flatpak manifest
  relay/            Dockerfile, its .dockerignore, docker-compose example, env example
  identity.toml     bundle id, publisher, copyright, licence, certificate subjects (one place)
  check-identity.sh fails when a copy of an identity.toml value disagrees with it
  set-version.sh    stamps one SemVer on the app and the Relay (release tags)
  check-size.sh     fails when an installer is over the size budget
```

The packager configuration lives in `app/Cargo.toml` under `[package.metadata.packager]`.

## Icons

`packaging/icons/build.sh` runs `brand/src/export_icons.py`, which drives the brand generator
(`brand/src/icon_d.py`, icon D · Braille). Needs `rsvg-convert` (apt `librsvg2-bin`) and Python
with Pillow, numpy, scipy and matplotlib. Output (committed, so packaging needs no Python):

- `png/ravenglass-{dark,light}-{16…1024}.png`. 48 px and up are the master SVG rendered at size.
  **16, 24 and 32 px are regenerated** with a coarse, pixel-aligned dot grid (8, 10 and 10 dot
  rows, 1 px gaps), as plan §4 asks; the master downscaled to these sizes turns to mush and fills
  the beak in. The plan says the master holds at 32 px; in practice it does not, so 32 px is
  regenerated too.
- `ravenglass.ico`: dark, 16–256 px, each size its own image (Windows).
- `hicolor/<N>x<N>/apps/ravenglass.png` (dark) and `ravenglass-light.png`, plus `scalable/` SVGs.
- `Ravenglass.iconset/`: dark, for `iconutil -c icns` (the macOS release job runs it; the
  resulting `.icns` is git-ignored).
- `svg/`: the SVGs the PNGs come from. `svg/ravenglass-dark.svg` is byte-identical to
  `assets/icon/ravenglass-icon-D_braille.svg`.

The macOS 26 Icon Composer `.icon` (dark default, light appearance, tinted layer) is a manual
step in Icon Composer from these SVGs; the `.icns` is the fallback for older macOS.

## Building installers locally

```sh
cargo install cargo-packager --version 0.11.8 --locked
cargo build --release -p ravenglass
cargo packager -p ravenglass --release --formats deb,appimage     # Linux
cargo packager -p ravenglass --release --formats app,dmg          # macOS (after the .icns step)
cargo packager -p ravenglass --release --formats wix              # Windows
```

Output goes to `target/packager/`. The AppImage step downloads `linuxdeploy` from GitHub.
Without FUSE, run an AppImage with `--appimage-extract-and-run`.

**mailto.** cargo-packager's `deep-link-protocols` is deliberately not used: its Windows
installers write `HKLM\Software\Classes\mailto` (taking every mailto link) and delete the key on
uninstall. Instead: `linux/ravenglass.desktop` has `MimeType=x-scheme-handler/mailto`,
`macos/Info.plist` declares `CFBundleURLTypes` for mailto, and `windows/mail-client.wxs`
registers a `Ravenglass.mailto` ProgID under `RegisteredApplications`/`Clients\Mail`, so Windows
offers Ravenglass in Default apps without taking over. The app does not act on a `mailto:` launch
argument yet (it is ignored); that is app work.

**Why MSI (WiX) and not NSIS or MSIX.** The plan asks for MSI or MSIX. cargo-packager builds MSI
and NSIS, not MSIX. MSI is the format IT departments deploy (Intune, Group Policy, `msiexec /qn`),
winget accepts it, upgrades are handled by the template's `UpgradeCode`, and extra registry
entries go in as a WiX fragment (used for mailto). NSIS would also cross-build from Linux, but
gives none of the managed-deployment story. MSIX (Store, clean uninstall) needs separate tooling
(`makeappx`, a signed package identity) and is a later decision.

## Relay

```sh
rustup target add x86_64-unknown-linux-musl
sudo apt-get install musl-tools
CC_x86_64_unknown_linux_musl=musl-gcc \
  cargo build --release -p ravenglass-relay --target x86_64-unknown-linux-musl
```

CI uses `cargo zigbuild` (zig as the linker) for both x86_64 and aarch64, as does the Dockerfile.

```sh
docker build -f packaging/relay/Dockerfile -t ravenglass-relay .     # context: the repo root
cp packaging/relay/relay.env.example packaging/relay/relay.env       # set RAVENGLASS_RELAY_TOKEN
docker compose -f packaging/relay/docker-compose.yml up -d
```

The image: `gcr.io/distroless/static-debian12:nonroot` (no shell), user 65532, `/data` volume
(empty today; for the Relay's state once it keeps any), port 8743. It bakes in only
`RAVENGLASS_RELAY_LISTEN=0.0.0.0:8743` (loopback is useless in a container); every other setting
is passed through from the environment. **No `HEALTHCHECK`**: the Relay has no health endpoint or
self-check flag, every route needs the token, and there is no curl in the image. Add one when
the binary grows `ravenglass-relay --health` or an unauthenticated `/healthz`.

## Flatpak

```sh
# flatpak-builder-tools: https://github.com/flatpak/flatpak-builder-tools (cargo/)
python3 flatpak-cargo-generator.py ../../Cargo.lock -o packaging/flatpak/cargo-sources.json
cd packaging/flatpak
flatpak-builder --user --install --force-clean build-dir io.github.jasonkolodziej.Ravenglass.yml
```

Not built in CI (optional per the plan). Flathub submission is a separate pull request to
`flathub/flathub` with this manifest and the generated `cargo-sources.json`.

## Versions

Tags `vX.Y.Z` (or `vX.Y.Z-pre`, published as a pre-release) drive the version:
`packaging/set-version.sh` writes it into `app/Cargo.toml` and
`crates/ravenglass-relay/Cargo.toml` for that build only (never committed). Manual runs keep
the version in the manifests. Build metadata (`+N`) must be numeric and at most 65535 for MSI.
Bump the `<release>` list in the metainfo for each release.

## Release workflow

`release.yml` runs on `v*` tags and on demand (`workflow_dispatch`). Jobs: `linux` (x86_64 on
`ubuntu-22.04`, aarch64 on `ubuntu-22.04-arm`), `macos` (universal on `macos-14`), `windows`
(`windows-2022`), `relay` (musl binaries), `relay-image`, and `publish` (collects everything,
writes `SHA256SUMS`, signs it, uploads). Workflow artifacts are always uploaded; a GitHub Release
is created and the image pushed to `ghcr.io/<owner>/ravenglass-relay` only on tags.

Each installer job checks its output against the size budget (`check-size.sh`, `SIZE_BUDGET_MB`,
40 MB, architecture §13) and fails over it. Release binaries are built with
`CARGO_PROFILE_RELEASE_STRIP=symbols` (the workspace's release profile does not strip; on Linux
that is 76 MB unstripped against 59 MB stripped). Measured on Linux x86_64: `.deb` 25.5 MB,
AppImage 25.9 MB. The universal macOS `.dmg` holds two architectures and is the one most at risk. The Linux job also renders a headless snapshot from
the packaged AppImage (software Vulkan) and uploads it with the artifacts.

### Secrets

Every signing step runs only when its secrets exist; without them the build still succeeds and
the job prints an "unsigned" notice.

| Secret | What it is | Used by |
| --- | --- | --- |
| `APPLE_CERTIFICATE` | Base64 of the **Developer ID Application** certificate exported as `.p12` (`base64 -i cert.p12`) | macOS: imported into a temporary keychain by cargo-packager |
| `APPLE_CERTIFICATE_PASSWORD` | Password of that `.p12` | macOS |
| `APPLE_SIGNING_IDENTITY` | The identity name, e.g. `Developer ID Application: Name (TEAMID)` | macOS: `codesign` of the `.app` and `.dmg` |
| `APPLE_API_KEY_ID` | App Store Connect API key ID (Users and Access → Integrations) | macOS: `notarytool` |
| `APPLE_API_ISSUER` | That key's issuer ID (UUID) | macOS: `notarytool` |
| `APPLE_API_KEY_P8` | Contents of the key's `AuthKey_<ID>.p8` file | macOS: `notarytool` |
| `WINDOWS_CERTIFICATE` | Base64 of the Authenticode code-signing certificate as `.pfx` | Windows: `signtool` on `ravenglass.exe` and the `.msi` |
| `WINDOWS_CERTIFICATE_PASSWORD` | Password of that `.pfx` | Windows |
| `GPG_PRIVATE_KEY` | ASCII-armoured private key that signs `SHA256SUMS` (→ `SHA256SUMS.asc`) | publish |
| `GPG_PASSPHRASE` | Its passphrase | publish |
| `COSIGN_PRIVATE_KEY` | A `cosign generate-key-pair` private key (contents) | relay-image (tags) |
| `COSIGN_PASSWORD` | Its password | relay-image |

`GITHUB_TOKEN` (automatic) pushes the image to GHCR and creates the release; the jobs that need
it ask for `packages: write` / `contents: write`, everything else is read-only.

Repository variables (not secrets):

| Variable | Effect |
| --- | --- |
| `RELEASE_LINUX_ARM64` | `false` drops the Linux aarch64 build (when `ubuntu-22.04-arm` runners are unavailable, e.g. some private repositories) |
| `SIZE_BUDGET_MB` | Overrides the 40 MB installer budget (e.g. while the universal macOS `.dmg`, which carries two architectures, is brought under it) |
| `COSIGN_KEYLESS` | `true` signs the image keyless with the workflow's OIDC identity when `COSIGN_PRIVATE_KEY` is absent (recorded in the public Sigstore log, which names the repository) |

Notes: hardware-backed code-signing certificates (required for new Authenticode OV/EV certs
since 2023) cannot be exported as `.pfx`; with one, replace the two `signtool` steps with the
provider's signer (Azure Trusted Signing, DigiCert KeyLocker, SSL.com eSigner). The `.exe`
carries no embedded icon or version resource yet (that needs a `build.rs` in the app crate); the
MSI, Start-menu shortcut and Add/Remove Programs entry use `ravenglass.ico`.

## Decisions left to a human

- **Identity** (bundle identifier, product name, publisher, copyright, licence, certificate
  subjects) lives in one file, [`identity.toml`](identity.toml). Its values are placeholders:
  `io.github.jasonkolodziej.Ravenglass` (a reverse-DNS id Flathub accepts for a GitHub-hosted
  project) and "Ravenglass contributors". Changing the identifier after the first public release
  breaks macOS preferences and keychain access groups, the Flatpak app id and the single-instance
  socket, so decide before then. The publisher must match the signing certificates' subject for
  clean Gatekeeper and SmartScreen prompts. To change a value, edit `identity.toml`, run
  `packaging/check-identity.sh --list` to see every copy (`app/Cargo.toml`, `app/src/links_in.rs`,
  `Info.plist`, the metainfo and Flatpak files, which are named after the id), and update them;
  `check-identity.sh` runs in the Rust workflow and fails while any copy disagrees.
- **Licence**: MIT (`LICENSE`), shipped in every installer.
- **MSI vs MSIX**, winget manifest, Microsoft Store, Mac App Store (needs sandboxing), Flathub
  submission, Sparkle-style auto-update.
