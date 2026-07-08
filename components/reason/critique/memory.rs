//! Unified episodic-memory facade over the four fragmented memory/search seams.
//!
//! An architecture review flagged that our cross-attempt state lives in FOUR
//! overlapping-but-disconnected mechanisms, each with its own store-backed or
//! pure API:
//!
//! * [`plan_history`](crate::plan_history) — episodic *plan-attempt* memory
//!   (the "Do NOT try again" strategy log), per project;
//! * [`taint`](crate::taint) — three-valued *taint provenance* over the
//!   dependency graph (is a node poisoned / a self-admitted gap / clean);
//! * [`proof_pool`](crate::proof_pool) — the scored *proof-candidate pool* with
//!   refinement-lineage provenance, per project; and
//! * a **lemma reuse cache**, which currently has *no Rust seam at all* — it is
//!   implemented Python-side (`components/verify/python/.../lemma_cache.py`).
//!
//! This module introduces ONE coherent read/write facade, [`EpisodicMemory`],
//! that composes those seams behind a single, documented API. It **delegates**
//! to each module's existing public items — it does not re-implement, widen, or
//! change any of them. The point is legibility: a caller that wants "everything
//! we remember about node X in project P" calls [`EpisodicMemory::snapshot`]
//! once instead of stitching three accessors and one missing one together.
//!
//! Determinism: every method is a pure function of the store's contents plus its
//! explicit arguments — no wall-clock reads, no RNG, no ambient thread state.
//! Ranking is delegated to [`ProofPool::rank_and_refine`], whose tie-break is
//! deterministic by id.
//!
//! Scoping note (inherited from the delegated modules, not a choice made here):
//! plan-history and the proof-pool are persisted **per project** (their events
//! carry no node id), whereas taint is a **per-node** verdict computed over the
//! project graph. [`snapshot`](EpisodicMemory::snapshot) therefore takes both a
//! `project_id` and a `node_id`: the taint verdict is node-specific; the
//! attempt log and proof pool are the project-wide views.

use crate::db::Store;
use crate::model::Taint;
use crate::plan_history::{PlanHistory, PlanHistoryEntry};
use crate::proof_pool::{ProofCandidate, ProofPool, ProofPoolStore};
use anyhow::Result;

/// State of the lemma-reuse seam as seen from Rust.
///
/// The lemma cache is implemented Python-side (`lemma_cache.py`) and exposes no
/// Rust accessor today, so the facade cannot read or rank it. This enum is the
/// honest, explicit boundary marker returned in a [`MemorySnapshot`] so the seam
/// is *visible* (and not silently forgotten) even though it is not yet unified.
/// When a Rust seam lands, this becomes the place to carry the reused-lemma hits
/// without changing the facade's shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LemmaReuse {
    /// No Rust access path exists; lemma reuse is resolved in the Python worker.
    PythonSide,
}

impl LemmaReuse {
    /// Whether a Rust-side lemma-reuse seam is available (always `false` today).
    pub fn is_available(self) -> bool {
        matches!(self, LemmaReuse::PythonSide) && false
    }
}

/// A single unified view of everything remembered about one node in one project,
/// composed from all four seams. Produced by [`EpisodicMemory::snapshot`].
#[derive(Debug, Clone)]
pub struct MemorySnapshot {
    /// The project this snapshot is for.
    pub project_id: String,
    /// The node whose taint verdict is reported.
    pub node_id: String,
    /// The project's plan-attempt history in append order (the "Do NOT try
    /// again" strategy memory). Project-scoped — see the module note.
    pub attempts: Vec<PlanHistoryEntry>,
    /// The taint verdict for `node_id`, or `None` if the node is not in the
    /// project graph. `Clean` / `Tainted` / `SelfAdmitted`.
    pub taint: Option<Taint>,
    /// The project's proof pool, ranked best-first (score, then self-eval, then
    /// id). Owned clones so the snapshot outlives the transient pool.
    pub ranked_proofs: Vec<ProofCandidate>,
    /// The all-pass candidate, if the pool already cleared the stop gate.
    pub all_pass: Option<ProofCandidate>,
    /// The lemma-reuse seam's state (Python-side today).
    pub lemma_reuse: LemmaReuse,
}

/// The unifying episodic-memory facade. Holds only a borrowed [`Store`]; every
/// method constructs the relevant per-seam accessor on demand and delegates.
pub struct EpisodicMemory<'a> {
    store: &'a Store,
}

impl<'a> EpisodicMemory<'a> {
    /// Wrap a store. Cheap; borrows only.
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    // --- (a) plan-attempt history -----------------------------------------

