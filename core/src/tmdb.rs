//! TMDB lookups for Jellyfin names (RIPPING.md, "Naming"): search movies and shows together,
//! and a show's seasons and episodes. Requests go through curl with the user's API Read Access
//! Token; parsing is separate and tested with captured responses.

use std::ffi::OsString;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::process::Runner;
use crate::{Error, Result};

const API: &str = "https://api.themoviedb.org/3";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MediaKind {
    Movie,
    Show,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Match {
    pub id: u64,
    pub kind: MediaKind,
    pub name: String,
    pub year: Option<u32>,
    pub original_language: String,
    /// The first line or so of the overview, to tell remakes apart.
    pub overview: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Season {
    pub number: u32,
    pub episodes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Episode {
    pub number: u32,
    pub name: String,
    /// Minutes, when TMDB knows.
    pub runtime: Option<u32>,
}

/// Percent-encodes a query parameter (RFC 3986 unreserved characters stay).
pub fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub fn search_url(query: &str) -> String {
    format!(
        "{API}/search/multi?query={}&include_adult=false&page=1",
        encode(query.trim())
    )
}

pub fn show_url(id: u64) -> String {
    format!("{API}/tv/{id}")
}

pub fn season_url(id: u64, season: u32) -> String {
    format!("{API}/tv/{id}/season/{season}")
}

/// curl's arguments for a GET with the token. The token goes in a header, never in the URL.
pub fn curl_args(url: &str, token: &str) -> Vec<OsString> {
    [
        "-sS",
        "--fail-with-body",
        "--max-time",
        "15",
        "-H",
        "Accept: application/json",
        "-H",
        &format!("Authorization: Bearer {}", token.trim()),
        url,
    ]
    .map(OsString::from)
    .into()
}

fn year(date: Option<&str>) -> Option<u32> {
    date?.get(..4)?.parse().ok()
}

/// Movies and shows from `/search/multi`, in TMDB's order (people are dropped).
pub fn parse_search(v: &Value) -> Vec<Match> {
    let Some(results) = v.get("results").and_then(Value::as_array) else {
        return Vec::new();
    };
    results
        .iter()
        .filter_map(|r| {
            let s = |k: &str| r.get(k).and_then(Value::as_str);
            let (kind, name, date) = match s("media_type")? {
                "movie" => (MediaKind::Movie, s("title")?, s("release_date")),
                "tv" => (MediaKind::Show, s("name")?, s("first_air_date")),
                _ => return None,
            };
            let overview = s("overview").unwrap_or("");
            Some(Match {
                id: r.get("id")?.as_u64()?,
                kind,
                name: name.to_string(),
                year: year(date),
                original_language: s("original_language").unwrap_or("").to_string(),
                overview: overview.chars().take(160).collect(),
            })
        })
        .collect()
}

/// A show's seasons with their episode counts (`/tv/{id}`), specials (season 0) included.
pub fn parse_show(v: &Value) -> Vec<Season> {
    v.get("seasons")
        .and_then(Value::as_array)
        .map(|seasons| {
            seasons
                .iter()
                .filter_map(|s| {
                    Some(Season {
                        number: s.get("season_number")?.as_u64()? as u32,
                        episodes: s.get("episode_count")?.as_u64()? as u32,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A season's episodes (`/tv/{id}/season/{n}`).
pub fn parse_season(v: &Value) -> Vec<Episode> {
    v.get("episodes")
        .and_then(Value::as_array)
        .map(|eps| {
            eps.iter()
                .filter_map(|e| {
                    Some(Episode {
                        number: e.get("episode_number")?.as_u64()? as u32,
                        name: e
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        runtime: e.get("runtime").and_then(Value::as_u64).map(|m| m as u32),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// GETs a TMDB URL. A 401 means the token is wrong; other failures carry TMDB's message.
pub fn get(runner: &dyn Runner, curl: &Path, url: &str, token: &str) -> Result<Value> {
    if token.trim().is_empty() {
        return Err(Error::Tool(
            "No TMDB token: add your API Read Access Token in Settings.".into(),
        ));
    }
    let out = runner.output(curl, &curl_args(url, token))?;
    let body: Value = serde_json::from_str(&out.stdout).unwrap_or(Value::Null);
    if !out.success() {
        let msg = body
            .get("status_message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| out.stderr.trim().to_string());
        return Err(Error::Tool(format!("TMDB: {msg}")));
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn urls_and_arguments() {
        assert_eq!(
            search_url(" Sample the Sample Picture "),
            "https://api.themoviedb.org/3/search/multi?query=Sample%20the%20Sample%20Picture&include_adult=false&page=1"
        );
        assert_eq!(encode("Re:Zero & Ä"), "Re%3AZero%20%26%20%C3%84");
        assert_eq!(
            season_url(1, 2),
            "https://api.themoviedb.org/3/tv/1/season/2"
        );
        let args: Vec<String> = curl_args("https://x", " tok ")
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&"Authorization: Bearer tok".to_string()));
        assert_eq!(args.last().unwrap(), "https://x");
    }

    #[test]
    fn search_results() {
        // In the shape of TMDB's documented /search/multi response; check against a live one
        let v = json!({ "page": 1, "results": [
            { "id": 155, "media_type": "movie", "title": "The Sample Picture", "release_date": "2008-07-16",
              "original_language": "en", "overview": "A short overview of the picture." },
            { "id": 3570, "media_type": "person", "name": "Someone" },
            { "id": 2304, "media_type": "tv", "name": "Show Alpha", "first_air_date": "1992-10-10",
              "original_language": "ja", "overview": "" },
            { "id": 9, "media_type": "movie", "title": "Untitled", "release_date": "" }
        ]});
        let m = parse_search(&v);
        assert_eq!(m.len(), 3);
        assert_eq!(
            (m[0].kind, m[0].name.as_str(), m[0].year),
            (MediaKind::Movie, "The Sample Picture", Some(2008))
        );
        assert_eq!(
            (m[1].kind, m[1].year, m[1].original_language.as_str()),
            (MediaKind::Show, Some(1992), "ja")
        );
        assert_eq!(m[2].year, None);
        assert!(parse_search(&json!({ "status_code": 7 })).is_empty());
    }

    #[test]
    fn seasons_and_episodes() {
        let show = json!({ "seasons": [
            { "season_number": 0, "episode_count": 2 },
            { "season_number": 1, "episode_count": 28 },
            { "season_number": 2, "episode_count": 66 } ]});
        assert_eq!(
            parse_show(&show),
            [
                Season {
                    number: 0,
                    episodes: 2
                },
                Season {
                    number: 1,
                    episodes: 28
                },
                Season {
                    number: 2,
                    episodes: 66
                }
            ]
        );
        let season = json!({ "episodes": [
            { "episode_number": 7, "name": "The Keeper's Gamble", "runtime": 23 },
            { "episode_number": 8, "name": "", "runtime": null } ]});
        assert_eq!(
            parse_season(&season),
            [
                Episode {
                    number: 7,
                    name: "The Keeper's Gamble".into(),
                    runtime: Some(23)
                },
                Episode {
                    number: 8,
                    name: String::new(),
                    runtime: None
                }
            ]
        );
    }
}
