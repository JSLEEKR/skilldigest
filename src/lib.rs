//! skilldigest — static analyzer for AI coding-assistant skill libraries.
//!
//! This crate exposes the library surface used by the `skilldigest` CLI. The
//! library is split into small focused modules so that each concern can be
//! unit tested in isolation:
//!
//! - [`config`] — loading of `.skilldigest.toml`
//! - [`scan`]   — directory walking and file classification
//! - [`parse`]  — markdown + frontmatter parsing
//! - [`model`]  — core data types (Skill, Issue, Report, ...)
//! - [`tokenize`] — tokenizer pool (cl100k, o200k, llama3-approx)
//! - [`graph`]  — skill reference graph + algorithms
//! - [`rules`]  — conflict / bloat / dead / stale detection heuristics
//! - [`audit`]  — high-level orchestration of scan + parse + tokenize + rules
//! - [`loadout`] — task-tag-driven skill loadout recommendation
//! - [`output`] — output format implementations (text, json, sarif, markdown, dot)
//!
//! The crate is deliberately `#![forbid(unsafe_code)]` and has no `async`
//! runtime requirement. Parallelism is provided via `rayon` for the CPU-bound
//! tokenization step.

#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]
#![warn(missing_debug_implementations)]

pub mod audit;
pub mod cli;
pub mod config;
pub mod error;
pub mod graph;
pub mod loadout;
pub mod model;
pub mod output;
pub mod parse;
pub mod rules;
pub mod scan;
pub mod tokenize;

pub use error::{Error, ExitCode, Result};
pub use model::{
    Frontmatter, Issue, IssueKind, Location, Report, Rule, RuleKind, Severity, Skill, SkillId,
    SkillRef, SkillSummary, TokenCounts,
};

/// Library semver, baked in at build time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Stable JSON/SARIF schema version. Bump only on breaking changes.
pub const SCHEMA_VERSION: &str = "skilldigest-report/1";
