//! Managed Homebrew HEAD updates with one recoverable ggml/llama.cpp formula transaction.
use std::{
    collections::BTreeSet,
    env,
    ffi::OsStr,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicI32, Ordering},
};

use color_eyre::eyre::{Context, ContextCompat, Result, bail, ensure};
use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};
use serde::Deserialize;

use crate::{command::CommandExt as _, ctrlc};

mod journal;
#[cfg(test)]
mod tests;

const LLAMA_REMOTE: &str = "https://github.com/ggml-org/llama.cpp.git";
const REF: &str = "refs/heads/master";
const PAIR: [&str; 2] = ["ggml", "llama.cpp"];
static DEFERRED_SIGNAL: AtomicI32 = AtomicI32::new(0);

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
        let core = command_path(self.brew(&["--repo", "homebrew/core"]))?.canonicalize()?;
        let txn = journal::Journal::new(self.state.canonicalize()?, core)?;
        txn.recover()?;

        let result = self.upgrade(&txn);
        if result.is_ok() {
            println!("\n== Restore official Homebrew formulae ==");
        }
        match (result, txn.recover()) {
            (Ok(()), Ok(())) => {}
            (Err(error), Ok(())) => return Err(error),
            (Ok(()), Err(error)) => return Err(error.wrap_err("HEAD-sync transaction recovery failed")),
            (Err(operation), Err(recovery)) => {
                bail!("{operation:#}; HEAD-sync transaction recovery failed: {recovery:#}");
            }
        }
        self.core_clean(txn.core(), "after transaction recovery")?;
        println!("homebrew/core: CLEAN\nHEAD sync transaction: OK");
        Ok(())
    }

    fn upgrade(&mut self, txn: &journal::Journal) -> Result<()> {
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
        println!("\n== Homebrew update ==");
        status(&mut self.brew(&["update"]))?;
        self.core_clean(txn.core(), "after Homebrew update")?;
        let pins = self.pins()?;
        let revision = self.target_revision()?;
        println!("\nTarget llama.cpp snapshot:\n  {revision}");

        txn.prepare(&revision, |file| {
            status(self.command("ruby").arg("-c").arg(file).stdout(Stdio::null()))
        })?;
        println!("HEAD-sync transaction prepared and applied");
        println!("\n== Check upgrade requirements for both synchronized formulae ==");
        status(self.brew(&["upgrade", "--dry-run", "--formula", "--fetch-HEAD"]).args(PAIR))
            .wrap_err("Synchronized formula preflight failed before either keg was upgraded; inspect the requirements above and preserve the LLVM pin. The official stable LLVM revision may have advanced since the pinned keg was installed")?;
        for formula in PAIR {
            println!("\n== Upgrade {formula} at {revision} ==");
            status(&mut self.brew(&["upgrade", "--formula", "--fetch-HEAD", formula]))?;
        }
        println!("\n== Verify active pair before remaining formulae ==");
        self.verify_pair(&revision)?;
        self.remaining(&installed, &pins)?;
        self.verify_runtime(&revision)
    }

    fn core_clean(&self, core: &Path, phase: &str) -> Result<()> {
        let mut command = self.command("git");
        command.env("GIT_OPTIONAL_LOCKS", "0").arg("-C").arg(core).args([
            "status",
            "--porcelain",
            "--untracked-files=all",
        ]);
        let changes =
            command_text(command).wrap_err_with(|| format!("cannot inspect homebrew/core status ({phase})"))?;
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

    fn remaining(&self, installed: &[String], pinned: &[String]) -> Result<()> {
        println!("\n== Upgrade remaining formulae ==");
        let candidates: Vec<&str> = installed
            .iter()
            .filter_map(|formula| {
                if matches!(basename(formula), "llvm" | "llvm-head" | "topgrade")
                    || pinned.iter().any(|pin| basename(pin) == basename(formula))
                {
                    println!("Skipping protected formula: {formula}");
                    None
                } else {
                    Some(formula.as_str())
                }
            })
            .collect();
        if candidates.is_empty() {
            println!("No eligible installed formulae; skipping outdated scan");
            return Ok(());
        }

        interrupted()?;
        let output = self
            .brew(&["outdated", "--formula", "--fetch-HEAD", "--json=v2"])
            .args(&candidates)
            .output_checked_with(|output| match output.status.code() {
                Some(0 | 1) => Ok(()),
                _ => Err(()),
            })
            .wrap_err("unable to enumerate outdated formulae")?;
        io::stderr().write_all(&output.stderr)?;
        let outdated = outdated_names(&output.stdout, output.status.code() == Some(1), &candidates)
            .wrap_err("invalid Homebrew outdated result")?;
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
        if outdated.is_empty() {
            println!("No remaining outdated formulae");
        } else {
            status(self.brew(&["upgrade", "--formula", "--fetch-HEAD"]).args(outdated))?;
        }
        Ok(())
    }

    fn verify_pair(&self, revision: &str) -> Result<Vec<PathBuf>> {
        let expected_keg = format!("HEAD-{}", &revision[..7]);
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
            println!("{formula}: {name}");
            ensure!(
                name.split('_').next() == Some(expected_keg.as_str()),
                "{formula} did not land on target snapshot"
            );
            let receipt_path = keg.join("INSTALL_RECEIPT.json");
            let receipt: Receipt = serde_json::from_slice(&fs::read(&receipt_path)?)
                .wrap_err_with(|| format!("cannot read {formula} full revision from its receipt"))?;
            ensure!(
                receipt.source.spec == "head" && receipt.source.scm_revision == revision,
                "{formula} full revision does not match target"
            );
            active.push(keg);
        }
        println!("Both active receipts record full revision {revision}");
        Ok(active)
    }

    fn verify_runtime(&self, revision: &str) -> Result<()> {
        println!("\n== Verify matched runtime ==");
        let active = self.verify_pair(revision)?;
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
        let missing = command_text(self.brew(&["missing"]))?;
        ensure!(missing.is_empty(), "Homebrew dependencies are missing:\n{missing}");
        Ok(())
    }
}

fn interrupted() -> Result<()> {
    ensure!(
        !ctrlc::interrupted() && DEFERRED_SIGNAL.load(Ordering::Relaxed) == 0,
        "HEAD-sync interrupted"
    );
    Ok(())
}

extern "C" fn defer_signal(signal: i32) {
    let _ = DEFERRED_SIGNAL.compare_exchange(0, signal, Ordering::Relaxed, Ordering::Relaxed);
}

struct DeferredSignals(Vec<(Signal, SigAction)>);

impl DeferredSignals {
    fn install() -> Result<Self> {
        DEFERRED_SIGNAL.store(0, Ordering::Relaxed);
        let mut guard = Self(Vec::new());
        let handler = SigAction::new(SigHandler::Handler(defer_signal), SaFlags::SA_RESTART, SigSet::empty());
        // Keep the lock until any running child exits and formula recovery finishes.
        for signal in [Signal::SIGTERM, Signal::SIGHUP] {
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
