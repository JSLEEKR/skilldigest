//! Loadout recommendation.
//!
//! Given a task tag and a token budget, select the smallest set of skills
//! whose total token cost is under budget while maximising tag relevance.
//!
//! The algorithm is deliberately simple and deterministic:
//!
//! 1. Score each skill for the tag.
//! 2. Sort by `score / tokens` (descending) with skill ID as tiebreaker.
//! 3. Greedily add skills whose tokens fit the remaining budget.

use crate::model::{Loadout, Skill, SkillId};

/// Score a skill against a tag. Higher is more relevant.
#[must_use]
pub fn score(skill: &Skill, tag: &str) -> u32 {
    let tag_lower = tag.to_ascii_lowercase();
    let mut s = 0u32;
    for t in &skill.tags {
        if t.eq_ignore_ascii_case(tag) {
            s = s.saturating_add(10);
        }
    }
    // Exact word match in the display name.
    for w in skill.name.split(|c: char| !c.is_alphanumeric()) {
        if w.eq_ignore_ascii_case(tag) {
            s = s.saturating_add(5);
        }
    }
    // Skill ID contains the tag
    if skill.id.as_str().to_ascii_lowercase().contains(&tag_lower) {
        s = s.saturating_add(3);
    }
    // Frontmatter description contains the tag
    if let Some(desc) = &skill.frontmatter.description {
        if desc.to_ascii_lowercase().contains(&tag_lower) {
            s = s.saturating_add(2);
        }
    }
    s
}

/// Recommend a loadout.
#[must_use]
pub fn recommend(skills: &[Skill], tag: &str, max_tokens: usize) -> Loadout {
    let mut scored: Vec<(u32, &Skill)> = skills
        .iter()
        .map(|s| (score(s, tag), s))
        .filter(|(score, _)| *score > 0)
        .collect();

    // Sort by (score_per_token desc, score desc, id asc).
    scored.sort_by(|(score_a, sa), (score_b, sb)| {
        let tokens_a = sa.tokens.total.max(1);
        let tokens_b = sb.tokens.total.max(1);
        // multiply to avoid float division so that the order is deterministic
        // across platforms.
        let lhs = (*score_b as u128) * (tokens_a as u128);
        let rhs = (*score_a as u128) * (tokens_b as u128);
        lhs.cmp(&rhs)
            .then(score_b.cmp(score_a))
            .then(sa.id.cmp(&sb.id))
    });

    let mut selected: Vec<SkillId> = Vec::new();
    let mut spent = 0usize;
    for (_, skill) in &scored {
        let cost = skill.tokens.total;
        if spent.saturating_add(cost) <= max_tokens {
            selected.push(skill.id.clone());
            spent = spent.saturating_add(cost);
        }
    }

    Loadout {
        tag: tag.to_string(),
        max_tokens,
        skills: selected,
        total_tokens: spent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Frontmatter, Skill, SkillId, TokenCounts};

    fn mk(id: &str, tokens: usize, tags: &[&str], name: Option<&str>) -> Skill {
        Skill {
            id: SkillId::new(id),
            name: name.unwrap_or(id).to_string(),
            path: format!("{id}/SKILL.md").into(),
            frontmatter: Frontmatter::default(),
            tokens: TokenCounts::new(0, tokens),
            refs: vec![],
            rules: vec![],
            tags: tags.iter().map(|s| s.to_string()).collect(),
            warnings: vec![],
            body_bytes: 0,
        }
    }

    #[test]
    fn score_tag_hit_in_tags_is_highest() {
        let s = mk("a", 100, &["git"], None);
        assert!(score(&s, "git") >= 10);
    }

    #[test]
    fn score_name_match_contributes() {
        let s = mk("a", 100, &[], Some("git-commit"));
        assert!(score(&s, "git") >= 5);
    }

    #[test]
    fn score_id_contains_tag() {
        let s = mk("git/commit", 100, &[], None);
        assert!(score(&s, "commit") >= 3);
    }

    #[test]
    fn score_description_hit() {
        let mut s = mk("a", 100, &[], None);
        s.frontmatter.description = Some("Use this for git".into());
        assert!(score(&s, "git") >= 2);
    }

    #[test]
    fn score_zero_when_no_match() {
        let s = mk("x", 100, &["zz"], Some("unrelated"));
        assert_eq!(score(&s, "git"), 0);
    }

    #[test]
    fn recommend_filters_zero_scorers() {
        let skills = vec![
            mk("a", 100, &["git"], None),
            mk("b", 100, &["unrelated"], None),
        ];
        let loadout = recommend(&skills, "git", 10000);
        assert_eq!(loadout.skills.len(), 1);
        assert_eq!(loadout.skills[0].as_str(), "a");
    }

    #[test]
    fn recommend_respects_budget() {
        let skills = vec![
            mk("a", 500, &["git"], None),
            mk("b", 500, &["git"], None),
            mk("c", 500, &["git"], None),
        ];
        let loadout = recommend(&skills, "git", 1000);
        assert_eq!(loadout.skills.len(), 2);
        assert_eq!(loadout.total_tokens, 1000);
    }

    #[test]
    fn recommend_prefers_cheaper_skill_when_scores_tie() {
        let skills = vec![
            mk("expensive", 1000, &["git"], None),
            mk("cheap", 100, &["git"], None),
        ];
        let loadout = recommend(&skills, "git", 500);
        // Only cheap fits within budget.
        assert_eq!(loadout.skills.len(), 1);
        assert_eq!(loadout.skills[0].as_str(), "cheap");
    }

    #[test]
    fn recommend_deterministic_tiebreak() {
        let skills = vec![mk("b", 100, &["x"], None), mk("a", 100, &["x"], None)];
        let l1 = recommend(&skills, "x", 10000);
        let l2 = recommend(&skills, "x", 10000);
        assert_eq!(l1.skills, l2.skills);
    }

    #[test]
    fn recommend_empty_library() {
        let loadout = recommend(&[], "git", 10000);
        assert_eq!(loadout.skills.len(), 0);
        assert_eq!(loadout.total_tokens, 0);
    }

    #[test]
    fn recommend_returns_tag_back() {
        let loadout = recommend(&[], "mytag", 100);
        assert_eq!(loadout.tag, "mytag");
        assert_eq!(loadout.max_tokens, 100);
    }

    #[test]
    fn recommend_skips_zero_token_skill_safely() {
        let skills = vec![mk("a", 0, &["git"], None)];
        let loadout = recommend(&skills, "git", 100);
        assert_eq!(loadout.skills.len(), 1);
        assert_eq!(loadout.total_tokens, 0);
    }

    #[test]
    fn recommend_case_insensitive_tag() {
        let skills = vec![mk("a", 10, &["Git"], None)];
        let loadout = recommend(&skills, "GIT", 100);
        assert_eq!(loadout.skills.len(), 1);
    }

    #[test]
    fn recommend_budget_zero_returns_empty_but_scores_nonzero() {
        let skills = vec![mk("a", 10, &["git"], None)];
        let loadout = recommend(&skills, "git", 0);
        assert_eq!(loadout.skills.len(), 0);
    }
}
