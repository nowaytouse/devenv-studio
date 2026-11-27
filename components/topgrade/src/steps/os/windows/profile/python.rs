use super::{env_path, field, legacy, output, scoop_root, sha};
use crate::execution_context::RunType;
use color_eyre::eyre::{Result, ensure, eyre};
use regex::Regex;
use serde_json::{Value, json};
use std::{
    env, fs,
    path::{Path, PathBuf},
};
use windows_registry::CURRENT_USER;

pub(super) fn optional<T>(v: windows_registry::Result<T>) -> Result<Option<T>> {
    match v {
        Ok(t) => Ok(Some(t)),
        Err(e) if matches!(e.code().0 as u32, 0x80070002 | 0x80070003) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
pub(super) fn probe(mode: RunType, path: &Path) -> Result<Value> {
    let script = "import json,sys,ssl,sqlite3,venv,tomllib;print(json.dumps(dict(version=sys.version.split()[0],minor='%d.%d'%sys.version_info[:2],executable=sys.executable,bits=64 if sys.maxsize>2**32 else 32,ssl=ssl.OPENSSL_VERSION,sqlite=sqlite3.sqlite_version)))";
    Ok(serde_json::from_str(&output(
        mode.execute(path).args(["-I", "-S", "-B", "-c", script]),
    )?)?)
}
struct Managed {
    mode: RunType,
    uv: PathBuf,
    exe: PathBuf,
    root: PathBuf,
}
impl Managed {
    fn new(mode: RunType) -> Result<Self> {
        let scoop = scoop_root()?;
        let dir = |key, subpath| {
            if env::var_os(key).is_some() {
                env_path(key)
            } else {
                Ok(scoop.join(subpath))
            }
        };
        Ok(Self {
            mode,
            uv: scoop.join("apps/uv/current/uv.exe"),
            exe: dir("UV_PYTHON_BIN_DIR", "persist/uv/python/shims")?.join("python.exe"),
            root: dir("UV_PYTHON_INSTALL_DIR", "persist/uv/python/versions")?,
        })
    }
    fn runtime(&self) -> Result<Value> {
        let runtime = probe(self.mode, &self.exe)?;
        let version = field(&runtime, "version")?;
        ensure!(
            Regex::new(r"^3\.\d+\.\d+(?:(?:a|b|rc)\d+)?$")?.is_match(version) && runtime["bits"] == 64,
            "Unexpected managed Python version or architecture"
        );
        let expected = self
            .root
            .join(format!("cpython-{version}-windows-x86_64-none/python.exe"));
        let actual = fs::canonicalize(field(&runtime, "executable")?)?;
        ensure!(
            actual.starts_with(fs::canonicalize(&self.root)?) && actual == fs::canonicalize(expected)?,
            "Runtime resolves outside the exact managed Python installation"
        );
        Ok(runtime)
    }
    fn install(&self, version: &str) -> Result<()> {
        output(self.mode.execute(&self.uv).args([
            "python",
            "install",
            version,
            "--default",
            "--preview-features",
            "python-install-default",
            "--no-progress",
        ]))?;
        Ok(())
    }
    fn registered(&self, version: &str) -> Result<bool> {
        let Some(key) = optional(CURRENT_USER.open(format!(r"Software\Python\Astral\CPython{version}")))? else {
            return Ok(false);
        };
        if let Some(install) = optional(key.open("InstallPath"))?
            && let Some(path) = optional(install.get_string("ExecutablePath"))?
        {
            let expected = self
                .root
                .join(format!("cpython-{version}-windows-x86_64-none/python.exe"));
            ensure!(
                fs::canonicalize(&path)? == fs::canonicalize(expected)?,
                "Astral registration belongs to a different installation"
            );
            return Ok(optional(key.get_string("Version"))?.as_deref() == Some(version)
                && optional(key.get_string("SysArchitecture"))?.as_deref() == Some("64bit"));
        }
        Ok(false)
    }
}
pub(super) fn launcher(mode: RunType, check: bool) -> Result<Value> {
    let m = Managed::new(mode)?;
    let package = scoop_root()?.join("apps/python-beta");
    let legacy = if package.join("current/python.exe").try_exists()? {
        Some(legacy::maintain(mode, &package, check)?)
    } else {
        None
    };
    if !m.exe.try_exists()? {
        ensure!(legacy.is_some(), "No managed Python installation found");
        return Ok(json!({"status":"Healthy","legacy":legacy}));
    }
    let runtime = m.runtime()?;
    let version = field(&runtime, "version")?;
    if !m.registered(version)? {
        ensure!(!check, "Astral Python registration requires maintenance");
        m.install(version)?;
        ensure!(
            m.registered(version)?,
            "Astral Python registration repair failed verification"
        );
    }
    Ok(json!({"status":"Healthy","runtime":runtime,"legacy":legacy}))
}
fn select(list: &Value) -> Result<&Value> {
    let pattern = Regex::new(r"^3\.(\d+)\.(\d+)(a|b|rc)(\d+)$")?;
    let mut best = None;
    for item in list.as_array().ok_or_else(|| eyre!("Invalid uv download catalog"))? {
        if item["implementation"] != "cpython"
            || item["os"] != "windows"
            || item["arch"] != "x86_64"
            || item["variant"] != "default"
        {
            continue;
        }
        let Some(c) = pattern.captures(field(item, "version")?) else {
            continue;
        };
        let rank = (
            c[1].parse::<u32>()?,
            c[2].parse::<u32>()?,
            match &c[3] {
                "a" => 0,
                "b" => 1,
                _ => 2,
            },
            c[4].parse::<u32>()?,
        );
        if best.is_none_or(|(prior, _)| rank > prior) {
            best = Some((rank, item));
        }
    }
    let (_, item) = best.ok_or_else(|| eyre!("No compatible Windows x64 preview in uv catalog"))?;
    let url = field(item, "url")?;
    ensure!(
        url.starts_with("https://github.com/astral-sh/python-build-standalone/releases/download/")
            || url.starts_with("https://releases.astral.sh/github/python-build-standalone/releases/download/"),
        "Unrecognized Python distribution source"
    );
    Ok(item)
}
pub(super) fn preview(mode: RunType, check: bool) -> Result<Value> {
    let m = Managed::new(mode)?;
    let manager = output(mode.execute(&m.uv).arg("--version"))?;
    let catalog = output(mode.execute(&m.uv).args([
        "python",
        "list",
        "--only-downloads",
        "--all-versions",
        "--show-urls",
        "--output-format",
        "json",
    ]))?;
    let catalog = serde_json::from_str(&catalog)?;
    let selected = select(&catalog)?;
    let version = field(selected, "version")?;
    let previous = if m.exe.try_exists()? { Some(m.runtime()?) } else { None };
    let pending = previous.as_ref().is_none_or(|v| v["version"] != version);
    if check && pending {
        return Ok(
            json!({"status":"UpdateAvailable","selected_version":version,"previous":previous,"manager":manager}),
        );
    }
    if !check {
        m.install(version)?;
    }
    let runtime = m.runtime()?;
    ensure!(
        runtime["version"] == version,
        "uv default runtime differs from selected preview"
    );
    let registration = launcher(mode, true)?;
    println!("{manager}: CPython {version} (x64)");
    Ok(
        json!({"status":"Current","manager":manager,"source":selected["url"],"selected_version":version,"runtime":runtime,"executable_sha256":sha(Path::new(field(&runtime,"executable")?))?,"launcher":registration}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_orders_prereleases_and_rejects_untrusted_owner() {
        let item = |v: &str| json!({"version":v,"implementation":"cpython","os":"windows","arch":"x86_64","variant":"default","url":"https://github.com/astral-sh/python-build-standalone/releases/download/build/python.tar.gz"});
        let catalog = json!([
            item("3.16.0a10"),
            item("3.16.0b2"),
            item("3.16.0b10"),
            item("3.15.0rc3"),
            item("3.16.0")
        ]);
        assert_eq!(select(&catalog).unwrap()["version"], "3.16.0b10");
        let mut invalid = catalog;
        invalid[2]["url"] = json!("https://github.com/other/python-build-standalone/releases/download/build/x");
        assert!(select(&invalid).is_err());
    }
}
