//! WinGet-matched installed software from a local, non-interactive export.
use anyhow::Result;
use std::sync::{Arc, atomic::AtomicBool};

#[cfg(any(windows, test))]
use anyhow::ensure;
#[cfg(any(windows, test))]
use serde::Deserialize;
#[cfg(any(windows, test))]
use std::collections::BTreeMap;

/// A command example. The app supplies a unique temporary output path.
pub const COMMAND: &str =
    "winget export --output winget.json --include-versions --disable-interactivity";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub id: String,
    pub source: String,
    pub version: Option<String>,
    pub official_winget: bool,
    /// The exported identity uniquely maps to an official community-source package.
    pub can_target: bool,
}

/// Only packages that WinGet matched and exported are represented.
#[derive(Debug, Clone)]
pub struct Inventory {
    pub packages: Vec<Package>,
    pub command: &'static str,
}

#[cfg(any(windows, test))]
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Export {
    #[serde(rename = "$schema")]
    schema: String,
    sources: Vec<Source>,
}

#[cfg(any(windows, test))]
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Source {
    source_details: SourceDetails,
    packages: Vec<ExportedPackage>,
    #[serde(flatten)]
    additional: BTreeMap<String, serde_json::Value>,
}

#[cfg(any(windows, test))]
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SourceDetails {
    name: String,
    identifier: String,
    argument: String,
    #[serde(rename = "Type")]
    kind: String,
    #[serde(flatten)]
    additional: BTreeMap<String, serde_json::Value>,
}

#[cfg(any(windows, test))]
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ExportedPackage {
    package_identifier: String,
    version: Option<String>,
    #[serde(flatten)]
    additional: BTreeMap<String, serde_json::Value>,
}

#[cfg(any(windows, test))]
fn parse(bytes: &[u8]) -> Result<Inventory> {
    ensure!(
        bytes.len() as u64 <= studio_core::MAX_INPUT_BYTES,
        "WinGet export exceeds the inventory size limit"
    );
    let export: Export = serde_json::from_slice(bytes.trim_prefix(b"\xef\xbb\xbf"))
        .map_err(|_| anyhow::anyhow!("WinGet returned an invalid package export"))?;
    ensure!(
        export.schema == "https://aka.ms/winget-packages.schema.2.0.json",
        "Only WinGet package export schema 2.0 is supported"
    );

    let mut packages = Vec::new();
    let mut counts = BTreeMap::new();
    for source in export.sources {
        ensure!(
            packages.len() + source.packages.len() <= crate::backup::MAX_PACKAGES,
            "WinGet export exceeds the package count limit"
        );
        let details = source.source_details;
        let official_winget = source.additional.is_empty()
            && details.additional.is_empty()
            && details.name == "winget"
            && details.identifier == "Microsoft.Winget.Source_8wekyb3d8bbwe"
            && details.argument == "https://cdn.winget.microsoft.com/cache"
            && details.kind == "Microsoft.PreIndexed.Package";
        let source_name = if !details.name.is_empty()
            && details.name.len() <= 64
            && details
                .name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
        {
            details.name
        } else {
            "Other source".into()
        };

        for package in source.packages {
            let id = package.package_identifier;
            ensure!(
                id.len() <= 100
                    && id.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
                    && id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"-_.+".contains(&byte)),
                "WinGet export contains an unsupported package identity"
            );
            *counts.entry(id.to_ascii_lowercase()).or_insert(0usize) += 1;
            packages.push(Package {
                id,
                source: source_name.clone(),
                version: package
                    .version
                    .filter(|version| crate::backup::valid_version(version)),
                official_winget,
                can_target: official_winget && package.additional.is_empty(),
            });
        }
    }
    for package in &mut packages {
        package.can_target &= counts[&package.id.to_ascii_lowercase()] == 1;
    }
    packages.sort_by(|a, b| (&a.id, &a.source, &a.version).cmp(&(&b.id, &b.source, &b.version)));
    Ok(Inventory {
        packages,
        command: COMMAND,
    })
}

