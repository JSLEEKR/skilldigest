//! Configuration loading for `.skilldigest.toml`.
//!
//! The file is entirely optional. When present, its fields override CLI
//! defaults (but not the CLI flags — flags always win).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Top-level TOML document.
///
/// `deny_unknown_fields` is intentional: every silent typo in a user's
/// `.skilldigest.toml` eventually shows up as "why isn't my budget being
/// applied?" cycle-C-style bug report. Rejecting unknown keys up front keeps
/// config drift from masquerading as silently-broken behaviour.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigDoc {
    /// Budget section.
    #[serde(default)]
    pub budget: BudgetSection,
    /// Tokenizer section.
    #[serde(default)]
    pub tokenizer: TokenizerSection,
    /// Ignore section.
    #[serde(default)]
    pub ignore: IgnoreSection,
    /// Per-skill overrides.
    #[serde(default)]
    pub overrides: BTreeMap<String, SkillOverride>,
}

/// Budget section.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetSection {
    /// Per-skill budget (tokens).
    #[serde(default = "default_per_skill")]
    pub per_skill: usize,
    /// Optional aggregate cap.
    #[serde(default)]
    pub total: Option<usize>,
}

impl Default for BudgetSection {
    fn default() -> Self {
        Self {
            per_skill: default_per_skill(),
            total: None,
        }
    }
}

fn default_per_skill() -> usize {
    2000
}

/// Tokenizer section.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TokenizerSection {
    /// Tokenizer name ("cl100k", "o200k", "llama3").
    #[serde(default)]
    pub default: Option<String>,
}

/// Ignore section.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IgnoreSection {
    /// Glob patterns to skip.
    #[serde(default)]
    pub globs: Vec<String>,
}

/// Per-skill override.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SkillOverride {
    /// Custom budget for this specific skill.
    #[serde(default)]
    pub budget: Option<usize>,
}

/// Load a config doc from a path. Returns `Ok(None)` when the file does not
/// exist.
pub fn load(path: &Path) -> Result<Option<ConfigDoc>> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    let doc: ConfigDoc = toml::from_str(&bytes).map_err(|e| Error::Config {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    Ok(Some(doc))
}

/// Find the config file next to the scan root, or under a parent.
#[must_use]
pub fn find_default(scan_root: &Path) -> Option<std::path::PathBuf> {
    let candidate = scan_root.join(".skilldigest.toml");
    if candidate.is_file() {
        return Some(candidate);
    }
    // also look at the parent
    if let Some(parent) = scan_root.parent() {
        let p = parent.join(".skilldigest.toml");
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn default_config_has_sensible_budget() {
        let c = ConfigDoc::default();
        assert_eq!(c.budget.per_skill, 2000);
        assert!(c.budget.total.is_none());
    }

    #[test]
    fn load_returns_none_for_missing_file() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("nope.toml");
        let out = load(&p).unwrap();
        assert!(out.is_none());
    }

    #[test]
    fn load_parses_valid_file() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".skilldigest.toml");
        fs::write(
            &p,
            r#"
[budget]
per_skill = 4000
total = 50000

[tokenizer]
default = "cl100k"

[ignore]
globs = ["archive/**"]

[overrides."git/commit"]
budget = 3000
"#,
        )
        .unwrap();
        let doc = load(&p).unwrap().unwrap();
        assert_eq!(doc.budget.per_skill, 4000);
        assert_eq!(doc.budget.total, Some(50000));
        assert_eq!(doc.tokenizer.default.as_deref(), Some("cl100k"));
        assert_eq!(doc.ignore.globs, vec!["archive/**".to_string()]);
        assert_eq!(
            doc.overrides.get("git/commit").and_then(|o| o.budget),
            Some(3000)
        );
    }

    #[test]
    fn load_rejects_malformed() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".skilldigest.toml");
        fs::write(&p, "this is = not = toml [").unwrap();
        let err = load(&p).unwrap_err();
        assert!(matches!(err, Error::Config { .. }));
    }

    #[test]
    fn find_default_finds_in_scan_root() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".skilldigest.toml");
        fs::write(&p, "").unwrap();
        let found = find_default(dir.path()).unwrap();
        assert_eq!(found, p);
    }

    #[test]
    fn find_default_returns_none_when_absent() {
        let dir = tempdir().unwrap();
        let found = find_default(dir.path());
        assert!(found.is_none());
    }

    #[test]
    fn empty_toml_is_ok() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".skilldigest.toml");
        fs::write(&p, "").unwrap();
        let doc = load(&p).unwrap().unwrap();
        assert_eq!(doc.budget.per_skill, 2000);
    }

    #[test]
    fn skill_override_optional_budget() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".skilldigest.toml");
        fs::write(
            &p,
            r#"
[overrides."a"]
"#,
        )
        .unwrap();
        let doc = load(&p).unwrap().unwrap();
        assert!(doc.overrides.get("a").unwrap().budget.is_none());
    }
}
