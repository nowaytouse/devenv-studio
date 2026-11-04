use super::*;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Identity {
    sha256: String,
    mode: u32,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Change {
    before: Option<Identity>,
    after: Identity,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Skill {
    name: String,
    skill: Change,
    receipt: Change,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Journal {
    before: Option<String>,
    after: String,
    skills: Vec<Skill>,
}

struct FileState {
    payload: Vec<u8>,
    identity: Identity,
}

fn read(path: &Path) -> Result<Option<FileState>> {
    if !exists(path)? {
        return Ok(None);
    }
    let file = open_file(path, false, DATA_LIMIT)?;
    let mode = file.metadata()?.permissions().mode() & 0o777;
    let payload = bounded(file, DATA_LIMIT)?;
    Ok(Some(state(payload, mode)))
}

fn state(payload: Vec<u8>, mode: u32) -> FileState {
    let identity = Identity {
        sha256: digest(&payload),
        mode,
    };
    FileState { payload, identity }
}

fn backup_name(name: &str, receipt: bool) -> String {
    let ext = if receipt { "receipt.json" } else { "SKILL.md" };
    format!("{name}.{ext}")
}

fn finish(root: &Path) -> Result<()> {
    fs::remove_file(root.join(PENDING))?;
    sync_directory(root)
}

pub(super) fn recover(home: &Path, root: &Path) -> Result<()> {
    let pending = root.join(PENDING);
    if !exists(&pending)? {
        return Ok(());
    }
    let path = String::from_utf8(bytes(&pending, 256)?)?;
    let path = path.trim_end_matches('\n');
    ensure!(
        path.strip_prefix("backups/")
            .is_some_and(|name| name.starts_with("sync-") && safe_path(name) && !name.contains('/')),
        "Invalid pending design activation backup path"
    );
    let backup = directory(root, Path::new(path), false)?;
    let journal: Journal = serde_json::from_value(json(&bytes(&backup.join("transaction.json"), DATA_LIMIT)?)?)?;
    let active = current(root)?;
    ensure!(
        is_hex(&journal.after, 40)
            && journal.before.as_ref().is_none_or(|sha| is_hex(sha, 40))
            && (active == journal.before || active.as_deref() == Some(&journal.after))
            && !journal.skills.is_empty()
            && journal.skills.len() <= 256,
        "Pending design activation conflicts with current state"
    );
    let mut names = BTreeSet::new();
    let mut changes = Vec::new();
    for entry in &journal.skills {
        ensure!(
            names.insert(entry.name.as_str()),
            "Duplicate skill in design activation journal"
        );
        let (skill, receipt) = paths(home, root, &entry.name)?;
        for (path, change, receipt) in [(skill, &entry.skill, false), (receipt, &entry.receipt, true)] {
            let observed = read(&path)?.map(|file| file.identity);
            ensure!(
                observed == change.before || observed.as_ref() == Some(&change.after),
                "Pending design activation found external changes: {}",
                path.display()
            );
            let previous = change
                .before
                .as_ref()
                .map(|id| -> Result<FileState> {
                    ensure!(id.mode <= 0o777 && id.mode & 0o022 == 0, "Unsafe design backup mode");
                    let saved = bytes(&backup.join(backup_name(&entry.name, receipt)), DATA_LIMIT)?;
                    ensure!(digest(&saved) == id.sha256, "Design activation backup hash differs");
                    Ok(state(saved, id.mode))
                })
                .transpose()?;
            if observed != change.before {
                changes.push((path, observed, previous));
            }
        }
    }
    for (path, expected, old) in changes.into_iter().rev() {
        ensure!(
            read(&path)?.map(|file| file.identity) == expected,
            "Design target changed during rollback"
        );
        match old {
            Some(file) => atomic_write(&path, &file.payload, file.identity.mode)?,
            None => {
                fs::remove_file(&path)?;
                sync_directory(path.parent().context("Missing design rollback parent")?)?;
            }
        }
    }
    ensure!(current(root)? == active, "Design current changed during rollback");
    if active != journal.before {
        switch(root, journal.before.as_deref())?;
    }
    finish(root)?;
    eprintln!(
        "Recovered interrupted design activation. Backup retained at {}",
        backup.display()
    );
    Ok(())
}

struct Plan {
    name: String,
    skill_path: PathBuf,
    receipt_path: PathBuf,
    old_skill: FileState,
    old_receipt: Option<FileState>,
    skill: FileState,
    receipt: FileState,
}

impl Plan {
    fn unchanged(&self) -> bool {
        self.old_skill.identity == self.skill.identity
            && self
                .old_receipt
                .as_ref()
                .is_some_and(|old| old.identity == self.receipt.identity)
    }

    fn preflight(&self) -> Result<()> {
        ensure!(
            read(&self.skill_path)?.is_some_and(|file| file.identity == self.old_skill.identity)
                && read(&self.receipt_path)?.map(|file| file.identity)
                    == self.old_receipt.as_ref().map(|file| file.identity.clone()),
            "Design skill changed during staging: {}",
            self.name
        );
        Ok(())
    }

    fn backup(&self, directory: &Path) -> Result<Skill> {
        atomic_write(
            &directory.join(backup_name(&self.name, false)),
            &self.old_skill.payload,
            0o600,
        )?;
        if let Some(receipt) = &self.old_receipt {
            atomic_write(&directory.join(backup_name(&self.name, true)), &receipt.payload, 0o600)?;
        }
        Ok(Skill {
            name: self.name.clone(),
            skill: Change {
                before: Some(self.old_skill.identity.clone()),
                after: self.skill.identity.clone(),
            },
            receipt: Change {
                before: self.old_receipt.as_ref().map(|receipt| receipt.identity.clone()),
                after: self.receipt.identity.clone(),
            },
        })
    }

    fn apply(&self) -> Result<()> {
        self.preflight()?;
        if self.old_skill.identity != self.skill.identity {
            atomic_write(&self.skill_path, &self.skill.payload, self.skill.identity.mode)?;
        }
        if self
            .old_receipt
            .as_ref()
            .is_none_or(|old| old.identity != self.receipt.identity)
        {
            atomic_write(&self.receipt_path, &self.receipt.payload, self.receipt.identity.mode)?;
        }
        Ok(())
    }
}

fn plan(home: &Path, root: &Path, name: &str, sha: &str, commit: &str, manifest: &Manifest) -> Result<Plan> {
    let (skill_path, receipt_path) = paths(home, root, name)?;
    let old_skill = read(&skill_path)?.context("Design skill is missing")?;
    let old_receipt = read(&receipt_path)?;
    let (skill, receipt) = render(root, commit, name, sha, manifest)?;
    let prior = old_receipt
        .as_ref()
        .map(|record| read_receipt(&record.payload))
        .transpose()?;
    ensure!(
        old_skill.identity.sha256 == receipt.source_sha256
            || prior.is_some_and(|prior| prior.name == name && prior.rendered_sha256 == old_skill.identity.sha256),
        "Local design SKILL.md differs from source and prior rendering; preserved: {name}"
    );
    let skill = state(skill, old_skill.identity.mode);
    let mut encoded = serde_json::to_vec(&receipt)?;
    // The previous helper wrote the same field order with Python's default separators.
    if let Some(old) = &old_receipt
        && json(&old.payload)? == serde_json::to_value(&receipt)?
    {
        encoded = old.payload.clone();
    } else {
        encoded.push(b'\n');
    }
    let receipt = state(
        encoded,
        old_receipt.as_ref().map_or(0o644, |record| record.identity.mode),
    );
    Ok(Plan {
        name: name.to_owned(),
        skill_path,
        receipt_path,
        old_skill,
        old_receipt,
        skill,
        receipt,
    })
}

pub(super) fn sync(home: &Path, root: &Path) -> Result<()> {
    let lock = lock_path(home)?;
    let lock_before = bytes(&lock, DATA_LIMIT)?;
    let selected = selected(&lock_before)?;
    let before = current(root)?;
    if let Some(commit) = &before {
        release_manifest(&root.join("releases").join(commit), commit)?;
    }
    let route = Route::detect();
    let (source, commit) = fetch(home, &route)?;
    let manifest = export(home, root, &source, &commit, &selected, &route)?;
    directory(root, Path::new("receipts"), true)?;
    let plans = selected
        .iter()
        .map(|(name, sha)| plan(home, root, name, sha, &commit, &manifest))
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        bytes(&lock, DATA_LIMIT)? == lock_before && current(root)? == before,
        "Design skill lock or activation changed while staging"
    );
    for plan in &plans {
        plan.preflight()?;
    }
    if before.as_deref() == Some(&commit) && plans.iter().all(Plan::unchanged) {
        check(home, root)?;
        println!("Already current: {SOURCE}@{commit}; no skill files rewritten");
        return Ok(());
    }
    let backup_root = directory(root, Path::new("backups"), true)?;
    let backup = tempfile::Builder::new()
        .prefix("sync-")
        .tempdir_in(&backup_root)?
        .keep();
    let journal = Journal {
        before: before.clone(),
        after: commit.clone(),
        skills: plans.iter().map(|plan| plan.backup(&backup)).collect::<Result<_>>()?,
    };
    if let Some(previous) = &before {
        atomic_write(
            &backup.join("current-target.txt"),
            format!("releases/{previous}\n").as_bytes(),
            0o600,
        )?;
    }
    atomic_write(&backup.join("transaction.json"), &serde_json::to_vec(&journal)?, 0o600)?;
    sync_directory(&backup_root)?;
    let mut marker = tempfile::NamedTempFile::new_in(root)?;
    writeln!(marker, "{}", backup.strip_prefix(root)?.display())?;
    marker.as_file().sync_all()?;
    marker.persist_noclobber(root.join(PENDING))?;
    sync_directory(root)?;
    let applied = (|| {
        ensure!(
            bytes(&lock, DATA_LIMIT)? == lock_before && current(root)? == before,
            "Design state changed before activation"
        );
        if before.as_deref() != Some(&commit) {
            switch(root, Some(&commit))?;
        }
        for plan in &plans {
            plan.apply()?;
        }
        ensure!(
            bytes(&lock, DATA_LIMIT)? == lock_before,
            "Design skill lock changed during activation"
        );
        check(home, root)
    })();
    if let Err(error) = applied {
        recover(home, root).wrap_err_with(|| {
            format!(
                "Design activation failed ({error:#}); backup retained at {}",
                backup.display()
            )
        })?;
        return Err(error);
    }
    finish(root)?;
    println!(
        "Synced {} skills from {SOURCE}@{commit}; backup: {}",
        plans.len(),
        backup.display()
    );
    Ok(())
}

#[cfg(test)]
#[test]
fn interrupted_activation_restores_only_verified_originals() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let home = fixture.path().canonicalize()?;
    let root = directory(&home, Path::new(KIT), true)?;
    let backup = directory(&root, Path::new("backups/sync-fixture"), true)?;
    directory(&root, Path::new("receipts"), true)?;
    directory(&home, Path::new(".agents/skills/design"), true)?;
    let (skill, receipt) = paths(&home, &root, "design")?;
    let before = "a".repeat(40);
    let after = "b".repeat(40);
    switch(&root, Some(&after))?;
    atomic_write(&skill, b"new skill", 0o644)?;
    atomic_write(&receipt, b"external edit", 0o600)?;
    let journal = Journal {
        before: Some(before.clone()),
        after,
        skills: vec![Skill {
            name: "design".into(),
            skill: Change {
                before: Some(state(b"old skill".to_vec(), 0o644).identity),
                after: state(b"new skill".to_vec(), 0o644).identity,
            },
            receipt: Change {
                before: None,
                after: state(b"new receipt".to_vec(), 0o600).identity,
            },
        }],
    };
    atomic_write(&backup.join("design.SKILL.md"), b"old skill", 0o600)?;
    atomic_write(&backup.join("transaction.json"), &serde_json::to_vec(&journal)?, 0o600)?;
    atomic_write(&root.join(PENDING), b"backups/sync-fixture\n", 0o600)?;
    assert!(recover(&home, &root).is_err());
    assert_eq!(bytes(&skill, DATA_LIMIT)?, b"new skill");
    assert_eq!(bytes(&receipt, DATA_LIMIT)?, b"external edit");
    atomic_write(&receipt, b"new receipt", 0o600)?;
    recover(&home, &root)?;
    assert_eq!(bytes(&skill, DATA_LIMIT)?, b"old skill");
    assert!(!exists(&receipt)?);
    assert_eq!(current(&root)?.as_deref(), Some(before.as_str()));
    assert!(!exists(&root.join(PENDING))?);
    Ok(())
}
