//! Transactional installer for the revision-pinned macOS profile. Receipts and journals retain format v1.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, DirBuilder, File, FileTimes, Metadata, OpenOptions, Permissions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const STATE: &str = ".local/state/topgrade-macos-profile";
const LIMIT: u64 = 64 * 1024 * 1024;
const MAX_ASSETS: usize = 2_048;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Asset {
    source: String,
    destination: String,
    sha256: String,
    mode: u32,
    adopt_sha256: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u8,
    files: Vec<Asset>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Installed {
    version: u8,
    transaction: String,
    #[serde(deserialize_with = "unique_files")]
    files: BTreeMap<String, String>,
}

fn unique_files<'de, D>(input: D) -> std::result::Result<BTreeMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = BTreeMap<String, String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("distinct installed profile paths")
        }

        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut input: M,
        ) -> std::result::Result<Self::Value, M::Error> {
            let mut files = BTreeMap::new();
            while let Some((path, hash)) = input.next_entry()? {
                if files.len() == MAX_ASSETS || files.insert(path, hash).is_some() {
                    return Err(serde::de::Error::custom(
                        "duplicate or excessive installed paths",
                    ));
                }
            }
            Ok(files)
        }
    }
    input.deserialize_map(Visitor)
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum Status {
    Prepared,
    Complete,
    RolledBack,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    #[serde(flatten)]
    asset: Asset,
    before_sha256: Option<String>,
    before_mode: Option<u32>,
    before_mtime_ns: Option<i128>,
    backup: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u8,
    status: Status,
    previous: Option<Installed>,
    files: Vec<Row>,
}

#[derive(PartialEq, Eq)]
struct Snapshot {
    bytes: Vec<u8>,
    mode: u32,
    mtime_ns: i128,
}

struct Change {
    asset: Asset,
    desired: Vec<u8>,
    before: Option<Snapshot>,
}

struct Plan {
    desired: BTreeMap<String, (String, u32)>,
    previous: Option<Installed>,
    changes: Vec<Change>,
    unchanged: usize,
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn mapping(source: &str) -> Result<String> {
    let (category, name) = source.split_once('/').context("Invalid profile source")?;
    let destination = match category {
        "bin"
            if matches!(
                name,
                "brew-headsync-upgrade"
                    | "topgrade-rustup-headsync"
                    | "topgrade-mas-verified"
                    | "topgrade-gcloud-verified"
                    | "update-python-sqlite-trunk"
                    | "update-sqlite-trunk"
                    | "update-tourbox-beta"
                    | "update-preview-extras"
                    | "update-preview-apps"
                    | "update-project-deps"
                    | "cli-versions"
                    | "npm-bundled-security-guard.py"
                    | "topgrade-cursor-native-reconcile.py"
                    | "design-skill-kit-sync.py"
                    | "topgrade-skills-verified.py"
                    | "topgrade-skills-integrity.py"
                    | "normalize-gh-skill-lock.py"
                    | "topgrade-project-indexes.py"
                    | "topgrade-select-xcode-beta"
                    | "proxy-auto-run"
                    | "update-gradle-nightly"
                    | "update-xcode-beta"
                    | "topgrade-helm-verified"
                    | "topgrade-gh-skills-verified"
            ) =>
        {
            format!(".local/bin/{name}")
        }
        "libexec"
            if matches!(
                name,
                "rolling-deps/edge-extras.py"
                    | "rolling-deps/update-gradle-nightly.real.py"
                    | "preview-apps/update-preview-apps.py"
            ) =>
        {
            format!(".local/libexec/{name}")
        }
        "config"
            if matches!(
                name,
                "topgrade.toml"
                    | "topgrade.d/90-brew-headsync.toml"
                    | "topgrade.d/90-skills-coverage.toml"
                    | "topgrade.d/90-xcode-beta-select.toml"
                    | "topgrade.d/95-skills-unattended.toml"
                    | "topgrade.d/96-project-indexes.toml"
            ) =>
        {
            format!(".config/{name}")
        }
        "share" => {
            let tail = name
                .strip_prefix("npm-security/")
                .context("Unmanaged profile asset")?;
            ensure!(
                matches!(
                    tail,
                    "before.json"
                        | "leaf-candidate-after.json"
                        | "validation-report.json"
                        | "leaf-candidate-report.json"
                ) || [
                    "patches/undici/",
                    "patches/ip-address/",
                    "node_modules/undici/",
                    "node_modules/ip-address/"
                ]
                .iter()
                .any(|prefix| tail.starts_with(prefix)),
                "Unmanaged npm security asset"
            );
            format!(".local/share/topgrade-macos-profile/npm-security/{tail}")
        }
        _ => bail!("Source outside fixed profile allowlist: {source}"),
    };
    Ok(destination)
}

fn check_asset(asset: &Asset, seen: &mut BTreeSet<String>) -> Result<()> {
    ensure!(
        mapping(&asset.source)? == asset.destination && seen.insert(asset.destination.clone()),
        "Duplicate or inconsistent profile destination"
    );
    ensure!(
        hex(&asset.sha256, 64)
            && asset
                .adopt_sha256
                .as_deref()
                .is_none_or(|value| hex(value, 64))
            && matches!(asset.mode, 0o600 | 0o644 | 0o700 | 0o755),
        "Invalid profile checksum or mode for {}",
        asset.destination
    );
    Ok(())
}

fn metadata(path: &Path) -> Result<Option<Metadata>> {
    match path.symlink_metadata() {
        Ok(value) => {
            ensure!(!value.is_symlink(), "Refusing symlink: {}", path.display());
            ensure!(
                value.uid() == unsafe { libc::geteuid() },
                "Unexpected owner: {}",
                path.display()
            );
            Ok(Some(value))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("Inspect {}", path.display())),
    }
}

fn safe_path(root: &Path, relative: &str) -> Result<PathBuf> {
    let mut path = root.to_path_buf();
    let mut parts = relative.split('/').peekable();
    while let Some(part) = parts.next() {
        ensure!(
            !matches!(part, "" | "." | "..") && !part.contains(['\0', '\\']),
            "Unsafe profile path: {relative}"
        );
        path.push(part);
        if parts.peek().is_some()
            && let Some(info) = metadata(&path)?
        {
            ensure!(
                info.is_dir() && info.mode() & 0o022 == 0,
                "Unsafe profile directory: {}",
                path.display()
            );
        }
    }
    Ok(path)
}

fn root(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute()
            && path != Path::new("/")
            && path.canonicalize()? == path
            && metadata(path)?.is_some_and(|info| info.is_dir() && info.mode() & 0o022 == 0),
        "Expected a canonical user-owned directory: {}",
        path.display()
    );
    Ok(())
}

