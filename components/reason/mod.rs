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
    agent, blueprint_run, certification, chat, consolidate, observe, research, statement_validation,
    team,
};
pub use search::{
    driver, fitness, goal_cache, mcts, minimize, proof_pool, progress, sampler, sampling,
    subsumption, tactic_outcome, ttc,
};
pub use proving::{
    blueprint, decompose, evolve_sketch, falsification, formal_generate, formalize_portfolio,
    library, optimize, portfolio, repair, retry, router, sketch,
};
pub use critique::{critic, guard, memory, plan_history, taint};
