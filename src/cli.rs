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
    /// Tokenizer: cl100k, o200k, llama3. Falls back to the
    /// `.skilldigest.toml` `[tokenizer] default` value when unset, and to
    /// `cl100k` when neither is set.
    #[arg(long, short = 't', global = true)]
    pub tokenizer: Option<String>,
    /// Per-skill token budget. Falls back to the `.skilldigest.toml`
    /// `[budget] per_skill` value when unset, and to `2000` when neither is
    /// set.
    #[arg(long, short = 'b', global = true)]
    pub budget: Option<usize>,
    /// Aggregate token budget across the whole library. When unset, falls
    /// back to the `.skilldigest.toml` `[budget] total` value. Exceeding
    /// this cap emits a SKILL012 `total-bloated` issue.
    #[arg(long, global = true)]
    pub total_budget: Option<usize>,
    /// No-op retained for forward compatibility — skilldigest is always
    /// fully offline (tokenizer data is bundled inside the binary and the
    /// tool never performs network I/O at scan time).
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

/// Default per-skill token budget when neither the CLI nor the config file
/// sets one. Matches the value documented in the README.
pub const DEFAULT_PER_SKILL_BUDGET: usize = 2000;

/// Default tokenizer when neither the CLI nor the config file sets one.
pub const DEFAULT_TOKENIZER: &str = "cl100k";

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
    // Load config up-front so we can honor the documented precedence:
    //   1. CLI flag
    //   2. Frontmatter `budget:`            (inside `rules::effective_budget`)
    //   3. `[overrides]` section
    //   4. `[budget]` / `[tokenizer]` section
    //   5. Built-in default
    // Without pre-loading the config here the CLI layer previously supplied a
    // hard-coded default of 2000 tokens / `cl100k` which then beat every
    // config value silently — the README precedence table was effectively a
    // lie for anyone who shipped a `.skilldigest.toml`.
    // When the user passes `--config <path>` explicitly, the path MUST exist.
    // Silently falling back to defaults on a typo (the previous behaviour)
    // produces "why isn't my config being applied?" bug reports — the same
    // category of footgun that motivated `deny_unknown_fields` on the toml
    // schema. Auto-discovery via `config::find_default` already filters by
    // `is_file()` so it cannot hit this branch with a bogus path.
    if let Some(ref explicit) = cli.config {
        if !explicit.is_file() {
            return Err(Error::Config {
                path: explicit.clone(),
                message: "config file does not exist (or is not a regular file)".to_string(),
            });
        }
    }
    let config_path = cli.config.clone().or_else(|| config::find_default(dir));
    let doc: Option<crate::config::ConfigDoc> = if let Some(ref path) = config_path {
        config::load(path)?
    } else {
        None
    };

    let per_skill = cli
        .budget
        .or_else(|| doc.as_ref().map(|d| d.budget.per_skill))
        .unwrap_or(DEFAULT_PER_SKILL_BUDGET);
    let total_budget = cli
        .total_budget
        .or_else(|| doc.as_ref().and_then(|d| d.budget.total));
    let tokenizer_name = cli
        .tokenizer
        .clone()
        .or_else(|| doc.as_ref().and_then(|d| d.tokenizer.default.clone()))
        .unwrap_or_else(|| DEFAULT_TOKENIZER.to_string());

    let tokenizer: Arc<dyn Tokenizer> = tokenize::by_name(&tokenizer_name)?;
    let policy = ScanPolicy {
        follow_symlinks: cli.follow_symlinks,
        max_file_size: cli.max_file_size,
        ..ScanPolicy::default()
    };

    let mut options = AuditOptions {
        root: dir.to_path_buf(),
        tokenizer,
        budget: BudgetConfig {
            per_skill,
            total: total_budget,
        },
        policy,
        overrides: BTreeMap::new(),
    };

    if let Some(ref d) = doc {
        options.apply_config(d);
    }

    if cli.verbose {
        eprintln!(
            "skilldigest: scanning {} with tokenizer={} per_skill_budget={} total_budget={:?} config={:?}",
            dir.display(),
            options.tokenizer.name(),
            options.budget.per_skill,
            options.budget.total,
            config_path,
        );
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
    // Honor the config-file `[tokenizer] default` even for the `tokens`
    // subcommand (which does not go through `build_options`). Config discovery
    // uses the parent directory of the target file.
    //
    // An explicit `--config <path>` must point at an existing file; silently
    // falling back to defaults on a typo would mismatch the `scan` /
    // `loadout` / `graph` precedent and re-open the same footgun the
    // `deny_unknown_fields` change closed for unknown TOML keys.
    if let Some(ref explicit) = cli.config {
        if !explicit.is_file() {
            return Err(Error::Config {
                path: explicit.clone(),
                message: "config file does not exist (or is not a regular file)".to_string(),
            });
        }
    }
    let config_parent = cli
        .config
        .clone()
        .or_else(|| file.parent().and_then(config::find_default));
    // Load any auto-discovered (or explicit) config file using the same
    // error-propagating path as `scan` / `loadout` / `graph`. The previous
    // `.ok().flatten()` chain silently swallowed TOML parse errors, so a
    // malformed `.skilldigest.toml` next to the target file made the
    // `tokens` subcommand quietly fall back to the default tokenizer
    // instead of refusing to run. That hides exactly the same class of
    // silent-fallback footgun the eval-D `deny_unknown_fields` and eval-K
    // explicit-config-missing fixes already closed for the other
    // subcommands. The `tokens` subcommand now bubbles up `Error::Config`
    // (exit 2) on malformed TOML, matching scan/loadout/graph behaviour.
    let doc: Option<crate::config::ConfigDoc> = match config_parent.as_deref() {
        Some(p) => config::load(p)?,
        None => None,
    };
    let tokenizer_name: String = cli
        .tokenizer
        .clone()
        .or_else(|| doc.as_ref().and_then(|d| d.tokenizer.default.clone()))
        .unwrap_or_else(|| DEFAULT_TOKENIZER.to_string());
    let tokenizer: Arc<dyn Tokenizer> = tokenize::by_name(&tokenizer_name)?;
    if cli.verbose {
        eprintln!(
            "skilldigest: tokens {} (tokenizer={})",
            file.display(),
            tokenizer.name()
        );
    }
    let bytes = fs::read(file).map_err(|e| Error::io(file, e))?;
    let parsed = crate::parse::parse_bytes(&bytes, file);
    // When not splitting by section, tokenize the *original* UTF-8 text
    // directly (including the `---` delimiters) rather than concatenating the
    // already-stripped frontmatter and body — the latter drops the markers
    // and gives a slightly different count than the file as a whole. Strip a
    // leading UTF-8 BOM so the count matches the `scan` subcommand's per-skill
    // total on BOM-prefixed files (the parser also strips BOM before
    // tokenization). Without this strip, the byte-order mark contributes an
    // extra token that the audit never sees.
    let stripped: &[u8] = bytes
        .strip_prefix(b"\xEF\xBB\xBF")
        .unwrap_or(bytes.as_slice());
    let whole_text = String::from_utf8_lossy(stripped);
    let (frontmatter_tokens, body_tokens) = if by_section {
        (
            tokenizer.count(&parsed.frontmatter_raw),
            tokenizer.count(&parsed.body),
        )
    } else {
        (0, tokenizer.count(&whole_text))
    };

    let rendered = match format {
        Format::Json => serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": crate::SCHEMA_VERSION,
            "tokenizer": tokenizer.name(),
            "tokenizer_version": tokenizer.version(),
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
                tokenizer_version: tokenizer.version(),
                tool_version: crate::VERSION,
                scan_root: file.to_path_buf(),
                total_skills: 1,
                total_tokens: frontmatter_tokens + body_tokens,
                budget: BudgetConfig {
                    per_skill: cli.budget.unwrap_or(DEFAULT_PER_SKILL_BUDGET),
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
            // Ensure file output ends with a trailing newline, matching the
            // behaviour the stdout path already guarantees. Without this,
            // `skilldigest ... > foo.json` and `skilldigest ... --output
            // foo.json` produced byte-different files for formats whose
            // renderer (notably `serde_json::to_string_pretty`) did not emit a
            // final `\n` — violating the README's "same input → byte-identical
            // output" determinism contract and tripping up POSIX-style tooling
            // (git, diff, wc -l) that assumes a trailing newline.
            if rendered.ends_with('\n') {
                fs::write(path, rendered).map_err(|e| Error::io(path, e))?;
            } else {
                let mut buf = String::with_capacity(rendered.len() + 1);
                buf.push_str(rendered);
                buf.push('\n');
                fs::write(path, &buf).map_err(|e| Error::io(path, e))?;
            }
        }
        None => {
            // Treat `BrokenPipe` as a clean termination rather than an
            // operational error. Piping `skilldigest ... | head` must not
            // print `I/O error on <stdout>` and exit 2 — that pattern is a
            // first-principles Unix expectation and every well-behaved CLI
            // silently stops when the downstream reader closes the pipe.
            let mut stdout = std::io::stdout().lock();
            if let Err(e) = stdout.write_all(rendered.as_bytes()) {
                if e.kind() == std::io::ErrorKind::BrokenPipe {
                    return Ok(());
                }
                return Err(Error::io(PathBuf::from("<stdout>"), e));
            }
            if !rendered.ends_with('\n') {
                // Ignore trailing-newline write failures — if the first
                // write succeeded but this one fails with BrokenPipe, the
                // downstream consumer already closed the pipe and we have
                // nothing useful to report.
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
        assert_eq!(cli.tokenizer.as_deref(), Some("o200k"));
        assert_eq!(cli.budget, Some(3000));
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
