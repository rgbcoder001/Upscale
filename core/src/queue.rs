//! The app's job queue: jobs that survive a restart (`queue.json`), app
//! settings (`settings.json`), planning in the background, and one job at a time through the
//! pipeline. The UI only calls the methods here and listens to [`EngineEvent`]s.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::batch::{self, Batch};
use crate::chunks;
use crate::claim::{BUSY_PREFIX, held_elsewhere, host_name};
use crate::event::{Event, Stage};
use crate::files::{VideoFile, is_video, list_videos};
use crate::gpu::{GpuChoice, list_gpus, pick_gpu};
use crate::log::{FileLog, Log, MemoryLog};
use crate::pipeline::{Control, RunEnv, output_base};
use crate::plan::Route;
use crate::process::{Runner, SystemRunner};
use crate::report::file_report;
use crate::tools::install::Installer;
use crate::tools::set::{Needs, ToolSet, ToolStatus};
use crate::tools::{self, find_scripts_dir};
use crate::{Error, ProfileId, Result};

/// How often an otherwise finished queue looks again at episodes another computer claimed.
const CLAIM_RECHECK: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    /// One output folder for every profile.
    pub output_dir: PathBuf,
    /// The private tools folder (vs-mlrt, BtbN FFmpeg).
    pub tools_dir: PathBuf,
    /// The profile picked last time in the Add sheet.
    pub last_profile: ProfileId,
    /// Chunks of resumable encodes and temporary audio. Empty = [`tools::default_work_dir`],
    /// on this machine's own disk.
    pub temp_dir: Option<PathBuf>,
    /// Use tools from PATH and install them with winget/pip system-wide (the script's way),
    /// instead of the private tools folder.
    pub use_system_tools: bool,
    /// What the Add sheet starts with for the original file (`DeleteSource` / `ReplaceSource`).
    pub original_file: OriginalFile,
    /// Jellyfin library folders ripped discs go into.
    pub library_folders: Vec<LibraryFolder>,
    /// TMDB "API Read Access Token" for name suggestions; empty = names are typed by hand.
    pub tmdb_token: String,
    /// Languages a rip keeps by default (ISO 639-2), before the user adds or removes any.
    pub keep_languages: Vec<String>,
    /// The user approved installing and updating MakeMKV (not notarized by Apple), once, in
    /// Settings → Ripping.
    pub makemkv_approved: bool,
    /// Where MakeMKV writes raw rips before they're encoded (Settings → Ripping → Rip folder).
    /// Empty = the Temp folder, else this machine's local work folder.
    pub rip_dir: Option<PathBuf>,
    /// Chunks of an encode-only job (Anime AV1) encoded at the same time on this computer
    /// (1–3). Several SVT-AV1 encoders can use more of a many-core CPU than one.
    pub parallel_chunks: u32,
}

/// A Jellyfin library root and what it holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryFolder {
    /// What the Rip sheet calls it ("Anime Shows"); empty = the folder's own name.
    #[serde(default)]
    pub name: String,
    pub path: PathBuf,
    /// How files in it are named: movies or shows.
    pub kind: LibraryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LibraryKind {
    Movies,
    Shows,
}

/// English, Japanese and Spanish.
pub fn default_keep_languages() -> Vec<String> {
    ["eng", "jpn", "spa"].map(String::from).into()
}

/// What happens to the source after a checked encode; the Add sheet's choice for each job.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OriginalFile {
    #[default]
    Keep,
    Delete,
    Replace,
}

impl AppSettings {
    /// The app's saved settings (`<app data>/settings.json`), so `upscale-cli` writes to the
    /// same output folder as the app.
    pub fn load() -> Self {
        read_json(&tools::app_data_dir().join("settings.json")).unwrap_or_default()
    }

    /// Tools from PATH (Homebrew) instead of the private folder: the setting, and always
    /// outside Windows, where the private downloads have no builds.
    pub fn system_tools(&self) -> bool {
        self.use_system_tools || !cfg!(windows)
    }

    /// Where raw rips go: the Rip folder, else `<temp or local work folder>/rips`.
    pub fn rips_dir(&self) -> PathBuf {
        let set = |d: &Option<PathBuf>| d.clone().filter(|d| !d.as_os_str().is_empty());
        set(&self.rip_dir).unwrap_or_else(|| {
            set(&self.temp_dir)
                .unwrap_or_else(tools::default_work_dir)
                .join("rips")
        })
    }

