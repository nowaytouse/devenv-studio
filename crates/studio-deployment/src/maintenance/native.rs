use super::{
    Check, EngineIdentity, Review, Step,
    protocol::{IDS, valid_hex},
};
use crate::process::{Invocation, run_configured};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, DirBuilder, File, Metadata, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

mod run;
pub(super) use run::{history, output_excerpt, run};

const PATH: &str = "/opt/homebrew/bin:/opt/homebrew/sbin:/usr/bin:/bin:/usr/sbin:/sbin";
const FILE_LIMIT: u64 = 128 * 1024 * 1024;
const JSON_LIMIT: u64 = 128 * 1024;
const NO_FOLLOW_NONBLOCK: i32 = 0x100 | 0x4;
const ASSETS: [&str; 5] = [
    ".config/topgrade.toml",
    ".config/topgrade.d/90-brew-headsync.toml",
    ".config/topgrade.d/90-skills-coverage.toml",
    ".config/topgrade.d/90-xcode-beta-select.toml",
    ".config/topgrade.d/95-skills-unattended.toml",
];

pub(super) fn account_home() -> Result<PathBuf> {
    let record = Command::new("/usr/bin/id")
        .arg("-P")
        .env_clear()
        .output()
        .context("Read the current macOS account record")?;
    ensure!(
        record.status.success() && record.stdout.len() < 4096,
        "Cannot identify the current macOS account"
    );
    let record = std::str::from_utf8(&record.stdout)?.trim_end_matches('\n');
    let fields = record.split(':').collect::<Vec<_>>();
    ensure!(fields.len() == 10, "Unexpected macOS account record");
    let uid: u32 = fields[2].parse().context("Invalid macOS account UID")?;
    let home = PathBuf::from(fields[8]);
    ensure!(
        uid != 0
            && home.is_absolute()
            && std::env::var_os("HOME").is_some_and(|value| Path::new(&value) == home),
        "Maintenance requires the normal macOS account and its actual HOME"
    );
    let metadata = real_metadata(&home)?;
    ensure!(
        metadata.is_dir() && metadata.uid() == uid,
        "Account home is not a real user-owned directory"
    );
    Ok(home)
}

fn real_metadata(path: &Path) -> Result<Metadata> {
    ensure!(
        path.is_absolute() && path.canonicalize()? == path,
        "Maintenance path must be absolute and have no symlink components"
    );
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        !metadata.is_symlink(),
        "Maintenance path became a symbolic link"
    );
    Ok(metadata)
}

fn same_file(left: &Metadata, right: &Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}

fn unchanged(left: &Metadata, right: &Metadata) -> bool {
    same_file(left, right)
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

fn open_regular(path: &Path, limit: u64) -> Result<(File, Metadata)> {
    let before = real_metadata(path)?;
    ensure!(
        before.is_file() && before.nlink() == 1 && before.len() <= limit,
        "Maintenance input is not a bounded regular file"
    );
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(NO_FOLLOW_NONBLOCK)
        .open(path)?;
    let opened = file.metadata()?;
    ensure!(
        opened.is_file()
            && opened.nlink() == 1
            && same_file(&before, &opened)
            && opened.len() <= limit,
        "Maintenance file changed while opening"
    );
    Ok((file, opened))
}

fn read_regular(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let (file, before) = open_regular(path, limit)?;
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit && unchanged(&before, &real_metadata(path)?),
        "Maintenance file changed while reading"
    );
    Ok(bytes)
}

fn hash_command() -> Command {
    let mut command = Command::new("/usr/bin/shasum");
    command
        .args(["-a", "256"])
        .env_clear()
        .current_dir("/")
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}

fn hash_output(output: std::process::Output) -> Result<String> {
    let text = std::str::from_utf8(&output.stdout).context("Invalid SHA-256 output")?;
    let digest = text
        .strip_suffix("  -\n")
        .context("Unexpected SHA-256 output")?;
    ensure!(
        output.status.success() && valid_hex(digest, 64),
        "SHA-256 calculation failed"
    );
    Ok(digest.into())
}

pub(super) fn sha256_file(path: &Path) -> Result<String> {
    let (file, before) = open_regular(path, FILE_LIMIT)?;
    let output = hash_command()
        .stdin(file)
        .output()
        .context("Hash maintenance file")?;
    ensure!(
        unchanged(&before, &real_metadata(path)?),
        "Maintenance file changed while hashing"
    );
    hash_output(output)
}

