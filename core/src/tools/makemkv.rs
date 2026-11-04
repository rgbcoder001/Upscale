//! MakeMKV, for ripping discs (RIPPING.md, "MakeMKV: install and registration").
//!
//! On macOS Upscale installs and updates it itself: the
//! app is only ad-hoc signed and not notarized, so Homebrew's cask is disabled and Gatekeeper
//! rejects it. A download made by Upscale isn't quarantined, so Gatekeeper never looks at it;
//! the safeguard instead is MakeMKV's own hash list, PGP-signed with the developer's key
//! ("MakeMKV (signature) <support@makemkv.com>"), which is pinned here. The DMG must match its
//! SHA-256 in a list that verifies against that key, or nothing is installed. On Windows it
//! comes from winget like the other tools.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use pgp::composed::{CleartextSignedMessage, Deserializable, SignedPublicKey};
use pgp::types::KeyDetails;
use regex::Regex;
use serde::{Deserialize, Serialize};

use super::Version;
use super::fetch::Download;
use super::install::Installer;
use crate::process::Runner;
use crate::{Error, Result};

pub const DOWNLOAD_PAGE: &str = "https://www.makemkv.com/download/";
/// MakeMKV's signing key, from keys.openpgp.org and keyserver.ubuntu.com (identical).
const SIGNING_KEY: &str = include_str!("makemkv-signing-key.asc");
const SIGNING_FINGERPRINT: &str = "2ecf23305f1fc0b32001673394e3083a18042697";
/// Where the macOS app goes; `makemkvcon` is in `Contents/MacOS`.
pub const MAC_APP: &str = "/Applications/MakeMKV.app";

static LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"href=['"](/download/(makemkv_v(\d+\.\d+\.\d+)_osx\.dmg|makemkv-sha-(\d+\.\d+\.\d+)\.txt|Setup_MakeMKV_v(\d+\.\d+\.\d+)\.exe))['"]"#)
        .expect("valid regex")
});

/// The current release, as the download page lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub mac_dmg: String,
    pub windows_exe: String,
    pub hashes: String,
}

fn absolute(path: &str) -> String {
    format!("https://www.makemkv.com{path}")
}

/// The DMG, the Windows setup and the hash list of the version the download page offers.
pub fn parse_download_page(html: &str) -> Option<Release> {
    let (mut mac, mut win, mut hashes, mut version) = (None, None, None, None);
    for c in LINK.captures_iter(html) {
        let url = absolute(&c[1]);
        if let Some(v) = c.get(3) {
            version = Some(v.as_str().to_string());
            mac = Some(url);
        } else if c.get(4).is_some() {
            hashes = Some(url);
        } else {
            win = Some(url);
        }
    }
    Some(Release {
        version: version?,
        mac_dmg: mac?,
        windows_exe: win?,
        hashes: hashes?,
    })
}

fn signing_key() -> Result<SignedPublicKey> {
    let (key, _) = SignedPublicKey::from_armor_single(SIGNING_KEY.as_bytes())
        .map_err(|e| Error::Tool(format!("MakeMKV signing key: {e}")))?;
    if key.fingerprint().to_string() != SIGNING_FINGERPRINT {
        return Err(Error::Tool(
            "MakeMKV signing key: unexpected fingerprint".into(),
        ));
    }
    Ok(key)
}

/// `(sha256, file name)` from MakeMKV's hash list, only when its PGP signature verifies against
/// the pinned key.
pub fn verified_hashes(signed: &str) -> Result<Vec<(String, String)>> {
    let (msg, _) = CleartextSignedMessage::from_string(signed)
        .map_err(|e| Error::Tool(format!("MakeMKV hash list: {e}")))?;
    msg.verify(&signing_key()?).map_err(|_| {
        Error::Tool(
            "MakeMKV's hash list isn't signed by MakeMKV's key; nothing was installed".into(),
        )
    })?;
    Ok(msg
        .signed_text()
        .lines()
        .filter_map(|l| {
            let (hash, file) = l.trim().split_once(char::is_whitespace)?;
            super::fetch::is_sha256(hash)
                .then(|| (hash.to_ascii_lowercase(), file.trim().to_string()))
        })
        .collect())
}

