use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RegistryHive {
    CurrentUser,
    LocalMachine,
}

#[cfg(target_os = "windows")]
pub fn get_registry_dword(
    hive: RegistryHive,
    path: &str,
    name: &str,
) -> anyhow::Result<Option<u32>> {
    use std::io::ErrorKind;
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    let root = match hive {
        RegistryHive::CurrentUser => RegKey::predef(HKEY_CURRENT_USER),
        RegistryHive::LocalMachine => RegKey::predef(HKEY_LOCAL_MACHINE),
    };
    let key = match root.open_subkey(path) {
        Ok(key) => key,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    match key.get_value(name) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(not(target_os = "windows"))]
pub fn get_registry_dword(
    _hive: RegistryHive,
    _path: &str,
    _name: &str,
) -> anyhow::Result<Option<u32>> {
    anyhow::bail!("Windows 注册表仅能在 Windows 上读取")
}

#[cfg(target_os = "windows")]
pub fn set_registry_dword(
    hive: RegistryHive,
    path: &str,
    name: &str,
    value: u32,
) -> anyhow::Result<()> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    let root = match hive {
        RegistryHive::CurrentUser => RegKey::predef(HKEY_CURRENT_USER),
        RegistryHive::LocalMachine => RegKey::predef(HKEY_LOCAL_MACHINE),
    };
    let (key, _) = root.create_subkey(path)?;
    key.set_value(name, &value)?;
    let actual: u32 = key.get_value(name)?;
    anyhow::ensure!(
        actual == value,
        "Windows registry readback differed after writing {name}"
    );
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn set_registry_dword(
    _hive: RegistryHive,
    _path: &str,
    _name: &str,
    _value: u32,
) -> anyhow::Result<()> {
    anyhow::bail!("Windows 注册表仅能在 Windows 上修改")
}

pub(crate) fn scoop_root() -> anyhow::Result<std::path::PathBuf> {
    if let Some(value) = std::env::var_os("SCOOP") {
        anyhow::ensure!(!value.is_empty(), "SCOOP is empty");
        return Ok(value.into());
    }
    #[cfg(windows)]
    {
        use winreg::{RegKey, enums::HKEY_CURRENT_USER};
        match RegKey::predef(HKEY_CURRENT_USER).open_subkey("Environment") {
            Ok(environment) => match environment.get_value::<String, _>("SCOOP") {
                Ok(value) => {
                    anyhow::ensure!(!value.is_empty(), "Persisted SCOOP is empty");
                    return Ok(value.into());
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(std::path::PathBuf::from(
        std::env::var_os("USERPROFILE")
            .ok_or_else(|| anyhow::anyhow!("USERPROFILE is unavailable"))?,
    )
    .join("scoop"))
}

#[cfg(target_os = "windows")]
pub fn set_user_environment(name: &str, value: &str) -> anyhow::Result<()> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    let root = RegKey::predef(HKEY_CURRENT_USER);
    let (environment, _) = root.create_subkey("Environment")?;
    environment.set_value(name, &value)?;
    let actual: String = environment.get_value(name)?;
    anyhow::ensure!(
        actual == value,
        "User environment readback differed for {name}"
    );
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn set_user_environment(_name: &str, _value: &str) -> anyhow::Result<()> {
    anyhow::bail!("Windows 用户环境变量仅能在 Windows 上修改")
}

#[cfg(target_os = "windows")]
pub fn prepend_user_path(path: &Path) -> anyhow::Result<bool> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    let root = RegKey::predef(HKEY_CURRENT_USER);
    let (environment, _) = root.create_subkey("Environment")?;
    let current: String = match environment.get_value("Path") {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let candidate = path.to_string_lossy();
    let already_persisted = current
        .split(';')
        .any(|entry| entry.trim().eq_ignore_ascii_case(&candidate));
    if !already_persisted {
        let updated = if current.trim().is_empty() {
            candidate.clone().into_owned()
        } else {
            format!("{};{}", candidate, current)
        };
        environment.set_value("Path", &updated)?;
        let actual: String = environment.get_value("Path")?;
        anyhow::ensure!(
            actual == updated,
            "User PATH readback differed after writing"
        );
    }
    Ok(!already_persisted)
}

#[cfg(not(target_os = "windows"))]
pub fn prepend_user_path(_path: &Path) -> anyhow::Result<bool> {
    anyhow::bail!("Windows 用户 PATH 仅能在 Windows 上修改")
}
