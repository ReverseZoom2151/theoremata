//! QED-style hierarchical retry policy.
//!
//! Three nested budgets escalate from cheapest to most expensive: revise the
//! proof against a fixed plan, revise the plan, or rewrite the decomposition
//! entirely. When a tier's budget is exhausted the policy automatically
//! escalates to the next tier rather than giving up, and only terminates when
//! the outermost budget is spent.

/// The action to take after a failed step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    ReviseProof,
    RevisePlan,
    Rewrite,
    Terminate,
}

#[derive(Debug, Clone, Copy)]
pub struct RetryLimits {
    pub max_proof_attempts: u32,
    pub max_revisions: u32,
    pub max_decompositions: u32,
}

impl Default for RetryLimits {
    fn default() -> Self {
        Self {
            max_proof_attempts: 8,
            max_revisions: 4,
            max_decompositions: 4,
        }
    }
}

/// The three nested counters. `proof` resets when the plan is revised; `proof`
/// and `revision` both reset when the decomposition is rewritten.
#[derive(Debug, Clone, Copy)]
pub struct RetryState {
    pub limits: RetryLimits,
    pub proof: u32,
    pub revision: u32,
    pub attempt: u32,
}

impl RetryState {
    pub fn new(limits: RetryLimits) -> Self {
        Self {
            limits,
            proof: 1,
            revision: 1,
            attempt: 1,
        }
    }

    /// Resolve the regulator's requested decision against the remaining
    /// budgets, auto-escalating to the next tier when the requested tier is
    /// exhausted, and apply the resulting counter transition. Returns the
    /// decision actually taken (which may be an escalation of `requested`).
    pub fn resolve(&mut self, requested: Decision) -> Decision {
        match requested {
            Decision::ReviseProof => {
                if self.proof < self.limits.max_proof_attempts {
                    self.proof += 1;
                    Decision::ReviseProof
                } else {
                    self.resolve(Decision::RevisePlan)
                }
            }
            Decision::RevisePlan => {
                if self.revision < self.limits.max_revisions {
                    self.revision += 1;
                    self.proof = 1;
                    Decision::RevisePlan
                } else {
                    self.resolve(Decision::Rewrite)
                }
            }
            Decision::Rewrite => {
                if self.attempt < self.limits.max_decompositions {
                    self.attempt += 1;
                    self.revision = 1;
                    self.proof = 1;
                    Decision::Rewrite
                } else {
                    Decision::Terminate
                }
            }
            Decision::Terminate => Decision::Terminate,
        }
    }

    /// True when the innermost (proof) budget is fully spent for the current
    /// plan revision.
    pub fn proof_budget_exhausted(&self) -> bool {
        self.proof >= self.limits.max_proof_attempts
    }

    /// **Mechanical** budget-exhaustion escalation (QED
    /// `decomposition_prover.py:1885`), deliberately SEPARATE from the semantic
    /// regulator decision in [`RetryState::resolve`]. When the inner proof loop
    /// spends its whole budget without the regulator having chosen to escalate,
    /// the orchestrator escalates on its *own* — it does not trust the model to
    /// notice it is stuck — and synthesises the escalation guidance rather than
    /// reusing a stale REVISE_PROOF message.
    ///
    /// Escalates a spent proof budget to a plan revision (resetting the proof
    /// counter), a spent revision budget to a full rewrite (resetting proof +
    /// revision), and a spent decomposition budget to `Terminate`. The returned
    /// [`Escalation`] carries the deterministic next decision and its synthetic
    /// guidance string.
    pub fn escalate_exhausted(&mut self) -> Escalation {
        if self.revision < self.limits.max_revisions {
            self.new_revision();
            Escalation {
                decision: Decision::RevisePlan,
                guidance: format!(
                    "# Automatic Escalation to Plan Revision\n\nAll {} proof attempts failed \
                     verification. The repeated failures suggest structural issues with the \
                     decomposition plan rather than the individual proofs. Revise the plan \
                     (revision {}).",
                    self.limits.max_proof_attempts, self.revision
                ),
            }
        } else if self.attempt < self.limits.max_decompositions {
            self.new_attempt();
            Escalation {
                decision: Decision::Rewrite,
                guidance: format!(
                    "# Automatic Escalation to Complete Rewrite\n\nAll {} plan revisions have \
                     been exhausted without a verified proof. Discard the current decomposition \
                     and rewrite it from scratch (attempt {}).",
                    self.limits.max_revisions, self.attempt
                ),
            }
        } else {
            Escalation {
                decision: Decision::Terminate,
                guidance: format!(
                    "# Budget Exhausted\n\nAll {} decompositions, {} revisions, and {} proof \
                     attempts are spent. Escalating to a human.",
                    self.limits.max_decompositions,
                    self.limits.max_revisions,
                    self.limits.max_proof_attempts
                ),
            }
        }
    }

