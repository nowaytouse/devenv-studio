//! Verify the public Bitnami index before delegating changes to native Helm.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::LazyLock;

use color_eyre::eyre::{Context, Result, ensure, eyre};
use nix::libc::{O_NOFOLLOW, O_NONBLOCK};
use regex::bytes::Regex;
use serde::Deserialize;

use super::{resolve_tool, sha256, tool_command};
use crate::{command::CommandExt as _, error::SkipStep};

const MAX_INDEX: u64 = 64 * 1024 * 1024;
const CURL: &str = "/usr/bin/curl";
const PUBLIC: [&str; 2] = [
    "https://charts.bitnami.com/bitnami",
    "https://repo.broadcom.com/bitnami-files",
];

#[derive(Deserialize)]
struct Repository {
    name: String,
    url: String,
}

fn valid_name(name: &str) -> bool {
    name.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
        && !name.contains("..")
}

fn read_regular(path: &Path) -> io::Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(path)?;
    let before = file.metadata()?;
    if !before.is_file() || before.len() > MAX_INDEX {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "repository file is not regular or is too large",
        ));
    }
    let mut source = file.take(MAX_INDEX + 1);
    let mut data = Vec::new();
    source.read_to_end(&mut data)?;
    let after = source.into_inner().metadata()?;
    if data.len() as u64 != before.len()
        || (
            before.len(),
            before.mtime(),
            before.mtime_nsec(),
            before.ctime(),
            before.ctime_nsec(),
        ) != (
            after.len(),
            after.mtime(),
            after.mtime_nsec(),
            after.ctime(),
            after.ctime_nsec(),
        )
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "repository file changed while reading",
        ));
    }
    Ok(data)
}

fn invoke(command: &mut Command) -> Result<Output> {
    // Listing failures can delegate. Keep the streams and exit status for the caller to classify.
    command.output_checked_with(|_| Ok(()))
}

fn native(helm: &Path, names: &[&str]) -> Result<()> {
    let result = invoke(tool_command(helm)?.args(["repo", "update"]).args(names))?;
    if names.is_empty()
        && result.status.code() == Some(1)
        && result.stdout.iter().all(u8::is_ascii_whitespace)
        && String::from_utf8_lossy(&result.stderr).trim()
            == "Error: no repositories found. You must add one before updating"
    {
        return Err(SkipStep("No configured Helm repositories".into()).into());
    }
    let mut stdout = io::stdout().lock();
    stdout.write_all(&result.stdout)?;
    stdout.flush()?;
    let mut stderr = io::stderr().lock();
    stderr.write_all(&result.stderr)?;
    stderr.flush()?;

    ensure!(
        result.status.success(),
        "Helm repository update exited with {}",
        result.status
    );
    static FAILED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?im)Unable to get an update|failed to fetch|^Error:").expect("valid Helm diagnostic pattern")
    });
    ensure!(
        !FAILED.is_match(&result.stdout) && !FAILED.is_match(&result.stderr),
        "Helm reported a repository failure despite exit 0"
    );
    Ok(())
}

fn validate_response(response: &str) -> Result<()> {
    let mut lines = response.lines();
    ensure!(lines.next() == Some("200"), "Unexpected Helm index HTTP response");
    let scheme = lines.next();
    let host = lines.next();
    ensure!(lines.next().is_none(), "Unexpected Helm index HTTP response metadata");
    ensure!(
        scheme == Some("https")
            && host.is_some_and(|host| {
                ["charts.bitnami.com", "repo.broadcom.com"]
                    .iter()
                    .any(|official| host.eq_ignore_ascii_case(official))
            }),
        "Helm index redirect left the official HTTPS hosts"
    );
    Ok(())
}

fn fetch_index(curl: &Path, url: &str, limit: u64) -> Result<Option<File>> {
    let mut body = tempfile::tempfile()?;
    let mut response = tempfile::tempfile()?;
    let mut child = tool_command(curl)?
        .args([
            "--disable",
            "--silent",
            "--fail",
            "--location",
            "--compressed",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--max-redirs",
            "3",
            "--connect-timeout",
            "15",
            "--max-time",
            "90",
            "--max-filesize",
        ])
        .arg(limit.to_string())
        .args([
            "--write-out",
            "%{stderr}%{http_code}\\n%{urle.scheme}\\n%{urle.host}\\n",
            "--",
        ])
        .arg(format!("{url}/index.yaml"))
        .stdout(Stdio::piped())
        .stderr(Stdio::from(response.try_clone()?))
        .spawn_checked()?;
    let stream = child.stdout.take().expect("piped curl output");
    let copied = io::copy(&mut stream.take(limit + 1), &mut body);
    // --max-filesize can apply to the compressed size. Bound the decoded stream ourselves.
    if !matches!(copied, Ok(size) if size <= limit) {
        let _ = child.kill();
    }
    let status = child.wait()?;
    ensure!(copied? <= limit, "Helm index exceeds the decoded size limit");
    if !status.success() {
        println!("Helm: compressed freshness check unavailable; delegating to native Helm");
        return Ok(None);
    }
    response.rewind()?;
    let mut metadata = String::new();
    response.take(1025).read_to_string(&mut metadata)?;
    ensure!(metadata.len() <= 1024, "Helm index HTTP response metadata is too large");
    validate_response(&metadata)?;
    body.rewind()?;
    Ok(Some(body))
}

