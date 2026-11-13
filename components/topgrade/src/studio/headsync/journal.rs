use std::{
    fs::{self, DirBuilder, File, FileTimes, Metadata, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

use color_eyre::eyre::{Context, Result, ensure};
use nix::libc;
use regex::Regex;

use super::super::{check_dir, check_file, exists, open_file};

const FILES: [(&str, &str); 2] = [("g", "ggml.rb"), ("l", "llama.cpp.rb")];
const WHISPER: [(&str, &str); 1] = [("w", "whisper.cpp.rb")];
const MAX_FORMULA: u64 = 1024 * 1024;
const CMAKE: &str = r#"    system "cmake", "-S", ".", "-B", "build", *args, *std_cmake_args"#;
const CMAKE_VENDORED: &str = r#"    source_dir = "."

    if build.head?
      # llama.cpp vendored ggml intentionally lacks standalone ggml.pc.in.
      (buildpath/"ggml/ggml.pc.in").write <<~PC
        prefix=@CMAKE_INSTALL_PREFIX@
        exec_prefix=${prefix}
        includedir=${prefix}/@CMAKE_INSTALL_INCLUDEDIR@
        libdir=${prefix}/@CMAKE_INSTALL_LIBDIR@

        Name: ggml
        Description: The GGML Tensor Library for Machine Learning
        Version: @GGML_VERSION@
        Cflags: -I${includedir}
        Libs: -L${libdir} -lggml
      PC

      source_dir = "ggml"
    end

    system "cmake", "-S", source_dir, "-B", "build", *args, *std_cmake_args"#;

pub(super) fn lock(state: &Path) -> Result<File> {
    DirBuilder::new().recursive(true).mode(0o700).create(state)?;
    check_dir(state, false)?;
    let path = state.join("lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)?;
    check_file(&file, &path, false, 0)?;
    // The retained zsh updater uses the same fcntl record lock and inode.
    let record = libc::flock {
        l_start: 0,
        l_len: 0,
        l_pid: 0,
        l_type: libc::F_WRLCK as _,
        l_whence: libc::SEEK_SET as _,
    };
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &record) } == -1 {
        return Err(io::Error::last_os_error()).wrap_err("Could not acquire HEAD-sync lock");
    }
    Ok(file)
}

pub(super) struct Journal {
    state: PathBuf,
    core: PathBuf,
    active: PathBuf,
    files: &'static [(&'static str, &'static str)],
}

impl Journal {
    pub(super) fn new(state: PathBuf, core: PathBuf) -> Result<Self> {
        Self::with_files(state, core, "active", &FILES)
    }

    pub(super) fn whisper(&self) -> Self {
        Self {
            state: self.state.clone(),
            core: self.core.clone(),
            active: self.state.join("whisper-formula-active"),
            files: &WHISPER,
        }
    }

    pub(super) fn pending(&self) -> Result<bool> {
        exists(&self.active)
    }

    fn with_files(
        state: PathBuf,
        core: PathBuf,
        active: &str,
        files: &'static [(&'static str, &'static str)],
    ) -> Result<Self> {
        for &(directory, _) in files {
            let parent = core.join("Formula").join(directory);
            ensure!(
                parent.canonicalize()? == parent,
                "Unexpected Formula directory: {}",
                parent.display()
            );
        }
        let active = state.join(active);
        Ok(Self {
            state,
            core,
            active,
            files,
        })
    }

    pub(super) fn core(&self) -> &Path {
        &self.core
    }

    pub(super) fn prepare(&self, revision: &str, syntax_check: impl Fn(&Path) -> Result<()>) -> Result<()> {
        self.prepare_patch(|source, name| patch(source, name, revision), syntax_check)
    }

    pub(super) fn prepare_whisper(&self, syntax_check: impl Fn(&Path) -> Result<()>) -> Result<()> {
        let path = self.core.join("Formula/w");
        ensure!(path.canonicalize()? == path, "Unexpected Whisper formula directory");
        self.prepare_patch(|source, _| whisper_patch(source), syntax_check)
    }

    fn prepare_patch(
        &self,
        patch_formula: impl Fn(&str, &str) -> Result<String>,
        syntax_check: impl Fn(&Path) -> Result<()>,
    ) -> Result<()> {
        ensure!(!exists(&self.active)?, "HEAD-sync transaction already exists");
        let stage = tempfile::Builder::new().prefix(".stage.").tempdir_in(&self.state)?;
        let original = stage.path().join("original");
        let patched = stage.path().join("patched");
        for path in [&original, &patched] {
            DirBuilder::new().mode(0o700).create(path)?;
        }
        for &(directory, name) in self.files {
            let (dest, pending) = self.paths(directory, name);
            ensure!(!exists(&pending)?, "unexpected pending file beside {}", dest.display());
            let snapshot = original.join(name);
            copy(&dest, &snapshot)?;
            let updated = patched.join(name);
            let source = String::from_utf8(contents(&snapshot)?)?;
            let updated_source = patch_formula(&source, name)?;
            copy(&snapshot, &updated)?;
            let mut output = OpenOptions::new()
                .write(true)
                .truncate(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&updated)?;
            output.write_all(updated_source.as_bytes())?;
            output.sync_all()?;
            syntax_check(&updated)?;
        }
        for &(directory, name) in self.files {
            let (dest, pending) = self.paths(directory, name);
            ensure!(!exists(&pending)?, "unexpected pending file beside {}", dest.display());
            ensure!(
                contents(&dest)? == contents(&original.join(name))?
                    && same_metadata(&dest.metadata()?, &original.join(name).metadata()?),
                "Formula changed during preparation: {}",
                dest.display()
            );
        }

        for path in [original.as_path(), patched.as_path(), stage.path()] {
            sync_dir(path)?;
        }
        ensure!(!exists(&self.active)?, "HEAD-sync transaction already exists");
        fs::rename(stage.path(), &self.active)?;
        let _ = stage.keep();
        sync_dir(&self.state)?;
        for &(directory, name) in self.files {
            let (dest, pending) = self.paths(directory, name);
            let original = self.active.join("original").join(name);
            replace(&dest, &pending, &original, &self.active.join("patched").join(name))?;
        }
        Ok(())
    }

    pub(super) fn recover(&self) -> Result<()> {
        if !exists(&self.active)? {
            return Ok(());
        }
        println!("== Recover HEAD-sync transaction ==");
        let original = self.active.join("original");
        let patched = self.active.join("patched");
        for path in [&self.active, &original, &patched] {
            check_dir(path, false).wrap_err("incomplete or unsafe transaction journal")?;
        }
        // Validate both formulae and pending paths before changing either.
        for &(directory, name) in self.files {
            let (dest, pending) = self.paths(directory, name);
            let original = contents(&original.join(name)).wrap_err("incomplete or unsafe transaction journal")?;
            let patched = contents(&patched.join(name)).wrap_err("incomplete or unsafe transaction journal")?;
            let current = contents(&dest)?;
            ensure!(
                current == original || current == patched,
                "Formula changed outside this transaction: {}. Journal preserved",
                dest.display()
            );
            if exists(&pending)? {
                open_file(&pending, false, MAX_FORMULA).wrap_err("Unsafe HEAD-sync pending file")?;
            }
        }
        for &(directory, name) in self.files {
            let (dest, pending) = self.paths(directory, name);
            if exists(&pending)? {
                fs::remove_file(&pending)?;
                sync_dir(dest.parent().expect("Formula path has a parent"))?;
            }
            let original = original.join(name);
            if contents(&dest)? != contents(&original)? {
                replace(&dest, &pending, &patched.join(name), &original)?;
            }
        }
        // Move the complete journal out of the active name before recursive cleanup.
        let retired = tempfile::Builder::new().prefix(".recovered.").tempdir_in(&self.state)?;
        fs::rename(&self.active, retired.path().join("journal"))?;
        let retired = retired.keep();
        sync_dir(&retired)?;
        sync_dir(&self.state)?;
        fs::remove_dir_all(&retired)?;
        sync_dir(&self.state)?;
        println!("HEAD-sync transaction recovered");
        Ok(())
    }

    fn paths(&self, directory: &str, name: &str) -> (PathBuf, PathBuf) {
        let directory = self.core.join("Formula").join(directory);
        (directory.join(name), directory.join(format!("{name}.headsync-pending")))
    }
}

fn new_file(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?)
}