fn mtime(info: &Metadata) -> i128 {
    i128::from(info.mtime()) * 1_000_000_000 + i128::from(info.mtime_nsec())
}

fn snapshot(path: &Path) -> Result<Option<Snapshot>> {
    let Some(info) = metadata(path)? else {
        return Ok(None);
    };
    ensure!(
        info.is_file() && info.nlink() == 1 && info.len() <= LIMIT,
        "Unsafe profile file: {}",
        path.display()
    );
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let same = |other: &Metadata| {
        other.is_file()
            && other.dev() == info.dev()
            && other.ino() == info.ino()
            && other.nlink() == info.nlink()
            && other.uid() == info.uid()
            && other.mode() == info.mode()
            && other.len() == info.len()
            && mtime(other) == mtime(&info)
            && other.ctime() == info.ctime()
            && other.ctime_nsec() == info.ctime_nsec()
    };
    ensure!(
        same(&file.metadata()?),
        "Profile file changed while opening: {}",
        path.display()
    );
    let mut bytes = Vec::new();
    (&mut file).take(LIMIT + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 == info.len()
            && same(&file.metadata()?)
            && metadata(path)?.is_some_and(|current| same(&current)),
        "Profile file changed during read: {}",
        path.display()
    );
    Ok(Some(Snapshot {
        bytes,
        mode: info.mode() & 0o7777,
        mtime_ns: mtime(&info),
    }))
}

pub(super) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    snapshot(path)?
        .map(|file| {
            serde_json::from_slice(&file.bytes).with_context(|| format!("Read {}", path.display()))
        })
        .transpose()
}

fn checked_record(record: &Installed) -> Result<()> {
    ensure!(
        record.version == 1
            && hex(&record.transaction, 32)
            && record.files.values().all(|hash| hex(hash, 64)),
        "Invalid installed profile record"
    );
    Ok(())
}

fn installed(home: &Path) -> Result<Option<Installed>> {
    let record = read_json::<Installed>(&safe_path(home, &format!("{STATE}/installed.json"))?)?;
    if let Some(record) = &record {
        checked_record(record)?;
    }
    Ok(record)
}

