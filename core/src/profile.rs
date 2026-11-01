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
    Cg,
}

impl FromStr for ProfileId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "anime-topaz" => Ok(Self::AnimeTopaz),
            "anime" => Ok(Self::Anime),
            "cg" => Ok(Self::Cg),
            _ => Err(Error::UnknownProfile(s.to_string())),
        }
    }
}

impl fmt::Display for ProfileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::AnimeTopaz => "anime-topaz",
            Self::Anime => "anime",
            Self::Cg => "cg",
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
        assert!("live".parse::<ProfileId>().is_err());
    }
}
