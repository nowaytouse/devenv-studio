use anyhow::{Context, Result};
use jsonc_parser::cst::{CstInputValue, CstObject, CstRootNode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

const SCHEMA_VERSION: u32 = 1;
const MAX_SETTINGS_BYTES: usize = 1024 * 1024;
const MAX_PROFILE_BYTES: usize = 64 * 1024;
const ALLOWED_SETTINGS: &[&str] = &[
    "editor.fontFamily",
    "editor.fontSize",
    "editor.fontWeight",
    "editor.formatOnSave",
    "editor.insertSpaces",
    "editor.lineHeight",
    "editor.lineNumbers",
    "editor.minimap.enabled",
    "editor.renderWhitespace",
    "editor.tabSize",
    "editor.wordWrap",
    "files.autoSave",
    "files.autoSaveDelay",
    "files.eol",
    "files.insertFinalNewline",
    "files.trimTrailingWhitespace",
    "terminal.integrated.fontFamily",
    "terminal.integrated.fontSize",
    "window.zoomLevel",
    "workbench.colorTheme",
    "workbench.iconTheme",
];
static APPLY_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SettingsProfile {
    pub schema_version: u32,
    pub settings: BTreeMap<String, Value>,
}

impl SettingsProfile {
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.schema_version == SCHEMA_VERSION,
            "Unsupported VS Code settings profile version"
        );
        anyhow::ensure!(
            self.settings.len() <= ALLOWED_SETTINGS.len(),
            "VS Code settings profile has too many entries"
        );
        for (key, value) in &self.settings {
            validate_value(key, value)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SettingsSnapshot {
    path: PathBuf,
    original: Option<Vec<u8>>,
    pub settings: BTreeMap<String, Value>,
}

impl SettingsSnapshot {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SettingsApplyReport {
    pub schema_version: u32,
    pub selected: Vec<String>,
    pub changed: Vec<String>,
    pub verified: bool,
    pub history_complete: bool,
    pub cancelled: bool,
    pub detail: String,
    pub history: PathBuf,
}

#[derive(Debug)]
pub struct SettingsApplyOutcome {
    pub report: SettingsApplyReport,
    pub after: Option<SettingsSnapshot>,
}

pub fn profile_from_snapshot(snapshot: &SettingsSnapshot) -> SettingsProfile {
    SettingsProfile {
        schema_version: SCHEMA_VERSION,
        settings: snapshot.settings.clone(),
    }
}

pub fn capture_settings(cancel: &Arc<AtomicBool>) -> Result<SettingsSnapshot> {
    anyhow::ensure!(
        !cancel.load(Ordering::Acquire),
        "VS Code settings capture was cancelled"
    );
    let path = default_settings_path()?;
    capture_settings_at(&path, cancel)
}

pub fn save_settings_profile(profile: &SettingsProfile, path: &Path) -> Result<()> {
    anyhow::ensure!(path.is_absolute(), "Settings profile path must be absolute");
    profile.validate()?;
    anyhow::ensure!(
        serde_json::to_vec(profile)?.len() <= MAX_PROFILE_BYTES,
        "VS Code settings profile exceeds the size limit"
    );
    studio_core::export_document(profile, path)
}

pub fn load_settings_profile(path: &Path) -> Result<SettingsProfile> {
    anyhow::ensure!(path.is_absolute(), "Settings profile path must be absolute");
    let metadata = fs::symlink_metadata(path).context("Inspect VS Code settings profile")?;
    anyhow::ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "VS Code settings profile must be a regular file"
    );
    anyhow::ensure!(
        metadata.len() <= MAX_PROFILE_BYTES as u64,
        "VS Code settings profile exceeds the size limit"
    );
    let bytes = fs::read(path).context("Read VS Code settings profile")?;
    anyhow::ensure!(
        bytes.len() <= MAX_PROFILE_BYTES,
        "VS Code settings profile exceeds the size limit"
    );
    reject_duplicate_profile_keys(&bytes)?;
    let profile: SettingsProfile =
        serde_json::from_slice(&bytes).context("Parse VS Code settings profile")?;
    profile.validate()?;
    Ok(profile)
}

pub fn apply_settings(
    profile: &SettingsProfile,
    selected: &BTreeSet<String>,
    expected_current: &SettingsSnapshot,
    cancel: &Arc<AtomicBool>,
) -> Result<SettingsApplyOutcome> {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        let _guard = APPLY_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("VS Code settings apply lock was poisoned"))?;
        apply_settings_with(
            profile,
            selected,
            expected_current,
            &studio_platform::data_directory()?,
            cancel,
        )
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = (profile, selected, expected_current, cancel);
        anyhow::bail!("VS Code settings profiles are supported on macOS, Windows and Linux only")
    }
}

