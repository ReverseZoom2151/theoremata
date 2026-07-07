//! Obligation scheduler (plan §7).
//!
//! Pure functions over the proof DAG that produce a work plan: topological
//! levels (independent obligations in the same level can be solved in
//! parallel), a centrality-ordered suggested order (high-leverage obligations
//! first), the subset of levels that is actually open work, and templated
//! sibling families that can be dispatched as a batch.
//!
//! Everything here operates on borrowed `&[Node]` / `&[Edge]` slices and never
//! touches the `Store`, so it is trivially testable.

use crate::model::{Edge, EdgeKind, Node, NodeStatus};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, serde::Serialize)]
pub struct Schedule {
    /// Topological levels over `DependsOn`. Level 0 is nodes with no
    /// dependencies; nodes within a level are mutually independent and may be
    /// solved concurrently.
    pub levels: Vec<Vec<String>>,
    /// A flat suggested order: levels flattened, each level sorted by
    /// descending transitive-dependent centrality (ties broken by id).
    pub order: Vec<String>,
    /// The levels restricted to still-open, untainted work — the batches a
    /// scheduler can actually dispatch in parallel right now.
    pub parallel_batches: Vec<Vec<String>>,
    /// Groups of node ids whose titles form a templated family (e.g. `I₁..I₆`
    /// or `Case 1 / Case 2`), each group of size >= 2. Batchable together.
    pub sibling_groups: Vec<Vec<String>>,
}

fn is_open(status: NodeStatus, tainted: bool) -> bool {
    !tainted && matches!(status, NodeStatus::Proposed | NodeStatus::Active)
}

/// Centrality of each node = the number of nodes that transitively depend on
/// it (reverse reachability over `DependsOn`). A base lemma many things rest on
/// scores high; a leaf conclusion scores zero.
pub fn centrality(nodes: &[Node], edges: &[Edge]) -> HashMap<String, usize> {
    // reverse adjacency: target -> [sources that depend on it]
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in edges {
        if e.kind == EdgeKind::DependsOn {
            dependents
                .entry(e.target_id.as_str())
                .or_default()
                .push(e.source_id.as_str());
        }
    }
    let mut out = HashMap::new();
    for n in nodes {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut stack = vec![n.id.as_str()];
        while let Some(cur) = stack.pop() {
            if let Some(deps) = dependents.get(cur) {
                for &d in deps {
                    if seen.insert(d) {
                        stack.push(d);
                    }
                }
            }
        }
        out.insert(n.id.clone(), seen.len());
    }
    out
}

/// Kahn's algorithm producing topological levels. A `DependsOn` edge
/// `source -> target` means `target` must be scheduled before `source`, so a
/// node is ready once all of its in-set dependency targets are scheduled.
/// Robust to a residual cycle: any leftover nodes are appended as a final
/// level rather than looping forever.
fn topological_levels(nodes: &[Node], edges: &[Edge]) -> Vec<Vec<String>> {
    let ids: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    // remaining unmet dependencies per node (targets still to be scheduled)
    let mut remaining: HashMap<&str, usize> = nodes.iter().map(|n| (n.id.as_str(), 0)).collect();
    // dependents[target] = sources whose count drops when target is scheduled
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in edges {
        if e.kind == EdgeKind::DependsOn
            && ids.contains(e.source_id.as_str())
            && ids.contains(e.target_id.as_str())
        {
            *remaining.get_mut(e.source_id.as_str()).unwrap() += 1;
            dependents
                .entry(e.target_id.as_str())
                .or_default()
                .push(e.source_id.as_str());
        }
    }

    let mut scheduled: HashSet<&str> = HashSet::new();
    let mut levels: Vec<Vec<String>> = Vec::new();
    let mut frontier: Vec<&str> = nodes
        .iter()
        .map(|n| n.id.as_str())
        .filter(|&id| remaining[id] == 0)
        .collect();
    frontier.sort_unstable();

    while !frontier.is_empty() {
        let mut next: Vec<&str> = Vec::new();
        let mut level: Vec<String> = Vec::new();
        for &id in &frontier {
            if !scheduled.insert(id) {
                continue;
            }
            level.push(id.to_owned());
            if let Some(deps) = dependents.get(id) {
                for &s in deps {
                    let r = remaining.get_mut(s).unwrap();
                    *r -= 1;
                    if *r == 0 {
                        next.push(s);
                    }
                }
            }
        }
        if !level.is_empty() {
            levels.push(level);
        }
        next.sort_unstable();
        next.dedup();
        frontier = next;
    }

    // Any nodes left unscheduled participate in a cycle; append them so the
    // schedule still covers every node.
    let leftover: Vec<String> = nodes
        .iter()
        .map(|n| n.id.clone())
        .filter(|id| !scheduled.contains(id.as_str()))
        .collect();
    if !leftover.is_empty() {
        levels.push(leftover);
    }
    levels
}

