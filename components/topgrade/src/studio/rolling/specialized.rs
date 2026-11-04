use std::{collections::BTreeMap, fs::File, io::Write, path::Path, sync::LazyLock, thread, time::Duration};

use color_eyre::eyre::{ContextCompat, Result, ensure};
use regex::Regex;

use super::{Status, files, plan::Specialized, process};

mod gradle;

pub(super) fn run(kind: Specialized, root: &Path, edge: bool, log: &mut File) -> Result<(Status, Vec<String>)> {
    match kind {
        Specialized::Zig => zig(root, edge, log),
        Specialized::Docker => docker(root, edge, log),
        Specialized::Gradle => gradle::run(root, edge, log),
    }
}

fn block(text: &str, open: usize) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut cursor = open;
    let mut depth = 0_u32;
    let mut quoted = false;
    while let Some(&byte) = bytes.get(cursor) {
        match byte {
            b'\\' if quoted => cursor += 1,
            b'"' => quoted = !quoted,
            b'{' if !quoted => depth += 1,
            b'}' if !quoted => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(&text[open + 1..cursor]);
                }
            }
            b'/' if !quoted && bytes.get(cursor + 1) == Some(&b'/') => {
                cursor += bytes[cursor..].iter().position(|&value| value == b'\n')?;
            }
            _ => {}
        }
        cursor += 1;
    }
    None
}

fn github_zig_url(value: &str) -> Option<String> {
    let path = if let Some(path) = value.strip_prefix("git+https://github.com/") {
        path.split(['?', '#']).next()?
    } else {
        value.strip_prefix("https://github.com/")?.split_once("/archive/")?.0
    };
    let (owner, repo) = path.split_once('/')?;
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    let safe = |part: &str| {
        !part.is_empty()
            && !matches!(part, "." | "..")
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    };
    (safe(owner) && safe(repo)).then(|| format!("git+https://github.com/{owner}/{repo}.git"))
}

fn zig_dependencies(text: &str) -> Result<(Vec<(String, String)>, usize)> {
    static DEPS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^[ \t]*\.dependencies\s*=\s*\.\{").unwrap());
    static ENTRY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?m)^[ \t]*\.(?:@"([^"\r\n]+)"|([A-Za-z_][A-Za-z0-9_]*))[ \t]*=[ \t]*\.\{"#).unwrap()
    });
    static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?m)^[ \t]*\.url\s*=\s*"([^"\r\n]+)""#).unwrap());
    let Some(outer) = DEPS.find(text) else {
        return Ok((Vec::new(), 0));
    };
    let body = block(text, outer.end() - 1).context("Unterminated Zig dependency block")?;
    let mut remaining = body;
    let mut updates = Vec::new();
    let mut unsupported = 0;
    while let Some(entry) = ENTRY.captures(remaining) {
        let start = entry.get(0).context("Missing Zig dependency entry")?.end();
        let name = entry
            .get(1)
            .or_else(|| entry.get(2))
            .context("Missing Zig dependency name")?
            .as_str();
        let declaration = block(remaining, start - 1).context("Unterminated Zig dependency entry")?;
        if let Some(url) = URL.captures(declaration) {
            ensure!(!name.contains(char::is_control), "Invalid Zig dependency name");
            if let Some(target) = github_zig_url(&url[1]) {
                updates.push((name.to_owned(), target));
            } else {
                unsupported += 1;
            }
        }
        remaining = &remaining[start + declaration.len() + 1..];
    }
    Ok((updates, unsupported))
}

fn zig(root: &Path, edge: bool, log: &mut File) -> Result<(Status, Vec<String>)> {
    if !edge {
        return Ok((
            Status::Skipped,
            vec!["Zig GitHub HEAD rolling is disabled by compatible policy".into()],
        ));
    }
    let manifest = files::read(&root.join("build.zig.zon"), 1024 * 1024)?;
    let (updates, unsupported) = zig_dependencies(std::str::from_utf8(&manifest.bytes)?)?;
    if unsupported > 0 {
        writeln!(
            log,
            "{unsupported} Zig dependency URLs have no supported GitHub HEAD form"
        )?;
    }
    if updates.is_empty() {
        return Ok((
            if unsupported > 0 {
                Status::Discovered
            } else {
                Status::Current
            },
            Vec::new(),
        ));
    }
    let Some(zig) = process::find_tool("zig")? else {
        return Ok((Status::Skipped, vec!["missing command: zig".into()]));
    };
    for (name, target) in updates {
        writeln!(log, "zig fetch --save={name} {target}")?;
        let mut command = process::command(&zig, root);
        command.args(["fetch", &format!("--save={name}"), &target]);
        let output = process::capture(command, process::UPDATE_TIMEOUT)?;
        if !output.status.success() {
            return Ok((
                Status::Failed,
                vec![format!("Zig fetch for {name} → {}", output.status)],
            ));
        }
    }
    Ok((Status::Current, Vec::new()))
}

