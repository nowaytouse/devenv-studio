//! Application chrome translations. Canonical catalog values and diagnostics stay untouched.
use crate::prefs::Language;
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{cell::Cell, collections::BTreeMap, sync::LazyLock};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Message {
    source: String,
    english: String,
}

static MESSAGES: LazyLock<Result<BTreeMap<String, String>>> =
    LazyLock::new(|| read_messages(include_str!("../../../../data/locales/ui.json")));

fn read_messages(resource: &str) -> Result<BTreeMap<String, String>> {
    let entries: Vec<Message> =
        serde_json::from_str(resource).context("Invalid GUI translations")?;
    ensure!(!entries.is_empty(), "Empty GUI translations");
    let mut messages = BTreeMap::new();
    for Message { source, english } in entries {
        ensure!(
            !source.trim().is_empty() && !english.trim().is_empty(),
            "Blank GUI translation"
        );
        ensure!(
            placeholders(&source) == placeholders(&english),
            "GUI translation placeholders differ for {source}"
        );
        ensure!(
            messages.insert(source, english).is_none(),
            "Duplicate GUI translation source"
        );
    }
    Ok(messages)
}

fn placeholders(message: &str) -> Vec<&str> {
    let mut fields: Vec<_> = message
        .split('{')
        .skip(1)
        .filter_map(|part| part.split_once('}').map(|(field, _)| field))
        .collect();
    fields.sort_unstable();
    fields
}

thread_local! { static ENGLISH: Cell<bool> = const { Cell::new(false) }; }
pub(super) fn set_language(language: Language) {
    ENGLISH.with(|value| value.set(matches!(language, Language::EnUs)));
}
pub(super) fn english() -> bool {
    ENGLISH.with(Cell::get)
}
pub(super) fn language() -> Language {
    if english() {
        Language::EnUs
    } else {
        Language::ZhCn
    }
}
pub(super) fn tx<'a>(zh: &'a str, en: &'a str) -> &'a str {
    if english() { en } else { zh }
}

pub(super) fn t(source: &str) -> &str {
    if !english() {
        return source;
    }
    MESSAGES
        .as_ref()
        .ok()
        .and_then(|messages| messages.get(source))
        .map(String::as_str)
        .unwrap_or(source)
}

/// Both literals remain visible to review; arguments are evaluated only in the active branch.
macro_rules! lf {
    ($zh:literal, $en:literal, $($args:tt)*) => {
        if $crate::gui::i18n::english() { format!($en, $($args)*) } else { format!($zh, $($args)*) }
    };
    ($zh:literal, $en:literal) => {
        if $crate::gui::i18n::english() { format!($en) } else { format!($zh) }
    };
}
pub(super) use lf;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_catalog_preserves_translation_and_source_fallback() {
        assert!(MESSAGES.is_ok());
        set_language(Language::EnUs);
        assert_eq!(t("设置"), "Settings");
        assert_eq!(t("unknown message"), "unknown message");
        set_language(Language::ZhCn);
        assert_eq!(t("设置"), "设置");
    }

    #[test]
    fn rejects_duplicate_sources_and_changed_format_fields() {
        assert!(
            read_messages(r#"[{"source":"a","english":"b"},{"source":"a","english":"c"}]"#)
                .is_err()
        );
        assert!(read_messages(r#"[{"source":"值：{count}","english":"Value: {}"}]"#).is_err());
    }
}
