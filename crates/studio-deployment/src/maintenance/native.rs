use super::{Check, EngineIdentity, Review, Step};
use crate::process::{Invocation, run_configured};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{c_char, c_int, c_void},
    fs::{self, DirBuilder, File, Metadata, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use studio_core::maintenance::{Step as EngineStep, valid_hex};

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
        real_metadata(&base).context("Inspect the Topgrade activation state directory")?;
    ensure!(
        directory.is_dir() && directory.uid() == uid && directory.mode() & 0o022 == 0,
        "Topgrade activation state directory must be user-owned and protected from other writers"
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
    profile_version: u32,
    assets: BTreeMap<String, String>,
}

struct VerifiedEngine {
    identity: EngineIdentity,
    digest: String,
    assets: BTreeMap<String, String>,
    metadata: Metadata,
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
    llvm: Option<LlvmPlan>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct LlvmPlan {
    digest: String,
    detail: String,
}

fn require_running(cancel: &AtomicBool) -> Result<()> {
    ensure!(
        !cancel.load(Ordering::Acquire),
        "Maintenance operation was cancelled"
    );
    Ok(())
}

fn valid_version(version: &str) -> bool {
    version.len() <= 96
        && version.as_bytes().first().is_some_and(u8::is_ascii_digit)
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".+-".contains(&byte))
}

unsafe extern "C" {
    fn fgetxattr(
        fd: c_int,
        name: *const c_char,
        value: *mut c_void,
        size: usize,
        position: u32,
        options: c_int,
    ) -> isize;
}

fn engine_receipt(file: &File) -> Result<Vec<u8>> {
    let mut bytes = vec![0; JSON_LIMIT as usize];
    // The descriptor remains open and the buffer is writable for its entire length.
    let len = unsafe {
        fgetxattr(
            file.as_raw_fd(),
            c"org.devenv-studio.topgrade-engine".as_ptr(),
            bytes.as_mut_ptr().cast(),
            bytes.len(),
            0,
            0,
        )
    };
    if len < 0 {
        return Err(std::io::Error::last_os_error())
            .context("Read the installed Topgrade engine receipt");
    }
    bytes.truncate(len as usize);
    Ok(bytes)
}

fn inspect_engine(home: &Path, binary: &Path, cancel: &AtomicBool) -> Result<VerifiedEngine> {
    ensure!(
        binary == home.join(".local/bin/topgrade"),
        "Expected the default Topgrade executable"
    );
    let directory = real_metadata(&home.join(".local/bin"))?;
    let uid = real_metadata(home)?.uid();
    ensure!(
        directory.is_dir() && directory.uid() == uid && directory.mode() & 0o022 == 0,
        "Invalid Topgrade executable directory"
    );
    let (file, executable) = open_regular(binary, FILE_LIMIT)?;
    ensure!(
        executable.uid() == uid && executable.mode() & 0o022 == 0 && executable.mode() & 0o111 != 0,
        "Unsafe Topgrade executable"
    );
    let bytes = engine_receipt(&file)?;
    let receipt: Receipt = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("Invalid Studio engine receipt schema"))?;
    ensure!(
        receipt.schema_version == 1
            && receipt.profile_version == 1
            && valid_hex(&receipt.component_tree, 40)
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
    let output = hash_command()
        .stdin(file)
        .output()
        .context("Hash the verified Topgrade executable")?;
    ensure!(
        hash_output(output)? == receipt.binary_sha256
            && unchanged(&executable, &real_metadata(binary)?),
        "Managed Topgrade binary has drifted"
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
    Ok(VerifiedEngine {
        identity: EngineIdentity {
            binary: binary.to_owned(),
            studio_commit: receipt.studio_commit,
            component_tree: receipt.component_tree,
            binary_sha256: receipt.binary_sha256,
            profile_sha256: receipt.profile_sha256,
            upstream_version: version.into(),
            profile_version: receipt.profile_version,
        },
        digest: sha256_bytes(&bytes)?,
        assets: receipt.assets,
        metadata: executable,
    })
}

pub(super) fn verified_engine(home: &Path, binary: &Path) -> Result<EngineIdentity> {
    let cancel = AtomicBool::new(false);
    let engine = inspect_engine(home, binary, &cancel)?;
    Ok(profile_proof(home.to_owned(), &engine, &cancel)?.engine)
}

fn active_engine(home: &Path, cancel: &AtomicBool) -> Result<VerifiedEngine> {
    inspect_engine(home, &home.join(".local/bin/topgrade"), cancel)
}

fn profile_proof(home: PathBuf, engine: &VerifiedEngine, cancel: &AtomicBool) -> Result<Proof> {
    no_pending(&home)?;
    for name in ASSETS {
        require_running(cancel)?;
        let path = home.join(name);
        let metadata = real_metadata(&path)
            .with_context(|| format!("Inspect managed Topgrade asset {name}"))?;
        ensure!(
            metadata.uid() == real_metadata(&home)?.uid()
                && metadata.mode() & 0o022 == 0
                && sha256_file(&path)? == engine.assets[name],
            "Managed Topgrade asset has drifted: {name}"
        );
    }
    ensure!(
        unchanged(&engine.metadata, &real_metadata(&engine.identity.binary)?),
        "Managed Topgrade executable changed during verification"
    );
    no_pending(&home)?;
    Ok(Proof {
        home,
        engine: engine.identity.clone(),
        digest: engine.digest.clone(),
        tools: BTreeMap::new(),
        llvm: None,
    })
}

fn load_proof(cancel: &AtomicBool) -> Result<Proof> {
    require_running(cancel)?;
    let home = account_home()?;
    let engine = active_engine(&home, cancel)?;
    profile_proof(home, &engine, cancel)
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
    EngineStep::find(id).map_or(&[], EngineStep::tools)
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
            terminal && row.tools.is_empty() && EngineStep::find(&row.id).is_none()
                || !terminal
                    && EngineStep::find(&row.id).is_some()
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
        EngineStep::ALL
            .iter()
            .filter(|step| !step.manual_llvm())
            .all(|step| seen.contains(step.id())),
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
    let checked = (|| -> Result<_> {
        let home = account_home()?;
        let active = active_engine(&home, cancel)?;
        engine = Some(active.identity.clone());
        let proof = profile_proof(home, &active, cancel)?;
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
    if let Some(id) = selected
        .iter()
        .find(|id| EngineStep::find(id).is_some_and(EngineStep::manual_llvm))
    {
        ensure!(selected.len() == 1, "Review each LLVM channel on its own");
        let output = run_configured(
            &Invocation {
                program: proof.engine.binary.clone(),
                args: vec!["--studio-llvm-plan".into(), id.into()],
                vscode_node: false,
                npm_node: false,
            },
            cancel,
            Duration::from_secs(45),
            |command| {
                command
                    .env_clear()
                    .env("HOME", &proof.home)
                    .env("PATH", PATH)
                    .env("HOMEBREW_NO_AUTO_UPDATE", "1")
                    .current_dir(&proof.home);
            },
        )
        .context("Inspect the exact LLVM candidate")?;
        require_running(cancel)?;
        ensure!(
            !output.timed_out && !output.cancelled,
            "LLVM candidate check timed out or was cancelled"
        );
        ensure!(
            output.code == 0,
            "LLVM candidate unavailable: {}",
            output.stderr.trim()
        );
        let plan: LlvmPlan =
            serde_json::from_str(&output.stdout).context("Invalid LLVM review output")?;
        ensure!(
            valid_hex(&plan.digest, 64)
                && (1..=600).contains(&plan.detail.len())
                && !plan.detail.chars().any(char::is_control),
            "Invalid LLVM review"
        );
        proof.llvm = Some(plan);
    }
    Ok(proof)
}

pub(super) fn review(selected: &BTreeSet<String>, cancel: &AtomicBool) -> Result<Review> {
    ensure!(
        !selected.is_empty() && selected.iter().all(|id| EngineStep::find(id).is_some()),
        "Select only known macOS maintenance steps"
    );
    let selected = selected.iter().cloned().collect::<Vec<_>>();
    let proof = review_proof(&selected, cancel)?;
    let mut steps: Vec<_> = inventory(&proof)?
        .into_iter()
        .filter(|step| !step.terminal && selected.contains(&step.id))
        .collect();
    ensure!(
        steps.len() == selected.len(),
        "The active engine does not support every selected step"
    );
    if let Some(plan) = &proof.llvm {
        steps[0].detail.clone_from(&plan.detail);
    }
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
        let mut rows = EngineStep::ALL
            .into_iter()
            .map(EngineStep::id)
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
