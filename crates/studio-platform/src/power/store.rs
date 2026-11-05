//! Bounded, authenticated power journals, independent of the registry log format.
use super::engine::{Event, Header, Journal, validate_header};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

const MAX_BYTES: usize = 512 * 1024;
const MAX_EVENTS: usize = 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    sequence: usize,
    previous: String,
    payload: Event,
    mac: String,
}
fn mac(sequence: usize, previous: &str, payload: &Event, key: &[u8; 32]) -> Result<blake3::Hash> {
    Ok(blake3::keyed_hash(
        key,
        &serde_json::to_vec(&(sequence, previous, payload))?,
    ))
}
fn encode(sequence: usize, previous: &str, event: &Event, key: &[u8; 32]) -> Result<Vec<u8>> {
    let signed = Envelope {
        sequence,
        previous: previous.into(),
        payload: event.clone(),
        mac: mac(sequence, previous, event, key)?.to_hex().to_string(),
    };
    let mut bytes = serde_json::to_vec(&signed)?;
    bytes.push(b'\n');
    Ok(bytes)
}
pub(super) struct Loaded {
    pub header: Header,
    pub intended: BTreeSet<usize>,
    pub status: String,
    pub count: usize,
    pub valid_len: u64,
    last_mac: String,
}
fn decode(bytes: &[u8], key: &[u8; 32]) -> Result<Loaded> {
    ensure!(bytes.len() <= MAX_BYTES, "Power recovery exceeds 512 KiB");
    let valid_len = bytes
        .iter()
        .rposition(|&b| b == b'\n')
        .map(|i| i + 1)
        .context("Incomplete power recovery header")?;
    let mut header: Option<Header> = None;
    let mut intended = BTreeSet::new();
    let mut applied = BTreeSet::new();
    let mut restore_pending = BTreeSet::new();
    let mut restored = BTreeSet::new();
    let mut finished = false;
    let mut restoring = false;
    let mut status = "interrupted".to_owned();
    let mut count = 0;
    let mut previous = String::new();
    for line in bytes[..valid_len].split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        ensure!(count < MAX_EVENTS, "Power recovery event limit exceeded");
        let envelope: Envelope =
            serde_json::from_slice(line).context("Invalid power recovery event")?;
        ensure!(
            envelope.sequence == count && envelope.previous == previous,
            "Power recovery sequence or hash chain mismatch"
        );
        let expected = mac(count, &previous, &envelope.payload, key)?;
        ensure!(
            blake3::Hash::from_hex(&envelope.mac).context("Invalid power recovery MAC")?
                == expected,
            "Power recovery integrity check failed"
        );
        match envelope.payload {
            Event::Header(h) => {
                ensure!(count == 0, "Repeated power recovery header");
                validate_header(&h)?;
                header = Some(h);
            }
            Event::ApplyIntent { index } => {
                let h = header.as_ref().context("Missing power recovery header")?;
                let next = h
                    .records
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.original != r.choice.value)
                    .nth(intended.len())
                    .map(|(i, _)| i);
                ensure!(
                    !finished
                        && !restoring
                        && next == Some(index)
                        && intended.len() == applied.len(),
                    "Invalid power apply intent"
                );
                intended.insert(index);
            }
            Event::Applied { index } => {
                ensure!(
                    !finished
                        && !restoring
                        && intended.contains(&index)
                        && intended.iter().nth(applied.len()) == Some(&index)
                        && applied.insert(index),
                    "Invalid power apply verification"
                );
            }
            Event::ApplyFinished { cancelled } => {
                let h = header.as_ref().context("Missing power recovery header")?;
                ensure!(!finished && !restoring, "Repeated power apply completion");
                ensure!(
                    cancelled
                        || applied.len()
                            == h.records
                                .iter()
                                .filter(|r| r.original != r.choice.value)
                                .count(),
                    "Partial power apply marked successful"
                );
                finished = true;
                status = if cancelled { "cancelled" } else { "applied" }.into();
            }
            Event::RestoreIntent { index } => {
                ensure!(
                    intended.contains(&index),
                    "Power restore lacks durable apply intent"
                );
                restoring = true;
                restore_pending.insert(index);
                status = "restore interrupted".into();
            }
            Event::Restored { index } => {
                ensure!(
                    restoring && restore_pending.remove(&index),
                    "Power restore lacks durable restore intent"
                );
                restored.insert(index);
            }
            Event::RestoreFinished { cancelled } => {
                ensure!(header.is_some(), "Missing power recovery header");
                ensure!(
                    cancelled || (restored == intended && restore_pending.is_empty()),
                    "Partial power restore marked successful"
                );
                restoring = true;
                status = if cancelled {
                    "restore cancelled"
                } else {
                    "restored"
                }
                .into();
            }
        }
        previous = envelope.mac;
        count += 1;
    }
    Ok(Loaded {
        header: header.context("Missing power recovery header")?,
        intended,
        status,
        count,
        valid_len: valid_len as u64,
        last_mac: previous,
    })
}
fn regular(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Power recovery file must be a regular file"
    );
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "Power recovery file cannot be a reparse point"
        );
    }
    Ok(())
}
pub(super) fn ensure_directory(path: &Path, create: bool) -> Result<()> {
    ensure!(path.is_absolute(), "Power data directory must be absolute");
    for part in path.ancestors() {
        match fs::symlink_metadata(part) {
            Ok(metadata) => {
                ensure!(
                    metadata.is_dir() && !metadata.file_type().is_symlink(),
                    "Power data directory cannot contain symbolic links"
                );
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    ensure!(
                        metadata.file_attributes() & 0x400 == 0,
                        "Power data directory cannot contain reparse points"
                    );
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {}
            Err(e) => return Err(e).context("Inspect power data directory"),
        }
    }
    if create {
        fs::create_dir_all(path).context("Create power recovery directory")?;
    }
    ensure!(
        fs::symlink_metadata(path)?.is_dir(),
        "Power data directory unavailable"
    );
    Ok(())
}
pub(super) fn mutation_lock(directory: &Path) -> Result<File> {
    ensure_directory(directory, true)?;
    let path = directory.join("mutation.lock");
    if path.try_exists()? {
        regular(&path)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    regular(&path)?;
    file.try_lock()
        .context("Another DevEnv Studio mutation is running")?;
    Ok(file)
}
pub(super) fn validate_key_path(directory: &Path) -> Result<()> {
    let path = directory.join("recovery.key");
    if path.try_exists()? {
        regular(&path)?;
    }
    Ok(())
}
pub(super) fn journal_path(path: &Path, directory: &Path) -> Result<PathBuf> {
    ensure_directory(directory, false)?;
    regular(path)?;
    let canonical = path.canonicalize()?;
    ensure!(
        canonical.parent() == Some(directory.canonicalize()?.as_path())
            && canonical.extension().is_some_and(|e| e == "jsonl"),
        "Power recovery must use this user's power recovery directory"
    );
    Ok(canonical)
}
pub(super) fn paths(directory: &Path) -> Result<Vec<PathBuf>> {
    ensure_directory(directory, false)?;
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "jsonl") {
            ensure!(paths.len() < 256, "Power recovery limit reached (256)");
            paths.push(journal_path(&path, directory)?);
        }
    }
    Ok(paths)
}
pub(super) fn load(path: &Path, key: &[u8; 32]) -> Result<Loaded> {
    regular(path)?;
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    decode(&bytes, key)
}
pub(super) struct FileJournal {
    file: Option<File>,
    path: PathBuf,
    key: [u8; 32],
    sequence: usize,
    bytes: usize,
    previous: String,
    pub has_intent: bool,
}
impl FileJournal {
    pub fn create(path: PathBuf, key: [u8; 32]) -> Result<Self> {
        ensure!(!path.try_exists()?, "Power recovery file already exists");
        ensure_directory(
            path.parent().context("Missing power recovery directory")?,
            false,
        )?;
        Ok(Self {
            file: None,
            path,
            key,
            sequence: 0,
            bytes: 0,
            previous: String::new(),
            has_intent: false,
        })
    }
    pub fn resume(path: PathBuf, key: [u8; 32], loaded: &Loaded) -> Result<Self> {
        regular(&path)?;
        // Windows append-only handles cannot truncate an incomplete journal tail.
        let mut file = OpenOptions::new().write(true).truncate(false).open(&path)?;
        file.set_len(loaded.valid_len)?;
        file.sync_all()?;
        file.seek(SeekFrom::Start(loaded.valid_len))?;
        Ok(Self {
            file: Some(file),
            path,
            key,
            sequence: loaded.count,
            bytes: loaded.valid_len as usize,
            previous: loaded.last_mac.clone(),
            has_intent: !loaded.intended.is_empty(),
        })
    }
}
impl Journal for FileJournal {
    fn append(&mut self, event: &Event) -> Result<()> {
        ensure!(
            self.sequence < MAX_EVENTS,
            "Power recovery event limit reached"
        );
        let bytes = encode(self.sequence, &self.previous, event, &self.key)?;
        ensure!(
            self.bytes + bytes.len() <= MAX_BYTES,
            "Power recovery byte limit reached"
        );
        if self.file.is_none() {
            let mut options = OpenOptions::new();
            options.read(true).write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            self.file = Some(options.open(&self.path)?);
        }
        let file = self
            .file
            .as_mut()
            .context("Power recovery file unavailable")?;
        file.write_all(&bytes)?;
        file.sync_all()
            .context("Durably flush power recovery journal")?;
        #[cfg(unix)]
        if self.sequence == 0 {
            File::open(
                self.path
                    .parent()
                    .context("Missing power recovery directory")?,
            )?
            .sync_all()?;
        }
        self.previous = mac(self.sequence, &self.previous, event, &self.key)?
            .to_hex()
            .to_string();
        self.sequence += 1;
        self.bytes += bytes.len();
        if matches!(event, Event::ApplyIntent { .. }) {
            self.has_intent = true;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{build_plan, engine::Record};
    use super::*;
    fn header() -> Header {
        let plan = build_plan(
            studio_core::Target::Win11Ltsc2024,
            &BTreeSet::from(["power-display-15".into()]),
        )
        .unwrap();
        Header {
            schema_version: 1,
            id: "b".repeat(32),
            started_at: "1792700000".into(),
            scheme: "381b4222-f694-41f0-9685-ff5bb260df2e".into(),
            records: plan
                .choices
                .iter()
                .map(|c| Record {
                    choice: c.clone(),
                    original: u32::MAX,
                })
                .collect(),
            plan,
        }
    }
    fn events(events: &[Event]) -> Vec<u8> {
        let mut output = Vec::new();
        let mut previous = String::new();
        for (i, event) in events.iter().enumerate() {
            output.extend(encode(i, &previous, event, &[9; 32]).unwrap());
            previous = mac(i, &previous, event, &[9; 32])
                .unwrap()
                .to_hex()
                .to_string();
        }
        output
    }
    #[test]
    fn authenticates_originals_and_rejects_tampering_order_bounds_and_false_success() {
        let bytes = events(&[Event::Header(header()), Event::ApplyIntent { index: 0 }]);
        let loaded = decode(&bytes, &[9; 32]).unwrap();
        assert!(loaded.intended.contains(&0));
        assert_eq!(loaded.header.records[0].original, u32::MAX);
        assert_eq!(loaded.header.plan, header().plan);
        assert_eq!(loaded.header.scheme, header().scheme);
        assert!(decode(&bytes, &[8; 32]).is_err());
        let mut tampered = bytes.clone();
        let position = tampered.iter().position(|&b| b == b'b').unwrap();
        tampered[position] = b'c';
        assert!(decode(&tampered, &[9; 32]).is_err());
        assert!(decode(&vec![b' '; MAX_BYTES + 1], &[9; 32]).is_err());
        assert!(
            decode(
                &events(&[Event::Header(header()), Event::Applied { index: 0 }]),
                &[9; 32]
            )
            .is_err()
        );
        assert!(
            decode(
                &events(&[
                    Event::Header(header()),
                    Event::ApplyFinished { cancelled: false }
                ]),
                &[9; 32]
            )
            .is_err()
        );
        assert!(
            decode(
                &events(&[
                    Event::Header(header()),
                    Event::ApplyIntent { index: 0 },
                    Event::RestoreFinished { cancelled: false }
                ]),
                &[9; 32]
            )
            .is_err()
        );
    }
    #[test]
    fn partial_intent_and_incomplete_tail_remain_recoverable() {
        let mut bytes = events(&[Event::Header(header()), Event::ApplyIntent { index: 0 }]);
        let valid = bytes.len();
        bytes.extend(b"{incomplete");
        let loaded = decode(&bytes, &[9; 32]).unwrap();
        assert_eq!(loaded.valid_len, valid as u64);
        assert_eq!(loaded.status, "interrupted");
        assert_eq!(loaded.count, 2);
    }
    #[test]
    fn real_store_resumes_truncated_tail_and_reuses_shared_lock_and_key() {
        let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "ltsc-power-test-{}-{}",
            std::process::id(),
            header().id
        ));
        ensure_directory(&root, true).unwrap();
        let lock = mutation_lock(&root).unwrap();
        assert!(mutation_lock(&root).is_err());
        validate_key_path(&root).unwrap();
        let key = crate::journal::key(&root, true).unwrap();
        validate_key_path(&root).unwrap();
        assert_eq!(key, crate::journal::key(&root, false).unwrap());
        let path = root.join(format!("{}.jsonl", header().id));
        let mut journal = FileJournal::create(path.clone(), key).unwrap();
        journal.append(&Event::Header(header())).unwrap();
        journal.append(&Event::ApplyIntent { index: 0 }).unwrap();
        assert!(journal.has_intent);
        drop(journal);
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"incomplete")
            .unwrap();
        let loaded = load(&path, &key).unwrap();
        assert_eq!(paths(&root).unwrap(), vec![path.clone()]);
        let mut journal = FileJournal::resume(path.clone(), key, &loaded).unwrap();
        journal.append(&Event::RestoreIntent { index: 0 }).unwrap();
        journal.append(&Event::Restored { index: 0 }).unwrap();
        journal
            .append(&Event::RestoreFinished { cancelled: false })
            .unwrap();
        assert_eq!(load(&path, &key).unwrap().status, "restored");
        drop(journal);
        drop(lock);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[cfg(unix)]
    fn rejects_linked_files_keys_and_directory_ancestors() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("ltsc-power-links-{}", std::process::id()));
        ensure_directory(&root, true).unwrap();
        let child = root.join("real");
        ensure_directory(&child, true).unwrap();
        symlink(&child, root.join("link")).unwrap();
        assert!(ensure_directory(&root.join("link/nested"), true).is_err());
        fs::write(child.join("file"), b"value").unwrap();
        symlink(child.join("file"), root.join("recovery.key")).unwrap();
        assert!(validate_key_path(&root).is_err());
        symlink(child.join("file"), root.join("test.jsonl")).unwrap();
        assert!(journal_path(&root.join("test.jsonl"), &root).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