fn default_settings_path() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME").context("HOME is unavailable")?;
        let home = PathBuf::from(home);
        anyhow::ensure!(home.is_absolute(), "HOME must be an absolute path");
        Ok(home.join("Library/Application Support/Code/User/settings.json"))
    }
    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var_os("APPDATA").context("APPDATA is unavailable")?;
        let appdata = PathBuf::from(appdata);
        anyhow::ensure!(appdata.is_absolute(), "APPDATA must be an absolute path");
        Ok(appdata.join("Code").join("User").join("settings.json"))
    }
    #[cfg(target_os = "linux")]
    {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .context("XDG_CONFIG_HOME and HOME are unavailable")?;
        anyhow::ensure!(
            config.is_absolute(),
            "VS Code configuration directory must be absolute"
        );
        Ok(config.join("Code").join("User").join("settings.json"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        anyhow::bail!("VS Code settings profiles are supported on macOS, Windows and Linux only")
    }
}

fn capture_settings_at(path: &Path, cancel: &AtomicBool) -> Result<SettingsSnapshot> {
    anyhow::ensure!(path.is_absolute(), "VS Code settings path must be absolute");
    anyhow::ensure!(
        !cancel.load(Ordering::Acquire),
        "VS Code settings capture was cancelled"
    );
    let original = read_settings_file(path)?;
    let settings = match original.as_deref() {
        Some(bytes) => parse_settings(bytes)?.1,
        None => BTreeMap::new(),
    };
    anyhow::ensure!(
        !cancel.load(Ordering::Acquire),
        "VS Code settings capture was cancelled"
    );
    Ok(SettingsSnapshot {
        path: path.to_owned(),
        original,
        settings,
    })
}

fn read_settings_file(path: &Path) -> Result<Option<Vec<u8>>> {
    anyhow::ensure!(path.is_absolute(), "VS Code settings path must be absolute");
    let parent = path
        .parent()
        .context("VS Code settings path has no parent directory")?;
    let parent_metadata =
        fs::symlink_metadata(parent).context("Inspect VS Code user settings directory")?;
    anyhow::ensure!(
        parent_metadata.is_dir() && !parent_metadata.file_type().is_symlink(),
        "VS Code user settings directory must be a real directory"
    );
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("Inspect VS Code user settings file"),
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "VS Code user settings must be a regular file"
            );
            anyhow::ensure!(
                metadata.len() <= MAX_SETTINGS_BYTES as u64,
                "VS Code user settings exceed the size limit"
            );
            let bytes = fs::read(path).context("Read VS Code user settings")?;
            anyhow::ensure!(
                bytes.len() <= MAX_SETTINGS_BYTES,
                "VS Code user settings exceed the size limit"
            );
            Ok(Some(bytes))
        }
    }
}

fn parse_settings(bytes: &[u8]) -> Result<(Value, BTreeMap<String, Value>)> {
    anyhow::ensure!(
        bytes.len() <= MAX_SETTINGS_BYTES,
        "VS Code user settings exceed the size limit"
    );
    let text = std::str::from_utf8(bytes).context("VS Code user settings are not UTF-8")?;
    let options = settings_parse_options();
    let document: Value = jsonc_parser::parse_to_serde_value(text, &options)
        .context("Parse VS Code JSONC user settings")?;
    anyhow::ensure!(
        document.is_object(),
        "VS Code user settings must contain a JSON object"
    );
    let root =
        CstRootNode::parse(text, &options).context("Parse VS Code settings for safe merge")?;
    let object = root
        .value()
        .and_then(|node| node.as_object())
        .context("VS Code user settings must contain a JSON object")?;
    reject_duplicate_allowed_keys(&object)?;
    let settings = allowed_settings(&document)?;
    Ok((document, settings))
}

