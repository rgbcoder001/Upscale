//! `CHANGELOG.md` is every `.github/release-notes/v*.md`, newest first, so users can catch up
//! in one place. The per-release files stay the source (GitHub releases and the app's update
//! banner use them); this test fails when the changelog is out of date.
//! Regenerate it with `UPDATE_CHANGELOG=1 cargo test -p upscale-core --test changelog`.

use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// `v0.1.10` → `(0, 1, 10)`, for ordering.
fn version(name: &str) -> Option<(u32, u32, u32)> {
    let mut parts = name.strip_prefix('v')?.split('.').map(|p| p.parse().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// Moves every Markdown heading down a level (outside code blocks), so a release's own
/// `## Files` sits under its `## v0.x.y`.
fn demote(notes: &str) -> String {
    let mut in_code = false;
    notes
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("```") {
                in_code = !in_code;
            }
            if !in_code && line.starts_with('#') {
                format!("#{line}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn expected() -> String {
    let dir = repo().join(".github").join("release-notes");
    let mut releases: Vec<((u32, u32, u32), String, PathBuf)> = std::fs::read_dir(&dir)
        .expect("release-notes folder")
        .filter_map(|e| {
            let path = e.ok()?.path();
            let name = path.file_stem()?.to_str()?.to_string();
            (path.extension()? == "md").then_some((version(&name)?, name, path))
        })
        .collect();
    releases.sort_by_key(|r| std::cmp::Reverse(r.0));
    let mut out = String::from(
        "# Changelog\n\nEvery Upscale release, newest first. Each section is the release's notes \
         from `.github/release-notes/`, as shown on GitHub and in the app's update banner.\n",
    );
    for (_, name, path) in releases {
        let notes = std::fs::read_to_string(&path).expect("release notes");
        out.push_str(&format!("\n## {name}\n\n{}\n", demote(notes.trim())));
    }
    out
}

#[test]
fn changelog_has_every_release() {
    let want = expected();
    let path = repo().join("CHANGELOG.md");
    if std::env::var_os("UPDATE_CHANGELOG").is_some() {
        std::fs::write(&path, &want).expect("write CHANGELOG.md");
        return;
    }
    let have = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        have == want,
        "CHANGELOG.md is out of date with .github/release-notes. Run: UPDATE_CHANGELOG=1 cargo test -p upscale-core --test changelog"
    );
}

#[test]
fn headings_move_down_outside_code() {
    assert_eq!(
        demote("Intro\n## Files\n```sh\n# a comment\n```\n### Deep"),
        "Intro\n### Files\n```sh\n# a comment\n```\n#### Deep"
    );
    assert_eq!(version("v0.1.10"), Some((0, 1, 10)));
    assert_eq!(version("notes"), None);
}
