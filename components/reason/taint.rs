//! Executable taint propagation + ready graph algorithms (alethfeld's
//! `graph.clj` / `validators.clj`, ported to our schema).
//!
//! The centrepiece is [`propagate`]: three-valued taint over the dependency
//! subtree. A rejected/blocked node is the poison source (`Tainted`); a node
//! explicitly marked as an admitted gap stays `SelfAdmitted`; and any node that
//! transitively *depends on* a poisoned node becomes `Tainted`. This is the
//! executable version of "a rejected/counterexampled node taints its
//! dependents" — the store calls it to persist the three-valued state, and the
//! agent loop leans on it so rejecting a node on a counterexample poisons the
//! branch that relied on it.
//!
//! Alongside it are the pure graph utilities alethfeld shipped and we lacked as
//! reusable code: [`find_cycle`] (acyclicity with the offending cycle path),
//! [`assumption_scope`] (scope algebra — which assumptions a node rests on), and
//! [`extraction_benefit`] (the lemma-extraction benefit metric).

use crate::model::{Edge, EdgeKind, Node, NodeKind, NodeStatus, Taint};
use std::collections::{HashMap, HashSet};

/// Whether an edge carries taint from its target *up* to its source: a
/// dependency/derivation/formalization link propagates a poisoned target to the
/// node that relies on it. Adversarial/replacement links (Contradicts,
/// Supersedes, Verifies) deliberately do not — mirrors `db::recompute_taint`.
pub fn carries_taint(kind: EdgeKind) -> bool {
    matches!(
        kind,
        EdgeKind::DependsOn | EdgeKind::DerivedFrom | EdgeKind::Formalizes
    )
}

/// Three-valued taint propagation over the dependency graph.
///
/// Seeds:
/// * a node with status `Rejected`/`Blocked` is a `Tainted` source (recomputed
///   fresh each call from status, so it is not sticky);
/// * a node whose stored taint is already `SelfAdmitted` is preserved as a
///   `SelfAdmitted` source (an explicit, sticky "this is a gap" mark).
///
/// Propagation: any node that reaches a seed source through a taint-carrying
/// edge (`source` depends on `target`) becomes `Tainted`. A self-admitted node
/// keeps its own `SelfAdmitted` label (its own gap dominates) but still poisons
/// everything that depends on it.
pub fn propagate(nodes: &[Node], edges: &[Edge]) -> HashMap<String, Taint> {
    let self_admitted: HashSet<&str> = nodes
        .iter()
        .filter(|n| n.taint == Taint::SelfAdmitted)
        .map(|n| n.id.as_str())
        .collect();

    // Poison sources: rejected/blocked (Tainted) plus self-admitted gaps.
    let mut poisoned: HashSet<String> = nodes
        .iter()
        .filter(|n| {
            matches!(n.status, NodeStatus::Rejected | NodeStatus::Blocked)
                || self_admitted.contains(n.id.as_str())
        })
        .map(|n| n.id.clone())
        .collect();

    // Transitive closure: a node depending on any poisoned node is poisoned.
    loop {
        let before = poisoned.len();
        for e in edges {
            if carries_taint(e.kind) && poisoned.contains(&e.target_id) {
                poisoned.insert(e.source_id.clone());
            }
        }
        if poisoned.len() == before {
            break;
        }
    }

    nodes
        .iter()
        .map(|n| {
            let taint = if self_admitted.contains(n.id.as_str()) {
                Taint::SelfAdmitted
            } else if poisoned.contains(&n.id) {
                Taint::Tainted
            } else {
                Taint::Clean
            };
            (n.id.clone(), taint)
        })
        .collect()
}

/// The direct + transitive dependents that become tainted when `root` is
/// poisoned: every node that reaches `root` through a taint-carrying edge,
/// excluding `root` itself. Used by the agent loop to report the blast radius
/// of a counterexample.
pub fn tainted_dependents(root: &str, edges: &[Edge]) -> Vec<String> {
    // reverse adjacency: target -> sources that depend on it
    let mut rev: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in edges {
        if carries_taint(e.kind) {
            rev.entry(e.target_id.as_str())
                .or_default()
                .push(e.source_id.as_str());
        }
    }
    let mut seen: HashSet<&str> = HashSet::new();
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if let Some(sources) = rev.get(n) {
            for &s in sources {
                if seen.insert(s) {
                    stack.push(s);
                }
            }
        }
    }
    let mut out: Vec<String> = seen.into_iter().map(str::to_owned).collect();
    out.sort();
    out
}

