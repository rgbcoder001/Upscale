//! vs-mlrt, the AI upscaling runtime (`Initialize-VsMlrt`, `Test-VsMlrt`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::Value;

use super::find_file;
use super::install::{Asset, Installer, release_assets};
use crate::log::Log;
use crate::process::Runner;
use crate::settings::Backend;
use crate::{Error, Result};

pub const LATEST_RELEASE: &str =
    "https://api.github.com/repos/AmusementClub/vs-mlrt/releases/latest";

/// vs-mlrt publishes Windows packages only (Linux and macOS would need their own builds).
pub const AVAILABLE: bool = cfg!(windows);

/// The Windows package for `flavor` (`cuda` or `vk`), possibly split into `.7z.001`, `.002`, …
pub fn pick_assets(release: &Value, flavor: &str) -> Vec<Asset> {
    let re = Regex::new(&format!(
        r"^vsmlrt-windows-x64-{}\.v[\d.]+\.7z(\.\d{{3}})?$",
        regex::escape(flavor)
    ))
    .expect("valid regex");
    let mut assets: Vec<Asset> = release_assets(release)
        .into_iter()
        .filter(|a| re.is_match(&a.name))
        .collect();
    assets.sort_by(|a, b| a.name.cmp(&b.name));
    assets
}

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
    let root = inst.tools_dir.join("vs-mlrt");
    let installed = installed_version(&root);
    let existing = find_mlrt_dir(&root);
    if let Some(dir) = &existing
        && installed.ends_with(&format!("-{flavor}"))
        && !inst.update
    {
        return Ok(dir.clone());
    }

    let rel = inst.http_json(LATEST_RELEASE)?;
    let tag = rel
        .get("tag_name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let want = format!("{tag}-{flavor}");
    if let Some(dir) = &existing
        && installed == want
    {
        inst.log
            .info(&format!("  vs-mlrt {tag} ({flavor}) is current"));
        return Ok(dir.clone());
    }

    let assets = pick_assets(&rel, flavor);
    if assets.is_empty() {
        let names: Vec<String> = release_assets(&rel).into_iter().map(|a| a.name).collect();
        return Err(Error::Tool(format!(
            "vs-mlrt {tag} has no '{flavor}' package. Assets: {}",
            names.join(", ")
        )));
    }
    let size_gb = assets.iter().map(|a| a.size).sum::<u64>() as f64 / (1u64 << 30) as f64;
    inst.log.info(&format!(
        "  Downloading vs-mlrt {tag} ({flavor} package, {} GB) ...",
        crate::util::fmt_n(size_gb, 1)
    ));
    // Every part is checked against its SHA-256; verified parts are kept for a retry
    let mut parts = Vec::new();
    for a in &assets {
        parts.push(inst.fetch(&a.download()?)?);
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
    use serde_json::json;

    #[test]
    fn picks_the_flavor_package_parts_in_order() {
        let rel = json!({ "tag_name": "v15.16", "assets": [
            { "name": "vsmlrt-windows-x64-cuda.v15.16.7z.002", "size": 1 },
            { "name": "vsmlrt-windows-x64-cuda.v15.16.7z.001", "size": 1 },
            { "name": "vsmlrt-windows-x64-vk.v15.16.7z", "size": 1 },
            { "name": "vsmlrt-windows-x64-tensorrt.v15.16.7z", "size": 1 },
            { "name": "vsmlrt-windows-x64-cuda.v15.16.7z.sha256", "size": 1 }] });
        let names: Vec<String> = pick_assets(&rel, "cuda")
            .into_iter()
            .map(|a| a.name)
            .collect();
        assert_eq!(
            names,
            [
                "vsmlrt-windows-x64-cuda.v15.16.7z.001",
                "vsmlrt-windows-x64-cuda.v15.16.7z.002"
            ]
        );
        assert_eq!(pick_assets(&rel, "vk").len(), 1);
    }

    #[test]
    fn selftest_command_and_reason() {
        let args: Vec<String> = selftest_args(
            Path::new("upscale.vpy"),
            Backend::Trt,
            Path::new(r"C:\t\vs-mlrt"),
            Path::new(r"C:\t\vs-mlrt"),
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
                r"mlrt=C:\t\vs-mlrt",
                "-a",
                r"mlrt_root=C:\t\vs-mlrt",
                "upscale.vpy",
                "-"
            ]
        );
        let out = "Script evaluation failed:\nPython exception: vsmlrt: failed to load vstrt.dll\nError: Failed to evaluate the script\n";
        assert_eq!(failure_reason(out), "Error: Failed to evaluate the script");
        assert_eq!(failure_reason(" just this \n"), "just this");
    }
}
