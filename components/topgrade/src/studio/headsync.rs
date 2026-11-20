//! Managed Homebrew HEAD updates and recoverable repairs of stale consumer linkage.
use std::{
    collections::BTreeSet,
    env,
    ffi::OsStr,
    fs,
    io::{self, Write},
    os::fd::{AsRawFd, OwnedFd},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        OnceLock,
        atomic::{AtomicI32, Ordering},
    },
};

use color_eyre::eyre::{Context, ContextCompat, Result, bail, ensure};
use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};
use serde::Deserialize;

use crate::{
    command::CommandExt as _,
    ctrlc,
    error::{PartialStep, TopgradeError},
};

mod journal;
mod llvm_pin;
#[cfg(test)]
mod tests;

const LLAMA_REMOTE: &str = "https://github.com/ggml-org/llama.cpp.git";
const REF: &str = "refs/heads/master";
const PAIR: [&str; 2] = ["ggml", "llama.cpp"];
const MEDIA_FORMULAE: &[&str] = &[
    "ffmpeg",
    "jpeg-xl",
    "libavif",
    "imagemagick",
    "opencv",
    "yt-dlp",
    "exiftool",
];
const JPEG_JOURNAL: &str = "jpeg-xl-active.json";
const PAIR_JOURNAL: &str = "ggml-llama-active.json";
const PAIR_UPGRADE_JOURNAL: &str = "ggml-llama-upgrade-active.json";
static DEFERRED_SIGNAL: AtomicI32 = AtomicI32::new(0);
static SIGNAL_WAKE_ERROR: AtomicI32 = AtomicI32::new(0);
// Retain these descriptors for the process lifetime: a concurrent signal handler
// must never write to a closed descriptor that has been reused for another file.
static SIGNAL_PIPE: OnceLock<[OwnedFd; 2]> = OnceLock::new();

pub(super) fn run() -> Result<()> {
    super::user_only()?;
    let state = match env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
        Some(value) => PathBuf::from(value),
        None => env::home_dir()
            .context("HEAD-sync home directory is unavailable")?
            .join(".local/state"),
    };
    ensure!(state.is_absolute(), "HEAD-sync state must be an absolute path");
    Headsync {
        brew: super::resolve_tool("brew")?,
        state: state.join("brew-headsync"),
        git_config_index: None,
    }
    .run()
}

struct Headsync {
    brew: PathBuf,
    state: PathBuf,
    git_config_index: Option<usize>,
}

struct CatalogRefresh {
    deferred: Vec<String>,
    errors: Vec<String>,
}

struct TapCheckout {
    name: String,
    path: PathBuf,
    dirty: bool,
}

