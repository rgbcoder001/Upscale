//! Running a batch: for each planned file the script's steps (lines 1300–1649), with the pump
//! for progress and pause, a process group per job for cancel, and events for the UI.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use regex::Regex;
use tokio::io::AsyncReadExt;
use tokio::sync::{mpsc, watch};

use crate::batch::Batch;
use crate::chunks::{self, ChunkState, WorkDir};
use crate::claim::{BUSY_PREFIX, Claim, host_name};
use crate::cmd::{self, NewTrack};
use crate::color::video_color;
use crate::disk::{GB, free_space, space_needed};
use crate::event::{Event, Stage};
use crate::log::Log;
use crate::plan::{FilePlan, Route};
use crate::platform::{ProcessGroup, command, resume, suspend};
use crate::process::Runner;
use crate::pump::{Rate, pump};
use crate::report::noise_label;
use crate::tools::install::Installer;
use crate::tools::set::ToolSet;
use crate::tools::svt::EncoderKind;
use crate::tools::vapoursynth::{info_fps, info_frames};
use crate::tracks::default_tracks;
use crate::util::{fmt_n, parse_rational};

pub type Events = mpsc::UnboundedSender<Event>;

/// Pause, cancel and "stop after the current file", shared with whoever drives the batch.
pub struct Control {
    paused: watch::Sender<bool>,
    cancelled: watch::Sender<bool>,
    stop_after: AtomicBool,
}

impl Control {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            paused: watch::Sender::new(false),
            cancelled: watch::Sender::new(false),
            stop_after: AtomicBool::new(false),
        })
    }
    /// Pauses the video stage by not reading frames (GPU memory stays allocated).
    pub fn pause(&self) {
        self.paused.send_replace(true);
    }
    pub fn resume(&self) {
        self.paused.send_replace(false);
    }
    pub fn is_paused(&self) -> bool {
        *self.paused.borrow()
    }
    /// Kills the running job; its partial outputs are deleted.
    pub fn cancel(&self) {
        self.resume();
        self.cancelled.send_replace(true);
    }
    pub fn is_cancelled(&self) -> bool {
        *self.cancelled.borrow()
    }
    pub fn stop_after_current(&self, on: bool) {
        self.stop_after.store(on, Ordering::Relaxed);
    }
    pub fn stopping(&self) -> bool {
        self.stop_after.load(Ordering::Relaxed)
    }
    pub(crate) async fn wait_cancelled(&self) {
        let mut rx = self.cancelled.subscribe();
        let _ = rx.wait_for(|c| *c).await;
    }
}

/// What a batch run needs.
pub struct RunEnv<'a> {
    pub batch: &'a Batch,
    pub tools: ToolSet,
    pub vpy: PathBuf,
    pub runner: &'a dyn Runner,
    /// Installs HandBrakeCLI on the first 4K file.
    pub installer: &'a Installer<'a>,
    /// The batch log.
    pub log: &'a dyn Log,
    pub events: Events,
    pub control: Arc<Control>,
    pub keep_ivf: bool,
    /// Chunk work folders and EAC3 temp files.
    pub temp_dir: PathBuf,
    /// The job was interrupted before: append to its log instead of starting a new one.
    pub restart: bool,
}

/// One line of the summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobResult {
    pub file: String,
    pub result: String,
    pub time: String,
}

enum Outcome {
    Done(PathBuf),
    Skipped(String),
    Failed(String),
    /// Not enough disk space: the batch stops.
    StopBatch(String),
    Cancelled,
}

#[derive(Debug)]
enum JobError {
    Cancelled,
    Io(std::io::Error),
}

impl From<std::io::Error> for JobError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// The job's log file plus the batch log (the script's `Write-Log` writes both).
struct EpisodeLog<'a> {
    batch: &'a dyn Log,
    file: Mutex<File>,
    path: PathBuf,
    job_id: String,
    events: Events,
}

impl EpisodeLog<'_> {
    /// An append handle for a child process's output.
    fn stdio(&self) -> std::io::Result<Stdio> {
        Ok(OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.path)?
            .into())
    }
    fn raw(&self, text: &str) {
        let _ = writeln!(self.file.lock().expect("log lock"), "{text}");
    }
}

impl Log for EpisodeLog<'_> {
    fn info(&self, msg: &str) {
        self.batch.info(msg);
        let _ = writeln!(
            self.file.lock().expect("log lock"),
            "[{}] {msg}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        );
        let _ = self.events.send(Event::Log {
            job_id: self.job_id.clone(),
            line: msg.to_string(),
        });
    }
}

fn hms(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

static NON_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\W").expect("valid regex"));
static NOT_SAFE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\w\-]").expect("valid regex"));
static HB_PROGRESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Encoding: task \d+ of \d+, ([\d.]+) %(?: \(([\d.]+) fps)?").expect("valid regex")
});

/// Resumable (chunked) encoding for this file: on, the upscale route, and long enough for
/// two chunks. Frames are estimated from the source (4 in 5 after decimation).
fn uses_chunks(s: &crate::settings::Settings, p: &FilePlan) -> bool {
    let fps = if p.ivtc_mode == "decimate" {
        p.src.frame_rate * 0.8
    } else {
        p.src.frame_rate
    };
    let frames = if s.frames > 0 {
        s.frames
    } else {
        (p.src.duration * fps) as u64
    };
    // HandBrake pieces cut at source keyframes join frame-exactly (checked on a VC-1 Blu-ray);
    // Dolby Vision stays in one piece until joining its RPUs has been tested on a DV disc
    let route =
        p.route == Route::Upscale || (p.route == Route::Handbrake && p.src.dv_profile.is_none());
    route && s.resumable_encoding && chunks::worth_chunking(frames, fps.max(1.0))
}

/// Output name: the source name, plus `_test_<model>_<start>` for a test clip.
/// Where a job's finished file goes: its `OutputDir`, else the output folder.
pub fn final_dir(b: &Batch, out_dir: &Path) -> PathBuf {
    b.resolved
        .settings
        .output_dir
        .clone()
        .unwrap_or_else(|| out_dir.to_path_buf())
}

pub fn output_base(b: &Batch, p: &FilePlan) -> String {
    let mut base = p.file.stem();
    if b.resolved.settings.frames > 0 {
        let model = Path::new(&b.model_label)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        base.push_str(&format!(
            "_test_{}_{}",
            NON_WORD.replace_all(&model, "_"),
            p.start
        ));
    }
    base
}

/// Runs a command to completion with stdout and stderr appended to the episode log.
async fn run_logged(
    exe: &Path,
    args: &[OsString],
    ep: &EpisodeLog<'_>,
    control: &Control,
) -> Result<Option<i32>, JobError> {
    let group = ProcessGroup::new()?;
    let mut child = command(exe, args)
        .stdin(Stdio::null())
        .stdout(ep.stdio()?)
        .stderr(ep.stdio()?)
        .spawn()?;
    group.add(&child)?;
    tokio::select! {
        status = child.wait() => Ok(status?.code()),
        _ = control.wait_cancelled() => { group.kill(); Err(JobError::Cancelled) }
    }
}

