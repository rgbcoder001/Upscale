//! The exact tool versions this release of Upscale installs in the private tools folder
//! (Windows). Every computer on the same Upscale version gets the same, tested set, and an
//! upstream release can't break a new install. New tool versions come with Upscale updates:
//! when a pin changes, the tool is replaced the next time tools are checked (a batch start or
//! Update tools), and only then.
//!
//! To move a tool to a new version, change its entry here: the name, a URL that stays put (a
//! tagged release, not "latest") and the SHA-256 published with it. BtbN deletes its daily
//! FFmpeg builds after a few weeks but keeps the last build of each month, so FFmpeg is
//! pinned to one of those.
//!
//! Not pinned: MakeMKV (its trial and beta keys need a current version) and the optional
//! deband plugin. Tools from PATH, winget or Homebrew ("Use system tools", and always on
//! macOS) are whatever those install.

use super::fetch::Download;

/// One pinned download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pinned {
    pub name: &'static str,
    pub url: &'static str,
    /// Lowercase hex SHA-256.
    pub sha256: &'static str,
    pub size: u64,
}

impl Pinned {
    pub fn download(&self) -> Download {
        Download {
            name: self.name.into(),
            url: self.url.into(),
            sha256: self.sha256.into(),
            size: Some(self.size),
        }
    }

    /// What a tool's `SOURCE.txt` says when this build is installed.
    pub fn stamp(&self) -> String {
        format!("{} {}", self.name, self.sha256)
    }
}

/// Python 3.12 (python-build-standalone 20261026).
pub const PYTHON: Pinned = Pinned {
    name: "cpython-3.12.16+20261026-x86_64-pc-windows-msvc-install_only.tar.gz",
    url: "https://github.com/astral-sh/python-build-standalone/releases/download/20261026/cpython-3.12.16%2B20261026-x86_64-pc-windows-msvc-install_only.tar.gz",
    sha256: "590dfcbe3a34c07301e6332e13e2188b98655cd072b08afa45a7998dc4c22bc5",
    size: 47_113_364,
};

/// FFmpeg 9.0.2 with libsvtav1 (BtbN, the October 2026 month-end build).
pub const FFMPEG: Pinned = Pinned {
    name: "ffmpeg-n9.0.2-39-g7adfbe40dc-win64-gpl-9.0.zip",
    url: "https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/download/autobuild-2026-10-31-12-59/ffmpeg-n9.0.2-39-g7adfbe40dc-win64-gpl-9.0.zip",
    sha256: "693f3e224bc184b40524a62ca47526cfebaf2504414021b92fe75f677ad868b9",
    size: 195_110_184,
};

/// MKVToolNix 103.0 portable.
pub const MKVTOOLNIX: Pinned = Pinned {
    name: "mkvtoolnix-64-bit-103.0.zip",
    url: "https://mkvtoolnix.download/windows/releases/103.0/mkvtoolnix-64-bit-103.0.zip",
    sha256: "c860a10c707cb1ee587d1464f6171842f2ada3bb6f031dfff55fec75542f0417",
    size: 103_859_301,
};

/// HandBrakeCLI 1.11.2.
pub const HANDBRAKE: Pinned = Pinned {
    name: "HandBrakeCLI-1.11.2-win-x86_64.zip",
    url: "https://github.com/HandBrake/HandBrake/releases/download/1.11.2/HandBrakeCLI-1.11.2-win-x86_64.zip",
    sha256: "80bfe8d5f5d11cc3ef76b834add3ed4e82dee6523ffeb435c283f88b1a21f09d",
    size: 25_721_458,
};

/// 7-Zip 26.05's standalone extractor (for the vs-mlrt archive).
pub const SEVEN_ZIP: Pinned = Pinned {
    name: "7zr.exe",
    url: "https://github.com/ip7z/7zip/releases/download/26.05/7zr.exe",
    sha256: "9bf582b894dfa665f852d7e9112b02290a1c303c2d95a21fcdc41ffcb03143b7",
    size: 602_624,
};

/// The vs-mlrt release.
pub const VS_MLRT_TAG: &str = "v15.16";

/// vs-mlrt's package for NVIDIA cards (TensorRT and CUDA), in two parts.
pub const VS_MLRT_CUDA: &[Pinned] = &[
    Pinned {
        name: "vsmlrt-windows-x64-cuda.v15.16.7z.001",
        url: "https://github.com/AmusementClub/vs-mlrt/releases/download/v15.16/vsmlrt-windows-x64-cuda.v15.16.7z.001",
        sha256: "19959fcd94b17f4f8cc775fd7c2b5bb6ca93d6584e465c22b94591d1576f75fa",
        size: 2_147_483_647,
    },
    Pinned {
        name: "vsmlrt-windows-x64-cuda.v15.16.7z.002",
        url: "https://github.com/AmusementClub/vs-mlrt/releases/download/v15.16/vsmlrt-windows-x64-cuda.v15.16.7z.002",
        sha256: "c2b3e1e313401fdc87963fa5bccf41728a937c8e16420d77cefeb4c1673f06c7",
        size: 1_450_964_812,
    },
];

