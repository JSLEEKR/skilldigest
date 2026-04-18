//! Cycle-Z regression probes for CommonMark-conformance edge cases.
//!
//! Each test pins behaviour the line-based extractor in `parse.rs` must keep
//! aligned with `pulldown-cmark` (and therefore with the CommonMark spec).
//! When the extractor disagrees with the canonical event stream, the
//! conflict / dead / stale rules silently ingest phantom rules and produce
//! noisy false-positives across an entire skill library — the same defect
//! class as the cycle A / U / V / W / X / Y fence-handling fixes.

use skilldigest::model::Warning;
use skilldigest::parse::parse_text;
use std::path::Path;

fn parse(body: &str) -> skilldigest::parse::ParsedSkill {
    let mut warnings: Vec<Warning> = Vec::new();
    parse_text(body, Path::new("probe/SKILL.md"), &mut warnings)
}

#[test]
fn y1_fence_info_with_backticks_should_not_suppress() {
    // Per CommonMark §4.5: "If the info string comes after a backtick fence,
    // it may not contain any backtick characters." So ```rust`bad`info is
    // NOT a fence opener — it's paragraph text. pulldown-cmark agrees the
    // line is a paragraph start, which means `MUST use phantom` on the next
    // line is real prose and must extract as a rule. Without the
    // backtick-info-string guard the extractor opened a phantom fence and
    // silently swallowed every real rule that followed.
    let body = "```rust`bad`info\nMUST use `phantom`\n```\nMUST use `real`\n";
    let parsed = parse(body);
    let subjects: Vec<&str> = parsed.rules.iter().map(|r| r.subject.as_str()).collect();
    assert!(
        subjects.contains(&"phantom"),
        "Y1: per CommonMark §4.5, ```rust`bad`info is paragraph not fence — phantom must extract; got {subjects:?}"
    );
}

#[test]
fn y2_indented_code_after_setext_heading() {
    // pulldown-cmark: setext-style heading underline (`====`) followed
    // immediately by a 4-space-indented chunk is parsed as Heading + Indented
    // code block. CommonMark §4.4 is explicit: indented code "cannot
    // interrupt a paragraph", but a heading is not a paragraph — it
    // terminates the prior block on its own — so the indented chunk IS a
    // code block even without a blank-line separator. The rule extractor
    // must skip indented code, so `phantom` must NOT extract.
    let body = "Heading\n=======\n    MUST use `phantom`\n";
    let parsed = parse(body);
    let subjects: Vec<&str> = parsed.rules.iter().map(|r| r.subject.as_str()).collect();
    assert!(
        !subjects.contains(&"phantom"),
        "Y2: indented code after setext heading must not extract; got {subjects:?}"
    );
}

#[test]
fn y2b_indented_code_after_atx_heading() {
    // Same defect class as Y2 but with an ATX heading (`# Heading`) instead
    // of a setext underline. pulldown-cmark agrees the indented chunk is a
    // code block, so `phantom` must not extract.
    let body = "# Heading\n    MUST use `phantom`\n";
    let parsed = parse(body);
    let subjects: Vec<&str> = parsed.rules.iter().map(|r| r.subject.as_str()).collect();
    assert!(
        !subjects.contains(&"phantom"),
        "Y2b: indented code after ATX heading must not extract; got {subjects:?}"
    );
}

#[test]
fn z1_indented_code_after_thematic_break() {
    // Companion to Y2/Y2b: a CommonMark thematic break (`---` / `***` /
    // `___` on its own line) is also a block-terminator, so an indented
    // chunk that follows it IS a code block per §4.4. Picking up
    // `MUST use phantom` here as a rule produces the same false-positive
    // conflict noise as the heading-then-indented-code case.
    let body = "intro\n\n---\n    MUST use `phantom`\n";
    let parsed = parse(body);
    let subjects: Vec<&str> = parsed.rules.iter().map(|r| r.subject.as_str()).collect();
    assert!(
        !subjects.contains(&"phantom"),
        "Z1: indented code after thematic break must not extract; got {subjects:?}"
    );
}

#[test]
fn z2_paragraph_continuation_into_indented_text_still_extracts() {
    // Regression sanity: WITHOUT a preceding block-terminator the indented
    // line is paragraph continuation, not a code block. The rule extractor
    // must still pick `git` up — over-suppressing here would silently drop
    // every legitimate indented modal sentence in real skill bodies.
    let body = "First line of paragraph.\n    MUST use `git`\n";
    let parsed = parse(body);
    let subjects: Vec<&str> = parsed.rules.iter().map(|r| r.subject.as_str()).collect();
    assert!(
        subjects.contains(&"git"),
        "Z2: indented paragraph-continuation must still extract rule; got {subjects:?}"
    );
}