fn allowed_settings(document: &Value) -> Result<BTreeMap<String, Value>> {
    let values = document
        .as_object()
        .context("VS Code user settings must contain a JSON object")?;
    let mut settings = BTreeMap::new();
    for key in ALLOWED_SETTINGS {
        if let Some(value) = values.get(*key) {
            validate_value(key, value)?;
            settings.insert((*key).to_owned(), value.clone());
        }
    }
    Ok(settings)
}

fn reject_duplicate_allowed_keys(object: &CstObject) -> Result<()> {
    let mut seen = BTreeSet::new();
    for property in object.properties() {
        let key = property
            .decoded_name()
            .context("VS Code settings contains an unreadable property name")?;
        if ALLOWED_SETTINGS.contains(&key.as_str()) {
            anyhow::ensure!(
                seen.insert(key.clone()),
                "VS Code settings contains duplicate supported key {key}"
            );
        }
    }
    Ok(())
}

fn reject_duplicate_profile_keys(bytes: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(bytes).context("VS Code settings profile is not UTF-8")?;
    let root = CstRootNode::parse(text, &strict_json_options())
        .context("VS Code settings profile must be strict JSON")?;
    let document = root
        .value()
        .and_then(|node| node.as_object())
        .context("VS Code settings profile must contain a JSON object")?;
    let settings = document
        .get("settings")
        .and_then(|property| property.value())
        .and_then(|node| node.as_object())
        .context("VS Code settings profile must contain a settings object")?;
    let mut seen = BTreeSet::new();
    for property in settings.properties() {
        let key = property
            .decoded_name()
            .context("VS Code settings profile contains an unreadable key")?;
        anyhow::ensure!(
            seen.insert(key.clone()),
            "VS Code settings profile contains duplicate key {key}"
        );
    }
    Ok(())
}

fn settings_parse_options() -> jsonc_parser::ParseOptions {
    jsonc_parser::ParseOptions {
        allow_comments: true,
        allow_loose_object_property_names: false,
        allow_trailing_commas: true,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
        allow_bare_decimal_point_numbers: false,
        allow_non_finite_numbers: false,
        allow_extended_string_escapes: false,
    }
}

fn strict_json_options() -> jsonc_parser::ParseOptions {
    jsonc_parser::ParseOptions {
        allow_comments: false,
        allow_loose_object_property_names: false,
        allow_trailing_commas: false,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
        allow_bare_decimal_point_numbers: false,
        allow_non_finite_numbers: false,
        allow_extended_string_escapes: false,
    }
}

