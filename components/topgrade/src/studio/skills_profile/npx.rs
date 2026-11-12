use super::*;
use crate::studio::{check_file, github_skills::ownership, is_hex};
use chrono::{DateTime, Utc};
use nix::libc::{O_NOFOLLOW, O_NONBLOCK};
use regex::Regex;
use serde_json::json;
use std::{
    fs::OpenOptions,
    io::{BufRead, BufReader, Seek, SeekFrom},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    sync::LazyLock,
};

const SOURCE: &str = "mattpocock/skills";
const SOURCE_URL: &str = "https://github.com/mattpocock/skills";
const FIELDS: [&str; 4] = ["source", "sourceType", "sourceUrl", "skillPath"];
const ROOTS: [&str; 4] = [".agents/skills", ".codex/skills", ".claude/skills", ".cursor/skills"];
const SKILLS_PACKAGE: &str = "skills@1.7.6";
const RETIRED_DISCOVERY: &str = "discovering-gcp-data-assets";
const DISCOVERY_SOURCE: &str = "GoogleCloudPlatform/data-agent-kit-plugin";
const DISCOVERY_URL: &str = "https://github.com/GoogleCloudPlatform/data-agent-kit-plugin.git";
const DISCOVERY_TREE: &str = "db4886d8941a99c86ac44b2fe1ad427763c6c7f5";
const DISCOVERY_SHA256: &str = "558a0e0738a9d9448463193371400a3a82c6e17d24b6915cd35477f5329d3bd4";
const DISCOVERY_MARKER: &str = "Verified upstream retirement of discovering-gcp-data-assets";

static ANSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").expect("static ANSI pattern"));
static FAILURE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)✗|✘|^\s*Error:|Failed to (?:check|update)\b|appear to have been deleted upstream")
        .expect("static failure pattern")
});
static SUCCESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)All global skills are up to date|Updated [0-9]+ skill\(s\)|No global skills (?:to check|tracked in lock file)|^Skill source checks finished with warnings: [1-9][0-9]* source\(s\) skipped$")
        .expect("static success pattern")
});

fn exact_segment(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|c| c.is_ascii_alphanumeric() || b"_-.".contains(&c))
        && !matches!(value, "." | "..")
}

fn timestamp(record: &Value) -> Result<DateTime<chrono::FixedOffset>> {
    Ok(DateTime::parse_from_rfc3339(
        record["updatedAt"]
            .as_str()
            .context("Invalid skill lock updatedAt timestamp")?,
    )?)
}

fn present_ref(record: &Value) -> bool {
    ["ref", "pinnedRef"]
        .iter()
        .any(|name| !record[name].is_null() && record[name].as_str() != Some(""))
}

fn reviewed_discovery_lock(record: &Value) -> bool {
    record["source"] == DISCOVERY_SOURCE
        && record["sourceType"] == "github"
        && record["sourceUrl"] == DISCOVERY_URL
        && record["skillPath"] == "skills/discovering-gcp-data-assets/SKILL.md"
        && record["skillFolderHash"] == DISCOVERY_TREE
        && !present_ref(record)
}

fn discovery_paths(home: &Path, record: &Value, reviewed_digest: &str) -> Result<Option<Vec<(PathBuf, bool)>>> {
    if !reviewed_discovery_lock(record) {
        return Ok(None);
    }
    let relative = Path::new(ROOTS[0]).join(RETIRED_DISCOVERY);
    let canonical = home.join(&relative);
    let mut paths = Vec::new();
    for root in &ROOTS[1..] {
        let path = home.join(root).join(RETIRED_DISCOVERY);
        if exists(&path)? && fs::symlink_metadata(&path)?.is_symlink() {
            let target = fs::read_link(&path)?;
            if target == canonical || target == Path::new("../..").join(&relative) {
                directory(home, Path::new(root), false)?;
                paths.push((path, false));
            }
        }
    }
    if exists(&canonical)? {
        let local = directory(home, &relative, false)?;
        let mut files = fs::read_dir(&local)?;
        // An empty or absent canonical directory can be left by an interrupted retirement.
        if let Some(file) = files.next().transpose()? {
            let skill = file.path();
            if file.file_name() != "SKILL.md"
                || files.next().transpose()?.is_some()
                || digest(&bytes(&skill, DATA_LIMIT)?) != reviewed_digest
            {
                return Ok(None);
            }
            paths.push((skill, false));
        }
        paths.push((local, true));
    }
    Ok(Some(paths))
}

