//! High-level audit orchestration.
//!
//! Given a scan root and configuration, produce a [`Report`] by running
//! scan → parse → tokenize → graph → rules.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rayon::prelude::*;

use crate::config::ConfigDoc;
use crate::error::Result;
use crate::graph::SkillGraph;
use crate::model::{
    BudgetConfig, Issue, IssueKind, Location, Report, Skill, SkillRef, SkillSummary,
};
use crate::parse::{self, ParsedSkill};
use crate::rules;
use crate::scan::{self, ScanPolicy};
use crate::tokenize::Tokenizer;

/// Options for a full audit run.
pub struct AuditOptions {
    /// Scan root.
    pub root: PathBuf,
    /// Tokenizer to use.
    pub tokenizer: Arc<dyn Tokenizer>,
    /// Budget configuration.
    pub budget: BudgetConfig,
    /// Scan policy.
    pub policy: ScanPolicy,
    /// Per-skill budget overrides.
    pub overrides: BTreeMap<String, Option<usize>>,
}

impl std::fmt::Debug for AuditOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuditOptions")
            .field("root", &self.root)
            .field("tokenizer", &self.tokenizer.name())
            .field("budget", &self.budget)
            .field("policy", &self.policy)
            .field("overrides", &self.overrides)
            .finish()
    }
}

impl AuditOptions {
    /// Merge a config doc into the audit options (CLI still wins).
    pub fn apply_config(&mut self, doc: &ConfigDoc) {
        for g in &doc.ignore.globs {
            self.policy.ignore_globs.push(g.clone());
        }
        // budget/tokenizer are applied by the CLI layer before this — we do
        // pick up the per-skill overrides however.
        for (k, v) in &doc.overrides {
            self.overrides.insert(k.clone(), v.budget);
        }
    }
}

/// Run a full audit.
pub fn run(options: AuditOptions) -> Result<Report> {
    let (report, _skills) = run_inner(options)?;
    Ok(report)
}