/// Acyclicity check with cycle-path reconstruction (alethfeld `validators.clj`
/// `find-cycle`, DFS 3-colouring). Returns `Some(path)` where `path` is the
/// nodes of one dependency cycle in order (first repeated node closes it), or
/// `None` when the dependency graph is acyclic.
pub fn find_cycle(nodes: &[Node], edges: &[Edge]) -> Option<Vec<String>> {
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for n in nodes {
        adj.entry(n.id.as_str()).or_default();
    }
    for e in edges {
        if carries_taint(e.kind) {
            adj.entry(e.source_id.as_str())
                .or_default()
                .push(e.target_id.as_str());
        }
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Color {
        White,
        Gray,
        Black,
    }
    let mut color: HashMap<&str, Color> = adj.keys().map(|&k| (k, Color::White)).collect();

    // Iterative DFS keeping an explicit stack of the current path.
    fn dfs<'a>(
        node: &'a str,
        adj: &HashMap<&'a str, Vec<&'a str>>,
        color: &mut HashMap<&'a str, Color>,
        path: &mut Vec<&'a str>,
    ) -> Option<Vec<String>> {
        color.insert(node, Color::Gray);
        path.push(node);
        if let Some(nbrs) = adj.get(node) {
            for &nbr in nbrs {
                match color.get(nbr).copied().unwrap_or(Color::White) {
                    Color::Gray => {
                        // Found a back-edge: slice the path from nbr to close it.
                        let start = path.iter().position(|&p| p == nbr).unwrap_or(0);
                        let mut cycle: Vec<String> =
                            path[start..].iter().map(|s| s.to_string()).collect();
                        cycle.push(nbr.to_string());
                        return Some(cycle);
                    }
                    Color::White => {
                        if let Some(c) = dfs(nbr, adj, color, path) {
                            return Some(c);
                        }
                    }
                    Color::Black => {}
                }
            }
        }
        path.pop();
        color.insert(node, Color::Black);
        None
    }

    let ids: Vec<&str> = adj.keys().copied().collect();
    for id in ids {
        if color.get(id).copied().unwrap_or(Color::White) == Color::White {
            let mut path = Vec::new();
            if let Some(cycle) = dfs(id, &adj, &mut color, &mut path) {
                return Some(cycle);
            }
        }
    }
    None
}

/// Scope algebra (alethfeld `compute-all-scopes`, adapted): the set of
/// `Assumption` node ids that `root` transitively rests on. Assumptions are the
/// scope-defining nodes in our schema, so a node's scope is the assumptions in
/// its dependency closure — what must hold for its statement to be meaningful.
pub fn assumption_scope(root: &str, nodes: &[Node], edges: &[Edge]) -> Vec<String> {
    let by_id: HashMap<&str, &Node> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in edges {
        if e.kind == EdgeKind::DependsOn {
            adj.entry(e.source_id.as_str())
                .or_default()
                .push(e.target_id.as_str());
        }
    }
    let mut seen: HashSet<&str> = HashSet::new();
    let mut stack = vec![root];
    let mut scope: HashSet<String> = HashSet::new();
    while let Some(n) = stack.pop() {
        if !seen.insert(n) {
            continue;
        }
        if let Some(node) = by_id.get(n) {
            if node.kind == NodeKind::Assumption {
                scope.insert(n.to_owned());
            }
        }
        if let Some(targets) = adj.get(n) {
            stack.extend(targets.iter().copied());
        }
    }
    let mut out: Vec<String> = scope.into_iter().collect();
    out.sort();
    out
}

/// Lemma-extraction benefit metric (alethfeld's decomposer:
/// `0.3·size_reduction + 0.3·isolation + 0.2·reusability + 0.2·depth_reduction`).
/// A candidate extraction is a rooted set `S` (the root plus its dependency
/// closure). `total_nodes` sizes the whole graph; `external_dependents` counts
/// how many nodes *outside* `S` depend on a node inside `S` (only the root
/// should, for a clean lemma). Returns a score in `[0, 1]`; alethfeld only
/// proposes extraction when it exceeds `0.4`.
pub fn extraction_benefit(
    set_size: usize,
    total_nodes: usize,
    external_dependents: usize,
    reuse_count: usize,
    set_depth: usize,
    total_depth: usize,
) -> f64 {
    let size_reduction = ratio(set_size, total_nodes);
    // Isolation is best when only the root is depended on from outside (1 hole).
    let isolation = if external_dependents <= 1 {
        1.0
    } else {
        1.0 / external_dependents as f64
    };
    let reusability = (reuse_count as f64 / 3.0).min(1.0);
    let depth_reduction = ratio(set_depth, total_depth);
    0.3 * size_reduction + 0.3 * isolation + 0.2 * reusability + 0.2 * depth_reduction
}