impl Headsync {
    #[expect(
        clippy::disallowed_methods,
        reason = "HEAD-sync preserves the managed terminal environment and scopes Homebrew/Git settings to children"
    )]
    fn command(&self, executable: impl AsRef<OsStr>) -> Command {
        let mut command = Command::new(executable);
        command.envs([
            ("HOMEBREW_NO_ASK", "1"),
            ("HOMEBREW_NO_AUTO_UPDATE", "1"),
            ("HOMEBREW_NO_INSTALL_FROM_API", "1"),
            ("HOMEBREW_NO_INSTALL_CLEANUP", "1"),
        ]);
        if let Some(index) = self.git_config_index {
            command
                .env("GIT_CONFIG_COUNT", (index + 1).to_string())
                .env(
                    format!("GIT_CONFIG_KEY_{index}"),
                    "url.https://github.com/exiftool/exiftool.git.insteadOf",
                )
                .env(
                    format!("GIT_CONFIG_VALUE_{index}"),
                    "https://git.code.sf.net/p/exiftool/code.git",
                );
        }
        command
    }

    fn brew(&self, args: &[&str]) -> Command {
        let mut command = self.command(&self.brew);
        command.args(args);
        command
    }

    fn run(mut self) -> Result<()> {
        let mut signals = DeferredSignals::install()?;
        let result = self.run_locked();
        signals.restore()?;
        let signal = DEFERRED_SIGNAL.swap(0, Ordering::Relaxed);
        if signal != 0 {
            if let Err(error) = result {
                eprintln!("{error:#}");
            }
            std::process::exit(128 + signal);
        }
        result
    }

    fn run_locked(&mut self) -> Result<()> {
        let _lock = journal::lock(&self.state)?;
        // Restore pre-existing formula overlays and ordinary ABI journals
        // before touching LLVM. In particular, a pending JPEG XL or consumer
        // repair must be recovered before any unrelated Homebrew probe.
        if self.abi_pending("whisper-formula-active")? {
            let core = command_path(self.brew(&["--repo", "homebrew/core"]))?.canonicalize()?;
            journal::Journal::new(self.state.canonicalize()?, core)?
                .whisper()
                .recover()?;
        }
        // An interrupted provider upgrade can leave the LLVM pin suspended.
        // Restore that pin *before* replaying its provider-keg transaction.
        if self.abi_pending(PAIR_UPGRADE_JOURNAL)? {
            let brew_prefix = command_path(self.brew(&["--prefix"]))?;
            llvm_pin::recover(&brew_prefix)?;
            self.abi_transaction("pair-upgrade-recover", PAIR_UPGRADE_JOURNAL, &[])?;
        }
        for (journal, mode) in [(PAIR_JOURNAL, "pair-recover"), (JPEG_JOURNAL, "recover")] {
            if self.abi_pending(journal)? {
                self.abi_transaction(mode, journal, &[])?;
            }
        }
        // A crash after pin suspension but before a provider journal is also
        // recoverable. Never refresh catalogs or upgrade while it remains held.
        let brew_prefix = command_path(self.brew(&["--prefix"]))?;
        llvm_pin::recover(&brew_prefix)?;
        let core = command_path(self.brew(&["--repo", "homebrew/core"]))?.canonicalize()?;
        let txn = journal::Journal::new(self.state.canonicalize()?, core)?;
        txn.recover()?;
        for step in [super::Step::LlvmStable, super::Step::LlvmHead] {
            super::llvm::automatic_journal(&self.state.canonicalize()?, step)?;
        }

        let result = self.upgrade(&txn);
        if result.is_ok() {
            println!("\n== Restore official Homebrew formulae ==");
        }
        let incomplete = match (result, txn.recover()) {
            (Ok(incomplete), Ok(())) => incomplete,
            (Err(error), Ok(())) => return Err(error),
            (Ok(_), Err(error)) => {
                return Err(error.wrap_err("HEAD-sync transaction recovery failed"));
            }
            (Err(operation), Err(recovery)) => {
                bail!("{operation:#}; HEAD-sync transaction recovery failed: {recovery:#}");
            }
        };
        self.core_clean(txn.core(), "after transaction recovery")?;
        println!("homebrew/core: CLEAN\nHEAD sync transaction: OK");
        if !incomplete.is_empty() {
            return Err(PartialStep(incomplete.join(" | ")).into());
        }
        Ok(())
    }

    fn abi_pending(&self, journal: &str) -> Result<bool> {
        match fs::symlink_metadata(self.state.join(journal)) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    fn abi_command(&self, script: &Path, mode: &str, formulae: &[String]) -> Result<Command> {
        let mut command = self.brew(&["ruby"]);
        command
            .arg(script)
            .arg(mode)
            .arg(self.state.canonicalize()?)
            .args(formulae)
            .envs([
                ("HOMEBREW_NO_INSTALLED_DEPENDENTS_CHECK", "1"),
                ("HOMEBREW_NO_AUTOREMOVE", "1"),
                ("LANG", "C"),
                ("LC_ALL", "C"),
            ]);
        Ok(command)
    }

    fn abi_script(&self) -> Result<tempfile::NamedTempFile> {
        let mut script = tempfile::Builder::new()
            .prefix(".homebrew-abi-")
            .suffix(".rb")
            .tempfile_in(self.state.canonicalize()?)?;
        script.write_all(include_bytes!("headsync/jpeg_xl.rb"))?;
        Ok(script)
    }

    fn abi_transaction(&self, mode: &str, journal: &str, formulae: &[String]) -> Result<()> {
        let script = self.abi_script()?;
        let mut command = self.abi_command(script.path(), mode, formulae)?;
        let result = if mode == "pair-upgrade" {
            pair_upgrade_status(&mut command)
        } else {
            status(&mut command)
        };
        if let Err(error) = result {
            let exited = matches!(
                error.downcast_ref::<TopgradeError>(),
                Some(TopgradeError::ProcessFailed(_, status)) if status.code().is_some_and(|code| code < 128)
            );
            if !mode.ends_with("recover") && exited && interrupted().is_ok() && self.abi_pending(journal)? {
                let recovery = match journal {
                    PAIR_UPGRADE_JOURNAL => "pair-upgrade-recover",
                    PAIR_JOURNAL => "pair-recover",
                    _ => "recover",
                };
                self.abi_transaction(recovery, journal, &[])
                    .wrap_err_with(|| format!("{error:#}; immediate recovery of {journal} also failed"))?;
            }
            return Err(error);
        }
        ensure!(
            !self.abi_pending(journal)?,
            "Homebrew ABI transaction left a pending journal: {journal}"
        );
        Ok(())
    }

    fn upgrade(&mut self, txn: &journal::Journal) -> Result<Vec<String>> {
        let inherited = env::var_os("GIT_CONFIG_COUNT").unwrap_or_default();
        let inherited = inherited.to_str().context("malformed inherited GIT_CONFIG_COUNT")?;
        ensure!(
            inherited.len() <= 4 && inherited.bytes().all(|byte| byte.is_ascii_digit()),
            "malformed inherited GIT_CONFIG_COUNT"
        );
        self.git_config_index = Some(if inherited.is_empty() { 0 } else { inherited.parse()? });

        self.core_clean(txn.core(), "before Homebrew update")?;
        self.pins()?;
        let installed = names(
            &command_text(self.brew(&["list", "--formula", "--full-name"]))
                .wrap_err("unable to enumerate installed Homebrew formulae")?,
        )?;
        let catalogs = self.refresh_catalogs(txn.core())?;
        let pins = self.pins()?;
        self.update_llvm(super::Step::LlvmStable)?;
        ensure!(
            self.pins()? == pins,
            "Homebrew pin set changed during the stable LLVM update"
        );
        let revision = self.target_revision()?;
        println!("\nTarget llama.cpp snapshot:\n  {revision}");

        let active = if let Ok(active) = self.verify_pair(&revision) {
            println!("Synchronized ggml and llama.cpp are already installed at the full target revision");
            active
        } else {
            txn.prepare(&revision, |file| {
                status(self.command("ruby").arg("-c").arg(file).stdout(Stdio::null()))
            })?;
            println!("HEAD-sync transaction prepared and applied");
            let prefix = command_path(self.brew(&["--prefix"]))?;
            let suspended = llvm_pin::suspend(&prefix)?;
            let operation = (|| -> Result<()> {
                println!("\n== Check upgrade requirements for both synchronized formulae ==");
                let output = self
                    .brew(&["upgrade", "--dry-run", "--formula", "--fetch-HEAD"])
                    .args(PAIR)
                    .output_checked_with(|_| Ok(()))?;
                io::stdout().write_all(&output.stdout)?;
                io::stderr().write_all(&output.stderr)?;
                ensure!(
                    output.status.success(),
                    "Synchronized formula preflight failed before either keg was upgraded"
                );
                // The provider journal covers ggml and llama.cpp, not LLVM.
                // Reject an additional LLVM dependency upgrade before starting it.
                let plan = format!(
                    "{}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                ensure!(
                    !includes_llvm_dependency_plan(&plan),
                    "Homebrew preflight would also modify LLVM; the provider transaction cannot safely journal that change"
                );
                self.abi_transaction("pair-upgrade", PAIR_UPGRADE_JOURNAL, std::slice::from_ref(&revision))
                    .wrap_err("Synchronized provider upgrade or exact-keg link repair failed")
            })();
            let restored = suspended.restore();
            match (operation, restored) {
                (Ok(()), Ok(())) => {}
                (Err(error), Ok(())) | (Ok(()), Err(error)) => return Err(error),
                (Err(operation), Err(recovery)) => {
                    bail!("{operation:#}; LLVM pin restoration failed: {recovery:#}");
                }
            }
            self.pins()?;
            let active = self.verify_pair(&revision)?;
            txn.recover()?;
            active
        };
        self.verify_runtime(&active)?;
        self.repair_pair_consumers(txn)?;
        let residual = self.remaining(txn, &installed, &pins)?;
        self.verify_runtime(&self.verify_pair(&revision)?)?;
        self.verify_linkage()?;
        let missing = command_text(self.brew(&["missing"]))?;
        ensure!(missing.is_empty(), "Homebrew dependencies are missing:\n{missing}");
        ensure!(
            catalogs.errors.is_empty(),
            "Some Homebrew tap catalogs could not be refreshed:\n{}",
            catalogs.errors.join("\n")
        );
        let mut incomplete = Vec::new();
        if !catalogs.deferred.is_empty() {
            incomplete.push(format!(
                "Homebrew catalog refresh deferred for: {}",
                catalogs.deferred.join(", ")
            ));
        }
        if !residual.is_empty() {
            incomplete.push(format!(
                "Homebrew still reports outdated formulae: {}",
                residual.join(", ")
            ));
        }
        if installed.iter().any(|formula| formula == "nyamiiko/tools/llvm-head")
            && let Err(error) = self.update_llvm(super::Step::LlvmHead)
        {
            if let Some(partial) = error.downcast_ref::<PartialStep>() {
                incomplete.push(partial.0.clone());
            } else {
                return Err(error);
            }
        }
        Ok(incomplete)
    }

    fn update_llvm(&self, step: super::Step) -> Result<()> {
        println!(
            "\n== Update {} ==",
            if step == super::Step::LlvmStable {
                "Homebrew LLVM"
            } else {
                "LLVM preview (official main)"
            }
        );
        let prefix = command_path(self.brew(&["--prefix"]))?;
        super::llvm::automatic(
            step,
            &self.brew,
            &prefix,
            &self.state,
            |executable| Ok(self.command(executable)),
            status,
        )
    }

    fn refresh_catalogs(&self, core: &Path) -> Result<CatalogRefresh> {
        let taps = self.tap_checkouts()?;
        let mut refresh = CatalogRefresh {
            deferred: Vec::new(),
            errors: Vec::new(),
        };
        if taps.iter().all(|tap| !tap.dirty) {
            println!("\n== Homebrew update ==");
            status(&mut self.brew(&["update"]))?;
            self.core_clean(core, "after Homebrew update")?;
            self.tap_checkouts()?;
            return Ok(refresh);
        }

        // A regular brew update may reset and stash the index of every tap.
        println!("\n== Refresh Homebrew catalogs (Git fast-forward only) ==");
        ensure!(
            self.refresh_checkout("homebrew/core", core, false)?,
            "Cannot safely refresh homebrew/core"
        );
        self.core_clean(core, "after Homebrew catalog refresh")?;
        for tap in taps {
            match self.refresh_checkout(&tap.name, &tap.path, true) {
                Ok(true) => {}
                Ok(false) => refresh.deferred.push(tap.name),
                Err(error) => {
                    interrupted()?;
                    refresh.errors.push(format!("{error:#}"));
                }
            }
        }
        Ok(refresh)
    }

    fn refresh_checkout(&self, tap: &str, path: &Path, allow_local_changes: bool) -> Result<bool> {
        let refresh = || -> Result<bool> {
            let branch = command_text(self.git(path, &["branch", "--show-current"]))?;
            let upstream = if branch.is_empty() {
                String::new()
            } else {
                command_text(self.git(
                    path,
                    &[
                        "for-each-ref",
                        "--format=%(upstream)%09%(upstream:remotename)%09%(upstream:remoteref)",
                        &format!("refs/heads/{branch}"),
                    ],
                ))?
            };
            if upstream.is_empty() || upstream == "\t\t" {
                ensure!(tap != "homebrew/core", "homebrew/core has no tracked upstream branch");
                // A self-remote is an explicitly local catalog, not a pending upstream refresh.
                if command_text(self.git(path, &["remote"]))? == "origin"
                    && command_text(self.git(path, &["remote", "get-url", "origin"]))? == "."
                {
                    println!("Preserving {tap}: local-only catalog");
                    return Ok(true);
                }
                println!("Preserving {tap}: no tracked upstream branch");
                return Ok(false);
            }
            let fields: Vec<_> = upstream.split('\t').collect();
            let [tracking, remote, source] = fields.as_slice() else {
                bail!("Invalid tracked upstream for {tap}");
            };
            ensure!(
                tracking.starts_with("refs/remotes/") && !remote.is_empty() && source.starts_with("refs/heads/"),
                "Invalid remote branch for {tap}"
            );
            let head = command_text(self.git(path, &["rev-parse", "--verify", "HEAD"]))?;
            let changes = self.catalog_changes(path)?;
            ensure!(
                allow_local_changes || changes.is_empty(),
                "{tap} changed during catalog refresh"
            );
            println!("Refreshing {tap}");
            status(&mut self.git(
                path,
                &[
                    "fetch",
                    "--no-tags",
                    "--no-recurse-submodules",
                    "--no-write-fetch-head",
                    "--",
                    remote,
                    &format!("{source}:{tracking}"),
                ],
            ))?;
            ensure!(
                command_text(self.git(path, &["branch", "--show-current"]))? == branch
                    && command_text(self.git(path, &["rev-parse", "--verify", "HEAD"]))? == head
                    && self.catalog_changes(path)? == changes,
                "{tap} changed during catalog refresh"
            );
            let target = command_text(self.git(path, &["rev-parse", "--verify", tracking]))?;
            if self.is_ancestor(path, &target, &head)? {
                println!("Current {tap}: contains tracked upstream {target}");
                return Ok(true);
            }
            if !self.is_ancestor(path, &head, &target)? {
                println!("Preserving {tap}: tracked upstream diverged");
                return Ok(false);
            }
            if !changes.is_empty() {
                let touched = self.git_bytes(
                    path,
                    &["diff", "--name-only", "--no-renames", "-z", &head, &target, "--"],
                )?;
                for entry in changes.split(|byte| *byte == 0).filter(|entry| !entry.is_empty()) {
                    let local = entry.get(3..).context("Malformed Git status entry")?;
                    if touched.split(|byte| *byte == 0).any(|remote| overlaps(local, remote)) {
                        println!("Preserving {tap}: incoming paths overlap local changes");
                        return Ok(false);
                    }
                }
            }
            status(&mut self.git(
                path,
                &[
                    "merge",
                    "--ff-only",
                    "--no-autostash",
                    "--no-squash",
                    "--no-overwrite-ignore",
                    "--",
                    &target,
                ],
            ))?;
            ensure!(
                command_text(self.git(path, &["branch", "--show-current"]))? == branch
                    && command_text(self.git(path, &["rev-parse", "--verify", "HEAD"]))? == target
                    && self.catalog_changes(path)? == changes,
                "{tap} could not be verified after catalog fast-forward"
            );
            println!("Updated {tap}: {head} -> {target}");
            Ok(true)
        };
        refresh().wrap_err_with(|| format!("Cannot refresh Homebrew tap {tap}"))
    }

    fn git_bytes(&self, path: &Path, args: &[&str]) -> Result<Vec<u8>> {
        interrupted()?;
        let output = self.git(path, args).env("GIT_OPTIONAL_LOCKS", "0").output_checked()?;
        io::stderr().write_all(&output.stderr)?;
        interrupted()?;
        Ok(output.stdout)
    }

    fn catalog_changes(&self, path: &Path) -> Result<Vec<u8>> {
        self.git_bytes(
            path,
            &[
                "status",
                "--porcelain=v1",
                "--untracked-files=all",
                "--no-renames",
                "-z",
            ],
        )
    }

    fn is_ancestor(&self, path: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
        interrupted()?;
        let output = self
            .git(path, &["merge-base", "--is-ancestor", ancestor, descendant])
            .output_checked_with(|output| matches!(output.status.code(), Some(0 | 1)).then_some(()).ok_or(()))?;
        io::stderr().write_all(&output.stderr)?;
        interrupted()?;
        Ok(output.status.success())
    }

    fn git(&self, path: &Path, args: &[&str]) -> Command {
        let mut command = self.command("git");
        command.arg("-C").arg(path).args(args);
        command
    }

    fn worktree_changes(&self, path: &Path) -> Result<String> {
        let mut command = self.git(path, &["status", "--porcelain=v1", "--untracked-files=all"]);
        command.env("GIT_OPTIONAL_LOCKS", "0");
        command_text(command)
    }

    fn tap_checkouts(&self) -> Result<Vec<TapCheckout>> {
        let mut taps = Vec::new();
        for tap in names(&command_text(self.brew(&["tap"]))?)? {
            ensure!(tap.split('/').count() == 2, "Unexpected Homebrew tap identifier");
            if tap == "homebrew/core" {
                continue;
            }
            let path = command_path(self.brew(&["--repo", &tap]))?;
            let changes = self
                .worktree_changes(&path)
                .wrap_err_with(|| format!("Cannot inspect Homebrew tap {tap}"))?;
            ensure!(
                !changes
                    .lines()
                    .any(|line| matches!(line.get(..2), Some("DD" | "AU" | "UD" | "UA" | "DU" | "AA" | "UU"))),
                "Homebrew tap {tap} has unresolved Git conflicts. Resolve them before HEAD-sync"
            );
            taps.push(TapCheckout {
                name: tap,
                path,
                dirty: !changes.is_empty(),
            });
        }
        Ok(taps)
    }

    fn core_clean(&self, core: &Path, phase: &str) -> Result<()> {
        let changes = self
            .worktree_changes(core)
            .wrap_err_with(|| format!("cannot inspect homebrew/core status ({phase})"))?;
        ensure!(changes.is_empty(), "homebrew/core is dirty ({phase}):\n{changes}");
        Ok(())
    }

    fn pins(&self) -> Result<Vec<String>> {
        let pinned = names(
            &command_text(self.brew(&["list", "--formula", "--pinned"]))
                .wrap_err("unable to enumerate pinned Homebrew formulae")?,
        )?;
        ensure!(
            pinned
                .iter()
                .any(|name| matches!(name.as_str(), "llvm" | "homebrew/core/llvm")),
            "Homebrew llvm must remain pinned to preserve the Rust runtime"
        );
        ensure!(
            !pinned.iter().any(|name| PAIR.contains(&basename(name))),
            "The synchronized ggml/llama.cpp pair cannot be upgraded while either formula is pinned"
        );
        let prefix = command_path(self.brew(&["--prefix"]))?;
        let opt = prefix.join("opt/llvm");
        let pin = prefix.join("var/homebrew/pinned/llvm");
        for link in [&opt, &pin] {
            ensure!(
                link.symlink_metadata()
                    .wrap_err_with(|| format!("cannot inspect LLVM link {}", link.display()))?
                    .is_symlink(),
                "LLVM opt and pin must be valid symlinks"
            );
        }
        let active = opt.canonicalize().wrap_err("cannot resolve active LLVM keg")?;
        let pinned_keg = pin.canonicalize().wrap_err("cannot resolve pinned LLVM keg")?;
        let cellar = command_path(self.brew(&["--cellar", "llvm"]))?.canonicalize()?;
        ensure!(
            active.is_dir() && active == pinned_keg && active.parent() == Some(cellar.as_path()),
            "LLVM pin and opt must resolve to the same LLVM Cellar keg"
        );
        let name = active
            .file_name()
            .and_then(OsStr::to_str)
            .context("invalid LLVM keg name")?;
        ensure!(
            name.as_bytes().first().is_some_and(u8::is_ascii_digit),
            "LLVM must be pinned to a stable keg, not HEAD"
        );
        let receipt_path = active.join("INSTALL_RECEIPT.json");
        let mut file =
            super::open_file(&receipt_path, false, super::MAX_JSON).wrap_err("cannot read pinned LLVM receipt")?;
        let receipt_bytes = super::read_json_bytes(&mut file).wrap_err("cannot read pinned LLVM receipt")?;
        let receipt: serde_json::Value =
            serde_json::from_slice(&receipt_bytes).wrap_err("invalid pinned LLVM receipt")?;
        ensure!(
            receipt.pointer("/source/spec").and_then(serde_json::Value::as_str) == Some("stable"),
            "LLVM must be pinned to a stable keg, not HEAD"
        );
        Ok(pinned)
    }

    fn target_revision(&self) -> Result<String> {
        let mut command = self.command("git");
        command.args(["ls-remote", LLAMA_REMOTE, REF]);
        let output = command_text(command)?;
        let (revision, reference) = output
            .split_once('\t')
            .context("unable to resolve llama.cpp master SHA")?;
        ensure!(
            super::is_hex(revision, 40) && reference == REF,
            "unable to resolve llama.cpp master SHA"
        );
        Ok(revision.to_owned())
    }

    fn remaining(&self, txn: &journal::Journal, installed: &[String], pinned: &[String]) -> Result<Vec<String>> {
        println!("\n== Upgrade remaining formulae ==");
        let candidates: Vec<&str> = installed
            .iter()
            .filter_map(|formula| {
                if matches!(basename(formula), "llvm" | "llvm-head" | "topgrade")
                    || basename(formula).starts_with("llvm@")
                    || PAIR.contains(&basename(formula))
                    || pinned.iter().any(|pin| basename(pin) == basename(formula))
                {
                    println!("Skipping protected formula: {formula}");
                    None
                } else {
                    Some(formula.as_str())
                }
            })
            .collect();
        let mut jpeg = installed.iter().filter(|formula| basename(formula) == "jpeg-xl");
        let provider = jpeg.next();
        ensure!(jpeg.next().is_none(), "Ambiguous installed JPEG XL formula identity");
        println!("\n== Media update checks (Homebrew --fetch-HEAD) ==");
        for formula in installed
            .iter()
            .filter(|formula| MEDIA_FORMULAE.contains(&basename(formula)))
        {
            if candidates.contains(&formula.as_str()) {
                println!("CHECKING {formula}");
            } else {
                println!("SKIPPED {formula}: protected or pinned");
            }
        }
        let mut outdated = if candidates.is_empty() {
            println!("No eligible installed formulae; skipping outdated scan");
            Vec::new()
        } else {
            self.outdated(&candidates).inspect_err(|_| {
                eprintln!("FAILED formula update checks: Homebrew --fetch-HEAD scan did not complete");
            })?
        };
        let requested = outdated.clone();
        for formula in candidates
            .iter()
            .filter(|formula| MEDIA_FORMULAE.contains(&basename(formula)))
        {
            let status = if requested.iter().any(|pending| pending == formula) {
                "UPDATE_PENDING"
            } else {
                "CURRENT"
            };
            println!("{status} {formula}");
        }
        if let Some(index) = outdated.iter().position(|formula| basename(formula) == "jpeg-xl") {
            let jxl = outdated.remove(index);
            let group = self.jpeg_group(&jxl)?;
            // An already-broken consumer must be repaired before changing its provider.
            self.abi_transaction("repair", JPEG_JOURNAL, &group)
                .wrap_err("JPEG XL linkage repair before coordinated upgrade failed")?;
            self.abi_transaction("run", JPEG_JOURNAL, &group)
                .wrap_err("Coordinated JPEG XL upgrade failed")?;
            // Rebuilds may already have upgraded an outdated consumer.
            if !outdated.is_empty() {
                let pending: Vec<&str> = outdated.iter().map(String::as_str).collect();
                outdated = self.outdated(&pending)?;
            }
        }
        if !outdated.is_empty() {
            self.upgrade_remaining(&outdated)?;
            self.repair_pair_consumers(txn)?;
        }
        if let Some(provider) = provider {
            // Bottled consumers can reintroduce stale ABI links without an outdated provider.
            self.abi_transaction("repair", JPEG_JOURNAL, &self.jpeg_group(provider)?)
                .wrap_err("JPEG XL dependent linkage repair failed")?;
        }
        if requested.is_empty() {
            println!("No remaining outdated formulae");
            return Ok(Vec::new());
        }
        println!("\n== Verify requested formula upgrades ==");
        let requested: Vec<&str> = requested.iter().map(String::as_str).collect();
        let residual = self.outdated(&requested).inspect_err(|_| {
            eprintln!("FAILED formula update verification: Homebrew --fetch-HEAD readback did not complete");
        })?;
        for formula in requested
            .iter()
            .filter(|formula| MEDIA_FORMULAE.contains(&basename(formula)))
        {
            let status = if residual.iter().any(|pending| pending == formula) {
                "STILL_OUTDATED"
            } else {
                "UPDATED"
            };
            println!("{status} {formula}");
        }
        Ok(residual)
    }

    fn repair_pair_consumers(&self, txn: &journal::Journal) -> Result<()> {
        let mut consumers = BTreeSet::new();
        for provider in PAIR {
            consumers.extend(
                self.abi_consumers(provider)?
                    .into_iter()
                    .filter(|formula| !PAIR.contains(&basename(formula))),
            );
        }
        if !consumers.is_empty() {
            let formulae: Vec<_> = PAIR.into_iter().map(str::to_owned).chain(consumers).collect();
            let whisper = txn.whisper();
            let result = (|| {
                let script = self.abi_script()?;
                let plan = command_text(self.abi_command(script.path(), "pair-plan", &formulae)?)?;
                ensure!(
                    matches!(plan.as_str(), "current" | "repair" | "whisper-compat"),
                    "Unexpected ggml/llama.cpp repair plan: {plan}"
                );
                if plan == "current" {
                    return Ok(());
                }
                if plan == "whisper-compat" {
                    self.core_clean(txn.core(), "before Whisper compatibility patch")?;
                    whisper.prepare_whisper(|file| {
                        status(self.command("ruby").arg("-c").arg(file).stdout(Stdio::null()))
                    })?;
                }
                self.abi_transaction("pair-repair", PAIR_JOURNAL, &formulae)
                    .wrap_err("ggml/llama.cpp consumer linkage repair failed")
            })();
            let recovered = if whisper.pending()? { whisper.recover() } else { Ok(()) };
            match (result, recovered) {
                (Ok(()), Ok(())) => {}
                (Err(error), Ok(())) | (Ok(()), Err(error)) => return Err(error),
                (Err(error), Err(recovery)) => {
                    bail!("{error:#}; Whisper formula recovery failed: {recovery:#}")
                }
            }
        }
        Ok(())
    }

    fn jpeg_group(&self, provider: &str) -> Result<Vec<String>> {
        let mut group = self.abi_consumers(provider)?;
        group.insert(0, provider.to_owned());
        Ok(group)
    }

    fn abi_consumers(&self, provider: &str) -> Result<Vec<String>> {
        names(
            &command_text(self.brew(&[
                "uses",
                "--installed",
                "--recursive",
                "--include-implicit",
                "--include-optional",
                "--formula",
                provider,
            ]))
            .wrap_err_with(|| format!("Unable to verify installed consumers of {provider}"))?,
        )
    }

    fn outdated(&self, candidates: &[&str]) -> Result<Vec<String>> {
        interrupted()?;
        let output = self
            .brew(&["outdated", "--formula", "--fetch-HEAD", "--json=v2"])
            .args(candidates)
            .output_checked_with(|output| match output.status.code() {
                Some(0 | 1) => Ok(()),
                _ => Err(()),
            })
            .wrap_err("unable to enumerate outdated formulae")?;
        io::stderr().write_all(&output.stderr)?;
        ensure!(
            !output.stdout.is_empty(),
            "Homebrew returned no formula inventory (exit {:?}); inspect its diagnostic above",
            output.status.code()
        );
        outdated_names(&output.stdout, output.status.code() == Some(1), candidates)
            .wrap_err("invalid Homebrew outdated result")
    }

    fn upgrade_remaining(&self, outdated: &[String]) -> Result<()> {
        let dependents = names(
            &command_text(self.brew(&["uses", "--installed", "--include-build", "--formula", "rust"]))
                .wrap_err("unable to identify installed Homebrew Rust dependents")?,
        )?;
        if let Some(formula) = outdated.iter().find(|formula| {
            dependents
                .iter()
                .any(|dependent| basename(dependent) == basename(formula))
        }) {
            interrupted()?;
            self.brew(&["linkage", "--test", "rust"])
                .envs([("LANG", "C"), ("LC_ALL", "C")])
                .output_checked_with(|output| {
                    if super::diagnostics::linkage_failed(output) { Err(()) } else { Ok(()) }
                })
                .wrap_err_with(|| format!(
                    "outdated formula directly depends on Homebrew Rust: {formula}. Repair Homebrew Rust before upgrading; Rustup PATH does not override Homebrew dependencies"
                ))?;
            let prefix = command_path(self.brew(&["--prefix", "rust"]))?;
            status(
                self.command(prefix.join("bin/rustc"))
                    .arg("--version")
                    .stdout(Stdio::null()),
            )
            .wrap_err_with(|| format!("Homebrew Rust cannot run; required by {formula}"))?;
        }
        // Homebrew considers an installed HEAD keg when checking whether the
        // simultaneously installed, current stable LLVM satisfies dependencies.
        // Its pin must therefore be suspended for the *remaining* formulae as
        // well as the ggml/llama provider pair. The exact stable pin is held
        // durably and restored on every exit path (including an interrupted
        // upgrade); a subsequent run recovers it before doing any other work.
        let prefix = command_path(self.brew(&["--prefix"]))?;
        let suspended = llvm_pin::suspend(&prefix)?;
        let operation = (|| -> Result<()> {
            println!("\n== Check remaining formula upgrade requirements with stable LLVM temporarily unpinned ==");
            let output = self
                .brew(&["upgrade", "--dry-run", "--formula", "--fetch-HEAD"])
                .args(outdated)
                .output_checked_with(|_| Ok(()))?;
            io::stdout().write_all(&output.stdout)?;
            io::stderr().write_all(&output.stderr)?;
            ensure!(
                output.status.success(),
                "Remaining formula preflight failed before modifying installed formulae"
            );
            let plan = format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            ensure!(
                !includes_llvm_dependency_plan(&plan),
                "Remaining formula upgrade would modify LLVM; its installed keg is not part of a protected transaction"
            );
            status(self.brew(&["upgrade", "--formula", "--fetch-HEAD"]).args(outdated))
        })();
        let restored = suspended.restore();
        match (operation, restored) {
            (Ok(()), Ok(())) => {}
            (Err(error), Ok(())) | (Ok(()), Err(error)) => return Err(error),
            (Err(operation), Err(recovery)) => {
                bail!("{operation:#}; LLVM pin restoration after remaining formulae failed: {recovery:#}");
            }
        }
        self.pins()?;
        Ok(())
    }

    fn verify_linkage(&self) -> Result<()> {
        let linkage = self
            .brew(&["linkage", "--test"])
            .envs([("LANG", "C"), ("LC_ALL", "C")])
            .output_checked_with(|_| Ok(()))?;
        super::diagnostics::verify_linkage(linkage).wrap_err("Linkage check at the end of Homebrew maintenance")
    }

    fn verify_pair(&self, revision: &str) -> Result<Vec<PathBuf>> {
        ensure!(super::is_hex(revision, 40), "Expected a full target HEAD revision");
        let expected_keg = format!("HEAD-{}", &revision[..7]);
        let prefix = command_path(self.brew(&["--prefix"]))?;
        let mut active = Vec::with_capacity(PAIR.len());
        for formula in PAIR {
            let opt = command_path(self.brew(&["--prefix", formula]))?;
            ensure!(
                opt.symlink_metadata()?.is_symlink() && opt.is_dir(),
                "active {formula} opt link is missing or invalid: {}",
                opt.display()
            );
            let keg = opt.canonicalize()?;
            let cellar = command_path(self.brew(&["--cellar", formula]))?.canonicalize()?;
            ensure!(
                keg.parent() == Some(cellar.as_path()),
                "active {formula} points outside its Homebrew cellar: {}",
                keg.display()
            );
            let name = keg
                .file_name()
                .and_then(OsStr::to_str)
                .context("Invalid active keg name")?;
            ensure!(
                name.split('_').next() == Some(expected_keg.as_str()),
                "{formula} did not land on target snapshot"
            );
            let receipt_path = keg.join("INSTALL_RECEIPT.json");
            let mut file = super::open_file(&receipt_path, false, super::MAX_JSON)?;
            let receipt: Receipt = serde_json::from_slice(&super::read_json_bytes(&mut file)?)
                .wrap_err_with(|| format!("cannot read {formula} full revision from its receipt"))?;
            ensure!(
                receipt.source.spec == "head" && receipt.source.scm_revision == revision,
                "{formula} full revision does not match target"
            );
            verify_keg_links(&prefix, formula, &keg)?;
            active.push(keg);
        }
        println!("Both active receipts record full revision {revision}");
        Ok(active)
    }

    fn verify_runtime(&self, active: &[PathBuf]) -> Result<()> {
        println!("\n== Verify matched runtime ==");
        const SYMBOL: &str = "ggml_backend_sched_set_copy_callback";
        ensure!(
            fs::read_to_string(active[0].join("include/ggml-backend.h"))?.contains(SYMBOL),
            "Missing ggml backend declaration {SYMBOL}"
        );
        let mut nm = self.command("nm");
        nm.arg("-gU").arg(active[0].join("lib/libggml-base.dylib"));
        ensure!(
            command_text(nm)?.lines().any(|line| line
                .split_whitespace()
                .next_back()
                .is_some_and(|symbol| symbol.trim_start_matches('_') == SYMBOL)),
            "Missing ggml backend symbol {SYMBOL}"
        );
        status(self.command(active[1].join("bin/llama-cli")).arg("--version"))?;
        let unified = active[1].join("bin/llama");
        if unified.try_exists()? {
            status(self.command(unified).arg("--version"))?;
        }
        Ok(())
    }
}

