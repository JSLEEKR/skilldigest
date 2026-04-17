//! Markdown renderer for PR comments.

use std::fmt::Write;

use crate::model::{Report, Severity};

/// Render a report as GitHub-friendly Markdown.
#[must_use]
pub fn render(report: &Report) -> String {
    let (err, warn, note) = report.severity_counts();
    let mut out = String::new();
    let _ = writeln!(out, "### skilldigest report");
    let _ = writeln!(
        out,
        "**{} skills**, **{} tokens** ({}), **{} issues** ({} error, {} warning, {} note)",
        report.total_skills,
        report.total_tokens,
        report.tokenizer,
        report.issues.len(),
        err,
        warn,
        note
    );
    let _ = writeln!(out);

    if !report.skills.is_empty() {
        let _ = writeln!(out, "| Skill | Tokens | Issues |");
        let _ = writeln!(out, "|-------|-------:|--------|");
        let mut rows: Vec<_> = report.skills.iter().collect();
        rows.sort_by(|a, b| b.tokens.total.cmp(&a.tokens.total).then(a.id.cmp(&b.id)));
        for row in rows.iter().take(50) {
            let kinds = if row.issue_kinds.is_empty() {
                "—".to_string()
            } else {
                row.issue_kinds
                    .iter()
                    .map(|k| k.title())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let _ = writeln!(out, "| `{}` | {} | {} |", row.id, row.tokens.total, kinds);
        }
        if report.skills.len() > 50 {
            let _ = writeln!(out, "\n_...and {} more skills_", report.skills.len() - 50);
        }
    }

    if !report.issues.is_empty() {
        let _ = writeln!(out, "\n#### Issues\n");
        for issue in &report.issues {
            let icon = match issue.severity {
                Severity::Error => "[ERROR]",
                Severity::Warning => "[warn]",
                Severity::Note => "[note]",
            };
            let path = issue
                .location
                .as_ref()
                .map(|l| format!("`{}:{}`", l.path.display(), l.line))
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "- {} **{}** `{}` {} — {}",
                icon,
                issue.kind.title(),
                issue.skill,
                path,
                issue.message,
            );
        }
    }

    if let Some(loadout) = &report.loadout {
        let _ = writeln!(out, "\n#### Loadout for `{}`\n", loadout.tag);
        let _ = writeln!(
            out,
            "_budget: {} tokens; selected: {} skills; used: {} tokens_\n",
            loadout.max_tokens,
            loadout.skills.len(),
            loadout.total_tokens
        );
        for id in &loadout.skills {
            let _ = writeln!(out, "- `{id}`");
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        BudgetConfig, Issue, IssueKind, Location, Report, SkillId, SkillSummary, TokenCounts,
    };

    fn report() -> Report {
        Report {
            schema_version: crate::SCHEMA_VERSION,
            tokenizer: "cl100k".into(),
            tokenizer_version: "tiktoken-rs 0.7 cl100k".into(),
            tool_version: crate::VERSION,
            scan_root: ".".into(),
            total_skills: 1,
            total_tokens: 100,
            budget: BudgetConfig {
                per_skill: 2000,
                total: None,
            },
            skills: vec![SkillSummary {
                id: SkillId::new("a"),
                name: "a".into(),
                path: "a/SKILL.md".into(),
                tokens: TokenCounts::new(0, 100),
                tags: vec![],
                refs_out: 0,
                refs_in: 0,
                issue_kinds: vec![IssueKind::Dead],
            }],
            issues: vec![
                Issue::new(IssueKind::Dead, SkillId::new("a"), "never referenced")
                    .with_location(Location::start_of("a/SKILL.md")),
            ],
            loadout: None,
        }
    }

    #[test]
    fn markdown_has_header() {
        let s = render(&report());
        assert!(s.contains("### skilldigest report"));
    }

    #[test]
    fn markdown_has_table_rows() {
        let s = render(&report());
        assert!(s.contains("| Skill"));
        assert!(s.contains("`a`"));
    }

    #[test]
    fn markdown_issues_section_present() {
        let s = render(&report());
        assert!(s.contains("#### Issues"));
        assert!(s.contains("never referenced"));
    }

    #[test]
    fn markdown_counts_displayed() {
        let s = render(&report());
        assert!(s.contains("1 skills"));
        assert!(s.contains("1 warning"));
    }

    #[test]
    fn markdown_loadout_appended() {
        let mut r = report();
        r.loadout = Some(crate::model::Loadout {
            tag: "git".into(),
            max_tokens: 1000,
            skills: vec![SkillId::new("a")],
            total_tokens: 100,
        });
        let s = render(&r);
        assert!(s.contains("Loadout for `git`"));
        assert!(s.contains("- `a`"));
    }

    #[test]
    fn markdown_empty_report_is_valid() {
        let r = Report {
            schema_version: crate::SCHEMA_VERSION,
            tokenizer: "cl100k".into(),
            tokenizer_version: "tiktoken-rs 0.7 cl100k".into(),
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
        let s = render(&r);
        assert!(s.contains("0 skills"));
        assert!(!s.contains("Issues"));
    }

    #[test]
    fn markdown_truncates_long_tables() {
        let mut r = report();
        r.skills = (0..60)
            .map(|i| SkillSummary {
                id: SkillId::new(format!("skill-{i:03}")),
                name: format!("skill {i}"),
                path: format!("skill-{i:03}/SKILL.md").into(),
                tokens: TokenCounts::new(0, 100),
                tags: vec![],
                refs_out: 0,
                refs_in: 0,
                issue_kinds: vec![],
            })
            .collect();
        let s = render(&r);
        assert!(s.contains("and 10 more skills"));
    }
}
