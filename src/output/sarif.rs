//! SARIF 2.1.0 renderer.
//!
//! GitHub code-scanning is our primary consumer so we emit the shape that
//! `github/codeql-action/upload-sarif` accepts. We do not implement the
//! entire 2.1.0 surface — only the parts required to pass validation:
//!
//! - `version`
//! - `$schema`
//! - `runs[].tool.driver.{name, version, informationUri, rules[]}`
//! - `runs[].results[].{ruleId, ruleIndex, level, message, locations[]}`

use crate::error::{Error, Result};
use crate::model::{IssueKind, Report, Severity};

const SCHEMA_URI: &str =
    "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/main/Schemata/sarif-schema-2.1.0.json";

/// Render a report as SARIF 2.1.0 JSON.
pub fn render(report: &Report) -> Result<String> {
    let all_rules: Vec<IssueKind> = vec![
        IssueKind::Dead,
        IssueKind::Bloated,
        IssueKind::Conflict,
        IssueKind::Stale,
        IssueKind::Cycle,
        IssueKind::Oversize,
        IssueKind::NonUtf8,
        IssueKind::BadFrontmatter,
        IssueKind::Symlink,
        IssueKind::Duplicate,
        IssueKind::PathEscape,
        IssueKind::TotalBloated,
    ];
    let rule_descriptions: Vec<serde_json::Value> = all_rules
        .iter()
        .map(|k| {
            serde_json::json!({
                "id": k.rule_id(),
                "name": k.title(),
                "shortDescription": { "text": rule_short(*k) },
                "fullDescription": { "text": rule_full(*k) },
                "defaultConfiguration": {
                    "level": k.default_severity().as_sarif()
                },
                "helpUri": format!("https://github.com/JSLEEKR/skilldigest#{id}",
                                   id = k.rule_id().to_ascii_lowercase()),
            })
        })
        .collect();

    let mut results: Vec<serde_json::Value> = Vec::new();
    for issue in &report.issues {
        let rule_index = all_rules.iter().position(|k| *k == issue.kind).unwrap_or(0);
        let level = match issue.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        };
        let mut result = serde_json::json!({
            "ruleId": issue.kind.rule_id(),
            "ruleIndex": rule_index,
            "level": level,
            "message": { "text": issue.message },
        });
        if let Some(loc) = &issue.location {
            result["locations"] = serde_json::json!([
                {
                    "physicalLocation": {
                        "artifactLocation": {
                            "uri": loc.path.to_string_lossy().replace('\\', "/")
                        },
                        "region": {
                            "startLine": loc.line.max(1),
                            "startColumn": loc.column.max(1)
                        }
                    }
                }
            ]);
        }
        if !issue.related.is_empty() {
            let related: Vec<serde_json::Value> = issue
                .related
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "message": { "text": format!("related skill: {r}") }
                    })
                })
                .collect();
            result["relatedLocations"] = serde_json::Value::Array(related);
        }
        results.push(result);
    }

    let sarif = serde_json::json!({
        "$schema": SCHEMA_URI,
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "skilldigest",
                    "version": report.tool_version,
                    "informationUri": "https://github.com/JSLEEKR/skilldigest",
                    "rules": rule_descriptions,
                    "semanticVersion": report.tool_version,
                }
            },
            "results": results,
            "columnKind": "unicodeCodePoints",
            "properties": {
                "schema_version": report.schema_version,
                "tokenizer": report.tokenizer,
                "tokenizer_version": report.tokenizer_version,
                "total_skills": report.total_skills,
                "total_tokens": report.total_tokens,
            }
        }]
    });

    serde_json::to_string_pretty(&sarif).map_err(|e| Error::Other(anyhow::anyhow!("sarif: {e}")))
}

fn rule_short(k: IssueKind) -> &'static str {
    match k {
        IssueKind::Dead => "Skill is never referenced",
        IssueKind::Bloated => "Skill exceeds token budget",
        IssueKind::Conflict => "Two skills define conflicting rules",
        IssueKind::Stale => "Skill links a missing file",
        IssueKind::Cycle => "Cycle in skill reference graph",
        IssueKind::Oversize => "File larger than max-file-size",
        IssueKind::NonUtf8 => "File contained non-UTF-8 bytes",
        IssueKind::BadFrontmatter => "Frontmatter could not be parsed",
        IssueKind::Symlink => "Symlink skipped",
        IssueKind::Duplicate => "Duplicate skill identifier",
        IssueKind::PathEscape => "Path traversal outside scan root",
        IssueKind::TotalBloated => "Library total exceeds --total-budget",
    }
}

