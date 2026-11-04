//! Check npm's bundled dependencies and promote only the reviewed patch leaves.
mod advisory;
mod transaction;

use super::{account_home, check_dir, check_file, exists, is_hex, open_file, tool_command, user_only};
use crate::command::CommandExt as _;
use color_eyre::eyre::{Context, ContextCompat, Result, ensure};
use nix::libc::{O_NOFOLLOW, O_NONBLOCK};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, Read},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

const REGISTRY: &str = "https://registry.npmjs.org";
const NAMES: [&str; 2] = ["undici", "ip-address"];
const LIVE: &str = "/opt/homebrew/lib/node_modules/npm";
const FILE_LIMIT: u64 = 32 * 1024 * 1024;
const JSON_LIMIT: u64 = 4 * 1024 * 1024;
const TREE_LIMIT: u64 = 512 * 1024 * 1024;
const FILES_LIMIT: usize = 20_000;
const REFERENCES: [(&str, &[u8]); 4] = [
    (
        "before.json",
        include_bytes!("../../profiles/macos/share/npm-security/before.json"),
    ),
    (
        "leaf-candidate-after.json",
        include_bytes!("../../profiles/macos/share/npm-security/leaf-candidate-after.json"),
    ),
    (
        "validation-report.json",
        include_bytes!("../../profiles/macos/share/npm-security/validation-report.json"),
    ),
    (
        "leaf-candidate-report.json",
        include_bytes!("../../profiles/macos/share/npm-security/leaf-candidate-report.json"),
    ),
];

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged, deny_unknown_fields)]
enum Entry {
    File { sha256: String },
    Symlink { link: String },
}

type Tree = BTreeMap<String, Entry>;

fn relative(value: &str) -> bool {
    value
        .split('/')
        .all(|part| !matches!(part, "" | "." | "..") && !part.chars().any(char::is_control))
}

fn tree(root: &Path) -> Result<Tree> {
    check_dir(root, false)?;
    let mut tree = Tree::new();
    let mut total = 0;
    for (index, entry) in WalkDir::new(root)
        .follow_links(false)
        .min_depth(1)
        .into_iter()
        .enumerate()
    {
        ensure!(index < FILES_LIMIT, "npm tree exceeds its entry limit");
        let entry = entry?;
        let path = entry.path();
        let key = path.strip_prefix(root)?.to_str().context("Invalid npm path encoding")?;
        ensure!(relative(key), "Invalid npm file path");
        if entry.file_type().is_dir() {
            check_dir(path, false)?;
            continue;
        }
        let value = if entry.file_type().is_symlink() {
            ensure!(path.canonicalize()?.starts_with(root), "Escaping npm package symlink");
            Entry::Symlink {
                link: fs::read_link(path)?
                    .to_str()
                    .context("Invalid npm symlink encoding")?
                    .into(),
            }
        } else {
            let mut file = open_file(path, false, FILE_LIMIT)?;
            let size = file.metadata()?.len();
            total += size;
            ensure!(total <= TREE_LIMIT, "npm tree exceeds its byte limit");
            let mut digest = Sha256::new();
            ensure!(
                io::copy((&mut file).take(FILE_LIMIT + 1).by_ref(), &mut digest)? == size,
                "npm file changed while hashing"
            );
            Entry::File {
                sha256: format!("{:x}", digest.finalize()),
            }
        };
        tree.insert(key.into(), value);
    }
    Ok(tree)
}

fn bytes(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    open_file(path, false, limit)?.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "npm reference exceeds its byte limit");
    Ok(bytes)
}

fn json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    advisory::strict(&bytes(path, JSON_LIMIT)?)
}

fn manifest(path: &Path) -> Result<Tree> {
    let tree: Tree = json(path)?;
    ensure!(
        !tree.is_empty()
            && tree.len() <= FILES_LIMIT
            && tree.iter().all(|(key, entry)| relative(key)
                && match entry {
                    Entry::File { sha256 } => is_hex(sha256, 64),
                    Entry::Symlink { link } => !link.is_empty() && !link.chars().any(char::is_control),
                }),
        "Invalid npm tree reference"
    );
    Ok(tree)
}

