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
            // SARIF 2.1.0 §3.4.4 requires `artifactLocation.uri` to be a
            // valid URI Reference per RFC 3986. Path components that contain
            // a space, non-ASCII byte, or any of the URI reserved/control
            // characters must be percent-encoded — without that step a path
            // like `my dir/SKILL.md` or `한글/SKILL.md` produced invalid URIs
            // that GitHub code-scanning's SARIF validator rejects with
            // "value does not match URI format". `/` is preserved as the
            // path separator. `\` is normalised to `/` first so Windows-style
            // paths land in the same shape.
            let raw = loc.path.to_string_lossy().replace('\\', "/");
            let uri = encode_uri_path(&raw);
            result["locations"] = serde_json::json!([
                {
                    "physicalLocation": {
                        "artifactLocation": {
                            "uri": uri
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

/// Percent-encode a forward-slash-separated path so the result is a valid
/// RFC 3986 URI Reference (relative-path form). Each path segment is
/// independently encoded with the `pchar` rule (unreserved + sub-delims +
/// `:` + `@`), and `/` separators are preserved untouched. Non-ASCII bytes
/// are percent-encoded one byte at a time after UTF-8 expansion. ASCII
/// control bytes (< 0x20) and DEL (0x7F) are also percent-encoded.
///
/// Why this matters: GitHub code-scanning's SARIF validator (and the OASIS
/// reference checker) rejects `artifactLocation.uri` values that contain
/// raw spaces, non-ASCII bytes, or control characters. A skill library
/// hosted under a directory like `Skill Library/SKILL.md` or `한글/SKILL.md`
/// would otherwise upload as invalid SARIF and never surface in the PR
/// review UI.
fn encode_uri_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for (i, segment) in path.split('/').enumerate() {
        if i > 0 {
            out.push('/');
        }
        for byte in segment.bytes() {
            if is_uri_pchar_unreserved(byte) {
                out.push(byte as char);
            } else {
                use std::fmt::Write;
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

/// True when `byte` is allowed unencoded in an RFC 3986 path segment.
/// Subset chosen to match the `pchar` production minus the percent sign
/// (which is itself percent-encoded if it appears literally in a path).
fn is_uri_pchar_unreserved(byte: u8) -> bool {
    matches!(byte,
        b'A'..=b'Z'
        | b'a'..=b'z'
        | b'0'..=b'9'
        | b'-' | b'.' | b'_' | b'~'    // unreserved
        | b'!' | b'$' | b'&' | b'\'' | b'(' | b')'
        | b'*' | b'+' | b',' | b';' | b'='   // sub-delims
        | b':' | b'@'                  // pchar extras
    )
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
    fn sarif_uri_percent_encodes_spaces() {
        // SARIF 2.1.0 §3.4.4 requires the artifactLocation.uri to be a valid
        // RFC 3986 URI Reference. A literal space is illegal in a URI and
        // GitHub code-scanning's SARIF validator rejects it. The encoder
        // must convert " " to "%20" while preserving "/" as the path
        // separator.
        let issue = Issue::new(IssueKind::Bloated, SkillId::new("a"), "too big")
            .with_location(Location::start_of("my dir/SKILL.md"));
        let r = Report {
            schema_version: crate::SCHEMA_VERSION,
            tokenizer: "cl100k".into(),
            tokenizer_version: "tiktoken-rs 0.7 cl100k".into(),
            tool_version: crate::VERSION,
            scan_root: ".".into(),
            total_skills: 1,
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
        let uri = v["runs"][0]["results"][0]["locations"][0]["physicalLocation"]
            ["artifactLocation"]["uri"]
            .as_str()
            .unwrap();
        assert_eq!(
            uri, "my%20dir/SKILL.md",
            "spaces must be %20-encoded; got {uri:?}"
        );
        assert!(
            !uri.contains(' '),
            "raw space leaked into SARIF uri: {uri:?}"
        );
    }

    #[test]
    fn sarif_uri_percent_encodes_non_ascii() {
        // Non-ASCII path segments (Korean, Japanese, etc.) must be
        // percent-encoded byte-by-byte after UTF-8 expansion. Without this
        // the resulting URI fails RFC 3986 validation and SARIF readers
        // either reject the file or strip the location entirely.
        let issue = Issue::new(IssueKind::Dead, SkillId::new("k"), "unused")
            .with_location(Location::start_of("한글/SKILL.md"));
        let r = Report {
            schema_version: crate::SCHEMA_VERSION,
            tokenizer: "cl100k".into(),
            tokenizer_version: "tiktoken-rs 0.7 cl100k".into(),
            tool_version: crate::VERSION,
            scan_root: ".".into(),
            total_skills: 1,
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
        let uri = v["runs"][0]["results"][0]["locations"][0]["physicalLocation"]
            ["artifactLocation"]["uri"]
            .as_str()
            .unwrap();
        // Korean "한글" is U+D55C U+AE00 → UTF-8: EC 9D 9C EA B8 80
        assert_eq!(
            uri, "%ED%95%9C%EA%B8%80/SKILL.md",
            "non-ASCII bytes must be %-encoded; got {uri:?}"
        );
        for b in uri.bytes() {
            assert!(
                b.is_ascii() && b >= 0x20 && b != 0x7F,
                "non-ASCII or control byte leaked into SARIF uri: {uri:?}"
            );
        }
    }

    #[test]
    fn sarif_uri_preserves_slash_separator() {
        // `/` is a path separator and must NOT be percent-encoded. Plain
        // ASCII path segments must round-trip without decoration.
        let issue = Issue::new(IssueKind::Stale, SkillId::new("git/commit"), "x")
            .with_location(Location::start_of("git/commit/SKILL.md"));
        let r = Report {
            schema_version: crate::SCHEMA_VERSION,
            tokenizer: "cl100k".into(),
            tokenizer_version: "tiktoken-rs 0.7 cl100k".into(),
            tool_version: crate::VERSION,
            scan_root: ".".into(),
            total_skills: 1,
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
        let uri = v["runs"][0]["results"][0]["locations"][0]["physicalLocation"]
            ["artifactLocation"]["uri"]
            .as_str()
            .unwrap();
        assert_eq!(uri, "git/commit/SKILL.md");
    }

    #[test]
    fn encode_uri_path_basic_invariants() {
        assert_eq!(encode_uri_path(""), "");
        assert_eq!(encode_uri_path("a"), "a");
        assert_eq!(encode_uri_path("a/b/c"), "a/b/c");
        assert_eq!(encode_uri_path("a b"), "a%20b");
        assert_eq!(encode_uri_path("a?b"), "a%3Fb");
        assert_eq!(encode_uri_path("a#b"), "a%23b");
        // CRLF / control characters must be encoded.
        assert_eq!(encode_uri_path("a\tb"), "a%09b");
        assert_eq!(encode_uri_path("a\nb"), "a%0Ab");
        // Percent itself must be re-encoded (it is not in the unreserved set).
        assert_eq!(encode_uri_path("a%b"), "a%25b");
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
