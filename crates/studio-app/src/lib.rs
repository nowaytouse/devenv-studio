mod backup_migration;
mod catalog_locale;
mod gui;
mod migration;
mod migration_locale;
mod prefs;
mod solutions;
mod topgrade_cli;
mod topgrade_engine;
mod workspace_io;

use anyhow::{Context, Result, bail};
use std::{collections::BTreeSet, path::PathBuf, process::ExitCode};
use studio_core::{Profile, Target, load_catalog};

const HELP: &str = concat!("DevEnv Studio ", env!("CARGO_PKG_VERSION"), "\nNative desktop: devenv-studio
List reviewed settings and presets: devenv-studio catalog
List software/features/maintenance: devenv-studio tasks
List reviewed AC power choices: devenv-studio power
Inspect developer provider candidates: devenv-studio providers
Back up native macOS or Windows developer inventory: devenv-studio backup --output /absolute/path/environment-backup.json
Back up Topgrade configs and explicit scripts: devenv-studio topgrade-backup --output /absolute/path/topgrade.json [--include /absolute/path/script]\nExtract reviewed files to a new directory: devenv-studio topgrade-extract --input /absolute/path/topgrade.json --output /absolute/new-directory --all\nRead delete-notification policy only: devenv-studio storage-status
Add opt-in AC choices to a plan: devenv-studio plan --target win11-ltsc-2024 --power <choice-id>
Build mixed plan: devenv-studio plan --target win11-ltsc-2024 --select explorer-extensions --tasks software-git-git
Mixed workspace recipe: devenv-studio plan --target win11-ltsc-2024 --recipe rust
Review legacy migration: devenv-studio migrate --target win11-ltsc-2024 --input legacy.json --output migration-report.json
Choose an editable settings preset: devenv-studio plan --target win11-ltsc-2024 --preset essentials
Inspect selected current states: devenv-studio inspect --target win11-ltsc-2024 --select explorer-extensions
Preview JSON plan: devenv-studio plan --target win11-ltsc-2024 --select explorer-extensions,privacy-advertising-id
Write new plan file: devenv-studio plan --target win10-ltsc-2021 --select explorer-extensions --output plan.json
Preview saved profile: devenv-studio plan --profile profile.json
Capture synthetic native preview: devenv-studio --screenshot preview.png
Apple Silicon Topgrade engine: devenv-studio topgrade-engine <plan|prepare|prepare-upstream|install|rollback> <absolute checkout> <full commit>
Topgrade upstream tracking: devenv-studio topgrade-engine <check-upstream|record-upstream|export-upstream-agent> <absolute checkout>
Apple Silicon rollback: devenv-studio topgrade-engine rollback-current <active component tree> <previous component tree>
Apple Silicon verification: devenv-studio topgrade-engine verify-current <active component tree>

Compatibility command: ltsc-studio (same implementation as devenv-studio).
Targets for Windows system configuration: win10-ltsc-2021, win11-ltsc-2024
All changes are opt-in. Windows apply and recovery use the desktop review flow.
An existing output file is never overwritten.");

#[derive(Debug, PartialEq, Eq)]
struct PlanArgs {
    target: Option<Target>,
    selected: BTreeSet<String>,
    tasks: BTreeSet<String>,
    power: BTreeSet<String>,
    preset: Option<String>,
    recipe: Option<String>,
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
        power: BTreeSet::new(),
        preset: None,
        recipe: None,
        profile: None,
        output: None,
    };
    let mut options = BTreeSet::new();
    let mut index = 0;
    while index < args.len() {
        let flag = &args[index];
        if !matches!(
            flag.as_str(),
            "--target"
                | "--select"
                | "--tasks"
                | "--power"
                | "--preset"
                | "--recipe"
                | "--profile"
                | "--output"
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
            "--power" => {
                for id in value.split(',') {
                    if id.is_empty() || id.trim() != id || !result.power.insert(id.to_owned()) {
                        bail!("Power IDs must be nonempty, unique and separated by commas.");
                    }
                }
            }
            "--preset" => result.preset = Some(value.to_owned()),
            "--recipe" => result.recipe = Some(value.to_owned()),
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
            || !result.power.is_empty()
            || result.preset.is_some()
            || result.recipe.is_some())
    {
        bail!("Use --profile on its own, or --target with --select.");
    }
    if result.profile.is_none() && result.target.is_none() {
        bail!("Specify --target, or load a saved --profile.");
    }
    Ok(result)
}

fn parse_migration_args(args: &[String]) -> Result<(Target, PathBuf, Option<PathBuf>)> {
    let mut values = std::collections::BTreeMap::new();
    let (pairs, remainder) = args.as_chunks::<2>();
    if !remainder.is_empty() {
        bail!("Every option requires a value");
    }
    for pair in pairs {
        let flag = pair[0].as_str();
        if !matches!(flag, "--target" | "--input" | "--output") {
            bail!("Unknown migration option {flag}");
        }
        if pair[1].is_empty()
            || pair[1].starts_with("--")
            || values.insert(flag, pair[1].as_str()).is_some()
        {
            bail!("Missing or duplicate migration option {flag}");
        }
    }
    let target = parse_target(
        values
            .get("--target")
            .context("Migration requires --target")?,
    )?;
    let input = PathBuf::from(
        values
            .get("--input")
            .context("Migration requires --input")?,
    );
    Ok((target, input, values.get("--output").map(PathBuf::from)))
}
fn execute(args: &[String]) -> Result<()> {
    let Some(command) = args.first() else {
        return gui::run().map_err(|error| anyhow::anyhow!("Cannot start desktop: {error}"));
    };
    match command.as_str() {
        "--help" | "-h" if args.len() == 1 => println!("{HELP}"),
        "--version" | "-V" if args.len() == 1 => {
            println!("DevEnv Studio {}", env!("CARGO_PKG_VERSION"))
        }
        "catalog" if args.len() == 1 => {
            let catalog = load_catalog()?;
            println!("DevEnv Studio catalog {}\n", catalog.revision);
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
            println!("DevEnv Studio deployment {}\n", catalog.revision);
            for task in catalog.tasks {
                println!(
                    "{}\t{}\t{}\t{}",
                    task.id, task.kind, task.category, task.title
                );
            }
        }
        "power" if args.len() == 1 => {
            for choice in studio_platform::power::catalog()? {
                println!(
                    "{}\t{}\tAC={}\t{}",
                    choice.id, choice.group, choice.value, choice.title[0]
                );
            }
        }
        "topgrade-backup" | "topgrade-extract" => topgrade_cli::execute(args)?,
        "topgrade-engine" => topgrade_engine::execute(&args[1..])?,
        "backup" if args.len() == 3 && args[1] == "--output" => {
            let path = PathBuf::from(&args[2]);
            anyhow::ensure!(path.is_absolute(), "Backup output must be an absolute path");
            anyhow::ensure!(!path.try_exists()?, "Backup output already exists");
            let snapshot = studio_deployment::backup::collect(std::sync::Arc::new(
                std::sync::atomic::AtomicBool::new(false),
            ))?;
            studio_deployment::backup::save(&snapshot, &path)?;
            let complete = snapshot
                .providers
                .iter()
                .filter(|p| p.status == "complete")
                .count();
            let failed = snapshot
                .providers
                .iter()
                .filter(|p| p.status == "failed")
                .count();
            println!(
                "Saved local inventory to {}: {complete} complete providers, {failed} failed. Inspect provider states before migration.",
                path.display()
            );
        }
        "providers" if args.len() == 1 => {
            let providers: Vec<_> = studio_deployment::provider_capabilities()
                .into_iter()
                .map(|provider| {
                    serde_json::json!({
                        "provider": provider.provider,
                        "available": provider.available,
                        "detail": provider.detail,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&providers)?);
        }
        "storage-status" if args.len() == 1 => {
            let status = studio_deployment::storage_status(std::sync::Arc::new(
                std::sync::atomic::AtomicBool::new(false),
            ))?;
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
        "migrate" => {
            let (target, input, output) = parse_migration_args(&args[1..])?;
            let report = migration::load(&input, target)?;
            if let Some(path) = output {
                studio_core::export_document(&report, &path)?;
            } else {
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
        }
        "plan" | "inspect" => {
            let options = parse_plan_args(&args[1..])?;
            let catalog = load_catalog()?;
            let profile = match options.profile {
                Some(path) => workspace_io::load(&path, &catalog)?,
                None => {
                    let mut selected = options.selected;
                    let mut tasks = options.tasks;
                    if let Some(recipe_id) = options.recipe {
                        let recipe = solutions::RECIPES
                            .iter()
                            .find(|recipe| recipe.id == recipe_id)
                            .context("Unknown recipe")?;
                        let (recipe_settings, recipe_tasks) =
                            solutions::resolve(recipe, options.target.context("Missing target")?)?;
                        selected.extend(recipe_settings);
                        tasks.extend(recipe_tasks);
                    }
                    if let Some(preset_id) = options.preset {
                        let preset = catalog
                            .presets
                            .iter()
                            .find(|preset| preset.id == preset_id)
                            .with_context(|| format!("Unknown preset: {preset_id}"))?;
                        selected.extend(preset.actions.iter().cloned());
                    }
                    workspace_io::WorkspaceProfile {
                        schema_version: 3,
                        settings: Profile {
                            schema_version: 1,
                            target: options.target.context("Missing target")?,
                            selected,
                        },
                        deployment: tasks,
                        power: options.power,
                    }
                }
            };
            let plan = workspace_io::plan(
                &catalog,
                profile.settings.target,
                &profile.settings.selected,
                &profile.deployment,
                &profile.power,
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
                for state in
                    studio_platform::power::inspect(plan.power, std::sync::Arc::clone(&cancel))?
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
                            "Saved {} settings, {} deployment tasks and {} power choices to {}",
                            plan.settings.actions.len(),
                            plan.deployment.tasks.len(),
                            plan.power.choices.len(),
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

/// Shared desktop and CLI entry point, also used by the legacy executable alias.
pub fn run() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match execute(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("DevEnv Studio: {error:#}");
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
    fn migration_arguments_require_unambiguous_input_and_target() {
        let (target, path, output) = parse_migration_args(&strings(&[
            "--target",
            "win11-ltsc-2024",
            "--input",
            "old profile.json",
            "--output",
            "report.json",
        ]))
        .unwrap();
        assert_eq!(target, Target::Win11Ltsc2024);
        assert_eq!(path, PathBuf::from("old profile.json"));
        assert_eq!(output, Some(PathBuf::from("report.json")));
        for args in [
            vec!["--target", "win11-ltsc-2024", "--input"],
            vec!["--input", "old.json"],
            vec![
                "--target",
                "win11-ltsc-2024",
                "--input",
                "old.json",
                "--input",
                "other.json",
            ],
            vec![
                "--target",
                "win11-ltsc-2024",
                "--input",
                "old.json",
                "--execute",
                "yes",
            ],
        ] {
            assert!(parse_migration_args(&strings(&args)).is_err());
        }
        assert!(parse_plan_args(&strings(&["--profile", "old.json", "--recipe", "rust"])).is_err());
        assert_eq!(
            parse_plan_args(&strings(&[
                "--target",
                "win11-ltsc-2024",
                "--recipe",
                "rust"
            ]))
            .unwrap()
            .recipe
            .as_deref(),
            Some("rust")
        );
    }

    #[test]
    fn power_arguments_are_explicit_and_do_not_override_a_loaded_profile() {
        let args = parse_plan_args(&strings(&[
            "--target",
            "win11-ltsc-2024",
            "--power",
            "choice-a,choice-b",
        ]))
        .unwrap();
        assert_eq!(
            args.power,
            BTreeSet::from(["choice-a".into(), "choice-b".into()])
        );
        for args in [
            vec!["--profile", "saved.json", "--power", "choice-a"],
            vec![
                "--target",
                "win11-ltsc-2024",
                "--power",
                "choice-a,choice-a",
            ],
            vec!["--target", "win11-ltsc-2024", "--power", "choice-a,"],
            vec![
                "--target",
                "win11-ltsc-2024",
                "--power",
                "choice-a",
                "--power",
                "choice-b",
            ],
        ] {
            assert!(parse_plan_args(&strings(&args)).is_err());
        }
        assert!(HELP.contains(env!("CARGO_PKG_VERSION")));
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