fn unchanged_public_index(repo: &Repository, cache: &Path, curl: &Path, helm: &Path) -> Result<Option<String>> {
    let index = cache.join(format!("{}-index.yaml", repo.name));
    let before = match read_regular(&index) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            println!("Helm: cache is absent; using native updater");
            return Ok(None);
        }
        Err(error) => return Err(error).wrap_err("Could not safely read Helm repository cache"),
    };
    let Some(mut body) = fetch_index(curl, repo.url.trim_end_matches('/'), MAX_INDEX)? else {
        return Ok(None);
    };
    let mut downloaded = Vec::new();
    body.read_to_end(&mut downloaded)?;
    if downloaded != before {
        return Ok(None);
    }

    let pattern = format!("^{}/", regex::escape(&repo.name));
    let parsed = invoke(tool_command(helm)?.args(["search", "repo", "--regexp", &pattern, "--output", "json"]))?;
    ensure!(
        parsed.status.success()
            && parsed.stderr.iter().all(u8::is_ascii_whitespace)
            && serde_json::from_slice::<Vec<serde_json::Value>>(&parsed.stdout).is_ok(),
        "Native Helm could not validate the current repository index"
    );
    ensure!(
        read_regular(&index)? == before,
        "Helm repository cache changed during verification"
    );
    body.rewind()?;
    Ok(Some(sha256(body)?))
}

fn run_with_tools(helm: &Path, curl: Option<&Path>) -> Result<()> {
    let listing = invoke(tool_command(helm)?.args(["repo", "list", "--output", "json"]))?;
    if !listing.status.success() {
        return native(helm, &[]);
    }
    let repos: Vec<Repository> =
        serde_json::from_slice(&listing.stdout).map_err(|_| eyre!("Invalid Helm repository listing"))?;
    if repos.is_empty() {
        // Helm's listing also returns [] when it cannot read or parse its configuration.
        return native(helm, &[]);
    }
    let mut seen = HashSet::new();
    let mut names = Vec::new();
    for repo in &repos {
        let name = repo.name.as_str();
        ensure!(
            valid_name(name) && seen.insert(name),
            "Unsafe or duplicate Helm repository name"
        );
        names.push(name);
    }

    let cache_info = invoke(tool_command(helm)?.args(["env", "HELM_REPOSITORY_CACHE"]))?;
    let config_info = invoke(tool_command(helm)?.args(["env", "HELM_REPOSITORY_CONFIG"]))?;
    if !cache_info.status.success() || !config_info.status.success() {
        return native(helm, &names);
    }
    let cache = PathBuf::from(std::str::from_utf8(&cache_info.stdout)?.trim());
    let config = PathBuf::from(std::str::from_utf8(&config_info.stdout)?.trim());
    ensure!(
        cache.is_absolute() && config.is_absolute() && !cache.is_symlink(),
        "Unexpected Helm repository paths"
    );
    let config_before = read_regular(&config).wrap_err("Could not safely read Helm repository configuration")?;
    let mut pending = Vec::new();
    for repo in &repos {
        let digest = if let Some(curl) = curl
            && PUBLIC.contains(&repo.url.trim_end_matches('/'))
        {
            unchanged_public_index(repo, &cache, curl, helm)?
        } else {
            None
        };
        ensure!(
            read_regular(&config)? == config_before,
            "Helm repository configuration changed during verification"
        );
        if let Some(digest) = digest {
            println!(
                "Helm {}: CURRENT — official compressed index matches cache (SHA256 {digest})",
                repo.name
            );
        } else {
            pending.push(repo.name.as_str());
        }
    }
    if pending.is_empty() {
        Ok(())
    } else {
        native(helm, &pending)
    }
}

