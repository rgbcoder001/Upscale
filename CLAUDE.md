# CLAUDE.md

A desktop app (Tauri 2 + React/TS + Rust) that batch-upscales anime and CG animation to 4K and encodes to AV1. It replaces `reference/upscale.ps1`. **Read `PLAN.md` first.** This file holds the project conventions and the domain rules the reference script learned on real discs.

## Repo map

- `PLAN.md`: phases, architecture, UI, and what counts as done.
- `reference/`: the working PowerShell version. **Read-only.** It's the specification for behaviour: when this file and the script disagree, the script wins, unless the difference is listed in "Intentional differences" below.
  - `upscale.ps1`: batch logic (tools, analysis, decisions, pipelines, mux).
  - `upscale.vpy`: VapourSynth script (modes `upscale`, `prep`, `passthrough`, `selftest`). Ship it unchanged in `resources/scripts/`.
  - `analyze.py`: native-resolution and noise analysis (prints JSON). Ship it unchanged.
  - `README.md`: user-facing docs for the script; useful for UI wording.
- `core/` (Rust lib + `upscale-cli` binary), `app/` (Tauri + React): created in Phase 0.

## Conventions

- Rust: stable toolchain, `cargo fmt`, `clippy -D warnings`. Errors via `thiserror` in `core` and `anyhow` at the edges. Async process work on `tokio`.
- `core` has **no Tauri dependency**. The UI talks to it only through commands and events, and the CLI uses the same API.
- Progress/events are typed structs serialized as JSON lines (the CLI prints them; Tauri forwards them).
- Every external command is built by a pure function (`fn svt_args(plan) -> Vec<OsString>` etc.) with snapshot tests. Execution is separate from argument building.
- Never use shell strings. Pass argument vectors (paths contain spaces, apostrophes and brackets, e.g. `Tokyo Godfathers The Ultimate Enthusiast's Collected Edition`).
- Child processes: hidden console window on Windows (`CREATE_NO_WINDOW`), Below-normal priority, all in one Job Object per job (kill-on-close). On Unix: nice 10, each child its own process group, killed with `killpg` (`platform.rs`).
- macOS is the second platform (`MACOS.md`): Topaz and Anime AV1 routes with Homebrew tools. Keep platform differences in `platform.rs` and `tools/` and prefer `cfg!(…)` runtime checks to `#[cfg]` blocks, so Windows builds still type-check the macOS branches. Check with `cargo clippy -p upscale-core --all-targets --target aarch64-apple-darwin`.
- Format numbers with `.` as the decimal separator regardless of locale; the script uses invariant culture everywhere.
- Paths in logs and UI: show full paths. Never assume `C:\`.
- Tests: fixtures in `core/tests/fixtures/` (captured ffprobe / mkvmerge JSON). No test may need a GPU, Topaz or network; those go in an ignored `#[ignore]` "machine" test group the owner runs locally.

## Hard-won rules (do not "simplify" these)

