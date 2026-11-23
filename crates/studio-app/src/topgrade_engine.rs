//! Build the tracked Studio Topgrade component and atomically replace its only installation.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod profile;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod upstream;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod mac {
    use super::{profile, upstream};
    use anyhow::{Context, Result, bail, ensure};
    use serde::{Deserialize, Serialize};
    use std::{
        collections::BTreeMap,
        fs,
        io::{self, ErrorKind},
        os::fd::AsRawFd,
        os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
        path::{Path, PathBuf},
        process::{Command, Output},
        sync::atomic::{AtomicBool, Ordering},
    };
    use studio_deployment::maintenance::{
        account_home, acquire_engine_activation_lock, acquire_mutation_lock, sha256_file,
    };

    const PROFILE: &str = "profiles/macos";
    const RECEIPT_XATTR: &std::ffi::CStr = c"org.devenv-studio.topgrade-engine";
    const ASSETS: [&str; 5] = [
        ".config/topgrade.toml",
        ".config/topgrade.d/90-brew-headsync.toml",
        ".config/topgrade.d/90-skills-coverage.toml",
        ".config/topgrade.d/90-xcode-beta-select.toml",
        ".config/topgrade.d/95-skills-unattended.toml",
    ];
    static INTERRUPTED: AtomicBool = AtomicBool::new(false);

    #[derive(Deserialize)]
    struct Manifest {
        version: u32,
        files: Vec<Asset>,
    }

    #[derive(Deserialize)]
    struct Asset {
        destination: String,
        sha256: String,
    }

    #[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Receipt {
        schema_version: u32,
        studio_commit: String,
        component_tree: String,
        binary_sha256: String,
        profile_sha256: String,
        profile_version: u32,
        assets: BTreeMap<String, String>,
    }

    struct Prepared {
        binary: PathBuf,
        receipt: Receipt,
        profile: Option<PathBuf>,
    }

    fn hexadecimal(value: &str, size: usize) -> bool {
        value.len() == size
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }

    extern "C" fn signal(_: libc::c_int) {
        INTERRUPTED.store(true, Ordering::Relaxed);
    }

    fn interrupted() -> Result<()> {
        ensure!(
            !INTERRUPTED.load(Ordering::Relaxed),
            "Topgrade update interrupted"
        );
        Ok(())
    }

    fn handle_signals() -> Result<()> {
        // SAFETY: The handler only stores to a lock-free atomic flag.
        unsafe {
            for number in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
                let old = libc::signal(number, signal as *const () as libc::sighandler_t);
                ensure!(
                    old != libc::SIG_ERR,
                    "Could not watch for updater termination"
                );
            }
        }
        Ok(())
    }

    fn output(command: &mut Command, action: &str) -> Result<Output> {
        let result = command.output().with_context(|| action.to_owned())?;
        ensure!(
            result.status.success(),
            "{action} failed ({}): {}",
            result.status,
            String::from_utf8_lossy(&result.stderr).trim()
        );
        Ok(result)
    }

    fn status(command: &mut Command, action: &str) -> Result<()> {
        interrupted()?;
        ensure!(
            command
                .status()
                .with_context(|| action.to_owned())?
                .success(),
            "{action} failed"
        );
        interrupted()
    }

    fn git_command(checkout: &Path) -> Command {
        let mut command = Command::new("/usr/bin/git");
        command.current_dir(checkout).arg("--no-replace-objects");
        command
    }

    pub(super) fn git(checkout: &Path, args: &[&str]) -> Result<String> {
        let result = output(
            git_command(checkout).args(args),
            "Inspect Studio tracking branch",
        )?;
        Ok(String::from_utf8(result.stdout)?.trim().to_owned())
    }

    fn clean(checkout: &Path) -> Result<()> {
        ensure!(
            git(
                checkout,
                &["status", "--porcelain=v1", "--untracked-files=all"]
            )?
            .is_empty(),
            "The Studio checkout has local changes; preserve them before updating"
        );
        let state = git(
            checkout,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                "MERGE_HEAD",
                "--git-path",
                "CHERRY_PICK_HEAD",
                "--git-path",
                "REVERT_HEAD",
                "--git-path",
                "rebase-merge",
                "--git-path",
                "rebase-apply",
                "--git-path",
                "sequencer",
            ],
        )?;
        for path in state.lines() {
            ensure!(
                entry(Path::new(path))?.is_none(),
                "The Studio checkout has an unfinished Git operation"
            );
        }
        Ok(())
    }

    fn tracked(checkout: &Path, refresh: bool) -> Result<(String, String)> {
        ensure!(
            checkout.is_absolute()
                && checkout.canonicalize()? == checkout
                && checkout.to_str()
                    == Some(git(checkout, &["rev-parse", "--show-toplevel"])?.as_str()),
            "Expected a canonical absolute Studio checkout"
        );
        clean(checkout)?;
        let branch = git(checkout, &["symbolic-ref", "--quiet", "HEAD"])?;
        let remote = git(
            checkout,
            &["for-each-ref", "--format=%(upstream:remotename)", &branch],
        )?;
        ensure!(
            !remote.is_empty() && !remote.starts_with('-'),
            "The Studio branch has no tracking remote"
        );
        let upstream = git(
            checkout,
            &["rev-parse", "--symbolic-full-name", "@{upstream}"],
        )?;
        let remote_ref = git(
            checkout,
            &["for-each-ref", "--format=%(upstream:remoteref)", &branch],
        )?;
        ensure!(
            remote_ref.starts_with("refs/heads/"),
            "Expected a tracked Studio branch"
        );
        let head = git(checkout, &["rev-parse", "HEAD^{commit}"])?;
        if refresh {
            status(
                git_command(checkout).args([
                    "fetch",
                    "--no-tags",
                    "--no-recurse-submodules",
                    &remote,
                    &remote_ref,
                ]),
                "Fetch the configured Studio tracking remote",
            )?;
            clean(checkout)?;
            ensure!(
                git(checkout, &["symbolic-ref", "--quiet", "HEAD"])? == branch
                    && git(
                        checkout,
                        &["rev-parse", "--symbolic-full-name", "@{upstream}"]
                    )? == upstream
                    && git(checkout, &["rev-parse", "HEAD^{commit}"])? == head,
                "The checkout or tracking branch changed during fetch"
            );
        }
        let revision = git(
            checkout,
            &["rev-parse", "--verify", &format!("{upstream}^{{commit}}")],
        )?;
        ensure!(hexadecimal(&revision, 40), "Invalid tracked Studio commit");
        if refresh {
            ensure!(
                git(checkout, &["rev-parse", "FETCH_HEAD^{commit}"])? == revision,
                "Configured tracking ref does not match the fetched Studio branch"
            );
        }
        output(
            git_command(checkout).args(["merge-base", "--is-ancestor", &head, &revision]),
            "Require the tracked Studio revision to be a fast-forward",
        )?;
        if refresh && head != revision {
            status(
                git_command(checkout).args(["merge", "--ff-only", "--no-autostash", &revision]),
                "Fast-forward the clean Studio checkout",
            )?;
            clean(checkout)?;
            ensure!(
                git(checkout, &["rev-parse", "HEAD"])? == revision,
                "Studio HEAD changed during update"
            );
        }
        let tree = git(
            checkout,
            &["rev-parse", &format!("{revision}:components/topgrade")],
        )?;
        ensure!(hexadecimal(&tree, 40), "Invalid Topgrade component tree");
        Ok((revision, tree))
    }

    fn entry(path: &Path) -> Result<Option<fs::Metadata>> {
        match path.symlink_metadata() {
            Ok(meta) => Ok(Some(meta)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn checked_launcher(home: &Path) -> Result<PathBuf> {
        let uid = home.metadata()?.uid();
        let mut directory = home.to_path_buf();
        for name in [".local", "bin"] {
            directory.push(name);
            match fs::DirBuilder::new().mode(0o700).create(&directory) {
                Ok(()) => (),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => (),
                Err(error) => return Err(error.into()),
            }
            let metadata = directory.symlink_metadata()?;
            ensure!(
                metadata.is_dir()
                    && metadata.uid() == uid
                    && metadata.mode() & 0o022 == 0
                    && directory.canonicalize()? == directory,
                "Expected a real user-owned launcher directory: {}",
                directory.display()
            );
        }
        Ok(directory.join("topgrade"))
    }

    fn binary_file(path: &Path, uid: u32) -> Result<fs::File> {
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        let meta = file.metadata()?;
        ensure!(
            meta.is_file()
                && meta.uid() == uid
                && meta.nlink() == 1
                && meta.mode() & 0o111 != 0
                && meta.mode() & 0o022 == 0,
            "Unsafe Topgrade executable: {}",
            path.display()
        );
        Ok(file)
    }

    fn read_receipt(file: &fs::File) -> Result<Option<Receipt>> {
        let mut bytes = vec![0u8; 64 * 1024];
        // SAFETY: The open descriptor and the writable buffer stay live for the syscall.
        let size = unsafe {
            libc::fgetxattr(
                file.as_raw_fd(),
                RECEIPT_XATTR.as_ptr(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
                0,
                0,
            )
        };
        if size < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ENOATTR) {
                return Ok(None);
            }
            return Err(error).context("Read Topgrade executable receipt");
        }
        bytes.truncate(usize::try_from(size)?);
        Ok(Some(
            serde_json::from_slice(&bytes).context("Invalid Topgrade executable receipt")?,
        ))
    }

    fn write_receipt(file: &fs::File, receipt: &Receipt) -> Result<()> {
        let bytes = serde_json::to_vec(receipt)?;
        // SAFETY: The open descriptor and the immutable byte buffer stay live for the syscall.
        let result = unsafe {
            libc::fsetxattr(
                file.as_raw_fd(),
                RECEIPT_XATTR.as_ptr(),
                bytes.as_ptr().cast(),
                bytes.len(),
                0,
                libc::XATTR_CREATE,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error())
                .context("Write staged Topgrade executable receipt");
        }
        ensure!(
            read_receipt(file)?.as_ref() == Some(receipt),
            "Staged Topgrade receipt failed readback"
        );
        Ok(())
    }

    fn receipt(binary: &Path, revision: &str, tree: &str, manifest: &Path) -> Result<Receipt> {
        let data: Manifest = serde_json::from_slice(&fs::read(manifest)?)?;
        ensure!(
            data.version == 1,
            "Invalid Topgrade profile manifest version"
        );
        let assets = ASSETS
            .iter()
            .map(|asset| {
                let mut matches = data.files.iter().filter(|row| row.destination == *asset);
                let row = matches
                    .next()
                    .with_context(|| format!("Missing profile asset {asset}"))?;
                ensure!(
                    matches.next().is_none() && hexadecimal(&row.sha256, 64),
                    "Invalid profile asset {asset}"
                );
                Ok(((*asset).to_owned(), row.sha256.clone()))
            })
            .collect::<Result<_>>()?;
        Ok(Receipt {
            schema_version: 1,
            studio_commit: revision.to_owned(),
            component_tree: tree.to_owned(),
            binary_sha256: sha256_file(binary)?,
            profile_sha256: sha256_file(manifest)?,
            profile_version: data.version,
            assets,
        })
    }

    fn export_tracked_source(checkout: &Path, revision: &str, stage: &Path) -> Result<PathBuf> {
        let source = stage.join("source");
        fs::create_dir(&source)?;
        let archive = stage.join("source.tar");
        status(
            git_command(checkout)
                .args(["archive", "--format=tar", "--output"])
                .arg(&archive)
                .arg(revision),
            "Export tested objects from the exact Studio commit",
        )?;
        status(
            Command::new("/usr/bin/tar")
                .arg("-xf")
                .arg(&archive)
                .arg("-C")
                .arg(&source),
            "Extract the tracked Studio source",
        )?;
        Ok(source)
    }

    fn prepare(
        checkout: &Path,
        revision: &str,
        tree: &str,
        stage: &Path,
        home: &Path,
    ) -> Result<Prepared> {
        let source = export_tracked_source(checkout, revision, stage)?;
        status(
            Command::new("/opt/homebrew/bin/brew")
                .current_dir(&source)
                .args(["ruby", "scripts/test_jpeg_xl_transaction.rb"])
                .env("HOMEBREW_NO_AUTO_UPDATE", "1")
                .env("HOMEBREW_NO_INSTALL_FROM_API", "1"),
            "Test the Homebrew dependency transaction",
        )?;
        let component = source.join("components/topgrade");
        let target = stage.join("target");
        let run_cargo = |args: &[&str], action| -> Result<()> {
            status(
                Command::new(home.join(".cargo/bin/cargo"))
                    .current_dir(&source)
                    .arg("+nightly")
                    .args(args)
                    .args(["--locked", "--package", "topgrade", "--target-dir"])
                    .arg(&target)
                    .env("CARGO_BUILD_BUILD_DIR", &target)
                    .env("HOMEBREW_NO_AUTO_UPDATE", "1"),
                action,
            )
        };
        run_cargo(&["test"], "Test the tracked Topgrade component")?;
        run_cargo(
            &[
                "build",
                "--profile",
                "topgrade-release",
                "--bin",
                "topgrade",
            ],
            "Build Topgrade with Rustup Nightly",
        )?;
        let compiled = target.join("topgrade-release/topgrade");
        let binary = stage.join("topgrade");
        // Cargo can hard-link its output into the artifact directory.
        fs::copy(&compiled, &binary)?;
        ensure!(
            sha256_file(&compiled)? == sha256_file(&binary)?,
            "Staged Topgrade executable differs from the compiler output"
        );
        let profile = component.join(PROFILE);
        let manifest = profile.join("manifest.json");
        let blob = git(
            checkout,
            &["rev-parse", &format!("{tree}:{PROFILE}/manifest.json")],
        )?;
        ensure!(
            git(
                checkout,
                &[
                    "hash-object",
                    "--no-filters",
                    manifest.to_str().context("Invalid profile path")?
                ]
            )? == blob,
            "Staged profile manifest differs from the tracked component"
        );
        ensure!(
            tracked(checkout, false)? == (revision.to_owned(), tree.to_owned()),
            "Studio tracking state changed during the Topgrade build"
        );
        Ok(Prepared {
            receipt: receipt(&binary, revision, tree, &manifest)?,
            binary,
            profile: Some(profile),
        })
    }

    fn same_entry(old: Option<&fs::Metadata>, now: Option<&fs::Metadata>) -> bool {
        match (old, now) {
            (Some(a), Some(b)) => {
                a.dev() == b.dev()
                    && a.ino() == b.ino()
                    && a.mode() == b.mode()
                    && a.ctime() == b.ctime()
                    && a.ctime_nsec() == b.ctime_nsec()
            }
            (None, None) => true,
            _ => false,
        }
    }

    fn active_receipt(launcher: &Path, uid: u32) -> Result<Option<Receipt>> {
        if entry(launcher)?.is_none_or(|meta| meta.is_symlink()) {
            return Ok(None);
        }
        let file = binary_file(launcher, uid)?;
        let receipt = read_receipt(&file)?;
        match receipt {
            Some(receipt)
                if receipt.schema_version == 1
                    && sha256_file(launcher)? == receipt.binary_sha256 =>
            {
                Ok(Some(receipt))
            }
            _ => Ok(None),
        }
    }

    fn install_rolling(
        home: &Path,
        revision: &str,
        tree: &str,
        build: impl FnOnce(&Path) -> Result<Prepared>,
    ) -> Result<bool> {
        let launcher = checked_launcher(home)?;
        let uid = home.metadata()?.uid();
        let original = entry(&launcher)?;
        ensure!(
            original
                .as_ref()
                .is_none_or(|meta| meta.uid() == uid && (meta.is_file() || meta.is_symlink())),
            "Unsafe installed Topgrade entry"
        );
        if active_receipt(&launcher, uid)?
            .is_some_and(|r| r.studio_commit == revision && r.component_tree == tree)
        {
            return Ok(false);
        }

        let parent = launcher.parent().context("Missing launcher parent")?;
        let stage = tempfile::Builder::new()
            .prefix(".studio-topgrade-")
            .tempdir_in(parent)?;
        let prepared = build(stage.path())?;
        interrupted()?;
        let binary = &prepared.binary;
        ensure!(
            binary.starts_with(stage.path()) && binary.canonicalize()? == *binary,
            "Prepared executable is outside staging"
        );
        let file = binary_file(binary, uid)?;
        ensure!(
            prepared.receipt.schema_version == 1
                && prepared.receipt.studio_commit == revision
                && prepared.receipt.component_tree == tree
                && sha256_file(binary)? == prepared.receipt.binary_sha256,
            "Staged Topgrade executable failed integrity verification"
        );
        let version = output(
            Command::new(binary).arg("--version"),
            "Verify candidate CLI version",
        )?;
        ensure!(
            version.stdout.starts_with(b"topgrade "),
            "Unexpected staged Topgrade version"
        );
        let reference = output(
            Command::new(binary).arg("--config-reference"),
            "Verify candidate CLI startup",
        )?;
        ensure!(
            !reference.stdout.is_empty(),
            "No configuration reference from staged Topgrade"
        );
        write_receipt(&file, &prepared.receipt)?;
        file.sync_all()?;
        let parent_directory = fs::File::open(parent)?;
        let verify_entry = || -> Result<()> {
            interrupted()?;
            ensure!(
                same_entry(original.as_ref(), entry(&launcher)?.as_ref()),
                "Topgrade entry changed during the update"
            );
            Ok(())
        };
        verify_entry()?;

        let profile = prepared.profile.as_deref();
        let transaction = if let Some(source) = profile {
            let applied = profile::execute(source, home, &["--apply"])?;
            match applied["status"].as_str() {
                Some("installed") => Some(
                    applied["transaction"]
                        .as_str()
                        .context("Missing installed profile transaction")?
                        .to_owned(),
                ),
                Some("already-current") => None,
                _ => bail!("Unexpected profile transaction status"),
            }
        } else {
            None
        };
        let activation = (|| -> Result<()> {
            if let Some(source) = profile {
                profile::verify_installed(source, home)?;
            }
            verify_entry()?;
            fs::rename(binary, &launcher).context("Atomically replace the Topgrade executable")
        })();
        if let Err(error) = activation {
            if let (Some(source), Some(id)) = (profile, transaction.as_deref())
                && let Err(recovery) = profile::execute(source, home, &["--rollback", id])
            {
                return Err(error.context(format!("Profile recovery {id} failed: {recovery:#}")));
            }
            return Err(error);
        }
        if let Err(error) = parent_directory.sync_all() {
            eprintln!("Topgrade is active, but launcher directory synchronization failed: {error}");
        }
        Ok(true)
    }

    pub(super) fn execute(args: &[String]) -> Result<()> {
        let [mode, checkout] = args else {
            bail!(
                "Usage: devenv-studio topgrade-engine <plan|update|verify-current|check-upstream|record-upstream|export-upstream-agent> <absolute checkout>"
            );
        };
        let checkout = Path::new(checkout);
        if matches!(
            mode.as_str(),
            "check-upstream" | "record-upstream" | "export-upstream-agent"
        ) {
            return upstream::execute(mode, checkout);
        }
        ensure!(
            matches!(mode.as_str(), "plan" | "update" | "verify-current"),
            "Unknown engine maintenance action"
        );
        let home = account_home()?;
        if mode == "plan" {
            let (revision, tree) = tracked(checkout, false)?;
            println!(
                "Tracked Studio commit {revision}, Topgrade tree {tree}; activation path: {}",
                home.join(".local/bin/topgrade").display()
            );
            return Ok(());
        }
        if mode == "verify-current" {
            let (revision, tree) = tracked(checkout, false)?;
            let launcher = home.join(".local/bin/topgrade");
            let identity = studio_deployment::maintenance::verified_engine(&home, &launcher)?;
            ensure!(
                identity.studio_commit == revision && identity.component_tree == tree,
                "Installed Topgrade differs from the tracked Studio commit"
            );
            println!(
                "Verified {} (Studio {revision}, tree {tree}, SHA-256 {})",
                launcher.display(),
                identity.binary_sha256
            );
            return Ok(());
        }
        handle_signals()?;
        let _lock = acquire_mutation_lock()?;
        let _activation = acquire_engine_activation_lock()?;
        let pending = home.join(".local/share/topgrade-rustup-head/studio-activation.pending.json");
        ensure!(
            entry(&pending)?.is_none(),
            "Pending engine activation needs review at {}",
            pending.display()
        );
        let (revision, tree) = tracked(checkout, true)?;
        let launcher = checked_launcher(&home)?;
        if let Some(active) = active_receipt(&launcher, home.metadata()?.uid())? {
            ensure!(
                hexadecimal(&active.studio_commit, 40),
                "Invalid installed Topgrade provenance"
            );
            output(
                git_command(checkout).args([
                    "merge-base",
                    "--is-ancestor",
                    &active.studio_commit,
                    &revision,
                ]),
                "Require the tracked Studio commit to descend from the installed engine",
            )?;
        }
        if install_rolling(&home, &revision, &tree, |stage| {
            prepare(checkout, &revision, &tree, stage, &home)
        })? {
            println!(
                "Activated {} from Studio {revision}",
                home.join(".local/bin/topgrade").display()
            );
        } else {
            profile::verify_installed(&checkout.join("components/topgrade").join(PROFILE), &home)?;
            println!("Topgrade and its profile are current at Studio {revision}");
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests;
}

pub(super) fn execute(args: &[String]) -> anyhow::Result<()> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        mac::execute(args)
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = args;
        anyhow::bail!("Topgrade engine maintenance is available on Apple Silicon macOS")
    }
}