/// `hdiutil attach` for a DMG: read-only, hidden from Finder, at `mount`.
pub fn attach_args(dmg: &Path, mount: &Path) -> Vec<OsString> {
    let mut a: Vec<OsString> = [
        "attach",
        "-nobrowse",
        "-readonly",
        "-noautoopen",
        "-mountpoint",
    ]
    .map(OsString::from)
    .into();
    a.push(mount.into());
    a.push(dmg.into());
    a
}

pub fn detach_args(mount: &Path) -> Vec<OsString> {
    vec!["detach".into(), mount.into(), "-force".into()]
}

/// `makemkvcon` of an installed MakeMKV.
pub fn find_makemkvcon() -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = if cfg!(target_os = "macos") {
        vec![Path::new(MAC_APP).join("Contents/MacOS/makemkvcon")]
    } else {
        ["ProgramFiles(x86)", "ProgramFiles"]
            .iter()
            .filter_map(|v| super::env_dir(v, &["MakeMKV", "makemkvcon64.exe"]))
            .collect()
    };
    candidates.into_iter().find(|p| p.is_file())
}

/// The installed version (the macOS bundle's `CFBundleShortVersionString`).
pub fn installed_version(runner: &dyn Runner) -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let plist = Path::new(MAC_APP).join("Contents/Info.plist");
    if !plist.is_file() {
        return None;
    }
    let mut args: Vec<OsString> = ["-extract", "CFBundleShortVersionString", "raw", "-o", "-"]
        .map(OsString::from)
        .into();
    args.push(plist.into());
    let out = runner.output(Path::new("/usr/bin/plutil"), &args).ok()?;
    let v = out.stdout.trim().trim_start_matches('v').to_string();
    (out.success() && !v.is_empty()).then_some(v)
}

/// The one-time approval Settings → Ripping asks for (RIPPING.md): MakeMKV isn't notarized by
/// Apple, so nothing installs or replaces it until the user has said yes in the app.
pub const NOT_APPROVED: &str =
    "MakeMKV isn't installed: approve it once in Settings → Ripping → Install MakeMKV.";

/// Installs MakeMKV when it's missing, or updates it when updating and a newer release is out,
/// if the user approved that (`approved`). Without approval an installed MakeMKV is used as it
/// is and never replaced. Returns `makemkvcon`.
pub fn ensure(inst: &Installer, approved: bool) -> Result<PathBuf> {
    if !approved {
        return find_makemkvcon().ok_or_else(|| Error::Tool(NOT_APPROVED.into()));
    }
    if cfg!(windows) {
        let candidates: Vec<PathBuf> = find_makemkvcon().into_iter().collect();
        return inst.system_tool(super::install::MAKEMKV, "makemkvcon64", &candidates);
    }
    if !cfg!(target_os = "macos") {
        return find_makemkvcon()
            .ok_or_else(|| Error::Tool("MakeMKV not found: install it from makemkv.com".into()));
    }
    let installed = installed_version(inst.runner);
    if let (Some(con), Some(_)) = (find_makemkvcon(), &installed)
        && !inst.update
    {
        return Ok(con);
    }
    let release = inst
        .http_text(DOWNLOAD_PAGE)
        .ok()
        .and_then(|page| parse_download_page(&page));
    let Some(release) = release else {
        // Offline or the page changed: keep what's installed
        if let Some(con) = find_makemkvcon() {
            inst.log
                .warn("Couldn't check for a newer MakeMKV; keeping the installed one.");
            return Ok(con);
        }
        return Err(Error::Tool(format!(
            "MakeMKV's download page ({DOWNLOAD_PAGE}) couldn't be read, so MakeMKV wasn't installed."
        )));
    };
    let newer = installed
        .as_deref()
        .and_then(Version::parse)
        .zip(Version::parse(&release.version))
        .is_none_or(|(have, latest)| latest > have);
    if let Some(con) = find_makemkvcon().filter(|_| !newer) {
        inst.log
            .info(&format!("  MakeMKV {} is current", release.version));
        return Ok(con);
    }
    install_mac(inst, &release, installed.as_deref())
}

