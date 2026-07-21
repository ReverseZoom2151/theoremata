//! Per-obligation router (plan §2).
//!
//! An inspectable state machine that decides what to do next with a node,
//! encoding the *falsify-before-prove* policy: spend the cheap counterexample
//! check as a gate before any expensive proof effort, so a refuted branch dies
//! immediately. The router is pure — it reasons over a node, a compact summary
//! of its history (`NodeSignals`), and which tools are available — so the
//! decision is auditable and testable in isolation.

use crate::informal_defect_prior::RoutingHints;
use crate::model::{Node, NodeKind};

/// The next action to take on a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    Falsify,
    Retrieve,
    Prove,
    Decompose,
    Formalize,
    Verify,
    Escalate,
}

/// Which capabilities are currently available to act on a node.
#[derive(Debug, Clone, Copy)]
pub struct ToolAvailability {
    pub python: bool,
    pub lean: bool,
    /// Whether the configured target formal system has a live verifier.
    pub formal_verifier: bool,
    pub mathlib_search: bool,
    pub model: bool,
    pub external_prover: bool,
}

/// A compact summary of a node's history that the caller derives from the
/// graph (evidence, attempts, edges) so the router itself stays pure.
#[derive(Debug, Clone, Copy, Default)]
pub struct NodeSignals {
    pub falsified: bool,
    pub counterexample_found: bool,
    pub retrieved: bool,
    pub has_formal_statement: bool,
    pub attempts: u32,
}

/// True if the node's claim admits a bounded/computational check worth running
/// as a cheap gate before proof effort.
fn admits_falsification(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Conjecture | NodeKind::Computation | NodeKind::Obligation
    )
}

/// Decide the next action for `node`, in strict priority order. See the module
/// docs; the ordering is the policy.
///
/// This is the hint-free entry point and is exactly equivalent to
/// [`route_with_hints`] called with `None`.
pub fn route(
    node: &Node,
    signals: &NodeSignals,
    tools: &ToolAvailability,
    max_attempts: u32,
) -> Route {
    base_route(node, signals, tools, max_attempts)
}

/// The unmodified priority chain. Both entry points funnel through here so the
/// hint path can never drift from the base policy.
fn base_route(
    node: &Node,
    signals: &NodeSignals,
    tools: &ToolAvailability,
    max_attempts: u32,
) -> Route {
    // 1. Bounded effort: too many attempts → hand to a human.
    if signals.attempts >= max_attempts {
        return Route::Escalate;
    }

    // 2. Falsify before prove: run the cheap counterexample gate first.
    if admits_falsification(node.kind)
        && !signals.falsified
        && !signals.counterexample_found
        && tools.python
    {
        return Route::Falsify;
    }

    // 3. A refuted claim cannot be proved; it needs graph repair / a human.
    if signals.counterexample_found {
        return Route::Escalate;
    }

    // 4. Gather candidate lemmas before attempting a proof.
    if !signals.retrieved
        && tools.mathlib_search
        && matches!(node.kind, NodeKind::Obligation | NodeKind::Lemma)
    {
        return Route::Retrieve;
    }

    // 5. High-level nodes are decomposed into obligations *before* any proof
    //    effort: decompose the theorem, formalize the leaves.
    if matches!(node.kind, NodeKind::Conjecture | NodeKind::Strategy) && tools.model {
        return Route::Decompose;
    }

    // 6. Has a formal statement → verify it with the configured native backend.
    if (signals.has_formal_statement || node.formal_statement.is_some()) && tools.formal_verifier {
        return Route::Verify;
    }

    // 7. Informal leaf but not yet formal → formalize against the configured
    //    native verifier (not necessarily Lean).
    if !signals.has_formal_statement
        && node.formal_statement.is_none()
        && tools.formal_verifier
        && tools.model
    {
        return Route::Formalize;
    }

    // 8. Attempt proof via external prover and/or model-backed backends.
    if tools.external_prover || tools.model {
        Route::Prove
    } else {
        Route::Escalate
    }
}

