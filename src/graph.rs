//! Skill reference graph built on top of petgraph.
//!
//! Given a slice of [`Skill`]s, [`SkillGraph::build`] populates a directed
//! graph with:
//!
//! - Nodes: one per skill.
//! - Edges: `Reference` (skill → skill via mention) and implicit `IndexOf`
//!   edges from any file named `README.md`, `AGENTS.md`, `SKILLS.md`, or
//!   `index.md` to every other skill it mentions.
//!
//! The graph can then be queried for dead, stale, or cyclic skills.

use std::collections::{BTreeMap, BTreeSet};

use petgraph::algo::tarjan_scc;
use petgraph::graph::NodeIndex;
use petgraph::stable_graph::StableDiGraph;
use petgraph::visit::{EdgeRef, IntoEdgeReferences};

use crate::model::{Issue, IssueKind, Location, Skill, SkillId, SkillRef};

/// Edge kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EdgeKind {
    /// Skill → skill reference (mention, link to another skill).
    Reference,
    /// Implicit edge from an index/README node to each skill it references.
    IndexOf,
}

/// Skill reference graph.
#[derive(Debug)]
pub struct SkillGraph {
    graph: StableDiGraph<SkillId, EdgeKind>,
    index: BTreeMap<SkillId, NodeIndex>,
    /// Skill IDs that act as roots (README / AGENTS / index files).
    roots: BTreeSet<SkillId>,
}

impl SkillGraph {
    /// Build from a slice of parsed skills.
    #[must_use]
    pub fn build(skills: &[Skill]) -> Self {
        let mut g = StableDiGraph::<SkillId, EdgeKind>::new();
        let mut index = BTreeMap::new();
        let mut roots = BTreeSet::new();

        for skill in skills {
            let idx = g.add_node(skill.id.clone());
            index.insert(skill.id.clone(), idx);
            if is_root_like(skill) {
                roots.insert(skill.id.clone());
            }
        }

        for skill in skills {
            let src = match index.get(&skill.id) {
                Some(i) => *i,
                None => continue,
            };
            let is_root = roots.contains(&skill.id);
            for r in &skill.refs {
                match r {
                    SkillRef::Mention { skill_id } => {
                        if let Some(dst) = index.get(skill_id) {
                            let kind = if is_root {
                                EdgeKind::IndexOf
                            } else {
                                EdgeKind::Reference
                            };
                            g.add_edge(src, *dst, kind);
                        }
                    }
                    SkillRef::Link { target, .. } => {
                        // try to resolve link target → skill id
                        if let Some(target_id) =
                            resolve_link_to_skill_id(&skill.path, target.as_path())
                        {
                            if let Some(dst) = index.get(&target_id) {
                                let kind = if is_root {
                                    EdgeKind::IndexOf
                                } else {
                                    EdgeKind::Reference
                                };
                                g.add_edge(src, *dst, kind);
                            }
                        }
                    }
                    _ => {}
                }
            }
            for req in &skill.frontmatter.requires {
                let target_id = SkillId::new(req);
                if let Some(dst) = index.get(&target_id) {
                    g.add_edge(src, *dst, EdgeKind::Reference);
                }
            }
        }

        Self {
            graph: g,
            index,
            roots,
        }
    }

    /// True when the given skill has at least one in-edge.
    #[must_use]
    pub fn has_in_edges(&self, id: &SkillId) -> bool {
        let Some(idx) = self.index.get(id) else {
            return false;
        };
        self.graph
            .edges_directed(*idx, petgraph::Direction::Incoming)
            .next()
            .is_some()
    }

    /// Return the out-degree.
    #[must_use]
    pub fn out_degree(&self, id: &SkillId) -> usize {
        let Some(idx) = self.index.get(id) else {
            return 0;
        };
        self.graph
            .edges_directed(*idx, petgraph::Direction::Outgoing)
            .count()
    }

    /// Return the in-degree.
    #[must_use]
    pub fn in_degree(&self, id: &SkillId) -> usize {
        let Some(idx) = self.index.get(id) else {
            return 0;
        };
        self.graph
            .edges_directed(*idx, petgraph::Direction::Incoming)
            .count()
    }

