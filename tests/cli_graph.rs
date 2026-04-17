//! CLI graph subcommand tests.

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
fn graph_default_emits_dot() {
    let output = Command::new(bin())
        .args([
            "graph",
            fixtures("tiny").to_str().unwrap(),
            "--format",
            "dot",
        ])
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("digraph skilldigest"));
}

#[test]
fn graph_json_has_nodes_array() {
    let output = Command::new(bin())
        .args([
            "graph",
            fixtures("tiny").to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .expect("run");
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    let v: serde_json::Value = serde_json::from_str(stdout).expect("json");
    assert!(v["nodes"].is_array());
    assert!(v["edges"].is_array());
}

#[test]
fn graph_markdown_embeds_dot_codeblock() {
    let output = Command::new(bin())
        .args([
            "graph",
            fixtures("tiny").to_str().unwrap(),
            "--format",
            "markdown",
        ])
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("### skilldigest graph"));
    assert!(stdout.contains("```dot"));
}

#[test]
fn graph_rejects_sarif_format() {
    let output = Command::new(bin())
        .args([
            "graph",
            fixtures("tiny").to_str().unwrap(),
            "--format",
            "sarif",
        ])
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(2));
}