/// Internal helper that runs the full audit pipeline and returns both the
/// public-facing [`Report`] and the underlying [`Vec<Skill>`].
///
/// `run_with_loadout` needs the full `Skill` objects (specifically
/// `frontmatter.description`) to drive the loadout scorer; building a fresh
/// `Skill` from a `SkillSummary` after the fact loses every `Frontmatter`
/// field, so the description-based scoring branch in `loadout::score`
/// silently became dead code in the CLI loadout pipeline. Keeping the
/// intermediate `skills` vec around closes that gap without changing the
/// public `run` signature.
fn run_inner(options: AuditOptions) -> Result<(Report, Vec<Skill>)> {
    let scan_out = scan::scan_dir(&options.root, &options.policy)?;

    // Parse in parallel.
    let root = options.root.clone();
    let parsed_results: Vec<(ParsedSkill, PathBuf)> = scan_out
        .files
        .par_iter()
        .map(|f| {
            let parsed = parse::parse_bytes(&f.bytes, &f.relative);
            (parsed, root.join(&f.relative))
        })
        .collect();

    // Fill in `exists` on every link ref by checking the filesystem. This is
    // done serially to avoid stat contention on slow filesystems.
    let parsed_resolved: Vec<ParsedSkill> = parsed_results
        .into_iter()
        .map(|(mut parsed, abs_path)| {
            let base = abs_path.parent().map(Path::to_path_buf).unwrap_or_default();
            for r in &mut parsed.refs {
                match r {
                    SkillRef::Link { target, exists } => {
                        if !target.to_string_lossy().starts_with("http") {
                            let candidate = resolve_path(&base, target);
                            *exists = candidate.exists();
                        } else {
                            *exists = true;
                        }
                    }
                    SkillRef::File { path, exists } => {
                        let candidate = resolve_path(&base, path);
                        *exists = candidate.exists();
                    }
                    _ => {}
                }
            }
            parsed
        })
        .collect();

    // Tokenize in parallel.
    let tokenizer = options.tokenizer.clone();
    let skills: Vec<Skill> = parsed_resolved
        .into_par_iter()
        .map(|parsed| parse::finalise(parsed, tokenizer.as_ref()))
        .collect();

    // Apply config-level budget overrides (frontmatter overrides always win
    // inside `rules::effective_budget` already — for config-doc overrides we
    // map into the skill's frontmatter if the skill doesn't already override).
    let mut skills = apply_overrides(skills, &options.overrides);

    // Deterministic skill ordering.
    skills.sort_by(|a, b| a.id.cmp(&b.id));

    let graph = SkillGraph::build(&skills);
    let mut issues: Vec<Issue> = rules::run_all(&skills, &graph, &options.budget);

    // Fold in scan-time issues (oversize, symlink skipped).
    issues.extend(scan_out.issues);

    // Deterministic final order.
    issues.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then(a.kind.cmp(&b.kind))
            .then(a.skill.cmp(&b.skill))
            .then(a.message.cmp(&b.message))
    });

    let total_tokens = skills.iter().map(|s| s.tokens.total).sum();
    let summaries: Vec<SkillSummary> = skills
        .iter()
        .map(|s| {
            // Include issues where this skill is the primary *or* appears in
            // the `related` list. Cycle and conflict issues attach to a single
            // "primary" skill chosen by canonical sort order; without this
            // rollup the other N-1 participants silently advertise
            // `issue_kinds: []` in the JSON summary, which misleads UIs and
            // the PR-comment markdown table into thinking only one skill is
            // involved.
            //
            // `Dead` is excluded from the `related` rollup because its
            // `related` field carries a different semantic: it lists the
            // *root/index files* that failed to reference the dead skill, not
            // other skills that share the same problem. Rolling up via
            // `related` therefore wrongly tagged the README / SKILLS.md index
            // nodes with `dead` in the per-skill summary, which then appeared
            // as "dead" rows in the Markdown PR-comment table — a highly
            // visible false positive on every library that uses an index file
            // alongside at least one unreachable skill.
            let issue_kinds: BTreeSet<IssueKind> = issues
                .iter()
                .filter(|i| {
                    i.skill == s.id || (i.kind != IssueKind::Dead && i.related.contains(&s.id))
                })
                .map(|i| i.kind)
                .collect();
            SkillSummary {
                id: s.id.clone(),
                name: s.name.clone(),
                path: s.path.clone(),
                tokens: s.tokens,
                tags: s.tags.clone(),
                refs_out: graph.out_degree(&s.id),
                refs_in: graph.in_degree(&s.id),
                issue_kinds: issue_kinds.into_iter().collect(),
            }
        })
        .collect();

    let report = Report {
        schema_version: crate::SCHEMA_VERSION,
        tokenizer: options.tokenizer.name().to_string(),
        tokenizer_version: options.tokenizer.version(),
        tool_version: crate::VERSION,
        scan_root: options.root,
        total_skills: skills.len(),
        total_tokens,
        budget: options.budget,
        skills: summaries,
        issues,
        loadout: None,
    };

    Ok((report, skills))
}

/// Audit + append a loadout recommendation for the given tag.
///
/// Computes the loadout against the *full* `Skill` objects produced by the
/// audit pipeline so that every field used by [`crate::loadout::score`] —
/// including `frontmatter.description` — is honored. Building a stand-in
/// `Skill` from the public `SkillSummary` (which does not carry the
/// frontmatter) silently dropped description-based scoring in the CLI
/// loadout pipeline; running the loadout against the original `skills`
/// closes that gap.
pub fn run_with_loadout(options: AuditOptions, tag: &str, max_tokens: usize) -> Result<Report> {
    let (mut report, skills) = run_inner(options)?;
    let loadout = crate::loadout::recommend(&skills, tag, max_tokens);
    report.loadout = Some(loadout);
    Ok(report)
}

