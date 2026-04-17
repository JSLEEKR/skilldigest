//! High-level audit rules.
//!
//! Each rule function takes the parsed skills (+ graph) and emits issues.
//! Rules are pure — they never read the filesystem — so they're trivially
//! testable in isolation.

use std::collections::BTreeMap;

use crate::graph::SkillGraph;
use crate::model::{
    BudgetConfig, Frontmatter, Issue, IssueKind, Location, Skill, SkillId, SkillRef,
};

/// Emit [`IssueKind::Bloated`] for every skill whose total token count
/// exceeds its effective budget.
#[must_use]
pub fn bloated(skills: &[Skill], budget: &BudgetConfig) -> Vec<Issue> {
    let mut out = Vec::new();
    for skill in skills {
        let per = effective_budget(&skill.frontmatter, budget);
        if skill.tokens.total > per {
            out.push(
                Issue::new(
                    IssueKind::Bloated,
                    skill.id.clone(),
                    format!(
                        "{} tokens exceeds budget {} (frontmatter {} + body {})",
                        skill.tokens.total, per, skill.tokens.frontmatter, skill.tokens.body
                    ),
                )
                .with_location(Location::start_of(skill.path.clone())),
            );
        }
    }
    out
}

/// Effective per-skill budget: the frontmatter override if set, else the
/// global per-skill budget.
#[must_use]
pub fn effective_budget(f: &Frontmatter, budget: &BudgetConfig) -> usize {
    f.budget.unwrap_or(budget.per_skill)
}

/// Emit [`IssueKind::TotalBloated`] when the aggregate token count across
/// the library exceeds the configured `--total-budget` cap.
///
/// This rule is only active when `budget.total` is `Some(_)` — users who do
/// not set a total cap get the same zero-noise behaviour as before.
///
/// The issue attaches to the synthetic skill id `<library>` so SARIF
/// consumers that group diagnostics by skill get a stable, non-colliding
/// key. We pick the skill with the highest token cost as the primary
/// location so the rendered `location.path` points at a concrete file (the
/// "worst offender") rather than an empty or ambiguous path.
#[must_use]
pub fn total_bloated(skills: &[Skill], budget: &BudgetConfig) -> Vec<Issue> {
    let Some(cap) = budget.total else {
        return Vec::new();
    };
    let total: usize = skills.iter().map(|s| s.tokens.total).sum();
    if total <= cap {
        return Vec::new();
    }

    // Pick the heaviest skill as the primary location. Ties broken by skill
    // id for determinism.
    let worst = skills
        .iter()
        .max_by(|a, b| {
            a.tokens.total.cmp(&b.tokens.total).then(b.id.cmp(&a.id)) // reverse id sort so smallest id wins ties
        })
        .cloned();

    let mut issue = Issue::new(
        IssueKind::TotalBloated,
        SkillId::new("<library>"),
        format!(
            "library total {total} tokens exceeds --total-budget {cap} across {} skills",
            skills.len()
        ),
    );
    if let Some(w) = worst {
        issue = issue
            .with_location(Location::start_of(w.path.clone()))
            .with_related(vec![w.id]);
    }
    vec![issue]
}

/// Conflict detection: two skills that define rules on the same subject
/// with opposing modals.
#[must_use]
pub fn conflicts(skills: &[Skill]) -> Vec<Issue> {
    let mut out = Vec::new();
    for i in 0..skills.len() {
        for j in (i + 1)..skills.len() {
            let a = &skills[i];
            let b = &skills[j];
            for ra in &a.rules {
                for rb in &b.rules {
                    if normalise_subject(&ra.subject) == normalise_subject(&rb.subject)
                        && ra.modal.conflicts_with(rb.modal)
                    {
                        out.push(
                            Issue::new(
                                IssueKind::Conflict,
                                a.id.clone(),
                                format!(
                                    "conflicting rules for '{}': {} says \"{}\" (line {}), {} says \"{}\" (line {})",
                                    ra.subject,
                                    a.id,
                                    ra.raw,
                                    ra.line,
                                    b.id,
                                    rb.raw,
                                    rb.line,
                                ),
                            )
                            .with_location(Location {
                                path: a.path.clone(),
                                line: ra.line.max(1),
                                column: 1,
                            })
                            .with_related(vec![b.id.clone()]),
                        );
                    }
                }
            }
        }
    }
    out
}