fn verify_keg_links(prefix: &Path, formula: &str, keg: &Path) -> Result<()> {
    let linked = prefix.join("var/homebrew/linked").join(formula);
    ensure!(
        linked.symlink_metadata()?.is_symlink() && linked.canonicalize()? == keg,
        "{formula} linked-keg record does not select {}",
        keg.display()
    );
    let bin = keg.join("bin");
    let entries = match fs::read_dir(&bin) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).wrap_err_with(|| format!("Cannot inspect {}", bin.display()));
        }
    };
    for entry in entries {
        let entry = entry?;
        let source = entry.path();
        if source.is_dir() {
            continue;
        }
        let destination = prefix.join("bin").join(entry.file_name());
        ensure!(
            destination.symlink_metadata()?.is_symlink() && destination.canonicalize()? == source.canonicalize()?,
            "{} does not resolve to the exact active {formula} executable {}",
            destination.display(),
            source.display()
        );
    }
    Ok(())
}

fn includes_llvm_dependency_plan(plan: &str) -> bool {
    plan.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '@' && ch != '.' && ch != '-')
        .any(|token| token.eq_ignore_ascii_case("llvm") || token.starts_with("llvm@"))
}

#[test]
fn prevents_unjournaled_llvm_dependency_upgrade() {
    assert!(includes_llvm_dependency_plan(
        "==> Would upgrade dependency:\nllvm HEAD-a -> HEAD-b"
    ));
    assert!(includes_llvm_dependency_plan("homebrew/core/llvm@22 needs upgrading"));
    assert!(!includes_llvm_dependency_plan(
        "ggml HEAD-abc -> HEAD-def\nllama.cpp HEAD-abc -> HEAD-def"
    ));
}

