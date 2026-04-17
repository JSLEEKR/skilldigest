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
    let parser = Parser::new_ext(body, options);

    let mut in_link_dest: Option<String> = None;
    for event in parser {
        match event {
            Event::Start(Tag::Link { dest_url, .. }) => {
                in_link_dest = Some(dest_url.to_string());
            }
            Event::End(TagEnd::Link) => {
                if let Some(dest) = in_link_dest.take() {
                    add_link_ref(&mut refs, &dest);
                }
            }
            Event::Code(ref c) => {
                scan_tool_invocation(c, &mut refs);
            }
            Event::Text(ref t) => {
                scan_mentions_and_files(t, &mut refs);
                scan_tool_invocation(t, &mut refs);
            }
            _ => {}
        }
    }

    // Rule extraction works line-by-line on the raw body.
    for (i, line) in body.lines().enumerate() {
        if let Some(rule) = extract_rule_from_line(line, i + 1) {
            rules.push(rule);
        }
    }

    refs.sort_by_key(|r| format!("{r:?}"));
    refs.dedup_by_key(|r| format!("{r:?}"));

    (refs, rules)
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
    let path = PathBuf::from(dest);
    refs.push(SkillRef::Link {
        target: path,
        exists: false, // filled in later by the scanner
    });
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
                if !inner.is_empty() && !inner.contains(' ') {
                    refs.push(SkillRef::Mention {
                        skill_id: SkillId::new(inner),
                    });
                }
                i = start + rel + 2;
                continue;
            }
        }
        i += 1;
    }
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
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
    // Otherwise the first "word"
    text.split_whitespace()
        .next()
        .map(|s| s.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .filter(|s| !s.is_empty())
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
    fn subject_fallback_first_word() {
        let s = extract_subject("foo bar baz").unwrap();
        assert_eq!(s, "foo");
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
}
