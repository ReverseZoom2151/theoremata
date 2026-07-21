//! Project a live MCGS search DAG into the process-reward *tree* world.
//!
//! [`crate::search::driver`] runs a real Monte-Carlo *Graph* Search: its
//! `DagNode` arena is a transposition-collapsed DAG (two tactic paths reaching an
//! α-equivalent goal share **one** node, its visit statistics, and all downstream
//! work). [`crate::search::process_reward`] is the AlphaMath-style **process
//! supervision** core — [`backup_q`](super::process_reward::backup_q),
//! [`q_targets`](super::process_reward::q_targets), and the backup-free
//! [`step_beam_select`](super::process_reward::step_beam_select) — but it consumes
//! a decoupled [`SearchTree`] of [`TreeNode`]s (a *tree*: one parent per node,
//! `±1` terminal leaves). Its own doc-comment names the missing seam:
//!
//! > *"A real integration would project a finished driver DAG into this shape
//! > (one `TreeNode` per proof state, terminal reward = the formal-gate verdict)."*
//!
//! This module **is** that seam. It turns real search output into the two shapes
//! the already-tested process-reward selectors expect:
//!
//! * [`project_dag_to_tree`] — **unrolls** the DAG into a [`SearchTree`] (a shared
//!   node reached by two paths becomes two tree nodes, so the tree view is a
//!   genuine tree). Each closed proof node becomes a `+1` terminal leaf, each
//!   node's scalar value is carried into [`TreeNode::value_estimate`], and every
//!   internal reasoning node is marked step-final. The
//!   [`backup_q`](super::process_reward::backup_q) →
//!   [`q_targets`](super::process_reward::q_targets) pipeline and
//!   [`step_beam_select`](super::process_reward::step_beam_select) (over the tree's
//!   leaves) then run on **live** search rather than only unit-test fixtures.
//! * [`project_dag_nodes`] — a **1:1** faithful mirror (`Vec<TreeNode>`, one node
//!   per DAG node) that preserves the DAG's own `visits` / `value_sum` /
//!   `value_estimate` verbatim, since those private fields cannot be set through
//!   the [`SearchTree`] builder. This is the slice
//!   [`step_beam_select`](super::process_reward::step_beam_select) ranks over when
//!   the caller wants the DAG's live statistics, and the exact round-trip target.
//!
//! Everything here is **offline, pure, and deterministic**: the projection reads a
//! finished [`DagView`] snapshot and walks it in edge order — no wall-clock, no
//! randomness, no model, no Lean. It only *connects* the existing tested selectors
//! to real search output; it does not itself search.
//!
//! ## Input: [`DagView`]
//!
//! The driver's `DagNode`/`Edge` arena is private and lives inside
//! `run_attempt`, so this function consumes a small public [`DagView`] snapshot
//! instead. See the module-level integration note (and the crate REPORT) for the
//! one-liner accessor the driver should grow to hand one out.

use super::process_reward::{backup_q, gate_reward, q_targets, SearchTree, TreeNode};
use crate::db::Store;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Event type under which a projection summary is persisted.
const EVENT_TYPE: &str = "search.dag_projection";

/// Max unroll depth — a guard against a DAG whose cycles or deep sharing would
/// otherwise unroll without bound. Real proof DAGs are shallow; this only caps
/// pathological input. Combined with the on-path cycle guard the unroll always
/// terminates.
pub const MAX_UNROLL_DEPTH: usize = 64;

/// A read-only snapshot of the driver's search DAG — the input this module
/// projects. Mirrors the fields of the driver's private `DagNode`/`Edge` that the
/// process-reward selectors care about (identity, closure, value signals, visit
/// statistics, and out-edges), and nothing else.
/// Serde lives on the view (not on the driver's private arena) so a CLI arm can
/// hand a finished search DAG in as JSON until the driver grows its own accessor.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DagView {
    /// The DAG's nodes, indexed exactly as the driver indexes its arena (edge
    /// `child` fields point into this vector).
    #[serde(default)]
    pub nodes: Vec<DagViewNode>,
    /// Arena index of the root goal (the driver always pushes it first, so this is
    /// normally `0`).
    #[serde(default)]
    pub root: usize,
}