fn remove_discovery(lock_path: &Path, snapshot: &[u8], lock: &mut Value, paths: &[(PathBuf, bool)]) -> Result<()> {
    let file = open_file(lock_path, false, DATA_LIMIT)?;
    file.try_lock()
        .wrap_err("Another process is maintaining the shared skill lock")?;
    let identity = file.metadata()?;
    let current = fs::symlink_metadata(lock_path)?;
    ensure!(
        current.is_file()
            && (current.dev(), current.ino()) == (identity.dev(), identity.ino())
            && bounded(&file, DATA_LIMIT)? == snapshot,
        "Shared skill lock changed before retiring {RETIRED_DISCOVERY}"
    );
    for (path, is_directory) in paths {
        if *is_directory {
            fs::remove_dir(path)?;
        } else {
            fs::remove_file(path)?;
        }
        sync_directory(path.parent().context("Missing retired skill parent")?)?;
        ensure!(!exists(path)?, "Retired skill path was recreated: {}", path.display());
    }
    lock["skills"]
        .as_object_mut()
        .context("Invalid shared lock entries")?
        .remove(RETIRED_DISCOVERY)
        .context("Missing verified retired skill lock entry")?;
    let mut data = serde_json::to_vec_pretty(lock)?;
    data.push(b'\n');
    ensure!(data.len() as u64 <= DATA_LIMIT, "Retired skill lock is too large");
    ensure!(
        bytes(lock_path, DATA_LIMIT)? == snapshot,
        "Shared skill lock changed during retirement"
    );
    atomic_write(lock_path, &data, identity.mode() & 0o777)?;
    ensure!(
        bytes(lock_path, DATA_LIMIT)? == data,
        "Retired skill lock failed verification"
    );
    println!("Removed upstream-retired {RETIRED_DISCOVERY} and its lock entry. Successor: discover-assets.");
    Ok(())
}

fn plan(mut lock: Value, rows: &[Row], home: &Path) -> Result<(Value, Vec<Value>)> {
    lock_entries(&lock)?;
    let entries = lock["skills"].as_object_mut().context("Invalid shared lock entries")?;
    let aliases: Vec<_> = entries
        .iter()
        .filter(|(alias, record)| alias.contains('/') && record["source"] == SOURCE)
        .map(|(alias, record)| (alias.clone(), record.clone()))
        .collect();
    let mut changes = Vec::new();
    for (alias, record) in aliases {
        ensure!(alias.split('/').all(exact_segment), "Unsafe GitHub alias");
        for root in ROOTS {
            let root = home.join(root);
            if exists(&root)? {
                directory(home, Path::new(root.strip_prefix(home)?), false)?;
                ensure!(
                    !exists(&root.join(&alias))?,
                    "Namespaced skill is installed, ownership review required: {alias}"
                );
            }
        }
        let mut matches = entries.iter().filter(|(name, value)| {
            !name.contains('/') && value.is_object() && FIELDS.iter().all(|field| value[field] == record[field])
        });
        let (name, canonical) = matches.next().context("GitHub alias has no canonical lock record")?;
        ensure!(
            matches.next().is_none(),
            "GitHub alias has ambiguous canonical lock records: {alias}"
        );
        ensure!(
            !name.is_empty() && name.bytes().all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)),
            "Unsafe canonical skill name"
        );
        let name = name.clone();
        let canonical = canonical.clone();
        let expected = directory(home, &Path::new(ROOTS[0]).join(&name), false)?;
        let mut observations = rows.iter().filter(|row| row.skill_name == alias);
        let row = observations
            .next()
            .context("GitHub alias missing from installed-state inventory")?;
        ensure!(
            observations.next().is_none() && row.path == expected && row.source_url.as_deref() == Some(SOURCE_URL),
            "GitHub installed-state mismatch for {alias}"
        );
        ensure!(
            !row.pinned && !present_ref(&record) && !present_ref(&canonical),
            "Pinned or ref-specific GitHub alias requires review: {alias}"
        );
        let meta = ownership::metadata(&expected)?;
        let sha = record["skillFolderHash"]
            .as_str()
            .context("Invalid GitHub skill folder hash")?;
        ensure!(
            meta.name == name
                && meta.source == SOURCE_URL
                && record["skillPath"] == format!("{}/SKILL.md", meta.directory)
                && record["sourceType"] == "github"
                && is_hex(sha, 40)
                && meta.tree == sha,
            "Installed metadata does not verify GitHub alias: {alias}"
        );
        let mut normalized = canonical.clone();
        normalized["skillFolderHash"] = sha.into();
        if timestamp(&record)? > timestamp(&canonical)? {
            normalized["updatedAt"] = record["updatedAt"].clone();
        }
        let original = bytes(&expected.join("SKILL.md"), DATA_LIMIT)?;
        changes.push(json!({
            "ghAlias": alias,
            "canonicalName": name,
            "installedPath": expected,
            "installedSkillSHA256": digest(&original),
            "originalAlias": record,
            "originalCanonical": canonical,
            "normalizedRecord": normalized,
        }));
        entries.remove(&alias);
        entries.insert(name, normalized);
    }
    Ok((lock, changes))
}

