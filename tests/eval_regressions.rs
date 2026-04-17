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

// ---------------------------------------------------------------------------
// Regression tests added by the independent evaluator (Agent D, round 85).
//
// - `config_rejects_unknown_top_level_field`: Bug 12 (MEDIUM). Silent
//   acceptance of unknown config keys hid the cycle-C bug for weeks (users
//   thought their `[budget] per_skill = 5` was taking effect when in fact the
//   whole section was being ignored). `deny_unknown_fields` now rejects
//   `bogus_field = 1` up front with a clear error.
// - `config_rejects_unknown_nested_field`: same, but for nested sections
//   (catches typos like `per_skil` inside `[budget]`).
// - `unreadable_file_does_not_abort_scan`: Bug 13 (MEDIUM). A single
//   permission-denied skill used to abort the entire scan with exit 2 (an
//   operational error). It now emits a non-fatal `symlink`-kind note and
//   processes every other file, reserving exit 2 for CLI-level errors.
// - `readme_mentions_twelve_rules`: Bug 14 (LOW). `README.md` claimed 11
//   distinct issue classes (and cited `SKILL001`–`SKILL011`) in two places
//   even though cycle C added `SKILL012` total-bloated.
// - `changelog_documents_new_rule_ids`: Bug 15 (LOW). CHANGELOG.md was stuck
//   on the original cycle-0 feature list and did not reflect SKILL011 /
//   SKILL012 or the behavioural fixes from cycles A–C.
// ---------------------------------------------------------------------------

#[test]
fn config_rejects_unknown_top_level_field() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".skilldigest.toml"),
        "bogus_field = 42\n[budget]\nper_skill = 100\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("a")).unwrap();
    std::fs::write(dir.path().join("a/SKILL.md"), b"body").unwrap();
    let output = Command::new(bin())
        .args(["scan", dir.path().to_str().unwrap(), "--no-color"])
        .output()
        .expect("run scan");
    assert_eq!(
        output.status.code(),
        Some(2),
        "unknown top-level config field must produce an operational (exit 2) config error, \
         got stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("bogus_field") || stderr.contains("unknown field"),
        "config error must name the offending field; got: {stderr}",
    );
}

#[test]
fn config_rejects_unknown_nested_field() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".skilldigest.toml"),
        "[budget]\nper_skil = 123\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("a")).unwrap();
    std::fs::write(dir.path().join("a/SKILL.md"), b"body").unwrap();
    let output = Command::new(bin())
        .args(["scan", dir.path().to_str().unwrap(), "--no-color"])
        .output()
        .expect("run scan");
    assert_eq!(
        output.status.code(),
        Some(2),
        "unknown nested config field must fail; stderr={:?}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[cfg(unix)]
fn unreadable_file_does_not_abort_scan() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("good/SKILL.md");
    let bad = dir.path().join("bad/SKILL.md");
    std::fs::create_dir_all(good.parent().unwrap()).unwrap();
    std::fs::create_dir_all(bad.parent().unwrap()).unwrap();
    std::fs::write(&good, b"readable body").unwrap();
    std::fs::write(&bad, b"unreadable body").unwrap();
    std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o000)).unwrap();

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

    // Restore permissions before asserting so tempdir cleanup succeeds even
    // when the assertion fails.
    let _ = std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o644));

    let code = output.status.code();
    assert!(
        matches!(code, Some(0) | Some(1)),
        "per-file read failure must NOT return operational exit 2; got {code:?}; stderr={}",
        String::from_utf8_lossy(&output.stderr),
    );
    // The readable skill must still make it into the report.
    let v: serde_json::Value =
        serde_json::from_str(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
    let skill_ids: Vec<&str> = v["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap_or_default())
        .collect();
    assert!(
        skill_ids.contains(&"good"),
        "scan must continue past an unreadable file; ids: {skill_ids:?}"
    );
}

#[test]
fn readme_mentions_twelve_rules() {
    let readme =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("README.md"))
            .expect("read README");
    assert!(
        !readme.contains("11 distinct issue classes"),
        "README still claims 11 issue classes but SKILL012 has been added",
    );
    assert!(
        !readme.contains("(`SKILL001`–`SKILL011`)")
            && !readme.contains("(`SKILL001` – `SKILL011`)"),
        "README must cite the full SKILL001–SKILL012 range",
    );
    assert!(
        readme.contains("SKILL012"),
        "README rule catalogue must include SKILL012",
    );
}

