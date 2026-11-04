use super::{
    FILE_LIMIT, check_dir, command, ensure_cli, exists, is_hex, open_file, owned_directories, read_small, sync_dir,
    tool_command,
};
use crate::command::CommandExt as _;
use color_eyre::eyre::{Context, ContextCompat, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    os::unix::{fs::symlink, prelude::PermissionsExt},
    path::{Path, PathBuf},
};

const PYTHON: &str = "/opt/homebrew/opt/python@3.15/bin/python3.15";
const OLD_LIBRARY: &str = "/opt/homebrew/opt/sqlite/lib/libsqlite3.dylib";
const NEW_LIBRARY: &str = "@loader_path/libsqlite3.dylib";
const ROOT: &str = ".local/share/sqlite-python-trunk";
const USER_SITE: &str = "Library/Python/3.15/lib/python/site-packages";
const PTH: &str = "00-sqlite-trunk.pth";
const ABI: &str = r#"
import json, pathlib, site, sys, sysconfig
print(json.dumps({
    "version": list(sys.version_info[:2]),
    "site_enabled": site.check_enableusersite(),
    "user_site": site.getusersitepackages(),
    "extension": str(pathlib.Path(sysconfig.get_path("stdlib")) / "lib-dynload" /
                     ("_sqlite3" + sysconfig.get_config_var("EXT_SUFFIX"))),
}))
"#;
const PROBE: &str = r#"
import pathlib, sqlite3, sys, _sqlite3
source, directory = sys.argv[1:3]
assert pathlib.Path(_sqlite3.__file__).resolve().parent == pathlib.Path(directory).resolve(), (
    "Unexpected SQLite extension location"
)
db = sqlite3.connect(":memory:")
assert db.execute("SELECT sqlite_source_id()").fetchone() == (source,), "SQLite source mismatch"
assert db.execute("PRAGMA quick_check").fetchone() == ("ok",), "SQLite quick-check failed"
db.execute("CREATE VIRTUAL TABLE docs USING fts5(body)")
db.execute("INSERT INTO docs VALUES ('sqlite trunk')")
assert db.execute("SELECT count(*) FROM docs WHERE docs MATCH 'trunk'").fetchone() == (1,), (
    "SQLite FTS5 verification failed"
)
print("Python SQLite:", sqlite3.sqlite_version)
print("SQLite source:", source)
"#;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PythonAbi {
    version: [u8; 2],
    site_enabled: bool,
    user_site: PathBuf,
    extension: PathBuf,
}

struct Profile {
    root: PathBuf,
    current: PathBuf,
    pth: PathBuf,
    content: Vec<u8>,
    journal: PathBuf,
}

impl Profile {
    fn new(home: &Path, state: &Path) -> Result<Self> {
        let root = owned_directories(home, Path::new(ROOT), true)?;
        let current = root.join("current");
        let current_text = current.to_str().context("SQLite Python current path is not UTF-8")?;
        ensure!(
            !current_text.contains(['\'', '\\']) && !current_text.chars().any(char::is_control),
            "SQLite Python path cannot be represented by the managed .pth format"
        );
        let content = format!("import sys; sys.path.insert(0, '{current_text}')\n").into_bytes();
        Ok(Self {
            root,
            current,
            pth: home.join(USER_SITE).join(PTH),
            content,
            journal: state.join("python-active.json"),
        })
    }

