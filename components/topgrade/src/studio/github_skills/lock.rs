use super::{
    Item, hex,
    ownership::{directory, repository},
};
use color_eyre::eyre::{Context, Result, ensure};
use serde::{
    Deserialize, Deserializer,
    de::{Error, MapAccess, Visitor},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    io::Read,
    marker::PhantomData,
    path::Path,
};

const LOCK_LIMIT: u64 = 8 * 1024 * 1024;

fn present_value<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

fn unique_map<'de, D, T>(deserializer: D) -> Result<BTreeMap<String, T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Unique<T>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>> Visitor<'de> for Unique<T> {
        type Value = BTreeMap<String, T>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an object without duplicate keys")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
            let mut value = BTreeMap::new();
            while let Some((key, entry)) = access.next_entry()? {
                if value.insert(key, entry).is_some() {
                    return Err(A::Error::custom("duplicate shared lock key"));
                }
            }
            Ok(value)
        }
    }
    deserializer.deserialize_map(Unique(PhantomData))
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Entry {
    source: String,
    source_type: String,
    source_url: String,
    skill_folder_hash: String,
    installed_at: String,
    updated_at: String,
    #[serde(default, deserialize_with = "present_value")]
    skill_path: Option<String>,
    #[serde(default, deserialize_with = "present_value")]
    pinned_ref: Option<String>,
    #[serde(default, deserialize_with = "present_value")]
    plugin_name: Option<String>,
}

impl Entry {
    fn identity(&self, item: &Item) -> bool {
        self.source_type == "github"
            && self.source.eq_ignore_ascii_case(&item.repo)
            && repository(&self.source_url).is_ok_and(|source| source == item.repo)
            && self.skill_path.as_deref() == Some(&item.skill_path())
            && self.pinned_ref.is_none()
    }

    fn updated(&self, item: &Item) -> bool {
        self.identity(item) && self.skill_folder_hash == item.expected
    }
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Lock {
    version: u8,
    #[serde(deserialize_with = "unique_map")]
    skills: BTreeMap<String, Entry>,
    #[serde(default, deserialize_with = "unique_map")]
    dismissed: BTreeMap<String, bool>,
    #[serde(default, deserialize_with = "present_value")]
    last_selected_agents: Option<Vec<String>>,
}

impl Default for Lock {
    fn default() -> Self {
        Self {
            version: 3,
            skills: BTreeMap::new(),
            dismissed: BTreeMap::new(),
            last_selected_agents: None,
        }
    }
}

impl Lock {
    fn parse(raw: &[u8]) -> Result<Self> {
        let lock: Self = serde_json::from_slice(raw).map_err(|_| {
            color_eyre::eyre::eyre!("Shared skill-lock schema cannot safely round-trip through native GH")
        })?;
        ensure!(
            lock.version == 3
                && lock
                    .last_selected_agents
                    .iter()
                    .flatten()
                    .all(|agent| !agent.is_empty())
                && lock
                    .skills
                    .values()
                    .all(|entry| [&entry.skill_path, &entry.pinned_ref, &entry.plugin_name]
                        .into_iter()
                        .all(|field| field.as_ref().is_none_or(|value| !value.is_empty()))),
            "Shared skill-lock schema cannot safely round-trip through native GH"
        );
        Ok(lock)
    }

    pub(super) fn has_entry(&self, name: &str) -> bool {
        self.skills.keys().any(|key| key.rsplit('/').next() == Some(name))
    }

    pub(super) fn npx_registered(&self, name: &str) -> bool {
        self.skills.get(name).is_some_and(|entry| {
            entry.source_type == "github"
                && repository(&entry.source_url).is_ok_and(|repo| repo.eq_ignore_ascii_case(&entry.source))
                && entry
                    .skill_path
                    .as_ref()
                    .is_some_and(|path| (path == "SKILL.md" || path.ends_with("/SKILL.md")) && directory(path).is_ok())
                && hex(&entry.skill_folder_hash, 40)
        })
    }

    pub(super) fn protected(&self) -> bool {
        self.last_selected_agents.is_some() || self.skills.values().any(|entry| entry.plugin_name.is_some())
    }

    pub(super) fn collisions(&self, selected: &[Item]) -> Result<()> {
        let mut proposed = BTreeMap::new();
        for item in selected.iter().filter(|item| item.changed()) {
            let identity = (&item.repo, directory(&item.meta.directory)?);
            for key in item.keys() {
                ensure!(
                    self.skills.get(key).is_none_or(|entry| entry.identity(item))
                        && proposed.insert(key, identity).is_none_or(|old| old == identity),
                    "Planned GH update conflicts with shared lock ownership"
                );
            }
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Snapshot {
    pub lock: Lock,
    raw: Option<Vec<u8>>,
}

impl Snapshot {
    pub(super) fn read(home: &Path) -> Result<Self> {
        let path = home.join(".agents/.skill-lock.json");
        if !super::super::exists(&path)? {
            return Ok(Self {
                lock: Lock::default(),
                raw: None,
            });
        }
        let file = super::super::open_file(&path, false, LOCK_LIMIT).context("Read the shared skill lock")?;
        let mut raw = Vec::new();
        file.take(LOCK_LIMIT + 1).read_to_end(&mut raw)?;
        ensure!(
            raw.len() as u64 <= LOCK_LIMIT,
            "Shared skill lock exceeds its verification limit"
        );
        Ok(Self {
            lock: Lock::parse(&raw)?,
            raw: Some(raw),
        })
    }

    pub(super) fn verify_after(&self, after: &Self, group: &[&Item], successful: bool) -> Result<()> {
        ensure!(
            self.raw.is_none() || after.raw.is_some(),
            "Native GH removed the shared skill lock"
        );
        let (old, new) = (&self.lock, &after.lock);
        ensure!(
            old.version == new.version
                && old.dismissed == new.dismissed
                && old.last_selected_agents == new.last_selected_agents,
            "Native GH changed shared lock version or user preferences"
        );

        let keys = old.skills.keys().chain(new.skills.keys()).collect::<BTreeSet<_>>();
        for key in keys {
            let (previous, current) = (old.skills.get(key), new.skills.get(key));
            if previous != current {
                ensure!(
                    group.iter().any(|item| item.keys().contains(&key.as_str())
                        && current.is_some_and(|entry| entry.updated(item))
                        && previous.is_none_or(|entry| entry.identity(item))),
                    "Native GH clobbered an unrelated or mismatched shared lock entry"
                );
            }
        }
        if successful {
            for item in group.iter().filter(|item| item.changed()) {
                ensure!(
                    item.keys()
                        .iter()
                        .any(|key| new.skills.get(*key).is_some_and(|entry| entry.updated(item))),
                    "Native GH did not record its planned update in the shared lock"
                );
            }
        }
        Ok(())
    }
}
