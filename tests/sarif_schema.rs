//! SARIF 2.1.0 shape validation.
//!
//! We do not pull in a full SARIF JSON-schema validator (would add a heavy
//! dep), but we do check all the required fields GitHub's code-scanning
//! uploader enforces.

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
fn sarif_top_level_fields() {
    let output = Command::new(bin())
        .args([
            "scan",
            fixtures("broken").to_str().unwrap(),
            "--format",
            "sarif",
        ])
        .output()
        .expect("run");
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    let v: serde_json::Value = serde_json::from_str(stdout).expect("valid json");

    assert_eq!(v["version"].as_str().unwrap(), "2.1.0");
    assert!(v["$schema"].as_str().unwrap().starts_with("https://"));
    assert!(v["runs"].is_array());
    assert_eq!(v["runs"].as_array().unwrap().len(), 1);
}

#[test]
fn sarif_has_tool_driver() {
    let output = Command::new(bin())
        .args([
            "scan",
            fixtures("broken").to_str().unwrap(),
            "--format",
            "sarif",
        ])
        .output()
        .expect("run");
    let v: serde_json::Value =
        serde_json::from_str(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
    let driver = &v["runs"][0]["tool"]["driver"];
    assert_eq!(driver["name"].as_str().unwrap(), "skilldigest");
    assert!(driver["version"]
        .as_str()
        .unwrap()
        .chars()
        .next()
        .unwrap()
        .is_ascii_digit());
    assert!(driver["informationUri"]
        .as_str()
        .unwrap()
        .starts_with("https://"));
    assert!(driver["rules"].is_array());
}

#[test]
fn sarif_rules_have_all_required_fields() {
    let output = Command::new(bin())
        .args([
            "scan",
            fixtures("broken").to_str().unwrap(),
            "--format",
            "sarif",
        ])
        .output()
        .expect("run");
    let v: serde_json::Value =
        serde_json::from_str(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
    let rules = v["runs"][0]["tool"]["driver"]["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 11);
    for rule in rules {
        assert!(rule["id"].is_string());
        assert!(rule["name"].is_string());
        assert!(rule["shortDescription"]["text"].is_string());
        assert!(rule["fullDescription"]["text"].is_string());
        assert!(rule["defaultConfiguration"]["level"].is_string());
        assert!(rule["helpUri"].is_string());
    }
}

#[test]
fn sarif_results_valid_shape_when_present() {
    let output = Command::new(bin())
        .args([
            "scan",
            fixtures("broken").to_str().unwrap(),
            "--format",
            "sarif",
        ])
        .output()
        .expect("run");
    let v: serde_json::Value =
        serde_json::from_str(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
    let results = v["runs"][0]["results"].as_array().unwrap();
    assert!(!results.is_empty(), "expected issues in broken fixture");
    for r in results {
        assert!(r["ruleId"].is_string());
        assert!(r["message"]["text"].is_string());
        assert!(
            r["level"].is_string()
                && ["error", "warning", "note", "none"].contains(&r["level"].as_str().unwrap())
        );
    }
}

#[test]
fn sarif_empty_issues_still_valid() {
    let dir = tempfile::tempdir().unwrap();
    // empty directory → no issues
    let output = Command::new(bin())
        .args(["scan", dir.path().to_str().unwrap(), "--format", "sarif"])
        .output()
        .expect("run");
    let v: serde_json::Value =
        serde_json::from_str(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
    assert_eq!(v["version"].as_str().unwrap(), "2.1.0");
    assert_eq!(v["runs"][0]["results"].as_array().unwrap().len(), 0);
}