fn install_mac(inst: &Installer, release: &Release, installed: Option<&str>) -> Result<PathBuf> {
    let dmg_name = release
        .mac_dmg
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();
    let hashes = verified_hashes(&inst.http_text(&release.hashes)?)?;
    let sha256 = hashes
        .iter()
        .find(|(_, f)| *f == dmg_name)
        .map(|(h, _)| h.clone())
        .ok_or_else(|| Error::Tool(format!("{dmg_name} isn't in MakeMKV's signed hash list")))?;
    inst.log.info(&format!(
        "  {} MakeMKV {} (verified with MakeMKV's PGP signature; not notarized by Apple) ...",
        if installed.is_some() {
            "Updating to"
        } else {
            "Installing"
        },
        release.version
    ));
    let dmg = inst.fetch(&Download {
        name: dmg_name,
        url: release.mac_dmg.clone(),
        sha256,
        size: None,
    })?;

    let mount = inst.tools_dir.join("makemkv-mount");
    let _ = inst
        .runner
        .output(Path::new("/usr/bin/hdiutil"), &detach_args(&mount));
    std::fs::create_dir_all(&mount)?;
    let out = inst
        .runner
        .output(Path::new("/usr/bin/hdiutil"), &attach_args(&dmg, &mount))?;
    if !out.success() {
        return Err(Error::Tool(format!(
            "Mounting {} failed: {}",
            dmg.display(),
            out.stderr.trim()
        )));
    }
    let copied = (|| -> Result<()> {
        let app = Path::new(MAC_APP);
        let staging = app.with_file_name(".MakeMKV.app.new");
        let old = app.with_file_name(".MakeMKV.app.old");
        let _ = std::fs::remove_dir_all(&staging);
        let _ = std::fs::remove_dir_all(&old);
        let args: Vec<OsString> = vec![mount.join("MakeMKV.app").into(), staging.clone().into()];
        let out = inst.runner.output(Path::new("/usr/bin/ditto"), &args)?;
        if !out.success() {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(Error::Tool(format!(
                "Copying MakeMKV to /Applications failed: {}",
                out.stderr.trim()
            )));
        }
        // Swap in the new app, so a failure never leaves MakeMKV half-copied
        if app.exists() {
            std::fs::rename(app, &old)?;
        }
        if let Err(e) = std::fs::rename(&staging, app) {
            let _ = std::fs::rename(&old, app);
            return Err(e.into());
        }
        let _ = std::fs::remove_dir_all(&old);
        Ok(())
    })();
    let _ = inst
        .runner
        .output(Path::new("/usr/bin/hdiutil"), &detach_args(&mount));
    let _ = std::fs::remove_dir(&mount);
    copied?;
    find_makemkvcon()
        .ok_or_else(|| Error::Tool("makemkvcon not found after installing MakeMKV".into()))
}

