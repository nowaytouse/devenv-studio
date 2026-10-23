use crate::{
    Capability, DeploymentEvent, DeploymentPlan, DeploymentReport, Detection, ProviderCapability,
    engine::{self, DeveloperTool, History, Tools},
    process::{Invocation, Runner, TASK_TIMEOUT},
};
use anyhow::{Context, Result, ensure};
use std::{
    ffi::{OsString, c_void},
    fs::{self, File, OpenOptions},
    io::Write,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::OpenOptionsExt,
        io::AsRawHandle,
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::Sender,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use winreg::{
    RegKey,
    enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY},
};

#[repr(C)]
struct Guid {
    a: u32,
    b: u16,
    c: u16,
    d: [u8; 8],
}
#[link(name = "shell32")]
unsafe extern "system" {
    fn SHGetKnownFolderPath(
        folder: *const Guid,
        flags: u32,
        token: *mut c_void,
        path: *mut *mut u16,
    ) -> i32;
    fn IsUserAnAdmin() -> i32;
}
#[link(name = "ole32")]
unsafe extern "system" {
    fn CoTaskMemFree(memory: *mut c_void);
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetSystemWindowsDirectoryW(buffer: *mut u16, size: u32) -> u32;
    fn GetVolumePathNameW(file: *const u16, volume: *mut u16, size: u32) -> i32;
    fn GetVolumeNameForVolumeMountPointW(mount: *const u16, name: *mut u16, size: u32) -> i32;
    fn DeviceIoControl(
        handle: *mut c_void,
        code: u32,
        input: *const c_void,
        input_size: u32,
        output: *mut c_void,
        output_size: u32,
        returned: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
}
fn local_app_data() -> Result<PathBuf> {
    const LOCAL: Guid = Guid {
        a: 0xf1b32785,
        b: 0x6fba,
        c: 0x4fcf,
        d: [0x9d, 0x55, 0x7b, 0x8e, 0x7f, 0x15, 0x70, 0x91],
    };
    known_folder(&LOCAL)
}
fn roaming_app_data() -> Result<PathBuf> {
    const ROAMING: Guid = Guid {
        a: 0x3eb685db,
        b: 0x65f9,
        c: 0x4cf6,
        d: [0xa0, 0x3a, 0xe3, 0xef, 0x65, 0x72, 0x9f, 0x3d],
    };
    known_folder(&ROAMING)
}
fn user_profile() -> Result<PathBuf> {
    const PROFILE: Guid = Guid {
        a: 0x5e6c858f,
        b: 0x0e22,
        c: 0x4760,
        d: [0x9a, 0xfe, 0xea, 0x33, 0x17, 0xb6, 0x71, 0x73],
    };
    known_folder(&PROFILE)
}
fn known_folder(folder: &Guid) -> Result<PathBuf> {
    let mut pointer = std::ptr::null_mut();
    // SAFETY: Windows allocates a nul-terminated string; it is freed exactly once.
    let result = unsafe { SHGetKnownFolderPath(folder, 0, std::ptr::null_mut(), &mut pointer) };
    ensure!(
        result >= 0 && !pointer.is_null(),
        "Read Windows known folder failed: 0x{result:08X}"
    );
    let mut len = 0;
    while len < 32768 && unsafe { *pointer.add(len) } != 0 {
        len += 1;
    }
    if len == 32768 {
        unsafe { CoTaskMemFree(pointer.cast()) };
        anyhow::bail!("Known folder path exceeds limit");
    }
    let path = PathBuf::from(OsString::from_wide(unsafe {
        std::slice::from_raw_parts(pointer, len)
    }));
    unsafe { CoTaskMemFree(pointer.cast()) };
    ensure!(path.is_absolute(), "Windows known folder is not absolute");
    Ok(path)
}
fn windows_directory() -> Result<PathBuf> {
    let mut buffer = [0u16; 32768];
    let len = unsafe { GetSystemWindowsDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
    ensure!(
        len > 0 && (len as usize) < buffer.len(),
        "GetSystemWindowsDirectoryW failed: {}",
        std::io::Error::last_os_error()
    );
    let path = PathBuf::from(OsString::from_wide(&buffer[..len as usize]));
    ensure!(
        path.is_absolute(),
        "System Windows directory is not absolute"
    );
    Ok(path)
}
fn system_volume() -> Result<String> {
    let path: Vec<u16> = windows_directory()?
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut mount = [0u16; 32768];
    // SAFETY: both API outputs are bounded, writable UTF-16 buffers.
    ensure!(
        unsafe { GetVolumePathNameW(path.as_ptr(), mount.as_mut_ptr(), mount.len() as u32) } != 0,
        "Resolve Windows system mount point: {}",
        std::io::Error::last_os_error()
    );
    ensure!(
        mount.contains(&0),
        "Windows volume mount point is not terminated"
    );
    let mut name = [0u16; 50];
    ensure!(
        unsafe {
            GetVolumeNameForVolumeMountPointW(mount.as_ptr(), name.as_mut_ptr(), name.len() as u32)
        } != 0,
        "Resolve Windows system volume GUID: {}",
        std::io::Error::last_os_error()
    );
    let len = name
        .iter()
        .position(|&unit| unit == 0)
        .context("Windows volume GUID is not terminated")?;
    let volume =
        String::from_utf16(&name[..len]).context("Windows volume GUID is not valid UTF-16")?;
    ensure!(
        crate::storage::valid_volume_guid(&volume),
        "Windows returned an invalid volume GUID path"
    );
    Ok(volume)
}
fn system_tool(name: &str) -> Result<PathBuf> {
    let program = windows_directory()?.join("System32").join(name);
    ensure!(
        program.is_file(),
        "Native System32 {name} is unavailable; no PATH fallback is allowed"
    );
    Ok(program)
}
pub(super) fn storage_status(cancel: Arc<AtomicBool>) -> Result<crate::StorageStatus> {
    let detected = studio_platform::host();
    ensure!(
        detected.can_apply && detected.target.is_some(),
        "Storage policy inspection requires a supported Windows LTSC host: {}",
        detected.explanation
    );
    ensure!(
        unsafe { IsUserAnAdmin() } != 0,
        "Reading fsutil policy requires an already elevated administrator process. No elevation was attempted."
    );
    let command = Invocation {
        program: system_tool("fsutil.exe")?,
        args: vec![
            "behavior".into(),
            "query".into(),
            "DisableDeleteNotify".into(),
        ],
        vscode_node: false,
        npm_node: false,
    };
    let mut runner = DeadlineRunner {
        deadline: Instant::now() + Duration::from_secs(45),
    };
    crate::storage::inspect_with_runner(&command, &mut runner, &cancel)
}
fn program_files() -> Result<PathBuf> {
    let key = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(
        r"SOFTWARE\Microsoft\Windows\CurrentVersion",
        KEY_READ | KEY_WOW64_64KEY,
    )?;
    let path: OsString = key.get_value("ProgramFilesDir")?;
    let path = PathBuf::from(path);
    ensure!(path.is_absolute(), "ProgramFilesDir must be absolute");
    Ok(path)
}
fn winget_path() -> Result<PathBuf> {
    let alias = local_app_data()?.join(r"Microsoft\WindowsApps\winget.exe");
    // Open the reparse point itself. An ordinary executable at this user-writable
    // path is rejected; the OS AppExecLink must identify Microsoft's App Installer.
    let handle = OpenOptions::new()
        .read(true)
        .custom_flags(0x00200000 | 0x02000000)
        .open(&alias)
        .context("App Installer WinGet execution alias is unavailable")?;
    let mut bytes = [0u8; 16384];
    let mut returned = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            handle.as_raw_handle(),
            0x000900a8,
            std::ptr::null(),
            0,
            bytes.as_mut_ptr().cast(),
            bytes.len() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    ensure!(
        ok != 0 && returned >= 12,
        "WinGet alias is not an inspectable Windows app execution link: {}",
        std::io::Error::last_os_error()
    );
    ensure!(
        u32::from_le_bytes(bytes[..4].try_into()?) == 0x8000001b,
        "Refusing WinGet path that is not an AppExecLink"
    );
    let payload = String::from_utf16(
        &bytes[12..returned as usize]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect::<Vec<_>>(),
    )
    .context("Invalid AppExecLink text")?;
    let parts: Vec<_> = payload.split('\0').filter(|s| !s.is_empty()).collect();
    ensure!(parts.iter().any(|s|s.starts_with("Microsoft.DesktopAppInstaller_") && s.contains("8wekyb3d8bbwe")),"WinGet alias is not Microsoft's Desktop App Installer");
    let protected = program_files()?.join("WindowsApps");
    ensure!(
        parts.iter().any(|s| {
            let path = Path::new(s);
            path.starts_with(&protected)
                && path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("winget.exe"))
                && path.parent().and_then(Path::file_name).is_some_and(|n| {
                    let n = n.to_string_lossy();
                    n.starts_with("Microsoft.DesktopAppInstaller_")
                        && n.ends_with("__8wekyb3d8bbwe")
                })
        }),
        "AppExecLink executable is not the protected Microsoft App Installer WinGet"
    );
    Ok(alias)
}
pub(super) fn winget_capability() -> Capability {
    match winget_path() {
        Ok(path) => Capability {
            available: true,
            detail: format!(
                "Microsoft App Installer execution alias: {}. A working source and any required agreements are checked at execution; no package-manager bootstrap is performed.",
                path.display()
            ),
        },
        Err(error) => Capability {
            available: false,
            detail: format!(
                "WinGet prerequisite unavailable: {error:#}. Install/register Microsoft's App Installer and enable its WinGet execution alias. Official guidance: https://learn.microsoft.com/windows/package-manager/winget/ . LTSC Studio never downloads a bootstrap script."
            ),
        },
    }
}
fn installed_file(path: &Path) -> Result<bool> {
    use std::os::windows::fs::MetadataExt;
    ensure!(
        path.is_absolute(),
        "Installed provider path must be absolute"
    );
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(
                metadata.file_attributes() & 0x400 == 0 && metadata.is_file(),
                "Installed provider must be a regular executable, not a reparse point: {}",
                path.display()
            );
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => {
            Err(error).with_context(|| format!("Inspect installed provider {}", path.display()))
        }
    }
}
fn regular_directory_if_present(path: &Path) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    match fs::symlink_metadata(path) {
        Ok(metadata) => ensure!(
            metadata.is_dir() && metadata.file_attributes() & 0x400 == 0,
            "Provider directory must not be a reparse point or file: {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("Inspect fixed provider directory"),
    }
    Ok(())
}
fn winget_uv(root: &Path) -> Result<Option<PathBuf>> {
    let link = root.join("Links/uv.exe");
    match fs::symlink_metadata(&link) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("Inspect WinGet uv link"),
    }
    let target = link
        .canonicalize()
        .context("Resolve installed WinGet uv link")?;
    let packages = root
        .join("Packages")
        .canonicalize()
        .context("Resolve installed WinGet package directory")?;
    let relative = target
        .strip_prefix(&packages)
        .context("WinGet uv target is outside its package directory")?;
    let package = relative
        .components()
        .next()
        .context("WinGet uv package component missing")?
        .as_os_str()
        .to_string_lossy();
    ensure!(
        package.starts_with("astral-sh.uv_")
            && target.file_name().is_some_and(|name| name == "uv.exe"),
        "WinGet uv link does not identify the expected package"
    );
    ensure!(installed_file(&target)?, "WinGet uv executable is missing");
    Ok(Some(target))
}
pub(crate) fn pip_layout() -> Result<crate::pip::Layout> {
    use winreg::enums::HKEY_CURRENT_USER;
    crate::pip::check_environment()?;
    let user_python = local_app_data()?.join("Programs/Python");
    let system_python = program_files()?;
    let user_base = roaming_app_data()?.join("Python");
    for minor in (11..=14).rev() {
        let tag = format!(r"SOFTWARE\Python\PythonCore\3.{minor}");
        for (hive, expected) in [
            (
                HKEY_CURRENT_USER,
                user_python.join(format!("Python3{minor}")),
            ),
            (
                HKEY_LOCAL_MACHINE,
                system_python.join(format!("Python3{minor}")),
            ),
        ] {
            let key = match RegKey::predef(hive)
                .open_subkey_with_flags(&tag, KEY_READ | KEY_WOW64_64KEY)
            {
                Ok(key) => key,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error).context("Read registered CPython installation"),
            };
            // HKCU shadows the same HKLM tag even when its registration is unusable.
            let version: String = key
                .get_value("SysVersion")
                .context("Read CPython SysVersion")?;
            let architecture: String = key
                .get_value("SysArchitecture")
                .context("Read CPython SysArchitecture")?;
            ensure!(
                version == format!("3.{minor}") && architecture == "64bit",
                "Registered PythonCore candidate is not supported stable 64-bit CPython"
            );
            let install = key
                .open_subkey_with_flags("InstallPath", KEY_READ | KEY_WOW64_64KEY)
                .context("Read CPython InstallPath")?;
            let root = PathBuf::from(
                install
                    .get_value::<String, _>("")
                    .context("Read CPython install root")?,
            );
            ensure!(
                root.is_absolute()
                    && root
                        .to_string_lossy()
                        .trim_end_matches(['\\', '/'])
                        .eq_ignore_ascii_case(&expected.to_string_lossy()),
                "CPython registration points outside its supported standard install location"
            );
            match install.get_value::<String, _>("ExecutableArguments") {
                Ok(value) => ensure!(
                    value.is_empty(),
                    "Custom Python executable arguments are not supported"
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).context("Read CPython executable arguments"),
            }
            let layout = crate::pip::Layout::new(expected, minor, user_base.clone())?;
            match install.get_value::<String, _>("ExecutablePath") {
                Ok(value) => ensure!(
                    value.eq_ignore_ascii_case(&layout.program.to_string_lossy()),
                    "Registered Python executable differs from its standard root"
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).context("Read registered CPython executable"),
            }
            for path in [
                &layout.root,
                &layout.root.join("Lib"),
                &layout.root.join("Lib/site-packages"),
                &layout.root.join("Lib/site-packages/pip"),
                &layout.user_base,
                &layout.user_base.join(format!("Python3{minor}")),
                &layout.user_site,
                &layout.scripts,
            ] {
                regular_directory_if_present(path)?;
            }
            ensure!(
                installed_file(&layout.program)?
                    && installed_file(&layout.root.join("Lib/site-packages/pip/__init__.py"))?,
                "Selected standard CPython/pip is missing; install or repair it manually. No bootstrap was attempted"
            );
            for name in [
                "pyvenv.cfg".into(),
                "python._pth".into(),
                format!("python3{minor}._pth"),
            ] {
                ensure!(
                    !layout.root.join(name).try_exists()?,
                    "Embedded, free-threaded and virtual-environment Python layouts are not supported"
                );
            }
            return Ok(layout);
        }
    }
    Err(crate::backup::MissingProvider).context(
        "Pip requires registered standard 64-bit CPython 3.11–3.14 with bundled pip. WindowsApps, portable and custom/version-manager installations are not searched; no Python/pip bootstrap was attempted"
    )
}
pub(crate) fn developer_tool(kind: &str) -> Result<DeveloperTool> {
    let profile = user_profile()?;
    let missing = || match kind {
        "cargo" => {
            "Install Rustup/Cargo in the standard user-profile .cargo/bin location and configure a working Rust toolchain (https://rustup.rs). Cargo crates are compiled with --locked; native build tools may also be required."
        }
        "npm" => {
            "Install official Node.js LTS with bundled npm in the standard system Program Files/nodejs location (https://nodejs.org/en/download). Custom, portable and version-manager locations are not searched. npm uses the fixed user Roaming AppData/npm prefix, public npm registry, and disables package lifecycle scripts; installed identity is not a tool launch test."
        }
        "uv" => {
            "Install uv in the standard user-profile .local/bin or .cargo/bin, or through the explicit astral-sh.uv WinGet task (https://docs.astral.sh/uv/getting-started/installation/). Python must already be available; automatic Python downloads are disabled."
        }
        _ => {
            "Install the official stable Visual Studio Code user/system package (https://code.visualstudio.com/docs/setup/windows). A standard installation with Code.exe and resources/app/out/cli.js is required; custom, portable and Insiders installs are not searched."
        }
    };
    match kind {
        "cargo" => {
            let root = profile.join(".cargo");
            let program = root.join("bin/cargo.exe");
            if !installed_file(&program)? {
                return Err(crate::backup::MissingProvider).context(missing());
            }
            Ok(DeveloperTool {
                program,
                cargo_root: Some(root),
                vscode_cli: None,
                npm_cli: None,
                npm_prefix: None,
            })
        }
        "uv" => {
            for path in [
                profile.join(".local/bin/uv.exe"),
                profile.join(".cargo/bin/uv.exe"),
            ] {
                if installed_file(&path)? {
                    return Ok(DeveloperTool {
                        program: path,
                        cargo_root: None,
                        vscode_cli: None,
                        npm_cli: None,
                        npm_prefix: None,
                    });
                }
            }
            let mut roots = vec![local_app_data()?.join("Microsoft/WinGet")];
            if let Ok(files) = program_files() {
                roots.push(files.join("WinGet"));
            }
            for root in roots {
                if let Some(program) = winget_uv(&root)? {
                    return Ok(DeveloperTool {
                        program,
                        cargo_root: None,
                        vscode_cli: None,
                        npm_cli: None,
                        npm_prefix: None,
                    });
                }
            }
            Err(crate::backup::MissingProvider).context(missing())
        }
        "npm" => {
            let root = program_files()?.join("nodejs");
            for directory in [
                &root,
                &root.join("node_modules"),
                &root.join("node_modules/npm"),
                &root.join("node_modules/npm/bin"),
            ] {
                regular_directory_if_present(directory)?;
            }
            let program = root.join("node.exe");
            let cli = root.join("node_modules/npm/bin/npm-cli.js");
            if !installed_file(&program)? || !installed_file(&cli)? {
                return Err(crate::backup::MissingProvider).context(missing());
            }
            let prefix = roaming_app_data()?.join("npm");
            regular_directory_if_present(&prefix)?;
            regular_directory_if_present(&prefix.join("node_modules"))?;
            Ok(DeveloperTool {
                program,
                cargo_root: None,
                vscode_cli: None,
                npm_cli: Some(cli),
                npm_prefix: Some(prefix),
            })
        }
        "vscode" => {
            let mut roots = vec![local_app_data()?.join("Programs/Microsoft VS Code")];
            if let Ok(files) = program_files() {
                roots.push(files.join("Microsoft VS Code"));
            }
            for root in roots {
                let program = root.join("Code.exe");
                let cli = root.join("resources/app/out/cli.js");
                if installed_file(&program)? && installed_file(&cli)? {
                    return Ok(DeveloperTool {
                        program,
                        cargo_root: None,
                        vscode_cli: Some(cli),
                        npm_cli: None,
                        npm_prefix: None,
                    });
                }
            }
            Err(crate::backup::MissingProvider).context(missing())
        }
        _ => anyhow::bail!("Unknown developer provider {kind}"),
    }
}
pub(super) fn provider_capabilities() -> Vec<ProviderCapability> {
    let winget = winget_capability();
    let mut capabilities = vec![ProviderCapability {
        provider: "winget".into(),
        available: winget.available,
        detail: winget.detail,
    }];
    for provider in ["cargo", "uv", "vscode", "npm"] {
        let (available, detail) = match developer_tool(provider) {
            Ok(tool) if provider == "npm" => match tool.npm_prefix {
                Some(prefix) => (
                    true,
                    format!(
                        "Installed Node/npm files: {}. Fixed user global prefix: {}. Public registry; npm lifecycle scripts disabled. Availability checks files only; installed identity does not verify a command can run.",
                        tool.program.display(),
                        prefix.display()
                    ),
                ),
                None => (false, "Missing fixed user npm prefix".into()),
            },
            Ok(tool) => (
                true,
                format!(
                    "Installed provider: {}. Availability is rechecked for each task after any explicitly selected prerequisites; no provider is bootstrapped automatically.",
                    tool.program.display()
                ),
            ),
            Err(error) => (false, format!("{error:#}")),
        };
        capabilities.push(ProviderCapability {
            provider: provider.into(),
            available,
            detail,
        });
    }
    let (available, detail) = match pip_layout() {
        Ok(layout) => (true, layout.detail()),
        Err(error) => (false, format!("{error:#}")),
    };
    capabilities.push(ProviderCapability {
        provider: "pip".into(),
        available,
        detail,
    });
    capabilities
}
fn tools(plan: &DeploymentPlan) -> Result<Tools> {
    let winget = plan
        .tasks
        .iter()
        .any(|task| task.kind == "package")
        .then(|| winget_path().with_context(|| winget_capability().detail))
        .transpose()?;
    let dism = plan
        .tasks
        .iter()
        .any(|task| {
            task.kind == "feature"
                || matches!(
                    task.id.as_str(),
                    "maintenance-check-health"
                        | "maintenance-scan-health"
                        | "maintenance-restore-health"
                )
        })
        .then(|| system_tool("dism.exe"))
        .transpose()?;
    let sfc = plan
        .tasks
        .iter()
        .any(|task| task.id == "maintenance-sfc-verify")
        .then(|| system_tool("sfc.exe"))
        .transpose()?;
    let defrag = plan
        .tasks
        .iter()
        .any(|task| crate::storage::is_task(&task.id))
        .then(|| system_tool("defrag.exe"))
        .transpose()?;
    let system_volume = plan
        .tasks
        .iter()
        .any(|task| crate::storage::is_task(&task.id) && task.id.ends_with("-system"))
        .then(system_volume)
        .transpose()?;
    Ok(Tools {
        winget,
        dism,
        sfc,
        defrag,
        system_volume,
        developer: developer_tool,
        pip: pip_layout,
    })
}
fn host(target: studio_core::Target) -> Result<()> {
    let host = studio_platform::host();
    ensure!(
        host.can_apply && host.target == Some(target),
        "Deployment requires the actual supported Windows LTSC target: {}",
        host.explanation
    );
    Ok(())
}
pub(super) fn inspect(plan: DeploymentPlan, cancel: Arc<AtomicBool>) -> Result<Vec<Detection>> {
    host(plan.target)?;
    let tools = tools(&plan)?;
    if plan.tasks.iter().any(|t| t.requires_admin) {
        ensure!(
            unsafe { IsUserAnAdmin() } != 0,
            "Inspecting selected features or system maintenance tasks requires an already elevated administrator process. No elevation was attempted."
        );
    }
    let mut runner = DeadlineRunner {
        deadline: Instant::now() + TASK_TIMEOUT,
    };
    let mut detections = Vec::new();
    for task in &plan.tasks {
        ensure!(!cancel.load(Ordering::Acquire), "Inspection cancelled");
        detections.push(engine::detect(task, &tools, &mut runner, &cancel)?);
    }
    Ok(detections)
}
struct DeadlineRunner {
    deadline: Instant,
}
impl Runner for DeadlineRunner {
    fn run(
        &mut self,
        command: &Invocation,
        cancel: &Arc<AtomicBool>,
    ) -> Result<crate::process::Output> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        ensure!(
            remaining > Duration::ZERO,
            "The 90-minute deployment session limit was reached"
        );
        let timeout = if command
            .program
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("defrag.exe"))
        {
            remaining.min(Duration::from_secs(45 * 60))
        } else {
            remaining
        };
        crate::process::run_bounded(command, cancel, timeout)
    }
}
struct FileHistory {
    file: File,
    bytes: usize,
}
impl History for FileHistory {
    fn append(&mut self, value: &serde_json::Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(value)?;
        bytes.push(b'\n');
        ensure!(
            self.bytes + bytes.len() <= 32 * 1024 * 1024,
            "Deployment history reached 32 MiB; execution stopped before another command"
        );
        self.file.write_all(&bytes)?;
        self.file.sync_all()?;
        self.bytes += bytes.len();
        Ok(())
    }
}
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
fn directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    ensure!(
        fs::symlink_metadata(path)?.is_dir(),
        "Deployment data directory must be a real directory"
    );
    Ok(())
}
pub(super) fn execute(
    plan: DeploymentPlan,
    cancel: Arc<AtomicBool>,
    events: Option<Sender<DeploymentEvent>>,
) -> Result<DeploymentReport> {
    host(plan.target)?;
    if plan.tasks.iter().any(|t| t.requires_admin) {
        ensure!(
            unsafe { IsUserAnAdmin() } != 0,
            "Selected features/maintenance require an already elevated administrator process. No elevation was attempted; no task ran."
        );
    }
    let tools = tools(&plan)?;
    let data = studio_platform::data_directory()?;
    directory(&data)?;
    let lock_path = data.join("mutation.lock");
    if lock_path.try_exists()? {
        ensure!(
            !fs::symlink_metadata(&lock_path)?.file_type().is_symlink(),
            "Mutation lock cannot be a symbolic link"
        );
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    lock.try_lock()
        .context("Another registry/deployment/restore operation is running")?;
    let history_dir = data.join("deployment-history");
    directory(&history_dir)?;
    let mut count = 0;
    for entry in fs::read_dir(&history_dir)? {
        let entry = entry?;
        if entry.path().extension().is_some_and(|s| s == "jsonl") {
            count += 1;
        }
        ensure!(
            count < 256,
            "Deployment history limit reached; archive reviewed history before running more tasks"
        );
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let history = history_dir.join(format!(
        "{now}-{}-{}.jsonl",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let file = File::create_new(&history)
        .context("Create deployment history before running any command")?;
    crate::network::record_current(&history.with_extension("network.json"))?;
    let mut history_store = FileHistory { file, bytes: 0 };
    let mut runner = DeadlineRunner {
        deadline: Instant::now() + TASK_TIMEOUT,
    };
    let outcome=engine::run_plan(&plan,&tools,&mut runner,&mut history_store,&cancel,events.as_ref()).with_context(||format!("Deployment stopped because execution evidence could not be persisted; partial system changes may exist. History: {}",history.display()))?;
    let summary = format!(
        "部署{}：完成 {} / {} 项，失败 {} 项，{}。{} 执行历史：{}。执行历史不提供安装或系统修复回滚；取消后 Windows servicing/安装器子进程可能仍运行。",
        if outcome.cancelled {
            "已取消"
        } else if outcome.failed > 0 {
            "已停止"
        } else {
            "完成"
        },
        outcome.completed,
        plan.tasks.len(),
        outcome.failed,
        if outcome.restart_required {
            "需要重启"
        } else {
            "未收到明确重启要求"
        },
        outcome.detail,
        history.display()
    );
    Ok(DeploymentReport {
        summary,
        history,
        completed: outcome.completed,
        failed: outcome.failed,
        cancelled: outcome.cancelled,
        restart_required: outcome.restart_required,
    })
}
