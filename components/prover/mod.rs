//! Prover interaction layer: ProofTask/ProofResult contracts, AttemptRun API,
//! and external-prover backends (Aristotle).
pub mod aristotle;
pub mod attempt_run;
pub mod formal;
pub mod isabelle;
pub mod leandojo;
pub mod model;
pub mod proof_job;
pub mod reprover;
pub mod rocq;
pub mod statement_guard;
pub mod verify;

#[cfg(test)]
mod tests;