fn ratio(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        (part as f64 / whole as f64).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn node(id: &str, kind: NodeKind, status: NodeStatus, taint: Taint) -> Node {
        let now = Utc::now();
        Node {
            id: id.into(),
            project_id: "p".into(),
            kind,
            status,
            title: id.into(),
            statement: id.into(),
            formal_statement: None,
            provenance: "test".into(),
            content_hash: "h".into(),
            tainted: taint.is_tainted(),
            taint,
            tier: crate::model::NodeTier::Spine,
            parent_id: None,
            strategy_hint: None,
            suggested_lemmas: Vec::new(),
            stmt_formalized: false,
            proof_done: false,
            created_at: now,
            updated_at: now,
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
    fn propagates_taint_over_a_small_dag() {
        // C depends on B depends on A; A is rejected -> B and C tainted.
        let nodes = vec![
            node("A", NodeKind::Lemma, NodeStatus::Rejected, Taint::Clean),
            node("B", NodeKind::Lemma, NodeStatus::Proposed, Taint::Clean),
            node("C", NodeKind::Lemma, NodeStatus::Proposed, Taint::Clean),
            node("D", NodeKind::Lemma, NodeStatus::Proposed, Taint::Clean),
        ];
        let edges = vec![dep(1, "B", "A"), dep(2, "C", "B")];
        let taints = propagate(&nodes, &edges);
        assert_eq!(taints["A"], Taint::Tainted);
        assert_eq!(taints["B"], Taint::Tainted);
        assert_eq!(taints["C"], Taint::Tainted);
        // D is unrelated -> clean.
        assert_eq!(taints["D"], Taint::Clean);
    }

    #[test]
    fn self_admitted_is_preserved_and_poisons_dependents() {
        // B is a self-admitted gap; C depends on B.
        let nodes = vec![
            node("B", NodeKind::Lemma, NodeStatus::Proposed, Taint::SelfAdmitted),
            node("C", NodeKind::Lemma, NodeStatus::Proposed, Taint::Clean),
        ];
        let edges = vec![dep(1, "C", "B")];
        let taints = propagate(&nodes, &edges);
        // The gap keeps its own SelfAdmitted label...
        assert_eq!(taints["B"], Taint::SelfAdmitted);
        // ...but its dependents are Tainted (propagated).
        assert_eq!(taints["C"], Taint::Tainted);
    }

    #[test]
    fn tainted_dependents_reports_blast_radius() {
        let edges = vec![dep(1, "B", "A"), dep(2, "C", "B"), dep(3, "D", "A")];
        let mut deps = tainted_dependents("A", &edges);
        deps.sort();
        assert_eq!(deps, vec!["B".to_string(), "C".to_string(), "D".to_string()]);
    }

    #[test]
    fn find_cycle_returns_the_cycle_path() {
        let nodes = vec![
            node("A", NodeKind::Lemma, NodeStatus::Proposed, Taint::Clean),
            node("B", NodeKind::Lemma, NodeStatus::Proposed, Taint::Clean),
            node("C", NodeKind::Lemma, NodeStatus::Proposed, Taint::Clean),
        ];
        // A -> B -> C -> A is a cycle.
        let edges = vec![dep(1, "A", "B"), dep(2, "B", "C"), dep(3, "C", "A")];
        let cycle = find_cycle(&nodes, &edges).expect("cycle expected");
        // The returned path revisits its first node to close the loop.
        assert_eq!(cycle.first(), cycle.last());
        // It contains all three nodes.
        let set: HashSet<&String> = cycle.iter().collect();
        assert!(set.contains(&"A".to_string()));
        assert!(set.contains(&"B".to_string()));
        assert!(set.contains(&"C".to_string()));
    }

    #[test]
    fn find_cycle_none_when_acyclic() {
        let nodes = vec![
            node("A", NodeKind::Lemma, NodeStatus::Proposed, Taint::Clean),
            node("B", NodeKind::Lemma, NodeStatus::Proposed, Taint::Clean),
        ];
        let edges = vec![dep(1, "A", "B")];
        assert!(find_cycle(&nodes, &edges).is_none());
    }

    #[test]
    fn assumption_scope_collects_transitive_assumptions() {
        let nodes = vec![
            node("root", NodeKind::Lemma, NodeStatus::Proposed, Taint::Clean),
            node("mid", NodeKind::Lemma, NodeStatus::Proposed, Taint::Clean),
            node("h1", NodeKind::Assumption, NodeStatus::Proposed, Taint::Clean),
            node("h2", NodeKind::Assumption, NodeStatus::Proposed, Taint::Clean),
        ];
        let edges = vec![dep(1, "root", "mid"), dep(2, "mid", "h1"), dep(3, "root", "h2")];
        let scope = assumption_scope("root", &nodes, &edges);
        assert_eq!(scope, vec!["h1".to_string(), "h2".to_string()]);
    }

    #[test]
    fn extraction_benefit_favours_isolated_reusable_sets() {
        // Isolated (1 external dependent), reused thrice, meaningful size/depth.
        let good = extraction_benefit(4, 10, 1, 3, 3, 6);
        // Leaky (5 external dependents), never reused, tiny.
        let poor = extraction_benefit(1, 10, 5, 0, 1, 6);
        assert!(good > 0.4);
        assert!(good > poor);
    }
}
