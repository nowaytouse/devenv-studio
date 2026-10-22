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
        "uv" => {
            ends_alphanumeric(value)
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
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
    fn exact_identifiers_accept_known_packages_and_reject_arguments_and_urls() {
        for (kind, id) in [
            ("cargo", "ripgrep"),
            ("cargo", "fd-find"),
            ("uv", "pre-commit"),
            ("uv", "ruff"),
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
            for kind in ["cargo", "uv", "vscode"] {
                assert!(!valid(kind, id), "{kind}: {id}");
            }
        }
        assert!(!valid("vscode", "publisher.extension.extra"));
        assert!(!valid("vscode", "publisher.-extension"));
        assert!(!valid("cargo", "Cargo-Name"));
    }
}
