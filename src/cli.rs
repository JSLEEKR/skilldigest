//! CLI layer — `clap` v4 derive with subcommand dispatch.
//!
//! All user-facing paths go through this module. `main.rs` is a tiny shim
//! that calls [`run`].

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};

use crate::audit::{self, AuditOptions};
use crate::config;
use crate::error::{Error, ExitCode, Result};
use crate::model::BudgetConfig;
use crate::output::{self, Format};
use crate::scan::ScanPolicy;
use crate::tokenize::{self, Tokenizer};

/// Top-level CLI.
#[derive(Parser, Debug)]
#[command(
    name = "skilldigest",
    version = crate::VERSION,
    about = "Static analyzer for AI agent skill libraries",
    long_about = "Static analyzer for AI coding-assistant skill libraries (SKILL.md, AGENTS.md, .cursorrules, CLAUDE.md, agent plugins). Parses skill/agent markdown, builds a dependency + reference graph, measures per-skill token cost, and reports dead / bloated / conflicting / stale skills plus a consolidated loadout recommendation."
)]
pub struct Cli {
    /// Output format.
    #[arg(long, short = 'f', global = true, default_value = "text")]
    pub format: String,
    /// Write output to this file instead of stdout.
    #[arg(long, short = 'o', global = true)]
    pub output: Option<PathBuf>,
    /// Tokenizer: cl100k, o200k, llama3.
    #[arg(long, short = 't', global = true, default_value = "cl100k")]
    pub tokenizer: String,
    /// Per-skill token budget.
    #[arg(long, short = 'b', global = true, default_value_t = 2000)]
    pub budget: usize,
    /// Aggregate token budget.
    #[arg(long, global = true)]
    pub total_budget: Option<usize>,
    /// Force fully offline mode (never touch the filesystem cache).
    #[arg(long, global = true)]
    pub offline: bool,
    /// Follow symlinks during scan.
    #[arg(long, global = true)]
    pub follow_symlinks: bool,
    /// Maximum file size (bytes) to consider.
    #[arg(long, global = true, default_value_t = 1024 * 1024)]
    pub max_file_size: u64,
    /// Path to `.skilldigest.toml`.
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    /// Disable ANSI colors.
    #[arg(long, global = true)]
    pub no_color: bool,
    /// Verbose logging to stderr.
    #[arg(long, short = 'v', global = true)]
    pub verbose: bool,
    /// Suppress non-error output.
    #[arg(long, short = 'q', global = true)]
    pub quiet: bool,

    /// Subcommand.
    #[command(subcommand)]
    pub command: Command,
}

/// Subcommands.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Full audit of a skill library directory.
    Scan {
        /// Root directory to scan.
        dir: PathBuf,
        /// Emit `rm` hints for dead skills.
        #[arg(long)]
        fix_hint: bool,
    },
    /// Token count for a single file.
    Tokens {
        /// File to measure.
        file: PathBuf,
        /// Split frontmatter and body.
        #[arg(long)]
        by_section: bool,
    },
    /// Recommend a skill loadout for a task tag.
    Loadout {
        /// Root directory to scan.
        dir: PathBuf,
        /// Task tag to match.
        #[arg(long)]
        tag: String,
        /// Maximum total tokens the loadout may consume.
        #[arg(long, default_value_t = 10_000)]
        max_tokens: usize,
    },
    /// Emit the skill reference graph (dot or json).
    Graph {
        /// Root directory to scan.
        dir: PathBuf,
    },
}

/// Top-level entry point. Returns an [`ExitCode`] the runtime can propagate
/// to `std::process::exit`.
pub fn run(cli: Cli) -> Result<ExitCode> {
    let format: Format = cli.format.parse()?;

    match &cli.command {
        Command::Scan { dir, fix_hint } => scan_cmd(&cli, dir, *fix_hint, format),
        Command::Tokens { file, by_section } => tokens_cmd(&cli, file, *by_section, format),
        Command::Loadout {
            dir,
            tag,
            max_tokens,
        } => loadout_cmd(&cli, dir, tag, *max_tokens, format),
        Command::Graph { dir } => graph_cmd(&cli, dir, format),
    }
}

fn build_options(cli: &Cli, dir: &std::path::Path) -> Result<AuditOptions> {
    let tokenizer: Arc<dyn Tokenizer> = tokenize::by_name(&cli.tokenizer)?;
    let policy = ScanPolicy {
        follow_symlinks: cli.follow_symlinks,
        max_file_size: cli.max_file_size,
        ..ScanPolicy::default()
    };

    let mut options = AuditOptions {
        root: dir.to_path_buf(),
        tokenizer,
        budget: BudgetConfig {
            per_skill: cli.budget,
            total: cli.total_budget,
        },
        policy,
        overrides: BTreeMap::new(),
    };

    let config_path = cli.config.clone().or_else(|| config::find_default(dir));
    if let Some(path) = config_path {
        if let Some(doc) = config::load(&path)? {
            options.apply_config(&doc);
        }
    }
    Ok(options)
}

