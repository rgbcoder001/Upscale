use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::Error;

/// The processing profiles (PLAN.md §5). Live action arrives in Phase 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileId {
    /// Anime through Topaz: anime crop, IVTC and track defaults, Topaz upscaling. The default.
    #[serde(rename = "anime-topaz")]
    AnimeTopaz,
    /// Anime through vs-mlrt (Real-CUGAN / ONNX) with denoise and native-resolution analysis.
    Anime,
    /// Anime encoded to AV1 at its own size: crop and IVTC, no upscaling.
    #[serde(rename = "anime-av1")]
    AnimeAv1,
    Cg,
    /// Live action: HandBrake encode at the source size (CRF 14, SSIM tune), no upscaling.
    Live,
}

impl FromStr for ProfileId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "anime-topaz" => Ok(Self::AnimeTopaz),
            "anime" => Ok(Self::Anime),
            "anime-av1" => Ok(Self::AnimeAv1),
            "cg" => Ok(Self::Cg),
            "live" => Ok(Self::Live),
            _ => Err(Error::UnknownProfile(s.to_string())),
        }
    }
}

impl fmt::Display for ProfileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::AnimeTopaz => "anime-topaz",
            Self::Anime => "anime",
            Self::AnimeAv1 => "anime-av1",
            Self::Cg => "cg",
            Self::Live => "live",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_case_insensitively() {
        assert_eq!("Anime".parse::<ProfileId>().unwrap(), ProfileId::Anime);
        assert_eq!(" CG ".parse::<ProfileId>().unwrap(), ProfileId::Cg);
        assert_eq!(
            "Anime-Topaz".parse::<ProfileId>().unwrap(),
            ProfileId::AnimeTopaz
        );
        assert_eq!(ProfileId::AnimeTopaz.to_string(), "anime-topaz");
        assert_eq!(
            serde_json::to_string(&ProfileId::AnimeTopaz).unwrap(),
            "\"anime-topaz\""
        );
        assert!("live-action".parse::<ProfileId>().is_err());
        for p in [
            ProfileId::AnimeTopaz,
            ProfileId::Anime,
            ProfileId::AnimeAv1,
            ProfileId::Cg,
            ProfileId::Live,
        ] {
            assert_eq!(p.to_string().parse::<ProfileId>().unwrap(), p);
            assert_eq!(
                serde_json::to_string(&p).unwrap(),
                format!("\"{p}\""),
                "serde and Display agree"
            );
        }
    }
}
