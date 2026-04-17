//! CLI tokens subcommand tests.

use std::path::PathBuf;
use std::process::Command;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_skilldigest"))
}

#[test]
fn tokens_counts_file() {
    let fx = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/tiny/alpha/SKILL.md");
    let output = Command::new(bin())
        .args(["tokens", fx.to_str().unwrap()])
        .output()
        .expect("run");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("tokens"));
}

#[test]
fn tokens_json_format() {
    let fx = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/tiny/alpha/SKILL.md");
    let output = Command::new(bin())
        .args(["tokens", fx.to_str().unwrap(), "--format", "json"])
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert!(v["total"].as_u64().unwrap() > 0);
    assert!(v["tokenizer"].as_str().unwrap().contains("cl100k"));
}

#[test]
fn tokens_by_section_splits_counts() {
    let fx = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/tiny/alpha/SKILL.md");
    let output = Command::new(bin())
        .args([
            "tokens",
            fx.to_str().unwrap(),
            "--by-section",
            "--format",
            "json",
        ])
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert!(v["frontmatter"].as_u64().unwrap() > 0);
    assert!(v["body"].as_u64().unwrap() > 0);
}

#[test]
fn tokens_o200k_tokenizer() {
    let fx = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/tiny/alpha/SKILL.md");
    let output = Command::new(bin())
        .args([
            "tokens",
            fx.to_str().unwrap(),
            "--tokenizer",
            "o200k",
            "--format",
            "json",
        ])
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(v["tokenizer"].as_str().unwrap(), "o200k_base");
}

#[test]
fn tokens_llama3_approximate() {
    let fx = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/tiny/alpha/SKILL.md");
    let output = Command::new(bin())
        .args([
            "tokens",
            fx.to_str().unwrap(),
            "--tokenizer",
            "llama3",
            "--format",
            "json",
        ])
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(v["tokenizer"].as_str().unwrap(), "llama3_approx");
}

#[test]
fn tokens_missing_file_returns_error() {
    let output = Command::new(bin())
        .args(["tokens", "/definitely/not/there/abc.md"])
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn tokens_unknown_tokenizer_returns_error() {
    let fx = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/tiny/alpha/SKILL.md");
    let output = Command::new(bin())
        .args(["tokens", fx.to_str().unwrap(), "--tokenizer", "gpt99"])
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(2));
}