fn contents(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    open_file(path, false, MAX_FORMULA)?
        .take(MAX_FORMULA + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= MAX_FORMULA, "Formula grew while reading");
    Ok(bytes)
}

fn same_metadata(left: &Metadata, right: &Metadata) -> bool {
    left.mode() == right.mode()
        && left.uid() == right.uid()
        && left.gid() == right.gid()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
}

fn copy(from: &Path, to: &Path) -> Result<()> {
    let source = open_file(from, false, MAX_FORMULA)?;
    let metadata = source.metadata()?;
    let target = new_file(to)?;
    // Use opened descriptors so metadata copying never follows a pending symlink.
    if unsafe {
        libc::fcopyfile(
            source.as_raw_fd(),
            target.as_raw_fd(),
            std::ptr::null_mut(),
            libc::COPYFILE_DATA | libc::COPYFILE_METADATA,
        )
    } == -1
    {
        return Err(io::Error::last_os_error()).wrap_err_with(|| format!("Could not copy {}", from.display()));
    }
    target.set_times(
        FileTimes::new()
            .set_accessed(metadata.accessed()?)
            .set_modified(metadata.modified()?),
    )?;
    ensure!(
        same_metadata(&metadata, &target.metadata()?),
        "Could not preserve Formula mode, owner or mtime: {}",
        from.display()
    );
    target.sync_all()?;
    Ok(())
}

