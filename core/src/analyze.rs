//! Picture analysis behind a trait. Milestone 1B adds the ffmpeg cropdetect/idet runners,
//! `analyze.py`, the cache and season consensus.

use std::path::Path;

use serde::Serialize;

use crate::probe::SourceInfo;

/// Black borders to remove, in pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Crop {
    pub l: u32,
    pub r: u32,
    pub t: u32,
    pub b: u32,
}

/// `idet` result over the sampled frames.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InterlaceDetect {
    pub interlaced: bool,
    /// `tff` or `bff`.
    pub field: &'static str,
    pub detail: String,
}

pub trait Analyzer {
    /// `Get-BlackBorders`; `None` = no crop.
    fn black_borders(&self, file: &Path, src: &SourceInfo) -> Option<Crop>;
    /// `Get-InterlaceDetect`; `None` = not enough frames to decide.
    fn interlace(&self, file: &Path, src: &SourceInfo) -> Option<InterlaceDetect>;
}

/// No picture analysis: nothing cropped, metadata trusted.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoAnalysis;

impl Analyzer for NoAnalysis {
    fn black_borders(&self, _: &Path, _: &SourceInfo) -> Option<Crop> {
        None
    }
    fn interlace(&self, _: &Path, _: &SourceInfo) -> Option<InterlaceDetect> {
        None
    }
}
