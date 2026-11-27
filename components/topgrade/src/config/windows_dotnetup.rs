use super::{CommandLineArgs, ConfigFile, Path, Step};

const STEPS: [(Step, &str); 2] = [
    (Step::DotnetDaily, "Update-DotnetDaily.ps1"),
    (Step::DotnetupPreview, "Update-DotnetupPreview.ps1"),
];

// Match the deployed -File launchers exactly. Additional arguments or shell operators belong to the user.
pub(super) fn launcher(command: &str, appdata: &Path) -> Option<Step> {
    let appdata = appdata.to_str()?;
    let (shell, command) = command.trim().split_once(' ')?;
    if !["pwsh.exe", "pwsh", "powershell.exe", "powershell"]
        .iter()
        .any(|s| shell.eq_ignore_ascii_case(s))
    {
        return None;
    }
    STEPS.iter().find_map(|&(step, script)| {
        [appdata, "$env:APPDATA"]
            .iter()
            .any(|root| command.eq_ignore_ascii_case(&format!(r#"-NoProfile -File "{root}\topgrade\{script}""#)))
            .then_some(step)
    })
}

pub(super) fn select(
    enabled: &mut Vec<Step>,
    disabled: &[Step],
    opt: &CommandLineArgs,
    file: &ConfigFile,
    appdata: &Path,
) {
    let only = if opt.only.is_empty() {
        file.misc.as_ref().and_then(|m| m.only.as_deref()).unwrap_or_default()
    } else {
        &opt.only
    };
    for (step, _) in STEPS {
        let setting = file.windows.as_ref().and_then(|w| {
            if step == Step::DotnetDaily {
                w.dotnet_daily
            } else {
                w.dotnetup_preview
            }
        });
        let legacy = enabled.contains(&Step::CustomCommands)
            && file.commands.as_ref().is_some_and(|commands| {
                commands.iter().any(|(name, command)| {
                    (opt.custom_commands.is_empty() || opt.custom_commands.contains(name))
                        && launcher(command, appdata) == Some(step)
                })
            });
        let native = enabled.contains(&step) && (only.contains(&step) || setting == Some(true));
        enabled.retain(|s| *s != step);
        if native || (legacy && setting != Some(false) && !disabled.contains(&step)) {
            enabled.push(step);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    const ROOT: &str = r"C:\Test Profile\Roaming";
    const COMMAND: &str = r#"pwsh.exe -NoProfile -File "C:\Test Profile\Roaming\topgrade\Update-DotnetDaily.ps1""#;

    #[test]
    fn recognizes_only_the_deployed_launcher() {
        let appdata = Path::new(ROOT);
        assert_eq!(launcher(COMMAND, appdata), Some(Step::DotnetDaily));
        assert_eq!(
            launcher(&COMMAND.replace(ROOT, "$env:APPDATA"), appdata),
            Some(Step::DotnetDaily)
        );
        for command in [
            format!("{COMMAND} -CheckOnly"),
            format!("{COMMAND}; exit"),
            COMMAND.replace(ROOT, r"C:\custom"),
        ] {
            assert_eq!(launcher(&command, appdata), None);
        }
    }

    #[test]
    fn respects_opt_in_filters_and_deduplicates() {
        use Step::{CustomCommands, DotnetDaily, DotnetupPreview};
        let select_for = |args: &[&str], steps: &[Step], disabled: &[Step], config: &str| {
            let opt = CommandLineArgs::try_parse_from(std::iter::once("topgrade").chain(args.iter().copied())).unwrap();
            let mut enabled = steps.to_vec();
            select(
                &mut enabled,
                disabled,
                &opt,
                &toml::from_str(config).unwrap(),
                Path::new(ROOT),
            );
            enabled
        };
        let all = [CustomCommands, DotnetDaily, DotnetupPreview];
        let legacy = format!("[commands]\ndaily = '{COMMAND}'");
        assert_eq!(select_for(&[], &all, &[], ""), [CustomCommands]);
        assert_eq!(select_for(&[], &all, &[], &legacy), [CustomCommands, DotnetDaily]);
        assert_eq!(
            select_for(
                &["--only", "custom_commands", "--custom-commands", "other"],
                &[CustomCommands],
                &[],
                &legacy
            ),
            [CustomCommands]
        );
        assert_eq!(
            select_for(&["--only", "custom_commands"], &[CustomCommands], &[], &legacy),
            [CustomCommands, DotnetDaily]
        );
        assert_eq!(
            select_for(&[], &[CustomCommands], &[DotnetDaily], &legacy),
            [CustomCommands]
        );
        assert_eq!(
            select_for(&["--only", "dotnetup_preview"], &[DotnetupPreview], &[], ""),
            [DotnetupPreview]
        );
        assert_eq!(
            select_for(
                &[],
                &all,
                &[],
                &format!("{legacy}\n[windows]\ndotnet_daily=false\ndotnetup_preview=true")
            ),
            [CustomCommands, DotnetupPreview]
        );
    }
}
