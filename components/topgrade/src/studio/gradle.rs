//! macOS Gradle Nightly updater with an independently verified local launcher.
mod archive;
mod transport;

use color_eyre::eyre::{Context, Result, ensure, eyre};
use nix::libc::{self, O_NOFOLLOW, O_NONBLOCK};
use regex::Regex;
use serde::Deserialize;
use std::{
    ffi::CString,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        process::CommandExt as _,
    },
    path::{Path, PathBuf},
    process::Stdio,
    sync::{LazyLock, mpsc},
    thread,
    time::{Duration, Instant},
};

use super::{account_home, check_dir, check_file, command, open_file, user_only};
use crate::command::CommandExt as _;
use transport::{Failure, Transport};

const API: &str = "https://services.gradle.org/versions/nightly";
const MAX_LAUNCHER: u64 = 8192;
static VERSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[0-9]+(?:\.[0-9]+){1,2}-[0-9]{14}\+[0-9]{4}").expect("static Gradle version pattern")
});

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Nightly {
    version: String,
    download_url: String,
    checksum: String,
}

fn valid_version(version: &str) -> bool {
    version.len() <= 96 && VERSION.find(version).is_some_and(|found| found.as_str() == version)
}

impl Nightly {
    fn validate(mut self) -> Result<Self> {
        ensure!(valid_version(&self.version), "Invalid Gradle Nightly version");
        self.checksum.make_ascii_lowercase();
        ensure!(
            self.checksum.len() == 64 && self.checksum.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "Invalid Gradle Nightly SHA-256"
        );
        ensure!(
            self.download_url
                == format!(
                    "https://services.gradle.org/distributions-snapshots/gradle-{}-bin.zip",
                    self.version
                ),
            "Gradle Nightly metadata did not name its official immutable ZIP"
        );
        Ok(self)
    }
}

fn directory(path: &Path) -> Result<()> {
    match DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    check_dir(path, false)
}