// ---------------------------------------------------------------------------
// Hint-aware routing
// ---------------------------------------------------------------------------

/// The minimum region weight that counts as "high risk" for routing purposes.
///
/// Region weights are sums of per-finding prior weights from
/// [`crate::informal_defect_prior`], and those weights are a prior from a
/// SINGLE diffed case study (n = 1) — read that module's header before touching
/// this number. `0.7` is the weight of the lightest single finite-check
/// pattern, so a lone low-confidence hand-wave does not qualify but the
/// case-study headline defect does.
pub const HIGH_RISK_REGION_WEIGHT: f64 = 0.7;

/// A node's position in the scanned informal text, paired with the hints that
/// scan produced.
///
/// `span` is a `[start, end)` byte range into the SAME text the hints were
/// computed over (in the sketch pipeline that is the newline-joined statement +
/// step prose, not any single step's prose). Get it wrong and the worst case is
/// a wasted or missed bias — never an unsound decision, by the invariant below.
#[derive(Debug, Clone, Copy)]
pub struct HintContext<'a> {
    pub hints: &'a RoutingHints,
    pub span: (usize, usize),
}

impl HintContext<'_> {
    /// The route preferred by the heaviest high-risk region overlapping
    /// `span`, or `None` if no such region exists.
    ///
    /// Deterministic: strictly-greater weight wins, so on a tie the
    /// falsify bucket (scanned first) is preferred — consistent with the
    /// falsify-before-prove policy.
    fn preferred_route(&self) -> Option<Route> {
        let (start, end) = self.span;
        let mut best: Option<(f64, Route)> = None;
        let buckets = [
            (&self.hints.falsify_first, Route::Falsify),
            (&self.hints.decompose_first, Route::Decompose),
        ];
        for (regions, bucket_route) in buckets {
            for r in regions.iter() {
                // Half-open overlap. A zero-length node span never overlaps.
                if r.weight < HIGH_RISK_REGION_WEIGHT || r.start >= end || start >= r.end {
                    continue;
                }
                // Trust the region's own route field; the bucket is a
                // consistency check only.
                debug_assert_eq!(r.route, bucket_route);
                let better = match best {
                    Some((w, _)) => r.weight > w,
                    None => true,
                };
                if better {
                    best = Some((r.weight, r.route));
                }
            }
        }
        best.map(|(_, route)| route)
    }
}

/// Whether [`Route::Falsify`] is a PERMITTED action for this node, independent
/// of where it sits in the priority chain: the kind admits a computational
/// check, the claim is not already refuted, and Python is available.
///
/// Note this is deliberately weaker than the step-2 guard, which additionally
/// requires `!signals.falsified` (run the gate once). Re-probing an already
/// probed node is a permitted action; it just is not what the base chain picks.
fn falsify_permitted(node: &Node, signals: &NodeSignals, tools: &ToolAvailability) -> bool {
    admits_falsification(node.kind) && !signals.counterexample_found && tools.python
}

/// Whether [`Route::Decompose`] is a PERMITTED action for this node — exactly
/// the step-5 guard. Decomposition is never permitted for a leaf kind, so a
/// decompose hint can never turn an obligation into a decomposition.
fn decompose_permitted(node: &Node, tools: &ToolAvailability) -> bool {
    matches!(node.kind, NodeKind::Conjecture | NodeKind::Strategy) && tools.model
}

