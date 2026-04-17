# Changelog

All notable changes to this project will be documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.0] - 2026-04-17

### Added
- Initial release of `skilldigest` — a static analyzer for AI coding-assistant
  skill libraries.
- `scan` subcommand: full audit of a skill-library directory with 10 distinct
  issue classes (dead, bloated, conflict, stale, cycle, oversize, non-utf8,
  bad-frontmatter, symlink, duplicate).
- `tokens` subcommand: token count for a single file with optional split
  between frontmatter and body.
- `loadout` subcommand: task-tag-based skill loadout recommendation using a
  deterministic greedy selection respecting a per-call token budget.
- `graph` subcommand: GraphViz dot and JSON emission of the skill reference
  graph.
- Three tokenizers: `cl100k` (default, GPT-4 / Claude-ish), `o200k`
  (GPT-4o), and `llama3` (deterministic approximation).
- Five output formats: text, JSON, SARIF 2.1.0, Markdown (for PR comments),
  GraphViz dot.
- Stable JSON schema `skilldigest-report/1` with explicit `schema_version`
  and `tokenizer_version` fields.
- `.skilldigest.toml` configuration with budget, tokenizer, ignore-globs,
  and per-skill overrides.
- Deterministic ordering of all collections in the output.
- Offline-first design: cl100k/o200k BPE tables are bundled via
  `tiktoken-rs`.
- `#![forbid(unsafe_code)]` at the crate root.

[1.0.0]: https://github.com/JSLEEKR/skilldigest/releases/tag/v1.0.0
