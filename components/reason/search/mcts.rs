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

use super::critic_scorer::{blend_priority, CriticScorer, GoalStateLike};
use crate::{model::ModelRequest, provider::ModelProvider};
use anyhow::Result;
use serde_json::json;
use std::sync::Arc;

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
    /// (the [`super::critic_scorer`] seam). `0.0` (default) makes the critic term
    /// inert, so behaviour is unchanged until a critic is injected.
    ///
    /// Read by BOTH selection loops that exist: the MCGS graph driver
    /// ([`super::driver`]) and the closure-driven tree search here
    /// ([`search_with_critic`]). Both gate it the same way: the weight is forced
    /// to `0.0` unless a critic was actually injected, so no config value alone
    /// can change behaviour.
    pub critic_weight: f64,
    /// Optional eta-MCTS adaptive per-node expansion budget
    /// ([`super::distance_critic::expansion_budget`]). `None` (default) keeps the
    /// fixed `expand_k` breadth; `Some(cfg)` spends more breadth on high-critic
    /// (uncertain) nodes and less on settled ones.
    pub eta_mcts: Option<super::distance_critic::EtaMctsConfig>,
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
            eta_mcts: None,
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
    /// Trained-critic `V(s)` in `[0, 1]` (see [`search_with_critic`]). Defaults to
    /// this node's `progress` when no critic is injected, mirroring the driver, so
    /// the blended critic term contributes nothing new on the no-critic path.
    critic: f64,
}

/// Read a node's state-value estimate, falling back to `progress`.
///
/// Deliberately the same two guards as [`super::driver`]'s `critic_estimate`, for
/// the same reasons, rather than a second policy:
/// * with no critic the estimate *is* `progress`, so the extra term duplicates a
///   signal already present instead of introducing one;
/// * a non-finite critic value is discarded in favour of `progress`, because a
///   `NaN` would poison every `score > best_score` comparison in the selection
///   loop (all comparisons against `NaN` are false), silently collapsing the
///   descent. Finite values are clamped to the documented `[0, 1]` contract so a
///   miscalibrated critic cannot swamp `q` and `u` outright.
fn critic_estimate<S: GoalStateLike>(
    critic: Option<&Arc<dyn CriticScorer>>,
    state: &S,
    progress: f64,
) -> f64 {
    match critic {
        None => progress,
        Some(c) => {
            let v = c.score(state);
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                progress
            }
        }
    }
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
    prior_expand: EXPAND,
    reward: REWARD,
    value: VALUE,
) -> SearchResult<A>
where
    A: Clone,
    EXPAND: FnMut(&S) -> Vec<(A, S, f64)>,
    REWARD: FnMut(&S) -> Option<f64>,
    VALUE: FnMut(&S) -> f64,
{
    // The critic term is switched off two independent ways on this path: the
    // per-node estimate is just `progress` (the identity fallback) AND the weight
    // is a literal `0.0`. No `CriticScorer` is constructed or called, so this entry
    // point cannot be perturbed by a critic even in principle. Note `S` carries no
    // `GoalStateLike` bound here, which is what keeps every existing caller
    // (including non-goal states such as the integer toy tree in the tests)
    // compiling unchanged.
    search_core(root, cfg, prior_expand, reward, value, |_, p| p, 0.0)
}

/// Run MCTS from `root` with an injectable trained state-value critic folded into
/// PUCT selection: the [`super::critic_scorer`] seam, applied to the tree search.
///
/// This is [`search_with_value`] plus one extra additive term. A child's selection
/// score becomes
/// `q + progress_weight·progress + critic_weight·V(s') + c·P(a)·√N_parent/(1+N_child)`,
/// computed by the shared [`blend_priority`] so the driver and the tree search
/// cannot drift into two different blends.
///
/// # Why this is safe to land unmeasured
///
/// `critic_weight` is forced to `0.0` whenever `critic` is `None`, exactly as the
/// driver does. So the *only* way the critic term is non-zero is a caller both
/// injecting a critic and setting a non-zero weight, and at the default
/// `critic_weight = 0.0` the term is `0.0 · V(s')`, which leaves every score bit
/// for bit what it is today.
///
/// # What a critic may and may not do
///
/// It reorders exploration and nothing else. Terminality, the backed-up reward and
/// [`SearchResult::solved`] all come exclusively from the `reward` closure; the
/// critic value is never compared against a threshold, never written into
/// `value_sum`, and never reaches [`rollout`]. A wrong critic costs search
/// efficiency, never soundness.
///
/// The critic is passed as `Option<Arc<dyn CriticScorer>>` so the caller obtains it
/// from [`super::critic_scorer::critic_from_config`] and swapping the placeholder
/// [`HeuristicCritic`](super::critic_scorer::HeuristicCritic) for a trained value
/// head stays a one-line change to that factory, with no trained-model dependency
/// visible from this loop.
pub fn search_with_critic<S, A, EXPAND, REWARD, VALUE>(
    root: S,
    cfg: &SearchConfig,
    prior_expand: EXPAND,
    reward: REWARD,
    value: VALUE,
    critic: Option<Arc<dyn CriticScorer>>,
) -> SearchResult<A>
where
    S: GoalStateLike,
    A: Clone,
    EXPAND: FnMut(&S) -> Vec<(A, S, f64)>,
    REWARD: FnMut(&S) -> Option<f64>,
    VALUE: FnMut(&S) -> f64,
{
    // Gate the weight on a critic actually being present. Without this, a non-zero
    // `critic_weight` with no critic would double-count the progress prior (the
    // fallback estimate IS progress), which is a silent behaviour change driven by
    // config alone. That is precisely what we are refusing to allow.
    let critic_weight = if critic.is_some() {
        cfg.critic_weight
    } else {
        0.0
    };
    // The `Arc` is moved INTO the closure rather than borrowed, so there is no
    // outliving-borrow temporary for the borrow checker to reject at the tail call.
    search_core(
        root,
        cfg,
        prior_expand,
        reward,
        value,
        move |state, progress| critic_estimate(critic.as_ref(), state, progress),
        critic_weight,
    )
}