pub(super) fn run() -> Result<()> {
    let curl = Path::new(CURL);
    run_with_tools(&resolve_tool("helm")?, curl.is_file().then_some(curl))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    use tempfile::TempDir;

    use super::*;

    const INDEX: &[u8] = b"apiVersion: v1\nentries: {}\n";

    struct Fixture {
        dir: TempDir,
        helm: PathBuf,
        curl: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            fs::create_dir(root.join("cache")).unwrap();
            fs::write(root.join("cache/bitnami-index.yaml"), INDEX).unwrap();
            fs::write(root.join("download"), INDEX).unwrap();
            fs::write(root.join("repositories.yaml"), b"repositories: []\n").unwrap();
            fs::write(
                root.join("listing"),
                br#"[{"name":"bitnami","url":"https://charts.bitnami.com/bitnami/"}]"#,
            )
            .unwrap();
            fs::write(root.join("response"), b"200\nhttps\nrepo.broadcom.com\n").unwrap();
            fs::write(root.join("native-code"), b"0\n").unwrap();
            fs::write(root.join("native-diagnostic"), b"").unwrap();
            let helm = Self::script(
                root,
                "helm",
                r#"case "$1 $2" in
  'repo list') /bin/cat listing ;;
  'env HELM_REPOSITORY_CACHE') printf '%s/cache\n' "$PWD" ;;
  'env HELM_REPOSITORY_CONFIG') printf '%s/repositories.yaml\n' "$PWD" ;;
  'search repo')
    printf '[]\n'
    if [ -e parser-warning ]; then /bin/cat parser-warning >&2; fi
    if [ -e change-config ]; then printf 'changed\n' > repositories.yaml; fi
    if [ -e change-index ]; then printf 'changed\n' > cache/bitnami-index.yaml; fi
    ;;
  'repo update')
    shift 2
    printf '%s\n' "$@" >> native-calls
    /bin/cat native-diagnostic >&2
    read -r code < native-code
    exit "$code" ;;
  *) exit 99 ;;
