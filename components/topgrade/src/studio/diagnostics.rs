//! Native equivalents of the managed linkage audit and tool-version report.
use super::{account_home, command};
use crate::{command::CommandExt, utils::which};
use color_eyre::eyre::{Context, Result, bail, ensure};
use std::{
    path::{Path, PathBuf},
    process::Output,
};

const BREW: &str = "/opt/homebrew/bin/brew";

fn probe(executable: &Path, args: &[&str]) -> Result<Output> {
    command(executable)
        .args(args)
        .env("HOME", account_home()?)
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .output_checked_with(|_| Ok(()))
        .wrap_err_with(|| format!("Run {}", executable.display()))
}

pub(super) fn linkage_failed(output: &Output) -> bool {
    !output.status.success()
        || [&output.stdout, &output.stderr].into_iter().any(|stream| {
            String::from_utf8_lossy(stream).lines().any(|line| {
                let line = line.trim_start();
                line.starts_with("Missing libraries:") || line.starts_with("Broken dependencies:")
            })
        })
}

pub(super) fn run_linkage() -> Result<()> {
    let output = probe(Path::new(BREW), &["linkage"])?;
    if linkage_failed(&output) {
        print!("{}", String::from_utf8_lossy(&output.stdout));
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        bail!("Homebrew linkage audit failed ({})", output.status);
    }
    println!("PASS: No missing or broken Homebrew libraries");
    Ok(())
}

type VersionProbe = (&'static str, &'static str, &'static str, &'static str);

const VERSION_TOOLS: &[VersionProbe] = &[
    ("Topgrade", "topgrade", "--version", "topgrade "),
    ("Rust", "rustc", "--version", "rustc "),
    ("Cargo", "cargo", "--version", "cargo "),
    ("uv", "uv", "--version", ""),
    ("Python", "python3", "--version", "Python "),
    ("Node.js", "node", "--version", ""),
    ("npm", "npm", "--version", ""),
    ("pnpm", "pnpm", "--version", ""),
    ("Yarn", "yarn", "--version", ""),
    ("Deno", "deno", "--version", ""),
    ("Git", "git", "--version", "git version "),
    ("GitHub CLI", "gh", "--version", "gh version "),
    ("Claude Code", "claude", "--version", ""),
    ("Codex", "codex", "--version", "codex-cli "),
    ("CMake", "cmake", "--version", "cmake version "),
    ("Ninja", "ninja", "--version", ""),
    ("Zig", "zig", "version", ""),
    ("Go", "go", "version", "go version "),
    ("yt-dlp", "yt-dlp", "--version", ""),
    ("ExifTool", "exiftool", "-ver", ""),
    ("ripgrep", "rg", "--version", "ripgrep "),
    ("sing-box", "sing-box", "version", "sing-box version "),
];

fn resolve(home: &Path, tool: &str) -> Option<PathBuf> {
    match tool {
        "topgrade" => std::env::current_exe().ok(),
        "codex" => {
            let embedded = PathBuf::from(
                "/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex",
            );
            if embedded.is_file() {
                Some(embedded)
            } else {
                which(tool).ok().flatten()
            }
        }
        "rustc" | "cargo" => which(tool).ok().flatten().or_else(|| {
            let shim = home.join(".cargo/bin").join(tool);
            shim.is_file().then_some(shim)
        }),
        _ => which(tool).ok().flatten(),
    }
}

fn reported_version(path: &Path, arg: &str, prefix: &str, label: &str) -> Result<String> {
    let result = if arg.is_empty() {
        probe(path, &[])?
    } else {
        probe(path, &[arg])?
    };
    ensure!(
        result.status.success(),
        "{label} version probe exited with {}",
        result.status
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);
    let line = stdout
        .lines()
        .chain(stderr.lines())
        .find(|line| !line.trim().is_empty())
        .ok_or_else(|| color_eyre::eyre::eyre!("{label} returned no version"))?;
    let value = line.strip_prefix(prefix).unwrap_or(line).trim();
    let value = match label {
        "Claude Code" => value.strip_suffix(" (Claude Code)").unwrap_or(value),
        "FFmpeg" | "FFprobe" => value.split(" Copyright (c)").next().unwrap_or(value),
        _ => value,
    };
    let value = if label == "ImageMagick" {
        let words = value
            .split_whitespace()
            .filter(|part| !part.starts_with("https://") && !part.starts_with("http://"))
            .collect::<Vec<_>>();
        let value = words.join(" ").replace("(Beta)", "Beta");
        let (base, embedded) = value.rsplit_once(' ').unwrap_or(("", &value));
        match embedded.split_once(':') {
            Some((hash, date))
                if hash.len() >= 7
                    && hash.bytes().all(|b| b.is_ascii_hexdigit())
                    && date.len() == 8
                    && date.bytes().all(|b| b.is_ascii_digit()) =>
            {
                format!("{base} | embedded {hash}@{date}")
            }
            _ => value,
        }
    } else {
        value.to_owned()
    };
    Ok(value.chars().filter(|c| !c.is_control()).collect())
}