fn sha256_bytes(bytes: &[u8]) -> Result<String> {
    let mut child = hash_command()
        .stdin(Stdio::piped())
        .spawn()
        .context("Start receipt hashing")?;
    let written = child
        .stdin
        .take()
        .context("Missing hash input")?
        .write_all(bytes);
    let output = child
        .wait_with_output()
        .context("Wait for receipt hashing")?;
    written.context("Write receipt hash input")?;
    hash_output(output)
}

fn present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn data_dir(home: &Path) -> PathBuf {
    home.join("Library/Application Support/LTSC Studio")
}

fn no_pending(home: &Path) -> Result<()> {
    ensure!(
        !present(&data_dir(home).join("maintenance-active.json"))?,
        "An active or interrupted Topgrade run needs review. Check the recorded logs and affected state before acknowledging the interruption."
    );
    ensure!(
        !present(&home.join(".local/share/topgrade-rustup-head/studio-activation.pending.json"))?,
        "A Topgrade engine activation is unresolved. Review studio-activation.pending.json, the current launcher and the profile transaction before making further macOS changes."
    );
    Ok(())
}

fn ensure_dir(path: &Path, private: bool) -> Result<()> {
    let parent = path
        .parent()
        .context("Maintenance directory has no parent")?;
    let parent_metadata = real_metadata(parent)?;
    ensure!(
        parent_metadata.is_dir(),
        "Maintenance directory parent must be a real directory"
    );
    if !present(path)? {
        match DirBuilder::new().mode(0o700).create(path) {
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            result => result.context("Create maintenance directory")?,
        }
        File::open(parent)?.sync_all()?;
    }
    let metadata = real_metadata(path)?;
    ensure!(
        metadata.is_dir()
            && metadata.uid() == parent_metadata.uid()
            && metadata.mode() & if private { 0o077 } else { 0o022 } == 0,
        "Maintenance state must be in a real private directory"
    );
    Ok(())
}

fn ensure_data(home: &Path) -> Result<PathBuf> {
    for path in [
        "Library",
        "Library/Application Support",
        "Library/Application Support/LTSC Studio",
    ] {
        ensure_dir(&home.join(path), false)?;
    }
    Ok(data_dir(home))
}

