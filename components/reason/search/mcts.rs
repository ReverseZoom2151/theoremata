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

/// Which selection rule the graph driver ([`crate::search::driver`]) uses to
/// choose the next node to descend into.
///
/// Only consumed by the MCGS driver; the closure-driven [`search`] here always
/// uses plain PUCT. Default is [`SelectionMode::Puct`] so existing behaviour is
/// unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionMode {
    /// Classic PUCT: pick the single child with the highest
    /// `Q + progress_weight·value + c·P·√N_parent/(1+N_child)`.
    Puct,
    /// Aristotle-style AND/OR minimax (plan: `docs/paper-mining/aristotle.md`).
    /// Pick the highest-**UCB** *action* (an OR choice over tactics), then among
    /// that action's resulting subgoals descend into the **hardest** = lowest-**LCB**
    /// child (the AND child most likely to sink the whole action).
    AndOrMinimax,
}

/// Where a node's action priors come from in the MCGS driver.
///
/// Only consumed by the driver. Default [`PriorMode::Progress`] keeps the fixed
/// per-step progress/prior heuristic, so existing behaviour is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriorMode {
    /// Use the prior each [`crate::search::driver::TacticStep`] carries (the
    /// progress/heuristic weight the expander assigned).
    Progress,
    /// Aristotle's *empirical sampled-action distribution*: sample the expander
    /// `N` times and set each action's prior to the frequency with which it was
    /// drawn (explicitly **not** sequence logprobs). The `usize` is the sample
    /// count `N`.
    EmpiricalSampled(usize),
}

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
    /// Blend weight for the LeanProgress-style progress prior in PUCT selection
    /// (see [`search_with_value`]). Higher biases expansion toward proof states
    /// the value estimate rates as closer to done. Has no effect when the value
    /// closure returns `0` for every state — so plain [`search`] is unaffected
    /// regardless of this value.
    pub progress_weight: f64,
    /// Selection rule for the MCGS driver. Default [`SelectionMode::Puct`].
    pub selection: SelectionMode,
    /// Where the MCGS driver draws action priors from. Default
    /// [`PriorMode::Progress`].
    pub prior_mode: PriorMode,
    /// Blend weight for a trained state-value critic `V(s)` in PUCT selection
    /// (the [`crate::critic_scorer`] seam). `0.0` (default) makes the critic term
    /// inert, so behaviour is unchanged until a critic is injected.
    pub critic_weight: f64,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            max_nodes: 200,
            exploration: 1.41,
            expand_k: 4,
            max_depth: 12,
            progress_weight: crate::progress::PROGRESS_PRIOR_WEIGHT,
            selection: SelectionMode::Puct,
            prior_mode: PriorMode::Progress,
            critic_weight: 0.0,
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
    /// LeanProgress-style value estimate of this state in `[0, 1]` (see
    /// [`search_with_value`]); `0.0` under plain [`search`].
    progress: f64,
}

/// Run MCTS from `root`.
///
/// `prior_expand(&state)` returns up to `expand_k` candidate
/// `(action, next_state, prior_probability)` triples — this is where the model
/// prior enters. `reward(&state)` returns `Some(r)` when the state is terminal
/// (e.g. `1.0` if the proof compiles / `0.0` if refuted) and `None` otherwise.
///
/// This is the value-free entry point: it defers to [`search_with_value`] with a
/// constant-`0` progress estimate, so PUCT selection is unchanged regardless of
/// `cfg.progress_weight`.
pub fn search<S, A, EXPAND, REWARD>(
    root: S,
    cfg: &SearchConfig,
    prior_expand: EXPAND,
    reward: REWARD,
) -> SearchResult<A>
where
    A: Clone,
    EXPAND: FnMut(&S) -> Vec<(A, S, f64)>,
    REWARD: FnMut(&S) -> Option<f64>,
{
    search_with_value(root, cfg, prior_expand, reward, |_| 0.0)
}

/// Run MCTS from `root` with a LeanProgress-style state-value prior.
///
/// Identical to [`search`] but takes a `value(&state) -> f64` closure returning a
/// progress estimate in `[0, 1]` (e.g. [`crate::progress::progress_value`]). The
/// estimate is folded into PUCT selection: a child's score becomes
/// `Q(s,a) + progress_weight · value(s') + c · P(a) · √N_parent / (1 + N_child)`,
/// so before a node has been visited (`Q = 0`) selection is biased toward the
/// children the value model rates as closest to `no goals`. This is the search
/// prior LeanProgress-v2 wires in as "negative predicted remaining steps".
pub fn search_with_value<S, A, EXPAND, REWARD, VALUE>(
    root: S,
    cfg: &SearchConfig,
    mut prior_expand: EXPAND,
    mut reward: REWARD,
    mut value: VALUE,
) -> SearchResult<A>
where
    A: Clone,
    EXPAND: FnMut(&S) -> Vec<(A, S, f64)>,
    REWARD: FnMut(&S) -> Option<f64>,
    VALUE: FnMut(&S) -> f64,
{
    let root_terminal = reward(&root);
    let root_progress = value(&root);
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
        progress: root_progress,
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
                // LeanProgress-style value prior: nudge selection toward states
                // rated closer to done. Zero under plain `search`.
                let score = q + cfg.progress_weight * c.progress + u;
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
                    let child_progress = value(&state);
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
                        progress: child_progress,
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

    #[test]
    fn progress_prior_biases_the_root_choice() {
        // Unsolvable tree so the search never short-circuits: every terminal
        // (n >= 8) scores 0. With a value prior that rates the even ("L") branch
        // much closer to done, selection keeps descending the L child, so the
        // most-visited (robust) root action is "L".
        let expand = |n: &i64| vec![("L", n * 2, 0.5f64), ("R", n * 2 + 1, 0.5f64)];
        let reward = |n: &i64| if *n >= 8 { Some(0.0) } else { None };
        let value = |n: &i64| if n % 2 == 0 { 0.9 } else { 0.1 };
        let cfg = SearchConfig {
            max_nodes: 64,
            progress_weight: 1.0,
            ..SearchConfig::default()
        };
        let result = super::search_with_value(1i64, &cfg, expand, reward, value);
        assert_eq!(result.best_action, Some("L"));

        // Flipping the prior to favour the odd ("R") branch flips the choice —
        // confirming it is the prior, not a fixed tie-break, driving selection.
        let expand = |n: &i64| vec![("L", n * 2, 0.5f64), ("R", n * 2 + 1, 0.5f64)];
        let reward = |n: &i64| if *n >= 8 { Some(0.0) } else { None };
        let value_r = |n: &i64| if n % 2 == 0 { 0.1 } else { 0.9 };
        let result_r = super::search_with_value(1i64, &cfg, expand, reward, value_r);
        assert_eq!(result_r.best_action, Some("R"));
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
