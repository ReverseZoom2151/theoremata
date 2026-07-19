//! Prover interaction layer: ProofTask/ProofResult contracts, AttemptRun API,
//! and external-prover backends (Aristotle).
pub mod attempt_run;
pub mod axiom_audit;
pub mod backends;
pub mod decl_index_adapter;
pub mod declaration_lookup;
pub mod error_feedback;
pub mod formal;
pub mod hypothesis_audit;
pub mod infotree;
pub mod model;
pub mod proof_job;
pub mod proof_log;
pub mod session;
pub mod statement_preservation;
pub mod subgoal_extract;
pub mod vacuity;

// Re-export every leaf module flat at the component root so existing paths
// (`prover::aristotle`, hence `crate::prover::aristotle`, and sibling
// references) continue to resolve after the subgroup reorganization.
pub use backends::{aristotle, external, isabelle, lean, leandojo, reprover, rocq};
pub use session::{exec, goal_state, statement_guard, verify};

#[cfg(test)]
mod tests;
