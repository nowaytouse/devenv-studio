#[cfg(any(windows, test))]
use anyhow::{Context, Result, ensure};
#[cfg(any(windows, test))]
use serde::Deserialize;
#[cfg(any(windows, test))]
use std::collections::{BTreeMap, BTreeSet};

#[cfg(any(windows, test))]
use super::{MAX_PACKAGES, Package};

#[cfg(any(windows, test))]
#[derive(Deserialize)]
struct Export {
    buckets: Vec<Bucket>,
    apps: Vec<App>,
    #[serde(default)]
    config: Option<serde_json::Value>,
}

#[cfg(any(windows, test))]
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Bucket {
    name: String,
    source: String,
}

#[cfg(any(windows, test))]
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct App {
    name: String,
    version: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    info: Option<String>,
}

#[cfg(any(windows, test))]
pub(crate) struct Inventory {
    pub packages: Vec<Package>,
    pub official_buckets: BTreeSet<String>,
    pub omitted: bool,
}

pub(crate) fn valid_identity(name: &str, version: &str) -> bool {
    valid_name(name) && valid_version(version)
}

pub(crate) fn valid_name(name: &str) -> bool {
    let Some((bucket, app)) = name.split_once('/') else {
        return false;
    };
    !app.contains('/') && matches!(bucket, "main" | "extras" | "versions") && safe_component(app)
}

fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.+".contains(&byte))
}

fn valid_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".,_+~-".contains(&byte))
}

#[cfg(any(windows, test))]
fn official_source(name: &str, source: &str) -> bool {
    let expected = match name {
        "main" => "https://github.com/scoopinstaller/main",
        "extras" => "https://github.com/scoopinstaller/extras",
        "versions" => "https://github.com/scoopinstaller/versions",
        _ => return false,
    };
    let mut normalized = source.trim().to_ascii_lowercase();
    while normalized.ends_with('/') {
        normalized.pop();
    }
    if let Some(without_git) = normalized.strip_suffix(".git") {
        normalized = without_git.to_owned();
    }
    normalized == expected
}

#[cfg(any(windows, test))]
pub(crate) fn parse_export(text: &str) -> Result<Inventory> {
    ensure!(
        text.len() <= 1024 * 1024 && !text.contains('\u{fffd}'),
        "Incomplete Scoop export capture"
    );
    let export: Export = serde_json::from_str(text).context("Invalid Scoop export JSON")?;
    ensure!(
        export.config.is_none(),
        "Scoop configuration export is not supported"
    );
    ensure!(
        export.apps.len() <= MAX_PACKAGES && export.buckets.len() <= 128,
        "Scoop export exceeds inventory limits"
    );

    let mut names = BTreeSet::new();
    let mut sources = BTreeMap::new();
    let mut official_buckets = BTreeSet::new();
    let mut omitted = false;
    for bucket in export.buckets {
        let name = bucket.name.to_ascii_lowercase();
        ensure!(safe_component(&name), "Invalid Scoop bucket name");
        ensure!(names.insert(name.clone()), "Duplicate Scoop bucket name");
        let official = official_source(&name, &bucket.source);
        if official {
            official_buckets.insert(name.clone());
        }
        sources.insert(name, official);
    }

    let mut packages = Vec::new();
    let mut identities = BTreeSet::new();
    for app in export.apps {
        let Some(source) = app.source.as_deref() else {
            omitted = true;
            continue;
        };
        let bucket = source.to_ascii_lowercase();
        let Some(true) = sources.get(&bucket) else {
            omitted = true;
            continue;
        };
        if app
            .info
            .as_deref()
            .is_some_and(|info| !info.trim().is_empty())
        {
            omitted = true;
            continue;
        }
        let name = format!("{bucket}/{}", app.name);
        if !valid_identity(&name, &app.version) {
            omitted = true;
            continue;
        }
        ensure!(
            identities.insert(name.to_ascii_lowercase()),
            "Duplicate Scoop package identity"
        );
        packages.push(Package {
            name,
            version: app.version,
        });
    }
    ensure!(
        packages.len() <= MAX_PACKAGES,
        "Scoop inventory exceeds package count limit"
    );
    Ok(Inventory {
        packages,
        official_buckets,
        omitted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn export(apps: &str, buckets: &str) -> String {
        format!(r#"{{"buckets":{buckets},"apps":{apps}}}"#)
    }

    #[test]
    fn inventory_accepts_registered_official_buckets_and_exact_versions() {
        let value = export(
            r#"[{"Name":"rustup","Version":"1.28.0","Source":"main","Info":""},{"Name":"nightly-tool","Version":"nightly","Source":"versions","Info":""}]"#,
            r#"[{"Name":"main","Source":"https://github.com/ScoopInstaller/Main.git","Updated":"2026-10-23","Manifests":100},{"Name":"versions","Source":"https://github.com/ScoopInstaller/Versions"}]"#,
        );
        let parsed = parse_export(&value).unwrap();
        assert_eq!(
            parsed
                .packages
                .iter()
                .map(|package| (package.name.as_str(), package.version.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("main/rustup", "1.28.0"),
                ("versions/nightly-tool", "nightly")
            ]
        );
        assert_eq!(
            parsed.official_buckets,
            BTreeSet::from(["main".into(), "versions".into()])
        );
        assert!(!parsed.omitted);
    }

    #[test]
    fn inventory_marks_unsupported_sources_and_package_metadata_as_omitted() {
        let value = export(
            r#"[{"Name":"local-tool","Version":"1.0","Source":"custom","Info":""},{"Name":"local-manifest","Version":"1.0","Source":"<auto-generated>","Info":""},{"Name":"admin-tool","Version":"2.0","Source":"main","Info":"Global install"},{"Name":"held-tool","Version":"3.0","Source":"main","Info":"Held package"}]"#,
            r#"[{"Name":"main","Source":"https://github.com/ScoopInstaller/Main"},{"Name":"custom","Source":"https://example.invalid/bucket"}]"#,
        );
        let parsed = parse_export(&value).unwrap();
        assert!(parsed.packages.is_empty());
        assert!(parsed.omitted);
        assert_eq!(parsed.official_buckets, BTreeSet::from(["main".into()]));
    }

    #[test]
    fn export_rejects_configuration_duplicates_and_unbounded_output() {
        assert!(parse_export(r#"{"buckets":[],"apps":[],"config":{}}"#).is_err());
        assert!(
            parse_export(
                r#"{"buckets":[{"Name":"main","Source":"https://github.com/ScoopInstaller/Main"},{"Name":"MAIN","Source":"https://github.com/ScoopInstaller/Main"}],"apps":[]}"#
            )
            .is_err()
        );
        assert!(parse_export(&" ".repeat(1024 * 1024 + 1)).is_err());
        assert!(!valid_identity("main/tool", "latest;Invoke-Expression"));
        assert!(!valid_identity("custom/tool", "1.0"));
        assert!(valid_identity("extras/tool.name", "nightly"));
    }
}
