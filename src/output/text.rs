//! Plain-text renderer.

use crate::model::{Report, Severity};

const RESET: &str = "\x1b[0m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const GREEN: &str = "\x1b[32m";
const DIM: &str = "\x1b[2m";

/// Render a scan report as text. When `no_color` is true, ANSI escape
/// sequences are omitted.
#[must_use]
pub fn render(report: &Report, no_color: bool) -> String {
    let mut out = String::new();
    let (err, warn, note) = report.severity_counts();

    let header = format!(
        "skilldigest {}  {}  {}\n",
        report.tool_version,
        report.tokenizer,
        report.scan_root.display()
    );
    out.push_str(&header);
    let summary = format!(
        "  {} skills  {} tokens  {} issues ({} error, {} warning, {} note)\n\n",
        report.total_skills,
        report.total_tokens,
        report.issues.len(),
        err,
        warn,
        note,
    );
    out.push_str(&summary);

    for issue in &report.issues {
        let level = match issue.severity {
            Severity::Error => colorize("ERROR", RED, no_color),
            Severity::Warning => colorize("WARN ", YELLOW, no_color),
            Severity::Note => colorize("NOTE ", GREEN, no_color),
        };
        let location = issue
            .location
            .as_ref()
            .map(|l| format!("{}:{}:{}", l.path.display(), l.line, l.column))
            .unwrap_or_else(|| issue.skill.to_string());
        out.push_str(&format!(
            "{} {:<9} {} — {}\n",
            level,
            issue.kind.title(),
            colorize(location.as_str(), DIM, no_color),
            issue.message,
        ));
    }

    if let Some(loadout) = &report.loadout {
        out.push_str("\nLoadout\n");
        out.push_str(&format!(
            "  tag={} budget={} selected={} tokens_used={}\n",
            loadout.tag,
            loadout.max_tokens,
            loadout.skills.len(),
            loadout.total_tokens
        ));
        for id in &loadout.skills {
            out.push_str(&format!("   - {id}\n"));
        }
    }

    out
}

fn colorize(s: &str, color: &str, no_color: bool) -> String {
    if no_color {
        s.to_string()
    } else {
        format!("{color}{s}{RESET}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BudgetConfig, Issue, IssueKind, Location, Report, SkillId};

    fn report_with_issue() -> Report {
        let issue = Issue::new(IssueKind::Dead, SkillId::new("a"), "unused")
            .with_location(Location::start_of("a/SKILL.md"));
        Report {
            schema_version: crate::SCHEMA_VERSION,
            tokenizer: "cl100k".to_string(),
            tokenizer_version: "tiktoken-rs 0.7 cl100k".to_string(),
            tool_version: crate::VERSION,
            scan_root: std::path::PathBuf::from("."),
            total_skills: 1,
            total_tokens: 42,
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
    fn header_includes_version_and_tokenizer() {
        let r = report_with_issue();
        let s = render(&r, true);
        assert!(s.contains(crate::VERSION));
        assert!(s.contains("cl100k"));
    }

    #[test]
    fn severity_counts_displayed() {
        let r = report_with_issue();
        let s = render(&r, true);
        assert!(s.contains("0 error"));
        assert!(s.contains("1 warning"));
    }

    #[test]
    fn no_color_strips_ansi() {
        let r = report_with_issue();
        let s = render(&r, true);
        assert!(!s.contains("\x1b["));
    }

    #[test]
    fn color_adds_ansi() {
        let r = report_with_issue();
        let s = render(&r, false);
        assert!(s.contains("\x1b["));
    }

    #[test]
    fn loadout_block_printed() {
        let mut r = report_with_issue();
        r.loadout = Some(crate::model::Loadout {
            tag: "git".into(),
            max_tokens: 1000,
            skills: vec![SkillId::new("a")],
            total_tokens: 100,
        });
        let s = render(&r, true);
        assert!(s.contains("Loadout"));
        assert!(s.contains("tag=git"));
        assert!(s.contains("- a"));
    }

    #[test]
    fn empty_report_renders_clean() {
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
        let s = render(&r, true);
        assert!(s.contains("0 skills"));
    }
}