/// Audit and return the graph for the `graph` subcommand.
pub fn run_graph(options: AuditOptions) -> Result<(Report, SkillGraph)> {
    let scan_out = scan::scan_dir(&options.root, &options.policy)?;
    let root = options.root.clone();
    let tokenizer = options.tokenizer.clone();

    let parsed: Vec<ParsedSkill> = scan_out
        .files
        .par_iter()
        .map(|f| parse::parse_bytes(&f.bytes, &f.relative))
        .collect();

    let skills: Vec<Skill> = parsed
        .into_par_iter()
        .map(|p| parse::finalise(p, tokenizer.as_ref()))
        .collect();

    let mut skills = skills;
    skills.sort_by(|a, b| a.id.cmp(&b.id));
    let graph = SkillGraph::build(&skills);

    let total_tokens = skills.iter().map(|s| s.tokens.total).sum();
    let summaries: Vec<SkillSummary> = skills
        .iter()
        .map(|s| SkillSummary {
            id: s.id.clone(),
            name: s.name.clone(),
            path: s.path.clone(),
            tokens: s.tokens,
            tags: s.tags.clone(),
            refs_out: graph.out_degree(&s.id),
            refs_in: graph.in_degree(&s.id),
            issue_kinds: vec![],
        })
        .collect();

    let mut issues: Vec<Issue> = graph.cycles(&skills);
    // No issue sort needed beyond canonical.
    issues.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.skill.cmp(&b.skill)));
    let report = Report {
        schema_version: crate::SCHEMA_VERSION,
        tokenizer: options.tokenizer.name().to_string(),
        tokenizer_version: options.tokenizer.version(),
        tool_version: crate::VERSION,
        scan_root: root,
        total_skills: skills.len(),
        total_tokens,
        budget: options.budget,
        skills: summaries,
        issues,
        loadout: None,
    };
    Ok((report, graph))
}

fn apply_overrides(
    mut skills: Vec<Skill>,
    overrides: &BTreeMap<String, Option<usize>>,
) -> Vec<Skill> {
    for s in &mut skills {
        if s.frontmatter.budget.is_some() {
            continue;
        }
        if let Some(budget) = overrides.get(s.id.as_str()).and_then(|v| *v) {
            s.frontmatter.budget = Some(budget);
        }
    }
    skills
}

fn resolve_path(base: &Path, target: &Path) -> PathBuf {
    if target.is_absolute() {
        target.to_path_buf()
    } else {
        base.join(target)
    }
}