fn replace_open(file: &mut File, data: &[u8]) -> Result<()> {
    file.seek(SeekFrom::Start(0))?;
    file.write_all(data)?;
    file.set_len(data.len() as u64)?;
    file.sync_all()?;
    file.seek(SeekFrom::Start(0))?;
    ensure!(
        bounded(file, DATA_LIMIT)? == data,
        "Shared skill lock failed write verification"
    );
    Ok(())
}

fn normalize(home: &Path, route: &Route) -> Result<()> {
    let path = lock_path(home)?;
    directory(home, Path::new(".agents"), false)?;
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(&path)?;
    check_file(&file, &path, false, DATA_LIMIT)?;
    file.try_lock()
        .wrap_err("Another process is normalizing the shared skill lock")?;
    let identity = file.metadata()?;
    let original = bounded(&mut file, DATA_LIMIT)?;
    let lock = super::json(&original)?;
    if !lock_entries(&lock)?
        .iter()
        .any(|(name, item)| name.contains('/') && item["source"] == SOURCE)
    {
        println!("Skill lock interoperability: already canonical; no changes");
        return Ok(());
    }
    let (candidate, changes) = plan(lock, &rows(home, Some(route))?, home)?;
    let archive_root = directory(home, Path::new(".local/state/topgrade-skill-lock"), true)?;
    let archive = tempfile::Builder::new()
        .prefix("gh-npx-")
        .tempdir_in(&archive_root)?
        .keep();
    atomic_write(&archive.join("original.skill-lock.json"), &original, 0o600)?;
    let report = json!({
        "checkedAt": Utc::now().to_rfc3339(),
        "source": "gh 2.103.3 InstallName / NPX frontmatter-name compatibility",
        "changes": changes,
    });
    let mut report_bytes = serde_json::to_vec_pretty(&report)?;
    report_bytes.push(b'\n');
    atomic_write(&archive.join("normalization.json"), &report_bytes, 0o600)?;
    sync_directory(&archive_root)?;
    for item in &changes {
        let path = item["installedPath"]
            .as_str()
            .context("Invalid normalized skill path")?;
        ensure!(
            item["installedSkillSHA256"] == digest(&bytes(&Path::new(path).join("SKILL.md"), DATA_LIMIT)?),
            "Installed skill changed during normalization"
        );
    }
    let current = fs::symlink_metadata(&path)?;
    file.seek(SeekFrom::Start(0))?;
    ensure!(
        current.is_file()
            && (current.dev(), current.ino()) == (identity.dev(), identity.ino())
            && bounded(&mut file, DATA_LIMIT)? == original,
        "Shared skill lock changed before normalization"
    );
    let mut output = serde_json::to_vec_pretty(&candidate)?;
    output.push(b'\n');
    ensure!(output.len() as u64 <= DATA_LIMIT, "Normalized skill lock is too large");
    if let Err(error) = replace_open(&mut file, &output) {
        replace_open(&mut file, &original)
            .wrap_err_with(|| format!("Normalization failed ({error:#}); restore from {}", archive.display()))?;
        return Err(error);
    }
    println!(
        "Skill lock interoperability: normalized {} GitHub aliases; original retained at {}",
        changes.len(),
        archive.display()
    );
    Ok(())
}