fn lock(root: &Path) -> Result<File> {
    let path = root.join(".update.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(&path)?;
    check_file(&file, &path, false, 0)?;
    file.try_lock()
        .wrap_err("Another Gradle Nightly update may be running")?;
    Ok(file)
}

fn java_home() -> Result<PathBuf> {
    for prefix in ["/opt/homebrew", "/usr/local"] {
        let path = Path::new(prefix).join("opt/openjdk/libexec/openjdk.jdk/Contents/Home");
        let executable = match path.join("bin/java").canonicalize() {
            Ok(executable) => executable,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let metadata = executable.metadata()?;
        ensure!(
            executable.starts_with(Path::new(prefix).join("Cellar/openjdk"))
                && metadata.is_file()
                && metadata.mode() & 0o111 != 0
                && metadata.mode() & 0o022 == 0,
            "Homebrew OpenJDK executable could not be trusted"
        );
        return Ok(path);
    }
    Err(eyre!("Homebrew OpenJDK was not found"))
}

fn read_launcher(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => ensure!(
            metadata.is_file() && metadata.mode() & 0o111 != 0,
            "Unsafe Gradle Nightly launcher"
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let mut bytes = Vec::new();
    open_file(path, false, MAX_LAUNCHER)?
        .take(MAX_LAUNCHER + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_LAUNCHER,
        "Gradle Nightly launcher is too large"
    );
    Ok(Some(bytes))
}

fn quote(path: &Path) -> Result<String> {
    let path = path
        .to_str()
        .ok_or_else(|| eyre!("Gradle launcher path is not UTF-8"))?;
    ensure!(!path.chars().any(char::is_control), "Invalid Gradle launcher path");
    let mut quoted = String::from("\"");
    for character in path.chars() {
        if matches!(character, '\\' | '\"' | '$' | '`') {
            quoted.push('\\');
        }
        quoted.push(character);
    }
    quoted.push('"');
    Ok(quoted)
}

fn launcher_contents(binary: &Path, java: &Path) -> Result<Vec<u8>> {
    Ok(format!(
        "#!/bin/sh\nset -eu\n\nif [ -z \"${{JAVA_HOME:-}}\" ]; then\n    export JAVA_HOME={}\nfi\n\nexport PATH=\"$JAVA_HOME/bin:$PATH\"\n\nexec {} \"$@\"\n",
        quote(java)?,
        quote(binary)?
    )
    .into_bytes())
}

fn launcher_version(bytes: &[u8], root: &Path, java: &Path) -> Result<String> {
    let text = std::str::from_utf8(bytes)?;
    let exec = text
        .lines()
        .last()
        .ok_or_else(|| eyre!("Empty Gradle Nightly launcher"))?;
    for candidate in VERSION.find_iter(exec) {
        let version = candidate.as_str();
        if valid_version(version) && bytes == launcher_contents(&root.join(version).join("bin/gradle"), java)? {
            return Ok(version.to_owned());
        }
    }
    Err(eyre!(
        "Existing Gradle Nightly launcher differs from the managed layout; it was preserved"
    ))
}

fn verify_runtime(root: &Path, version: &str, java: &Path) -> Result<()> {
    check_dir(root, false)?;
    check_dir(&root.join("bin"), false)?;
    check_dir(&root.join("lib"), false)?;
    let binary = root.join("bin/gradle");
    let metadata = open_file(&binary, false, 1024 * 1024)?.metadata()?;
    ensure!(
        metadata.mode() & 0o111 != 0,
        "Gradle Nightly launcher is not executable"
    );
    let probe = tempfile::tempdir()?;
    let mut child = command(&binary)
        .args(["--version"])
        .current_dir(probe.path())
        .env("HOME", account_home()?)
        .env("GRADLE_USER_HOME", probe.path())
        .env("JAVA_HOME", java)
        .env(
            "PATH",
            std::env::join_paths([
                java.join("bin"),
                "/usr/bin".into(),
                "/bin".into(),
                "/usr/sbin".into(),
                "/sbin".into(),
            ])?,
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn_checked()?;
    let pipe = child.stdout.take().expect("piped Gradle stdout");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = pipe.take(64 * 1024 + 1).read_to_end(&mut bytes).map(|_| bytes);
        let _ = tx.send(result);
    });
    let deadline = Instant::now() + Duration::from_secs(90);
    let result = (|| -> Result<()> {
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            ensure!(Instant::now() < deadline, "Gradle runtime verification timed out");
            thread::sleep(Duration::from_millis(50));
        };
        let output = rx
            .recv_timeout(Duration::from_secs(2))
            .wrap_err("Gradle runtime output did not close")??;
        let expected = format!("Gradle {version}");
        ensure!(
            status.success()
                && output.len() <= 64 * 1024
                && std::str::from_utf8(&output)?
                    .lines()
                    .any(|line| line.trim() == expected),
            "Installed Gradle runtime did not report its exact expected version"
        );
        Ok(())
    })();
    if result.is_err() {
        // A launcher's Java process may outlive the script.
        unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
        let _ = child.wait();
    }
    result
}

fn activate(link: &Path, before: Option<&[u8]>, contents: &[u8]) -> Result<()> {
    ensure!(
        read_launcher(link)?.as_deref() == before,
        "Gradle launcher changed during verification"
    );
    if before == Some(contents) {
        return Ok(());
    }
    let parent = link
        .parent()
        .ok_or_else(|| eyre!("Gradle launcher has no parent directory"))?;
    let mut staged = tempfile::Builder::new()
        .prefix(".gradle-nightly-")
        .tempfile_in(parent)?;
    staged.write_all(contents)?;
    staged.as_file().set_permissions(fs::Permissions::from_mode(0o755))?;
    staged.as_file().sync_all()?;
    ensure!(
        read_launcher(link)?.as_deref() == before,
        "Gradle launcher changed before activation"
    );
    staged.persist(link)?;
    Ok(())
}

fn move_directory(source: &Path, dest: &Path, swap: bool) -> Result<()> {
    let source = CString::new(source.as_os_str().as_bytes())?;
    let dest = CString::new(dest.as_os_str().as_bytes())?;
    let flags = if swap { libc::RENAME_SWAP } else { libc::RENAME_EXCL };
    // Keep the selected Gradle path continuously present during same-version repair.
    if unsafe { libc::renamex_np(source.as_ptr(), dest.as_ptr(), flags) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error().into())
    }
}

fn directory_id(path: &Path) -> Result<(u64, u64)> {
    check_dir(path, false)?;
    let metadata = fs::symlink_metadata(path)?;
    Ok((metadata.dev(), metadata.ino()))
}

fn publish(
    stage: tempfile::TempDir,
    candidate: &Path,
    target: &Path,
    previous: Option<(u64, u64)>,
    validate: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let proposed = directory_id(candidate)?;
    if let Some(previous) = previous {
        ensure!(
            directory_id(target)? == previous,
            "Existing Gradle runtime changed before repair"
        );
    }
    move_directory(candidate, target, previous.is_some())?;
    let result = (|| {
        ensure!(directory_id(target)? == proposed, "Published Gradle runtime changed");
        validate()
    })();
    if let Err(error) = result {
        let restored = (|| {
            ensure!(
                directory_id(target)? == proposed,
                "Published Gradle runtime changed before rollback"
            );
            if let Some(previous) = previous {
                ensure!(
                    directory_id(candidate)? == previous,
                    "Preserved Gradle runtime changed before rollback"
                );
            }
            move_directory(target, candidate, previous.is_some())
        })();
        if let Err(rollback) = restored {
            let retained = stage.keep();
            return Err(eyre!(
                "{error:#}; Gradle rollback could not complete: {rollback:#}. Staging retained at {}",
                retained.display()
            ));
        }
        return Err(error);
    }
    if previous.is_some() {
        // The original is in the staging directory after an atomic exchange. Keep it for recovery.
        let retained = stage.keep();
        println!("Gradle: previous runtime retained at {}", retained.display());
    }
    Ok(())
}

