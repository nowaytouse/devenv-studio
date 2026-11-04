//! Installed Homebrew metadata from the fast, local list command.
use anyhow::Result;
use std::sync::{Arc, atomic::AtomicBool};

#[cfg(target_os = "macos")]
use anyhow::Context;
#[cfg(any(target_os = "macos", test))]
use anyhow::ensure;
#[cfg(any(target_os = "macos", test))]
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Formula,
    Cask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub kind: Kind,
    pub name: String,
    pub versions: Vec<String>,
    pub linked_version: Option<String>,
    pub opt_version: Option<String>,
    pub pinned_version: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Inventory {
    pub packages: Vec<Package>,
    pub command: &'static str,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Deserialize)]
struct List {
    formulae: Vec<Formula>,
    casks: Vec<Cask>,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Deserialize)]
struct Formula {
    name: String,
    versions: Vec<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    linked_version: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    optlinked_version: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pinned_version: Option<String>,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Deserialize)]
struct Cask {
    token: String,
    versions: Vec<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pinned_version: Option<String>,
}

#[cfg(any(target_os = "macos", test))]
fn parse(text: &str, command: &'static str) -> Result<Inventory> {
    let list: List = serde_json::from_str(text).map_err(|_| {
        anyhow::anyhow!("Homebrew returned an unsupported installed-package response")
    })?;
    let mut packages: Vec<_> = list
        .formulae
        .into_iter()
        .map(|formula| Package {
            kind: Kind::Formula,
            name: formula.name,
            versions: formula.versions,
            linked_version: formula.linked_version,
            opt_version: formula.optlinked_version,
            pinned_version: formula.pinned_version,
        })
        .chain(list.casks.into_iter().map(|cask| Package {
            kind: Kind::Cask,
            name: cask.token,
            versions: cask.versions,
            linked_version: None,
            opt_version: None,
            pinned_version: cask.pinned_version,
        }))
        .collect();
    for package in &mut packages {
        let kind = match package.kind {
            Kind::Formula => "brew_formula",
            Kind::Cask => "brew_cask",
        };
        ensure!(
            crate::backup::valid_name(kind, &package.name),
            "Homebrew returned an invalid package name"
        );
        ensure!(
            !package.versions.is_empty()
                && package.versions.iter().all(|version| {
                    !version.is_empty()
                        && version.len() <= 256
                        && version.bytes().all(|byte| byte.is_ascii_graphic())
                }),
            "Homebrew returned an invalid installed version"
        );
        package.versions.sort_unstable();
        ensure!(
            !package.versions.windows(2).any(|pair| pair[0] == pair[1]),
            "Homebrew returned duplicate installed versions"
        );
        ensure!(
            [
                &package.linked_version,
                &package.opt_version,
                &package.pinned_version
            ]
            .into_iter()
            .flatten()
            .all(|version| package.versions.binary_search(version).is_ok()),
            "Homebrew link or pin metadata does not match the installed versions"
        );
    }
    packages.sort_unstable_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
    ensure!(
        !packages
            .windows(2)
            .any(|pair| pair[0].kind == pair[1].kind && pair[0].name == pair[1].name),
        "Homebrew returned duplicate installed package entries"
    );
    Ok(Inventory { packages, command })
}

/// Inspect the installed Formula and Cask records without refreshing package metadata.
pub fn read(cancel: Arc<AtomicBool>) -> Result<Inventory> {
    #[cfg(target_os = "macos")]
    {
        use crate::process::{Invocation, run_brew_inventory};
        use std::path::Path;

        let program = crate::backup::homebrew_program()
            .context("Homebrew or its local runtime was not found in a supported location")?;
        let command = if program == Path::new("/opt/homebrew/bin/brew") {
            "HOMEBREW_NO_AUTO_UPDATE=1 /opt/homebrew/bin/brew list --versions --json"
        } else {
            "HOMEBREW_NO_AUTO_UPDATE=1 /usr/local/bin/brew list --versions --json"
        };
        let invocation = Invocation {
            program,
            args: ["list", "--versions", "--json"].map(str::to_owned).into(),
            vscode_node: false,
            npm_node: false,
        };
        let output = run_brew_inventory(&invocation, &cancel)
            .context("Could not read the local Homebrew inventory")?;
        ensure!(!output.cancelled, "Homebrew inventory was cancelled");
        ensure!(!output.timed_out, "Homebrew inventory timed out");
        ensure!(
            output.code == 0,
            "Homebrew could not list installed packages (exit {}). This view requires support for `brew list --versions --json` and jq.",
            output.code
        );
        parse(&output.stdout, command)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = cancel;
        anyhow::bail!("Homebrew inventory is available on macOS only")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INPUT: &str = r#"{
        "formulae": [
            {"name":"llvm","versions":["HEAD-123abcd","23.1.4"],"linked_version":null,
             "optlinked_version":"23.1.4","pinned_version":"23.1.4"},
            {"name":"same-name","versions":["1.2.3"],"linked_version":"1.2.3",
             "optlinked_version":"1.2.3","pinned_version":null}
        ],
        "casks": [{"token":"same-name","versions":["latest","1.0,123"],"pinned_version":"1.0,123"}]
    }"#;

    #[test]
    fn preserves_distinct_packages_and_actual_link_pin_metadata() -> Result<()> {
        let inventory = parse(INPUT, "brew command")?;
        assert_eq!(inventory.command, "brew command");
        assert_eq!(inventory.packages.len(), 3);
        let llvm = &inventory.packages[0];
        assert_eq!(llvm.kind, Kind::Formula);
        assert_eq!(llvm.versions, ["23.1.4", "HEAD-123abcd"]);
        assert_eq!(llvm.linked_version, None);
        assert_eq!(llvm.opt_version.as_deref(), Some("23.1.4"));
        assert_eq!(llvm.pinned_version.as_deref(), Some("23.1.4"));
        assert_eq!(inventory.packages[1].name, inventory.packages[2].name);
        let cask = &inventory.packages[2];
        assert_eq!(cask.kind, Kind::Cask);
        assert_eq!(cask.versions, ["1.0,123", "latest"]);
        assert_eq!(cask.linked_version, None);
        assert_eq!(cask.pinned_version.as_deref(), Some("1.0,123"));
        Ok(())
    }

    #[test]
    fn rejects_incomplete_or_conflicting_response() {
        for input in [
            r#"{"formulae":[]}"#.to_owned(),
            INPUT.replace(r#""linked_version":null,"#, ""),
            INPUT.replace(r#""pinned_version":"23.1.4""#, r#""pinned_version":"999""#),
            INPUT.replace(r#""name":"same-name""#, r#""name":"llvm""#),
            INPUT.replace("same-name", r#"unsafe\u001b[31m"#),
            INPUT.replace("HEAD-123abcd", r#"version\u202e"#),
        ] {
            assert!(parse(&input, "").is_err(), "Accepted invalid inventory");
        }
    }
}
