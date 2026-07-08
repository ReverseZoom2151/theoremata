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
    /// Aletheia abstention: `true` when the verifier DECLINED to rule on this
    /// node because its confidence was below the abstention threshold. An
    /// abstention is a first-class terminal state DISTINCT from a failure — the
    /// node is neither certified nor counted as wrong (see
    /// [`conditional_accuracy`]). Only ever set by
    /// [`PoolMetaGate::evaluate_with_abstention`]; the default [`evaluate`] path
    /// never abstains, so `certified` behaviour is unchanged.
    pub abstained: bool,
    /// Human-readable reason for an abstention (`None` unless `abstained`).
    pub abstain_reason: Option<String>,
}

impl GateOutcome {
    /// Collapse the sub-verdicts into a single first-class terminal state.
    /// Certification wins over abstention (a proved node is never "declined");
    /// an abstention is distinct from a plain failure.
    pub fn terminal_state(&self) -> CertifyOutcome {
        if self.certified {
            CertifyOutcome::Proved
        } else if self.abstained {
            CertifyOutcome::Abstained
        } else {
            CertifyOutcome::Failed
        }
    }
}

/// The first-class terminal state of a certification decision (Aletheia): a node
/// is either PROVED, FAILED, or — when confidence is too low to rule either way —
/// ABSTAINED. Abstaining lets the verifier DECLINE rather than bluff, and is
/// excluded from [`conditional_accuracy`] so it is not scored as a wrong answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertifyOutcome {
    Proved,
    Failed,
    Abstained,
}

/// Strip chain-of-thought / scratch reasoning from a candidate so the critic and
/// the pool gate see ONLY the final proof term (Aletheia: the critic must judge
/// the proof, never the model's private reasoning). Removes paired reasoning
/// blocks — `<think>…</think>`, `<reasoning>…</reasoning>`, and a Lean
/// line-comment CoT block delimited by `-- BEGIN COT` / `-- END COT` — then trims
/// surrounding blank lines. A proof with no such block is returned unchanged
/// (aside from trimming), so existing callers are unaffected.
pub fn strip_reasoning(proof: &str) -> String {
    let mut s = proof.to_string();
    for (open, close) in [("<think>", "</think>"), ("<reasoning>", "</reasoning>")] {
        s = strip_paired_blocks(&s, open, close);
    }
    s = strip_line_marked_block(&s, "-- BEGIN COT", "-- END COT");
    s.trim().to_string()
}

