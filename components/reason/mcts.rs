//! MCTS over tactics with an LLM prior (plan §14, the AlphaProof pattern).
//!
//! A generic, closure-driven Monte-Carlo tree search: PUCT selection biased by
//! a learned/model prior, expansion of candidate actions, a greedy simulation
//! to a terminal check, and backpropagation of the (compiler) reward. The state
//! and action types are generic and the environment is supplied as closures, so
//! the core is unit-testable without a model or Lean. The root visit counts are
//! exposed as the distilled policy target — feed them back into the model to
//! turn expensive search into cheap single-shot capability over time.
//!
//! PUCT score for a child of a parent with `N_parent` visits:
//! `Q(s,a) + c · P(a) · sqrt(N_parent) / (1 + N_child)`, where `Q` is the mean
//! backed-up reward (`0` when unvisited), `P` the prior probability, and `c` the
//! exploration constant.

use crate::{model::ModelRequest, provider::ModelProvider};
use anyhow::Result;
use serde_json::json;

/// Search budget and PUCT tuning.
#[derive(Debug, Clone, Copy)]
pub struct SearchConfig {
    /// Hard cap on tree size (expanded nodes) and on iterations.
    pub max_nodes: usize,
    /// PUCT exploration constant `c`.
    pub exploration: f64,
    /// Max children expanded per node (the top-`k` prior candidates).
    pub expand_k: usize,
    /// Rollout depth cap before a non-terminal simulation is scored `0`.
    pub max_depth: usize,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            max_nodes: 200,
            exploration: 1.41,
            expand_k: 4,
            max_depth: 12,
        }
    }
}

/// The outcome of a search from the root.
#[derive(Debug, serde::Serialize)]
pub struct SearchResult<A> {
    /// The most-visited (robust) action at the root, if any child was expanded.
    pub best_action: Option<A>,
    /// How many simulations passed through the root.
    pub root_visits: usize,
    /// Mean backed-up value of the chosen root action.
    pub best_value: f64,
    /// `(action, visit_count)` for every root child — the distilled policy
    /// target to train the prior on.
    pub visit_counts: Vec<(A, usize)>,
    /// Whether a terminal state with reward `>= 1.0` was found.
    pub solved: bool,
}

struct Node<S, A> {
    state: S,
    #[allow(dead_code)] // arena back-link; backprop uses the selection path
    parent: Option<usize>,
    action: Option<A>,
    prior: f64,
    visits: usize,
    value_sum: f64,
    children: Vec<usize>,
    expanded: bool,
    terminal: Option<f64>,
}

