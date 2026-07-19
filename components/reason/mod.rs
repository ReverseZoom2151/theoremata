//! Orchestration and reasoning: the workflows, the autonomous agent loop, the
//! research/critic/router/sampling/retry building blocks, and chat.
pub mod critique;
pub mod orchestration;
pub mod proving;
pub mod search;

// Re-export every leaf module flat at the component root so existing paths
// (`reason::mcts`, hence `crate::mcts` via app/main.rs, and sibling references
// like `crate::mcts`) continue to resolve after the subgroup reorganization.
pub use critique::{critic, guard, guardrails, memory, plan_history, taint};
pub use orchestration::{
    agent, blueprint_generate, blueprint_run, certification, chat, consolidate, context_assembly,
    live_plan, meta_tools, method_transfer, observe, proof_import, research, statement_validation,
    team, trace,
};
pub use proving::{
    blueprint, checker_cache, conjecture_engine, decompose, definition_synthesis, evolve_sketch,
    falsification, formal_generate, formalize_modes, formalize_portfolio, graph_rag, library,
    mathlib_export, model_router, optimize, portfolio, refine_ops, repair, retry, router, sketch,
};
pub use search::{
    best_first, concurrent, critic_scorer, dag_projection, discovery_game, distance_critic, driver,
    fitness, goal_cache, hybrid_search, inverse_method, mcts, minimize, model_elimination,
    preference_pairs, process_reward, progress, proof_pool, rewriting, sampler, sampling,
    search_telemetry, skest, subsumption, symmetry_dedup, tactic_outcome, ttc,
};
