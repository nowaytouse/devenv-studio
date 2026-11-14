use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Write,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use color_eyre::eyre::{Context, Result, ensure};
use regex::{Captures, Regex};
use serde::Deserialize;

use super::{Status, files, process};

mod proxy;

const MAX: u64 = 8 * 1024 * 1024;
const CHANNEL_URL: &str = "https://services.gradle.org/versions/nightly";
const INIT: &str = r#"
import com.github.benmanes.gradle.versions.VersionsSettingsPlugin

initscript {
    repositories { gradlePluginPortal() }
    dependencies { classpath 'io.github.ben-manes:gradle-versions-plugin:0.67.0' }
}

settingsEvaluated { settings ->
    if (!settings.pluginManager.hasPlugin("io.github.ben-manes.versions.settings")) {
        settings.pluginManager.apply(VersionsSettingsPlugin)
    }
}
"#;

#[derive(Deserialize)]
struct Report {
    outdated: Dependencies<Dependency>,
    unresolved: Dependencies<BTreeMap<String, serde_json::Value>>,
    skipped: Configurations,
    gradle: Channels,
}

#[derive(Deserialize)]
struct Dependencies<T> {
    count: usize,
    dependencies: Vec<T>,
}

#[derive(Deserialize)]
struct Configurations {
    count: usize,
    configurations: Vec<BTreeMap<String, serde_json::Value>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Channels {
    enabled: bool,
    running: Option<Channel>,
    current: Option<Channel>,
    release_candidate: Option<Channel>,
    nightly: Option<Channel>,
}

#[derive(Deserialize)]
struct Channel {
    #[serde(rename = "isFailure")]
    failure: bool,
}

#[derive(Deserialize)]
struct Dependency {
    group: String,
    name: String,
    version: String,
    available: Available,
    #[serde(default)]
    configurations: Vec<String>,
    #[serde(default)]
    contributed: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Available {
    pre_release: Option<String>,
    integration: Option<String>,
    milestone: Option<String>,
    release: Option<String>,
}

impl Report {
    fn check(&self) -> Result<()> {
        ensure!(
            self.outdated.count == self.outdated.dependencies.len()
                && self.unresolved.count == self.unresolved.dependencies.len()
                && self.skipped.count == self.skipped.configurations.len(),
            "Gradle report section counts do not match their contents"
        );
        ensure!(
            self.unresolved.count == 0
                && self.skipped.count == 0
                && (!self.gradle.enabled || self.gradle.nightly.is_some())
                && [
                    self.gradle.running.as_ref(),
                    self.gradle.current.as_ref(),
                    self.gradle.release_candidate.as_ref(),
                    self.gradle.nightly.as_ref(),
                ]
                .into_iter()
                .flatten()
                .all(|channel| !channel.failure),
            "Gradle report contains unverified dependencies, configurations, or channels"
        );
        Ok(())
    }
}

impl Dependency {
    fn target(&self, edge: bool) -> Option<&str> {
        let target = if edge {
            [
                &self.available.pre_release,
                &self.available.integration,
                &self.available.milestone,
                &self.available.release,
            ]
            .into_iter()
            .find_map(Option::as_deref)?
        } else {
            let target = self.available.release.as_deref()?;
            let major = |value: &str| value.split('.').next().and_then(|part| part.parse::<u32>().ok());
            if major(&self.version).is_none() || major(&self.version) != major(target) {
                return None;
            }
            target
        };
        (target != self.version).then_some(target)
    }