fn pending(state: &Path) -> Result<()> {
    let directory = safe_path(state, "transactions")?;
    if let Some(info) = metadata(&directory)? {
        ensure!(
            info.is_dir() && info.mode() & 0o022 == 0,
            "Unsafe profile transactions directory"
        );
        for item in fs::read_dir(directory)? {
            let item = item?;
            let id = item.file_name();
            let id = id
                .to_str()
                .filter(|id| hex(id, 32))
                .context("Invalid profile transaction directory")?;
            let path = safe_path(state, &format!("transactions/{id}/journal.json"))?;
            if let Some(journal) = read_json::<Journal>(&path)? {
                ensure!(journal.version == 1, "Unsupported profile transaction {id}");
                ensure!(
                    journal.status != Status::Prepared,
                    "Incomplete profile transaction {id}; recover it before applying"
                );
            }
        }
    }
    Ok(())
}

fn plan(source: &Path, home: &Path) -> Result<Plan> {
    root(source)?;
    root(home)?;
    pending(&safe_path(home, STATE)?)?;
    let previous = installed(home)?;
    let manifest = read_json::<Manifest>(&safe_path(source, "manifest.json")?)?
        .context("Missing profile manifest")?;
    ensure!(
        manifest.version == 1 && (1..=MAX_ASSETS).contains(&manifest.files.len()),
        "Invalid profile manifest"
    );
    let mut plan = Plan {
        desired: BTreeMap::new(),
        previous,
        changes: Vec::new(),
        unchanged: 0,
    };
    let mut seen = BTreeSet::new();
    let mut total = 0u64;
    for asset in manifest.files {
        check_asset(&asset, &mut seen)?;
        let desired = snapshot(&safe_path(source, &asset.source)?)?
            .context("Missing source profile asset")?
            .bytes;
        ensure!(
            digest(&desired) == asset.sha256,
            "Source checksum mismatch: {}",
            asset.source
        );
        let target = safe_path(home, &asset.destination)?;
        let before = snapshot(&target)?;
        let hash = before.as_ref().map(|file| digest(&file.bytes));
        plan.desired.insert(
            asset.destination.clone(),
            (asset.sha256.clone(), asset.mode),
        );
        if hash.as_deref() == Some(&asset.sha256)
            && before.as_ref().is_some_and(|file| file.mode == asset.mode)
        {
            plan.unchanged += 1;
            continue;
        }
        let expected = plan
            .previous
            .as_ref()
            .and_then(|record| record.files.get(&asset.destination))
            .or(asset.adopt_sha256.as_ref());
        ensure!(
            hash.as_ref().is_none_or(|hash| Some(hash) == expected),
            "Local drift: {}; preserve and reconcile it before applying",
            asset.destination
        );
        total += desired.len() as u64 + before.as_ref().map_or(0, |file| file.bytes.len() as u64);
        ensure!(
            total <= 2 * LIMIT,
            "Profile transaction exceeds its total data limit"
        );
        plan.changes.push(Change {
            asset,
            desired,
            before,
        });
    }
    let order = |source: &str| match source.split_once('/').map(|(prefix, _)| prefix) {
        Some("share") => 0,
        Some("libexec") => 1,
        Some("bin") => 2,
        _ => 3,
    };
    plan.changes.sort_by(|left, right| {
        order(&left.asset.source)
            .cmp(&order(&right.asset.source))
            .then_with(|| left.asset.source.cmp(&right.asset.source))
    });
    Ok(plan)
}

fn sync_dir(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn ensure_dirs(root: &Path, path: &Path) -> Result<()> {
    let mut current = root.to_path_buf();
    for component in path.strip_prefix(root)?.components() {
        current.push(component);
        match DirBuilder::new().mode(0o700).create(&current) {
            Ok(()) => sync_dir(current.parent().context("Missing directory parent")?)?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error.into()),
        }
        ensure!(
            metadata(&current)?.is_some_and(|info| info.is_dir() && info.mode() & 0o022 == 0),
            "Unsafe profile directory: {}",
            current.display()
        );
    }
    Ok(())
}