fn overlaps(left: &[u8], right: &[u8]) -> bool {
    left == right
        || left.strip_prefix(right).is_some_and(|suffix| suffix.starts_with(b"/"))
        || right.strip_prefix(left).is_some_and(|suffix| suffix.starts_with(b"/"))
}

fn interrupted() -> Result<()> {
    ensure!(
        !ctrlc::interrupted() && DEFERRED_SIGNAL.load(Ordering::Relaxed) == 0,
        "HEAD-sync interrupted"
    );
    Ok(())
}

extern "C" fn defer_signal(signal: i32) {
    if DEFERRED_SIGNAL
        .compare_exchange(0, signal, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    if let Some(pipe) = SIGNAL_PIPE.get() {
        let saved_errno = nix::errno::Errno::last_raw();
        let byte = 1_u8;
        // POSIX write is async-signal-safe; this pipe is nonblocking. A full
        // pipe already wakes the waiter, so EAGAIN needs no second notification.
        loop {
            let written = unsafe { nix::libc::write(pipe[1].as_raw_fd(), (&byte as *const u8).cast(), 1) };
            let error = nix::errno::Errno::last_raw();
            if written == 1 || (written == -1 && error == nix::libc::EAGAIN) {
                break;
            }
            if written == -1 && error == nix::libc::EINTR {
                continue;
            }
            SIGNAL_WAKE_ERROR.store(if written == -1 { error } else { nix::libc::EIO }, Ordering::Relaxed);
            break;
        }
        nix::errno::Errno::set_raw(saved_errno);
    }
}

fn signal_pipe() -> Result<&'static [OwnedFd; 2]> {
    if let Some(pipe) = SIGNAL_PIPE.get() {
        return Ok(pipe);
    }
    let (read, write) = nix::unistd::pipe().wrap_err("Could not create the HEAD-sync signal pipe")?;
    for descriptor in [&read, &write] {
        for (operation, flags) in [
            (nix::libc::F_SETFD, nix::libc::FD_CLOEXEC),
            (nix::libc::F_SETFL, nix::libc::O_NONBLOCK),
        ] {
            if unsafe { nix::libc::fcntl(descriptor.as_raw_fd(), operation, flags) } == -1 {
                return Err(io::Error::last_os_error()).wrap_err("Could not configure the HEAD-sync signal pipe");
            }
        }
    }
    if let Err(unused) = SIGNAL_PIPE.set([read, write]) {
        // Another initializer installed the shared pipe; close only our unused pair.
        drop(unused);
    }
    SIGNAL_PIPE.get().context("HEAD-sync signal pipe was not initialized")
}

