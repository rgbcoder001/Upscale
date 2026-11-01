# Upscale

Desktop app that batch-upscales anime and CG animation to 4K and encodes to AV1. It replaces
`reference/upscale.ps1`. Read [PLAN.md](PLAN.md) for the roadmap and [CLAUDE.md](CLAUDE.md) for the
conventions and domain rules.

## Layout

- `core/`: Rust engine (`upscale_core` lib + `upscale-cli` binary). No Tauri dependency.
- `app/`: Tauri 2 shell (`app/src-tauri`) and React + TypeScript UI (`app/src`).
- `resources/scripts/`: `upscale.vpy` and `analyze.py`, unchanged copies from `reference/`, bundled with the app.
- `reference/`: the PowerShell version. Read-only.

## Prerequisites (Windows)

- Rust stable via rustup, with the MSVC toolchain and the Visual Studio Build Tools ("Desktop development with C++").
- Node 22 and pnpm 9 (`corepack enable`, or `corepack pnpm@9.15.9 …`).
- WebView2 (preinstalled on Windows 11).

## Commands

```sh
cd app && pnpm install
pnpm tauri dev                 # run the app
pnpm lint && pnpm test         # frontend checks
cargo fmt --all --check        # from the repo root
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p upscale-core --bin upscale-cli -- --help
```

## CLI

```sh
upscale-cli tools [--update] [--tools-dir DIR]        # status, self-tests; --update installs/upgrades
upscale-cli plan  <file-or-folder> [--profile cg]     # same report as upscale.ps1 -Plan (profiles: anime-topaz (default), anime, cg)
upscale-cli run   <file-or-folder> [--test-clip]      # JSON-line events on stdout, log on stderr
```

All three take `--set Key=Value` for any script setting, and `plan`/`run` take `--out DIR`.
While `run` works, type `pause`, `resume`, `stop` (after the current file), `continue` or
`cancel` on its stdin; Ctrl-C cancels. Machine tests (GPU, Topaz) run with
`cargo test -p upscale-core --test machine -- --ignored`.

## Tools

The app keeps every tool in a private folder (Settings → Tools folder, default
`%APPDATA%\com.upscale.app\tools`): Python 3.12 (python-build-standalone) with VapourSynth R79
and its plugins, vs-mlrt, BtbN FFmpeg, MKVToolNix portable, HandBrakeCLI and 7-Zip's `7zr.exe`.
They are downloaded on first use (about 4–5 GB, mostly vs-mlrt) and every file is checked
against its published SHA-256: GitHub asset digests, MKVToolNix's `.sha256` files, and pip
`--require-hashes` for the wheels. Nothing is installed system-wide. `--system-tools` (CLI) or
Settings → "Use system tools" switches back to PATH, winget and your own Python.

## Installer

```sh
cd app && pnpm bundle
```

This builds the release CLI and the app, and writes `target\release\bundle\nsis\Upscale_<version>_x64-setup.exe`.
It's a per-user install (no admin) with `upscale-cli.exe` and the scripts next to the app.

**Signing** (not set up): the installer is unsigned, so Windows SmartScreen warns on first run.
To sign, set `bundle.windows.certificateThumbprint` (a certificate in the Windows store) or
`bundle.windows.signCommand` (e.g. Azure Trusted Signing) in `app/src-tauri/tauri.conf.json`.

**Updates** (not set up): installing a newer setup over the old one keeps the queue and
settings (they live in `%APPDATA%\com.upscale.app`). For automatic updates, add
`tauri-plugin-updater`, generate a key pair with `pnpm tauri signer generate`, and publish a
`latest.json` next to each release where the app can reach it.
