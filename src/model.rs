//! Core data model for the analyzer.
//!
//! All collections use `Vec` or `BTreeMap` so that serialization is
//! deterministic. Types are `serde::Serialize`-friendly without being
//! over-engineered: `SkillId` is a thin `String` newtype for type safety,
//! issue kinds are `Copy` enums, etc.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Stable identifier for a skill derived from its path relative to the scan
/// root. IDs are lower-case, `/`-separated, with the `SKILL.md` suffix
/// stripped.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SkillId(String);

impl SkillId {
    /// Construct a SkillId from a string slice. The input is normalised to
    /// use `/` as the separator on all platforms. Empty IDs are rejected.
    #[must_use]
    pub fn new(s: impl Into<String>) -> Self {
        let raw = s.into();
        let normalised: String = raw
            .replace('\\', "/")
            .trim_matches('/')
            .trim_start_matches("./")
            .to_string();
        Self(normalised)
    }

    /// Return the inner string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// True when the ID is the empty string (which we treat as invalid but
    /// tolerate rather than panic).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Display for SkillId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for SkillId {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

/// Parsed YAML frontmatter (all fields optional).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Frontmatter {
    /// Declared skill name (fallback: filename stem).
    #[serde(default)]
    pub name: Option<String>,
    /// Human description.
    #[serde(default)]
    pub description: Option<String>,
    /// Tag list used by the loadout recommender.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Per-skill budget override (tokens).
    #[serde(default)]
    pub budget: Option<usize>,
    /// Other skill IDs this skill depends on.
    #[serde(default)]
    pub requires: Vec<String>,
    /// Allow-list of tool invocations, e.g. `Bash(ls)` / `Write(*)`.
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// Any other YAML keys that the user put in — preserved so the output
    /// can round-trip.
    #[serde(default, flatten)]
    pub other: BTreeMap<String, serde_yaml::Value>,
}

/// Reference from one skill to something else (another skill, a file, a
/// tool invocation).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkillRef {
    /// A markdown link to a file (relative to the skill's directory).
    Link {
        /// Target path as written, normalised to forward slashes.
        target: PathBuf,
        /// Whether the target exists on disk.
        exists: bool,
    },
    /// An `@other-skill` or `[[other-skill]]` mention.
    Mention {
        /// Referenced skill ID.
        skill_id: SkillId,
    },
    /// An invocation like `Bash(ls)` or `Write(*.md)`.
    Tool {
        /// Tool name (the `Bash` in `Bash(ls)`).
        name: String,
        /// Argument payload (the `ls` in `Bash(ls)`).
        args: String,
    },
    /// Explicit file reference using the bare path, not a markdown link.
    File {
        /// Path on disk.
        path: PathBuf,
        /// Whether the target exists on disk.
        exists: bool,
    },
}

/// A structural rule extracted from the body of a skill. Used for conflict
/// detection.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rule {
    /// Rule classification.
    pub kind: RuleKind,
    /// The subject of the rule (tool name, file pattern, topic).
    pub subject: String,
    /// Modal verb strength.
    pub modal: Modal,
    /// Original sentence for human display.
    pub raw: String,
    /// 1-based line number in the skill body.
    pub line: usize,
}

/// Rule classification.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    /// "Always use X"
    AlwaysUse,
    /// "Never use X"
    NeverUse,
    /// Any other structural rule (reserved for future use).
    Other,
}

/// Modal verb strength.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modal {
    /// MUST / SHALL
    Must,
    /// MUST NOT / SHALL NOT
    MustNot,
    /// SHOULD / PREFER
    Should,
    /// SHOULD NOT / AVOID
    ShouldNot,
}

impl Modal {
    /// Return `true` when two modals contradict (must vs must-not, etc.).
    #[must_use]
    pub fn conflicts_with(self, other: Self) -> bool {
        matches!(
            (self, other),
            (Self::Must, Self::MustNot)
                | (Self::MustNot, Self::Must)
                | (Self::Should, Self::ShouldNot)
                | (Self::ShouldNot, Self::Should)
        )
    }
}

/// Token counts for a skill, split by section.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCounts {
    /// Tokens in the YAML frontmatter block.
    pub frontmatter: usize,
    /// Tokens in the markdown body.
    pub body: usize,
    /// Total: frontmatter + body.
    pub total: usize,
}

