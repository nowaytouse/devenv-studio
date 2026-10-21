mod gui;
mod workspace_io;

use anyhow::{Context, Result, bail};
use std::{collections::BTreeSet, path::PathBuf, process::ExitCode};
use studio_core::{Profile, Target, load_catalog};

const HELP: &str = "LTSC Studio 0.2
Native desktop: ltsc-studio
List reviewed settings and presets: ltsc-studio catalog
List software/features/maintenance: ltsc-studio tasks
Build mixed plan: ltsc-studio plan --target win11-ltsc-2024 --select explorer-extensions --tasks software-git-git
Choose an editable settings preset: ltsc-studio plan --target win11-ltsc-2024 --preset essentials
Inspect selected current states: ltsc-studio inspect --target win11-ltsc-2024 --select explorer-extensions
Preview JSON plan: ltsc-studio plan --target win11-ltsc-2024 --select explorer-extensions,privacy-advertising-id
Write new plan file: ltsc-studio plan --target win10-ltsc-2021 --select explorer-extensions --output plan.json
Preview saved profile: ltsc-studio plan --profile profile.json
Capture native preview: ltsc-studio --screenshot preview.png

Targets: win10-ltsc-2021, win11-ltsc-2024
All changes are opt-in. CLI commands only inspect/export; apply and recovery use the desktop review flow.
An existing output file is never overwritten.";

#[derive(Debug, PartialEq, Eq)]
struct PlanArgs {
    target: Option<Target>,
    selected: BTreeSet<String>,
    tasks: BTreeSet<String>,
    preset: Option<String>,
    profile: Option<PathBuf>,
    output: Option<PathBuf>,
}

fn parse_target(value: &str) -> Result<Target> {
    match value {
        "win10-ltsc-2021" => Ok(Target::Win10Ltsc2021),
        "win11-ltsc-2024" => Ok(Target::Win11Ltsc2024),
        _ => bail!("Unknown target '{value}'. Use win10-ltsc-2021 or win11-ltsc-2024."),
    }
}

fn parse_plan_args(args: &[String]) -> Result<PlanArgs> {
    let mut result = PlanArgs {
        target: None,
        selected: BTreeSet::new(),
        tasks: BTreeSet::new(),
        preset: None,
        profile: None,
        output: None,
    };
    let mut options = BTreeSet::new();
    let mut index = 0;
    while index < args.len() {
        let flag = &args[index];
        if !matches!(
            flag.as_str(),
            "--target" | "--select" | "--tasks" | "--preset" | "--profile" | "--output"
        ) {
            bail!("Unknown plan option '{flag}'. See --help.");
        }
        if !options.insert(flag.clone()) {
            bail!("Option '{flag}' was supplied more than once.");
        }
        let value = args
            .get(index + 1)
            .with_context(|| format!("Missing value for {flag}"))?;
        if value.is_empty() || value.starts_with("--") {
            bail!("Missing value for {flag}");
        }
        match flag.as_str() {
            "--target" => result.target = Some(parse_target(value)?),
            "--select" => {
                for id in value.split(',') {
                    if id.is_empty() || id.trim() != id || !result.selected.insert(id.to_owned()) {
                        bail!("Selection IDs must be nonempty, unique and separated by commas.");
                    }
                }
            }
            "--tasks" => {
                for id in value.split(',') {
                    if id.is_empty() || id.trim() != id || !result.tasks.insert(id.to_owned()) {
                        bail!("Task IDs must be nonempty, unique and separated by commas.");
                    }
                }
            }
            "--preset" => result.preset = Some(value.to_owned()),
            "--profile" => result.profile = Some(PathBuf::from(value)),
            "--output" => result.output = Some(PathBuf::from(value)),
            _ => unreachable!(),
        }
        index += 2;
    }
    if result.profile.is_some()
        && (result.target.is_some()
            || !result.selected.is_empty()
            || !result.tasks.is_empty()
            || result.preset.is_some())
    {
        bail!("Use --profile on its own, or --target with --select.");
    }
    if result.profile.is_none() && result.target.is_none() {
        bail!("Specify --target, or load a saved --profile.");
    }
    Ok(result)
}

