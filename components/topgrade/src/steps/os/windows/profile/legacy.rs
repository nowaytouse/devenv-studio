//! Preserve PEP 514 registrations belonging to other Python installations.
use super::{
    FRONTIER, desktop, env_path, field, output,
    python::{optional, probe},
    sha, write_report,
};
use crate::execution_context::RunType;
use color_eyre::eyre::{Result, ensure, eyre};
use regex::Regex;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    os::windows::fs::MetadataExt,
    path::{Component, Path},
};
use windows_registry::{CURRENT_USER, Key, LOCAL_MACHINE, Transaction, Type};

type Sections = BTreeMap<String, BTreeMap<String, String>>;
const COMPANY: &str = r"HKEY_CURRENT_USER\Software\Python\PythonCore";
fn unquote(raw: &str) -> Result<String> {
    let s = raw
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or_else(|| eyre!("Expected a quoted REG_SZ value"))?;
    let mut chars = s.chars();
    let mut result = String::new();
    while let Some(c) = chars.next() {
        ensure!(!c.is_control() && c != '"', "Unexpected character in registry text");
        if c == '\\' {
            let c = chars.next().ok_or_else(|| eyre!("Truncated registry escape"))?;
            ensure!(matches!(c, '\\' | '"'), "Unsupported registry escape");
            result.push(c);
        } else {
            result.push(c);
        }
    }
    Ok(result)
}
fn parse(bytes: &[u8]) -> Result<Sections> {
    let text = if let Some(data) = bytes.strip_prefix(&[255, 254]) {
        ensure!(data.len().is_multiple_of(2), "Invalid UTF-16 registry data");
        String::from_utf16(
            &data
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect::<Vec<_>>(),
        )?
    } else {
        std::str::from_utf8(bytes)?.trim_start_matches('\u{feff}').to_owned()
    };
    let mut lines = text.lines();
    ensure!(
        lines.next() == Some("Windows Registry Editor Version 5.00"),
        "Unrecognized package registration format"
    );
    let mut out = Sections::new();
    let mut section = None;
    for line in lines.map(str::trim).filter(|l| !l.is_empty() && !l.starts_with(';')) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            ensure!(!out.contains_key(name), "Duplicate registry section");
            section = Some(out.entry(name.to_owned()).or_default());
        } else {
            let (key, val) = line
                .split_once('=')
                .ok_or_else(|| eyre!("Invalid registry assignment"))?;
            let key = if key == "@" { String::new() } else { unquote(key)? };
            let values = section
                .as_deref_mut()
                .ok_or_else(|| eyre!("Missing registry section"))?;
            ensure!(values.insert(key, unquote(val)?).is_none(), "Duplicate registry value");
        }
    }
    Ok(out)
}
fn within(path: &str, root: &Path) -> Result<()> {
    let p = Path::new(path);
    ensure!(
        path.as_bytes().get(1..3) == Some(b":\\")
            && !path.contains('%')
            && !path.chars().any(char::is_control)
            && !path[2..].contains(':')
            && !p.components().any(|c| matches!(c, Component::ParentDir)),
        "Unsafe registry path"
    );
    let norm = |p: &Path| {
        p.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    };
    let path = norm(p);
    let base = norm(root);
    ensure!(
        path == base || path.starts_with(&format!("{base}\\")),
        "Registration points outside the Scoop package"
    );
    Ok(())
}
fn owned(key: &Key, package: &Path) -> Result<()> {
    let mut evidence = false;
    for sub in ["InstallPath", "PythonPath", r"Help\Main Python Documentation"] {
        if let Some(k) = optional(key.open(sub))? {
            for (name, val) in k.values()? {
                if sub.starts_with("Help") && !name.is_empty() {
                    continue;
                }
                ensure!(val.ty() == Type::String, "Unsupported registration path type");
                for path in String::try_from(val)?.split(';').filter(|p| !p.is_empty()) {
                    within(path, package)?;
                    evidence = true;
                }
            }
        }
    }
    ensure!(evidence, "Existing Python registration has no Scoop ownership evidence");
    Ok(())
}
fn matches(key: Option<&Key>, sections: &Sections) -> Result<bool> {
    let Some(key) = key else { return Ok(false) };
    for (section, values) in sections {
        let Some(k) = optional(key.open(section))? else {
            return Ok(false);
        };
        for (name, want) in values {
            let Some(val) = optional(k.get_value(name))? else {
                return Ok(false);
            };
            if val.ty() != Type::String || String::try_from(val)? != *want {
                return Ok(false);
            }
        }
    }
    Ok(true)
}
pub(super) fn maintain(mode: RunType, package: &Path, check: bool) -> Result<Value> {
    let current = package.join("current");
    let source = current.join("install-pep-514.reg");
    ensure!(
        fs::symlink_metadata(&source)?.file_attributes() & 0x400 == 0,
        "Redirected package registration file"
    );
    let hash = sha(&source)?;
    let registration = parse(&fs::read(&source)?)?;
    let runtime = probe(mode, &current.join("python.exe"))?;
    let version = field(&runtime, "version")?;
    let minor = field(&runtime, "minor")?;
    ensure!(
        Regex::new(r"^\d+\.\d+\.\d+(?:(?:a|b|rc)\d+)?$")?.is_match(version)
            && Regex::new(r"^\d+\.\d+$")?.is_match(minor),
        "Invalid Python version"
    );
    let bits = runtime["bits"]
        .as_u64()
        .ok_or_else(|| eyre!("Missing runtime bitness"))?;
    ensure!(matches!(bits, 32 | 64), "Unsupported Python architecture");
    let version_key = format!("{COMPANY}\\{minor}");
    let schema: &[(&str, &[&str])] = &[
        (
            "",
            &["DisplayName", "SupportUrl", "Version", "SysVersion", "SysArchitecture"],
        ),
        ("InstallPath", &["", "ExecutablePath", "WindowedExecutablePath"]),
        ("PythonPath", &[""]),
        (r"Help\Main Python Documentation", &["", "DisplayName"]),
    ];
    let company = registration
        .get(COMPANY)
        .ok_or_else(|| eyre!("Missing company metadata"))?;
    ensure!(
        company.len() == 2
            && company.contains_key("DisplayName")
            && company
                .get("SupportUrl")
                .is_some_and(|s| s == "https://www.python.org/")
            && registration.len() == schema.len() + 1,
        "Unexpected package registration schema"
    );
    let mut sections = Sections::new();
    for (sub, names) in schema {
        let key = if sub.is_empty() {
            version_key.clone()
        } else {
            format!("{version_key}\\{sub}")
        };
        let values = registration
            .get(&key)
            .ok_or_else(|| eyre!("Missing expected registry section {sub}"))?;
        ensure!(
            values.len() == names.len() && names.iter().all(|name| values.contains_key(*name)),
            "Unexpected values for {sub}"
        );
        sections.insert(sub.to_string(), values.clone());
    }
    let info = &sections[""];
    ensure!(
        info["Version"] == version
            && info["SysVersion"] == minor
            && info["SysArchitecture"] == format!("{bits}bit")
            && info["SupportUrl"] == "https://www.python.org/",
        "Package metadata does not match installed Python"
    );
    sections
        .get_mut("")
        .unwrap()
        .insert("DisplayName".into(), format!("Python {minor} ({bits}-bit)"));
    let dir = package.join(version);
    ensure!(
        fs::canonicalize(&dir)?.starts_with(fs::canonicalize(package)?)
            && fs::canonicalize(&current)? == fs::canonicalize(&dir)?,
        "Scoop current points outside the selected package version"
    );
    let install = &sections["InstallPath"];
    within(&install[""], &dir)?;
    ensure!(
        fs::canonicalize(&install[""])? == fs::canonicalize(&dir)?,
        "Unexpected Python installation root"
    );
    for (key, exe) in [
        ("ExecutablePath", "python.exe"),
        ("WindowedExecutablePath", "pythonw.exe"),
    ] {
        let path = Path::new(&install[key]);
        within(&install[key], &dir)?;
        ensure!(
            fs::symlink_metadata(path)?.file_attributes() & 0x400 == 0
                && fs::canonicalize(path)? == fs::canonicalize(dir.join(exe))?
                && sha(path)? == sha(&current.join(exe))?,
            "Mismatched Python package executable"
        );
    }
    for path in sections["PythonPath"][""]
        .split(';')
        .chain([sections[r"Help\Main Python Documentation"][""].as_str()])
    {
        within(path, &dir)?;
    }
    let key_path = format!(r"Software\Python\PythonCore\{minor}");
    let view = if bits == 64 { 0x0100 } else { 0x0200 };
    let user = optional(CURRENT_USER.options().read().access(view).open(&key_path))?;
    if let Some(key) = &user {
        owned(key, package)?;
    }
    for view in [0x100, 0x200] {
        if let Some(key) = optional(LOCAL_MACHINE.options().read().access(view).open(&key_path))? {
            owned(&key, package)?;
        }
    }
    let mut report =
        json!({"status":"Healthy","version":version,"bits":bits,"source_sha256":hash,"version_key":key_path});
    if matches(user.as_ref(), &sections)? {
        return Ok(report);
    }
    ensure!(!check, "Scoop Python launcher registration requires maintenance");
    let parent = desktop()?.join(FRONTIER).join("Python/legacy-registration-backups");
    fs::create_dir_all(&parent)?;
    let backup = tempfile::Builder::new()
        .prefix("registration-")
        .tempdir_in(&parent)?
        .keep();
    write_report(
        &backup.join("before.json"),
        &json!({"version_key":key_path,"source_sha256":hash,"existed":user.is_some()}),
    )?;
    if user.is_some() {
        output(
            mode.execute(env_path("SystemRoot")?.join("System32/reg.exe"))
                .args(["export", &format!(r"HKCU\{key_path}")])
                .arg(backup.join("before.reg"))
                .arg(format!("/reg:{bits}")),
        )?;
        ensure!(
            fs::metadata(backup.join("before.reg"))?.len() > 0,
            "Empty registry backup"
        );
    } else {
        fs::write(backup.join("key-was-absent.txt"), &key_path)?;
    }
    ensure!(
        sha(&source)? == hash,
        "Package registration changed during verification"
    );
    let tx = Transaction::new()?;
    {
        let observed = optional(
            CURRENT_USER
                .options()
                .access(view)
                .read()
                .transaction(&tx)
                .open(&key_path),
        )?;
        ensure!(
            observed.is_some() == user.is_some(),
            "Version key changed while its backup was being prepared"
        );
        let key = CURRENT_USER
            .options()
            .access(view)
            .read()
            .write()
            .create()
            .transaction(&tx)
            .open(&key_path)?;
        if user.is_some() {
            owned(&key, package)?;
        }
        write_sections(&key, &tx, &sections)?;
    }
    tx.commit()?;
    let after = CURRENT_USER.options().read().access(view).open(&key_path)?;
    ensure!(
        matches(Some(&after), &sections)?,
        "Python registry readback failed; backup at {}",
        backup.display()
    );
    report["status"] = json!("Maintained");
    report["backup_directory"] = json!(backup);
    write_report(&backup.join("after.json"), &report)?;
    Ok(report)
}
fn write_sections(key: &Key, tx: &Transaction, sections: &Sections) -> Result<()> {
    for (section, values) in sections {
        let dest = key.options().read().write().create().transaction(tx).open(section)?;
        for (name, value) in values {
            dest.set_string(name, value)?;
        }
        for (name, expected) in values {
            ensure!(
                dest.get_string(name)? == *expected,
                "Transactional registry readback failed"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn version_write_preserves_other_values_and_rolls_back_without_commit() {
        let unique = tempfile::tempdir().unwrap();
        let name = format!(
            r"Software\Topgrade-PEP514-Test-{}",
            unique.path().file_name().unwrap().to_string_lossy()
        );
        assert!(optional(CURRENT_USER.open(&name)).unwrap().is_none());
        {
            let tx = Transaction::new().unwrap();
            let key = CURRENT_USER
                .options()
                .read()
                .write()
                .create()
                .transaction(&tx)
                .open(&name)
                .unwrap();
            key.set_string("Unrelated", "keep").unwrap();
            let data = Sections::from([
                (
                    String::new(),
                    BTreeMap::from([("DisplayName".into(), "Python test".into())]),
                ),
                (
                    "InstallPath".into(),
                    BTreeMap::from([(String::new(), String::from(r"C:\Synthetic\Scoop\python"))]),
                ),
            ]);
            write_sections(&key, &tx, &data).unwrap();
            assert_eq!(key.get_string("Unrelated").unwrap(), "keep");
        }
        assert!(optional(CURRENT_USER.open(&name)).unwrap().is_none());
    }

    #[test]
    fn rejects_duplicate_and_non_string_reg_data() {
        for lines in [
            "[key]\n@=dword:00000001",
            "[key]\n@=\"a\"\n@=\"b\"",
            "[key]\n@=\"bad\\n\"",
        ] {
            assert!(parse(format!("Windows Registry Editor Version 5.00\n{lines}").as_bytes()).is_err());
        }
    }
    #[test]
    fn does_not_confuse_sibling_or_parent_paths() {
        let root = Path::new(r"C:\Scoop\Python");
        assert!(within(r"C:\scoop\python\v1\python.exe", root).is_ok());
        for p in [
            r"C:\Scoop\PythonOld\file",
            r"C:\Scoop\Python\..\outside",
            r"\\host\C$\Scoop\Python",
            r"C:\Scoop\Python\a:stream",
        ] {
            assert!(within(p, root).is_err());
        }
    }
}
