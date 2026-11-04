use super::{NAMES, REGISTRY, bytes};
use crate::command::CommandExt as _;
use color_eyre::eyre::{Context, ContextCompat, Result, ensure};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::Path,
};

const LIMIT: u64 = 1024 * 1024;
const ENDPOINT: &str = "https://registry.npmjs.org/-/npm/v1/security/advisories/bulk";

pub(super) fn strict<T: serde::de::DeserializeOwned>(input: &[u8]) -> Result<T> {
    serde_json::from_value(super::super::strict_json::parse(input)?).wrap_err("Unexpected npm JSON schema")
}

#[derive(Deserialize)]
struct Metadata {
    name: String,
    version: String,
}

fn versions(root: &Path) -> Result<[String; 3]> {
    let mut result: [String; 3] = std::array::from_fn(|_| String::new());
    for (index, name) in ["npm", NAMES[0], NAMES[1]].into_iter().enumerate() {
        let path = if index == 0 {
            root.to_path_buf()
        } else {
            root.join("node_modules").join(name)
        };
        super::check_dir(&path, false)?;
        let metadata: Metadata = strict(&bytes(&path.join("package.json"), LIMIT)?)?;
        ensure!(
            metadata.name == name
                && semver::Version::parse(&metadata.version).is_ok()
                && nodejs_semver::Version::parse(&metadata.version).is_ok(),
            "Invalid installed package identity: {name}"
        );
        result[index] = metadata.version;
    }
    Ok(result)
}

#[derive(Deserialize)]
struct Advisory {
    id: u64,
    url: String,
    title: String,
    severity: String,
    vulnerable_versions: String,
}

fn check(input: &[u8], versions: &[String; 3]) -> Result<()> {
    let report: BTreeMap<String, Vec<Advisory>> = strict(input)?;
    let mut ids = BTreeSet::new();
    let mut matches = Vec::new();
    for (name, records) in report {
        let index = NAMES
            .iter()
            .position(|package| *package == name)
            .context("Unexpected advisory package")?;
        let version = &versions[index + 1];
        let parsed = nodejs_semver::Version::parse(version).wrap_err("Invalid installed package version")?;
        for record in records {
            let official = record
                .url
                .strip_prefix("https://github.com/advisories/GHSA-")
                .is_some_and(|id| {
                    !id.is_empty()
                        && id
                            .bytes()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
                });
            ensure!(
                record.id > 0
                    && ids.insert(record.id)
                    && official
                    && !record.title.trim().is_empty()
                    && matches!(record.severity.as_str(), "low" | "moderate" | "high" | "critical")
                    && !record.vulnerable_versions.trim().is_empty()
                    && record.vulnerable_versions.len() <= 1024,
                "Invalid advisory registry fields"
            );
            let range =
                nodejs_semver::Range::parse(&record.vulnerable_versions).wrap_err("Invalid npm advisory range")?;
            if range.satisfies(&parsed) {
                matches.push(format!("{name} {version} {}", record.url));
            }
        }
    }
    ensure!(
        matches.is_empty(),
        "Known vulnerable dependencies in an unfamiliar npm tree: {}",
        matches.join(", ")
    );
    Ok(())
}

pub(super) fn audit(root: &Path, network: &super::super::proxy::Route) -> Result<[String; 3]> {
    let versions = versions(root)?;
    let workspace = tempfile::tempdir()?;
    let directory = workspace.path().canonicalize()?;
    let reply = directory.join("response.json");
    let mut body = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(
        &mut body,
        &serde_json::json!({(NAMES[0]): [&versions[1]], (NAMES[1]): [&versions[2]]}),
    )?;
    body.flush()?;
    let request = format!("@{}", body.path().display());
    let mut command = super::tool_command(Path::new("/usr/bin/curl"))?;
    network.apply(&mut command);
    let output = command
        .args([
            "--disable",
            "--silent",
            "--show-error",
            "--fail",
            "--proto",
            "=https",
            "--max-time",
            "20",
            "--max-filesize",
            "1048576",
            "--header",
            "Content-Type: application/json",
            "--data-binary",
            &request,
            "--output",
        ])
        .arg(&reply)
        .args(["--write-out", "%{http_code}", "--", ENDPOINT])
        .output_checked_utf8()
        .wrap_err_with(|| format!("Query {REGISTRY} npm advisories"))?;
    ensure!(
        output.stdout == "200",
        "Unexpected npm advisory HTTP status or redirect"
    );
    check(&bytes(&reply, LIMIT)?, &versions)?;
    Ok(versions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_matching_npm_ranges_and_ambiguous_or_malformed_registry_results() -> Result<()> {
        let versions = ["12.2.1".into(), "6.29.0".into(), "10.7.5".into()];
        let record = |range: &str| {
            serde_json::json!({
                "undici": [{"id":123,"url":"https://github.com/advisories/GHSA-abcd-1234-efgh",
                    "title":"test","severity":"high","vulnerable_versions":range}]
            })
            .to_string()
        };
        check(record("<6.28.0 || 6.30.x").as_bytes(), &versions)?;
        assert!(check(record("6.28.0 - 6.29.0").as_bytes(), &versions).is_err());
        assert!(check(record("nonsense").as_bytes(), &versions).is_err());
        assert!(check(br#"{"undici":[],"undici":[]}"#, &versions).is_err());
        assert!(check(br#"{"other":[]}"#, &versions).is_err());
        Ok(())
    }
}
