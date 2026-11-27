use color_eyre::eyre::{Result, WrapErr, bail, eyre};
use serde_json::Value;
use std::path::Path;

fn equals(value: &Value, expected: &str) -> bool {
    value.as_str().is_some_and(|v| v.eq_ignore_ascii_case(expected))
}

// A multi-root or multi-spec manifest may update the selected SDK but cannot safely retry the command.
pub(super) fn validate_manifest(bytes: &[u8], install: &Path) -> Result<bool> {
    let spec: Value = serde_json::from_slice(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes))
        .wrap_err("Invalid dotnetup manifest JSON")?;
    let roots = spec["dotnetRoots"]
        .as_array()
        .ok_or_else(|| eyre!("Invalid dotnetup manifest roots"))?;
    let mut matching = roots
        .iter()
        .filter(|r| equals(&r["path"], &install.to_string_lossy()) && equals(&r["architecture"], "x64"));
    let root = matching
        .next()
        .filter(|_| matching.next().is_none())
        .ok_or_else(|| eyre!("Expected one matching x64 install root"))?;
    let specs = root["installSpecs"]
        .as_array()
        .ok_or_else(|| eyre!("Invalid daily SDK install specs"))?;
    let mut daily = specs
        .iter()
        .filter(|s| equals(&s["component"], "SDK") && equals(&s["versionOrChannel"], "daily"));
    let spec = daily
        .next()
        .filter(|_| daily.next().is_none())
        .ok_or_else(|| eyre!("Expected one rolling daily SDK requirement"))?;
    Ok(roots.len() == 1 && specs.len() == 1 && equals(&spec["installSource"], "Explicit"))
}

pub(super) fn validate_inventory(inventory: &Value, install: &Path, version: &str) -> Result<()> {
    let items = inventory
        .get("installations")
        .and_then(Value::as_array)
        .ok_or_else(|| eyre!("Invalid dotnetup inventory"))?;
    let valid: Vec<_> = items
        .iter()
        .filter(|item| {
            equals(&item["component"], "SDK")
                && equals(&item["installRoot"], &install.to_string_lossy())
                && equals(&item["architecture"], "x64")
                && item["isValid"] == true
        })
        .collect();
    if valid.is_empty() {
        bail!("dotnetup found no valid x64 daily SDK in the selected root");
    }
    if version.is_empty()
        || version.contains(['\r', '\n'])
        || valid
            .iter()
            .filter(|item| item["version"].as_str() == Some(version))
            .count()
            != 1
    {
        bail!("Runtime SDK resolution does not match the verified manager inventory");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    const INSTALL: &str = r"D:\test\managed";
    #[test]
    fn requires_one_daily_spec_and_unique_verified_host() {
        let root = Path::new(INSTALL);
        let mut manifest = json!({"dotnetRoots":[{"path":INSTALL,"architecture":"x64","installSpecs":[{"component":"SDK","versionOrChannel":"daily"}]}]});
        assert!(!validate_manifest(manifest.to_string().as_bytes(), root).unwrap());
        manifest["dotnetRoots"][0]["installSpecs"][0]["installSource"] = json!("Explicit");
        assert!(validate_manifest(manifest.to_string().as_bytes(), root).unwrap());
        manifest["dotnetRoots"][0]["installSpecs"] = json!([]);
        assert!(validate_manifest(manifest.to_string().as_bytes(), root).is_err());
        let item =
            json!({"component":"SDK","installRoot":INSTALL,"architecture":"x64","isValid":true,"version":"11.0.100"});
        assert!(validate_inventory(&json!({"installations":[item.clone()]}), root, "11.0.100").is_ok());
        assert!(validate_inventory(&json!({"installations":[item.clone(),item.clone()]}), root, "11.0.100").is_err());
        assert!(validate_inventory(&json!({"installations":[]}), root, "11.0.100").is_err());
        assert!(validate_inventory(&json!({"installations":[item]}), root, "11.0.100\n").is_err());
    }
}