/// Run MCTS from `root`.
///
/// `prior_expand(&state)` returns up to `expand_k` candidate
/// `(action, next_state, prior_probability)` triples — this is where the model
/// prior enters. `reward(&state)` returns `Some(r)` when the state is terminal
/// (e.g. `1.0` if the proof compiles / `0.0` if refuted) and `None` otherwise.
pub fn search<S, A, EXPAND, REWARD>(
    root: S,
    cfg: &SearchConfig,
    mut prior_expand: EXPAND,
    mut reward: REWARD,
) -> SearchResult<A>
where
    A: Clone,
    EXPAND: FnMut(&S) -> Vec<(A, S, f64)>,
    REWARD: FnMut(&S) -> Option<f64>,
{
    let root_terminal = reward(&root);
    let mut nodes: Vec<Node<S, A>> = vec![Node {
        state: root,
        parent: None,
        action: None,
        prior: 1.0,
        visits: 0,
        value_sum: 0.0,
        children: Vec::new(),
        expanded: false,
        terminal: root_terminal,
    }];

    let mut solved = matches!(root_terminal, Some(r) if r >= 1.0);

    for _ in 0..cfg.max_nodes.max(1) {
        // 1. Selection: descend by PUCT to a leaf / terminal.
        let mut path = vec![0usize];
        let mut current = 0usize;
        while !nodes[current].children.is_empty() && nodes[current].terminal.is_none() {
            let n_parent = (nodes[current].visits.max(1) as f64).sqrt();
            let mut best = nodes[current].children[0];
            let mut best_score = f64::NEG_INFINITY;
            for &ci in &nodes[current].children {
                let c = &nodes[ci];
                let q = if c.visits > 0 {
                    c.value_sum / c.visits as f64
                } else {
                    0.0
                };
                let u = cfg.exploration * c.prior * n_parent / (1.0 + c.visits as f64);
                let score = q + u;
                if score > best_score {
                    best_score = score;
                    best = ci;
                }
            }
            current = best;
            path.push(current);
        }

        // 2/3. Evaluate the leaf: terminal reward, or expand + simulate.
        let leaf_reward = if let Some(r) = nodes[current].terminal {
            r
        } else {
            if !nodes[current].expanded && nodes.len() < cfg.max_nodes {
                let candidates = prior_expand(&nodes[current].state);
                let mut child_ids = Vec::new();
                for (action, state, prior) in candidates.into_iter().take(cfg.expand_k) {
                    if nodes.len() >= cfg.max_nodes {
                        break;
                    }
                    let terminal = reward(&state);
                    let id = nodes.len();
                    nodes.push(Node {
                        state,
                        parent: Some(current),
                        action: Some(action),
                        prior: prior.max(1e-9),
                        visits: 0,
                        value_sum: 0.0,
                        children: Vec::new(),
                        expanded: false,
                        terminal,
                    });
                    child_ids.push(id);
                }
                nodes[current].children = child_ids;
                nodes[current].expanded = true;
            }
            rollout(&nodes[current].state, cfg, &mut prior_expand, &mut reward)
        };

        if leaf_reward >= 1.0 {
            solved = true;
        }

        // 4. Backpropagation.
        for &ni in &path {
            nodes[ni].visits += 1;
            nodes[ni].value_sum += leaf_reward;
        }

        // A perfect verifier means once solved we can stop spending expansions.
        if solved {
            break;
        }
    }

    // Robust child: pick the most-visited root action.
    let mut visit_counts: Vec<(A, usize)> = Vec::new();
    let mut best_action = None;
    let mut best_visits = 0usize;
    let mut best_value = 0.0f64;
    for &ci in &nodes[0].children {
        let c = &nodes[ci];
        if let Some(a) = &c.action {
            visit_counts.push((a.clone(), c.visits));
            if c.visits >= best_visits {
                best_visits = c.visits;
                best_action = Some(a.clone());
                best_value = if c.visits > 0 {
                    c.value_sum / c.visits as f64
                } else {
                    0.0
                };
            }
        }
    }

    SearchResult {
        best_action,
        root_visits: nodes[0].visits,
        best_value,
        visit_counts,
        solved,
    }
}

/// Greedy simulation: follow the first (highest-prior) child until a terminal
/// state is reached or the depth cap is hit.
fn rollout<S, A, EXPAND, REWARD>(
    start: &S,
    cfg: &SearchConfig,
    prior_expand: &mut EXPAND,
    reward: &mut REWARD,
) -> f64
where
    EXPAND: FnMut(&S) -> Vec<(A, S, f64)>,
    REWARD: FnMut(&S) -> Option<f64>,
{
    if let Some(r) = reward(start) {
        return r;
    }
    let mut state = match prior_expand(start).into_iter().next() {
        Some((_, next, _)) => next,
        None => return 0.0,
    };
    for _ in 0..cfg.max_depth {
        if let Some(r) = reward(&state) {
            return r;
        }
        state = match prior_expand(&state).into_iter().next() {
            Some((_, next, _)) => next,
            None => return 0.0,
        };
    }
    reward(&state).unwrap_or(0.0)
}

/// Uses a model to propose candidate tactics with prior weights — the `EXPAND`
/// prior for a real proof search.
pub struct TacticMcts<'a> {
    pub provider: &'a dyn ModelProvider,
}

