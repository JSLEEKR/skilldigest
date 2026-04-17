//! Output format dispatch.
//!
//! All emitters are pure string producers (or byte producers) — we do not
//! take a `&mut dyn Write` to keep them trivially testable.

use std::str::FromStr;

use crate::error::{Error, Result};
use crate::model::Report;

pub mod dot;
pub mod json;
pub mod markdown;
pub mod sarif;
pub mod text;

/// Supported output formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Human-readable text (default).
    Text,
    /// Structured JSON.
    Json,
    /// SARIF 2.1.0.
    Sarif,
    /// Markdown for PR comments.
    Markdown,
    /// GraphViz dot (only meaningful for the `graph` subcommand).
    Dot,
}

impl Format {
    /// All format identifiers we accept.
    pub const NAMES: &'static [&'static str] = &["text", "json", "sarif", "markdown", "dot"];

    /// File extension for the format — used when `--output` is a bare
    /// directory.
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Text => "txt",
            Self::Json => "json",
            Self::Sarif => "sarif.json",
            Self::Markdown => "md",
            Self::Dot => "dot",
        }
    }
}

impl FromStr for Format {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "text" | "txt" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            "sarif" => Ok(Self::Sarif),
            "markdown" | "md" => Ok(Self::Markdown),
            "dot" | "graphviz" => Ok(Self::Dot),
            other => Err(Error::UnknownFormat(other.to_string())),
        }
    }
}

/// Render a scan report in the requested format.
pub fn render_report(report: &Report, format: Format, no_color: bool) -> Result<String> {
    match format {
        Format::Text => Ok(text::render(report, no_color)),
        Format::Json => json::render(report),
        Format::Sarif => sarif::render(report),
        Format::Markdown => Ok(markdown::render(report)),
        Format::Dot => Err(Error::bad_arg(
            "--format dot is only valid for the `graph` subcommand",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_from_str_accepts_aliases() {
        assert_eq!("text".parse::<Format>().unwrap(), Format::Text);
        assert_eq!("txt".parse::<Format>().unwrap(), Format::Text);
        assert_eq!("MD".parse::<Format>().unwrap(), Format::Markdown);
        assert_eq!("markdown".parse::<Format>().unwrap(), Format::Markdown);
        assert_eq!("sarif".parse::<Format>().unwrap(), Format::Sarif);
        assert_eq!("graphviz".parse::<Format>().unwrap(), Format::Dot);
    }

    #[test]
    fn format_from_str_rejects_unknown() {
        let e = "bogus".parse::<Format>().unwrap_err();
        assert!(matches!(e, Error::UnknownFormat(_)));
    }

    #[test]
    fn extensions_unique() {
        let all = [
            Format::Text,
            Format::Json,
            Format::Sarif,
            Format::Markdown,
            Format::Dot,
        ];
        let mut exts: Vec<&str> = all.iter().map(|f| f.extension()).collect();
        exts.sort_unstable();
        exts.dedup();
        assert_eq!(exts.len(), 5);
    }

    #[test]
    fn names_constant_complete() {
        assert_eq!(Format::NAMES.len(), 5);
    }

    #[test]
    fn render_dot_rejected_for_report() {
        let r = empty_report();
        let err = render_report(&r, Format::Dot, false).unwrap_err();
        assert!(matches!(err, Error::BadArg(_)));
    }

    fn empty_report() -> Report {
        Report {
            schema_version: crate::SCHEMA_VERSION,
            tokenizer: "cl100k_base".to_string(),
            tokenizer_version: "tiktoken-rs 0.7 cl100k_base".to_string(),
            tool_version: crate::VERSION,
            scan_root: std::path::PathBuf::from("."),
            total_skills: 0,
            total_tokens: 0,
            budget: crate::model::BudgetConfig {
                per_skill: 2000,
                total: None,
            },
            skills: vec![],
            issues: vec![],
            loadout: None,
        }
    }
}