/// One node of a [`DagView`] — the public echo of the driver's `DagNode`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagViewNode {
    /// The state's canonical dedup key (the transposition-table key). Used only as
    /// a stable human-readable label; identity in the tree is positional.
    pub key: String,
    /// Whether the proof is complete at this state (`is_closed`). A closed node
    /// projects to a `+1` terminal leaf — the formal-gate PASS verdict.
    #[serde(default)]
    pub closed: bool,
    /// LeanProgress-style progress estimate in `[0, 1]`.
    #[serde(default)]
    pub progress: f64,
    /// Trained-critic `V(s)` in `[0, 1]` (defaults to `progress` when no critic is
    /// injected, exactly as the driver stores it).
    #[serde(default)]
    pub critic: f64,
    /// Simulations that passed through this node during search.
    #[serde(default)]
    pub visits: usize,
    /// Sum of backed-up rewards over those simulations. Mean `Q = value_sum /
    /// visits`.
    #[serde(default)]
    pub value_sum: f64,
    /// Out-edges (tactic applications) to (possibly shared) child nodes.
    #[serde(default)]
    pub edges: Vec<DagViewEdge>,
}

/// One out-edge of a [`DagViewNode`] — the public echo of the driver's `Edge`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagViewEdge {
    /// The tactic text applied.
    #[serde(default)]
    pub tactic: String,
    /// The prior / weight this tactic carried.
    #[serde(default)]
    pub prior: f64,
    /// Arena index of the child node this edge points at.
    pub child: usize,
}

impl DagView {
    /// An empty view rooted at index `0`.
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            root: 0,
        }
    }

    /// Push a node and return its index (convenience for building a view).
    pub fn push(&mut self, node: DagViewNode) -> usize {
        let idx = self.nodes.len();
        self.nodes.push(node);
        idx
    }
}

impl DagViewNode {
    /// An open (non-closed) node whose critic falls back to `progress`.
    pub fn open(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            closed: false,
            progress: 0.0,
            critic: 0.0,
            visits: 0,
            value_sum: 0.0,
            edges: Vec::new(),
        }
    }

    /// A closed (proof-complete) node — projects to a `+1` terminal leaf.
    pub fn closed(key: impl Into<String>) -> Self {
        Self {
            closed: true,
            ..Self::open(key)
        }
    }

    /// Set progress (and mirror it into the critic fallback, as the driver does
    /// when no critic is injected).
    pub fn with_progress(mut self, p: f64) -> Self {
        self.progress = p;
        self.critic = p;
        self
    }

    /// Set the Monte-Carlo statistics `(visits, value_sum)`.
    pub fn with_stats(mut self, visits: usize, value_sum: f64) -> Self {
        self.visits = visits;
        self.value_sum = value_sum;
        self
    }

    /// Add an out-edge to `child` (by arena index).
    pub fn edge(mut self, tactic: impl Into<String>, prior: f64, child: usize) -> Self {
        self.edges.push(DagViewEdge {
            tactic: tactic.into(),
            prior,
            child,
        });
        self
    }

    /// The DAG node's scalar value signal, mapped into a [`TreeNode::value_estimate`]:
    /// the backed-up mean `Q = value_sum / visits` once visited, else the static
    /// `progress` estimate (what selection falls back to before a node is visited).
    /// Kept in `[0, 1]`; [`step_beam_select`](super::process_reward::step_beam_select)
    /// ranks by relative order, so the exact scale is irrelevant.
    fn value(&self) -> f64 {
        if self.visits > 0 {
            self.value_sum / self.visits as f64
        } else {
            self.progress
        }
    }
}

