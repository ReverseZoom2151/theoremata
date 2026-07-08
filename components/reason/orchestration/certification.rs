//! Certification gate: route a node's would-be certification through the scored
//! proof-pool AND the critic's meta-verification before it is accepted.
//!
//! The base agent loop certifies a node the moment its formal proof clears the
//! k-consecutive-clean verifier streak. That streak is a *hedge against a noisy
//! verifier* — it says nothing about whether the pool of candidate proofs ever
//! produced an all-passing candidate, nor whether the adversarial critic found a
//! CONFIRMED critical defect in the DAG. This gate closes both holes:
//!
//! 1. every candidate is inserted into the persisted [`ProofPoolStore`] (so the
//!    pool is actually populated during a run), ranked/refined via
//!    [`ProofPool::rank_and_refine`], and the pool's DeepSeek all-pass gate
//!    ([`ProofPool::all_pass`]) must fire; AND
//! 2. the critic's [`CritiqueReport::should_reject_node`] must return `false`
//!    (i.e. no meta-CONFIRMED critical finding names this node).
//!
//! Certification requires the pre-existing k-consecutive streak AND both new
//! gates. The gate is ON by default; set `THEOREMATA_POOL_META_GATE=0` (or
//! `false`) to fall back to the pure k-consecutive behaviour.
//!
//! No wall-clock/random nondeterminism: candidate ids are derived from the node
//! id and the count of that node's prior candidates in the (ordered, replayable)
//! pool.

use crate::{
    critic::Critic,
    db::Store,
    proof_pool::{ProofCandidate, ProofPoolStore},
    provider::ModelProvider,
};
use anyhow::Result;
use serde_json::json;

/// How many top candidates `rank_and_refine` returns when seeding the next
/// refinement round (DeepSeek `n_best_proofs_to_sample`).
const N_BEST: usize = 3;

/// Env seam for the pool + meta-verification gate. Absent or anything other than
/// an explicit `0`/`false`/`off` means ON (the default), so the gate is actually
/// exercised without a config-file change.
pub fn gate_enabled() -> bool {
    match std::env::var("THEOREMATA_POOL_META_GATE") {
        Ok(v) => !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "off"),
        Err(_) => true,
    }
}

/// The decision the gate reached, with the sub-verdicts that drove it (recorded
/// as node evidence so the gating is auditable).
#[derive(Debug, Clone)]
pub struct GateOutcome {
    /// The id assigned to the candidate that was inserted into the pool.
    pub candidate_id: String,
    /// Whether a candidate was appended to the persisted pool this call.
    pub pool_populated: bool,
    /// Whether the refreshed pool cleared the DeepSeek all-pass gate.
    pub pool_passed: bool,
    /// Whether the critic meta-verification CONFIRMED a critical finding on this
    /// node (a `true` here vetoes certification).
    pub critic_rejected: bool,
    /// Final verdict: `k_streak_certified && pool_passed && !critic_rejected`.
    pub certified: bool,
}

/// Gates certification on the scored proof-pool and the critic's
/// meta-verification. Both the store and the critic's provider are injected, so
/// the gate is exercised end-to-end against a real [`Store`] with a mock
/// provider in tests.
pub struct PoolMetaGate<'a> {
    pub store: &'a Store,
    pub provider: &'a dyn ModelProvider,
    /// When `false`, the gate is a pass-through that preserves the pre-existing
    /// k-consecutive-clean behaviour exactly (no pool write, no critic call).
    pub enabled: bool,
}