/// One line of `makemkvcon -r` ("robot mode") output: `KIND:field,field,"text",…`, with
/// quoted fields unescaped.
pub fn robot_fields(line: &str) -> Option<(&str, Vec<String>)> {
    let (kind, rest) = line.split_once(':')?;
    let mut fields = Vec::new();
    let mut cur = String::new();
    let (mut quoted, mut escaped) = (false, false);
    for c in rest.chars() {
        match c {
            _ if escaped => {
                cur.push(c);
                escaped = false;
            }
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    fields.push(cur);
    Some((kind, fields))
}

/// Whether MakeMKV may rip (RIPPING.md): registered or in its trial, yes; expired, no.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum Registration {
    /// MakeMKV hasn't read a disc for Upscale yet.
    Unknown,
    /// `MSG:5050`, "Evaluation version, N day(s) out of M remaining".
    Trial { days_left: u32, days: u32 },
    /// MakeMKV reports no expiry date for keys; `since` is when the key was entered through
    /// Upscale (`YYYY-MM-DD`), when known.
    Registered {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since: Option<String>,
    },
    /// The trial ran out, or the key expired or was refused; MakeMKV's own message.
    Expired { message: String },
}

impl Registration {
    pub fn can_rip(&self) -> bool {
        !matches!(self, Registration::Expired { .. })
    }
}

/// The status from a `makemkvcon -r info` run that opened a disc (`TCOUNT`): the evaluation
/// message gives the trial, an expiry message ends it, and a disc read without either means a
/// key is in place. A run that didn't open a disc says nothing (`None`).
///
/// Verified on a trial (`MSG:5050`) and after registering (no evaluation message). The texts
/// treated as expiry are MakeMKV's documented wording, not yet seen; tighten them once captured.
pub fn registration_from_output(output: &str) -> Option<Registration> {
    let mut trial = None;
    let mut opened = false;
    for line in output.lines() {
        let Some((kind, f)) = robot_fields(line) else {
            continue;
        };
        match kind {
            "TCOUNT" => opened = true,
            "MSG" => {
                let code: u32 = f.first()?.parse().unwrap_or(0);
                let text = f.get(3).cloned().unwrap_or_default();
                if code == 5050 {
                    let n = |i: usize| f.get(i).and_then(|v| v.parse().ok()).unwrap_or(0);
                    trial = Some((n(5), n(6)));
                }
                let lower = text.to_lowercase();
                if lower.contains("expired")
                    || lower.contains("too old")
                    || lower.contains("key is invalid")
                {
                    return Some(Registration::Expired { message: text });
                }
            }
            _ => {}
        }
    }
    match trial {
        Some((0, _)) => Some(Registration::Expired {
            message: "MakeMKV's evaluation period has ended.".into(),
        }),
        Some((days_left, days)) => Some(Registration::Trial { days_left, days }),
        None if opened => Some(Registration::Registered { since: None }),
        None => None,
    }
}

/// `h:mm:ss` → seconds.
fn hms(s: &str) -> f64 {
    s.split(':')
        .try_fold(0.0, |acc, part| {
            part.trim().parse::<f64>().map(|v| acc * 60.0 + v)
        })
        .unwrap_or(0.0)
}

/// The disc from `makemkvcon -r info` (RIPPING.md, "Reading the disc"): `CINFO` (disc),
/// `TINFO` (titles) and `SINFO` (streams). Attribute ids checked on a real disc: 2 name,
/// 8 chapters, 9 duration, 11 bytes, 16 playlist, 26 segment map; streams: 1 type, 3 language.
pub fn parse_info(output: &str, source: &str) -> crate::disc::Disc {
    use crate::disc::{Disc, Title, Track, TrackKind};
    let (mut title_name, mut volume) = (String::new(), String::new());
    let mut titles: Vec<Title> = Vec::new();
    let title = |titles: &mut Vec<Title>, index: u32| -> usize {
        if let Some(i) = titles.iter().position(|t| t.index == index) {
            return i;
        }
        titles.push(Title {
            index,
            seconds: 0.0,
            bytes: 0,
            chapters: 0,
            source: String::new(),
            segments: vec![],
            tracks: vec![],
        });
        titles.len() - 1
    };
    for line in output.lines() {
        let Some((kind, f)) = robot_fields(line) else {
            continue;
        };
        let num = |i: usize| f.get(i).and_then(|v| v.parse::<u32>().ok());
        let value = f.last().cloned().unwrap_or_default();
        match kind {
            "CINFO" => match num(0) {
                Some(2) => title_name = value,
                Some(32) => volume = value,
                _ => {}
            },
            "TINFO" => {
                let (Some(t), Some(id)) = (num(0), num(1)) else {
                    continue;
                };
                let i = title(&mut titles, t);
                let t = &mut titles[i];
                match id {
                    8 => t.chapters = value.parse().unwrap_or(0),
                    9 => t.seconds = hms(&value),
                    11 => t.bytes = value.parse().unwrap_or(0),
                    16 => t.source = value,
                    26 => {
                        t.segments = value
                            .split(',')
                            .filter_map(|v| v.trim().parse().ok())
                            .collect()
                    }
                    _ => {}
                }
            }
            "SINFO" => {
                let (Some(t), Some(stream), Some(id)) = (num(0), num(1), num(2)) else {
                    continue;
                };
                let i = title(&mut titles, t);
                let tracks = &mut titles[i].tracks;
                while tracks.len() <= stream as usize {
                    tracks.push(Track {
                        kind: TrackKind::Video,
                        language: String::new(),
                    });
                }
                let track = &mut tracks[stream as usize];
                match id {
                    1 => {
                        track.kind = match value.as_str() {
                            "Audio" => TrackKind::Audio,
                            "Subtitles" => TrackKind::Subtitle,
                            _ => TrackKind::Video,
                        }
                    }
                    3 => track.language = value,
                    _ => {}
                }
            }
            _ => {}
        }
    }
    titles.sort_by_key(|t| t.index);
    Disc {
        name: if title_name.is_empty() {
            volume
        } else {
            title_name
        },
        titles,
        source: source.to_string(),
    }
}

/// The messages `makemkvcon` printed (robot `MSG` texts), for showing the user its own words.
pub fn messages(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(robot_fields)
        .filter(|(k, _)| *k == "MSG")
        .filter_map(|(_, f)| f.get(3).cloned())
        .filter(|t| !t.is_empty())
        .collect()
}

/// `makemkvcon -r reg <file>`: the key goes in through a file, never on a command line.
pub fn reg_args(key_file: &Path) -> Vec<OsString> {
    vec!["-r".into(), "reg".into(), key_file.into()]
}

/// Enters the user's key into MakeMKV. The key is written to a file only the user can read,
/// passed to `makemkvcon reg`, and deleted; it's never logged or stored by Upscale. Returns
/// MakeMKV's own messages; an error when it refused the key.
pub fn register(
    runner: &dyn Runner,
    makemkvcon: &Path,
    key: &str,
    temp_dir: &Path,
) -> Result<Vec<String>> {
    let key = key.trim();
    if key.is_empty() {
        return Err(Error::Tool("Enter a MakeMKV key.".into()));
    }
    std::fs::create_dir_all(temp_dir)?;
    let file = temp_dir.join(format!("makemkv-key-{}.txt", std::process::id()));
    write_private(&file, key)?;
    let out = runner.output(makemkvcon, &reg_args(&file));
    let _ = std::fs::remove_file(&file);
    let out = out?;
    let msgs = messages(&out.stdout);
    if !out.success() {
        let why = msgs
            .last()
            .cloned()
            .unwrap_or_else(|| out.stderr.trim().to_string());
        return Err(Error::Tool(format!("MakeMKV didn't accept the key: {why}")));
    }
    Ok(msgs)
}

fn write_private(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut f = opts.open(path)?;
    f.write_all(text.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disc::TrackKind;

    fn fixture(name: &str) -> String {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/makemkv")
            .join(name);
        std::fs::read_to_string(p).unwrap()
    }

    #[test]
    fn the_download_page_names_the_release() {
        assert_eq!(
            parse_download_page(&fixture("download-page.html")),
            Some(Release {
                version: "2.0.0".into(),
                mac_dmg: "https://www.makemkv.com/download/makemkv_v2.0.0_osx.dmg".into(),
                windows_exe: "https://www.makemkv.com/download/Setup_MakeMKV_v2.0.0.exe".into(),
                hashes: "https://www.makemkv.com/download/makemkv-sha-2.0.0.txt".into(),
            })
        );
        assert_eq!(parse_download_page("<html></html>"), None);
    }

    #[test]
    fn the_signed_hash_list_verifies_and_a_changed_one_does_not() {
        let signed = fixture("makemkv-sha-2.0.0.txt");
        let hashes = verified_hashes(&signed).unwrap();
        assert!(hashes.contains(&(
            "e3adad095fb9344e23870d99fd9770a187816332c5a2aa1b226c0bd4a922867a".to_string(),
            "makemkv_v2.0.0_osx.dmg".to_string()
        )));
        assert_eq!(hashes.len(), 4);
        // One changed digit in a hash: the signature no longer matches
        let tampered = signed.replace("e3adad09", "e3adad0a");
        assert!(verified_hashes(&tampered).is_err());
        // An unsigned list is refused too
        assert!(verified_hashes("e3adad095fb9344e23870d99fd9770a187816332c5a2aa1b226c0bd4a922867a  makemkv_v2.0.0_osx.dmg").is_err());
    }

    #[test]
    fn the_pinned_key_is_makemkvs() {
        let key = signing_key().unwrap();
        let ids: Vec<String> = key
            .details
            .users
            .iter()
            .map(|u| String::from_utf8_lossy(u.id.id()).into_owned())
            .collect();
        assert_eq!(ids, ["MakeMKV (signature) <support@makemkv.com>"]);
    }

    #[test]
    fn sample_movie() {
        let disc = parse_info(&fixture("sample-movie-info.txt"), "dev:/dev/rdisk8");
        assert_eq!(
            (disc.name.as_str(), disc.titles.len(), disc.source.as_str()),
            ("Sample Movie", 18, "dev:/dev/rdisk8")
        );
        let feature = &disc.titles[2];
        assert_eq!(
            (feature.seconds, feature.bytes, feature.chapters),
            (2.0 * 3600.0 + 8.0 * 60.0 + 33.0, 32169320448, 28)
        );
        assert_eq!(
            (feature.source.as_str(), feature.segments.as_slice()),
            ("00100.mpls", &[20][..])
        );
        assert_eq!(feature.tracks[0].kind, TrackKind::Video);
        assert_eq!(
            (feature.tracks[1].kind, feature.tracks[1].language.as_str()),
            (TrackKind::Audio, "eng")
        );
        assert!(feature.tracks.iter().any(|t| t.kind == TrackKind::Subtitle));

        // The picker chooses the feature, not the bonus features or another playlist
        let s = crate::disc::choose_titles(&disc);
        let ticked: Vec<u32> = s
            .picks
            .iter()
            .filter(|p| p.ticked)
            .map(|p| p.index)
            .collect();
        assert_eq!((s.kind, ticked), (crate::disc::DiscKind::Movie, vec![2]));
        let langs: Vec<String> = crate::disc::languages(&disc, &s)
            .into_iter()
            .map(|(c, _)| c)
            .collect();
        assert!(
            langs.starts_with(&["eng".to_string()]) && langs.contains(&"spa".to_string()),
            "{langs:?}"
        );
    }

    #[test]
    fn robot_lines() {
        let (kind, f) = robot_fields(
            r#"MSG:5050,0,2,"Evaluation version, 30 day(s) out of 30 remaining","x","30","30""#,
        )
        .unwrap();
        assert_eq!(kind, "MSG");
        assert_eq!(
            f,
            [
                "5050",
                "0",
                "2",
                "Evaluation version, 30 day(s) out of 30 remaining",
                "x",
                "30",
                "30"
            ]
        );
        let (_, f) = robot_fields(r#"MSG:2010,0,1,"Drive \"BD-RE\" opened""#).unwrap();
        assert_eq!(f[3], r#"Drive "BD-RE" opened"#);
        assert_eq!(robot_fields("no colon"), None);
    }

    #[test]
    fn registration_status() {
        // Captured on this Mac during the trial (serial numbers removed)
        let trial = fixture("sample-movie-info.txt");
        assert_eq!(
            registration_from_output(&trial),
            Some(Registration::Trial {
                days_left: 30,
                days: 30
            })
        );
        assert!(
            Registration::Trial {
                days_left: 3,
                days: 30
            }
            .can_rip()
        );
        // Captured after registering: the disc opens with no evaluation message
        assert_eq!(
            registration_from_output(&fixture("sample-movie-info-registered.txt")),
            Some(Registration::Registered { since: None })
        );
        // The last trial day used up, or an expiry message
        let over = trial.replace(r#""30","30""#, r#""0","30""#);
        assert!(!registration_from_output(&over).unwrap().can_rip());
        let expired = r#"MSG:5095,0,0,"Evaluation period has expired.","x"
TCOUNT:0"#;
        assert_eq!(
            registration_from_output(expired),
            Some(Registration::Expired {
                message: "Evaluation period has expired.".into()
            })
        );
        // The drive list alone opens no disc and says nothing
        assert_eq!(
            registration_from_output("DRV:0,2,999,12,\"x\",\"y\",\"/dev/rdisk8\""),
            None
        );
    }

    #[test]
    fn older_status_files_still_load() {
        let old: Registration = serde_json::from_str(r#"{"state":"registered"}"#).unwrap();
        assert_eq!(old, Registration::Registered { since: None });
        let new = Registration::Registered {
            since: Some("2026-11-03".into()),
        };
        assert_eq!(
            serde_json::to_string(&new).unwrap(),
            r#"{"state":"registered","since":"2026-11-03"}"#
        );
    }

    #[test]
    fn the_key_never_goes_on_the_command_line() {
        let args: Vec<String> = reg_args(Path::new("/t/makemkv-key-1.txt"))
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["-r", "reg", "/t/makemkv-key-1.txt"]);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("k.txt");
        write_private(&file, "T-secret").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert!(
            write_private(&file, "again").is_err(),
            "an existing file is never reused"
        );
    }

    #[test]
    fn hdiutil_arguments() {
        let s = |v: Vec<OsString>| {
            v.iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            s(attach_args(Path::new("/t/m.dmg"), Path::new("/t/mnt"))),
            [
                "attach",
                "-nobrowse",
                "-readonly",
                "-noautoopen",
                "-mountpoint",
                "/t/mnt",
                "/t/m.dmg"
            ]
        );
        assert_eq!(
            s(detach_args(Path::new("/t/mnt"))),
            ["detach", "/t/mnt", "-force"]
        );
    }
}