fn drain_signal_pipe(pipe: &[OwnedFd; 2]) -> Result<()> {
    let mut bytes = [0_u8; 64];
    loop {
        let count = unsafe { nix::libc::read(pipe[0].as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len()) };
        if count > 0 {
            continue;
        }
        let error = io::Error::last_os_error();
        if count == -1 && error.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if count == -1 && error.kind() == io::ErrorKind::WouldBlock {
            return Ok(());
        }
        ensure!(count != 0, "HEAD-sync signal pipe closed unexpectedly");
        return Err(error).wrap_err("Could not read the HEAD-sync signal pipe");
    }
}

fn wait_for_pair_signal(pipe: &[OwnedFd; 2]) -> Result<()> {
    let mut descriptor = nix::libc::pollfd {
        fd: pipe[0].as_raw_fd(),
        events: nix::libc::POLLIN,
        revents: 0,
    };
    let ready = unsafe { nix::libc::poll(&mut descriptor, 1, 100) };
    if ready == -1 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            return Ok(());
        }
        return Err(error).wrap_err("Could not wait for a HEAD-sync signal");
    }
    ensure!(
        descriptor.revents & (nix::libc::POLLERR | nix::libc::POLLHUP | nix::libc::POLLNVAL) == 0,
        "HEAD-sync signal pipe became unavailable"
    );
    if ready > 0 {
        drain_signal_pipe(pipe)?;
    }
    Ok(())
}