fn execute(args: &[String]) -> Result<()> {
    let Some(command) = args.first() else {
        return gui::run().map_err(|error| anyhow::anyhow!("Cannot start desktop: {error}"));
    };
    match command.as_str() {
        "--help" | "-h" if args.len() == 1 => println!("{HELP}"),
        "catalog" if args.len() == 1 => {
            let catalog = load_catalog()?;
            println!("LTSC Studio catalog {}\n", catalog.revision);
            for action in catalog.actions {
                println!("{}\t{}\t{}", action.id, action.category, action.title);
            }
            println!("\nEditable presets:");
            for preset in catalog.presets {
                println!(
                    "{}\t{}\t{} settings",
                    preset.id,
                    preset.title,
                    preset.actions.len()
                );
            }
        }
        "tasks" if args.len() == 1 => {
            let catalog = studio_deployment::catalog()?;
            println!("LTSC Studio deployment {}\n", catalog.revision);
            for task in catalog.tasks {
                println!(
                    "{}\t{}\t{}\t{}",
                    task.id, task.kind, task.category, task.title
                );
            }
        }
        "plan" | "inspect" => {
            let options = parse_plan_args(&args[1..])?;
            let catalog = load_catalog()?;
            let profile = match options.profile {
                Some(path) => workspace_io::load(&path, &catalog)?,
                None => {
                    let mut selected = options.selected;
                    if let Some(preset_id) = options.preset {
                        let preset = catalog
                            .presets
                            .iter()
                            .find(|preset| preset.id == preset_id)
                            .with_context(|| format!("Unknown preset: {preset_id}"))?;
                        selected.extend(preset.actions.iter().cloned());
                    }
                    workspace_io::WorkspaceProfile {
                        schema_version: 2,
                        settings: Profile {
                            schema_version: 1,
                            target: options.target.context("Missing target")?,
                            selected,
                        },
                        deployment: options.tasks,
                    }
                }
            };
            let plan = workspace_io::plan(
                &catalog,
                profile.settings.target,
                &profile.settings.selected,
                &profile.deployment,
            )?;
            if command == "inspect" {
                if options.output.is_some() {
                    bail!("inspect prints current state; --output is only supported for plan");
                }
                let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                for state in
                    studio_platform::inspect(plan.settings, std::sync::Arc::clone(&cancel))?
                {
                    println!("{}\t{}\t{}", state.id, state.state, state.detail);
                }
                if !profile.deployment.is_empty() {
                    for state in studio_deployment::inspect(
                        profile.settings.target,
                        &profile.deployment,
                        cancel,
                    )? {
                        println!("{}\t{}\t{}", state.id, state.state, state.detail);
                    }
                }
            } else {
                match options.output {
                    Some(path) => {
                        studio_core::export_document(&plan, &path)?;
                        println!(
                            "Saved {} settings and {} deployment tasks to {}",
                            plan.settings.actions.len(),
                            plan.deployment.tasks.len(),
                            path.display()
                        );
                    }
                    None => println!("{}", serde_json::to_string_pretty(&plan)?),
                }
            }
        }
        "--screenshot" if args.len() == 2 => {
            let path = PathBuf::from(&args[1]);
            if path.try_exists()? {
                bail!("Screenshot output already exists: {}", path.display());
            }
            gui::run_with_options(Some(path.clone()))
                .map_err(|error| anyhow::anyhow!("Cannot render native preview: {error}"))?;
            image::open(&path).with_context(|| {
                format!("Screenshot was not successfully saved: {}", path.display())
            })?;
        }
        _ => bail!("Unknown command or extra arguments.\n\n{HELP}"),
    }
    Ok(())
}

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match execute(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("LTSC Studio: {error:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn explicit_target_empty_plan_is_allowed() {
        let args =
            parse_plan_args(&strings(&["--target", "win10-ltsc-2021"])).expect("valid target");
        assert_eq!(args.target, Some(Target::Win10Ltsc2021));
        assert!(args.selected.is_empty());
    }

    #[test]
    fn rejects_ambiguous_or_malformed_selection() {
        for args in [
            vec!["--target"],
            vec!["--target", "windows"],
            vec!["--target", "win11-ltsc-2024", "--select", "a,a"],
            vec!["--target", "win11-ltsc-2024", "--select", "a,"],
            vec!["--profile", "a.json", "--target", "win11-ltsc-2024"],
            vec!["--profile", "a.json", "--select", "a"],
            vec!["--profile", "a.json", "--tasks", "software-git-git"],
            vec!["--profile", "a.json", "--preset", "essentials"],
            vec!["--target", "win11-ltsc-2024", "--tasks", "a,a"],
            vec!["--target", "win11-ltsc-2024", "--tasks", "a,"],
            vec!["--target", "win11-ltsc-2024", "--target", "win10-ltsc-2021"],
            vec!["--profile", "a.json", "--execute", "yes"],
        ] {
            assert!(parse_plan_args(&strings(&args)).is_err(), "{args:?}");
        }
    }

    #[test]
    fn mixed_selection_requires_explicit_target_and_preserves_ids() {
        let options = parse_plan_args(&strings(&[
            "--target",
            "win11-ltsc-2024",
            "--select",
            "explorer-extensions",
            "--tasks",
            "software-git-git,feature-netfx3",
            "--preset",
            "essentials",
        ]))
        .unwrap();
        assert_eq!(
            options.selected,
            BTreeSet::from(["explorer-extensions".into()])
        );
        assert_eq!(
            options.tasks,
            BTreeSet::from(["software-git-git".into(), "feature-netfx3".into()])
        );
        assert_eq!(options.preset.as_deref(), Some("essentials"));
        assert!(parse_plan_args(&strings(&["--tasks", "software-git-git"])).is_err());
    }

    #[test]
    fn profile_and_output_path_preserve_spaces() {
        let args = parse_plan_args(&strings(&[
            "--profile",
            "my profile.json",
            "--output",
            "my plan.json",
        ]))
        .expect("paths are values");
        assert_eq!(args.profile, Some(PathBuf::from("my profile.json")));
        assert_eq!(args.output, Some(PathBuf::from("my plan.json")));
    }
}