fn accepted(output: &str, success: bool) -> bool {
    let plain = ANSI.replace_all(output, "").replace('\r', "\n");
    success && !FAILURE.is_match(&plain) && SUCCESS.is_match(&plain)
}

fn tee(reader: &mut dyn Read, mut sink: impl Write, secrets: &[String], mut remaining: u64) -> Result<Vec<u8>> {
    let mut reader = BufReader::new(reader);
    let mut record = Vec::new();
    let mut evidence = Vec::new();
    let mut emit = |record: &[u8]| -> Result<()> {
        let mut text = String::from_utf8_lossy(record).into_owned();
        for secret in secrets {
            text = text.replace(secret, "[REDACTED]");
        }
        sink.write_all(text.as_bytes())?;
        sink.flush()?;
        evidence.extend_from_slice(text.as_bytes());
        Ok(())
    };
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            if !record.is_empty() {
                emit(&record)?;
            }
            return Ok(evidence);
        }
        let boundary = chunk.iter().position(|byte| matches!(byte, b'\n' | b'\r'));
        let count = boundary.map_or(chunk.len(), |index| index + 1);
        ensure!(count as u64 <= remaining, "Skill update output exceeded its size limit");
        remaining -= count as u64;
        record.extend_from_slice(&chunk[..count]);
        reader.consume(count);
        if boundary.is_some() {
            // Buffer one record so credentials split across pipe reads never reach the terminal.
            emit(&record)?;
            record.clear();
        }
    }
}

fn verify_lock(before: &Value, after: &Value) -> Result<()> {
    let new = lock_entries(after)?;
    let old = lock_entries(before)?;
    ensure!(
        before
            .as_object()
            .context("Invalid NPX lock root")?
            .iter()
            .filter(|(key, _)| key.as_str() != "skills")
            .all(|(key, value)| after.get(key) == Some(value)),
        "NPX changed existing global skill lock metadata"
    );
    for (name, previous) in old {
        let entry = new.get(name).context("NPX removed an installed skill lock entry")?;
        ensure!(
            previous
                .as_object()
                .context("Invalid NPX lock entry")?
                .iter()
                .filter(|(key, _)| !matches!(key.as_str(), "skillFolderHash" | "updatedAt"))
                .all(|(key, value)| entry.get(key) == Some(value)),
            "NPX changed an installed skill identity or auxiliary metadata: {name}"
        );
    }
    Ok(())
}