fn validate_value(key: &str, value: &Value) -> Result<()> {
    anyhow::ensure!(
        ALLOWED_SETTINGS.contains(&key),
        "Unsupported VS Code setting {key}"
    );
    match key {
        "editor.fontFamily" | "terminal.integrated.fontFamily" => validate_text(value, 256, true),
        "workbench.colorTheme" => validate_text(value, 256, false),
        "workbench.iconTheme" if value.is_null() => Ok(()),
        "workbench.iconTheme" => validate_text(value, 256, true),
        "editor.fontWeight" => {
            let text = value
                .as_str()
                .context("editor.fontWeight must be a string")?;
            anyhow::ensure!(
                matches!(
                    text,
                    "normal"
                        | "bold"
                        | "100"
                        | "200"
                        | "300"
                        | "400"
                        | "500"
                        | "600"
                        | "700"
                        | "800"
                        | "900"
                ),
                "editor.fontWeight has an unsupported value"
            );
            Ok(())
        }
        "editor.wordWrap" => validate_enum(value, &["off", "on", "wordWrapColumn", "bounded"]),
        "editor.lineNumbers" => validate_enum(value, &["off", "on", "relative", "interval"]),
        "editor.renderWhitespace" => {
            validate_enum(value, &["none", "boundary", "selection", "trailing", "all"])
        }
        "files.autoSave" => validate_enum(
            value,
            &["off", "afterDelay", "onFocusChange", "onWindowChange"],
        ),
        "files.eol" => {
            let text = value.as_str().context("files.eol must be a string")?;
            anyhow::ensure!(
                matches!(text, "auto" | "\n" | "\r\n"),
                "files.eol has an unsupported value"
            );
            Ok(())
        }
        "editor.fontSize" => validate_integer(value, 6, 100),
        "editor.tabSize" => validate_integer(value, 1, 16),
        "files.autoSaveDelay" => validate_integer(value, 0, 60_000),
        "editor.lineHeight" => validate_number(value, 0.0, 200.0),
        "terminal.integrated.fontSize" => validate_number(value, 6.0, 100.0),
        "window.zoomLevel" => validate_number(value, -8.0, 8.0),
        "editor.formatOnSave"
        | "editor.insertSpaces"
        | "editor.minimap.enabled"
        | "files.insertFinalNewline"
        | "files.trimTrailingWhitespace" => {
            anyhow::ensure!(value.is_boolean(), "{key} must be a boolean");
            Ok(())
        }
        _ => anyhow::bail!("Unsupported VS Code setting {key}"),
    }
}

fn validate_text(value: &Value, maximum: usize, allow_empty: bool) -> Result<()> {
    let text = value.as_str().context("VS Code setting must be a string")?;
    anyhow::ensure!(
        (allow_empty || !text.is_empty())
            && text.len() <= maximum
            && !text.chars().any(char::is_control),
        "VS Code setting string is empty, too long or contains control characters"
    );
    Ok(())
}

fn validate_enum(value: &Value, options: &[&str]) -> Result<()> {
    let text = value.as_str().context("VS Code setting must be a string")?;
    anyhow::ensure!(
        options.contains(&text),
        "VS Code setting has an unsupported value"
    );
    Ok(())
}

fn validate_integer(value: &Value, minimum: u64, maximum: u64) -> Result<()> {
    let number = value
        .as_u64()
        .context("VS Code setting must be an unsigned integer")?;
    anyhow::ensure!(
        (minimum..=maximum).contains(&number),
        "VS Code setting is outside its supported range"
    );
    Ok(())
}

fn validate_number(value: &Value, minimum: f64, maximum: f64) -> Result<()> {
    let number = value.as_f64().context("VS Code setting must be a number")?;
    anyhow::ensure!(
        number.is_finite() && (minimum..=maximum).contains(&number),
        "VS Code setting is outside its supported range"
    );
    Ok(())
}

fn render_merged_settings(
    original: Option<&[u8]>,
    profile: &SettingsProfile,
    selected: &BTreeSet<String>,
) -> Result<Vec<u8>> {
    profile.validate()?;
    let text = original
        .map(std::str::from_utf8)
        .transpose()
        .context("VS Code user settings are not UTF-8")?
        .unwrap_or("{}\n");
    let (document, root, object) = parse_settings_text(text)?;
    let _ = allowed_settings(&document)?;
    for key in selected {
        anyhow::ensure!(
            ALLOWED_SETTINGS.contains(&key.as_str()),
            "Selected VS Code setting is unsupported"
        );
        let value = profile
            .settings
            .get(key)
            .with_context(|| format!("Selected VS Code setting {key} has no profile value"))?;
        let replacement = cst_value(value)?;
        if let Some(property) = object.get(key) {
            property.set_value(replacement);
        } else {
            object.append(key, replacement);
        }
    }
    let output = root.to_string().into_bytes();
    anyhow::ensure!(
        output.len() <= MAX_SETTINGS_BYTES,
        "Merged VS Code settings exceed the size limit"
    );
    Ok(output)
}

