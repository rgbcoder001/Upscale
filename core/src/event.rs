//! Typed progress and state events. The CLI prints them as JSON lines; the app forwards
//! them to the UI as Tauri events.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Where a job is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    /// vspipe --info (and building the source index on first use).
    Reading,
    /// Upscaling + encoding, or the HandBrake encode.
    Video,
    Verify,
    Audio,
    Muxing,
    Compare,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    /// A log line for a job.
    #[serde(rename_all = "camelCase")]
    Log { job_id: String, line: String },
    #[serde(rename_all = "camelCase")]
    JobStarted { job_id: String, file: PathBuf },
    #[serde(rename_all = "camelCase")]
    Stage { job_id: String, stage: Stage },
    /// Frame progress for the current video stage.
    #[serde(rename_all = "camelCase")]
    Progress {
        job_id: String,
        frames_done: u64,
        frames_total: u64,
        fps: f64,
        /// Seconds left at the current speed.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        eta_seconds: Option<u64>,
        /// Video stream size so far, scaled to the whole file.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        projected_bytes: Option<u64>,
        paused: bool,
    },
    #[serde(rename_all = "camelCase")]
    JobDone {
        job_id: String,
        output: PathBuf,
        seconds: u64,
    },
    #[serde(rename_all = "camelCase")]
    JobSkipped { job_id: String, reason: String },
    #[serde(rename_all = "camelCase")]
    JobFailed { job_id: String, reason: String },
    /// The batch finished, was stopped after a job, or was cancelled.
    #[serde(rename_all = "camelCase")]
    BatchDone {
        done: usize,
        failed: usize,
        skipped: usize,
        cancelled: bool,
    },
}

impl Event {
    /// Serialises the event as one line of JSON, without the trailing newline.
    pub fn to_json_line(&self) -> String {
        // Serialising these plain structs cannot fail.
        serde_json::to_string(self).expect("event serialises")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_serialises_as_tagged_camel_case() {
        let e = Event::Progress {
            job_id: "j1".into(),
            frames_done: 10,
            frames_total: 100,
            fps: 11.5,
            eta_seconds: Some(8),
            projected_bytes: None,
            paused: false,
        };
        assert_eq!(
            e.to_json_line(),
            r#"{"type":"progress","jobId":"j1","framesDone":10,"framesTotal":100,"fps":11.5,"etaSeconds":8,"paused":false}"#
        );
        let back: Event = serde_json::from_str(&e.to_json_line()).unwrap();
        assert_eq!(back, e);
        let s = Event::Stage {
            job_id: "j".into(),
            stage: Stage::Muxing,
        }
        .to_json_line();
        assert_eq!(s, r#"{"type":"stage","jobId":"j","stage":"muxing"}"#);
    }
}