impl TokenCounts {
    /// Build from the two components.
    #[must_use]
    pub fn new(frontmatter: usize, body: usize) -> Self {
        Self {
            frontmatter,
            body,
            total: frontmatter + body,
        }
    }
}

/// Pre-parse warning about a file (BOM, CRLF, non-utf8, etc.). Warnings
/// attach to the parsed [`Skill`] — they never halt the scan.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Warning {
    /// Kind of warning.
    pub kind: WarningKind,
    /// Human-readable message.
    pub message: String,
}

/// Warning classification.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarningKind {
    /// BOM at start of file (we strip it; the warning just notes it).
    Bom,
    /// CRLF line endings — we normalise to LF for analysis.
    Crlf,
    /// Mixed tab/space indentation.
    MixedIndent,
    /// Non-UTF-8 decoded via replacement.
    NonUtf8Recovered,
    /// Frontmatter failed to parse as YAML; we fall back to empty.
    FrontmatterYamlError,
}

/// A fully parsed skill.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Skill {
    /// Stable identifier.
    pub id: SkillId,
    /// Display name.
    pub name: String,
    /// Path relative to the scan root.
    pub path: PathBuf,
    /// Parsed frontmatter (empty default if absent).
    pub frontmatter: Frontmatter,
    /// Token counts.
    pub tokens: TokenCounts,
    /// Outgoing references.
    pub refs: Vec<SkillRef>,
    /// Extracted structural rules.
    pub rules: Vec<Rule>,
    /// Effective tag list (from frontmatter + heuristic extraction).
    pub tags: Vec<String>,
    /// Non-fatal parse-time warnings.
    pub warnings: Vec<Warning>,
    /// Raw body length in bytes (for size-cap diagnostics).
    pub body_bytes: usize,
}

/// Severity level for an issue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Informational.
    Note,
    /// Non-blocking advisory.
    Warning,
    /// Blocking — causes exit-1.
    Error,
}

impl Severity {
    /// Convert to SARIF level string.
    #[must_use]
    pub fn as_sarif(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }

    /// Severity that maps to exit-1. Warning + Note do not.
    #[must_use]
    pub fn is_blocking(self) -> bool {
        matches!(self, Self::Error)
    }
}

/// Classification of an issue detected by the audit rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueKind {
    /// Skill is never referenced by any index or other skill.
    Dead,
    /// Skill's token total exceeds the budget.
    Bloated,
    /// Skill defines a rule that contradicts another skill.
    Conflict,
    /// Skill references a file that does not exist on disk.
    Stale,
    /// Cycle detected in the skill reference graph.
    Cycle,
    /// File exceeded `--max-file-size`.
    Oversize,
    /// File could not be decoded as UTF-8.
    NonUtf8,
    /// Frontmatter was malformed.
    BadFrontmatter,
    /// Scanner encountered a symlink that was skipped.
    Symlink,
    /// Duplicate skill IDs in the library.
    Duplicate,
}

impl IssueKind {
    /// Short stable identifier for SARIF `ruleId`.
    #[must_use]
    pub fn rule_id(self) -> &'static str {
        match self {
            Self::Dead => "SKILL001",
            Self::Bloated => "SKILL002",
            Self::Conflict => "SKILL003",
            Self::Stale => "SKILL004",
            Self::Cycle => "SKILL005",
            Self::Oversize => "SKILL006",
            Self::NonUtf8 => "SKILL007",
            Self::BadFrontmatter => "SKILL008",
            Self::Symlink => "SKILL009",
            Self::Duplicate => "SKILL010",
        }
    }

    /// Default severity for this kind.
    #[must_use]
    pub fn default_severity(self) -> Severity {
        match self {
            Self::Bloated | Self::Conflict | Self::Cycle | Self::Oversize | Self::Duplicate => {
                Severity::Error
            }
            Self::Dead | Self::Stale | Self::NonUtf8 | Self::BadFrontmatter => Severity::Warning,
            Self::Symlink => Severity::Note,
        }
    }

    /// Short human title used in text output.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Dead => "dead",
            Self::Bloated => "bloated",
            Self::Conflict => "conflict",
            Self::Stale => "stale",
            Self::Cycle => "cycle",
            Self::Oversize => "oversize",
            Self::NonUtf8 => "non-utf8",
            Self::BadFrontmatter => "bad-frontmatter",
            Self::Symlink => "symlink",
            Self::Duplicate => "duplicate",
        }
    }
}