/// Read the WinGet-matched subset of installed applications on the current Windows host.
pub fn read(cancel: Arc<AtomicBool>) -> Result<Inventory> {
    #[cfg(windows)]
    return native::read(&cancel);
    #[cfg(not(windows))]
    {
        let _ = cancel;
        anyhow::bail!("WinGet inventory is available on Windows only")
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use crate::process::{Invocation, run_bounded};
    use anyhow::Context;
    use std::{
        fs::{self, OpenOptions},
        io::{self, Read},
        os::windows::fs::{MetadataExt, OpenOptionsExt},
        path::Path,
        sync::atomic::{AtomicU64, Ordering},
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    const REPARSE_POINT: u32 = 0x400;
    const OPEN_REPARSE_POINT: u32 = 0x00200000;
    const SHARE_READ: u32 = 1;

    fn read_export(path: &Path) -> Result<Vec<u8>> {
        let file = OpenOptions::new()
            .read(true)
            .share_mode(SHARE_READ)
            .custom_flags(OPEN_REPARSE_POINT)
            .open(path)
            .context("Open WinGet's temporary export")?;
        let metadata = file.metadata()?;
        ensure!(
            metadata.is_file()
                && metadata.file_attributes() & REPARSE_POINT == 0
                && metadata.len() <= studio_core::MAX_INPUT_BYTES,
            "WinGet export is not a bounded regular file"
        );
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(studio_core::MAX_INPUT_BYTES + 1)
            .read_to_end(&mut bytes)
            .context("Read WinGet's temporary export")?;
        ensure!(
            bytes.len() as u64 == metadata.len(),
            "WinGet export changed during the read"
        );
        Ok(bytes)
    }

    fn cleanup(directory: &Path, export: &Path) -> Result<()> {
        let metadata = fs::symlink_metadata(directory)?;
        ensure!(
            metadata.is_dir() && metadata.file_attributes() & REPARSE_POINT == 0,
            "WinGet temporary directory changed before cleanup"
        );
        match fs::remove_file(export) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("Remove temporary WinGet export"),
        }
        fs::remove_dir(directory).context("Remove WinGet temporary directory")
    }

    pub(super) fn read(cancel: &Arc<AtomicBool>) -> Result<Inventory> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        ensure!(
            !cancel.load(Ordering::Acquire),
            "WinGet inventory was cancelled"
        );
        let program = crate::windows::winget_path()
            .context("Microsoft App Installer WinGet is unavailable")?;
        let local = crate::windows::local_app_data()?.canonicalize()?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let directory = local.join(format!(
            "devenv-studio-winget-{}-{now}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).context("Create a private WinGet export directory")?;
        let export = directory.join("packages.json");
        let result = (|| {
            let invocation = Invocation {
                program,
                args: vec![
                    "export".into(),
                    "--output".into(),
                    export
                        .to_str()
                        .context("Invalid WinGet temporary path encoding")?
                        .into(),
                    "--include-versions".into(),
                    "--disable-interactivity".into(),
                ],
                vscode_node: false,
                npm_node: false,
            };
            let output = run_bounded(&invocation, cancel, Duration::from_secs(60))
                .context("Could not read the local WinGet inventory")?;
            ensure!(
                !output.cancelled && !cancel.load(Ordering::Acquire),
                "WinGet inventory was cancelled"
            );
            ensure!(!output.timed_out, "WinGet inventory timed out");
            ensure!(
                output.code == 0,
                "WinGet could not export packages (exit 0x{:08X}). Check App Installer and configured sources in Terminal.",
                output.code
            );
            ensure!(
                export.canonicalize()? == export,
                "WinGet export path changed"
            );
            let inventory = parse(&read_export(&export)?)?;
            ensure!(
                !cancel.load(Ordering::Acquire),
                "WinGet inventory was cancelled"
            );
            Ok(inventory)
        })();
        match (result, cleanup(&directory, &export)) {
            (result, Ok(())) => result,
            (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(cleanup)) => Err(error.context(format!("{cleanup:#}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &[u8] = include_bytes!("../../../../examples/winget-export.json");

    #[test]
    fn exact_source_identity_is_required_for_package_commands() -> Result<()> {
        let inventory = parse(EXAMPLE)?;
        assert_eq!(inventory.packages.len(), 6);
        let lookup = |id| {
            inventory
                .packages
                .iter()
                .find(|package| package.id == id)
                .unwrap()
        };
        let git = lookup("Git.Git");
        assert_eq!(git.source, "winget");
        assert_eq!(git.version.as_deref(), Some("2.50.1"));
        assert!(git.official_winget && git.can_target);
        assert_eq!(lookup("Microsoft.VisualStudioCode").version, None);
        assert!(!lookup("Python.Python.3.14").can_target);
        assert!(!lookup("Mozilla.Firefox").official_winget);
        assert!(!lookup("Mozilla.Firefox").can_target);
        Ok(())
    }

    #[test]
    fn accepts_win_get_export_with_utf8_bom() -> Result<()> {
        let mut bytes = b"\xef\xbb\xbf".to_vec();
        bytes.extend_from_slice(EXAMPLE);
        assert_eq!(parse(&bytes)?.packages, parse(EXAMPLE)?.packages);
        Ok(())
    }

    #[test]
    fn ambiguous_ids_and_untrusted_source_metadata_are_display_only() -> Result<()> {
        let input = std::str::from_utf8(EXAMPLE)?
            .replace("Example.UnreviewedApp", "git.git")
            .replace("example-untrusted", "winget");
        let inventory = parse(input.as_bytes())?;
        let mut matches = inventory
            .packages
            .iter()
            .filter(|row| row.id.eq_ignore_ascii_case("Git.Git"));
        assert_eq!(matches.clone().count(), 2);
        assert!(matches.all(|row| row.official_winget && !row.can_target));
        assert!(
            inventory
                .packages
                .iter()
                .any(|row| row.source == "winget" && !row.official_winget)
        );
        Ok(())
    }

    #[test]
    fn malformed_exports_fail_without_interpreting_data_as_arguments() -> Result<()> {
        let input = std::str::from_utf8(EXAMPLE)?;
        for broken in [
            input.replace("schema.2.0.json", "schema.1.0.json"),
            input.replace("Git.Git", "--include-unknown"),
            input.replace(
                r#""PackageIdentifier": "Git.Git""#,
                r#""PackageIdentifier": "Git.Git", "PackageIdentifier": "Other.App""#,
            ),
            input.replace(r#""Version": "2.50.1""#, r#""Version": 2501"#),
        ] {
            assert!(parse(broken.as_bytes()).is_err());
        }
        Ok(())
    }
}
