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

use serde::Serialize;
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

/// Standing of a shrunk tactic sequence with respect to the real proof gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MinimizeStatus {
    /// No closed node is reachable, so there was nothing to shrink.
    NoProofFound,
    /// A shorter sequence was found but no gate was run against it. It is a
    /// candidate, not a proof.
    Unverified,
    /// The gate accepted the shrunk sequence.
    Verified,
    /// The gate rejected the shrunk sequence, so the shrink was wrong and the
    /// original must be kept.
    RejectedByGate,
}

impl MinimizeStatus {
    /// Whether a sequence with this status may be emitted as a proof.
    pub fn is_emittable(self) -> bool {
        matches!(self, MinimizeStatus::Verified)
    }
}

/// JSON-able summary of one minimization pass.
#[derive(Debug, Clone, Serialize)]
pub struct MinimizeReport {
    /// Length of the sequence handed in.
    pub original_len: usize,
    /// Length of the BFS-shortest sequence, if any closed node was reachable.
    pub candidate_len: Option<usize>,
    /// Tactics the shrink would remove. Zero when the input was already minimal.
    pub tactics_removed: usize,
    /// Standing of the candidate against the gate.
    pub status: MinimizeStatus,
    /// Whether the candidate was re-checked against a real gate at all. False
    /// means `candidate` is a guess about the DAG, not a checked proof.
    pub gate_checked: bool,
    /// The shrunk sequence, recorded even when rejected so the failure is
    /// inspectable rather than silently dropped.
    pub candidate: Option<Vec<String>>,
}

/// Result of minimizing a solved DAG.
///
/// `accepted` is deliberately the only field a caller should emit as a proof; the
/// shorter `candidate` is exposed for logging and for handing to a gate later.
#[derive(Debug, Clone)]
pub struct MinimizeOutcome {
    /// The shrunk sequence, whatever its standing. Not safe to emit on its own.
    pub candidate: Option<Vec<String>>,
    /// The shrunk sequence only when the gate accepted it. `None` otherwise, so
    /// an unchecked shrink can never be mistaken for an equivalent smaller proof.
    pub accepted: Option<Vec<String>>,
    /// Standing of `candidate`.
    pub status: MinimizeStatus,
    /// Summary for the run log.
    pub report: MinimizeReport,
}

impl MinimizeOutcome {
    /// The sequence to actually emit: the accepted shrink if there is one, else
    /// the original, which already passed the gate upstream.
    pub fn best_safe<'a>(&'a self, original: &'a [String]) -> &'a [String] {
        self.accepted.as_deref().unwrap_or(original)
    }
}

/// Shrink a solved DAG to its shortest closing tactic sequence **without**
/// re-checking it.
///
/// The result is always labelled [`MinimizeStatus::Unverified`] and `accepted` is
/// always `None`. BFS finds a shortest path through the DAG the search recorded,
/// which is only a proof if every edge in that DAG really was a successful tactic
/// application and the recorded child states are exact. Any drift between the DAG
/// and the checker (stale nodes, edges recorded before a later failure, a
/// transposition that merged two states the checker distinguishes) makes the short
/// path a guess. Use [`minimize_proof_checked`] when a gate is available.
pub fn minimize_proof_unverified<G: ProofGraph>(graph: &G, original: &[String]) -> MinimizeOutcome {
    let candidate = minimal_proof(graph);
    let status = match candidate {
        Some(_) => MinimizeStatus::Unverified,
        None => MinimizeStatus::NoProofFound,
    };
    build(original, candidate, status, false)
}

/// Shrink a solved DAG and re-check the result against the real gate.
///
/// `gate` receives the shrunk sequence and returns whether it verifies. Only when
/// it returns `true` does the outcome carry an `accepted` sequence; a rejection
/// keeps the shrink in the report for inspection and leaves `accepted` empty so
/// the caller falls back to the original via [`MinimizeOutcome::best_safe`].
pub fn minimize_proof_checked<G: ProofGraph>(
    graph: &G,
    original: &[String],
    mut gate: impl FnMut(&[String]) -> bool,
) -> MinimizeOutcome {
    let candidate = minimal_proof(graph);
    let status = match candidate.as_deref() {
        None => MinimizeStatus::NoProofFound,
        Some(seq) if gate(seq) => MinimizeStatus::Verified,
        Some(_) => MinimizeStatus::RejectedByGate,
    };
    build(original, candidate, status, true)
}