fn cache_path(path: &str) -> bool {
    path.starts_with("node_modules/node-gyp/gyp/pylib/")
        && path.contains("/__pycache__/")
        && path.ends_with(".cpython-315.pyc")
}

fn matches_reference(current: &Tree, expected: &Tree) -> bool {
    current
        .iter()
        .filter(|(path, entry)| !(cache_path(path) && expected.get(*path) == Some(*entry)))
        .eq(expected.iter().filter(|(path, _)| !cache_path(path)))
}

fn leaf(tree: &Tree, name: &str) -> Tree {
    let prefix = format!("node_modules/{name}/");
    tree.iter()
        .filter_map(|(key, value)| key.strip_prefix(&prefix).map(|key| (key.into(), value.clone())))
        .collect()
}

fn replace_leaf(tree: &mut Tree, name: &str, leaf: &Tree) {
    let prefix = format!("node_modules/{name}/");
    tree.retain(|key, _| !key.starts_with(&prefix));
    tree.extend(
        leaf.iter()
            .map(|(key, value)| (format!("{prefix}{key}"), value.clone())),
    );
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Validation {
    all_checks_passed: bool,
    candidate_unchanged_after_tests: bool,
    official_registry_advisories: BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReviewedPackage {
    name: String,
    before: String,
    after: String,
    integrity: String,
    consumer: String,
    declared_range: String,
    runtime_dependencies_unchanged: bool,
    install_lifecycle_hooks: Vec<String>,
    prepare_script_not_executed: String,
}

#[derive(Deserialize)]
struct Report {
    registry: String,
    packages: Vec<ReviewedPackage>,
}

fn receipt(probe: &Path, staging: &Path) -> Result<Vec<u8>> {
    let validation: Validation = json(&probe.join("validation-report.json"))?;
    ensure!(
        validation.all_checks_passed
            && validation.candidate_unchanged_after_tests
            && validation.official_registry_advisories.is_empty(),
        "npm patch validation report did not approve the candidate"
    );
    let report: Report = json(&probe.join("leaf-candidate-report.json"))?;
    ensure!(
        report.registry == REGISTRY && report.packages.len() == NAMES.len(),
        "Invalid npm patch report"
    );
    for (row, expected) in report.packages.iter().zip([
        ("undici", "6.29.0", "6.29.1", "node-gyp"),
        ("ip-address", "10.7.2", "10.7.5", "socks"),
    ]) {
        ensure!(
            (&*row.name, &*row.before, &*row.after, &*row.consumer) == expected
                && row
                    .integrity
                    .strip_prefix("sha512-")
                    .is_some_and(|hash| hash.len() == 88
                        && hash
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"+/=".contains(&byte)))
                && row.runtime_dependencies_unchanged
                && row.install_lifecycle_hooks.is_empty(),
            "Unexpected npm patch report identity"
        );
        let range = nodejs_semver::Range::parse(&row.declared_range).wrap_err("Invalid reviewed npm range")?;
        ensure!(
            range.satisfies(&nodejs_semver::Version::parse(&row.after).wrap_err("Invalid reviewed npm version")?),
            "Reviewed npm package is outside its consumer's range"
        );
    }
    let mut data = serde_json::to_vec_pretty(&serde_json::json!({
        "npmVersion": "12.2.1", "backupDirectory": staging, "source": REGISTRY, "packages": report.packages,
        "beforeManifest": probe.join("before.json"), "afterManifest": probe.join("leaf-candidate-after.json"),
        "validationReport": probe.join("validation-report.json")
    }))?;
    data.push(b'\n');
    Ok(data)
}

fn directory(home: &Path, suffix: &str) -> Result<PathBuf> {
    let mut path = home.to_path_buf();
    for part in suffix.split('/') {
        path.push(part);
        if !exists(&path)? {
            DirBuilder::new().mode(0o700).create(&path)?;
            File::open(path.parent().context("Missing npm state parent")?)?.sync_all()?;
        }
        check_dir(&path, false)?;
    }
    Ok(path)
}

pub(super) fn run() -> Result<()> {
    user_only()?;
    let home = account_home()?;
    let base = directory(&home, ".local/state/topgrade-macos-profile/npm-security")?;
    let probe = home.join(".local/share/topgrade-macos-profile/npm-security");
    check_dir(&probe, false)?;
    let live = Path::new(LIVE);
    check_dir(live, false)?;
    ensure!(
        probe.canonicalize()? == probe && live.canonicalize()? == live,
        "Refusing a relocated npm profile or npm root"
    );
    let lock_path = base.join("lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(&lock_path)?;
    check_file(&lock, &lock_path, true, 0)?;
    lock.try_lock().wrap_err("Another npm security guard is running")?;
    transaction::recover(&base, live)?;
    for (name, expected) in REFERENCES {
        ensure!(
            bytes(&probe.join(name), JSON_LIMIT)? == expected,
            "Installed npm review asset differs from the engine: {name}"
        );
    }
    let before = manifest(&probe.join("before.json"))?;
    let after = manifest(&probe.join("leaf-candidate-after.json"))?;
    ensure!(
        before
            .iter()
            .filter(|(key, _)| cache_path(key))
            .all(|(key, value)| after.get(key) == Some(value)),
        "npm manifests disagree on optional bytecode caches"
    );
    let network = super::proxy::Route::detect();
    let mut update = tool_command(Path::new("/opt/homebrew/bin/npm"))?;
    network.apply(&mut update);
    update
        .args([
            "update",
            "--global",
            "npm",
            "--ignore-scripts",
            "--registry",
            REGISTRY,
            "--prefix",
            "/opt/homebrew",
            "--strict-ssl=true",
        ])
        .status_checked()?;
    let current = tree(live)?;
    if matches_reference(&current, &after) {
        println!("Already current: both verified npm security patch leaves");
    } else if matches_reference(&current, &before) {
        transaction::install(&base, &probe, live, &current, &after)?;
    } else {
        let versions = advisory::audit(live, &network)?;
        ensure!(tree(live)? == current, "Live npm changed during its advisory query");
        println!(
            "PASS npm {}: undici {} and ip-address {} have no matching known advisories",
            versions[0], versions[1], versions[2]
        );
    }
    Ok(())
}

#[cfg(test)]
#[test]
fn the_bundled_patch_reference_matches_both_shipped_candidate_trees() -> Result<()> {
    let probe = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("profiles/macos/share/npm-security")
        .canonicalize()?;
    let after = manifest(&probe.join("leaf-candidate-after.json"))?;
    for name in NAMES {
        assert_eq!(tree(&probe.join("node_modules").join(name))?, leaf(&after, name));
    }
    receipt(&probe, Path::new("/test-stage"))?;
    Ok(())
}

#[cfg(test)]
#[test]
#[ignore = "reads this Mac's installed npm and may query official advisories"]
fn installed_tree_can_be_checked_without_running_npm_update() -> Result<()> {
    let home = account_home()?;
    let probe = home.join(".local/share/topgrade-macos-profile/npm-security");
    let live = Path::new(LIVE);
    let current = tree(live)?;
    for (name, expected) in REFERENCES {
        ensure!(
            bytes(&probe.join(name), JSON_LIMIT)? == expected,
            "Installed npm reference drift: {name}"
        );
    }
    let before = manifest(&probe.join("before.json"))?;
    let after = manifest(&probe.join("leaf-candidate-after.json"))?;
    if matches_reference(&current, &before) || matches_reference(&current, &after) {
        println!("Installed npm matches a reviewed tree");
    } else {
        let versions = advisory::audit(live, &super::proxy::Route::detect())?;
        println!("Installed npm {} passed the bundled advisory check", versions[0]);
    }
    ensure!(tree(live)? == current, "npm tree changed during inspection");
    Ok(())
}