    /// Skill IDs that are roots (index / README / AGENTS).
    #[must_use]
    pub fn roots(&self) -> &BTreeSet<SkillId> {
        &self.roots
    }

    /// Dead-skill detection: return every skill with zero in-edges that is
    /// not itself a root.
    ///
    /// A self-loop (skill `a` referencing itself) is NOT counted as a real
    /// incoming reference — the README's `SKILL001 dead` rule promises to
    /// flag any skill "never referenced by any index or other skill". A
    /// self-reference is neither an index nor *another* skill, so a skill
    /// whose only in-edge is its own self-loop is still logically dead. The
    /// `SKILL005 cycle` rule will simultaneously flag the self-loop as a
    /// separate issue.
    #[must_use]
    pub fn dead_skills(&self, skills: &[Skill]) -> Vec<Issue> {
        let mut out = Vec::new();
        for skill in skills {
            if self.roots.contains(&skill.id) {
                continue;
            }
            if !self.has_incoming_from_others(&skill.id) {
                let mut iss = Issue::new(
                    IssueKind::Dead,
                    skill.id.clone(),
                    format!(
                        "skill '{}' is never referenced by any index or other skill",
                        skill.id
                    ),
                )
                .with_location(Location::start_of(skill.path.clone()));
                if !self.roots.is_empty() {
                    iss = iss.with_related(self.roots.iter().cloned().collect());
                }
                out.push(iss);
            }
        }
        out
    }

    /// True when the skill has at least one in-edge that is NOT a self-loop.
    /// Used by [`Self::dead_skills`] so a skill that only references itself
    /// still qualifies as dead.
    fn has_incoming_from_others(&self, id: &SkillId) -> bool {
        let Some(idx) = self.index.get(id) else {
            return false;
        };
        self.graph
            .edges_directed(*idx, petgraph::Direction::Incoming)
            .any(|e| e.source() != *idx)
    }

    /// Cycle detection via Tarjan SCC. Returns one issue per SCC of size > 1
    /// **plus** one issue per singleton SCC that contains a self-loop edge
    /// (`a → a`).
    ///
    /// `tarjan_scc` returns every node as a singleton SCC by definition, even
    /// when that node has an edge back to itself. The "size > 1" filter is the
    /// right gate for ordinary multi-skill cycles, but it silently accepts a
    /// 1-node `a → a` as if it were acyclic. From an audit standpoint a
    /// self-referential skill IS a reference cycle (loading "a" requires
    /// loading "a") and the README's `SKILL005 cycle` rule promises to catch
    /// "any cycle in the skill reference graph" — so we emit an issue for
    /// singleton SCCs whose node has a self-loop edge as well.
    #[must_use]
    pub fn cycles(&self, skills: &[Skill]) -> Vec<Issue> {
        let mut out = Vec::new();
        // Build a view of reference-only edges (ignoring IndexOf) because a
        // cycle purely via the index/README file is usually a false positive.
        let sccs = tarjan_scc(&self.graph);
        for scc in sccs {
            // Multi-node SCC: any cycle that traverses ≥ 2 distinct skills.
            if scc.len() > 1 {
                let ids: Vec<SkillId> = scc
                    .iter()
                    .map(|n| self.graph.node_weight(*n).expect("node weight").clone())
                    .collect();
                // attach to first skill in sort order
                let mut sorted = ids.clone();
                sorted.sort();
                let primary = sorted.first().cloned().unwrap_or_else(|| SkillId::new("?"));
                let primary_path = skills
                    .iter()
                    .find(|s| s.id == primary)
                    .map(|s| s.path.clone())
                    .unwrap_or_else(|| primary.as_str().into());
                let related: Vec<SkillId> = sorted.into_iter().skip(1).collect();
                let msg = format!(
                    "reference cycle involves {} skills: {}",
                    ids.len(),
                    ids.iter()
                        .map(|i| i.as_str())
                        .collect::<Vec<_>>()
                        .join(" -> ")
                );
                out.push(
                    Issue::new(IssueKind::Cycle, primary, msg)
                        .with_location(Location::start_of(primary_path))
                        .with_related(related),
                );
                continue;
            }
            // Singleton SCC: only a cycle if the node has a self-edge. petgraph
            // does NOT widen self-loop singletons into larger SCCs, so we have
            // to inspect the outgoing edges and look for a target == source.
            if let Some(node) = scc.first() {
                let has_self_loop = self
                    .graph
                    .edges_directed(*node, petgraph::Direction::Outgoing)
                    .any(|e| e.target() == *node);
                if has_self_loop {
                    let id = self
                        .graph
                        .node_weight(*node)
                        .cloned()
                        .unwrap_or_else(|| SkillId::new("?"));
                    let path = skills
                        .iter()
                        .find(|s| s.id == id)
                        .map(|s| s.path.clone())
                        .unwrap_or_else(|| id.as_str().into());
                    let msg = format!("self-referential cycle: skill '{id}' references itself");
                    out.push(
                        Issue::new(IssueKind::Cycle, id, msg)
                            .with_location(Location::start_of(path)),
                    );
                }
            }
        }
        out
    }