fn scan_cmd(cli: &Cli, dir: &std::path::Path, fix_hint: bool, format: Format) -> Result<ExitCode> {
    let options = build_options(cli, dir)?;
    let report = audit::run(options)?;
    let rendered = output::render_report(&report, format, cli.no_color)?;
    emit(cli, &rendered)?;

    if fix_hint {
        let hints = build_fix_hints(&report);
        if !hints.is_empty() {
            eprintln!("\n# skilldigest fix hints");
            for h in hints {
                eprintln!("{h}");
            }
        }
    }

    Ok(if report.has_blocking() {
        ExitCode::IssuesFound
    } else {
        ExitCode::Clean
    })
}

fn tokens_cmd(
    cli: &Cli,
    file: &std::path::Path,
    by_section: bool,
    format: Format,
) -> Result<ExitCode> {
    let tokenizer: Arc<dyn Tokenizer> = tokenize::by_name(&cli.tokenizer)?;
    let bytes = fs::read(file).map_err(|e| Error::io(file, e))?;
    let parsed = crate::parse::parse_bytes(&bytes, file);
    let (frontmatter_tokens, body_tokens) = if by_section {
        (
            tokenizer.count(&parsed.frontmatter_raw),
            tokenizer.count(&parsed.body),
        )
    } else {
        (
            0,
            tokenizer.count(&format!("{}{}", parsed.frontmatter_raw, parsed.body)),
        )
    };

    let rendered = match format {
        Format::Json => serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": crate::SCHEMA_VERSION,
            "tokenizer": tokenizer.name(),
            "tool_version": crate::VERSION,
            "file": file.to_string_lossy(),
            "frontmatter": frontmatter_tokens,
            "body": body_tokens,
            "total": frontmatter_tokens + body_tokens,
        }))
        .map_err(|e| Error::Other(anyhow::anyhow!("json: {e}")))?,
        Format::Markdown => format!(
            "**{}** — {} tokens ({})\n",
            file.display(),
            frontmatter_tokens + body_tokens,
            tokenizer.name()
        ),
        Format::Sarif => {
            // Synthesize an empty SARIF with properties.
            let shim = crate::model::Report {
                schema_version: crate::SCHEMA_VERSION,
                tokenizer: tokenizer.name().to_string(),
                tool_version: crate::VERSION,
                scan_root: file.to_path_buf(),
                total_skills: 1,
                total_tokens: frontmatter_tokens + body_tokens,
                budget: BudgetConfig {
                    per_skill: cli.budget,
                    total: cli.total_budget,
                },
                skills: vec![],
                issues: vec![],
                loadout: None,
            };
            crate::output::sarif::render(&shim)?
        }
        Format::Dot => {
            return Err(Error::bad_arg(
                "--format dot is only valid for the `graph` subcommand",
            ));
        }
        Format::Text => {
            if by_section {
                format!(
                    "{}\n  frontmatter: {} tokens\n  body: {} tokens\n  total: {} tokens ({})\n",
                    file.display(),
                    frontmatter_tokens,
                    body_tokens,
                    frontmatter_tokens + body_tokens,
                    tokenizer.name()
                )
            } else {
                format!(
                    "{}: {} tokens ({})\n",
                    file.display(),
                    frontmatter_tokens + body_tokens,
                    tokenizer.name()
                )
            }
        }
    };

    emit(cli, &rendered)?;
    Ok(ExitCode::Clean)
}

fn loadout_cmd(
    cli: &Cli,
    dir: &std::path::Path,
    tag: &str,
    max_tokens: usize,
    format: Format,
) -> Result<ExitCode> {
    let options = build_options(cli, dir)?;
    let report = audit::run_with_loadout(options, tag, max_tokens)?;
    let rendered = output::render_report(&report, format, cli.no_color)?;
    emit(cli, &rendered)?;
    Ok(if report.has_blocking() {
        ExitCode::IssuesFound
    } else {
        ExitCode::Clean
    })
}

fn graph_cmd(cli: &Cli, dir: &std::path::Path, format: Format) -> Result<ExitCode> {
    let options = build_options(cli, dir)?;
    let (report, graph) = audit::run_graph(options)?;
    let rendered = match format {
        Format::Dot => crate::output::dot::render(&graph),
        Format::Json => crate::output::dot::render_json(&graph),
        Format::Text => graph.to_dot(),
        Format::Markdown => {
            let mut s = String::from("### skilldigest graph\n\n```dot\n");
            s.push_str(&graph.to_dot());
            s.push_str("```\n");
            s
        }
        Format::Sarif => {
            return Err(Error::bad_arg(
                "--format sarif is not meaningful for the `graph` subcommand",
            ));
        }
    };
    emit(cli, &rendered)?;
    Ok(if report.has_blocking() {
        ExitCode::IssuesFound
    } else {
        ExitCode::Clean
    })
}

