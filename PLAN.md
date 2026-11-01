# Upscale app — build plan

A small desktop app that replaces `reference/upscale.ps1`. Drop in videos, pick **Anime** or **CG** (later **Live action**), press Start. Each file is upscaled to 4K where needed and encoded to AV1, with the audio and subtitles cleaned up. You can pause, see progress for each file and reorder the queue.

The PowerShell script is the **reference version**. Its decisions, settings and track rules were tuned on real discs and must carry over unchanged. Every phase below says how to check parity with it.

---

## 1. Goals and non-goals

**Goals**
- Simple by default. Pick a profile and press Start. A handful of options can be changed; everything else is automatic, as it is in the script.
- **Pause / resume** at any time, plus "stop after the current file".
- **Progress per file**: current stage, frames done/total, fps, ETA, projected output size.
- **One default output folder** for every profile (configurable), so input files can live anywhere.
- A queue that keeps its state: closing and reopening the app keeps the queue. From Phase 3, an interrupted encode resumes where it left off.
- Windows first. The code must stay portable to Linux, then macOS (see §9).
- Profiles: **Anime** and **CG** now; **Live action** later (upscale then encode, or encode only when the source is already 4K).

**Non-goals for v1**
- Editing video (trimming, filters beyond the automatic ones).
- Running several GPU-heavy jobs at once. One video job runs at a time; audio work for the next file may overlap.
- Bundling Topaz. It's the user's own install, found on disk.
- A plugin system, cloud features, accounts.

---

## 2. Tech stack

| Layer | Choice | Why |
|---|---|---|
| Shell | **Tauri 2** | Small installer, native webview, cross-platform. |
| UI | **React + TypeScript + Vite** | Use a light component library (e.g. shadcn/ui + Tailwind); no heavy state framework (Zustand is enough) |
| Core | **Rust** (`tokio` for processes and IO) | Process orchestration, the frame pump (pause/progress), parsing tool output. A separate crate with no Tauri dependency, so it can also run as a CLI and be unit-tested |
| Video work | External tools, unchanged from the script: **VapourSynth R79** + plugins + **vs-mlrt** (`upscale.vpy`, `analyze.py`), **ffmpeg/ffprobe**, **SvtAv1EncApp** (or ffmpeg libsvtav1), **mkvmerge**, **HandBrakeCLI**, **Topaz Video AI 7.x ffmpeg** | All the quality decisions live here and are proven. Don't reimplement them in Rust |
| Persistence | JSON files in the app data dir (`settings.json`, `queue.json`, per-job state) | Enough for one user. Can move to SQLite later if history or search is wanted |
| CI | GitHub Actions: lint, test, build the Windows installer on tag | Standard Rust + Node pipeline |

---

## 3. Architecture

```
┌──────────────── React UI ────────────────┐
│ Queue · Add files · Profile/options ·    │
│ File detail (plan, log, compare) ·       │
│ Settings (output folder, tools, Topaz)   │
└──────────────┬───────────────────────────┘
        Tauri commands (UI → core) / events (core → UI: progress, state, log lines)
┌──────────────▼───────────────────────────┐
│ core (Rust crate, also a CLI binary)     │
│  tools/     find, install, version-check and self-test every external tool
│  probe/     ffprobe + mkvmerge -J → SourceInfo, tracks
│  analyze/   cropdetect, idet, analyze.py, season consensus, cache
│  profile/   Anime / CG / (Live) defaults + user overrides → FilePlan
│  plan/      FilePlan: route, crop, IVTC, native, denoise, output size,
│             tracks to keep, names, defaults, EAC3 companions
│  pipeline/  builds and runs the stage chain for a FilePlan
│  pump/      y4m frame pump: progress counting + pause by backpressure
│  mux/       EAC3 encodes, mkvmerge arguments, partial → final rename
│  queue/     jobs, states, persistence, scheduling, cancel, resume
│  report/    plan text, per-job log, comparison screenshots
└──────────────────────────────────────────┘
resources/scripts/upscale.vpy, analyze.py   (bundled with the app)
```

### 3.1 The pipelines

Same as the script; see `CLAUDE.md` for the exact arguments.

| Route | When | Chain |
|---|---|---|
| **Upscale (vs-mlrt)** | Anime, source < 2160p | `vspipe upscale.vpy mode=upscale` → **pump** → SVT-AV1 → `.ivf` |
| **Upscale (Topaz)** | CG, source < 2160p | `vspipe upscale.vpy mode=prep` → Topaz `ffmpeg tvai_up` → **pump** → SVT-AV1 → `.ivf` |
| **Encode only** | Source ≥ 2160p, not AV1 | HandBrakeCLI (keeps HDR10 / HDR10+ / Dolby Vision) |
| **Skip** | Source ≥ 2160p AV1 | none |