/// Assemble the outcome, keeping the accepted/candidate split in one place so the
/// two entry points cannot disagree about what is safe to emit.
fn build(
    original: &[String],
    candidate: Option<Vec<String>>,
    status: MinimizeStatus,
    gate_checked: bool,
) -> MinimizeOutcome {
    let candidate_len = candidate.as_ref().map(|c| c.len());
    let tactics_removed = candidate_len
        .map(|n| original.len().saturating_sub(n))
        .unwrap_or(0);
    let accepted = match status {
        MinimizeStatus::Verified => candidate.clone(),
        _ => None,
    };
    MinimizeOutcome {
        report: MinimizeReport {
            original_len: original.len(),
            candidate_len,
            tactics_removed,
            status,
            gate_checked,
            candidate: candidate.clone(),
        },
        candidate,
        accepted,
        status,
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

    /// The DAG from the first test: a 2-tactic line and a 3-tactic detour.
    fn solved_dag() -> AdjacencyGraph {
        AdjacencyGraph::new(0)
            .edge(0, "intro", 1)
            .edge(1, "simp", 3)
            .edge(0, "cases", 4)
            .edge(4, "ring", 5)
            .edge(5, "linarith", 3)
            .close(3)
    }

    fn seq(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn unchecked_shrink_is_labelled_unverified_and_not_accepted() {
        let original = seq(&["cases", "ring", "linarith"]);
        let out = minimize_proof_unverified(&solved_dag(), &original);

        assert_eq!(out.status, MinimizeStatus::Unverified);
        assert_eq!(out.candidate, Some(seq(&["intro", "simp"])));
        assert_eq!(out.accepted, None, "an unchecked shrink is never emittable");
        assert!(!out.status.is_emittable());
        assert!(!out.report.gate_checked);
        assert_eq!(out.report.tactics_removed, 1);
        // Falling back keeps the sequence that did pass a gate upstream.
        assert_eq!(out.best_safe(&original), original.as_slice());
    }

    #[test]
    fn gate_acceptance_makes_the_shrink_emittable() {
        let original = seq(&["cases", "ring", "linarith"]);
        let out = minimize_proof_checked(&solved_dag(), &original, |_| true);

        assert_eq!(out.status, MinimizeStatus::Verified);
        assert_eq!(out.accepted, Some(seq(&["intro", "simp"])));
        assert!(out.report.gate_checked);
        assert_eq!(out.best_safe(&original), seq(&["intro", "simp"]).as_slice());
    }

    #[test]
    fn gate_rejection_keeps_the_original_but_records_the_shrink() {
        let original = seq(&["cases", "ring", "linarith"]);
        let mut saw: Vec<Vec<String>> = Vec::new();
        let out = minimize_proof_checked(&solved_dag(), &original, |s| {
            saw.push(s.to_vec());
            false
        });

        assert_eq!(out.status, MinimizeStatus::RejectedByGate);
        assert_eq!(out.accepted, None);
        assert_eq!(saw, vec![seq(&["intro", "simp"])], "gate saw the shrink");
        assert_eq!(
            out.report.candidate,
            Some(seq(&["intro", "simp"])),
            "the rejected shrink stays in the log"
        );
        assert_eq!(out.best_safe(&original), original.as_slice());
    }

    #[test]
    fn unsolved_dag_reports_no_proof_and_never_runs_the_gate() {
        let g = AdjacencyGraph::new(0).edge(0, "t", 1);
        let original = seq(&["t"]);
        let mut calls = 0;
        let out = minimize_proof_checked(&g, &original, |_| {
            calls += 1;
            true
        });

        assert_eq!(out.status, MinimizeStatus::NoProofFound);
        assert_eq!(calls, 0);
        assert_eq!(out.candidate, None);
        assert_eq!(out.accepted, None);
        assert_eq!(out.report.tactics_removed, 0);
    }

    #[test]
    fn already_minimal_input_removes_nothing() {
        let original = seq(&["intro", "simp"]);
        let out = minimize_proof_checked(&solved_dag(), &original, |_| true);
        assert_eq!(out.report.tactics_removed, 0);
        assert_eq!(out.status, MinimizeStatus::Verified);
    }

    #[test]
    fn report_serializes_to_json() {
        let original = seq(&["cases", "ring", "linarith"]);
        let out = minimize_proof_unverified(&solved_dag(), &original);
        let json = serde_json::to_value(&out.report).expect("report is JSON-able");
        assert_eq!(json["status"], "unverified");
        assert_eq!(json["gate_checked"], false);
        assert_eq!(json["candidate_len"], 2);
        assert_eq!(json["tactics_removed"], 1);
    }
}