fn normalise_subject(s: &str) -> String {
    s.trim()
        .trim_matches(|c: char| matches!(c, '.' | ',' | '`' | '"' | '\'' | ';' | ':'))
        .to_ascii_lowercase()
}

/// Stale-file detection: for every [`SkillRef::File`] or [`SkillRef::Link`]
/// whose `exists` field is `false`, emit an issue.
#[must_use]
pub fn stale(skills: &[Skill]) -> Vec<Issue> {
    let mut out = Vec::new();
    for skill in skills {
        for r in &skill.refs {
            match r {
                SkillRef::Link { target, exists } if !exists => {
                    out.push(
                        Issue::new(
                            IssueKind::Stale,
                            skill.id.clone(),
                            format!("broken link to '{}'", target.display()),
                        )
                        .with_location(Location::start_of(skill.path.clone())),
                    );
                }
                SkillRef::File { path, exists } if !exists => {
                    out.push(
                        Issue::new(
                            IssueKind::Stale,
                            skill.id.clone(),
                            format!("missing file '{}'", path.display()),
                        )
                        .with_location(Location::start_of(skill.path.clone())),
                    );
                }
                _ => {}
            }
        }
    }
    out
}

/// Duplicate-ID detection.
#[must_use]
pub fn duplicates(skills: &[Skill]) -> Vec<Issue> {
    let mut by_id: BTreeMap<&SkillId, Vec<&Skill>> = BTreeMap::new();
    for s in skills {
        by_id.entry(&s.id).or_default().push(s);
    }
    let mut out = Vec::new();
    for (id, group) in by_id {
        if group.len() > 1 {
            let related: Vec<SkillId> = group.iter().skip(1).map(|s| s.id.clone()).collect();
            out.push(
                Issue::new(
                    IssueKind::Duplicate,
                    id.clone(),
                    format!("duplicate skill id '{}' in {} files", id, group.len()),
                )
                .with_location(Location::start_of(group[0].path.clone()))
                .with_related(related),
            );
        }
    }
    out
}

/// Emit warning-only issues for parse-time problems.
#[must_use]
pub fn from_parse_warnings(skills: &[Skill]) -> Vec<Issue> {
    let mut out = Vec::new();
    for skill in skills {
        for w in &skill.warnings {
            match w.kind {
                crate::model::WarningKind::FrontmatterYamlError => {
                    out.push(
                        Issue::new(
                            IssueKind::BadFrontmatter,
                            skill.id.clone(),
                            w.message.clone(),
                        )
                        .with_location(Location::start_of(skill.path.clone())),
                    );
                }
                crate::model::WarningKind::NonUtf8Recovered => {
                    out.push(
                        Issue::new(IssueKind::NonUtf8, skill.id.clone(), w.message.clone())
                            .with_location(Location::start_of(skill.path.clone())),
                    );
                }
                _ => {}
            }
        }
    }
    out
}

/// Compose all audits and return a canonical sorted list.
#[must_use]
pub fn run_all(skills: &[Skill], graph: &SkillGraph, budget: &BudgetConfig) -> Vec<Issue> {
    let mut issues = Vec::new();
    issues.extend(bloated(skills, budget));
    issues.extend(total_bloated(skills, budget));
    issues.extend(conflicts(skills));
    issues.extend(stale(skills));
    issues.extend(duplicates(skills));
    issues.extend(graph.dead_skills(skills));
    issues.extend(graph.cycles(skills));
    issues.extend(from_parse_warnings(skills));
    sort_canonical(&mut issues);
    issues
}