/// Normalize a title by stripping a trailing index (ascii digits, subscript
/// digits, common separators/brackets) and a trailing roman-numeral token, so
/// members of a templated family collapse to the same key.
fn normalize_title(title: &str) -> String {
    let is_index_char = |c: char| {
        c.is_ascii_digit()
            || ('\u{2080}'..='\u{2089}').contains(&c)
            || c.is_whitespace()
            || matches!(c, '.' | '-' | '_' | '#' | '(' | ')' | '[' | ']' | ':')
    };
    let mut s: String = title.trim().to_string();
    while s.chars().next_back().map(is_index_char).unwrap_or(false) {
        s.pop();
    }
    // Strip a trailing roman-numeral word (e.g. "Case II" -> "Case").
    let trimmed = s.trim_end();
    if let Some(pos) = trimmed.rfind(char::is_whitespace) {
        let last = trimmed[pos..].trim();
        if !last.is_empty() && last.chars().all(|c| "ivxlcdmIVXLCDM".contains(c)) {
            return trimmed[..pos].trim_end().to_string();
        }
    }
    trimmed.to_string()
}

/// Group node ids whose titles share a normalized template key (families of
/// size >= 2). Groups are ordered by their key for determinism.
fn sibling_groups(nodes: &[Node]) -> Vec<Vec<String>> {
    let mut buckets: HashMap<String, Vec<String>> = HashMap::new();
    for n in nodes {
        let key = normalize_title(&n.title);
        if key.is_empty() {
            continue;
        }
        buckets.entry(key).or_default().push(n.id.clone());
    }
    let mut groups: Vec<(String, Vec<String>)> =
        buckets.into_iter().filter(|(_, v)| v.len() >= 2).collect();
    groups.sort_by(|a, b| a.0.cmp(&b.0));
    groups.into_iter().map(|(_, v)| v).collect()
}

