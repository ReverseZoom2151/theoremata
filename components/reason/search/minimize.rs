//! Proof minimization: recover the shortest closing tactic sequence from a solved
//! proof DAG.
//!
//! A finished [`crate::search::driver`] search leaves a proof *DAG* — nodes are
//! goal states, directed edges are tactic applications, and one or more nodes are
//! `closed` (proof complete). Because the graph search explores many branches,
//! the DAG usually contains several closing paths of different lengths (and
//! redundant detours that transpose back onto the main line). The *proof* we want
//! to emit is the **shortest** tactic sequence from the root goal to a closed
//! node — every extra tactic is dead weight a reader and the compiler both pay
//! for.
//!
//! This module treats the DAG abstractly through the [`ProofGraph`] trait (so the
//! real driver DAG or a test mock plugs in identically) and runs a breadth-first
//! shortest-path from the root, since every tactic edge has unit cost. The result
//! is the ordered list of tactic labels along that shortest path.

use std::collections::{HashMap, VecDeque};

/// An abstract solved proof DAG the minimizer walks. Nodes are addressed by a
/// `usize` id; edges carry the tactic text that produced them.
///
/// Implemented by the real driver DAG and by test mocks alike (see
/// [`AdjacencyGraph`], which doubles as a builder for both).
pub trait ProofGraph {
    /// The root goal node id (the search's starting state).
    fn root(&self) -> usize;

    /// Whether `node` is a closed (proof-complete) state.
    fn is_closed(&self, node: usize) -> bool;

    /// The out-edges of `node` as `(tactic, child_node)` pairs, in a stable order
    /// (the order ties are broken by during the shortest-path search).
    fn successors(&self, node: usize) -> Vec<(String, usize)>;
}

/// Recover the minimal closing tactic sequence: the tactic labels along a
/// shortest path from the graph's root to any closed node, or `None` if no closed
/// node is reachable (an unsolved DAG).
///
/// Unit-cost BFS, so the first closed node reached sits on a shortest path. Ties
/// are resolved deterministically by [`ProofGraph::successors`] order and FIFO
/// discovery, so the same DAG always yields the same sequence.
pub fn minimal_proof<G: ProofGraph>(graph: &G) -> Option<Vec<String>> {
    let root = graph.root();

    // Trivial: the root is already closed — the empty tactic sequence proves it.
    if graph.is_closed(root) {
        return Some(Vec::new());
    }

    // BFS, recording each node's predecessor edge so the path can be rebuilt.
    let mut came_from: HashMap<usize, (usize, String)> = HashMap::new();
    let mut visited: HashMap<usize, ()> = HashMap::new();
    visited.insert(root, ());
    let mut queue: VecDeque<usize> = VecDeque::new();
    queue.push_back(root);

    while let Some(node) = queue.pop_front() {
        for (tactic, child) in graph.successors(node) {
            if visited.contains_key(&child) {
                continue;
            }
            visited.insert(child, ());
            came_from.insert(child, (node, tactic));
            if graph.is_closed(child) {
                return Some(reconstruct(root, child, &came_from));
            }
            queue.push_back(child);
        }
    }
    None
}

/// Walk the predecessor chain from `target` back to `root`, collecting tactic
/// labels, then reverse to get root-to-target order.
fn reconstruct(
    root: usize,
    target: usize,
    came_from: &HashMap<usize, (usize, String)>,
) -> Vec<String> {
    let mut tactics = Vec::new();
    let mut node = target;
    while node != root {
        let (prev, tactic) = &came_from[&node];
        tactics.push(tactic.clone());
        node = *prev;
    }
    tactics.reverse();
    tactics
}

/// A concrete adjacency-list [`ProofGraph`] — the injectable graph used to feed a
/// real driver DAG (or a mock) into [`minimal_proof`]. Build it fluently with
/// [`edge`](Self::edge) and mark closed nodes with [`close`](Self::close).
#[derive(Debug, Clone, Default)]
pub struct AdjacencyGraph {
    root: usize,
    edges: HashMap<usize, Vec<(String, usize)>>,
    closed: std::collections::BTreeSet<usize>,
}

impl AdjacencyGraph {
    /// A graph rooted at `root`.
    pub fn new(root: usize) -> Self {
        Self {
            root,
            edges: HashMap::new(),
            closed: std::collections::BTreeSet::new(),
        }
    }

    /// Add a tactic edge `from -[tactic]-> to` (insertion order is preserved and
    /// is the tie-break order during minimization).
    pub fn edge(mut self, from: usize, tactic: &str, to: usize) -> Self {
        self.edges
            .entry(from)
            .or_default()
            .push((tactic.to_string(), to));
        self
    }

    /// Mark `node` as closed (proof-complete).
    pub fn close(mut self, node: usize) -> Self {
        self.closed.insert(node);
        self
    }
}

impl ProofGraph for AdjacencyGraph {
    fn root(&self) -> usize {
        self.root
    }
    fn is_closed(&self, node: usize) -> bool {
        self.closed.contains(&node)
    }
    fn successors(&self, node: usize) -> Vec<(String, usize)> {
        self.edges.get(&node).cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortest_path_beats_a_redundant_longer_branch() {
        // Node 0 = root goal, node 3 = closed.
        // Short line:  0 -[intro]-> 1 -[simp]-> 3   (2 tactics)
        // Long detour: 0 -[cases]-> 4 -[ring]-> 5 -[linarith]-> 3  (3 tactics)
        let g = AdjacencyGraph::new(0)
            .edge(0, "intro", 1)
            .edge(1, "simp", 3)
            .edge(0, "cases", 4)
            .edge(4, "ring", 5)
            .edge(5, "linarith", 3)
            .close(3);

        let proof = minimal_proof(&g).expect("a closed node is reachable");
        assert_eq!(proof, vec!["intro".to_string(), "simp".to_string()]);
    }

    #[test]
    fn already_closed_root_needs_no_tactics() {
        let g = AdjacencyGraph::new(0).close(0);
        assert_eq!(minimal_proof(&g), Some(Vec::new()));
    }

    #[test]
    fn unsolved_dag_returns_none() {
        // No closed node reachable.
        let g = AdjacencyGraph::new(0).edge(0, "t", 1).edge(1, "u", 2);
        assert_eq!(minimal_proof(&g), None);
    }

    #[test]
    fn transposition_diamond_takes_either_two_step_path() {
        // Diamond both arms length 2: 0->1->3 and 0->2->3, 3 closed. Either
        // 2-tactic path is minimal; BFS + successor order picks the first arm.
        let g = AdjacencyGraph::new(0)
            .edge(0, "l", 1)
            .edge(0, "r", 2)
            .edge(1, "a", 3)
            .edge(2, "b", 3)
            .close(3);
        let proof = minimal_proof(&g).unwrap();
        assert_eq!(proof.len(), 2);
        assert_eq!(proof, vec!["l".to_string(), "a".to_string()]);
    }
}
