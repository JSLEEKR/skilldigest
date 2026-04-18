//! Markdown + YAML frontmatter parser.
//!
//! Robustness is the main design constraint here:
//! - Tolerate BOM, CRLF, mixed indentation.
//! - Surface non-UTF-8 as a *warning* rather than a panic.
//! - Never crash on malformed frontmatter — log a [`Warning`] and continue
//!   with an empty `Frontmatter`.
//! - Extract references (links, mentions, tool invocations) via a
//!   lightweight scan of the pulldown-cmark event stream.

use std::path::{Path, PathBuf};

use pulldown_cmark::{CowStr, Event, Options, Parser, Tag, TagEnd};

use crate::model::{
    Frontmatter, Modal, Rule, RuleKind, Skill, SkillId, SkillRef, TokenCounts, Warning, WarningKind,
};
use crate::tokenize::Tokenizer;

/// Result of parsing a single file (without tokenization yet applied — the
/// caller fills in the token counts using whatever tokenizer is active).
#[derive(Debug)]
pub struct ParsedSkill {
    /// Stable identifier.
    pub id: SkillId,
    /// Display name.
    pub name: String,
    /// Path relative to scan root.
    pub path: PathBuf,
    /// Parsed frontmatter.
    pub frontmatter: Frontmatter,
    /// Raw frontmatter block (for token counting).
    pub frontmatter_raw: String,
    /// Markdown body (post-frontmatter, normalised).
    pub body: String,
    /// Outgoing references.
    pub refs: Vec<SkillRef>,
    /// Structural rules.
    pub rules: Vec<Rule>,
    /// Collected tag list.
    pub tags: Vec<String>,
    /// Parse warnings.
    pub warnings: Vec<Warning>,
    /// Body byte length.
    pub body_bytes: usize,
}

/// Parse a single file's raw bytes into a [`ParsedSkill`].
///
/// `scan_root_relative` should be the file's path **relative to the scan
/// root**, used to derive the skill ID.
pub fn parse_bytes(raw: &[u8], scan_root_relative: &Path) -> ParsedSkill {
    let (text, mut warnings) = decode_bytes(raw, scan_root_relative);
    parse_text(&text, scan_root_relative, &mut warnings)
}

/// Parse a UTF-8 string into a [`ParsedSkill`]. Public for tests.
pub fn parse_text(
    text: &str,
    scan_root_relative: &Path,
    warnings: &mut Vec<Warning>,
) -> ParsedSkill {
    let text = text.replace("\r\n", "\n");
    if text.contains('\t') && text.contains("  ") {
        // mixed indent warning is best-effort
        if !warnings.iter().any(|w| w.kind == WarningKind::MixedIndent) {
            warnings.push(Warning {
                kind: WarningKind::MixedIndent,
                message: "file mixes tab and space indentation".into(),
            });
        }
    }
    let (frontmatter_raw, body) = split_frontmatter(&text);
    let frontmatter = parse_frontmatter(&frontmatter_raw, warnings);

    let id = derive_skill_id(scan_root_relative);
    let name = frontmatter
        .name
        .clone()
        .or_else(|| derive_name_from_path(scan_root_relative))
        .unwrap_or_else(|| id.as_str().to_string());

    let (refs, rules) = extract_refs_and_rules(&body);
    let mut tags = frontmatter.tags.clone();
    extract_inline_tags(&body, &mut tags);

    ParsedSkill {
        id,
        name,
        path: scan_root_relative.to_path_buf(),
        frontmatter,
        frontmatter_raw,
        body: body.clone(),
        refs,
        rules,
        tags,
        warnings: std::mem::take(warnings),
        body_bytes: body.len(),
    }
}

/// Finalise a [`ParsedSkill`] into a [`Skill`] by applying a tokenizer.
pub fn finalise(parsed: ParsedSkill, tokenizer: &dyn Tokenizer) -> Skill {
    let frontmatter_tokens = tokenizer.count(&parsed.frontmatter_raw);
    let body_tokens = tokenizer.count(&parsed.body);
    let tokens = TokenCounts::new(frontmatter_tokens, body_tokens);

    Skill {
        id: parsed.id,
        name: parsed.name,
        path: parsed.path,
        frontmatter: parsed.frontmatter,
        tokens,
        refs: parsed.refs,
        rules: parsed.rules,
        tags: dedup_in_order(parsed.tags),
        warnings: parsed.warnings,
        body_bytes: parsed.body_bytes,
    }
}

fn decode_bytes(raw: &[u8], path: &Path) -> (String, Vec<Warning>) {
    let mut warnings = Vec::new();
    // strip UTF-8 BOM
    let (stripped, bom) = if raw.starts_with(b"\xEF\xBB\xBF") {
        (&raw[3..], true)
    } else {
        (raw, false)
    };
    if bom {
        warnings.push(Warning {
            kind: WarningKind::Bom,
            message: format!("BOM stripped from {}", path.display()),
        });
    }
    if raw.contains(&b'\r') {
        warnings.push(Warning {
            kind: WarningKind::Crlf,
            message: format!("CRLF line endings in {}", path.display()),
        });
    }
    match simdutf8::basic::from_utf8(stripped) {
        Ok(s) => (s.to_string(), warnings),
        Err(_) => {
            warnings.push(Warning {
                kind: WarningKind::NonUtf8Recovered,
                message: format!(
                    "non-UTF-8 bytes in {}; decoded with replacement",
                    path.display()
                ),
            });
            (String::from_utf8_lossy(stripped).into_owned(), warnings)
        }
    }
}

/// Split a markdown file into (frontmatter_yaml, body).
///
/// Frontmatter is recognised only when the file begins with a line of
/// exactly `---` and a matching closing line appears later.
fn split_frontmatter(text: &str) -> (String, String) {
    let mut iter = text.splitn(2, '\n');
    let first = iter.next().unwrap_or("");
    let rest = iter.next().unwrap_or("");
    if first.trim() != "---" {
        return (String::new(), text.to_string());
    }
    // look for closing --- on its own line
    let mut depth_line = 0usize;
    let mut close_idx = None;
    for (idx, line) in rest.split_inclusive('\n').enumerate() {
        depth_line = idx;
        if line.trim_end_matches('\n').trim_end() == "---" {
            close_idx = Some(idx);
            break;
        }
    }
    let _ = depth_line;
    match close_idx {
        Some(idx) => {
            let mut front = String::new();
            let mut body = String::new();
            for (i, line) in rest.split_inclusive('\n').enumerate() {
                if i < idx {
                    front.push_str(line);
                } else if i == idx {
                    // skip the closing marker line
                } else {
                    body.push_str(line);
                }
            }
            (front, body)
        }
        None => (String::new(), text.to_string()),
    }
}

fn parse_frontmatter(raw: &str, warnings: &mut Vec<Warning>) -> Frontmatter {
    if raw.trim().is_empty() {
        return Frontmatter::default();
    }
    match serde_yaml::from_str::<Frontmatter>(raw) {
        Ok(f) => f,
        Err(e) => {
            warnings.push(Warning {
                kind: WarningKind::FrontmatterYamlError,
                message: format!("frontmatter yaml error: {e}"),
            });
            Frontmatter::default()
        }
    }
}

fn derive_skill_id(path: &Path) -> SkillId {
    let mut s = path.to_string_lossy().into_owned().replace('\\', "/");
    // strip common skill filenames
    for suffix in [
        "/SKILL.md",
        "/skill.md",
        "/AGENT.md",
        "/agent.md",
        "/README.md",
        "/index.md",
    ] {
        if let Some(stripped) = s.strip_suffix(suffix) {
            s = stripped.to_string();
            return SkillId::new(s);
        }
    }
    // Otherwise strip extension
    if let Some(stem) = Path::new(&s).with_extension("").to_str() {
        return SkillId::new(stem);
    }
    SkillId::new(s)
}

fn derive_name_from_path(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_string_lossy();
    if stem == "SKILL" || stem == "skill" || stem == "README" {
        if let Some(parent) = path.parent().and_then(|p| p.file_name()) {
            return Some(parent.to_string_lossy().into_owned());
        }
    }
    Some(stem.into_owned())
}

