//! Scored proof-pool with provenance (DeepSeek-Math-V2 `inference/main.py`).
//!
//! A persisted pool of candidate proofs, each carrying a verifier score, its
//! self-eval score, a `verdict`, and — the load-bearing part — its
//! `dep_proof_ids` lineage: which earlier candidates it was refined from. This
//! is the "hand-rolled MCTS-lite over a scored candidate pool with provenance
//! edges" the DeepSeek paper's high-compute loop uses, ported as an optional
//! enrichment our agent loop / MCTS can track candidate lineage through.
//!
//! Two layers, mirroring `plan_history.rs`:
//! * a pure [`ProofPool`] (rank/refine/stop logic) that is testable without a
//!   store; and
//! * a store-backed [`ProofPoolStore`] that persists each candidate as one
//!   `proof_pool.candidate` event, so the pool is durable, ordered, and
//!   replayable alongside every other event (no schema migration).
//!
//! The stop gate is DeepSeek's all-pass rule: a candidate whose mean verifier
//! score exceeds [`ALL_PASS_THRESHOLD`] (`> 0.99999`, i.e. it passed every
//! verification) terminates the search.

use crate::db::Store;
use anyhow::Result;
use serde::{Deserialize, Serialize};

/// The event type under which pool candidates are persisted.
const EVENT_TYPE: &str = "proof_pool.candidate";

/// DeepSeek's all-pass stop gate: a proof that passes *every* verification has a
/// mean score of 1.0; we accept the same `> 0.99999` slack the reference uses
/// (`main.py:222`) so floating-point averaging noise does not miss the gate.
pub const ALL_PASS_THRESHOLD: f64 = 0.99999;

/// Verifier verdict on a pool candidate, aligned with our three-valued taint and
/// DeepSeek's `{0, 0.5, 1}` rubric: `Passing` ≈ 1, `Suspect` ≈ 0.5, `Failing` ≈ 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PoolVerdict {
    /// Not yet verified.
    Pending,
    /// The verifier(s) confirmed the proof (mean score at the all-pass gate).
    Passing,
    /// Minor gaps — a graded, partial-credit candidate worth refining.
    Suspect,
    /// A fatal defect was found.
    Failing,
}

impl PoolVerdict {
    /// Classify a mean verifier score into the three-valued verdict, matching the
    /// DeepSeek `{0, 0.5, 1}` bands (all-pass → `Passing`, ≥ 0.5 → `Suspect`).
    pub fn from_score(score: f64) -> PoolVerdict {
        if score > ALL_PASS_THRESHOLD {
            PoolVerdict::Passing
        } else if score >= 0.5 {
            PoolVerdict::Suspect
        } else {
            PoolVerdict::Failing
        }
    }
}

/// One scored candidate proof in the pool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProofCandidate {
    /// Stable id (the provenance handle other candidates cite in `dep_proof_ids`).
    pub id: String,
    /// Mean verifier score in `[0, 1]` (DeepSeek `meanscore`).
    pub score: f64,
    /// The generator's own self-assessment in `[0, 1]` (DeepSeek `self_eval_score`),
    /// used as the rank tie-break.
    #[serde(default)]
    pub self_eval_score: f64,
    /// Provenance lineage: the ids of the candidates this one was refined from.
    #[serde(default)]
    pub dep_proof_ids: Vec<String>,
    /// The verifier verdict.
    pub verdict: PoolVerdict,
    /// Which refinement round produced this candidate (0-based).
    #[serde(default)]
    pub round_idx: u32,
    /// The candidate proof text / blueprint (opaque to the pool).
    #[serde(default)]
    pub proof: String,
}

impl ProofCandidate {
    /// A freshly-generated candidate (no provenance), verdict derived from score.
    pub fn new(id: impl Into<String>, score: f64, self_eval_score: f64) -> Self {
        Self {
            id: id.into(),
            score,
            self_eval_score,
            dep_proof_ids: Vec::new(),
            verdict: PoolVerdict::from_score(score),
            round_idx: 0,
            proof: String::new(),
        }
    }

