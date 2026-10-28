//! Disk space (`Get-FreeSpace`, `Get-SpaceNeeded`).

use std::path::Path;

use crate::plan::Route;

pub const GB: u64 = 1 << 30;

/// Free space for the drive holding `dir`; `None` when it can't be read (network shares etc.).
pub fn free_space(dir: &Path) -> Option<u64> {
    // The output folder may not exist yet: ask about the nearest folder that does
    let existing = dir.ancestors().find(|d| d.is_dir())?;
    fs4::available_space(existing).ok()
}

/// Peak space for one file: the video stream and the final MKV exist together at mux time.
/// Upscaled 4K AV1 at CRF 14 runs up to ~2x a 1080p Blu-ray source; HandBrake re-encodes of
/// 4K sources come out smaller than the source.
pub fn space_needed(source_size: u64, route: Route) -> u64 {
    let out_estimate = match route {
        Route::Handbrake => source_size,
        _ => (source_size * 2).max(4 * GB),
    };
    out_estimate * 2 + 2 * GB
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_formula() {
        assert_eq!(
            space_needed(GB, Route::Upscale),
            10 * GB,
            "max(2 GB, 4 GB) x 2 + 2 GB"
        );
        assert_eq!(space_needed(10 * GB, Route::Upscale), 42 * GB);
        assert_eq!(space_needed(50 * GB, Route::Handbrake), 102 * GB);
    }
}