#[test]
fn changelog_documents_new_rule_ids() {
    let log =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("CHANGELOG.md"))
            .expect("read CHANGELOG");
    assert!(
        log.contains("SKILL011") && log.contains("SKILL012"),
        "CHANGELOG must document the path-escape (SKILL011) and total-bloated (SKILL012) rules",
    );
    assert!(
        log.contains("total-bloated") || log.contains("total_bloated"),
        "CHANGELOG must mention the total-bloated behaviour",
    );
}

// ---------------------------------------------------------------------------
// Regression tests added by the independent evaluator (Agent G, round 85).
//
// - `wiki_link_mention_creates_reference_edge`: Bug 17 (HIGH). README and spec
//   both document `[[wiki-style]]` mentions as a supported form of skill
//   reference. In practice pulldown-cmark splits `[[foo]]` into five separate
//   Text events (`[`, `[`, `foo`, `]`, `]`) so the event-driven scanner never
//   observed the full `[[foo]]` string. Every wiki-link was silently dropped,
//   which meant any skill referenced ONLY via wiki-links was erroneously
//   flagged as dead. Fix: run an additional raw-body scan for wiki links.
// ---------------------------------------------------------------------------

#[test]
fn wiki_link_mention_creates_reference_edge() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a/SKILL.md");
    let target = dir.path().join("target-skill/SKILL.md");
    std::fs::create_dir_all(a.parent().unwrap()).unwrap();
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&a, "---\nname: a\n---\nReference to [[target-skill]].\n").unwrap();
    std::fs::write(&target, "---\nname: target\n---\nbody\n").unwrap();

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

    // target-skill must have an incoming edge from a.
    let target_summary = v["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"].as_str() == Some("target-skill"))
        .expect("target-skill present in report");
    assert!(
        target_summary["refs_in"].as_u64().unwrap() >= 1,
        "wiki-link [[target-skill]] must produce an incoming ref on target-skill; \
         summary={target_summary}"
    );

    // And target-skill must NOT be flagged dead.
    let dead_ids: Vec<&str> = v["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["kind"].as_str() == Some("dead"))
        .map(|i| i["skill"].as_str().unwrap_or_default())
        .collect();
    assert!(
        !dead_ids.contains(&"target-skill"),
        "skill referenced via wiki link must not be flagged dead; dead={dead_ids:?}"
    );
}

// ---------------------------------------------------------------------------
// Regression tests added by the independent evaluator (Agent F, round 85).
//
// - `readme_does_not_claim_nonexistent_env_vars`: Bug 16 (LOW). Pre-fix the
//   README promised an opt-in env var `SKILLDIGEST_EMIT_TIMESTAMP=1` that
//   *no code in the crate ever reads*. Setting it did absolutely nothing —
//   a silent documentation lie. Documentation-vs-behavior drift of this
//   kind hid the cycle-C config-precedence bug for weeks; this guard pins
//   the README so a future contributor cannot re-introduce an env-var
//   claim that is not actually honored by the binary.
// ---------------------------------------------------------------------------

#[test]
fn readme_does_not_claim_nonexistent_env_vars() {
    let readme =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("README.md"))
            .expect("read README");
    // The binary reads no environment variables at all. Any README claim of
    // `SKILLDIGEST_*` as an opt-in must be backed by an actual `std::env::var`
    // read somewhere in `src/`, or removed.
    assert!(
        !readme.contains("SKILLDIGEST_EMIT_TIMESTAMP"),
        "README must not advertise SKILLDIGEST_EMIT_TIMESTAMP — no code reads it",
    );
    // Scan every source file for any `env::var` usage. If a future PR adds a
    // new env var, this guard makes the author either honor the docs claim or
    // remove it.
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found_env_read = false;
    let walker = walk(&src);
    for p in walker {
        let text = std::fs::read_to_string(&p).unwrap_or_default();
        if text.contains("env::var") || text.contains("std::env::var") {
            found_env_read = true;
            break;
        }
    }
    // This assertion is informational: if a future contributor introduces
    // `std::env::var(...)` inside `src/`, they must also add the corresponding
    // README documentation. Today, `found_env_read` is `false` and the README
    // correctly documents zero env-var knobs.
    if found_env_read {
        assert!(
            readme.contains("Environment variables")
                || readme.contains("environment variable")
                || readme.contains("SKILLDIGEST_"),
            "src/ reads env vars but README does not document any — drift risk",
        );
    }
}

