# upscale

Batch-upscales anime and CG animation to 4K and encodes it to AV1 on Windows. Point it at a folder of rips and it works out, file by file, how to handle each one, then produces finished MKVs ready for Jellyfin.

## Folder layout

One copy of the code, plus one folder per kind of job. **Which folder you run it in decides how it's handled.**

```
C:\Tools\upscale\                    the code: upscale.ps1, upscale.vpy, analyze.py, README.md
  └── tools\                         downloaded automatically (vs-mlrt, FFmpeg, ...). Never put videos here
C:\Tools\upscale-anime\              anime rips. No settings file needed: anime is the default
  └── upscaled\                      finished files, logs, comparison images (created automatically)
C:\Tools\upscale-animation\          CG / 3D films
  ├── upscale-settings.json          { "Content": "cg" }
  └── upscaled\
```

To process a folder, open PowerShell, `cd` into it and run `upscale.ps1`.

- **1080p and lower** (Blu-ray, DVD): AI upscale to 2160p with vs-mlrt, then encoded with SVT-AV1.
- **Already 4K, not AV1:** no upscale; encoded with HandBrake so HDR10, HDR10+ and Dolby Vision survive.
- **Already 4K AV1:** skipped.

Every file also gets the same audio and subtitle treatment:

- **Languages:** only English, Japanese and undefined tracks are kept.
- **EAC3 companions:** lossless and DTS tracks get an extra EAC3 track for soundbar compatibility; the originals stay in the file.
- **Defaults:** Japanese audio and English subtitles.