impl TacticMcts<'_> {
    /// Ask the model for up to `k` candidate Lean tactics for `goal`, each with
    /// a prior weight in `[0, 1]`.
    pub fn propose_tactics(&self, goal: &str, k: usize) -> Result<Vec<(String, f64)>> {
        let request = ModelRequest {
            role: "tactic_proposer".into(),
            task: format!(
                "Propose up to {k} candidate Lean 4 tactics that make progress on the goal, \
                 each with a prior weight in [0,1] reflecting how promising it is. Order most \
                 promising first."
            ),
            context: json!({ "goal": goal }),
            output_schema: json!({
                "type": "object",
                "required": ["tactics"],
                "properties": {
                    "tactics": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["tactic", "weight"],
                            "properties": {
                                "tactic": {"type": "string"},
                                "weight": {"type": "number"}
                            }
                        }
                    }
                }
            }),
        };
        let response = self.provider.complete(&request)?;
        let mut out = Vec::new();
        if let Some(items) = response.content["tactics"].as_array() {
            for item in items.iter().take(k) {
                if let Some(tactic) = item["tactic"].as_str() {
                    let weight = item["weight"].as_f64().unwrap_or(1.0);
                    out.push((tactic.to_owned(), weight));
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelResponse;

    /// A deterministic toy tree over heap-indexed integers: node `n` has
    /// children `2n` (action "L") and `2n+1` (action "R"). Nodes `>= 8` are
    /// terminal; `target` scores 1.0, every other terminal scores 0.0.
    fn toy(
        target: i64,
    ) -> (
        impl FnMut(&i64) -> Vec<(&'static str, i64, f64)>,
        impl FnMut(&i64) -> Option<f64>,
    ) {
        let expand = |n: &i64| vec![("L", n * 2, 0.5f64), ("R", n * 2 + 1, 0.5f64)];
        let reward = move |n: &i64| {
            if *n >= 8 {
                Some(if *n == target { 1.0 } else { 0.0 })
            } else {
                None
            }
        };
        (expand, reward)
    }

    #[test]
    fn finds_the_rewarding_path() {
        // target 12 is reachable as 1 -> 3 -> 6 -> 12.
        let (expand, reward) = toy(12);
        let cfg = SearchConfig::default();
        let result = super::search(1i64, &cfg, expand, reward);
        assert!(result.solved, "search should find the rewarding terminal");
        assert!(result.best_action.is_some());
        assert!(!result.visit_counts.is_empty());
        // Never exceeds the iteration budget.
        assert!(result.root_visits <= cfg.max_nodes);
        assert!(result.root_visits > 0);
    }

    #[test]
    fn respects_max_nodes_when_unsolved() {
        // target 999 is unreachable, so the search runs its full budget without
        // solving, and the tree size stays bounded (no panic / no runaway).
        let (expand, reward) = toy(999);
        let cfg = SearchConfig {
            max_nodes: 32,
            ..SearchConfig::default()
        };
        let result = super::search(1i64, &cfg, expand, reward);
        assert!(!result.solved);
        assert!(result.root_visits <= cfg.max_nodes);
        assert!(!result.visit_counts.is_empty());
    }

    struct MockProposer;
    impl ModelProvider for MockProposer {
        fn complete(&self, _: &ModelRequest) -> Result<ModelResponse> {
            Ok(ModelResponse {
                provider: "test".into(),
                model: "test".into(),
                content: json!({
                    "tactics": [
                        {"tactic": "simp", "weight": 0.6},
                        {"tactic": "ring", "weight": 0.4}
                    ]
                }),
            })
        }
        fn name(&self) -> &str {
            "test"
        }
    }

    #[test]
    fn proposes_tactics_from_the_model() {
        let mcts = TacticMcts {
            provider: &MockProposer,
        };
        let tactics = mcts.propose_tactics("⊢ n + 0 = n", 4).unwrap();
        assert_eq!(tactics.len(), 2);
        assert_eq!(tactics[0].0, "simp");
        assert!((tactics[0].1 - 0.6).abs() < 1e-9);
        assert_eq!(tactics[1].0, "ring");
    }
}
