//! JSON renderer with a stable schema.

use crate::error::{Error, Result};
use crate::model::Report;

/// Render a report as pretty JSON.
pub fn render(report: &Report) -> Result<String> {
    serde_json::to_string_pretty(report).map_err(|e| Error::Other(anyhow::anyhow!("json: {e}")))
}

/// Render a report as compact JSON.
pub fn render_compact(report: &Report) -> Result<String> {
    serde_json::to_string(report).map_err(|e| Error::Other(anyhow::anyhow!("json: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BudgetConfig, Issue, IssueKind, Report, SkillId};

    fn minimal_report() -> Report {
        Report {
            schema_version: crate::SCHEMA_VERSION,
            tokenizer: "cl100k_base".to_string(),
            tool_version: crate::VERSION,
            scan_root: ".".into(),
            total_skills: 1,
            total_tokens: 100,
            budget: BudgetConfig {
                per_skill: 2000,
                total: None,
            },
            skills: vec![],
            issues: vec![Issue::new(
                IssueKind::Dead,
                SkillId::new("a"),
                "never referenced",
            )],
            loadout: None,
        }
    }

    #[test]
    fn json_contains_schema_version() {
        let r = minimal_report();
        let s = render(&r).unwrap();
        assert!(s.contains("skilldigest-report/1"));
    }

    #[test]
    fn json_roundtrips_through_serde() {
        let r = minimal_report();
        let s = render(&r).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["schema_version"].as_str().unwrap(), crate::SCHEMA_VERSION);
    }

    #[test]
    fn compact_has_no_newlines_in_body() {
        let r = minimal_report();
        let s = render_compact(&r).unwrap();
        assert!(!s.contains('\n'));
    }

    #[test]
    fn issues_serialized_as_array() {
        let r = minimal_report();
        let s = render(&r).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert!(v["issues"].is_array());
        assert_eq!(v["issues"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn tool_version_included() {
        let r = minimal_report();
        let s = render(&r).unwrap();
        assert!(s.contains(crate::VERSION));
    }

    #[test]
    fn tokenizer_field_serialized() {
        let r = minimal_report();
        let s = render(&r).unwrap();
        assert!(s.contains("cl100k_base"));
    }
}