/// Build a fresh [`Location`] for ad-hoc reporting. Kept here so the
/// CLI layer doesn't need to reach into `model`.
#[must_use]
pub fn location_for(path: &Path) -> Location {
    Location::start_of(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokenize;
    use std::fs;
    use tempfile::tempdir;

    fn write(base: &Path, rel: &str, content: &[u8]) {
        let path = base.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn default_options(root: PathBuf) -> AuditOptions {
        AuditOptions {
            root,
            tokenizer: tokenize::by_name("cl100k").unwrap(),
            budget: BudgetConfig {
                per_skill: 2000,
                total: None,
            },
            policy: ScanPolicy::default(),
            overrides: BTreeMap::new(),
        }
    }

    #[test]
    fn audit_empty_dir_is_clean() {
        let dir = tempdir().unwrap();
        let options = default_options(dir.path().to_path_buf());
        let report = run(options).unwrap();
        assert_eq!(report.total_skills, 0);
        assert!(report.issues.is_empty());
    }

    #[test]
    fn audit_detects_bloat() {
        let dir = tempdir().unwrap();
        let big_body: String = "word ".repeat(10_000);
        write(dir.path(), "big/SKILL.md", big_body.as_bytes());
        let mut options = default_options(dir.path().to_path_buf());
        options.budget = BudgetConfig {
            per_skill: 100,
            total: None,
        };
        let report = run(options).unwrap();
        assert_eq!(report.total_skills, 1);
        assert!(report.issues.iter().any(|i| i.kind == IssueKind::Bloated));
    }

    #[test]
    fn audit_detects_dead_skill() {
        let dir = tempdir().unwrap();
        write(dir.path(), "a/SKILL.md", b"body");
        let options = default_options(dir.path().to_path_buf());
        let report = run(options).unwrap();
        assert!(report
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::Dead && i.skill.as_str() == "a"));
    }

    #[test]
    fn audit_readme_root_unmarks_dead() {
        let dir = tempdir().unwrap();
        write(dir.path(), "README.md", b"See @a for details");
        write(dir.path(), "a/SKILL.md", b"body");
        let options = default_options(dir.path().to_path_buf());
        let report = run(options).unwrap();
        assert!(!report
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::Dead && i.skill.as_str() == "a"));
    }

    #[test]
    fn audit_detects_stale_link() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "a/SKILL.md",
            b"see [missing](./nope.md) for details",
        );
        let options = default_options(dir.path().to_path_buf());
        let report = run(options).unwrap();
        assert!(report.issues.iter().any(|i| i.kind == IssueKind::Stale));
    }

    #[test]
    fn audit_deterministic_across_runs() {
        let dir = tempdir().unwrap();
        write(dir.path(), "a/SKILL.md", b"body");
        write(dir.path(), "b/SKILL.md", b"other body");
        let r1 = run(default_options(dir.path().to_path_buf())).unwrap();
        let r2 = run(default_options(dir.path().to_path_buf())).unwrap();
        assert_eq!(
            serde_json::to_string(&r1).unwrap(),
            serde_json::to_string(&r2).unwrap()
        );
    }

    #[test]
    fn audit_reports_token_totals() {
        let dir = tempdir().unwrap();
        write(dir.path(), "a/SKILL.md", b"hello world");
        let options = default_options(dir.path().to_path_buf());
        let report = run(options).unwrap();
        assert!(report.total_tokens > 0);
    }

    #[test]
    fn audit_with_loadout_respects_budget() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "a/SKILL.md",
            b"---\ntags:\n  - git\n---\nshort body",
        );
        let options = default_options(dir.path().to_path_buf());
        let report = run_with_loadout(options, "git", 10_000).unwrap();
        assert!(report.loadout.is_some());
        assert_eq!(report.loadout.as_ref().unwrap().skills.len(), 1);
    }

    #[test]
    fn audit_detects_cycle() {
        let dir = tempdir().unwrap();
        write(dir.path(), "a/SKILL.md", b"@b");
        write(dir.path(), "b/SKILL.md", b"@a");
        let options = default_options(dir.path().to_path_buf());
        let report = run(options).unwrap();
        assert!(report.issues.iter().any(|i| i.kind == IssueKind::Cycle));
    }

    #[test]
    fn apply_overrides_config_budget() {
        let dir = tempdir().unwrap();
        write(dir.path(), "a/SKILL.md", &"word ".repeat(2000).into_bytes());
        let mut options = default_options(dir.path().to_path_buf());
        options.budget = BudgetConfig {
            per_skill: 100,
            total: None,
        };
        options.overrides.insert("a".to_string(), Some(100_000));
        let report = run(options).unwrap();
        assert!(!report.issues.iter().any(|i| i.kind == IssueKind::Bloated));
    }

    #[test]
    fn run_graph_returns_graph() {
        use crate::model::SkillId;
        let dir = tempdir().unwrap();
        write(dir.path(), "a/SKILL.md", b"body");
        let options = default_options(dir.path().to_path_buf());
        let (report, graph) = run_graph(options).unwrap();
        assert_eq!(report.total_skills, 1);
        assert_eq!(graph.in_degree(&SkillId::new("a")), 0);
    }

    #[test]
    fn apply_config_merges_ignores() {
        let dir = tempdir().unwrap();
        let mut options = default_options(dir.path().to_path_buf());
        let mut doc = ConfigDoc::default();
        doc.ignore.globs.push("archive/**".into());
        options.apply_config(&doc);
        assert!(options
            .policy
            .ignore_globs
            .iter()
            .any(|g| g == "archive/**"));
    }

    #[test]
    fn apply_config_merges_overrides() {
        let dir = tempdir().unwrap();
        let mut options = default_options(dir.path().to_path_buf());
        let mut doc = ConfigDoc::default();
        doc.overrides.insert(
            "git/commit".into(),
            crate::config::SkillOverride { budget: Some(5000) },
        );
        options.apply_config(&doc);
        assert_eq!(options.overrides.get("git/commit"), Some(&Some(5000)));
    }

    #[test]
    fn location_for_builds_start() {
        let l = location_for(Path::new("x.md"));
        assert_eq!(l.line, 1);
    }
}