/// Unroll a live search [`DagView`] into a [`SearchTree`] the process-reward
/// pipeline consumes.
///
/// The DAG is walked from its root in edge order, **duplicating** any transposed
/// node reached by more than one path into distinct tree nodes (so the result is a
/// true tree, one parent per node). During the walk:
/// * a **closed** proof node becomes a `+1` terminal leaf (the formal-gate PASS
///   verdict) — the only place a `±1` enters, exactly as the process-reward core
///   requires;
/// * every internal (non-root, non-terminal) node is marked **step-final**, so
///   [`q_targets`](super::process_reward::q_targets) emits its backed-up `Q`;
/// * each node's scalar value ([`DagViewNode::value`]) is carried into
///   [`TreeNode::value_estimate`], so
///   [`step_beam_select`](super::process_reward::step_beam_select) over the
///   returned tree's leaves reflects the real search's value signal.
///
/// The returned tree's `visits`/`value_sum` start at zero — they are the
/// process-reward core's *own* Monte-Carlo backup, filled by calling
/// [`backup_q`](super::process_reward::backup_q) on it (that is precisely the
/// value-backup selector "running on real search output"). To instead preserve the
/// DAG's *own* backed-up statistics verbatim, use [`project_dag_nodes`].
///
/// Deterministic and offline: same [`DagView`] ⇒ identical tree, every time.
/// Cycles and pathological sharing are bounded by an on-path guard and
/// [`MAX_UNROLL_DEPTH`], so the walk always terminates.
pub fn project_dag_to_tree(dag: &DagView) -> SearchTree {
    let mut tree = SearchTree::new();
    if dag.nodes.is_empty() || dag.root >= dag.nodes.len() {
        return tree;
    }
    let mut on_path = vec![false; dag.nodes.len()];
    unroll(
        dag,
        dag.root,
        None,
        MAX_UNROLL_DEPTH,
        &mut on_path,
        &mut tree,
    );
    tree
}

/// Recursively unroll `idx` under `parent` (a tree id, `None` for the root).
fn unroll(
    dag: &DagView,
    idx: usize,
    parent: Option<usize>,
    depth: usize,
    on_path: &mut [bool],
    tree: &mut SearchTree,
) {
    let node = &dag.nodes[idx];
    // Place this node in the tree. The root is added via `add_root`; a closed proof
    // node becomes a terminal `+1` leaf; anything else is an internal step node.
    let tree_id = match parent {
        None => tree.add_root(),
        Some(p) => {
            if node.closed {
                tree.add_leaf(p, true) // gate PASS -> +1
            } else {
                tree.add_node(p)
            }
        }
    };
    tree.set_value_estimate(tree_id, node.value());
    // Internal reasoning steps (not the root, not a terminal leaf) are step-final,
    // so `q_targets` emits their backed-up Q.
    if parent.is_some() && !node.closed {
        tree.mark_step_final(tree_id);
    }

    // A closed leaf has no successors; a depth-exhausted node stops unrolling.
    if node.closed || depth == 0 {
        return;
    }
    on_path[idx] = true;
    for e in &node.edges {
        // Guard the two ways the walk could fail to terminate: an edge back onto a
        // node already on this root-path (a DAG cycle / transposition to an
        // ancestor), and an out-of-range child index.
        if e.child >= dag.nodes.len() || on_path[e.child] {
            continue;
        }
        unroll(dag, e.child, Some(tree_id), depth - 1, on_path, tree);
    }
    on_path[idx] = false;
}

/// Project a live search [`DagView`] into a **1:1** slice of [`TreeNode`]s — one
/// node per DAG node — preserving the DAG's `visits` / `value_sum` /
/// `value_estimate` / terminal verdict **verbatim**.
///
/// [`SearchTree`]'s `visits`/`value_sum` are private and settable only by its own
/// [`backup_q`](super::process_reward::backup_q), so [`project_dag_to_tree`]
/// cannot transplant the DAG's live statistics into the tree it returns. This
/// function does — [`TreeNode`]'s fields are public, so the DAG's numbers are
/// mirrored exactly. Node `id` equals the DAG arena index; `children` mirror the
/// DAG out-edges (adjacency, not a strict single-parent tree); `parent` is the
/// source of the first in-edge (the root's is `None`). A closed node carries the
/// `+1` gate verdict in `terminal`.
///
/// This is the slice [`step_beam_select`](super::process_reward::step_beam_select)
/// ranks over when the caller wants the DAG's own live value estimates, and the
/// exact structural round-trip target (node count, edges, visits, values).
///
/// Deterministic and offline.
pub fn project_dag_nodes(dag: &DagView) -> Vec<TreeNode> {
    // First in-edge source for each node, in arena order — the mirror's `parent`.
    let mut parent_of: Vec<Option<usize>> = vec![None; dag.nodes.len()];
    for (src, node) in dag.nodes.iter().enumerate() {
        for e in &node.edges {
            if e.child < dag.nodes.len() && parent_of[e.child].is_none() && e.child != dag.root {
                parent_of[e.child] = Some(src);
            }
        }
    }

    dag.nodes
        .iter()
        .enumerate()
        .map(|(id, node)| TreeNode {
            id,
            parent: parent_of[id],
            children: node
                .edges
                .iter()
                .filter(|e| e.child < dag.nodes.len())
                .map(|e| e.child)
                .collect(),
            terminal: if node.closed {
                Some(gate_reward(true))
            } else {
                None
            },
            // A step boundary at every internal (non-root, non-terminal) node,
            // matching the unrolled tree's marking.
            step_final: id != dag.root && !node.closed,
            value_estimate: node.value(),
            visits: node.visits,
            value_sum: node.value_sum,
        })
        .collect()
}