pub(super) fn acquire_mutation_lock() -> Result<File> {
    let home = account_home()?;
    let data = ensure_data(&home)?;
    let path = data.join("maintenance.lock");
    let uid = real_metadata(&home)?.uid();
    let valid = |metadata: &Metadata| {
        metadata.is_file()
            && metadata.uid() == uid
            && metadata.mode() & 0o077 == 0
            && metadata.nlink() == 1
            && metadata.len() == 0
    };
    if present(&path)? {
        let metadata = real_metadata(&path)?;
        ensure!(
            valid(&metadata),
            "Maintenance lock must be a private regular file"
        );
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(NO_FOLLOW_NONBLOCK)
        .open(&path)
        .context("Open the shared macOS maintenance lock")?;
    let opened = lock.metadata()?;
    ensure!(
        valid(&opened) && same_file(&opened, &real_metadata(&path)?),
        "Maintenance lock changed while opening"
    );
    lock.try_lock()
        .context("Another macOS mutation or Topgrade run holds the maintenance lock")?;
    no_pending(&home)?;
    Ok(lock)
}

pub(super) fn acquire_engine_activation_lock(home: &Path) -> Result<File> {
    let base = home.join(".local/share/topgrade-rustup-head");
    let uid = real_metadata(home)?.uid();
    let directory =
        real_metadata(&base).context("Inspect the managed Topgrade release directory")?;
    ensure!(
        directory.is_dir() && directory.uid() == uid && directory.mode() & 0o022 == 0,
        "Topgrade release directory must be user-owned and protected from other writers"
    );
    let path = base.join("lock");
    let valid = |metadata: &Metadata| {
        metadata.is_file()
            && metadata.uid() == uid
            && metadata.nlink() == 1
            && metadata.mode() & 0o077 == 0
    };
    if present(&path)? {
        ensure!(
            valid(&real_metadata(&path)?),
            "Unsafe Topgrade activation lock"
        );
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(NO_FOLLOW_NONBLOCK)
        .open(&path)
        .context("Open the Topgrade activation lock")?;
    let opened = lock.metadata()?;
    ensure!(
        valid(&opened) && same_file(&opened, &real_metadata(&path)?),
        "Topgrade activation lock changed while opening"
    );
    lock.try_lock()
        .context("Another run or engine switch holds the Topgrade activation lock")?;
    Ok(lock)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema_version: u8,
    studio_commit: String,
    component_tree: String,
    binary_sha256: String,
    profile_sha256: String,
    assets: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct Manifest {
    version: u32,
    files: Vec<ManifestFile>,
}

#[derive(Deserialize)]
struct ManifestFile {
    destination: String,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Activation {
    schema_version: u32,
    previous: PathBuf,
    target: PathBuf,
    profile_transaction: Option<String>,
}

struct Release {
    identity: EngineIdentity,
    digest: String,
    assets: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Tool {
    path: PathBuf,
    device: u64,
    inode: u64,
    size: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in super::super) struct Proof {
    home: PathBuf,
    engine: EngineIdentity,
    digest: String,
    tools: BTreeMap<&'static str, Tool>,
}

fn require_running(cancel: &AtomicBool) -> Result<()> {
    ensure!(
        !cancel.load(Ordering::Acquire),
        "Maintenance operation was cancelled"
    );
    Ok(())
}

fn release_tree<'a>(home: &Path, binary: &'a Path) -> Result<&'a str> {
    let releases = home.join(".local/share/topgrade-rustup-head/releases");
    let relative = binary
        .strip_prefix(releases)
        .context("Topgrade engine is outside its managed releases")?;
    let parts = relative.components().collect::<Vec<_>>();
    let [
        Component::Normal(tree),
        Component::Normal(bin),
        Component::Normal(name),
    ] = parts.as_slice()
    else {
        anyhow::bail!("Unexpected managed Topgrade engine layout");
    };
    let tree = tree.to_str().context("Invalid Topgrade component tree")?;
    ensure!(
        *bin == "bin" && *name == "topgrade" && valid_hex(tree, 40),
        "Unexpected managed Topgrade engine layout"
    );
    Ok(tree)
}

fn owned_file(path: &Path, uid: u32) -> Result<Metadata> {
    let metadata = real_metadata(path)?;
    ensure!(
        metadata.is_file() && metadata.uid() == uid && metadata.mode() & 0o022 == 0,
        "Managed Topgrade file is unsafe: {}",
        path.display()
    );
    Ok(metadata)
}

fn valid_version(version: &str) -> bool {
    version.len() <= 96
        && version.as_bytes().first().is_some_and(u8::is_ascii_digit)
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".+-".contains(&byte))
}

fn inspect_release(home: &Path, binary: &Path, cancel: &AtomicBool) -> Result<Release> {
    let tree = release_tree(home, binary)?;
    let release = home
        .join(".local/share/topgrade-rustup-head/releases")
        .join(tree);
    let uid = real_metadata(home)?.uid();
    let receipt_path = release.join("studio-engine.json");
    owned_file(&receipt_path, uid)?;
    let bytes = read_regular(&receipt_path, JSON_LIMIT)
        .context("Installed Topgrade does not have a readable Studio engine receipt")?;
    let receipt: Receipt = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("Invalid Studio engine receipt schema"))?;
    ensure!(
        receipt.schema_version == 1
            && receipt.component_tree == tree
            && valid_hex(&receipt.studio_commit, 40)
            && valid_hex(&receipt.binary_sha256, 64)
            && valid_hex(&receipt.profile_sha256, 64)
            && receipt.assets.len() == ASSETS.len()
            && ASSETS.iter().all(|name| receipt
                .assets
                .get(*name)
                .is_some_and(|hash| valid_hex(hash, 64))),
        "Studio engine receipt has an unexpected provenance or asset set"
    );
    let executable = owned_file(binary, uid)?;
    ensure!(
        executable.mode() & 0o111 != 0 && sha256_file(binary)? == receipt.binary_sha256,
        "Managed Topgrade binary has drifted"
    );
    let manifest_path = release.join("profile-manifest.json");
    owned_file(&manifest_path, uid)?;
    let manifest_bytes = read_regular(&manifest_path, JSON_LIMIT)?;
    let packaged_manifest = release.join("profile/manifest.json");
    owned_file(&packaged_manifest, uid)?;
    ensure!(
        sha256_bytes(&manifest_bytes)? == receipt.profile_sha256
            && sha256_file(&packaged_manifest)? == receipt.profile_sha256,
        "Managed Topgrade profile manifest has drifted"
    );
    let manifest: Manifest =
        serde_json::from_slice(&manifest_bytes).context("Invalid managed profile manifest")?;
    ensure!(
        manifest.version == 1
            && receipt.assets.iter().all(|(destination, hash)| {
                let mut entries = manifest
                    .files
                    .iter()
                    .filter(|file| &file.destination == destination);
                entries.next().is_some_and(|file| &file.sha256 == hash) && entries.next().is_none()
            }),
        "Managed Topgrade receipt disagrees with the profile manifest"
    );
    let output = run_configured(
        &Invocation {
            program: binary.to_owned(),
            args: vec!["--version".into()],
            vscode_node: false,
            npm_node: false,
        },
        cancel,
        Duration::from_secs(5),
        |command| {
            command.env_clear().env("HOME", home).current_dir(home);
        },
    )
    .context("Read the verified Topgrade binary version")?;
    ensure!(
        output.code == 0
            && !output.cancelled
            && !output.timed_out
            && unchanged(&executable, &real_metadata(binary)?),
        "Managed Topgrade version probe failed or the executable changed"
    );
    let version = output
        .stdout
        .trim_end_matches(['\n', '\r'])
        .strip_prefix("topgrade ")
        .filter(|version| valid_version(version))
        .context("Unexpected Topgrade version output")?;
    Ok(Release {
        identity: EngineIdentity {
            binary: binary.to_owned(),
            studio_commit: receipt.studio_commit,
            component_tree: receipt.component_tree,
            binary_sha256: receipt.binary_sha256,
            profile_sha256: receipt.profile_sha256,
            upstream_version: version.into(),
            profile_version: manifest.version,
        },
        digest: sha256_bytes(&bytes)?,
        assets: receipt.assets,
    })
}

pub(super) fn verified_engine_release(home: &Path, binary: &Path) -> Result<EngineIdentity> {
    Ok(inspect_release(home, binary, &AtomicBool::new(false))?.identity)
}

fn active_release(home: &Path, cancel: &AtomicBool) -> Result<Release> {
    let link = home.join(".local/bin/topgrade");
    ensure!(
        fs::symlink_metadata(&link)
            .context("Managed Topgrade launcher is missing")?
            .is_symlink(),
        "Managed Topgrade launcher must point to a Studio engine release"
    );
    ensure!(
        real_metadata(&home.join(".local/bin"))?.is_dir(),
        "Invalid Topgrade launcher directory"
    );
    let binary = link
        .canonicalize()
        .context("Resolve the managed Topgrade engine")?;
    let release = inspect_release(home, &binary, cancel)?;
    ensure!(
        link.canonicalize()? == binary,
        "Managed Topgrade launcher changed during verification"
    );
    Ok(release)
}

fn rollback_candidate(
    home: &Path,
    current: &EngineIdentity,
    cancel: &AtomicBool,
) -> Result<Option<EngineIdentity>> {
    let path = home
        .join(".local/share/topgrade-rustup-head")
        .join(format!("studio-activation-{}.json", current.component_tree));
    if !present(&path)? {
        return Ok(None);
    }
    owned_file(&path, real_metadata(home)?.uid())?;
    let activation: Activation = serde_json::from_slice(&read_regular(&path, JSON_LIMIT)?)
        .context("Invalid Topgrade activation record")?;
    ensure!(
        activation.schema_version == 1
            && activation.target == current.binary
            && activation.previous != current.binary,
        "Topgrade activation record does not match the active release"
    );
    let previous = inspect_release(home, &activation.previous, cancel)?.identity;
    ensure!(
        match activation.profile_transaction.as_deref() {
            Some(tx) => valid_hex(tx, 32),
            None => previous.profile_sha256 == current.profile_sha256,
        },
        "Topgrade activation has no matching profile rollback"
    );
    Ok(Some(previous))
}

fn profile_proof(home: PathBuf, release: &Release, cancel: &AtomicBool) -> Result<Proof> {
    no_pending(&home)?;
    for name in ASSETS {
        require_running(cancel)?;
        let path = home.join(name);
        let metadata = real_metadata(&path)
            .with_context(|| format!("Inspect managed Topgrade asset {name}"))?;
        ensure!(
            metadata.uid() == real_metadata(&home)?.uid()
                && metadata.mode() & 0o022 == 0
                && sha256_file(&path)? == release.assets[name],
            "Managed Topgrade asset has drifted: {name}"
        );
    }
    ensure!(
        home.join(".local/bin/topgrade").canonicalize()? == release.identity.binary,
        "Managed Topgrade launcher changed during verification"
    );
    no_pending(&home)?;
    Ok(Proof {
        home,
        engine: release.identity.clone(),
        digest: release.digest.clone(),
        tools: BTreeMap::new(),
    })
}

fn load_proof(cancel: &AtomicBool) -> Result<Proof> {
    require_running(cancel)?;
    let home = account_home()?;
    let release = active_release(&home, cancel)?;
    profile_proof(home, &release, cancel)
}

fn tool(name: &str) -> Result<Tool> {
    let candidates = if name == "curl" {
        vec![PathBuf::from("/usr/bin/curl")]
    } else {
        PATH.split(':')
            .map(|dir| Path::new(dir).join(name))
            .collect()
    };
    for link in candidates {
        if !present(&link)? {
            continue;
        }
        let path = link
            .canonicalize()
            .with_context(|| format!("Resolve {name} in the fixed maintenance PATH"))?;
        ensure!(
            path.starts_with("/opt/homebrew")
                || path.starts_with("/usr/bin")
                || path.starts_with("/bin")
                || path.starts_with("/usr/sbin")
                || path.starts_with("/sbin"),
            "Maintenance tool {name} resolves outside the trusted tool roots"
        );
        let metadata = real_metadata(&path)?;
        ensure!(
            metadata.is_file() && metadata.mode() & 0o111 != 0 && metadata.mode() & 0o022 == 0,
            "Maintenance tool {name} must be a trusted executable file"
        );
        return Ok(Tool {
            path,
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            mtime: (metadata.mtime(), metadata.mtime_nsec()),
            ctime: (metadata.ctime(), metadata.ctime_nsec()),
        });
    }
    anyhow::bail!("Required maintenance tool {name} is unavailable in the fixed PATH")
}

fn required_tools(id: &str) -> &'static [&'static str] {
    match id {
        "github_cli_skills" | "github_cli_extensions" => &["gh"],
        "helm" => &["helm", "curl"],
        "brew_linkage_audit" => &["brew"],
        "tldr" => &["tldr"],
        _ => &[],
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inventory {
    schema_version: u8,
    steps: Vec<InventoryStep>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InventoryStep {
    id: String,
    label: String,
    mode: InventoryMode,
    tools: Vec<String>,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum InventoryMode {
    Controlled,
    Terminal,
}

fn parse_inventory(bytes: &[u8]) -> Result<Vec<Step>> {
    ensure!(
        bytes.len() as u64 <= JSON_LIMIT,
        "Topgrade inventory is too large"
    );
    let inventory: Inventory = serde_json::from_slice(bytes).context("Parse Topgrade inventory")?;
    ensure!(
        inventory.schema_version == 1 && (1..=512).contains(&inventory.steps.len()),
        "Unsupported Topgrade inventory"
    );
    let mut seen = BTreeSet::new();
    let mut steps = Vec::with_capacity(inventory.steps.len());
    for row in inventory.steps {
        ensure!(
            (1..=96).contains(&row.id.len())
                && row
                    .id
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
                && (1..=160).contains(&row.label.len())
                && !row.label.chars().any(char::is_control)
                && seen.insert(row.id.clone()),
            "Invalid or duplicate Topgrade inventory row"
        );
        let terminal = row.mode == InventoryMode::Terminal;
        ensure!(
            terminal && row.tools.is_empty() && !IDS.contains(&row.id.as_str())
                || !terminal
                    && IDS.contains(&row.id.as_str())
                    && row.tools == required_tools(&row.id),
            "Unrecognized Topgrade execution route"
        );
        steps.push(Step {
            id: row.id,
            label: row.label,
            detail: String::new(),
            terminal,
            available: !terminal,
        });
    }
    ensure!(
        IDS.iter().all(|id| seen.contains(*id)),
        "Topgrade engine lacks expected native steps"
    );
    Ok(steps)
}

fn inventory(proof: &Proof) -> Result<Vec<Step>> {
    let output = Command::new(&proof.engine.binary)
        .arg("--studio-inventory")
        .current_dir(&proof.home)
        .env_clear()
        .env("HOME", &proof.home)
        .env("PATH", PATH)
        .stdin(Stdio::null())
        .output()
        .context("Read the verified Topgrade engine inventory")?;
    ensure!(
        output.status.success(),
        "Installed Topgrade did not provide a Studio step inventory. Install the matching engine."
    );
    parse_inventory(&output.stdout)
}

pub(super) fn check(cancel: &AtomicBool) -> Result<Check> {
    require_running(cancel)?;
    let mut engine = None;
    let mut rollback = None;
    let mut rollback_detail = None;
    let checked = (|| -> Result<_> {
        let home = account_home()?;
        let release = active_release(&home, cancel)?;
        engine = Some(release.identity.clone());
        let proof = profile_proof(home, &release, cancel)?;
        match rollback_candidate(&proof.home, &proof.engine, cancel) {
            Ok(Some(previous)) => rollback = Some(previous),
            Ok(None) => rollback_detail = Some("No retained activation is recorded.".into()),
            Err(error) => rollback_detail = Some(format!("{error:#}")),
        }
        inventory(&proof)
    })();
    let mut steps = match checked {
        Ok(steps) => steps,
        Err(error) => {
            require_running(cancel)?;
            return Ok(Check {
                available: false,
                detail: format!("{error:#}"),
                steps: Vec::new(),
                engine,
                rollback,
                rollback_detail,
            });
        }
    };
    for step in &mut steps {
        if let Err(error) = required_tools(&step.id)
            .iter()
            .try_for_each(|name| tool(name).map(|_| ()))
        {
            step.available = false;
            step.detail = error.to_string();
        }
    }
    Ok(Check {
        available: steps.iter().any(|step| step.available && !step.terminal),
        detail: "Managed Topgrade engine, profile and step inventory verified.".into(),
        steps,
        engine,
        rollback,
        rollback_detail,
    })
}

fn review_proof(selected: &[String], cancel: &AtomicBool) -> Result<Proof> {
    let mut proof = load_proof(cancel)?;
    for id in selected {
        for name in required_tools(id) {
            if !proof.tools.contains_key(name) {
                proof.tools.insert(name, tool(name)?);
            }
        }
    }
    Ok(proof)
}

pub(super) fn review(selected: &BTreeSet<String>, cancel: &AtomicBool) -> Result<Review> {
    ensure!(
        !selected.is_empty() && selected.iter().all(|id| IDS.contains(&id.as_str())),
        "Select only known macOS maintenance steps"
    );
    let selected = selected.iter().cloned().collect::<Vec<_>>();
    let proof = review_proof(&selected, cancel)?;
    let steps = inventory(&proof)?
        .into_iter()
        .filter(|step| !step.terminal && selected.contains(&step.id))
        .collect();
    Ok(Review {
        steps,
        engine: proof.engine.clone(),
        proof,
        selected,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_cannot_grant_execution_to_a_terminal_step() {
        let mut rows = IDS
            .iter()
            .map(|id| {
                serde_json::json!({
                    "id": id, "label": id, "mode": "controlled", "tools": required_tools(id)
                })
            })
            .collect::<Vec<_>>();
        rows.push(serde_json::json!({
            "id": "brew_formula", "label": "Homebrew Formulae", "mode": "terminal", "tools": []
        }));
        let bytes = |rows: &[serde_json::Value]| {
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1, "steps": rows
            }))
            .unwrap()
        };
        let steps = parse_inventory(&bytes(&rows)).unwrap();
        assert!(steps.last().unwrap().terminal && !steps.last().unwrap().available);
        rows.last_mut().unwrap()["mode"] = serde_json::json!("controlled");
        assert!(parse_inventory(&bytes(&rows)).is_err());
    }
}