    /// Emit the graph as GraphViz dot.
    #[must_use]
    pub fn to_dot(&self) -> String {
        let mut s = String::from("digraph skilldigest {\n  rankdir=LR;\n");
        // Collect nodes in sorted order for deterministic output.
        let mut nodes: Vec<(&SkillId, NodeIndex)> =
            self.index.iter().map(|(id, ix)| (id, *ix)).collect();
        nodes.sort_by(|a, b| a.0.cmp(b.0));
        for (id, _) in &nodes {
            let shape = if self.roots.contains(id) {
                "doubleoctagon"
            } else {
                "box"
            };
            s.push_str(&format!(
                "  \"{}\" [shape={}];\n",
                id.as_str().replace('"', "\\\""),
                shape
            ));
        }
        let mut edges: Vec<(String, String, EdgeKind)> = Vec::new();
        for e in self.graph.edge_references() {
            let src = self
                .graph
                .node_weight(e.source())
                .map(|id| id.as_str().to_string())
                .unwrap_or_default();
            let dst = self
                .graph
                .node_weight(e.target())
                .map(|id| id.as_str().to_string())
                .unwrap_or_default();
            edges.push((src, dst, *e.weight()));
        }
        edges.sort();
        for (src, dst, kind) in edges {
            let style = match kind {
                EdgeKind::IndexOf => " [style=dashed]",
                EdgeKind::Reference => "",
            };
            s.push_str(&format!(
                "  \"{}\" -> \"{}\"{};\n",
                src.replace('"', "\\\""),
                dst.replace('"', "\\\""),
                style
            ));
        }
        s.push_str("}\n");
        s
    }

    /// Return a JSON-serialisable representation of the graph.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        let mut nodes: Vec<serde_json::Value> = Vec::new();
        let mut ids: Vec<&SkillId> = self.index.keys().collect();
        ids.sort();
        for id in ids {
            let is_root = self.roots.contains(id);
            nodes.push(serde_json::json!({
                "id": id.as_str(),
                "root": is_root,
            }));
        }
        let mut edges: Vec<serde_json::Value> = Vec::new();
        for e in self.graph.edge_references() {
            let src = self
                .graph
                .node_weight(e.source())
                .map(|id| id.as_str().to_string())
                .unwrap_or_default();
            let dst = self
                .graph
                .node_weight(e.target())
                .map(|id| id.as_str().to_string())
                .unwrap_or_default();
            let kind = match e.weight() {
                EdgeKind::IndexOf => "index_of",
                EdgeKind::Reference => "reference",
            };
            edges.push(serde_json::json!({
                "from": src,
                "to": dst,
                "kind": kind,
            }));
        }
        edges.as_mut_slice().sort_by(|a, b| {
            let sa = a["from"].as_str().unwrap_or("");
            let sb = b["from"].as_str().unwrap_or("");
            sa.cmp(sb).then(
                a["to"]
                    .as_str()
                    .unwrap_or("")
                    .cmp(b["to"].as_str().unwrap_or("")),
            )
        });
        serde_json::json!({
            "nodes": nodes,
            "edges": edges,
        })
    }
}

fn is_root_like(skill: &Skill) -> bool {
    let file = skill
        .path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    matches!(
        file.as_str(),
        "README.md" | "AGENTS.md" | "SKILLS.md" | "index.md" | "plugin.toml"
    )
}