fn rule_full(k: IssueKind) -> &'static str {
    match k {
        IssueKind::Dead => "The skill is not referenced by any index file or any other skill in the library. Consider removing it.",
        IssueKind::Bloated => "The skill's token count exceeds the per-skill budget. Consider splitting it or raising the budget.",
        IssueKind::Conflict => "Two skills contain opposing rules (for example MUST X and MUST NOT X) for the same subject.",
        IssueKind::Stale => "A link or file reference inside the skill points to a file that does not exist on disk.",
        IssueKind::Cycle => "The skill reference graph contains a cycle. Break the cycle to avoid ambiguous loading.",
        IssueKind::Oversize => "A file exceeded the configured max-file-size and was not analyzed.",
        IssueKind::NonUtf8 => "The file contained bytes that could not be decoded as UTF-8 and was recovered with replacement characters.",
        IssueKind::BadFrontmatter => "The YAML frontmatter at the top of the file failed to parse. The rest of the file was still analyzed.",
        IssueKind::Symlink => "A symlink was skipped because --follow-symlinks was not set.",
        IssueKind::Duplicate => "Two or more files produced the same normalized skill identifier.",
        IssueKind::PathEscape => "A discovered file canonicalised to a path outside the scan root (typically via a symlink). The file was not analyzed. This is distinct from a routine 'symlink skipped' note and may indicate a misconfigured library layout or a malicious link.",
        IssueKind::TotalBloated => "The aggregate token count across every skill in the library exceeds the --total-budget (or `[budget] total` config-file) cap. Consider removing dead skills, raising the cap, or splitting the library.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BudgetConfig, Issue, IssueKind, Location, Report, SkillId};

    fn report_with_issue() -> Report {
        let issue = Issue::new(IssueKind::Bloated, SkillId::new("a"), "too big")
            .with_location(Location::start_of("a/SKILL.md"));
        Report {
            schema_version: crate::SCHEMA_VERSION,
            tokenizer: "cl100k_base".into(),
            tokenizer_version: "tiktoken-rs 0.7 cl100k_base".into(),
            tool_version: crate::VERSION,
            scan_root: ".".into(),
            total_skills: 1,
            total_tokens: 100,
            budget: BudgetConfig {
                per_skill: 2000,
                total: None,
            },
            skills: vec![],
            issues: vec![issue],
            loadout: None,
        }
    }

    #[test]
    fn sarif_has_version_and_schema() {
        let r = report_with_issue();
        let s = render(&r).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["version"].as_str().unwrap(), "2.1.0");
        assert!(v["$schema"].as_str().unwrap().contains("sarif"));
    }

    #[test]
    fn sarif_has_driver_name_and_version() {
        let r = report_with_issue();
        let s = render(&r).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let driver = &v["runs"][0]["tool"]["driver"];
        assert_eq!(driver["name"].as_str().unwrap(), "skilldigest");
        assert_eq!(driver["version"].as_str().unwrap(), crate::VERSION);
    }

    #[test]
    fn sarif_lists_all_rule_ids() {
        let r = report_with_issue();
        let s = render(&r).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let rules = v["runs"][0]["tool"]["driver"]["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 12);
        let ids: Vec<&str> = rules.iter().map(|r| r["id"].as_str().unwrap()).collect();
        assert!(ids.contains(&"SKILL001"));
        assert!(ids.contains(&"SKILL010"));
        assert!(ids.contains(&"SKILL011"));
        assert!(ids.contains(&"SKILL012"));
    }

    #[test]
    fn sarif_result_has_location() {
        let r = report_with_issue();
        let s = render(&r).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let result = &v["runs"][0]["results"][0];
        assert_eq!(result["ruleId"].as_str().unwrap(), "SKILL002");
        assert_eq!(result["level"].as_str().unwrap(), "error");
        let loc = &result["locations"][0]["physicalLocation"];
        assert_eq!(
            loc["artifactLocation"]["uri"].as_str().unwrap(),
            "a/SKILL.md"
        );
    }

    #[test]
    fn sarif_empty_report_valid() {
        let r = Report {
            schema_version: crate::SCHEMA_VERSION,
            tokenizer: "cl100k_base".into(),
            tokenizer_version: "tiktoken-rs 0.7 cl100k_base".into(),
            tool_version: crate::VERSION,
            scan_root: ".".into(),
            total_skills: 0,
            total_tokens: 0,
            budget: BudgetConfig {
                per_skill: 2000,
                total: None,
            },
            skills: vec![],
            issues: vec![],
            loadout: None,
        };
        let s = render(&r).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["runs"][0]["results"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn sarif_properties_carry_metadata() {
        let r = report_with_issue();
        let s = render(&r).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let props = &v["runs"][0]["properties"];
        assert_eq!(
            props["schema_version"].as_str().unwrap(),
            crate::SCHEMA_VERSION
        );
        assert_eq!(props["tokenizer"].as_str().unwrap(), "cl100k_base");
    }

    #[test]
    fn sarif_related_locations_included() {
        let issue = Issue::new(IssueKind::Conflict, SkillId::new("a"), "conflict")
            .with_related(vec![SkillId::new("b")]);
        let r = Report {
            schema_version: crate::SCHEMA_VERSION,
            tokenizer: "cl100k".into(),
            tokenizer_version: "tiktoken-rs 0.7 cl100k".into(),
            tool_version: crate::VERSION,
            scan_root: ".".into(),
            total_skills: 2,
            total_tokens: 0,
            budget: BudgetConfig {
                per_skill: 2000,
                total: None,
            },
            skills: vec![],
            issues: vec![issue],
            loadout: None,
        };
        let s = render(&r).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let related = &v["runs"][0]["results"][0]["relatedLocations"];
        assert!(related.is_array());
        assert!(related[0]["message"]["text"]
            .as_str()
            .unwrap()
            .contains("b"));
    }

    #[test]
    fn rule_full_text_nonempty() {
        for k in [
            IssueKind::Dead,
            IssueKind::Bloated,
            IssueKind::Conflict,
            IssueKind::Stale,
            IssueKind::Cycle,
            IssueKind::Oversize,
            IssueKind::NonUtf8,
            IssueKind::BadFrontmatter,
            IssueKind::Symlink,
            IssueKind::Duplicate,
            IssueKind::PathEscape,
            IssueKind::TotalBloated,
        ] {
            assert!(!rule_short(k).is_empty());
            assert!(!rule_full(k).is_empty());
        }
    }
}
