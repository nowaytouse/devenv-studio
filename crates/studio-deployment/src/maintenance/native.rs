use super::{
    Check, Review, Step,
    protocol::{IDS, valid_hex},
};
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
};

mod run;
pub(super) use run::{history, run};

const PATH: &str = "/opt/homebrew/bin:/opt/homebrew/sbin:/usr/bin:/bin:/usr/sbin:/sbin";
const PYTHON: &str = "/opt/homebrew/opt/python@3.15/bin/python3.15";
const FILE_LIMIT: u64 = 128 * 1024 * 1024;
const JSON_LIMIT: u64 = 128 * 1024;
const NO_FOLLOW_NONBLOCK: i32 = 0x100 | 0x4;
const ASSETS: [&str; 9] = [
    ".local/bin/topgrade-gh-skills-verified",
    ".local/bin/topgrade-helm-verified",
    ".local/bin/topgrade-cursor-native-reconcile.py",
    ".local/bin/normalize-gh-skill-lock.py",
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
    binary: PathBuf,
    digest: String,
    component_tree: String,
    tools: BTreeMap<&'static str, Tool>,
}

fn require_running(cancel: &AtomicBool) -> Result<()> {
    ensure!(
        !cancel.load(Ordering::Acquire),
        "Maintenance operation was cancelled"
    );
    Ok(())
}

fn load_proof(cancel: &AtomicBool) -> Result<Proof> {
    require_running(cancel)?;
    let home = account_home()?;
    no_pending(&home)?;
    let link = home.join(".local/bin/topgrade");
    ensure!(
        fs::symlink_metadata(&link)
            .context("Managed Topgrade launcher is missing")?
            .is_symlink(),
        "Managed Topgrade launcher must point to a Studio engine release"
    );
    ensure!(
        real_metadata(link.parent().unwrap())?.is_dir(),
        "Invalid Topgrade launcher directory"
    );
    let binary = link
        .canonicalize()
        .context("Resolve the managed Topgrade engine")?;
    let releases = home.join(".local/share/topgrade-rustup-head/releases");
    let relative = binary
        .strip_prefix(&releases)
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
    let release = releases.join(tree);
    let bytes = read_regular(&release.join("studio-engine.json"), JSON_LIMIT)
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
    ensure!(
        real_metadata(&binary)?.mode() & 0o111 != 0
            && sha256_file(&binary)? == receipt.binary_sha256,
        "Managed Topgrade binary has drifted"
    );
    ensure!(
        sha256_file(&release.join("profile-manifest.json"))? == receipt.profile_sha256,
        "Managed Topgrade profile manifest has drifted"
    );
    for name in ASSETS {
        require_running(cancel)?;
        let path = home.join(name);
        let metadata = real_metadata(&path)
            .with_context(|| format!("Inspect managed Topgrade asset {name}"))?;
        ensure!(
            metadata.uid() == real_metadata(&home)?.uid()
                && metadata.mode() & 0o022 == 0
                && sha256_file(&path)? == receipt.assets[name],
            "Managed Topgrade asset has drifted: {name}"
        );
    }
    let digest = sha256_bytes(&bytes)?;
    ensure!(
        link.canonicalize()? == binary,
        "Managed Topgrade launcher changed during verification"
    );
    Ok(Proof {
        home,
        binary,
        digest,
        component_tree: receipt.component_tree,
        tools: BTreeMap::new(),
    })
}

fn tool(name: &str) -> Result<Tool> {
    let candidates = if name == "python3.15" {
        vec![PathBuf::from(PYTHON)]
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
        if name == "python3.15" {
            ensure!(
                path.starts_with("/opt/homebrew/Cellar/python@3.15")
                    && path.ends_with("bin/python3.15"),
                "Unexpected Homebrew Python 3.15 interpreter"
            );
        }
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
        "github_cli_skills" => &["python3.15", "gh"],
        "helm" => &["python3.15", "helm", "curl"],
        _ => &[],
    }
}

fn steps() -> Vec<Step> {
    [
        (
            IDS[0],
            "GitHub Skills",
            "Update the verified GitHub CLI-owned skills.",
        ),
        (
            IDS[1],
            "Helm repositories",
            "Verify or refresh the existing Helm repository indexes.",
        ),
    ]
    .into_iter()
    .map(|(id, label, detail)| Step {
        id: id.into(),
        label: label.into(),
        detail: detail.into(),
        terminal: false,
        available: true,
    })
    .collect()
}

pub(super) fn check(cancel: &AtomicBool) -> Result<Check> {
    require_running(cancel)?;
    let mut steps = steps();
    if let Err(error) = load_proof(cancel) {
        require_running(cancel)?;
        for step in &mut steps {
            step.available = false;
        }
        return Ok(Check {
            available: false,
            detail: format!("{error:#}"),
            steps,
        });
    }
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
        available: steps.iter().any(|step| step.available),
        detail: "Managed Topgrade engine and profile assets verified.".into(),
        steps,
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
    let steps = steps()
        .into_iter()
        .filter(|step| selected.contains(&step.id))
        .collect();
    let selected = selected.iter().cloned().collect::<Vec<_>>();
    let proof = review_proof(&selected, cancel)?;
    Ok(Review {
        steps,
        proof,
        selected,
    })
}
