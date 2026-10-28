//! Explicit local Topgrade archives; extraction never installs or executes their contents.
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Backup {
        output: PathBuf,
        attachments: Vec<PathBuf>,
    },
    Extract {
        input: PathBuf,
        output: PathBuf,
        all: bool,
        files: BTreeSet<String>,
    },
}

fn parse(args: &[String]) -> Result<Command> {
    let command = args.first().context("Missing Topgrade command")?.as_str();
    ensure!(
        matches!(command, "topgrade-backup" | "topgrade-extract"),
        "Unknown Topgrade command"
    );
    let mut input = None;
    let mut output = None;
    let mut attachments = Vec::new();
    let mut files = BTreeSet::new();
    let mut all = false;
    let mut index = 1;
    while index < args.len() {
        let flag = args[index].as_str();
        if flag == "--all" && command == "topgrade-extract" {
            ensure!(!all, "--all was supplied more than once");
            all = true;
            index += 1;
            continue;
        }
        ensure!(
            flag == "--output"
                || (command == "topgrade-backup" && flag == "--include")
                || (command == "topgrade-extract" && matches!(flag, "--input" | "--file")),
            "Unknown Topgrade option"
        );
        let value = args
            .get(index + 1)
            .filter(|value| !value.is_empty() && !value.starts_with("--"))
            .with_context(|| format!("Missing value for {flag}"))?;
        match flag {
            "--output" => {
                ensure!(output.is_none(), "--output was supplied more than once");
                output = Some(absolute_path(value)?);
            }
            "--input" => {
                ensure!(input.is_none(), "--input was supplied more than once");
                input = Some(absolute_path(value)?);
            }
            "--include" => {
                let path = absolute_path(value)?;
                ensure!(
                    !attachments.contains(&path),
                    "Duplicate Topgrade attachment"
                );
                attachments.push(path);
            }
            "--file" => {
                ensure!(
                    !value.contains('\\')
                        && Path::new(value)
                            .components()
                            .all(|part| matches!(part, Component::Normal(_))),
                    "--file requires a home-relative archive path without traversal"
                );
                ensure!(
                    files.insert(value.clone()),
                    "Duplicate Topgrade file selection"
                );
            }
            _ => bail!("Unknown Topgrade option"),
        }
        index += 2;
    }
    let output = output.context("Topgrade command requires --output")?;
    if command == "topgrade-backup" {
        Ok(Command::Backup {
            output,
            attachments,
        })
    } else {
        ensure!(
            !all || files.is_empty(),
            "--all cannot be combined with --file"
        );
        ensure!(
            all || !files.is_empty(),
            "Choose --all or one or more --file selections"
        );
        Ok(Command::Extract {
            input: input.context("Topgrade extraction requires --input")?,
            output,
            all,
            files,
        })
    }
}

fn absolute_path(value: &str) -> Result<PathBuf> {
    let path = PathBuf::from(value);
    ensure!(
        path.is_absolute(),
        "Topgrade input, output and attachment paths must be absolute"
    );
    Ok(path)
}

pub fn execute(args: &[String]) -> Result<()> {
    use studio_deployment::topgrade;
    match parse(args)? {
        Command::Backup {
            output,
            attachments,
        } => {
            let profile = topgrade::capture(&topgrade::default_home()?, &attachments)?;
            topgrade::save(&profile, &output)?;
            println!(
                "Saved {} Topgrade configuration/script files to {}. Review the archive before sharing; no command was executed.",
                profile.files.len(),
                output.display()
            );
        }
        Command::Extract {
            input,
            output,
            all,
            files,
        } => {
            let profile = topgrade::load(&input)?;
            let selected = if all {
                profile.files.iter().map(|file| file.path.clone()).collect()
            } else {
                files
            };
            topgrade::restore(&profile, &selected, &output)?;
            println!(
                "Extracted and verified {} Topgrade files in {}. No files were executed; Unix executable bits were not restored. Review local paths and dependencies before manual activation.",
                selected.len(),
                output.display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|value| (*value).into()).collect()
    }

    #[test]
    fn backup_preserves_explicit_attachment_order() {
        let root = std::env::temp_dir();
        let output = root.join("topgrade-backup.json").display().to_string();
        let first = root.join("first.sh").display().to_string();
        let second = root.join("second.py").display().to_string();
        assert_eq!(
            parse(&strings(&[
                "topgrade-backup",
                "--output",
                &output,
                "--include",
                &first,
                "--include",
                &second
            ]))
            .unwrap(),
            Command::Backup {
                output: root.join("topgrade-backup.json"),
                attachments: vec![root.join("first.sh"), root.join("second.py")]
            }
        );
    }

    #[test]
    fn extraction_requires_explicit_complete_or_per_file_selection() {
        let root = std::env::temp_dir();
        let input = root.join("archive.json").display().to_string();
        let output = root.join("new-directory").display().to_string();
        for selection in [
            vec!["--all"],
            vec![
                "--file",
                ".config/topgrade.toml",
                "--file",
                ".local/bin/example.sh",
            ],
        ] {
            let mut args = strings(&["topgrade-extract", "--input", &input, "--output", &output]);
            args.extend(strings(&selection));
            assert!(matches!(parse(&args).unwrap(), Command::Extract { .. }));
        }
        for tail in [
            vec![],
            vec!["--all", "--file", ".config/topgrade.toml"],
            vec!["--all", "--all"],
            vec!["--file", "../escape"],
            vec!["--file", "a\\b"],
            vec![
                "--file",
                ".config/topgrade.toml",
                "--file",
                ".config/topgrade.toml",
            ],
        ] {
            let mut args = strings(&["topgrade-extract", "--input", &input, "--output", &output]);
            args.extend(strings(&tail));
            assert!(
                parse(&args).is_err(),
                "Accepted invalid selection: {tail:?}"
            );
        }
    }

    #[test]
    fn malformed_options_fail_before_accessing_the_device() {
        let absolute = std::env::temp_dir()
            .join("topgrade-test")
            .display()
            .to_string();
        for args in [
            vec![],
            vec!["other"],
            vec!["topgrade-backup"],
            vec!["topgrade-backup", "--output"],
            vec!["topgrade-backup", "--output", "relative.json"],
            vec![
                "topgrade-backup",
                "--output",
                &absolute,
                "--include",
                "relative.sh",
            ],
            vec![
                "topgrade-backup",
                "--output",
                &absolute,
                "--include",
                &absolute,
                "--include",
                &absolute,
            ],
            vec![
                "topgrade-backup",
                "--output",
                &absolute,
                "--output",
                &absolute,
            ],
            vec!["topgrade-backup", "--input", &absolute],
            vec!["topgrade-backup", "--output", &absolute, "unexpected"],
            vec!["topgrade-extract", "--output", &absolute, "--all"],
            vec![
                "topgrade-extract",
                "--input",
                &absolute,
                "--input",
                &absolute,
                "--output",
                &absolute,
                "--all",
            ],
            vec![
                "topgrade-extract",
                "--input",
                "relative.json",
                "--output",
                &absolute,
                "--all",
            ],
            vec![
                "topgrade-extract",
                "--input",
                &absolute,
                "--output",
                &absolute,
                "--file",
            ],
            vec![
                "topgrade-extract",
                "--input",
                &absolute,
                "--output",
                &absolute,
                "--include",
                &absolute,
            ],
        ] {
            assert!(
                parse(&strings(&args)).is_err(),
                "Accepted invalid arguments: {args:?}"
            );
        }
    }
}