fn extract_refs_and_rules(body: &str) -> (Vec<SkillRef>, Vec<Rule>) {
    let mut refs = Vec::new();
    let mut rules = Vec::new();
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_FOOTNOTES);

    // Single pass over the pulldown-cmark event stream. We collect:
    //
    //   * `link_refs` (Tag::Link)
    //   * `tool_invocations` and `@mentions` from Text/Code events outside
    //     any code block
    //   * `code_block_ranges`: byte ranges of every Tag::CodeBlock
    //     (fenced + indented). pulldown-cmark already enforces every
    //     CommonMark §4.4/§4.5 edge case the previous hand-rolled walkers
    //     re-implemented (≤3-space fence-opener indent, tab-as-indent,
    //     same-character closer, no info string on closer, no backticks
    //     in backtick-fence info strings, indented-code-after-heading,
    //     etc. — see cycle V through CC regressions).
    //   * `code_span_ranges`: byte ranges of every inline `Code` event so
    //     the per-line wiki walker can mask `\`[[foo]]\`` examples.
    //
    // The two line-based extractors below (rules, wiki-links) then consult
    // the byte-range index — they no longer hand-roll fence detection. Any
    // future CommonMark edge case is automatically inherited from
    // pulldown-cmark instead of having to be patched into two parallel
    // state machines that drift out of lockstep.
    let parser = Parser::new_ext(body, options).into_offset_iter();

    let mut in_link_dest: Option<String> = None;
    let mut code_block_depth: usize = 0;
    let mut code_block_ranges: Vec<(usize, usize)> = Vec::new();
    let mut code_span_ranges: Vec<(usize, usize)> = Vec::new();
    let mut current_code_block_start: Option<usize> = None;

    for (event, range) in parser {
        match event {
            Event::Start(Tag::CodeBlock(_kind)) => {
                if code_block_depth == 0 {
                    current_code_block_start = Some(range.start);
                }
                code_block_depth += 1;
            }
            Event::End(TagEnd::CodeBlock) => {
                code_block_depth = code_block_depth.saturating_sub(1);
                if code_block_depth == 0 {
                    if let Some(start) = current_code_block_start.take() {
                        code_block_ranges.push((start, range.end));
                    }
                }
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                in_link_dest = Some(dest_url.to_string());
            }
            Event::End(TagEnd::Link) => {
                if let Some(dest) = in_link_dest.take() {
                    add_link_ref(&mut refs, &dest);
                }
            }
            // `Event::Code` is an inline code span. Tool-invocation extraction
            // here is intentional because backtick-quoted `Bash(ls)` tokens
            // are the documented way authors flag tool calls. The match guard
            // suppresses extraction inside fenced/indented code blocks, where
            // pulldown-cmark also emits Code/Text events for the literal
            // contents.
            Event::Code(ref c) => {
                code_span_ranges.push((range.start, range.end));
                if code_block_depth == 0 {
                    scan_tool_invocation(c, &mut refs);
                }
            }
            Event::Text(ref t) if code_block_depth == 0 => {
                scan_mentions_and_files(t, &mut refs);
                scan_tool_invocation(t, &mut refs);
            }
            _ => {}
        }
    }

    // Sort code-block ranges by start so the membership test is a simple
    // linear scan (the input is already roughly sorted; dedupe just in case).
    code_block_ranges.sort_unstable();
    code_span_ranges.sort_unstable();

    // Walk lines once. For each line:
    //   * Compute its byte range in `body`.
    //   * If the line falls inside a code-block range, skip both rule and
    //     wiki-link extraction — pulldown-cmark has already classified the
    //     line as code (fenced or indented). Sample / how-to documentation
    //     frequently contains "MUST NOT use X" or "[[other-skill]]" examples
    //     inside code blocks; treating them as real rules / mentions
    //     produces the false-positive `conflict` and phantom-edge bugs the
    //     cycle V–CC fixes were designed to prevent.
    //   * Otherwise, try `extract_rule_from_line` and run the wiki-link
    //     scanner with inline-code-span byte ranges masked out.
    let mut byte_offset = 0usize;
    for (i, line) in body.lines().enumerate() {
        let line_start = byte_offset;
        let line_end = line_start + line.len();
        // Account for the `\n` consumed by `lines()`. The last line may have
        // no trailing newline; we never need exact match on the final byte
        // so the off-by-one is harmless.
        byte_offset = line_end + 1;

        if byte_in_any_range(line_start, &code_block_ranges)
            || byte_in_any_range(line_end.saturating_sub(1), &code_block_ranges)
        {
            continue;
        }

        if let Some(rule) = extract_rule_from_line(line, i + 1) {
            rules.push(rule);
        }

        scan_wiki_links_in_line(line, line_start, &code_span_ranges, &mut refs);
    }

    refs.sort_by_key(|r| format!("{r:?}"));
    refs.dedup_by_key(|r| format!("{r:?}"));

    (refs, rules)
}

/// Return true when `byte` falls inside any half-open `[start, end)` range
/// in `ranges` (which must be sorted by `start`). Linear scan; the per-file
/// range count is tiny in practice (≤ a few dozen).
fn byte_in_any_range(byte: usize, ranges: &[(usize, usize)]) -> bool {
    ranges.iter().any(|(s, e)| byte >= *s && byte < *e)
}

fn add_link_ref(refs: &mut Vec<SkillRef>, dest: &str) {
    if dest.is_empty() {
        return;
    }
    if dest.starts_with("http://") || dest.starts_with("https://") || dest.starts_with("mailto:") {
        return;
    }
    if dest.starts_with('#') {
        return;
    }
    // Strip `#fragment` and `?query` from the destination before resolving on
    // disk. Markdown links like `[t](./foo.md#section)` and
    // `[t](./foo.md?v=1)` should resolve to `./foo.md` — the fragment is a
    // browser-side anchor and the query is a versioning hint, neither of
    // which is part of the file path. Without this strip, every deep-link
    // (an extremely common idiom in skill libraries that link to a heading
    // inside another skill) was reported as a `stale` broken link, polluting
    // CI output with false positives. Mirrors the same fragment-strip we
    // already apply to wiki-links via `wiki_link_target`.
    let path_only = strip_link_modifiers(dest);
    if path_only.is_empty() {
        return;
    }
    let path = PathBuf::from(path_only);
    refs.push(SkillRef::Link {
        target: path,
        exists: false, // filled in later by the scanner
    });
}

/// Strip `#fragment` and `?query` from a markdown link destination so the
/// remainder is a plain filesystem path. Returns the input unchanged when
/// neither sigil is present.
fn strip_link_modifiers(dest: &str) -> &str {
    // `?` and `#` may legally appear inside a filename only if percent-encoded
    // (per the URI spec) — anything literal terminates the path component.
    let cut = dest
        .find('#')
        .map(|i| dest.find('?').map_or(i, |j| i.min(j)))
        .or_else(|| dest.find('?'));
    match cut {
        Some(i) => &dest[..i],
        None => dest,
    }
}

