use super::{Output, Summary};
use color_eyre::eyre::Result;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn update(home: &Path, run: &mut impl FnMut(&[&str]) -> Result<Output>) -> Result<Summary> {
    super::update(home, run, &mut |_, _| panic!("Unexpected skill migration"))
}

const OLD: &str = "1111111111111111111111111111111111111111";
const NEW: &str = "2222222222222222222222222222222222222222";
const ROOT: &str = "3333333333333333333333333333333333333333";
const ANCESTOR: &str = "4444444444444444444444444444444444444444";
const COMMIT: &str = "5555555555555555555555555555555555555555";
const TAG: &str = "6666666666666666666666666666666666666666";
const SOURCE: &str = "https://github.com/demo/repo";

struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    github: PathBuf,
    rows: Vec<Value>,
}

impl Fixture {
    fn new(tree: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let github = home.join(".cursor/skills/demo");
        skill(&github, "demo", Some(tree));
        let rows = vec![row(&github, "publisher/demo", SOURCE)];
        Self {
            _temp: temp,
            home,
            github,
            rows,
        }
    }

    fn lock(&self, value: &Value) -> PathBuf {
        let path = self.home.join(".agents/.skill-lock.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
        path
    }
}

fn row(path: &Path, alias: &str, source: &str) -> Value {
    json!({"skillName": alias, "sourceURL": source, "path": path, "pinned": false, "scope": "user"})
}

fn skill(path: &Path, name: &str, tree: Option<&str>) {
    fs::create_dir_all(path).unwrap();
    let metadata = tree.map_or_else(String::new, |tree| {
        format!("metadata:\n  github-repo: {SOURCE}\n  github-path: skills/demo\n  github-tree-sha: {tree}\n  github-ref: refs/tags/release/v1\n")
    });
    fs::write(
        path.join("SKILL.md"),
        format!("---\nname: {name}\n{metadata}---\nFixture body\n"),
    )
    .unwrap();
}

fn entry(hash: &str, skill_path: &str) -> Value {
    json!({"source":"demo/repo","sourceType":"github","sourceUrl":SOURCE,
        "skillPath":skill_path,"skillFolderHash":hash,"installedAt":"test","updatedAt":"test"})
}

fn output(text: String, code: i32) -> Output {
    Output {
        success: code == 0,
        code: Some(code),
        stdout: text,
        stderr: String::new(),
    }
}

fn http(value: &Value, status: u16, code: i32) -> Output {
    output(
        format!("HTTP/2.0 {status} Status\r\nContent-Type: application/json\r\n\r\n{value}"),
        code,
    )
}

fn tree(sha: &str, path: &str, node_type: &str, mode: &str, child: &str) -> Value {
    json!({"sha":sha,"truncated":false,"tree":[{"path":path,"type":node_type,"mode":mode,"sha":child}]})
}

fn fake_read(args: &[&str], rows: &[Value], expected: &str) -> Result<Output> {
    if args.first() == Some(&"skill") {
        assert_eq!(
            args,
            ["skill", "list", "--json", "skillName,sourceURL,path,pinned,scope"]
        );
        return Ok(output(serde_json::to_string(rows).unwrap(), 0));
    }
    assert_eq!(&args[..4], ["api", "--hostname", "github.com", "--include"]);
    let route = args[4].strip_prefix("repos/demo/repo").unwrap();
    let body = match route {
        "/releases/latest" => json!({"tag_name":"release/v2"}),
        "/git/ref/tags/release%2Fv2" => json!({"object":{"type":"tag","sha":TAG}}),
        _ if route == format!("/git/tags/{TAG}") => json!({"object":{"type":"commit","sha":COMMIT}}),
        _ if route == format!("/git/trees/{COMMIT}") => tree(ROOT, "skills", "tree", "040000", ANCESTOR),
        _ if route == format!("/git/trees/{ANCESTOR}") => tree(ANCESTOR, "demo", "tree", "040000", expected),
        _ if route == format!("/git/trees/{expected}") => tree(expected, "SKILL.md", "blob", "100644", OLD),
        _ => panic!("Unexpected GitHub route: {route}"),
    };
    Ok(http(&body, 200, 0))
}

#[test]
fn staged_update_preserves_shared_lock_and_locally_edited_skills() {
    for edited in [false, true] {
        let fixture = Fixture::new(OLD);
        let lock_path = fixture.lock(&json!({"version":3, "skills":{}, "lastSelectedAgents":["cursor"]}));
        let before = fs::read(&lock_path).unwrap();
        let live = fixture.github.join("SKILL.md");
        let pristine = fs::read_to_string(&live).unwrap();
        if edited {
            fs::write(&live, pristine.replace("Fixture body", "Local edit")).unwrap();
        }
        let mut staged = 0;
        let result = super::update(
            &fixture.home,
            &mut |args| fake_read(args, &fixture.rows, NEW),
            &mut |home, args| {
                assert_ne!(home, fixture.home);
                assert_eq!(&args[..3], ["skill", "install", "demo/repo"]);
                let revision = if staged == 0 {
                    assert_eq!(args[3], "skills/demo/SKILL.md@refs/tags/release/v1");
                    OLD
                } else {
                    assert_eq!(args[3], format!("skills/demo/SKILL.md@{COMMIT}"));
                    NEW
                };
                staged += 1;
                skill(&Path::new(args[5]).join("demo"), "demo", Some(revision));
                Ok(output(String::new(), 0))
            },
        );
        if edited {
            let error = result.unwrap_err();
            assert!(error.downcast_ref::<crate::error::PartialStep>().is_some());
            assert!(error.to_string().contains("kept local changes"));
            assert!(fs::read_to_string(&live).unwrap().contains("Local edit"));
            assert_eq!(staged, 1);
        } else {
            assert_eq!(result.unwrap().updated, 1);
            assert!(fs::read_to_string(&live).unwrap().contains(NEW));
            let state = super::super::state_dir(&fixture.home).unwrap();
            let backup = fs::read_dir(state).unwrap().next().unwrap().unwrap().path();
            assert_eq!(fs::read_to_string(backup.join("0-demo/SKILL.md")).unwrap(), pristine);
            assert_eq!(staged, 2);
        }
        assert_eq!(fs::read(lock_path).unwrap(), before);
    }
}

#[test]
fn verifies_native_change_and_delegates_known_owners() {
    let mut fixture = Fixture::new(OLD);
    let npx = fixture.home.join(".agents/skills/lean-ctx");
    let lean_copy = fixture.home.join(".copilot/skills/lean-ctx");
    for path in [&npx, &lean_copy] {
        skill(path, "lean-ctx", None);
        fs::write(path.join("SKILL.md.v4-migration.lock"), b"").unwrap();
    }
    fs::write(lean_copy.join("SKILL.md.bak"), b"local archive").unwrap();
    let shared = fixture.home.join(".agents/skills/automate");
    let native = fixture.home.join(".cursor/skills-cursor/automate");
    skill(&shared, "automate", None);
    skill(&native, "automate", None);
    fs::write(shared.join("settings.local.json"), b"{}").unwrap();
    fixture.rows.extend([
        row(&npx, "lean-ctx", ""),
        row(&lean_copy, "lean-ctx", ""),
        row(&shared, "automate", ""),
    ]);
    let baseline = json!({"version":3,"skills":{"lean-ctx":entry(OLD,"SKILL.md")},"dismissed":{"old":true}});
    fixture.lock(&baseline);
    let mut mutations = 0;
    let result = update(&fixture.home, &mut |args| {
        if args.starts_with(&["skill", "update"]) {
            assert_eq!(
                args,
                [
                    "skill",
                    "update",
                    "--dir",
                    fixture.github.parent().unwrap().to_str().unwrap(),
                    "--all",
                    "demo"
                ]
            );
            mutations += 1;
            skill(&fixture.github, "demo", Some(NEW));
            let mut lock = baseline.clone();
            lock["skills"]["publisher/demo"] = entry(NEW, "skills/demo/SKILL.md");
            fixture.lock(&lock);
            return Ok(output("Updated publisher/demo\n".into(), 0));
        }
        fake_read(args, &fixture.rows, NEW)
    })
    .unwrap();
    assert_eq!(mutations, 1);
    assert_eq!(
        result,
        Summary {
            github: 1,
            npx: 2,
            cursor: 1,
            local: 0,
            updated: 1
        }
    );
}

#[test]
fn rejected_lock_schema_and_post_update_clobber_are_failures() {
    let fixture = Fixture::new(OLD);
    for raw in [
        r#"{"version":3,"skills":{},"lastSelectedAgents":null}"#,
        r#"{"version":3,"skills":{},"unknown":true}"#,
        r#"{"version":3,"skills":{},"dismissed":{"repeat":true,"repeat":false}}"#,
    ] {
        let path = fixture.lock(&json!({"version":3,"skills":{}}));
        fs::write(path, raw).unwrap();
        let error = update(&fixture.home, &mut |args| {
            assert_eq!(args[0], "skill");
            fake_read(args, &fixture.rows, NEW)
        })
        .unwrap_err();
        assert!(error.to_string().contains("cannot safely round-trip"));
    }
    fixture.lock(&json!({"version":3,"skills":{"unrelated":entry(OLD,"SKILL.md")}}));
    let error = update(&fixture.home, &mut |args| {
        if args.starts_with(&["skill", "update"]) {
            fixture.lock(&json!({"version":3,"skills":{"demo":entry(NEW,"skills/demo/SKILL.md")}}));
            skill(&fixture.github, "demo", Some(NEW));
            return Ok(output("Updated demo\n".into(), 0));
        }
        fake_read(args, &fixture.rows, NEW)
    })
    .unwrap_err();
    assert!(format!("{error:#}").contains("clobbered an unrelated"));
}

#[test]
fn local_ownership_drift_stops_before_network_or_update() {
    let mut fixture = Fixture::new(OLD);
    let shared = fixture.home.join(".agents/skills/automate");
    let native = fixture.home.join(".cursor/skills-cursor/automate");
    skill(&shared, "automate", None);
    skill(&native, "automate", None);
    std::os::unix::fs::symlink(native.join("SKILL.md"), shared.join("bad-link")).unwrap();
    fixture.rows.push(row(&shared, "automate", ""));
    assert!(
        update(&fixture.home, &mut |args| {
            assert!(args.starts_with(&["skill", "list"]));
            fake_read(args, &fixture.rows, OLD)
        })
        .is_err()
    );

    fixture.rows.pop();
    let path = fixture.github.join("SKILL.md");
    let header = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        header.replace("  github-path:", "  github-path: duplicate\n  github-path:"),
    )
    .unwrap();
    let error = update(&fixture.home, &mut |args| {
        assert!(args.starts_with(&["skill", "list"]));
        fake_read(args, &fixture.rows, OLD)
    })
    .unwrap_err();
    assert!(format!("{error:#}").contains("duplicate GitHub skill metadata"));
}