/// CLI entry point: project a finished search [`DagView`] into the process-reward
/// world and persist the result.
///
/// A thin adapter over the two projections in this module plus the already-tested
/// backup pipeline: [`project_dag_to_tree`] unrolls the DAG, [`backup_q`] turns the
/// closed-goal `+1` verdicts into a mean reward on every ancestor, and
/// [`q_targets`] reads off the per-step regression labels a value head trains on.
/// [`project_dag_nodes`] is reported alongside so the caller can see the DAG's own
/// statistics survived the projection unchanged.
///
/// Offline and deterministic: no model, no Lean, no wall-clock in the projection
/// itself. The returned JSON is also what is written to the store under
/// `search.dag_projection`, so a run is inspectable from the event log alone.
pub fn project_search_dag(
    store: &Store,
    project_id: &str,
    dag: &DagView,
) -> Result<serde_json::Value> {
    let mut tree = project_dag_to_tree(dag);
    backup_q(&mut tree);
    let targets = q_targets(&tree);
    let mirror = project_dag_nodes(dag);

    let terminal_leaves = tree.nodes().iter().filter(|n| n.is_terminal()).count();
    // The unroll duplicates transposed nodes, so a tree larger than the DAG is the
    // signal that the search actually shared states. Worth reporting, not an error.
    let summary = json!({
        "project_id": project_id,
        "dag_nodes": dag.nodes.len(),
        "tree_nodes": tree.nodes().len(),
        "mirror_nodes": mirror.len(),
        "terminal_leaves": terminal_leaves,
        "unrolled_duplicates": tree.nodes().len().saturating_sub(dag.nodes.len()),
        "root_q": if tree.nodes().is_empty() { 0.0 } else { tree.q(0) },
        "q_targets": targets,
        "max_unroll_depth": MAX_UNROLL_DEPTH,
    });
    store.event(
        Some(project_id),
        None,
        EVENT_TYPE,
        "dag_projection",
        summary.clone(),
    )?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::super::process_reward::{step_beam_select, REWARD_PASS};
    use super::*;
    use std::path::Path;

    /// A solvable chain `g3 -> g2 -> g1 -> g0(closed)`, the DAG a driver would
    /// produce for a linear proof — with crafted visit statistics on each node.
    fn chain_view() -> DagView {
        let mut v = DagView::new();
        // Push closed leaf first so indices are stable, then wire parents by index.
        let g0 = v.push(
            DagViewNode::closed("g0")
                .with_stats(1, 1.0)
                .with_progress(1.0),
        );
        let g1 = v.push(
            DagViewNode::open("g1")
                .with_stats(2, 2.0)
                .with_progress(0.7)
                .edge("close", 1.0, g0),
        );
        let g2 = v.push(
            DagViewNode::open("g2")
                .with_stats(3, 3.0)
                .with_progress(0.4)
                .edge("close", 1.0, g1),
        );
        let g3 = v.push(
            DagViewNode::open("g3")
                .with_stats(4, 4.0)
                .with_progress(0.1)
                .edge("close", 1.0, g2),
        );
        v.root = g3;
        v
    }

    /// A diamond `A -> {B, C}; B -> D; C -> D; D(closed)` — the canonical
    /// transposition the MCGS driver collapses to ONE node D. Projection must
    /// *unroll* it back into two tree copies of D.
    fn diamond_view() -> DagView {
        let mut v = DagView::new();
        let d = v.push(DagViewNode::closed("D").with_progress(1.0));
        let b = v.push(DagViewNode::open("B").with_progress(0.6).edge("d1", 1.0, d));
        let c = v.push(DagViewNode::open("C").with_progress(0.6).edge("d2", 1.0, d));
        let a = v.push(
            DagViewNode::open("A")
                .with_progress(0.3)
                .edge("l", 0.5, b)
                .edge("r", 0.5, c),
        );
        v.root = a;
        v
    }

    #[test]
    fn chain_projects_to_faithful_tree_preserving_structure() {
        let dag = chain_view();
        let tree = project_dag_to_tree(&dag);

        // A linear chain has no transposition, so the tree mirrors it 1:1:
        // 4 nodes, 3 parent->child edges.
        assert_eq!(tree.nodes().len(), 4, "chain must project to 4 tree nodes");
        let edges = tree.nodes().len() - 1;
        assert_eq!(edges, 3, "a 4-node tree has exactly 3 edges");

        // Exactly one terminal leaf (the closed g0), carrying a +1 gate verdict.
        let terminals: Vec<&TreeNode> = tree.nodes().iter().filter(|n| n.is_terminal()).collect();
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0].terminal, Some(REWARD_PASS));

        // value_estimate is carried from the DAG: the root g3 (unvisited-in-mean
        // sense here it IS visited, so mean = value_sum/visits = 4/4 = 1.0... use
        // the leaf, which is closed with progress 1.0 and mean 1.0).
        let root = tree.node(0);
        assert!(
            (root.value_estimate - 1.0).abs() < 1e-12,
            "g3 mean = 4/4 = 1.0"
        );
    }

    #[test]
    fn one_to_one_mirror_preserves_nodes_edges_visits_values() {
        let dag = chain_view();
        let nodes = project_dag_nodes(&dag);

        // Node count preserved exactly.
        assert_eq!(nodes.len(), dag.nodes.len());

        // Per-node: visits, value_sum and value_estimate mirror the DAG verbatim,
        // and out-edge adjacency (children) mirrors the DAG edges.
        for (i, dn) in dag.nodes.iter().enumerate() {
            let tn = &nodes[i];
            assert_eq!(tn.id, i);
            assert_eq!(tn.visits, dn.visits, "visits preserved at node {i}");
            assert!(
                (tn.value_sum - dn.value_sum).abs() < 1e-12,
                "value_sum preserved at node {i}"
            );
            let expect_ve = if dn.visits > 0 {
                dn.value_sum / dn.visits as f64
            } else {
                dn.progress
            };
            assert!(
                (tn.value_estimate - expect_ve).abs() < 1e-12,
                "value_estimate preserved at node {i}"
            );
            let dag_children: Vec<usize> = dn.edges.iter().map(|e| e.child).collect();
            assert_eq!(tn.children, dag_children, "edges preserved at node {i}");
        }

        // The closed leaf is terminal (+1); every open node is not.
        let closed_idx = 0; // g0 was pushed first
        assert_eq!(nodes[closed_idx].terminal, Some(REWARD_PASS));
        assert!(nodes[closed_idx].is_terminal());
        assert_eq!(nodes.iter().filter(|n| n.is_terminal()).count(), 1);
    }

    #[test]
    fn diamond_transposition_is_unrolled_into_two_tree_nodes() {
        let dag = diamond_view();
        // The DAG has 4 distinct nodes (A, B, C, D) — D shared by two paths.
        assert_eq!(dag.nodes.len(), 4);

        let tree = project_dag_to_tree(&dag);
        // Unrolled: A, B, D(under B), C, D(under C) = 5 tree nodes. The shared D is
        // duplicated so the tree view is a genuine tree.
        assert_eq!(
            tree.nodes().len(),
            5,
            "the shared node D must unroll into two tree copies"
        );
        // Two terminal leaves now — one per unrolled copy of the closed D.
        assert_eq!(tree.nodes().iter().filter(|n| n.is_terminal()).count(), 2);
    }

    #[test]
    fn value_backup_selector_runs_on_projected_tree() {
        // The value-backup selector (backup_q + q_targets) — previously exercised
        // only on hand-built fixtures — runs on projected live search output.
        let dag = chain_view();
        let mut tree = project_dag_to_tree(&dag);
        backup_q(&mut tree);

        // One passing simulation flows through the whole chain, so every node's
        // backed-up Q is +1 and the root records exactly one visit.
        assert_eq!(
            tree.node(0).visits,
            1,
            "one terminal simulation reaches root"
        );
        assert!((tree.q(0) - 1.0).abs() < 1e-12);

        // q_targets emits a label at each internal step-final node (g1, g2, g3 —
        // not the root's terminal leaf, not the root itself which is step-final=
        // false only if root... here root g3 IS internal). Root is step_final=false
        // by construction, so targets are g2, g1 (the two internal non-root steps).
        let targets = q_targets(&tree);
        assert!(!targets.is_empty(), "projected tree yields process targets");
        for t in &targets {
            assert!(
                (t.q - 1.0).abs() < 1e-12,
                "every step on the winning path scores +1"
            );
        }
    }

    #[test]
    fn step_beam_selector_picks_expected_node_on_projected_tree() {
        // A branching frontier: root R -> three open leaves with different value
        // signals. step_beam_select must rank them by the projected value_estimate.
        let mut v = DagView::new();
        let a = v.push(DagViewNode::open("A").with_progress(0.2));
        let b = v.push(DagViewNode::open("B").with_progress(0.9));
        let c = v.push(DagViewNode::open("C").with_progress(0.5));
        let r = v.push(
            DagViewNode::open("R")
                .with_progress(0.0)
                .edge("ta", 0.3, a)
                .edge("tb", 0.3, b)
                .edge("tc", 0.3, c),
        );
        v.root = r;

        let tree = project_dag_to_tree(&v);
        // Tree ids (unroll order R,A,B,C): R=0, A=1, B=2, C=3. Leaves are A,B,C.
        let leaf_ids = tree.leaves();
        assert_eq!(leaf_ids, vec![1, 2, 3]);
        let frontier: Vec<TreeNode> = leaf_ids.iter().map(|&i| tree.node(i).clone()).collect();

        // Highest value_estimate is B (0.9) at tree id 2.
        let top = step_beam_select(&frontier, 1);
        assert_eq!(
            top,
            vec![2],
            "beam must pick the highest-value frontier node (B)"
        );
        assert!((tree.node(2).value_estimate - 0.9).abs() < 1e-12);

        // Full ranking is B(0.9) > C(0.5) > A(0.2).
        let all = step_beam_select(&frontier, 9);
        assert_eq!(all, vec![2, 3, 1]);
    }

    #[test]
    fn step_beam_selector_runs_on_one_to_one_mirror_with_live_stats() {
        // The 1:1 mirror preserves the DAG's live statistics, so a beam over the
        // mirror's leaves ranks by the DAG's own value estimates.
        let dag = chain_view();
        let nodes = project_dag_nodes(&dag);
        // The only leaf (no children) is the closed g0 at index 0.
        let leaves: Vec<TreeNode> = nodes
            .iter()
            .filter(|n| n.children.is_empty())
            .cloned()
            .collect();
        assert_eq!(leaves.len(), 1);
        let top = step_beam_select(&leaves, 4);
        assert_eq!(top, vec![0]);
    }

    #[test]
    fn projection_is_deterministic() {
        let dag = diamond_view();
        let t1 = project_dag_to_tree(&dag);
        let t2 = project_dag_to_tree(&dag);
        // TreeNode has no PartialEq; compare a stable Debug rendering instead.
        assert_eq!(format!("{:?}", t1.nodes()), format!("{:?}", t2.nodes()));

        let n1 = project_dag_nodes(&dag);
        let n2 = project_dag_nodes(&dag);
        assert_eq!(format!("{n1:?}"), format!("{n2:?}"));

        // And the backup pipeline over the projection is reproducible.
        let mut b1 = project_dag_to_tree(&dag);
        let mut b2 = project_dag_to_tree(&dag);
        backup_q(&mut b1);
        backup_q(&mut b2);
        assert_eq!(q_targets(&b1), q_targets(&b2));
    }

    #[test]
    fn cyclic_dag_terminates_via_on_path_guard() {
        // A back-edge C -> A (a transposition onto an ancestor) must not loop.
        let mut v = DagView::new();
        // Reserve indices: A=0, B=1, C=2.
        v.nodes.push(DagViewNode::open("A")); // 0
        v.nodes.push(DagViewNode::open("B")); // 1
        v.nodes.push(DagViewNode::open("C")); // 2
        v.nodes[0].edges.push(DagViewEdge {
            tactic: "ab".into(),
            prior: 1.0,
            child: 1,
        });
        v.nodes[1].edges.push(DagViewEdge {
            tactic: "bc".into(),
            prior: 1.0,
            child: 2,
        });
        v.nodes[2].edges.push(DagViewEdge {
            tactic: "ca".into(),
            prior: 1.0,
            child: 0,
        }); // cycle
        v.root = 0;

        let tree = project_dag_to_tree(&v);
        // A -> B -> C, then C's back-edge to A is skipped (A is on the path).
        assert_eq!(
            tree.nodes().len(),
            3,
            "the cycle is broken, not unrolled forever"
        );
    }

    #[test]
    fn entry_point_projects_chain_and_persists_summary() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "a linear proof").unwrap();

        let summary = project_search_dag(&store, &project.id, &chain_view()).unwrap();
        assert_eq!(summary["dag_nodes"], 4);
        assert_eq!(summary["tree_nodes"], 4, "a chain has no transposition");
        assert_eq!(summary["terminal_leaves"], 1);
        assert_eq!(summary["unrolled_duplicates"], 0);
        assert!((summary["root_q"].as_f64().unwrap() - 1.0).abs() < 1e-12);
        assert!(
            !summary["q_targets"].as_array().unwrap().is_empty(),
            "the backed-up chain yields step targets"
        );

        // The summary is readable back off the event log with no other reader.
        let events = store.events(&project.id, 50).unwrap();
        assert!(events
            .iter()
            .any(|e| e.event_type == "search.dag_projection"));
    }

    #[test]
    fn entry_point_reports_unrolled_transpositions() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "a diamond").unwrap();

        let summary = project_search_dag(&store, &project.id, &diamond_view()).unwrap();
        // 4 DAG nodes unroll to 5 tree nodes because D is shared by two paths.
        assert_eq!(summary["dag_nodes"], 4);
        assert_eq!(summary["tree_nodes"], 5);
        assert_eq!(summary["unrolled_duplicates"], 1);
        assert_eq!(summary["terminal_leaves"], 2);
    }

    #[test]
    fn entry_point_on_empty_view_reports_nothing_rather_than_failing() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "no search ran").unwrap();

        let summary = project_search_dag(&store, &project.id, &DagView::new()).unwrap();
        assert_eq!(summary["dag_nodes"], 0);
        assert_eq!(summary["tree_nodes"], 0);
        assert_eq!(summary["terminal_leaves"], 0);
        assert_eq!(summary["q_targets"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn view_round_trips_through_json_for_cli_input() {
        // The CLI hands a finished DAG in as JSON, so the view must survive a
        // serialize/deserialize round trip with the projection unchanged.
        let dag = chain_view();
        let text = serde_json::to_string(&dag).unwrap();
        let back: DagView = serde_json::from_str(&text).unwrap();
        assert_eq!(
            format!("{:?}", project_dag_nodes(&dag)),
            format!("{:?}", project_dag_nodes(&back))
        );
    }

    #[test]
    fn empty_or_out_of_range_view_yields_empty_tree() {
        assert_eq!(project_dag_to_tree(&DagView::new()).nodes().len(), 0);
        let mut bad = DagView::new();
        bad.nodes.push(DagViewNode::open("x"));
        bad.root = 9; // out of range
        assert_eq!(project_dag_to_tree(&bad).nodes().len(), 0);
    }
}
