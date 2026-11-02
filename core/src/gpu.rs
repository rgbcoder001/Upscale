//! Which GPU the upscaler uses: NVIDIA first, then a discrete AMD/Intel card. Integrated
//! graphics (e.g. "AMD Radeon(TM) Graphics" on Ryzen) are listed as not used.

use std::ffi::OsString;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use crate::process::Runner;
use crate::settings::Backend;

static NVIDIA: LazyLock<Regex> = LazyLock::new(|| Regex::new("(?i)NVIDIA").expect("valid regex"));
static DISCRETE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("(?i)Radeon RX|Radeon Pro|Arc").expect("valid regex"));

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuChoice {
    pub name: Option<String>,
    /// Present but not used.
    pub others: Vec<String>,
    pub nvidia: bool,
}

impl GpuChoice {
    /// vs-mlrt backend: TensorRT on NVIDIA, ncnn (Vulkan) otherwise.
    pub fn default_backend(&self) -> Backend {
        if self.nvidia {
            Backend::Trt
        } else {
            Backend::Ncnn
        }
    }

    /// The vs-mlrt package flavour to install.
    pub fn mlrt_flavor(&self) -> &'static str {
        if self.nvidia { "cuda" } else { "vk" }
    }

    /// `GPU: name  (also present, not used: …)`
    pub fn log_line(&self) -> String {
        let mut line = format!("GPU: {}", self.name.as_deref().unwrap_or(""));
        if !self.others.is_empty() {
            line.push_str(&format!(
                "  (also present, not used: {})",
                self.others.join(", ")
            ));
        }
        line
    }
}

pub fn pick_gpu(names: &[String]) -> GpuChoice {
    let names: Vec<&String> = names.iter().filter(|n| !n.trim().is_empty()).collect();
    let name = names
        .iter()
        .find(|n| NVIDIA.is_match(n))
        .or_else(|| names.iter().find(|n| DISCRETE.is_match(n)))
        .or_else(|| names.first())
        .map(|n| (*n).clone());
    let others = names
        .iter()
        .filter(|n| {
            name.as_ref()
                .is_none_or(|chosen| !n.eq_ignore_ascii_case(chosen))
        })
        .map(|n| (*n).clone())
        .collect();
    GpuChoice {
        nvidia: name.as_ref().is_some_and(|n| NVIDIA.is_match(n)),
        name,
        others,
    }
}

/// Display adapter names reported by the OS.
pub fn list_gpus(runner: &dyn Runner) -> Vec<String> {
    #[cfg(windows)]
    {
        let Ok(ps) = which::which("powershell") else {
            return Vec::new();
        };
        let args: Vec<OsString> = [
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "(Get-CimInstance Win32_VideoController).Name",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        lines(runner, &ps, &args)
    }
    #[cfg(target_os = "macos")]
    {
        let args: Vec<OsString> = ["SPDisplaysDataType", "-json"].map(OsString::from).into();
        runner
            .output(Path::new("/usr/sbin/system_profiler"), &args)
            .map(|o| mac_gpu_names(&o.stdout))
            .unwrap_or_default()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        // Linux (PLAN.md Phase 6): lspci
        let _ = runner;
        Vec::new()
    }
}

/// GPU names from `system_profiler SPDisplaysDataType -json` (e.g. `Apple M4 Pro`).
pub fn mac_gpu_names(json: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    v.get("SPDisplaysDataType")
        .and_then(|a| a.as_array())
        .map(|gpus| {
            gpus.iter()
                .filter_map(|g| {
                    g.get("sppci_model")
                        .or_else(|| g.get("_name"))
                        .and_then(|n| n.as_str())
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

#[allow(dead_code)]
fn lines(runner: &dyn Runner, exe: &Path, args: &[OsString]) -> Vec<String> {
    runner
        .output(exe, args)
        .map(|o| {
            o.stdout
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn nvidia_beats_integrated_graphics() {
        let g = pick_gpu(&names(&[
            "AMD Radeon(TM) Graphics",
            "NVIDIA GeForce RTX 5090",
        ]));
        assert_eq!(g.name.as_deref(), Some("NVIDIA GeForce RTX 5090"));
        assert_eq!(g.others, ["AMD Radeon(TM) Graphics"]);
        assert!(g.nvidia);
        assert_eq!(g.default_backend(), Backend::Trt);
        assert_eq!(
            g.log_line(),
            "GPU: NVIDIA GeForce RTX 5090  (also present, not used: AMD Radeon(TM) Graphics)"
        );
    }

    #[test]
    fn discrete_amd_beats_integrated() {
        let g = pick_gpu(&names(&[
            "AMD Radeon(TM) Graphics",
            "AMD Radeon RX 7900 XTX",
        ]));
        assert_eq!(g.name.as_deref(), Some("AMD Radeon RX 7900 XTX"));
        assert_eq!(g.default_backend(), Backend::Ncnn);
        assert_eq!(g.mlrt_flavor(), "vk");
    }

    #[test]
    fn falls_back_to_first_or_none() {
        assert_eq!(
            pick_gpu(&names(&["Intel UHD"])).name.as_deref(),
            Some("Intel UHD")
        );
        assert_eq!(pick_gpu(&[]), GpuChoice::default());
    }

    #[test]
    fn mac_system_profiler() {
        let json = r#"{ "SPDisplaysDataType" : [ {
            "_name" : "Apple M4 Pro", "sppci_cores" : "20", "sppci_model" : "Apple M4 Pro",
            "spdisplays_ndrvs" : [ { "_name" : "LG UltraFine" } ] } ] }"#;
        assert_eq!(mac_gpu_names(json), ["Apple M4 Pro"]);
        let g = pick_gpu(&mac_gpu_names(json));
        assert_eq!(g.name.as_deref(), Some("Apple M4 Pro"));
        assert!(!g.nvidia);
        assert!(mac_gpu_names("not json").is_empty());
    }
}
