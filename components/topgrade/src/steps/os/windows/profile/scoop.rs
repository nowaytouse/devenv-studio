use super::{field, output, read_json, scoop_root};
use crate::{command::CommandExt, execution_context::RunType};
use color_eyre::eyre::{Result, ensure, eyre};
use regex::Regex;
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};

fn version_key(app: &str, label: &str) -> Result<(Vec<u32>, u32)> {
    let pattern = if app == "nodejs-nightly" {
        r"^(\d+\.\d+\.\d+)-nightly(\d{8})[a-f0-9]+$"
    } else {
        r"^(\d+\.\d+\.\d+)-canary\.(\d+)\+[a-f0-9]+$"
    };
    let caps = Regex::new(pattern)?
        .captures(label)
        .ok_or_else(|| eyre!("Unknown {app} version: {label}"))?;
    Ok((
        caps[1].split('.').map(str::parse).collect::<Result<_, _>>()?,
        caps[2].parse()?,
    ))
}
fn advance(app: &str, old: &str, new: &str, hash_changed: bool) -> Result<bool> {
    let a = version_key(app, old)?;
    let b = version_key(app, new)?;
    Ok(if old == new {
        hash_changed
    } else if app == "nodejs-nightly" {
        b.0 >= a.0 && b.1 >= a.1
    } else {
        b >= a
    })
}
fn strings(v: &Value) -> Result<Vec<&str>> {
    if let Some(s) = v.as_str() {
        return Ok(vec![s]);
    }
    let a = v.as_array().ok_or_else(|| eyre!("Expected a list of strings"))?;
    ensure!(!a.is_empty(), "Empty manifest list");
    a.iter()
        .map(|v| v.as_str().ok_or_else(|| eyre!("Invalid manifest list entry")))
        .collect()
}
fn hashes(manifest: &Value, app: &str) -> Result<Vec<String>> {
    let info = &manifest["architecture"]["64bit"];
    let urls = strings(&info["url"])?;
    let hashes = strings(&info["hash"])?;
    let version = field(manifest, "version")?;
    version_key(app, version)?;
    ensure!(urls.len() == hashes.len(), "Missing {app} checksums");
    for (url, hash) in urls.iter().zip(&hashes) {
        let valid = if app == "nodejs-nightly" {
            *url == format!("https://nodejs.org/download/nightly/v{version}/node-v{version}-win-x64.7z")
        } else {
            matches!(
                *url,
                "https://github.com/oven-sh/bun/releases/download/canary/bun-windows-x64.zip"
                    | "https://github.com/oven-sh/bun/releases/download/canary/bun-windows-x64-baseline.zip"
            )
        };
        ensure!(
            valid && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
            "Unexpected {app} release source or checksum"
        );
    }
    Ok(hashes.into_iter().map(str::to_ascii_lowercase).collect())
}
pub(super) fn rolling(mode: RunType, check: bool) -> Result<Value> {
    let root = scoop_root()?;
    let mut items = Vec::new();
    for app in ["nodejs-nightly", "bun-canary"] {
        let current = root.join("apps").join(app).join("current");
        if !current.try_exists()? {
            continue;
        }
        let manifest = current.join("scoop-manifest.json");
        let before = read_json(&manifest)?;
        let owner = read_json(&current.join("scoop-install.json"))?;
        if owner["bucket"] != "versions" || !matches!(owner["hold"], Value::Null | Value::Bool(false)) {
            items.push(json!({"app":app,"status":"Held or owned by another bucket"}));
            continue;
        }
        let catalog = read_json(&root.join("buckets/versions/bucket").join(format!("{app}.json")))?;
        let old = field(&before, "version")?;
        let new = field(&catalog, "version")?;
        let wanted = hashes(&catalog, app)?;
        let pending = advance(app, old, new, hashes(&before, app)? != wanted)?;
        if pending && !check {
            mode.execute(root.join("apps/pwsh-beta/current/pwsh.exe"))
                .args(["-NoProfile", "-File"])
                .arg(root.join("shims/scoop.ps1"))
                .args(["update", "--force", app])
                .status_checked()?;
            let actual = read_json(&manifest)?;
            ensure!(
                actual["version"] == new && hashes(&actual, app)? == wanted,
                "Installed {app} manifest differs from the selected release"
            );
        }
        let (exe, arg) = if app == "nodejs-nightly" {
            ("node.exe", "--version")
        } else {
            ("bun.exe", "--revision")
        };
        let runtime = output(mode.execute(current.join(exe)).arg(arg))?;
        println!("{app}: {runtime} (catalog {new})");
        items.push(json!({"app":app,"before":old,"catalog":new,"update_available":pending,"changed":pending && !check,"runtime":runtime}));
    }
    Ok(
        json!({"status":if check && items.iter().any(|v|v["update_available"]==true){"UpdateAvailable"}else{"Verified"},"items":items}),
    )
}
const PLUGINS: [&str; 5] = ["polars", "formats", "gstat", "query", "inc"];
pub(super) fn plugins(mode: RunType, check: bool) -> Result<Value> {
    let current = scoop_root()?.join("apps/nu/current");
    let nu = current.join("nu.exe");
    let registry_dir =
        output(
            mode.execute(&nu)
                .args(["--no-config-file", "--commands", "$nu.default-config-dir | print"]),
        )?;
    let dir = Path::new(&registry_dir);
    ensure!(dir.is_absolute(), "Nushell returned a relative registry directory");
    let registry = dir.join("plugin.msgpackz");
    for name in PLUGINS {
        ensure!(
            current.join(format!("nu_plugin_{name}.exe")).is_file(),
            "Missing bundled Nushell plugin {name}"
        );
    }
    if !check {
        std::fs::create_dir_all(dir)?;
        for name in PLUGINS {
            output(
                mode.execute(&nu)
                    .env("TOPGRADE_NU_REGISTRY", &registry)
                    .env("TOPGRADE_NU_PLUGIN", current.join(format!("nu_plugin_{name}.exe")))
                    .args([
                        "--no-config-file",
                        "--commands",
                        "plugin add --plugin-config $env.TOPGRADE_NU_REGISTRY $env.TOPGRADE_NU_PLUGIN",
                    ]),
            )?;
        }
    }
    let result = output(mode.execute(nu).env("TOPGRADE_NU_REGISTRY", &registry).args([
        "--no-config-file",
        "--commands",
        "plugin list --plugin-config $env.TOPGRADE_NU_REGISTRY --registry | get name | to json --raw",
    ]))?;
    let actual: BTreeSet<String> = serde_json::from_str(&result)?;
    for name in PLUGINS {
        ensure!(actual.contains(name), "Plugin registry is missing {name}");
    }
    Ok(json!({"status":"Verified","plugins":PLUGINS}))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rolling_versions_respect_each_channel() {
        let a = "28.0.0-nightly20261109abcd";
        let b = "28.0.0-nightly20261114abcd";
        assert!(advance("nodejs-nightly", a, b, true).unwrap());
        assert!(!advance("nodejs-nightly", b, a, true).unwrap());
        assert!(!advance("nodejs-nightly", b, b, false).unwrap());
        let v = "1.4.5-canary.1+abc";
        for (new, hash, want) in [
            ("1.4.5-canary.1+def", true, true),
            (v, true, true),
            ("1.4.4-canary.1+def", true, false),
            ("1.4.5-canary.0+def", true, false),
        ] {
            assert_eq!(advance("bun-canary", v, new, hash).unwrap(), want);
        }
    }
}
