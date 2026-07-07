//! Model-driven proof decomposition with the QED-style retry policy: turn a
//! statement into independently-verifiable obligations. Returns an empty vec
//! (not a canned skeleton) when no model is configured or the model fails after
//! retries — no hardcoded fallback.
//!
//! Two mining findings shape this (plan Tier 1, item 4):
//!
//! * **The blueprint DAG is a *skeleton*; executors invent ~1.8x un-blueprinted
//!   helper decls per node** (measured: Kakeya 2x, RHCurves/strongpnt 1.8x,
//!   ZkLinalg 1.6x). Node granularity is a *dial* (`model::Granularity`); the
//!   decomposer budgets for hidden-helper fan-out rather than expecting 1:1, and
//!   an obligation is free to expand into helper sub-lemmas without the parent
//!   being treated as failed.
//! * **Typed claims + transfer-schema** (MathResearchPrompts): each obligation
//!   can carry a `ClaimKind` (invariant / norm-identity / …) and the
//!   `TransferIngredient`s (invariant subspace, progress coordinate, local
//!   update, comparison inequality) a convergence/optimality proof reduces to.

use crate::{
    db::Store,
    model::{ClaimKind, Granularity, ModelRequest, TransferIngredient},
    provider::ModelProvider,
    retry::{Decision, RetryLimits, RetryState},
};
use anyhow::{Context, Result};
use serde_json::json;

/// One decomposed obligation, optionally typed and reduced to transfer-schema
/// ingredients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Obligation {
    pub title: String,
    pub statement: String,
    /// MathResearchPrompts typed-claim label, when the model tagged one.
    pub claim_kind: Option<ClaimKind>,
    /// Transfer-schema ingredients this obligation reduces to.
    pub ingredients: Vec<TransferIngredient>,
}

pub struct Decomposer<'a> {
    pub store: &'a Store,
    pub provider: &'a dyn ModelProvider,
}

