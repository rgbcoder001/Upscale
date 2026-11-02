# Running the Anime (Topaz) and Anime AV1 routes on an Apple Silicon Mac

Plan for letting the Mac mini share encoding work with the Windows PC. It covers two routes:
**Anime (Topaz)** (`anime-topaz`: vspipe `prep` → Topaz `tvai_up` → SVT-AV1) and **Anime AV1**
(`anime-av1`: vspipe `prep` → SVT-AV1). The vs-mlrt route (`anime`), HandBrake/live and the
installer polish come later or not at all.

Checked on 2026-11-02 against PyPI, GitHub releases, mkvtoolnix.download, Homebrew and Topaz's docs.
Decided with the owner on 2026-11-02: **Homebrew** for the Mac's tools (the app's own tools folder
stays Windows-only), and **claim files** so both machines can work through the same folders.

**Status:** everything that could be done without a Mac is done (next section). What's left
needs the Mac or GitHub: see "First day with the Mac" below.

## Summary

- **Most of the code is already portable.** Every Win32 call is behind `#[cfg(windows)]`, paths go
  through `std::path`, `exe()` uses `EXE_SUFFIX`, `app_data_dir()` already has a macOS branch, and
  the pump, planning, tracks, mux and chunking don't depend on the OS.
- **Every tool these two routes need exists for macOS arm64.** That includes the exact
  VapourSynth R79 wheel and every plugin wheel the app pins. Only vs-mlrt is missing, and the
  Topaz route doesn't need it.
- **Blocker #1 is a one-line logic bug, not a port.** Every batch calls `tools::set::ensure` with
  `mlrt: true` (`queue.rs:877`, `upscale-cli.rs:427`), so even a Topaz-only batch tries to
  download vs-mlrt. vs-mlrt has no macOS package, so this fails on a Mac before any file starts.
- **Blocker #2 is the private tools folder.** Each download in `tools/private.rs` and `tools/svt.rs`
  is hard-wired to a Windows asset name. `--system-tools` with Homebrew avoids most of that for the
  CLI-first step.
- **Splitting the library:** give each machine its own series folders, put temp/work folders on
  each machine's local disk, and use one shared output folder. Two machines can't encode the same
  episode at once today only because nobody queues it twice. Nothing in the code prevents it
  (see §4).

## Done before the Mac arrived (2026-11-02, branch `macos-prep`)

Tested on Windows (`cargo test`, clippy). The core crate also passes clippy for the
`aarch64-apple-darwin` target (`rustup target add aarch64-apple-darwin`, then
`cargo clippy -p upscale-core --all-targets --target aarch64-apple-darwin`). Nothing has run on a
Mac yet, and the Tauri app can't be compiled for macOS on Windows (it needs Apple's SDK); the
`rust-macos` CI job covers both once the repo is on GitHub.
- **Gap 1:** vs-mlrt is installed and self-tested only when a job uses the vs-mlrt route
  (`Settings::uses_vs_mlrt`). Update tools installs it only where it exists (`tools::mlrt::AVAILABLE`,
  Windows). The CLI no longer asks for vs-mlrt on Anime AV1 either.
- **Gaps 2–4:** `tools::topaz::default_dir()`, the default models folder and `app_version()` depend
  on the OS. On macOS that's `Contents/MacOS` and `Contents/Resources/models`, with the version
  read from `Info.plist` through `plutil`. The app now takes the Topaz folder from the queued
  job's settings, so `TopazPath` works there too.
- **Gap 5:** no more `ffmpeg.exe` literals (`tools::exe()` is public; `topaz::is_installed()`).
- **Gap 6 and part of 13:** missing-tool hints name Homebrew outside Windows. `Installer::winget`
  outside Windows reports "install it with your package manager" instead of failing on winget.
  When updating, it skips.