    /// Record the outcome of a plan attempt (delegates to
    /// [`PlanHistory::add`]). Append-only.
    pub fn record_attempt(&self, project_id: &str, entry: &PlanHistoryEntry) -> Result<()> {
        PlanHistory::new(self.store).add(project_id, entry)
    }

    /// Recall every prior plan attempt for a project in append order (delegates
    /// to [`PlanHistory::read`]). The log is project-scoped, so this is "all
    /// attempts on this theorem" — the substrate a decomposer/retry reads to
    /// avoid a dead strategy.
    pub fn recall_attempts(&self, project_id: &str) -> Result<Vec<PlanHistoryEntry>> {
        PlanHistory::new(self.store).read(project_id)
    }

    /// The compact, prompt-injectable rendering of the strategy memory
    /// (delegates to [`PlanHistory::render`]); `None` when empty.
    pub fn render_attempts(&self, project_id: &str) -> Result<Option<String>> {
        PlanHistory::new(self.store).render(project_id)
    }

    // --- (b) taint provenance for a node ----------------------------------

    /// Fetch a node's three-valued taint verdict, freshly propagated over the
    /// project graph (delegates to [`taint::propagate`](crate::taint::propagate)
    /// on the store's nodes/edges). `None` when the node is not in the project.
    pub fn taint_verdict(&self, project_id: &str, node_id: &str) -> Result<Option<Taint>> {
        let nodes = self.store.nodes(project_id)?;
        let edges = self.store.edges(project_id)?;
        let verdicts = crate::taint::propagate(&nodes, &edges);
        Ok(verdicts.get(node_id).copied())
    }

    // --- (c) scored proof pool for a project ------------------------------

    /// Load the project's full persisted proof pool in append order (delegates
    /// to [`ProofPoolStore::load`]).
    pub fn proof_pool(&self, project_id: &str) -> Result<ProofPool> {
        ProofPoolStore::new(self.store).load(project_id)
    }

    /// Fetch the proof pool ranked best-first and truncated to `n_best`
    /// (delegates to [`ProofPool::rank_and_refine`]). Returned as owned clones
    /// so the ranking outlives the transient pool.
    pub fn ranked_proofs(&self, project_id: &str, n_best: usize) -> Result<Vec<ProofCandidate>> {
        let pool = self.proof_pool(project_id)?;
        Ok(pool
            .rank_and_refine(n_best)
            .into_iter()
            .cloned()
            .collect())
    }

    /// Record a proof candidate and report whether the pool now clears the
    /// all-pass stop gate (delegates to [`ProofPoolStore::record`]).
    pub fn record_proof_candidate(
        &self,
        project_id: &str,
        candidate: &ProofCandidate,
    ) -> Result<bool> {
        ProofPoolStore::new(self.store).record(project_id, candidate)
    }

    // --- (d) lemma reuse (Python-side stub boundary) ----------------------

    /// The lemma-reuse seam's state. There is no Rust access path today (the
    /// cache lives in the Python worker), so this is a documented stub boundary
    /// that keeps the seam visible in the unified view. See [`LemmaReuse`].
    pub fn lemma_reuse(&self, _project_id: &str) -> LemmaReuse {
        LemmaReuse::PythonSide
    }

    // --- unified composition ----------------------------------------------