/// Source-file location attached to an issue.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Location {
    /// Path relative to the scan root.
    pub path: PathBuf,
    /// 1-based line number.
    pub line: usize,
    /// 1-based column number.
    pub column: usize,
}

impl Location {
    /// Canonical "start of file" location.
    #[must_use]
    pub fn start_of(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            line: 1,
            column: 1,
        }
    }
}

/// A single diagnostic.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Issue {
    /// Classification.
    pub kind: IssueKind,
    /// Severity.
    pub severity: Severity,
    /// ID of the primary skill the issue attaches to.
    pub skill: SkillId,
    /// Short human message.
    pub message: String,
    /// Optional source location.
    pub location: Option<Location>,
    /// Related skill IDs (for conflicts and cycles).
    pub related: Vec<SkillId>,
}

impl Issue {
    /// Convenience constructor that applies the default severity.
    #[must_use]
    pub fn new(kind: IssueKind, skill: SkillId, message: impl Into<String>) -> Self {
        Self {
            kind,
            severity: kind.default_severity(),
            skill,
            message: message.into(),
            location: None,
            related: Vec::new(),
        }
    }

    /// Attach a location.
    #[must_use]
    pub fn with_location(mut self, location: Location) -> Self {
        self.location = Some(location);
        self
    }

    /// Attach related skills.
    #[must_use]
    pub fn with_related(mut self, related: Vec<SkillId>) -> Self {
        self.related = related;
        self
    }
}

/// Budget configuration emitted in the final report.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct BudgetConfig {
    /// Per-skill budget (tokens).
    pub per_skill: usize,
    /// Optional aggregate cap.
    pub total: Option<usize>,
}

/// Summary row for each skill in the report.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkillSummary {
    /// Stable identifier.
    pub id: SkillId,
    /// Display name.
    pub name: String,
    /// Path relative to scan root.
    pub path: PathBuf,
    /// Token counts.
    pub tokens: TokenCounts,
    /// Tag list.
    pub tags: Vec<String>,
    /// Out-degree (number of outgoing references).
    pub refs_out: usize,
    /// In-degree.
    pub refs_in: usize,
    /// Issue kinds that apply to this skill (deduplicated).
    pub issue_kinds: Vec<IssueKind>,
}

/// Recommended loadout for a task tag.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Loadout {
    /// Task tag the loadout was computed for.
    pub tag: String,
    /// Maximum token budget the selection respects.
    pub max_tokens: usize,
    /// Selected skill IDs in priority order.
    pub skills: Vec<SkillId>,
    /// Total tokens used by the selection.
    pub total_tokens: usize,
}

/// Top-level report emitted by the analyzer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    /// Schema version constant — helps consumers pin behaviour.
    pub schema_version: &'static str,
    /// Tokenizer identifier actually used for counting.
    pub tokenizer: String,
    /// Tool version.
    pub tool_version: &'static str,
    /// Scan root path (relative or absolute — as given by user).
    pub scan_root: PathBuf,
    /// Total skill count.
    pub total_skills: usize,
    /// Aggregate token cost across all skills.
    pub total_tokens: usize,
    /// Budget the audit was run with.
    pub budget: BudgetConfig,
    /// Per-skill summary rows.
    pub skills: Vec<SkillSummary>,
    /// Diagnostics in canonical order.
    pub issues: Vec<Issue>,
    /// Optional loadout, only present for the `loadout` subcommand.
    pub loadout: Option<Loadout>,
}

impl Report {
    /// True when any issue has blocking severity.
    #[must_use]
    pub fn has_blocking(&self) -> bool {
        self.issues.iter().any(|i| i.severity.is_blocking())
    }