- **§4 item 3:** the app's default temp folder is local (`%LOCALAPPDATA%\com.upscale.app\work`,
  `~/Library/Application Support/com.upscale.app/work` on macOS). A job with chunks in the old
  `<output>\.work` still resumes from there. `upscale-cli run --temp DIR` was added. The disk
  check also covers the temp drive.
- **Gaps 7 and 8:** on Unix every pipeline child leads its own process group and runs at nice 10
  (`platform::command`, `process::command`). `ProcessGroup::kill` sends `killpg(SIGKILL)` to each
  group, so Cancel ends vspipe, Topaz and the encoder, and anything they started. Two Unix tests
  (grandchildren die, children run at nice 10) only run on a Mac.
- **Gap 9:** `gpu::list_gpus` reads `system_profiler SPDisplaysDataType -json` on macOS (parser
  tested with a sample).
- **Gap 10:** `find_scripts_dir` also looks in `Contents/Resources/scripts`.
- **Gap 12:** system Python on macOS is `python3.12`, `python3`, then `python`. The pip scripts
  folder uses the platform's user scheme (`sysconfig.get_preferred_scheme('user')`).
- **Gap 16:** `app/src-tauri/tauri.macos.conf.json` (bundle `app` + `dmg`),
  `tauri.release.macos.conf.json` (bundles `upscale-cli` and the scripts), `pnpm bundle:mac`, and
  the `rust-macos` / `build-macos` CI jobs (clippy, tests, an unsigned `.dmg` artifact).
- **Gap 17:** `platform::KeepAwake` while a batch runs, in the app and the CLI: a
  `SetThreadExecutionState(ES_SYSTEM_REQUIRED)` thread on Windows, `caffeinate -i -w <pid>` on
  macOS. The display can still turn off.