fn replace(dest: &Path, pending: &Path, previous: &Path, next: &Path) -> Result<()> {
    copy(next, pending)?;
    ensure!(
        contents(dest)? == contents(previous)?,
        "Formula changed before transaction replacement: {}",
        dest.display()
    );
    fs::rename(pending, dest)?;
    sync_dir(dest.parent().expect("Formula path has a parent"))?;
    ensure!(
        contents(dest)? == contents(next)? && same_metadata(&dest.metadata()?, &next.metadata()?),
        "Formula replacement verification failed: {}",
        dest.display()
    );
    Ok(())
}

fn sync_dir(path: &Path) -> Result<()> {
    File::open(path)?
        .sync_all()
        .wrap_err_with(|| format!("Could not synchronize {}", path.display()))
}

fn patch(source: &str, name: &str, revision: &str) -> Result<String> {
    let repo = regex::escape(name.trim_end_matches(".rb"));
    let head = Regex::new(&format!(
        r#"(?m)^([ \t]*)head[ \t]+"https://github\.com/ggml-org/{repo}\.git",\s*branch:\s*"master"[ \t]*$"#
    ))?;
    ensure!(
        head.find_iter(source).count() == 1,
        "{name} HEAD declaration changed upstream"
    );
    let line = format!(r#"${{1}}head "{}", revision: "{revision}""#, super::LLAMA_REMOTE);
    let mut updated = head.replace(source, line.as_str()).into_owned();
    if name == "ggml.rb" {
        ensure!(
            updated.matches(CMAKE).count() == 1,
            "ggml build layout changed upstream"
        );
        updated = updated.replacen(CMAKE, CMAKE_VENDORED, 1);
    }
    Ok(updated)
}

fn whisper_patch(source: &str) -> Result<String> {
    ensure!(
        source.contains("class WhisperCpp < Formula")
            && source.contains("ggml-org/whisper.cpp/archive/refs/tags/v1.9.5.tar.gz")
            && source.matches(CMAKE).count() == 1,
        "The reviewed Whisper 1.9.5 formula changed; compatibility patch needs review"
    );
    let patch = r#"    inreplace "examples/talk-llama/CMakeLists.txt",
              "find_package(llama 0.6.0 REQUIRED)", "find_package(llama REQUIRED)"
"#;
    Ok(source.replacen(CMAKE, &format!("{patch}{CMAKE}"), 1))
}

#[test]
fn whisper_patch_requires_the_reviewed_formula_and_one_build_call() {
    let source = format!(
        "class WhisperCpp < Formula\nurl \"https://github.com/ggml-org/whisper.cpp/archive/refs/tags/v1.9.5.tar.gz\"\n{CMAKE}"
    );
    assert!(whisper_patch(&source).unwrap().contains("find_package(llama REQUIRED)"));
    assert!(whisper_patch(&source.replace("1.9.5", "1.9.6")).is_err());
    assert!(whisper_patch(&format!("{source}\n{CMAKE}")).is_err());
}

#[cfg(test)]
#[test]
#[ignore = "reads a user-specified Homebrew Core checkout and validates private formula copies"]
#[expect(
    clippy::disallowed_methods,
    reason = "Only this opt-in test invokes Ruby on private copies"
)]
fn validates_local_formula_snapshots() -> Result<()> {
    use crate::command::CommandExt as _;

    let core =
        PathBuf::from(std::env::var_os("TOPGRADE_HEADSYNC_CORE").expect("Specify the existing Homebrew Core checkout"))
            .canonicalize()?;
    let stage = tempfile::tempdir()?;
    let revision = "0123456789abcdef0123456789abcdef01234567";
    for (directory, name) in FILES {
        let source = core.join("Formula").join(directory).join(name);
        let original = contents(&source)?;
        let updated = patch(std::str::from_utf8(&original)?, name, revision)?;
        let staged = stage.path().join(name);
        fs::write(&staged, updated.as_bytes())?;
        std::process::Command::new("/usr/bin/ruby")
            .arg("-c")
            .arg(&staged)
            .status_checked()?;
        assert_eq!(contents(&source)?, original, "Live formula drifted during review");
        assert!(updated.contains(&format!("revision: \"{revision}\"")));
    }
    Ok(())
}
