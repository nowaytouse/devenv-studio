use crate::{
    Detection, Task, normalize_pip_id,
    process::{Invocation, Output, Runner},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub(crate) const PROBE: &str = "import json,sys,site,struct,sysconfig,importlib.util,importlib.metadata; spec=importlib.util.find_spec('pip'); print(json.dumps({'schema_version':1,'executable':sys.executable,'implementation':sys.implementation.name,'version':list(sys.version_info[:3]),'releaselevel':sys.version_info.releaselevel,'bits':struct.calcsize('P')*8,'prefix':sys.prefix,'base_prefix':sys.base_prefix,'user_site_enabled':site.ENABLE_USER_SITE,'user_base':site.getuserbase(),'user_site':site.getusersitepackages(),'scripts':sysconfig.get_path('scripts',scheme='nt_user'),'pip_version':importlib.metadata.version('pip'),'pip_origin':spec.origin if spec else None,'free_threaded':bool(sysconfig.get_config_var('Py_GIL_DISABLED'))},ensure_ascii=True))";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Layout {
    pub program: PathBuf,
    pub root: PathBuf,
    pub minor: u32,
    pub user_base: PathBuf,
    pub user_site: PathBuf,
    pub scripts: PathBuf,
}
impl Layout {
    pub(crate) fn new(root: PathBuf, minor: u32, user_base: PathBuf) -> Result<Self> {
        ensure!(
            (11..=14).contains(&minor),
            "Pip supports stable CPython 3.11–3.14 only"
        );
        ensure!(
            root.is_absolute() && user_base.is_absolute(),
            "Python directories must be absolute"
        );
        let user_root = user_base.join(format!("Python3{minor}"));
        Ok(Self {
            program: root.join("python.exe"),
            root,
            minor,
            user_base,
            user_site: user_root.join("site-packages"),
            scripts: user_root.join("Scripts"),
        })
    }
    #[cfg(windows)]
    pub(crate) fn detail(&self) -> String {
        format!(
            "Registered CPython candidate: {}; expected user site: {}; scripts: {}. Runtime/pip checks run only after explicit inspection or execution. No PATH, bootstrap or config change.",
            self.program.display(),
            self.user_site.display(),
            self.scripts.display()
        )
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    schema_version: u32,
    executable: String,
    implementation: String,
    version: [u32; 3],
    releaselevel: String,
    bits: u32,
    prefix: String,
    base_prefix: String,
    user_site_enabled: bool,
    user_base: String,
    user_site: String,
    scripts: String,
    pip_version: String,
    pip_origin: String,
    free_threaded: bool,
}
pub(crate) struct Prepared {
    layout: Layout,
    identity: Identity,
}
pub(crate) fn same_path(actual: &str, expected: &std::path::Path) -> bool {
    actual
        .replace('/', "\\")
        .trim_end_matches('\\')
        .eq_ignore_ascii_case(
            expected
                .to_string_lossy()
                .replace('/', "\\")
                .trim_end_matches('\\'),
        )
}
fn parse_probe(text: &str, layout: &Layout) -> Result<Identity> {
    let identity: Identity =
        serde_json::from_str(text).context("Invalid complete Python preflight JSON")?;
    ensure!(
        identity.schema_version == 1
            && identity.implementation == "cpython"
            && identity.version[0] == 3
            && identity.version[1] == layout.minor
            && identity.releaselevel == "final"
            && identity.bits == 64
            && !identity.free_threaded
            && identity.user_site_enabled
            && !identity.pip_version.is_empty()
            && identity.pip_version.len() <= 128
            && identity
                .pip_version
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b".-_+".contains(&byte)),
        "Unsupported Python/pip runtime or disabled user site"
    );
    for (actual, expected) in [
        (&identity.executable, &layout.program),
        (&identity.prefix, &layout.root),
        (&identity.base_prefix, &layout.root),
        (&identity.user_base, &layout.user_base),
        (&identity.user_site, &layout.user_site),
        (&identity.scripts, &layout.scripts),
        (
            &identity.pip_origin,
            &layout.root.join("Lib/site-packages/pip/__init__.py"),
        ),
    ] {
        ensure!(
            same_path(actual, expected),
            "Python preflight path differs from the selected standard installation: {actual}"
        );
    }
    Ok(identity)
}
pub(crate) fn incompatible_environment(name: &str, value: &std::ffi::OsStr) -> bool {
    !value.is_empty()
        && matches!(
            name.to_ascii_uppercase().as_str(),
            "VIRTUAL_ENV"
                | "CONDA_PREFIX"
                | "PYTHONHOME"
                | "PYTHONPATH"
                | "PYTHONUSERBASE"
                | "PYTHONNOUSERSITE"
        )
}
pub(crate) fn check_environment() -> Result<()> {
    for (name, value) in std::env::vars_os() {
        ensure!(
            !incompatible_environment(&name.to_string_lossy(), &value),
            "Pip provider refuses an active/custom Python environment override: {}. Use a normal Windows session; existing settings were preserved.",
            name.to_string_lossy()
        );
    }
    Ok(())
}
pub(crate) fn is_invocation(command: &Invocation) -> bool {
    command
        .program
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("python.exe"))
        && command.args.first().is_some_and(|arg| arg == "-E")
        && command.args.get(1).is_some_and(|arg| arg == "-P")
        && ((command.args.get(2).is_some_and(|arg| arg == "-c")
            && command.args.get(3).is_some_and(|arg| arg == PROBE))
            || (command.args.get(2).is_some_and(|arg| arg == "-m")
                && command.args.get(3).is_some_and(|arg| arg == "pip")))
}
fn checked(output: Output, cancel: &Arc<AtomicBool>) -> Result<Output> {
    ensure!(
        !output.cancelled
            && !output.timed_out
            && !cancel.load(Ordering::Acquire)
            && output.code == 0
            && output.stderr.trim().is_empty(),
        "Python/pip inspection failed or emitted incomplete-metadata diagnostics: {}",
        output.diagnostic()
    );
    // Both fixed metadata commands emit ASCII JSON; reject repaired or unexpected encoding.
    ensure!(
        output.stdout.is_ascii(),
        "Python/pip metadata capture is not ASCII JSON"
    );
    Ok(output)
}
fn probe(layout: &Layout, runner: &mut impl Runner, cancel: &Arc<AtomicBool>) -> Result<Identity> {
    ensure!(
        !cancel.load(Ordering::Acquire),
        "Python preflight cancelled before command"
    );
    check_environment()?;
    let command = Invocation {
        program: layout.program.clone(),
        args: vec!["-E".into(), "-P".into(), "-c".into(), PROBE.into()],
        vscode_node: false,
        npm_node: false,
    };
    let output = checked(runner.run(&command, cancel)?, cancel)?;
    parse_probe(&output.stdout, layout)
}
pub(crate) fn prepare(
    layout: Layout,
    runner: &mut impl Runner,
    cancel: &Arc<AtomicBool>,
) -> Result<Prepared> {
    let identity = probe(&layout, runner, cancel)?;
    Ok(Prepared { layout, identity })
}
pub(crate) fn verify(
    prepared: &Prepared,
    runner: &mut impl Runner,
    cancel: &Arc<AtomicBool>,
) -> Result<()> {
    ensure!(
        probe(&prepared.layout, runner, cancel)? == prepared.identity,
        "Python identity or user installation paths changed during the task; result is unverified"
    );
    Ok(())
}
pub(crate) fn invocation(prepared: &Prepared, task: &Task, inspect: bool) -> Result<Invocation> {
    let id = task
        .package_id
        .as_deref()
        .context("Missing Pip distribution identity")?;
    ensure!(
        normalize_pip_id(id).as_deref() == Some(id),
        "Pip task requires a canonical bare distribution identity"
    );
    if inspect {
        return Ok(list_invocation(prepared));
    }
    let mut args: Vec<String> = [
        "-E",
        "-P",
        "-m",
        "pip",
        "--isolated",
        "--disable-pip-version-check",
        "--no-input",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    args.extend(
        [
            "install",
            "--user",
            "--only-binary=:all:",
            "--index-url=https://pypi.org/simple",
            "--progress-bar=off",
            "--",
            id,
        ]
        .into_iter()
        .map(str::to_owned),
    );
    Ok(Invocation {
        program: prepared.layout.program.clone(),
        args,
        vscode_node: false,
        npm_node: false,
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Distribution {
    name: String,
    version: String,
    #[serde(default)]
    editable_project_location: Option<String>,
}
fn distributions(text: &str) -> Result<Vec<Distribution>> {
    let mut entries: Vec<Distribution> =
        serde_json::from_str(text).context("Invalid complete pip list JSON")?;
    ensure!(entries.len() <= 16384, "Pip metadata list is too large");
    let mut names = BTreeSet::new();
    for entry in &mut entries {
        let name =
            normalize_pip_id(&entry.name).context("Invalid distribution name in Pip metadata")?;
        ensure!(
            names.insert(name.clone()),
            "Duplicate normalized distribution in Pip metadata"
        );
        ensure!(
            !entry.version.is_empty()
                && entry.version.len() <= 128
                && !entry.version.chars().any(char::is_control),
            "Invalid distribution version in Pip metadata"
        );
        if let Some(path) = &entry.editable_project_location {
            ensure!(
                !path.is_empty() && !path.contains('\0'),
                "Invalid editable distribution metadata"
            );
        }
        entry.name = name;
    }
    Ok(entries)
}
fn listed(text: &str, requested: &str) -> Result<bool> {
    let entries = distributions(text)?;
    for entry in &entries {
        ensure!(
            entry.name != requested || entry.editable_project_location.is_none(),
            "Requested Pip distribution is editable in a project; no installation or project change was attempted"
        );
    }
    Ok(entries.iter().any(|entry| entry.name == requested))
}
fn list_invocation(prepared: &Prepared) -> Invocation {
    Invocation {
        program: prepared.layout.program.clone(),
        args: [
            "-E",
            "-P",
            "-m",
            "pip",
            "--isolated",
            "--disable-pip-version-check",
            "--no-input",
            "list",
            "--format=json",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        vscode_node: false,
        npm_node: false,
    }
}
pub(crate) fn inventory(
    layout: Layout,
    runner: &mut impl Runner,
    cancel: &Arc<AtomicBool>,
) -> Result<Vec<crate::backup::Package>> {
    let prepared = prepare(layout, runner, cancel)?;
    let output = checked(runner.run(&list_invocation(&prepared), cancel)?, cancel)?;
    let packages = inventory_packages(&output.stdout)?;
    verify(&prepared, runner, cancel)?;
    Ok(packages)
}
fn inventory_packages(text: &str) -> Result<Vec<crate::backup::Package>> {
    distributions(text)?
        .into_iter()
        .map(|entry| {
            ensure!(
                entry.editable_project_location.is_none(),
                "Editable project distributions cannot be captured as restorable package identities"
            );
            ensure!(
                crate::backup::valid_version(&entry.version),
                "Unsupported distribution version"
            );
            Ok(crate::backup::Package {
                name: entry.name,
                version: entry.version,
            })
        })
        .collect()
}
pub(crate) fn detect(
    prepared: &Prepared,
    task: &Task,
    runner: &mut impl Runner,
    cancel: &Arc<AtomicBool>,
) -> Result<Detection> {
    ensure!(
        !cancel.load(Ordering::Acquire),
        "Pip inspection cancelled before command"
    );
    let command = invocation(prepared, task, true)?;
    let output = checked(runner.run(&command, cancel)?, cancel)?;
    let installed = listed(
        &output.stdout,
        task.package_id.as_deref().context("Missing Pip identity")?,
    )?;
    Ok(Detection {
        id: task.id.clone(),
        state: if installed {
            "installed"
        } else {
            "not_installed"
        }
        .into(),
        detail: format!(
            "Distribution metadata visible in this interpreter (global or user), not proof of Studio ownership or a tool launch. Python preflight: {}",
            serde_json::to_string(&prepared.identity)?
        ),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn layout() -> Layout {
        let (root, user) = if cfg!(windows) {
            (r"Q:\trusted\Python314", r"Q:\user\Python")
        } else {
            ("/trusted/Python314", "/user/Python")
        };
        Layout::new(root.into(), 14, user.into()).unwrap()
    }
    pub(crate) fn identity() -> serde_json::Value {
        let layout = layout();
        serde_json::json!({"schema_version":1,"executable":layout.program,"implementation":"cpython","version":[3,14,2],"releaselevel":"final","bits":64,"prefix":layout.root,"base_prefix":layout.root,"user_site_enabled":true,"user_base":layout.user_base,"user_site":layout.user_site,"scripts":layout.scripts,"pip_version":"25.3","pip_origin":layout.root.join("Lib/site-packages/pip/__init__.py"),"free_threaded":false})
    }
    #[test]
    fn probe_rejects_wrong_identity_scope_and_user_site() {
        let layout = layout();
        parse_probe(&identity().to_string(), &layout).unwrap();
        for (key, value) in [
            ("implementation", serde_json::json!("pypy")),
            ("bits", serde_json::json!(32)),
            ("user_site_enabled", serde_json::json!(false)),
            ("prefix", serde_json::json!("/project/venv")),
            ("user_base", serde_json::json!("/custom")),
            ("executable", serde_json::json!("/other/python.exe")),
            ("free_threaded", serde_json::json!(true)),
            ("pip_origin", serde_json::json!("/user/pip/__init__.py")),
        ] {
            let mut bad = identity();
            bad[key] = value;
            assert!(parse_probe(&bad.to_string(), &layout).is_err(), "{key}");
        }
        assert!(parse_probe(&(identity().to_string() + "trailing"), &layout).is_err());
    }
    #[test]
    fn complete_metadata_normalizes_names_and_refuses_requested_editables() {
        assert!(listed(r#"[{"name":"PyWavelets","version":"1.9"}]"#, "pywavelets").unwrap());
        assert!(!listed(r#"[{"name":"numpy-extra","version":"1"}]"#, "numpy").unwrap());
        assert!(!listed("[]", "numpy").unwrap());
        for text in [
            r#"[{"name":"numpy","version":"2","editable_project_location":"/project"}]"#,
            r#"[{"name":"numpy","version":"1"},{"name":"NumPy","version":"2"}]"#,
            r#"[{"name":"numpy","name":"scipy","version":"1"}]"#,
            r#"[{"name":"numpy","version":""}]"#,
            r#"[{"name":"--help","version":"1"}]"#,
            r#"[{"name":"numpy","version":"2"}] garbage"#,
        ] {
            assert!(listed(text, "numpy").is_err(), "{text}");
        }
        assert!(
            !listed(
                r#"[{"name":"other","version":"1","editable_project_location":"/project"}]"#,
                "numpy"
            )
            .unwrap()
        );
    }
    #[test]
    fn incomplete_inspections_never_become_absent_or_installed() {
        let cancel = Arc::new(AtomicBool::new(false));
        let success = Output {
            code: 0,
            stdout: "[]".into(),
            stderr: String::new(),
            cancelled: false,
            timed_out: false,
        };
        checked(success.clone(), &cancel).unwrap();
        for output in [
            Output {
                stderr: "WARNING: Ignoring invalid distribution".into(),
                ..success.clone()
            },
            Output {
                stdout: "[{\"name\":\"numpy\",\"version\":\"2�\"}]".into(),
                ..success.clone()
            },
            Output {
                code: 1,
                ..success.clone()
            },
            Output {
                cancelled: true,
                ..success.clone()
            },
            Output {
                timed_out: true,
                ..success.clone()
            },
        ] {
            assert!(checked(output, &cancel).is_err());
        }
        cancel.store(true, Ordering::Release);
        assert!(checked(success, &cancel).is_err());
    }
    #[test]
    fn inventory_uses_only_probe_list_probe_and_refuses_partial_or_editable_metadata() {
        use std::collections::VecDeque;
        struct Fake {
            outputs: VecDeque<Output>,
            commands: Vec<Invocation>,
        }
        impl Runner for Fake {
            fn run(&mut self, command: &Invocation, _: &Arc<AtomicBool>) -> Result<Output> {
                self.commands.push(command.clone());
                Ok(self.outputs.pop_front().unwrap())
            }
        }
        let output = |text: String| Output {
            code: 0,
            stdout: text,
            stderr: String::new(),
            cancelled: false,
            timed_out: false,
        };
        let mut runner = Fake {
            outputs: [
                output(identity().to_string()),
                output(r#"[{"name":"PyWavelets","version":"1.9"}]"#.into()),
                output(identity().to_string()),
            ]
            .into(),
            commands: vec![],
        };
        let packages = inventory(layout(), &mut runner, &Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!(packages[0].name, "pywavelets");
        assert_eq!(runner.commands.len(), 3);
        assert_eq!(runner.commands[0].args[3], PROBE);
        assert_eq!(runner.commands[2].args[3], PROBE);
        assert!(
            runner.commands[1]
                .args
                .ends_with(&["list".into(), "--format=json".into()])
        );
        assert!(runner.commands.iter().all(|command| {
            !command
                .args
                .iter()
                .any(|arg| matches!(arg.as_str(), "install" | "ensurepip" | "--upgrade"))
        }));
        for text in [
            r#"[{"name":"pkg","version":"1","editable_project_location":"/private/project"}]"#,
            r#"[{"name":"pkg","version":"/private/secret1"}]"#,
            r#"[{"name":"a-b","version":"1"},{"name":"a_b","version":"2"}]"#,
            "{}",
        ] {
            assert!(inventory_packages(text).is_err(), "{text}");
        }
        assert!(inventory_packages("[]").unwrap().is_empty());
        let mut changed = identity();
        changed["version"] = serde_json::json!([3, 14, 3]);
        let mut runner = Fake {
            outputs: [
                output(identity().to_string()),
                output("[]".into()),
                output(changed.to_string()),
            ]
            .into(),
            commands: vec![],
        };
        assert!(inventory(layout(), &mut runner, &Arc::new(AtomicBool::new(false))).is_err());
    }
    #[test]
    fn invocation_recognition_cannot_capture_other_developer_commands() {
        let layout = layout();
        let mut invocation = Invocation {
            program: layout.program,
            args: vec!["-E".into(), "-P".into(), "-c".into(), PROBE.into()],
            vscode_node: false,
            npm_node: false,
        };
        assert!(is_invocation(&invocation));
        invocation.args[3] = "print('unrelated')".into();
        assert!(!is_invocation(&invocation));
        invocation.args[2] = "-m".into();
        invocation.args[3] = "pip".into();
        assert!(is_invocation(&invocation));
        invocation.program.set_file_name("python3");
        assert!(!is_invocation(&invocation));
        invocation.program.set_file_name("node.exe");
        assert!(!is_invocation(&invocation));
    }
    #[test]
    fn custom_environment_refused_without_removing_user_settings() {
        for key in [
            "PYTHONHOME",
            "pythonpath",
            "PYTHONUSERBASE",
            "PYTHONNOUSERSITE",
            "VIRTUAL_ENV",
            "CONDA_PREFIX",
        ] {
            assert!(incompatible_environment(
                key,
                std::ffi::OsStr::new("configured")
            ));
            assert!(!incompatible_environment(key, std::ffi::OsStr::new("")));
        }
        assert!(!incompatible_environment(
            "PATH",
            std::ffi::OsStr::new("existing")
        ));
    }
}
