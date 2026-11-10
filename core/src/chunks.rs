//! Chunked, resumable encoding: an upscale is split at scene cuts into chunks
//! of about four minutes, each encoded to its own `.ivf` with a `.done` marker. After a crash
//! or a closed app, finished chunks are kept and the job continues with the next one; at the
//! end the chunks are joined losslessly with mkvmerge.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// Target chunk length.
pub const TARGET_SECS: f64 = 240.0;
/// HandBrake pieces are longer: fewer HandBrake starts per movie, at most ten minutes lost.
pub const HANDBRAKE_TARGET_SECS: f64 = 600.0;
/// How far from the target a scene cut may be.
pub const WINDOW_SECS: f64 = 60.0;
/// No chunk is shorter than this (the last one merges into the one before).
pub const MIN_SECS: f64 = 90.0;

static SCD_TIME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"lavfi\.scd\.time:\s*([\d.]+)").expect("valid regex"));

/// Scene-cut frames from ffmpeg `scdet` output (`lavfi.scd.time: T`), sorted and unique.
pub fn parse_scene_cuts(text: &str, fps: f64) -> Vec<u64> {
    let mut cuts: Vec<u64> = SCD_TIME
        .captures_iter(text)
        .filter_map(|c| c[1].parse::<f64>().ok())
        .map(|t| (t * fps).round() as u64)
        .filter(|f| *f > 0)
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    cuts
}

/// Splits `0..total` into chunks at scene cuts near every target length.
pub fn plan_chunks(total: u64, fps: f64, cuts: &[u64]) -> Vec<Range<u64>> {
    plan_chunks_of(total, fps, cuts, TARGET_SECS)
}

/// `plan_chunks` with another target length (HandBrake's ten minutes).
pub fn plan_chunks_of(total: u64, fps: f64, cuts: &[u64], target_secs: f64) -> Vec<Range<u64>> {
    let frames = |secs: f64| (secs * fps).round().max(1.0) as u64;
    let (target, window, min) = (frames(target_secs), frames(WINDOW_SECS), frames(MIN_SECS));
    let mut bounds = vec![0u64];
    let mut pos = 0u64;
    // Another cut only when what follows is long enough to stand on its own
    while total.saturating_sub(pos) > target + min {
        let want = pos + target;
        let lo = want.saturating_sub(window).max(pos + min);
        let hi = (want + window).min(total - min);
        let cut = cuts
            .iter()
            .copied()
            .filter(|c| (lo..=hi).contains(c))
            .min_by_key(|c| c.abs_diff(want))
            .unwrap_or(want);
        bounds.push(cut);
        pos = cut;
    }
    bounds.push(total);
    bounds
        .windows(2)
        .filter(|w| w[1] > w[0])
        .map(|w| w[0]..w[1])
        .collect()
}

/// Whether an encode of `frames` would split into at least two chunks (shorter ones are
/// encoded in one go: scene detection and a join would buy nothing).
pub fn worth_chunking(frames: u64, fps: f64) -> bool {
    worth_chunking_of(frames, fps, TARGET_SECS)
}

pub fn worth_chunking_of(frames: u64, fps: f64, target_secs: f64) -> bool {
    frames as f64 > (target_secs + MIN_SECS) * fps
}

