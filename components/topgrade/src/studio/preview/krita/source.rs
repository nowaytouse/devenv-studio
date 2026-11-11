use color_eyre::eyre::{Context, ContextCompat, Result, bail, ensure};
use regex::Regex;
use serde::Deserialize;
use std::{collections::BTreeMap, fs, path::Path, process::Command, sync::LazyLock, time::Duration};

use super::super::{bounded_output, download_to, tool_command, trusted_https};

pub(super) const CDN: &str = "https://cdn.kde.org/ci-builds/graphics/krita/master/macos-universal/";
pub(super) const CHANNEL: &str = "master/macos-universal";
const GIT: &str = "https://invent.kde.org/graphics/krita.git";
const INVENT: &[&str] = &["invent.kde.org"];
const CDN_HOSTS: &[&str] = &["cdn.kde.org", "origin.cdn.kde.org"];
const MAX_LISTING: u64 = 8 * 1024 * 1024;
pub(super) const MAX_IMAGE: u64 = 1_500_000_000;

#[derive(Debug)]
pub(super) struct Release {
    pub(super) name: String,
    pub(super) version: String,
    pub(super) sha: String,
    pub(super) url: String,
    pub(super) modified_utc: Option<String>,
}

pub(super) fn valid_next_version(version: &str) -> bool {
    static VERSION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\A(?:[6-9]|[1-9][0-9]+)\.[0-9]+\.[0-9]+(?:[-+][a-zA-Z0-9][a-zA-Z0-9.+_-]*)?\z")
            .expect("valid Krita Next version pattern")
    });
    VERSION.is_match(version)
}

fn releases(html: &str) -> Result<Vec<Release>> {
    static LINK: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)<a\b[^>]*?\bhref\s*=\s*(?:"([^"]+)"|'([^']+)')[^>]*>[^<]*</a>"#)
            .expect("valid KDE artifact link pattern")
    });
    static NAME: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\Akrita-([a-z0-9][a-z0-9.+_-]{0,96})-([a-f0-9]{7,40})\.dmg\z")
            .expect("valid Krita artifact name pattern")
    });
    let mut releases = BTreeMap::new();
    for entry in LINK.captures_iter(html) {
        let href = entry.get(1).or_else(|| entry.get(2)).expect("href capture").as_str();
        let name = href.strip_prefix(CDN).unwrap_or(href);
        let Some(parts) = NAME.captures(name) else { continue };
        if !valid_next_version(&parts[1]) {
            continue;
        }
        releases.entry(name).or_insert_with(|| Release {
            name: name.into(),
            version: parts[1].into(),
            sha: parts[2].to_ascii_lowercase(),
            url: format!("{CDN}{name}"),
            modified_utc: None,
        });
    }
    ensure!(
        !releases.is_empty(),
        "Official Krita Next {CHANNEL} has no matching macOS build"
    );
    ensure!(
        releases.len() <= 20,
        "Official Krita Next {CHANNEL} DMG listing is ambiguous"
    );
    Ok(releases.into_values().collect())
}

pub(super) fn release(workspace: &Path) -> Result<Release> {
    let html = workspace.join("krita-index.html");
    download_to(CDN, CDN_HOSTS, &html, MAX_LISTING, 30, false, true)?;
    let candidates = releases(&fs::read_to_string(html)?)?;
    let mut by_date = BTreeMap::new();
    let headers = workspace.join("krita-headers");
    for mut release in candidates {
        let modified = download_to(&release.url, CDN_HOSTS, &headers, 64 * 1024, 25, true, false)?;
        let date = chrono::DateTime::parse_from_rfc2822(&modified)
            .wrap_err("Cannot verify official Krita publication without valid Last-Modified")?;
        release.modified_utc = Some(date.with_timezone(&chrono::Utc).to_rfc3339());
        ensure!(
            by_date.insert(date, release).is_none(),
            "Multiple Krita snapshots share one modification time"
        );
    }
    Ok(by_date.pop_last().expect("nonempty release candidates").1)
}

pub(super) fn image(release: &Release, target: &Path) -> Result<()> {
    ensure!(
        release.url == format!("{CDN}{}", release.name),
        "Krita image does not match its official name"
    );
    download_to(&release.url, CDN_HOSTS, target, MAX_IMAGE, 1200, false, false)?;
    ensure!(
        fs::metadata(target)?.len() >= 1_000_000,
        "Krita image is unexpectedly small"
    );
    Ok(())
}

#[derive(PartialEq, Eq)]
pub(super) enum Relation {
    Same,
    Newer,
    Older,
    Diverged,
}