/// Decide the next action for `node`, optionally biased by the informal-defect
/// routing hints for the region of text this node came from.
///
/// # Invariant (this is the load-bearing part)
///
/// Hints may only REORDER among actions the base policy already permits for
/// this node. Concretely, for every input:
///
/// 1. `route_with_hints(.., None) == route(..)`, byte for byte.
/// 2. The result is always either the base route, or a route that passes its
///    own `*_permitted` predicate — hints never enable an action forbidden by
///    node kind, tool availability, or an existing refutation.
/// 3. Falsification is never skipped: if the base route is [`Route::Falsify`],
///    hints are ignored.
/// 4. [`Route::Escalate`] is never overridden — neither the bounded-effort cap
///    nor the "refuted claim" escalation can be hinted away.
/// 5. Hints can only ever yield [`Route::Falsify`] or [`Route::Decompose`].
///    They can never yield [`Route::Verify`] or [`Route::Prove`], so no node
///    can be pushed toward certification by a hint.
/// 6. `overall_risk` is never read here. Only per-region evidence that overlaps
///    THIS node's span can move a decision; a globally scary document does not
///    reroute anything.
///
/// # Current reach
///
/// Under today's priority chain the decompose bias can only ever CONFIRM the
/// base route: whenever `decompose_permitted` holds, step 5 has already chosen
/// [`Route::Decompose`] unless an earlier, non-overridable step won. It is
/// implemented symmetrically anyway so that it stays correct if the chain is
/// reordered. The falsify bias does bite: it re-probes a high-risk node that
/// has already had one falsification pass.
pub fn route_with_hints(
    node: &Node,
    signals: &NodeSignals,
    tools: &ToolAvailability,
    max_attempts: u32,
    hints: Option<&HintContext<'_>>,
) -> Route {
    let base = base_route(node, signals, tools, max_attempts);

    let Some(ctx) = hints else {
        return base;
    };

    // Invariants 3 and 4: never skip a falsification, never un-escalate.
    if matches!(base, Route::Falsify | Route::Escalate) {
        return base;
    }

    match ctx.preferred_route() {
        Some(Route::Falsify) if falsify_permitted(node, signals, tools) => Route::Falsify,
        Some(Route::Decompose) if decompose_permitted(node, tools) => Route::Decompose,
        _ => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NodeStatus, NodeTier, Taint};
    use chrono::Utc;

    fn node(kind: NodeKind, formal: Option<&str>) -> Node {
        let now = Utc::now();
        Node {
            id: "n".into(),
            project_id: "p".into(),
            kind,
            status: NodeStatus::Active,
            title: "t".into(),
            statement: "s".into(),
            formal_statement: formal.map(str::to_owned),
            provenance: "test".into(),
            content_hash: "hash".into(),
            tainted: false,
            taint: Taint::Clean,
            tier: NodeTier::Spine,
            parent_id: None,
            strategy_hint: None,
            suggested_lemmas: Vec::new(),
            lean_decls: Vec::new(),
            stmt_formalized: false,
            proof_done: false,
            created_at: now,
            updated_at: now,
        }
    }

    const ALL: ToolAvailability = ToolAvailability {
        python: true,
        lean: true,
        formal_verifier: true,
        mathlib_search: true,
        model: true,
        external_prover: true,
    };

    #[test]
    fn fresh_computation_falsifies_first() {
        let n = node(NodeKind::Computation, None);
        assert_eq!(route(&n, &NodeSignals::default(), &ALL, 5), Route::Falsify);
    }

    #[test]
    fn counterexample_escalates() {
        let n = node(NodeKind::Obligation, None);
        let sig = NodeSignals {
            falsified: true,
            counterexample_found: true,
            ..Default::default()
        };
        assert_eq!(route(&n, &sig, &ALL, 5), Route::Escalate);
    }

    #[test]
    fn obligation_retrieves_after_falsify() {
        let n = node(NodeKind::Obligation, None);
        let sig = NodeSignals {
            falsified: true,
            ..Default::default()
        };
        assert_eq!(route(&n, &sig, &ALL, 5), Route::Retrieve);
    }

    #[test]
    fn exhausted_attempts_escalate() {
        let n = node(NodeKind::Obligation, None);
        let sig = NodeSignals {
            attempts: 5,
            ..Default::default()
        };
        assert_eq!(route(&n, &sig, &ALL, 5), Route::Escalate);
    }

    #[test]
    fn formal_statement_verifies() {
        let n = node(
            NodeKind::FormalStatement,
            Some("theorem t : True := trivial"),
        );
        let sig = NodeSignals {
            falsified: true,
            retrieved: true,
            has_formal_statement: true,
            ..Default::default()
        };
        assert_eq!(route(&n, &sig, &ALL, 5), Route::Verify);
    }

    #[test]
    fn informal_obligation_formalizes() {
        let n = node(NodeKind::Obligation, None);
        let sig = NodeSignals {
            falsified: true,
            retrieved: true,
            ..Default::default()
        };
        assert_eq!(route(&n, &sig, &ALL, 5), Route::Formalize);
    }

    #[test]
    fn formal_statement_verifies_without_lean_when_native_backend_exists() {
        let n = node(
            NodeKind::FormalStatement,
            Some("Theorem t : True. Proof. exact I. Qed."),
        );
        let sig = NodeSignals {
            falsified: true,
            retrieved: true,
            has_formal_statement: true,
            ..Default::default()
        };
        let tools = ToolAvailability { lean: false, ..ALL };
        assert_eq!(route(&n, &sig, &tools, 5), Route::Verify);
    }

    // -----------------------------------------------------------------------
    // Hint-aware routing
    // -----------------------------------------------------------------------

    use crate::informal_defect_prior::{DefectCategory, RiskRegion};

    /// The node's span in the scanned text, used by every hint test below.
    const SPAN: (usize, usize) = (100, 200);

    fn region(start: usize, end: usize, weight: f64, route: Route) -> RiskRegion {
        RiskRegion {
            start,
            end,
            weight,
            categories: vec![if route == Route::Falsify {
                DefectCategory::HandWavedFiniteCheck
            } else {
                DefectCategory::StandardEstimate
            }],
            finding_indices: vec![0],
            route,
        }
    }

    /// Hints with one high-risk falsify-preferring region overlapping `SPAN`.
    fn falsify_hints() -> RoutingHints {
        RoutingHints {
            falsify_first: vec![region(120, 160, 1.0, Route::Falsify)],
            decompose_first: Vec::new(),
            overall_risk: 0.8,
        }
    }

    /// Hints with one high-risk decompose-preferring region overlapping `SPAN`.
    fn decompose_hints() -> RoutingHints {
        RoutingHints {
            falsify_first: Vec::new(),
            decompose_first: vec![region(120, 160, 1.0, Route::Decompose)],
            overall_risk: 0.8,
        }
    }

    /// Every kind the policy distinguishes, plus a couple it does not.
    const KINDS: &[NodeKind] = &[
        NodeKind::Conjecture,
        NodeKind::Definition,
        NodeKind::Assumption,
        NodeKind::Strategy,
        NodeKind::Lemma,
        NodeKind::Obligation,
        NodeKind::Computation,
        NodeKind::Counterexample,
        NodeKind::InformalProof,
        NodeKind::FormalStatement,
        NodeKind::FormalProof,
        NodeKind::Evidence,
    ];

    /// All 2^5 signal combinations x a few attempt counts.
    fn signal_matrix() -> Vec<NodeSignals> {
        let mut out = Vec::new();
        for bits in 0u8..32 {
            for attempts in [0u32, 2, 5] {
                out.push(NodeSignals {
                    falsified: bits & 1 != 0,
                    counterexample_found: bits & 2 != 0,
                    retrieved: bits & 4 != 0,
                    has_formal_statement: bits & 8 != 0,
                    attempts: if bits & 16 != 0 { attempts } else { 0 },
                });
            }
        }
        out
    }

    /// All 2^6 tool combinations.
    fn tool_matrix() -> Vec<ToolAvailability> {
        (0u8..64)
            .map(|b| ToolAvailability {
                python: b & 1 != 0,
                lean: b & 2 != 0,
                formal_verifier: b & 4 != 0,
                mathlib_search: b & 8 != 0,
                model: b & 16 != 0,
                external_prover: b & 32 != 0,
            })
            .collect()
    }

    /// HARD CONSTRAINT: with no hints, the new entry point is the old one.
    #[test]
    fn route_and_route_with_hints_none_agree_across_the_matrix() {
        let mut checked = 0usize;
        for &kind in KINDS {
            for formal in [None, Some("theorem t : True := trivial")] {
                let n = node(kind, formal);
                for sig in signal_matrix() {
                    for tools in tool_matrix() {
                        assert_eq!(
                            route(&n, &sig, &tools, 5),
                            route_with_hints(&n, &sig, &tools, 5, None),
                            "hint-free divergence: {kind:?} formal={formal:?} {sig:?} {tools:?}"
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 10_000, "matrix was too small: {checked}");
    }

    /// A falsify-preferring hint re-probes a high-risk node whose one
    /// falsification pass is already done (base route would be Retrieve).
    #[test]
    fn falsify_hint_moves_a_node_toward_falsify_when_falsify_is_permitted() {
        let n = node(NodeKind::Obligation, None);
        let sig = NodeSignals {
            falsified: true,
            ..Default::default()
        };
        assert_eq!(route(&n, &sig, &ALL, 5), Route::Retrieve);

        let hints = falsify_hints();
        let ctx = HintContext {
            hints: &hints,
            span: SPAN,
        };
        assert_eq!(
            route_with_hints(&n, &sig, &ALL, 5, Some(&ctx)),
            Route::Falsify
        );
    }

    /// The same hint is inert when falsification is not a permitted action.
    #[test]
    fn falsify_hint_does_not_move_a_node_when_falsify_is_illegal() {
        let hints = falsify_hints();
        let ctx = HintContext {
            hints: &hints,
            span: SPAN,
        };

        // (a) Tool unavailable: no Python, no falsification, whatever the hint says.
        let n = node(NodeKind::Obligation, None);
        let sig = NodeSignals {
            falsified: true,
            ..Default::default()
        };
        let no_python = ToolAvailability {
            python: false,
            ..ALL
        };
        assert_eq!(
            route_with_hints(&n, &sig, &no_python, 5, Some(&ctx)),
            route(&n, &sig, &no_python, 5)
        );

        // (b) Kind does not admit falsification.
        let lemma = node(NodeKind::Lemma, None);
        assert_eq!(
            route_with_hints(&lemma, &sig, &ALL, 5, Some(&ctx)),
            route(&lemma, &sig, &ALL, 5)
        );

        // (c) Already refuted: escalation stands, and Falsify is not permitted.
        let refuted = NodeSignals {
            falsified: true,
            counterexample_found: true,
            ..Default::default()
        };
        assert_eq!(
            route_with_hints(&n, &refuted, &ALL, 5, Some(&ctx)),
            Route::Escalate
        );
    }

    /// A decompose-preferring hint confirms Decompose where it is permitted,
    /// and is inert where it is not.
    #[test]
    fn decompose_hint_only_applies_where_decompose_is_permitted() {
        let hints = decompose_hints();
        let ctx = HintContext {
            hints: &hints,
            span: SPAN,
        };

        // Permitted: a conjecture whose falsification pass is done.
        let conj = node(NodeKind::Conjecture, None);
        let sig = NodeSignals {
            falsified: true,
            ..Default::default()
        };
        assert_eq!(route(&conj, &sig, &ALL, 5), Route::Decompose);
        assert_eq!(
            route_with_hints(&conj, &sig, &ALL, 5, Some(&ctx)),
            Route::Decompose
        );

        // Not permitted (leaf kind): the hint must not manufacture a decomposition.
        let obl = node(NodeKind::Obligation, None);
        assert_eq!(
            route_with_hints(&obl, &sig, &ALL, 5, Some(&ctx)),
            route(&obl, &sig, &ALL, 5)
        );

        // Not permitted (no model): likewise.
        let no_model = ToolAvailability {
            model: false,
            ..ALL
        };
        assert_eq!(
            route_with_hints(&conj, &sig, &no_model, 5, Some(&ctx)),
            route(&conj, &sig, &no_model, 5)
        );
    }

    /// `overall_risk` carries no routing authority on its own.
    #[test]
    fn overall_risk_alone_never_changes_a_route() {
        let hints = RoutingHints {
            falsify_first: Vec::new(),
            decompose_first: Vec::new(),
            overall_risk: 0.999,
        };
        let ctx = HintContext {
            hints: &hints,
            span: SPAN,
        };
        for &kind in KINDS {
            let n = node(kind, None);
            for sig in signal_matrix() {
                for tools in tool_matrix() {
                    assert_eq!(
                        route(&n, &sig, &tools, 5),
                        route_with_hints(&n, &sig, &tools, 5, Some(&ctx)),
                        "overall_risk moved a route: {kind:?} {sig:?} {tools:?}"
                    );
                }
            }
        }
    }

    /// A high-risk region that does not overlap the node, and an overlapping
    /// region below the high-risk threshold, are both inert.
    #[test]
    fn only_overlapping_high_risk_regions_bias() {
        let n = node(NodeKind::Obligation, None);
        let sig = NodeSignals {
            falsified: true,
            ..Default::default()
        };
        let base = route(&n, &sig, &ALL, 5);

        let disjoint = RoutingHints {
            falsify_first: vec![region(0, 50, 1.0, Route::Falsify)],
            decompose_first: Vec::new(),
            overall_risk: 0.8,
        };
        let ctx = HintContext {
            hints: &disjoint,
            span: SPAN,
        };
        assert_eq!(route_with_hints(&n, &sig, &ALL, 5, Some(&ctx)), base);

        let too_light = RoutingHints {
            falsify_first: vec![region(120, 160, 0.35, Route::Falsify)],
            decompose_first: Vec::new(),
            overall_risk: 0.8,
        };
        let ctx = HintContext {
            hints: &too_light,
            span: SPAN,
        };
        assert_eq!(route_with_hints(&n, &sig, &ALL, 5, Some(&ctx)), base);
    }

    /// The documented safety invariant, asserted exhaustively: a hint never
    /// enables a forbidden action, never skips falsification, never
    /// un-escalates, and never pushes a node toward certification.
    #[test]
    fn hints_never_widen_the_legal_route_set() {
        let mut moved = 0usize;
        for hints in [falsify_hints(), decompose_hints()] {
            let ctx = HintContext {
                hints: &hints,
                span: SPAN,
            };
            for &kind in KINDS {
                for formal in [None, Some("theorem t : True := trivial")] {
                    let n = node(kind, formal);
                    for sig in signal_matrix() {
                        for tools in tool_matrix() {
                            let base = route(&n, &sig, &tools, 5);
                            let hinted = route_with_hints(&n, &sig, &tools, 5, Some(&ctx));
                            if hinted == base {
                                continue;
                            }
                            moved += 1;

                            // Never skip falsification; never un-escalate.
                            assert_ne!(base, Route::Falsify, "hint skipped falsification");
                            assert_ne!(base, Route::Escalate, "hint un-escalated a node");

                            // Only ever these two, and only when permitted.
                            match hinted {
                                Route::Falsify => assert!(
                                    falsify_permitted(&n, &sig, &tools),
                                    "hint enabled a forbidden falsification: {kind:?} {sig:?} {tools:?}"
                                ),
                                Route::Decompose => assert!(
                                    decompose_permitted(&n, &tools),
                                    "hint enabled a forbidden decomposition: {kind:?} {sig:?} {tools:?}"
                                ),
                                other => panic!("hint produced a non-hintable route: {other:?}"),
                            }

                            // Never toward certification.
                            assert!(!matches!(hinted, Route::Verify | Route::Prove));
                        }
                    }
                }
            }
        }
        assert!(
            moved > 0,
            "the hint path never fired; the test proves nothing"
        );
    }
}
