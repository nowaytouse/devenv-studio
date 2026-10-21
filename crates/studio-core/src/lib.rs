//! Trusted configuration catalog, plans and portable, non-overwriting exports.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{Read, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_INPUT_BYTES: u64 = 1_048_576;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    Win10Ltsc2021,
    Win11Ltsc2024,
}
impl Target {
    pub const ALL: [Self; 2] = [Self::Win10Ltsc2021, Self::Win11Ltsc2024];
    pub fn label(self) -> &'static str {
        match self {
            Self::Win10Ltsc2021 => "Windows 10 Enterprise LTSC 2021",
            Self::Win11Ltsc2024 => "Windows 11 Enterprise / IoT Enterprise LTSC 2024",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub schema_version: u32,
    pub revision: String,
    pub sources: Vec<Source>,
    pub actions: Vec<Action>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub name: String,
    pub url: String,
    pub revision: String,
    pub license: String,
    pub description: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    pub id: String,
    pub title: String,
    pub description: String,
    pub category: String,
    pub sources: Vec<String>,
    pub evidence: Vec<String>,
    pub targets: Vec<Target>,
    pub restart: String,
    pub risk: String,
    pub changes: Vec<RegistryChange>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hive {
    CurrentUser,
    LocalMachine,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryChange {
    pub hive: Hive,
    pub path: String,
    pub name: String,
    pub value: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub schema_version: u32,
    pub target: Target,
    #[serde(deserialize_with = "unique_selection")]
    pub selected: BTreeSet<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub schema_version: u32,
    pub catalog_revision: String,
    pub target: Target,
    pub actions: Vec<Action>,
}

fn unique_selection<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<BTreeSet<String>, D::Error> {
    let ids = Vec::<String>::deserialize(d)?;
    let mut selected = BTreeSet::new();
    for id in ids {
        if !selected.insert(id) {
            return Err(serde::de::Error::custom("duplicate selected action ID"));
        }
    }
    Ok(selected)
}

fn identifier(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 100
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"-_.".contains(&c))
}
fn text(s: &str) -> bool {
    !s.trim().is_empty()
        && s.len() <= 16_384
        && !s
            .chars()
            .any(|c| c == '\0' || (c.is_control() && c != '\n'))
}

/// Validate a catalog before displaying it or constructing an executable plan.
pub fn validate_catalog(catalog: &Catalog) -> Result<()> {
    ensure!(
        catalog.schema_version == SCHEMA_VERSION,
        "Unsupported catalog schema"
    );
    ensure!(text(&catalog.revision), "Invalid catalog revision");
    ensure!(
        !catalog.sources.is_empty() && catalog.sources.len() <= 128,
        "Invalid source count"
    );
    ensure!(
        !catalog.actions.is_empty() && catalog.actions.len() <= 256,
        "Invalid action count"
    );
    let mut sources = BTreeSet::new();
    for source in &catalog.sources {
        ensure!(
            identifier(&source.id) && sources.insert(&source.id),
            "Invalid or duplicate source ID: {}",
            source.id
        );
        ensure!(
            [
                &source.name,
                &source.revision,
                &source.license,
                &source.description
            ]
            .iter()
            .all(|s| text(s)),
            "Incomplete source: {}",
            source.id
        );
        let url = source
            .url
            .strip_prefix("https://")
            .context("Source URL must use HTTPS")?;
        let authority = url.split('/').next().unwrap_or_default();
        ensure!(
            !authority.is_empty()
                && authority.contains('.')
                && authority
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
                && !url
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control() || c == '\\'),
            "Invalid source URL: {}",
            source.id
        );
        ensure!(
            source.revision != "main"
                && source.revision != "master"
                && source.revision != "HEAD"
                && source.revision != "latest",
            "Source revision must be pinned: {}",
            source.id
        );
        ensure!(
            (source.revision.len() == 40 || source.revision.len() == 64)
                && source.revision.bytes().all(|b| b.is_ascii_hexdigit()),
            "Source revision must be an immutable full commit hash: {}",
            source.id
        );
    }
    let mut actions = BTreeSet::new();
    let mut addresses = BTreeSet::new();
    for action in &catalog.actions {
        ensure!(
            identifier(&action.id) && actions.insert(&action.id),
            "Invalid or duplicate action ID: {}",
            action.id
        );
        ensure!(
            [
                &action.title,
                &action.description,
                &action.category,
                &action.restart,
                &action.risk
            ]
            .iter()
            .all(|s| text(s)),
            "Incomplete action: {}",
            action.id
        );
        ensure!(
            !action.sources.is_empty() && action.sources.len() <= sources.len(),
            "Invalid sources for {}",
            action.id
        );
        let mut refs = BTreeSet::new();
        for id in &action.sources {
            ensure!(
                sources.contains(id) && refs.insert(id),
                "Unknown or duplicate source {} in {}",
                id,
                action.id
            );
        }
        ensure!(
            !action.evidence.is_empty() && action.evidence.len() <= 16,
            "Missing or excessive evidence for {}",
            action.id
        );
        for url in &action.evidence {
            let rest = url
                .strip_prefix("https://")
                .context("Evidence URL must use HTTPS")?;
            let host = rest.split('/').next().unwrap_or_default();
            ensure!(
                host.contains('.')
                    && host
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
                    && !url
                        .chars()
                        .any(|c| c.is_whitespace() || c.is_control() || c == '\\'),
                "Invalid evidence URL"
            );
        }
        ensure!(
            !action.targets.is_empty()
                && action
                    .targets
                    .iter()
                    .copied()
                    .collect::<BTreeSet<_>>()
                    .len()
                    == action.targets.len(),
            "Invalid targets for {}",
            action.id
        );
        ensure!(
            !action.changes.is_empty() && action.changes.len() <= 32,
            "Invalid change count for {}",
            action.id
        );
        for change in &action.changes {
            validate_address(change)?;
            ensure!(
                addresses.insert((
                    change.hive,
                    change.path.to_lowercase(),
                    change.name.to_lowercase()
                )),
                "Duplicate registry address in {}",
                action.id
            );
        }
    }
    Ok(())
}

pub fn validate_address(change: &RegistryChange) -> Result<()> {
    ensure!(
        change.path.len() <= 512 && change.path.starts_with("Software\\"),
        "Registry address must be beneath Software"
    );
    ensure!(
        change.path.split('\\').all(|part| !part.is_empty()
            && part != "."
            && part != ".."
            && part.trim() == part
            && !part.chars().any(|c| c.is_control() || c == '/')),
        "Invalid registry key path"
    );
    ensure!(
        !change.name.is_empty()
            && change.name.len() <= 128
            && !change
                .name
                .chars()
                .any(|c| c.is_control() || c == '\\' || c == '/'),
        "Invalid registry value name"
    );
    Ok(())
}

pub fn load_catalog() -> Result<Catalog> {
    let catalog = serde_json::from_str(include_str!("../../../data/catalog.json"))
        .context("Embedded catalog is invalid JSON")?;
    validate_catalog(&catalog)?;
    Ok(catalog)
}

pub fn build_plan(catalog: &Catalog, target: Target, selected: &BTreeSet<String>) -> Result<Plan> {
    validate_catalog(catalog)?;
    ensure!(
        selected.len() <= catalog.actions.len(),
        "Too many selected actions"
    );
    for id in selected {
        let action = catalog
            .actions
            .iter()
            .find(|a| &a.id == id)
            .with_context(|| format!("Unknown action: {id}"))?;
        ensure!(
            action.targets.contains(&target),
            "Action {id} does not support {}",
            target.label()
        );
    }
    Ok(Plan {
        schema_version: SCHEMA_VERSION,
        catalog_revision: catalog.revision.clone(),
        target,
        actions: catalog
            .actions
            .iter()
            .filter(|a| selected.contains(&a.id))
            .cloned()
            .collect(),
    })
}

/// Rebuild a plan from trusted actions; imported action payloads cannot introduce writes.
pub fn validate_plan(plan: &Plan) -> Result<Plan> {
    let mut ids = BTreeSet::new();
    for action in &plan.actions {
        ensure!(ids.insert(action.id.clone()), "Duplicate action in plan");
    }
    let expected = build_plan(&load_catalog()?, plan.target, &ids)?;
    ensure!(
        *plan == expected,
        "Plan differs from the embedded catalog; rebuild it before applying"
    );
    Ok(expected)
}

pub fn export_plan(plan: &Plan, path: &Path) -> Result<()> {
    validate_plan(plan)?;
    create_json(plan, path)
}
pub fn save_profile(profile: &Profile, path: &Path) -> Result<()> {
    ensure!(
        profile.schema_version == SCHEMA_VERSION,
        "Unsupported profile schema"
    );
    build_plan(&load_catalog()?, profile.target, &profile.selected)?;
    create_json(profile, path)
}
pub fn load_profile(path: &Path, catalog: &Catalog) -> Result<Profile> {
    let mut bytes = Vec::new();
    File::open(path)
        .with_context(|| format!("Open profile {}", path.display()))?
        .take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_INPUT_BYTES,
        "Profile exceeds 1 MiB limit"
    );
    let profile: Profile = serde_json::from_slice(&bytes).context("Invalid profile JSON")?;
    ensure!(
        profile.schema_version == SCHEMA_VERSION,
        "Unsupported profile schema"
    );
    build_plan(catalog, profile.target, &profile.selected)?;
    Ok(profile)
}