pub(super) fn valid_sha(sha: &str) -> bool {
    (7..=40).contains(&sha.len()) && sha.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn relation_from_base(local: &str, remote: &str, base: &str) -> Result<Relation> {
    ensure!(
        base.len() == 40 && valid_sha(base),
        "KDE returned an invalid merge-base SHA"
    );
    let base = base.to_ascii_lowercase();
    Ok(match (base.starts_with(local), base.starts_with(remote)) {
        (true, true) => Relation::Same,
        (true, false) => Relation::Newer,
        (false, true) => Relation::Older,
        (false, false) => Relation::Diverged,
    })
}

fn official_relation(local: &str, remote: &str, workspace: &Path) -> Result<Relation> {
    #[derive(Deserialize)]
    struct Base {
        id: String,
    }
    let url = format!(
        "https://invent.kde.org/api/v4/projects/graphics%2Fkrita/repository/merge_base?refs%5B%5D={local}&refs%5B%5D={remote}"
    );
    let response = workspace.join("krita-merge-base.json");
    download_to(&url, INVENT, &response, 32 * 1024, 25, false, true)?;
    let base: Base = serde_json::from_slice(&fs::read(response)?)?;
    relation_from_base(local, remote, &base.id)
}

fn git() -> Result<Command> {
    let mut command = tool_command(&super::super::super::resolve_tool("git")?)?;
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(["-c", "http.sslVerify=true", "-c", "http.followRedirects=false"]);
    Ok(command)
}

fn git_status(command: &mut Command) -> Result<i32> {
    let (status, _, _) = bounded_output(command, Duration::from_secs(90), 512 * 1024)?;
    status.code().context("KDE Git command ended by signal")
}

fn git_relation(local: &str, remote: &str, workspace: &Path) -> Result<Relation> {
    ensure!(trusted_https(GIT, INVENT), "Unexpected KDE Git origin");
    let repo = workspace.join("krita-history");
    ensure!(
        git_status(
            git()?
                .args([
                    "clone",
                    "--quiet",
                    "--no-checkout",
                    "--single-branch",
                    "--branch",
                    "master",
                    "--filter=blob:none",
                    "--depth=128",
                    GIT
                ])
                .arg(&repo)
        )? == 0,
        "Official KDE Git history could not be cloned"
    );
    for depth in [0, 384, 1536] {
        let mut command = git()?;
        command.arg("-C").arg(&repo);
        if depth != 0 {
            ensure!(
                git_status(command.args([
                    "fetch",
                    "--quiet",
                    "--no-tags",
                    &format!("--deepen={depth}"),
                    "origin",
                    "master"
                ]))? == 0,
                "Could not deepen official KDE Git history"
            );
        }
        let has_commit = |sha: &str| -> Result<bool> {
            Ok(git_status(
                git()?
                    .arg("-C")
                    .arg(&repo)
                    .args(["cat-file", "-e", &format!("{sha}^{{commit}}")]),
            )? == 0)
        };
        if !(has_commit(local)? && has_commit(remote)?) {
            continue;
        }
        let ancestor = |first: &str, second: &str| -> Result<bool> {
            match git_status(
                git()?
                    .arg("-C")
                    .arg(&repo)
                    .args(["merge-base", "--is-ancestor", first, second]),
            )? {
                0 => Ok(true),
                1 => Ok(false),
                _ => bail!("KDE Git ancestry could not be verified"),
            }
        };
        match (ancestor(local, remote)?, ancestor(remote, local)?) {
            (true, true) => return Ok(Relation::Same),
            (true, false) => return Ok(Relation::Newer),
            (false, true) => return Ok(Relation::Older),
            (false, false) => {}
        }
        // A negative shallow ancestry result is only final after the last depth.
        if depth == 1536 {
            return Ok(Relation::Diverged);
        }
    }
    bail!("Krita commits not found within bounded KDE Git history")
}

pub(super) fn relation(local: &str, remote: &str, workspace: &Path) -> Result<Relation> {
    ensure!(valid_sha(local) && valid_sha(remote), "Invalid Krita release SHA");
    let local = local.to_ascii_lowercase();
    let remote = remote.to_ascii_lowercase();
    official_relation(&local, &remote, workspace).or_else(|official| {
        git_relation(&local, &remote, workspace)
            .wrap_err_with(|| format!("Official KDE merge-base unavailable ({official:#}) and Git verification failed"))
    })
}

#[cfg(test)]
#[test]
fn official_listing_and_merge_base_do_not_guess_from_hash_order() -> Result<()> {
    let html = "<a href=\"krita-6.1.0-prealpha-32bfa3f.dmg\">artifact</a> 2026-10-03 14:11 <a href=\"https://lookalike.invalid/krita-6.2.0-ab12345.dmg\">other</a>";
    let found = releases(html)?;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].sha, "32bfa3f");
    assert!(found[0].modified_utc.is_none());
    let apache = "<tr><td><a href=\"krita-6.1.0-prealpha-2687e5d.dmg\">krita-6.1.0-prealpha-2687e5d.dmg</a></td><td align=\"right\">2026-10-24 02:22</td></tr>";
    assert_eq!(releases(apache)?[0].sha, "2687e5d");
    let mixed = format!(
        "{html} <a href=\"krita-5.4.0-prealpha-123abcd.dmg\">Qt5</a> \
         <a href=\"krita-6.2.0-prealpha-123abcd.dmg\">next series</a> \
         <a href=\"krita-7.0.0-prealpha-abcdef0.dmg\">future major</a> \
         <a href=\"../macos-universal-qt5/krita-7.0.0-1111111.dmg\">wrong feed</a>"
    );
    let versions: Vec<_> = releases(&mixed)?.into_iter().map(|release| release.version).collect();
    assert_eq!(versions, ["6.1.0-prealpha", "6.2.0-prealpha", "7.0.0-prealpha"]);
    assert!(releases("<a href=\"krita-5.4.0-prealpha-123abcd.dmg\">Qt5</a>").is_err());
    for version in ["6.1.0", "6.2.12-prealpha", "6.10.2+build.4", "7.0.0", "10.0.0-rc1"] {
        assert!(valid_next_version(version));
    }
    for version in ["6.1", "5.4.0", "6.1.0.1", "6.1.0other"] {
        assert!(!valid_next_version(version));
    }
    assert!(
        relation_from_base("32bfa3f", "ff12345", "32bfa3f000000000000000000000000000000000")
            .is_ok_and(|value| value == Relation::Newer)
    );
    assert!(relation_from_base("32bfa3f", "ff12345", "invalid").is_err());
    let full = "32bfa3f000000000000000000000000000000000";
    assert!(relation_from_base("32bfa3f", full, full).is_ok_and(|value| value == Relation::Same));
    Ok(())
}