struct DeferredSignals(Vec<(Signal, SigAction)>);

impl DeferredSignals {
    fn install() -> Result<Self> {
        drain_signal_pipe(signal_pipe()?)?;
        SIGNAL_WAKE_ERROR.store(0, Ordering::Relaxed);
        DEFERRED_SIGNAL.store(0, Ordering::Relaxed);
        let mut guard = Self(Vec::new());
        let handler = SigAction::new(SigHandler::Handler(defer_signal), SaFlags::SA_RESTART, SigSet::empty());
        // Keep the lock until any running child exits and formula recovery finishes.
        for signal in [Signal::SIGINT, Signal::SIGTERM, Signal::SIGHUP] {
            guard.0.push((signal, unsafe { sigaction(signal, &handler) }?));
        }
        Ok(guard)
    }

    fn restore(&mut self) -> Result<()> {
        let mut result = Ok(());
        for (signal, previous) in self.0.drain(..).rev() {
            if let Err(error) = unsafe { sigaction(signal, &previous) } {
                result =
                    Err(error).wrap_err_with(|| format!("Could not restore HEAD-sync signal handler for {signal}"));
            }
        }
        result
    }
}

impl Drop for DeferredSignals {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            eprintln!("{error:#}");
        }
    }
}

fn status(command: &mut Command) -> Result<()> {
    interrupted()?;
    command.status_checked()?;
    interrupted()
}

