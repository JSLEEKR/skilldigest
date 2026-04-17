//! End-to-end CLI scan tests.

use std::path::PathBuf;
use std::process::Command;

fn bin() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_BIN_EXE_skilldigest"));
    if !p.exists() {
        // some sandboxes put the binary one level up
        p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/skilldigest");
    }
    p
}

fn fixtures(sub: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(sub)
}

#[test]
fn scan_tiny_fixture_exits_clean_or_warn() {
    let output = Command::new(bin())
        .args(["scan", fixtures("tiny").to_str().unwrap(), "--no-color"])
        .output()
        .expect("run skilldigest");
    // tiny has stale link (gamma -> docs/intro.md missing) which is a warning — exit 0.
    assert!(
        output.status.code() == Some(0) || output.status.code() == Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("skilldigest"));
}

#[test]
fn scan_broken_fixture_reports_dead_and_stale() {
    let output = Command::new(bin())
        .args(["scan", fixtures("broken").to_str().unwrap(), "--no-color"])
        .output()
        .expect("run skilldigest");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("dead") || stdout.contains("stale"),
        "stdout={stdout}"
    );
}

#[test]
fn scan_conflict_fixture_exits_nonzero() {
    let output = Command::new(bin())
        .args(["scan", fixtures("conflict").to_str().unwrap(), "--no-color"])
        .output()
        .expect("run skilldigest");
    assert_eq!(
        output.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("conflict"));
}

#[test]
fn scan_json_output_is_valid_json() {
    let output = Command::new(bin())
        .args([
            "scan",
            fixtures("tiny").to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .expect("run skilldigest");
    let stdout = std::str::from_utf8(&output.stdout).expect("utf-8");
    let _: serde_json::Value = serde_json::from_str(stdout).expect("parses as json");
}

#[test]
fn scan_sarif_output_includes_driver() {
    let output = Command::new(bin())
        .args([
            "scan",
            fixtures("tiny").to_str().unwrap(),
            "--format",
            "sarif",
        ])
        .output()
        .expect("run skilldigest");
    let stdout = std::str::from_utf8(&output.stdout).expect("utf-8");
    let v: serde_json::Value = serde_json::from_str(stdout).expect("parses as json");
    assert_eq!(v["version"].as_str().unwrap(), "2.1.0");
    assert_eq!(
        v["runs"][0]["tool"]["driver"]["name"].as_str().unwrap(),
        "skilldigest"
    );
}

#[test]
fn scan_markdown_output_starts_with_header() {
    let output = Command::new(bin())
        .args([
            "scan",
            fixtures("tiny").to_str().unwrap(),
            "--format",
            "markdown",
        ])
        .output()
        .expect("run skilldigest");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("### skilldigest report"));
}

#[test]
fn scan_rejects_nonexistent_path() {
    let output = Command::new(bin())
        .args(["scan", "/definitely/not/there/abc123xyz"])
        .output()
        .expect("run skilldigest");
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn scan_no_color_omits_ansi() {
    let output = Command::new(bin())
        .args(["scan", fixtures("tiny").to_str().unwrap(), "--no-color"])
        .output()
        .expect("run skilldigest");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains('\x1b'));
}

#[test]
fn scan_writes_output_file_when_flagged() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.json");
    let status = Command::new(bin())
        .args([
            "scan",
            fixtures("tiny").to_str().unwrap(),
            "--format",
            "json",
            "--output",
            out.to_str().unwrap(),
        ])
        .status()
        .expect("run skilldigest");
    assert!(status.success() || status.code() == Some(1));
    assert!(out.exists(), "output file not written");
    let contents = std::fs::read_to_string(&out).unwrap();
    let _: serde_json::Value = serde_json::from_str(&contents).expect("valid json file");
}

#[test]
fn scan_respects_budget_flag() {
    let output = Command::new(bin())
        .args([
            "scan",
            fixtures("tiny").to_str().unwrap(),
            "--budget",
            "5",
            "--no-color",
        ])
        .output()
        .expect("run");
    // Very small budget forces bloated → exit 1.
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("bloated"));
}

#[test]
fn scan_fix_hint_prints_rm_commands() {
    let output = Command::new(bin())
        .args([
            "scan",
            fixtures("broken").to_str().unwrap(),
            "--no-color",
            "--fix-hint",
        ])
        .output()
        .expect("run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("rm -r") || stderr.contains("# skilldigest"));
}

#[test]
fn scan_help_exits_zero() {
    let output = Command::new(bin())
        .args(["scan", "--help"])
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(0));
}