    /// A candidate refined from `deps` (records the provenance lineage).
    pub fn refined_from(
        id: impl Into<String>,
        score: f64,
        self_eval_score: f64,
        round_idx: u32,
        deps: Vec<String>,
    ) -> Self {
        Self {
            id: id.into(),
            score,
            self_eval_score,
            dep_proof_ids: deps,
            verdict: PoolVerdict::from_score(score),
            round_idx,
            proof: String::new(),
        }
    }
}

/// A pure, in-memory pool of scored candidates — all selection logic lives here
/// so it is unit-testable without a store.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProofPool {
    pub candidates: Vec<ProofCandidate>,
}

impl ProofPool {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_candidates(candidates: Vec<ProofCandidate>) -> Self {
        Self { candidates }
    }

    pub fn push(&mut self, c: ProofCandidate) {
        self.candidates.push(c);
    }

    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    pub fn len(&self) -> usize {
        self.candidates.len()
    }

    /// The all-pass stop gate: `Some(candidate)` when any candidate cleared
    /// [`ALL_PASS_THRESHOLD`] (passed every verification), else `None`. This is
    /// the DeepSeek `meanscore > 0.99999` termination signal.
    pub fn all_pass(&self) -> Option<&ProofCandidate> {
        self.candidates
            .iter()
            .find(|c| c.score > ALL_PASS_THRESHOLD)
    }

    /// Whether the search should stop (an all-pass candidate exists).
    pub fn should_stop(&self) -> bool {
        self.all_pass().is_some()
    }

    /// Rank the pool by `(score, self_eval_score)` descending and return the top
    /// `n_best` candidates — the DeepSeek `rank pool by (meanscore, self_eval)` /
    /// `n_best_proofs_to_sample` selection that seeds the next refinement round.
    /// Ties are broken deterministically by id so selection is reproducible.
    pub fn rank_and_refine(&self, n_best: usize) -> Vec<&ProofCandidate> {
        let mut ranked: Vec<&ProofCandidate> = self.candidates.iter().collect();
        ranked.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(
                    b.self_eval_score
                        .partial_cmp(&a.self_eval_score)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
                .then(a.id.cmp(&b.id))
        });
        ranked.truncate(n_best);
        ranked
    }

    /// The transitive provenance closure of `id`: every ancestor candidate it was
    /// (directly or indirectly) refined from. Cycle-guarded. Empty when `id` is a
    /// freshly-generated root or is unknown.
    pub fn provenance(&self, id: &str) -> Vec<String> {
        use std::collections::{HashMap, HashSet};
        let by_id: HashMap<&str, &ProofCandidate> =
            self.candidates.iter().map(|c| (c.id.as_str(), c)).collect();
        let mut seen: HashSet<String> = HashSet::new();
        let mut stack: Vec<String> = by_id
            .get(id)
            .map(|c| c.dep_proof_ids.clone())
            .unwrap_or_default();
        while let Some(next) = stack.pop() {
            if !seen.insert(next.clone()) {
                continue;
            }
            if let Some(c) = by_id.get(next.as_str()) {
                stack.extend(c.dep_proof_ids.iter().cloned());
            }
        }
        let mut out: Vec<String> = seen.into_iter().collect();
        out.sort();
        out
    }
}

/// Store-backed accessor for a project's proof pool (persists each candidate as
/// one append-only `proof_pool.candidate` event).
pub struct ProofPoolStore<'a> {
    pub store: &'a Store,
}