fn head_revision(value: &str) -> String {
    if let Some((head, revision)) = value.rsplit_once('_')
        && let Some(hash) = head.strip_prefix("HEAD-")
        && !hash.is_empty()
        && hash.bytes().all(|b| b.is_ascii_hexdigit())
        && !revision.is_empty()
        && revision.bytes().all(|b| b.is_ascii_digit())
    {
        return format!("{head} (rev {revision})");
    }
    value.into()
}

fn brew_version(formula: &str) -> Option<String> {
    let path = |flag| {
        let output = probe(Path::new(BREW), &[flag, formula]).ok()?;
        let path = PathBuf::from(String::from_utf8(output.stdout).ok()?.trim());
        (output.status.success() && path.is_absolute()).then_some(path)
    };
    let prefix = path("--prefix")?;
    if !prefix.is_symlink() || !prefix.is_dir() {
        return None;
    }
    let keg = prefix.canonicalize().ok()?;
    let rack = path("--cellar")?.canonicalize().ok()?;
    if keg.parent()? != rack {
        return None;
    }
    Some(head_revision(keg.file_name()?.to_str()?))
}

pub(super) fn run_versions() -> Result<()> {
    let home = account_home()?;
    let mut failed = Vec::new();
    println!("{:<18} Installed / Reported", "Tool");
    let mut report = |label: &str, program, arg, prefix, formula: Option<&str>, show_absent| {
        let executable = resolve(&home, program);
        if executable.is_none() && !show_absent {
            return;
        }
        let reported = executable
            .as_ref()
            .and_then(|path| match reported_version(path, arg, prefix, label) {
                Ok(value) => Some(value),
                Err(error) => {
                    eprintln!("{error:#}");
                    failed.push(label.to_owned());
                    None
                }
            });
        let brew = formula.and_then(brew_version);
        let version = match (brew, reported) {
            (Some(brew), Some(reported)) => format!("{brew} | {reported}"),
            (Some(value), None) | (None, Some(value)) => value,
            (None, None) => "—".into(),
        };
        println!("{label:<18} {version}");
    };
    for &(label, program, arg, prefix) in VERSION_TOOLS {
        report(label, program, arg, prefix, None, program == "codex");
    }
    for (label, program, arg, prefix, formula, show_absent) in [
        ("FFmpeg", "ffmpeg", "-version", "ffmpeg version ", "ffmpeg", false),
        ("FFprobe", "ffprobe", "-version", "ffprobe version ", "ffmpeg", false),
        ("JPEG XL / cjxl", "cjxl", "--version", "cjxl ", "jpeg-xl", false),
        ("JPEG XL / djxl", "djxl", "--version", "djxl ", "jpeg-xl", false),
        ("libavif", "avifenc", "--version", "Version:", "libavif", true),
        (
            "ImageMagick",
            "magick",
            "--version",
            "Version: ImageMagick",
            "imagemagick",
            true,
        ),
        ("OpenCV", "opencv_version", "", "", "opencv", true),
        ("llama.cpp", "llama-cli", "--version", "version:", "llama.cpp", true),
    ] {
        report(label, program, arg, prefix, Some(formula), show_absent);
    }
    println!(
        "{:<18} {}",
        "libssh2",
        brew_version("libssh2").as_deref().unwrap_or("—")
    );
    ensure!(failed.is_empty(), "Version probes failed: {}", failed.join(", "));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::unix::process::ExitStatusExt, process::ExitStatus};

    #[test]
    fn brew_linkage_catches_success_with_broken_sections() {
        let mut output = Output {
            status: ExitStatus::from_raw(0),
            stdout: b"All linked libraries\n".to_vec(),
            stderr: vec![],
        };
        assert!(!linkage_failed(&output));
        output.stderr = b"   Missing libraries:\n".to_vec();
        assert!(linkage_failed(&output));
        output.stderr.clear();
        output.status = ExitStatus::from_raw(1 << 8);
        assert!(linkage_failed(&output));
    }

    #[test]
    fn retains_active_keg_revision() {
        assert_eq!(head_revision("HEAD-20e3671_7"), "HEAD-20e3671 (rev 7)");
        assert_eq!(head_revision("23.1.4_1"), "23.1.4_1");
        assert_eq!(head_revision("HEAD-foo_1"), "HEAD-foo_1");
    }
}