fn atomic(path: &Path, bytes: &[u8], mode: u32, timestamp: Option<i128>) -> Result<()> {
    let parent = path.parent().context("Missing profile file parent")?;
    let mut staged = tempfile::Builder::new()
        .prefix(".topgrade-profile.")
        .tempfile_in(parent)?;
    staged.write_all(bytes)?;
    staged
        .as_file()
        .set_permissions(Permissions::from_mode(mode))?;
    if let Some(timestamp) = timestamp {
        let nanos = u64::try_from(timestamp.unsigned_abs())?;
        let when = if timestamp < 0 {
            UNIX_EPOCH - Duration::from_nanos(nanos)
        } else {
            UNIX_EPOCH + Duration::from_nanos(nanos)
        };
        staged
            .as_file()
            .set_times(FileTimes::new().set_accessed(when).set_modified(when))?;
    }
    staged.as_file().sync_all()?;
    staged.persist(path)?;
    sync_dir(parent)
}

fn write_json(path: &Path, data: &impl Serialize) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(data)?;
    bytes.push(b'\n');
    atomic(path, &bytes, 0o600, None)
}

pub(super) struct ProfileLock(File);

impl Drop for ProfileLock {
    fn drop(&mut self) {
        // A concurrent fork can retain the descriptor until exec closes it.
        let _ = self.0.unlock();
    }
}

pub(super) fn lock(home: &Path) -> Result<(PathBuf, ProfileLock)> {
    root(home)?;
    let state = safe_path(home, STATE)?;
    ensure_dirs(home, &state)?;
    let lock_path = safe_path(&state, "lock")?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(lock_path)?;
    let info = file.metadata()?;
    ensure!(
        info.is_file()
            && info.nlink() == 1
            && info.mode() & 0o077 == 0
            && info.uid() == unsafe { libc::geteuid() },
        "Unsafe profile lock"
    );
    file.try_lock().context("Profile installer is busy")?;
    Ok((state, ProfileLock(file)))
}

fn prepare(state: &Path, id: &str, plan: &Plan) -> Result<(PathBuf, Journal)> {
    let transactions = safe_path(state, "transactions")?;
    ensure_dirs(state, &transactions)?;
    let folder = safe_path(&transactions, id)?;
    DirBuilder::new().mode(0o700).create(&folder)?;
    let backup = folder.join("backup");
    DirBuilder::new().mode(0o700).create(&backup)?;
    let mut journal = Journal {
        version: 1,
        status: Status::Prepared,
        previous: plan.previous.clone(),
        files: Vec::new(),
    };
    for (index, change) in plan.changes.iter().enumerate() {
        let name = index.to_string();
        if let Some(before) = &change.before {
            atomic(&backup.join(&name), &before.bytes, 0o600, None)?;
        }
        journal.files.push(Row {
            asset: change.asset.clone(),
            before_sha256: change.before.as_ref().map(|file| digest(&file.bytes)),
            before_mode: change.before.as_ref().map(|file| file.mode),
            before_mtime_ns: change.before.as_ref().map(|file| file.mtime_ns),
            backup: name,
        });
    }
    let path = folder.join("journal.json");
    write_json(&path, &journal)?;
    sync_dir(&backup)?;
    sync_dir(&transactions)?;
    Ok((path, journal))
}

fn apply(source: &Path, home: &Path) -> Result<Value> {
    let (state, _lock) = lock(home)?;
    let plan = plan(source, home)?;
    let files: BTreeMap<_, _> = plan
        .desired
        .iter()
        .map(|(name, (hash, _))| (name.clone(), hash.clone()))
        .collect();
    if plan.changes.is_empty()
        && plan
            .previous
            .as_ref()
            .is_some_and(|record| record.files == files)
    {
        return Ok(json!({"status": "already-current", "changed": 0, "unchanged": plan.unchanged}));
    }
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        ^ (u128::from(std::process::id()) << 96);
    let id = format!("{nonce:032x}");
    let (path, mut journal) = prepare(&state, &id, &plan)?;
    let outcome = (|| -> Result<()> {
        for change in &plan.changes {
            let target = safe_path(home, &change.asset.destination)?;
            ensure!(
                snapshot(&target)? == change.before,
                "Changed during deployment: {}",
                change.asset.destination
            );
            ensure_dirs(home, target.parent().context("Missing target parent")?)?;
            atomic(&target, &change.desired, change.asset.mode, None)?;
        }
        for (destination, (hash, mode)) in &plan.desired {
            let target = safe_path(home, destination)?;
            let file = snapshot(&target)?.context("Profile asset missing after deployment")?;
            ensure!(
                digest(&file.bytes) == *hash && file.mode == *mode,
                "Profile asset failed readback: {destination}"
            );
        }
        ensure!(
            installed(home)? == plan.previous,
            "Installed profile record changed during deployment"
        );
        let record = Installed {
            version: 1,
            transaction: id.clone(),
            files,
        };
        write_json(&safe_path(&state, "installed.json")?, &record)?;
        ensure!(
            installed(home)? == Some(record),
            "Profile receipt failed readback"
        );
        journal.status = Status::Complete;
        write_json(&path, &journal)
    })();
    if let Err(error) = outcome {
        return match rollback(home, &state, &id, None, || Ok(())) {
            Ok(_) => Err(error.context("Profile apply failed and its changes were rolled back")),
            Err(recovery) => {
                Err(error.context(format!("Profile recovery {id} failed: {recovery:#}")))
            }
        };
    }
    Ok(
        json!({"status": "installed", "transaction": id, "changed": plan.changes.len(), "unchanged": plan.unchanged,
        "rollback": format!("--rollback {id}")}),
    )
}