    fn release(&self, path: &Path) -> Result<()> {
        let releases = self.root.join("releases");
        let (revision, extension) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.split_once('-'))
            .context("SQLite Python link must name a managed release")?;
        ensure!(
            path.parent() == Some(releases.as_path()) && is_hex(revision, 40) && is_hex(extension, 12),
            "SQLite Python link points outside the managed releases"
        );
        check_dir(&releases, false)?;
        check_dir(path, false)
    }

    fn current(&self) -> Result<Option<PathBuf>> {
        if !exists(&self.current)? {
            return Ok(None);
        }
        let link = fs::read_link(&self.current).wrap_err("Refusing to replace a non-symlink Python SQLite current")?;
        self.release(&link)?;
        Ok(Some(link))
    }

    fn pth_exists(&self) -> Result<bool> {
        if !exists(&self.pth)? {
            return Ok(false);
        }
        ensure!(
            read_small(&self.pth, 4096)? == self.content,
            "Existing SQLite Python .pth differs; left unchanged"
        );
        Ok(true)
    }

    fn swap(&self, target: &Path) -> Result<()> {
        let temporary = tempfile::Builder::new()
            .prefix(".sqlite-link.")
            .tempdir_in(&self.root)?;
        let link = temporary.path().join("current");
        symlink(target, &link)?;
        fs::rename(link, &self.current)?;
        sync_dir(&self.root)
    }

    fn finish(&self) -> Result<()> {
        fs::remove_file(&self.journal)?;
        sync_dir(self.journal.parent().context("SQLite Python journal has no parent")?)
    }

    fn recover(&self) -> Result<()> {
        if !exists(&self.journal)? {
            return Ok(());
        }
        let journal: Activation = serde_json::from_slice(&read_small(&self.journal, 4096)?)?;
        self.release(&journal.candidate)?;
        if let Some(previous) = &journal.previous {
            self.release(previous)?;
        }
        let current = self.current()?;
        let pth = self.pth_exists()?;
        ensure!(
            (current == journal.previous || current.as_ref() == Some(&journal.candidate))
                && (!journal.pth_existed || pth),
            "Python SQLite activation changed outside this transaction; recovery left it unchanged"
        );
        if pth && !journal.pth_existed {
            ensure!(self.pth_exists()?, "SQLite Python .pth changed during recovery");
            fs::remove_file(&self.pth)?;
            sync_dir(self.pth.parent().context("SQLite Python .pth has no parent")?)?;
        }
        if current != journal.previous {
            ensure!(self.current()? == current, "SQLite Python link changed during recovery");
            if let Some(previous) = &journal.previous {
                self.swap(previous)?;
            } else {
                fs::remove_file(&self.current)?;
                sync_dir(&self.root)?;
            }
        }
        ensure!(
            self.current()? == journal.previous && self.pth_exists()? == journal.pth_existed,
            "SQLite Python recovery failed readback"
        );
        self.finish()
    }

    fn activate(&self, release: &Path, verify: impl FnOnce() -> Result<()>) -> Result<()> {
        self.release(release)?;
        let journal = Activation {
            previous: self.current()?,
            candidate: release.to_path_buf(),
            pth_existed: self.pth_exists()?,
        };
        if journal.previous.as_deref() == Some(release) && journal.pth_existed {
            return verify();
        }
        let state = self.journal.parent().context("SQLite Python journal has no parent")?;
        let mut pending = tempfile::NamedTempFile::new_in(state)?;
        serde_json::to_writer(&mut pending, &journal)?;
        pending.as_file().sync_all()?;
        pending.persist_noclobber(&self.journal)?;
        sync_dir(state)?;
        let result = (|| {
            ensure!(
                self.current()? == journal.previous && self.pth_exists()? == journal.pth_existed,
                "Python SQLite settings changed before activation"
            );
            if journal.previous.as_deref() != Some(release) {
                self.swap(release)?;
            }
            if !journal.pth_existed {
                let parent = self.pth.parent().context("SQLite Python .pth has no parent")?;
                let mut pth = tempfile::NamedTempFile::new_in(parent)?;
                pth.write_all(&self.content)?;
                pth.as_file().sync_all()?;
                pth.persist_noclobber(&self.pth)?;
                sync_dir(parent)?;
            }
            verify()?;
            ensure!(
                self.current()?.as_deref() == Some(release) && self.pth_exists()?,
                "SQLite Python activation failed readback"
            );
            Ok(())
        })();
        if let Err(error) = result {
            self.recover()
                .wrap_err_with(|| format!("SQLite Python activation failed: {error:#}; recovery stopped"))?;
            return Err(error);
        }
        self.finish()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Activation {
    previous: Option<PathBuf>,
    candidate: PathBuf,
    pth_existed: bool,
}

fn interpreter(home: &Path) -> Result<(PathBuf, PythonAbi)> {
    let path = Path::new(PYTHON)
        .canonicalize()
        .wrap_err("Homebrew Python 3.15 is unavailable")?;
    let prefix = Path::new("/opt/homebrew/Cellar/python@3.15");
    let path_in_keg = path
        .strip_prefix(prefix)
        .wrap_err("Python 3.15 is outside its Homebrew keg")?;
    let metadata = fs::metadata(&path)?;
    ensure!(
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 && metadata.permissions().mode() & 0o022 == 0,
        "Unsafe Python 3.15 executable"
    );
    let output = command(&path)
        .env("HOME", home)
        .args(["-B", "-P", "-S", "-c", ABI])
        .output_checked_utf8()?;
    let abi: PythonAbi = serde_json::from_str(&output.stdout).wrap_err("Invalid Python 3.15 ABI probe")?;
    ensure!(
        abi.version == [3, 15] && abi.site_enabled,
        "Python 3.15 with user site enabled is required"
    );
    ensure!(
        abi.user_site == home.join(USER_SITE),
        "Unexpected Python 3.15 user site path"
    );
    let keg = prefix.join(path_in_keg.iter().next().context("Missing Python 3.15 keg version")?);
    ensure!(
        abi.extension.starts_with(&keg)
            && abi
                .extension
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "lib-dynload")
            && abi
                .extension
                .file_name()
                .is_some_and(|name| name == "_sqlite3.cpython-315-darwin.so"),
        "Unexpected Homebrew Python SQLite extension or ABI"
    );
    Ok((path, abi))
}

