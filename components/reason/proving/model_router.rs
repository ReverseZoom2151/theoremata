//! Difficulty-aware model-tier router + provider fallback ladder
//! (agentic-patterns-mining §A4).
//!
//! Our test-time-compute controller ([`crate::ttc`]) budgets *how much search*
//! to spend on a goal from a numeric difficulty estimate in `[0, 1]`, but it
//! says nothing about *which model* runs that search, nor what to do when the
//! chosen provider call fails. This module closes both gaps with two pure,
//! deterministic pieces that consume the *same* `difficulty` signal the TTC
//! controller already produces:
//!
//! * [`route_model`] maps `(difficulty, attempt)` to a [`ModelTier`] — a cheap
//!   model for easy goals, a strong model for hard ones — and *escalates* the
//!   tier as a goal's failed attempts pile up, so a stuck goal is promoted to
//!   more capable (and more expensive) hardware without changing the caller.
//! * [`FallbackLadder`] walks an ordered list of concrete provider/model
//!   endpoints, so a provider error (rate-limit, timeout, empty output) degrades
//!   to the *next* endpoint rather than aborting the whole attempt.
//!
//! [`ModelPlan`] fuses the two: for a `(difficulty, attempt)` it picks a tier,
//! anchors the ladder at the endpoint matching that tier, and hands back the
//! primary choice plus the ordered fallback sequence to try on failure.
//!
//! Everything here is *routing metadata*: pure functions over configuration and
//! the difficulty/attempt/error signals. No clock, no randomness, no I/O — the
//! decision is auditable and exhaustively testable in isolation. The actual
//! model call (and thus which concrete model each tier resolves to) stays in the
//! provider layer ([`crate::provider`]); this module only *chooses*.
//!
//! Relationship to [`crate::guard::model_tier`]: that router keys off the node
//! *kind* plus a free-text difficulty hint and returns a `Cheap/Standard/Frontier`
//! tier — it is the coarse, structural cousin. This module keys off the *numeric*
//! `[0, 1]` difficulty (the TTC signal) and, crucially, adds the attempt-count
//! escalation *and* the provider fallback ladder that `guard` has no notion of.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::{
    model::{ModelRequest, ModelResponse},
    provider::ModelProvider,
};

/// A model cost/capability tier. `Fast` is the cheapest model, `Strong` the most
/// capable; the ordinal (`as_index`) is the escalation order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelTier {
    Fast,
    Balanced,
    Strong,
}

impl ModelTier {
    /// The tier's position in the escalation order (`Fast = 0 … Strong = 2`).
    pub fn as_index(self) -> usize {
        match self {
            ModelTier::Fast => 0,
            ModelTier::Balanced => 1,
            ModelTier::Strong => 2,
        }
    }

    /// The strongest tier (the escalation ceiling).
    pub const MAX: ModelTier = ModelTier::Strong;

    /// Reconstruct a tier from its escalation index, saturating at [`ModelTier::Strong`]
    /// so the result is always bounded.
    pub fn from_index(idx: usize) -> ModelTier {
        match idx {
            0 => ModelTier::Fast,
            1 => ModelTier::Balanced,
            _ => ModelTier::Strong,
        }
    }

    /// The env-var role suffix a caller uses to resolve this tier to a concrete
    /// model, e.g. `THEOREMATA_MODEL_<SUFFIX>` (mirrors
    /// [`crate::guard::tier_env_suffix`]).
    pub fn env_suffix(self) -> &'static str {
        match self {
            ModelTier::Fast => "FAST",
            ModelTier::Balanced => "BALANCED",
            ModelTier::Strong => "STRONG",
        }
    }
}

/// Tuning for [`route_model`]. `balanced_at` / `strong_at` are the difficulty
/// thresholds (in `[0, 1]`) at which the base tier steps up, and
/// `escalate_every_attempts` is how many *failed* attempts bump the tier by one
/// rung (`0` disables attempt escalation).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RouteConfig {
    /// Difficulty at or above which an un-escalated goal starts at [`ModelTier::Balanced`].
    #[serde(default = "default_balanced_at")]
    pub balanced_at: f64,
    /// Difficulty at or above which an un-escalated goal starts at [`ModelTier::Strong`].
    #[serde(default = "default_strong_at")]
    pub strong_at: f64,
    /// Failed attempts per one-tier bump. `0` disables attempt-count escalation.
    #[serde(default = "default_escalate_every_attempts")]
    pub escalate_every_attempts: usize,
}

