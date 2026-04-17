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

// ---------------------------------------------------------------------------
// Regression tests added by the independent evaluator (Agent B, round 85).
//
// - `tokens_strips_bom_like_scan`: Bug 5 (MEDIUM). The tokens subcommand used
//   to tokenize raw bytes with a leading UTF-8 BOM intact, producing a count
//   that disagreed with the parser (which strips BOM). A BOM-prefixed file
//   and the same file without BOM must report the same token count from the
//   `tokens` subcommand.
// - `cycle_participants_all_list_cycle_kind`: Bug 6 (LOW). In a 3-node cycle
//   only the canonical "primary" skill carried `cycle` in its
//   `issue_kinds`; the other two participants showed an empty list despite
//   being named in the cycle issue's `related` array. PR-comment markdown
//   and UIs were misled.
// - `readme_lists_path_escape_rule`: Bug 7 (LOW). README rule catalogue must
//   list SKILL011 (path-escape); the initial cycle-A fix added the issue
//   kind but did not update docs, so users had no documentation for the new
//   SARIF rule id.
// ---------------------------------------------------------------------------

#[test]
fn tokens_strips_bom_like_scan() {
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let bom_path = dir.path().join("with_bom.md");
    let plain_path = dir.path().join("no_bom.md");
    let payload = b"hello world";
    let mut f = std::fs::File::create(&bom_path).unwrap();
    f.write_all(&[0xEF, 0xBB, 0xBF]).unwrap();
    f.write_all(payload).unwrap();
    drop(f);
    std::fs::write(&plain_path, payload).unwrap();

    let run = |p: &std::path::Path| {
        let out = Command::new(bin())
            .args(["tokens", p.to_str().unwrap(), "--format", "json"])
            .output()
            .expect("run tokens");
        let v: serde_json::Value =
            serde_json::from_str(std::str::from_utf8(&out.stdout).unwrap()).expect("valid json");
        v["total"].as_u64().unwrap()
    };

    let bom_total = run(&bom_path);
    let plain_total = run(&plain_path);
    assert_eq!(
        bom_total, plain_total,
        "a leading UTF-8 BOM must not change the token count — \
         both the scanner and the tokens subcommand strip it before tokenizing",
    );
}

#[test]
fn cycle_participants_all_list_cycle_kind() {
    // Build a 3-node cycle: a -> b -> c -> a. Every skill in the cycle
    // should surface `cycle` in its `issue_kinds`, not just the canonical
    // primary.
    let dir = tempfile::tempdir().unwrap();
    for (name, body) in [("a", "@b\n"), ("b", "@c\n"), ("c", "@a\n")] {
        let p = dir.path().join(name).join("SKILL.md");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
    }

    let output = Command::new(bin())
        .args([
            "scan",
            dir.path().to_str().unwrap(),
            "--format",
            "json",
            "--no-color",
        ])
        .output()
        .expect("run scan");
    let v: serde_json::Value =
        serde_json::from_str(std::str::from_utf8(&output.stdout).unwrap()).expect("valid json");
    let skills = v["skills"].as_array().expect("skills array");
    let participants: Vec<&str> = skills
        .iter()
        .filter(|s| {
            s["issue_kinds"]
                .as_array()
                .map(|arr| arr.iter().any(|k| k.as_str() == Some("cycle")))
                .unwrap_or(false)
        })
        .map(|s| s["id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        participants.len(),
        3,
        "all three cycle participants must carry the cycle kind; got {:?}",
        participants
    );
}

#[test]
fn readme_lists_path_escape_rule() {
    let readme =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("README.md"))
            .expect("read README");
    assert!(
        readme.contains("SKILL011"),
        "README must document the SKILL011 path-escape rule so users can cross-reference SARIF output",
    );
    assert!(
        readme.contains("path-escape") || readme.contains("path_escape"),
        "README rule catalogue must describe the path-escape rule",
    );
}