// --- Eval-H: pulldown-cmark blind-spot audit + Unix hygiene ---
//
// Cycle G restored `[[wiki-link]]` mentions that pulldown-cmark splits into
// five Text events. Cycle H extends that audit:
//
// 1. Obsidian-style `[[target|display]]` aliases must resolve to `target`.
//    Without this, every skill that renames a link via a pipe alias gets
//    flagged dead while the display label becomes an (unresolvable) phantom
//    skill id.
// 2. Frontmatter `requires: [typo]` targeting a skill that does not exist in
//    the library must produce a `Stale` (SKILL004) issue rather than being
//    silently dropped by the graph builder.
// 3. `skilldigest ... | head` must not exit 2 with `I/O error on <stdout>`.
//    A well-behaved Unix CLI treats BrokenPipe as clean termination.

#[test]
fn wiki_link_pipe_alias_resolves_to_target() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("foo")).unwrap();
    std::fs::write(
        dir.path().join("README.md"),
        "See [[foo|custom display]] for details\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("foo/SKILL.md"), "body\n").unwrap();

    let output = Command::new(bin())
        .args(["scan", dir.path().to_str().unwrap(), "--format", "json"])
        .output()
        .unwrap();
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    let v: serde_json::Value = serde_json::from_str(stdout).unwrap();
    let issues = v["issues"].as_array().unwrap();
    // `foo` must not appear as a dead skill, because the README pipe-alias
    // wiki link points at it.
    for issue in issues {
        let kind = issue["kind"].as_str().unwrap_or("");
        let skill = issue["skill"].as_str().unwrap_or("");
        assert!(
            !(kind == "dead" && skill == "foo"),
            "pipe-alias target 'foo' wrongly flagged dead: {stdout}"
        );
    }
}

#[test]
fn wiki_link_rejects_prose_with_space_before_pipe() {
    // `[[foo bar|display]]` — target itself has a space, so the whole thing
    // is prose, not a wiki link. Must not silently create a phantom mention.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("foo")).unwrap();
    std::fs::write(
        dir.path().join("README.md"),
        "Maybe [[foo bar|display]] is prose\n@foo\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("foo/SKILL.md"), "body\n").unwrap();

    let output = Command::new(bin())
        .args(["scan", dir.path().to_str().unwrap(), "--format", "json"])
        .output()
        .unwrap();
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    let v: serde_json::Value = serde_json::from_str(stdout).unwrap();
    // The `@foo` mention keeps `foo` alive; the prose-ish `[[foo bar|...]]`
    // should not itself trigger any diagnostic (no crash, no false mention).
    let issues = v["issues"].as_array().unwrap();
    for issue in issues {
        assert!(
            !(issue["kind"].as_str() == Some("dead") && issue["skill"].as_str() == Some("foo")),
            "prose probe: 'foo' wrongly dead: {stdout}"
        );
    }
}

#[test]
fn frontmatter_requires_missing_target_is_stale() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("a")).unwrap();
    std::fs::write(
        dir.path().join("a/SKILL.md"),
        "---\nname: a\nrequires:\n  - this-skill-does-not-exist\n---\nbody\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("README.md"), "See @a for details\n").unwrap();

    let output = Command::new(bin())
        .args(["scan", dir.path().to_str().unwrap(), "--format", "json"])
        .output()
        .unwrap();
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    let v: serde_json::Value = serde_json::from_str(stdout).unwrap();
    let issues = v["issues"].as_array().unwrap();
    let found = issues.iter().any(|i| {
        i["kind"].as_str() == Some("stale")
            && i["skill"].as_str() == Some("a")
            && i["message"]
                .as_str()
                .unwrap_or("")
                .contains("this-skill-does-not-exist")
    });
    assert!(
        found,
        "frontmatter requires: missing target must produce a Stale issue: {stdout}"
    );
}

#[test]
fn frontmatter_requires_existing_target_is_clean() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("a")).unwrap();
    std::fs::create_dir_all(dir.path().join("b")).unwrap();
    std::fs::write(
        dir.path().join("a/SKILL.md"),
        "---\nname: a\nrequires:\n  - b\n---\nbody\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("b/SKILL.md"), "body\n").unwrap();
    std::fs::write(dir.path().join("README.md"), "@a @b\n").unwrap();

    let output = Command::new(bin())
        .args(["scan", dir.path().to_str().unwrap(), "--format", "json"])
        .output()
        .unwrap();
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    let v: serde_json::Value = serde_json::from_str(stdout).unwrap();
    let issues = v["issues"].as_array().unwrap();
    // No stale issues should be present when requires target exists.
    for issue in issues {
        assert_ne!(
            issue["kind"].as_str(),
            Some("stale"),
            "valid requires produced a stale issue: {stdout}"
        );
    }
}