/// Remove every `open`…`close` block (case-insensitive on the delimiters,
/// spanning newlines). Unbalanced/absent delimiters leave the text untouched.
fn strip_paired_blocks(text: &str, open: &str, close: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let open_l = open.to_ascii_lowercase();
    let close_l = close.to_ascii_lowercase();
    let mut out = String::new();
    let mut cursor = 0usize;
    while let Some(rel) = lower[cursor..].find(&open_l) {
        let start = cursor + rel;
        let after_open = start + open.len();
        let Some(rel_end) = lower[after_open..].find(&close_l) else {
            break; // no matching close: keep the remainder verbatim
        };
        let end = after_open + rel_end + close.len();
        out.push_str(&text[cursor..start]);
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    out
}

/// Remove a block whose start line contains `open_marker` and end line contains
/// `close_marker` (whole lines dropped, delimiters included).
fn strip_line_marked_block(text: &str, open_marker: &str, close_marker: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut skipping = false;
    for line in text.lines() {
        if !skipping && line.contains(open_marker) {
            skipping = true;
            continue;
        }
        if skipping {
            if line.contains(close_marker) {
                skipping = false;
            }
            continue;
        }
        out.push(line);
    }
    out.join("\n")
}

/// Conditional accuracy (Aletheia): `proved / (proved + failed)`, EXCLUDING
/// abstentions from the denominator so that declining to answer is never scored
/// as a wrong answer. Returns `0.0` when nothing was decided (no proved and no
/// failed).
pub fn conditional_accuracy(proved: usize, failed: usize) -> f64 {
    let decided = proved + failed;
    if decided == 0 {
        0.0
    } else {
        proved as f64 / decided as f64
    }
}

/// Convenience: compute [`conditional_accuracy`] directly over a slice of
/// terminal outcomes (abstentions are dropped from the denominator).
pub fn conditional_accuracy_of(outcomes: &[CertifyOutcome]) -> f64 {
    let proved = outcomes.iter().filter(|o| **o == CertifyOutcome::Proved).count();
    let failed = outcomes.iter().filter(|o| **o == CertifyOutcome::Failed).count();
    conditional_accuracy(proved, failed)
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
                abstained: false,
                abstain_reason: None,
            });
        }

        // Feed the critic/pool ONLY the final proof term, never the model's
        // chain-of-thought/scratch (Aletheia). A proof with no CoT block is
        // unchanged, so this is behaviour-preserving for existing callers.
        let proof = &strip_reasoning(proof);

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
            abstained: false,
            abstain_reason: None,
        })
    }

    /// Aletheia abstention wrapper over [`PoolMetaGate::evaluate`]. Runs the full
    /// gate; then, when the node was NOT certified AND its confidence
    /// (`min(verifier_score, self_eval_score)`) is strictly below
    /// `abstain_threshold`, promotes the outcome from a plain failure to a
    /// first-class ABSTENTION — the verifier DECLINES rather than bluffs on a
    /// low-confidence node. A certified node is never demoted to an abstention,
    /// and a confident-but-failed node still FAILS (it is a real negative). With
    /// `abstain_threshold <= 0.0` this is exactly [`evaluate`].
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_with_abstention(
        &self,
        project_id: &str,
        node_id: &str,
        proof: &str,
        verifier_score: f64,
        self_eval_score: f64,
        k_streak_certified: bool,
        abstain_threshold: f64,
    ) -> Result<GateOutcome> {
        let mut outcome = self.evaluate(
            project_id,
            node_id,
            proof,
            verifier_score,
            self_eval_score,
            k_streak_certified,
        )?;
        let confidence = verifier_score.min(self_eval_score);
        if !outcome.certified && confidence < abstain_threshold {
            let reason = format!(
                "confidence {confidence:.3} below abstention threshold {abstain_threshold:.3}"
            );
            outcome.abstained = true;
            outcome.abstain_reason = Some(reason.clone());
            if self.enabled {
                self.store.add_evidence(
                    project_id,
                    node_id,
                    "abstention",
                    "certification_gate",
                    "abstained",
                    json!({
                        "confidence": confidence,
                        "abstain_threshold": abstain_threshold,
                        "reason": reason,
                    }),
                )?;
            }
        }
        Ok(outcome)
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
    fn low_confidence_node_abstains_instead_of_failing() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let (project_id, node_id) = project_with_node(&store);
        let gate = PoolMetaGate {
            store: &store,
            provider: &CleanCritic,
            enabled: true,
        };
        // Low scores + broken streak: the gate does not certify. With an
        // abstention threshold above the confidence, this is an ABSTENTION, not
        // a failure.
        let outcome = gate
            .evaluate_with_abstention(&project_id, &node_id, "partial", 0.2, 0.2, false, 0.5)
            .unwrap();
        assert!(!outcome.certified);
        assert!(outcome.abstained, "low confidence ⇒ abstain, not fail");
        assert!(outcome.abstain_reason.is_some());
        assert_eq!(outcome.terminal_state(), CertifyOutcome::Abstained);
    }

    #[test]
    fn confident_but_uncertified_node_still_fails_not_abstains() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let (project_id, node_id) = project_with_node(&store);
        let gate = PoolMetaGate {
            store: &store,
            provider: &RejectingCritic {
                node_id: node_id.clone(),
            },
            enabled: true,
        };
        // High confidence but the critic vetoes: a real negative, so it FAILS —
        // abstention only covers the low-confidence case.
        let outcome = gate
            .evaluate_with_abstention(&project_id, &node_id, "theorem t : True := trivial", 1.0, 1.0, true, 0.5)
            .unwrap();
        assert!(!outcome.certified);
        assert!(!outcome.abstained, "confident failure is a real negative");
        assert_eq!(outcome.terminal_state(), CertifyOutcome::Failed);
    }

    #[test]
    fn certified_node_is_never_demoted_to_abstention() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let (project_id, node_id) = project_with_node(&store);
        let gate = PoolMetaGate {
            store: &store,
            provider: &CleanCritic,
            enabled: true,
        };
        let outcome = gate
            .evaluate_with_abstention(&project_id, &node_id, "theorem t : True := trivial", 1.0, 1.0, true, 0.5)
            .unwrap();
        assert!(outcome.certified);
        assert!(!outcome.abstained);
        assert_eq!(outcome.terminal_state(), CertifyOutcome::Proved);
    }

    #[test]
    fn conditional_accuracy_excludes_abstentions() {
        use super::{conditional_accuracy, conditional_accuracy_of};
        // 3 proved, 1 failed, 6 abstained: accuracy is 3/(3+1), NOT 3/10.
        assert!((conditional_accuracy(3, 1) - 0.75).abs() < 1e-9);
        let outcomes = [
            CertifyOutcome::Proved,
            CertifyOutcome::Proved,
            CertifyOutcome::Proved,
            CertifyOutcome::Failed,
            CertifyOutcome::Abstained,
            CertifyOutcome::Abstained,
        ];
        assert!((conditional_accuracy_of(&outcomes) - 0.75).abs() < 1e-9);
        // All abstained ⇒ nothing decided ⇒ 0.0 (not NaN).
        assert_eq!(conditional_accuracy(0, 0), 0.0);
    }

    #[test]
    fn strip_reasoning_removes_cot_but_preserves_the_proof_term() {
        use super::strip_reasoning;
        let with_cot = "<think>\nfirst I try induction, then simp...\n</think>\ntheorem t : True := trivial";
        let stripped = strip_reasoning(with_cot);
        assert_eq!(stripped, "theorem t : True := trivial");
        assert!(!stripped.contains("induction"));

        // Lean line-comment CoT block is also removed.
        let lean_cot = "-- BEGIN COT\n-- scratch: unfold, then ring\n-- END COT\ntheorem u : 1 = 1 := rfl";
        assert_eq!(strip_reasoning(lean_cot), "theorem u : 1 = 1 := rfl");

        // A proof with no reasoning block is unchanged (aside from trimming).
        assert_eq!(
            strip_reasoning("theorem v : True := trivial"),
            "theorem v : True := trivial"
        );
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