impl Decomposer<'_> {
    /// The number of un-blueprinted helper declarations to budget *beyond* the
    /// `obligation_count` spine obligations, given a granularity dial. Derived
    /// from the measured ~1.8x fan-out; e.g. Medium over 5 obligations budgets
    /// `ceil(5 * 0.8) = 4` helpers. Callers use this to size a workspace / not
    /// treat helper expansion as failure.
    pub fn expected_helper_nodes(granularity: Granularity, obligation_count: usize) -> usize {
        // Round the projected total then subtract the spine count — rounding the
        // product avoids f64 imprecision in `multiplier - 1.0` (e.g. 1.6 - 1.0).
        let total = (obligation_count as f64 * granularity.fanout_multiplier()).round() as usize;
        total.saturating_sub(obligation_count)
    }

    /// Decompose `statement` into obligations at the requested `granularity`,
    /// bounded by the QED retry policy. Each model attempt is recorded (with the
    /// hidden-helper budget). Empty vec when offline or after the retry budget.
    pub fn run(
        &self,
        project_id: &str,
        run_id: &str,
        statement: &str,
        granularity: Granularity,
    ) -> Result<Vec<Obligation>> {
        if self.provider.name() == "offline" {
            return Ok(Vec::new());
        }
        let mut state = RetryState::new(RetryLimits::default());
        loop {
            match self.decompose(statement, granularity) {
                Ok(obligations) if !obligations.is_empty() => {
                    let budget = Self::expected_helper_nodes(granularity, obligations.len());
                    self.store.add_attempt(
                        project_id,
                        None,
                        Some(run_id),
                        "proof_decomposer",
                        &json!({ "statement": statement, "granularity": granularity.to_string() }),
                        &json!({
                            "obligations": obligations.len(),
                            "expected_helper_nodes": budget,
                            "fanout_multiplier": granularity.fanout_multiplier(),
                        }),
                        true,
                    )?;
                    return Ok(obligations);
                }
                other => {
                    let detail = match &other {
                        Ok(_) => "empty decomposition".to_string(),
                        Err(e) => e.to_string(),
                    };
                    self.store.add_attempt(
                        project_id,
                        None,
                        Some(run_id),
                        "proof_decomposer",
                        &json!({ "statement": statement }),
                        &json!({ "error": detail }),
                        false,
                    )?;
                    if state.resolve(Decision::ReviseProof) == Decision::Terminate {
                        return Ok(Vec::new());
                    }
                }
            }
        }
    }

    fn decompose(&self, statement: &str, granularity: Granularity) -> Result<Vec<Obligation>> {
        let granularity_hint = match granularity {
            Granularity::Coarse => "Prefer a few coarse, paper-sized obligations.",
            Granularity::Medium => "Aim for balanced, individually-provable obligations.",
            Granularity::Fine => "Prefer many small micro-lemma obligations; let the DAG carry the reasoning.",
        };
        let response = self.provider.complete(&ModelRequest {
            role: "proof_decomposer".into(),
            task: format!(
                "Decompose the statement into independently verifiable obligations. {granularity_hint} \
                 Optionally tag each obligation with a claim type (invariant, norm-identity, \
                 scalar-recursion, spectral, convergence, stability, normal-form, obstruction, \
                 counterexample) and any transfer-schema ingredients it reduces to \
                 (invariant-subspace, gradient-plane, scalar-progress-coordinate, \
                 structured-local-update, comparison-inequality, admissible-updates)."
            ),
            context: json!({ "statement": statement, "granularity": granularity.to_string() }),
            output_schema: json!({"type":"object","required":["obligations"],"properties":{
                "obligations":{"type":"array","items":{"type":"object","required":["title","statement"],
                    "properties":{
                        "title":{"type":"string"},
                        "statement":{"type":"string"},
                        "claim_kind":{"type":"string"},
                        "ingredients":{"type":"array","items":{"type":"string"}}
                    }}}}}),
        })?;
        Ok(response.content["obligations"]
            .as_array()
            .context("missing obligations")?
            .iter()
            .map(|x| {
                let claim_kind = x["claim_kind"]
                    .as_str()
                    .or_else(|| x["type_label"].as_str())
                    .and_then(ClaimKind::from_label);
                let ingredients = x["ingredients"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|i| i.as_str())
                            .filter_map(TransferIngredient::from_label)
                            .collect()
                    })
                    .unwrap_or_default();
                Obligation {
                    title: x["title"].as_str().unwrap_or("Obligation").into(),
                    statement: x["statement"].as_str().unwrap_or("").into(),
                    claim_kind,
                    ingredients,
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelResponse;
    use std::path::Path;

    use crate::model::{ClaimKind, Granularity, TransferIngredient};

    struct DecomposeProvider;
    impl ModelProvider for DecomposeProvider {
        fn complete(&self, _: &ModelRequest) -> Result<ModelResponse> {
            Ok(ModelResponse {
                content: json!({"obligations":[
                    {"title":"Step 1","statement":"first obligation",
                     "claim_kind":"norm identity",
                     "ingredients":["invariant subspace","comparison-inequality"]},
                    {"title":"Step 2","statement":"second obligation"}
                ]}),
                model: "test".into(),
                provider: "command".into(),
            })
        }
        fn name(&self) -> &str {
            "command"
        }
    }

    #[test]
    fn decomposes_via_model_with_retry() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "t").unwrap();
        let run = store.begin_run(&project.id, "test").unwrap();
        let obligations = Decomposer {
            store: &store,
            provider: &DecomposeProvider,
        }
        .run(&project.id, &run, "some theorem", Granularity::Medium)
        .unwrap();
        assert_eq!(obligations.len(), 2);
        // The typed-claim label and transfer ingredients are parsed leniently.
        assert_eq!(obligations[0].claim_kind, Some(ClaimKind::NormIdentity));
        assert_eq!(
            obligations[0].ingredients,
            vec![
                TransferIngredient::InvariantSubspace,
                TransferIngredient::ComparisonInequality
            ]
        );
        assert_eq!(obligations[1].claim_kind, None);
    }

    #[test]
    fn offline_returns_empty_not_a_skeleton() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "t").unwrap();
        let run = store.begin_run(&project.id, "test").unwrap();
        let obligations = Decomposer {
            store: &store,
            provider: &crate::provider::OfflineProvider,
        }
        .run(&project.id, &run, "t", Granularity::Medium)
        .unwrap();
        assert!(obligations.is_empty());
    }

    #[test]
    fn hidden_helper_budget_scales_with_granularity() {
        // ~1.8x fan-out at Medium: 5 obligations budget ceil(5*0.8)=4 helpers.
        assert_eq!(Decomposer::expected_helper_nodes(Granularity::Medium, 5), 4);
        // Coarse (1.6x) budgets fewer, Fine (2.0x) budgets more.
        assert_eq!(Decomposer::expected_helper_nodes(Granularity::Coarse, 5), 3);
        assert_eq!(Decomposer::expected_helper_nodes(Granularity::Fine, 5), 5);
        assert_eq!(Decomposer::expected_helper_nodes(Granularity::Medium, 0), 0);
    }
}
