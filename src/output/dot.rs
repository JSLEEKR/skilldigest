//! GraphViz dot renderer — thin wrapper around [`SkillGraph::to_dot`].

use crate::graph::SkillGraph;

/// Render the skill graph as GraphViz dot.
#[must_use]
pub fn render(graph: &SkillGraph) -> String {
    graph.to_dot()
}

/// Render the skill graph as a JSON document.
#[must_use]
pub fn render_json(graph: &SkillGraph) -> String {
    serde_json::to_string_pretty(&graph.to_json()).unwrap_or_else(|_| "{}".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_produces_dot() {
        let g = SkillGraph::build(&[]);
        assert!(render(&g).starts_with("digraph skilldigest"));
    }

    #[test]
    fn render_json_is_parseable() {
        let g = SkillGraph::build(&[]);
        let s = render_json(&g);
        let _: serde_json::Value = serde_json::from_str(&s).unwrap();
    }
}