/// FNV-1a, so keys stay the same across builds (std's hasher may change between releases).
pub fn fnv1a(parts: &[&[u8]]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for b in part.iter().chain(&[0u8]) {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

/// `chunks.json` in a job's work folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkState {
    /// Identifies the source, the plan and the encoder: a different key means start over.
    pub key: String,
    /// Frame ranges relative to the start of the encode.
    pub chunks: Vec<(u64, u64)>,
}

/// A job's work folder: `<temp>\<base>.work`.
pub struct WorkDir(pub PathBuf);

impl WorkDir {
    pub fn new(temp: &Path, base: &str) -> Self {
        Self(temp.join(format!("{base}.work")))
    }
    fn state_path(&self) -> PathBuf {
        self.0.join("chunks.json")
    }
    pub fn ivf(&self, i: usize) -> PathBuf {
        self.0.join(format!("chunk-{i:03}.ivf"))
    }
    /// A HandBrake piece (HandBrake writes MKV).
    pub fn mkv(&self, i: usize) -> PathBuf {
        self.0.join(format!("chunk-{i:03}.mkv"))
    }
    fn done_path(&self, i: usize) -> PathBuf {
        self.0.join(format!("chunk-{i:03}.done"))
    }

    /// The saved state, if its key matches.
    pub fn load(&self, key: &str) -> Option<ChunkState> {
        let state: ChunkState =
            serde_json::from_str(&std::fs::read_to_string(self.state_path()).ok()?).ok()?;
        (state.key == key).then_some(state)
    }

    /// Starts over with a new chunk list.
    pub fn reset(&self, state: &ChunkState) -> std::io::Result<()> {
        self.remove();
        std::fs::create_dir_all(&self.0)?;
        std::fs::write(self.state_path(), serde_json::to_string_pretty(state)?)
    }

    pub fn is_done(&self, i: usize) -> bool {
        self.done_path(i).is_file()
    }

    /// Marks chunk `i` finished. `file` (the chunk) is flushed to disk first and the marker
    /// records its size, so a marker never says more than what is really on the disk.
    pub fn mark_done(&self, i: usize, file: &Path) -> std::io::Result<()> {
        use std::io::Write;
        let chunk = std::fs::OpenOptions::new().write(true).open(file)?;
        chunk.sync_all()?;
        let size = chunk.metadata()?.len();
        let mut marker = std::fs::File::create(self.done_path(i))?;
        write!(marker, "{size}")?;
        marker.sync_all()
    }

    /// Whether `file` still has the size chunk `i`'s marker recorded. Markers written before
    /// sizes were recorded are empty and pass: the frame count then decides.
    pub fn size_matches(&self, i: usize, file: &Path) -> bool {
        let Ok(marker) = std::fs::read_to_string(self.done_path(i)) else {
            return false;
        };
        let Ok(size) = std::fs::metadata(file).map(|m| m.len()) else {
            return false;
        };
        match marker.trim() {
            "" => true,
            recorded => recorded.parse() == Ok(size),
        }
    }

    /// Forgets that chunk `i` was finished, so it's encoded again.
    pub fn clear_done(&self, i: usize) {
        let _ = std::fs::remove_file(self.done_path(i));
    }

    /// Finished chunks, counted from the start (resuming continues after these).
    pub fn done_count(&self, state: &ChunkState) -> usize {
        (0..state.chunks.len())
            .take_while(|i| self.is_done(*i))
            .count()
    }

    pub fn remove(&self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }

    pub fn exists(&self) -> bool {
        self.0.is_dir()
    }
}

/// "resumes at chunk 7 of 24" for a work folder with finished chunks, else `None`.
pub fn resume_note(temp: &Path, base: &str) -> Option<String> {
    let dir = WorkDir::new(temp, base);
    let state: ChunkState =
        serde_json::from_str(&std::fs::read_to_string(dir.state_path()).ok()?).ok()?;
    let done = dir.done_count(&state);
    (done > 0 && done < state.chunks.len())
        .then(|| format!("resumes at chunk {} of {}", done + 1, state.chunks.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FPS: f64 = 24.0;

    fn mins(m: f64) -> u64 {
        (m * 60.0 * FPS) as u64
    }

    #[test]
    fn parses_scdet() {
        let log = "[scdet @ 0x1] lavfi.scd.score: 28.1, lavfi.scd.time: 10.5\n[scdet @ 0x1] lavfi.scd.score: 12.0, lavfi.scd.time: 1.0\nlavfi.scd.time: 10.5";
        assert_eq!(parse_scene_cuts(log, 24.0), [24, 252]);
    }

    #[test]
    fn short_files_are_one_chunk() {
        assert_eq!(plan_chunks(mins(5.0), FPS, &[]), vec![0..mins(5.0)]);
        assert_eq!(plan_chunks(0, FPS, &[]), Vec::<Range<u64>>::new());
    }

    #[test]
    fn cuts_at_the_scene_change_nearest_each_target() {
        let total = mins(12.0);
        let cuts = [
            mins(3.2),
            mins(3.9),
            mins(4.5),
            mins(7.5),
            mins(8.2),
            mins(11.5),
        ];
        let c = plan_chunks(total, FPS, &cuts);
        // 3.9 is nearest to 4; then 8.2 is nearest to 3.9 + 4 = 7.9
        assert_eq!(c, [0..mins(3.9), mins(3.9)..mins(8.2), mins(8.2)..total]);
    }

    #[test]
    fn without_cuts_in_the_window_it_cuts_at_the_target() {
        let total = mins(12.0);
        let c = plan_chunks(total, FPS, &[mins(1.0), mins(6.0)]);
        assert_eq!(c, [0..mins(4.0), mins(4.0)..mins(8.0), mins(8.0)..total]);
    }

    #[test]
    fn no_chunk_is_shorter_than_the_minimum() {
        // 9 min: a cut at 4 and 8 would leave 1 min; the tail merges instead
        let c = plan_chunks(mins(9.0), FPS, &[mins(8.0)]);
        assert_eq!(c, [0..mins(4.0), mins(4.0)..mins(9.0)]);
        for r in plan_chunks(mins(120.0), FPS, &[]) {
            assert!(r.end - r.start >= mins(1.5));
        }
        // A cut too close to the end is not used
        // HandBrake: ten-minute pieces, a 128-minute movie in thirteen
        let hb = plan_chunks_of(mins(128.0), FPS, &[], HANDBRAKE_TARGET_SECS);
        assert_eq!(hb.len(), 13);
        assert!(hb.iter().all(|r| r.end - r.start <= mins(11.6)));
        assert!(!worth_chunking_of(mins(11.0), FPS, HANDBRAKE_TARGET_SECS));
        let c = plan_chunks(mins(10.0), FPS, &[mins(4.9)]);
        assert!(c.iter().all(|r| r.end - r.start >= mins(1.5)));
    }

    #[test]
    fn short_encodes_are_not_chunked() {
        assert!(!worth_chunking(720, FPS), "a test clip");
        assert!(!worth_chunking(mins(5.5), FPS));
        assert!(worth_chunking(mins(5.6), FPS));
    }

    #[test]
    fn keys_are_stable_and_sensitive() {
        assert_eq!(fnv1a(&[b"a", b"b"]), fnv1a(&[b"a", b"b"]));
        assert_ne!(fnv1a(&[b"ab"]), fnv1a(&[b"a", b"b"]), "parts are separated");
        assert_eq!(fnv1a(&[]), "cbf29ce484222325");
    }

    #[test]
    fn work_dir_state_and_resume_note() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = WorkDir::new(tmp.path(), "ep01");
        let state = ChunkState {
            key: "k1".into(),
            chunks: vec![(0, 10), (10, 20), (20, 30)],
        };
        dir.reset(&state).unwrap();
        assert_eq!(dir.load("k1"), Some(state.clone()));
        assert_eq!(dir.load("k2"), None, "a different plan starts over");
        assert_eq!(resume_note(tmp.path(), "ep01"), None, "nothing done yet");
        for i in [0, 2] {
            std::fs::write(dir.ivf(i), b"frames").unwrap();
            dir.mark_done(i, &dir.ivf(i)).unwrap();
        }
        assert_eq!(
            dir.done_count(&state),
            1,
            "only chunks done from the start count"
        );
        assert_eq!(
            resume_note(tmp.path(), "ep01").as_deref(),
            Some("resumes at chunk 2 of 3")
        );
        dir.remove();
        assert!(!dir.exists());
    }

    #[test]
    fn a_marker_only_vouches_for_the_chunk_it_saw() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = WorkDir::new(tmp.path(), "ep01");
        dir.reset(&ChunkState {
            key: "k1".into(),
            chunks: vec![(0, 10)],
        })
        .unwrap();
        let ivf = dir.ivf(0);
        assert!(dir.mark_done(0, &ivf).is_err(), "no chunk, no marker");
        assert!(!dir.is_done(0));
        std::fs::write(&ivf, b"ten frames").unwrap();
        dir.mark_done(0, &ivf).unwrap();
        assert!(dir.size_matches(0, &ivf));
        // The chunk lost its tail (power loss): the marker no longer matches
        std::fs::write(&ivf, b"ten").unwrap();
        assert!(dir.is_done(0) && !dir.size_matches(0, &ivf));
        // A marker from before sizes were recorded leaves it to the frame count
        std::fs::write(dir.done_path(0), b"").unwrap();
        assert!(dir.size_matches(0, &ivf));
        std::fs::remove_file(&ivf).unwrap();
        assert!(!dir.size_matches(0, &ivf), "a missing chunk never matches");
        dir.clear_done(0);
        assert!(!dir.is_done(0));
    }
}
