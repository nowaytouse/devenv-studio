//! Follow the official stable release tag without changing the reviewed component.
use super::mac::git;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::{ErrorKind, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use studio_deployment::maintenance::{account_home, topgrade_upstream::official_tag_refs};

const PIN_PATH: &str = "topgrade-upstream.json";
const MANIFEST_PATH: &str = "components/topgrade/Cargo.toml";
const AGENT_LABEL: &str = "org.devenv-studio.topgrade-upstream";
const STATE_DIR: &str = "Library/Application Support/DevEnv Studio Topgrade Upstream";

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Release {
    tag: String,
    tag_object: String,
    commit: String,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Current,
    NewRelease,
    PinChanged,
    PinMissing,
}

#[derive(Serialize)]
struct Report {
    schema_version: u32,
    checked_at_unix_seconds: u64,
    studio_commit: String,
    pinned: Release,
    latest: Release,
    status: Status,
}

fn sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn stable_version(tag: &str) -> Option<[u64; 3]> {
    let mut parts = tag.strip_prefix('v')?.split('.');
    let mut version = [0; 3];
    for component in &mut version {
        let part = parts.next()?;
        if !part.bytes().all(|byte| byte.is_ascii_digit())
            || (part.len() > 1 && part.starts_with('0'))
        {
            return None;
        }
        *component = part.parse().ok()?;
    }
    parts.next().is_none().then_some(version)
}

fn reviewed_pin(pin: &str, manifest: &str) -> Result<Release> {
    let release: Release = serde_json::from_str(pin).context("Invalid Topgrade upstream pin")?;
    ensure!(
        stable_version(&release.tag).is_some() && sha(&release.tag_object) && sha(&release.commit),
        "Topgrade pin requires a stable tag and full tag object and commit SHAs"
    );
    // The component declares a literal version in its [package] table.
    let mut package = false;
    let mut versions = manifest.lines().filter_map(|line| {
        let line = line.trim();
        if line.starts_with('[') {
            package = line == "[package]";
        }
        let (key, value) = line.split_once('=')?;
        (package && key.trim() == "version").then_some(value.trim())
    });
    let expected = format!("\"{}\"", &release.tag[1..]);
    ensure!(
        versions.next() == Some(expected.as_str()) && versions.next().is_none(),
        "Reviewed Topgrade component version does not match the upstream pin"
    );
    Ok(release)
}

#[derive(Default)]
struct TagRefs<'a> {
    object: Option<&'a str>,
    peeled: Option<&'a str>,
}

impl TagRefs<'_> {
    fn release(&self, tag: &str) -> Result<Release> {
        Ok(Release {
            tag: tag.to_owned(),
            tag_object: self
                .object
                .context("Missing upstream tag object")?
                .to_owned(),
            commit: self
                .peeled
                .context("Missing peeled upstream tag commit")?
                .to_owned(),
        })
    }
}