esac"#,
            );
            let curl = Self::script(
                root,
                "curl",
                r#"printf '%s\n' "$@" > curl-args
if [ -e transfer-error ]; then exit 18; fi
/bin/cat download
/bin/cat response >&2"#,
            );
            Self { dir, helm, curl }
        }

        fn script(root: &Path, name: &str, body: &str) -> PathBuf {
            let path = root.join(name);
            let cwd = shell_words::quote(root.to_str().unwrap());
            fs::write(&path, format!("#!/bin/sh\ncd {cwd} || exit 99\n{body}\n")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            path
        }

        fn file(&self, path: &str) -> PathBuf {
            self.dir.path().join(path)
        }

        fn run(&self) -> Result<()> {
            run_with_tools(&self.helm, Some(&self.curl))
        }
    }

    #[test]
    fn empty_listing_distinguishes_no_repositories_from_invalid_configuration() {
        let fixture = Fixture::new();
        fs::write(fixture.file("listing"), b"[]\n").unwrap();
        fs::write(fixture.file("native-code"), b"1\n").unwrap();
        let diagnostic = fixture.file("native-diagnostic");
        fs::write(
            &diagnostic,
            b"Error: no repositories found. You must add one before updating\n",
        )
        .unwrap();
        let skipped = fixture.run().unwrap_err();
        assert!(skipped.downcast_ref::<SkipStep>().is_some(), "{skipped:#}");
        assert_eq!(fs::read(fixture.file("native-calls")).unwrap(), b"\n");

        fs::write(&diagnostic, b"Error: failed loading file: malformed YAML\n").unwrap();
        let failed = fixture.run().unwrap_err();
        assert!(failed.downcast_ref::<SkipStep>().is_none());
        assert!(failed.to_string().contains("exit status: 1"), "{failed:#}");
        assert!(!fixture.file("curl-args").exists());
    }

    #[test]
    fn identical_official_index_is_verified_without_cache_writes() {
        let fixture = Fixture::new();
        let index = fixture.file("cache/bitnami-index.yaml");
        let modified = fs::metadata(&index).unwrap().modified().unwrap();
        fixture.run().unwrap();
        assert!(!fixture.file("native-calls").exists());
        assert_eq!(fs::read(&index).unwrap(), INDEX);
        assert_eq!(fs::metadata(&index).unwrap().modified().unwrap(), modified);
        let arguments = fs::read_to_string(fixture.file("curl-args")).unwrap();
        assert!(arguments.starts_with("--disable\n"));
        assert!(arguments.contains("--compressed\n--proto\n=https\n--proto-redir\n=https\n--max-redirs\n3\n"));
        assert!(arguments.contains("--connect-timeout\n15\n--max-time\n90\n--max-filesize\n67108864\n"));
        assert!(arguments.contains("%{http_code}\\n%{urle.scheme}\\n%{urle.host}\\n"));
        assert!(arguments.ends_with("https://charts.bitnami.com/bitnami/index.yaml\n"));
    }

    #[test]
    fn changed_index_preserves_native_failure_and_only_delegates_pending_repositories() {
        let fixture = Fixture::new();
        fs::write(
            fixture.file("listing"),
            br#"[
                {"name":"bitnami","url":"https://charts.bitnami.com/bitnami"},
                {"name":"vendor","url":"https://example.invalid/charts"}
            ]"#,
        )
        .unwrap();
        fixture.run().unwrap();
        assert_eq!(fs::read(fixture.file("native-calls")).unwrap(), b"vendor\n");

        fs::write(fixture.file("download"), b"changed").unwrap();
        fs::write(fixture.file("native-code"), b"7\n").unwrap();
        let error = fixture.run().unwrap_err().to_string();
        assert!(error.contains("exit status: 7"), "{error}");
        assert_eq!(
            fs::read(fixture.file("native-calls")).unwrap(),
            b"vendor\nbitnami\nvendor\n"
        );
        assert_eq!(fs::read(fixture.file("cache/bitnami-index.yaml")).unwrap(), INDEX);
    }

    #[test]
    fn transfer_failure_delegates_but_native_false_success_fails() {
        let fixture = Fixture::new();
        fs::write(fixture.file("transfer-error"), b"").unwrap();
        fs::write(
            fixture.file("native-diagnostic"),
            b"Unable to get an update from repository\n",
        )
        .unwrap();
        let error = fixture.run().unwrap_err().to_string();
        assert!(error.contains("despite exit 0"), "{error}");
        assert_eq!(fs::read(fixture.file("native-calls")).unwrap(), b"bitnami\n");
    }

    #[test]
    fn unsafe_response_or_parser_warning_fails_without_native_update() {
        let fixture = Fixture::new();
        for response in [
            b"200\nhttps\nrepo.broadcom.com.evil.invalid\n".as_slice(),
            b"200\nhttp\nrepo.broadcom.com\n",
            b"206\nhttps\nrepo.broadcom.com\n",
        ] {
            fs::write(fixture.file("response"), response).unwrap();
            assert!(fixture.run().is_err());
            assert!(!fixture.file("native-calls").exists());
        }
        fs::write(fixture.file("response"), b"200\nhttps\nrepo.broadcom.com\n").unwrap();
        fs::write(fixture.file("parser-warning"), b"WARNING: corrupt index\n").unwrap();
        let error = fixture.run().unwrap_err().to_string();
        assert!(error.contains("could not validate"), "{error}");
        assert!(!fixture.file("native-calls").exists());
    }

    #[test]
    fn configuration_or_cache_changes_during_parser_fail_closed() {
        for (trigger, message) in [
            ("change-config", "configuration changed during verification"),
            ("change-index", "cache changed during verification"),
        ] {
            let fixture = Fixture::new();
            fs::write(fixture.file(trigger), b"").unwrap();
            let error = fixture.run().unwrap_err().to_string();
            assert!(error.contains(message), "{error}");
            assert!(!fixture.file("native-calls").exists());
        }
    }

    #[test]
    fn absent_index_delegates_while_symlink_or_fifo_fails_closed() {
        let fixture = Fixture::new();
        let index = fixture.file("cache/bitnami-index.yaml");
        fs::remove_file(&index).unwrap();
        fixture.run().unwrap();
        assert_eq!(fs::read(fixture.file("native-calls")).unwrap(), b"bitnami\n");
        fs::remove_file(fixture.file("native-calls")).unwrap();

        symlink(fixture.file("download"), &index).unwrap();
        assert!(fixture.run().is_err());
        fs::remove_file(&index).unwrap();
        tool_command(Path::new("/usr/bin/mkfifo"))
            .unwrap()
            .arg(&index)
            .status_checked()
            .unwrap();
        assert!(fixture.run().is_err());
        assert!(!fixture.file("curl-args").exists());
        assert!(!fixture.file("native-calls").exists());
    }

    #[test]
    fn decoded_stream_size_is_bounded_independently_of_curl() {
        let fixture = Fixture::new();
        let error = fetch_index(&fixture.curl, PUBLIC[0], 4).unwrap_err().to_string();
        assert!(error.contains("decoded size limit"), "{error}");
        assert!(!fixture.file("native-calls").exists());
    }

    #[test]
    fn invalid_repository_names_fail_before_native_update() {
        let fixture = Fixture::new();
        for invalid in [
            br#"[{"name":"../other","url":"https://example.invalid"}]"#.as_slice(),
            br#"[{"name":"bitnami","url":""},{"name":"bitnami","url":""}]"#,
        ] {
            fs::write(fixture.file("listing"), invalid).unwrap();
            assert!(fixture.run().is_err());
        }
        assert!(!fixture.file("native-calls").exists());
        assert!(!fixture.file("curl-args").exists());
    }
}