/// Runs a command and captures stdout and stderr.
async fn run_capture(
    exe: &Path,
    args: &[OsString],
    control: &Control,
) -> Result<(Option<i32>, String, String), JobError> {
    let group = ProcessGroup::new()?;
    let child = command(exe, args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    group.add(&child)?;
    tokio::select! {
        out = child.wait_with_output() => {
            let out = out?;
            Ok((out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned()))
        }
        _ = control.wait_cancelled() => { group.kill(); Err(JobError::Cancelled) }
    }
}

/// Renames `from` to `to`, or copies and deletes it when they're on different drives.
async fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    let (f, t) = (from.to_path_buf(), to.to_path_buf());
    tokio::task::spawn_blocking(move || std::fs::copy(&f, &t))
        .await
        .map_err(std::io::Error::other)??;
    std::fs::remove_file(from)
}

/// Encoded packets in the first video stream.
async fn count_frames(ffprobe: &Path, file: &Path, control: &Control) -> Result<u64, JobError> {
    let (_, out, _) = run_capture(ffprobe, &cmd::count_packets_args(file), control).await?;
    Ok(out
        .lines()
        .next()
        .and_then(|l| l.trim().parse().ok())
        .unwrap_or(0))
}

/// Where one encode sits in the whole file (a chunk, or everything).
#[derive(Debug, Clone, Copy)]
struct Span {
    total: u64,
    /// Frames finished before this encode (earlier chunks).
    offset: u64,
    /// Bytes of the earlier chunks.
    size_offset: u64,
    chunk: Option<(u32, u32)>,
}

impl Span {
    fn whole(total: u64) -> Self {
        Self {
            total,
            offset: 0,
            size_offset: 0,
            chunk: None,
        }
    }
}

struct Progress<'a> {
    events: &'a Events,
    control: &'a Control,
    job_id: &'a str,
    span: Span,
    video: &'a Path,
}

impl Progress<'_> {
    /// Emits a progress event every second until `done`.
    async fn tick(&self, frames: &AtomicU64, done: &AtomicBool) {
        let start = Instant::now();
        let mut rate = Rate::default();
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        while !done.load(Ordering::Relaxed) {
            interval.tick().await;
            let local = frames.load(Ordering::Relaxed);
            let fps = rate.add(start.elapsed().as_secs_f64(), local);
            let Span {
                total,
                offset,
                size_offset,
                chunk,
            } = self.span;
            let n = offset + local;
            let size = size_offset + std::fs::metadata(self.video).map(|m| m.len()).unwrap_or(0);
            let _ = self.events.send(Event::Progress {
                job_id: self.job_id.into(),
                frames_done: n,
                frames_total: total,
                fps: (fps * 100.0).round() / 100.0,
                eta_seconds: (fps > 0.0 && total >= n).then(|| ((total - n) as f64 / fps) as u64),
                projected_bytes: (n > 0 && size > 0)
                    .then(|| (size as f64 * total as f64 / n as f64) as u64),
                paused: self.control.is_paused(),
                chunk,
            });
        }
    }
}