pub fn plan(nodes: &[Node], edges: &[Edge]) -> Schedule {
    let levels = topological_levels(nodes, edges);
    let central = centrality(nodes, edges);

    // Flatten to a suggested order, most-central-first within each level.
    let mut order: Vec<String> = Vec::new();
    for level in &levels {
        let mut lvl = level.clone();
        lvl.sort_by(|a, b| central.get(b).cmp(&central.get(a)).then_with(|| a.cmp(b)));
        order.extend(lvl);
    }

    // Only open, untainted nodes are dispatchable work.
    let open: HashSet<&str> = nodes
        .iter()
        .filter(|n| is_open(n.status, n.tainted))
        .map(|n| n.id.as_str())
        .collect();
    let parallel_batches: Vec<Vec<String>> = levels
        .iter()
        .map(|level| {
            level
                .iter()
                .filter(|id| open.contains(id.as_str()))
                .cloned()
                .collect::<Vec<String>>()
        })
        .filter(|level| !level.is_empty())
        .collect();

    Schedule {
        levels,
        order,
        parallel_batches,
        sibling_groups: sibling_groups(nodes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NodeKind, NodeTier, Taint};
    use chrono::Utc;

    fn node(id: &str, title: &str, status: NodeStatus) -> Node {
        Node {
            id: id.into(),
            project_id: "p".into(),
            kind: NodeKind::Obligation,
            status,
            title: title.into(),
            statement: String::new(),
            formal_statement: None,
            provenance: "test".into(),
            content_hash: String::new(),
            tainted: false,
            taint: Taint::Clean,
            tier: NodeTier::Spine,
            parent_id: None,
            strategy_hint: None,
            suggested_lemmas: Vec::new(),
            lean_decls: Vec::new(),
            stmt_formalized: false,
            proof_done: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn dep(id: i64, source: &str, target: &str) -> Edge {
        Edge {
            id,
            project_id: "p".into(),
            source_id: source.into(),
            target_id: target.into(),
            kind: EdgeKind::DependsOn,
            evidence_strength: crate::model::EdgeStrength::NumericScreen,
            dep_scope: crate::model::DepScope::Statement,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn linear_chain_has_one_node_per_level() {
        // C depends on B depends on A: A is the base and must come first.
        let nodes = vec![
            node("A", "base", NodeStatus::Proposed),
            node("B", "mid", NodeStatus::Proposed),
            node("C", "top", NodeStatus::Proposed),
        ];
        let edges = vec![dep(1, "B", "A"), dep(2, "C", "B")];
        let s = plan(&nodes, &edges);
        assert_eq!(s.levels.len(), 3);
        assert_eq!(s.levels[0], vec!["A".to_string()]);
        assert_eq!(s.levels[2], vec!["C".to_string()]);
    }

    #[test]
    fn independent_chains_share_a_parallel_level() {
        // A<-B and C<-D are two disjoint chains; A and C can run in parallel.
        let nodes = vec![
            node("A", "a base", NodeStatus::Proposed),
            node("B", "a top", NodeStatus::Proposed),
            node("C", "c base", NodeStatus::Proposed),
            node("D", "c top", NodeStatus::Proposed),
        ];
        let edges = vec![dep(1, "B", "A"), dep(2, "D", "C")];
        let s = plan(&nodes, &edges);
        assert_eq!(s.levels[0], vec!["A".to_string(), "C".to_string()]);
        assert_eq!(s.parallel_batches[0].len(), 2);
    }

    #[test]
    fn central_root_scores_highest() {
        // A is depended on by B, C, D.
        let nodes = vec![
            node("A", "root", NodeStatus::Proposed),
            node("B", "b", NodeStatus::Proposed),
            node("C", "c", NodeStatus::Proposed),
            node("D", "d", NodeStatus::Proposed),
        ];
        let edges = vec![dep(1, "B", "A"), dep(2, "C", "A"), dep(3, "D", "A")];
        let c = centrality(&nodes, &edges);
        assert_eq!(c["A"], 3);
        assert_eq!(c["B"], 0);
        // Most-central node leads the suggested order.
        let s = plan(&nodes, &edges);
        assert_eq!(s.order.first().unwrap(), "A");
    }

    #[test]
    fn templated_family_is_grouped() {
        let nodes = vec![
            node("e1", "Estimate 1", NodeStatus::Proposed),
            node("e2", "Estimate 2", NodeStatus::Proposed),
            node("e3", "Estimate 3", NodeStatus::Proposed),
            node("x", "Main theorem", NodeStatus::Proposed),
        ];
        let s = plan(&nodes, &[]);
        assert_eq!(s.sibling_groups.len(), 1);
        assert_eq!(s.sibling_groups[0].len(), 3);
    }

    #[test]
    fn tainted_and_closed_nodes_drop_from_parallel_batches() {
        let mut nodes = vec![
            node("A", "base", NodeStatus::FormallyVerified),
            node("B", "top", NodeStatus::Proposed),
        ];
        nodes[1].tainted = true;
        let s = plan(&nodes, &[dep(1, "B", "A")]);
        // A is verified (not open), B is tainted (not open) -> nothing to do.
        assert!(s.parallel_batches.is_empty());
    }
}