fn run_update(root: &Path, link: &Path, transport: &Transport) -> std::result::Result<(), Failure> {
    let nightly = transport.metadata()?.validate()?;
    let java = java_home()?;
    let previous_launcher = read_launcher(link)?;
    if let Some(bytes) = &previous_launcher {
        launcher_version(bytes, root, &java)?;
    }
    let target = root.join(&nightly.version);
    let previous = match fs::symlink_metadata(&target) {
        Ok(_) => Some(directory_id(&target)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let contents = launcher_contents(&target.join("bin/gradle"), &java)?;
    if previous.is_some() && verify_runtime(&target, &nightly.version, &java).is_ok() {
        activate(link, previous_launcher.as_deref(), &contents)?;
        println!("Gradle Nightly is current: {}", nightly.version);
        return Ok(());
    }

    let stage = tempfile::Builder::new().prefix(".gradle-nightly-").tempdir_in(root)?;
    let zip = transport.download(&nightly, stage.path())?;
    let candidate = archive::extract(zip, &stage.path().join("extracted"), &nightly.version)?;
    verify_runtime(&candidate, &nightly.version, &java).wrap_err("Staged Gradle runtime failed verification")?;
    publish(stage, &candidate, &target, previous, || {
        File::open(root)?.sync_all()?;
        verify_runtime(&target, &nightly.version, &java).wrap_err("Installed Gradle runtime failed verification")?;
        activate(link, previous_launcher.as_deref(), &contents)
    })?;
    println!("Gradle Nightly ready: {}", nightly.version);
    Ok(())
}

fn verified_cached(root: &Path, link: &Path) -> Result<String> {
    let java = java_home()?;
    let launcher = read_launcher(link)?.ok_or_else(|| eyre!("No cached Gradle Nightly launcher"))?;
    let version = launcher_version(&launcher, root, &java)?;
    verify_runtime(&root.join(&version), &version, &java)?;
    ensure!(
        read_launcher(link)? == Some(launcher),
        "Cached Gradle Nightly launcher changed"
    );
    Ok(version)
}

pub(super) fn run() -> Result<()> {
    user_only()?;
    let home = account_home()?;
    let root = home.join(".local/opt/gradle-nightly");
    let bin = home.join(".local/bin");
    for path in [&home.join(".local"), &home.join(".local/opt"), &root, &bin] {
        directory(path)?;
    }
    let _lock = lock(&root)?;
    let link = bin.join("gradle-nightly");
    match run_update(&root, &link, &Transport::new()) {
        Ok(()) => Ok(()),
        Err(Failure::Fatal(error)) => Err(error),
        Err(Failure::Retry(problem)) => {
            match verified_cached(&root, &link) {
                Ok(version) => eprintln!(
                    "Gradle: CACHED_FALLBACK — upstream check failed; verified local runtime {version} remains selected."
                ),
                Err(error) => eprintln!("Gradle: cached runtime could not be verified: {error:#}"),
            }
            Err(eyre!("Gradle Nightly: RETRYABLE after 3 attempts ({problem})"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NIGHTLY: &str = "9.10.0-20261104002831+0000";

    #[test]
    fn launcher_preserves_legacy_shape_and_rejects_unmanaged_commands() -> Result<()> {
        let root = Path::new("/tmp/Gradle Test");
        let java = Path::new("/opt/homebrew/opt/openjdk/libexec/openjdk.jdk/Contents/Home");
        let bytes = launcher_contents(&root.join(NIGHTLY).join("bin/gradle"), java)?;
        assert_eq!(launcher_version(&bytes, root, java)?, NIGHTLY);
        assert!(launcher_version(b"#!/bin/sh\nexec /tmp/unreviewed", root, java).is_err());
        assert!(!valid_version("../9.10.0-20261104002831+0000"));
        Ok(())
    }

    #[test]
    fn launcher_activation_is_idempotent_and_detects_drift() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().canonicalize()?.join("gradle-nightly");
        activate(&path, None, b"first launcher")?;
        let inode = fs::metadata(&path)?.ino();
        activate(&path, Some(b"first launcher"), b"first launcher")?;
        assert_eq!(fs::metadata(&path)?.ino(), inode);
        assert!(activate(&path, Some(b"stale launcher"), b"replacement").is_err());
        assert_eq!(read_launcher(&path)?.as_deref(), Some(b"first launcher".as_slice()));
        Ok(())
    }

    #[test]
    fn atomic_same_version_repair_rolls_back_on_verification_failure() -> Result<()> {
        let root = tempfile::tempdir()?;
        let stage = tempfile::tempdir_in(root.path())?;
        let target = root.path().join(NIGHTLY);
        let candidate = stage.path().join("candidate");
        for (path, text) in [(&target, "previous"), (&candidate, "replacement")] {
            fs::create_dir(path)?;
            fs::write(path.join("version"), text)?;
        }
        let previous = directory_id(&target)?;
        let failed = publish(stage, &candidate, &target, Some(previous), || {
            assert_eq!(fs::read_to_string(target.join("version"))?, "replacement");
            Err(eyre!("verification failed"))
        });
        assert!(failed.is_err());
        assert_eq!(directory_id(&target)?, previous);
        assert_eq!(fs::read_to_string(target.join("version"))?, "previous");
        Ok(())
    }
}