fn default_balanced_at() -> f64 {
    0.34
}

fn default_strong_at() -> f64 {
    0.67
}

fn default_escalate_every_attempts() -> usize {
    2
}

impl Default for RouteConfig {
    fn default() -> Self {
        // Even thirds of the difficulty range, and a one-tier bump every 2 failed
        // attempts (so the third attempt on an easy goal is already Balanced, and a
        // persistently failing goal reaches Strong).
        Self {
            balanced_at: default_balanced_at(),
            strong_at: default_strong_at(),
            escalate_every_attempts: default_escalate_every_attempts(),
        }
    }
}

impl RouteConfig {
    /// The base tier from difficulty alone (before attempt escalation).
    fn base_tier(&self, difficulty: f64) -> ModelTier {
        let d = difficulty.clamp(0.0, 1.0);
        // `strong_at` wins ties so a mis-ordered config (strong_at <= balanced_at)
        // still degrades sanely toward the stronger tier.
        if d >= self.strong_at {
            ModelTier::Strong
        } else if d >= self.balanced_at {
            ModelTier::Balanced
        } else {
            ModelTier::Fast
        }
    }
}

/// Route a goal to a [`ModelTier`] from its `difficulty` estimate (`[0, 1]`,
/// clamped) and how many failed `attempt`s it has already had.
///
/// Properties (all exercised by the unit tests):
/// * **Easy ⇒ Fast, hard ⇒ Strong** — the base tier steps up at the configured
///   difficulty thresholds.
/// * **Attempt escalation is monotone** — each additional failed attempt can only
///   raise the tier, never lower it, one rung per `escalate_every_attempts`.
/// * **Bounded** — the result never exceeds [`ModelTier::Strong`], for any
///   difficulty or attempt count.
/// * **Deterministic** — a pure function of its inputs and `cfg`.
pub fn route_model(difficulty: f64, attempt: usize, cfg: &RouteConfig) -> ModelTier {
    let base = cfg.base_tier(difficulty).as_index();
    let bumps = if cfg.escalate_every_attempts == 0 {
        0
    } else {
        attempt / cfg.escalate_every_attempts
    };
    ModelTier::from_index(base.saturating_add(bumps))
}

/// Why a provider call failed, classified for the fallback ladder. The first
/// three are *retriable* — a transient provider condition where degrading to the
/// next endpoint is worthwhile; [`ProviderErrorKind::InvalidRequest`] is a caller
/// error that the next provider would reject identically, so it stops the ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderErrorKind {
    /// The provider rate-limited the request (HTTP 429 / quota).
    RateLimit,
    /// The call timed out or the connection dropped.
    Timeout,
    /// The provider returned a well-formed but empty/degenerate completion.
    EmptyOutput,
    /// The request itself was rejected as malformed (bad schema, oversize prompt).
    InvalidRequest,
}

impl ProviderErrorKind {
    /// Whether this error justifies degrading to the next ladder endpoint. A
    /// transient/provider-specific failure is retriable; a malformed request is
    /// not (every endpoint would reject it the same way).
    pub fn is_retriable(self) -> bool {
        matches!(
            self,
            ProviderErrorKind::RateLimit
                | ProviderErrorKind::Timeout
                | ProviderErrorKind::EmptyOutput
        )
    }

    /// Classify an opaque provider failure conservatively. Unknown failures are
    /// treated as invalid requests so the ladder does not spray a malformed or
    /// policy-rejected prompt across every configured endpoint.
    pub fn classify_error(error: &anyhow::Error) -> Self {
        let message = format!("{error:#}").to_ascii_lowercase();
        if message.contains("429")
            || message.contains("rate limit")
            || message.contains("rate-limit")
            || message.contains("quota")
        {
            Self::RateLimit
        } else if message.contains("timed out")
            || message.contains("timeout")
            || message.contains("connection reset")
            || message.contains("connection refused")
            || message.contains("network unreachable")
        {
            Self::Timeout
        } else {
            Self::InvalidRequest
        }
    }
}

