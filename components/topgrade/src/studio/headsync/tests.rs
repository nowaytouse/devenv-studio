use super::{Headsync, names, outdated_names};
use std::{env, path::Path, process::Command};

#[test]
fn formula_names_are_exact_and_cannot_be_options() {
    let qualified = "nyamiiko/headfixes/libsndfile";
    assert_eq!(
        names(&format!("llvm@23\n{qualified}\n")).unwrap(),
        ["llvm@23", qualified]
    );
    for invalid in ["--formula", "bad formula", "foo//bar", "foo/../bar", "llvm\nllvm"] {
        assert!(names(invalid).is_err(), "{invalid}");
    }
    let output = format!(r#"{{"formulae":[{{"name":"{qualified}","pinned":false}}],"casks":[]}}"#);
    assert_eq!(
        outdated_names(output.as_bytes(), true, &[qualified]).unwrap(),
        [qualified]
    );
    assert!(outdated_names(output.as_bytes(), true, &["libsndfile"]).is_err());
    assert!(outdated_names(output.as_bytes(), false, &[qualified]).is_err());
}

#[test]
fn fixture_child() {
    if env::var_os("TEST_HEADSYNC_NATIVE_CHILD").as_deref() != Some("1".as_ref()) {
        return;
    }
    let root = std::path::PathBuf::from(env::var_os("TEST_ROOT").unwrap());
    assert!(root.join("bin/brew").is_file() && root.join("core").is_dir());
    Headsync {
        brew: root.join("bin/brew"),
        state: root.join("state/brew-headsync"),
        git_config_index: None,
    }
    .run()
    .unwrap_or_else(|error| panic!("{error:#}"));
}

#[test]
#[ignore = "requires a Python interpreter and macOS command-line tools for the legacy parity fixtures"]
#[expect(
    clippy::disallowed_methods,
    reason = "Only this opt-in test launches the mock Homebrew fixture"
)]
fn legacy_fixture_parity() {
    let python = env::var_os("TOPGRADE_FIXTURE_PYTHON").unwrap_or_else(|| "python3.15".into());
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("profiles/macos/tests/headsync_cases.py");
    let output = Command::new(python)
        .arg(path)
        .arg("--native")
        .arg(env::current_exe().unwrap())
        .args(["--exact", "studio::headsync::tests::fixture_child", "--nocapture"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.lines().filter(|line| line.starts_with("PASS ")).count(),
        51,
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