fn pair_upgrade_status(command: &mut Command) -> Result<()> {
    interrupted()?;
    let pipe = signal_pipe()?;
    let mut child = command.spawn_checked()?;
    let result = (|| {
        let pid = nix::unistd::Pid::from_raw(i32::try_from(child.id())?);
        let mut forwarded = false;
        let exit = loop {
            if let Some(exit) = child.try_wait()? {
                break exit;
            }
            if !forwarded {
                let deferred = DEFERRED_SIGNAL.load(Ordering::Relaxed);
                let signal = if deferred != 0 {
                    Some(Signal::try_from(deferred)?)
                } else if ctrlc::interrupted() {
                    Some(Signal::SIGINT)
                } else {
                    None
                };
                if let Some(signal) = signal {
                    // Signal only the coordinating Ruby process. Its trap lets
                    // the current builder finish, then rolls back before the
                    // next provider. Retain the outer lock until it exits.
                    forwarded = true;
                    if let Err(error) = nix::sys::signal::kill(pid, signal)
                        && error != nix::errno::Errno::ESRCH
                    {
                        return Err(error).wrap_err("Could not forward interruption to the provider transaction");
                    }
                }
            }
            wait_for_pair_signal(pipe)?;
        };
        let wake_error = SIGNAL_WAKE_ERROR.load(Ordering::Relaxed);
        if wake_error != 0 {
            return Err(io::Error::from_raw_os_error(wake_error))
                .wrap_err("Could not notify the provider waiter of an interruption");
        }
        if !exit.success() {
            return Err(TopgradeError::ProcessFailed(
                command.get_program().to_string_lossy().into_owned(),
                exit,
            ))
            .wrap_err("Synchronized provider transaction subprocess failed");
        }
        interrupted()
    })();
    match result {
        Ok(()) => Ok(()),
        Err(error) => match child.wait() {
            Ok(_) => Err(error),
            Err(wait) => bail!("{error:#}; could not wait for the provider transaction: {wait}"),
        },
    }
}