fn receipt_matches(record: Option<&Installed>, plan: &Plan) -> bool {
    record.is_some_and(|record| {
        record.files.len() == plan.desired.len()
            && plan
                .desired
                .iter()
                .all(|(path, (hash, _))| record.files.get(path) == Some(hash))
    })
}

fn verified_plan(source: &Path, home: &Path) -> Result<Plan> {
    let plan = plan(source, home)?;
    ensure!(
        plan.changes.is_empty() && receipt_matches(plan.previous.as_ref(), &plan),
        "Live profile files or installed receipt differ from the engine profile"
    );
    Ok(plan)
}

pub(super) fn verify_installed(source: &Path, home: &Path) -> Result<usize> {
    Ok(verified_plan(source, home)?.unchanged)
}

pub(super) fn rollback_activation(
    current: &Path,
    previous: &Path,
    home: &Path,
    state: &Path,
    id: Option<&str>,
    before_restore: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let current = verified_plan(current, home)?;
    if let Some(id) = id {
        let previous_plan = plan(previous, home)?;
        rollback(
            home,
            state,
            id,
            Some((&current, &previous_plan)),
            before_restore,
        )?;
        verified_plan(previous, home)?;
    } else {
        verified_plan(previous, home)?;
        before_restore()?;
    }
    Ok(())
}