fn libraries(extension: &Path) -> Result<Vec<String>> {
    let _ = open_file(extension, false, FILE_LIMIT)?;
    let stdout = command(Path::new("/usr/bin/otool"))
        .arg("-L")
        .arg(extension)
        .output_checked_utf8()?
        .stdout;
    Ok(stdout
        .lines()
        .skip(1)
        .filter_map(|line| {
            line.trim_start()
                .split_once(" (compatibility version ")
                .map(|(name, _)| name.to_owned())
        })
        .collect())
}

fn verify(python: &Path, home: &Path, directory: &Path, source: &str, isolated: bool) -> Result<()> {
    let mut cmd = command(python);
    cmd.env("HOME", home).args(["-B", "-P"]);
    if isolated {
        cmd.arg("-S").env("PYTHONPATH", directory);
    }
    cmd.args(["-c", PROBE, source]).arg(directory).status_checked()
}

fn verify_runtime(
    python: &Path,
    home: &Path,
    directory: &Path,
    extension_name: &std::ffi::OsStr,
    source: &str,
) -> Result<()> {
    check_dir(directory, false)?;
    let _ = open_file(&directory.join("libsqlite3.dylib"), false, FILE_LIMIT)?;
    let extension = directory.join(extension_name);
    let linkage = libraries(&extension)?;
    ensure!(
        linkage.iter().any(|path| path == NEW_LIBRARY) && !linkage.iter().any(|path| path == OLD_LIBRARY),
        "Staged Python SQLite extension is not linked to its adjacent trunk library"
    );
    command(Path::new("/usr/bin/codesign"))
        .args(["--verify", "--strict"])
        .arg(&extension)
        .status_checked()?;
    verify(python, home, directory, source, true)
}

