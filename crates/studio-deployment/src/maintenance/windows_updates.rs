//! Terminal handoff for one unambiguous WinGet inventory identity.
use super::windows_inventory::Package;

#[derive(Debug, PartialEq, Eq)]
pub struct TerminalCommands {
    pub check: String,
    pub upgrade: String,
}

/// Build commands for review in Terminal. Inventory membership does not establish an available update.
pub fn commands(package: &Package) -> Option<TerminalCommands> {
    let id = &package.id;
    if !package.official_winget
        || !package.can_target
        || !crate::safe_package_id(id)
        || !id.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        || !id.contains('.')
        || id.split('.').any(str::is_empty)
    {
        return None;
    }
    let scope = format!("--id \"{id}\" --exact --source winget");
    Some(TerminalCommands {
        check: format!("winget list --upgrade-available {scope}"),
        upgrade: format!("winget upgrade {scope}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(id: &str) -> Package {
        Package {
            id: id.into(),
            source: "winget".into(),
            version: Some("8.7".into()),
            official_winget: true,
            can_target: true,
        }
    }

    #[test]
    fn handoff_only_targets_the_reviewed_id_and_source() {
        let result = commands(&package("Notepad++.Notepad++")).unwrap();
        assert_eq!(
            result.check,
            "winget list --upgrade-available --id \"Notepad++.Notepad++\" --exact --source winget"
        );
        assert_eq!(
            result.upgrade,
            "winget upgrade --id \"Notepad++.Notepad++\" --exact --source winget"
        );
    }

    #[test]
    fn uncertain_source_identity_and_command_tokens_are_ineligible() {
        let mut row = package("Git.Git");
        row.official_winget = false;
        assert!(commands(&row).is_none());
        row.official_winget = true;
        row.can_target = false;
        assert!(commands(&row).is_none());
        for id in [
            "",
            "--all.Foo",
            "Git.Git --all",
            "Git.Git|other",
            "Git.Git\"",
            "Git.Git`x",
            "Git..Git",
        ] {
            assert!(commands(&package(id)).is_none(), "accepted {id:?}");
        }
    }
}