    fn plugin_managed(&self) -> bool {
        self.contributed
            || (!self.configurations.is_empty()
                && self.configurations.iter().all(|config| {
                    matches!(
                        config.as_str(),
                        "kotlinCompilerClasspath"
                            | "kotlinBuildToolsApiClasspath"
                            | "kotlinAbiValidationCompatClasspath"
                            | "kotlinKlibCommonizerClasspath"
                            | "kotlinBouncyCastleConfiguration"
                            | "androidLintTool"
                    ) || config.starts_with("unified-test-platform-")
                        || (config.starts_with("kotlinCompilerPluginClasspath")
                            && config != "kotlinCompilerPluginClasspath")
                }))
    }
}

fn executable(root: &Path) -> Result<Option<PathBuf>> {
    for directory in root.ancestors() {
        let wrapper = directory.join("gradlew");
        if super::super::super::exists(&wrapper)? {
            let metadata = super::super::super::open_file(&wrapper, false, 1024 * 1024)?.metadata()?;
            let properties = files::read(&directory.join("gradle/wrapper/gradle-wrapper.properties"), 128 * 1024)?;
            if metadata.mode() & 0o111 != 0 && wrapper_distribution(std::str::from_utf8(&properties.bytes)?)? {
                return Ok(Some(wrapper));
            }
            eprintln!("Using installed Gradle for an unpinned or non-executable project wrapper");
            break;
        }
        if directory.join(".git").exists() {
            break;
        }
    }
    let nightly = super::super::super::account_home()?.join(".local/bin/gradle-nightly");
    if super::super::super::exists(&nightly)? {
        let metadata = super::super::super::open_file(&nightly, false, 1024 * 1024)?.metadata()?;
        ensure!(
            metadata.mode() & 0o111 != 0,
            "Managed Gradle Nightly launcher is not executable"
        );
        return Ok(Some(nightly));
    }
    process::find_tool("gradle")
}

fn wrapper_distribution(properties: &str) -> Result<bool> {
    let urls = properties
        .lines()
        .filter_map(|line| line.strip_prefix("distributionUrl="))
        .collect::<Vec<_>>();
    let checksums = properties
        .lines()
        .filter_map(|line| line.strip_prefix("distributionSha256Sum="))
        .collect::<Vec<_>>();
    let [url] = urls.as_slice() else {
        color_eyre::eyre::bail!("Ambiguous Gradle wrapper distribution URL");
    };
    ensure!(
        Regex::new(r"^https://(?:services|downloads)\.gradle\.org/distributions(?:-snapshots)?/gradle-[0-9][A-Za-z0-9.+-]*-(?:bin|all)\.zip$")?
            .is_match(&url.replace("\\:", ":")),
        "Gradle wrapper distribution is outside the approved official URLs"
    );
    match checksums.as_slice() {
        [] => Ok(false),
        [digest] if super::super::super::is_hex(digest, 64) => Ok(true),
        _ => color_eyre::eyre::bail!("Invalid or duplicate Gradle wrapper checksum"),
    }
}

struct Source {
    original: files::Content,
    text: String,
}

fn replace(text: &mut String, expression: &str, version: &str) -> Result<bool> {
    let expression = Regex::new(expression)?;
    let updated = expression.replace_all(text, |matched: &Captures<'_>| {
        format!("{}{version}{}", &matched[1], &matched[2])
    });
    if updated == *text {
        return Ok(false);
    }
    *text = updated.into_owned();
    Ok(true)
}

fn token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+'))
}