impl<'a> ProofPoolStore<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// Append a candidate to the persisted pool (append-only).
    pub fn add(&self, project_id: &str, candidate: &ProofCandidate) -> Result<()> {
        self.store.event(
            Some(project_id),
            None,
            EVENT_TYPE,
            "proof_pool",
            serde_json::to_value(candidate)?,
        )
    }

    /// Load the full persisted pool in append order.
    pub fn load(&self, project_id: &str) -> Result<ProofPool> {
        let mut candidates: Vec<ProofCandidate> = self
            .store
            .events(project_id, 100_000)?
            .into_iter()
            .filter(|e| e.event_type == EVENT_TYPE)
            .filter_map(|e| serde_json::from_value(e.payload).ok())
            .collect();
        candidates.reverse(); // events come back newest-first
        Ok(ProofPool::from_candidates(candidates))
    }

    /// Convenience: append then report whether the pool now clears the all-pass
    /// gate (so a caller loop can `if store.record(..)? { break }`).
    pub fn record(&self, project_id: &str, candidate: &ProofCandidate) -> Result<bool> {
        self.add(project_id, candidate)?;
        Ok(self.load(project_id)?.should_stop())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn verdict_bands_match_the_deepseek_rubric() {
        assert_eq!(PoolVerdict::from_score(1.0), PoolVerdict::Passing);
        assert_eq!(PoolVerdict::from_score(0.75), PoolVerdict::Suspect);
        assert_eq!(PoolVerdict::from_score(0.5), PoolVerdict::Suspect);
        assert_eq!(PoolVerdict::from_score(0.25), PoolVerdict::Failing);
    }

    #[test]
    fn rank_and_refine_orders_by_score_then_self_eval() {
        let pool = ProofPool::from_candidates(vec![
            ProofCandidate::new("a", 0.5, 0.9),
            ProofCandidate::new("b", 0.9, 0.1),
            ProofCandidate::new("c", 0.9, 0.8),
            ProofCandidate::new("d", 0.2, 1.0),
        ]);
        let top = pool.rank_and_refine(2);
        // 0.9/0.8 (c) beats 0.9/0.1 (b); both beat the lower-scored ones.
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].id, "c");
        assert_eq!(top[1].id, "b");
    }

    #[test]
    fn all_pass_gate_triggers_only_above_threshold() {
        let mut pool = ProofPool::from_candidates(vec![
            ProofCandidate::new("a", 0.99, 0.9),
            ProofCandidate::new("b", 0.999, 0.9),
        ]);
        assert!(!pool.should_stop());
        assert!(pool.all_pass().is_none());
        pool.push(ProofCandidate::new("win", 1.0, 1.0));
        assert!(pool.should_stop());
        assert_eq!(pool.all_pass().unwrap().id, "win");
    }

    #[test]
    fn provenance_walks_the_refinement_lineage() {
        // c refined from a+b; d refined from c. d's closure is {a,b,c}.
        let pool = ProofPool::from_candidates(vec![
            ProofCandidate::new("a", 0.5, 0.5),
            ProofCandidate::new("b", 0.6, 0.5),
            ProofCandidate::refined_from("c", 0.8, 0.5, 1, vec!["a".into(), "b".into()]),
            ProofCandidate::refined_from("d", 0.95, 0.5, 2, vec!["c".into()]),
        ]);
        let prov = pool.provenance("d");
        assert_eq!(prov, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
        // A fresh root has no provenance.
        assert!(pool.provenance("a").is_empty());
    }

    #[test]
    fn store_round_trips_the_pool_and_reports_the_stop_gate() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let p = store.create_project("p", "t").unwrap();
        let pool = ProofPoolStore::new(&store);

        pool.add(&p.id, &ProofCandidate::new("a", 0.4, 0.3)).unwrap();
        let stop = pool
            .record(
                &p.id,
                &ProofCandidate::refined_from("b", 0.7, 0.6, 1, vec!["a".into()]),
            )
            .unwrap();
        assert!(!stop, "0.7 does not clear the all-pass gate");

        let loaded = pool.load(&p.id).unwrap();
        assert_eq!(loaded.len(), 2);
        // Append order preserved and provenance survives the round-trip.
        assert_eq!(loaded.candidates[0].id, "a");
        assert_eq!(loaded.candidates[1].dep_proof_ids, vec!["a".to_string()]);

        // A perfect candidate flips the stop gate.
        let stop = pool
            .record(&p.id, &ProofCandidate::new("win", 1.0, 1.0))
            .unwrap();
        assert!(stop);
    }
}