/// The shared selection/expansion/backprop core. `critic_of(state, progress)`
/// yields the node's `V(s)` estimate and `critic_weight` scales it; the identity
/// fallback plus a zero weight recovers the pre-critic search exactly.
#[allow(clippy::too_many_arguments)]
fn search_core<S, A, EXPAND, REWARD, VALUE, CRITIC>(
    root: S,
    cfg: &SearchConfig,
    mut prior_expand: EXPAND,
    mut reward: REWARD,
    mut value: VALUE,
    mut critic_of: CRITIC,
    critic_weight: f64,
) -> SearchResult<A>
where
    A: Clone,
    EXPAND: FnMut(&S) -> Vec<(A, S, f64)>,
    REWARD: FnMut(&S) -> Option<f64>,
    VALUE: FnMut(&S) -> f64,
    CRITIC: FnMut(&S, f64) -> f64,
{
    let root_terminal = reward(&root);
    let root_progress = value(&root);
    let root_critic = critic_of(&root, root_progress);
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
        critic: root_critic,
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
                // LeanProgress-style value prior plus the trained-critic term, via
                // the ONE shared blend the driver also uses, so the two selection
                // loops cannot drift apart. `critic_weight` is `0.0` unless a critic
                // was injected, and `0.0 * finite == 0.0`, so this is arithmetically
                // the previous `q + progress_weight*progress + u` on every existing
                // path. The estimate is guaranteed finite by `critic_estimate`, so
                // no NaN can reach the comparison below.
                let score = blend_priority(
                    q,
                    c.progress,
                    cfg.progress_weight,
                    c.critic,
                    critic_weight,
                    u,
                );
                // Strict `>` over `children` in insertion order: on an exact tie the
                // earliest-expanded (highest-prior) child keeps the win. The arena is
                // a `Vec` and children are pushed in expander order, so tie-breaking
                // is a deterministic function of the expander, never of hash order.
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
                    let child_critic = critic_of(&state, child_progress);
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
                        critic: child_critic,
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

    // ---- The critic seam ---------------------------------------------------
    //
    // A toy state that is scorable by a `CriticScorer`. It implements
    // `GoalStateLike` directly and is NOT a `driver::GoalState`, so the blanket
    // bridge in `critic_scorer` never applies and there is no coherence overlap.
    #[derive(Clone, Copy, PartialEq, Debug)]
    struct Toy(i64);
    impl GoalStateLike for Toy {
        fn state_text(&self) -> String {
            format!("⊢ node {}", self.0)
        }
    }

    fn toy_expand(n: &Toy) -> Vec<(&'static str, Toy, f64)> {
        vec![("L", Toy(n.0 * 2), 0.5f64), ("R", Toy(n.0 * 2 + 1), 0.5f64)]
    }
    fn toy_reward(n: &Toy) -> Option<f64> {
        if n.0 >= 8 {
            Some(0.0)
        } else {
            None
        }
    }

    /// A deterministic critic that rates the even ("L") branch closer to done.
    struct EvenCritic;
    impl super::CriticScorer for EvenCritic {
        fn score(&self, state: &dyn GoalStateLike) -> f64 {
            // Parse back out of the textual contract, which is all a critic ever
            // sees. Even nodes score high.
            let n: i64 = state
                .state_text()
                .rsplit(' ')
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            if n % 2 == 0 {
                0.9
            } else {
                0.1
            }
        }
    }

    /// A critic that returns `NaN`, the only way an untrained or erroring
    /// implementation can signal failure through an `f64`.
    struct NanCritic;
    impl super::CriticScorer for NanCritic {
        fn score(&self, _state: &dyn GoalStateLike) -> f64 {
            f64::NAN
        }
    }

    fn critic_cfg(critic_weight: f64) -> SearchConfig {
        SearchConfig {
            max_nodes: 64,
            critic_weight,
            ..SearchConfig::default()
        }
    }

    /// The safety property that makes this landable unmeasured: at
    /// `critic_weight == 0.0` an injected critic changes NOTHING, whatever it says.
    #[test]
    fn critic_weight_zero_is_identical_to_the_critic_free_search() {
        let cfg = critic_cfg(0.0);
        let baseline = super::search_with_value(Toy(1), &cfg, toy_expand, toy_reward, |_| 0.0);
        for critic in [
            None,
            Some(Arc::new(EvenCritic) as Arc<dyn super::CriticScorer>),
            Some(Arc::new(super::super::critic_scorer::ConstantCritic(1.0))
                as Arc<dyn super::CriticScorer>),
        ] {
            let got = super::search_with_critic(
                Toy(1),
                &cfg,
                toy_expand,
                toy_reward,
                |_: &Toy| 0.0,
                critic,
            );
            assert_eq!(got.best_action, baseline.best_action);
            assert_eq!(got.root_visits, baseline.root_visits);
            assert_eq!(got.visit_counts, baseline.visit_counts);
            assert_eq!(got.solved, baseline.solved);
        }
    }

    /// With no critic injected, `critic_weight` is inert for EVERY value: the
    /// weight alone can never change behaviour, so no config path can regress the
    /// default.
    #[test]
    fn weight_without_a_critic_is_inert_at_every_value() {
        let baseline = super::search_with_value(
            Toy(1),
            &critic_cfg(0.0),
            toy_expand,
            toy_reward,
            |_| 0.0,
        );
        for w in [0.0, 0.5, 5.0, -3.0] {
            let got = super::search_with_critic(
                Toy(1),
                &critic_cfg(w),
                toy_expand,
                toy_reward,
                |_: &Toy| 0.0,
                None,
            );
            assert_eq!(
                got.visit_counts, baseline.visit_counts,
                "no critic injected, critic_weight={w}"
            );
        }
    }

    /// The seam is LIVE: with a critic and a non-zero weight, the critic's verdict
    /// decides which root action the search commits its visits to, and flipping the
    /// critic flips the choice (so it is the critic, not a fixed tie-break).
    #[test]
    fn critic_steers_the_root_choice_when_the_weight_is_on() {
        let cfg = critic_cfg(1.0);
        let even = super::search_with_critic(
            Toy(1),
            &cfg,
            toy_expand,
            toy_reward,
            |_: &Toy| 0.0,
            Some(Arc::new(EvenCritic)),
        );
        assert_eq!(even.best_action, Some("L"));

        // Same search, critic inverted: the odd branch now wins.
        struct OddCritic;
        impl super::CriticScorer for OddCritic {
            fn score(&self, state: &dyn GoalStateLike) -> f64 {
                1.0 - EvenCritic.score(state)
            }
        }
        let odd = super::search_with_critic(
            Toy(1),
            &cfg,
            toy_expand,
            toy_reward,
            |_: &Toy| 0.0,
            Some(Arc::new(OddCritic)),
        );
        assert_eq!(odd.best_action, Some("R"));
    }

    /// A critic cannot make anything true. On a tree where every terminal scores
    /// `0.0`, a critic pinned at the maximum `1.0` must not flip `solved` and must
    /// not inflate the backed-up value: those come only from `reward`.
    #[test]
    fn critic_never_decides_that_something_is_proved() {
        let result = super::search_with_critic(
            Toy(1),
            &critic_cfg(10.0),
            toy_expand,
            toy_reward,
            |_: &Toy| 0.0,
            Some(Arc::new(super::super::critic_scorer::ConstantCritic(1.0))),
        );
        assert!(!result.solved, "only `reward` may declare a solve");
        assert_eq!(result.best_value, 0.0, "critic must not enter value_sum");
    }

    /// A `NaN` critic degrades to the progress signal instead of poisoning the
    /// `score > best_score` comparison and truncating the descent.
    #[test]
    fn a_non_finite_critic_degrades_to_progress() {
        let cfg = critic_cfg(1.0);
        let value = |n: &Toy| if n.0 % 2 == 0 { 0.9 } else { 0.1 };
        let with_nan = super::search_with_critic(
            Toy(1),
            &cfg,
            toy_expand,
            toy_reward,
            value,
            Some(Arc::new(NanCritic)),
        );
        assert!(with_nan.best_action.is_some(), "descent must not collapse");
        assert!(with_nan.root_visits > 0);
    }

    /// Determinism: identical inputs give identical visit counts across runs. The
    /// critic is a pure function of the state text and ties break by arena
    /// insertion order, so there is nothing left to vary.
    #[test]
    fn critic_guided_search_is_reproducible() {
        let cfg = critic_cfg(1.0);
        let run = || {
            super::search_with_critic(
                Toy(1),
                &cfg,
                toy_expand,
                toy_reward,
                |_: &Toy| 0.0,
                Some(Arc::new(EvenCritic) as Arc<dyn super::CriticScorer>),
            )
            .visit_counts
        };
        assert_eq!(run(), run());
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