    /// REVISE_PLAN transition: bump the revision counter and reset the proof
    /// counter to its first attempt.
    fn new_revision(&mut self) {
        self.revision += 1;
        self.proof = 1;
    }

    /// REWRITE transition: bump the decomposition attempt and reset the inner
    /// revision + proof counters.
    fn new_attempt(&mut self) {
        self.attempt += 1;
        self.revision = 1;
        self.proof = 1;
    }
}

/// A mechanical escalation: the deterministic next [`Decision`] plus the
/// synthesised guidance to inject into the next planning prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Escalation {
    pub decision: Decision,
    pub guidance: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revises_proof_within_budget() {
        let mut s = RetryState::new(RetryLimits::default());
        assert_eq!(s.resolve(Decision::ReviseProof), Decision::ReviseProof);
        assert_eq!(s.proof, 2);
    }

    #[test]
    fn auto_escalates_when_proof_budget_exhausted() {
        let limits = RetryLimits {
            max_proof_attempts: 2,
            max_revisions: 2,
            max_decompositions: 2,
        };
        let mut s = RetryState::new(limits);
        // proof 1 -> 2 (still ReviseProof)
        assert_eq!(s.resolve(Decision::ReviseProof), Decision::ReviseProof);
        // proof budget spent -> escalate to a plan revision (which resets proof)
        assert_eq!(s.resolve(Decision::ReviseProof), Decision::RevisePlan);
        assert_eq!(s.revision, 2);
        assert_eq!(s.proof, 1);
    }

    #[test]
    fn escalates_all_the_way_to_terminate() {
        let limits = RetryLimits {
            max_proof_attempts: 1,
            max_revisions: 1,
            max_decompositions: 1,
        };
        let mut s = RetryState::new(limits);
        // every tier is already at its single-shot budget, so a proof revision
        // cascades plan -> rewrite -> terminate.
        assert_eq!(s.resolve(Decision::ReviseProof), Decision::Terminate);
    }

    #[test]
    fn mechanical_escalation_bumps_plan_when_proof_budget_spent() {
        let limits = RetryLimits {
            max_proof_attempts: 3,
            max_revisions: 4,
            max_decompositions: 4,
        };
        let mut s = RetryState::new(limits);
        s.proof = 3; // proof budget spent
        assert!(s.proof_budget_exhausted());
        let esc = s.escalate_exhausted();
        assert_eq!(esc.decision, Decision::RevisePlan);
        assert_eq!(s.revision, 2);
        assert_eq!(s.proof, 1); // reset
        assert!(esc.guidance.contains("Plan Revision"));
    }

    #[test]
    fn mechanical_escalation_rewrites_when_revisions_spent() {
        let limits = RetryLimits {
            max_proof_attempts: 3,
            max_revisions: 2,
            max_decompositions: 4,
        };
        let mut s = RetryState::new(limits);
        s.revision = 2; // revision budget spent
        s.proof = 3;
        let esc = s.escalate_exhausted();
        assert_eq!(esc.decision, Decision::Rewrite);
        assert_eq!(s.attempt, 2);
        assert_eq!(s.revision, 1);
        assert_eq!(s.proof, 1);
    }

    #[test]
    fn mechanical_escalation_terminates_when_all_spent() {
        let limits = RetryLimits {
            max_proof_attempts: 1,
            max_revisions: 1,
            max_decompositions: 1,
        };
        let mut s = RetryState::new(limits);
        s.revision = 1;
        s.attempt = 1;
        let esc = s.escalate_exhausted();
        assert_eq!(esc.decision, Decision::Terminate);
    }

    #[test]
    fn rewrite_resets_inner_counters() {
        let limits = RetryLimits {
            max_proof_attempts: 8,
            max_revisions: 8,
            max_decompositions: 8,
        };
        let mut s = RetryState::new(limits);
        s.resolve(Decision::ReviseProof);
        s.resolve(Decision::RevisePlan);
        assert_eq!(s.resolve(Decision::Rewrite), Decision::Rewrite);
        assert_eq!(s.attempt, 2);
        assert_eq!(s.revision, 1);
        assert_eq!(s.proof, 1);
    }
}