    /// The single unified view: composes all four seams for one node in one
    /// project into a [`MemorySnapshot`]. This is the coherence win — one call
    /// instead of four accessors.
    ///
    /// `n_best` caps how many ranked proof candidates the snapshot carries.
    pub fn snapshot(
        &self,
        project_id: &str,
        node_id: &str,
        n_best: usize,
    ) -> Result<MemorySnapshot> {
        let attempts = self.recall_attempts(project_id)?;
        let taint = self.taint_verdict(project_id, node_id)?;
        let pool = self.proof_pool(project_id)?;
        let all_pass = pool.all_pass().cloned();
        let ranked_proofs = pool
            .rank_and_refine(n_best)
            .into_iter()
            .cloned()
            .collect();
        Ok(MemorySnapshot {
            project_id: project_id.to_owned(),
            node_id: node_id.to_owned(),
            attempts,
            taint,
            ranked_proofs,
            all_pass,
            lemma_reuse: self.lemma_reuse(project_id),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EdgeKind, NodeKind, NodeStatus};
    use std::path::Path;

    /// Build an in-memory store with a project, mirroring the sibling modules'
    /// test setup. No wall-clock/RNG beyond what the store itself does.
    fn store_with_project() -> (Store, String) {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let p = store.create_project("p", "t").unwrap();
        (store, p.id)
    }

    #[test]
    fn a_recorded_attempt_is_recalled() {
        let (store, pid) = store_with_project();
        let mem = EpisodicMemory::new(&store);

        let entry = PlanHistoryEntry::failed(1, "induct on n", "step case stuck");
        mem.record_attempt(&pid, &entry).unwrap();

        let recalled = mem.recall_attempts(&pid).unwrap();
        assert_eq!(recalled.len(), 1);
        assert_eq!(recalled[0], entry);

        // The rendering surfaces the anti-loop list too.
        let rendered = mem.render_attempts(&pid).unwrap().unwrap();
        assert!(rendered.contains("Do NOT try again"));
        assert!(rendered.contains("induct on n"));
    }

    #[test]
    fn a_tainted_nodes_verdict_surfaces() {
        let (store, pid) = store_with_project();
        let mem = EpisodicMemory::new(&store);

        // B depends on A; reject A so B is transitively tainted.
        let a = store
            .add_node(&pid, NodeKind::Lemma, "A", "A", "test")
            .unwrap();
        let b = store
            .add_node(&pid, NodeKind::Lemma, "B", "B", "test")
            .unwrap();
        store
            .add_edge(&pid, &b.id, &a.id, EdgeKind::DependsOn)
            .unwrap();
        store
            .set_node_status(&pid, &a.id, NodeStatus::Rejected, "test")
            .unwrap();

        assert_eq!(
            mem.taint_verdict(&pid, &a.id).unwrap(),
            Some(Taint::Tainted)
        );
        assert_eq!(
            mem.taint_verdict(&pid, &b.id).unwrap(),
            Some(Taint::Tainted)
        );
        // An unknown node yields no verdict.
        assert_eq!(mem.taint_verdict(&pid, "nope").unwrap(), None);
    }

    #[test]
    fn a_populated_proof_pool_is_returned_ranked() {
        let (store, pid) = store_with_project();
        let mem = EpisodicMemory::new(&store);

        mem.record_proof_candidate(&pid, &ProofCandidate::new("a", 0.5, 0.9))
            .unwrap();
        mem.record_proof_candidate(&pid, &ProofCandidate::new("b", 0.9, 0.1))
            .unwrap();
        mem.record_proof_candidate(&pid, &ProofCandidate::new("c", 0.9, 0.8))
            .unwrap();

        let ranked = mem.ranked_proofs(&pid, 2).unwrap();
        assert_eq!(ranked.len(), 2);
        // 0.9/0.8 (c) beats 0.9/0.1 (b), both beat 0.5.
        assert_eq!(ranked[0].id, "c");
        assert_eq!(ranked[1].id, "b");

        // Recording a perfect candidate flips the all-pass stop gate.
        let stop = mem
            .record_proof_candidate(&pid, &ProofCandidate::new("win", 1.0, 1.0))
            .unwrap();
        assert!(stop);
    }

    #[test]
    fn snapshot_composes_all_seams() {
        let (store, pid) = store_with_project();
        let mem = EpisodicMemory::new(&store);

        // (a) an attempt
        mem.record_attempt(&pid, &PlanHistoryEntry::failed(1, "s", "d"))
            .unwrap();
        // (b) a tainted node
        let n = store
            .add_node(&pid, NodeKind::Lemma, "N", "N", "test")
            .unwrap();
        store
            .set_node_status(&pid, &n.id, NodeStatus::Rejected, "test")
            .unwrap();
        // (c) a proof pool with an all-pass winner
        mem.record_proof_candidate(&pid, &ProofCandidate::new("a", 0.4, 0.3))
            .unwrap();
        mem.record_proof_candidate(&pid, &ProofCandidate::new("win", 1.0, 1.0))
            .unwrap();

        let snap = mem.snapshot(&pid, &n.id, 5).unwrap();

        // (a) attempt recalled
        assert_eq!(snap.attempts.len(), 1);
        assert_eq!(snap.attempts[0].strategy, "s");
        // (b) taint verdict surfaced
        assert_eq!(snap.taint, Some(Taint::Tainted));
        // (c) proof pool ranked, all-pass detected
        assert_eq!(snap.ranked_proofs.len(), 2);
        assert_eq!(snap.ranked_proofs[0].id, "win");
        assert_eq!(snap.all_pass.as_ref().map(|c| c.id.as_str()), Some("win"));
        // (d) lemma reuse boundary is explicit
        assert_eq!(snap.lemma_reuse, LemmaReuse::PythonSide);
        assert!(!snap.lemma_reuse.is_available());
        // identity carried through
        assert_eq!(snap.project_id, pid);
        assert_eq!(snap.node_id, n.id);
    }
}