#[cfg(unix)]
#[test]
fn broken_pipe_exits_cleanly_not_as_operational_error() {
    use std::io::Read;
    use std::process::Stdio;
    let dir = tempfile::tempdir().unwrap();
    for i in 0..50 {
        let sub = dir.path().join(format!("skill-{i:03}"));
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            sub.join("SKILL.md"),
            format!(
                "body text for skill {i} with plenty of words to make output fill the pipe buffer\n"
            ),
        )
        .unwrap();
    }
    let mut child = Command::new(bin())
        .args(["scan", dir.path().to_str().unwrap(), "--format", "json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn skilldigest");
    // Read exactly one byte and then drop the pipe — triggers BrokenPipe on
    // the child's next write.
    {
        let mut stdout = child.stdout.take().unwrap();
        let mut one = [0u8; 1];
        let _ = stdout.read(&mut one);
        drop(stdout);
    }
    let status = child.wait().expect("wait");
    // Acceptable: 0 (clean) or 1 (issues). 2 means the CLI treated the
    // BrokenPipe as an operational error, which is the exact bug we are
    // guarding against.
    let code = status.code().unwrap_or(-1);
    assert!(
        code == 0 || code == 1,
        "skilldigest | head exited {code}; BrokenPipe must not surface as operational error (2)",
    );
    // Also verify stderr does not leak the I/O error message to users who
    // piped into head/less/etc.
    let mut err = String::new();
    let _ = child.stderr.take().map(|mut s| s.read_to_string(&mut err));
    assert!(
        !err.contains("Broken pipe"),
        "BrokenPipe error text leaked to stderr: {err}"
    );
}

// ---------------------------------------------------------------------------
// Regression tests added by the independent evaluator (Agent I, round 85).
//
// - `wiki_link_heading_anchor_resolves_to_target`: Bug 18 (HIGH). Obsidian /
//   Dendron / many note-taking tools let authors deep-link into a section of
//   a document via `[[skill#heading]]`. Before the cycle-I fix the raw scan
//   captured the mention as the literal `skill#heading` string, which never
//   matched any skill id. Result: every deep link silently dropped, and the
//   target skill was wrongly reported as dead. Guard: a `[[foo#usage]]`
//   mention MUST resolve to skill id `foo` and keep it out of the dead list.
// ---------------------------------------------------------------------------

#[test]
fn wiki_link_heading_anchor_resolves_to_target() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("foo")).unwrap();
    std::fs::write(
        dir.path().join("README.md"),
        "See [[foo#usage]] for details\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("foo/SKILL.md"), "body\n").unwrap();

    let output = Command::new(bin())
        .args(["scan", dir.path().to_str().unwrap(), "--format", "json"])
        .output()
        .unwrap();
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    let v: serde_json::Value = serde_json::from_str(stdout).unwrap();
    let issues = v["issues"].as_array().unwrap();
    for issue in issues {
        assert!(
            !(issue["kind"].as_str() == Some("dead") && issue["skill"].as_str() == Some("foo")),
            "heading-anchor target 'foo' wrongly flagged dead: {stdout}"
        );
    }
    // And `foo` must pick up at least one incoming edge.
    let foo = v["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"].as_str() == Some("foo"))
        .expect("foo in report");
    assert!(
        foo["refs_in"].as_u64().unwrap_or(0) >= 1,
        "wiki-link with heading anchor must produce a refs_in edge on target; summary={foo}"
    );
}

#[test]
fn wiki_link_pipe_alias_and_heading_anchor_together() {
    // Belt-and-braces: `[[target#heading|display]]` must resolve to `target`.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("foo")).unwrap();
    std::fs::write(
        dir.path().join("README.md"),
        "See [[foo#usage|pretty label]] for details\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("foo/SKILL.md"), "body\n").unwrap();

    let output = Command::new(bin())
        .args(["scan", dir.path().to_str().unwrap(), "--format", "json"])
        .output()
        .unwrap();
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    let v: serde_json::Value = serde_json::from_str(stdout).unwrap();
    for issue in v["issues"].as_array().unwrap() {
        assert!(
            !(issue["kind"].as_str() == Some("dead") && issue["skill"].as_str() == Some("foo")),
            "pipe+anchor target 'foo' wrongly flagged dead: {stdout}"
        );
    }
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(p);
        }
    }
    out
}
