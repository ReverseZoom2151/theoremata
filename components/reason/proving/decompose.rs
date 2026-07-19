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
    decomposition_admission::{
        self, AdmissionReport, ChildProposal, DecompositionProposal, DischargeProbe, ParentNode,
        Violation,
    },
    model::{ClaimKind, Granularity, ModelRequest, TransferIngredient},
    provider::ModelProvider,
    retry::{Decision, RetryLimits, RetryState},
};
use anyhow::{Context, Result};
use serde_json::json;

/// Env seam for decomposition admission ENFORCEMENT. Defaults **off**: absent /
/// empty / `0`/`false`/`off` means the admission report is computed and recorded
/// for observability but never refuses a decomposition. Mirrors
/// `default_validate_statements` in `app/config.rs`; see the module docs on
/// [`Decomposer::run_admitted`] for the Config field this should become.
pub const ENFORCE_ADMISSION_ENV: &str = "THEOREMATA_ENFORCE_DECOMPOSITION_ADMISSION";

/// Whether admission violations REFUSE a decomposition. Read once per
/// [`Decomposer::run`] call; deterministic, no wall-clock/rand.
pub fn admission_enforced() -> bool {
    admission_enforced_from(std::env::var(ENFORCE_ADMISSION_ENV).ok())
}

/// Pure core of [`admission_enforced`], so the policy is testable without
/// mutating process env (which races under the test harness).
fn admission_enforced_from(raw: Option<String>) -> bool {
    match raw {
        Some(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "off"
        ),
        None => false,
    }
}