Then, for every route: verify the frame count, encode the EAC3 companions, mux with mkvmerge to `.partial.mkv`, rename, and save comparison screenshots.

### 3.2 Pause and progress: the frame pump

Instead of piping process A straight into process B, the core sits in the middle of the **last** pipe:

```
vspipe/Topaz stdout ──► [pump: parse y4m, count frames, honour pause] ──► encoder stdin
```

- **Progress:** the pump parses the y4m stream header (`W`, `H`, `C420p10`, `F`) and then each `FRAME\n` + payload (W×H×1.5×2 bytes for 10-bit 4:2:0). Frames forwarded ÷ expected frames = exact progress. Frames in the last N seconds give fps, and from that the ETA. The `.ivf` size so far × total/done gives the projected size.
- **Pause:** stop reading. Backpressure blocks every upstream process (vspipe, Topaz) and the encoder simply waits for input. Nothing is killed and no OS process suspension is needed. Resuming continues reading. GPU memory stays allocated while paused (document this in the UI tooltip).
- **HandBrake route:** no pump. Parse HandBrakeCLI's `--json` progress output. For pause, suspend/resume the HandBrake process (Windows: `NtSuspendProcess` / `NtResumeProcess`; Unix: SIGSTOP/SIGCONT), behind a small `platform::suspend()` abstraction.
- **Stop after current:** a queue flag, checked between jobs.
- **Cancel:** kill the whole process tree. On Windows, put every spawned process in a **Job Object** with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so nothing is left running even if the app crashes. Delete partial outputs.

### 3.3 Resume after the app closes (Phase 3): chunked encoding

Pausing in memory doesn't survive closing the app or a reboot. Phase 3 adds **chunked encoding**:

- Split each file into segments of about 3–5 minutes at scene cuts (ffmpeg `scdet`, or keyframes from ffprobe), recorded in the job state.
- Encode each segment to its own `.ivf`, using `vspipe start=/frames=`, which `upscale.vpy` already supports. Write `chunk-N.done` when one finishes.
- On resume, skip finished chunks. At the end, join the segments losslessly (`mkvmerge -o video.mkv c1.ivf + c2.ivf + …`), then verify the total frame count.
- Costs: the model loads once per chunk (seconds; the TensorRT engine is cached), and there's a small efficiency loss at chunk boundaries. Chunk boundaries **must** be scene cuts so no keyframe lands mid-scene. Make this an option ("Resumable encoding", default on) and compare quality and size against unchunked on one episode before turning it on by default.

---

## 4. UI (keep it smaller than HandBrake)

**Main window: Queue**
- A drop zone for files/folders, plus an **Add** button. A folder adds every video in it (same extensions as the script).
- On adding: a small sheet with **Profile: Anime (Topaz) | Anime | Anime AV1 | CG | Live action** (Anime (Topaz) is the default), remembering the last choice. When the folder or a parent has an `upscale-settings.json`, preselect from its `Content`.
- Queue rows: file name · profile badge · route (Upscale 1080p→2160p / Encode only / Skip) · stage (Analysing · Upscaling + encoding · Audio · Muxing · Done) · progress bar · fps · ETA · status icon. Drag to reorder; a context menu has Remove, Open log, Show in folder, Re-run.
- Toolbar: **Start**, **Pause/Resume**, **Stop after current**, **Cancel**. The window title and the Windows taskbar progress show overall progress.
- A batch notification when the queue finishes (Windows toast).

**File detail panel** (click a row)
- **Plan:** the same facts as the script's `-Plan` (crop, IVTC, native resolution with its reason, denoise, output size, tracks kept/dropped with their new names, defaults, EAC3 companions, disk estimate).
- **Log:** a live tail of the job log.
- **Compare:** a before/after slider over the comparison screenshots.

**Options** (per job, defaulting from the profile; behind an "Options" disclosure, closed by default)
- Quality: CRF (default 16) and Preset (default 5).
- Anime: Denoise (Auto / None / Light / Strong) and Native resolution (Auto / Off).
- CG: Topaz preset (the A Prores default; or paste a `tvai_up` string).
- Film grain: On (from the profile) / Off.
- Default audio language and default subtitles (language / None).
- **Test clip** (30 s from the middle, video only), for comparing settings.
- Everything else (SVT params, crop, IVTC, matrix) stays automatic, with an "Advanced" JSON editor for power users.

