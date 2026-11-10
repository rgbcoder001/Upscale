//! vs-mlrt, the AI upscaling runtime (`Initialize-VsMlrt`, `Test-VsMlrt`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::find_file;
use super::install::Installer;
use super::pins;
use crate::log::Log;
use crate::process::Runner;
use crate::settings::Backend;
use crate::{Error, Result};

/// vs-mlrt publishes Windows packages only (Linux and macOS would need their own builds).
pub const AVAILABLE: bool = cfg!(windows);

/// The folder holding `vsmlrt.py` under `root`.
pub fn find_mlrt_dir(root: &Path) -> Option<PathBuf> {
    find_file(root, "vsmlrt.py").and_then(|f| f.parent().map(Path::to_path_buf))
}

/// The installed `<tag>-<flavor>` from `VERSION.txt`.
pub fn installed_version(root: &Path) -> String {
    std::fs::read_to_string(root.join("VERSION.txt"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Installs or updates vs-mlrt in `<tools>/vs-mlrt`; returns the folder with `vsmlrt.py`.
pub fn init_vs_mlrt(inst: &Installer, flavor: &str) -> Result<PathBuf> {
    if !AVAILABLE {
        return Err(Error::Tool(
            "The Anime profile upscales with vs-mlrt, which only exists for Windows. \
             On this computer use Anime (Topaz) or Anime AV1."
                .into(),
        ));
    }
    let root = inst.tools_dir.join("vs-mlrt");
    let installed = installed_version(&root);
    let existing = find_mlrt_dir(&root);
    let tag = pins::VS_MLRT_TAG;
    let want = format!("{tag}-{flavor}");
    if let Some(dir) = &existing
        && installed == want
    {
        if inst.update {
            inst.log
                .info(&format!("  vs-mlrt {tag} ({flavor}) is current"));
        }
        return Ok(dir.clone());
    }
    let assets = pins::vs_mlrt(flavor)
        .ok_or_else(|| Error::Tool(format!("vs-mlrt has no '{flavor}' package")))?;
    let size_gb = assets.iter().map(|a| a.size).sum::<u64>() as f64 / (1u64 << 30) as f64;
    inst.log.info(&format!(
        "  Downloading vs-mlrt {tag} ({flavor} package, {} GB) ...",
        crate::util::fmt_n(size_gb, 1)
    ));
    // Every part is checked against its SHA-256; verified parts are kept for a retry
    let mut parts = Vec::new();
    for a in assets {
        parts.push(inst.fetch(&a.download())?);
    }

    let staging = inst.tools_dir.join("vs-mlrt.new");
    let _ = std::fs::remove_dir_all(&staging);
    inst.log.info("  Extracting vs-mlrt ...");
    inst.extract_7z(&parts[0], &staging)?;
    inst.replace_dir(&staging, &root)?;
    std::fs::write(root.join("VERSION.txt"), format!("{want}\n"))?;
    for p in parts {
        let _ = std::fs::remove_file(p);
    }

    let dir = find_mlrt_dir(&root)
        .ok_or_else(|| Error::Tool("vsmlrt.py not found in the vs-mlrt package.".into()))?;
    inst.log
        .info(&format!("  vs-mlrt {tag} ({flavor}) installed"));
    Ok(dir)
}

pub fn selftest_args(vpy: &Path, backend: Backend, mlrt: &Path, mlrt_root: &Path) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec!["--info".into()];
    for (k, v) in [
        ("mode", OsString::from("selftest")),
        ("backend", backend.as_str().into()),
        ("mlrt", mlrt.into()),
        ("mlrt_root", mlrt_root.into()),
    ] {
        let mut kv = OsString::from(format!("{k}="));
        kv.push(v);
        a.push("-a".into());
        a.push(kv);
    }
    a.push(vpy.into());
    a.push("-".into());
    a
}

/// The line that explains a failed self-test: the last one mentioning an error.
pub fn failure_reason(output: &str) -> String {
    output
        .lines()
        .rfind(|l| l.to_lowercase().contains("error"))
        .unwrap_or(output)
        .trim()
        .to_string()
}

/// Loads the vs-mlrt plugins through `upscale.vpy` (mode=selftest), so a broken AI runtime is
/// reported once, up front, with the real error.
pub fn selftest(
    runner: &dyn Runner,
    vspipe: &Path,
    vpy: &Path,
    backend: Backend,
    mlrt: &Path,
    mlrt_root: &Path,
    log: &dyn Log,
) -> Result<()> {
    let out = runner.output(vspipe, &selftest_args(vpy, backend, mlrt, mlrt_root))?;
    if !out.success() {
        let text = format!("{}{}", out.stdout, out.stderr);
        return Err(Error::Tool(format!(
            "vs-mlrt (the AI upscaling runtime) failed its self-test for backend '{}': {}",
            backend.as_str(),
            failure_reason(&text)
        )));
    }
    log.info(&format!(
        "  vs-mlrt self-test passed (backend {})",
        backend.as_str()
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selftest_command_and_reason() {
        let args: Vec<String> = selftest_args(
            Path::new("upscale.vpy"),
            Backend::Trt,
            Path::new(r"C:\y8\vs-mlrt"),
            Path::new(r"C:\y8\vs-mlrt"),
        )
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
        assert_eq!(
            args,
            [
                "--info",
                "-a",
                "mode=selftest",
                "-a",
                "backend=trt",
                "-a",
                r"mlrt=C:\y8\vs-mlrt",
                "-a",
                r"mlrt_root=C:\y8\vs-mlrt",
                "upscale.vpy",
                "-"
            ]
        );
        let out = "Script evaluation failed:\nPython exception: vsmlrt: failed to load vstrt.dll\nError: Failed to evaluate the script\n";
        assert_eq!(failure_reason(out), "Error: Failed to evaluate the script");
        assert_eq!(failure_reason(" just this \n"), "just this");
    }
}
