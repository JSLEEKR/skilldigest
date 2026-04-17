# skilldigest

[![for-the-badge](https://img.shields.io/badge/skilldigest-1.0.0-blue?style=for-the-badge)](https://github.com/JSLEEKR/skilldigest/releases)
[![language](https://img.shields.io/badge/language-rust-orange?style=for-the-badge)](https://www.rust-lang.org/)
[![edition](https://img.shields.io/badge/edition-2021-lightgrey?style=for-the-badge)](https://doc.rust-lang.org/edition-guide/rust-2021/index.html)
[![MSRV](https://img.shields.io/badge/MSRV-1.75-informational?style=for-the-badge)](https://releases.rs/)
[![license](https://img.shields.io/badge/license-MIT-brightgreen?style=for-the-badge)](./LICENSE)
[![platform](https://img.shields.io/badge/platform-linux%20%7C%20macOS%20%7C%20windows-purple?style=for-the-badge)](#installation)
[![status](https://img.shields.io/badge/status-v1-success?style=for-the-badge)](./CHANGELOG.md)
[![tokenizer](https://img.shields.io/badge/tokenizer-cl100k%20%7C%20o200k%20%7C%20llama3-red?style=for-the-badge)](#tokenizers)
[![output](https://img.shields.io/badge/output-text%20%7C%20json%20%7C%20sarif%20%7C%20markdown%20%7C%20dot-darkgreen?style=for-the-badge)](#output-formats)

> **skilldigest** is a static analyzer for AI coding-assistant skill libraries
> (`SKILL.md`, `AGENTS.md`, `.cursorrules`, `CLAUDE.md`, agent plugins, etc.).
> It walks a directory of skills, measures per-skill token cost with a
> tiktoken-compatible BPE, builds a reference graph, and reports
> **dead**, **bloated**, **conflicting**, **stale**, and **cyclic** skills,
> plus a recommended **loadout** for a given task tag. Single static Rust
> binary. SARIF output drops straight into GitHub code-scanning.

---

## Table of contents

- [Why this exists](#why-this-exists)
- [Features](#features)
- [Installation](#installation)
- [Quick start](#quick-start)
- [CLI reference](#cli-reference)
  - [Global flags](#global-flags)
  - [`scan`](#scan-subcommand)
  - [`tokens`](#tokens-subcommand)
  - [`loadout`](#loadout-subcommand)
  - [`graph`](#graph-subcommand)
- [Output formats](#output-formats)
  - [JSON schema](#json-schema)
  - [SARIF 2.1.0](#sarif-210)
  - [Markdown for PR comments](#markdown-for-pr-comments)
- [Exit codes](#exit-codes)
- [Configuration file](#configuration-file)
- [Tokenizers](#tokenizers)
- [Rule catalogue](#rule-catalogue)
- [CI integration (GitHub Actions)](#ci-integration-github-actions)
- [Performance](#performance)
- [Determinism and reproducibility](#determinism-and-reproducibility)
- [Security and robustness](#security-and-robustness)
- [Comparison with other JSLEEKR tools](#comparison-with-other-jsleekr-tools)
- [Architecture](#architecture)
- [Development](#development)
- [Roadmap](#roadmap)
- [Contributing](#contributing)
- [License](#license)

---

## Why this exists

AI coding-assistant skill libraries have *exploded* in 2026. A partial list:

| Project | Skills | Stars |
|---------|-------:|------:|
| `antigravity-awesome-skills` | 1,400+ | 33,455 |
| `Vibe-Skills` | 340+ | 1,535 |
| `claude-skills` | 232+ | 11,401 |
| `awesome-claude-code` | 190+ | 39,123 |
| `oh-my-claudecode` | many | 29,372 |

Every one of them ships as a giant directory of markdown. Nobody knows:

- Which skills are **actually referenced** by an index/manifest and which are dead code?
- Which skills **exceed the token budget** of the target model?
- Which skills **contradict each other** (e.g. one says "MUST use `Bash(jq)`", another says "MUST NOT")?
- Which skills link to **files that no longer exist**?
- Given a task tag `refactor-tests`, **which minimal loadout** fits in 10k tokens?

`skilldigest` answers all five. Adjacent tools do not:

- `skillpack` — packages/locks skills, doesn't audit them.
- `agentlint` — validates agent *config* files (YAML/JSON), not skill *bodies* (markdown).
- `tokencost` — counts tokens per *prompt*, not per skill-library entry.
- `rtk` — runtime token *reducer*, not a static analyzer.

skilldigest is the missing piece. One Rust binary, no runtime deps, ships a
SARIF report your CI already knows how to upload.

## Features

- **Deterministic** — same input → byte-identical output.
- **Offline-first** — cl100k tokenizer data ships inside the binary.
- **Fast** — ~1,400 skills in < 2 s on an 8-core laptop (rayon parallel tokenization).
- **Multi-format** — text, JSON, SARIF 2.1, Markdown (PR comment), GraphViz dot.
- **Library-format agnostic** — detects `SKILL.md`, `AGENT.md`, `AGENTS.md`, `CLAUDE.md`, `GEMINI.md`, `.cursorrules`, `.cursor/rules/**`, `.claude/skills/**`, `plugin.toml`.
- **Rule catalogue** — 12 distinct issue classes with SARIF `ruleId`s (`SKILL001`–`SKILL012`).
- **Robust** — tolerates BOM, CRLF, mixed indent, malformed frontmatter, non-UTF-8 bytes.
- **Configurable** — `.skilldigest.toml` with per-skill budget overrides and ignore globs.
- **Zero `unsafe`** — `#![forbid(unsafe_code)]` at the crate root.

## Installation

### From source

```bash
git clone https://github.com/JSLEEKR/skilldigest
cd skilldigest
cargo build --release
./target/release/skilldigest --help
```

### Via `cargo install`

```bash
cargo install --path .
# or, once published:
cargo install skilldigest
```

### MSRV

`rust-version = "1.75"`. Any newer stable toolchain works.

### Platforms

Linux (x86_64, aarch64), macOS (x86_64, aarch64), Windows (x86_64).
One static binary per platform. No runtime dependencies.

## Quick start

```bash
# Audit a skill library
skilldigest scan ./my-skills

# Token count for a single file
skilldigest tokens ./my-skills/git/commit/SKILL.md

# Recommend a loadout for the "refactor" task tag
skilldigest loadout ./my-skills --tag refactor --max-tokens 8000

# Emit the skill reference graph as GraphViz dot
skilldigest graph ./my-skills --format dot | dot -Tsvg > skills.svg
```

## CLI reference

### Global flags

| Flag | Default | Description |
|------|---------|-------------|
| `-f, --format <FORMAT>` | `text` | Output format: `text`, `json`, `sarif`, `markdown`, `dot` |
| `-o, --output <FILE>` | stdout | Write output to a file |
| `-t, --tokenizer <NAME>` | `cl100k` | Tokenizer: `cl100k`, `o200k`, `llama3` |
| `-b, --budget <N>` | `2000` | Per-skill token budget |
| `--total-budget <N>` | none | Aggregate token budget across the library |
| `--offline` | off | Force fully offline (no cache reads/writes) |
| `--follow-symlinks` | off | Follow symlinks during scan |
| `--max-file-size <B>` | `1048576` | Skip files larger than this many bytes |
| `--config <FILE>` | auto | Path to `.skilldigest.toml` |
| `--no-color` | off | Disable ANSI color in text output |
| `-v, --verbose` | off | Log to stderr |
| `-q, --quiet` | off | Suppress non-error output |
| `--version` | — | Print version and exit |
| `--help` | — | Print help and exit |

### `scan` subcommand

```
skilldigest scan <DIR> [OPTIONS]
```

Runs a full audit. Emits a report in the chosen format. Returns exit-1 when
any error-severity issue is found.

```bash
skilldigest scan ./skills
skilldigest scan ./skills --format json --output report.json
skilldigest scan ./skills --format sarif --output skills.sarif.json
skilldigest scan ./skills --budget 3000 --no-color
skilldigest scan ./skills --fix-hint  # emit rm hints to stderr
```

### `tokens` subcommand

```
skilldigest tokens <FILE> [OPTIONS]
```

Count tokens in a single file.

```bash
skilldigest tokens ./skills/git/commit/SKILL.md
skilldigest tokens ./skills/git/commit/SKILL.md --by-section --format json
skilldigest tokens ./CLAUDE.md --tokenizer o200k
```

### `loadout` subcommand

```
skilldigest loadout <DIR> --tag <TAG> [--max-tokens <N>] [OPTIONS]
```

Score every skill for the tag and greedily select the highest-scoring
subset that fits in `--max-tokens`. Ties broken deterministically by skill ID.

```bash
skilldigest loadout ./skills --tag git --max-tokens 10000
skilldigest loadout ./skills --tag refactor --max-tokens 5000 --format json
```

### `graph` subcommand

```
skilldigest graph <DIR> [OPTIONS]
```

Emit the skill reference graph.

```bash
skilldigest graph ./skills --format dot | dot -Tsvg -o graph.svg
skilldigest graph ./skills --format json
skilldigest graph ./skills --format markdown   # embedded code-block
```

## Output formats

### JSON schema

Pretty-printed; stable snake_case keys; versioned via `schema_version`.

```json
{
  "schema_version": "skilldigest-report/1",
  "tokenizer": "cl100k_base",
  "tool_version": "1.0.0",
  "scan_root": "./skills",
  "total_skills": 12,
  "total_tokens": 18432,
  "budget": { "per_skill": 2000, "total": null },
  "skills": [
    {
      "id": "git/commit-style",
      "name": "commit-style",
      "path": "git/commit-style/SKILL.md",
      "tokens": { "frontmatter": 32, "body": 814, "total": 846 },
      "tags": ["git", "commit"],
      "refs_out": 2,
      "refs_in": 1,
      "issue_kinds": ["bloated"]
    }
  ],
  "issues": [
    {
      "kind": "dead",
      "severity": "warning",
      "skill": "legacy/old-thing",
      "message": "skill 'legacy/old-thing' is never referenced by any index or other skill",
      "location": { "path": "legacy/old-thing/SKILL.md", "line": 1, "column": 1 },
      "related": []
    }
  ],
  "loadout": null
}
```

### SARIF 2.1.0

The SARIF emitter is designed to be accepted by GitHub code-scanning
(`github/codeql-action/upload-sarif@v3`). Each issue class has its own rule
(`SKILL001` – `SKILL012`) with stable `id`, `name`, `shortDescription`,
`fullDescription`, `defaultConfiguration.level`, and `helpUri`.

```bash
skilldigest scan ./skills --format sarif --output skills.sarif.json
# …then in your GH Actions workflow:
#   - uses: github/codeql-action/upload-sarif@v3
#     with: { sarif_file: skills.sarif.json }
```

### Markdown for PR comments

```markdown
### skilldigest report
**12 skills**, **18,432 tokens** (cl100k_base), **3 issues** (1 error, 2 warning, 0 note)

| Skill | Tokens | Issues |
|-------|-------:|--------|
| `git/commit-style` | 846 | bloated |
| `legacy/old-thing` | 1204 | dead |

#### Issues

- [ERROR] **bloated** `git/commit-style` `git/commit-style/SKILL.md:1` — 846 tokens exceeds budget 500
- [warn] **dead** `legacy/old-thing` `legacy/old-thing/SKILL.md:1` — skill 'legacy/old-thing' is never referenced
```

## Exit codes

| Code | Meaning | Typical CI reaction |
|-----:|---------|---------------------|
| `0` | Scan completed, no error-severity issues | green build |
| `1` | Error-severity issues found | fail the build / block merge |
| `2` | Operational error (bad args, IO, malformed config) | fail the build as infra error |

## Configuration file

Drop a `.skilldigest.toml` at the scan root.

```toml
# Global token budgets
[budget]
per_skill = 2000
total = 40000

# Default tokenizer (CLI flag still wins)
[tokenizer]
default = "cl100k"

# Gitignore-style globs to skip
[ignore]
globs = ["archive/**", "drafts/**", "*.bak.md"]

# Per-skill overrides
[overrides."git/commit-style"]
budget = 3000

[overrides."onboarding/company-context"]
budget = 5000
```

Precedence (highest wins):

1. CLI flag
2. Frontmatter `budget:` on an individual skill
3. `[overrides]` section
4. `[budget]` section
5. Built-in default (2000)

## Tokenizers

| Name | Backed by | Offline? | Notes |
|------|-----------|----------|-------|
| `cl100k` | `tiktoken-rs::cl100k_base` | Yes (bundled) | GPT-4, Claude-ish. **Default.** |
| `o200k` | `tiktoken-rs::o200k_base` | Yes (bundled) | GPT-4o. |
| `llama3` | Deterministic word-piece approximation | Yes (algorithmic) | Within ~10% of real Llama 3 counts on English prose. Useful for *relative* comparisons. |

The llama3 backend is intentionally an approximation — we do not ship the
full HuggingFace `tokenizer.json` (which would require either a network
fetch or a ~20 MB binary bloat). The approximation is deterministic and
side-effect free; documented as approximate so downstream tooling knows
not to trust it for absolute billing.

## Rule catalogue

| Rule ID | Issue kind | Default severity | Description |
|---------|-----------|------------------|-------------|
| `SKILL001` | dead | warning | Skill never referenced by any index or other skill |
| `SKILL002` | bloated | **error** | Skill exceeds per-skill token budget |
| `SKILL003` | conflict | **error** | Two skills contain opposing rules about the same subject |
| `SKILL004` | stale | warning | A link or file reference points to a missing file |
| `SKILL005` | cycle | **error** | Reference cycle in the skill graph |
| `SKILL006` | oversize | **error** | File exceeds `--max-file-size` |
| `SKILL007` | non-utf8 | warning | File contained bytes that could not be decoded as UTF-8 |
| `SKILL008` | bad-frontmatter | warning | YAML frontmatter failed to parse |
| `SKILL009` | symlink | note | Symlink skipped (use `--follow-symlinks`) |
| `SKILL010` | duplicate | **error** | Two files produced the same normalized skill identifier |
| `SKILL011` | path-escape | warning | Discovered file canonicalised to a path outside the scan root (e.g. via a symlink target) |
| `SKILL012` | total-bloated | **error** | Aggregate library token cost exceeds `--total-budget` / `[budget] total` |

## CI integration (GitHub Actions)

```yaml
name: skill-digest

on:
  pull_request:
    paths:
      - '.claude/skills/**'
      - '.cursor/rules/**'
      - 'AGENTS.md'
      - 'CLAUDE.md'

jobs:
  skilldigest:
    runs-on: ubuntu-latest
    permissions:
      security-events: write  # required for upload-sarif
      contents: read
    steps:
      - uses: actions/checkout@v4

      - name: Install skilldigest
        run: |
          curl -L https://github.com/JSLEEKR/skilldigest/releases/latest/download/skilldigest-linux-amd64 -o /usr/local/bin/skilldigest
          chmod +x /usr/local/bin/skilldigest

      - name: Run skilldigest (SARIF)
        run: skilldigest scan . --format sarif --output skills.sarif.json || true

      - name: Upload SARIF to GitHub code-scanning
        uses: github/codeql-action/upload-sarif@v3
        with:
          sarif_file: skills.sarif.json
          category: skilldigest

      - name: Fail on any error-severity issue
        run: skilldigest scan . --no-color
```

Or drop it straight into a PR comment:

```yaml
      - name: Render Markdown report
        id: digest
        run: skilldigest scan . --format markdown > digest.md
      - name: Comment on PR
        uses: marocchino/sticky-pull-request-comment@v2
        with:
          path: digest.md
```

## Performance

On an 8-core x86_64 laptop with warm filesystem cache:

| Library size | Wall time |
|-------------:|----------:|
| 20 skills | ~5 ms |
| 200 skills | ~35 ms |
| 1,400 skills | < 2 s |

Run the bench yourself:

```bash
cargo bench --bench bench_scan
cargo bench --bench bench_tokenize
```

## Determinism and reproducibility

- All collections sorted before emit.
- Tokenizer version and schema version are stamped into every JSON/SARIF output.
- No timestamps anywhere in the output — runs at different times produce byte-identical files.
- Deterministic tie-breakers in the loadout recommender (integer math, no floats).

```bash
skilldigest scan ./skills --format json > a.json
skilldigest scan ./skills --format json > b.json
diff -u a.json b.json   # → empty
```

## Security and robustness

- **`#![forbid(unsafe_code)]`** at the crate root.
- **File-size cap** (1 MiB default) prevents memory blowup on malicious inputs.
- **Symlinks skipped by default** — reject path traversal via canonicalization.
- **UTF-8 strict** on the fast path (`simdutf8`), graceful fallback flags
  non-UTF-8 files instead of panicking.
- **No network I/O** at scan time — tokenizer data is bundled inside the binary.
- **No shell-outs** — no subprocess execution at any point.
- **Frontmatter YAML** is parsed in a bounded mode with `serde_yaml` and
  failures produce `bad-frontmatter` issues rather than halting the scan.

## Comparison with other JSLEEKR tools

| Tool | Round | Language | Scope | Unique to skilldigest |
|------|------:|----------|-------|-----------------------|
| `skillpack` | R81 | Go | Lockfile + install for skills | Token audit, dead-code detection |
| `agentlint` | R83 | TypeScript | Validate agent *config* files (JSON/YAML) | Operates on skill *bodies* (markdown) |
| `tokencost` | R54 | — | Tokens per prompt | Tokens **per skill** + library audit |
| `mcpbench` | R84 | Go | Benchmark MCP servers | Different category |
| `ragcheck` | R82 | Python | RAG eval harness | Different category |
| `agentmem` | — | — | Agent memory persistence | Different category |

Together, `skillpack` (R81) + `agentlint` (R83) + `skilldigest` (R85) cover
packaging, config validation, and content analysis of AI-agent skill
libraries — three non-overlapping quality gates.

## Architecture

```
+------------------+
|  CLI (clap v4)   |
+---------+--------+
          |
          v
+---------+---------+      +----------------+
|  Scanner (walkdir)|---->| Parser (md+yaml)|
+---------+---------+      +-------+--------+
          |                        |
          |                        v
          |                 +------+------+
          |                 |  Skill AST  |
          |                 +------+------+
          |                        |
          v                        v
+---------+----------+      +------+---------+
| Tokenizer pool     |<---->| Graph (petgraph)|
| (tiktoken-rs)      |      +------+---------+
+---------+----------+             |
          |                        v
          |                 +------+---------+
          |                 |  Audit rules   |
          |                 +------+---------+
          |                        |
          v                        v
+-------------------+      +---------------+
|  Output emitter   |<-----+  Issue list   |
|  (text/json/sarif/md)  | +---------------+
+-------------------+
```

Module layout (`src/`):

| Module | Purpose |
|--------|---------|
| `cli.rs` | clap v4 derive, subcommand dispatch |
| `scan.rs` | directory walk, file classification |
| `parse.rs` | markdown + frontmatter parser |
| `model.rs` | core data types |
| `tokenize.rs` | cl100k / o200k / llama3-approx tokenizers |
| `graph.rs` | petgraph-backed reference graph |
| `rules.rs` | bloat / conflict / stale / duplicate / dead detectors |
| `audit.rs` | orchestration |
| `loadout.rs` | task-tag loadout recommender |
| `config.rs` | `.skilldigest.toml` loader |
| `output/*` | text / json / sarif / markdown / dot renderers |
| `error.rs` | canonical error type + exit codes |

## Development

```bash
# Full test suite
cargo test --all-features

# Clippy — strict, warnings = errors
cargo clippy --all-targets --all-features -- -D warnings

# Format check
cargo fmt --check

# Benchmarks
cargo bench
```

Test count at v1.0.0: **200+ tests** (unit + integration + doc).

## Roadmap

Out of scope for v1 (tracked for future rounds):

- **LLM-assisted conflict detection** — v1 is structural only.
- **`--fix` auto-repair** — v1 only emits shell-hints via `--fix-hint`.
- **VS Code / Cursor extension** — may ship as a separate project.
- **Integration with `skillpack` lockfile** — cross-reference pinned skill versions.
- **Language-specific rule packs** — currently tool-detection is hard-coded
  to Claude-style tool names; a plugin system would allow Cursor/Copilot
  tool-name dictionaries.

## Contributing

1. Fork the repo.
2. Create a topic branch (`git checkout -b feat/your-feature`).
3. Make sure `cargo fmt --check`, `cargo clippy -- -D warnings`,
   `cargo test --all-features` all pass.
4. Add tests for any new behavior.
5. Open a PR with a clear description of the change.

Commit messages loosely follow conventional-commits (`feat:`, `fix:`,
`docs:`, `refactor:`). The pre-commit checklist is simply the three
commands above.

## License

MIT © 2026 JSLEEKR. See [LICENSE](./LICENSE).