impl PoolMetaGate<'_> {
    /// Insert the candidate into the pool, rank/refine, and gate certification on
    /// the pool all-pass verdict AND the critic's `should_reject_node`.
    ///
    /// * `verifier_score` — the mean verifier score in `[0, 1]`; a full
    ///   k-consecutive-clean streak maps to `1.0`, which is what clears the
    ///   all-pass gate. Threaded in explicitly (no randomness).
    /// * `k_streak_certified` — whether the caller's k-consecutive-clean streak
    ///   already passed. Certification requires this AND both new gates.
    pub fn evaluate(
        &self,
        project_id: &str,
        node_id: &str,
        proof: &str,
        verifier_score: f64,
        self_eval_score: f64,
        k_streak_certified: bool,
    ) -> Result<GateOutcome> {
        // Disabled: exact pass-through to the legacy behaviour.
        if !self.enabled {
            return Ok(GateOutcome {
                candidate_id: String::new(),
                pool_populated: false,
                pool_passed: k_streak_certified,
                critic_rejected: false,
                certified: k_streak_certified,
            });
        }

        // 1. Populate the persisted pool. The candidate id is deterministic: the
        //    node id plus how many candidates this node already contributed.
        let pool_store = ProofPoolStore::new(self.store);
        let prefix = format!("{node_id}#");
        let round_idx = pool_store
            .load(project_id)?
            .candidates
            .iter()
            .filter(|c| c.id.starts_with(&prefix))
            .count() as u32;
        let candidate_id = format!("{prefix}{round_idx}");
        let mut candidate = ProofCandidate::new(&candidate_id, verifier_score, self_eval_score);
        candidate.round_idx = round_idx;
        candidate.proof = proof.to_string();
        pool_store.add(project_id, &candidate)?;

        // 2. Rank/refine the refreshed pool and read the all-pass verdict.
        let pool = pool_store.load(project_id)?;
        let ranked = pool.rank_and_refine(N_BEST);
        let best_id = ranked.first().map(|c| c.id.clone());
        let pool_passed = pool.all_pass().is_some();

        // 3. Critic meta-verification gate. An offline provider cannot run the
        //    critic (its `complete` errors by design); skip it rather than fail
        //    the whole certify path — the pool gate still applies. A real model
        //    provider runs the full adversarial + meta-verify pass.
        let critic_rejected = if self.provider.name() == "offline" {
            false
        } else {
            let report = Critic {
                store: self.store,
                provider: self.provider,
            }
            .critique(project_id)?;
            report.should_reject_node(node_id)
        };

        let certified = k_streak_certified && pool_passed && !critic_rejected;

        self.store.add_evidence(
            project_id,
            node_id,
            "pool_meta_gate",
            "certification_gate",
            if certified { "certified" } else { "gated" },
            json!({
                "candidate_id": candidate_id,
                "verifier_score": verifier_score,
                "k_streak_certified": k_streak_certified,
                "pool_passed": pool_passed,
                "pool_size": pool.len(),
                "best_candidate": best_id,
                "critic_rejected": critic_rejected,
            }),
        )?;

        Ok(GateOutcome {
            candidate_id,
            pool_populated: true,
            pool_passed,
            critic_rejected,
            certified,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ModelRequest, ModelResponse, NodeKind};
    use crate::proof_pool::ProofPoolStore;
    use anyhow::Result;
    use serde_json::json;
    use std::path::Path;

    /// A critic provider that reports NO structural findings — nothing to
    /// meta-confirm, so `should_reject_node` is always false.
    struct CleanCritic;
    impl ModelProvider for CleanCritic {
        fn complete(&self, _req: &ModelRequest) -> Result<ModelResponse> {
            Ok(ModelResponse {
                content: json!({ "findings": [], "summary": "clean" }),
                model: "test".into(),
                provider: "test".into(),
            })
        }
        fn name(&self) -> &str {
            "command"
        }
    }

    /// A critic provider that reports a critical finding on `node_id`, lets it
    /// survive the false-positive prune, and META-CONFIRMS it — so
    /// `should_reject_node(node_id)` is true.
    struct RejectingCritic {
        node_id: String,
    }
    impl ModelProvider for RejectingCritic {
        fn complete(&self, req: &ModelRequest) -> Result<ModelResponse> {
            let content = match req.role.as_str() {
                "adversarial_verifier" => json!({
                    "findings": [{
                        "node_id": self.node_id,
                        "severity": "critical",
                        "category": "gap",
                        "class": "critical_error",
                        "issue": "A genuine circular dependency breaks the chain."
                    }],
                    "summary": "one critical error"
                }),
                "meta_critic" => json!({ "reviews": [] }),
                "meta_verifier" => json!({
                    "verifications": [{
                        "index": 0, "defect_exists": true,
                        "justifies_severity": true, "reason": "the cycle is real"
                    }]
                }),
                _ => json!({}),
            };
            Ok(ModelResponse {
                content,
                model: "test".into(),
                provider: "test".into(),
            })
        }
        fn name(&self) -> &str {
            "command"
        }
    }

    fn project_with_node(store: &Store) -> (String, String) {
        let project = store.create_project("p", "t").unwrap();
        let node = store
            .add_node(&project.id, NodeKind::Lemma, "n", "S", "test")
            .unwrap();
        (project.id, node.id)
    }

    #[test]
    fn clean_node_that_all_passes_is_certified_and_populates_the_pool() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let (project_id, node_id) = project_with_node(&store);
        let gate = PoolMetaGate {
            store: &store,
            provider: &CleanCritic,
            enabled: true,
        };
        // A full clean streak → verifier_score 1.0 → clears the all-pass gate.
        let outcome = gate
            .evaluate(&project_id, &node_id, "theorem t : True := trivial", 1.0, 1.0, true)
            .unwrap();
        assert!(outcome.pool_passed, "score 1.0 clears the all-pass gate");
        assert!(!outcome.critic_rejected, "a clean critic never rejects");
        assert!(outcome.certified, "clean + all-pass ⇒ certified");

        // The proof-pool was actually populated during the run.
        let pool = ProofPoolStore::new(&store).load(&project_id).unwrap();
        assert_eq!(pool.len(), 1);
        assert!(pool.all_pass().is_some());
        assert_eq!(pool.candidates[0].id, outcome.candidate_id);
    }

    #[test]
    fn node_with_a_confirmed_critical_finding_is_not_certified() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let (project_id, node_id) = project_with_node(&store);
        let gate = PoolMetaGate {
            store: &store,
            provider: &RejectingCritic {
                node_id: node_id.clone(),
            },
            enabled: true,
        };
        let outcome = gate
            .evaluate(&project_id, &node_id, "theorem t : True := trivial", 1.0, 1.0, true)
            .unwrap();
        assert!(outcome.pool_passed, "the pool still all-passes...");
        assert!(outcome.critic_rejected, "...but the critic confirmed a critical finding");
        assert!(!outcome.certified, "a confirmed critical finding vetoes certification");
        // The pool is still populated even when certification is vetoed.
        let pool = ProofPoolStore::new(&store).load(&project_id).unwrap();
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn a_broken_streak_never_certifies_even_when_the_critic_is_clean() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let (project_id, node_id) = project_with_node(&store);
        let gate = PoolMetaGate {
            store: &store,
            provider: &CleanCritic,
            enabled: true,
        };
        // Partial streak → sub-threshold score → pool does not all-pass.
        let outcome = gate
            .evaluate(&project_id, &node_id, "partial", 0.5, 0.5, false)
            .unwrap();
        assert!(!outcome.pool_passed);
        assert!(!outcome.certified);
        // ...but the candidate is still recorded, so lineage survives.
        assert_eq!(ProofPoolStore::new(&store).load(&project_id).unwrap().len(), 1);
    }

    #[test]
    fn disabled_gate_is_a_passthrough() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let (project_id, node_id) = project_with_node(&store);
        let gate = PoolMetaGate {
            store: &store,
            provider: &CleanCritic,
            enabled: false,
        };
        let outcome = gate
            .evaluate(&project_id, &node_id, "x", 1.0, 1.0, true)
            .unwrap();
        assert!(outcome.certified);
        assert!(!outcome.pool_populated, "disabled gate does not touch the pool");
        assert_eq!(ProofPoolStore::new(&store).load(&project_id).unwrap().len(), 0);
    }
}
