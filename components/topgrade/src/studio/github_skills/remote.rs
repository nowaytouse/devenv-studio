use super::{Output, hex, ownership::directory};
use color_eyre::eyre::{ContextCompat, Result, ensure};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    fmt::Write,
};

fn github_json(
    run: &mut impl FnMut(&[&str]) -> Result<Output>,
    repo: &str,
    route: &str,
    allow_missing: bool,
) -> Result<Option<Value>> {
    let endpoint = format!("repos/{repo}{route}");
    let output = run(&["api", "--hostname", "github.com", "--include", &endpoint])?;
    let response = output.stdout.replace("\r\n", "\n");
    let (headers, body) = response
        .split_once("\n\n")
        .context("Missing GitHub API HTTP response")?;
    let mut status_line = headers.lines().next().unwrap_or_default().split_ascii_whitespace();
    let protocol = status_line.next().unwrap_or_default();
    ensure!(
        protocol.starts_with("HTTP/") && protocol.len() > 5,
        "Invalid GitHub API HTTP response"
    );
    let status = status_line.next();
    if status == Some("404") && !output.success && allow_missing {
        return Ok(None);
    }
    ensure!(
        output.success && status == Some("200"),
        "GitHub API request failed for {repo}"
    );
    let result: Value =
        serde_json::from_str(body).map_err(|_| color_eyre::eyre::eyre!("Invalid GitHub API response for {repo}"))?;
    ensure!(result.is_object(), "Invalid GitHub API response for {repo}");
    Ok(Some(result))
}

fn required_json(run: &mut impl FnMut(&[&str]) -> Result<Output>, repo: &str, route: &str) -> Result<Value> {
    github_json(run, repo, route, false)?.context("Missing required GitHub API response")
}

fn encode_ref(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            write!(encoded, "%{byte:02X}").expect("writing to a String");
        }
    }
    encoded
}

fn native_reference(run: &mut impl FnMut(&[&str]) -> Result<Output>, repo: &str) -> Result<String> {
    // Match gh discovery.ResolveRef(..., ""): a published release takes precedence.
    let release = github_json(run, repo, "/releases/latest", true)?;
    let tag = match release.as_ref().and_then(|release| release.get("tag_name")) {
        Some(tag) => tag.as_str().context("Invalid latest-release tag")?,
        None => "",
    };
    let branch;
    let (kind, reference) = if tag.is_empty() {
        branch = required_json(run, repo, "")?;
        (
            "heads",
            branch
                .get("default_branch")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .context("Missing default branch")?,
        )
    } else {
        ("tags", tag)
    };
    ensure!(
        reference.len() <= 1024,
        "GitHub reference exceeds its verification limit"
    );
    let reference = encode_ref(reference);
    let mut value = required_json(run, repo, &format!("/git/ref/{kind}/{reference}"))?;
    let mut object = value.get("object").context("Invalid remote Git reference")?;
    if kind == "tags" && object.get("type").and_then(Value::as_str) == Some("tag") {
        let sha = object
            .get("sha")
            .and_then(Value::as_str)
            .filter(|sha| hex(sha, 40))
            .context("Invalid annotated Git tag")?;
        value = required_json(run, repo, &format!("/git/tags/{sha}"))?;
        object = value.get("object").context("Invalid annotated Git tag object")?;
    }
    ensure!(
        object.get("type").and_then(Value::as_str) == Some("commit"),
        "Invalid remote Git commit reference"
    );
    object
        .get("sha")
        .and_then(Value::as_str)
        .filter(|sha| hex(sha, 40))
        .map(str::to_owned)
        .context("Invalid remote Git commit SHA")
}

#[derive(Deserialize)]
struct Node {
    path: String,
    #[serde(rename = "type")]
    kind: String,
    mode: Option<String>,
    sha: String,
}

#[derive(Deserialize)]
struct Tree {
    sha: String,
    truncated: bool,
    tree: Vec<Node>,
}

fn tree(
    run: &mut impl FnMut(&[&str]) -> Result<Output>,
    repo: &str,
    reference: &str,
    expected: Option<&str>,
) -> Result<(String, BTreeMap<String, Node>)> {
    let result: Tree = serde_json::from_value(required_json(run, repo, &format!("/git/trees/{reference}"))?)
        .map_err(|_| color_eyre::eyre::eyre!("Invalid GitHub tree for {repo}"))?;
    ensure!(
        !result.truncated && hex(&result.sha, 40) && expected.is_none_or(|sha| result.sha == sha),
        "Invalid or truncated remote GitHub tree for {repo}"
    );
    let mut entries = BTreeMap::new();
    for node in result.tree {
        ensure!(
            !node.path.is_empty()
                && !node.path.contains('/')
                && directory(&node.path)? == node.path
                && matches!(node.kind.as_str(), "blob" | "tree" | "commit")
                && hex(&node.sha, 40),
            "Invalid remote GitHub tree entry for {repo}"
        );
        ensure!(
            entries.insert(node.path.clone(), node).is_none(),
            "Duplicate remote GitHub tree entry for {repo}"
        );
    }
    Ok((result.sha, entries))
}

pub(super) fn expectations(
    run: &mut impl FnMut(&[&str]) -> Result<Output>,
    repo: &str,
    directories: &BTreeSet<&str>,
    missing: &mut Vec<String>,
) -> Result<(String, BTreeMap<String, String>)> {
    let commit = native_reference(run, repo)?;
    let (root_sha, root) = tree(run, repo, &commit, None)?;
    let mut cache = BTreeMap::from([(root_sha.clone(), root)]);
    let mut expected = BTreeMap::new();
    for value in directories {
        let path = directory(value)?;
        let mut parent = Some(root_sha.clone());
        if !path.is_empty() {
            for part in path.split('/') {
                let next = parent
                    .as_ref()
                    .and_then(|sha| cache[sha].get(part))
                    .filter(|node| node.kind == "tree" && node.mode.as_deref() == Some("040000"))
                    .map(|node| node.sha.clone());
                let Some(next) = next else {
                    parent = None;
                    break;
                };
                if let Entry::Vacant(slot) = cache.entry(next.clone()) {
                    let (_, children) = tree(run, repo, &next, Some(&next))?;
                    slot.insert(children);
                }
                parent = Some(next);
            }
        }
        if let Some(parent) = parent
            && cache[&parent]
                .get("SKILL.md")
                .is_some_and(|node| node.kind == "blob" && matches!(node.mode.as_deref(), Some("100644" | "100755")))
        {
            expected.insert(path.into(), parent);
        } else {
            let relative = if path.is_empty() {
                "SKILL.md".into()
            } else {
                format!("{path}/SKILL.md")
            };
            missing.push(format!(
                "Remote skill file is missing or not regular: {repo}/{relative}"
            ));
        }
    }
    Ok((commit, expected))
}
