//! Identifiers only: no versions, URLs, paths, options or requirement specifiers.
fn bounded(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128
}
fn ends_alphanumeric(value: &str) -> bool {
    value
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
}
/// A bare distribution identity, never a requirement, URL, path or option.
pub fn normalize_pip_id(value: &str) -> Option<String> {
    if !bounded(value)
        || !ends_alphanumeric(value)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
    {
        return None;
    }
    let mut result = String::with_capacity(value.len());
    let mut separator = false;
    for byte in value.bytes() {
        if b"-_.".contains(&byte) {
            if !separator {
                result.push('-');
            }
            separator = true;
        } else {
            result.push(char::from(byte.to_ascii_lowercase()));
            separator = false;
        }
    }
    Some(result)
}
pub(crate) fn valid(kind: &str, value: &str) -> bool {
    if !bounded(value) {
        return false;
    }
    match kind {
        "cargo" => {
            value.as_bytes()[0].is_ascii_lowercase()
                && value
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_".contains(&b))
        }
        "pip" => normalize_pip_id(value).as_deref() == Some(value),
        "uv" => {
            ends_alphanumeric(value)
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        }
        "npm" => {
            let component = |part: &str| {
                ends_alphanumeric(part)
                    && part.bytes().all(|b| {
                        b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_.".contains(&b)
                    })
            };
            if let Some(scoped) = value.strip_prefix('@') {
                scoped
                    .split_once('/')
                    .is_some_and(|(scope, name)| component(scope) && component(name))
            } else {
                component(value) && !matches!(value, "node_modules" | "favicon.ico")
            }
        }
        "vscode" => {
            let Some((publisher, name)) = value.split_once('.') else {
                return false;
            };
            [publisher, name].iter().all(|s| {
                ends_alphanumeric(s) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        }
        _ => false,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pip_names_are_bare_and_canonicalize_every_separator_run() {
        assert_eq!(
            normalize_pip_id("PyWavelets").as_deref(),
            Some("pywavelets")
        );
        assert_eq!(
            normalize_pip_id("friendly-._..Bard").as_deref(),
            Some("friendly-bard")
        );
        assert!(valid("pip", "opencv-python"));
        assert!(!valid("pip", "OpenCV_Python"));
        for name in [
            "",
            "../pkg",
            "name==1",
            "name[extra]",
            "-x",
            "foo ",
            "foo\n",
            "https://example.com",
            "éclair",
            "pkg;exit",
            "foo-",
        ] {
            assert!(normalize_pip_id(name).is_none(), "{name}");
        }
    }
    #[test]
    fn exact_identifiers_accept_known_packages_and_reject_arguments_and_urls() {
        for (kind, id) in [
            ("cargo", "ripgrep"),
            ("cargo", "fd-find"),
            ("uv", "pre-commit"),
            ("uv", "ruff"),
            ("npm", "typescript-language-server"),
            ("npm", "@scope/tool"),
            ("vscode", "rust-lang.rust-analyzer"),
            ("vscode", "ms-python.python"),
        ] {
            assert!(valid(kind, id));
        }
        for id in [
            "--help",
            "https://example.com/a",
            "../file",
            "foo@1.0",
            "foo==1",
            "foo;exit",
            "foo bar",
            "foo[extra]",
            "foo\\bar",
            "foo\0bar",
        ] {
            for kind in ["cargo", "uv", "vscode", "npm"] {
                assert!(!valid(kind, id), "{kind}: {id}");
            }
        }
        assert!(!valid("vscode", "publisher.extension.extra"));
        assert!(!valid("vscode", "publisher.-extension"));
        assert!(!valid("cargo", "Cargo-Name"));
        for id in [
            "@scope/tool@1",
            "@scope//tool",
            "@scope/../tool",
            "@scope",
            "@Scope/tool",
            "Name",
            ".name",
            "_name",
            "-name",
            "git+https://example.com/a",
            "npm:other",
            "@scope/tool;exit",
            "@scope/tool name",
        ] {
            assert!(!valid("npm", id), "{id}");
        }
    }
}