**CG / 3D animation films** (Pixar/Disney-style) get their own folder and are upscaled with your Topaz Video AI 7.x instead. See [CG films with Topaz](#cg-films-with-topaz).

---

## Contents

- [Requirements](#requirements)
- [Setup](#setup)
- [Quick start](#quick-start)
- [Moving from the old layout](#moving-from-the-old-layout)
- [What happens to each file](#what-happens-to-each-file)
- [What it decides automatically](#what-it-decides-automatically)
- [Output and logs](#output-and-logs)
- [Options](#options)
- [Per-show settings file](#per-show-settings-file)
- [CG films with Topaz](#cg-films-with-topaz)
- [Test clips and the grain comparison](#test-clips-and-the-grain-comparison)
- [Using a different upscaling model](#using-a-different-upscaling-model)
- [Troubleshooting](#troubleshooting)
- [Limitations](#limitations)

---

## Requirements

- **Windows 10 or 11** with **winget** ("App Installer" from the Microsoft Store; preinstalled on current Windows).
- **PowerShell 7.4 or newer.** Windows PowerShell 5.1 corrupts binary data piped between programs, so the script refuses to run on it. Install once with:
  ```powershell
  winget install Microsoft.PowerShell
  ```
  Then run the script from `pwsh`, not `powershell`.
- **A GPU.**
  - **NVIDIA:** uses TensorRT, which is fastest. The first run downloads a CUDA package of several GB.
  - **AMD or Intel:** uses Vulkan.
- **Disk space:** the script reserves about 4× the source size per upscaled episode (minimum ~10 GB), or about 2× for 4K re-encodes, while that episode is being worked on. It checks before each episode.

Everything else is installed automatically on first run (see [Setup](#setup)).

## Setup

### 1. Put the files in one folder

Create `C:\Tools\upscale` and put these files in it:

- `upscale.ps1`: the batch script you run
- `upscale.vpy`: the VapourSynth upscaling script
- `analyze.py`: per-file analysis (native resolution, noise)
- `README.md`: this file

Keep them together; the script looks for the other two files next to itself. Then create the job folders `C:\Tools\upscale-anime` and `C:\Tools\upscale-animation`, and put `upscale-settings.json` in `upscale-animation` (see [Folder layout](#folder-layout)).

> **Moving from the old layout** (everything in `C:\Tools\upscale-anime`, script called `upscale-anime.ps1`)? See [Moving from the old layout](#moving-from-the-old-layout) instead of steps 1–3.

### 2. Allow the script to run

Windows blocks PowerShell scripts that came from the internet. Open **PowerShell 7** (Start menu → type `pwsh` → press Enter) and run these two commands once. Change the folder path if you used a different one.

```powershell
Unblock-File -Path 'C:\Tools\upscale\*'
Set-ExecutionPolicy -Scope CurrentUser -ExecutionPolicy RemoteSigned
```

If the second command asks for confirmation, type `Y` and press Enter.

- `Unblock-File` removes the "downloaded from the internet" mark from the three files.
- `Set-ExecutionPolicy RemoteSigned` lets your own scripts run, while still blocking unsigned scripts downloaded later. It's saved permanently; you never need to run it again. Check with `Get-ExecutionPolicy -List`: the `CurrentUser` row should say `RemoteSigned`.

**Unblock updates automatically (recommended).** Every time you download a newer version of the files, Windows marks them as downloaded again and blocks the script until it's unblocked. To have that happen automatically whenever PowerShell 7 opens, run this once:

```powershell
if (-not (Test-Path $PROFILE)) { New-Item -ItemType File -Path $PROFILE -Force | Out-Null }
Add-Content -Path $PROFILE -Value "Unblock-File -Path 'C:\Tools\upscale\*' -ErrorAction SilentlyContinue"
```

This adds one line to your PowerShell profile (a script PowerShell runs at startup). It only affects the tool's folder. If you replace files while a PowerShell window is already open, open a new window, or run the `Unblock-File` line once by hand.

### 3. Make the script runnable from any folder (recommended)

Windows finds commands by looking through a list of folders called the **PATH**. Adding the tool's folder to that list lets you type `upscale.ps1` while you're inside any rips folder, instead of typing its full location every time.

Choose **one** of these two methods.

**Method A: one command (easiest)**

In PowerShell 7, paste this and press Enter. Change the folder on the first line if you used a different one.

```powershell
$dir = 'C:\Tools\upscale'
$current = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($current -split ';') -notcontains $dir) {
    [Environment]::SetEnvironmentVariable('Path', "$current;$dir", 'User')
    'Added. Close this window and open a new one.'
} else { 'Already on your PATH.' }
```

This only changes your own user account's PATH. It doesn't need administrator rights and doesn't affect other users.

**Method B: through Windows Settings**

1. Click **Start**, type `environment variables`, and click **Edit environment variables for your account**.
2. In the top box (**User variables for *your name***), click the row named **Path**, then click **Edit…**.
3. Click **New**, then type or paste `C:\Tools\upscale` (your tool folder).
4. Click **OK** on that window, then **OK** on the Environment Variables window.

**Then, for either method:**

1. Close every PowerShell window, then open a new one. Windows only reads the PATH when a window opens.
2. Check it worked:
   ```powershell
   Get-Command upscale.ps1
   ```
   If it prints a line containing `C:\Tools\upscale\upscale.ps1`, you're done. If it says "not recognized," the folder path you added doesn't exactly match where the files are.

**Skipping this step:** you can always run the script by its full location instead. Note the `&` and the quotes:

```powershell
cd 'D:\Rips\Show S01'
& 'C:\Tools\upscale\upscale.ps1' -Plan
```

### 4. Install the tools

Run this once. It can be run from any folder:

```powershell
upscale.ps1 -UpdateTools
```

It installs everything the tool needs (table below) and then stops; it doesn't touch any videos.

- **Time:** expect several minutes. On an NVIDIA PC it downloads a GPU package of several GB.
- **Prompts:** you may see a few Windows permission prompts while programs install; click **Yes**.
- **Finished:** it ends with "All tools are installed and up to date."

Run the same command again whenever you want to update everything.

### 5. First run: see the plan

Go to a folder of rips and run Plan mode. It analyzes every file and reports what it would do, without encoding anything:

```powershell
cd 'D:\Rips\Show S01'
upscale.ps1 -Plan
```

The report is printed and saved to `upscaled\logs\plan-<date>.txt`. When it looks right, run `upscale.ps1` in the same folder to do the real thing.

### What gets installed

| Tool | Source | Used for |
|---|---|---|
| FFmpeg / ffprobe | winget `Gyan.FFmpeg` | Probing, EAC3 audio, crop/interlace detection, screenshots, AV1 (if no SvtAv1EncApp) |
| FFmpeg (BtbN build) | GitHub, BtbN daily builds, into `tools\ffmpeg` next to the script (by `-UpdateTools` only) | AV1 encoding when its SVT-AV1 is newer than the winget FFmpeg's |
| MKVToolNix | winget `MoritzBunkus.MKVToolNix` | Track listing and final muxing |
| Python 3.13 | winget `Python.Python.3.13` | VapourSynth and the analysis script |
| VapourSynth R79 + plugins | pip: `vapoursynth==79`, `-bestsource` (<22), `-vivtc`, `-descale`, `-mvtools`, `-akarin` | Frame serving, inverse telecine, descaling, temporal stabilization; akarin is needed by vs-mlrt itself |
| vs-mlrt | GitHub, latest release, into `tools\vs-mlrt` next to the script | AI upscaling (CUDA package for NVIDIA, Vulkan package otherwise) |
| 7-Zip | winget `7zip.7zip` | Extracting vs-mlrt (only when downloading it) |
| HandBrakeCLI | winget `HandBrake.HandBrake.CLI` | 4K sources only (installed by `-UpdateTools`, or the first time a 4K file appears) |

If a tool is missing when you run the script normally, it's installed then too, so step 4 is a convenience, not a requirement.

**Why VapourSynth R79 and not the newest:** vs-mlrt's AI plugins still use VapourSynth's older plugin interface (API 3), which VapourSynth R80 removed ([vs-mlrt issue #171](https://github.com/AmusementClub/vs-mlrt/issues/171)). The script therefore installs R79, and BestSource 21 (version 22 needs R80). If a newer VapourSynth is already installed, the script switches it to R79. Once vs-mlrt publishes builds for the new interface, the pin (`$VsVersionPin` near the top of the tool section in `upscale.ps1`) can be raised.

**If you already use VapourSynth for something else:** this tool's VapourSynth is the same Python installation, so other VapourSynth setups on the PC will also see R79. R75 and newer only load plugins from their own plugin folder, so a setup that relies on plugins in older locations (for example an older mpv upscaling setup) may need its plugins reinstalled.

**SVT-AV1 (the AV1 encoder):** the script can get SVT-AV1 from three places and encodes with whichever has the **newest** version:

1. **FFmpeg (BtbN build):** `-UpdateTools` downloads the latest daily Windows build from [BtbN's FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds) (one of the Windows builds linked from ffmpeg.org) into `tools\ffmpeg`. These builds track SVT-AV1 closely, so this is usually the newest, and it's how `-UpdateTools` keeps the encoder current.
2. **FFmpeg from winget** (Gyan's build): also used for everything else (probing, audio, screenshots). Its SVT-AV1 can lag a release behind.
3. **SvtAv1EncApp.exe:** the standalone encoder, if you put one on your `PATH` or in `tools\svt-av1\`. Not installed automatically, since there's no official Windows download.

On a tie, SvtAv1EncApp wins, then the BtbN build. The tools check prints which one was chosen, for example `SVT-AV1 v4.2.0 via FFmpeg (BtbN build) libsvtav1 (latest)`. If it shows an older version with a note like `(newest release is v4.2.0 ...)`, that's information, not an error: encoding works normally, and a later `-UpdateTools` picks up the newer encoder once a build includes it.

## Quick start

After [Setup](#setup):

```powershell
upscale.ps1 -UpdateTools   # 0. once: install / update tools (any folder)

cd C:\Tools\upscale-anime    # or C:\Tools\upscale-animation for CG films
upscale.ps1 -Plan          # 1. see every decision it would make; encodes nothing
upscale.ps1 -CompareGrain  # 2. optional: grain vs. no-grain test clip with screenshots
upscale.ps1                # 3. do the whole folder
```

Re-running the same command resumes a batch: finished episodes are skipped and analysis results are reused.

A job folder can also be anywhere else, e.g. `D:\Rips\Show S01`: anime needs nothing extra there. For a CG film elsewhere, copy `upscale-settings.json` into its folder (or a parent).

## Moving from the old layout

If you already have everything in `C:\Tools\upscale-anime` with the script named `upscale-anime.ps1`, do this once. First close every PowerShell window that's inside `C:\Tools\upscale-anime`, or the rename fails. Then open a new PowerShell 7 window (`pwsh`) and run each block in order:

```powershell
# 1. The old folder becomes the code folder. tools\ moves with it, so nothing re-downloads
#    (including TensorRT engines).
Rename-Item C:\Tools\upscale-anime upscale

# 2. Replace the code with the new files. Copy upscale.ps1, upscale.vpy, analyze.py and
#    README.md into C:\Tools\upscale (overwrite), then delete the old script:
Remove-Item C:\Tools\upscale\upscale-anime.ps1

# 3. Create the job folders. Then copy upscale-settings.json into C:\Tools\upscale-animation
New-Item -ItemType Directory C:\Tools\upscale-anime, C:\Tools\upscale-animation

# 4. Point PATH at the new code folder (replaces the old entry in your user PATH)
$p = [Environment]::GetEnvironmentVariable('Path', 'User')
[Environment]::SetEnvironmentVariable('Path', ($p -replace [regex]::Escape('C:\Tools\upscale-anime'), 'C:\Tools\upscale'), 'User')

# 5. Point the auto-unblock line in your profile at the new folder
(Get-Content $PROFILE) -replace [regex]::Escape('C:\Tools\upscale-anime\*'), 'C:\Tools\upscale\*' | Set-Content $PROFILE
Unblock-File -Path 'C:\Tools\upscale\*'
```

Now **close PowerShell and open a new window** (PATH changes only reach new windows) and check:

```powershell
Get-Command upscale.ps1
```

It should show `C:\Tools\upscale\upscale.ps1`. If you had rips or an `upscaled` folder inside the old `C:\Tools\upscale-anime`, they're now in `C:\Tools\upscale`. Move them into the new `C:\Tools\upscale-anime`. Leave `tools` where it is.

## What happens to each file

### Upscale route (sources below 4K)

1. **Profile:** reads the video's properties, detects black bars and telecine, plans which tracks to keep.
2. **Analyze:** finds the show's native resolution and noise level, then applies the folder's majority decision (see [season consistency](#season-consistency)).
3. **Check disk space.** The batch stops cleanly if there isn't enough.
4. **Count frames:** the number of frames the upscale should produce.
5. **Upscale and encode:** VapourSynth streams upscaled frames straight into SVT-AV1. No intermediate file is written, so there's no ~100 GB MOV.
6. **Verify:** the encoded frame count must match exactly. A crash mid-episode is caught here instead of producing a truncated file.
7. **Audio:** encode EAC3 companion tracks.
8. **Mux:** written to `name.partial.mkv`, then renamed, so an interrupted mux is never mistaken for a finished episode.
9. **Screenshots:** side-by-side comparison images.

Inside the upscale script, in order:

1. **Inverse telecine**, if needed.
2. **Crop** black bars.
3. **Descale** to the native resolution, if one was detected.
4. **AI model passes**, repeated until the frame is at least 2160p:
   - 1080p: one 2x pass
   - 720p: one 3x pass
   - SD: two or three passes
5. **Downscale** to exactly 2160p in BT.709, keeping the correct display shape (a 4:3 show comes out 2880×2160).
6. **Optional** debanding and temporal stabilization.

### 4K route (sources already 2160p, not AV1)

HandBrakeCLI encodes with the same SVT-AV1 settings, with no upscale, crop or deinterlace. It carries HDR10, HLG, HDR10+ and Dolby Vision into the AV1 stream:

| Dolby Vision profile | Result in AV1 (profile 10) | Fallback on non-Dolby Vision players |
|---|---|---|
| 5 | 10.0 | None. Needs a player that supports Dolby Vision in AV1; the log warns |
| 7 | 10.1; enhancement layer dropped, RPU converted to 8.1 | HDR10 |
| 8.1, 8.4 | 10.x | HDR10 or HLG base |

Audio and subtitles then go through the same steps as the upscale route.

## What it decides automatically

| Decision | How | Override |
|---|---|---|
| **Backend** | NVIDIA → TensorRT; otherwise Vulkan (ncnn) | `-Backend` |
| **Model** | A `.onnx` file next to the script is used (newest if several); otherwise Real-CUGAN | `-Model`, `-Onnx` |
| **Encoder** | SvtAv1EncApp or FFmpeg's libsvtav1, whichever is newer | put `SvtAv1EncApp.exe` in `tools\svt-av1` |
| **Inverse telecine** | Metadata says interlaced, *or* the picture shows field combing (checked for 29.97 and 25 fps sources, since telecined files are often mislabeled "progressive"). 29.97 → field-match and decimate to 23.976; 25 → field-match only | `-Ivtc on/off` |
| **Black bars** | Detected at five points per episode; the smallest crop seen wins, so a dark scene can't cut into the picture | `-Crop off` |
| **Color matrix / pixel shape** | Read from the source (SD defaults to BT.601). Output is always BT.709 with the correct display shape | `-Matrix` |
| **Native resolution** (1080p sources) | Scans heights 540–1070 with six common scaling kernels for the sharp error dip that marks the resolution the studio drew at. A single episode only uses it on a **strong** match (prominence ≥ 0.12, most sampled frames agreeing). A weaker match (0.06–0.12) is only used when most of the season finds the same one. Otherwise it upscales from 1080p, since a wrong descale does more harm than none | `-NativeHeight` (+ kernel options) |
| **Denoise** (Real-CUGAN) | Noise measured in flat areas: < 0.8 none, < 1.6 light, otherwise strong | `-Noise` |
| **Audio / subtitles** | Keep eng/jpn/und. EAC3 companion for TrueHD, DTS, DTS-HD, FLAC and PCM (5.1 at 960k, stereo at 320k; 6.1/7.1 downmixed to 5.1). Default audio: the Japanese EAC3 track if one was made, else the first Japanese track. Default subs: first full English track (skips forced and "Signs & Songs" when possible). Every kept audio track is renamed `Language - Format Channels`, e.g. `Japanese - TrueHD 5.1`, `English - DTS-HD MA 5.1`, `Japanese - DD+ 5.1`; commentary tracks keep a `(Commentary)` suffix | `-KeepLanguages`, `-DefaultAudioLanguage`, `-DefaultSubtitleLanguage` |

### Season consistency

With `-Analysis auto` (the default), every file in the folder is analyzed before anything is encoded:

- **Native resolution:** if 60%+ of episodes find the same match (weak or strong), every episode uses it, except one with a *strong* match of its own that differs (a recap or special can genuinely differ; it's flagged in the log). Without that majority, only strong per-episode matches are used, so scattered weak matches (for example different heights on different episodes) mean no descale.
- **Denoise:** one level for the whole folder, from the median noise across episodes.

This needs at least three episodes. Use `-Analysis episode` to decide each file on its own, or `-Analysis off` to skip analysis.

## Output and logs

By default everything goes to an `upscaled` folder inside the rips folder:

```
D:\Rips\Show S01\
├── Episode 01.mkv                  (your rips, untouched)
├── upscale-settings.json           (optional, see below)
└── upscaled\
    ├── Episode 01.mkv              (finished files)
    ├── compare\
    │   └── Episode 01_01.png       (left: source scaled up, right: final encode)
    └── logs\
        ├── batch-20261028-090000.log
        ├── plan-20261028-085500.txt
        ├── Episode 01.log          (every decision, with the measurements behind it)
        └── analysis\
            └── Episode 01.json     (cached analysis; reused while the source is unchanged)
```

Each episode log records:

- the source details
- the crop, inverse telecine, native resolution and denoise decisions, with their reasons
- the tracks kept and dropped, and the default tracks
- frame counts and timings
- any errors from the upscaler, HandBrake or mkvmerge

The batch log ends with a summary of done, skipped and failed episodes.

## Options

All options are optional. Run with none for the automatic behavior.

### Input and output

| Option | Default | Description |
|---|---|---|
| `-Path` | current folder | A folder of videos (`.mkv .mp4 .m2ts .m4v .avi .webm`) or a single file |
| `-OutDir` | `<rips folder>\upscaled` | Where finished files, logs and screenshots go |
| `-Content` | `anime` | `anime` or `cg` (3D films). Normally set by the folder's settings file, see [CG films with Topaz](#cg-films-with-topaz) |
| `-Upscaler` | `auto` | `auto` (vs-mlrt for anime, Topaz for cg), `vs`, `topaz` |
| `-TopazFilter` | your "A Prores" preset | Topaz `tvai_up` settings; the output size is added automatically |
| `-TopazPath` | `C:\Program Files\Topaz Labs LLC\Topaz Video AI` | Topaz Video AI install folder |

### Workflow

| Option | Description |
|---|---|
| `-Plan` | Analyze everything and write a report of what would happen (route, crop, IVTC, native resolution, denoise, output size, tracks, defaults, disk space). Encodes nothing and doesn't download vs-mlrt |
| `-CompareGrain` | Encode a 720-frame clip from the middle of the first episode twice: with your film-grain settings and with `film-grain=0`. Screenshots of both go to `compare\` |
| `-Start`, `-Frames` | Test clip: `-Frames 720` encodes 720 frames starting at `-Start` (`-1` = middle of the episode). Test clips are video only |
| `-Screenshots` | Comparison images per upscaled file (default 4, `0` turns them off) |
| `-UpdateTools` | Install or upgrade every tool (including HandBrakeCLI), then stop. Doesn't process any videos; can be run from any folder |
| `-KeepIvf` | Keep the intermediate video stream after muxing |

### Upscaling

| Option | Default | Description |
|---|---|---|
| `-Model` | auto | `cugan` (Real-CUGAN) or `onnx` |
| `-Onnx` | auto | Path to an `.onnx` model |
| `-Backend` | auto | `trt`, `cuda` (NVIDIA), `dml`, `ncnn` (AMD/Intel/any Vulkan) |
| `-Noise` | auto | Real-CUGAN denoise: `-1` none, `0` light, `3` strong |
| `-Analysis` | `auto` | `auto` (per file + season consensus), `episode` (per file only), `off` |
| `-Crop` | `auto` | `auto` or `off` |
| `-Ivtc` | `auto` | `auto`, `on`, `off` |
| `-Matrix` | auto | Source color matrix: `709`, `170m`, `470bg` |
| `-NativeHeight` | auto | Descale to this height before upscaling (`0` = none). Overrides detection |
| `-NativeKernel` | `bicubic` | `bilinear`, `bicubic`, `lanczos`, `spline36` |
| `-NativeB`, `-NativeC` | `0`, `0.5` | Bicubic parameters |
| `-NativeTaps` | `3` | Lanczos taps |
| `-Temporal` | off | Temporal stabilization against frame-to-frame shimmer. Uses vs_temporalfix if installed, otherwise a gentler MVTools-based stabilizer. CPU-heavy |
| `-TemporalStrength` | method default | Strength for whichever method runs |
| `-Deband` | off | Light deband after upscaling (needs an f3kdb plugin; the script tries to install one and logs a warning if it can't) |
| `-Height` | `2160` | Output height |

### Encoding

| Option | Default | Description |
|---|---|---|
| `-Crf` | `14` | SVT-AV1 quality |
| `-Preset` | `5` | SVT-AV1 speed/efficiency preset |
| `-SvtParams` | `enable-qm=1:qm-min=0:chroma-qm-min=4:enable-variance-boost=1:variance-boost-strength=3:adaptive-film-grain=1:film-grain=2` | Extra SVT-AV1 settings in `key=value:key=value` form |

### Tracks

| Option | Default | Description |
|---|---|---|
| `-KeepLanguages` | `eng, jpn, und` | Audio and subtitle languages to keep |
| `-DefaultAudioLanguage` | `jpn` (cg: `eng`) | Language of the default audio track |
| `-DefaultSubtitleLanguage` | `eng` (cg: `none`) | Language of the default subtitle track. `none` = no subtitles on by default; forced tracks keep their own flag |

## Per-show settings file

To change settings for one show without typing options every time, put an `upscale-settings.json` in that rips folder. It's picked up automatically.

It also applies to every folder **below** it. So a settings file in `C:\Tools\upscale-animation` covers `C:\Tools\upscale-animation\Toy Story` too. If folders at several levels have one, the nearest folder's values win, and the log's first lines list which files were used.

```json
{
  "NativeHeight": 810,
  "NativeKernel": "bicubic",
  "NativeB": 0.3333,
  "NativeC": 0.3333,
  "Temporal": true,
  "SvtParams": "enable-qm=1:qm-min=0:chroma-qm-min=4:enable-variance-boost=1:variance-boost-strength=3:film-grain=0"
}
```

**Allowed keys:** `Content`, `Upscaler`, `TopazFilter`, `TopazPath`, `Model`, `Onnx` (a relative path is relative to the settings file's folder), `Noise`, `Matrix`, `Analysis`, `Crop`, `Screenshots`, `Ivtc`, `NativeHeight`, `NativeKernel`, `NativeB`, `NativeC`, `NativeTaps`, `Temporal`, `TemporalStrength`, `Deband`, `Crf`, `Preset`, `SvtParams`, `KeepLanguages` (a list, e.g. `["eng","und"]`), `DefaultAudioLanguage`, `DefaultSubtitleLanguage`. Unknown keys are ignored with a warning.

**Precedence:** command line > nearest settings file > settings files further up > `Content` defaults > automatic analysis > built-in defaults. Anything you set yourself is never overridden by analysis.

## CG films with Topaz

3D/CG films don't share anime's problems (no sub-1080p native drawings, no flat line art), so the anime analysis does nothing useful for them. Topaz Proteus, which looks at several frames at once, suits them better. CG mode runs your Topaz Video AI's own ffmpeg with the exact settings of your **A Prores** preset, and pipes its 4K frames straight into SVT-AV1. **No ProRes MOV is written**: the only file on disk is the AV1 stream, then the finished MKV.

```
source rip → decode, IVTC, crop (VapourSynth) → Topaz Proteus 4 at 4K → SVT-AV1 (CRF 14, preset 5, same settings as anime) → MKV with audio/subs from the rip
```

### One-time setup

1. Make the folder (File Explorer → `C:\Tools` → New → Folder → `upscale-animation`).
2. Put the included `upscale-settings.json` in it. It contains just:
   ```json
   { "Content": "cg" }
   ```
3. That's it. The code stays in `C:\Tools\upscale`. Because that folder is on your `PATH`, you run the same command from the new folder.

### Each time

```powershell
cd C:\Tools\upscale-animation
upscale.ps1 -Plan     # optional: check routes, crop and tracks
upscale.ps1
```

Films can go straight in `C:\Tools\upscale-animation` or in subfolders (run the command inside the subfolder; the settings file above it still applies). Output goes to `upscaled\` next to the films, as for anime.

### What `"Content": "cg"` changes

| | Anime (default) | CG |
|---|---|---|
| Upscaler | vs-mlrt (Real-CUGAN / ONNX) | Topaz Video AI, Proteus 4 (`prob-4`, relative to auto) |
| Native-resolution / denoise analysis | on | off (Topaz handles noise and detail) |
| IVTC, black-bar crop, anamorphic correction | on | on (done before Topaz) |
| AV1 settings | CRF 14, preset 5, your SVT params | same |
| Default audio | Japanese | English |
| Default subtitles | English | none (forced tracks keep their flag) |
| Languages kept, EAC3 companions, track names | eng/jpn/und, as usual | same |
| 4K sources (HandBrake route), skip already-4K AV1 | yes | same |

Any of these can still be overridden in the settings file, e.g. `{ "Content": "cg", "DefaultSubtitleLanguage": "eng" }`.

### Topaz requirements

- **Stay on Topaz Video AI 7.x** (you have 7.1.5). The newer "Topaz Video" app (1.1 and later) removed the command-line ffmpeg this uses. If you ever update, CG mode stops at the self-test with a clear message.
- Topaz must have been **signed in once** in the app (it keeps a sign-in file in `C:\ProgramData\Topaz Labs LLC\Topaz Video AI\models`), and the Proteus 4 model must be downloaded. Exporting anything once with the A Prores preset does both.
- Before each batch the script upscales a half-second test pattern with your settings. A sign-in, model or GPU problem stops the batch there with Topaz's own error.
- `-UpdateTools` doesn't install or update Topaz. It only runs the same self-test if Topaz is present.

### Changing the Topaz settings

The default `TopazFilter` is copied from your Topaz logs for the A Prores preset:

```
model=prob-4:scale=0:preblur=0.25:noise=0.75:details=0.75:halo=0.15:blur=0.5:compression=0.5:estimate=8:blend=0.75:device=0:vram=1:instances=1
```

To use a different preset, export a few seconds with it in Topaz. Then open the newest log in `%APPDATA%\Topaz Labs LLC\Topaz Video AI\logs` (paste that into the File Explorer address bar) and search for `tvai_up=`. Copy the text after `tvai_up=` up to the first comma, delete the `:w=3840:h=2160` part, and put the result in the folder's settings file as `"TopazFilter"`. The script adds the size for each film.

### Speed

On a 5090, Topaz Proteus at 4K runs about 10–12 fps, about as fast as SVT-AV1 at preset 5. Expect an encode to take a little over twice the film's running time (about 3.5–4 hours for a 100-minute film), with no intermediate file.

## Test clips and the grain comparison

```powershell
# 720 frames from the middle of one episode, video only
upscale.ps1 -Path '.\Episode 05.mkv' -Start -1 -Frames 720

# Same, with a different model or setting, for comparison
upscale.ps1 -Path '.\Episode 05.mkv' -Start -1 -Frames 720 -Noise 3

# Film grain on vs. off (first episode in the folder)
upscale.ps1 -CompareGrain
```

Test clip names include the model and start frame, so several versions sit side by side. Compare the images in `upscaled\compare`.

Your film-grain settings were tuned for real Blu-ray sources, and the upscaler's output is much cleaner. Synthetic grain may add texture that wasn't in the source, so `-CompareGrain` is worth one run per show style. If no-grain looks better, set `SvtParams` without the grain options in that show's settings file.

## Using a different upscaling model

Drop an `.onnx` model (for example from [OpenModelDB](https://openmodeldb.info/), such as AnimeJaNai V3) next to the script, and it's used automatically for every run. With several models present, the newest file wins. To pin a model to one show, set `"Onnx"` in that show's settings file.

Notes on ONNX models:

- Any scale factor works; passes repeat until the frame reaches 2160p.
- They have no denoise setting, so the automatic denoise only applies to Real-CUGAN.

## Troubleshooting

| Symptom | What to check |
|---|---|
| "requires PowerShell 7.4" | Run with `pwsh`, after `winget install Microsoft.PowerShell` |
| Tool still not found after install | Open a new PowerShell window so it picks up the updated `PATH`, then re-run |
| "VapourSynth R79 or one of its plugins is not working" | Run `vspipe --version`. VapourSynth installed via pip needs `vapoursynth config` once; the script runs it automatically, but a manual run shows any error |
| "vs-mlrt ... failed its self-test" | The message includes the real error. "uses API 3, which is no longer supported" means VapourSynth is R80 or newer; run `-UpdateTools` to switch back to R79 |
| "upscale script failed to load" | The episode log contains the VapourSynth error, usually a missing plugin or a vs-mlrt problem. `-UpdateTools` often fixes it |
| "frame count mismatch" | The upscaler crashed partway (often GPU memory or a driver reset). The video file is kept; re-run to retry |
| First episode very slow to start | TensorRT builds an engine for each new input resolution on first use (several minutes). It's cached afterwards |
| Analysis failed | See `logs\analysis\<episode>.stderr.log`. The file is still processed, just without automatic native/denoise decisions |
| Wrong native resolution or denoise | Check the reasons in the episode log, then set `NativeHeight` / `Noise` in the show's settings file. Delete `logs\analysis` to force re-analysis |
| HandBrake rejects an SVT setting | HandBrake bundles its own, possibly older, SVT-AV1. Use a settings file with a reduced `SvtParams` for 4K sources |
| "Topaz Video AI's ffmpeg.exe was not found" | Topaz isn't in the default folder. Set `"TopazPath"` in the CG folder's settings file |
| "Topaz Video AI failed its self-test" | Open Topaz Video AI, make sure you're signed in, and export a few seconds with the A Prores preset so the model downloads. The batch log has Topaz's full output |
| CG film: "frame count mismatch" | Topaz stopped partway (GPU memory or a driver reset). The Topaz messages are in the episode log; re-run to retry |
| Batch stopped for disk space | Free space on the output drive, or point `-OutDir` at another drive, and re-run |

## Limitations

- **Thresholds come from synthetic test frames.** The native-resolution and noise thresholds were tuned on synthetic upscaled, grained and H.264-compressed test video, not a library of real discs. Check the reasons in the logs on your first few shows. The noise thresholds (0.8 / 1.6) are set in both `analyze.py` and `upscale.ps1`.
- **Model choice** isn't automatic: no measurement predicts which model you'll prefer. Use test clips.
- **Temporal stabilization and debanding** are off by default, because neither can be judged from the source before upscaling.
- **Dolby Vision in AV1 has limited player support.** Players without it use the HDR10 layer (except profile 5). A device that can't decode AV1 at all makes Jellyfin transcode, and the Dolby Vision layer usually doesn't survive that.
- **HDR10+ is carried on the HandBrake route only.** On the upscale route, all output is SDR BT.709.
- **Interruptions:** an interrupted episode restarts from the beginning. Finished episodes are never redone.