fn command_text(mut command: Command) -> Result<String> {
    interrupted()?;
    let output = command.output_checked()?;
    io::stderr().write_all(&output.stderr)?;
    interrupted()?;
    Ok(String::from_utf8(output.stdout)?.trim_end_matches('\n').to_owned())
}

fn command_path(command: Command) -> Result<PathBuf> {
    let output = command_text(command)?;
    let path = PathBuf::from(&output);
    ensure!(
        path.is_absolute() && !output.chars().any(char::is_control),
        "Expected one absolute path from Homebrew"
    );
    Ok(path)
}

fn basename(formula: &str) -> &str {
    formula.rsplit('/').next().unwrap_or(formula)
}

fn names(output: &str) -> Result<Vec<String>> {
    let mut seen = BTreeSet::new();
    let mut names = Vec::new();
    for name in output.lines().filter(|name| !name.is_empty()) {
        ensure!(
            name.split('/').all(|part| {
                part.starts_with(|ch: char| ch.is_ascii_alphanumeric())
                    && part
                        .bytes()
                        .all(|ch| ch.is_ascii_alphanumeric() || b"@+_.-".contains(&ch))
            }) && seen.insert(name),
            "Unexpected or duplicate Homebrew formula identifier"
        );
        names.push(name.to_owned());
    }
    Ok(names)
}

#[derive(Deserialize)]
struct Outdated {
    formulae: Vec<OutdatedFormula>,
    casks: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct OutdatedFormula {
    name: String,
    pinned: bool,
}

fn outdated_names(bytes: &[u8], reports_outdated: bool, candidates: &[&str]) -> Result<Vec<String>> {
    let output: Outdated = serde_json::from_slice(bytes)?;
    ensure!(output.casks.is_empty(), "Unexpected casks in formula-only scan");
    ensure!(
        reports_outdated != output.formulae.is_empty(),
        "Exit status disagrees with outdated results"
    );
    let mut remaining: BTreeSet<_> = candidates.iter().copied().collect();
    output
        .formulae
        .into_iter()
        .map(|row| {
            ensure!(
                !row.pinned && remaining.remove(row.name.as_str()),
                "Unexpected, newly pinned, or duplicate formula"
            );
            Ok(row.name)
        })
        .collect()
}

#[derive(Deserialize)]
struct Receipt {
    source: ReceiptSource,
}

#[derive(Deserialize)]
struct ReceiptSource {
    spec: String,
    scm_revision: String,
}

#[cfg(test)]
mod pair_link_tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn matched_receipts_still_require_the_exact_linked_record_and_prefix_binary() -> Result<()> {
        let stage = tempfile::tempdir()?;
        let prefix = stage.path();
        let revision = "0123456789abcdef0123456789abcdef01234567";
        let keg = prefix.join("Cellar/llama.cpp/HEAD-0123456");
        let previous = prefix.join("Cellar/llama.cpp/HEAD-fedcba9");
        for path in [
            keg.join("bin"),
            previous.join("bin"),
            prefix.join("bin"),
            prefix.join("opt"),
            prefix.join("var/homebrew/linked"),
        ] {
            fs::create_dir_all(path)?;
        }
        for formula in PAIR {
            let current = prefix.join("Cellar").join(formula).join("HEAD-0123456");
            fs::create_dir_all(&current)?;
            fs::write(
                current.join("INSTALL_RECEIPT.json"),
                format!(r#"{{"source":{{"spec":"head","scm_revision":"{revision}"}}}}"#),
            )?;
            symlink(&current, prefix.join("opt").join(formula))?;
        }
        symlink(
            prefix.join("Cellar/ggml/HEAD-0123456"),
            prefix.join("var/homebrew/linked/ggml"),
        )?;
        let brew = prefix.join("brew");
        fs::write(
            &brew,
            concat!(
                "#!/bin/sh\nprefix=${0%/brew}\n",
                "case \"$1:$2\" in\n",
                "  --prefix:) printf '%s\\n' \"$prefix\";;\n",
                "  --prefix:*) printf '%s\\n' \"$prefix/opt/$2\";;\n",
                "  --cellar:*) printf '%s\\n' \"$prefix/Cellar/$2\";;\n",
                "  *) exit 2;;\nesac\n",
            ),
        )?;
        fs::set_permissions(&brew, fs::Permissions::from_mode(0o700))?;
        let headsync = Headsync {
            brew,
            state: prefix.join("state"),
            git_config_index: None,
        };
        fs::write(keg.join("bin/llama-cli"), "current")?;
        fs::write(previous.join("bin/llama-cli"), "previous")?;
        let linked = prefix.join("var/homebrew/linked/llama.cpp");
        let binary = prefix.join("bin/llama-cli");

        assert!(headsync.verify_pair(revision).is_err());
        symlink(&previous, &linked)?;
        symlink(previous.join("bin/llama-cli"), &binary)?;
        assert!(headsync.verify_pair(revision).is_err());
        fs::remove_file(&linked)?;
        symlink(&keg, &linked)?;
        assert!(headsync.verify_pair(revision).is_err());
        fs::remove_file(&binary)?;
        symlink(keg.join("bin/llama-cli"), &binary)?;
        headsync.verify_pair(revision)?;
        Ok(())
    }
}
