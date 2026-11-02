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

## Prerequisites (macOS, Apple Silicon)

Anime (Topaz) and Anime AV1 only; the vs-mlrt profile is Windows-only. See [MACOS.md](MACOS.md).

- `brew install rustup node pnpm ffmpeg svt-av1 mkvtoolnix` (plus `handbrake` for 4K sources).
- VapourSynth R79 in a venv: `python3.12 -m venv ~/upscale-venv`, activate it, then
  `pip install vapoursynth==79 "vapoursynth-bestsource<22" vapoursynth-vivtc vapoursynth-descale vapoursynth-mvtools vapoursynth-akarin`.
- Topaz Video AI 7.x in `/Applications`, signed in once.
- On macOS the app and CLI always use these tools from PATH (as with `--system-tools`); the private
  tools folder's downloads are Windows builds.

## Commands

After cloning, set the commit identity and enable the push guard (once per clone; see
`CLAUDE.md` → "Commit identity"):

```sh
git config user.email "11512795+rgbcoder001@users.noreply.github.com"
git config core.hooksPath .githooks
```

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
upscale-cli plan  <file-or-folder> [--profile cg]     # same report as upscale.ps1 -Plan (profiles: anime-topaz (default), anime, anime-av1, cg, live)
upscale-cli run   <file-or-folder> [--test-clip]      # JSON-line events on stdout, log on stderr
```

All three take `--set Key=Value` for any script setting, and `plan`/`run` take `--out DIR`.
`run` also takes `--temp DIR` for chunks and temporary audio (default `<out>\.work`).
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
cd app && pnpm bundle        # Windows
cd app && pnpm bundle:mac    # macOS: target/release/bundle/dmg/Upscale_<version>_aarch64.dmg (unsigned)
```

This builds the release CLI and the app, and writes `target\release\bundle\nsis\Upscale_<version>_x64-setup.exe`.
It's a per-user install (no admin) with `upscale-cli.exe` and the scripts next to the app.

**Signing** (not set up): the installer is unsigned, so Windows SmartScreen warns on first run.
To sign, set `bundle.windows.certificateThumbprint` (a certificate in the Windows store) or
`bundle.windows.signCommand` (e.g. Azure Trusted Signing) in `app/src-tauri/tauri.conf.json`.

**Updates** (from v0.1.1): the app checks the latest GitHub release at start-up
(`plugins.updater` in `tauri.conf.json`) and offers "Install and restart". Settings → App updates
checks on demand. Updates are signed with the updater key (`~/.tauri/upscale.key`, kept off the
repo; GitHub secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`), and the
app installs only files signed with it. The queue and settings are kept (they live in
`%APPDATA%\com.upscale.app`). Local `pnpm bundle` builds stay unsigned; only tag builds make update
files.

## Releasing

Releases happen only when you push a version tag; ordinary commits never build installers.

1. Set the new version in `Cargo.toml` (workspace), `app/src-tauri/tauri.conf.json` and
   `app/package.json`.
2. Write `.github/release-notes/v<version>.md` (shown on GitHub and in the app's update banner).
3. Commit, push `main`, then tag and push the tag:

   ```sh
   git tag -a v0.1.2 -m "Upscale v0.1.2" && git push origin v0.1.2
   ```

CI builds both installers, signs the update files, and publishes the release with `latest.json`.
Running apps see it on their next start.