pub(super) fn run(home: &Path, state: &Path) -> Result<()> {
    let profile = Profile::new(home, state)?;
    profile.recover()?;
    let _ = profile.current()?;
    let _ = profile.pth_exists()?;
    let (python, abi) = interpreter(home)?;
    ensure!(
        libraries(&abi.extension)?.iter().any(|path| path == OLD_LIBRARY),
        "Unexpected Homebrew Python SQLite library linkage"
    );
    let extension_name = abi.extension.file_name().context("Missing SQLite extension name")?;
    let digest = || super::super::sha256(open_file(&abi.extension, false, FILE_LIMIT)?);
    let hash = digest()?;
    let (source, source_id) = ensure_cli(home, state)?;
    let releases = owned_directories(&profile.root, Path::new("releases"), true)?;
    let release = releases.join(format!("{}-{}", source.revision, &hash[..12]));
    if !exists(&release)? {
        println!("Building Python SQLite library: {}", source.revision);
        let staging = tempfile::Builder::new()
            .prefix(".sqlite-build.")
            .tempdir_in(&profile.root)?;
        let build = staging.path().join("build");
        let runtime = staging.path().join("runtime");
        fs::create_dir(&build)?;
        fs::create_dir(&runtime)?;
        source.build(&build, "libsqlite3.dylib")?;
        let library = build.join("libsqlite3.dylib").canonicalize()?;
        ensure!(
            library.starts_with(&build),
            "SQLite build library points outside its staging directory"
        );
        let _ = open_file(&library, false, FILE_LIMIT)?;
        let staged_library = runtime.join("libsqlite3.dylib");
        fs::copy(library, &staged_library)?;
        let extension = runtime.join(extension_name);
        fs::copy(&abi.extension, &extension)?;
        ensure!(
            super::super::sha256(open_file(&extension, false, FILE_LIMIT)?)? == hash,
            "Homebrew Python SQLite extension changed before it was copied"
        );
        tool_command(Path::new("/usr/bin/install_name_tool"))?
            .args(["-change", OLD_LIBRARY, NEW_LIBRARY])
            .arg(&extension)
            .status_checked()?;
        command(Path::new("/usr/bin/codesign"))
            .args(["--force", "--sign", "-"])
            .arg(&extension)
            .status_checked()?;
        verify_runtime(&python, home, &runtime, extension_name, &source_id)?;
        open_file(&staged_library, false, FILE_LIMIT)?.sync_all()?;
        open_file(&extension, false, FILE_LIMIT)?.sync_all()?;
        sync_dir(&runtime)?;
        ensure!(!exists(&release)?, "Python SQLite release appeared while building");
        fs::rename(runtime, &release)?;
        sync_dir(&releases)?;
    }
    verify_runtime(&python, home, &release, extension_name, &source_id)?;
    ensure!(
        Path::new(PYTHON).canonicalize()? == python && digest()? == hash,
        "Homebrew Python changed during SQLite preparation"
    );
    source.check()?;
    owned_directories(home, Path::new(USER_SITE), true)?;
    profile.activate(&release, || verify(&python, home, &profile.current, &source_id, false))?;
    println!("Python 3.15 SQLite trunk is active and verified: {}", source.revision);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(home: &Path) -> (Profile, PathBuf, PathBuf) {
        let state = owned_directories(home, Path::new(".local/state/sqlite-trunk"), true).unwrap();
        owned_directories(home, Path::new(USER_SITE), true).unwrap();
        let profile = Profile::new(home, &state).unwrap();
        let releases = owned_directories(&profile.root, Path::new("releases"), true).unwrap();
        let previous = releases.join(format!("{}-{}", "a".repeat(40), "b".repeat(12)));
        let candidate = releases.join(format!("{}-{}", "c".repeat(40), "d".repeat(12)));
        fs::create_dir(&previous).unwrap();
        fs::create_dir(&candidate).unwrap();
        (profile, previous, candidate)
    }

    #[test]
    fn failed_default_import_restores_link_and_leaves_existing_pth() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        let (profile, previous, candidate) = fixture(&home);
        symlink(&previous, &profile.current).unwrap();
        fs::write(&profile.pth, &profile.content).unwrap();
        assert!(
            profile
                .activate(&candidate, || {
                    assert_eq!(profile.current().unwrap(), Some(candidate.clone()));
                    color_eyre::eyre::bail!("injected default interpreter mismatch")
                })
                .is_err()
        );
        assert_eq!(profile.current().unwrap(), Some(previous));
        assert_eq!(fs::read(&profile.pth).unwrap(), profile.content);
        assert!(!profile.journal.exists());
    }

    #[test]
    fn interrupted_first_activation_removes_only_its_link_and_pth() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        let (profile, _, candidate) = fixture(&home);
        symlink(&candidate, &profile.current).unwrap();
        fs::write(&profile.pth, &profile.content).unwrap();
        let journal = Activation {
            previous: None,
            candidate,
            pth_existed: false,
        };
        fs::write(&profile.journal, serde_json::to_vec(&journal).unwrap()).unwrap();
        profile.recover().unwrap();
        assert!(!profile.current.exists());
        assert!(!profile.pth.exists());
        assert!(!profile.journal.exists());
    }

    #[test]
    fn recovery_preserves_unexpected_pth_and_current() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        let (profile, previous, candidate) = fixture(&home);
        symlink(&candidate, &profile.current).unwrap();
        fs::write(&profile.pth, "external edit").unwrap();
        let journal = Activation {
            previous: Some(previous),
            candidate: candidate.clone(),
            pth_existed: false,
        };
        fs::write(&profile.journal, serde_json::to_vec(&journal).unwrap()).unwrap();
        assert!(profile.recover().is_err());
        assert_eq!(profile.current().unwrap(), Some(candidate));
        assert_eq!(fs::read(&profile.pth).unwrap(), b"external edit");
        assert!(profile.journal.exists());
    }
}
