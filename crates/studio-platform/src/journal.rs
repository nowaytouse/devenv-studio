//! Authenticated, append-only, bounded write-ahead recovery journals.
use crate::engine::{Event, Header, Journal, validate_header};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAX_JOURNAL_BYTES: u64 = 4 * 1024 * 1024;
const MAX_EVENTS: usize = 8192;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    sequence: usize,
    payload: Event,
    mac: String,
}
fn signature(sequence: usize, payload: &Event, key: &[u8; 32]) -> Result<String> {
    let bytes = serde_json::to_vec(&(sequence, payload))?;
    Ok(blake3::keyed_hash(key, &bytes).to_hex().to_string())
}
fn encode(sequence: usize, event: &Event, key: &[u8; 32]) -> Result<Vec<u8>> {
    let envelope = Envelope {
        sequence,
        payload: event.clone(),
        mac: signature(sequence, event, key)?,
    };
    let mut bytes = serde_json::to_vec(&envelope)?;
    bytes.push(b'\n');
    Ok(bytes)
}
pub(crate) struct Loaded {
    pub header: Header,
    pub intended: BTreeSet<usize>,
    pub status: String,
    pub count: usize,
    pub valid_len: u64,
}

pub(crate) fn load(path: &Path, key: &[u8; 32]) -> Result<Loaded> {
    ensure!(
        !fs::symlink_metadata(path)?.file_type().is_symlink(),
        "Recovery journal cannot be a symbolic link"
    );
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_JOURNAL_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_JOURNAL_BYTES,
        "Recovery journal exceeds 4 MiB limit"
    );
    decode(&bytes, key)
}
fn decode(bytes: &[u8], key: &[u8; 32]) -> Result<Loaded> {
    // A crash can leave the final appended line incomplete. Only newline-terminated,
    // authenticated events authorize recovery; an incomplete initial header never does.
    let valid_len = bytes
        .iter()
        .rposition(|&b| b == b'\n')
        .map(|p| p + 1)
        .context("Incomplete recovery header")?;
    let mut header = None;
    let mut intended = BTreeSet::new();
    let mut applied = BTreeSet::new();
    let mut apply_finished = false;
    let mut restoring = false;
    let mut status = "interrupted".to_owned();
    let mut count = 0;
    for line in bytes[..valid_len]
        .split(|&b| b == b'\n')
        .filter(|line| !line.is_empty())
    {
        ensure!(count < MAX_EVENTS, "Too many recovery events");
        let envelope: Envelope = serde_json::from_slice(line).context("Invalid recovery event")?;
        ensure!(envelope.sequence == count, "Recovery sequence mismatch");
        // Hash equality uses the constant-time comparison supplied by blake3::Hash.
        let expected = signature(envelope.sequence, &envelope.payload, key)?;
        let actual = blake3::Hash::from_hex(&envelope.mac).context("Invalid recovery signature")?;
        let expected = blake3::Hash::from_hex(expected)?;
        ensure!(
            actual == expected,
            "Recovery integrity check failed; journal was changed or belongs to another user"
        );
        match envelope.payload {
            Event::Header(h) => {
                ensure!(count == 0, "Repeated recovery header");
                validate_header(&h)?;
                header = Some(h);
            }
            Event::ApplyIntent { index } => {
                let h = header.as_ref().context("Missing recovery header")?;
                ensure!(
                    !apply_finished
                        && !restoring
                        && index < h.records.len()
                        && index == intended.len(),
                    "Invalid apply intent order"
                );
                ensure!(
                    index == applied.len(),
                    "Apply advanced after an unverified mutation"
                );
                intended.insert(index);
            }
            Event::Applied { index } => {
                ensure!(
                    !apply_finished
                        && !restoring
                        && intended.contains(&index)
                        && index == applied.len()
                        && applied.insert(index),
                    "Invalid apply verification"
                );
            }
            Event::ApplyFinished { cancelled } => {
                let h = header.as_ref().context("Missing recovery header")?;
                ensure!(!apply_finished && !restoring, "Repeated apply completion");
                if !cancelled {
                    ensure!(
                        applied.len() == h.records.len(),
                        "Incomplete apply marked successful"
                    );
                }
                apply_finished = true;
                status = if cancelled { "cancelled" } else { "applied" }.into();
            }
            Event::RestoreIntent { index } => {
                ensure!(
                    intended.contains(&index),
                    "Restore references a value without a durable apply intent"
                );
                restoring = true;
                status = "restore interrupted".into();
            }
            Event::Restored { index } => {
                ensure!(
                    restoring && intended.contains(&index),
                    "Invalid restore verification"
                );
            }
            Event::RestoreFinished { cancelled } => {
                restoring = true;
                status = if cancelled {
                    "restore cancelled"
                } else {
                    "restored"
                }
                .into();
            }
        }
        count += 1;
    }
    Ok(Loaded {
        header: header.context("Missing recovery header")?,
        intended,
        status,
        count,
        valid_len: valid_len as u64,
    })
}