fn image_ref(value: &str) -> Result<(String, &str)> {
    let base = value.split_once('@').map_or(value, |(base, _)| base);
    let (repo, tag) = match base.rsplit_once(':') {
        Some((repo, tag)) if !tag.contains('/') => (repo, tag),
        _ => (base, "latest"),
    };
    ensure!(
        !repo.is_empty()
            && repo.len() <= 255
            && repo.as_bytes()[0].is_ascii_alphanumeric()
            && repo
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b':' | b'/' | b'_' | b'-'))
            && repo.split('/').all(|part| !matches!(part, "" | "." | ".."))
            && !tag.is_empty()
            && tag.len() <= 128
            && tag
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')),
        "Unsupported Docker image reference"
    );
    let qualified = if let Some((first, _)) = repo.split_once('/') {
        if first.contains(['.', ':']) || first == "localhost" {
            repo.into()
        } else {
            format!("docker.io/{repo}")
        }
    } else {
        format!("docker.io/library/{repo}")
    };
    Ok((qualified, tag))
}

fn numeric_tag(tag: &str) -> Option<(&str, Vec<u32>, &str)> {
    static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(v?)([0-9]+(?:\.[0-9]+)*)(.*)$").unwrap());
    let captures = TAG.captures(tag)?;
    let parts = captures
        .get(2)?
        .as_str()
        .split('.')
        .map(str::parse)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    Some((captures.get(1)?.as_str(), parts, captures.get(3)?.as_str()))
}

fn pick_tag<'a>(current: &'a str, tags: impl Iterator<Item = &'a str>) -> &'a str {
    let Some((prefix, version, flavor)) = numeric_tag(current) else {
        return current;
    };
    tags.filter_map(|tag| {
        let (pfx, ver, suffix) = numeric_tag(tag)?;
        (pfx == prefix && suffix == flavor && ver > version).then_some((ver, tag))
    })
    .max_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(right.1)))
    .map_or(current, |(_, tag)| tag)
}

#[derive(Clone, Copy)]
enum RegistryFailure {
    Missing,
    Retryable,
    Other,
}

fn crane(executable: &Path, root: &Path, args: &[&str]) -> Result<std::result::Result<String, RegistryFailure>> {
    for attempt in 0..3 {
        let mut command = process::command(executable, root);
        command.args(args);
        let output = process::capture(command, process::QUERY_TIMEOUT)?;
        if output.status.success() {
            return Ok(Ok(output.stdout));
        }
        if output.contains_any(&["manifest_unknown", "manifest unknown", "404 not found"]) {
            return Ok(Err(RegistryFailure::Missing));
        }
        if output.contains_any(&["too many requests", "retry-after", "429"]) {
            return Ok(Err(RegistryFailure::Retryable));
        }
        if !output.contains_any(&[
            "unexpected eof",
            ": eof",
            "connection reset by peer",
            "tls handshake timeout",
            "i/o timeout",
            "context deadline exceeded",
            "temporary failure",
            "502 bad gateway",
            "503 service unavailable",
            "504 gateway timeout",
        ]) {
            return Ok(Err(RegistryFailure::Other));
        }
        if attempt < 2 {
            thread::sleep(Duration::from_secs(1));
        }
    }
    Ok(Err(RegistryFailure::Retryable))
}

fn resolve_image(
    executable: &Path,
    root: &Path,
    value: &str,
    edge: bool,
) -> Result<std::result::Result<String, RegistryFailure>> {
    let (repo, current) = image_ref(value)?;
    let tags;
    let tag = if edge && numeric_tag(current).is_some() {
        tags = match crane(executable, root, &["ls", &repo])? {
            Ok(tags) => tags,
            Err(status) => return Ok(Err(status)),
        };
        pick_tag(current, tags.lines().map(str::trim))
    } else {
        current
    };
    let selected = format!("{repo}:{tag}");
    let digest = match crane(executable, root, &["digest", &selected])? {
        Ok(digest) => digest,
        Err(status) => return Ok(Err(status)),
    };
    let digest = digest.trim();
    ensure!(
        digest
            .strip_prefix("sha256:")
            .is_some_and(|value| super::super::is_hex(value, 64)),
        "Registry returned an invalid SHA-256 manifest digest"
    );
    Ok(Ok(format!("{selected}@{digest}")))
}