fn parse_settings_text(text: &str) -> Result<(Value, CstRootNode, CstObject)> {
    anyhow::ensure!(
        text.len() <= MAX_SETTINGS_BYTES,
        "VS Code user settings exceed the size limit"
    );
    let options = settings_parse_options();
    let document: Value = jsonc_parser::parse_to_serde_value(text, &options)
        .context("Parse VS Code JSONC user settings")?;
    anyhow::ensure!(
        document.is_object(),
        "VS Code user settings must contain a JSON object"
    );
    let root =
        CstRootNode::parse(text, &options).context("Parse VS Code settings for safe merge")?;
    let object = root
        .value()
        .and_then(|node| node.as_object())
        .context("VS Code user settings must contain a JSON object")?;
    reject_duplicate_allowed_keys(&object)?;
    Ok((document, root, object))
}

fn cst_value(value: &Value) -> Result<CstInputValue> {
    match value {
        Value::Null => Ok(CstInputValue::Null),
        Value::Bool(value) => Ok(CstInputValue::Bool(*value)),
        Value::Number(value) => Ok(CstInputValue::Number(value.to_string())),
        Value::String(value) => Ok(CstInputValue::String(value.clone())),
        Value::Array(_) | Value::Object(_) => {
            anyhow::bail!("VS Code settings profiles accept scalar values only")
        }
    }
}