fn scan_tool_invocation(text: &CowStr<'_>, refs: &mut Vec<SkillRef>) {
    // matches: Bash(ls), Write(*), Edit(path/to/file), Read(...)
    // We avoid a regex crate dep and hand-roll a small parser.
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_uppercase() {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'(' {
                let name = &text[start..i];
                // find matching closing paren (no nesting)
                let arg_start = i + 1;
                let mut depth = 1;
                let mut j = arg_start;
                while j < bytes.len() && depth > 0 {
                    match bytes[j] {
                        b'(' => depth += 1,
                        b')' => depth -= 1,
                        _ => {}
                    }
                    if depth == 0 {
                        break;
                    }
                    j += 1;
                }
                if depth == 0 && j > arg_start {
                    let args = &text[arg_start..j];
                    if is_tool_name(name) {
                        refs.push(SkillRef::Tool {
                            name: name.to_string(),
                            args: args.to_string(),
                        });
                    }
                    i = j + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
}

fn is_tool_name(s: &str) -> bool {
    matches!(
        s,
        "Bash"
            | "Read"
            | "Write"
            | "Edit"
            | "Glob"
            | "Grep"
            | "Task"
            | "WebFetch"
            | "WebSearch"
            | "NotebookEdit"
            | "TodoWrite"
    )
}

fn scan_mentions_and_files(text: &str, refs: &mut Vec<SkillRef>) {
    // @mention-style: @foo, @foo/bar  (no whitespace, min len 2)
    // [[wiki]] links: [[foo]] or [[foo/bar]]
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'@' && (i == 0 || !is_word_byte(bytes[i - 1])) {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len()
                && (is_word_byte(bytes[j])
                    || bytes[j] == b'/'
                    || bytes[j] == b'-'
                    || bytes[j] == b'_')
            {
                j += 1;
            }
            if j > start {
                let id = &text[start..j];
                // Filter email-like tokens: preceding character alphanumeric
                // handled already via is_word_byte.
                refs.push(SkillRef::Mention {
                    skill_id: SkillId::new(id),
                });
            }
            i = j;
            continue;
        }
        if bytes[i] == b'[' && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            let start = i + 2;
            if let Some(rel) = text[start..].find("]]") {
                let inner = &text[start..start + rel];
                if let Some(target) = wiki_link_target(inner) {
                    refs.push(SkillRef::Mention {
                        skill_id: SkillId::new(target),
                    });
                }
                i = start + rel + 2;
                continue;
            }
        }
        i += 1;
    }
}

/// Parse the inner of a `[[...]]` wiki link into the target skill id, or
/// return `None` if the inner is empty, multi-line, or clearly prose.
///
/// Supports Obsidian-style `[[target|display text]]` aliases by returning
/// only the `target` portion. Everything after the first `|` is treated as
/// the human-facing display label and ignored for reference resolution.
///
/// Also strips an optional `#heading` anchor: `[[foo#usage]]` resolves to the
/// `foo` skill id rather than the (unresolvable) `foo#usage` literal. Without
/// this strip, authors who deep-link into a section heading (an idiom
/// Obsidian / Dendron / many wiki tools bake in) had their cross-references
/// silently dropped and the target skill wrongly flagged dead.
fn wiki_link_target(inner: &str) -> Option<&str> {
    if inner.is_empty() || inner.contains('\n') {
        return None;
    }
    // Obsidian allows `[[target|display text]]`. Split on the first `|` and
    // take the target half. The target itself must not contain whitespace —
    // a space inside the target is almost certainly prose rather than a
    // wiki link.
    let target = inner.split('|').next().unwrap_or("").trim();
    // Strip `#heading` anchor. `[[foo#sec]]` still resolves to `foo`.
    let target = target.split('#').next().unwrap_or("").trim_end();
    if target.is_empty() || target.contains(char::is_whitespace) {
        return None;
    }
    Some(target)
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Scan a single (non-code-block) line for `[[wiki]]` mentions, skipping any
/// match that falls inside a pulldown-cmark inline code-span byte range. The
/// raw `[[...]]` literal in a documentation example like
/// `\`see [[other]]\`` is illustrative, not a real cross-reference, so it
/// must not produce a Mention.
///
/// `line_offset` is the byte position of `line` within the parsed body; it
/// is used to translate per-line offsets into the absolute byte coordinates
/// the `code_span_ranges` index uses. `code_span_ranges` is the sorted list
/// of `(start, end)` pairs collected from `Event::Code` during the
/// pulldown-cmark pass in `extract_refs_and_rules`.
fn scan_wiki_links_in_line(
    line: &str,
    line_offset: usize,
    code_span_ranges: &[(usize, usize)],
    refs: &mut Vec<SkillRef>,
) {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            let start = i + 2;
            if let Some(rel) = line[start..].find("]]") {
                let abs_open = line_offset + i;
                let abs_close = line_offset + start + rel + 1;
                let inside_code = byte_in_any_range(abs_open, code_span_ranges)
                    || byte_in_any_range(abs_close, code_span_ranges);
                if !inside_code {
                    let inner = &line[start..start + rel];
                    if let Some(target) = wiki_link_target(inner) {
                        refs.push(SkillRef::Mention {
                            skill_id: SkillId::new(target),
                        });
                    }
                }
                i = start + rel + 2;
                continue;
            }
        }
        i += 1;
    }
}

/// True when `line` looks like an indented code-block line per CommonMark:
/// 4+ leading spaces or a leading tab, and at least one non-whitespace
/// character. Empty / whitespace-only lines are excluded so that blank
/// separators between code blocks don't themselves count as code.
///
/// The runtime extractor no longer consults this helper — pulldown-cmark's
/// event stream already emits `Tag::CodeBlock(CodeBlockKind::Indented)` for
/// these lines and the byte-range index in `extract_refs_and_rules` makes
/// the membership test trivial. The helper is retained as a `cfg(test)`
/// CommonMark §4.4 line-classifier the regression test pins.
#[cfg(test)]
fn is_indented_code_line(line: &str) -> bool {
    if line.trim().is_empty() {
        return false;
    }
    let mut spaces = 0usize;
    for b in line.bytes() {
        match b {
            b' ' => spaces += 1,
            b'\t' => return true,
            _ => break,
        }
        if spaces >= 4 {
            return true;
        }
    }
    spaces >= 4
}

fn extract_rule_from_line(line: &str, line_number: usize) -> Option<Rule> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let upper = trimmed.to_ascii_uppercase();

    // Detect modal
    let prefixes: &[(&str, Modal, RuleKind)] = &[
        ("MUST NOT ", Modal::MustNot, RuleKind::NeverUse),
        ("DO NOT ", Modal::MustNot, RuleKind::NeverUse),
        ("SHALL NOT ", Modal::MustNot, RuleKind::NeverUse),
        ("SHOULD NOT ", Modal::ShouldNot, RuleKind::Other),
        ("NEVER ", Modal::MustNot, RuleKind::NeverUse),
        ("AVOID ", Modal::ShouldNot, RuleKind::Other),
        ("REQUIRED TO ", Modal::Must, RuleKind::AlwaysUse),
        ("ALWAYS ", Modal::Must, RuleKind::AlwaysUse),
        ("SHALL ", Modal::Must, RuleKind::AlwaysUse),
        ("SHOULD ", Modal::Should, RuleKind::Other),
        ("PREFER ", Modal::Should, RuleKind::Other),
        ("MUST ", Modal::Must, RuleKind::AlwaysUse),
    ];
    let mut matched: Option<(Modal, RuleKind, &str)> = None;
    for (prefix, modal, kind) in prefixes {
        if upper.starts_with(prefix) {
            matched = Some((*modal, *kind, &trimmed[prefix.len()..]));
            break;
        }
    }
    let (modal, rule_kind, leftover) = matched?;

    // extract the first tool-call or quoted subject from the remainder of the
    // (original-case) line.
    let subject = extract_subject(leftover)?;
    Some(Rule {
        kind: rule_kind,
        subject,
        modal,
        raw: trimmed.to_string(),
        line: line_number,
    })
}

fn extract_subject(text: &str) -> Option<String> {
    // Prefer a `Tool(x)` signature
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_uppercase() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'(' {
                let name_end = i;
                i += 1;
                let args_start = i;
                while i < bytes.len() && bytes[i] != b')' {
                    i += 1;
                }
                if i < bytes.len() {
                    let name = &text[start..name_end];
                    let args = &text[args_start..i];
                    return Some(format!("{name}({args})"));
                }
            }
        }
        i += 1;
    }
    // Otherwise a backtick-quoted segment
    if let Some(start) = text.find('`') {
        if let Some(end_rel) = text[start + 1..].find('`') {
            let inner = &text[start + 1..start + 1 + end_rel];
            if !inner.is_empty() {
                return Some(inner.to_string());
            }
        }
    }
    // No structural subject. The previous fallback returned the first
    // whitespace-delimited "word" of the leftover, but that produced rampant
    // false-positive `conflict` issues in any skill library that contained
    // ordinary prose modal sentences. For example,
    //
    //   Skill A: "MUST use the system properly."
    //   Skill B: "MUST NOT use the disk slowly."
    //
    // both yielded subject = "use", which the conflict detector then flagged
    // as contradicting one another even though the rules are about completely
    // different topics. Authors who want a rule pinned to a specific subject
    // can quote it (`MUST use \`git\``) or write it as a tool call (`MUST use
    // Bash(git)`); without one of those signals there is no reliable way to
    // identify what the rule is "about" and we'd rather miss the rule
    // entirely than emit a noisy false positive that blocks a green CI build.
    None
}

fn extract_inline_tags(body: &str, tags: &mut Vec<String>) {
    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("tags:") {
            for token in rest.split(|c: char| c == ',' || c.is_whitespace()) {
                let t = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_');
                if !t.is_empty() {
                    tags.push(t.to_string());
                }
            }
        }
    }
}