fn rewrite_dependency(sources: &mut BTreeMap<PathBuf, Source>, dependency: &Dependency, version: &str) -> Result<bool> {
    let Dependency {
        group,
        name,
        version: current,
        ..
    } = dependency;
    ensure!(
        [group.as_str(), name.as_str(), current.as_str(), version]
            .into_iter()
            .all(token),
        "Invalid coordinate or version in Gradle report"
    );
    let coordinate = format!("{group}:{name}");
    let direct = regex::escape(&format!("{coordinate}:"));
    let old = regex::escape(current);
    let plugin = (name == &format!("{group}.gradle.plugin")).then(|| regex::escape(group));
    let variable = Regex::new(&format!(r#"{direct}\$\{{?([A-Za-z_][A-Za-z0-9_.-]*)\}}?"#))?;
    let mut variables = BTreeSet::new();
    let mut changed = false;
    for source in sources.values() {
        variables.extend(variable.captures_iter(&source.text).map(|match_| match_[1].to_owned()));
    }
    for (path, source) in sources.iter_mut() {
        let text = &mut source.text;
        changed |= replace(text, &format!(r#"(["']{direct}){old}(["'])"#), version)?;
        if let Some(plugin) = &plugin {
            changed |= replace(
                text,
                &format!(r#"(\bid\s*\(?\s*["']{plugin}["']\s*\)?\s*\.?version\s*\(?\s*["']){old}(["'])"#),
                version,
            )?;
        }
        if files::name(path) == "libs.versions.toml" {
            changed |= rewrite_catalog(text, dependency, version)?;
        }
        for variable in &variables {
            let key = regex::escape(variable);
            if files::name(path) == "gradle.properties" {
                changed |= replace(
                    text,
                    &format!(r"(?m)^([ \t]*{key}[ \t]*[=:][ \t]*){old}([ \t]*(?:\r)?$)"),
                    version,
                )?;
            } else if files::name(path) != "libs.versions.toml" {
                changed |= replace(
                    text,
                    &format!(r#"(?m)^([ \t]*(?:(?:val|var|def)[ \t]+)?{key}[ \t]*=[ \t]*["']){old}(["'])"#),
                    version,
                )?;
            }
        }
    }
    Ok(changed)
}

fn rewrite_catalog(text: &mut String, dependency: &Dependency, version: &str) -> Result<bool> {
    let catalog: toml::Table = toml::from_str(text).wrap_err("Invalid Gradle version catalog")?;
    let coordinate = format!("{}:{}", dependency.group, dependency.name);
    let plugin =
        (dependency.name == format!("{}.gradle.plugin", dependency.group)).then_some(dependency.group.as_str());
    let mut relevant = Vec::new();
    let mut reference_counts = BTreeMap::<String, usize>::new();
    for section in ["libraries", "plugins"] {
        if let Some(entries) = catalog.get(section).and_then(toml::Value::as_table) {
            for (alias, entry) in entries {
                let reference = entry
                    .get("version")
                    .and_then(|version| version.get("ref"))
                    .and_then(toml::Value::as_str);
                if let Some(reference) = reference {
                    *reference_counts.entry(reference.into()).or_default() += 1;
                }
                let matches = if section == "plugins" {
                    plugin.is_some_and(|id| entry.get("id").and_then(toml::Value::as_str) == Some(id))
                } else {
                    entry.get("module").and_then(toml::Value::as_str) == Some(coordinate.as_str())
                        || (entry.get("group").and_then(toml::Value::as_str) == Some(&dependency.group)
                            && entry.get("name").and_then(toml::Value::as_str) == Some(&dependency.name))
                };
                if matches {
                    relevant.push((alias, reference));
                }
            }
        }
    }
    let old = regex::escape(&dependency.version);
    let mut changed = false;
    for (alias, reference) in relevant {
        if let Some(reference) = reference {
            // A shared catalog key may also control a dependency absent from the report.
            if reference_counts.get(reference) == Some(&1)
                && catalog
                    .get("versions")
                    .and_then(|versions| versions.get(reference))
                    .and_then(toml::Value::as_str)
                    == Some(&dependency.version)
            {
                let key = regex::escape(reference);
                changed |= replace(
                    text,
                    &format!(r#"(?m)^([ \t]*(?:{key}|"{key}"|'{key}')[ \t]*=[ \t]*["']){old}(["'][^\n]*$)"#),
                    version,
                )?;
            }
        } else {
            let alias = regex::escape(alias);
            changed |= replace(
                text,
                &format!(
                    r#"(?m)^([ \t]*(?:{alias}|"{alias}"|'{alias}')[ \t]*=[ \t]*\{{[^}}\n]*\bversion[ \t]*=[ \t]*["']){old}(["'][^}}\n]*\}}[^\n]*$)"#
                ),
                version,
            )?;
        }
    }
    if changed {
        let _: toml::Table = toml::from_str(text).wrap_err("Updated Gradle version catalog is invalid")?;
    }
    Ok(changed)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Nightly {
    version: String,
    download_url: String,
    checksum: String,
}

fn wrapper_plan(root: &Path) -> Result<Option<(PathBuf, Source)>> {
    if !root.join("gradlew").exists() {
        return Ok(None);
    }
    let path = root.join("gradle/wrapper/gradle-wrapper.properties");
    let original = files::read(&path, 128 * 1024)?;
    let old = std::str::from_utf8(&original.bytes)?;
    wrapper_distribution(old)?;
    let Some(curl) = process::find_tool("curl")? else {
        color_eyre::eyre::bail!("Missing curl for Gradle Nightly metadata");
    };
    let mut command = process::command(&curl, root);
    command.args([
        "--disable",
        "--silent",
        "--show-error",
        "--fail",
        "--max-time",
        "20",
        "--proto",
        "=https",
        "--max-filesize",
        "1048576",
        "--",
        CHANNEL_URL,
    ]);
    let output = process::capture(command, process::QUERY_TIMEOUT)?;
    ensure!(output.status.success(), "Gradle Nightly metadata lookup failed");
    let nightly: Nightly = serde_json::from_str(&output.stdout)?;
    let url = format!(
        "https://services.gradle.org/distributions-snapshots/gradle-{}-bin.zip",
        nightly.version
    );
    ensure!(
        Regex::new(r"^[0-9]+(?:\.[0-9]+){1,2}-[0-9]{14}\+[0-9]{4}$")?.is_match(&nightly.version)
            && nightly.download_url == url
            && super::super::super::is_hex(&nightly.checksum, 64),
        "Invalid Gradle Nightly download identity"
    );
    let mut text = String::new();
    let mut urls = 0;
    let mut checksums = 0;
    let newline = if old.contains("\r\n") { "\r\n" } else { "\n" };
    for line in old.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        if body.starts_with("distributionUrl=") {
            urls += 1;
            text.push_str(&format!("distributionUrl={}{newline}", url.replace(':', "\\:")));
        } else if body.starts_with("distributionSha256Sum=") {
            checksums += 1;
            text.push_str(&format!("distributionSha256Sum={}{newline}", nightly.checksum));
        } else {
            text.push_str(line);
        }
    }
    ensure!(
        urls == 1 && checksums <= 1,
        "Ambiguous Gradle wrapper distribution properties"
    );
    if checksums == 0 {
        if !text.ends_with('\n') {
            text.push_str(newline);
        }
        text.push_str(&format!("distributionSha256Sum={}{newline}", nightly.checksum));
    }
    Ok(Some((path, Source { original, text })))
}

pub(super) fn run(root: &Path, edge: bool, log: &mut File) -> Result<(Status, Vec<String>)> {
    let Some(executable) = executable(root)? else {
        return Ok((Status::Skipped, vec!["missing Gradle executable".into()]));
    };
    let temp = tempfile::tempdir()?;
    let private = temp.path().canonicalize()?;
    let init = private.join("rolling.init.gradle");
    let report_dir = private.join("report");
    let mut init_file = super::super::super::create_file(&init)?;
    init_file.write_all(INIT.as_bytes())?;
    std::fs::create_dir(&report_dir)?;
    let mut command = process::command(&executable, root);
    let home = super::super::super::account_home()?;
    let route = proxy::gradle_args(&command, root, &home)?;
    command
        .args(route)
        .arg("--no-daemon")
        .arg("--project-cache-dir")
        .arg(private.join("project-cache"))
        .arg("--init-script")
        .arg(&init)
        .args([
            "dependencyUpdates",
            "--revision",
            if edge { "integration" } else { "release" },
            "--no-reject-out-of-bounds",
            "--output-formatter",
            "json",
            "--output-dir",
        ])
        .arg(&report_dir)
        .args(["--report-file-name", "report", "--gradle-release-channel", "nightly"]);
    let output = process::capture(command, process::UPDATE_TIMEOUT)?;
    ensure!(
        output.status.success(),
        "Gradle dependency discovery failed ({})",
        output.status
    );
    let paths = files::walk(&report_dir)?;
    let reports = paths
        .iter()
        .filter(|path| files::name(path) == "report.json")
        .collect::<Vec<_>>();
    let [report] = reports.as_slice() else {
        color_eyre::eyre::bail!("Gradle must produce exactly one dependency report");
    };
    let report: Report = serde_json::from_slice(&files::read(report, MAX)?.bytes)?;
    report.check()?;
    let mut sources = BTreeMap::new();
    for path in files::tracked(root)?
        .into_iter()
        .filter(|path| files::gradle_source(path))
    {
        let original = files::read(&path, MAX)?;
        let text = std::str::from_utf8(&original.bytes)?.to_owned();
        sources.insert(path, Source { original, text });
    }
    let mut unmapped = 0;
    for dependency in &report.outdated.dependencies {
        if let Some(target) = dependency.target(edge)
            && !rewrite_dependency(&mut sources, dependency, target)?
            && !dependency.plugin_managed()
        {
            unmapped += 1;
        }
    }
    if edge && let Some((path, wrapper)) = wrapper_plan(root)? {
        sources.insert(path, wrapper);
    }
    for (path, source) in sources {
        if source.text.as_bytes() != source.original.bytes {
            files::rewrite(&path, &source.original, source.text.as_bytes())?;
            writeln!(
                log,
                "Updated Gradle dependency declaration: {}",
                path.strip_prefix(root)?.display()
            )?;
        }
    }
    let notes = (unmapped > 0)
        .then(|| format!("{unmapped} Gradle updates had no unambiguous declaration"))
        .into_iter()
        .collect();
    Ok((
        if unmapped > 0 {
            Status::Discovered
        } else {
            Status::Current
        },
        notes,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapper_bootstrap_requires_a_pinned_official_distribution() -> Result<()> {
        let source = "distributionUrl=https\\://services.gradle.org/distributions/gradle-9.8.2-bin.zip\n";
        assert!(!wrapper_distribution(source)?);
        assert!(wrapper_distribution(&format!(
            "{source}distributionSha256Sum={}\n",
            "a".repeat(64)
        ))?);
        assert!(wrapper_distribution(&source.replace("services.gradle.org", "example.org")).is_err());
        assert!(wrapper_distribution(&source.repeat(2)).is_err());
        Ok(())
    }

    #[test]
    fn incomplete_report_prevents_version_changes() -> Result<()> {
        let report: Report = serde_json::from_str(
            r#"{
          "outdated":{"count":0,"dependencies":[]},
          "unresolved":{"count":1,"dependencies":[{"group":"test"}]},
          "skipped":{"count":0,"configurations":[]},
          "gradle":{"enabled":true,"nightly":{"isFailure":false}}
        }"#,
        )?;
        assert!(report.check().is_err());
        Ok(())
    }

    #[test]
    fn catalog_changes_exact_owned_declarations_but_not_shared_version_refs() -> Result<()> {
        let dep: Dependency = serde_json::from_str(
            r#"{
          "group":"example","name":"one","version":"1.2","available":{"release":"1.3"}
        }"#,
        )?;
        let mut catalog = r#"[versions]
shared = "1.2"
[libraries]
one = { module = "example:one", version.ref = "shared" }
two = { module = "example:two", version.ref = "shared" }
direct = { module = "example:one", version = "1.2" }
"#
        .to_owned();
        assert!(rewrite_catalog(&mut catalog, &dep, "1.3")?);
        assert!(catalog.contains(r#"shared = "1.2""#));
        assert!(catalog.contains(r#"direct = { module = "example:one", version = "1.3" }"#));
        assert!(!rewrite_catalog(&mut catalog, &dep, "1.3")?);
        Ok(())
    }
}