/// One concrete provider/model endpoint on the fallback ladder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelEndpoint {
    /// Provider identifier (matches [`crate::provider::ModelProvider::name`],
    /// e.g. `"command"`, or a named vendor).
    pub provider: String,
    /// The concrete model name/id the provider should call.
    pub model: String,
    /// The tier this endpoint serves (used to anchor a [`ModelPlan`]).
    pub tier: ModelTier,
}

/// Persisted, opt-in router configuration. It deliberately lives beside the
/// routing algorithm rather than provider credentials: endpoints name a
/// provider/model pair, while each provider owns how its credentials and API
/// are resolved.
///
/// `enabled = false` or an empty endpoint list means callers must use their
/// historical single-provider path. This keeps adding the field to [`Config`]
/// backward compatible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRoutingConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub route: RouteConfig,
    #[serde(default)]
    pub endpoints: Vec<ModelEndpoint>,
}

impl Default for ModelRoutingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            route: RouteConfig::default(),
            endpoints: Vec::new(),
        }
    }
}

impl ModelRoutingConfig {
    /// Build an executable plan only for an explicitly enabled, non-empty
    /// ladder. An absent/default config therefore changes no existing call.
    pub fn plan(&self) -> Option<ModelPlan> {
        (self.enabled && !self.endpoints.is_empty())
            .then(|| ModelPlan::new(self.route, FallbackLadder::new(self.endpoints.clone())))
    }
}

impl ModelEndpoint {
    /// Convenience constructor.
    pub fn new(provider: impl Into<String>, model: impl Into<String>, tier: ModelTier) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            tier,
        }
    }
}

/// The next endpoint to try after a failure: its ladder `index` and a borrow of
/// the [`ModelEndpoint`] itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NextModel<'a> {
    /// The endpoint's position in the ladder.
    pub index: usize,
    /// The endpoint to call next.
    pub endpoint: &'a ModelEndpoint,
}

/// An ordered list of provider/model endpoints, tried top-to-bottom. On a
/// retriable provider error the caller advances one rung; when the ladder is
/// exhausted there is nothing left to try and the attempt genuinely fails.
///
/// The order is fixed at construction, so fallback is fully deterministic.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct FallbackLadder {
    rungs: Vec<ModelEndpoint>,
}

impl FallbackLadder {
    /// Build a ladder from an ordered list of endpoints (index `0` is tried first).
    pub fn new(rungs: Vec<ModelEndpoint>) -> Self {
        Self { rungs }
    }

    /// The endpoints, in fallback order.
    pub fn rungs(&self) -> &[ModelEndpoint] {
        &self.rungs
    }

    /// Number of rungs.
    pub fn len(&self) -> usize {
        self.rungs.len()
    }

    /// Whether the ladder is empty.
    pub fn is_empty(&self) -> bool {
        self.rungs.is_empty()
    }

    /// The endpoint at `index`, if any.
    pub fn get(&self, index: usize) -> Option<&ModelEndpoint> {
        self.rungs.get(index)
    }

    /// Given the `current` rung index and the `error_kind` that just failed it,
    /// decide the next endpoint to try.
    ///
    /// Returns `Some(next)` — the rung at `current + 1` — when the error is
    /// [retriable](ProviderErrorKind::is_retriable) and such a rung exists;
    /// otherwise `None` (a non-retriable error, or the bottom of the ladder is
    /// reached). Deterministic: the same `(current, error_kind)` always yields the
    /// same answer.
    pub fn next_on_failure(
        &self,
        current: usize,
        error_kind: ProviderErrorKind,
    ) -> Option<NextModel<'_>> {
        if !error_kind.is_retriable() {
            return None;
        }
        let next = current.checked_add(1)?;
        self.rungs.get(next).map(|endpoint| NextModel {
            index: next,
            endpoint,
        })
    }

    /// The first rung whose tier is at least `tier`, i.e. where a goal routed to
    /// `tier` should *start*. Falls back to the strongest available rung when no
    /// endpoint reaches `tier`, and to `0` for a non-empty ladder with no match
    /// at all. `None` only for an empty ladder.
    pub fn anchor_for_tier(&self, tier: ModelTier) -> Option<usize> {
        if self.rungs.is_empty() {
            return None;
        }
        if let Some(idx) = self.rungs.iter().position(|e| e.tier >= tier) {
            return Some(idx);
        }
        // No rung reaches the requested tier: use the strongest one available
        // (the rung with the highest tier; `max_by_key` is deterministic).
        let (best_idx, _) = self
            .rungs
            .iter()
            .enumerate()
            .max_by_key(|(_, e)| e.tier)
            .expect("non-empty ladder has a max");
        Some(best_idx)
    }
}