/// The synthetic parent id used for the proposal built out of a `Decomposer`
/// call. The decomposer is handed a bare statement, not a graph node, so there
/// is no real node id to use; this id only has to be distinct from the child
/// ids for the acyclicity check.
const PARENT_ID: &str = "decomposition-parent";

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

    /// Build the [`DecompositionProposal`] that admission control judges.
    ///
    /// The decomposer is handed a bare `statement`, not a graph node, so three
    /// [`ParentNode`] fields have to be supplied synthetically:
    ///
    /// * `id` — [`PARENT_ID`], only needs to differ from the child ids;
    /// * `depth` — `0`. The decomposer does not know where in the tree this
    ///   statement sits. `0` is the *permissive* choice: it can never trip
    ///   `DepthExceeded`, so this call site simply does not enforce the depth
    ///   bound rather than enforcing it against a made-up number;
    /// * `stays_active` — `true`. A `Decomposer` run only produces obligations;
    ///   it has no channel through which to retire the parent.
    ///
    /// Children come through [`ChildProposal::from_obligation`], which fixes
    /// their status at `Unproved` by construction — so the authority-leak check
    /// cannot fire from *this* call site. It is still run, because the check is
    /// cheap and the invariant is worth asserting rather than assuming.
    pub fn admission_proposal(
        statement: &str,
        obligations: &[Obligation],
        probe: &DischargeProbe,
    ) -> DecompositionProposal {
        let children = obligations
            .iter()
            .enumerate()
            .map(|(i, ob)| ChildProposal::from_obligation(format!("ob{i}"), ob))
            .collect();
        DecompositionProposal::new(
            ParentNode::new(PARENT_ID, statement, 0),
            children,
            probe.clone(),
        )
    }

    /// The violations this call site is willing to REFUSE on.
    ///
    /// Everything is enforced except `Unearned` when no probe ran at all. A
    /// `Decomposer` is not currently handed discharge-probe evidence (see the
    /// `probe` parameter of [`Decomposer::run_admitted`]), and `NoProbe` would
    /// otherwise refuse every single decomposition — turning the flag into a
    /// kill switch rather than a gate. Absence of evidence is not enforced here;
    /// a probe that *did* run and failed to qualify **is**.
    fn enforceable_violations(report: &AdmissionReport, probe: &DischargeProbe) -> Vec<String> {
        report
            .violations
            .iter()
            .filter(|v| probe.ran || !matches!(v, Violation::Unearned { .. }))
            .map(|v| format!("{v:?}"))
            .collect()
    }

    /// Decompose `statement` into obligations at the requested `granularity`,
    /// bounded by the QED retry policy. Each model attempt is recorded (with the
    /// hidden-helper budget). Empty vec when offline or after the retry budget.
    ///
    /// Admission control runs with no probe evidence and enforcement taken from
    /// [`admission_enforced`] (env, default OFF).
    pub fn run(
        &self,
        project_id: &str,
        run_id: &str,
        statement: &str,
        granularity: Granularity,
    ) -> Result<Vec<Obligation>> {
        self.run_admitted(
            project_id,
            run_id,
            statement,
            granularity,
            &DischargeProbe::default(),
            admission_enforced(),
        )
    }

    /// [`Decomposer::run`] with admission control supplied explicitly.
    ///
    /// `probe` is the bounded discharge-probe evidence for the leaf being
    /// decomposed; `DischargeProbe::default()` means "none was supplied", which
    /// suppresses only the `Unearned` refusal (see
    /// [`Decomposer::enforceable_violations`]).
    ///
    /// `enforce` decides what happens to a refused proposal:
    ///
    /// * `false` (the default via env) — the [`AdmissionReport`] is computed and
    ///   recorded in the success attempt's detail for observability, and the
    ///   obligations are returned exactly as before. Enabling enforcement can
    ///   only ever turn accepted decompositions into rejected ones, so it stays
    ///   off until the recorded reports show what it would have refused.
    /// * `true` — a refused proposal does **not** return; the attempt is
    ///   recorded as a failure carrying the violations, and control falls
    ///   through to the existing retry/escalation path (plan history + budget
    ///   exhaustion), exactly like a model error would.
    pub fn run_admitted(
        &self,
        project_id: &str,
        run_id: &str,
        statement: &str,
        granularity: Granularity,
        probe: &DischargeProbe,
        enforce: bool,
    ) -> Result<Vec<Obligation>> {
        if self.provider.name() == "offline" {
            return Ok(Vec::new());
        }
        let history = crate::plan_history::PlanHistory::new(self.store);
        let mut state = RetryState::new(RetryLimits::default());
        loop {
            // Read the cross-attempt strategy memory BEFORE proposing a plan so
            // the model is steered away from strategies that already died.
            let prior = history.render(project_id)?;
            let detail = match self.decompose(statement, granularity, prior.as_deref()) {
                Ok(obligations) if !obligations.is_empty() => {
                    // ADMISSION GATE. Run before the obligations are accepted:
                    // a decomposition that degenerates (restates the goal, does
                    // not simplify, is a bare rename) or leaks proof authority
                    // must not become the accepted result.
                    let proposal = Self::admission_proposal(statement, &obligations, probe);
                    let report = decomposition_admission::admit(&proposal);
                    let refused = Self::enforceable_violations(&report, probe);
                    if enforce && !refused.is_empty() {
                        // Fall through to the shared failure path below.
                        format!("decomposition admission refused: {}", refused.join("; "))
                    } else {
                        let budget = Self::expected_helper_nodes(granularity, obligations.len());
                        self.store.add_attempt(
                            project_id,
                            None,
                            Some(run_id),
                            "proof_decomposer",
                            &json!({
                                "statement": statement,
                                "granularity": granularity.to_string(),
                            }),
                            &json!({
                                "obligations": obligations.len(),
                                "expected_helper_nodes": budget,
                                "fanout_multiplier": granularity.fanout_multiplier(),
                                "admission": {
                                    "enforced": enforce,
                                    "admitted": report.admitted,
                                    "probe_verdict": format!("{:?}", report.probe_verdict),
                                    "violations": report
                                        .violations
                                        .iter()
                                        .map(|v| format!("{v:?}"))
                                        .collect::<Vec<_>>(),
                                    "would_refuse": refused,
                                },
                            }),
                            true,
                        )?;
                        return Ok(obligations);
                    }
                }
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
            // Mechanical budget-exhaustion escalation, and append the failed
            // strategy to plan history so the next attempt reads it and does not
            // repeat this dead end.
            let escalation = state.escalate_exhausted();
            history.add(
                project_id,
                &crate::plan_history::PlanHistoryEntry::failed(
                    state.attempt,
                    format!("decomposition at {granularity} granularity"),
                    detail,
                ),
            )?;
            if escalation.decision == Decision::Terminate {
                return Ok(Vec::new());
            }
        }
    }

    fn decompose(
        &self,
        statement: &str,
        granularity: Granularity,
        plan_history: Option<&str>,
    ) -> Result<Vec<Obligation>> {
        let granularity_hint = match granularity {
            Granularity::Coarse => "Prefer a few coarse, paper-sized obligations.",
            Granularity::Medium => "Aim for balanced, individually-provable obligations.",
            Granularity::Fine => {
                "Prefer many small micro-lemma obligations; let the DAG carry the reasoning."
            }
        };
        let history_hint = if plan_history.is_some() {
            " Prior attempts are recorded in `plan_history`; do NOT repeat a strategy on its \
             'Do NOT try again' list."
        } else {
            ""
        };
        let response = self.provider.complete(&ModelRequest {
            role: "proof_decomposer".into(),
            task: format!(
                "Decompose the statement into independently verifiable obligations. {granularity_hint} \
                 Optionally tag each obligation with a claim type (invariant, norm-identity, \
                 scalar-recursion, spectral, convergence, stability, normal-form, obstruction, \
                 counterexample) and any transfer-schema ingredients it reduces to \
                 (invariant-subspace, gradient-plane, scalar-progress-coordinate, \
                 structured-local-update, comparison-inequality, admissible-updates).{history_hint}"
            ),
            context: json!({
                "statement": statement,
                "granularity": granularity.to_string(),
                "plan_history": plan_history,
            }),
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

    // =======================================================================
    // Admission control
    // =======================================================================

    /// A provider whose "decomposition" is a single obligation — a rename, not a
    /// split. Violates `min_children`, and its child does not undercut the
    /// parent's complexity either.
    struct DegenerateProvider;
    impl ModelProvider for DegenerateProvider {
        fn complete(&self, _: &ModelRequest) -> Result<ModelResponse> {
            Ok(ModelResponse {
                content: json!({"obligations":[
                    {"title":"The theorem","statement":"hA , hB ⊢ forall x , (f x) ∧ (g x) → (h x) ∨ (k x)"}
                ]}),
                model: "test".into(),
                provider: "command".into(),
            })
        }
        fn name(&self) -> &str {
            "command"
        }
    }

    /// A provider producing two genuinely simpler, distinct children.
    struct CleanProvider;
    impl ModelProvider for CleanProvider {
        fn complete(&self, _: &ModelRequest) -> Result<ModelResponse> {
            Ok(ModelResponse {
                content: json!({"obligations":[
                    {"title":"Left half","statement":"hA ⊢ f x"},
                    {"title":"Right half","statement":"hB ⊢ g x"}
                ]}),
                model: "test".into(),
                provider: "command".into(),
            })
        }
        fn name(&self) -> &str {
            "command"
        }
    }

    const PARENT: &str = "hA , hB ⊢ forall x , (f x) ∧ (g x) → (h x) ∨ (k x)";

    fn store_with_run() -> (Store, String, String) {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "t").unwrap();
        let run = store.begin_run(&project.id, "test").unwrap();
        let pid = project.id.clone();
        (store, pid, run)
    }

    /// The most recent `proof_decomposer` attempt.
    fn last_attempt(store: &Store, project_id: &str) -> crate::model::Attempt {
        store
            .attempts(project_id, 50)
            .unwrap()
            .into_iter()
            .find(|a| a.actor == "proof_decomposer")
            .expect("a decomposer attempt was recorded")
    }

    #[test]
    fn env_flag_defaults_off_and_is_truthy_only_when_set() {
        assert!(!admission_enforced_from(None));
        for off in ["", " ", "0", "false", "OFF"] {
            assert!(!admission_enforced_from(Some(off.to_string())), "{off:?}");
        }
        for on in ["1", "true", "yes", "on"] {
            assert!(admission_enforced_from(Some(on.to_string())), "{on:?}");
        }
    }

    #[test]
    fn flag_off_still_accepts_a_violating_decomposition_but_records_the_report() {
        let (store, project, run) = store_with_run();
        let obligations = Decomposer {
            store: &store,
            provider: &DegenerateProvider,
        }
        .run_admitted(
            &project,
            &run,
            PARENT,
            Granularity::Medium,
            &DischargeProbe::default(),
            false,
        )
        .unwrap();

        // Behaviour is unchanged: the obligations are still returned.
        assert_eq!(obligations.len(), 1);

        // But the report is there for observability.
        let attempt = last_attempt(&store, &project);
        assert!(attempt.success);
        let admission = &attempt.output["admission"];
        assert_eq!(admission["enforced"], json!(false));
        assert_eq!(admission["admitted"], json!(false));
        let would_refuse = admission["would_refuse"].as_array().unwrap();
        assert!(
            would_refuse.iter().any(|v| v.as_str().unwrap().contains("SelfChild")),
            "restating the parent as its own lemma should be flagged: {would_refuse:?}"
        );
        assert!(
            would_refuse.iter().any(|v| v.as_str().unwrap().contains("ChildCount")),
            "a one-child split should be flagged: {would_refuse:?}"
        );
    }

    #[test]
    fn flag_on_rejects_the_same_decomposition_and_routes_to_the_failure_path() {
        let (store, project, run) = store_with_run();
        let obligations = Decomposer {
            store: &store,
            provider: &DegenerateProvider,
        }
        .run_admitted(
            &project,
            &run,
            PARENT,
            Granularity::Medium,
            &DischargeProbe::default(),
            true,
        )
        .unwrap();

        // The provider is deterministic, so every retry is refused the same way
        // and the existing budget-exhaustion escalation terminates the loop.
        assert!(
            obligations.is_empty(),
            "a refused decomposition must not be returned as the accepted result"
        );

        // The refusal was recorded as a FAILED attempt carrying the violations,
        // i.e. it went down the existing failure path rather than a new one.
        let attempt = last_attempt(&store, &project);
        assert!(!attempt.success);
        let err = attempt.output["error"].as_str().unwrap();
        assert!(err.contains("decomposition admission refused"), "{err}");
        assert!(err.contains("SelfChild"), "{err}");

        // ...and the dead strategy reached plan history, so the next attempt
        // is steered away from it.
        let history = crate::plan_history::PlanHistory::new(&store)
            .render(&project)
            .unwrap()
            .unwrap_or_default();
        assert!(history.contains("decomposition"), "{history}");
    }

    #[test]
    fn a_clean_decomposition_is_admitted_either_way() {
        for enforce in [false, true] {
            let (store, project, run) = store_with_run();
            let obligations = Decomposer {
                store: &store,
                provider: &CleanProvider,
            }
            .run_admitted(
                &project,
                &run,
                PARENT,
                Granularity::Medium,
                &DischargeProbe::default(),
                enforce,
            )
            .unwrap();
            assert_eq!(obligations.len(), 2, "enforce={enforce}");

            let attempt = last_attempt(&store, &project);
            assert!(attempt.success, "enforce={enforce}");
            let would_refuse = attempt.output["admission"]["would_refuse"]
                .as_array()
                .unwrap();
            assert!(
                would_refuse.is_empty(),
                "enforce={enforce}, would_refuse={would_refuse:?}"
            );
        }
    }

    #[test]
    fn a_probe_that_ran_and_did_not_qualify_is_enforced() {
        // Absence of probe evidence is tolerated; a probe that ran and failed to
        // earn the split is not. Syntax errors route to REPAIR, never to a
        // decomposition.
        let probe = DischargeProbe {
            ran: true,
            syntax_errors: 3,
            ..Default::default()
        };
        let proposal = Decomposer::admission_proposal(
            PARENT,
            &[
                Obligation {
                    title: "Left".into(),
                    statement: "hA ⊢ f x".into(),
                    claim_kind: None,
                    ingredients: Vec::new(),
                },
                Obligation {
                    title: "Right".into(),
                    statement: "hB ⊢ g x".into(),
                    claim_kind: None,
                    ingredients: Vec::new(),
                },
            ],
            &probe,
        );
        let report = decomposition_admission::admit(&proposal);
        assert!(!Decomposer::enforceable_violations(&report, &probe).is_empty());

        // The same proposal with no probe at all is NOT refused here.
        let none = DischargeProbe::default();
        let proposal = DecompositionProposal::new(
            proposal.parent.clone(),
            proposal.children.clone(),
            none.clone(),
        );
        let report = decomposition_admission::admit(&proposal);
        assert!(!report.admitted, "admit() itself still fails closed");
        assert!(Decomposer::enforceable_violations(&report, &none).is_empty());
    }

    #[test]
    fn children_built_from_obligations_are_never_asserted_proved() {
        let proposal = Decomposer::admission_proposal(
            PARENT,
            &[Obligation {
                title: "Step".into(),
                statement: "hA ⊢ f x".into(),
                claim_kind: None,
                ingredients: Vec::new(),
            }],
            &DischargeProbe::default(),
        );
        assert!(proposal.children.iter().all(|c| !c.status.is_assertion()));
        // Parent id must not collide with any child id, or the cycle check would
        // see a self-loop.
        assert!(proposal.children.iter().all(|c| c.id != proposal.parent.id));
    }
}