#[test]
fn missing_release_uses_default_branch_and_other_http_errors_stop() {
    let fixture = Fixture::new(OLD);
    for status in [404, 403] {
        let result = update(&fixture.home, &mut |args| {
            if args.last() == Some(&"repos/demo/repo/releases/latest") {
                return Ok(http(&json!({}), status, 1));
            }
            if args.starts_with(&["skill", "list"]) {
                return fake_read(args, &fixture.rows, OLD);
            }
            assert_eq!(status, 404, "403 must not fall back");
            match args.last().copied() {
                Some("repos/demo/repo") => Ok(http(&json!({"default_branch":"base/stable"}), 200, 0)),
                Some("repos/demo/repo/git/ref/heads/base%2Fstable") => {
                    Ok(http(&json!({"object":{"type":"commit","sha":COMMIT}}), 200, 0))
                }
                Some("skillName,sourceURL,path,pinned,scope") => fake_read(args, &fixture.rows, OLD),
                Some("demo") => Ok(output("All skills are up to date.\n".into(), 0)),
                _ => fake_read(args, &fixture.rows, OLD),
            }
        });
        if status == 404 {
            assert_eq!(result.unwrap().updated, 0);
        } else {
            assert!(result.unwrap_err().to_string().contains("GitHub API request failed"));
        }
    }
}
