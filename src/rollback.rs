use crate::platform::RegistryHive;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RollbackAction {
    RegistryDword {
        hive: RegistryHive,
        path: String,
        name: String,
        previous: Option<u32>,
    },
    Command {
        label: String,
        program: String,
        args: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackJournal {
    pub schema_version: u32,
    pub created_at: String,
    pub profile_version: String,
    pub completed: bool,
    #[serde(default)]
    pub restored: bool,
    pub actions: Vec<RollbackAction>,
    #[serde(skip)]
    path: PathBuf,
}

impl RollbackJournal {
    pub fn create(profile_version: &str) -> anyhow::Result<Self> {
        let directory = journal_directory();
        std::fs::create_dir_all(&directory)?;
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S-%3f");
        let path = directory.join(format!("rollback-{stamp}.json"));
        let journal = Self {
            schema_version: 1,
            created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            profile_version: profile_version.to_string(),
            completed: false,
            restored: false,
            actions: Vec::new(),
            path,
        };
        journal.save()?;
        Ok(journal)
    }

    pub fn load_latest() -> anyhow::Result<Self> {
        let directory = journal_directory();
        let mut paths = std::fs::read_dir(&directory)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("rollback-") && name.ends_with(".json"))
            })
            .collect::<Vec<_>>();
        paths.sort();
        while let Some(path) = paths.pop() {
            let journal = Self::load(&path)?;
            if !journal.restored && !journal.actions.is_empty() {
                return Ok(journal);
            }
        }
        anyhow::bail!("没有包含可恢复变更的回滚账本")
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let mut journal: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        if journal.schema_version != 1 {
            anyhow::bail!("不支持的回滚账本版本：{}", journal.schema_version);
        }
        journal.path = path.to_path_buf();
        Ok(journal)
    }

    pub fn record(&mut self, action: RollbackAction) -> anyhow::Result<bool> {
        let already_recorded = self
            .actions
            .iter()
            .any(|existing| match (existing, &action) {
                (
                    RollbackAction::RegistryDword {
                        hive: left_hive,
                        path: left_path,
                        name: left_name,
                        ..
                    },
                    RollbackAction::RegistryDword {
                        hive: right_hive,
                        path: right_path,
                        name: right_name,
                        ..
                    },
                ) => {
                    left_hive == right_hive
                        && left_path.eq_ignore_ascii_case(right_path)
                        && left_name.eq_ignore_ascii_case(right_name)
                }
                _ => existing == &action,
            });
        if already_recorded {
            return Ok(false);
        }
        self.actions.push(action);
        if let Err(error) = self.save() {
            self.actions.pop();
            return Err(error);
        }
        Ok(true)
    }

    pub fn finish(&mut self) -> anyhow::Result<()> {
        let previous = self.completed;
        self.completed = true;
        if let Err(error) = self.save() {
            self.completed = previous;
            return Err(error);
        }
        Ok(())
    }

    pub fn mark_restored(&mut self) -> anyhow::Result<()> {
        let previous = self.restored;
        self.restored = true;
        if let Err(error) = self.save() {
            self.restored = previous;
            return Err(error);
        }
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn save(&self) -> anyhow::Result<()> {
        crate::utils::write_atomic(&self.path, &serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }
}

pub fn journal_directory() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("LTSCWorkspace")
        .join("rollback")
}

#[cfg(test)]
mod tests {
    use super::{RollbackAction, RollbackJournal};
    use crate::platform::RegistryHive;

    #[test]
    fn failed_journal_save_does_not_commit_in_memory_state() {
        let root = std::env::temp_dir().join(format!(
            "ltsc-journal-failure-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("journal.json");
        std::fs::create_dir(&path).unwrap();
        let mut journal = RollbackJournal {
            schema_version: 1,
            created_at: "test".into(),
            profile_version: "test".into(),
            completed: false,
            restored: false,
            actions: Vec::new(),
            path: path.clone(),
        };
        let action = RollbackAction::RegistryDword {
            hive: RegistryHive::CurrentUser,
            path: "Software\\Test".into(),
            name: "Value".into(),
            previous: Some(1),
        };
        assert!(journal.record(action.clone()).is_err());
        assert!(journal.actions.is_empty());
        assert!(journal.finish().is_err());
        assert!(!journal.completed);
        assert!(journal.mark_restored().is_err());
        assert!(!journal.restored);
        std::fs::remove_dir(&path).unwrap();
        assert!(journal.record(action.clone()).unwrap());
        assert_eq!(RollbackJournal::load(&path).unwrap().actions, [action]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn journal_keeps_only_the_first_value_for_each_registry_key() {
        let path = std::env::temp_dir().join(format!(
            "ltsc-rollback-test-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut journal = RollbackJournal {
            schema_version: 1,
            created_at: "test".into(),
            profile_version: "test".into(),
            completed: false,
            restored: false,
            actions: Vec::new(),
            path: path.clone(),
        };
        journal.save().unwrap();
        let first = RollbackAction::RegistryDword {
            hive: RegistryHive::CurrentUser,
            path: "Software\\Test".into(),
            name: "Value".into(),
            previous: None,
        };
        let later = RollbackAction::RegistryDword {
            hive: RegistryHive::CurrentUser,
            path: "software\\test".into(),
            name: "value".into(),
            previous: Some(99),
        };
        assert!(journal.record(first.clone()).unwrap());
        assert!(!journal.record(later).unwrap());
        journal.finish().unwrap();

        let loaded = RollbackJournal::load(&path).unwrap();
        assert!(loaded.completed);
        assert_eq!(loaded.actions, [first]);
        journal.mark_restored().unwrap();
        assert!(RollbackJournal::load(&path).unwrap().restored);
        let _ = std::fs::remove_file(path);
    }
}
