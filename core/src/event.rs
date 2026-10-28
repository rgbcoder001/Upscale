//! Typed progress and state events. The CLI prints them as JSON lines; the app forwards
//! them to the UI as Tauri events.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    /// A log line for a job.
    #[serde(rename_all = "camelCase")]
    Log { job_id: String, line: String },
    /// Frame progress for the current video stage.
    #[serde(rename_all = "camelCase")]
    Progress {
        job_id: String,
        frames_done: u64,
        frames_total: u64,
        fps: f64,
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
        };
        assert_eq!(
            e.to_json_line(),
            r#"{"type":"progress","jobId":"j1","framesDone":10,"framesTotal":100,"fps":11.5}"#
        );
        let back: Event = serde_json::from_str(&e.to_json_line()).unwrap();
        assert_eq!(back, e);
    }
}
