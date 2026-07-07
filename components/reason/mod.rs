//! Orchestration and reasoning: the workflows, the autonomous agent loop, the
//! research/critic/router/sampling/retry building blocks, and chat.
pub mod critique;
pub mod orchestration;
pub mod proving;
pub mod search;

// Re-export every leaf module flat at the component root so existing paths
// (`reason::mcts`, hence `crate::mcts` via app/main.rs, and sibling references
// like `crate::mcts`) continue to resolve after the subgroup reorganization.
pub use orchestration::{agent, chat, consolidate, observe, research, team};
pub use search::{mcts, progress, sampler, sampling};
pub use proving::{blueprint, decompose, falsification, retry, router};
pub use critique::{critic, guard, plan_history, taint};