/// vs-mlrt's package for other graphics cards (NCNN on Vulkan, DirectML).
pub const VS_MLRT_GENERIC_GPU: &[Pinned] = &[Pinned {
    name: "vsmlrt-windows-x64-generic-gpu.v15.16.7z",
    url: "https://github.com/AmusementClub/vs-mlrt/releases/download/v15.16/vsmlrt-windows-x64-generic-gpu.v15.16.7z",
    sha256: "45e432d228d689002cfbedaa20283ffedfc764e9ac77087ffc66c2f0b8df99d6",
    size: 883_570_116,
}];

/// The vs-mlrt package for a flavour (`GpuChoice::mlrt_flavor`): `cuda` or `generic-gpu`.
pub fn vs_mlrt(flavor: &str) -> Option<&'static [Pinned]> {
    match flavor {
        "cuda" => Some(VS_MLRT_CUDA),
        "generic-gpu" => Some(VS_MLRT_GENERIC_GPU),
        _ => None,
    }
}

/// One pinned Python wheel: name, version and the SHA-256 of its Windows wheel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wheel {
    pub name: &'static str,
    pub version: &'static str,
    pub sha256: &'static str,
}

/// VapourSynth R79 and the plugins `upscale.vpy` needs (`VS_PACKAGES`). R79 is the newest
/// release vs-mlrt's plugins work with, and bestsource stays below 22.
pub const VS_WHEELS: &[Wheel] = &[
    Wheel {
        name: "vapoursynth",
        version: "79",
        sha256: "643d9708df7e98807a66d4e8804e2e9a1380af22bea2873387d8af103e69aa5b",
    },
    Wheel {
        name: "vapoursynth-bestsource",
        version: "21.0",
        sha256: "f1003f5e610acc80ae1bd23752ba182455f807922c83db9958a299991f272ea4",
    },
    Wheel {
        name: "vapoursynth-vivtc",
        version: "2.0",
        sha256: "554355240ac9ceb15711b387010ec5d025877beca2d9a8e85a3f392e42a96919",
    },
    Wheel {
        name: "vapoursynth-descale",
        version: "12",
        sha256: "a0b258a281f6f110a450396194d44bc00b77d2395392f9d1fbee832dfdb5c0de",
    },
    Wheel {
        name: "vapoursynth-mvtools",
        version: "29",
        sha256: "d64d6525d88ce52572f93e04598083c7cff2a92bf6bc269168130fd7a5227746",
    },
    Wheel {
        name: "vapoursynth-akarin",
        version: "1.5.0",
        sha256: "66ba011c36f2c0a630c6a6662a40ca62d992b266ea36a0c1bf3c988e1738d7ae",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<Pinned> {
        let mut v = vec![PYTHON, FFMPEG, MKVTOOLNIX, HANDBRAKE, SEVEN_ZIP];
        v.extend_from_slice(VS_MLRT_CUDA);
        v.extend_from_slice(VS_MLRT_GENERIC_GPU);
        v
    }

    #[test]
    fn every_pin_is_a_fixed_checked_download() {
        for p in all() {
            assert!(crate::tools::fetch::is_sha256(p.sha256), "{}", p.name);
            assert_eq!(p.sha256, p.sha256.to_ascii_lowercase(), "{}", p.name);
            assert!(p.size > 0, "{}", p.name);
            assert!(p.url.starts_with("https://"), "{}", p.name);
            assert!(
                !p.url.contains("/latest"),
                "{}: a pin must not follow the newest release",
                p.name
            );
            let file = p.url.rsplit('/').next().unwrap().replace("%2B", "+");
            assert_eq!(file, p.name, "the URL downloads the named file");
        }
        for w in VS_WHEELS {
            assert!(crate::tools::fetch::is_sha256(w.sha256), "{}", w.name);
        }
        assert!(
            VS_MLRT_CUDA
                .iter()
                .chain(VS_MLRT_GENERIC_GPU)
                .all(|p| p.url.contains(&format!("/{VS_MLRT_TAG}/")))
        );
    }

    #[test]
    fn every_gpu_has_a_vs_mlrt_package() {
        for gpu in [true, false] {
            let choice = crate::gpu::GpuChoice {
                nvidia: gpu,
                ..Default::default()
            };
            assert!(vs_mlrt(choice.mlrt_flavor()).is_some());
        }
        assert_eq!(vs_mlrt("vk"), None);
    }

    #[test]
    fn wheels_cover_the_scripts_packages() {
        let names: Vec<&str> = crate::tools::vapoursynth::VS_PACKAGES
            .iter()
            .map(|p| p.split(['=', '<']).next().unwrap())
            .collect();
        assert_eq!(VS_WHEELS.iter().map(|w| w.name).collect::<Vec<_>>(), names);
        assert_eq!(
            VS_WHEELS[0].version,
            crate::tools::vapoursynth::VS_VERSION_PIN.to_string()
        );
    }
}