pub(super) fn run(home: &Path) -> Result<()> {
    let route = Route::detect();
    normalize(home, &route)?;
    let lock_path = lock_path(home)?;
    let original = bytes(&lock_path, DATA_LIMIT)?;
    let before = super::json(&original)?;
    let old = lock_entries(&before)?.get(RETIRED_DISCOVERY).unwrap_or(&Value::Null);
    let retired_paths = discovery_paths(home, old, DISCOVERY_SHA256)?;
    let mut command = command(home, "npx", Some(&route))?;
    command.env(
        "DEVENV_STUDIO_RETIRE_DISCOVERY",
        if retired_paths.is_some() { "1" } else { "0" },
    );
    let mut secrets = Vec::new();
    for name in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Ok(value) = std::env::var(name)
            && !value.is_empty()
        {
            ensure!(
                !value.chars().any(char::is_whitespace),
                "Malformed GitHub credential environment"
            );
            command.env(name, &value);
            secrets.push(value);
        }
    }
    if secrets.is_empty() {
        let result = capture(
            super::command(home, "gh", Some(&route))?.args(["auth", "token", "--hostname", "github.com"]),
            Duration::from_secs(15),
            4096,
        )?;
        let token = String::from_utf8(result.out)?;
        let token = token.trim();
        if result.status.success() && !token.is_empty() && !token.chars().any(char::is_whitespace) {
            command.env("GITHUB_TOKEN", token);
            secrets.push(token.to_owned());
        }
    }
    let work = tempfile::tempdir()?;
    npm_config(&mut command, work.path())?;
    let launcher = work.path().join("skills.mjs");
    fs::write(&launcher, include_str!("skills-1.7.6.mjs"))?;
    let node = resolve_tool("node")?;
    let archive_root = directory(home, Path::new(".local/state/topgrade-skill-lock"), true)?;
    let archive = tempfile::Builder::new()
        .prefix("npx-")
        .tempdir_in(&archive_root)?
        .keep();
    atomic_write(&archive.join("original.skill-lock.json"), &original, 0o600)?;
    sync_directory(&archive_root)?;
    ensure!(
        bytes(&lock_path, DATA_LIMIT)? == original,
        "Shared skill lock changed before NPX execution"
    );
    println!("Running {SKILLS_PACKAGE} update --global --yes (per-source results)");
    let out = capture_with(
        command
            .current_dir(work.path())
            .args(["--yes", "--package", SKILLS_PACKAGE, "--"])
            .arg(node)
            .arg(launcher)
            .args(["update", "--global", "--yes"]),
        Duration::from_secs(900),
        DATA_LIMIT,
        move |reader, limit, stderr| {
            if stderr {
                tee(reader, io::stderr(), &secrets, limit)
            } else {
                tee(reader, io::stdout(), &secrets, limit)
            }
        },
    )
    .wrap_err_with(|| {
        format!(
            "Review NPX state before retrying; pre-update lock saved at {}",
            archive.display()
        )
    })?;
    let output = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.out),
        String::from_utf8_lossy(&out.err)
    );
    ensure!(
        accepted(&output, out.status.success()),
        "Skills update or integrity verification failed ({}). Pre-update lock saved at {}",
        out.status,
        archive.display()
    );
    let after_bytes = bytes(&lock_path, DATA_LIMIT)?;
    let mut after = super::json(&after_bytes)?;
    verify_lock(&before, &after).wrap_err_with(|| {
        format!(
            "NPX lock verification failed; pre-update lock saved at {}",
            archive.display()
        )
    })?;
    if output.lines().any(|line| line == DISCOVERY_MARKER) {
        let paths = retired_paths.context("Unexpected proof for an unverified retired skill")?;
        let record = &after["skills"][RETIRED_DISCOVERY];
        ensure!(
            record == old && discovery_paths(home, record, DISCOVERY_SHA256)?.as_ref() == Some(&paths),
            "Retired discovery skill changed during the update; pre-update lock saved at {}",
            archive.display()
        );
        remove_discovery(&lock_path, &after_bytes, &mut after, &paths).wrap_err_with(|| {
            format!(
                "Retired skill cleanup interrupted; pre-update lock saved at {}",
                archive.display()
            )
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_output_arrives_before_exit_and_hidden_failure_still_fails() -> Result<()> {
        use std::os::unix::net::UnixStream;

        let work = tempfile::tempdir()?;
        let resume = work.path().join("resume");
        let (mut out, out_sink) = UnixStream::pair()?;
        let (mut err, err_sink) = UnixStream::pair()?;
        out.set_read_timeout(Some(Duration::from_secs(5)))?;
        err.set_read_timeout(Some(Duration::from_secs(5)))?;
        let arg = resume.clone();
        let worker = thread::spawn(move || {
            capture_with(
                crate::studio::command(Path::new("/bin/sh"))
                    .args([
                        "-c",
                        "printf 'fetching\\r'; printf 'diagnostic\\n' >&2; \
                         while [ ! -f \"$1\" ]; do /bin/sleep 0.01; done; \
                         printf 'test-cre'; printf 'dential\\n'; \
                         printf 'Failed to check source\\n' >&2; \
                         printf 'All global skills are up to date'",
                        "fixture",
                    ])
                    .arg(arg),
                Duration::from_secs(10),
                4096,
                move |reader, limit, stderr| {
                    tee(
                        reader,
                        if stderr { &err_sink } else { &out_sink },
                        &["test-credential".into()],
                        limit,
                    )
                },
            )
        });
        let mut first = [0; 9];
        out.read_exact(&mut first)?;
        assert_eq!(&first, b"fetching\r");
        let mut diagnostic = [0; 11];
        err.read_exact(&mut diagnostic)?;
        assert_eq!(&diagnostic, b"diagnostic\n");
        fs::write(resume, [])?;

        let captured = worker.join().expect("fixture capture thread")?;
        let stdout = String::from_utf8(captured.out)?;
        let stderr = String::from_utf8(captured.err)?;
        let mut tail = String::new();
        out.read_to_string(&mut tail)?;
        assert_eq!(tail, "[REDACTED]\nAll global skills are up to date");
        assert!(stdout.ends_with(&tail));
        assert!(captured.status.success());
        assert!(!accepted(&format!("{stdout}\n{stderr}"), captured.status.success()));
        Ok(())
    }

    #[test]
    fn output_requires_success_evidence_and_rejects_hidden_failure() {
        assert!(accepted("All global skills are up to date", true));
        assert!(accepted("No global skills tracked in lock file.", true));
        assert!(accepted("Updated 12 skill(s)", true));
        let warning = "Warning: example/unavailable — source unavailable; local skills kept";
        let summary = format!("{warning}\nSkill source checks finished with warnings: 1 source(s) skipped");
        assert!(!accepted(warning, true));
        assert!(accepted(&summary, true));
        assert!(!accepted(&summary, false));
        assert!(!accepted(&format!("{summary}\nError: lock write failed"), true));
        assert!(!accepted(
            "All global skills are up to date\r\u{1b}[32m✗ source check\u{1b}[0m",
            true
        ));
        assert!(!accepted("Updated 1 skill(s)\nError: source failed", true));
        assert!(!accepted("Updated 1 skill(s)", false));
        assert!(!accepted("", true));
    }

    #[test]
    fn retirement_is_limited_to_the_reviewed_discovery_lock() -> Result<()> {
        let known = json!({
            "source": DISCOVERY_SOURCE,
            "sourceType": "github",
            "sourceUrl": DISCOVERY_URL,
            "skillPath": "skills/discovering-gcp-data-assets/SKILL.md",
            "skillFolderHash": DISCOVERY_TREE,
        });
        assert!(reviewed_discovery_lock(&known));
        for (key, replacement) in [
            ("source", "example/other"),
            ("sourceUrl", "https://example.test/other.git"),
            ("skillPath", "skills/unreviewed/SKILL.md"),
            ("skillFolderHash", "unreviewed"),
            ("ref", "old-release"),
            ("pinnedRef", "old-release"),
        ] {
            let mut changed = known.clone();
            changed[key] = replacement.into();
            assert!(!reviewed_discovery_lock(&changed), "{key}");
        }
        let work = tempfile::tempdir()?;
        let home = work.path().canonicalize()?;
        let path = Path::new(ROOTS[0]).join(RETIRED_DISCOVERY);
        let canonical = directory(&home, &path, true)?;
        let skill = canonical.join("SKILL.md");
        fs::write(&skill, b"synthetic reviewed content")?;
        let hash = digest(b"synthetic reviewed content");
        let owned = directory(&home, Path::new(".claude/skills"), true)?.join(RETIRED_DISCOVERY);
        let independent = directory(&home, Path::new(".codex/skills"), true)?.join(RETIRED_DISCOVERY);
        std::os::unix::fs::symlink(Path::new("../..").join(&path), &owned)?;
        fs::create_dir(&independent)?;
        let unknown = canonical.join("personal.txt");
        fs::write(&unknown, b"synthetic unexpected file")?;
        assert!(discovery_paths(&home, &known, &hash)?.is_none());
        fs::remove_file(&unknown)?;
        assert!(discovery_paths(&home, &known, &digest(b"different"))?.is_none());
        let lock_path = home.join(LOCK_PATH);
        let mut lock = json!({"version": 3, "skills": {
            (RETIRED_DISCOVERY): known,
            "unrelated": {"source": "example/other"}
        }});
        let snapshot = serde_json::to_vec(&lock)?;
        atomic_write(&lock_path, &snapshot, 0o600)?;
        let paths = discovery_paths(&home, &lock["skills"][RETIRED_DISCOVERY], &hash)?.unwrap();
        assert!(remove_discovery(&lock_path, b"changed", &mut lock, &paths).is_err());
        assert!(exists(&owned)? && exists(&skill)?);

        fs::remove_file(&skill)?;
        let interrupted = discovery_paths(&home, &lock["skills"][RETIRED_DISCOVERY], &hash)?.unwrap();
        remove_discovery(&lock_path, &snapshot, &mut lock, &interrupted)?;
        assert!(!exists(&owned)? && !exists(&canonical)?);
        assert!(independent.is_dir());
        let installed = super::super::json(&bytes(&lock_path, DATA_LIMIT)?)?;
        assert_eq!(installed["skills"], json!({"unrelated": {"source": "example/other"}}));
        assert!(discovery_paths(&home, &installed["skills"][RETIRED_DISCOVERY], &hash)?.is_none());
        Ok(())
    }

    #[test]
    fn npx_may_update_hash_and_time_but_must_keep_ownership_metadata() {
        let before = json!({
            "version": 3, "lastSelectedAgents": ["codex"],
            "skills": {"design": {"source": "example/repo", "pluginName": "reviewed",
                                  "skillFolderHash": "old", "updatedAt": "old"}},
        });
        let mut after = before.clone();
        after["skills"]["design"]["skillFolderHash"] = "new".into();
        after["skills"]["design"]["updatedAt"] = "new".into();
        assert!(verify_lock(&before, &after).is_ok());
        after["skills"]["design"].as_object_mut().unwrap().remove("pluginName");
        assert!(verify_lock(&before, &after).is_err());
    }

    #[test]
    fn normalization_preserves_unrelated_metadata_and_newer_timestamp() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().canonicalize()?;
        let path = directory(&home, Path::new(".agents/skills/debug"), true)?;
        let hash = "0123456789012345678901234567890123456789";
        fs::write(
            path.join("SKILL.md"),
            format!(
                "---\nname: debug\nmetadata:\n  github-path: .claude/skills/debug\n  github-repo: {SOURCE_URL}\n  github-tree-sha: {hash}\n---\nBody\n"
            ),
        )?;
        let alias = "debugging/debug";
        let original = json!({
            "version": 3, "lastSelectedAgents": ["codex"],
            "skills": {
                "debug": {"source": SOURCE, "sourceType": "github", "sourceUrl": SOURCE_URL,
                          "skillPath": ".claude/skills/debug/SKILL.md", "skillFolderHash": "old",
                          "updatedAt": "2026-10-31T12:00:00Z", "pluginName": "reviewed"},
                (alias): {"source": SOURCE, "sourceType": "github", "sourceUrl": SOURCE_URL,
                          "skillPath": ".claude/skills/debug/SKILL.md", "skillFolderHash": hash,
                          "updatedAt": "2026-10-31T11:00:00Z"},
                "other": {"source": "example/other", "custom": [1, null]},
            },
        });
        let mut row = Row {
            skill_name: alias.into(),
            source_url: Some(SOURCE_URL.into()),
            path,
            pinned: false,
            scope: None,
        };
        let (normalized, changes) = plan(original.clone(), std::slice::from_ref(&row), &home)?;
        assert_eq!(changes.len(), 1);
        assert!(normalized["skills"].get(alias).is_none());
        assert_eq!(normalized["skills"]["debug"]["skillFolderHash"], hash);
        assert_eq!(normalized["skills"]["debug"]["pluginName"], "reviewed");
        assert_eq!(normalized["skills"]["debug"]["updatedAt"], "2026-10-31T12:00:00Z");
        assert_eq!(normalized["skills"]["other"], original["skills"]["other"]);
        assert_eq!(normalized["lastSelectedAgents"], original["lastSelectedAgents"]);
        row.pinned = true;
        assert!(plan(original, &[row], &home).is_err());
        Ok(())
    }
}
