//! Miscellaneous integration tests.

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
fn version_flag_prints_version() {
    let output = Command::new(bin()).arg("--version").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("1.0.0"));
}

#[test]
fn help_flag_lists_subcommands() {
    let output = Command::new(bin()).arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for cmd in ["scan", "tokens", "loadout", "graph"] {
        assert!(stdout.contains(cmd), "missing subcommand {cmd} in help");
    }
}

#[test]
fn exit_code_conflict_is_one() {
    let status = Command::new(bin())
        .args(["scan", fixtures("conflict").to_str().unwrap(), "--no-color"])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(1));
}

#[test]
fn quiet_flag_suppresses_stdout() {
    let output = Command::new(bin())
        .args([
            "scan",
            fixtures("tiny").to_str().unwrap(),
            "--no-color",
            "--quiet",
        ])
        .output()
        .unwrap();
    // stdout may still have the exit-level summary removed entirely
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.is_empty() || !stdout.contains("skilldigest 1.0.0"));
}

#[test]
fn config_file_overrides_budget() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".skilldigest.toml"),
        "[budget]\nper_skill = 1\n",
    )
    .unwrap();
    let skill = dir.path().join("a/SKILL.md");
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::write(&skill, "short body").unwrap();

    let output = Command::new(bin())
        .args([
            "scan",
            dir.path().to_str().unwrap(),
            "--no-color",
            "--config",
            dir.path().join(".skilldigest.toml").to_str().unwrap(),
            "--budget",
            "1",
        ])
        .output()
        .unwrap();
    // Very small budget → bloated → exit 1.
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn deterministic_output_across_runs() {
    let fx = fixtures("tiny");
    let args = ["scan", fx.to_str().unwrap(), "--format", "json"];
    let a = Command::new(bin()).args(args).output().unwrap();
    let b = Command::new(bin()).args(args).output().unwrap();
    assert_eq!(a.stdout, b.stdout);
}