pub(crate) struct FileJournal {
    file: Option<File>,
    key: [u8; 32],
    sequence: usize,
    bytes: u64,
    apply_intent: bool,
    pub path: PathBuf,
}
impl FileJournal {
    pub fn create(path: PathBuf, key: [u8; 32]) -> Result<Self> {
        // Defer creation until the first complete header is ready. Cancellation or
        // preflight failures therefore do not leave empty recovery files.
        ensure!(!path.try_exists()?, "Recovery path already exists");
        Ok(Self {
            file: None,
            key,
            sequence: 0,
            bytes: 0,
            apply_intent: false,
            path,
        })
    }
    pub fn resume(path: PathBuf, key: [u8; 32], loaded: &Loaded) -> Result<Self> {
        let file = OpenOptions::new().append(true).open(&path)?;
        // Discard only the incomplete trailing event, whose content was never trusted.
        file.set_len(loaded.valid_len)?;
        file.sync_all()?;
        Ok(Self {
            file: Some(file),
            key,
            sequence: loaded.count,
            bytes: loaded.valid_len,
            apply_intent: !loaded.intended.is_empty(),
            path,
        })
    }
    pub fn may_have_mutations(&self) -> bool {
        self.apply_intent
    }
}
impl Journal for FileJournal {
    fn append(&mut self, event: &Event) -> Result<()> {
        ensure!(
            self.sequence < MAX_EVENTS,
            "Recovery event limit reached; no further mutations permitted"
        );
        let bytes = encode(self.sequence, event, &self.key)?;
        ensure!(
            self.bytes + bytes.len() as u64 <= MAX_JOURNAL_BYTES,
            "Recovery journal size limit reached; no further mutations permitted"
        );
        if self.file.is_none() {
            self.file = Some(private_new(&self.path)?);
        }
        let file = self.file.as_mut().context("Recovery file unavailable")?;
        file.write_all(&bytes).context("Write recovery journal")?;
        file.sync_all().context("Durably flush recovery journal")?;
        #[cfg(unix)]
        if self.sequence == 0 {
            File::open(self.path.parent().context("Journal directory missing")?)?
                .sync_all()
                .context("Flush recovery directory")?;
        }
        self.sequence += 1;
        if matches!(event, Event::ApplyIntent { .. }) {
            self.apply_intent = true;
        }
        self.bytes += bytes.len() as u64;
        Ok(())
    }
}
fn private_new(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .with_context(|| format!("Create private file {}", path.display()))
}
pub(crate) fn key(directory: &Path, create: bool) -> Result<[u8; 32]> {
    let path = directory.join("recovery.key");
    if create && !path.try_exists()? {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("Generate recovery key: {e}"))?;
        let mut file = private_new(&path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        #[cfg(unix)]
        File::open(directory)?.sync_all()?;
    }
    ensure!(
        !fs::symlink_metadata(&path)?.file_type().is_symlink(),
        "Recovery key cannot be a symbolic link"
    );
    let mut bytes = Vec::new();
    File::open(path)?.take(33).read_to_end(&mut bytes)?;
    bytes.try_into().map_err(|_| {
        anyhow::anyhow!("Invalid recovery key; existing journals cannot be authenticated")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::{Target, build_plan, load_catalog};
    fn header() -> Header {
        let c = load_catalog().unwrap();
        let p = build_plan(
            &c,
            Target::Win10Ltsc2021,
            &BTreeSet::from([c.actions[0].id.clone()]),
        )
        .unwrap();
        Header {
            schema_version: 1,
            id: "aa-11".into(),
            started_at: "2026-10-21T00:00:00Z".into(),
            target: p.target,
            catalog_revision: p.catalog_revision,
            actions: p.actions.iter().map(|a| a.id.clone()).collect(),
            records: vec![crate::engine::Record {
                change: p.actions[0].changes[0].clone(),
                original: None,
            }],
        }
    }
    #[test]
    fn authentication_rejects_modified_originals_and_wrong_key() {
        let key = [7; 32];
        let mut bytes = encode(0, &Event::Header(header()), &key).unwrap();
        assert!(decode(&bytes, &key).is_ok());
        assert!(decode(&bytes, &[8; 32]).is_err());
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value["payload"]["records"][0]["original"] =
            serde_json::json!({"kind":4,"bytes":[1,0,0,0]});
        bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        assert!(decode(&bytes, &key).is_err());
    }
    #[test]
    fn incomplete_tail_does_not_authorize_a_mutation() {
        let key = [9; 32];
        let mut bytes = encode(0, &Event::Header(header()), &key).unwrap();
        let valid = bytes.len();
        bytes.extend_from_slice(b"{\"sequence\":1,\"payload\":");
        let loaded = decode(&bytes, &key).unwrap();
        assert!(loaded.intended.is_empty());
        assert_eq!(loaded.valid_len, valid as u64);
    }
    #[test]
    fn intent_without_verified_write_stays_recoverable() {
        let key = [1; 32];
        let mut bytes = encode(0, &Event::Header(header()), &key).unwrap();
        bytes.extend(encode(1, &Event::ApplyIntent { index: 0 }, &key).unwrap());
        let loaded = decode(&bytes, &key).unwrap();
        assert_eq!(loaded.intended, BTreeSet::from([0]));
        assert_eq!(loaded.status, "interrupted");
    }
    #[test]
    fn out_of_order_and_false_completion_are_rejected() {
        let key = [2; 32];
        for event in [
            Event::ApplyIntent { index: 1 },
            Event::ApplyFinished { cancelled: false },
        ] {
            let mut bytes = encode(0, &Event::Header(header()), &key).unwrap();
            bytes.extend(encode(1, &event, &key).unwrap());
            assert!(decode(&bytes, &key).is_err());
        }
    }
    #[test]
    fn file_journal_is_durable_and_locked_exclusively() {
        let dir = std::env::temp_dir().join(format!("ltsc-journal-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("file-lock-test");
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .unwrap();
        f.try_lock().unwrap();
        let other = OpenOptions::new().write(true).open(&path).unwrap();
        assert!(other.try_lock().is_err());
        drop(other);
        drop(f);
        let key = key(&dir, true).unwrap();
        let path = dir.join("journal.jsonl");
        let mut store = FileJournal::create(path.clone(), key).unwrap();
        assert!(!path.exists());
        store.append(&Event::Header(header())).unwrap();
        drop(store);
        let loaded = load(&path, &key).unwrap();
        assert_eq!(loaded.count, 1);
        assert_eq!(loaded.header.id, "aa-11");
        let mut store = FileJournal::resume(path.clone(), key, &loaded).unwrap();
        assert!(!store.may_have_mutations());
        store.append(&Event::ApplyIntent { index: 0 }).unwrap();
        assert!(store.may_have_mutations());
        drop(store);
        assert!(load(&path, &key).unwrap().intended.contains(&0));
        fs::remove_dir_all(dir).unwrap();
    }
}