fn dedup_in_order(mut v: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    v.retain(|s| seen.insert(s.clone()));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_frontmatter_happy_path() {
        let text = "---\nname: foo\n---\nbody\n";
        let (f, b) = split_frontmatter(text);
        assert_eq!(f.trim(), "name: foo");
        assert_eq!(b, "body\n");
    }

    #[test]
    fn split_frontmatter_no_frontmatter() {
        let text = "hello\nworld\n";
        let (f, b) = split_frontmatter(text);
        assert!(f.is_empty());
        assert_eq!(b, text);
    }

    #[test]
    fn split_frontmatter_unterminated_treats_all_as_body() {
        let text = "---\nname: foo\n(no close)\n";
        let (f, b) = split_frontmatter(text);
        assert!(f.is_empty());
        assert_eq!(b, text);
    }

    #[test]
    fn frontmatter_parses_tags() {
        let raw = "name: foo\ntags:\n  - bar\n  - baz\n";
        let mut w = Vec::new();
        let f = parse_frontmatter(raw, &mut w);
        assert_eq!(f.tags, vec!["bar".to_string(), "baz".to_string()]);
    }

    #[test]
    fn frontmatter_malformed_logs_warning() {
        let raw = "name: : : :\nbad: [unclosed\n";
        let mut w = Vec::new();
        let f = parse_frontmatter(raw, &mut w);
        assert!(w
            .iter()
            .any(|w| w.kind == WarningKind::FrontmatterYamlError));
        assert!(f.name.is_none());
    }

    #[test]
    fn decode_strips_bom() {
        let raw: Vec<u8> = [0xEF, 0xBB, 0xBF]
            .into_iter()
            .chain(b"hello".iter().copied())
            .collect();
        let (text, warnings) = decode_bytes(&raw, Path::new("x.md"));
        assert_eq!(text, "hello");
        assert!(warnings.iter().any(|w| w.kind == WarningKind::Bom));
    }

    #[test]
    fn decode_flags_crlf() {
        let raw = b"a\r\nb";
        let (_, warnings) = decode_bytes(raw, Path::new("x.md"));
        assert!(warnings.iter().any(|w| w.kind == WarningKind::Crlf));
    }

    #[test]
    fn decode_handles_invalid_utf8_gracefully() {
        let raw = [0x61, 0xFF, 0x62];
        let (text, warnings) = decode_bytes(&raw, Path::new("x.md"));
        assert!(text.contains('a'));
        assert!(text.contains('b'));
        assert!(warnings
            .iter()
            .any(|w| w.kind == WarningKind::NonUtf8Recovered));
    }

    #[test]
    fn derive_skill_id_strips_skill_md() {
        let id = derive_skill_id(Path::new("git/commit-style/SKILL.md"));
        assert_eq!(id.as_str(), "git/commit-style");
    }

    #[test]
    fn derive_skill_id_strips_readme() {
        let id = derive_skill_id(Path::new("git/README.md"));
        assert_eq!(id.as_str(), "git");
    }

    #[test]
    fn derive_skill_id_plain_md() {
        let id = derive_skill_id(Path::new("foo/bar.md"));
        assert_eq!(id.as_str(), "foo/bar");
    }

    #[test]
    fn extract_rule_must_use() {
        let r = extract_rule_from_line("MUST use `Bash(ls)` for listing", 1).unwrap();
        assert_eq!(r.modal, Modal::Must);
        assert_eq!(r.subject, "Bash(ls)");
    }

    #[test]
    fn extract_rule_must_not_use() {
        let r = extract_rule_from_line("Do not use `Write(/etc/*)`", 1).unwrap();
        assert_eq!(r.modal, Modal::MustNot);
        assert_eq!(r.subject, "Write(/etc/*)");
    }

    #[test]
    fn extract_rule_never() {
        let r = extract_rule_from_line("NEVER Bash(rm)", 1).unwrap();
        assert_eq!(r.modal, Modal::MustNot);
        assert_eq!(r.subject, "Bash(rm)");
    }

    #[test]
    fn extract_rule_irrelevant_line_is_none() {
        assert!(extract_rule_from_line("some prose", 1).is_none());
        assert!(extract_rule_from_line("", 1).is_none());
    }

    #[test]
    fn extract_mentions_picks_at_refs() {
        let mut refs = Vec::new();
        scan_mentions_and_files("see @foo/bar and @baz for details", &mut refs);
        let mentions: Vec<_> = refs
            .iter()
            .filter_map(|r| match r {
                SkillRef::Mention { skill_id } => Some(skill_id.as_str().to_string()),
                _ => None,
            })
            .collect();
        assert!(mentions.contains(&"foo/bar".to_string()));
        assert!(mentions.contains(&"baz".to_string()));
    }

    #[test]
    fn extract_wiki_links_detected() {
        let mut refs = Vec::new();
        scan_mentions_and_files("see [[foo-bar]] please", &mut refs);
        assert!(refs.iter().any(
            |r| matches!(r, SkillRef::Mention { skill_id } if skill_id.as_str() == "foo-bar")
        ));
    }

    #[test]
    fn wiki_link_target_strips_heading_anchor() {
        assert_eq!(wiki_link_target("foo#usage"), Some("foo"));
        assert_eq!(wiki_link_target("foo#usage|display"), Some("foo"));
        assert_eq!(wiki_link_target("foo|display#not-an-anchor"), Some("foo"));
    }

    #[test]
    fn wiki_link_target_rejects_anchor_only() {
        // `[[#section]]` is a same-page anchor — no skill id to resolve.
        assert_eq!(wiki_link_target("#section"), None);
    }

    #[test]
    fn extract_tool_invocations() {
        let mut refs = Vec::new();
        let text = CowStr::Borrowed("use Bash(ls) and Write(*)");
        scan_tool_invocation(&text, &mut refs);
        let tools: Vec<_> = refs
            .iter()
            .filter_map(|r| match r {
                SkillRef::Tool { name, args } => Some((name.clone(), args.clone())),
                _ => None,
            })
            .collect();
        assert!(tools.iter().any(|(n, a)| n == "Bash" && a == "ls"));
        assert!(tools.iter().any(|(n, a)| n == "Write" && a == "*"));
    }

    #[test]
    fn extract_tool_ignores_unknown_names() {
        let mut refs = Vec::new();
        let text = CowStr::Borrowed("Random(x) MoreRandom(y)");
        scan_tool_invocation(&text, &mut refs);
        assert!(!refs.iter().any(|r| matches!(r, SkillRef::Tool { .. })));
    }

    #[test]
    fn parse_bytes_end_to_end_minimal() {
        let raw = b"---\nname: greet\ntags:\n  - hi\n---\nSay hello.\n\nNEVER `Write(*)`.\n";
        let parsed = parse_bytes(raw, Path::new("greet/SKILL.md"));
        assert_eq!(parsed.id.as_str(), "greet");
        assert_eq!(parsed.name, "greet");
        assert!(parsed.tags.contains(&"hi".to_string()));
        assert!(!parsed.rules.is_empty());
    }

    #[test]
    fn parse_bytes_handles_crlf_and_bom() {
        let mut raw: Vec<u8> = vec![0xEF, 0xBB, 0xBF];
        raw.extend_from_slice(b"---\r\nname: a\r\n---\r\nbody\r\n");
        let parsed = parse_bytes(&raw, Path::new("a/SKILL.md"));
        assert!(parsed.warnings.iter().any(|w| w.kind == WarningKind::Bom));
        assert!(parsed.warnings.iter().any(|w| w.kind == WarningKind::Crlf));
    }

    #[test]
    fn derive_name_from_path_uses_parent_for_skill_md() {
        let name = derive_name_from_path(Path::new("git/commit/SKILL.md")).unwrap();
        assert_eq!(name, "commit");
    }

    #[test]
    fn derive_name_from_path_plain_stem() {
        let name = derive_name_from_path(Path::new("foo.md")).unwrap();
        assert_eq!(name, "foo");
    }

    #[test]
    fn dedup_in_order_works() {
        let v = vec!["a".to_string(), "b".to_string(), "a".to_string()];
        let d = dedup_in_order(v);
        assert_eq!(d, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn extract_inline_tags_collects() {
        let mut tags = Vec::new();
        extract_inline_tags("tags: a, b, c\nprose\n", &mut tags);
        assert!(tags.contains(&"a".to_string()));
        assert!(tags.contains(&"b".to_string()));
    }

    #[test]
    fn subject_extracts_backtick_quoted() {
        let s = extract_subject("use `rm -rf` never").unwrap();
        assert_eq!(s, "rm -rf");
    }

    #[test]
    fn subject_extracts_tool_call() {
        let s = extract_subject("call Bash(ls) please").unwrap();
        assert_eq!(s, "Bash(ls)");
    }

    #[test]
    fn subject_no_structural_signal_returns_none() {
        // Without a Tool(args) signature or a backtick-quoted segment we
        // refuse to invent a subject from prose. See `extract_subject` for
        // why the previous "first word" fallback was a footgun.
        assert!(extract_subject("foo bar baz").is_none());
        assert!(extract_subject("the system properly").is_none());
    }

    #[test]
    fn is_indented_code_line_recognises_four_space_prefix() {
        assert!(is_indented_code_line("    let x = 1;"));
        assert!(is_indented_code_line("\tlet x = 1;"));
        assert!(is_indented_code_line("    MUST use `git rebase`"));
        assert!(!is_indented_code_line("   only three spaces"));
        assert!(!is_indented_code_line(""));
        assert!(!is_indented_code_line("    "));
        assert!(!is_indented_code_line("regular paragraph"));
    }

    #[test]
    fn indented_code_block_rule_skipped_when_preceded_by_blank() {
        // CommonMark indented code: blank line then 4+ space indent. The
        // `MUST use foo` inside the block must NOT create a Rule. Without
        // the fix this picked up `Rule { subject: "foo" }` and the conflict
        // detector would happily collide it against a real prose rule
        // elsewhere in the library.
        let body = "Sample:\n\n    MUST use `foo`\n";
        let (_refs, rules) = extract_refs_and_rules(body);
        assert!(
            rules.is_empty(),
            "indented-code MUST line must not extract as a rule; got {rules:?}"
        );
    }

    #[test]
    fn indented_paragraph_continuation_still_extracts_rule() {
        // Without a preceding blank line, indented text is a paragraph
        // continuation, not a code block. (Today the rule extractor treats
        // such lines as text — we confirm we did not over-skip them.)
        let body = "First line.\n    MUST use `bar`\n";
        let (_refs, rules) = extract_refs_and_rules(body);
        assert!(
            !rules.is_empty(),
            "indented continuation (no blank above) is paragraph text — rule must still fire"
        );
    }

    #[test]
    fn indented_backticks_are_not_a_fence_opener() {
        // CommonMark §4.5: a code fence opener may be indented 0–3 spaces
        // only. A line with 4+ leading spaces and ```` is part of an indented
        // code block, NOT a fence — so it must not flip the rule extractor
        // into "we're inside a fence; drop everything that follows" mode.
        // Without the leading-space guard, the `MUST use ...` line below was
        // silently swallowed because the parser thought a fence was open.
        let body = "Sample:\n\n    ```rust\n    let x = 1;\n\nMUST use `git`\n";
        let (_refs, rules) = extract_refs_and_rules(body);
        assert!(
            rules.iter().any(|r| r.subject == "git"),
            "rule after indented (fake) fence must still be extracted; got {rules:?}"
        );
    }

    #[test]
    fn indented_backticks_inside_fence_dont_close_it() {
        // The mirror failure: an indented ```` *inside* a real fence used to
        // be treated as a closing fence (since the leading-space check was
        // missing on the close path too). That prematurely terminated the
        // block and exposed sample MUST/MUST-NOT prose below as fake rules.
        let body = "Real code:\n\n```text\n        ```\nMUST use `git`\n```\n";
        let (_refs, rules) = extract_refs_and_rules(body);
        assert!(
            rules.is_empty(),
            "rule inside a real fence (premature close was the bug) must not fire; got {rules:?}"
        );
    }

    #[test]
    fn standard_zero_indent_fence_still_works() {
        // Regression sanity: ordinary `\`\`\`` at column 0 must still be
        // recognised. The leading-space guard only rejects 4+ spaces.
        let body = "Code:\n\n```\nMUST use `git`\n```\n\nMUST use `bar`\n";
        let (_refs, rules) = extract_refs_and_rules(body);
        let subjects: Vec<_> = rules.iter().map(|r| r.subject.as_str()).collect();
        assert!(
            subjects.contains(&"bar"),
            "rule outside real fence must still extract; got {subjects:?}"
        );
        assert!(
            !subjects.contains(&"git"),
            "rule inside real fence must be suppressed; got {subjects:?}"
        );
    }

    #[test]
    fn closing_fence_with_info_string_is_not_a_closer() {
        // CommonMark §4.5: a closing fence must NOT carry an info string;
        // anything after the run other than trailing whitespace disqualifies
        // it from being a closer. Without enforcing this, a nested
        // documentation example like the one below caused the outer ```text
        // fence to terminate early at the inner ```rust line, after which the
        // `MUST use phantom` line was harvested as a real rule and collided
        // with any matching `MUST NOT use phantom` elsewhere in the library.
        let body = "Sample:\n\n```text\n```rust\nMUST use `phantom`\n```\nEnd.\n```\n";
        let (_refs, rules) = extract_refs_and_rules(body);
        assert!(
            rules.is_empty(),
            "closing fence with info string must not terminate the outer block; got {rules:?}"
        );
    }

    #[test]
    fn closing_fence_with_trailing_whitespace_still_closes() {
        // Trailing whitespace on a closer is explicitly allowed.
        let body = "Code:\n\n```\nMUST use `git`\n```   \n\nMUST use `bar`\n";
        let (_refs, rules) = extract_refs_and_rules(body);
        let subjects: Vec<_> = rules.iter().map(|r| r.subject.as_str()).collect();
        assert!(
            subjects.contains(&"bar"),
            "closer with trailing whitespace must still close the fence; got {subjects:?}"
        );
        assert!(
            !subjects.contains(&"git"),
            "rule inside the fence must remain suppressed; got {subjects:?}"
        );
    }

    #[test]
    fn three_space_indented_fence_is_still_a_fence() {
        // CommonMark allows up to 3 leading spaces before a fence opener.
        let body = "Code:\n\n   ```\nMUST use `git`\n   ```\n\nMUST use `bar`\n";
        let (_refs, rules) = extract_refs_and_rules(body);
        let subjects: Vec<_> = rules.iter().map(|r| r.subject.as_str()).collect();
        assert!(
            subjects.contains(&"bar"),
            "rule after 3-space-indented fence must extract; got {subjects:?}"
        );
        assert!(
            !subjects.contains(&"git"),
            "3-space-indented fence is still a fence — inner rule suppressed; got {subjects:?}"
        );
    }

    #[test]
    fn extract_rule_prose_only_yields_no_rule() {
        // "MUST use the system properly" looks rule-shaped but has no
        // structural subject — emitting a Rule with subject "use" causes
        // false-positive conflicts against any other prose rule that
        // happens to start with the same word. The rule extractor should
        // simply skip it.
        assert!(extract_rule_from_line("MUST use the system properly", 1).is_none());
        assert!(extract_rule_from_line("MUST NOT use the disk slowly", 1).is_none());
    }

    #[test]
    fn extract_refs_link_captured() {
        let body = "see [docs](./docs/intro.md) for details";
        let (refs, _) = extract_refs_and_rules(body);
        assert!(refs
            .iter()
            .any(|r| matches!(r, SkillRef::Link { target, .. } if target.as_os_str() == "./docs/intro.md")));
    }

    #[test]
    fn extract_refs_ignores_http() {
        let body = "visit [site](https://example.com)";
        let (refs, _) = extract_refs_and_rules(body);
        assert!(!refs.iter().any(|r| matches!(r, SkillRef::Link { .. })));
    }

    #[test]
    fn extract_refs_ignores_anchors() {
        let body = "see [section](#top)";
        let (refs, _) = extract_refs_and_rules(body);
        assert!(!refs.iter().any(|r| matches!(r, SkillRef::Link { .. })));
    }

    #[test]
    fn link_with_fragment_strips_to_path_only() {
        let body = "see [docs](./docs/intro.md#install)";
        let (refs, _) = extract_refs_and_rules(body);
        let target = refs
            .iter()
            .find_map(|r| match r {
                SkillRef::Link { target, .. } => Some(target.to_string_lossy().into_owned()),
                _ => None,
            })
            .expect("link captured");
        assert_eq!(target, "./docs/intro.md");
    }

    #[test]
    fn link_with_query_strips_to_path_only() {
        let body = "see [docs](./docs/intro.md?v=1)";
        let (refs, _) = extract_refs_and_rules(body);
        let target = refs
            .iter()
            .find_map(|r| match r {
                SkillRef::Link { target, .. } => Some(target.to_string_lossy().into_owned()),
                _ => None,
            })
            .expect("link captured");
        assert_eq!(target, "./docs/intro.md");
    }

    #[test]
    fn link_with_query_and_fragment_strips_both() {
        let body = "[a](./d.md?x=1#y) [b](./e.md#y?x=1)";
        let (refs, _) = extract_refs_and_rules(body);
        let targets: Vec<String> = refs
            .iter()
            .filter_map(|r| match r {
                SkillRef::Link { target, .. } => Some(target.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        assert!(targets.contains(&"./d.md".to_string()), "{targets:?}");
        assert!(targets.contains(&"./e.md".to_string()), "{targets:?}");
    }

    #[test]
    fn strip_link_modifiers_passthrough() {
        assert_eq!(strip_link_modifiers("./foo.md"), "./foo.md");
    }

    #[test]
    fn strip_link_modifiers_drops_fragment_only() {
        assert_eq!(strip_link_modifiers("./foo.md#abc"), "./foo.md");
    }

    #[test]
    fn strip_link_modifiers_drops_query_only() {
        assert_eq!(strip_link_modifiers("./foo.md?v=1"), "./foo.md");
    }

    #[test]
    fn wiki_link_inside_fenced_block_is_suppressed() {
        // CommonMark: a fenced code block is literal — markup inside is text,
        // not real cross-references. A documentation skill that explains the
        // wiki-link syntax in a sample block must NOT pin the sample target
        // as a live reference. Without this gate the raw `[[...]]` walker
        // would silently turn an illustrative `[[other-skill]]` into a live
        // edge that suppresses the genuine `dead` diagnostic and pollutes
        // the dependency graph.
        let body = "Example:\n\n```markdown\nSee [[fictitious-skill]] for details.\n```\n";
        let (refs, _) = extract_refs_and_rules(body);
        assert!(
            !refs.iter().any(|r| matches!(
                r,
                SkillRef::Mention { skill_id } if skill_id.as_str() == "fictitious-skill"
            )),
            "wiki-link inside fenced code block must not be extracted; got {refs:?}"
        );
    }

    #[test]
    fn at_mention_inside_fenced_block_is_suppressed() {
        // Same false-positive class for `@mention` text inside a fenced
        // sample. Pulldown-cmark emits Event::Text for code-block contents;
        // the event walker now tracks CodeBlock depth and skips mention
        // extraction while inside.
        let body = "Example:\n\n```\nSee @fictitious-mention for details.\n```\n";
        let (refs, _) = extract_refs_and_rules(body);
        assert!(
            !refs.iter().any(|r| matches!(
                r,
                SkillRef::Mention { skill_id } if skill_id.as_str() == "fictitious-mention"
            )),
            "@mention inside fenced code block must not be extracted; got {refs:?}"
        );
    }

    #[test]
    fn wiki_link_inside_inline_code_span_is_suppressed() {
        // An inline `\`[[...]]\`` is illustrative syntax, not a real link.
        // The raw wiki walker now masks backtick-fenced regions of each line.
        let body = "Use a wiki link by writing `[[other-skill]]` in markdown.\n";
        let (refs, _) = extract_refs_and_rules(body);
        assert!(
            !refs.iter().any(|r| matches!(
                r,
                SkillRef::Mention { skill_id } if skill_id.as_str() == "other-skill"
            )),
            "wiki-link inside inline code span must not be extracted; got {refs:?}"
        );
    }

    #[test]
    fn wiki_link_outside_code_still_extracts() {
        // Regression sanity: a real `[[...]]` outside any code context must
        // still produce a Mention. The new gate must not over-suppress.
        let body = "See [[real-skill]] for the canonical reference.\n";
        let (refs, _) = extract_refs_and_rules(body);
        assert!(
            refs.iter().any(|r| matches!(
                r,
                SkillRef::Mention { skill_id } if skill_id.as_str() == "real-skill"
            )),
            "real wiki-link outside code context must extract; got {refs:?}"
        );
    }

    #[test]
    fn tab_prefixed_backticks_are_not_a_fence_opener() {
        // CommonMark §4.5: a fence opener may be indented by 0–3 spaces only.
        // A leading tab counts as ≥4 columns of indentation, so a line
        // starting `\t\`\`\`` is an indented code block — NOT a fence — and
        // must not flip the rule extractor into "we're inside a fence; drop
        // everything that follows" mode. Without the tab guard the
        // `MUST use git` line below was silently swallowed because the parser
        // thought a fence was open and the matching tab-prefixed `\`\`\``
        // closed it (so the rule never fired even outside the fake block).
        let body = "Sample:\n\n\t```\n\t```\n\nMUST use `git`\n";
        let (_refs, rules) = extract_refs_and_rules(body);
        assert!(
            rules.iter().any(|r| r.subject == "git"),
            "rule after tab-prefixed (fake) fence pair must still be extracted; got {rules:?}"
        );
    }

    #[test]
    fn tab_prefixed_backticks_inside_fence_dont_close_it() {
        // Mirror failure: a tab-indented `\`\`\`` *inside* a real fence used
        // to be treated as a closing fence (since the leading-space check
        // ignored tabs). That prematurely terminated the block and exposed
        // sample `MUST use phantom` prose below as a fake rule, which then
        // collided with any matching `MUST NOT use phantom` elsewhere in the
        // library — a high-noise false-positive `conflict` issue.
        let body = "Real:\n\n```text\n\t```\nMUST use `phantom`\n```\nDone.\n";
        let (_refs, rules) = extract_refs_and_rules(body);
        assert!(
            rules.is_empty(),
            "tab-indented \\`\\`\\` inside a real fence must not close it; got {rules:?}"
        );
    }

    #[test]
    fn tab_prefixed_fence_does_not_suppress_inner_wiki_link() {
        // Same defect class for the wiki walker: a tab-prefixed `\`\`\`` must
        // not open a fake fence that suppresses real `[[...]]` mentions
        // appearing on subsequent (un-fenced) lines.
        let body = "Sample:\n\n\t```\n\t```\n\nSee [[real-skill]] please.\n";
        let (refs, _rules) = extract_refs_and_rules(body);
        assert!(
            refs.iter().any(|r| matches!(
                r,
                SkillRef::Mention { skill_id } if skill_id.as_str() == "real-skill"
            )),
            "wiki-link after tab-prefixed (fake) fence pair must still extract; got {refs:?}"
        );
    }

    #[test]
    fn wiki_walker_backtick_info_string_fence_not_an_opener() {
        // Lockstep regression for eval-AA: the wiki walker previously opened a
        // phantom fence on `\`\`\`rust\`bad\`info` (a line that pulldown-cmark
        // and the main rule extractor correctly treat as paragraph text per
        // CommonMark §4.5 — info strings on backtick fences may not contain
        // backticks). With the phantom fence open, the `[[real-skill]]` line
        // below was silently swallowed because the walker was waiting for a
        // matching closing run that never came (or only came at EOF, by which
        // point every subsequent mention had been dropped).
        let body = "```rust`bad`info\nSee [[real-skill]] for details.\n";
        let (refs, _rules) = extract_refs_and_rules(body);
        assert!(
            refs.iter().any(|r| matches!(
                r,
                SkillRef::Mention { skill_id } if skill_id.as_str() == "real-skill"
            )),
            "wiki-link after invalid backtick-info-string fence opener must extract; got {refs:?}"
        );
    }

    #[test]
    fn wiki_walker_tilde_info_string_fence_still_opens() {
        // Tilde fences are NOT subject to the backtick-in-info-string
        // restriction — `~~~text\`backtick\`info` IS a valid opener. The
        // wiki walker must keep treating it as a fence opener so the
        // `[[fake-inside]]` mention below stays suppressed.
        let body = "~~~text`backtick`info\n[[fake-inside]] should be sample\n~~~\n[[real-after]]\n";
        let (refs, _rules) = extract_refs_and_rules(body);
        let mentions: Vec<&str> = refs
            .iter()
            .filter_map(|r| match r {
                SkillRef::Mention { skill_id } => Some(skill_id.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            mentions.contains(&"real-after"),
            "wiki-link after tilde fence close must extract; {mentions:?}"
        );
        assert!(
            !mentions.contains(&"fake-inside"),
            "wiki-link inside tilde fence (with backticks in info string) must remain suppressed; {mentions:?}"
        );
    }

    #[test]
    fn wiki_link_inside_indented_code_block_is_suppressed() {
        // Eval-BB lockstep regression. The rule extractor learned to skip
        // CommonMark §4.4 indented code blocks (4+ space indent preceded by
        // blank line OR ATX heading / setext underline / thematic break) in
        // cycles U and Z. The wiki walker did NOT receive the same gate, so
        // a `[[wiki-link]]` inside an illustrative indented sample was still
        // pinned as a real cross-reference — suppressing the genuine `dead`
        // diagnostic on the (non-existent) target and polluting the
        // dependency graph with phantom edges.
        for body in [
            // Blank line + 4-space indent
            "Sample:\n\n    See [[fictitious-blank]] for example.\n",
            // ATX heading + indented code (no blank-line separator needed)
            "# Heading\n    See [[fictitious-heading]] for example.\n",
            // Setext underline (h2) + indented code
            "Title\n---\n    See [[fictitious-setext]] for example.\n",
            // Thematic break + indented code
            "para text\n\n***\n    See [[fictitious-thematic]] for example.\n",
            // Tab-prefixed indented code
            "Sample:\n\n\tSee [[fictitious-tab]] for example.\n",
        ] {
            let (refs, _) = extract_refs_and_rules(body);
            let mentions: Vec<&str> = refs
                .iter()
                .filter_map(|r| match r {
                    SkillRef::Mention { skill_id } => Some(skill_id.as_str()),
                    _ => None,
                })
                .collect();
            assert!(
                mentions.iter().all(|m| !m.starts_with("fictitious-")),
                "wiki-link inside indented code block must not be extracted (body={body:?}); got {mentions:?}"
            );
        }
    }

    #[test]
    fn wiki_link_indented_paragraph_continuation_still_extracts() {
        // Mirror sanity check: an indented line that is a paragraph
        // continuation (no blank line / heading above) is NOT a code block —
        // the wiki walker must still extract its `[[...]]` mention.
        let body = "First line of a paragraph.\n    See [[real-skill]] for details.\n";
        let (refs, _) = extract_refs_and_rules(body);
        let mentions: Vec<&str> = refs
            .iter()
            .filter_map(|r| match r {
                SkillRef::Mention { skill_id } => Some(skill_id.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            mentions.contains(&"real-skill"),
            "indented paragraph continuation (no blank above) must still extract; got {mentions:?}"
        );
    }

    #[test]
    fn wiki_link_mixed_real_and_sample() {
        // The realistic case: a single skill body has a real cross-reference
        // alongside an illustrative sample. Only the real one must survive.
        let body = "See [[real]] for details.\n\n```\nExample: [[fake]] is illustrative.\n```\n";
        let (refs, _) = extract_refs_and_rules(body);
        let mentions: Vec<&str> = refs
            .iter()
            .filter_map(|r| match r {
                SkillRef::Mention { skill_id } => Some(skill_id.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            mentions.contains(&"real"),
            "real mention dropped; {mentions:?}"
        );
        assert!(
            !mentions.contains(&"fake"),
            "sample mention leaked from inside fence; {mentions:?}"
        );
    }

    #[test]
    fn wiki_walker_keeps_prev_line_through_fence_open_close() {
        // Eval-CC lockstep regression. The rule extractor deliberately
        // leaves `prev_line` untouched when a line opens or closes a fence,
        // so any block-terminator (ATX heading / setext underline /
        // thematic break) immediately preceding an empty fence pair still
        // counts as the prior block when an indented chunk follows the
        // fence-close. The wiki walker previously updated `prev_line` to
        // the literal `\`\`\`` fence-delimiter line on both branches, which
        // silently broke that lockstep:
        //
        //     # Heading
        //     ```
        //     ```
        //         [[fictitious-after-empty-fence]]
        //
        // The rule extractor's prev_line stays at "# Heading" through the
        // empty fence pair, recognises L4 as a CommonMark §4.4 indented
        // code block (ATX heading is a block-terminator per §4.4 widening),
        // and suppresses any rule on it. The wiki walker's prev_line moved
        // to `\`\`\`` after L2, so by L4 the heading was lost,
        // `prev_was_terminator` evaluated to false, and the indented sample
        // `[[fictitious-after-empty-fence]]` was pinned as a real
        // cross-reference — suppressing the genuine `dead` diagnostic on
        // the (non-existent) target and polluting the dependency graph.
        //
        // Same false-positive class as eval-V through eval-BB: every
        // walker that reads markdown must agree on which lines are code
        // and which are prose, and they must agree across every shape of
        // fence/heading/indent interleaving — not just the ones that
        // appear in the most recent regression test.
        //
        // We only assert the cases where the rule extractor itself is
        // already correct (block-terminator immediately precedes the empty
        // fence pair). The non-empty-fence variant is covered by the
        // lockstep_parity test below — both walkers must agree, even when
        // both are conservatively permissive.
        for body in [
            // ATX heading + empty fence pair + indented wiki
            "# Heading\n```\n```\n    [[fictitious-after-empty-fence]]\n",
            // Setext underline (h1) + empty fence pair + indented wiki
            "Title\n===\n```\n```\n    [[fictitious-after-setext]]\n",
            // Thematic break + empty fence pair + indented wiki
            "para\n\n***\n```\n```\n    [[fictitious-after-thematic]]\n",
            // Tilde-fence variant of the empty-fence-after-heading case
            "# Heading\n~~~\n~~~\n    [[fictitious-after-tilde]]\n",
        ] {
            let (refs, _) = extract_refs_and_rules(body);
            let mentions: Vec<&str> = refs
                .iter()
                .filter_map(|r| match r {
                    SkillRef::Mention { skill_id } => Some(skill_id.as_str()),
                    _ => None,
                })
                .collect();
            assert!(
                mentions.iter().all(|m| !m.starts_with("fictitious-")),
                "wiki-link inside indented code block (preceded by block-terminator + empty \
                 fence pair) must not be extracted (body={body:?}); got {mentions:?}"
            );
        }
    }

    #[test]
    fn fence_closer_must_match_opener_character_backtick_then_tilde() {
        // Eval-CC bug class B: per CommonMark §4.5 a fenced code block ends
        // only when a closer uses the SAME character (backtick or tilde) as
        // the opener. A `~~~` line is NOT a valid closer for a `\`\`\``
        // fence, and vice versa. Both walkers previously accepted any
        // `is_valid_closer` line whose run was ≥ the opener's length,
        // ignoring the character — so an illustrative `~~~` inside a real
        // backtick fence prematurely terminated the block:
        //
        //   ```
        //   MUST use `phantom`
        //   ~~~                    ← intended as INSIDE the backtick block
        //   [[fictitious]]
        //   ```
        //
        // The phantom `MUST` was correctly suppressed (rule walker is in
        // fence), but the inner `~~~` flipped in_fence=false, exposing
        // `[[fictitious]]` as a real wiki mention. Same false-positive
        // class as every previous fence misclassification.
        let body = "```\nMUST use `phantom-tilde-close`\n~~~\n[[fictitious-after-tilde]]\n```\nReal: [[real-after-true-close]]\n";
        let (refs, rules) = extract_refs_and_rules(body);
        let phantom = rules.iter().any(|r| r.subject.contains("phantom"));
        let mentions: Vec<&str> = refs
            .iter()
            .filter_map(|r| match r {
                SkillRef::Mention { skill_id } => Some(skill_id.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            !phantom,
            "phantom rule inside backtick fence must remain suppressed; rules={rules:?}"
        );
        assert!(
            !mentions.contains(&"fictitious-after-tilde"),
            "`~~~` line must NOT close a backtick fence; \
             [[fictitious-after-tilde]] leaked from inside fenced block; got {mentions:?}"
        );
        assert!(
            mentions.contains(&"real-after-true-close"),
            "real wiki mention after the genuine `\\`\\`\\`` close must extract; got {mentions:?}"
        );
    }

    #[test]
    fn fence_closer_must_match_opener_character_tilde_then_backtick() {
        // Mirror: `\`\`\`` cannot close a `~~~` fence.
        let body = "~~~\nMUST use `phantom-bt-close`\n```\n[[fictitious-after-bt]]\n~~~\nReal: [[real-after-true-tilde]]\n";
        let (refs, rules) = extract_refs_and_rules(body);
        let phantom = rules.iter().any(|r| r.subject.contains("phantom"));
        let mentions: Vec<&str> = refs
            .iter()
            .filter_map(|r| match r {
                SkillRef::Mention { skill_id } => Some(skill_id.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            !phantom,
            "phantom rule inside tilde fence must remain suppressed; rules={rules:?}"
        );
        assert!(
            !mentions.contains(&"fictitious-after-bt"),
            "`\\`\\`\\`` line must NOT close a tilde fence; \
             [[fictitious-after-bt]] leaked from inside fenced block; got {mentions:?}"
        );
        assert!(
            mentions.contains(&"real-after-true-tilde"),
            "real wiki mention after the genuine `~~~` close must extract; got {mentions:?}"
        );
    }

    #[test]
    fn wiki_walker_lockstep_parity_with_rule_walker_through_fences() {
        // Lockstep parity: for any body that interleaves headings, fences,
        // and indented chunks, the wiki walker's `prev_line`/`prev_blank`
        // state must evolve identically to the rule extractor's. We probe
        // by constructing bodies that contain BOTH a `MUST` and a
        // `[[wiki]]` on the same indented line and asserting that whether
        // the rule fires (`MUST` extracted as a Rule) matches whether the
        // mention fires (`[[wiki]]` captured as a Mention). Walking out of
        // lockstep means a real-world skill could trip exactly one walker
        // and produce a half-broken diagnostic.
        for body in [
            "# Heading\n```\n```\n    MUST use `phantom-lockstep` and [[wiki-lockstep]]\n",
            "# Heading\n```\ninside\n```\n    MUST use `phantom-lockstep-2` and [[wiki-lockstep-2]]\n",
            "para\n\n***\n```\n```\n    MUST use `phantom-lockstep-3` and [[wiki-lockstep-3]]\n",
        ] {
            let (refs, rules) = extract_refs_and_rules(body);
            let rule_fired = rules.iter().any(|r| r.subject.starts_with("phantom-lockstep"));
            let mention_fired = refs.iter().any(|r| matches!(
                r,
                SkillRef::Mention { skill_id } if skill_id.as_str().starts_with("wiki-lockstep")
            ));
            assert_eq!(
                rule_fired, mention_fired,
                "lockstep parity broken (body={body:?}): rule_fired={rule_fired} \
                 mention_fired={mention_fired}; both walkers must agree on whether the \
                 indented line is code or prose"
            );
        }
    }

    /// Tiny oracle: build the set of body line indices that pulldown-cmark
    /// considers part of a `Tag::CodeBlock` (fenced or indented). Any line
    /// not in this set is "prose" per the canonical CommonMark parser, and
    /// both the rule extractor and the wiki-link walker MUST agree with it.
    /// The eval-DD refactor pinned both walkers to this oracle by routing
    /// fence/code detection through `Parser::into_offset_iter` instead of
    /// hand-rolling parallel state machines.
    fn lines_inside_code_per_pulldown(body: &str) -> std::collections::HashSet<usize> {
        let mut options = Options::empty();
        options.insert(Options::ENABLE_TABLES);
        options.insert(Options::ENABLE_STRIKETHROUGH);
        options.insert(Options::ENABLE_FOOTNOTES);
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        let mut depth = 0usize;
        let mut start: Option<usize> = None;
        for (ev, range) in Parser::new_ext(body, options).into_offset_iter() {
            match ev {
                Event::Start(Tag::CodeBlock(_)) => {
                    if depth == 0 {
                        start = Some(range.start);
                    }
                    depth += 1;
                }
                Event::End(TagEnd::CodeBlock) => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        if let Some(s) = start.take() {
                            ranges.push((s, range.end));
                        }
                    }
                }
                _ => {}
            }
        }
        let mut set = std::collections::HashSet::new();
        let mut byte = 0usize;
        for (i, line) in body.lines().enumerate() {
            let line_start = byte;
            let line_end = line_start + line.len();
            byte = line_end + 1;
            // Match the runtime predicate in `extract_refs_and_rules`: a
            // line counts as inside a CodeBlock if EITHER the starting byte
            // OR the last content byte falls within a range. The
            // start-only check misses `Tag::CodeBlock(Indented)` ranges,
            // which begin AFTER the line's leading whitespace prefix.
            let last = line_end.saturating_sub(1);
            if ranges
                .iter()
                .any(|(s, e)| (line_start >= *s && line_start < *e) || (last >= *s && last < *e))
            {
                set.insert(i);
            }
        }
        set
    }

    #[test]
    fn lockstep_parity_with_pulldown_oracle_rule_extraction() {
        // Eval-DD lockstep parity: for every body that places a `MUST use
        // \`x\`` line at a position pulldown-cmark classifies as code,
        // `extract_refs_and_rules` must NOT emit a Rule. Conversely, when
        // pulldown-cmark says the line is prose, the rule MUST extract. The
        // pulldown event stream is the canonical CommonMark oracle — any
        // disagreement is a re-introduction of the cycle V–CC bug class.
        let bodies = [
            // Plain prose
            "MUST use `git`\n",
            // Fenced code block
            "```\nMUST use `phantom`\n```\n",
            // Indented after blank
            "Sample:\n\n    MUST use `phantom`\n",
            // Indented after ATX heading (no blank separator)
            "# Heading\n    MUST use `phantom`\n",
            // Indented after setext h1
            "Title\n===\n    MUST use `phantom`\n",
            // Indented after setext h2
            "Title\n---\n    MUST use `phantom`\n",
            // Indented after thematic break
            "para\n\n***\n    MUST use `phantom`\n",
            // Indented paragraph continuation (NOT code)
            "First line.\n    MUST use `bar`\n",
            // 3-space-indented fence (still a fence)
            "Code:\n\n   ```\nMUST use `phantom`\n   ```\n",
            // Tab-prefixed `\`\`\`` (NOT a fence — indented code)
            "Sample:\n\n\t```\n\t```\n\nMUST use `git`\n",
            // Backtick info string with embedded backticks (NOT a fence)
            "```rust`bad`info\nMUST use `phantom`\n```\nMUST use `real`\n",
            // Tilde fence with backticks in info string (IS a fence)
            "~~~text`bt`info\nMUST use `phantom`\n~~~\nMUST use `real`\n",
            // Same-character closer rule: ~~~ cannot close a ``` fence
            "```\nMUST use `phantom`\n~~~\nMUST use `still-phantom`\n```\nMUST use `real`\n",
        ];
        for body in bodies {
            let code_lines = lines_inside_code_per_pulldown(body);
            let (_refs, rules) = extract_refs_and_rules(body);
            for (i, line) in body.lines().enumerate() {
                if !line.contains("MUST use") {
                    continue;
                }
                let want_rule = !code_lines.contains(&i);
                let got_rule = rules.iter().any(|r| r.line == i + 1);
                assert_eq!(
                    want_rule, got_rule,
                    "lockstep parity broken on rule extraction (body={body:?}, line {} = {line:?}): \
                     pulldown says code={}, but extractor emitted rule={}",
                    i + 1,
                    !want_rule,
                    got_rule,
                );
            }
        }
    }

    #[test]
    fn lockstep_parity_with_pulldown_oracle_wiki_extraction() {
        // Companion to the rule-extraction parity test: a `[[wiki-id]]`
        // mention must extract iff pulldown-cmark classifies its containing
        // line as prose. The pulldown event stream is the canonical
        // CommonMark oracle — both line-walkers (rules and wiki) inherit
        // their fence/indented-code state from it after eval-DD.
        let bodies: &[(&str, &[&str], &[&str])] = &[
            // (body, must-be-extracted, must-NOT-be-extracted)
            ("See [[real-1]] here.\n", &["real-1"], &[]),
            ("```\nSee [[fake-1]] sample.\n```\n", &[], &["fake-1"]),
            ("Sample:\n\n    See [[fake-2]] sample.\n", &[], &["fake-2"]),
            ("# Heading\n    See [[fake-3]] sample.\n", &[], &["fake-3"]),
            ("Title\n---\n    See [[fake-4]] sample.\n", &[], &["fake-4"]),
            (
                "para\n\n***\n    See [[fake-5]] sample.\n",
                &[],
                &["fake-5"],
            ),
            (
                "Use a wiki link by writing `[[fake-6]]` here.\nSee [[real-2]] too.\n",
                &["real-2"],
                &["fake-6"],
            ),
            (
                "```\nSee [[fake-7]].\n~~~\n[[fake-8]]\n```\nReal: [[real-3]]\n",
                &["real-3"],
                &["fake-7", "fake-8"],
            ),
            (
                "~~~text`bt`info\n[[fake-9]] inside.\n~~~\n[[real-4]]\n",
                &["real-4"],
                &["fake-9"],
            ),
            (
                "```rust`bad`info\nSee [[real-5]] for details.\n",
                &["real-5"],
                &[],
            ),
        ];
        for (body, must_extract, must_not_extract) in bodies {
            let (refs, _rules) = extract_refs_and_rules(body);
            let mentions: Vec<&str> = refs
                .iter()
                .filter_map(|r| match r {
                    SkillRef::Mention { skill_id } => Some(skill_id.as_str()),
                    _ => None,
                })
                .collect();
            for id in *must_extract {
                assert!(
                    mentions.contains(id),
                    "lockstep parity broken (body={body:?}): expected mention {id:?} in \
                     extracted set; got {mentions:?}"
                );
            }
            for id in *must_not_extract {
                assert!(
                    !mentions.contains(id),
                    "lockstep parity broken (body={body:?}): {id:?} must NOT extract \
                     (pulldown classifies its line as code); got {mentions:?}"
                );
            }
        }
    }
}
