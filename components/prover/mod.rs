//! Prover interaction layer: ProofTask/ProofResult contracts, AttemptRun API,
//! and external-prover backends (Aristotle).
pub mod backends;
pub mod session;
pub mod formal;
pub mod model;
pub mod proof_job;
pub mod attempt_run;
pub mod axiom_audit;
pub mod proof_log;
pub mod statement_preservation;

// Re-export every leaf module flat at the component root so existing paths
// (`prover::aristotle`, hence `crate::prover::aristotle`, and sibling
// references) continue to resolve after the subgroup reorganization.
pub use backends::{aristotle, isabelle, lean, leandojo, reprover, rocq};
pub use session::{exec, goal_state, statement_guard, verify};

#[cfg(test)]
mod tests;