impl RunEnv<'_> {
    fn stage(&self, job_id: &str, stage: Stage) {
        let _ = self.events.send(Event::Stage {
            job_id: job_id.into(),
            stage,
        });
    }

    /// Upscale route: vspipe [→ Topaz] → pump → SVT-AV1. Returns the encoder's exit code.
    async fn upscale_video(
        &self,
        p: &FilePlan,
        script_args: &[OsString],
        video_out: &Path,
        span: Span,
        ep: &EpisodeLog<'_>,
    ) -> Result<Option<i32>, JobError> {
        let s = &self.batch.resolved.settings;
        let vc = video_color(&p.src, false);
        let enc = &self.tools.encoder;
        let enc_args = match enc.kind {
            EncoderKind::SvtApp => cmd::svt_app_args(s, &vc, video_out),
            EncoderKind::Ffmpeg => cmd::ffmpeg_svt_args(s, &vc, video_out),
        };
        ep.raw("--- vspipe output ---");
        let group = ProcessGroup::new()?;
        let mut vspipe = command(
            &self.tools.vspipe,
            &cmd::vspipe_y4m_args(script_args, &self.vpy),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(ep.stdio()?)
        .spawn()?;
        group.add(&vspipe)?;
        let vs_out = vspipe.stdout.take().expect("piped");

        let mut topaz_child = None;
        let producer: Box<dyn tokio::io::AsyncRead + Unpin + Send> = if s.use_topaz() {
            let topaz = self
                .tools
                .topaz
                .as_ref()
                .expect("Topaz initialised before the batch");
            let args = cmd::topaz_args(&s.topaz_filter, p.out_w, p.out_h);
            ep.raw(&format!(
                "Topaz: {}",
                args.iter()
                    .map(|a| a.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
            ep.info("  Upscaling with Topaz + encoding ...");
            let stdin: Stdio = vs_out.try_into()?;
            let mut child = command(&topaz.ffmpeg, &args)
                .envs(topaz.env_os())
                .stdin(stdin)
                .stdout(Stdio::piped())
                .stderr(ep.stdio()?)
                .spawn()?;
            group.add(&child)?;
            let out = child.stdout.take().expect("piped");
            topaz_child = Some(child);
            Box::new(out)
        } else {
            ep.info(if p.upscale {
                "  Upscaling + encoding ..."
            } else {
                "  Encoding ..."
            });
            Box::new(vs_out)
        };

        let mut encoder = command(&enc.path, &enc_args)
            .stdin(Stdio::piped())
            .stdout(ep.stdio()?)
            .stderr(ep.stdio()?)
            .spawn()?;
        group.add(&encoder)?;
        let enc_in = encoder.stdin.take().expect("piped");

        let frames = AtomicU64::new(0);
        let done = AtomicBool::new(false);
        let progress = Progress {
            events: &self.events,
            control: &self.control,
            job_id: &p.file.name,
            span,
            video: video_out,
        };
        let work = async {
            let pumped = pump(producer, enc_in, self.control.paused.subscribe(), &frames).await;
            if let Err(e) = &pumped {
                ep.raw(&format!("pump: {e}"));
                // The encoder is gone or the stream is broken: stop the producers too
                group.kill();
            }
            let status = encoder.wait().await;
            let vs_status = vspipe.wait().await;
            if let Some(t) = topaz_child.as_mut() {
                let _ = t.wait().await;
            }
            if let Ok(st) = vs_status
                && !st.success()
            {
                ep.raw(&format!("vspipe exited with {:?}", st.code()));
            }
            done.store(true, Ordering::Relaxed);
            status
        };
        tokio::select! {
            (status, ()) = async { tokio::join!(work, progress.tick(&frames, &done)) } => Ok(status?.code()),
            _ = self.control.wait_cancelled() => { group.kill(); Err(JobError::Cancelled) }
        }
    }

    /// 4K route: HandBrakeCLI, progress from its stdout.
    /// HandBrake into `video_out`: the whole file, a test clip, or `range` (start frame,
    /// frames) of a chunked encode, with progress over `span`.
    async fn handbrake_video(
        &self,
        p: &FilePlan,
        video_out: &Path,
        range: Option<(cmd::HbStart, u64)>,
        span: Span,
        ep: &EpisodeLog<'_>,
    ) -> Result<Option<i32>, JobError> {
        let s = &self.batch.resolved.settings;
        let hb = self
            .tools
            .handbrake
            .as_deref()
            .expect("HandBrakeCLI found before the encode");
        let clip = range.or((s.frames > 0).then_some((cmd::HbStart::Frame(p.start), s.frames)));
        let len = range.map_or(span.total, |(_, n)| n);
        if range.is_none() {
            ep.info("  Encoding with HandBrake ...");
        }
        ep.raw("--- HandBrake log ---");
        let group = ProcessGroup::new()?;
        let mut child = command(hb, &cmd::handbrake_args(&p.file.path, video_out, s, clip))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(ep.stdio()?)
            .spawn()?;
        group.add(&child)?;
        let pid = child.id();
        // HandBrake has no pump: pausing suspends its threads instead
        let mut paused = self.control.paused.subscribe();
        let watch_pause = async {
            let mut suspended = false;
            while paused.changed().await.is_ok() {
                let want = *paused.borrow();
                if let Some(pid) = pid
                    && want != suspended
                {
                    let r = if want { suspend(pid) } else { resume(pid) };
                    if let Err(e) = r {
                        ep.warn(&format!(
                            "Couldn't {} HandBrake: {e}",
                            if want { "pause" } else { "resume" }
                        ));
                    }
                    suspended = want;
                }
            }
            std::future::pending::<()>().await
        };
        let mut out = child.stdout.take().expect("piped");
        let events = &self.events;
        let job_id = &p.file.name;
        let read = async {
            let mut buf = vec![0u8; 4096];
            let mut pending = String::new();
            while let Ok(n) = out.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                pending.push_str(&String::from_utf8_lossy(&buf[..n]));
                let last = pending.rfind(['\r', '\n']).map_or(0, |i| i + 1);
                for line in pending[..last].split(['\r', '\n']) {
                    if let Some(c) = HB_PROGRESS.captures(line) {
                        let pct: f64 = c[1].parse().unwrap_or(0.0);
                        let fps: f64 = c
                            .get(2)
                            .and_then(|m| m.as_str().parse().ok())
                            .unwrap_or(0.0);
                        let done = span.offset + (len as f64 * pct / 100.0) as u64;
                        let _ = events.send(Event::Progress {
                            job_id: job_id.clone(),
                            frames_done: done,
                            frames_total: span.total,
                            fps,
                            eta_seconds: (fps > 0.0)
                                .then(|| (span.total.saturating_sub(done) as f64 / fps) as u64),
                            projected_bytes: None,
                            paused: self.control.is_paused(),
                            chunk: span.chunk,
                        });
                    }
                }
                pending.drain(..last);
            }
            child.wait().await
        };
        tokio::select! {
            status = read => Ok(status?.code()),
            () = watch_pause => unreachable!("never finishes"),
            _ = self.control.wait_cancelled() => { group.kill(); Err(JobError::Cancelled) }
        }
    }

    /// Scene cuts (in encode frame numbers) and the frame rate, from the script's prep output.
    async fn scene_cuts(
        &self,
        p: &FilePlan,
        script_args: &[OsString],
        ep: &EpisodeLog<'_>,
    ) -> Result<(Vec<u64>, f64), JobError> {
        let prep = cmd::with_prep_mode(script_args);
        let (_, out, err) = run_capture(
            &self.tools.vspipe,
            &cmd::vspipe_info_args(&prep, &self.vpy),
            &self.control,
        )
        .await?;
        let fps = info_fps(&format!("{out}{err}"))
            .filter(|f| *f > 0.0)
            .unwrap_or(p.src.frame_rate.max(1.0));
        let group = ProcessGroup::new()?;
        let mut vspipe = command(&self.tools.vspipe, &cmd::vspipe_y4m_args(&prep, &self.vpy))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(ep.stdio()?)
            .spawn()?;
        group.add(&vspipe)?;
        let stdin: Stdio = vspipe.stdout.take().expect("piped").try_into()?;
        let ffmpeg = command(&self.tools.ffmpeg, &cmd::scdet_args())
            .stdin(stdin)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        group.add(&ffmpeg)?;
        let out = tokio::select! {
            out = ffmpeg.wait_with_output() => out?,
            _ = self.control.wait_cancelled() => { group.kill(); return Err(JobError::Cancelled) }
        };
        let _ = vspipe.wait().await;
        Ok((
            chunks::parse_scene_cuts(&String::from_utf8_lossy(&out.stderr), fps),
            fps,
        ))
    }

    /// Chunked HandBrake encode (Live action, 4K encode-only): pieces of about four minutes cut
    /// at the source's keyframes, each `--start-at frame:N --stop-at frame:LEN` into its own MKV
    /// with an exact frame count and a done marker, then joined with mkvmerge into
    /// `video_out`. A restart continues with the first piece that isn't done.
    /// `Ok(Err(reason))` is a failed job.
    async fn chunked_handbrake(
        &self,
        p: &FilePlan,
        expected: u64,
        base: &str,
        video_out: &Path,
        ep: &EpisodeLog<'_>,
    ) -> Result<Result<(), String>, JobError> {
        let s = &self.batch.resolved.settings;
        let hb = self
            .tools
            .handbrake
            .as_deref()
            .expect("HandBrakeCLI found before the encode");
        let args = cmd::handbrake_args(&p.file.path, Path::new("chunk.mkv"), s, None);
        let joined: String = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("\n");
        let key = chunks::fnv1a(&[
            b"handbrake",
            p.file.size.to_string().as_bytes(),
            p.file.mtime_ticks.to_string().as_bytes(),
            joined.as_bytes(),
            hb.to_string_lossy().as_bytes(),
            expected.to_string().as_bytes(),
        ]);
        let work = WorkDir::new(&self.temp_dir, base);
        let fps = p.src.frame_rate.max(1.0);
        let state = match work.load(&key) {
            Some(st) => {
                ep.info(&format!(
                    "  Resuming chunked encode at chunk {} of {}",
                    work.done_count(&st) + 1,
                    st.chunks.len()
                ));
                st
            }
            None => {
                self.stage(&p.file.name, Stage::Scenes);
                ep.info("  Finding the source's keyframes for chunked encoding ...");
                let (_, out, _) = run_capture(
                    &self.tools.ffprobe,
                    &cmd::keyframe_args(&p.file.path),
                    &self.control,
                )
                .await?;
                // Keyframes in the encode's own numbering (a test clip starts part-way in)
                let first = if s.frames > 0 {
                    p.start.max(0) as u64
                } else {
                    0
                };
                let cuts: Vec<u64> = cmd::parse_keyframes(&out, fps)
                    .into_iter()
                    .filter(|k| *k > first && *k < first + expected)
                    .map(|k| k - first)
                    .collect();
                let ranges =
                    chunks::plan_chunks_of(expected, fps, &cuts, chunks::HANDBRAKE_TARGET_SECS);
                let st = ChunkState {
                    key,
                    chunks: ranges.iter().map(|r| (r.start, r.end)).collect(),
                };
                work.reset(&st)?;
                ep.info(&format!(
                    "  {} keyframes; {} chunk(s) of about 10 minutes",
                    cuts.len(),
                    st.chunks.len()
                ));
                st
            }
        };

        self.stage(&p.file.name, Stage::Video);
        ep.info("  Encoding with HandBrake ...");
        let first = if s.frames > 0 { p.start } else { 0 };
        let n = state.chunks.len();
        let (mut offset, mut size_offset) = (0u64, 0u64);
        for (i, &(a, b)) in state.chunks.iter().enumerate() {
            let piece = work.mkv(i);
            let len = b - a;
            if work.is_done(i) {
                offset += len;
                size_offset += std::fs::metadata(&piece).map(|m| m.len()).unwrap_or(0);
                continue;
            }
            let _ = std::fs::remove_file(&piece);
            ep.info(&format!("  Chunk {}/{n}: frames {a}-{}", i + 1, b - 1));
            let span = Span {
                total: expected,
                offset,
                size_offset,
                chunk: Some((i as u32 + 1, n as u32)),
            };
            // Pieces start on keyframes, by timestamp so HandBrake seeks there instead of
            // decoding the film up to them; a test clip's first piece starts where it starts
            let at = (first.max(0) as u64) + a;
            let start = if at == 0 {
                cmd::HbStart::Pts(0)
            } else {
                cmd::HbStart::Pts(cmd::hb_start_pts(at, fps))
            };
            let code = self
                .handbrake_video(p, &piece, Some((start, len)), span, ep)
                .await?;
            if code != Some(0) || !piece.is_file() {
                return Ok(Err(format!(
                    "HandBrake exited with {} in chunk {} (details in the episode log)",
                    code.map_or("no exit code".into(), |c| c.to_string()),
                    i + 1
                )));
            }
            let got = count_frames(&self.tools.ffprobe, &piece, &self.control).await?;
            if got != len {
                return Ok(Err(format!(
                    "frame count mismatch in chunk {}: expected {len}, got {got} (set \"ResumableEncoding\": false to encode in one piece)",
                    i + 1
                )));
            }
            work.mark_done(i)?;
            offset += len;
            size_offset += std::fs::metadata(&piece).map(|m| m.len()).unwrap_or(0);
        }

        self.stage(&p.file.name, Stage::Joining);
        ep.info("  Joining chunks ...");
        let pieces: Vec<PathBuf> = (0..n).map(|i| work.mkv(i)).collect();
        let code = run_logged(
            &self.tools.mkvmerge,
            &cmd::join_args(video_out, &pieces),
            ep,
            &self.control,
        )
        .await?;
        if code.is_none_or(|c| c >= 2) {
            return Ok(Err(format!(
                "joining the chunks failed (mkvmerge exit {})",
                code.map_or("?".into(), |c| c.to_string())
            )));
        }
        Ok(Ok(()))
    }

    /// Chunked upscale: encodes the chunks that aren't done yet, then joins them into
    /// `video_out`. `Ok(Err(reason))` is a failed job.
    async fn chunked_video(
        &self,
        p: &FilePlan,
        script_args: &[OsString],
        expected: u64,
        base: &str,
        video_out: &Path,
        ep: &EpisodeLog<'_>,
    ) -> Result<Result<(), String>, JobError> {
        let s = &self.batch.resolved.settings;
        let enc = &self.tools.encoder;
        let vc = video_color(&p.src, false);
        let enc_args = match enc.kind {
            EncoderKind::SvtApp => cmd::svt_app_args(s, &vc, Path::new("chunk.ivf")),
            EncoderKind::Ffmpeg => cmd::ffmpeg_svt_args(s, &vc, Path::new("chunk.ivf")),
        };
        let joined = |a: &[OsString]| {
            a.iter()
                .map(|x| x.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("\n")
        };
        let key = chunks::fnv1a(&[
            p.file.size.to_string().as_bytes(),
            p.file.mtime_ticks.to_string().as_bytes(),
            joined(script_args).as_bytes(),
            joined(&enc_args).as_bytes(),
            enc.path.to_string_lossy().as_bytes(),
            enc.version.to_string().as_bytes(),
            expected.to_string().as_bytes(),
        ]);
        let work = WorkDir::new(&self.temp_dir, base);
        let state = match work.load(&key) {
            Some(st) => {
                let done = work.done_count(&st);
                ep.info(&format!(
                    "  Resuming chunked encode at chunk {} of {}",
                    done + 1,
                    st.chunks.len()
                ));
                st
            }
            None => {
                self.stage(&p.file.name, Stage::Scenes);
                ep.info("  Finding scene cuts for chunked encoding ...");
                let (cuts, fps) = self.scene_cuts(p, script_args, ep).await?;
                let ranges = chunks::plan_chunks(expected, fps, &cuts);
                let st = ChunkState {
                    key,
                    chunks: ranges.iter().map(|r| (r.start, r.end)).collect(),
                };
                work.reset(&st)?;
                ep.info(&format!(
                    "  {} scene cuts; {} chunk(s) of about 4 minutes",
                    cuts.len(),
                    st.chunks.len()
                ));
                st
            }
        };

        self.stage(&p.file.name, Stage::Video);
        let first = if s.frames > 0 { p.start } else { 0 };
        let n = state.chunks.len();
        let (mut offset, mut size_offset) = (0u64, 0u64);
        for (i, &(a, b)) in state.chunks.iter().enumerate() {
            let ivf = work.ivf(i);
            let len = b - a;
            if work.is_done(i) {
                offset += len;
                size_offset += std::fs::metadata(&ivf).map(|m| m.len()).unwrap_or(0);
                continue;
            }
            let _ = std::fs::remove_file(&ivf);
            ep.info(&format!("  Chunk {}/{n}: frames {a}-{}", i + 1, b - 1));
            let args = cmd::with_range(script_args, first + a as i64, len);
            let span = Span {
                total: expected,
                offset,
                size_offset,
                chunk: Some((i as u32 + 1, n as u32)),
            };
            let code = self.upscale_video(p, &args, &ivf, span, ep).await?;
            if code != Some(0) || !ivf.is_file() {
                return Ok(Err(format!(
                    "encoder exited with {} in chunk {} (details in the episode log)",
                    code.map_or("no exit code".into(), |c| c.to_string()),
                    i + 1
                )));
            }
            let got = count_frames(&self.tools.ffprobe, &ivf, &self.control).await?;
            if got != len {
                return Ok(Err(format!(
                    "frame count mismatch in chunk {}: expected {len}, got {got}",
                    i + 1
                )));
            }
            work.mark_done(i)?;
            offset += len;
            size_offset += std::fs::metadata(&ivf).map(|m| m.len()).unwrap_or(0);
        }

        self.stage(&p.file.name, Stage::Joining);
        ep.info("  Joining chunks ...");
        let ivfs: Vec<PathBuf> = (0..n).map(|i| work.ivf(i)).collect();
        let code = run_logged(
            &self.tools.mkvmerge,
            &cmd::join_args(video_out, &ivfs),
            ep,
            &self.control,
        )
        .await?;
        if code.is_none_or(|c| c >= 2) {
            return Ok(Err(format!(
                "joining the chunks failed (mkvmerge exit {})",
                code.map_or("?".into(), |c| c.to_string())
            )));
        }
        Ok(Ok(()))
    }

    async fn save_comparisons(
        &self,
        p: &FilePlan,
        mkv: &Path,
        base: &str,
        ep: &EpisodeLog<'_>,
    ) -> Result<(), JobError> {
        let s = &self.batch.resolved.settings;
        if s.screenshots == 0 {
            return Ok(());
        }
        let dir = self.batch.out_dir.join("compare");
        std::fs::create_dir_all(&dir)?;
        let probe: Vec<OsString> = [
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=r_frame_rate:format=duration",
            "-of",
            "json",
        ]
        .map(OsString::from)
        .into_iter()
        .chain([mkv.as_os_str().to_owned()])
        .collect();
        let (_, json, _) = run_capture(&self.tools.ffprobe, &probe, &self.control).await?;
        let o: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
        let out_dur: f64 = o
            .pointer("/format/duration")
            .and_then(|d| d.as_str()?.parse().ok())
            .unwrap_or(0.0);
        let out_fps = o
            .pointer("/streams/0/r_frame_rate")
            .and_then(|r| parse_rational(r.as_str()?))
            .unwrap_or(0.0);
        // Where the encoded clip starts in the source (test clips start part-way in)
        let offset = if s.frames > 0 && out_fps > 0.0 {
            p.start as f64 / out_fps
        } else {
            0.0
        };
        let mut saved = 0;
        for i in 1..=s.screenshots {
            let t = out_dur * i as f64 / (s.screenshots + 1) as f64;
            let png = dir.join(format!("{base}_{i:02}.png"));
            let args = cmd::compare_args(
                &p.file.path,
                offset + t,
                mkv,
                t,
                p.crop,
                p.src.width,
                p.src.height,
                p.out_w,
                p.out_h,
                &png,
            );
            if run_logged(&self.tools.ffmpeg, &args, ep, &self.control).await? == Some(0) {
                saved += 1;
            }
        }
        if saved > 0 {
            ep.info(&format!(
                "  Saved {saved} comparison image(s) to {} (left: source scaled, right: final)",
                dir.display()
            ));
        }
        Ok(())
    }

    async fn run_file(
        &mut self,
        p: &FilePlan,
        ep: &EpisodeLog<'_>,
        base: &str,
        partial: &Path,
        mkv: &Path,
        video_out: &Path,
    ) -> Result<Outcome, JobError> {
        let b = self.batch;
        let s = &b.resolved.settings;
        let src = &p.src;
        let passthrough = p.route == Route::Handbrake;
        let test_mode = s.frames > 0;
        let fail = |why: String| Ok(Outcome::Failed(why));

        let (expected, enc_exit) = if passthrough {
            // ── 4K: HandBrake encode, carries HDR10, HDR10+ and Dolby Vision ──
            ep.info(&format!(
                "  Source is {}: encoding with HandBrake, no upscaling.",
                p.note
            ));
            if !s.tune.is_empty() {
                ep.info(&format!("  HandBrake encoder tune: {}", s.tune));
            }
            match src.dv_profile {
                Some(5) => ep.warn("Dolby Vision profile 5 has no HDR10 fallback: it becomes AV1 profile 10.0 and needs a player that supports Dolby Vision in AV1."),
                Some(7) => ep.warn("Dolby Vision profile 7: the enhancement layer is dropped and the RPU converted to profile 8.1 (AV1 profile 10.1)."),
                _ => {}
            }
            if self.tools.handbrake.is_none() {
                let links: Vec<PathBuf> = crate::tools::winget_links()
                    .map(|d| d.join(crate::tools::exe("HandBrakeCLI")))
                    .into_iter()
                    .collect();
                let hb = self
                    .installer
                    .system_tool(crate::tools::install::HANDBRAKE, "HandBrakeCLI", &links)
                    .map_err(|e| JobError::Io(std::io::Error::other(e.to_string())))?;
                let version = self
                    .runner
                    .output(&hb, &[OsString::from("--version")])
                    .ok()
                    .and_then(|o| {
                        o.stdout
                            .lines()
                            .chain(o.stderr.lines())
                            .find(|l| l.contains("HandBrake"))
                            .map(|l| l.trim().to_string())
                    });
                ep.info(&format!(
                    "  Using {}",
                    version.unwrap_or_else(|| "HandBrakeCLI".into())
                ));
                self.tools.handbrake = Some(hb);
            }
            let expected = if test_mode {
                s.frames
            } else {
                ep.info("  Counting source frames ...");
                count_frames(&self.tools.ffprobe, &p.file.path, &self.control).await?
            };
            ep.info(&format!("  Expected frames: {expected}"));
            if uses_chunks(s, p) {
                if let Err(why) = self
                    .chunked_handbrake(p, expected, base, video_out, ep)
                    .await?
                {
                    return fail(why);
                }
                (expected, Some(0))
            } else {
                self.stage(&p.file.name, Stage::Video);
                (
                    expected,
                    self.handbrake_video(p, video_out, None, Span::whole(expected), ep)
                        .await?,
                )
            }
        } else {
            // ── upscale settings for this file (all decided in the plan) ──
            let deband_on = s.deband && self.tools.deband;
            let nat = &p.native;
            ep.info(&format!(
                "  Source: {}x{} {}, {} fps, SAR {}, matrix {}, field order {}, IVTC {}",
                src.width,
                src.height,
                src.codec,
                fmt_n(src.frame_rate, 3),
                p.sar,
                p.matrix_in,
                src.field_order.as_deref().unwrap_or("unknown"),
                p.ivtc_mode
            ));
            ep.info(&format!("  IVTC: {}  [{}]", p.ivtc_mode, p.ivtc_note));
            ep.info(&format!("  Crop: {}", p.crop_label()));
            if !p.upscale {
                ep.info(&format!(
                    "  Upscale: none, encoding at {}x{}",
                    p.out_w, p.out_h
                ));
            } else if s.use_topaz() {
                ep.info(&format!(
                    "  Upscale: {} -> {}x{}",
                    b.model_label, p.out_w, p.out_h
                ));
            } else {
                let native = if nat.height > 0 {
                    format!("{}p {}", nat.height, nat.name)
                } else {
                    "none".into()
                };
                ep.info(&format!("  Native: {native}  [{}]", p.native_note));
                if b.model == crate::settings::Model::Cugan {
                    ep.info(&format!(
                        "  Denoise: {}  [{}]",
                        noise_label(p.noise),
                        p.noise_note
                    ));
                }
                ep.info(&format!(
                    "  Upscale: {} -> {}x{} | deband: {} | temporal: {}",
                    b.model_label,
                    p.out_w,
                    p.out_h,
                    if deband_on { "on" } else { "off" },
                    if s.temporal { "on" } else { "off" }
                ));
            }

            // ── 1. expected frame count ──
            let mlrt = self.tools.mlrt.clone().unwrap_or_default();
            let script_args = cmd::vpy_args(b, p, &mlrt, &self.tools.mlrt_root, deband_on);
            ep.info("  Reading source (first run of a file builds an index) ...");
            self.stage(&p.file.name, Stage::Reading);
            let (code, out, err) = run_capture(
                &self.tools.vspipe,
                &cmd::vspipe_info_args(&script_args, &self.vpy),
                &self.control,
            )
            .await?;
            let info = format!("{out}{err}");
            let Some(expected) = info_frames(&info).filter(|_| code == Some(0)) else {
                ep.raw(&info);
                return fail("upscale script failed to load (details in the episode log)".into());
            };
            ep.info(&format!("  Expected frames: {expected}"));

            // ── 2. upscale + AV1 encode ──
            if uses_chunks(s, p) {
                if let Err(why) = self
                    .chunked_video(p, &script_args, expected, base, video_out, ep)
                    .await?
                {
                    return fail(why);
                }
                (expected, Some(0))
            } else {
                self.stage(&p.file.name, Stage::Video);
                (
                    expected,
                    self.upscale_video(p, &script_args, video_out, Span::whole(expected), ep)
                        .await?,
                )
            }
        };

        if enc_exit != Some(0) || !video_out.is_file() {
            return fail(format!(
                "encoder exited with {} (details in the episode log)",
                enc_exit.map_or("no exit code".into(), |c| c.to_string())
            ));
        }

        // ── 3. verify every frame made it ──
        self.stage(&p.file.name, Stage::Verify);
        let actual = count_frames(&self.tools.ffprobe, video_out, &self.control).await?;
        // The upscale path must match exactly; HandBrake can legitimately differ by a frame or two
        let tolerance = if passthrough {
            2u64.max((expected as f64 * 0.001).ceil() as u64)
        } else {
            0
        };
        if expected == 0 {
            ep.warn(&format!(
                "Couldn't count source frames; skipping the frame check ({actual} encoded)."
            ));
        } else if actual.abs_diff(expected) > tolerance {
            return fail(format!(
                "frame count mismatch: expected {expected}, got {actual} (encode likely crashed; video file kept)"
            ));
        } else {
            ep.info(&format!("  Encoded frames verified: {actual}"));
        }

        let mut vc = video_color(src, false);
        if let Some((w, h)) = p.display_dimensions() {
            // Non-square pixels kept (no upscale): tell players the display size
            vc.mkv_opts
                .extend(["--display-dimensions".to_string(), format!("0:{w}x{h}")]);
        }
        let mut temp_files = Vec::new();
        let mux_args = if test_mode {
            // Test clips are video-only: source audio/subs would be the wrong length
            ep.info("  Test clip: muxing video only.");
            cmd::mkvmerge_args(partial, video_out, passthrough, &vc, &p.file.path, None).0
        } else {
            // ── 4. tracks to keep (decided in the plan) ──
            let tp = p
                .tracks
                .as_ref()
                .expect("tracks planned for non-skip routes");
            if tp.audio_fallback {
                ep.warn(&format!(
                    "No {} audio found; keeping all audio rather than producing a silent file.",
                    s.keep_languages.join("/")
                ));
            }
            for t in &tp.audio_tracks {
                let keep = tp.keep_audio.contains(&t.id);
                let was = t
                    .track_name
                    .as_deref()
                    .filter(|n| !n.is_empty())
                    .map(|n| format!(" '{n}'"))
                    .unwrap_or_default();
                let to = if keep {
                    format!(" -> '{}'", t.new_name.as_deref().unwrap_or(""))
                } else {
                    String::new()
                };
                ep.info(&format!(
                    "  Audio #{} {} [{}]{was} -- {}{to}",
                    t.id,
                    t.codec,
                    t.iso,
                    if keep { "keep" } else { "drop" }
                ));
            }
            for t in &tp.sub_tracks {
                let keep = tp.keep_subs.contains(&t.id);
                ep.info(&format!(
                    "  Subs  #{} {} [{}] {} -- {}",
                    t.id,
                    t.codec,
                    t.iso,
                    t.track_name.as_deref().unwrap_or(""),
                    if keep { "keep" } else { "drop" }
                ));
            }

            // ── 5. EAC3 companion tracks ──
            self.stage(&p.file.name, Stage::Audio);
            let mut new_tracks = Vec::new();
            for e in &tp.eac3 {
                let _ = std::fs::create_dir_all(&self.temp_dir);
                let temp = self.temp_dir.join(format!(
                    "{}_stream{}.eac3",
                    NOT_SAFE.replace_all(base, "_"),
                    e.stream_index
                ));
                ep.info(&format!(
                    "  Stream #{} | {} | {} ch | lang={} -> EAC3 {}",
                    e.stream_index, e.codec, e.channels, e.lang_code, e.bitrate
                ));
                if run_logged(
                    &self.tools.ffmpeg,
                    &cmd::eac3_args(&p.file.path, e, &temp),
                    ep,
                    &self.control,
                )
                .await?
                    != Some(0)
                {
                    ep.warn(&format!(
                        "ffmpeg failed for stream #{}; skipping this track.",
                        e.stream_index
                    ));
                    continue;
                }
                temp_files.push(temp.clone());
                new_tracks.push(NewTrack {
                    path: temp,
                    name: e.name.clone(),
                    lang: e.lang_code.clone(),
                });
            }

            // ── 6. default tracks ──
            let langs: Vec<&str> = new_tracks.iter().map(|t| t.lang.as_str()).collect();
            let d = default_tracks(
                tp,
                &langs,
                &s.default_audio_language,
                &s.default_subtitle_language,
            );
            let (args, notes) = cmd::mkvmerge_args(
                partial,
                video_out,
                passthrough,
                &vc,
                &p.file.path,
                Some((tp, &new_tracks, &d)),
            );
            for n in notes {
                match n {
                    cmd::DefaultsNote::Audio(label) => {
                        ep.info(&format!("  Default audio: {label}"))
                    }
                    cmd::DefaultsNote::NoAudioInLanguage => ep.warn(&format!(
                        "No {} audio; keeping the source's default audio flags.",
                        d.audio_lang
                    )),
                    cmd::DefaultsNote::Subtitles(label) => {
                        ep.info(&format!("  Default subtitles: {label}"))
                    }
                    cmd::DefaultsNote::SubtitlesNone(why) => ep.info(&format!(
                        "  Default subtitles: none{}",
                        why.map(|w| format!(" ({w})")).unwrap_or_default()
                    )),
                    cmd::DefaultsNote::NoSubtitlesInLanguage => ep.warn(&format!(
                        "No {} subtitles; keeping the source's default subtitle flags.",
                        s.default_subtitle_language
                    )),
                }
            }
            args
        };

        // ── 7. mux to .partial.mkv, then rename ──
        self.stage(&p.file.name, Stage::Muxing);
        ep.info("  Muxing ...");
        let mux_exit = run_logged(&self.tools.mkvmerge, &mux_args, ep, &self.control).await;
        for t in &temp_files {
            let _ = std::fs::remove_file(t);
        }
        let mux_exit = mux_exit?;
        // mkvmerge exit 1 = warnings
        if mux_exit.is_none_or(|c| c >= 2) {
            let _ = std::fs::remove_file(partial);
            return fail(format!(
                "mkvmerge failed (exit {}); video file kept",
                mux_exit.map_or("?".into(), |c| c.to_string())
            ));
        }
        std::fs::rename(partial, mkv)?;
        if !self.keep_ivf {
            let _ = std::fs::remove_file(video_out);
        }
        // The chunks are in the finished file now
        WorkDir::new(&self.temp_dir, base).remove();

        // ── 8. comparison screenshots (upscaled files; HDR sources would need tone mapping) ──
        if !passthrough {
            self.stage(&p.file.name, Stage::Compare);
            match self.save_comparisons(p, mkv, base, ep).await {
                Err(JobError::Cancelled) => return Err(JobError::Cancelled),
                Err(JobError::Io(e)) => ep.warn(&format!("Comparison images failed: {e}")),
                Ok(()) => {}
            }
        }

        // ── 9. DeleteSource / ReplaceSource: the comparisons were the last use of the source ──
        if (s.delete_source || s.replace_source)
            && let Some(replaced) = self.remove_source(p, mkv, ep).await?
        {
            return Ok(Outcome::Done(replaced));
        }
        Ok(Outcome::Done(mkv.to_path_buf()))
    }

    /// Moves the source to the Recycle Bin (Trash) when the finished file lasts as long as it
    /// does. Test clips, a duration that doesn't match or a failed delete keep the source.
    /// With ReplaceSource the finished file first moves next to the source and takes its name
    /// once the source is gone; returns where it ended up.
    async fn remove_source(
        &self,
        p: &FilePlan,
        mkv: &Path,
        ep: &EpisodeLog<'_>,
    ) -> Result<Option<PathBuf>, JobError> {
        let source = &p.file.path;
        let replace = self.batch.resolved.settings.replace_source;
        if self.batch.resolved.settings.frames > 0 {
            ep.info("  Test clip: source kept (DeleteSource applies to whole episodes).");
            return Ok(None);
        }
        let probe: Vec<OsString> = [
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
        ]
        .map(OsString::from)
        .into_iter()
        .chain([mkv.as_os_str().to_owned()])
        .collect();
        let (_, out, _) = run_capture(&self.tools.ffprobe, &probe, &self.control).await?;
        let out_dur: f64 = out.trim().parse().unwrap_or(0.0);
        let src_dur = p.src.duration;
        // The frame check already passed; this catches a mux that lost part of the file
        let tolerance = 1f64.max(src_dur * 0.001);
        if src_dur <= 0.0 || (out_dur - src_dur).abs() > tolerance {
            ep.warn(&format!(
                "Source kept: the output lasts {out_dur:.1} s, the source {src_dur:.1} s ({}).",
                source.display()
            ));
            return Ok(None);
        }
        // The new file waits next to the source, so the source is only deleted once its
        // replacement is already in that folder
        let dest = source.with_extension("mkv");
        let staged = source.with_extension("upscale.partial.mkv");
        if replace {
            if dest != *source && dest.exists() {
                ep.warn(&format!("Source kept: {} already exists.", dest.display()));
                return Ok(None);
            }
            ep.info(&format!(
                "  Moving the new file to {} ...",
                staged.display()
            ));
            if let Err(e) = move_file(mkv, &staged).await {
                let _ = std::fs::remove_file(&staged);
                ep.warn(&format!(
                    "Source kept: couldn't move the new file next to it ({e})."
                ));
                return Ok(None);
            }
        }
        let removed = if self.batch.resolved.settings.source_is_rip {
            std::fs::remove_file(source)
        } else {
            crate::platform::move_to_trash(source)
        };
        if let Err(e) = removed {
            ep.warn(&format!(
                "Source kept: couldn't delete {} ({e}).",
                source.display()
            ));
            if replace && move_file(&staged, mkv).await.is_err() {
                ep.warn(&format!("The new file is at {}.", staged.display()));
                return Ok(Some(staged));
            }
            return Ok(None);
        }
        ep.info(&format!(
            "  {}: {}",
            if self.batch.resolved.settings.source_is_rip {
                "Raw rip deleted"
            } else {
                "Source moved to the trash"
            },
            source.display()
        ));
        if !replace {
            return Ok(None);
        }
        match std::fs::rename(&staged, &dest) {
            Ok(()) => {
                ep.info(&format!("  New file in place: {}", dest.display()));
                Ok(Some(dest))
            }
            Err(e) => {
                ep.warn(&format!(
                    "Couldn't rename {} to {} ({e}).",
                    staged.display(),
                    dest.display()
                ));
                Ok(Some(staged))
            }
        }
    }

    async fn run_job(&mut self, p: &FilePlan) -> (Outcome, Duration) {
        let b = self.batch;
        let s = &b.resolved.settings;
        let base = output_base(b, p);
        let dir = final_dir(b, &b.out_dir);
        let mkv = dir.join(format!("{base}.mkv"));
        let partial = dir.join(format!("{base}.partial.mkv"));
        let start = Instant::now();

        if p.route == Route::Skip {
            self.log
                .info(&format!("Skipping {} ({})", p.file.name, p.note));
            return (Outcome::Skipped(p.note.clone()), Duration::ZERO);
        }
        if mkv.exists() {
            self.log
                .info(&format!("Skipping {} (output exists)", p.file.name));
            return (Outcome::Skipped("output exists".into()), Duration::ZERO);
        }
        if let Err(e) = std::fs::create_dir_all(&dir) {
            let why = format!("can't create {} ({e})", dir.display());
            return (Outcome::Failed(why), Duration::ZERO);
        }
        // Another machine writing to the same output folder may be on this episode: claim it
        // before touching its partial files (claim.rs)
        let _claim = match Claim::acquire(&b.out_dir, &base, &host_name()) {
            Ok(Ok(mut claim)) => {
                claim.keep_fresh();
                Some(claim)
            }
            Ok(Err(holder)) => {
                let why = format!("{BUSY_PREFIX}{}", holder.host);
                self.log.info(&format!("Skipping {} ({why})", p.file.name));
                return (Outcome::Skipped(why), Duration::ZERO);
            }
            Err(e) => {
                self.log.warn(&format!(
                    "Couldn't claim {} in the output folder ({e}); encoding without a claim.",
                    p.file.name
                ));
                None
            }
        };
        // It may have finished elsewhere between the check above and the claim
        if mkv.exists() {
            self.log
                .info(&format!("Skipping {} (output exists)", p.file.name));
            return (Outcome::Skipped("output exists".into()), Duration::ZERO);
        }
        let _ = std::fs::remove_file(&partial);

        // Free space: stop the batch rather than fail hours into an encode
        // on the output drive, and on the temp drive (chunks about the size of the output, plus
        // temporary audio: half the estimate) when it's a different folder
        let need = space_needed(p.file.size, p.route);
        let mut checks = vec![(b.out_dir.as_path(), need, "output")];
        if self.temp_dir != b.out_dir.join(".work") {
            checks.push((self.temp_dir.as_path(), need / 2, "temp"));
        }
        for (dir, need, drive) in checks {
            if let Some(free) = free_space(dir).filter(|f| *f < need) {
                self.log.warn(&format!(
                    "Not enough disk space for {}: about {} GB needed, {} GB free on the {drive} drive. Stopping the batch.",
                    p.file.name,
                    fmt_n(need as f64 / GB as f64, 0),
                    fmt_n(free as f64 / GB as f64, 0)
                ));
                return (
                    Outcome::StopBatch("not enough disk space (batch stopped)".into()),
                    Duration::ZERO,
                );
            }
        }

        let log_path = b.out_dir.join("logs").join(format!("{base}.log"));
        // A restarted job keeps its log, so the earlier attempt stays readable
        if !self.restart {
            let _ = std::fs::remove_file(&log_path);
        }
        let file = match OpenOptions::new().create(true).append(true).open(&log_path) {
            Ok(f) => f,
            Err(e) => {
                return (
                    Outcome::Failed(format!("can't write {}: {e}", log_path.display())),
                    Duration::ZERO,
                );
            }
        };
        let ep = EpisodeLog {
            batch: self.log,
            file: Mutex::new(file),
            path: log_path,
            job_id: p.file.name.clone(),
            events: self.events.clone(),
        };
        let _ = self.events.send(Event::JobStarted {
            job_id: p.file.name.clone(),
            file: p.file.path.clone(),
        });

        let rule = "==============================================";
        ep.info("");
        ep.info(rule);
        ep.info(&format!("  {}", p.file.name));
        ep.info(&format!(
            "  CRF {}, preset {} | {}",
            s.crf, s.preset, s.svt_params
        ));
        ep.info(rule);
        if self.restart {
            let note = chunks::resume_note(&self.temp_dir, &base)
                .map(|n| format!(" ({n})"))
                .unwrap_or_default();
            ep.info(&format!("=== Restarted after an interruption{note} ==="));
        }

        let chunked = uses_chunks(s, p);
        let video_out = if p.route == Route::Handbrake || chunked {
            b.out_dir.join(format!("{base}.video.mkv"))
        } else {
            b.out_dir.join(format!("{base}.ivf"))
        };
        let outcome = match self
            .run_file(p, &ep, &base, &partial, &mkv, &video_out)
            .await
        {
            Ok(o) => o,
            Err(JobError::Cancelled) => {
                // Partial outputs of a cancelled job are deleted, chunks included. Killed
                // processes release their files a moment later on Windows, so retry for a few seconds.
                for _ in 0..50 {
                    let work = WorkDir::new(&self.temp_dir, &base);
                    work.remove();
                    if !work.exists() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                for f in [&video_out, &partial] {
                    for _ in 0..50 {
                        if !f.exists() || std::fs::remove_file(f).is_ok() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
                ep.warn("Cancelled; partial files deleted.");
                Outcome::Cancelled
            }
            Err(JobError::Io(e)) => Outcome::Failed(e.to_string()),
        };
        match &outcome {
            Outcome::Failed(why) => ep.warn(&format!("{}: {why}", p.file.name)),
            Outcome::Done(out) => ep.info(&format!(
                "  Done in {} -> {}",
                hms(start.elapsed()),
                out.display()
            )),
            _ => {}
        }
        (outcome, start.elapsed())
    }

    /// Runs every planned file in order. Returns the summary rows.
    pub async fn run_batch(&mut self) -> Vec<JobResult> {
        let mut results = Vec::new();
        let (mut done, mut failed, mut skipped, mut cancelled) = (0, 0, 0, false);
        for p in &self.batch.plans {
            if self.control.is_cancelled() {
                cancelled = true;
                break;
            }
            let (outcome, took) = self.run_job(p).await;
            let id = p.file.name.clone();
            let time = if took.is_zero() {
                String::new()
            } else {
                hms(took)
            };
            let (result, event) = match outcome {
                Outcome::Done(output) => {
                    done += 1;
                    (
                        "done".to_string(),
                        Event::JobDone {
                            job_id: id,
                            output,
                            seconds: took.as_secs(),
                        },
                    )
                }
                Outcome::Skipped(why) => {
                    skipped += 1;
                    let r = if why == "output exists" {
                        "skipped".to_string()
                    } else {
                        format!("skipped ({why})")
                    };
                    (
                        r,
                        Event::JobSkipped {
                            job_id: id,
                            reason: why,
                        },
                    )
                }
                Outcome::Failed(why) | Outcome::StopBatch(why) => {
                    failed += 1;
                    (
                        format!("FAILED: {why}"),
                        Event::JobFailed {
                            job_id: id,
                            reason: why,
                        },
                    )
                }
                Outcome::Cancelled => {
                    cancelled = true;
                    (
                        "cancelled".to_string(),
                        Event::JobFailed {
                            job_id: id,
                            reason: "cancelled".into(),
                        },
                    )
                }
            };
            let stop = result.ends_with("(batch stopped)") || cancelled;
            let _ = self.events.send(event);
            results.push(JobResult {
                file: p.file.name.clone(),
                result,
                time,
            });
            if stop {
                break;
            }
            if self.control.stopping() {
                self.log.info("Stopping after the current file, as asked.");
                break;
            }
        }

        self.log.info("");
        self.log.info("Summary");
        for r in &results {
            self.log
                .info(&format!("  {:<8} {}  {}", r.time, r.file, r.result));
        }
        let logs = self.batch.out_dir.join("logs");
        let mut line = format!(
            "Output: {}   Logs: {}",
            self.batch.out_dir.display(),
            logs.display()
        );
        if self.batch.resolved.settings.screenshots > 0 {
            line.push_str(&format!(
                "   Comparisons: {}",
                self.batch.out_dir.join("compare").display()
            ));
        }
        self.log.info(&line);
        let _ = self.events.send(Event::BatchDone {
            done,
            failed,
            skipped,
            cancelled,
        });
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(hms(Duration::from_secs(3 * 3600 + 7 * 60 + 9)), "03:07:09");
    }

    #[test]
    fn handbrake_progress_lines() {
        let c = HB_PROGRESS
            .captures("Encoding: task 1 of 1, 12.34 % (45.67 fps, avg 44.00 fps, ETA 00h12m34s)")
            .unwrap();
        assert_eq!(
            (&c[1], c.get(2).map(|m| m.as_str())),
            ("12.34", Some("45.67"))
        );
        assert!(
            HB_PROGRESS
                .captures("Encoding: task 1 of 1, 0.10 %")
                .is_some()
        );
    }

    #[test]
    fn safe_names() {
        assert_eq!(
            NOT_SAFE.replace_all("Sample Long Film (2003) [BD]", "_"),
            "Sample_Long_Film__2003___BD_"
        );
        assert_eq!(NON_WORD.replace_all("Real-CUGAN", "_"), "Real_CUGAN");
    }
}
