//! Orchestration and reasoning: the workflows, the autonomous agent loop, the
//! research/critic/router/sampling/retry building blocks, and chat.
pub mod critique;
pub mod orchestration;
pub mod proving;
pub mod search;

// Re-export every leaf module flat at the component root so existing paths
// (`reason::mcts`, hence `crate::mcts` via app/main.rs, and sibling references
// like `crate::mcts`) continue to resolve after the subgroup reorganization.
pub use orchestration::{
    agent, blueprint_generate, blueprint_run, certification, chat, consolidate, method_transfer,
    observe, proof_import, research, statement_validation, team,
};
pub use search::{
    discovery_game, driver, fitness, goal_cache, inverse_method, mcts, minimize, process_reward,
    proof_pool, progress, rewriting, sampler, sampling, skest, subsumption, symmetry_dedup,
    tactic_outcome, ttc,
};
pub use proving::{
    blueprint, decompose, definition_synthesis, evolve_sketch, falsification, formal_generate,
    formalize_portfolio, library, mathlib_export, optimize, portfolio, repair, retry, router, sketch,
};
pub use critique::{critic, guard, memory, plan_history, taint};