/// A concrete model choice for a goal: the routed [`ModelTier`] and the ordered
/// sequence of ladder indices to try — `order[0]` is the primary endpoint, the
/// rest are the fallbacks walked on successive provider errors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlannedSelection {
    /// The tier the goal was routed to.
    pub tier: ModelTier,
    /// Ladder indices, primary first, then the fallback sequence. Empty only when
    /// the ladder itself is empty.
    pub order: Vec<usize>,
}

impl PlannedSelection {
    /// The primary rung index (the first endpoint to call), if any.
    pub fn primary(&self) -> Option<usize> {
        self.order.first().copied()
    }
}

/// Fuses [`route_model`] with a [`FallbackLadder`]: for a `(difficulty, attempt)`
/// it routes to a tier, anchors the ladder at that tier, and produces the primary
/// endpoint plus the ordered fallback sequence.
#[derive(Debug, Clone)]
pub struct ModelPlan {
    cfg: RouteConfig,
    ladder: FallbackLadder,
}

impl ModelPlan {
    /// Build a plan from a routing config and a fallback ladder.
    pub fn new(cfg: RouteConfig, ladder: FallbackLadder) -> Self {
        Self { cfg, ladder }
    }

    /// The routing config.
    pub fn config(&self) -> &RouteConfig {
        &self.cfg
    }

    /// The fallback ladder.
    pub fn ladder(&self) -> &FallbackLadder {
        &self.ladder
    }

    /// The tier this goal routes to (before ladder anchoring).
    pub fn tier(&self, difficulty: f64, attempt: usize) -> ModelTier {
        route_model(difficulty, attempt, &self.cfg)
    }

    /// Plan the model choice for a goal: route to a tier, anchor the ladder at the
    /// first endpoint serving that tier, and return that rung plus every rung
    /// below it as the deterministic fallback sequence.
    ///
    /// Deterministic and bounded: `order` is a contiguous suffix of the ladder
    /// (so `order.len() <= ladder.len()`), and identical inputs reproduce it.
    pub fn select(&self, difficulty: f64, attempt: usize) -> PlannedSelection {
        let tier = self.tier(difficulty, attempt);
        let order = match self.ladder.anchor_for_tier(tier) {
            Some(start) => (start..self.ladder.len()).collect(),
            None => Vec::new(),
        };
        PlannedSelection { tier, order }
    }

    /// Resolve a ladder index to its endpoint (convenience for callers driving the
    /// [`PlannedSelection::order`]).
    pub fn endpoint(&self, index: usize) -> Option<&ModelEndpoint> {
        self.ladder.get(index)
    }
}

/// Successful result of an endpoint-aware provider call.
#[derive(Debug, Clone)]
pub struct RoutedResponse {
    pub response: ModelResponse,
    pub endpoint: ModelEndpoint,
    pub tier: ModelTier,
    /// Number of endpoints attempted, including the successful one.
    pub attempts: usize,
}

