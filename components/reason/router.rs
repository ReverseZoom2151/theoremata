//! Per-obligation router (plan §2).
//!
//! An inspectable state machine that decides what to do next with a node,
//! encoding the *falsify-before-prove* policy: spend the cheap counterexample
//! check as a gate before any expensive proof effort, so a refuted branch dies
//! immediately. The router is pure — it reasons over a node, a compact summary
//! of its history (`NodeSignals`), and which tools are available — so the
//! decision is auditable and testable in isolation.

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
    pub mathlib_search: bool,
    pub model: bool,
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
pub fn route(
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

    // 6. Has a formal statement → verify it with Lean.
    if (signals.has_formal_statement || node.formal_statement.is_some()) && tools.lean {
        return Route::Verify;
    }

    // 7. Informal leaf but not yet formal → formalize (needs Lean + a model).
    if !signals.has_formal_statement && node.formal_statement.is_none() && tools.lean && tools.model
    {
        return Route::Formalize;
    }

    // 8. Otherwise attempt a proof if we have a model, else escalate.
    if tools.model {
        Route::Prove
    } else {
        Route::Escalate
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NodeStatus, NodeTier};
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
            tier: NodeTier::Spine,
            parent_id: None,
            strategy_hint: None,
            suggested_lemmas: Vec::new(),
            stmt_formalized: false,
            proof_done: false,
            created_at: now,
            updated_at: now,
        }
    }

    const ALL: ToolAvailability = ToolAvailability {
        python: true,
        lean: true,
        mathlib_search: true,
        model: true,
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
        let n = node(NodeKind::FormalStatement, Some("theorem t : True := trivial"));
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
}
