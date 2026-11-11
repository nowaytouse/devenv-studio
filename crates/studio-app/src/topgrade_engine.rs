//! Local, revision-pinned maintenance of the independent Topgrade engine.
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
        os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, symlink},
        path::{Path, PathBuf},
        process::{Command, Output},
        time::{SystemTime, UNIX_EPOCH},
    };
    use studio_deployment::maintenance::{
        account_home, acquire_engine_activation_lock, acquire_mutation_lock, sha256_file,
        verified_engine_release,
    };

    const PROFILE: &str = "profiles/macos";
    const ASSETS: [&str; 5] = [
        ".config/topgrade.toml",
        ".config/topgrade.d/90-brew-headsync.toml",
        ".config/topgrade.d/90-skills-coverage.toml",
        ".config/topgrade.d/90-xcode-beta-select.toml",
        ".config/topgrade.d/95-skills-unattended.toml",
    ];

    #[derive(Deserialize)]
    struct Manifest {
        files: Vec<Asset>,
    }

    #[derive(Deserialize)]
    struct Asset {
        destination: String,
        sha256: String,
    }

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Receipt {
        schema_version: u32,
        studio_commit: String,
        component_tree: String,
        binary_sha256: String,
        profile_sha256: String,
        assets: BTreeMap<String, String>,
    }

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Activation {
        schema_version: u32,
        previous: PathBuf,
        target: PathBuf,
        profile_transaction: Option<String>,
    }

    struct Stage(PathBuf);

    impl Drop for Stage {
        fn drop(&mut self) {
            if let Err(error) = fs::remove_dir_all(&self.0) {
                eprintln!("Temporary stage retained at {}: {error}", self.0.display());
            }
        }
    }

    fn hexadecimal(value: &str, size: usize) -> bool {
        value.len() == size
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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
        ensure!(
            command
                .status()
                .with_context(|| action.to_owned())?
                .success(),
            "{action} failed"
        );
        Ok(())
    }

    pub(super) fn git(checkout: &Path, args: &[&str]) -> Result<String> {
        let result = output(
            Command::new("/usr/bin/git")
                .current_dir(checkout)
                .arg("--no-replace-objects")
                .args(args),
            "Inspect reviewed Studio revision",
        )?;
        Ok(String::from_utf8(result.stdout)?.trim().to_owned())
    }

    fn new_stage(parent: &Path) -> Result<Stage> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = parent.join(format!(".studio-stage-{}-{nonce}", std::process::id()));
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Stage(path))
    }

    fn regular(path: &Path) -> Result<()> {
        ensure!(
            path.symlink_metadata()?.is_file() && path.canonicalize()? == path,
            "Expected a regular file without symlinks: {}",
            path.display()
        );
        Ok(())
    }

    fn optional(path: &Path) -> Result<bool> {
        match path.symlink_metadata() {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        serde_json::to_writer_pretty(&mut file, value)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::File::open(path.parent().context("Missing journal parent")?)?.sync_all()?;
        Ok(())
    }

    fn verify_profile(checkout: &Path, tree: &str, release: &Path) -> Result<()> {
        let path = release.join("profile/manifest.json");
        regular(&path)?;
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
                    path.to_str().context("Profile path is not UTF-8")?
                ]
            )? == blob,
            "Prepared profile manifest differs from the pinned component"
        );
        ensure!(
            sha256_file(&release.join("profile").join("manifest.json"))?
                == sha256_file(&release.join("profile-manifest.json"))?,
            "Prepared profile manifest differs from the engine receipt"
        );
        Ok(())
    }

    fn asset_map(manifest: &Path) -> Result<BTreeMap<String, String>> {
        regular(manifest)?;
        let manifest: Manifest = serde_json::from_slice(&fs::read(manifest)?)?;
        ASSETS
            .iter()
            .map(|asset| {
                let mut matches = manifest
                    .files
                    .iter()
                    .filter(|row| row.destination == *asset);
                let row = matches
                    .next()
                    .with_context(|| format!("Missing profile asset {asset}"))?;
                ensure!(
                    matches.next().is_none() && hexadecimal(&row.sha256, 64),
                    "Invalid profile asset {asset}"
                );
                Ok(((*asset).to_owned(), row.sha256.clone()))
            })
            .collect()
    }

    fn prepare(
        checkout: &Path,
        revision: &str,
        tree: &str,
        release: &Path,
        home: &Path,
    ) -> Result<()> {
        if optional(release)? {
            verify_release(release, tree)?;
            verify_profile(checkout, tree, release)?;
            return status(
                Command::new(release.join("bin/topgrade")).arg("--version"),
                "Verify prepared Topgrade CLI",
            );
        }
        let stage = new_stage(release.parent().context("Missing release parent")?)?;
        let source = stage.0.join("source");
        fs::create_dir(&source)?;
        let archive = stage.0.join("component.tar");
        status(
            Command::new("/usr/bin/git")
                .current_dir(checkout)
                .args(["archive", "--format=tar", "--output"])
                .arg(&archive)
                .arg(tree),
            "Export the exact Topgrade component tree",
        )?;
        status(
            Command::new("/usr/bin/tar")
                .arg("-xf")
                .arg(&archive)
                .arg("-C")
                .arg(&source),
            "Extract the reviewed component",
        )?;
        let candidate = stage.0.join("release");
        status(
            Command::new(home.join(".cargo/bin/cargo"))
                .current_dir(&source)
                .args(["+nightly", "install", "--locked", "--path", ".", "--root"])
                .arg(&candidate)
                .env("HOMEBREW_NO_AUTO_UPDATE", "1"),
            "Build Topgrade with the existing Rustup Nightly",
        )?;
        let binary = candidate.join("bin/topgrade");
        regular(&binary)?;
        status(
            Command::new(&binary).arg("--version"),
            "Verify candidate CLI version",
        )?;
        status(
            Command::new(&binary)
                .arg("--config-reference")
                .stdout(std::process::Stdio::null()),
            "Verify candidate CLI startup",
        )?;
        let manifest = candidate.join("profile-manifest.json");
        fs::rename(source.join(PROFILE), candidate.join("profile"))?;
        fs::copy(candidate.join("profile").join("manifest.json"), &manifest)?;
        fs::File::open(&manifest)?.sync_all()?;
        fs::File::open(&binary)?.sync_all()?;
        let receipt = Receipt {
            schema_version: 1,
            studio_commit: revision.to_owned(),
            component_tree: tree.to_owned(),
            binary_sha256: sha256_file(&binary)?,
            profile_sha256: sha256_file(&manifest)?,
            assets: asset_map(&manifest)?,
        };
        write_json(&candidate.join("studio-engine.json"), &receipt)?;
        verify_profile(checkout, tree, &candidate)?;
        fs::rename(&candidate, release)?;
        fs::File::open(release.parent().context("Missing release parent")?)?.sync_all()?;
        verify_release(release, tree)?;
        Ok(())
    }

    fn verify_release(release: &Path, tree: &str) -> Result<()> {
        let receipt_path = release.join("studio-engine.json");
        regular(&receipt_path)?;
        let receipt: Receipt = serde_json::from_slice(&fs::read(receipt_path)?)?;
        let manifest = release.join("profile-manifest.json");
        let binary = release.join("bin/topgrade");
        regular(&binary)?;
        let binary_metadata = binary.metadata()?;
        ensure!(
            binary_metadata.mode() & 0o111 != 0
                && binary_metadata.mode() & 0o022 == 0
                && binary_metadata.nlink() == 1
                && binary_metadata.uid() == account_home()?.metadata()?.uid(),
            "Unsafe prepared executable"
        );
        ensure!(
            receipt.schema_version == 1
                && receipt.component_tree == tree
                && hexadecimal(&receipt.studio_commit, 40)
                && sha256_file(&binary)? == receipt.binary_sha256
                && sha256_file(&manifest)? == receipt.profile_sha256
                && asset_map(&manifest)? == receipt.assets,
            "Prepared release failed integrity verification"
        );
        Ok(())
    }

    fn managed_target(path: &Path, releases: &Path) -> Result<()> {
        let relative = path
            .strip_prefix(releases)
            .context("Topgrade launcher points outside managed releases")?;
        let parts: Vec<_> = relative.iter().collect();
        ensure!(
            parts.len() == 3
                && parts[0]
                    .to_str()
                    .is_some_and(|value| hexadecimal(value, 40))
                && parts[1] == "bin"
                && parts[2] == "topgrade",
            "Invalid managed launcher target"
        );
        regular(path)
    }

    fn checked_launcher(home: &Path) -> Result<PathBuf> {
        let directory = home.join(".local/bin");
        let metadata = directory.metadata()?;
        ensure!(
            directory.canonicalize()? == directory
                && metadata.is_dir()
                && metadata.uid() == home.metadata()?.uid()
                && metadata.mode() & 0o022 == 0,
            "Expected a real user-owned Topgrade launcher directory"
        );
        Ok(directory.join("topgrade"))
    }

    fn swap_link(link: &Path, expected: &Path, target: &Path) -> Result<()> {
        ensure!(
            fs::read_link(link)? == expected,
            "Topgrade launcher changed since review"
        );
        let stage = new_stage(link.parent().context("Missing launcher parent")?)?;
        let pending_link = stage.0.join("topgrade");
        symlink(target, &pending_link)?;
        fs::rename(&pending_link, link)?;
        fs::File::open(link.parent().context("Missing launcher parent")?)?.sync_all()?;
        Ok(())
    }

    fn require_topgrade_idle() -> Result<()> {
        let probe = Command::new("/usr/bin/pgrep")
            .args(["-x", "topgrade"])
            .output()?;
        ensure!(
            probe.status.code() == Some(1),
            "Topgrade must be idle before switching the active launcher"
        );
        Ok(())
    }

    fn profile(release: &Path, home: &Path, args: &[&str]) -> Result<serde_json::Value> {
        profile::execute(&release.join("profile"), home, args)
            .context("Topgrade profile transaction")
    }

    fn restore_activation(home: &Path, tree: &str, expected_previous: Option<&str>) -> Result<()> {
        let base = home.join(".local/share/topgrade-rustup-head");
        let releases = base.join("releases");
        let link = checked_launcher(home)?;
        let target = releases.join(tree).join("bin/topgrade");
        let pending = base.join("studio-activation.pending.json");
        let journal = base.join(format!("studio-activation-{tree}.json"));
        ensure!(
            fs::read_link(&link)? == target && !optional(&pending)?,
            "The reviewed engine is no longer active or a pending activation needs recovery"
        );
        regular(&journal)?;
        ensure!(
            journal.metadata()?.mode() & 0o077 == 0,
            "Expected a private activation receipt"
        );
        let saved: Activation =
            profile::read_json(&journal)?.context("Missing current engine activation")?;
        ensure!(
            saved.schema_version == 1 && saved.target == target && saved.previous != target,
            "This activation is not current or has no distinct rollback target"
        );
        let current = verified_engine_release(home, &target)?;
        let previous = verified_engine_release(home, &saved.previous)?;
        ensure!(
            expected_previous.is_none_or(|tree| tree == previous.component_tree),
            "The rollback target changed since review"
        );
        ensure!(
            saved.profile_transaction.is_some()
                || current.profile_sha256 == previous.profile_sha256,
            "Different profiles require a recorded rollback transaction"
        );
        let current_profile = releases.join(&current.component_tree).join("profile");
        let previous_profile = releases.join(&previous.component_tree).join("profile");
        let (state, _profile_lock) = profile::lock(home)?;
        profile::rollback_activation(
            &current_profile,
            &previous_profile,
            home,
            &state,
            saved.profile_transaction.as_deref(),
            || write_json(&pending, &saved),
        )?;
        swap_link(&link, &target, &saved.previous)?;
        let restored = verified_engine_release(home, &link.canonicalize()?)?;
        ensure!(
            fs::read_link(&link)? == saved.previous && restored == previous,
            "Restored launcher or receipt changed during rollback"
        );
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        fs::rename(
            &journal,
            base.join(format!("studio-rollback-{tree}-{timestamp}.json")),
        )?;
        fs::remove_file(&pending)?;
        fs::File::open(&base)?.sync_all()?;
        println!(
            "Restored Topgrade {} (Studio {}, tree {}, binary SHA-256 {}) and verified installed profile {} (SHA-256 {})",
            restored.upstream_version,
            restored.studio_commit,
            restored.component_tree,
            restored.binary_sha256,
            restored.profile_version,
            restored.profile_sha256,
        );
        Ok(())
    }

    fn rollback_current(tree: &str, previous_tree: &str) -> Result<()> {
        ensure!(
            hexadecimal(tree, 40) && hexadecimal(previous_tree, 40) && tree != previous_tree,
            "Supply the distinct full active and previous component tree SHAs"
        );
        let home = account_home()?;
        let base = home.join(".local/share/topgrade-rustup-head");
        ensure!(
            base.canonicalize()? == base,
            "Unexpected managed engine directory"
        );
        let _lock = acquire_mutation_lock()?;
        let _activation = acquire_engine_activation_lock()?;
        require_topgrade_idle()?;
        restore_activation(&home, tree, Some(previous_tree))
    }

    fn verify_current(tree: &str) -> Result<()> {
        ensure!(
            hexadecimal(tree, 40),
            "Supply the full active component tree SHA"
        );
        let home = account_home()?;
        let base = home.join(".local/share/topgrade-rustup-head");
        let release = base.join("releases").join(tree);
        let target = release.join("bin/topgrade");
        let link = checked_launcher(&home)?;
        let pending = base.join("studio-activation.pending.json");
        let current = || -> Result<()> {
            ensure!(
                fs::read_link(&link)? == target && !optional(&pending)?,
                "The reviewed engine is no longer active or a pending activation needs recovery"
            );
            Ok(())
        };
        current()?;
        let identity = verified_engine_release(&home, &target)?;
        let packaged = release.join("profile");
        let count = profile::verify_installed(&packaged, &home)?;
        ensure!(
            sha256_file(&packaged.join("manifest.json"))? == identity.profile_sha256,
            "Packaged profile changed during verification"
        );
        current()?;
        println!(
            "Verified Topgrade {} (Studio {}, tree {}, binary SHA-256 {}) and installed profile {} ({} assets, SHA-256 {})",
            identity.upstream_version,
            identity.studio_commit,
            identity.component_tree,
            identity.binary_sha256,
            identity.profile_version,
            count,
            identity.profile_sha256,
        );
        Ok(())
    }

    pub(super) fn execute(args: &[String]) -> Result<()> {
        if let [mode, value] = args {
            match mode.as_str() {
                "verify-current" => return verify_current(value),
                "check-upstream" | "record-upstream" | "export-upstream-agent" => {
                    return upstream::execute(mode, Path::new(value));
                }
                _ => {}
            }
        }
        let [mode, checkout_arg, revision] = args else {
            bail!(
                "Usage: devenv-studio topgrade-engine <plan|prepare|prepare-upstream|install|rollback> <absolute checkout> <full commit>\n       devenv-studio topgrade-engine <check-upstream|record-upstream|export-upstream-agent> <absolute checkout>\n       devenv-studio topgrade-engine rollback-current <active component tree> <previous component tree>\n       devenv-studio topgrade-engine verify-current <active component tree>"
            );
        };
        if mode == "rollback-current" {
            return rollback_current(checkout_arg, revision);
        }
        ensure!(
            matches!(
                mode.as_str(),
                "plan" | "prepare" | "prepare-upstream" | "install" | "rollback"
            ),
            "Unknown engine maintenance action"
        );
        ensure!(
            hexadecimal(revision, 40),
            "Supply the reviewed full Studio commit SHA"
        );
        let checkout = Path::new(checkout_arg);
        ensure!(
            checkout.is_absolute() && checkout.canonicalize()? == checkout,
            "Expected a canonical absolute checkout"
        );
        ensure!(
            git(checkout, &["rev-parse", "--show-toplevel"])? == checkout_arg.as_str()
                && git(checkout, &["rev-parse", "HEAD"])? == *revision
                && git(
                    checkout,
                    &["status", "--porcelain=v1", "--untracked-files=all"]
                )?
                .is_empty(),
            "Expected a clean Studio checkout at the reviewed commit"
        );
        let tree = git(
            checkout,
            &["rev-parse", &format!("{revision}:components/topgrade")],
        )?;
        ensure!(hexadecimal(&tree, 40), "Invalid Topgrade component tree");
        if mode == "prepare-upstream" {
            upstream::verify_current(checkout, revision)?;
        }
        let home = account_home()?;
        let base = home.join(".local/share/topgrade-rustup-head");
        let releases = base.join("releases");
        let release = releases.join(&tree);
        let link = checked_launcher(&home)?;
        let previous =
            fs::read_link(&link).context("Expected the existing managed Topgrade launcher")?;
        managed_target(&previous, &releases)?;
        ensure!(
            base.canonicalize()? == base && releases.canonicalize()? == releases,
            "Unexpected managed engine directories"
        );
        let target = release.join("bin/topgrade");
        let journal = base.join(format!("studio-activation-{tree}.json"));
        let pending = base.join("studio-activation.pending.json");
        ensure!(
            !optional(&pending)?,
            "Pending engine activation needs review at {}",
            pending.display()
        );
        if mode == "plan" {
            println!("Studio {revision}, Topgrade tree {tree}");
            println!("Current engine: {}", previous.display());
            println!("Candidate: {}", target.display());
            if optional(&release)? {
                verify_release(&release, &tree)?;
                verify_profile(checkout, &tree, &release)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&profile(&release, &home, &[])?)?
                );
            } else {
                println!("Prepare the verified release to inspect its profile plan.");
            }
            return Ok(());
        }
        let _lock = acquire_mutation_lock()?;
        let _activation = acquire_engine_activation_lock()?;
        ensure!(
            fs::read_link(&link)? == previous && !optional(&pending)?,
            "Engine changed while acquiring the maintenance lock"
        );
        if mode == "rollback" {
            require_topgrade_idle()?;
            verify_release(&release, &tree)?;
            verify_profile(checkout, &tree, &release)?;
            return restore_activation(&home, &tree, None);
        }
        prepare(checkout, revision, &tree, &release, &home)?;
        if matches!(mode.as_str(), "prepare" | "prepare-upstream") {
            println!("Prepared verified engine at {}", target.display());
            return Ok(());
        }
        if previous == target {
            profile(&release, &home, &["--verify"])?;
            println!("Engine and profile already current");
            return Ok(());
        }
        ensure!(
            !optional(&journal)?,
            "Activation history already exists for this release"
        );
        require_topgrade_idle()?;
        let mut activation = Activation {
            schema_version: 1,
            previous,
            target,
            profile_transaction: None,
        };
        profile(&release, &home, &[])?;
        write_json(&pending, &activation)?;
        let installed = profile(&release, &home, &["--apply"])?;
        match installed["status"].as_str() {
            Some("installed") => {
                let tx = installed["transaction"]
                    .as_str()
                    .context("Missing profile transaction")?;
                ensure!(hexadecimal(tx, 32), "Invalid profile transaction");
                activation.profile_transaction = Some(tx.to_owned());
            }
            Some("already-current") => {}
            _ => bail!("Unexpected profile transaction status"),
        }
        // Retain the activation and rollback evidence even when a later gate fails.
        write_json(&journal, &activation)?;
        let verified = (|| -> Result<()> {
            profile(&release, &home, &["--verify"])?;
            swap_link(&link, &activation.previous, &activation.target)?;
            status(
                Command::new(&link).arg("--version"),
                "Verify active Topgrade launcher",
            )
        })();
        if let Err(error) = verified {
            let recovery = (|| -> Result<()> {
                let current = fs::read_link(&link)?;
                if current == activation.target {
                    swap_link(&link, &activation.target, &activation.previous)?;
                } else {
                    ensure!(
                        current == activation.previous,
                        "Unexpected launcher after activation failure"
                    );
                }
                if let Some(tx) = activation.profile_transaction.as_deref() {
                    profile(&release, &home, &["--rollback", tx])?;
                }
                let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
                fs::rename(
                    &journal,
                    base.join(format!("studio-failed-{tree}-{timestamp}.json")),
                )?;
                fs::remove_file(&pending)?;
                fs::File::open(&base)?.sync_all()?;
                Ok(())
            })();
            return match recovery {
                Ok(()) => Err(error.context(
                    "Activation failed and the previous launcher and profile were restored",
                )),
                Err(recovery) => Err(error.context(format!(
                    "Activation recovery also failed: {recovery:#}; inspect {}",
                    pending.display()
                ))),
            };
        }
        fs::remove_file(&pending)?;
        fs::File::open(&base)?.sync_all()?;
        println!("Activated {}", activation.target.display());
        println!("Activation record: {}", journal.display());
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
