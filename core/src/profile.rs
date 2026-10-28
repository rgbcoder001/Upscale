use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::Error;

/// The processing profiles (PLAN.md §5). Live action arrives in Phase 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileId {
    Anime,
    Cg,
}

impl FromStr for ProfileId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "anime" => Ok(Self::Anime),
            "cg" => Ok(Self::Cg),
            _ => Err(Error::UnknownProfile(s.to_string())),
        }
    }
}

impl fmt::Display for ProfileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
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
        assert!("live".parse::<ProfileId>().is_err());
    }
}
