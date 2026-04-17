//! CLI loadout subcommand tests.

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
fn loadout_selects_matching_skills() {
    let output = Command::new(bin())
        .args([
            "loadout",
            fixtures("tiny").to_str().unwrap(),
            "--tag",
            "git",
            "--max-tokens",
            "10000",
            "--no-color",
        ])
        .output()
        .expect("run");
    assert!(output.status.code() == Some(0) || output.status.code() == Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Loadout") || stdout.contains("loadout"));
    assert!(stdout.contains("alpha") || stdout.contains("beta"));
}

#[test]
fn loadout_json_shape() {
    let output = Command::new(bin())
        .args([
            "loadout",
            fixtures("tiny").to_str().unwrap(),
            "--tag",
            "git",
            "--format",
            "json",
        ])
        .output()
        .expect("run");
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    let v: serde_json::Value = serde_json::from_str(stdout).expect("json");
    let loadout = &v["loadout"];
    assert_eq!(loadout["tag"].as_str().unwrap(), "git");
    assert!(!loadout["skills"].as_array().unwrap().is_empty());
}

#[test]
fn loadout_respects_budget() {
    let output = Command::new(bin())
        .args([
            "loadout",
            fixtures("tiny").to_str().unwrap(),
            "--tag",
            "git",
            "--max-tokens",
            "1",
            "--format",
            "json",
        ])
        .output()
        .expect("run");
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    let v: serde_json::Value = serde_json::from_str(stdout).expect("json");
    let loadout = &v["loadout"];
    let selected = loadout["skills"].as_array().unwrap().len();
    // Every skill in tiny is > 1 token so tiny budget → 0 selected.
    assert_eq!(selected, 0);
}

#[test]
fn loadout_unknown_tag_returns_zero_skills() {
    let output = Command::new(bin())
        .args([
            "loadout",
            fixtures("tiny").to_str().unwrap(),
            "--tag",
            "bogus-nonexistent-tag",
            "--format",
            "json",
        ])
        .output()
        .expect("run");
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    let v: serde_json::Value = serde_json::from_str(stdout).expect("json");
    assert_eq!(v["loadout"]["skills"].as_array().unwrap().len(), 0);
}