    /// The work folder for a job whose output is called `base`. Without a Temp folder setting
    /// it's the local default; a job interrupted while the default was `<output>\.work` keeps
    /// its chunks there, so it still resumes.
    pub fn work_dir(&self, base: &str) -> PathBuf {
        if let Some(dir) = self.temp_dir.clone().filter(|d| !d.as_os_str().is_empty()) {
            return dir;
        }
        let legacy = self.output_dir.join(".work");
        if chunks::WorkDir::new(&legacy, base).exists() {
            return legacy;
        }
        tools::default_work_dir()
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_default();
        Self {
            output_dir: home.join("Videos").join("Upscaled"),
            tools_dir: tools::default_tools_dir(),
            last_profile: ProfileId::AnimeTopaz,
            temp_dir: None,
            use_system_tools: !cfg!(windows),
            original_file: OriginalFile::Keep,
            library_folders: Vec::new(),
            tmdb_token: String::new(),
            keep_languages: default_keep_languages(),
            makemkv_approved: false,
            rip_dir: None,
            parallel_chunks: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobState {
    Analysing,
    Queued,
    Running,
    Done,
    Failed,
    Skipped,
    Cancelled,
    /// The app closed during the encode; it restarts from the beginning.
    Interrupted,
    /// A disc title waiting for the drive.
    WaitingToRip,
    /// MakeMKV is ripping this title; it's planned and encoded once ripped.
    Ripping,
}

impl JobState {
    fn runnable(self) -> bool {
        matches!(self, Self::Queued | Self::Interrupted)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobProgress {
    pub frames_done: u64,
    pub frames_total: u64,
    pub fps: f64,
    pub eta_seconds: Option<u64>,
    pub projected_bytes: Option<u64>,
    pub paused: bool,
    /// Chunk being encoded and the number of chunks (resumable encodes).
    #[serde(default)]
    pub chunk: Option<(u32, u32)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    /// While a disc title rips: where from and how far along.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rip: Option<crate::rip::RipProgress>,
    pub id: String,
    pub file: PathBuf,
    pub name: String,
    /// Jobs planned together (season consensus spans a group).
    pub group: String,
    /// Where settings files are looked up from: the folder added, or the file itself.
    pub input: PathBuf,
    /// Job options (script setting names), e.g. `Content`, `Crf`.
    pub options: Map<String, Value>,
    pub state: JobState,
    #[serde(default)]
    pub profile: Option<ProfileId>,
    /// `Upscale 1080p → 2160p`, `Encode only`, `Skip`.
    #[serde(default)]
    pub route: Option<String>,
    /// This file's part of the plan report.
    #[serde(default)]
    pub plan: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<Stage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<JobProgress>,
    #[serde(default)]
    pub output: Option<PathBuf>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub log: Option<PathBuf>,
    /// Size of the source file.
    #[serde(default)]
    pub source_bytes: Option<u64>,
    /// Size of the finished output file.
    #[serde(default)]
    pub output_bytes: Option<u64>,
    /// Where the finished file will be, for jobs that leave one in the output folder: not
    /// skipped files, and not ReplaceSource, whose file moves into the source's place. Two
    /// jobs with the same one would write the same file ([`name_clash`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planned_output: Option<PathBuf>,
    /// The finished file's VMAF, when it was measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vmaf: Option<f64>,
}

/// The job that already has `jobs[i]`'s output file: one that will write it or is writing it,
/// or a finished one whose file is still there. Outputs are named after their sources, so
/// `01.mkv` from two shows would be one file, and the second would be skipped as "output
/// exists" (or, with DeleteSource, lose its source for the wrong episode's file).
fn name_clash(jobs: &[Job], i: usize) -> Option<&Job> {
    let key = |p: &Path| p.to_string_lossy().to_lowercase();
    let out = key(jobs[i].planned_output.as_deref()?);
    jobs.iter().enumerate().find_map(|(k, other)| {
        let holds = match other.state {
            JobState::Analysing | JobState::Queued | JobState::Running | JobState::Interrupted => {
                true
            }
            JobState::Done => other.output == other.planned_output,
            _ => false,
        };
        (k != i
            && holds
            && other.file != jobs[i].file
            && other.planned_output.as_deref().map(key).as_ref() == Some(&out))
        .then_some(other)
    })
}

fn clash_message(other: &Job, out: &Path) -> String {
    format!(
        "Same output file as \"{}\": both would be written to {}. Rename one of the files, or give this one its own folder with \"OutputDir\" (Options → Advanced).",
        other.name,
        out.display()
    )
}

/// The groups an add makes, each a folder and its videos: a folder that's added brings the
/// folders below it (a show's seasons), each planned as its own group, and loose files are
/// grouped by their folder. `output_dir` is never looked into.
fn add_groups(paths: &[PathBuf], output_dir: &Path) -> Result<Vec<(PathBuf, Vec<VideoFile>)>> {
    let mut groups: Vec<(PathBuf, Vec<VideoFile>)> = Vec::new();
    let mut loose: Vec<(PathBuf, VideoFile)> = Vec::new();
    for p in paths {
        let p = std::path::absolute(p)?;
        if p.is_dir() {
            for dir in crate::files::video_folders(&p, output_dir) {
                let videos = list_videos(&dir)?;
                if !videos.is_empty() && !groups.iter().any(|(d, _)| *d == dir) {
                    groups.push((dir, videos));
                }
            }
        } else if is_video(&p) {
            let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
            loose.push((dir, VideoFile::from_path(&p)?));
        }
    }
    for (dir, file) in loose {
        match groups.iter_mut().find(|(d, _)| *d == dir) {
            Some((_, files)) => files.push(file),
            None => groups.push((dir, vec![file])),
        }
    }
    Ok(groups)
}

/// The plan's `Saves to:` line: where the finished file goes. None for skipped files.
pub(crate) fn saves_to_line(p: &crate::plan::FilePlan, output: Option<&Path>) -> Option<String> {
    if p.route == Route::Skip {
        return None;
    }
    Some(match output {
        Some(out) => format!("  Saves to:  {}", out.display()),
        None => format!(
            "  Saves to:  in place of the source ({})",
            p.file.path.with_extension("mkv").display()
        ),
    })
}

/// Where a planned file's output goes, if it leaves one in the output folder.
fn planned_output(b: &Batch, p: &crate::plan::FilePlan, out_dir: &Path) -> Option<PathBuf> {
    (p.route != Route::Skip && !b.resolved.settings.replace_source)
        .then(|| crate::pipeline::final_dir(b, out_dir).join(format!("{}.mkv", output_base(b, p))))
}

fn file_size(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| m.len())
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum EngineEvent {
    /// The whole queue (after any change other than progress).
    #[serde(rename_all = "camelCase")]
    Queue { snapshot: QueueSnapshot },
    /// One job changed (progress, stage).
    Job { job: Box<Job> },
    /// What the engine is doing right now (tool downloads); not saved.
    #[serde(rename_all = "camelCase")]
    Status { status: String },
    /// A log line of the running job.
    #[serde(rename_all = "camelCase")]
    Log { job_id: String, line: String },
    /// A disc was inserted and read.
    Disc { disc: Box<crate::disc::Disc> },
    /// The disc was ejected.
    DiscRemoved,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueSnapshot {
    pub jobs: Vec<Job>,
    pub running: bool,
    pub paused: bool,
    pub stopping: bool,
    /// What the engine is doing outside a job ("Checking tools ...", a tool error).
    pub status: String,
    /// A disc is being ripped.
    #[serde(default)]
    pub ripping: bool,
}

pub type Notify = Arc<dyn Fn(EngineEvent) + Send + Sync>;

/// A group to plan again: id, input, options, files.
type Replan = (String, PathBuf, Map<String, Value>, Vec<PathBuf>);
/// How a job ended: state, output, message.
type Ending = (Option<JobState>, Option<PathBuf>, Option<String>);

struct State {
    jobs: Vec<Job>,
    settings: AppSettings,
    control: Option<Arc<Control>>,
    running: bool,
    status: String,
}

/// The rip in progress: the running `makemkvcon`, the job it rips, and whether Cancel rip
/// (everything) or removing that job (just this title) stopped it.
#[derive(Default)]
struct Rip {
    child: Option<std::process::Child>,
    job: Option<String>,
    cancelled: bool,
    skip: bool,
}

struct Inner {
    data_dir: PathBuf,
    state: Mutex<State>,
    batches: Mutex<HashMap<String, Arc<Batch>>>,
    notify: Notify,
    gpu: GpuChoice,
    runtime: tokio::runtime::Runtime,
    seq: AtomicU64,
    /// Tool download progress, shown as the engine status.
    progress: Box<crate::tools::install::ProgressFn>,
    /// The disc in the drive, once MakeMKV has read it.
    disc: Mutex<Option<crate::disc::Disc>>,
    /// The rip in progress, if any.
    rip: Mutex<Option<Rip>>,
}

/// The queue and the worker that plans and runs it. Cheap to clone.
#[derive(Clone)]
pub struct Engine(Arc<Inner>);

fn parse_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// The save before the current one (`queue.json.bak`), kept by [`write_json`].
fn backup_path(path: &Path) -> PathBuf {
    path.with_extension("json.bak")
}

/// `path`, or the save before it when `path` is missing or can't be read.
fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    parse_json(path).or_else(|| parse_json(&backup_path(path)))
}

/// [`read_json`] at start-up, for files that must not be lost quietly: a file that can't be read
/// is kept as `<name>.json.bad` instead of being overwritten by the next save, and the note
/// says what happened (`None` when the file was fine or was never there).
fn recover_json<T: for<'de> Deserialize<'de>>(path: &Path) -> (Option<T>, Option<String>) {
    if let Some(value) = parse_json(path) {
        return (Some(value), None);
    }
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let unreadable = path.is_file();
    let bad = path.with_extension("json.bad");
    if unreadable {
        let _ = std::fs::rename(path, &bad);
    }
    let previous = parse_json(&backup_path(path));
    let note = match (unreadable, previous.is_some()) {
        (true, true) => Some(format!(
            "{name} couldn't be read, so the save before it was restored. The unreadable file is kept as {}.",
            bad.display()
        )),
        (true, false) => Some(format!(
            "{name} couldn't be read and there was no earlier save, so it starts empty. The unreadable file is kept as {}.",
            bad.display()
        )),
        (false, true) => Some(format!(
            "{name} was missing, so the save before it was restored."
        )),
        (false, false) => None,
    };
    (previous, note)
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    use std::io::Write;
    // One save at a time: the queue is saved from the UI's commands, the planning threads and
    // the run loop, and two writers of the same temporary file would garble it
    static SAVING: Mutex<()> = Mutex::new(());
    let _saving = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Write, flush to the disk, then rename, so neither a crash nor a power cut leaves half a
    // file (a rename can reach the disk before the data it names)
    let tmp = path.with_extension("json.tmp");
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(serde_json::to_string_pretty(value)?.as_bytes())?;
        file.sync_all()?;
    }
    // The current save becomes the backup. Between the two renames there is no `path`;
    // `read_json` then falls back to the backup.
    if path.is_file() {
        let _ = std::fs::rename(path, backup_path(path));
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// How much of a job's log the Log tab reads: the end of it. Encoders print progress all the
/// time, so the log of a long encode grows to many megabytes, and the tab asks every 2 s.
const LOG_TAIL: u64 = 512 * 1024;

/// The last `max` bytes of a file, from the start of a line.
fn read_tail(path: &Path, max: u64) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let from = len.saturating_sub(max);
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    if from > 0 {
        // Started mid-line: drop what's left of that line
        let line = bytes.iter().position(|b| *b == b'\n').map_or(0, |i| i + 1);
        bytes.drain(..line);
    }
    Some(bytes)
}

/// What a run of the queue set up beyond the tools every job uses. A job added while the queue
/// runs may need more than the jobs that were there when it started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Prepared {
    topaz: bool,
    vs_mlrt: bool,
    deband: bool,
}

impl Prepared {
    fn needed_by(s: &crate::settings::Settings) -> Self {
        Self {
            topaz: s.use_topaz(),
            vs_mlrt: s.uses_vs_mlrt(),
            deband: s.deband,
        }
    }

    fn covers(self, need: Self) -> bool {
        (self.topaz || !need.topaz)
            && (self.vs_mlrt || !need.vs_mlrt)
            && (self.deband || !need.deband)
    }
}

/// `new`, with the plans `old` already had for other files kept in front of its own.
fn merge_batch(old: &Batch, mut new: Batch) -> Batch {
    let mut plans: Vec<_> = old
        .plans
        .iter()
        .filter(|p| !new.plans.iter().any(|n| n.file.path == p.file.path))
        .cloned()
        .collect();
    plans.append(&mut new.plans);
    new.plans = plans;
    new
}

fn route_label(p: &crate::plan::FilePlan) -> String {
    match p.route {
        Route::Upscale if !p.upscale => format!("Encode {}p", p.out_h),
        Route::Upscale => format!("Upscale {}p → {}p", p.src.height, p.out_h),
        Route::Handbrake => "Encode only".into(),
        Route::Skip => "Skip".into(),
    }
}

impl Engine {
    /// Loads settings and the queue from `data_dir`. Jobs that were running when the app closed
    /// become Interrupted; unfinished jobs are planned again (analysis is cached).
    pub fn new(data_dir: PathBuf, notify: Notify) -> Result<Self> {
        let (settings, settings_note) =
            recover_json::<AppSettings>(&data_dir.join("settings.json"));
        let settings = settings.unwrap_or_default();
        crate::log::rotate(&data_dir.join("app.log"), crate::log::APP_LOG_MAX);
        crate::log::trim_run_logs(&settings.output_dir.join("logs"), crate::log::KEEP_RUN_LOGS);
        let (jobs, queue_note) = recover_json::<Vec<Job>>(&data_dir.join("queue.json"));
        let mut jobs = jobs.unwrap_or_default();
        // A lost queue or lost settings are said, in the log and in the window's status line
        let notes: Vec<String> = [settings_note, queue_note].into_iter().flatten().collect();
        if !notes.is_empty() {
            let log = FileLog::create(&data_dir.join("app.log"))
                .unwrap_or_else(|_| FileLog::stderr_only());
            for note in &notes {
                log.warn(note);
            }
        }
        for j in &mut jobs {
            j.stage = None;
            j.progress = None;
            // Queues saved before the sizes were recorded
            if j.source_bytes.is_none() {
                j.source_bytes = file_size(&j.file);
            }
            if j.output_bytes.is_none() && j.state == JobState::Done {
                j.output_bytes = j.output.as_deref().and_then(file_size);
            }
            if matches!(j.state, JobState::WaitingToRip | JobState::Ripping) {
                j.state = JobState::Failed;
                j.rip = None;
                j.message =
                    Some("The app closed before this title was ripped; rip the disc again".into());
            }
            if j.state == JobState::Running {
                j.state = JobState::Interrupted;
                // The stem is the encode's `output_base` except for test clips, which are
                // never chunked, so there's nothing to resume for them anyway
                let resume = j.file.file_stem().and_then(|stem| {
                    let stem = stem.to_string_lossy();
                    chunks::resume_note(&settings.work_dir(&stem), &stem)
                });
                j.message = Some(match resume {
                    Some(note) => format!("Interrupted, {note}"),
                    None => "Interrupted, will restart from the beginning".into(),
                });
            }
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(4)
            .build()?;
        let gpu = pick_gpu(&list_gpus(&SystemRunner));
        let notify_for_progress = notify.clone();
        let engine = Self(Arc::new(Inner {
            data_dir,
            state: Mutex::new(State {
                jobs,
                settings,
                control: None,
                running: false,
                status: notes.join(" "),
            }),
            batches: Mutex::new(HashMap::new()),
            notify,
            gpu,
            runtime,
            seq: AtomicU64::new(0),
            disc: Mutex::new(None),
            rip: Mutex::new(None),
            progress: {
                let notify = notify_for_progress;
                Box::new(move |file: &str, done: u64, total: u64| {
                    let mb = |b: u64| b / (1 << 20);
                    let status = if total > 0 {
                        format!("Downloading {file}: {} / {} MB", mb(done), mb(total))
                    } else {
                        format!("Downloading {file}: {} MB", mb(done))
                    };
                    notify(EngineEvent::Status { status });
                })
            },
        }));
        engine.replan_unfinished();
        Ok(engine)
    }

    pub fn gpu(&self) -> &GpuChoice {
        &self.0.gpu
    }

    /// An installer for the configured tools folder, reporting download progress.
    fn installer<'a>(
        &'a self,
        runner: &'a SystemRunner,
        log: &'a dyn Log,
        settings: &AppSettings,
        update: bool,
    ) -> Installer<'a> {
        Installer {
            runner,
            log,
            tools_dir: settings.tools_dir.clone(),
            update,
            system: settings.system_tools(),
            progress: Some(&*self.0.progress),
        }
    }

    fn next_id(&self, prefix: &str) -> String {
        let n = self.0.seq.fetch_add(1, Ordering::Relaxed);
        format!("{prefix}{:x}{n:03x}", chrono::Utc::now().timestamp_millis())
    }

    pub fn snapshot(&self) -> QueueSnapshot {
        let st = self.0.state.lock().expect("state");
        let control = st.control.as_ref();
        QueueSnapshot {
            jobs: st.jobs.clone(),
            running: st.running,
            paused: control.is_some_and(|c| c.is_paused()),
            stopping: control.is_some_and(|c| c.stopping()),
            status: st.status.clone(),
            ripping: self.0.rip.lock().expect("rip").is_some(),
        }
    }

    /// Whether a job is running, and whether a disc is being ripped (without copying the
    /// queue, as `snapshot` does).
    pub fn busy(&self) -> (bool, bool) {
        let encoding = {
            let st = self.0.state.lock().expect("state");
            st.jobs.iter().any(|j| j.state == JobState::Running)
        };
        (encoding, self.0.rip.lock().expect("rip").is_some())
    }

    /// Saves the queue and tells the UI.
    fn changed(&self) {
        let jobs = self.0.state.lock().expect("state").jobs.clone();
        let _ = write_json(&self.0.data_dir.join("queue.json"), &jobs);
        (self.0.notify)(EngineEvent::Queue {
            snapshot: self.snapshot(),
        });
    }

    fn update_job(&self, id: &str, f: impl FnOnce(&mut Job)) -> Option<Job> {
        let mut st = self.0.state.lock().expect("state");
        let job = st.jobs.iter_mut().find(|j| j.id == id)?;
        f(job);
        Some(job.clone())
    }

    pub fn settings(&self) -> AppSettings {
        self.0.state.lock().expect("state").settings.clone()
    }

    pub fn set_settings(&self, settings: AppSettings) -> Result<()> {
        write_json(&self.0.data_dir.join("settings.json"), &settings)?;
        self.0.state.lock().expect("state").settings = settings;
        Ok(())
    }

    /// Adds files and folders. A folder's videos are planned together; loose files are grouped
    /// by their folder. Files already waiting in the queue are not added twice.
    pub fn add(&self, paths: &[PathBuf], options: Map<String, Value>) -> Result<usize> {
        let groups = add_groups(paths, &self.settings().output_dir)?;

        let mut added = 0;
        let mut to_plan = Vec::new();
        {
            let mut st = self.0.state.lock().expect("state");
            if let Some(Value::String(c)) = options.get("Content")
                && let Ok(p) = c.parse()
            {
                st.settings.last_profile = p;
            }
            for (input, files) in groups {
                let group = self.next_id("g");
                let mut planned = Vec::new();
                for f in files {
                    let waiting = st.jobs.iter().any(|j| {
                        j.file == f.path && !matches!(j.state, JobState::Done | JobState::Skipped)
                    });
                    if waiting {
                        continue;
                    }
                    st.jobs.push(Job {
                        rip: None,
                        id: self.next_id("j"),
                        name: f.name.clone(),
                        file: f.path.clone(),
                        group: group.clone(),
                        input: input.clone(),
                        options: options.clone(),
                        state: JobState::Analysing,
                        profile: None,
                        route: None,
                        plan: Vec::new(),
                        stage: None,
                        progress: None,
                        output: None,
                        message: None,
                        log: None,
                        source_bytes: file_size(&f.path),
                        output_bytes: None,
                        planned_output: None,
                        vmaf: None,
                    });
                    planned.push(f);
                    added += 1;
                }
                if !planned.is_empty() {
                    to_plan.push((group, input, planned));
                }
            }
            let _ = write_json(&self.0.data_dir.join("settings.json"), &st.settings);
        }
        self.changed();
        for (group, input, files) in to_plan {
            self.spawn_plan(group, input, files, options.clone());
        }
        Ok(added)
    }

    fn replan_unfinished(&self) {
        let mut groups: Vec<Replan> = Vec::new();
        {
            let mut st = self.0.state.lock().expect("state");
            for j in &mut st.jobs {
                if !matches!(
                    j.state,
                    JobState::Analysing | JobState::Queued | JobState::Interrupted
                ) {
                    continue;
                }
                if j.state == JobState::Queued {
                    j.state = JobState::Analysing;
                }
                match groups.iter_mut().find(|g| g.0 == j.group) {
                    Some(g) => g.3.push(j.file.clone()),
                    None => groups.push((
                        j.group.clone(),
                        j.input.clone(),
                        j.options.clone(),
                        vec![j.file.clone()],
                    )),
                }
            }
        }
        for (group, input, options, files) in groups {
            let files: Vec<VideoFile> = files
                .iter()
                .filter_map(|f| VideoFile::from_path(f).ok())
                .collect();
            self.spawn_plan(group, input, files, options);
        }
    }

    /// Plans a group in the background; its jobs become Queued (or Failed with the reason).
    /// When no season consensus is needed ([`batch::plans_independently`]), episodes are planned
    /// one at a time and each is queued as soon as its own plan is ready, so the first can start
    /// while the rest are still being analysed.
    fn spawn_plan(
        &self,
        group: String,
        input: PathBuf,
        files: Vec<VideoFile>,
        options: Map<String, Value>,
    ) {
        let engine = self.clone();
        self.0.runtime.spawn_blocking(move || {
            let out_dir = engine.settings().output_dir;
            let _ = std::fs::create_dir_all(out_dir.join("logs"));
            let log = FileLog::create(&engine.0.data_dir.join("app.log"))
                .unwrap_or_else(|_| FileLog::stderr_only());
            let settings = engine.settings();
            let runner = SystemRunner;
            let probe = match tools::set::ensure_probe(
                &engine.installer(&runner, &log, &settings, false),
            ) {
                Ok(probe) => probe,
                Err(e) => {
                    let paths: Vec<PathBuf> = files.iter().map(|f| f.path.clone()).collect();
                    return engine.apply_plan(&group, &paths, Err(e));
                }
            };
            let prepare = |files: Vec<VideoFile>| {
                batch::prepare_files(
                    &input,
                    files,
                    &options,
                    &out_dir,
                    &runner,
                    &engine.0.gpu,
                    &probe,
                    &log,
                )
            };
            let one_by_one = batch::resolve_settings(&input, &options)
                .is_ok_and(|r| batch::plans_independently(&r));
            if one_by_one {
                for f in files {
                    // Removed from the queue while earlier episodes were being planned
                    if !engine.has_job(&group, &f.path) {
                        continue;
                    }
                    let path = f.path.clone();
                    engine.apply_plan(&group, &[path], prepare(vec![f]));
                }
            } else {
                let paths: Vec<PathBuf> = files.iter().map(|f| f.path.clone()).collect();
                engine.apply_plan(&group, &paths, prepare(files));
            }
        });
    }

    fn has_job(&self, group: &str, file: &Path) -> bool {
        let st = self.0.state.lock().expect("state");
        st.jobs.iter().any(|j| j.group == group && j.file == file)
    }

    /// Stores a planned batch (merged into the group's, which may already hold earlier
    /// episodes) and marks the jobs for `paths` Queued, or Failed with the reason. The plan is
    /// stored first, so the run loop never finds a queued job without one.
    fn apply_plan(&self, group: &str, paths: &[PathBuf], result: Result<Batch>) {
        (self.0.notify)(EngineEvent::Status {
            status: String::new(),
        });
        if let Ok(b) = &result {
            let mut batches = self.0.batches.lock().expect("batches");
            let merged = match batches.get(group) {
                Some(old) => merge_batch(old, b.clone()),
                None => b.clone(),
            };
            batches.insert(group.to_string(), Arc::new(merged));
        }
        {
            let mut st = self.0.state.lock().expect("state");
            let out_dir = st.settings.output_dir.clone();
            let mut queued = Vec::new();
            for (i, j) in st
                .jobs
                .iter_mut()
                .enumerate()
                .filter(|(_, j)| j.group == group && paths.contains(&j.file))
            {
                match &result {
                    Ok(b) => {
                        if let Some(p) = b.plans.iter().find(|p| p.file.path == j.file) {
                            j.profile = Some(b.resolved.settings.content);
                            j.route = Some(route_label(p));
                            j.plan = file_report(b, p);
                            j.planned_output = planned_output(b, p, &out_dir);
                            // Under the Source line: where it's from and where it goes
                            if let Some(line) = saves_to_line(p, j.planned_output.as_deref()) {
                                j.plan.insert(2.min(j.plan.len()), line);
                            }
                            if j.state == JobState::Analysing {
                                j.state = JobState::Queued;
                                queued.push(i);
                            }
                        }
                    }
                    Err(e) => {
                        j.state = JobState::Failed;
                        j.message = Some(e.to_string());
                    }
                }
            }
            // Last first, so of two new files with one name the earlier one stays queued
            for i in queued.into_iter().rev() {
                Self::fail_on_clash(&mut st.jobs, i);
            }
        }
        self.changed();
    }

    /// Fails `jobs[i]` with the reason when another job already has its output file.
    fn fail_on_clash(jobs: &mut [Job], i: usize) {
        let Some(other) = name_clash(jobs, i) else {
            return;
        };
        let out = jobs[i].planned_output.clone().unwrap_or_default();
        let message = clash_message(other, &out);
        jobs[i].state = JobState::Failed;
        jobs[i].message = Some(message);
    }

    /// Puts every failed and cancelled job back in the queue.
    pub fn retry_failed(&self) {
        let ids: Vec<String> = {
            let st = self.0.state.lock().expect("state");
            st.jobs
                .iter()
                .filter(|j| matches!(j.state, JobState::Failed | JobState::Cancelled))
                .map(|j| j.id.clone())
                .collect()
        };
        for id in ids {
            self.retry(&id);
        }
    }

    /// Removes every failed and cancelled job.
    pub fn clear_failed(&self) {
        self.0
            .state
            .lock()
            .expect("state")
            .jobs
            .retain(|j| !matches!(j.state, JobState::Failed | JobState::Cancelled));
        self.changed();
    }

    /// Gives waiting jobs new options (the Add sheet's, when a job is edited) and plans them
    /// again. Jobs that are running, ripping or being analysed are left alone, and so are
    /// finished ones. A ripped title keeps where it goes and that its raw rip is deleted.
    /// Returns how many jobs changed.
    pub fn set_options(&self, ids: &[String], options: Map<String, Value>) -> usize {
        const RIP_KEYS: [&str; 3] = ["OutputDir", "DeleteSource", "SourceIsRip"];
        let mut replan: Vec<Replan> = Vec::new();
        {
            let mut st = self.0.state.lock().expect("state");
            // Jobs that were planned together stay together, in a group of their own: a
            // group has one set of options
            let mut regroup: Vec<(String, String)> = Vec::new();
            for j in st.jobs.iter_mut().filter(|j| {
                ids.contains(&j.id)
                    && matches!(
                        j.state,
                        JobState::Queued
                            | JobState::Failed
                            | JobState::Cancelled
                            | JobState::Interrupted
                            | JobState::Skipped
                    )
            }) {
                let mut new = options.clone();
                if j.options.get("SourceIsRip") == Some(&Value::Bool(true)) {
                    for key in RIP_KEYS {
                        if let Some(v) = j.options.get(key) {
                            new.insert(key.into(), v.clone());
                        }
                    }
                }
                let group = match regroup.iter().find(|(old, _)| *old == j.group) {
                    Some((_, new)) => new.clone(),
                    None => {
                        let new = self.next_id("g");
                        regroup.push((j.group.clone(), new.clone()));
                        new
                    }
                };
                j.group = group.clone();
                j.options = new.clone();
                j.state = JobState::Analysing;
                j.profile = None;
                j.route = None;
                j.plan = Vec::new();
                j.message = None;
                j.progress = None;
                j.stage = None;
                j.planned_output = None;
                match replan.iter_mut().find(|g| g.0 == group) {
                    Some(g) => g.3.push(j.file.clone()),
                    None => replan.push((group, j.input.clone(), new, vec![j.file.clone()])),
                }
            }
        }
        let changed = replan.iter().map(|g| g.3.len()).sum();
        self.changed();
        for (group, input, options, files) in replan {
            let files: Vec<VideoFile> = files
                .iter()
                .filter_map(|f| VideoFile::from_path(f).ok())
                .collect();
            self.spawn_plan(group, input, files, options);
        }
        changed
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        {
            let mut st = self.0.state.lock().expect("state");
            if st
                .jobs
                .iter()
                .any(|j| j.id == id && j.state == JobState::Running)
            {
                return Err(Error::Tool(
                    "Cancel the running job before removing it.".into(),
                ));
            }
            // A title that's ripping stops; the next one on the disc starts
            if let Some(rip) = self.0.rip.lock().expect("rip").as_mut()
                && rip.job.as_deref() == Some(id)
            {
                rip.skip = true;
                if let Some(child) = rip.child.as_mut() {
                    let _ = child.kill();
                }
            }
            st.jobs.retain(|j| j.id != id);
        }
        self.changed();
        Ok(())
    }

    /// Moves a job to position `index` (drag and drop in the queue).
    pub fn move_job_to(&self, id: &str, index: usize) {
        {
            let mut st = self.0.state.lock().expect("state");
            if let Some(i) = st.jobs.iter().position(|j| j.id == id) {
                let job = st.jobs.remove(i);
                let to = index.min(st.jobs.len());
                st.jobs.insert(to, job);
            }
        }
        self.changed();
    }

    /// Moves a job `delta` places up (negative) or down.
    pub fn move_job(&self, id: &str, delta: i64) {
        {
            let mut st = self.0.state.lock().expect("state");
            if let Some(i) = st.jobs.iter().position(|j| j.id == id) {
                let to = (i as i64 + delta).clamp(0, st.jobs.len() as i64 - 1) as usize;
                let job = st.jobs.remove(i);
                st.jobs.insert(to, job);
            }
        }
        self.changed();
    }

    /// Puts a finished, failed or cancelled job back in the queue.
    pub fn retry(&self, id: &str) {
        let has_plan = |g: &str| self.0.batches.lock().expect("batches").contains_key(g);
        let job = self.update_job(id, |j| {
            if !matches!(j.state, JobState::Running | JobState::Analysing) {
                j.state = JobState::Queued;
                j.message = None;
                j.progress = None;
                j.stage = None;
            }
        });
        {
            // Still the same name as another job: it would only be skipped
            let mut st = self.0.state.lock().expect("state");
            if let Some(i) = st
                .jobs
                .iter()
                .position(|j| j.id == id && j.state == JobState::Queued)
            {
                Self::fail_on_clash(&mut st.jobs, i);
            }
        }
        if let Some(j) = job
            && !has_plan(&j.group)
        {
            self.update_job(id, |j| j.state = JobState::Analysing);
            if let Ok(f) = VideoFile::from_path(&j.file) {
                self.spawn_plan(j.group.clone(), j.input.clone(), vec![f], j.options.clone());
            }
        }
        self.changed();
    }

    /// Removes finished and skipped jobs.
    pub fn clear_finished(&self) {
        self.0
            .state
            .lock()
            .expect("state")
            .jobs
            .retain(|j| !matches!(j.state, JobState::Done | JobState::Skipped));
        self.changed();
    }

    fn control(&self) -> Option<Arc<Control>> {
        self.0.state.lock().expect("state").control.clone()
    }

    pub fn pause(&self) {
        if let Some(c) = self.control() {
            c.pause();
        }
        self.changed();
    }

    pub fn resume(&self) {
        if let Some(c) = self.control() {
            c.resume();
        }
        self.changed();
    }

    pub fn stop_after_current(&self, on: bool) {
        if let Some(c) = self.control() {
            c.stop_after_current(on);
        }
        self.changed();
    }

    pub fn cancel(&self) {
        if let Some(c) = self.control() {
            c.cancel();
        }
        self.changed();
    }

    /// A job's log, or the log of the same name in the current output folder's `logs` when the
    /// output folder was moved (with its logs and comparisons) after the job ran.
    fn current_log(&self, log: Option<PathBuf>) -> Option<PathBuf> {
        let log = log?;
        if log.is_file() {
            return Some(log);
        }
        let moved = self
            .settings()
            .output_dir
            .join("logs")
            .join(log.file_name()?);
        Some(if moved.is_file() { moved } else { log })
    }

    /// The comparison images of a finished job (`compare/<name>_NN.png`), in order.
    pub fn compare_images(&self, id: &str) -> Vec<PathBuf> {
        let job = self
            .0
            .state
            .lock()
            .expect("state")
            .jobs
            .iter()
            .find(|j| j.id == id)
            .map(|j| (j.output.clone(), j.log.clone()));
        let Some((Some(out), log)) = job else {
            return Vec::new();
        };
        let log = self.current_log(log);
        // The images are in the output folder's `compare`, next to `logs/<name>.log`. With
        // ReplaceSource the finished file is back in the source's folder, so it can't tell
        // where they are; older jobs without a log path fall back to it.
        let place = log
            .as_deref()
            .and_then(|l| Some((l.parent()?.parent()?, l.file_stem()?)))
            .or_else(|| Some((out.parent()?, out.file_stem()?)));
        let Some((dir, stem)) = place else {
            return Vec::new();
        };
        let prefix = format!("{}_", stem.to_string_lossy());
        let mut images: Vec<PathBuf> = std::fs::read_dir(dir.join("compare"))
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| {
                        let name = p
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        name.strip_prefix(&prefix)
                            .and_then(|rest| rest.strip_suffix(".png"))
                            .is_some_and(|n| n.len() == 2 && n.bytes().all(|b| b.is_ascii_digit()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        images.sort();
        images
    }

    /// The last `lines` lines of a job's log.
    pub fn job_log(&self, id: &str, lines: usize) -> String {
        let path = self
            .0
            .state
            .lock()
            .expect("state")
            .jobs
            .iter()
            .find(|j| j.id == id)
            .and_then(|j| j.log.clone());
        let Some(text) = self.current_log(path).and_then(|p| read_tail(&p, LOG_TAIL)) else {
            return String::new();
        };
        let text = String::from_utf8_lossy(&text);
        // Encoder progress uses \r; show only the latest state of each line
        let clean: Vec<&str> = text
            .lines()
            .map(|l| l.rsplit('\r').next().unwrap_or(l))
            .collect();
        clean[clean.len().saturating_sub(lines)..].join("\n")
    }

    /// Tool versions and paths, without installing anything.
    pub fn tools_status(&self) -> Vec<ToolStatus> {
        let s = self.settings();
        let (topaz, _) = self.queued_topaz();
        tools::set::inspect(
            &SystemRunner,
            &s.tools_dir,
            &self.0.gpu,
            topaz.as_deref(),
            s.system_tools(),
        )
        .0
    }

    /// Installs and updates every tool, then runs the self-tests. Returns the log.
    pub fn update_tools(&self) -> Result<Vec<String>> {
        let s = self.settings();
        std::fs::create_dir_all(&s.tools_dir)?;
        let log = MemoryLog::default();
        let runner = SystemRunner;
        let result = (|| -> Result<()> {
            let inst = self.installer(&runner, &log, &s, true);
            let set = tools::set::ensure(
                &inst,
                Needs {
                    gpu: &self.0.gpu,
                    mlrt: tools::mlrt::AVAILABLE,
                    deband: false,
                    handbrake: true,
                },
            )?;
            let vpy = find_scripts_dir()
                .ok_or_else(|| Error::Tool("upscale.vpy not found".into()))?
                .join("upscale.vpy");
            if let Some(mlrt) = &set.mlrt {
                tools::mlrt::selftest(
                    &runner,
                    &set.vspipe,
                    &vpy,
                    self.0.gpu.default_backend(),
                    mlrt,
                    &set.mlrt_root,
                    &log,
                )?;
            }
            let (topaz_dir, filter) = self.queued_topaz();
            if let Some(dir) = topaz_dir.filter(|d| tools::topaz::is_installed(d)) {
                let tested = tools::topaz::init_topaz(&runner, &dir, &log)
                    .and_then(|t| tools::topaz::selftest(&runner, &t, &filter, &log));
                if let Err(e) = tested {
                    log.warn(&format!("Topaz (used for CG films): {e}"));
                }
            }
            // Ripping: a failure here doesn't stop encoding tools from updating
            match tools::makemkv::ensure(&inst, s.makemkv_approved) {
                Err(e) if !s.makemkv_approved => log.info(&format!("  {e}")),
                Err(e) => log.warn(&format!("MakeMKV (for ripping discs): {e}")),
                Ok(_) => {}
            }
            log.info("All tools are installed and up to date.");
            Ok(())
        })();
        let mut lines = log.0.into_inner().expect("log");
        if let Err(e) = result {
            lines.push(format!("ERROR: {e}"));
        }
        Ok(lines)
    }

    /// MakeMKV's registration as last seen (`makemkv.json`): Unknown until a disc was read.
    pub fn makemkv_status(&self) -> tools::makemkv::Registration {
        read_json(&self.0.data_dir.join("makemkv.json"))
            .unwrap_or(tools::makemkv::Registration::Unknown)
    }

    fn save_makemkv_status(&self, status: &tools::makemkv::Registration) -> Result<()> {
        write_json(&self.0.data_dir.join("makemkv.json"), status)
    }

    /// Reads the disc in the first drive to learn MakeMKV's trial or registration. Without a
    /// disc the status is unchanged.
    pub fn check_makemkv(&self) -> Result<tools::makemkv::Registration> {
        let con = tools::makemkv::find_makemkvcon()
            .ok_or_else(|| Error::Tool("MakeMKV isn't installed.".into()))?;
        let args: Vec<std::ffi::OsString> =
            ["-r", "--cache=1", "info", "disc:0"].map(Into::into).into();
        let out = SystemRunner.output(&con, &args)?;
        match tools::makemkv::registration_from_output(&out.stdout) {
            Some(mut status) => {
                // A disc read can't tell when the key was entered; keep the date we know
                if let (
                    tools::makemkv::Registration::Registered { since },
                    tools::makemkv::Registration::Registered { since: known },
                ) = (&mut status, self.makemkv_status())
                {
                    *since = since.take().or(known);
                }
                self.save_makemkv_status(&status)?;
                Ok(status)
            }
            None => Err(Error::Tool(
                "Insert a disc so MakeMKV can report its trial or registration.".into(),
            )),
        }
    }

    /// Settings → Ripping → Register: the user's key goes to MakeMKV (never stored here).
    pub fn register_makemkv(&self, key: &str) -> Result<Vec<String>> {
        let con = tools::makemkv::find_makemkvcon()
            .ok_or_else(|| Error::Tool("Install MakeMKV first.".into()))?;
        let msgs = tools::makemkv::register(&SystemRunner, &con, key, &tools::default_work_dir())?;
        self.save_makemkv_status(&tools::makemkv::Registration::Registered {
            since: Some(chrono::Local::now().format("%Y-%m-%d").to_string()),
        })?;
        Ok(msgs)
    }

    /// Settings → Ripping → Install MakeMKV, after the user confirmed: records the approval,
    /// then installs (or updates) MakeMKV. Returns the log.
    pub fn install_makemkv(&self) -> Result<Vec<String>> {
        let mut s = self.settings();
        if !s.makemkv_approved {
            s.makemkv_approved = true;
            self.set_settings(s.clone())?;
        }
        std::fs::create_dir_all(&s.tools_dir)?;
        let log = MemoryLog::default();
        let runner = SystemRunner;
        let inst = self.installer(&runner, &log, &s, true);
        let result = tools::makemkv::ensure(&inst, true);
        let mut lines = log.0.into_inner().expect("log");
        match result {
            Ok(con) => lines.push(format!("MakeMKV is ready: {}", con.display())),
            Err(e) => lines.push(format!("ERROR: {e}")),
        }
        Ok(lines)
    }

    fn set_status(&self, status: &str) {
        self.0.state.lock().expect("state").status = status.into();
        self.changed();
    }

    /// The disc in the drive, if MakeMKV has read one (for a window that opens later).
    pub fn current_disc(&self) -> Option<crate::disc::Disc> {
        self.0.disc.lock().expect("disc").clone()
    }

    /// Watches for Blu-rays and DVDs: every few seconds a cheap
    /// look at the mounted volumes; a new disc is read with MakeMKV once and sent as
    /// `EngineEvent::Disc`, an ejected one as `DiscRemoved`. A disc already in the drive
    /// when the app starts counts as new.
    pub fn start_disc_watch(&self) {
        let engine = self.clone();
        std::thread::spawn(move || {
            let mut seen: Vec<PathBuf> = Vec::new();
            loop {
                let discs = crate::drives::video_discs(&SystemRunner);
                if seen.iter().any(|p| !discs.iter().any(|d| &d.path == p)) {
                    seen.retain(|p| discs.iter().any(|d| &d.path == p));
                    if seen.is_empty() && engine.0.disc.lock().expect("disc").take().is_some() {
                        (engine.0.notify)(EngineEvent::DiscRemoved);
                    }
                }
                let new: Vec<&crate::drives::DiscVolume> =
                    discs.iter().filter(|d| !seen.contains(&d.path)).collect();
                for d in new {
                    seen.push(d.path.clone());
                    engine.read_disc(d);
                }
                std::thread::sleep(Duration::from_secs(3));
            }
        });
    }

    /// Rips the titles one at a time. Each title is a row in the queue
    /// from the start (Waiting to rip, Ripping with its progress), and once ripped the same row
    /// is planned and encoded, with the queue started. The disc is ejected when every title
    /// ripped. Refused while another rip runs, when MakeMKV may not rip, or when the work
    /// folder is short of space.
    pub fn rip(&self, request: crate::rip::RipRequest) -> Result<()> {
        if !self.makemkv_status().can_rip() {
            return Err(Error::Tool(
                "Register MakeMKV in Settings → Ripping to rip discs.".into(),
            ));
        }
        let con = tools::makemkv::find_makemkvcon()
            .ok_or_else(|| Error::Tool("MakeMKV isn't installed.".into()))?;
        if request.titles.is_empty() {
            return Err(Error::Tool("Tick at least one title.".into()));
        }
        let settings = self.settings();
        let rips = settings.rips_dir();
        std::fs::create_dir_all(&rips)?;
        let need: u64 = request.titles.iter().map(|t| t.bytes).sum::<u64>() + (2 << 30);
        if request.rip_only {
            // The raw rips go to the library: none may be there already, and its drive needs room
            if let Some(t) = request.titles.iter().find(|t| t.target.exists()) {
                return Err(Error::Tool(format!(
                    "{} already exists; remove it first or pick other names.",
                    t.target.display()
                )));
            }
            if let Some(dir) = request.titles.first().and_then(|t| t.target.parent()) {
                std::fs::create_dir_all(dir)?;
                if let Some(free) = crate::disk::free_space(dir)
                    && free < need
                {
                    return Err(Error::Tool(format!(
                        "Not enough space in {} for the rips: {} GB needed, {} GB free.",
                        dir.display(),
                        need >> 30,
                        free >> 30
                    )));
                }
            }
        }
        if let Some(free) = crate::disk::free_space(&rips)
            && free < need
        {
            return Err(Error::Tool(format!(
                "Not enough space in {} to rip: {} GB needed, {} GB free.",
                rips.display(),
                need >> 30,
                free >> 30
            )));
        }
        {
            let mut rip = self.0.rip.lock().expect("rip");
            if rip.is_some() {
                return Err(Error::Tool("A disc is already being ripped.".into()));
            }
            *rip = Some(Rip::default());
        }
        let disc = self
            .current_disc()
            .map(|d| d.name)
            .unwrap_or_else(|| request.source.clone());
        let logs = settings.output_dir.join("logs");
        let _ = std::fs::create_dir_all(&logs);
        // One row per title, in ripping order
        let mut ids = Vec::new();
        {
            let mut st = self.0.state.lock().expect("state");
            if let Some(Value::String(c)) = request.options.get("Content")
                && let Ok(p) = c.parse()
            {
                st.settings.last_profile = p;
            }
            for t in &request.titles {
                let named = crate::rip::named_rip(&rips, &t.target);
                let id = self.next_id("j");
                let stem = named
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                st.jobs.push(Job {
                    rip: Some(crate::rip::RipProgress::new(
                        &request.source,
                        t.index,
                        &disc,
                        t.bytes,
                    )),
                    id: id.clone(),
                    name: named
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    file: named,
                    group: self.next_id("g"),
                    input: rips.clone(),
                    options: if request.rip_only {
                        crate::rip::rip_only_options(&request.options)
                    } else {
                        crate::rip::job_options(&request.options, &t.target)
                    },
                    state: JobState::WaitingToRip,
                    profile: None,
                    route: Some(if request.rip_only {
                        format!("Rip only: title {} from {disc}", t.index)
                    } else {
                        format!("Rip title {} from {disc}", t.index)
                    }),
                    plan: Vec::new(),
                    stage: None,
                    progress: None,
                    output: None,
                    message: None,
                    log: Some(logs.join(format!("{stem}.rip.log"))),
                    source_bytes: Some(t.bytes).filter(|b| *b > 0),
                    output_bytes: None,
                    // Where it ends up in the library (the raw rip with Rip only, else the encode)
                    planned_output: Some(t.target.clone()),
                    vmaf: None,
                });
                ids.push(id);
            }
        }
        self.changed();
        let engine = self.clone();
        std::thread::spawn(move || {
            let n = ids.len();
            let mut ripped = 0;
            for (i, id) in ids.iter().enumerate() {
                if engine.rip_cancelled() {
                    break;
                }
                // Removed from the queue while it waited
                let Some(job) = engine.job(id).filter(|j| j.state == JobState::WaitingToRip) else {
                    continue;
                };
                let label = format!("{} ({}/{n})", job.name, i + 1);
                let ripped_to = engine.rip_title(&con, &rips, &job, &label);
                let ripped_to = match (ripped_to, request.rip_only) {
                    (Ok(path), true) => {
                        let target = &request.titles[i].target;
                        match engine.place_rip(&job, &path, target) {
                            Ok(()) => {
                                ripped += 1;
                                engine.update_job(id, |j| {
                                    j.state = JobState::Done;
                                    j.rip = None;
                                    j.file = target.clone();
                                    j.input = target.parent().map(Path::to_path_buf).unwrap_or_default();
                                    j.output = Some(target.clone());
                                    j.source_bytes = file_size(target);
                                    j.message = Some(
                                        "Ripped only: add its folder on the computer that encodes (Replace original), or Re-run it here."
                                            .into(),
                                    );
                                });
                                engine.changed();
                                continue;
                            }
                            Err(e) => Err(e),
                        }
                    }
                    (result, _) => result,
                };
                match ripped_to {
                    Ok(path) => {
                        ripped += 1;
                        let size = file_size(&path);
                        engine.update_job(id, |j| {
                            j.state = JobState::Analysing;
                            j.rip = None;
                            j.message = None;
                            j.source_bytes = size;
                        });
                        engine.changed();
                        if let Ok(f) = VideoFile::from_path(&path) {
                            engine.spawn_plan(
                                job.group.clone(),
                                job.input.clone(),
                                vec![f],
                                job.options.clone(),
                            );
                        }
                        engine.start();
                    }
                    Err(why) => {
                        let cancelled = engine.rip_cancelled();
                        engine.update_job(id, |j| {
                            j.rip = None;
                            j.state = if cancelled {
                                JobState::Cancelled
                            } else {
                                JobState::Failed
                            };
                            j.message = Some(if cancelled {
                                "cancelled".into()
                            } else {
                                why.to_string()
                            });
                        });
                        engine.changed();
                    }
                }
            }
            let cancelled = engine.rip_cancelled();
            // Titles that never started because Cancel rip stopped the rest
            {
                let mut st = engine.0.state.lock().expect("state");
                for j in st
                    .jobs
                    .iter_mut()
                    .filter(|j| ids.contains(&j.id) && j.state == JobState::WaitingToRip)
                {
                    j.state = JobState::Cancelled;
                    j.rip = None;
                    j.message = Some("cancelled".into());
                }
            }
            *engine.0.rip.lock().expect("rip") = None;
            if !cancelled && ripped == n {
                if let Some(dev) = crate::rip::eject_device(&request.source) {
                    let args: Vec<std::ffi::OsString> = vec!["eject".into(), dev.into()];
                    let _ = SystemRunner.output(Path::new("/usr/sbin/diskutil"), &args);
                } else if let Some(drive) = crate::rip::windows_drive(&request.source)
                    && let Ok(powershell) = which::which("powershell")
                {
                    let _ =
                        SystemRunner.output(&powershell, &crate::rip::windows_eject_args(&drive));
                }
                engine.set_status(&format!("Ripped {n} of {n} titles; the disc was ejected."));
            } else {
                engine.set_status(&format!("Ripped {ripped} of {n} titles."));
            }
        });
        Ok(())
    }

    fn job(&self, id: &str) -> Option<Job> {
        self.0
            .state
            .lock()
            .expect("state")
            .jobs
            .iter()
            .find(|j| j.id == id)
            .cloned()
    }

    fn rip_cancelled(&self) -> bool {
        self.0
            .rip
            .lock()
            .expect("rip")
            .as_ref()
            .is_some_and(|r| r.cancelled)
    }

    /// Cancel rip: stops the running title (its partial file is deleted) and every title still
    /// waiting.
    pub fn cancel_rip(&self) {
        if let Some(rip) = self.0.rip.lock().expect("rip").as_mut() {
            rip.cancelled = true;
            if let Some(child) = rip.child.as_mut() {
                let _ = child.kill();
            }
        }
    }

    /// Rip only: moves a finished rip from the rip folder to its library place, trying again
    /// when a network folder drops, with each step in the title's log. On failure the rip stays
    /// in the rip folder and the error says where.
    fn place_rip(&self, job: &Job, ripped: &Path, target: &Path) -> Result<()> {
        use std::io::Write;
        let mut log = job.log.as_deref().and_then(|p| {
            std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(p)
                .ok()
        });
        let mut note = |line: &str| {
            if let Some(f) = log.as_mut() {
                let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
                let _ = writeln!(f, "[{stamp}] {line}");
            }
        };
        note(&format!("Rip only: moving it to {} ...", target.display()));
        self.update_job(&job.id, |j| {
            j.message = Some(format!("Saving to {} ...", target.display()))
        });
        self.changed();
        let moved = crate::transfer::move_file_blocking(
            ripped,
            target,
            &crate::transfer::OUTPUT_RETRY_WAITS,
            |e, wait| {
                note(&format!(
                    "Couldn't write it ({e}); trying again in {} s.",
                    wait.as_secs()
                ))
            },
        );
        match moved {
            Ok(()) => {
                note(&format!("Saved: {}", target.display()));
                Ok(())
            }
            Err(e) => {
                let why = format!(
                    "couldn't save the rip to {} ({e}); it's still at {}",
                    target.display(),
                    ripped.display()
                );
                note(&format!("Failed: {why}"));
                Err(Error::Tool(why))
            }
        }
    }

    /// One title: `makemkvcon mkv` into its own folder, with its progress on its queue row and
    /// MakeMKV's messages in its log, then renamed to its Jellyfin name (`job.file`).
    fn rip_title(&self, con: &Path, rips: &Path, job: &Job, label: &str) -> Result<PathBuf> {
        use std::io::{BufRead, Write};
        let rip = job
            .rip
            .clone()
            .ok_or_else(|| Error::Tool("not a disc title".into()))?;
        let dir = crate::rip::title_dir(rips, rip.index);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        if job.file.exists() {
            return Err(Error::Tool(format!(
                "{} already exists; remove it first",
                job.file.display()
            )));
        }
        let mut log = job
            .log
            .as_deref()
            .and_then(|p| std::fs::File::create(p).ok());
        let mut note = |line: &str| {
            if let Some(f) = log.as_mut() {
                let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
                let _ = writeln!(f, "[{stamp}] {line}");
            }
        };
        note(&format!(
            "Ripping title {} from {} ({}) to {}",
            rip.index,
            rip.disc,
            rip.source,
            dir.display()
        ));
        self.update_job(&job.id, |j| j.state = JobState::Ripping);
        self.changed();
        let mut child = std::process::Command::new(con)
            .args(crate::rip::rip_args(&rip.source, rip.index, &dir))
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        let stdout = child.stdout.take().expect("piped");
        if let Some(r) = self.0.rip.lock().expect("rip").as_mut() {
            r.child = Some(child);
            r.job = Some(job.id.clone());
            r.skip = false;
        }
        let started = std::time::Instant::now();
        let mut last = std::time::Instant::now() - Duration::from_secs(10);
        let mut messages = Vec::new();
        for line in std::io::BufReader::new(stdout)
            .lines()
            .map_while(std::io::Result::ok)
        {
            if let Some(f) = crate::rip::progress(&line) {
                if last.elapsed() >= Duration::from_secs(1) {
                    last = std::time::Instant::now();
                    let p = rip.at(f, started.elapsed());
                    self.update_job(&job.id, |j| j.rip = Some(p.clone()));
                    if let Some(j) = self.job(&job.id) {
                        (self.0.notify)(EngineEvent::Job { job: Box::new(j) });
                    }
                    self.set_status(&format!("Ripping {label}: {:.0}%", f * 100.0));
                }
            } else if line.starts_with("MSG:") {
                for m in tools::makemkv::messages(&line) {
                    note(&m);
                }
                messages.push(line);
            }
        }
        let (status, skipped) = {
            let mut r = self.0.rip.lock().expect("rip");
            let r = r.as_mut();
            let skipped = r.as_ref().is_some_and(|r| r.skip);
            let status = r.and_then(|r| {
                r.job = None;
                r.child.take()
            });
            (status.map(|mut c| c.wait()), skipped)
        };
        self.set_status("");
        let ok = matches!(status, Some(Ok(s)) if s.success());
        let file = crate::rip::find_rip(&dir);
        if self.rip_cancelled() || skipped || !ok || file.is_none() {
            let _ = std::fs::remove_dir_all(&dir);
            if self.rip_cancelled() || skipped {
                note("Cancelled.");
                return Err(Error::Tool("cancelled".into()));
            }
            let why = tools::makemkv::messages(&messages.join("\n"))
                .pop()
                .unwrap_or_else(|| "MakeMKV failed".into());
            note(&format!("Failed: {why}"));
            return Err(Error::Tool(why));
        }
        std::fs::rename(file.expect("checked"), &job.file)?;
        let _ = std::fs::remove_dir_all(&dir);
        note(&format!(
            "Ripped in {} min: {}",
            started.elapsed().as_secs() / 60,
            job.file.display()
        ));
        Ok(job.file.clone())
    }

    fn read_disc(&self, volume: &crate::drives::DiscVolume) {
        let Some(con) = tools::makemkv::find_makemkvcon() else {
            self.set_status(&format!(
                "Disc {}: install MakeMKV in Settings → Ripping to rip it.",
                volume.label
            ));
            return;
        };
        self.set_status(&format!("Reading disc {} ...", volume.label));
        let args: Vec<std::ffi::OsString> = ["-r", "--cache=1", "info", &volume.source]
            .map(Into::into)
            .into();
        let out = SystemRunner.output(&con, &args);
        self.set_status("");
        let Ok(out) = out else {
            self.set_status(&format!(
                "Couldn't read disc {} with MakeMKV.",
                volume.label
            ));
            return;
        };
        if let Some(status) = tools::makemkv::registration_from_output(&out.stdout) {
            let status = match (status, self.makemkv_status()) {
                (
                    tools::makemkv::Registration::Registered { since: None },
                    tools::makemkv::Registration::Registered { since },
                ) => tools::makemkv::Registration::Registered { since },
                (s, _) => s,
            };
            let _ = self.save_makemkv_status(&status);
        }
        let mut disc = tools::makemkv::parse_info(&out.stdout, &volume.source);
        if disc.name.is_empty() {
            disc.name = volume.label.clone();
        }
        if disc.titles.is_empty() {
            let why = tools::makemkv::messages(&out.stdout)
                .pop()
                .unwrap_or_else(|| "no titles found".into());
            self.set_status(&format!("Disc {}: {why}", volume.label));
            return;
        }
        *self.0.disc.lock().expect("disc") = Some(disc.clone());
        (self.0.notify)(EngineEvent::Disc {
            disc: Box::new(disc),
        });
    }

    /// Starts working through the queue (no-op when already running).
    pub fn start(&self) {
        let control = {
            let mut st = self.0.state.lock().expect("state");
            if st.running {
                return;
            }
            st.running = true;
            let c = Control::new();
            st.control = Some(c.clone());
            c
        };
        self.changed();
        let engine = self.clone();
        self.0.runtime.spawn(async move {
            engine.run_loop(control).await;
            {
                let mut st = engine.0.state.lock().expect("state");
                st.running = false;
                st.control = None;
            }
            engine.changed();
        });
    }

    /// The settings of each group with jobs still to run. A group still being analysed has no
    /// batch yet; its settings files decide.
    fn queued_settings(&self) -> Vec<crate::settings::Resolved> {
        let st = self.0.state.lock().expect("state");
        let b = self.0.batches.lock().expect("batches");
        let mut groups: Vec<&str> = Vec::new();
        st.jobs
            .iter()
            .filter(|j| j.state.runnable() || j.state == JobState::Analysing)
            .filter(|j| {
                let new = !groups.contains(&j.group.as_str());
                if new {
                    groups.push(&j.group);
                }
                new
            })
            .filter_map(|j| match b.get(&j.group) {
                Some(batch) => Some(batch.resolved.clone()),
                None => batch::resolve_settings(&j.input, &j.options).ok(),
            })
            .collect()
    }

    /// The Topaz folder and filter of the first queued Topaz job (its `TopazPath` and
    /// `TopazFilter`), else the default install and the default filter.
    fn queued_topaz(&self) -> (Option<PathBuf>, String) {
        match self
            .queued_settings()
            .into_iter()
            .find(|r| r.settings.use_topaz())
        {
            Some(r) => (r.settings.topaz_dir(), r.settings.topaz_filter),
            None => (
                crate::settings::Settings::default().topaz_dir(),
                crate::settings::DEFAULT_TOPAZ_FILTER.to_string(),
            ),
        }
    }

    /// Episodes skipped because another computer had claimed them: finished ones get their
    /// output file, released or abandoned ones go back in the queue (the claim of a crashed
    /// machine goes stale after `claim::STALE_AFTER`). Returns (requeued, still being encoded).
    fn recheck_claimed(&self, settings: &AppSettings) -> (usize, usize) {
        let host = host_name();
        let (mut requeued, mut waiting, mut changed) = (0, 0, false);
        {
            let mut st = self.0.state.lock().expect("state");
            let batches = self.0.batches.lock().expect("batches");
            let claimed = st.jobs.iter_mut().filter(|j| {
                j.state == JobState::Skipped
                    && j.message
                        .as_deref()
                        .is_some_and(|m| m.starts_with(BUSY_PREFIX))
            });
            for j in claimed {
                let Some(batch) = batches.get(&j.group) else {
                    continue;
                };
                let Some(plan) = batch.plans.iter().find(|p| p.file.path == j.file) else {
                    continue;
                };
                let base = output_base(batch, plan);
                let mkv = crate::pipeline::final_dir(batch, &settings.output_dir)
                    .join(format!("{base}.mkv"));
                if mkv.is_file() {
                    let by = j.message.as_deref().unwrap_or_default();
                    j.message = Some(format!("encoded on {}", &by[BUSY_PREFIX.len()..]));
                    j.output_bytes = file_size(&mkv);
                    j.output = Some(mkv);
                    changed = true;
                } else if held_elsewhere(&settings.output_dir, &base, &host).is_some() {
                    waiting += 1;
                } else {
                    j.state = JobState::Queued;
                    j.message = None;
                    requeued += 1;
                    changed = true;
                }
            }
        }
        if changed {
            self.changed();
        }
        (requeued, waiting)
    }

    /// Tools for the queued jobs, with the self-tests they need (before any job starts).
    fn prepare_tools(&self, settings: &AppSettings) -> Result<(ToolSet, PathBuf, Prepared)> {
        let resolved = self.queued_settings();
        let topaz = resolved.iter().any(|r| r.settings.use_topaz());
        let vs = resolved.iter().any(|r| r.settings.uses_vs_mlrt()) || resolved.is_empty();
        let deband = resolved.iter().any(|r| r.settings.deband);
        let runner = SystemRunner;
        std::fs::create_dir_all(&settings.tools_dir)?;
        let log = FileLog::create(&self.0.data_dir.join("app.log"))
            .unwrap_or_else(|_| FileLog::stderr_only());
        let inst = self.installer(&runner, &log, settings, false);
        let mut set = tools::set::ensure(
            &inst,
            Needs {
                gpu: &self.0.gpu,
                // Only the vs-mlrt route needs it (about 4 GB; no macOS build)
                mlrt: vs,
                deband,
                handbrake: false,
            },
        )?;
        let vpy = find_scripts_dir()
            .ok_or_else(|| Error::Tool("upscale.vpy not found".into()))?
            .join("upscale.vpy");
        if vs {
            let mlrt = set.mlrt.clone().unwrap_or_default();
            tools::mlrt::selftest(
                &runner,
                &set.vspipe,
                &vpy,
                self.0.gpu.default_backend(),
                &mlrt,
                &set.mlrt_root,
                &log,
            )?;
        }
        if topaz {
            let first = resolved.iter().find(|r| r.settings.use_topaz());
            let dir = first
                .and_then(|r| r.settings.topaz_dir())
                .ok_or_else(|| Error::Tool("no Topaz folder".into()))?;
            let filter = first
                .map(|r| r.settings.topaz_filter.clone())
                .unwrap_or_default();
            let t = tools::topaz::init_topaz(&runner, &dir, &log)?;
            tools::topaz::selftest(&runner, &t, &filter, &log)?;
            set.topaz = Some(t);
        }
        let prepared = Prepared {
            topaz,
            vs_mlrt: vs,
            deband,
        };
        Ok((set, vpy, prepared))
    }

    async fn run_loop(&self, control: Arc<Control>) {
        let settings = self.settings();
        self.set_status("Checking tools ...");
        let engine = self.clone();
        let s2 = settings.clone();
        let prepared = tokio::task::spawn_blocking(move || engine.prepare_tools(&s2)).await;
        let (mut tools, mut vpy, mut prepared) = match prepared {
            Ok(Ok(t)) => t,
            Ok(Err(e)) => return self.set_status(&format!("Tools: {e}")),
            Err(e) => return self.set_status(&format!("Tools: {e}")),
        };
        self.set_status("");
        // No sleep while jobs run (an overnight batch stalls if the computer sleeps)
        let _awake = crate::platform::KeepAwake::start();

        loop {
            if control.is_cancelled() || control.stopping() {
                break;
            }
            let next = {
                let st = self.0.state.lock().expect("state");
                let batches = self.0.batches.lock().expect("batches");
                let ready = st
                    .jobs
                    .iter()
                    .find(|j| j.state.runnable() && batches.contains_key(&j.group))
                    .map(|j| (j.id.clone(), j.file.clone(), batches[&j.group].clone()));
                (
                    ready,
                    st.jobs.iter().any(|j| j.state == JobState::Analysing),
                )
            };
            let (id, file, group_batch) = match next {
                (Some(n), _) => n,
                (None, true) => {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
                (None, false) => {
                    // Nothing left here: look again at the episodes another machine had
                    match self.recheck_claimed(&settings) {
                        (0, 0) => break,
                        (requeued, _) if requeued > 0 => continue,
                        (_, waiting) => {
                            self.set_status(&format!(
                                "Waiting for another computer to finish {waiting} episode{}",
                                if waiting == 1 { "" } else { "s" }
                            ));
                            tokio::select! {
                                _ = tokio::time::sleep(CLAIM_RECHECK) => {}
                                _ = control.wait_cancelled() => {}
                            }
                            self.set_status("");
                            continue;
                        }
                    }
                }
            };
            let Some(plan) = group_batch
                .plans
                .iter()
                .find(|p| p.file.path == file)
                .cloned()
            else {
                self.update_job(&id, |j| {
                    j.state = JobState::Failed;
                    j.message = Some("no plan for this file".into());
                });
                self.changed();
                continue;
            };
            // A job added (or ripped) after the queue started may need tools this run never
            // set up, e.g. Topaz in a queue that began with encode-only jobs: set them up now
            let need = Prepared::needed_by(&group_batch.resolved.settings);
            if !prepared.covers(need) {
                self.set_status("Checking tools ...");
                let engine = self.clone();
                let s2 = settings.clone();
                let again = tokio::task::spawn_blocking(move || engine.prepare_tools(&s2)).await;
                self.set_status("");
                let failed = match again {
                    Ok(Ok((t, v, p))) if p.covers(need) => {
                        (tools, vpy, prepared) = (t, v, p);
                        None
                    }
                    Ok(Ok(_)) => Some("the tools this job needs couldn't be set up".to_string()),
                    Ok(Err(e)) => Some(e.to_string()),
                    Err(e) => Some(e.to_string()),
                };
                if let Some(why) = failed {
                    self.update_job(&id, |j| {
                        j.state = JobState::Failed;
                        j.message = Some(format!("Tools: {why}"));
                    });
                    self.changed();
                    continue;
                }
            }
            let mut job_batch = (*group_batch).clone();
            job_batch.out_dir = settings.output_dir.clone();
            let log_path = job_batch
                .out_dir
                .join("logs")
                .join(format!("{}.log", output_base(&job_batch, &plan)));
            job_batch.plans = vec![plan];
            let restart = self
                .0
                .state
                .lock()
                .expect("state")
                .jobs
                .iter()
                .any(|j| j.id == id && j.state == JobState::Interrupted);
            self.update_job(&id, |j| {
                j.state = JobState::Running;
                j.message = None;
                j.progress = None;
                j.output = None;
                j.output_bytes = None;
                j.vmaf = None;
                j.log = Some(log_path);
            });
            self.changed();

            let outcome = self
                .run_one(
                    &id,
                    &job_batch,
                    tools.clone(),
                    vpy.clone(),
                    control.clone(),
                    restart,
                )
                .await;
            self.update_job(&id, |j| {
                j.stage = None;
                match outcome {
                    Ok((state, output, message)) => {
                        j.state = state;
                        j.output_bytes = output.as_deref().and_then(file_size);
                        j.output = output;
                        j.message = message;
                    }
                    Err(e) => {
                        j.state = JobState::Failed;
                        j.message = Some(e.to_string());
                    }
                }
            });
            self.changed();
        }
    }

    async fn run_one(
        &self,
        id: &str,
        b: &Batch,
        tools: ToolSet,
        vpy: PathBuf,
        control: Arc<Control>,
        restart: bool,
    ) -> Result<(JobState, Option<PathBuf>, Option<String>)> {
        std::fs::create_dir_all(b.out_dir.join("logs"))?;
        let log = FileLog::create(&b.out_dir.join("logs").join(format!(
            "batch-{}.log",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        )))?;
        let runner = SystemRunner;
        let settings = self.settings();
        let installer = self.installer(&runner, &log, &settings, false);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
        let engine = self.clone();
        let job_id = id.to_string();
        let forward = tokio::spawn(async move {
            let mut last = (None, None, None);
            while let Some(e) = rx.recv().await {
                engine.apply_event(&job_id, e, &mut last);
            }
            last
        });
        let mut env = RunEnv {
            batch: b,
            tools,
            vpy,
            runner: &runner,
            installer: &installer,
            log: &log,
            events: tx,
            control,
            keep_ivf: false,
            temp_dir: b
                .plans
                .first()
                .map(|p| settings.work_dir(&output_base(b, p)))
                .unwrap_or_else(tools::default_work_dir),
            restart,
            parallel_chunks: settings.parallel_chunks,
        };
        env.run_batch().await;
        drop(env);
        let (state, output, message) = forward.await.unwrap_or_default();
        Ok((state.unwrap_or(JobState::Failed), output, message))
    }

    /// Applies a pipeline event to the running job. `last` collects how it ended.
    fn apply_event(&self, id: &str, e: Event, last: &mut Ending) {
        let job = match e {
            Event::Log { line, .. } => {
                (self.0.notify)(EngineEvent::Log {
                    job_id: id.into(),
                    line,
                });
                return;
            }
            Event::Stage { stage, .. } => self.update_job(id, |j| {
                j.stage = Some(stage);
                j.progress = None;
            }),
            Event::Progress {
                frames_done,
                frames_total,
                fps,
                eta_seconds,
                projected_bytes,
                paused,
                chunk,
                ..
            } => self.update_job(id, |j| {
                j.progress = Some(JobProgress {
                    frames_done,
                    frames_total,
                    fps,
                    eta_seconds,
                    projected_bytes,
                    paused,
                    chunk,
                });
            }),
            Event::Quality { vmaf, .. } => self.update_job(id, |j| j.vmaf = Some(vmaf)),
            Event::JobDone { output, .. } => {
                *last = (Some(JobState::Done), Some(output), None);
                return;
            }
            Event::JobSkipped { reason, .. } => {
                *last = (Some(JobState::Skipped), None, Some(reason));
                return;
            }
            Event::JobFailed { reason, .. } => {
                let state = if reason == "cancelled" {
                    JobState::Cancelled
                } else {
                    JobState::Failed
                };
                *last = (Some(state), None, Some(reason));
                return;
            }
            Event::JobStarted { .. } | Event::BatchDone { .. } => return,
        };
        if let Some(job) = job {
            (self.0.notify)(EngineEvent::Job { job: Box::new(job) });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(dir: &Path) -> (Engine, Arc<Mutex<Vec<EngineEvent>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let e = Engine::new(
            dir.to_path_buf(),
            Arc::new(move |ev| s.lock().unwrap().push(ev)),
        )
        .unwrap();
        (e, seen)
    }

    /// A batch for group "g" with a (skipped, already 4K AV1) plan for each `ids` file.
    fn planned(ids: &[&str]) -> Batch {
        let resolved = crate::settings::resolve(&Map::new(), &[]).unwrap();
        let plans = ids
            .iter()
            .map(|id| {
                let file = VideoFile {
                    path: PathBuf::from(format!("{id}.mkv")),
                    name: format!("{id}.mkv"),
                    size: 1,
                    mtime_ticks: 0,
                };
                let src = crate::probe::SourceInfo {
                    codec: "av1".into(),
                    width: 3840,
                    height: 2160,
                    ..Default::default()
                };
                crate::plan::plan_file(
                    &resolved,
                    crate::settings::Model::Cugan,
                    file,
                    src,
                    &crate::analyze::NoAnalysis,
                )
            })
            .collect();
        Batch {
            input: PathBuf::from("."),
            out_dir: PathBuf::from("."),
            resolved,
            model: crate::settings::Model::Cugan,
            onnx: None,
            model_label: String::new(),
            backend: crate::settings::Backend::Trt,
            gpu: GpuChoice::default(),
            deband_available: false,
            plans,
        }
    }

    #[test]
    fn each_episode_is_queued_as_soon_as_its_plan_is_ready() {
        let dir = tempfile::tempdir().unwrap();
        // Saved as Done: Analysing jobs in queue.json would be replanned with real tools
        let jobs = vec![job("a", JobState::Done), job("b", JobState::Done)];
        write_json(&dir.path().join("queue.json"), &jobs).unwrap();
        let (e, _) = engine(dir.path());
        for id in ["a", "b"] {
            e.update_job(id, |j| j.state = JobState::Analysing);
        }
        let states = |e: &Engine| {
            e.snapshot()
                .jobs
                .iter()
                .map(|j| j.state)
                .collect::<Vec<_>>()
        };
        let planned_files = |e: &Engine| {
            e.0.batches.lock().unwrap()["g"]
                .plans
                .iter()
                .map(|p| p.file.name.clone())
                .collect::<Vec<_>>()
        };

        e.apply_plan("g", &[PathBuf::from("a.mkv")], Ok(planned(&["a"])));
        assert_eq!(states(&e), [JobState::Queued, JobState::Analysing]);
        assert_eq!(planned_files(&e), ["a.mkv"]);

        e.apply_plan("g", &[PathBuf::from("b.mkv")], Ok(planned(&["b"])));
        assert_eq!(states(&e), [JobState::Queued, JobState::Queued]);
        assert_eq!(
            planned_files(&e),
            ["a.mkv", "b.mkv"],
            "earlier plans are kept"
        );

        // A replan replaces that file's plan rather than adding a second one
        e.apply_plan("g", &[PathBuf::from("a.mkv")], Ok(planned(&["a"])));
        assert_eq!(planned_files(&e), ["b.mkv", "a.mkv"]);

        e.apply_plan(
            "g",
            &[PathBuf::from("b.mkv")],
            Err(Error::Tool("probe failed".into())),
        );
        let snap = e.snapshot();
        assert_eq!(snap.jobs[1].state, JobState::Failed);
        assert_eq!(
            snap.jobs[1].message.as_deref(),
            Some(Error::Tool("probe failed".into()).to_string().as_str())
        );
        assert!(e.has_job("g", Path::new("a.mkv")) && !e.has_job("g", Path::new("c.mkv")));
    }

    #[test]
    fn episodes_claimed_elsewhere_are_checked_again() {
        let dir = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let jobs = vec![
            job("a", JobState::Done),
            job("b", JobState::Done),
            job("c", JobState::Done),
        ];
        write_json(&dir.path().join("queue.json"), &jobs).unwrap();
        let (e, _) = engine(dir.path());
        let files: Vec<PathBuf> = ["a.mkv", "b.mkv", "c.mkv"].map(PathBuf::from).into();
        e.apply_plan("g", &files, Ok(planned(&["a", "b", "c"])));
        for id in ["a", "b", "c"] {
            e.update_job(id, |j| {
                j.state = JobState::Skipped;
                j.message = Some(format!("{BUSY_PREFIX}MAC-MINI"));
            });
        }
        // a: finished on the other machine; b: still being encoded there; c: released
        std::fs::write(out.path().join("a.mkv"), b"mkv").unwrap();
        std::mem::forget(
            crate::claim::Claim::acquire(out.path(), "b", "MAC-MINI")
                .unwrap()
                .unwrap(),
        );
        let s = AppSettings {
            output_dir: out.path().into(),
            ..AppSettings::default()
        };
        assert_eq!(e.recheck_claimed(&s), (1, 1));
        let snap = e.snapshot();
        assert_eq!(snap.jobs[0].state, JobState::Skipped);
        assert_eq!(snap.jobs[0].message.as_deref(), Some("encoded on MAC-MINI"));
        assert_eq!(snap.jobs[0].output_bytes, Some(3));
        assert_eq!(snap.jobs[1].state, JobState::Skipped, "still waiting");
        assert_eq!(snap.jobs[2].state, JobState::Queued, "back in the queue");
        assert_eq!(
            e.recheck_claimed(&s),
            (0, 1),
            "nothing changes while b runs"
        );
    }

    fn job(id: &str, state: JobState) -> Job {
        Job {
            rip: None,
            id: id.into(),
            file: PathBuf::from(format!("{id}.mkv")),
            name: format!("{id}.mkv"),
            group: "g".into(),
            input: PathBuf::from("."),
            options: Map::new(),
            state,
            profile: None,
            route: None,
            plan: vec![],
            stage: Some(Stage::Video),
            progress: Some(JobProgress::default()),
            output: None,
            message: None,
            log: None,
            source_bytes: None,
            output_bytes: None,
            planned_output: None,
            vmaf: None,
        }
    }

    #[test]
    fn a_restart_marks_running_jobs_interrupted() {
        let dir = tempfile::tempdir().unwrap();
        write_json(
            &dir.path().join("queue.json"),
            &vec![job("a", JobState::Running), job("b", JobState::Done)],
        )
        .unwrap();
        let (e, _) = engine(dir.path());
        let snap = e.snapshot();
        assert_eq!(snap.jobs[0].state, JobState::Interrupted);
        assert_eq!(
            snap.jobs[0].message.as_deref(),
            Some("Interrupted, will restart from the beginning")
        );
        assert!(snap.jobs[0].stage.is_none() && snap.jobs[0].progress.is_none());
        assert_eq!(snap.jobs[1].state, JobState::Done);
    }

    #[test]
    fn comparisons_are_found_after_the_source_was_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("Upscaled");
        let season = dir.path().join("Show").join("Season 1");
        std::fs::create_dir_all(out.join("compare")).unwrap();
        std::fs::create_dir_all(out.join("logs")).unwrap();
        std::fs::create_dir_all(season.join("compare")).unwrap();
        for name in [
            "ep1_02.png",
            "ep1_01.png",
            "ep10_01.png",
            "ep1_1.png",
            "ep2_01.png",
        ] {
            std::fs::write(out.join("compare").join(name), b"").unwrap();
        }
        std::fs::write(season.join("compare").join("ep2_01.png"), b"").unwrap();
        // ReplaceSource: the finished file took the source's place, the images stayed in out
        let mut replaced = job("ep1", JobState::Done);
        replaced.output = Some(season.join("ep1.mkv"));
        replaced.log = Some(out.join("logs").join("ep1.log"));
        // An older queue entry without a log path still looks next to its output
        let mut older = job("ep2", JobState::Done);
        older.output = Some(season.join("ep2.mkv"));
        write_json(&dir.path().join("queue.json"), &vec![replaced, older]).unwrap();
        let (e, _) = engine(dir.path());
        assert_eq!(
            e.compare_images("ep1"),
            vec![
                out.join("compare/ep1_01.png"),
                out.join("compare/ep1_02.png")
            ]
        );
        assert_eq!(
            e.compare_images("ep2"),
            vec![season.join("compare/ep2_01.png")]
        );
    }

    #[test]
    fn logs_and_comparisons_follow_a_moved_output_folder() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("Old");
        let new = dir.path().join("New");
        std::fs::create_dir_all(new.join("logs")).unwrap();
        std::fs::create_dir_all(new.join("compare")).unwrap();
        std::fs::write(new.join("logs").join("ep1.log"), "moved log").unwrap();
        std::fs::write(new.join("compare").join("ep1_01.png"), b"").unwrap();
        let mut done = job("ep1", JobState::Done);
        done.output = Some(dir.path().join("Show").join("ep1.mkv"));
        done.log = Some(old.join("logs").join("ep1.log"));
        write_json(&dir.path().join("queue.json"), &vec![done]).unwrap();
        let (e, _) = engine(dir.path());
        let mut s = e.settings();
        s.output_dir = new.clone();
        e.set_settings(s).unwrap();
        assert_eq!(e.job_log("ep1", 10), "moved log");
        assert_eq!(
            e.compare_images("ep1"),
            vec![new.join("compare/ep1_01.png")]
        );
    }

    #[test]
    fn a_rip_cut_off_by_closing_the_app_fails_clearly() {
        let dir = tempfile::tempdir().unwrap();
        let mut ripping = job("a", JobState::Ripping);
        ripping.rip = Some(crate::rip::RipProgress::new("dev:/dev/rdisk8", 2, "D", 100));
        let waiting = job("b", JobState::WaitingToRip);
        write_json(&dir.path().join("queue.json"), &vec![ripping, waiting]).unwrap();
        let (e, _) = engine(dir.path());
        let snap = e.snapshot();
        for j in &snap.jobs {
            assert_eq!(j.state, JobState::Failed);
            assert!(j.rip.is_none());
            assert!(j.message.as_deref().unwrap().contains("rip the disc again"));
        }
        // Neither is something the encoder would pick up
        assert!(!JobState::WaitingToRip.runnable() && !JobState::Ripping.runnable());
    }

    #[test]
    fn dragging_moves_a_job_to_its_new_place() {
        let dir = tempfile::tempdir().unwrap();
        let jobs: Vec<Job> = ["a", "b", "c", "d"]
            .iter()
            .map(|id| job(id, JobState::Queued))
            .collect();
        write_json(&dir.path().join("queue.json"), &jobs).unwrap();
        let (e, _) = engine(dir.path());
        let ids = |e: &Engine| {
            e.snapshot()
                .jobs
                .iter()
                .map(|j| j.id.clone())
                .collect::<Vec<_>>()
        };
        e.move_job_to("d", 0);
        assert_eq!(ids(&e), ["d", "a", "b", "c"]);
        e.move_job_to("d", 2);
        assert_eq!(ids(&e), ["a", "b", "d", "c"]);
        e.move_job_to("a", 99);
        assert_eq!(ids(&e), ["b", "d", "c", "a"], "past the end means last");
    }

    #[test]
    fn older_queues_get_file_sizes_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("ep1.mkv");
        let out = dir.path().join("ep1.out.mkv");
        std::fs::write(&src, vec![0u8; 300]).unwrap();
        std::fs::write(&out, vec![0u8; 100]).unwrap();
        let mut done = job("a", JobState::Done);
        done.file = src.clone();
        done.output = Some(out);
        let mut queued = job("b", JobState::Queued);
        queued.file = src;
        write_json(&dir.path().join("queue.json"), &vec![done, queued]).unwrap();
        let (e, _) = engine(dir.path());
        let snap = e.snapshot();
        assert_eq!(
            (snap.jobs[0].source_bytes, snap.jobs[0].output_bytes),
            (Some(300), Some(100))
        );
        assert_eq!(
            (snap.jobs[1].source_bytes, snap.jobs[1].output_bytes),
            (Some(300), None)
        );
    }

    #[test]
    fn reorder_remove_and_settings_persist() {
        let dir = tempfile::tempdir().unwrap();
        write_json(
            &dir.path().join("queue.json"),
            &vec![
                job("a", JobState::Done),
                job("b", JobState::Done),
                job("c", JobState::Failed),
            ],
        )
        .unwrap();
        let (e, seen) = engine(dir.path());
        e.move_job("c", -2);
        e.move_job("a", 5);
        let ids = |e: &Engine| {
            e.snapshot()
                .jobs
                .iter()
                .map(|j| j.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&e), ["c", "b", "a"]);
        e.remove("b").unwrap();
        e.clear_finished();
        assert_eq!(ids(&e), ["c"]);
        assert!(!seen.lock().unwrap().is_empty());

        let mut s = e.settings();
        assert_eq!(s.original_file, OriginalFile::Keep);
        s.output_dir = PathBuf::from(r"D:\h2tj");
        s.original_file = OriginalFile::Replace;
        e.set_settings(s).unwrap();
        drop(e);
        let (e, _) = engine(dir.path());
        assert_eq!(ids(&e), ["c"], "the queue survives a restart");
        assert_eq!(e.settings().output_dir, PathBuf::from(r"D:\h2tj"));
        assert_eq!(e.settings().original_file, OriginalFile::Replace);
    }

    #[test]
    fn older_settings_keep_the_original_file() {
        let s: AppSettings = serde_json::from_str(r#"{"outputDir": "D:\\h2tj"}"#).unwrap();
        assert_eq!(s.original_file, OriginalFile::Keep);
        let s: AppSettings = serde_json::from_str(r#"{"originalFile": "delete"}"#).unwrap();
        assert_eq!(s.original_file, OriginalFile::Delete);
        // 0.2 settings get the rip defaults
        assert_eq!(s.keep_languages, ["eng", "jpn", "spa"]);
        assert!(s.library_folders.is_empty() && s.tmdb_token.is_empty());
        assert!(
            !s.makemkv_approved,
            "MakeMKV is only installed after the user approves it"
        );
        let s: AppSettings = serde_json::from_str(
            r#"{"libraryFolders": [{"path": "/L/Anime Shows", "kind": "shows"}]}"#,
        )
        .unwrap();
        assert_eq!(s.library_folders[0].kind, LibraryKind::Shows);
        assert_eq!(
            s.library_folders[0].name, "",
            "folders saved before names had none"
        );
    }

    #[test]
    fn rips_go_to_the_rip_folder() {
        let mut s = AppSettings::default();
        assert_eq!(s.rips_dir(), tools::default_work_dir().join("rips"));
        s.temp_dir = Some(PathBuf::from("/Temp"));
        assert_eq!(s.rips_dir(), PathBuf::from("/Temp/rips"));
        s.rip_dir = Some(PathBuf::from("/Big Drive/Rips"));
        assert_eq!(s.rips_dir(), PathBuf::from("/Big Drive/Rips"));
        s.rip_dir = Some(PathBuf::new());
        assert_eq!(
            s.rips_dir(),
            PathBuf::from("/Temp/rips"),
            "an empty setting means the default"
        );
    }

    #[test]
    fn work_dir_is_local_unless_a_job_left_chunks_in_the_output() {
        let out = tempfile::tempdir().unwrap();
        let mut s = AppSettings {
            output_dir: out.path().into(),
            ..AppSettings::default()
        };
        assert_eq!(s.work_dir("ep1"), tools::default_work_dir());
        std::fs::create_dir_all(out.path().join(".work").join("ep1.work")).unwrap();
        assert_eq!(
            s.work_dir("ep1"),
            out.path().join(".work"),
            "an interrupted job resumes from the old default"
        );
        assert_eq!(s.work_dir("ep2"), tools::default_work_dir());
        s.temp_dir = Some(PathBuf::from(r"D:\n9pd"));
        assert_eq!(s.work_dir("ep1"), PathBuf::from(r"D:\n9pd"));
    }

    #[test]
    fn progress_events_update_the_job() {
        let dir = tempfile::tempdir().unwrap();
        write_json(
            &dir.path().join("queue.json"),
            &vec![job("a", JobState::Done)],
        )
        .unwrap();
        let (e, seen) = engine(dir.path());
        let mut last = (None, None, None);
        e.apply_event(
            "a",
            Event::Progress {
                job_id: String::new(),
                frames_done: 5,
                frames_total: 10,
                fps: 2.0,
                eta_seconds: Some(3),
                projected_bytes: None,
                paused: true,
                chunk: Some((2, 3)),
            },
            &mut last,
        );
        assert_eq!(
            e.snapshot().jobs[0].progress.as_ref().unwrap().frames_done,
            5
        );
        e.apply_event(
            "a",
            Event::JobFailed {
                job_id: String::new(),
                reason: "cancelled".into(),
            },
            &mut last,
        );
        assert_eq!(last.0, Some(JobState::Cancelled));
        assert!(matches!(
            seen.lock().unwrap().last(),
            Some(EngineEvent::Job { .. })
        ));
    }

    #[test]
    fn an_unreadable_file_is_kept_and_the_save_before_it_restored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.json");
        write_json(&path, &vec![1, 2]).unwrap();
        write_json(&path, &vec![1, 2, 3]).unwrap();
        assert_eq!(read_json::<Vec<u32>>(&path), Some(vec![1, 2, 3]));
        assert_eq!(
            parse_json::<Vec<u32>>(&backup_path(&path)),
            Some(vec![1, 2]),
            "each save keeps the one before it"
        );
        // A power cut can leave the file empty
        std::fs::write(&path, b"").unwrap();
        assert_eq!(read_json::<Vec<u32>>(&path), Some(vec![1, 2]));
        let (restored, note) = recover_json::<Vec<u32>>(&path);
        assert_eq!(restored, Some(vec![1, 2]));
        assert!(note.unwrap().contains("queue.json couldn't be read"));
        assert!(dir.path().join("queue.json.bad").is_file());
        assert!(!path.exists(), "the next save can't overwrite the evidence");
        // The next save leaves the good backup alone until there's a newer good file
        write_json(&path, &vec![9]).unwrap();
        assert_eq!(
            parse_json::<Vec<u32>>(&backup_path(&path)),
            Some(vec![1, 2])
        );
        // A file that was never there is not a loss
        let fresh = recover_json::<Vec<u32>>(&dir.path().join("settings.json"));
        assert_eq!(fresh, (None, None));
    }

    #[test]
    fn the_log_tab_reads_only_the_end_of_a_long_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ep.log");
        let lines: Vec<String> = (0..1000).map(|i| format!("line {i:04}")).collect();
        std::fs::write(&path, lines.join("\n")).unwrap();
        let all = read_tail(&path, 1 << 20).unwrap();
        assert_eq!(all.len(), 1000 * 10 - 1, "a short log is read whole");
        let tail = String::from_utf8(read_tail(&path, 35).unwrap()).unwrap();
        assert_eq!(tail, "line 0997\nline 0998\nline 0999", "from a line start");
        assert_eq!(read_tail(&dir.path().join("none.log"), 35), None);
    }

    #[test]
    fn the_engine_says_when_the_queue_was_lost() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("queue.json"), b"[{\"half\": ").unwrap();
        let (e, _) = engine(dir.path());
        let snap = e.snapshot();
        assert!(snap.jobs.is_empty());
        assert!(
            snap.status.contains("queue.json couldn't be read"),
            "{}",
            snap.status
        );
        assert!(dir.path().join("queue.json.bad").is_file());
        let log = std::fs::read_to_string(dir.path().join("app.log")).unwrap();
        assert!(log.contains("WARNING: queue.json couldn't be read"));
    }

    #[test]
    fn a_job_that_needs_more_tools_than_the_run_set_up_is_noticed() {
        let settings = |content: &str| {
            let mut o = Map::new();
            o.insert("Content".into(), Value::String(content.into()));
            crate::settings::resolve(&o, &[]).unwrap().settings
        };
        // A queue that started with encode-only jobs set up neither Topaz nor vs-mlrt
        let encode_only = Prepared::needed_by(&settings("anime-av1"));
        assert_eq!(encode_only, Prepared::default());
        let topaz = Prepared::needed_by(&settings("anime-topaz"));
        assert!(topaz.topaz && !topaz.vs_mlrt);
        assert!(Prepared::needed_by(&settings("anime")).vs_mlrt);
        assert!(!encode_only.covers(topaz), "Topaz must be set up first");
        assert!(topaz.covers(topaz) && topaz.covers(encode_only));
    }

    /// A plan for a 1080p file at `path` that will be upscaled into the output folder.
    fn upscale_plan(path: &str, options: &Map<String, Value>) -> Batch {
        let mut b = planned(&[]);
        b.resolved = crate::settings::resolve(options, &[]).unwrap();
        let path = PathBuf::from(path);
        let file = VideoFile {
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            path,
            size: 1,
            mtime_ticks: 0,
        };
        let src = crate::probe::SourceInfo {
            codec: "h264".into(),
            width: 1920,
            height: 1080,
            frame_rate: 24.0,
            ..Default::default()
        };
        b.plans = vec![crate::plan::plan_file(
            &b.resolved,
            b.model,
            file,
            src,
            &crate::analyze::NoAnalysis,
        )];
        b
    }

    /// An engine with Analysing jobs `(id, group, file)`.
    fn analysing(dir: &Path, jobs: &[(&str, &str, &str)]) -> Engine {
        let saved: Vec<Job> = jobs
            .iter()
            .map(|(id, group, file)| {
                let mut j = job(id, JobState::Done);
                j.group = (*group).into();
                j.file = PathBuf::from(file);
                j.name = j.file.file_name().unwrap().to_string_lossy().into_owned();
                j
            })
            .collect();
        write_json(&dir.join("queue.json"), &saved).unwrap();
        let (e, _) = engine(dir);
        for (id, _, _) in jobs {
            e.update_job(id, |j| j.state = JobState::Analysing);
        }
        e
    }

    #[test]
    fn two_files_with_one_output_name_are_not_both_queued() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b, c) = ("ShowA/01.mkv", "ShowB/01.mkv", "ShowB/02.mkv");
        let e = analysing(
            dir.path(),
            &[("a", "ga", a), ("b", "gb", b), ("c", "gb", c)],
        );
        let none = Map::new();
        e.apply_plan("ga", &[a.into()], Ok(upscale_plan(a, &none)));
        e.apply_plan("gb", &[b.into()], Ok(upscale_plan(b, &none)));
        e.apply_plan("gb", &[c.into()], Ok(upscale_plan(c, &none)));
        let snap = e.snapshot();
        let states: Vec<JobState> = snap.jobs.iter().map(|j| j.state).collect();
        assert_eq!(
            states,
            [JobState::Queued, JobState::Failed, JobState::Queued]
        );
        let why = snap.jobs[1].message.clone().unwrap();
        assert!(why.starts_with("Same output file as \"01.mkv\""), "{why}");
        // Retrying doesn't get around it while the other job still has the name
        e.retry("b");
        assert_eq!(e.snapshot().jobs[1].state, JobState::Failed);
        // Once the other job is gone, it does
        e.remove("a").unwrap();
        e.retry("b");
        assert_eq!(e.snapshot().jobs[0].state, JobState::Queued);
    }

    #[test]
    fn files_that_replace_their_source_may_share_a_name() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = ("ShowA/01.mkv", "ShowB/01.mkv");
        let e = analysing(dir.path(), &[("a", "ga", a), ("b", "gb", b)]);
        let mut replace = Map::new();
        replace.insert("ReplaceSource".into(), Value::Bool(true));
        e.apply_plan("ga", &[a.into()], Ok(upscale_plan(a, &replace)));
        e.apply_plan("gb", &[b.into()], Ok(upscale_plan(b, &replace)));
        let snap = e.snapshot();
        assert!(snap.jobs.iter().all(|j| j.state == JobState::Queued));
        assert!(snap.jobs.iter().all(|j| j.planned_output.is_none()));
    }

    #[test]
    fn failed_jobs_are_retried_or_removed_together() {
        let dir = tempfile::tempdir().unwrap();
        let jobs = vec![
            job("a", JobState::Failed),
            job("b", JobState::Done),
            job("c", JobState::Cancelled),
        ];
        write_json(&dir.path().join("queue.json"), &jobs).unwrap();
        let (e, _) = engine(dir.path());
        e.0.batches
            .lock()
            .unwrap()
            .insert("g".into(), Arc::new(planned(&["a", "b", "c"])));
        e.retry_failed();
        let states: Vec<JobState> = e.snapshot().jobs.iter().map(|j| j.state).collect();
        assert_eq!(states, [JobState::Queued, JobState::Done, JobState::Queued]);
        e.update_job("a", |j| j.state = JobState::Failed);
        e.clear_failed();
        let ids: Vec<String> = e.snapshot().jobs.iter().map(|j| j.id.clone()).collect();
        assert_eq!(ids, ["b", "c"]);
    }

    #[test]
    fn a_folder_is_added_with_the_folders_below_it() {
        let dir = tempfile::tempdir().unwrap();
        let lib = tempfile::tempdir().unwrap();
        let show = lib.path().join("Show");
        for f in ["Season 1/e1.mkv", "Season 1/e2.mkv", "Season 2/e1.mkv"] {
            let path = show.join(f);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"x").unwrap();
        }
        // (Not through `Engine::add`, which would start planning with the real tools)
        let groups = add_groups(std::slice::from_ref(&show), dir.path()).unwrap();
        let shape: Vec<(String, usize)> = groups
            .iter()
            .map(|(d, files)| {
                let name = d.file_name().unwrap().to_string_lossy().into_owned();
                (name, files.len())
            })
            .collect();
        assert_eq!(
            shape,
            [("Season 1".to_string(), 2), ("Season 2".to_string(), 1)],
            "each season is a group of its own"
        );
        // A season and one of its files dropped together are still one group
        let both = [show.join("Season 2"), show.join("Season 1").join("e2.mkv")];
        let groups = add_groups(&both, dir.path()).unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[1].1.len(), 1);
    }

    #[test]
    fn a_measured_vmaf_lands_on_the_job_and_a_rerun_clears_it() {
        let dir = tempfile::tempdir().unwrap();
        write_json(
            &dir.path().join("queue.json"),
            &vec![job("a", JobState::Done)],
        )
        .unwrap();
        let (e, _) = engine(dir.path());
        let mut last = (None, None, None);
        e.apply_event(
            "a",
            Event::Quality {
                job_id: "a".into(),
                vmaf: 96.5,
            },
            &mut last,
        );
        assert_eq!(e.snapshot().jobs[0].vmaf, Some(96.5));
        let saved = serde_json::to_value(&e.snapshot().jobs[0]).unwrap();
        assert_eq!(saved["vmaf"], 96.5);
        assert!(
            AppSettings::default().parallel_chunks == 1,
            "one chunk at a time unless asked"
        );
    }
}
