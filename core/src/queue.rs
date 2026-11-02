//! The app's job queue (PLAN.md §4, Phase 2): jobs that survive a restart (`queue.json`), app
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
use crate::event::{Event, Stage};
use crate::files::{VideoFile, is_video, list_videos};
use crate::gpu::{GpuChoice, list_gpus, pick_gpu};
use crate::log::{FileLog, Log, MemoryLog};
use crate::pipeline::{Control, RunEnv, output_base};
use crate::plan::Route;
use crate::process::SystemRunner;
use crate::report::file_report;
use crate::tools::install::Installer;
use crate::tools::set::{Needs, ToolSet, ToolStatus};
use crate::tools::{self, find_scripts_dir};
use crate::{Error, ProfileId, Result};

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
}

impl AppSettings {
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
            use_system_tools: false,
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
}

/// The queue and the worker that plans and runs it. Cheap to clone.
#[derive(Clone)]
pub struct Engine(Arc<Inner>);

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Write then rename, so a crash never leaves half a file
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(value)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
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
        let settings: AppSettings = read_json(&data_dir.join("settings.json")).unwrap_or_default();
        let mut jobs: Vec<Job> = read_json(&data_dir.join("queue.json")).unwrap_or_default();
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
            if j.state == JobState::Running {
                j.state = JobState::Interrupted;
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
                status: String::new(),
            }),
            batches: Mutex::new(HashMap::new()),
            notify,
            gpu,
            runtime,
            seq: AtomicU64::new(0),
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
            system: settings.use_system_tools,
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
        }
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
        let mut groups: Vec<(PathBuf, Vec<VideoFile>)> = Vec::new();
        let mut loose: Vec<(PathBuf, VideoFile)> = Vec::new();
        for p in paths {
            let p = std::path::absolute(p)?;
            if p.is_dir() {
                let videos = list_videos(&p)?;
                if !videos.is_empty() {
                    groups.push((p, videos));
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
            for j in st
                .jobs
                .iter_mut()
                .filter(|j| j.group == group && paths.contains(&j.file))
            {
                match &result {
                    Ok(b) => {
                        if let Some(p) = b.plans.iter().find(|p| p.file.path == j.file) {
                            j.profile = Some(b.resolved.settings.content);
                            j.route = Some(route_label(p));
                            j.plan = file_report(b, p);
                            if j.state == JobState::Analysing {
                                j.state = JobState::Queued;
                            }
                        }
                    }
                    Err(e) => {
                        j.state = JobState::Failed;
                        j.message = Some(e.to_string());
                    }
                }
            }
        }
        self.changed();
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
            st.jobs.retain(|j| j.id != id);
        }
        self.changed();
        Ok(())
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

    /// The comparison images of a finished job (`compare/<name>_NN.png`), in order.
    pub fn compare_images(&self, id: &str) -> Vec<PathBuf> {
        let output = self
            .0
            .state
            .lock()
            .expect("state")
            .jobs
            .iter()
            .find(|j| j.id == id)
            .and_then(|j| j.output.clone());
        let Some(out) = output else {
            return Vec::new();
        };
        let (Some(dir), Some(stem)) = (out.parent(), out.file_stem()) else {
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
        let Some(text) = path.and_then(|p| std::fs::read(p).ok()) else {
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
        let topaz = crate::settings::Settings::default().topaz_dir();
        tools::set::inspect(
            &SystemRunner,
            &s.tools_dir,
            &self.0.gpu,
            topaz.as_deref(),
            s.use_system_tools,
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
            if let Some(dir) = crate::settings::Settings::default()
                .topaz_dir()
                .filter(|d| tools::topaz::is_installed(d))
            {
                let tested = tools::topaz::init_topaz(&runner, &dir, &log).and_then(|t| {
                    tools::topaz::selftest(&runner, &t, crate::settings::DEFAULT_TOPAZ_FILTER, &log)
                });
                if let Err(e) = tested {
                    log.warn(&format!("Topaz (used for CG films): {e}"));
                }
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

    fn set_status(&self, status: &str) {
        self.0.state.lock().expect("state").status = status.into();
        self.changed();
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

    /// Tools for the queued jobs, with the self-tests they need (before any job starts).
    fn prepare_tools(&self, settings: &AppSettings) -> Result<(ToolSet, PathBuf)> {
        // A group still being analysed has no batch yet; its settings decide the tools it needs
        let resolved: Vec<crate::settings::Resolved> = {
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
        };
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
        Ok((set, vpy))
    }

    async fn run_loop(&self, control: Arc<Control>) {
        let settings = self.settings();
        self.set_status("Checking tools ...");
        let engine = self.clone();
        let s2 = settings.clone();
        let prepared = tokio::task::spawn_blocking(move || engine.prepare_tools(&s2)).await;
        let (tools, vpy) = match prepared {
            Ok(Ok(t)) => t,
            Ok(Err(e)) => return self.set_status(&format!("Tools: {e}")),
            Err(e) => return self.set_status(&format!("Tools: {e}")),
        };
        self.set_status("");

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
                (None, false) => break,
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

    fn job(id: &str, state: JobState) -> Job {
        Job {
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
        s.output_dir = PathBuf::from(r"D:\Out");
        e.set_settings(s).unwrap();
        drop(e);
        let (e, _) = engine(dir.path());
        assert_eq!(ids(&e), ["c"], "the queue survives a restart");
        assert_eq!(e.settings().output_dir, PathBuf::from(r"D:\Out"));
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
        s.temp_dir = Some(PathBuf::from(r"D:\Temp"));
        assert_eq!(s.work_dir("ep1"), PathBuf::from(r"D:\Temp"));
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
}
