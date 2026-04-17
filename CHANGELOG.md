# Changelog

All notable changes to this project will be documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.0] - 2026-04-17

### Added
- Initial release of `skilldigest` — a static analyzer for AI coding-assistant
  skill libraries.
- `scan` subcommand: full audit of a skill-library directory with **12 distinct
  issue classes** (`SKILL001` – `SKILL012`): dead, bloated, conflict, stale,
  cycle, oversize, non-utf8, bad-frontmatter, symlink, duplicate,
  **path-escape** (SKILL011: symlink target canonicalises outside the scan
  root), and **total-bloated** (SKILL012: aggregate library tokens exceed
  `--total-budget` / `[budget] total`).
- `tokens` subcommand: token count for a single file with optional split
  between frontmatter and body. UTF-8 BOM is stripped before tokenization so
  the `tokens` and `scan` subcommands report identical counts on BOM-prefixed
  files.
- `loadout` subcommand: task-tag-based skill loadout recommendation using a
  deterministic greedy selection respecting a per-call token budget. Ties
  broken by skill-id alphabetical order.
- `graph` subcommand: GraphViz dot and JSON emission of the skill reference
  graph.
- Three tokenizers: `cl100k` (default, GPT-4 / Claude-ish), `o200k`
  (GPT-4o), and `llama3` (deterministic approximation).
- Five output formats: text, JSON, SARIF 2.1.0, Markdown (for PR comments),
  GraphViz dot.
- Stable JSON schema `skilldigest-report/1` with explicit `schema_version`
  and `tokenizer_version` fields (the latter embeds the library crate id
  alongside the logical tokenizer name so downstream consumers can detect
  silent BPE drift).
- `.skilldigest.toml` configuration with budget (`per_skill` + `total`),
  tokenizer, ignore-globs, and per-skill overrides. Unknown keys are
  rejected up front with a clear error message so typos never silently
  disable a rule. Config values apply transitively to the `scan`, `tokens`,
  `loadout`, and `graph` subcommands. Documented precedence (highest wins):
  CLI flag → frontmatter `budget:` → `[overrides]` → `[budget]` → built-in
  default.
- Global flags `--verbose` / `-v` (stderr log line with the active
  tokenizer, budget, and config file) and `--offline` (documented no-op
  retained for forward compatibility — skilldigest never performs network
  I/O at scan time because tokenizer data is bundled in the binary).
- Rule-extraction parser ignores any `MUST` / `MUST NOT` prefix that
  appears inside a fenced code block (` ``` ` or `~~~`) so sample /
  documentation snippets do not produce false-positive conflict issues.
- Per-file metadata / read failures now emit a warning-level issue and
  continue rather than aborting the entire scan with operational exit
  code 2, reserving exit 2 for genuine CLI-level errors (bad scan root,
  bad flags, malformed config).
- Deterministic ordering of all collections in the output; every cycle
  participant carries the `cycle` kind in its `issue_kinds` summary (not
  just the canonical primary skill).
- Offline-first design: cl100k/o200k BPE tables are bundled via
  `tiktoken-rs`.
- `#![forbid(unsafe_code)]` at the crate root.

[1.0.0]: https://github.com/JSLEEKR/skilldigest/releases/tag/v1.0.0