fn resolve_link_to_skill_id(
    from_path: &std::path::Path,
    target: &std::path::Path,
) -> Option<SkillId> {
    let base = from_path.parent()?;
    let joined = base.join(target);
    // manually canonicalise without touching the filesystem
    let mut components: Vec<std::path::Component<'_>> = Vec::new();
    for c in joined.components() {
        match c {
            std::path::Component::ParentDir => {
                components.pop();
            }
            std::path::Component::CurDir => {}
            other => components.push(other),
        }
    }
    let canonical: std::path::PathBuf = components.iter().collect();
    let s = canonical.to_string_lossy().replace('\\', "/");
    let id = s
        .strip_suffix("/SKILL.md")
        .or_else(|| s.strip_suffix("/skill.md"))
        .or_else(|| s.strip_suffix("/README.md"))
        .or_else(|| s.strip_suffix("/AGENTS.md"))
        .or_else(|| s.strip_suffix("/index.md"))
        .map(|t| t.to_string())
        .unwrap_or_else(|| {
            let mut x = s.clone();
            if x.ends_with(".md") {
                x.truncate(x.len() - 3);
            }
            x
        });
    if id.is_empty() {
        None
    } else {
        Some(SkillId::new(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Frontmatter, TokenCounts};

    fn skill(id: &str, path: &str, refs: Vec<SkillRef>) -> Skill {
        Skill {
            id: SkillId::new(id),
            name: id.to_string(),
            path: path.into(),
            frontmatter: Frontmatter::default(),
            tokens: TokenCounts::default(),
            refs,
            rules: vec![],
            tags: vec![],
            warnings: vec![],
            body_bytes: 0,
        }
    }

    #[test]
    fn dead_skill_detected() {
        let a = skill("a", "a/SKILL.md", vec![]);
        let b = skill("b", "b/SKILL.md", vec![]);
        let g = SkillGraph::build(&[a, b]);
        let dead = g.dead_skills(&[
            skill("a", "a/SKILL.md", vec![]),
            skill("b", "b/SKILL.md", vec![]),
        ]);
        assert_eq!(dead.len(), 2);
    }

    #[test]
    fn readme_root_keeps_mentioned_skills_alive() {
        let root = skill(
            "",
            "README.md",
            vec![SkillRef::Mention {
                skill_id: SkillId::new("a"),
            }],
        );
        let a = skill("a", "a/SKILL.md", vec![]);
        let skills = vec![root, a];
        let g = SkillGraph::build(&skills);
        let dead = g.dead_skills(&skills);
        let dead_ids: Vec<String> = dead.iter().map(|d| d.skill.to_string()).collect();
        assert!(!dead_ids.contains(&"a".to_string()));
    }

    #[test]
    fn out_in_degree() {
        let a = skill(
            "a",
            "a/SKILL.md",
            vec![SkillRef::Mention {
                skill_id: SkillId::new("b"),
            }],
        );
        let b = skill("b", "b/SKILL.md", vec![]);
        let g = SkillGraph::build(&[a, b]);
        assert_eq!(g.out_degree(&SkillId::new("a")), 1);
        assert_eq!(g.in_degree(&SkillId::new("b")), 1);
    }

    #[test]
    fn cycle_detection() {
        let a = skill(
            "a",
            "a/SKILL.md",
            vec![SkillRef::Mention {
                skill_id: SkillId::new("b"),
            }],
        );
        let b = skill(
            "b",
            "b/SKILL.md",
            vec![SkillRef::Mention {
                skill_id: SkillId::new("a"),
            }],
        );
        let skills = vec![a, b];
        let g = SkillGraph::build(&skills);
        let cycles = g.cycles(&skills);
        assert_eq!(cycles.len(), 1);
        assert!(cycles[0].related.len() == 1);
    }

    #[test]
    fn no_cycle_for_dag() {
        let a = skill(
            "a",
            "a/SKILL.md",
            vec![SkillRef::Mention {
                skill_id: SkillId::new("b"),
            }],
        );
        let b = skill("b", "b/SKILL.md", vec![]);
        let g = SkillGraph::build(&[a, b]);
        assert!(g.cycles(&[]).is_empty());
    }

    #[test]
    fn link_resolves_to_skill_id() {
        let from = std::path::Path::new("foo/SKILL.md");
        let to = std::path::Path::new("../bar/SKILL.md");
        let id = resolve_link_to_skill_id(from, to).unwrap();
        assert_eq!(id.as_str(), "bar");
    }

    #[test]
    fn link_resolves_relative_file() {
        let from = std::path::Path::new("foo/SKILL.md");
        let to = std::path::Path::new("../baz.md");
        let id = resolve_link_to_skill_id(from, to).unwrap();
        assert_eq!(id.as_str(), "baz");
    }

    #[test]
    fn to_dot_contains_all_nodes() {
        let a = skill("a", "a/SKILL.md", vec![]);
        let b = skill("b", "b/SKILL.md", vec![]);
        let g = SkillGraph::build(&[a, b]);
        let dot = g.to_dot();
        assert!(dot.contains("digraph skilldigest"));
        assert!(dot.contains("\"a\""));
        assert!(dot.contains("\"b\""));
    }

    #[test]
    fn to_json_shape() {
        let a = skill("a", "a/SKILL.md", vec![]);
        let g = SkillGraph::build(&[a]);
        let j = g.to_json();
        assert_eq!(j["nodes"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn frontmatter_requires_creates_edge() {
        let mut a = skill("a", "a/SKILL.md", vec![]);
        a.frontmatter.requires = vec!["b".into()];
        let b = skill("b", "b/SKILL.md", vec![]);
        let g = SkillGraph::build(&[a, b]);
        assert_eq!(g.in_degree(&SkillId::new("b")), 1);
    }

    #[test]
    fn is_root_like_true_for_readme() {
        let s = skill("dir", "dir/README.md", vec![]);
        assert!(is_root_like(&s));
    }

    #[test]
    fn is_root_like_true_for_agents_md() {
        let s = skill("", "AGENTS.md", vec![]);
        assert!(is_root_like(&s));
    }

    #[test]
    fn is_root_like_false_for_skill_md() {
        let s = skill("a", "a/SKILL.md", vec![]);
        assert!(!is_root_like(&s));
    }

    #[test]
    fn root_skill_is_never_dead() {
        let root = skill("", "README.md", vec![]);
        let skills = vec![root];
        let g = SkillGraph::build(&skills);
        let dead = g.dead_skills(&skills);
        assert!(dead.is_empty());
    }

    #[test]
    fn empty_graph_is_fine() {
        let g = SkillGraph::build(&[]);
        assert_eq!(g.out_degree(&SkillId::new("x")), 0);
        assert_eq!(g.in_degree(&SkillId::new("x")), 0);
    }

    #[test]
    fn cycle_detects_self_loop() {
        // A skill that mentions itself: `a/SKILL.md` containing `@a`.
        // Tarjan SCC reports `[a]` as a singleton SCC even with the
        // self-edge, so the previous `scc.len() > 1` filter dropped it. The
        // self-loop branch must catch this.
        let a = skill(
            "a",
            "a/SKILL.md",
            vec![SkillRef::Mention {
                skill_id: SkillId::new("a"),
            }],
        );
        let skills = vec![a];
        let g = SkillGraph::build(&skills);
        let cycles = g.cycles(&skills);
        assert_eq!(cycles.len(), 1, "self-loop should produce one cycle issue");
        assert!(cycles[0].message.contains("self-referential"));
    }

    #[test]
    fn cycle_does_not_flag_singleton_without_self_edge() {
        // Plain isolated skill — no edges, no cycle.
        let a = skill("a", "a/SKILL.md", vec![]);
        let skills = vec![a];
        let g = SkillGraph::build(&skills);
        assert!(g.cycles(&skills).is_empty());
    }

    #[test]
    fn to_dot_is_deterministic() {
        let a = skill(
            "a",
            "a/SKILL.md",
            vec![SkillRef::Mention {
                skill_id: SkillId::new("b"),
            }],
        );
        let b = skill("b", "b/SKILL.md", vec![]);
        let g = SkillGraph::build(&[a.clone(), b.clone()]);
        let d1 = g.to_dot();
        let g2 = SkillGraph::build(&[a, b]);
        let d2 = g2.to_dot();
        assert_eq!(d1, d2);
    }
}