### Tools
- **VapourSynth is pinned to R79** (`vapoursynth==79`) and **`vapoursynth-bestsource<22`**. vs-mlrt's plugins use API 3, which R80 removed (vs-mlrt issue #171). If a newer VapourSynth is installed, downgrade it. After a pip install, run `vapoursynth config` once.
- Required VS plugins: `bs`, `vivtc`, `descale`, `mv`, **`akarin`** (vs-mlrt's internal `_expr` needs `core.akarin.Expr`). Pip packages: `vapoursynth-vivtc`, `-descale`, `-mvtools`, `-akarin`.
- vs-mlrt: the latest GitHub release, the **CUDA package** for NVIDIA (`vk` otherwise), extracted to `tools/vs-mlrt`. `upscale.vpy` loads the DLLs itself from `mlrt_root` (recursive walk) and reports every load error. Don't rely on VapourSynth autoload.
- Backend: `trt` (TensorRT) on NVIDIA, `ncnn` otherwise. On NVIDIA the first use of a new input resolution builds a TensorRT engine (minutes); it's cached afterwards.
- **Self-test before a batch:** `vspipe --info -a mode=selftest -a backend=… -a mlrt=… -a mlrt_root=… upscale.vpy -`. A non-zero exit means stop the batch and show the error line.
- SVT-AV1: use the newest version among SvtAv1EncApp, BtbN FFmpeg (`ffmpeg-n*-latest-win64-gpl-*.zip`) and winget Gyan.FFmpeg. On a tie, prefer SvtAv1EncApp, then BtbN, then winget. Read the version from `SvtAv1EncApp --version` or ffmpeg's libsvtav1 banner.
- GPU: prefer NVIDIA. Integrated graphics (e.g. "AMD Radeon(TM) Graphics" on Ryzen) are listed as "not used", never shown first.

### Encoding
- Defaults: **CRF 16, preset 5** (CRF 14 until 2026-11-01), 10-bit, and SVT params
  `enable-qm=1:qm-min=0:chroma-qm-min=4:enable-variance-boost=1:variance-boost-strength=3:adaptive-film-grain=1:film-grain=2`
  (the same for anime and CG). `variance-boost-strength` does nothing without `enable-variance-boost=1`.
- SvtAv1EncApp form: `-i stdin --crf N --preset N --input-depth 10 --color-primaries P --transfer-characteristics T --matrix-coefficients M --<k> <v>… -b out.ivf`. The ffmpeg form: `-f yuv4mpegpipe -i - -c:v libsvtav1 -crf N -preset N -pix_fmt yuv420p10le -color_primaries … -svtav1-params <params> -f ivf out.ivf`.
- Upscaled output is always **BT.709** (SDR). Colour tags go on both the encoder and the mkvmerge video track (`--colour-primaries 0:1` etc.).
- **Frame verification:** count packets in the `.ivf` (`ffprobe -count_packets`). The upscale routes must match **exactly** (expected = `vspipe --info` Frames). HandBrake gets a tolerance of max(2, 0.1%). On a mismatch, fail the job and keep the video file.

### 4K sources (encode-only route)
- Source height ≥ 2160: if it's AV1, **skip**; otherwise use **HandBrakeCLI**: `-e svt_av1_10bit -q CRF --encoder-preset P -x <SvtParams> --hdr-dynamic-metadata all --crop-mode none --no-comb-detect --no-decomb --no-deinterlace --no-detelecine -a none -s none --no-markers -f av_mkv`.
- Dolby Vision profiles 5, 7, 8.1 and 8.4 become AV1 profile 10. Warn for profile 5 (no HDR10 fallback) and profile 7 (the enhancement layer is dropped and the RPU converted to 8.1). Mux only HandBrake's video track (`--no-audio --no-subtitles --no-chapters --no-attachments --no-global-tags`), because HandBrake already wrote HDR/DV info.

### Analysis (anime only; off for CG)
- **Crop:** ffmpeg `cropdetect=limit=0.094:round=2:reset=0`, 24 frames at each of 20/35/50/65/80% of the duration. Need ≥3 results. Take the **least aggressive** (minimum) value per side, round down to even, and treat < 4 px as 0. Reject if more than 40% of the width or height would be removed.
- **Interlace/telecine:** trust flags if interlaced. Otherwise, only for **29.97 or 25 fps**, run `idet` on 200 frames at 30% and 60%. Interlaced if (TFF+BFF)/total > 0.3 (total ≥ 50); field order = BFF if BFF > TFF. IVTC mode: `decimate` for NTSC (VFM + VDecimate), `match` for PAL. Needs an 8-bit source.
- **Output width** = round(cropped_w × SAR × 2160 / cropped_h / 2) × 2.
- **Native resolution** (descale), only for progressive 1080p sources (height 1000–1100, cropped height ≥ 700, no IVTC):
  - `analyze.py` reports a local-dip **prominence** (min 0.06) and frame agreement (≥ 60%).
  - Use it for a single episode only if prominence ≥ **0.12**.
  - Weaker matches are used only when ≥ 60% of a season (≥ 3 episodes) agree on the same height and kernel.
  - An episode with its own strong match that differs from the season keeps its own result, with a warning.
- **Denoise** (Real-CUGAN only): measured noise in flat areas, < 0.8 → none (-1), < 1.6 → light (0), else strong (3). With ≥ 3 episodes, the **season median** decides one level for all.
- Cache analysis per file, keyed on size + mtime + analysis version + which analyses ran + IVTC mode + crop.

### Tracks
- Keep audio and subtitles whose language is in `keepLanguages` (default `eng`, `jpn`, `und`). Normalise language codes to ISO 639-2 (`en`→`eng`, `ja`→`jpn`, IETF tags too). If **no** audio matches, keep all audio and warn (never produce a silent file).
- **Rename every kept audio track** to `Language - Format Channels` with full language names, e.g. `Japanese - TrueHD 5.1`, `English - DTS-HD MA 5.1`, `English - DD+ 5.1`, `Japanese - DD 2.0`. Formats: TrueHD Atmos, TrueHD, DTS-HD MA, DTS-HD HRA, DTS-ES, DTS, DD+ (E-AC-3), DD (AC-3), AAC, FLAC, PCM, Opus, MP3, MP2, Vorbis. Append ` (Commentary)` when the original title contains "comment". Channel labels: 1→1.0, 2→2.0, 3→2.1, 4→4.0, 5→5.0, 6→5.1, 7→6.1, 8→7.1, else `Nch`.
- **EAC3 companions** for TrueHD, DTS (any profile), FLAC and PCM in kept languages: `-c:a eac3`, 960k for ≥ 6 channels, 320k otherwise, downmixed to 6 channels when there are more than 6. They're named like the others (`English - DD+ 5.1`). Originals stay.
- **Default audio:** `auto` (the anime profiles' default) picks English when its best non-commentary track has more channels than the best Japanese one (5.1 vs 2.0), otherwise Japanese. Then the first kept track in that language is default when it is TrueHD (TrueHD stays default); otherwise its EAC3 companion if one was made (DTS, FLAC, PCM), else the track itself. Set all other kept audio to default=no. If there's no track in that language, leave the source flags alone and warn.
- **Default subtitles:** `auto` (the anime profiles' default) looks at the kept English tracks that aren't forced or dubtitles. When the smallest holds under half the data of the largest (mkvmerge's statistics tags), the largest is the full subtitles and the smallest is Signs and Songs; with exactly two such tracks they're renamed "English Subtitles" and "Signs and Songs". Without sizes, or with tracks of similar size, the names decide (`sign|song`). Japanese audio gets the full track as default, English audio gets Signs and Songs; when that track doesn't exist, no subtitles are on by default. A language code picks the first kept track in that language that isn't forced, dubtitles or `sign|song`, else the first in that language that isn't dubtitles.
- **Dubtitles:** a subtitle track whose name contains "dubtitle" is kept but never made default. `none` = set every non-forced kept subtitle to default=no and leave forced tracks' flags untouched.
- Mux: video `.ivf` (+ colour options), then the source with `--no-video --audio-tracks … --subtitle-tracks … --track-name id:Name …` (chapters and attachments kept), then each EAC3 file with `--language 0:xxx --track-name 0:Name --default-track-flag 0:yes|no`. Write `<name>.partial.mkv` and rename on success. mkvmerge exit 1 = warnings (OK); ≥ 2 = failure.
- Test clips (`frames > 0`) are video only.

### Topaz (CG route)
- Only **Topaz Video AI ≤ 7.x** (and Topaz Video 1.0) ship the CLI ffmpeg with `tvai_up`. Topaz Video 1.1+ removed it. The owner has **7.1.5** (perpetual licence) at `C:\Program Files\Topaz Labs LLC\Topaz Video AI\ffmpeg.exe`.
- Set `TVAI_MODEL_DIR` and `TVAI_MODEL_DATA_DIR` (default `C:\ProgramData\Topaz Labs LLC\Topaz Video AI\models`) if they aren't set. `auth.tpz` in that folder means the user is signed in.
- Chain: `vspipe -c y4m upscale.vpy mode=prep …` (decode, IVTC, crop, trim, convert to 10-bit BT.709) → Topaz ffmpeg
  `-hide_banner -nostdin -nostats -v warning -f yuv4mpegpipe -i - -sws_flags spline+accurate_rnd+full_chroma_int -vf "setparams=range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709,tvai_up=<filter>:w=W:h=H,scale=w=W:h=H:flags=lanczos,setsar=1,format=yuv420p10le" -f yuv4mpegpipe -strict -1 -`
  → encoder. y4m carries no colour information, hence `setparams`.
- Default filter (the owner's "A Prores" preset, taken from his Topaz logs):
  `model=prob-4:scale=0:preblur=0.25:noise=0.75:details=0.75:halo=0.15:blur=0.5:compression=0.5:estimate=8:blend=0.75:device=0:vram=1:instances=1`
- Self-test: a half-second `testsrc2` through the same chain to **stdout**. Check that the output starts with `YUV4MPEG2`, so no Topaz log text is mixed into the video stream.
- Measured speed on the test PC: about 10–12 fps at 4K, close to SVT-AV1 preset 5. Never write a ProRes intermediate.

### Settings files
- `upscale-settings.json` in the input folder **or any parent folder**. The nearest wins; values merge from the farthest down. Keys are listed in `reference/README.md`. `"Content": "cg"` selects the CG profile. Keep supporting these, so the app and the old folder layout both work.
- Precedence: job options > nearest settings file > farther settings files > profile defaults > analysis > built-in.

### Disk and output
- Space per job: upscale = max(2 × source, 4 GB) × 2 + 2 GB; HandBrake = source × 2 + 2 GB. Check before each job and stop the batch when it's short.
- Comparison screenshots: N evenly spaced frames, source (cropped and scaled to the output size with lanczos) on the left and the final file on the right, `hstack`, to `compare/<name>_NN.png`. Upscale routes only.

## Things that bit us before (avoid repeating)

- The PowerShell-specific traps (case-insensitive `$t`/`$T`, byte pipes) don't apply in Rust. But piping **binary** video between processes must never go through any text/line decoding.
- `SystemExit` inside a `.vpy` counts as a failure for vspipe. That's why selftest mode outputs a BlankClip instead of exiting.
- Telecined DVDs are often flagged "progressive". Always check the picture with idet.
- On noisy sources the native-resolution dip is hidden in the absolute error. That's why `analyze.py` uses local prominence. Don't swap in a "simpler" absolute-error test.
- Isolated weak native matches on real discs were false positives (different heights per episode). Hence the 0.12 rule and season agreement.

## Intentional differences from the script

(Record any behaviour change here with the reason.)

- The output folder is a single app-wide setting (the script defaulted to `<rips folder>\upscaled`). `upscale-cli` keeps the script's default unless `--out` is given.
- `KeepLanguages` given as a string (`"eng,und"`) is split on commas. The script turned it into one language named `eng,und`, which matched nothing.
- A lone `.onnx` model is picked up from the input folder. The script looked next to itself, and the app has no script folder.
- **Bug fix:** the Topaz chain is `…,tvai_up=<TopazFilter>:w=W:h=H,…`. The script's `Get-TopazChain` left out `tvai_up=`, so ffmpeg rejected the filter graph ("Error parsing filterchain") and both its Topaz self-test and the CG route failed. A `TopazFilter` that already starts with `tvai_up=` is accepted. Verified with Topaz Video AI 7.1.5 on 2026-10-28.
- `upscale-cli tools` without `--update` only inspects and runs the self-tests; it installs nothing. `--update` is the script's `-UpdateTools`, and a run installs missing tools as the script does.
- **Resumable encoding** (`ResumableEncoding`, **on by default** since 2026-10-28; set `"ResumableEncoding": false` to opt out). A/B on Akito 1, 6.5 min from the middle, chunked vs unchunked: size −0.02 %, SSIM 0.99897, VMAF 97.70 (the two encodes compared with each other, not with the source). Encodes too short for two chunks (≤ 5.5 min, e.g. test clips) are not chunked. The upscale route encodes chunks of about 4 minutes cut at scene changes (found by ffmpeg `scdet` on the script's `mode=prep` output, so frame numbers are post-IVTC), each with an exact frame check, then joins them with `mkvmerge a.ivf + b.ivf …`. The joined `.video.mkv` gets the usual full frame check. Chunks live in `<temp>\<name>.work`; a crash keeps them, cancel deletes them. `upscale-cli ab` compares chunked and unchunked on size, VMAF and SSIM.
- A job restarted after an interruption appends to its episode log, with a `=== Restarted after an interruption ===` line. The script always starts a fresh log.
- EAC3 temp files go to the app's temp folder, not `%TEMP%`.
- **Local temp folder** (2026-11-02, preparation for a second machine, see `MACOS.md`): with Settings → Temp folder empty, chunks and temporary audio go to `%LOCALAPPDATA%\com.upscale.app\work` (`<app data>/work` on other OSes) instead of `<output>\.work`, so two machines writing to one shared output folder never share chunk files. A job that already has a `<output>\.work\<name>.work` folder (interrupted under the old default) keeps using it and resumes. When the temp folder isn't `<output>\.work`, the disk check also wants half the job's estimate free on the temp drive. `upscale-cli run` keeps `<out>\.work` unless `--temp DIR` is given.
- **Claim files** (2026-11-02, `claim.rs`): a job holds `<output>\.claims\<name>.claim` while it runs, so two machines sharing an output folder never encode the same episode. Another machine's fresh claim (refreshed every 2 min) makes the job skip with "being encoded on HOST"; a claim older than 30 min, or one from this machine, is taken over. When the app's queue runs out of work it rechecks those skipped episodes every minute (`Engine::recheck_claimed`): finished elsewhere → "encoded on HOST" with its size; claim released or stale → queued again here; still claimed → it waits ("Waiting for another computer …") until Stop. The script had no such thing.
- **Keep awake** (2026-11-02): while a batch runs, the app and CLI stop the computer from sleeping (`platform::KeepAwake`: `SetThreadExecutionState(ES_SYSTEM_REQUIRED)` on Windows, `caffeinate -i` on macOS). The display can still turn off.
- **vs-mlrt only when a job needs it** (2026-11-02): a batch installs and self-tests vs-mlrt only when a queued job uses the vs-mlrt route (`Settings::uses_vs_mlrt`). The script always did it; Topaz and Anime AV1 batches don't need its ~4 GB. In the app, the Topaz folder (and the self-test's filter) now comes from the first queued Topaz job's settings, for batches as well as Settings → Tools and Update tools, so `TopazPath` / `TopazFilter` in `upscale-settings.json` apply there as they do in the CLI; with no Topaz job queued, the default install and filter are used. Update tools still installs vs-mlrt (on Windows; it has no other builds).
- **Private tools folder** (Phase 4): every tool comes from `<tools>` (Settings → Tools folder, default `%APPDATA%\com.upscale.app\tools`) and is downloaded there, checked against SHA-256 (GitHub asset digests, MKVToolNix `.sha256` files, pip `--require-hashes` from PyPI's hashes). Python is python-build-standalone 3.12 instead of a system Python, MKVToolNix and HandBrakeCLI are the portable builds, and `7zr.exe` replaces the winget 7-Zip. The script's winget/pip-into-global-Python behaviour remains behind `UseSystemTools` (Settings) / `--system-tools` (CLI). Planning installs the probe tools (FFmpeg, MKVToolNix, Python + VapourSynth) when they're missing; vs-mlrt waits for the batch.
- SvtAv1EncApp is not downloaded (SVT-AV1's releases ship no Windows binaries). The BtbN FFmpeg's libsvtav1 is used unless a newer `SvtAv1EncApp.exe` is put in `<tools>\svt-av1`.
- **Anime (Topaz) profile** (`"Content": "anime-topaz"`, added 2026-11-01) is the **default** profile. It's the anime profile (crop, IVTC, `jpn` audio / `eng` subtitles) upscaled through Topaz with the same `TopazFilter` as CG; denoise and native-resolution analysis don't run, as on any Topaz route. `"Content": "anime"` keeps the vs-mlrt route. The default **CRF is 16** (the script used 14) to get smaller files.
- **Anime AV1 profile** (`"Content": "anime-av1"`, 2026-11-01): encode only, no upscale. Same anime defaults (crop, IVTC, `jpn`/`eng`, CRF 16, preset 5, default SVT params), with `upscale.vpy` in `mode=prep` straight into the encoder at the cropped source size. Non-square pixels (anamorphic DVDs) stay and get `--display-dimensions` at the mux. Sources at/above the target height still take the HandBrake/skip route. `"Upscaler": "none"` gives any profile this encode-only route.
- **Live action profile** (`"Content": "live"`, 2026-11-01, the owner's settings): HandBrake at **every** source size (AV1 sources are skipped), **CRF 14**, preset 5, `--encoder-tune ssim` (new `Tune` setting, HandBrake route only) and `LIVE_SVT_PARAMS` = `enable-qm=1:qm-min=2:chroma-qm-min=8:qp-scale-compress-strength=1:variance-boost-strength=2:adaptive-film-grain=1:film-grain=4`. English audio, subtitles none, no analysis. HandBrake runs with the existing flags (no crop, deinterlace or detelecine). Live upscaling (PLAN.md Phase 5) is not built; `"Upscaler": "topaz"` on live sends it through the Topaz route instead.
- **Preset stays 5.** Preset 6 was tried on 2026-11-01 and reverted: on the Topaz route the chain is CPU-bound on the test PC's 8-core CPU (libsvtav1 about 62% of the CPU, Topaz's ffmpeg about 24% for its CPU-side colour conversion, GPU about 11%). The same 5753-frame Black Clover chunk took 4.27 fps / 190 MB at preset 5 and 4.50 fps / 198 MB at preset 6: Topaz's ffmpeg took the freed CPU, so the gain was 5%.
- **Episodes start as soon as they're planned** (2026-11-01) when no season consensus is needed (`batch::plans_independently`: any Topaz route, or `Analysis` not `auto`). The queue then plans a folder one episode at a time and queues each when its plan is ready; the vs-mlrt route with automatic analysis still plans the whole folder first. Jobs still run one at a time. At Start, tools are chosen from each group's settings even before its first plan exists.
- **Delete source** (`DeleteSource`, off by default, 2026-11-02; Add sheet checkbox, `--set DeleteSource=true` in the CLI): after a job finishes (frame check, mux, comparison images), the source is moved to the Recycle Bin (`platform::move_to_trash`: `SHFileOperationW` with `FOF_ALLOWUNDO`; Finder via `osascript` on macOS) if the finished file's duration matches the source's within max(1 s, 0.1 %). A test clip, a duration mismatch or a failed delete keeps the source with a warning; the job still counts as done. On drives without a Recycle Bin (network shares) Windows deletes for good. The script never deleted sources. `ReplaceSource` (Add sheet "Original file: Replace with the new file") does the same, but first moves the finished file next to the source as `<stem>.upscale.partial.mkv` (rename, or copy + delete across drives) and renames it to `<stem>.mkv` once the source is gone, so the new file takes the source's place. An existing `<stem>.mkv` that isn't the source keeps the source.
- `-CompareGrain` and `-Tag` are not ported. A test clip (`--test-clip`) plus `--set SvtParams=…` covers the same comparison.

## Owner preferences

- Values the owner has stated: quality over speed, slightly larger files are fine, full automation by default, simple UI.
- Live-action encode settings were given on 2026-11-01 (see the Live action profile above). Live-action **upscaling** settings are not decided yet. Ask; don't invent them (PLAN.md Phase 5).