- **Gap 18:** claim files (`claim.rs`, §4).
- **Homebrew by default:** outside Windows the app and CLI always use tools from PATH
  (`AppSettings::system_tools()`, the CLI's `--system-tools` implied), so nothing tries the
  Windows-only private downloads.
- **Gap 19:** the Anime (vs-mlrt) profile on a Mac stops with "vs-mlrt only exists for Windows; use
  Anime (Topaz) or Anime AV1".

Still open: gap 11 (optional), gap 15 (not wanted: Homebrew), and checking 2–4, 7–10, 12, 16 and 17
on the real Mac.

## First day with the Mac

1. `brew install rustup node pnpm ffmpeg svt-av1 mkvtoolnix` (and `handbrake` for 4K sources),
   then `rustup default stable`.
2. A Python venv for VapourSynth:
   ```sh
   python3.12 -m venv ~/upscale-venv && source ~/upscale-venv/bin/activate
   pip install vapoursynth==79 "vapoursynth-bestsource<22" vapoursynth-vivtc vapoursynth-descale vapoursynth-mvtools vapoursynth-akarin
   ```
3. Clone the repo, then `cargo test --workspace` (this also runs the two Unix-only process tests).
4. Install Topaz Video AI 7.x, open it once and sign in, then run
   `cargo run --release -p upscale-core --bin upscale-cli -- tools`. Expect the
   VapourSynth and Topaz self-tests to pass (vs-mlrt shows as not available).
5. Measure the speed (Step 0 below), then do one `--test-clip` episode with each route and compare
   it with the PC's output.
6. Mount the share and point both machines at the same output folder. Leave the temp folder
   local. Queue the same folder on both and check that they take different episodes.
7. `cd app && pnpm install && pnpm bundle:mac` for the app (or download the `.dmg` from CI).

## 1. Gap list, smallest effort first

Effort: **XS** under an hour, **S** half a day, **M** one to two days, **L** more than that.

| # | Effort | Where | What's Windows-only | What macOS needs |
|---|---|---|---|---|
| 1 | XS | `queue.rs:877`, `bin/upscale-cli.rs:427` (`run`) | `Needs { mlrt: true }` for every batch, so vs-mlrt is installed even when no job uses it. `queue.rs` already computes `vs` (any job on the vs-mlrt route) one line above. | Pass `mlrt: vs`. The CLI needs the same check (`settings.upscales() && !settings.use_topaz()`). `tools --update` and `update_tools` (`queue.rs:770`, `upscale-cli.rs:265`) should skip vs-mlrt on macOS and only warn. Also fixes Windows Topaz-only batches downloading 4 GB they never use. |
| 2 | XS | `settings.rs:202` `topaz_dir()` | Defaults to `%ProgramFiles%\Topaz Labs LLC\Topaz Video AI`. | `#[cfg(target_os = "macos")]`: `/Applications/Topaz Video AI.app/Contents/MacOS` (Topaz's docs put the CLI ffmpeg there). `"TopazPath"` in `upscale-settings.json` already works as an override today. |
| 3 | XS | `tools/topaz.rs:43` model dirs | `TVAI_MODEL_DIR` / `TVAI_MODEL_DATA_DIR` default to `%ProgramData%\Topaz Labs LLC\Topaz Video AI\models`. `auth.tpz` is looked for there. | On macOS both default to `/Applications/Topaz Video AI.app/Contents/Resources/models` (Topaz CLI docs). Check this on the Mac: `auth.tpz` must be there after signing in, and the data dir may need to move to a disk with ~80 GB free. |
| 4 | XS | `tools/topaz.rs:63,83` `product_version` | Reads `Topaz Video AI.exe` ProductVersion through PowerShell. It already returns `None` off Windows, so the log just says "unknown version". | Read `CFBundleShortVersionString` from `…/Contents/Info.plist` (`plutil -extract CFBundleShortVersionString raw <plist>` or `defaults read`). Cosmetic, but it also confirms the install is 7.x. |
| 5 | XS | `queue.rs:792`, `bin/upscale-cli.rs:287,345,587` | `d.join("ffmpeg.exe")` hard-coded (Topaz probe, VMAF tool). | `exe("ffmpeg")`. |
| 6 | XS | `tools/topaz.rs:38` and other messages | Error texts name `ffmpeg.exe`, winget, "ships with Windows 10". | Neutral wording, or a per-OS hint (`brew install …`). |
| 7 | S | `process.rs:64`, `platform.rs:14` priority | `BELOW_NORMAL_PRIORITY_CLASS` + `CREATE_NO_WINDOW`. Off Windows, children run at normal priority. | Unix: `pre_exec` with `libc::setpriority(PRIO_PROCESS, 0, 10)` (nice 10), or `nice -n 10` as a wrapper. On macOS, also consider `taskpolicy -b` / `setiopolicy_np` so the Mac stays usable. No console-window flag is needed. |
| 8 | S | `platform.rs` `ProcessGroup` | Windows Job Object with kill-on-close. Off Windows `add()` and `kill()` do nothing, so **Cancel doesn't kill vspipe/Topaz/ffmpeg on macOS** (only `kill_on_drop` on each `Child`), and a crashed app leaves them running. | Spawn each job's children with `Command::process_group(pgid)` (tokio supports it on unix): the first child gets `0`, the rest join its pgid. `kill()` sends `killpg(pgid, SIGKILL)`. For crash safety, rely on the pipes: when the pump dies, vspipe/Topaz get EPIPE and the encoder gets EOF, and the frame check rejects the short chunk. Optionally keep a pid file per job and clean it up at start. `suspend`/`resume` already use `kill -STOP/-CONT` and work on macOS (HandBrake route only). |
| 9 | S | `gpu.rs:96` `list_gpus` | PowerShell `Win32_VideoController`. Off Windows it returns nothing, so the log says `GPU: ` with no name. | `system_profiler SPDisplaysDataType -json` → `sppci_model` ("Apple M…"). This only affects the log/Settings line on these routes. `pick_gpu` already falls back to the first name. Topaz's `device=0` is its own GPU index; confirm with the self-test that `device=0` works on Apple GPUs. If not, try `device=-2` (auto) as a per-machine `TopazFilter`. |
| 10 | S | `tools/mod.rs:189` `find_scripts_dir` | Looks for `scripts/` next to the exe, or `resources/scripts` above it. In a macOS `.app` the exe is in `Contents/MacOS` and Tauri resources are in `Contents/Resources/scripts`. It only works today because APFS is case-insensitive (`Contents/resources/scripts` matches). | Add `exe_dir/../Resources/scripts` explicitly. Not needed for the CLI from a checkout: `UPSCALE_SCRIPTS_DIR` or the dev-build search finds `resources/scripts`. |
| 11 | S | `tools/vapoursynth.rs` `REQUIRED_PLUGINS` | Not Windows-only, but the batch self-test requires `descale`, `mv` and `akarin`, which `prep` never uses (`prep` needs `bs` (BestSource source filter), `vivtc` (VFM/VDecimate IVTC) and the core `std.Crop` / `resize`). | Nothing to change for macOS: all six wheels exist for `macosx_*_arm64` (table in §2). Optional: test only `bs, vivtc` on Topaz/AV1-only batches. |
| 12 | S | `tools/vapoursynth.rs:42`, `tools/mod.rs:144` (system mode) | `find_python` tries `py -3`, then `python`, and skips the Store stub. `find_vspipe` adds `%ProgramFiles%\VapourSynth`. `python_script_dirs` asks for the `nt_user` scheme. | On macOS `python` usually isn't on PATH (Homebrew gives `python3` / `python3.12`). Try `python3.12`, `python3`, then `python`. Ask for `posix_user`/`osx_framework_user` instead of `nt_user`. A venv on PATH already works today (`python` → venv, `vspipe` in `venv/bin`). |
| 13 | S | `tools/install.rs` winget, system mode | `winget_tool` only calls winget when the tool is **missing**. With Homebrew tools on PATH the lookups succeed and winget never runs. When something is missing, the error says to install App Installer. | macOS: replace the winget call with a clear message (`brew install ffmpeg mkvtoolnix svt-av1 handbrake`). Running `brew` from the app isn't worth it. |
| 14 | S | `tools/set.rs:77` `find_svt_app` | Private mode only looks in `<tools>/svt-av1`. System mode adds PATH. | Nothing in system mode: Homebrew's `svt-av1` puts `SvtAv1EncApp` (4.3.0 today) on PATH, and `choose_encoder` prefers it on a tie. In private mode, symlinking `/opt/homebrew/bin/SvtAv1EncApp` into `<tools>/svt-av1/` works without a code change. |
| 15 | M | `tools/private.rs`, `tools/svt.rs` private downloads | Every asset pattern is Windows-only: `cpython-3.12.*-x86_64-pc-windows-msvc-install_only`, `ffmpeg-n*-latest-win64-gpl-*.zip`, `mkvtoolnix-64-bit-*.zip` from `/windows/releases/`, `HandBrakeCLI-*-win-x86_64.zip`, `7zr.exe`, and wheels filtered on `win_amd64`. The layout is Windows too: `python/python.exe`, `python/Lib/site-packages/vapoursynth/vspipe.exe`. | Per-OS asset table: see §2. Layout on macOS: `python/bin/python3.12` and `python/lib/python3.12/site-packages/vapoursynth/vspipe` (checked in the R79 arm64 wheel). Wheels: accept `macosx_*_arm64` in `pick_wheel`. DMGs (MKVToolNix, HandBrakeCLI) need `hdiutil attach -nobrowse -readonly` + copy + `detach`, a new extractor next to `extract_zip`. FFmpeg has **no BtbN macOS build**, so pick another source (Homebrew, or a signed static build such as Martin Riedl's) or keep FFmpeg in system mode on macOS. Skip 7zr/vs-mlrt on macOS. |
| 16 | M | `app/src-tauri/tauri.conf.json`, `tauri.release.conf.json`, `.github/workflows/ci.yml` | `bundle.targets: ["nsis"]`. The release config bundles `target/release/upscale-cli.exe`. CI builds on `windows-latest` only. `main.rs` `windows_subsystem` is already `cfg_attr`-gated. | Add `app/src-tauri/tauri.macos.conf.json` (Tauri 2 merges it automatically on macOS) with `bundle.targets: ["app", "dmg"]`, `bundle.macOS.minimumSystemVersion`, and a resources map that takes `upscale-cli` without `.exe`. Better: `bundle.externalBin` with the target-triple name (`upscale-cli-aarch64-apple-darwin`). Add a `macos-latest` (arm64) CI job for `cargo test` + `pnpm bundle`. Signing: an unsigned/ad-hoc-signed app for the owner's own Mac is fine (right-click → Open once). Distributing needs a Developer ID + notarization (`APPLE_SIGNING_IDENTITY`, `APPLE_ID`/`APPLE_PASSWORD`/`APPLE_TEAM_ID` in CI). |
| 17 | M | new: sleep | Nothing keeps either machine awake. Windows has managed by power settings so far. A Mac mini in default settings may sleep mid-batch and stall the pipes. | Start a `caffeinate -i -w <app pid>` child while a batch runs (or `IOPMAssertionCreateWithName` through `core-foundation`). For the CLI-first step: run `caffeinate -i upscale-cli run …`. |
| 18 | M | new: shared-output claims (§4) | Not Windows-specific: nothing stops two machines from starting the same episode. `run_file` only skips when the final `<name>.mkv` already exists (`pipeline.rs:1139`), and the default work folder `<output>/.work` would be **shared** between machines. | See §4. |
| 19 | L (skip) | `tools/mlrt.rs`, `upscale.vpy` | vs-mlrt: Windows-only packages (`vsmlrt-windows-x64-*`), `.dll` plugin names hard-coded in `upscale.vpy` (`MLRT_PLUGINS`), 7z extraction. | **Not needed for these routes.** `upscale.vpy` `prep` mode skips vs-mlrt (`need_mlrt = mode != "prep"`). If wanted later: vs-mlrt publishes no macOS builds. It would mean building vs-ncnn (Vulkan via MoltenVK) or using a CoreML backend, plus `.dylib` names in `upscale.vpy` (which is meant to ship unchanged). Leave `"Content": "anime"` as Windows-only, and have the Mac reject it with a clear message. |
| — | none | everything else | Checked, nothing to do: `app_data_dir()` (already `~/Library/Application Support/com.upscale.app`), `fs4::available_space` (works on SMB mounts, else `None`), y4m pump, `tar -xf` (bsdtar ships with macOS), `curl` (ships with macOS), suspend/resume (`kill`), `AppSettings` default output (`$HOME/Videos/Upscaled`), cancel-cleanup retries (harmless on macOS), CRLF in `ab-report.txt` and the plan text (cosmetic). The Windows-path strings in tests and insta snapshots (`C:\Rips\…`) are only argument strings, and `Path::new` keeps them unchanged on Unix, so the snapshots should still match. Confirm in the first macOS `cargo test`. |

## 2. Tool availability on macOS arm64

| Tool | Windows source today | macOS arm64 |
|---|---|---|
| Python 3.12 | python-build-standalone `x86_64-pc-windows-msvc-install_only` | Same release: `cpython-3.12.16+20261026-aarch64-apple-darwin-install_only.tar.gz` ✔ (or Homebrew `python@3.12`) |
| VapourSynth **R79** | PyPI `vapoursynth==79` win_amd64 | `vapoursynth-79-cp312-abi3-macosx_13_0_arm64.whl` ✔ (`vspipe` is inside the package; no separate install). Homebrew's `vapoursynth` is R81, newer than the pin. Use the wheel to stay on the same release as the PC. |
| `bs` (BestSource, source filter) | `vapoursynth-bestsource<22` | 21.0 `macosx_15_0_arm64` ✔ |
| `vivtc` (IVTC) | `vapoursynth-vivtc` | 2.0 `macosx_11_0_arm64` ✔ |
| `descale`, `mv`, `akarin` (self-test only) | pip | `descale` 12, `mvtools` 29, `akarin` 1.5.0: all have `macosx_*_arm64` ✔ |
| Crop | core `std.Crop` | built in ✔ (cropdetect itself is ffmpeg) |
| FFmpeg/ffprobe with libsvtav1 | BtbN `win64-gpl` | **No BtbN macOS build** (only linux64/linuxarm64/win). Homebrew `ffmpeg` 9.0.2 (depends on `svt-av1`) ✔ |
| SVT-AV1 | BtbN's libsvtav1 | Homebrew `svt-av1` 4.3.0 gives `SvtAv1EncApp` ✔ (the app prefers it) |
| mkvmerge | MKVToolNix portable zip | `MKVToolNix-103.0-1-arm64.dmg` (+ `.sha256`) on mkvtoolnix.download/macos/releases ✔, or Homebrew `mkvtoolnix` 103.0 ✔ |
| HandBrakeCLI | `HandBrakeCLI-*-win-x86_64.zip` | `HandBrakeCLI-1.11.2.dmg` on GitHub ✔, or Homebrew `handbrake` ✔. Only for ≥ 2160p sources. |
| 7-Zip | `7zr.exe` | `7z2605-mac.tar.xz` exists, but only vs-mlrt needs it, so it's not needed |
| vs-mlrt | `vsmlrt-windows-x64-cuda/vk` | ✘ No macOS packages in v15.16. Not needed for these routes. |
| Topaz Video AI 7.x | `C:\Program Files\Topaz Labs LLC\Topaz Video AI\ffmpeg.exe` | `/Applications/Topaz Video AI.app/Contents/MacOS/ffmpeg`, models in `…/Contents/Resources/models`. Needs a **7.x macOS installer** from the owner's Topaz account: Topaz Video 1.1+ removed the CLI ffmpeg on every platform. |

## 3. Smallest-first path

Each step ends with something usable, and Windows behaviour doesn't change.

### Step 0: measure first (no code, about an hour)
On the Mac, before any port, check whether it's worth it:
```sh
brew install ffmpeg svt-av1 mkvtoolnix
caffeinate -i ffmpeg -i <a 1080p episode> -t 120 -vf scale=3840:2160 -pix_fmt yuv420p10le -f yuv4mpegpipe -strict -1 - \
  | SvtAv1EncApp -i stdin --crf 16 --preset 5 --input-depth 10 -b /tmp/t.ivf
```
That shows SVT-AV1's 4K preset 5 speed on the Mac's CPU. Then open Topaz Video AI 7.x, sign in, and
run the same 2 minutes through the Proteus prob-4 preset in the GUI to see Topaz's fps on the Apple
GPU. The slower of the two numbers is the Mac's rate for Anime (Topaz). For Anime AV1 (1080p, no
upscale) only SVT matters, and it will be several times faster. **If the Mac is much slower at
4K, give it the Anime AV1 series instead.** That route is CPU-only, needs no Topaz seat, and is
where the Mac adds the most.

### Step 1: CLI with Homebrew tools, Anime AV1 only (gaps 1, 5, 12, half a day)
- Fix gap 1 (`mlrt: vs`). On its own, this is the change that makes the CLI start on macOS.
- Fix gap 5, and gap 12's `python3` lookup (or document "activate the venv").
- Owner setup on the Mac:
  ```sh
  brew install rustup ffmpeg svt-av1 mkvtoolnix
  python3.12 -m venv ~/upscale-venv && source ~/upscale-venv/bin/activate
  pip install vapoursynth==79 "vapoursynth-bestsource<22" vapoursynth-vivtc vapoursynth-descale vapoursynth-mvtools vapoursynth-akarin
  cargo build --release -p upscale-core --bin upscale-cli
  export UPSCALE_SCRIPTS_DIR=$PWD/resources/scripts
  caffeinate -i target/release/upscale-cli run "/Volumes/<share>/<Series>" --profile anime-av1 --system-tools --out "/Volumes/<share>/Upscaled"
  ```
  (`vapoursynth config` isn't needed for wheels on macOS; check with `vspipe --version`.)
- Parity check: one episode on both machines with `--test-clip`. Compare frame count, tracks and
  SVT params in the log. Sizes won't match byte for byte (SVT 4.3.0 via SvtAv1EncApp on the Mac
  versus BtbN's libsvtav1 on the PC, and ARM versus x86 code paths). Use `upscale-cli ab`-style
  VMAF/SSIM if that matters.
- `cargo test --workspace` on the Mac, to catch snapshot or path assumptions early.

### Step 2: CLI, Anime (Topaz) (gaps 2, 3, 4, 9, a day)
- Topaz defaults for macOS (gaps 2 and 3), the version from `Info.plist` (4), the GPU name (9).
- Run `upscale-cli tools` and expect the Topaz self-test to pass. It already checks that stdout
  starts with `YUV4MPEG2`, which matters even more here, because a forum report describes Proteus
  segfaults from the macOS CLI on some builds. If `device=0` fails, try `--set TopazFilter=…device=-2…`.
- Run one episode end to end and compare it with the PC's output.

### Step 3: make the CLI safe to leave running (gaps 7, 8, 17, 18, one to two days)
- nice/background priority, process groups so Cancel kills the chain, `caffeinate` from the app,
  and the work claims from §4. After this step the Mac can run unattended overnight.

### Step 4: the app on macOS (gaps 10, 16, plus 11/13 polish, one to two days)
- `tauri.macos.conf.json`, `Contents/Resources/scripts`, `externalBin` for `upscale-cli`, a macOS CI job.
- An unsigned build is fine for the owner. Notarize only if it will be shared.

### Step 5 (optional): private tools folder on macOS (gap 15, about two days)
- python-build-standalone + pip wheels (`macosx_*_arm64`), MKVToolNix/HandBrakeCLI from DMG, and
  FFmpeg from a chosen macOS source. Only worth doing if the owner wants "no Homebrew". Homebrew
  works fine for one Mac.

Not planned: vs-mlrt on macOS (gap 19).

## 4. Splitting one library between two machines

How it works now (built 2026-11-02):
- Each machine has its own queue (`queue.json` in its app data dir) with absolute paths. The PC
  sees `Z:\…`; the Mac sees something like `/Volumes/<share>/…`.
  `upscale-settings.json` lookup walks the parents of whatever path it gets, so both work.
- **Claim files** (`core/src/claim.rs`). Before a job touches anything, it creates
  `<output>/.claims/<name>.claim` exclusively (SMB2 honours exclusive create). The file holds the
  computer name, pid and start time. The job refreshes the file's modification time every 2
  minutes and deletes it when it ends: done, failed or cancelled.
  - Another machine skips the episode while the claim is fresh. Its queue shows "skipped (being
    encoded on MAC-MINI)", and the finished `.mkv` arrives from the other machine.
  - A claim not refreshed for 30 minutes is taken over: its machine crashed or lost the share.
  - A machine always takes back its own claim (after a crash it resumes right away).
  - The output folder is checked again after claiming, in case the other machine has just finished.
  - If the claims folder can't be written, the job warns and runs without a claim.
- **Work folders are local** by default (§ "Done before the Mac arrived"), so chunks and EAC3 files
  never cross machines, and the share only gets the finished `.mkv`.

**Setup:**
1. **One shared output folder** on the NAS (`Z:\…\Upscaled` / `/Volumes/…/Upscaled`), set in each
   machine's Settings (or `--out` for the CLI), so the media server sees one library and the claims meet.
2. **Leave the temp folder empty** in the app (local default). For the CLI, pass
   `--temp <local folder>`.
3. Queue the same series on both machines, or different ones. Either works now.

Limits: the two machines must use the same output folder; claims in different output folders
don't see each other. If both machines take over the same stale claim within the same instant, both
can encode that episode. That needs a crash plus exact timing, and costs only duplicate work.
A skipped episode stays skipped in that machine's queue. That's fine, because the other machine
finishes it. If that machine is cancelled instead, re-run the episode.

Paths: nothing in the core hard-codes `C:\` or drive letters (CLAUDE.md requires that), and the
Mac reaches the NAS through an SMB mount under `/Volumes`. One thing to watch: SMB on macOS can be
slow with many small reads. BestSource indexes the source on first open and writes a `.bsindex`
cache. Check where it writes that (next to the source or in a user cache) and that two machines
don't fight over it.

## 5. Open questions for the owner

1. **Topaz seats.** Topaz says legacy perpetual licences come with **2 seats** (two machines signed
   in at once). Is the 7.1.5 licence already signed in anywhere else (a laptop, an old PC)? Does
   Topaz still offer a **7.x macOS** installer in your account, and does it run on the Mac's macOS?
   If not, the Mac can only take Anime AV1.
2. **Which work goes to the Mac?** Anime AV1 (CPU only, no seat) or Anime (Topaz)? Decide after
   the Step 0 numbers.
3. **Where does the Mac read and write?** Which share and mount point (`/Volumes/<share>`)? Is the
   NAS the PC itself (`Z:` shared from Windows) or a separate box? If `Z:` lives on the PC, the
   Mac's reads and writes also load the PC's disk and network while it encodes.
4. **Local scratch on the Mac.** How much free SSD space does it have for chunks and temp files?
   The disk check wants about 2 × source × 2 + 2 GB per job.
5. **Homebrew OK?** Is Homebrew acceptable on the Mac (Step 1), or does it have to be the app's
   private tools folder like on Windows (Step 5)?
6. **Same encoder build on both machines?** On the Mac, SVT-AV1 would come from Homebrew
   (4.3.0 today). The PC uses BtbN's libsvtav1. Is a small size/quality difference between
   machines fine, or should both run the same SVT-AV1 version?
7. **Is the CLI enough on the Mac**, or do you want the app there too (Step 4)?
8. **Should the Mac stay awake on its own** (`caffeinate` during batches), or will you set its
   energy settings to never sleep?
9. **Claim files (§4):** is one-series-per-machine enough, or should both machines work through
   one shared queue?

## Sources

- Topaz CLI docs (macOS path `Contents/MacOS`, `TVAI_MODEL_DIR` / `TVAI_MODEL_DATA_DIR` defaults):
  https://docs.topazlabs.com/video-ai/advanced-functions-in-topaz-video-ai/command-line-interface
- Topaz seats: https://docs.topazlabs.com/sales-account-licensing/your-account/what-are-seats,
  https://docs.topazlabs.com/sales-account-licensing/before-you-buy/how-many-computers-can-i-install-my-apps-on
- Proteus segfault report from the macOS CLI (older beta): https://community.topazlabs.com/t/proteus-segfaults-on-macos-when-run-from-the-command-line/44629
- PyPI JSON for `vapoursynth`, `vapoursynth-bestsource`, `-vivtc`, `-descale`, `-mvtools`, `-akarin`
  (wheel tags as of 2026-11-02); the R79 macOS arm64 wheel's file list (`vapoursynth/vspipe`).
- GitHub latest releases: astral-sh/python-build-standalone 20261026, BtbN/FFmpeg-Builds `latest`
  (no macOS), HandBrake 1.11.2 (`HandBrakeCLI-1.11.2.dmg`), ip7z/7zip 26.05, AmusementClub/vs-mlrt v15.16 (Windows only).
- https://mkvtoolnix.download/macos/releases/103.0/ (arm64 DMG + `.sha256`).
- Homebrew formulae API: ffmpeg 9.0.2 (depends on svt-av1), svt-av1 4.3.0, mkvtoolnix 103.0,
  handbrake 1.11.2, vapoursynth 81.