fn rollback(
    home: &Path,
    state: &Path,
    id: &str,
    expected: Option<(&Plan, &Plan)>,
    before_restore: impl FnOnce() -> Result<()>,
) -> Result<Value> {
    ensure!(hex(id, 32), "Invalid profile transaction ID");
    let path = safe_path(state, &format!("transactions/{id}/journal.json"))?;
    let folder = path.parent().context("Missing transaction directory")?;
    let mut journal = read_json::<Journal>(&path)?.context("Missing profile journal")?;
    ensure!(
        journal.version == 1 && journal.files.len() <= MAX_ASSETS,
        "Invalid profile journal"
    );
    if expected.is_none() && journal.status == Status::RolledBack {
        return Ok(json!({"status": "already-rolled-back", "transaction": id}));
    }
    if let Some(record) = &journal.previous {
        checked_record(record)?;
    }
    let active = installed(home)?;
    if let Some((current, previous)) = expected {
        ensure!(
            journal.status == Status::Complete
                && active == current.previous
                && active
                    .as_ref()
                    .is_some_and(|record| record.transaction == id)
                && receipt_matches(journal.previous.as_ref(), previous),
            "Profile transaction does not bind the active and previous installed profiles"
        );
        let mut backups = BTreeMap::new();
        for row in &journal.files {
            let asset = &row.asset;
            ensure!(
                current
                    .desired
                    .get(&asset.destination)
                    .is_some_and(|(hash, mode)| { hash == &asset.sha256 && *mode == asset.mode })
                    && backups.insert(&asset.destination, row).is_none(),
                "Profile transaction contains a different or duplicate asset"
            );
        }
        for (destination, (hash, mode)) in &previous.desired {
            if let Some(row) = backups.get(destination) {
                ensure!(
                    row.before_sha256.as_ref() == Some(hash) && row.before_mode == Some(*mode),
                    "Profile backup does not match the previous manifest: {destination}"
                );
            }
        }
        for change in &previous.changes {
            ensure!(
                backups.contains_key(&change.asset.destination),
                "Missing rollback backup for previous profile asset: {}",
                change.asset.destination
            );
        }
    }
    ensure!(
        active == journal.previous
            || active
                .as_ref()
                .is_some_and(|record| record.transaction == id),
        "A newer or changed profile is active; refusing rollback"
    );
    let mut seen = BTreeSet::new();
    let mut restores = Vec::new();
    let mut total = 0u64;
    for (index, row) in journal.files.iter().enumerate() {
        check_asset(&row.asset, &mut seen)?;
        ensure!(
            row.backup == index.to_string(),
            "Invalid profile backup path"
        );
        let target = safe_path(home, &row.asset.destination)?;
        let current = snapshot(&target)?;
        let before = match (&row.before_sha256, row.before_mode, row.before_mtime_ns) {
            (None, None, None) => None,
            (Some(hash), Some(mode), Some(timestamp)) if hex(hash, 64) && mode <= 0o7777 => {
                let bytes = snapshot(&safe_path(folder, &format!("backup/{}", row.backup))?)?
                    .context("Missing profile backup")?
                    .bytes;
                ensure!(digest(&bytes) == *hash, "Profile backup checksum mismatch");
                Some(Snapshot {
                    bytes,
                    mode,
                    mtime_ns: timestamp,
                })
            }
            _ => bail!("Invalid profile backup metadata"),
        };
        let matches_previous = match (&current, &before) {
            (None, None) => true,
            (Some(current), Some(before)) => {
                current.bytes == before.bytes && current.mode == before.mode
            }
            _ => false,
        };
        ensure!(
            matches_previous
                || current
                    .as_ref()
                    .is_some_and(|file| file.mode == row.asset.mode
                        && digest(&file.bytes) == row.asset.sha256),
            "Rollback would overwrite local drift: {}",
            row.asset.destination
        );
        total += current.as_ref().map_or(0, |file| file.bytes.len() as u64)
            + before.as_ref().map_or(0, |file| file.bytes.len() as u64);
        ensure!(
            total <= 2 * LIMIT,
            "Profile rollback exceeds its total data limit"
        );
        restores.push((target, current, before));
    }
    before_restore()?;
    for (target, current, before) in restores.into_iter().rev() {
        if current == before {
            continue;
        }
        ensure!(
            snapshot(&target)? == current,
            "Profile destination changed during rollback"
        );
        if let Some(before) = before {
            atomic(&target, &before.bytes, before.mode, Some(before.mtime_ns))?;
            ensure!(
                snapshot(&target)? == Some(before),
                "Profile rollback readback failed"
            );
        } else {
            fs::remove_file(&target)?;
            sync_dir(target.parent().context("Missing rollback parent")?)?;
            ensure!(
                metadata(&target)?.is_none(),
                "Profile file remained after rollback"
            );
        }
    }
    ensure!(
        installed(home)? == active,
        "Installed profile record changed during rollback"
    );
    let record = safe_path(state, "installed.json")?;
    if let Some(previous) = &journal.previous {
        write_json(&record, previous)?;
    } else if metadata(&record)?.is_some() {
        fs::remove_file(&record)?;
        sync_dir(state)?;
    }
    ensure!(
        installed(home)? == journal.previous,
        "Previous profile receipt failed readback"
    );
    journal.status = Status::RolledBack;
    write_json(&path, &journal)?;
    Ok(json!({"status": "rolled-back", "transaction": id}))
}

pub(super) fn execute(source: &Path, home: &Path, args: &[&str]) -> Result<Value> {
    match args {
        ["--apply"] => apply(source, home),
        ["--rollback", id] => {
            let (state, _lock) = lock(home)?;
            rollback(home, &state, id, None, || Ok(()))
        }
        [] | ["--verify"] => {
            let plan = plan(source, home)?;
            let verify = args == ["--verify"];
            ensure!(
                !verify || plan.changes.is_empty(),
                "Profile is not fully installed: {} file(s)",
                plan.changes.len()
            );
            let first: Vec<_> = plan
                .changes
                .iter()
                .take(30)
                .map(|change| &change.asset.destination)
                .collect();
            Ok(
                json!({"status": if verify {"verified"} else {"plan"}, "changed": plan.changes.len(),
                "unchanged": plan.unchanged, "firstChanges": first, "moreChanges": plan.changes.len().saturating_sub(30)}),
            )
        }
        _ => bail!("Invalid profile transaction action"),
    }
}

#[cfg(test)]
mod tests;