/// Execute an opt-in model plan against one provider implementation.
///
/// The provider must own the selected endpoint name. A provider registry can
/// later dispatch multiple implementations; this narrow API refuses a mismatch
/// rather than silently routing a request to the wrong vendor. Each request is
/// passed through [`ModelProvider::complete_at`], which places the exact
/// endpoint in `ModelRequest.context` for command-backed providers.
///
/// Only rate-limit, timeout, and empty-output failures advance the ladder.
/// Everything else fails immediately, preserving request safety and avoiding
/// duplicate work for deterministic errors.
pub fn execute_with_fallback(
    provider: &dyn ModelProvider,
    request: &ModelRequest,
    plan: &ModelPlan,
    difficulty: f64,
    attempt: usize,
) -> Result<RoutedResponse> {
    let selection = plan.select(difficulty, attempt);
    if selection.order.is_empty() {
        return Err(anyhow!(
            "model routing is enabled but its endpoint ladder is empty"
        ));
    }

    let mut last_error = None;
    for (calls, index) in selection.order.iter().copied().enumerate() {
        let endpoint = plan
            .endpoint(index)
            .expect("ModelPlan selections always reference its ladder");
        if endpoint.provider != provider.name() {
            return Err(anyhow!(
                "configured endpoint provider '{}' is unavailable on provider '{}'",
                endpoint.provider,
                provider.name()
            ));
        }

        match provider.complete_at(endpoint, request) {
            Ok(response) if response_is_empty(&response.content) => {
                let error = anyhow!("model provider returned empty output");
                last_error = Some(error);
                if plan
                    .ladder()
                    .next_on_failure(index, ProviderErrorKind::EmptyOutput)
                    .is_none()
                {
                    break;
                }
            }
            Ok(response) => {
                return Ok(RoutedResponse {
                    response,
                    endpoint: endpoint.clone(),
                    tier: selection.tier,
                    attempts: calls + 1,
                });
            }
            Err(error) => {
                let kind = ProviderErrorKind::classify_error(&error);
                last_error = Some(error);
                if plan.ladder().next_on_failure(index, kind).is_none() {
                    break;
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow!("model routing exhausted without an endpoint call")))
}

fn response_is_empty(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => true,
        serde_json::Value::String(text) => text.trim().is_empty(),
        serde_json::Value::Array(values) => values.is_empty(),
        serde_json::Value::Object(values) => {
            values.is_empty() || values.values().all(response_is_empty)
        }
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn ladder() -> FallbackLadder {
        FallbackLadder::new(vec![
            ModelEndpoint::new("command", "fast-a", ModelTier::Fast),
            ModelEndpoint::new("command", "fast-b", ModelTier::Fast),
            ModelEndpoint::new("vendor", "balanced-a", ModelTier::Balanced),
            ModelEndpoint::new("vendor", "strong-a", ModelTier::Strong),
        ])
    }

    #[test]
    fn easy_routes_fast_hard_routes_strong() {
        let cfg = RouteConfig::default();
        assert_eq!(route_model(0.0, 0, &cfg), ModelTier::Fast);
        assert_eq!(route_model(0.1, 0, &cfg), ModelTier::Fast);
        assert_eq!(route_model(0.5, 0, &cfg), ModelTier::Balanced);
        assert_eq!(route_model(0.9, 0, &cfg), ModelTier::Strong);
        assert_eq!(route_model(1.0, 0, &cfg), ModelTier::Strong);
    }

    #[test]
    fn difficulty_is_clamped() {
        let cfg = RouteConfig::default();
        // Out-of-range inputs clamp rather than mis-route or panic.
        assert_eq!(route_model(-5.0, 0, &cfg), ModelTier::Fast);
        assert_eq!(route_model(42.0, 0, &cfg), ModelTier::Strong);
    }

    #[test]
    fn repeated_attempts_escalate_the_tier_monotonically() {
        let cfg = RouteConfig::default(); // one bump per 2 attempts
        let mut prev = route_model(0.0, 0, &cfg);
        for attempt in 0..12 {
            let t = route_model(0.0, attempt, &cfg);
            assert!(
                t.as_index() >= prev.as_index(),
                "tier must not drop as attempts rise (attempt {attempt}: {t:?} < {prev:?})"
            );
            prev = t;
        }
        // An easy goal that keeps failing eventually reaches the strongest tier…
        assert_eq!(route_model(0.0, 0, &cfg), ModelTier::Fast);
        assert_eq!(route_model(0.0, 2, &cfg), ModelTier::Balanced);
        assert_eq!(route_model(0.0, 4, &cfg), ModelTier::Strong);
        // …and never exceeds it, however many attempts.
        assert_eq!(route_model(0.0, 1_000, &cfg), ModelTier::Strong);
        assert_eq!(route_model(1.0, 1_000, &cfg), ModelTier::Strong);
    }

    #[test]
    fn escalation_can_be_disabled() {
        let cfg = RouteConfig {
            escalate_every_attempts: 0,
            ..RouteConfig::default()
        };
        // With escalation off, the tier depends on difficulty alone.
        assert_eq!(route_model(0.0, 99, &cfg), ModelTier::Fast);
    }

    #[test]
    fn ladder_advances_on_each_failure_and_stops_at_the_end() {
        let l = ladder();
        // Walk from the top, advancing on a retriable error each time.
        let step0 = l.next_on_failure(0, ProviderErrorKind::Timeout).unwrap();
        assert_eq!(step0.index, 1);
        assert_eq!(step0.endpoint.model, "fast-b");
        let step1 = l.next_on_failure(1, ProviderErrorKind::RateLimit).unwrap();
        assert_eq!(step1.index, 2);
        let step2 = l
            .next_on_failure(2, ProviderErrorKind::EmptyOutput)
            .unwrap();
        assert_eq!(step2.index, 3);
        // Past the last rung there is nothing left to try.
        assert!(l.next_on_failure(3, ProviderErrorKind::Timeout).is_none());
        assert!(l.next_on_failure(99, ProviderErrorKind::Timeout).is_none());
    }

    #[test]
    fn rate_limit_and_timeout_both_advance() {
        let l = ladder();
        assert_eq!(
            l.next_on_failure(0, ProviderErrorKind::RateLimit)
                .unwrap()
                .index,
            1
        );
        assert_eq!(
            l.next_on_failure(0, ProviderErrorKind::Timeout)
                .unwrap()
                .index,
            1
        );
        assert_eq!(
            l.next_on_failure(0, ProviderErrorKind::EmptyOutput)
                .unwrap()
                .index,
            1
        );
    }

    #[test]
    fn non_retriable_error_stops_the_ladder() {
        let l = ladder();
        // A malformed request would fail identically on every provider: no advance.
        assert!(l
            .next_on_failure(0, ProviderErrorKind::InvalidRequest)
            .is_none());
    }

    #[test]
    fn empty_ladder_never_advances() {
        let l = FallbackLadder::default();
        assert!(l.is_empty());
        assert!(l.next_on_failure(0, ProviderErrorKind::Timeout).is_none());
        assert_eq!(l.anchor_for_tier(ModelTier::Fast), None);
    }

    #[test]
    fn plan_anchors_at_the_routed_tier_and_lists_fallbacks() {
        let plan = ModelPlan::new(RouteConfig::default(), ladder());
        // Easy goal → Fast → starts at rung 0, fallback sequence is the whole ladder.
        let easy = plan.select(0.0, 0);
        assert_eq!(easy.tier, ModelTier::Fast);
        assert_eq!(easy.order, vec![0, 1, 2, 3]);
        assert_eq!(
            plan.endpoint(easy.primary().unwrap()).unwrap().model,
            "fast-a"
        );

        // Balanced goal → anchors at the first Balanced rung (index 2), then below.
        let mid = plan.select(0.5, 0);
        assert_eq!(mid.tier, ModelTier::Balanced);
        assert_eq!(mid.order, vec![2, 3]);

        // Hard goal → Strong → anchors at the strong rung (index 3), no fallbacks.
        let hard = plan.select(0.95, 0);
        assert_eq!(hard.tier, ModelTier::Strong);
        assert_eq!(hard.order, vec![3]);
    }

    #[test]
    fn plan_anchor_falls_back_to_strongest_when_tier_unavailable() {
        // A ladder with no Strong rung: a Strong-routed goal still gets the best
        // available (the Balanced rung), never an empty order.
        let l = FallbackLadder::new(vec![
            ModelEndpoint::new("command", "fast", ModelTier::Fast),
            ModelEndpoint::new("vendor", "balanced", ModelTier::Balanced),
        ]);
        let plan = ModelPlan::new(RouteConfig::default(), l);
        let hard = plan.select(1.0, 0);
        assert_eq!(hard.tier, ModelTier::Strong);
        assert_eq!(hard.order, vec![1]); // anchored at the strongest present
    }

    #[test]
    fn selection_order_is_a_bounded_suffix_of_the_ladder() {
        let plan = ModelPlan::new(RouteConfig::default(), ladder());
        for &d in &[0.0, 0.4, 0.7, 1.0] {
            for attempt in 0..6 {
                let sel = plan.select(d, attempt);
                assert!(sel.order.len() <= plan.ladder().len());
                // Contiguous suffix: strictly increasing, ending at the last rung.
                if let Some(&first) = sel.order.first() {
                    let expected: Vec<usize> = (first..plan.ladder().len()).collect();
                    assert_eq!(sel.order, expected);
                }
            }
        }
    }

    #[test]
    fn routing_and_planning_are_deterministic() {
        let cfg = RouteConfig::default();
        let route = |()| route_model(0.55, 3, &cfg);
        assert_eq!(route(()), route(()), "route_model must reproduce");

        let plan = ModelPlan::new(cfg, ladder());
        assert_eq!(
            plan.select(0.55, 3),
            plan.select(0.55, 3),
            "select must reproduce"
        );

        let l = ladder();
        let walk = || {
            let mut idx = 0usize;
            let mut path = vec![idx];
            while let Some(n) = l.next_on_failure(idx, ProviderErrorKind::Timeout) {
                idx = n.index;
                path.push(idx);
            }
            path
        };
        assert_eq!(walk(), walk(), "ladder walk must reproduce");
        assert_eq!(walk(), vec![0, 1, 2, 3]);
    }

    struct ScriptedProvider {
        outcomes: RefCell<Vec<Result<ModelResponse>>>,
        seen_models: RefCell<Vec<String>>,
    }

    impl ScriptedProvider {
        fn new(outcomes: Vec<Result<ModelResponse>>) -> Self {
            Self {
                outcomes: RefCell::new(outcomes),
                seen_models: RefCell::new(Vec::new()),
            }
        }
    }

    impl ModelProvider for ScriptedProvider {
        fn complete(&self, request: &ModelRequest) -> Result<ModelResponse> {
            self.seen_models.borrow_mut().push(
                request.context["model_endpoint"]["model"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            );
            self.outcomes.borrow_mut().remove(0)
        }

        fn name(&self) -> &str {
            "scripted"
        }
    }

    fn request() -> ModelRequest {
        ModelRequest {
            role: "test".into(),
            task: "route endpoint".into(),
            context: serde_json::json!({"goal": "True"}),
            output_schema: serde_json::json!({}),
        }
    }

    fn scripted_plan() -> ModelPlan {
        ModelPlan::new(
            RouteConfig {
                escalate_every_attempts: 0,
                ..RouteConfig::default()
            },
            FallbackLadder::new(vec![
                ModelEndpoint::new("scripted", "first", ModelTier::Fast),
                ModelEndpoint::new("scripted", "second", ModelTier::Fast),
            ]),
        )
    }

    fn response(content: serde_json::Value) -> ModelResponse {
        ModelResponse {
            content,
            model: "test".into(),
            provider: "scripted".into(),
        }
    }

    #[test]
    fn execution_retries_timeout_and_carries_the_selected_endpoint() {
        let provider = ScriptedProvider::new(vec![
            Err(anyhow!("request timed out")),
            Ok(response(serde_json::json!({"proof": "by trivial"}))),
        ]);
        let routed = execute_with_fallback(&provider, &request(), &scripted_plan(), 0.0, 0)
            .expect("timeout should fall through to the next endpoint");
        assert_eq!(routed.endpoint.model, "second");
        assert_eq!(routed.attempts, 2);
        assert_eq!(provider.seen_models.into_inner(), vec!["first", "second"]);
    }

    #[test]
    fn execution_retries_empty_output_but_not_invalid_requests() {
        let empty = ScriptedProvider::new(vec![
            Ok(response(serde_json::json!({"message": "  "}))),
            Ok(response(serde_json::json!("usable"))),
        ]);
        let routed = execute_with_fallback(&empty, &request(), &scripted_plan(), 0.0, 0)
            .expect("empty output should retry");
        assert_eq!(routed.endpoint.model, "second");

        let invalid = ScriptedProvider::new(vec![
            Err(anyhow!("request schema is malformed")),
            Ok(response(serde_json::json!("must not run"))),
        ]);
        assert!(execute_with_fallback(&invalid, &request(), &scripted_plan(), 0.0, 0).is_err());
        assert_eq!(invalid.seen_models.into_inner(), vec!["first"]);
    }

    #[test]
    fn disabled_or_empty_routing_config_has_no_plan() {
        assert!(ModelRoutingConfig::default().plan().is_none());
        assert!(ModelRoutingConfig {
            enabled: true,
            ..ModelRoutingConfig::default()
        }
        .plan()
        .is_none());
    }
}