fn sort_canonical(issues: &mut [Issue]) {
    issues.sort_by(|a, b| {
        // Higher severity first.
        b.severity
            .cmp(&a.severity)
            .then(a.kind.cmp(&b.kind))
            .then(a.skill.cmp(&b.skill))
            .then(a.message.cmp(&b.message))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Frontmatter, Modal, Rule, RuleKind, Skill, SkillId, TokenCounts};

    fn mk_skill(id: &str, tokens: usize) -> Skill {
        Skill {
            id: SkillId::new(id),
            name: id.into(),
            path: format!("{id}/SKILL.md").into(),
            frontmatter: Frontmatter::default(),
            tokens: TokenCounts::new(0, tokens),
            refs: vec![],
            rules: vec![],
            tags: vec![],
            warnings: vec![],
            body_bytes: 0,
        }
    }

    #[test]
    fn bloated_detected_at_default_budget() {
        let mut a = mk_skill("a", 3000);
        a.tokens = TokenCounts::new(0, 3000);
        let b = mk_skill("b", 100);
        let budget = BudgetConfig {
            per_skill: 2000,
            total: None,
        };
        let out = bloated(&[a, b], &budget);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, IssueKind::Bloated);
    }

    #[test]
    fn bloated_respects_frontmatter_override() {
        let mut a = mk_skill("a", 3000);
        a.tokens = TokenCounts::new(0, 3000);
        a.frontmatter.budget = Some(5000);
        let budget = BudgetConfig {
            per_skill: 2000,
            total: None,
        };
        let out = bloated(&[a], &budget);
        assert!(out.is_empty());
    }

    #[test]
    fn effective_budget_default() {
        let f = Frontmatter::default();
        let b = BudgetConfig {
            per_skill: 2000,
            total: None,
        };
        assert_eq!(effective_budget(&f, &b), 2000);
    }

    #[test]
    fn effective_budget_override() {
        let f = Frontmatter {
            budget: Some(3000),
            ..Frontmatter::default()
        };
        let b = BudgetConfig {
            per_skill: 2000,
            total: None,
        };
        assert_eq!(effective_budget(&f, &b), 3000);
    }

    #[test]
    fn conflict_detected_between_opposing_rules() {
        let mut a = mk_skill("a", 10);
        a.rules.push(Rule {
            kind: RuleKind::AlwaysUse,
            subject: "Bash(ls)".into(),
            modal: Modal::Must,
            raw: "MUST use Bash(ls)".into(),
            line: 1,
        });
        let mut b = mk_skill("b", 10);
        b.rules.push(Rule {
            kind: RuleKind::NeverUse,
            subject: "Bash(ls)".into(),
            modal: Modal::MustNot,
            raw: "NEVER Bash(ls)".into(),
            line: 2,
        });
        let out = conflicts(&[a, b]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, IssueKind::Conflict);
    }

    #[test]
    fn conflict_ignores_same_direction() {
        let mut a = mk_skill("a", 10);
        a.rules.push(Rule {
            kind: RuleKind::AlwaysUse,
            subject: "Bash(ls)".into(),
            modal: Modal::Must,
            raw: "MUST".into(),
            line: 1,
        });
        let mut b = mk_skill("b", 10);
        b.rules.push(Rule {
            kind: RuleKind::AlwaysUse,
            subject: "Bash(ls)".into(),
            modal: Modal::Must,
            raw: "MUST".into(),
            line: 1,
        });
        assert!(conflicts(&[a, b]).is_empty());
    }

    #[test]
    fn conflict_handles_case_and_punctuation() {
        let mut a = mk_skill("a", 10);
        a.rules.push(Rule {
            kind: RuleKind::AlwaysUse,
            subject: "Bash(ls).".into(),
            modal: Modal::Must,
            raw: "MUST".into(),
            line: 1,
        });
        let mut b = mk_skill("b", 10);
        b.rules.push(Rule {
            kind: RuleKind::NeverUse,
            subject: "bash(ls)".into(),
            modal: Modal::MustNot,
            raw: "NEVER".into(),
            line: 1,
        });
        let out = conflicts(&[a, b]);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn stale_detects_broken_links() {
        let mut a = mk_skill("a", 10);
        a.refs.push(SkillRef::Link {
            target: "missing.md".into(),
            exists: false,
        });
        let out = stale(&[a]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, IssueKind::Stale);
    }

    #[test]
    fn stale_ignores_existing_links() {
        let mut a = mk_skill("a", 10);
        a.refs.push(SkillRef::Link {
            target: "there.md".into(),
            exists: true,
        });
        assert!(stale(&[a]).is_empty());
    }

    #[test]
    fn duplicates_detected() {
        let a = mk_skill("same", 10);
        let b = mk_skill("same", 10);
        let out = duplicates(&[a, b]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, IssueKind::Duplicate);
    }

    #[test]
    fn duplicates_allow_unique_ids() {
        let a = mk_skill("one", 10);
        let b = mk_skill("two", 10);
        assert!(duplicates(&[a, b]).is_empty());
    }

    #[test]
    fn bad_frontmatter_becomes_issue() {
        let mut a = mk_skill("a", 10);
        a.warnings.push(crate::model::Warning {
            kind: crate::model::WarningKind::FrontmatterYamlError,
            message: "oh".into(),
        });
        let out = from_parse_warnings(&[a]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, IssueKind::BadFrontmatter);
    }

    #[test]
    fn non_utf8_becomes_issue() {
        let mut a = mk_skill("a", 10);
        a.warnings.push(crate::model::Warning {
            kind: crate::model::WarningKind::NonUtf8Recovered,
            message: "oh".into(),
        });
        let out = from_parse_warnings(&[a]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, IssueKind::NonUtf8);
    }

    #[test]
    fn sort_canonical_puts_errors_first() {
        let a = Issue::new(IssueKind::Dead, SkillId::new("a"), "x"); // warning
        let b = Issue::new(IssueKind::Bloated, SkillId::new("b"), "y"); // error
        let mut v = vec![a, b];
        sort_canonical(&mut v);
        assert_eq!(v[0].kind, IssueKind::Bloated);
    }

    #[test]
    fn run_all_composes_rules() {
        let mut a = mk_skill("a", 10);
        a.tokens = TokenCounts::new(0, 3000);
        a.refs.push(SkillRef::Link {
            target: "missing.md".into(),
            exists: false,
        });
        let graph = crate::graph::SkillGraph::build(std::slice::from_ref(&a));
        let budget = BudgetConfig {
            per_skill: 1000,
            total: None,
        };
        let issues = run_all(&[a], &graph, &budget);
        // bloated + stale + dead (no in-edges)
        let kinds: std::collections::BTreeSet<_> = issues.iter().map(|i| i.kind).collect();
        assert!(kinds.contains(&IssueKind::Bloated));
        assert!(kinds.contains(&IssueKind::Stale));
        assert!(kinds.contains(&IssueKind::Dead));
    }

    #[test]
    fn normalise_subject_strips_trailing_dot() {
        assert_eq!(normalise_subject("Bash(ls)."), "bash(ls)");
    }

    #[test]
    fn normalise_subject_strips_quotes() {
        assert_eq!(normalise_subject("\"Rm\""), "rm");
    }

    #[test]
    fn total_bloated_none_when_cap_absent() {
        let mut a = mk_skill("a", 1000);
        a.tokens = TokenCounts::new(0, 1000);
        let budget = BudgetConfig {
            per_skill: 10_000,
            total: None,
        };
        assert!(total_bloated(&[a], &budget).is_empty());
    }

    #[test]
    fn total_bloated_none_when_under_cap() {
        let mut a = mk_skill("a", 100);
        a.tokens = TokenCounts::new(0, 100);
        let budget = BudgetConfig {
            per_skill: 10_000,
            total: Some(200),
        };
        assert!(total_bloated(&[a], &budget).is_empty());
    }

    #[test]
    fn total_bloated_emits_when_over_cap() {
        let mut a = mk_skill("a", 500);
        a.tokens = TokenCounts::new(0, 500);
        let mut b = mk_skill("b", 700);
        b.tokens = TokenCounts::new(0, 700);
        let budget = BudgetConfig {
            per_skill: 10_000,
            total: Some(1000),
        };
        let out = total_bloated(&[a, b], &budget);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, IssueKind::TotalBloated);
        // Worst offender should be surfaced in the related list.
        assert_eq!(out[0].related[0].as_str(), "b");
    }
}
