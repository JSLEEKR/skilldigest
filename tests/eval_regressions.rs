//! Regression tests added by the independent evaluator (Agent A, round 85).
//!
//! Each test pins a specific bug found during the hostile evaluation pass so
//! the fix cannot silently regress.
//!
//! - `json_report_has_tokenizer_version_field`: Bug 1 (HIGH).
//! - `sarif_properties_include_tokenizer_version`: Bug 1 (HIGH).
//! - `tokens_json_includes_tokenizer_version_field`: Bug 1 (HIGH).
//! - `rules_inside_fenced_code_blocks_are_ignored`: Bug 2 (HIGH).
//! - `symlink_escape_uses_path_escape_kind_not_symlink`: Bug 3 (MEDIUM).

use std::path::PathBuf;
use std::process::Command;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_skilldigest"))
}

fn fixtures(sub: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(sub)
}

#[test]
fn json_report_has_tokenizer_version_field() {
    let output = Command::new(bin())
        .args([
            "scan",
            fixtures("tiny").to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .expect("run skilldigest");
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    let v: serde_json::Value = serde_json::from_str(stdout).expect("valid json");
    let tv = v["tokenizer_version"]
        .as_str()
        .expect("tokenizer_version field present and a string");
    // Must contain both the library id and the tokenizer name so consumers
    // can detect drift in either dimension.
    assert!(
        tv.contains("tiktoken-rs") && tv.contains("cl100k"),
        "tokenizer_version = {tv}"
    );
}

#[test]
fn sarif_properties_include_tokenizer_version() {
    let output = Command::new(bin())
        .args([
            "scan",
            fixtures("tiny").to_str().unwrap(),
            "--format",
            "sarif",
        ])
        .output()
        .expect("run skilldigest");
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    let v: serde_json::Value = serde_json::from_str(stdout).expect("valid json");
    let props = &v["runs"][0]["properties"];
    let tv = props["tokenizer_version"]
        .as_str()
        .expect("tokenizer_version in SARIF run properties");
    assert!(tv.contains("tiktoken-rs"), "tokenizer_version = {tv}");
}

#[test]
fn tokens_json_includes_tokenizer_version_field() {
    let fx = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/tiny/alpha/SKILL.md");
    let output = Command::new(bin())
        .args(["tokens", fx.to_str().unwrap(), "--format", "json"])
        .output()
        .expect("run");
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    let v: serde_json::Value = serde_json::from_str(stdout).expect("valid json");
    let tv = v["tokenizer_version"]
        .as_str()
        .expect("tokenizer_version on the tokens subcommand json output");
    assert!(tv.contains("cl100k"), "tokenizer_version = {tv}");
}

#[test]
fn rules_inside_fenced_code_blocks_are_ignored() {
    // Two skills that each contain a modal prefix INSIDE a fenced code block.
    // Before the fix this produced a false-positive `conflict` issue.
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a/SKILL.md");
    let b = dir.path().join("b/SKILL.md");
    std::fs::create_dir_all(a.parent().unwrap()).unwrap();
    std::fs::create_dir_all(b.parent().unwrap()).unwrap();
    std::fs::write(
        &a,
        "---\nname: a\n---\n# A\n\n```\nMUST NOT use Bash(rm)\n```\n",
    )
    .unwrap();
    std::fs::write(
        &b,
        "---\nname: b\n---\n# B\n\n```\nMUST use Bash(rm)\n```\n",
    )
    .unwrap();

    let output = Command::new(bin())
        .args([
            "scan",
            dir.path().to_str().unwrap(),
            "--format",
            "json",
            "--no-color",
        ])
        .output()
        .expect("run");
    let v: serde_json::Value =
        serde_json::from_str(std::str::from_utf8(&output.stdout).unwrap()).expect("valid json");
    let conflicts: Vec<_> = v["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["kind"].as_str() == Some("conflict"))
        .collect();
    assert!(
        conflicts.is_empty(),
        "rules inside fenced code blocks must not create a conflict: {:?}",
        conflicts
    );
}

#[test]
fn rules_outside_code_blocks_still_detected() {
    // Guard test: make sure the code-block fix did NOT kill rule extraction
    // for prose rules.
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a/SKILL.md");
    let b = dir.path().join("b/SKILL.md");
    std::fs::create_dir_all(a.parent().unwrap()).unwrap();
    std::fs::create_dir_all(b.parent().unwrap()).unwrap();
    std::fs::write(&a, "---\nname: a\n---\n# A\n\nMUST NOT use `Bash(rm)`.\n").unwrap();
    std::fs::write(&b, "---\nname: b\n---\n# B\n\nMUST use `Bash(rm)`.\n").unwrap();

    let output = Command::new(bin())
        .args([
            "scan",
            dir.path().to_str().unwrap(),
            "--format",
            "json",
            "--no-color",
        ])
        .output()
        .expect("run");
    let v: serde_json::Value =
        serde_json::from_str(std::str::from_utf8(&output.stdout).unwrap()).expect("valid json");
    let has_conflict = v["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["kind"].as_str() == Some("conflict"));
    assert!(
        has_conflict,
        "prose rules must still produce conflicts; issues: {:?}",
        v["issues"]
    );
}

#[test]
fn symlink_escape_uses_path_escape_kind_not_symlink() {
    // Only meaningful on platforms that support symlinks. On Windows this is
    // a no-op test because creating symlinks requires elevation.
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let inside_skill = dir.path().join("inside/SKILL.md");
        std::fs::create_dir_all(inside_skill.parent().unwrap()).unwrap();
        std::fs::write(&inside_skill, "---\nname: inside\n---\nhi").unwrap();

        // Create a secondary directory *outside* the scan root, containing a
        // SKILL.md, then symlink to that directory from inside the scan
        // root. With --follow-symlinks, skilldigest must reject the escape
        // and tag it with kind=path_escape (NOT the old symlink kind).
        let outside = tempfile::tempdir().unwrap();
        let evil_skill = outside.path().join("evil/SKILL.md");
        std::fs::create_dir_all(evil_skill.parent().unwrap()).unwrap();
        std::fs::write(&evil_skill, "---\nname: evil\n---\nsecret").unwrap();
        symlink(outside.path(), dir.path().join("escape-link")).unwrap();

        let output = Command::new(bin())
            .args([
                "scan",
                dir.path().to_str().unwrap(),
                "--follow-symlinks",
                "--format",
                "json",
                "--no-color",
            ])
            .output()
            .expect("run");
        let v: serde_json::Value =
            serde_json::from_str(std::str::from_utf8(&output.stdout).unwrap()).expect("valid json");
        let issues = v["issues"].as_array().unwrap();
        let has_path_escape = issues
            .iter()
            .any(|i| i["kind"].as_str() == Some("path_escape"));
        assert!(
            has_path_escape,
            "expected a path_escape issue; issues: {:?}",
            issues
        );
        // And the secret body must not have been analyzed.
        let skill_ids: Vec<&str> = v["skills"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap_or_default())
            .collect();
        assert!(
            !skill_ids.iter().any(|id| id.contains("evil")),
            "scan analyzed a skill outside its root; ids: {:?}",
            skill_ids
        );
    }
}

#[test]
fn sarif_knows_about_path_escape_rule() {
    let output = Command::new(bin())
        .args([
            "scan",
            fixtures("tiny").to_str().unwrap(),
            "--format",
            "sarif",
        ])
        .output()
        .expect("run skilldigest");
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    let v: serde_json::Value = serde_json::from_str(stdout).expect("valid json");
    let rules = v["runs"][0]["tool"]["driver"]["rules"].as_array().unwrap();
    let ids: Vec<&str> = rules
        .iter()
        .map(|r| r["id"].as_str().unwrap_or_default())
        .collect();
    assert!(
        ids.contains(&"SKILL011"),
        "SARIF driver.rules missing the path_escape entry: {:?}",
        ids
    );
}

#[test]
fn tokens_total_matches_whole_file_tokenization() {
    // Bug 4 (LOW): in non --by-section mode, the tokens subcommand used to
    // tokenize a `format!("{frontmatter_raw}{body}")` string which dropped
    // the `---` delimiters, producing a count slightly off from the actual
    // file contents. Regression guard: the reported total must equal the
    // count of the raw file bytes under the same tokenizer.
    let fx = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/tiny/alpha/SKILL.md");
    let output = Command::new(bin())
        .args(["tokens", fx.to_str().unwrap(), "--format", "json"])
        .output()
        .expect("run");
    let v: serde_json::Value =
        serde_json::from_str(std::str::from_utf8(&output.stdout).unwrap()).expect("valid json");
    let reported_total = v["total"].as_u64().unwrap() as usize;

    // Hand-compute via the library.
    let tok = skilldigest::tokenize::by_name("cl100k").unwrap();
    let bytes = std::fs::read(&fx).unwrap();
    let whole = String::from_utf8_lossy(&bytes);
    let expected = tok.count(&whole);
    assert_eq!(
        reported_total, expected,
        "tokens subcommand must tokenize the whole file"
    );
}