fn emit(cli: &Cli, rendered: &str) -> Result<()> {
    if cli.quiet {
        return Ok(());
    }
    match &cli.output {
        Some(path) => {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
                }
            }
            fs::write(path, rendered).map_err(|e| Error::io(path, e))?;
        }
        None => {
            let mut stdout = std::io::stdout().lock();
            stdout
                .write_all(rendered.as_bytes())
                .map_err(|e| Error::io(PathBuf::from("<stdout>"), e))?;
            if !rendered.ends_with('\n') {
                let _ = stdout.write_all(b"\n");
            }
        }
    }
    Ok(())
}

fn build_fix_hints(report: &crate::model::Report) -> Vec<String> {
    report
        .issues
        .iter()
        .filter(|i| i.kind == crate::model::IssueKind::Dead)
        .map(|i| format!("rm -r -- '{}'  # dead skill {}", i.skill, i.skill))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_parses_scan() {
        let cli = Cli::try_parse_from(["skilldigest", "scan", "/tmp"]).unwrap();
        assert!(matches!(cli.command, Command::Scan { .. }));
    }

    #[test]
    fn cli_parses_tokens_subcommand() {
        let cli = Cli::try_parse_from(["skilldigest", "tokens", "/tmp/a.md"]).unwrap();
        assert!(matches!(cli.command, Command::Tokens { .. }));
    }

    #[test]
    fn cli_parses_loadout_subcommand() {
        let cli = Cli::try_parse_from([
            "skilldigest",
            "loadout",
            "/tmp",
            "--tag",
            "git",
            "--max-tokens",
            "500",
        ])
        .unwrap();
        assert!(matches!(cli.command, Command::Loadout { .. }));
    }

    #[test]
    fn cli_parses_graph_subcommand() {
        let cli = Cli::try_parse_from(["skilldigest", "graph", "/tmp"]).unwrap();
        assert!(matches!(cli.command, Command::Graph { .. }));
    }

    #[test]
    fn cli_parses_global_flags() {
        let cli = Cli::try_parse_from([
            "skilldigest",
            "--format",
            "json",
            "--tokenizer",
            "o200k",
            "--budget",
            "3000",
            "scan",
            "/tmp",
        ])
        .unwrap();
        assert_eq!(cli.format, "json");
        assert_eq!(cli.tokenizer, "o200k");
        assert_eq!(cli.budget, 3000);
    }

    #[test]
    fn cli_rejects_unknown_subcommand() {
        let err = Cli::try_parse_from(["skilldigest", "bogus"]).unwrap_err();
        assert!(
            err.kind() == clap::error::ErrorKind::InvalidSubcommand
                || err.kind() == clap::error::ErrorKind::UnknownArgument
        );
    }

    #[test]
    fn cli_requires_subcommand() {
        let err = Cli::try_parse_from(["skilldigest"]).unwrap_err();
        // clap 4.x may return either MissingSubcommand or
        // DisplayHelpOnMissingArgumentOrSubcommand depending on config.
        assert!(matches!(
            err.kind(),
            clap::error::ErrorKind::MissingSubcommand
                | clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
                | clap::error::ErrorKind::DisplayHelp
        ));
    }

    #[test]
    fn cli_structure_is_valid() {
        // This ensures clap does not panic on metadata construction.
        Cli::command().debug_assert();
    }

    #[test]
    fn cli_help_includes_description() {
        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("Static analyzer"));
    }

    #[test]
    fn cli_version_available() {
        let help = Cli::command().render_long_help().to_string();
        assert!(!help.is_empty());
    }

    #[test]
    fn build_fix_hints_emits_only_for_dead() {
        let report = crate::model::Report {
            schema_version: crate::SCHEMA_VERSION,
            tokenizer: "cl100k".into(),
            tool_version: crate::VERSION,
            scan_root: ".".into(),
            total_skills: 1,
            total_tokens: 0,
            budget: BudgetConfig {
                per_skill: 2000,
                total: None,
            },
            skills: vec![],
            issues: vec![
                crate::model::Issue::new(
                    crate::model::IssueKind::Dead,
                    crate::model::SkillId::new("a"),
                    "x",
                ),
                crate::model::Issue::new(
                    crate::model::IssueKind::Bloated,
                    crate::model::SkillId::new("b"),
                    "x",
                ),
            ],
            loadout: None,
        };
        let hints = build_fix_hints(&report);
        assert_eq!(hints.len(), 1);
        assert!(hints[0].contains("rm -r"));
    }
}