**Settings**
- **Default output folder** (one for all profiles). Output naming: `<name>.mkv`. "Keep the folder structure below the input root": on/off.
- Temp folder (for `.ivf` files and chunks). Defaults to the output drive.
- Tools: a table of every tool with its found version, path and **Self-test**, plus **Update tools** (the script's `-UpdateTools`). The Topaz path is shown with its own self-test.
- GPU in use (NVIDIA preferred; others listed as not used).
- Process priority (default Below normal).

---

## 5. Profiles as data

Profiles are JSON files shipped with the app. A user's edits are saved as overrides; the shipped files are never edited.

```jsonc
// profiles/anime.json
{
  "id": "anime", "name": "Anime",
  "upscaler": "vsmlrt", "model": "cugan",
  "analysis": "auto", "crop": "auto", "ivtc": "auto",
  "crf": 14, "preset": 5,
  "svtParams": "enable-qm=1:qm-min=0:chroma-qm-min=4:enable-variance-boost=1:variance-boost-strength=3:adaptive-film-grain=1:film-grain=2",
  "keepLanguages": ["eng", "jpn", "und"],
  "defaultAudio": "jpn", "defaultSubtitles": "eng",
  "height": 2160
}
// profiles/cg.json: same, except
{ "id": "cg", "name": "CG animation", "upscaler": "topaz",
  "topazFilter": "model=prob-4:scale=0:preblur=0.25:noise=0.75:details=0.75:halo=0.15:blur=0.5:compression=0.5:estimate=8:blend=0.75:device=0:vram=1:instances=1",
  "analysis": "off", "defaultAudio": "eng", "defaultSubtitles": "none" }
```

Precedence, as in the script: job options > `upscale-settings.json` next to or above the file (keep supporting these) > user profile overrides > shipped profile > automatic analysis > built-in defaults. Anything set explicitly is never overridden by analysis.

---

## 6. Phases

Each phase ends with something usable and a parity check. Work on a branch and open a PR per milestone.

### Phase 0: Scaffold (½ day)
- Tauri 2 + React + TS + Vite app; a Cargo workspace with `core` (lib + `upscale-cli` binary) and `app` (Tauri).
- Copy `reference/upscale.vpy` and `reference/analyze.py` to `resources/scripts/`, bundled as Tauri resources.
- GitHub Actions: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, `pnpm lint`, `pnpm test`, and a Windows build artifact.
- **Done when:** the app opens an empty Queue window and CI is green.

### Phase 1: Core engine, headless (largest phase)
Port the script's logic into `core`, driven by `upscale-cli`:
- `upscale-cli tools [--update]`: find, install and self-test tools (§7).
- `upscale-cli plan <path> [--profile anime|cg]`: prints the same plan the script's `-Plan` prints.
- `upscale-cli run <path> [--profile …] [--test-clip]`: full processing with progress lines on stdout (JSON lines, which the UI reuses later).

Modules: `probe`, `analyze` (cropdetect, idet, analyze.py runner, cache, season consensus), `plan` (all decisions), `pipeline` + `pump`, `mux`, `report` (plan text, log, screenshots).

**Tests (required):**
- Unit tests with fixture JSON (captured `ffprobe -show_streams -show_format -of json` and `mkvmerge -J` output from real files) for: track keep/drop, track names (`Japanese - TrueHD 5.1`, `English - DD+ 5.1`, commentary), default flags (including `none`), EAC3 companion selection and bitrate, the 4K route decision, crop parsing, the IVTC decision, output width with SAR, settings precedence and parent-folder inheritance, and season consensus.
- A y4m pump test with a generated stream: frame counting, and that pause blocks the producer.
- Golden tests: the arguments generated for vspipe, Topaz, SvtAv1EncApp, ffmpeg-libsvtav1, HandBrakeCLI and mkvmerge, checked against snapshots taken from what `reference/upscale.ps1` builds for the same plan.

**Parity check (manual, on the owner's PC):** run the script and the CLI on the same 3 files (1080p anime, telecined DVD, 4K HDR film). They must match on frame count, track layout, names and flags, and the SVT settings shown in the encoder log. Output sizes should be within ±1%.

### Phase 2: Desktop UI
- The Queue, Add sheet, Options, File detail (Plan / Log) and Settings screens from §4.
- Tauri events stream progress from `core`. Pause/resume via the pump; HandBrake via process suspension. Cancel through the Job Object.
- The queue is persisted in `queue.json`. On restart, unfinished jobs show as "Interrupted, will restart from the beginning" (until Phase 3).
- **Done when:** a whole season can be queued, paused overnight and resumed, with a correct progress bar and ETA.

### Phase 3: Robustness
- Chunked, resumable encoding (§3.3), with its quality/size A/B before it becomes the default.
- Crash recovery: partial files cleaned up, and the job log notes the restart.
- Disk-space check before each job (the script's formula), shown in the plan.
- The Compare view (before/after slider) and a Windows toast when the batch finishes.

### Phase 4: Self-contained tools and installer
- Move from winget/global installs to a **private tools folder in the app data dir**: portable Python plus VapourSynth R79 (pip into that Python, then `vapoursynth config`), the vs-mlrt CUDA/Vulkan package, BtbN FFmpeg, SvtAv1EncApp, MKVToolNix portable and HandBrakeCLI portable. Downloads are checked against SHA-256 and use the latest-release logic from the script.
- Signed Windows installer (Tauri bundler, NSIS or MSI) and Tauri's updater for the app itself.
- **Done when:** a clean Windows VM goes from installer to a finished episode without manual steps (Topaz excepted).

### Phase 5: Live-action profile
- **Done (2026-11-01):** the `live` profile encodes with HandBrake at every size: CRF 14, preset 5, tune SSIM, `enable-qm=1:qm-min=2:chroma-qm-min=8:qp-scale-compress-strength=1:variance-boost-strength=2:adaptive-film-grain=1:film-grain=4` (the owner's values).
- Upscale for live action: Topaz (Proteus / Iris / Rhea, user-chosen) through the same prep → Topaz → pump → encoder chain. No anime analysis.
- An optional **H.265 output** for devices that can't decode AV1 (the owner has one). It needs separate encoder settings.
- Live-action defaults: English audio, subtitles none, keep forced.

### Phase 6: Linux, then macOS
See §9. Abstract early (Phase 1) so this is mostly packaging.

---

## 7. Tool management

Port `Initialize-Tools`, `Initialize-VsMlrt`, `Initialize-BtbnFfmpeg`, `Test-VsMlrt` and `Initialize-Topaz`/`Test-Topaz` from the script. Rules that must be kept are in `CLAUDE.md` → "Hard-won rules".

- Every tool has `find()`, `version()`, `install_or_update()` and `self_test()`.
- **Self-tests run before a batch starts**, not inside each job, so a broken tool stops the batch up front with the tool's own error.
- Tool status is shown in Settings. "Update tools" runs install_or_update followed by every self-test.
- Encoder choice: the newest SVT-AV1 among SvtAv1EncApp, BtbN ffmpeg and winget ffmpeg (ties: SvtAv1EncApp, then BtbN, then winget).

---

## 8. Risks and decisions

| Risk | Mitigation |
|---|---|
| VapourSynth R80+ breaks vs-mlrt (API 3) | Pin R79 and `vapoursynth-bestsource<22`. Re-check vs-mlrt releases each quarter; the pin is one constant |
| The Topaz CLI only exists in Video AI ≤ 7.x / Topaz Video 1.0 | Detect the version and show a clear message. Topaz is optional: the CG profile can fall back to vs-mlrt with a warning, if the owner wants that |
| Topaz writing non-video to stdout | Self-test checks the output starts with `YUV4MPEG2` (already in the script) |
| Python dependency is fragile | Phase 4 private portable Python. Never touch the user's global Python once that's done |
| GPU memory held while paused | Tooltip. "Stop after current" for long breaks |
| Chunking hurts quality | A/B before making it the default; minimum chunk length; cut only at scene changes |
| GPL tools | Run them as separate processes (as now). Don't link them. Don't bundle Topaz |

---

## 9. Cross-platform notes (for later, but design for it now)

- All paths go through `std::path`. No hard-coded `C:\`. Tool lookup goes through `tools/` with per-OS candidates.
- Process control (suspend, job object or process group, priority, hidden console window) goes in `platform/` with Windows, Linux and macOS implementations.
- **Linux:** vs-mlrt TensorRT works on Linux with NVIDIA. A Linux box with an **AMD** GPU needs the ncnn/Vulkan backend (already supported) or MIGraphX. Topaz Video AI has no Linux version as far as we know, so on Linux CG would fall back to vs-mlrt (e.g. a waifu2x/photo model).
- **macOS:** no NVIDIA, so use vs-mlrt ncnn (Vulkan via MoltenVK) or CoreML if vs-mlrt supports it then. Topaz has a Mac version, but the CLI-removal caveat applies.
- HandBrakeCLI, ffmpeg, mkvmerge and SVT-AV1 exist on all three.

---

## 10. Working with Claude Code on this repo

1. Create a GitHub repo, unzip this starter into it and commit (`CLAUDE.md`, `PLAN.md`, `reference/`).
2. Open the folder in Claude Code. First prompt:
   > Read CLAUDE.md and PLAN.md. Then do Phase 0. Use plan mode first and show me the plan before writing code.
3. One phase (or one module of Phase 1) per session or PR. Start each with "Read PLAN.md §X and the matching parts of reference/upscale.ps1, then propose the implementation."
4. After each Phase 1 module, ask for the unit tests listed above and run them in CI.
5. Run the parity checks yourself on your Windows PC with real files. Claude Code can't see your GPU or Topaz from a cloud session. Run Claude Code locally on the PC for those steps.
6. Keep `reference/` read-only. If the app's behaviour should change, note it in `CLAUDE.md` → "Intentional differences from the script".