// ---------------------------------------------------------------------------
// Regression tests added by the independent evaluator (Agent C, round 85).
//
// - `config_budget_section_actually_applied`: Bug 8 (HIGH). The README
//   precedence table promises `[budget] per_skill` takes effect when the CLI
//   does not supply `--budget`, but cycles A and B shipped a CLI layer that
//   always fabricated 2000 and beat the config silently. After the fix, the
//   config value wins.
// - `config_tokenizer_section_actually_applied`: Bug 9 (MEDIUM). Same class
//   of bug for `[tokenizer] default`.
// - `total_budget_flag_emits_total_bloated_issue`: Bug 10 (HIGH). The
//   advertised `--total-budget` flag did nothing — no issue was ever
//   emitted when the library total exceeded the cap. Now it produces a
//   SKILL012 error-severity issue.
// - `readme_lists_total_bloated_rule`: README catalogue must document
//   SKILL012 so SARIF consumers can cross-reference it.
// - `verbose_flag_produces_stderr_output`: Bug 11 (LOW). The `--verbose`
//   flag was declared but never consulted.
// ---------------------------------------------------------------------------

#[test]
fn config_budget_section_actually_applied() {
    let dir = tempfile::tempdir().unwrap();
    // 50 repeated "word" tokens ~= well over 5 tokens.
    std::fs::create_dir_all(dir.path().join("a")).unwrap();
    std::fs::write(dir.path().join("a/SKILL.md"), "word ".repeat(50).as_bytes()).unwrap();
    std::fs::write(
        dir.path().join(".skilldigest.toml"),
        "[budget]\nper_skill = 5\n",
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
        .expect("run scan");
    let v: serde_json::Value =
        serde_json::from_str(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
    assert_eq!(
        v["budget"]["per_skill"].as_u64().unwrap(),
        5,
        "config per_skill must survive when CLI does not override",
    );
    let bloat = v["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["kind"].as_str() == Some("bloated"));
    assert!(
        bloat,
        "[budget] per_skill = 5 must be applied and trigger bloated",
    );
}

#[test]
fn config_tokenizer_section_actually_applied() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("a")).unwrap();
    std::fs::write(dir.path().join("a/SKILL.md"), b"hello world\n").unwrap();
    std::fs::write(
        dir.path().join(".skilldigest.toml"),
        "[tokenizer]\ndefault = \"o200k\"\n",
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
        .expect("run scan");
    let v: serde_json::Value =
        serde_json::from_str(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
    assert_eq!(
        v["tokenizer"].as_str().unwrap(),
        "o200k_base",
        "[tokenizer] default must be honored when --tokenizer is not passed",
    );
}

#[test]
fn total_budget_flag_emits_total_bloated_issue() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("a")).unwrap();
    std::fs::write(dir.path().join("a/SKILL.md"), b"hello world\n").unwrap();
    let output = Command::new(bin())
        .args([
            "scan",
            dir.path().to_str().unwrap(),
            "--total-budget",
            "1",
            "--format",
            "json",
            "--no-color",
        ])
        .output()
        .expect("run scan");
    let v: serde_json::Value =
        serde_json::from_str(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
    let total_bloat: Vec<_> = v["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["kind"].as_str() == Some("total_bloated"))
        .collect();
    assert_eq!(
        total_bloat.len(),
        1,
        "exactly one total_bloated issue expected; issues: {:?}",
        v["issues"]
    );
    // Exit code is 1 because default_severity of TotalBloated is Error.
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn readme_lists_total_bloated_rule() {
    let readme =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("README.md"))
            .expect("read README");
    assert!(
        readme.contains("SKILL012"),
        "README must document the new SKILL012 total-bloated rule",
    );
}

#[test]
fn verbose_flag_produces_stderr_output() {
    let dir = tempfile::tempdir().unwrap();
    // empty dir is enough — we only care that --verbose causes a log line.
    let output = Command::new(bin())
        .args([
            "scan",
            dir.path().to_str().unwrap(),
            "--verbose",
            "--no-color",
        ])
        .output()
        .expect("run scan");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("scanning") || stderr.contains("skilldigest:"),
        "--verbose must produce a stderr log line; got: {stderr}",
    );
}