/// Publish a complete, flushed file with an atomic no-replace hard link.
/// A filesystem without hard-link support fails without overwriting the destination.
fn create_json(value: &impl Serialize, path: &Path) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        bail!("Export exceeds 1 MiB limit");
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let sequence = EXPORT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".ltsc-export-{}-{sequence}.tmp",
        std::process::id()
    ));
    let mut file = File::create_new(&temporary).context("Create export staging file")?;
    let result = (|| -> Result<()> {
        file.write_all(&bytes)?;
        file.write_all(b"\n")?;
        file.sync_all().context("Flush export before publication")?;
        fs::hard_link(&temporary, path).with_context(|| {
            format!(
                "Publish {} (existing files are never overwritten)",
                path.display()
            )
        })?;
        #[cfg(unix)]
        File::open(parent)?
            .sync_all()
            .context("Flush export directory")?;
        Ok(())
    })();
    drop(file);
    let cleanup = fs::remove_file(&temporary).context("Remove export staging file");
    result?;
    cleanup
}
static EXPORT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_selection_is_empty_plan() {
        let c = load_catalog().unwrap();
        assert!(
            build_plan(&c, Target::Win10Ltsc2021, &BTreeSet::new())
                .unwrap()
                .actions
                .is_empty()
        );
    }
    #[test]
    fn profile_rejects_duplicate_and_unknown_fields() {
        assert!(
            serde_json::from_str::<Profile>(
                r#"{"schema_version":1,"target":"win10_ltsc2021","selected":["x","x"]}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<Profile>(
                r#"{"schema_version":1,"target":"win10_ltsc2021","selected":[],"execute":true}"#
            )
            .is_err()
        );
    }
    #[test]
    fn invalid_catalog_and_unknown_selection_fail() {
        let c = load_catalog().unwrap();
        assert!(
            build_plan(
                &c,
                Target::Win10Ltsc2021,
                &BTreeSet::from(["not-real".into()])
            )
            .is_err()
        );
        let mut bad = c.clone();
        bad.actions.push(bad.actions[0].clone());
        assert!(validate_catalog(&bad).is_err());
        let mut bad = c.clone();
        bad.actions[0].changes[0].path = "Software\\..\\Bad".into();
        assert!(validate_catalog(&bad).is_err());
        let mut bad = c;
        bad.actions[0].sources.push("not-real".into());
        assert!(validate_catalog(&bad).is_err());
    }
    #[test]
    fn unsupported_target_and_plan_payload_fail() {
        let mut c = load_catalog().unwrap();
        c.actions[0].targets = vec![Target::Win10Ltsc2021];
        let ids = BTreeSet::from([c.actions[0].id.clone()]);
        assert!(build_plan(&c, Target::Win11Ltsc2024, &ids).is_err());
        let c = load_catalog().unwrap();
        let mut p = build_plan(
            &c,
            c.actions[0].targets[0],
            &BTreeSet::from([c.actions[0].id.clone()]),
        )
        .unwrap();
        p.actions[0].changes[0].value ^= 1;
        assert!(validate_plan(&p).is_err());
    }
    #[test]
    fn export_never_overwrites_and_profile_roundtrips() {
        let dir = std::env::temp_dir().join(format!(
            "ltsc-core-{}-{}",
            std::process::id(),
            EXPORT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("profile.json");
        let p = Profile {
            schema_version: 1,
            target: Target::Win10Ltsc2021,
            selected: BTreeSet::new(),
        };
        save_profile(&p, &path).unwrap();
        let original = fs::read(&path).unwrap();
        assert!(save_profile(&p, &path).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(load_profile(&path, &load_catalog().unwrap()).unwrap(), p);
        fs::write(&path, vec![b' '; MAX_INPUT_BYTES as usize + 1]).unwrap();
        assert!(load_profile(&path, &load_catalog().unwrap()).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
