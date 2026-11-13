use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use color_eyre::eyre::{ContextCompat, Result, ensure};
use serde::{Deserialize, Serialize};

use super::files::{self, Fingerprint, Stamp};

const LIMIT: u64 = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    before: Option<Fingerprint>,
    after: Option<Fingerprint>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    repo: PathBuf,
    baseline: String,
    changes: BTreeMap<PathBuf, Change>,
    eligible: bool,
}

fn marker(run: &Path, stem: &str) -> Result<PathBuf> {
    Ok(run
        .parent()
        .context("Missing rolling cache")?
        .join(format!("{stem}.pending")))
}

fn receipt_path(run: &Path, stem: &str) -> PathBuf {
    run.join(format!("{stem}.receipt.json"))
}

fn read(path: &Path) -> Result<Vec<u8>> {
    let file = super::super::open_file(path, true, LIMIT)?;
    let mut data = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut data)?;
    ensure!(data.len() as u64 <= LIMIT, "Rolling receipt exceeds its size limit");
    Ok(data)
}

pub(super) fn invalidate(run: &Path, stem: &str) -> Result<()> {
    let marker = marker(run, stem)?;
    if super::super::exists(&marker)? {
        super::super::open_file(&marker, true, LIMIT)?;
        fs::remove_file(&marker)?;
        File::open(marker.parent().context("Missing rolling cache")?)?.sync_all()?;
    }
    Ok(())
}

impl Receipt {
    fn matches_status(&self, status: &str) -> bool {
        let Some(status) = status.strip_suffix('\0') else {
            return false;
        };
        !self.changes.is_empty()
            && status.split('\0').count() == self.changes.len()
            && status.split('\0').all(|record| {
                let Some(path) = record.get(3..).map(Path::new) else {
                    return false;
                };
                !record.contains('\u{fffd}')
                    && path.components().all(|part| matches!(part, Component::Normal(_)))
                    && self.changes.get(path).is_some_and(|change| {
                        (change.before.is_some() && change.after.is_some() && record.starts_with(" M "))
                            || (change.before.is_some() && change.after.is_none() && record.starts_with(" D "))
                    })
            })
    }
}

pub(super) fn pending(repo: &Path, run: &Path, stem: &str, status: &str) -> Result<Option<PathBuf>> {
    let marker = marker(run, stem)?;
    if status.is_empty() || !super::super::exists(&marker)? {
        return Ok(None);
    }
    let id = String::from_utf8(read(&marker)?)?;
    ensure!(
        !id.is_empty() && id.len() < 128 && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "Invalid rolling receipt reference"
    );
    let previous = marker.parent().context("Missing rolling cache")?.join(id);
    let path = receipt_path(&previous, stem);
    if !super::super::exists(&previous)? {
        return Ok(None);
    }
    super::super::check_dir(&previous, true)?;
    if !super::super::exists(&path)? {
        return Ok(None);
    }
    let receipt: Receipt = serde_json::from_slice(&read(&path)?)?;
    if receipt.repo != repo
        || !receipt.eligible
        || !receipt.matches_status(status)
        || files::git_head(repo)? != receipt.baseline
    {
        return Ok(None);
    }
    for (relative, change) in &receipt.changes {
        let file = repo.join(relative);
        let current = match fs::symlink_metadata(&file) {
            Ok(info) if info.is_file() => {
                let Ok(fingerprint) = files::fingerprint(&file) else {
                    return Ok(None);
                };
                Some(fingerprint)
            }
            Ok(_) => return Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        if current != change.after {
            return Ok(None);
        }
    }
    if files::git_status(repo, "receipt readback")? != status || files::git_head(repo)? != receipt.baseline {
        return Ok(None);
    }
    Ok(Some(path))
}

pub(super) fn save(
    repo: &Path,
    run: &Path,
    stem: &str,
    baseline: String,
    snapshots: (&BTreeMap<PathBuf, Stamp>, &BTreeMap<PathBuf, Stamp>),
    status: &str,
    verified: bool,
) -> Result<()> {
    let (before, after) = snapshots;
    let changes = files::changed(before, after)
        .into_iter()
        .map(|path| {
            let change = Change {
                before: before.get(&path).map(Stamp::fingerprint),
                after: after.get(&path).map(Stamp::fingerprint),
            };
            (path, change)
        })
        .collect();
    let mut receipt = Receipt {
        repo: repo.to_path_buf(),
        baseline,
        changes,
        eligible: false,
    };
    receipt.eligible = verified && receipt.matches_status(status);
    let payload = serde_json::to_vec_pretty(&receipt)?;
    ensure!(payload.len() as u64 <= LIMIT, "Rolling receipt exceeds its size limit");
    let mut out = super::super::create_file(&receipt_path(run, stem))?;
    out.write_all(&payload)?;
    out.sync_all()?;
    File::open(run)?.sync_all()?;
    if receipt.eligible {
        let marker = marker(run, stem)?;
        let parent = marker.parent().context("Missing rolling cache")?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(run.file_name().context("Missing rolling run name")?.as_encoded_bytes())?;
        file.as_file().sync_all()?;
        file.persist_noclobber(&marker)?;
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}