fn compare_refs(pinned: &Release, refs: &str) -> Result<(Release, Status)> {
    let mut tags = BTreeMap::<[u64; 3], (&str, TagRefs<'_>)>::new();
    for line in refs.lines() {
        let (id, reference) = line
            .split_once('\t')
            .context("Malformed upstream Git ref")?;
        let tag = reference
            .strip_prefix("refs/tags/")
            .context("Unexpected upstream Git ref")?;
        let (tag, peeled) = tag
            .strip_suffix("^{}")
            .map_or((tag, false), |tag| (tag, true));
        let Some(version) = stable_version(tag) else {
            continue;
        };
        ensure!(sha(id), "Malformed upstream Git object ID");
        let (_, entry) = tags
            .entry(version)
            .or_insert_with(|| (tag, TagRefs::default()));
        let slot = if peeled {
            &mut entry.peeled
        } else {
            &mut entry.object
        };
        ensure!(slot.replace(id).is_none(), "Duplicate upstream Git ref");
    }
    let (tag, entry) = tags
        .values()
        .next_back()
        .context("No official stable Topgrade tags")?;
    let latest = entry.release(tag)?;
    let version = stable_version(&pinned.tag).context("Invalid pinned Topgrade release")?;
    let status = match tags.get(&version) {
        None => Status::PinMissing,
        Some((tag, entry)) if entry.release(tag)? != *pinned => Status::PinChanged,
        Some(_) if latest != *pinned => Status::NewRelease,
        Some(_) => Status::Current,
    };
    Ok((latest, status))
}

fn snapshot(checkout: &Path) -> Result<String> {
    ensure!(
        checkout.is_absolute()
            && checkout.canonicalize()? == checkout
            && checkout.to_str()
                == Some(git(checkout, &["rev-parse", "--show-toplevel"])?.as_str()),
        "Expected a canonical absolute Studio checkout"
    );
    let revision = git(checkout, &["rev-parse", "HEAD"])?;
    ensure!(sha(&revision), "Invalid Studio commit");
    Ok(revision)
}

fn read_pin(checkout: &Path, revision: &str) -> Result<Release> {
    let pin = git(checkout, &["show", &format!("{revision}:{PIN_PATH}")])?;
    let manifest = git(checkout, &["show", &format!("{revision}:{MANIFEST_PATH}")])?;
    reviewed_pin(&pin, &manifest)
}

fn check(checkout: &Path, revision: &str) -> Result<Report> {
    let pinned = read_pin(checkout, revision)?;
    let (latest, status) = compare_refs(&pinned, &official_tag_refs()?)?;
    Ok(Report {
        schema_version: 1,
        checked_at_unix_seconds: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        studio_commit: revision.to_owned(),
        pinned,
        latest,
        status,
    })
}

pub(super) fn verify_current(checkout: &Path, revision: &str) -> Result<()> {
    let report = check(checkout, revision)?;
    ensure!(
        report.status == Status::Current,
        "Reviewed Topgrade release {} is not the current verified upstream pin (latest {}, {:?})",
        report.pinned.tag,
        report.latest.tag,
        report.status
    );
    Ok(())
}

fn status_directory(home: &Path) -> Result<PathBuf> {
    let directory = home.join(STATE_DIR);
    let parent = directory
        .parent()
        .context("Missing upstream status parent")?;
    let uid = home.metadata()?.uid();
    let metadata = parent.symlink_metadata()?;
    ensure!(
        metadata.is_dir()
            && metadata.uid() == uid
            && metadata.mode() & 0o022 == 0
            && parent.canonicalize()? == parent,
        "Expected the account-owned Application Support directory"
    );
    match fs::DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error).context("Create private upstream status directory"),
    }
    let metadata = directory.symlink_metadata()?;
    ensure!(
        metadata.is_dir() && metadata.uid() == uid && metadata.mode() & 0o077 == 0,
        "Expected a private account-owned upstream status directory"
    );
    Ok(directory)
}

fn record(report: &Report, home: &Path) -> Result<PathBuf> {
    let directory = status_directory(home)?;
    let target = directory.join("status-v1.json");
    match target.symlink_metadata() {
        Ok(metadata) => ensure!(
            metadata.is_file()
                && metadata.uid() == home.metadata()?.uid()
                && metadata.mode() & 0o077 == 0,
            "Expected a private account-owned upstream status file"
        ),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("Inspect existing upstream status file"),
    }
    let mut temporary = tempfile::NamedTempFile::new_in(&directory)?;
    serde_json::to_writer_pretty(temporary.as_file_mut(), report)?;
    temporary.write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    temporary.persist(&target)?;
    fs::File::open(directory)?.sync_all()?;
    Ok(target)
}

fn xml_path(path: &Path) -> Result<String> {
    let value = path.to_str().context("LaunchAgent paths must be UTF-8")?;
    ensure!(
        !value.chars().any(char::is_control),
        "LaunchAgent paths must not contain control characters"
    );
    Ok(value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;"))
}

fn launch_agent(executable: &Path, checkout: &Path) -> Result<String> {
    let executable = xml_path(executable)?;
    let checkout = xml_path(checkout)?;
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{AGENT_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{executable}</string>
        <string>topgrade-engine</string>
        <string>record-upstream</string>
        <string>{checkout}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>StartInterval</key>
    <integer>86400</integer>
    <key>Umask</key>
    <integer>63</integer>
</dict>
</plist>
"#
    ))
}

pub(super) fn execute(mode: &str, checkout: &Path) -> Result<()> {
    let revision = snapshot(checkout)?;
    if mode == "export-upstream-agent" {
        read_pin(checkout, &revision)?;
        let executable = std::env::current_exe()?.canonicalize()?;
        print!("{}", launch_agent(&executable, checkout)?);
        return Ok(());
    }
    let report = check(checkout, &revision)?;
    if mode == "record-upstream" {
        println!("{}", record(&report, &account_home()?)?.display());
    } else {
        println!("{}", serde_json::to_string_pretty(&report)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