fn rewrite_docker(
    source: &str,
    mut resolve: impl FnMut(&str) -> Result<std::result::Result<String, RegistryFailure>>,
) -> Result<(String, Vec<RegistryFailure>)> {
    static FROM: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^([ \t]*FROM[ \t]+(?:--platform=[^ \t]+[ \t]+)?)([^ \t\r\n]+)(.*)$").unwrap()
    });
    static ALIAS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bAS[ \t]+([^ \t\r\n]+)").unwrap());
    let mut aliases = std::collections::BTreeSet::new();
    let mut result = String::with_capacity(source.len());
    let mut failures = Vec::new();
    for line in source.split_inclusive('\n') {
        let body = line.trim_end_matches('\n');
        let Some(from) = FROM.captures(body) else {
            result.push_str(line);
            continue;
        };
        let reference = &from[2];
        let lower = reference.to_ascii_lowercase();
        if lower == "scratch" || aliases.contains(&lower) {
            result.push_str(line);
        } else if reference.contains(['$', '\\']) || reference.starts_with('-') {
            failures.push(RegistryFailure::Other);
            result.push_str(line);
        } else {
            match resolve(reference)? {
                Ok(new) => {
                    result.push_str(&from[1]);
                    result.push_str(&new);
                    result.push_str(&from[3]);
                    if line.ends_with('\n') {
                        result.push('\n');
                    }
                }
                Err(failure) => {
                    failures.push(failure);
                    result.push_str(line);
                }
            }
        }
        if let Some(alias) = ALIAS.captures(&from[3]) {
            aliases.insert(alias[1].to_ascii_lowercase());
        }
    }
    Ok((result, failures))
}

fn docker(root: &Path, edge: bool, log: &mut File) -> Result<(Status, Vec<String>)> {
    let Some(crane_executable) = process::find_tool("crane")? else {
        return Ok((Status::Skipped, vec!["missing command: crane".into()]));
    };
    let mut resolutions = BTreeMap::new();
    let mut failures = Vec::new();
    let mut edits = Vec::new();
    for path in files::walk(root)?.into_iter().filter(|path| files::dockerfile(path)) {
        let original = files::read(&path, 1024 * 1024)?;
        let (updated, problems) = rewrite_docker(std::str::from_utf8(&original.bytes)?, |reference| {
            if let Some(cached) = resolutions.get(reference) {
                return Ok(<std::result::Result<String, RegistryFailure>>::clone(cached));
            }
            let result = resolve_image(&crane_executable, root, reference, edge)?;
            resolutions.insert(reference.to_owned(), result.clone());
            Ok(result)
        })?;
        failures.extend(problems);
        edits.push((path, original, updated));
    }
    let status = if failures.iter().any(|failure| matches!(failure, RegistryFailure::Other)) {
        Status::Failed
    } else if failures
        .iter()
        .any(|failure| matches!(failure, RegistryFailure::Retryable))
    {
        Status::Retryable
    } else if failures
        .iter()
        .any(|failure| matches!(failure, RegistryFailure::Missing))
    {
        Status::UpstreamMissing
    } else {
        Status::Current
    };
    for (path, original, updated) in edits {
        if updated.as_bytes() != original.bytes {
            files::rewrite(&path, &original, updated.as_bytes())?;
            writeln!(log, "Pinned image references: {}", path.strip_prefix(root)?.display())?;
        }
    }
    let notes = if failures.is_empty() {
        Vec::new()
    } else {
        vec![format!("{} Docker FROM references did not resolve", failures.len())]
    };
    Ok((status, notes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_supported_zig_urls_are_rewritten() -> Result<()> {
        let source = r#".{
    .dependencies = .{
        .@"reviewed-tool" = .{
            // Ignore braces in comments: }
            .url = "https://github.com/example/zig-tool/archive/abc.tar.gz",
        },
        .other = .{
            .url = "https://github.com.example.net/repo/file",
        },
    },
}"#;
        let (plans, skipped) = zig_dependencies(source)?;
        assert_eq!(
            plans,
            [(
                "reviewed-tool".into(),
                "git+https://github.com/example/zig-tool.git".into()
            )]
        );
        assert_eq!(skipped, 1);
        assert!(github_zig_url("git+https://github.com/example/repo/subpath").is_none());
        Ok(())
    }

    #[test]
    fn docker_preserves_channels_and_stage_aliases() -> Result<()> {
        let source = "FROM --platform=linux/arm64 rust:1.90-bookworm AS build\r\nFROM build AS copy\nFROM scratch\n";
        let (text, failed) = rewrite_docker(source, |reference| {
            assert_eq!(reference, "rust:1.90-bookworm");
            Ok(Ok("docker.io/library/rust:1.91-bookworm@sha256:example".into()))
        })?;
        assert!(failed.is_empty());
        assert_eq!(
            text,
            "FROM --platform=linux/arm64 docker.io/library/rust:1.91-bookworm@sha256:example AS build\r\nFROM build AS copy\nFROM scratch\n"
        );
        assert_eq!(
            pick_tag(
                "1.90-bookworm",
                ["1.92-slim", "1.91-bookworm", "1.89-bookworm"].into_iter()
            ),
            "1.91-bookworm"
        );
        assert_eq!(pick_tag("3.0", ["2.9"].into_iter()), "3.0");
        Ok(())
    }
}