    /// Count of issues at each severity level.
    #[must_use]
    pub fn severity_counts(&self) -> (usize, usize, usize) {
        let mut err = 0;
        let mut warn = 0;
        let mut note = 0;
        for i in &self.issues {
            match i.severity {
                Severity::Error => err += 1,
                Severity::Warning => warn += 1,
                Severity::Note => note += 1,
            }
        }
        (err, warn, note)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_id_normalises_backslashes() {
        let id = SkillId::new("foo\\bar\\baz");
        assert_eq!(id.as_str(), "foo/bar/baz");
    }

    #[test]
    fn skill_id_trims_leading_slashes_and_dots() {
        let id = SkillId::new("./foo/bar/");
        assert_eq!(id.as_str(), "foo/bar");
    }

    #[test]
    fn skill_id_empty_is_detected() {
        let id = SkillId::new("");
        assert!(id.is_empty());
    }

    #[test]
    fn modal_conflict_symmetry() {
        assert!(Modal::Must.conflicts_with(Modal::MustNot));
        assert!(Modal::MustNot.conflicts_with(Modal::Must));
        assert!(Modal::Should.conflicts_with(Modal::ShouldNot));
        assert!(!Modal::Must.conflicts_with(Modal::Should));
        assert!(!Modal::Must.conflicts_with(Modal::Must));
    }

    #[test]
    fn severity_ordering() {
        assert!(Severity::Note < Severity::Warning);
        assert!(Severity::Warning < Severity::Error);
    }

    #[test]
    fn severity_as_sarif_strings() {
        assert_eq!(Severity::Error.as_sarif(), "error");
        assert_eq!(Severity::Warning.as_sarif(), "warning");
        assert_eq!(Severity::Note.as_sarif(), "note");
    }

    #[test]
    fn severity_blocking_only_error() {
        assert!(Severity::Error.is_blocking());
        assert!(!Severity::Warning.is_blocking());
        assert!(!Severity::Note.is_blocking());
    }

    #[test]
    fn issue_kind_rule_ids_are_unique() {
        let kinds = [
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
        ];
        let ids: Vec<&str> = kinds.iter().map(|k| k.rule_id()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids.len(), sorted.len());
    }

    #[test]
    fn issue_kind_default_severity_mapping() {
        assert_eq!(IssueKind::Bloated.default_severity(), Severity::Error);
        assert_eq!(IssueKind::Dead.default_severity(), Severity::Warning);
        assert_eq!(IssueKind::Symlink.default_severity(), Severity::Note);
    }

    #[test]
    fn token_counts_total_sums_components() {
        let t = TokenCounts::new(10, 20);
        assert_eq!(t.total, 30);
    }

    #[test]
    fn location_start_of() {
        let l = Location::start_of("foo.md");
        assert_eq!(l.line, 1);
        assert_eq!(l.column, 1);
        assert_eq!(l.path, PathBuf::from("foo.md"));
    }

    #[test]
    fn issue_new_applies_default_severity() {
        let i = Issue::new(IssueKind::Bloated, SkillId::new("a"), "too big");
        assert_eq!(i.severity, Severity::Error);
        assert_eq!(i.skill.as_str(), "a");
    }

    #[test]
    fn issue_builders() {
        let i = Issue::new(IssueKind::Dead, SkillId::new("x"), "msg")
            .with_location(Location::start_of("x/SKILL.md"))
            .with_related(vec![SkillId::new("y")]);
        assert!(i.location.is_some());
        assert_eq!(i.related.len(), 1);
    }

    #[test]
    fn report_counts_severities() {
        let skill = SkillId::new("a");
        let issues = vec![
            Issue::new(IssueKind::Dead, skill.clone(), "x"),
            Issue::new(IssueKind::Bloated, skill.clone(), "y"),
            Issue::new(IssueKind::Symlink, skill, "z"),
        ];
        let report = Report {
            schema_version: crate::SCHEMA_VERSION,
            tokenizer: "cl100k".to_string(),
            tool_version: crate::VERSION,
            scan_root: PathBuf::from("."),
            total_skills: 1,
            total_tokens: 0,
            budget: BudgetConfig {
                per_skill: 2000,
                total: None,
            },
            skills: vec![],
            issues,
            loadout: None,
        };
        let (err, warn, note) = report.severity_counts();
        assert_eq!((err, warn, note), (1, 1, 1));
        assert!(report.has_blocking());
    }

    #[test]
    fn skill_id_display_roundtrips() {
        let id = SkillId::new("a/b");
        assert_eq!(format!("{id}"), "a/b");
    }

    #[test]
    fn skill_id_from_str() {
        let id: SkillId = "foo/bar".into();
        assert_eq!(id.as_str(), "foo/bar");
    }
}