fn apply_settings_with(
    profile: &SettingsProfile,
    selected: &BTreeSet<String>,
    expected_current: &SettingsSnapshot,
    data_directory: &Path,
    cancel: &AtomicBool,
) -> Result<SettingsApplyOutcome> {
    profile.validate()?;
    anyhow::ensure!(!selected.is_empty(), "Select at least one VS Code setting");
    for key in selected {
        anyhow::ensure!(
            profile.settings.contains_key(key),
            "Selected VS Code setting {key} has no profile value"
        );
        anyhow::ensure!(
            ALLOWED_SETTINGS.contains(&key.as_str()),
            "Selected VS Code setting {key} is unsupported"
        );
    }
    anyhow::ensure!(
        !cancel.load(Ordering::Acquire),
        "VS Code settings apply was cancelled before review"
    );

    let current = capture_settings_at(&expected_current.path, cancel)?;
    for key in selected {
        anyhow::ensure!(
            current.settings.get(key) == expected_current.settings.get(key),
            "VS Code setting {key} changed after review; refresh the comparison"
        );
    }
    let before = SettingsProfile {
        schema_version: SCHEMA_VERSION,
        settings: current
            .settings
            .iter()
            .filter(|(key, _)| selected.contains(*key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    };
    let requested = SettingsProfile {
        schema_version: SCHEMA_VERSION,
        settings: selected
            .iter()
            .map(|key| (key.clone(), profile.settings[key].clone()))
            .collect(),
    };
    let planned = render_merged_settings(current.original.as_deref(), &requested, selected)?;
    let history = crate::environment::create_history(data_directory)?;
    studio_core::export_document(&before, &history.join("before.json"))?;
    studio_core::export_document(&requested, &history.join("request.json"))?;

    let mut operation_error = None;
    if cancel.load(Ordering::Acquire) {
        operation_error = Some("VS Code settings apply was cancelled before commit".to_owned());
    } else if before.settings != requested.settings {
        match read_settings_file(&current.path) {
            Ok(bytes) if bytes == current.original => {
                if let Err(error) = studio_core::write_atomic(&current.path, &planned) {
                    operation_error = Some(format!(
                        "Could not atomically write VS Code settings: {error}"
                    ));
                }
            }
            Ok(_) => {
                operation_error = Some(
                    "VS Code settings changed while preparing the merge; refresh and review again"
                        .to_owned(),
                );
            }
            Err(error) => {
                operation_error = Some(format!(
                    "Could not recheck VS Code settings before commit: {error:#}"
                ));
            }
        }
    }

    let never_cancel = AtomicBool::new(false);
    let after_result = capture_settings_at(&current.path, &never_cancel);
    let (after, after_values) = match after_result {
        Ok(snapshot) => {
            let selected_values = snapshot
                .settings
                .iter()
                .filter(|(key, _)| selected.contains(*key))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            (Some(snapshot), Some(selected_values))
        }
        Err(error) => {
            operation_error
                .get_or_insert_with(|| format!("VS Code settings readback failed: {error:#}"));
            (None, None)
        }
    };
    let verified = operation_error.is_none() && after_values.as_ref() == Some(&requested.settings);
    let cancelled = cancel.load(Ordering::Acquire);
    let changed = after_values.as_ref().map_or_else(Vec::new, |values| {
        selected
            .iter()
            .filter(|key| before.settings.get(*key) != values.get(*key))
            .cloned()
            .collect()
    });
    let mut detail = if verified {
        "All selected VS Code settings match the requested values.".to_owned()
    } else {
        operation_error
            .unwrap_or_else(|| "VS Code settings readback did not match the request.".to_owned())
    };
    if verified && cancelled {
        detail.push_str(" Cancellation arrived after commit; the completed write was not undone.");
    }

    let mut history_complete = true;
    if let Some(after) = &after {
        let after_profile = SettingsProfile {
            schema_version: SCHEMA_VERSION,
            settings: after
                .settings
                .iter()
                .filter(|(key, _)| selected.contains(*key))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        };
        if let Err(error) =
            studio_core::export_document(&after_profile, &history.join("after.json"))
        {
            history_complete = false;
            detail.push_str(&format!(
                " Could not save the post-apply history: {error:#}."
            ));
        }
    } else {
        history_complete = false;
    }
    let mut report = SettingsApplyReport {
        schema_version: SCHEMA_VERSION,
        selected: selected.iter().cloned().collect(),
        changed,
        verified,
        history_complete,
        cancelled,
        detail,
        history: history.clone(),
    };
    if let Err(error) = studio_core::export_document(&report, &history.join("result.json")) {
        report.history_complete = false;
        report
            .detail
            .push_str(&format!(" Could not save the result history: {error:#}."));
    }
    Ok(SettingsApplyOutcome { report, after })
}

#[cfg(test)]
mod settings_tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::AtomicU64;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "devenv-vscode-settings-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn profile(settings: &[(&str, Value)]) -> SettingsProfile {
        SettingsProfile {
            schema_version: SCHEMA_VERSION,
            settings: settings
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
        }
    }

    #[test]
    fn accepted_defaults_and_theme_reset_values_roundtrip() {
        let settings = profile(&[
            ("editor.fontFamily", json!("")),
            ("terminal.integrated.fontFamily", json!("")),
            ("workbench.iconTheme", Value::Null),
            ("window.zoomLevel", json!(8.0)),
        ]);
        assert!(settings.validate().is_ok());
        assert!(validate_value("files.eol", &json!("\n")).is_ok());
        assert!(validate_value("files.eol", &json!("\r\n")).is_ok());
        assert!(validate_value("files.eol", &json!("auto")).is_ok());
        assert!(validate_value("files.eol", &json!(r#"\n"#)).is_err());
        let unsupported_zoom = profile(&[("window.zoomLevel", json!(8.1))]);
        assert!(unsupported_zoom.validate().is_err());
    }

    #[test]
    fn jsonc_merge_keeps_comments_and_unselected_settings() {
        let input = br#"{
  // keep this note
  "editor.fontSize": 12,
  "editor.tabSize": 2,
  "terminal.integrated.env.osx": { "TOKEN": "private-value" },
}"#;
        let requested = profile(&[("editor.fontSize", json!(16))]);
        let selected = BTreeSet::from(["editor.fontSize".to_owned()]);
        let output = render_merged_settings(Some(input), &requested, &selected).unwrap();
        let text = std::str::from_utf8(&output).unwrap();
        assert!(text.contains("// keep this note"));
        assert!(text.contains(r#""editor.tabSize": 2"#));
        assert!(text.contains(r#""TOKEN": "private-value""#));
        let (document, _, _) = parse_settings_text(text).unwrap();
        assert_eq!(document["editor.fontSize"], json!(16));
    }

    #[test]
    fn duplicate_supported_keys_and_invalid_values_are_rejected() {
        let duplicate = br#"{"editor.fontSize": 12, "editor.fontSize": 13}"#;
        assert!(parse_settings(duplicate).is_err());
        let invalid = profile(&[("editor.fontSize", json!(1000))]);
        assert!(invalid.validate().is_err());
        let unsupported = profile(&[("terminal.integrated.shell.osx", json!("/bin/zsh"))]);
        assert!(unsupported.validate().is_err());
    }

    #[test]
    fn applying_keeps_concurrent_unselected_edits_and_never_journals_unknown_values() {
        let temp = TestDirectory::new();
        let user = temp.0.join("Code/User");
        fs::create_dir_all(&user).unwrap();
        let settings = user.join("settings.json");
        let original = r#"{
  // preserve comment
  "editor.fontSize": 12,
  "terminal.integrated.env.osx": { "TOKEN": "do-not-journal" }
}"#;
        fs::write(&settings, original).unwrap();
        let cancel = AtomicBool::new(false);
        let reviewed = capture_settings_at(&settings, &cancel).unwrap();
        fs::write(
            &settings,
            r#"{
  // preserve comment
  "editor.fontSize": 12,
  "files.trimTrailingWhitespace": true,
  "terminal.integrated.env.osx": { "TOKEN": "do-not-journal" }
}"#,
        )
        .unwrap();
        let requested = profile(&[("editor.fontSize", json!(16))]);
        let selected = BTreeSet::from(["editor.fontSize".to_owned()]);
        let data = temp.0.join("app-data");
        let outcome =
            apply_settings_with(&requested, &selected, &reviewed, &data, &cancel).unwrap();
        assert!(outcome.report.verified, "{}", outcome.report.detail);
        assert!(outcome.report.history_complete, "{}", outcome.report.detail);
        let written = fs::read_to_string(&settings).unwrap();
        assert!(written.contains("// preserve comment"));
        assert!(written.contains(r#""files.trimTrailingWhitespace": true"#));
        assert!(written.contains(r#""TOKEN": "do-not-journal""#));
        assert_eq!(
            outcome.after.unwrap().settings["editor.fontSize"],
            json!(16)
        );
        for entry in fs::read_dir(&outcome.report.history).unwrap() {
            let entry = entry.unwrap();
            let bytes = fs::read(entry.path()).unwrap();
            assert!(
                !bytes
                    .windows(b"do-not-journal".len())
                    .any(|window| window == b"do-not-journal")
            );
        }
    }

    #[test]
    fn selected_external_changes_stop_before_journaling_or_writing() {
        let temp = TestDirectory::new();
        let user = temp.0.join("Code/User");
        fs::create_dir_all(&user).unwrap();
        let settings = user.join("settings.json");
        fs::write(&settings, r#"{"editor.fontSize": 12}"#).unwrap();
        let cancel = AtomicBool::new(false);
        let reviewed = capture_settings_at(&settings, &cancel).unwrap();
        fs::write(&settings, r#"{"editor.fontSize": 14}"#).unwrap();
        let requested = profile(&[("editor.fontSize", json!(16))]);
        let selected = BTreeSet::from(["editor.fontSize".to_owned()]);
        let data = temp.0.join("app-data");
        assert!(apply_settings_with(&requested, &selected, &reviewed, &data, &cancel).is_err());
        assert_eq!(
            fs::read_to_string(settings).unwrap(),
            r#"{"editor.fontSize": 14}"#
        );
        assert!(!data.exists());
    }

    #[test]
    fn profile_roundtrip_is_bounded_strict_and_no_clobber() {
        let temp = TestDirectory::new();
        let path = temp.0.join("vscode-settings.json");
        let expected = profile(&[
            ("editor.fontSize", json!(14)),
            ("editor.insertSpaces", json!(true)),
        ]);
        save_settings_profile(&expected, &path).unwrap();
        assert_eq!(load_settings_profile(&path).unwrap(), expected);
        assert!(save_settings_profile(&expected, &path).is_err());
        fs::write(
            &path,
            r#"{"schema_version":1,"settings":{"editor.fontSize":12,"editor.fontSize":16}}"#,
        )
        .unwrap();
        assert!(load_settings_profile(&path).is_err());
    }
}
