//! Autonomous agent loop (plan §1): drives an informal theorem end to end —
//! research to seed the claim DAG, then per-obligation routing
//! (falsify-before-prove), retrieval, best-of-N formalization selected by the
//! compiler, layered verification (warm compile + axiom gate + LeanParanoia
//! hardening), and a final adversarial critique. Bounded by iteration budgets;
//! degrades gracefully when a model or Lean toolchain is absent.

use crate::{
    config::Config,
    critic::Critic,
    db::Store,
    hardening,
    lean_session::LeanSession,
    model::{EdgeKind, ModelRequest, Node, NodeKind, NodeStatus, NodeTier},
    provider::ModelProvider,
    research::ResearchEngine,
    router::{self, NodeSignals, Route, ToolAvailability},
    sampling, scheduler,
    prover::{attempt_run, proof_job, formal::FormalSystem},
    tools::{LeanCheck, MathlibSearch, PythonCheck, Tool},
};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::HashSet;

pub struct AgentLoop<'a> {
    pub store: &'a Store,
    pub config: &'a Config,
    pub provider: &'a dyn ModelProvider,
}

#[derive(Debug, serde::Serialize)]
pub struct AgentSummary {
    pub project_id: String,
    pub run_id: String,
    pub steps: Vec<Value>,
    pub certified: usize,
    /// Aletheia abstentions: nodes the certification gate DECLINED on low
    /// confidence rather than certifying or failing. Excluded from conditional
    /// accuracy so an abstention is not scored as a wrong answer. `0` unless the
    /// abstention threshold env-seam is set (default behaviour is unchanged).
    pub abstained: usize,
    pub critique_findings: usize,
    pub state: String,
    pub notes: Vec<String>,
}

/// Optional env seam for the Aletheia abstention threshold. Absent (or
/// unparseable) means abstention is OFF — the agent keeps its exact prior
/// certify-or-fail behaviour. A value in `(0, 1]` makes the certify gate DECLINE
/// (abstain) on any uncertified node whose confidence is below it, instead of
/// scoring it as a failure. Deterministic: read once per certify call, no
/// wall-clock/rand.
pub fn abstain_threshold() -> Option<f64> {
    std::env::var("THEOREMATA_ABSTAIN_THRESHOLD")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|t| *t > 0.0)
}

struct Formalization {
    lean: String,
    compiles: bool,
    axioms_clean: bool,
}

impl AgentLoop<'_> {
    pub fn run(&self, project_id: &str) -> Result<AgentSummary> {
        let project = self.store.project(project_id)?;
        let run = self.store.begin_run(project_id, "autonomous_agent")?;
        let mut notes = Vec::new();
        let mut steps = Vec::new();
        // Ensure the theorem itself is a node so it can be routed and decomposed.
        if !self
            .store
            .nodes(project_id)?
            .iter()
            .any(|n| n.kind == NodeKind::Conjecture)
        {
            self.store.add_node(
                project_id,
                NodeKind::Conjecture,
                "Main conjecture",
                project.theorem.trim(),
                "agent:seed",
            )?;
        }

        let model_ready = self.config.model_command.is_some();
        let tools = ToolAvailability {
            python: PythonCheck::new().available(),
            lean: LeanCheck::new(self.config).available(),
            mathlib_search: MathlibSearch::new(self.config).available(),
            model: model_ready,
            external_prover: proof_job::any_prover_available(self.config, model_ready),
        };

        // Phase 1 — research: seed the claim DAG.
        self.store
            .update_run(project_id, &run, "running", "research", 0)?;
        if tools.model {
            let engine = ResearchEngine {
                store: self.store,
                provider: self.provider,
            };
            match engine.run(project_id) {
                Ok(s) => steps.push(json!({"phase":"research","created":s.created_nodes})),
                Err(e) => notes.push(format!("research phase failed: {e}")),
            }
        } else {
            notes.push("no model provider: research phase skipped".into());
        }

        // Warm Lean session (falls back to cold LeanCheck on failure). Only
        // warm it when a model can actually produce proofs to check.
        let mut session = if tools.lean && tools.model {
            match LeanSession::start(self.config, &["Mathlib".to_string()]) {
                Ok(s) => {
                    notes.push("warm Lean session active".into());
                    Some(s)
                }
                Err(e) => {
                    notes.push(format!("warm Lean unavailable, using cold checks: {e}"));
                    None
                }
            }
        } else {
            None
        };

        // Phase 2 — solve: route each open obligation, bounded passes.
        self.store
            .update_run(project_id, &run, "running", "solve", 0)?;
        let max_attempts = self.config.max_iterations.max(1);
        let mut falsified: HashSet<String> = HashSet::new();
        let mut counterexamples: HashSet<String> = HashSet::new();
        let mut decomposed: HashSet<String> = HashSet::new();
        let mut certified = 0usize;
        let mut abstained = 0usize;
        let mut loop_guard = crate::guard::LoopGuard::new(64);
        for _pass in 0..max_attempts {
            let nodes = self.store.nodes(project_id)?;
            let edges = self.store.edges(project_id)?;
            let schedule = scheduler::plan(&nodes, &edges);
            let mut progressed = false;
            for node_id in schedule.parallel_batches.iter().flatten() {
                let Some(node) = nodes.iter().find(|n| &n.id == node_id).cloned() else {
                    continue;
                };
                // A decomposed conjecture waits on its obligations; don't re-route it.
                if decomposed.contains(&node.id) {
                    continue;
                }
                let attempts = self.node_attempts(project_id, &node.id)?;
                let signals = NodeSignals {
                    falsified: falsified.contains(&node.id),
                    counterexample_found: counterexamples.contains(&node.id),
                    retrieved: !node.suggested_lemmas.is_empty(),
                    has_formal_statement: node.formal_statement.is_some(),
                    attempts,
                };
                let route = router::route(&node, &signals, &tools, max_attempts);
                // Loop detection: if we keep routing the same node the same way,
                // escalate to a human instead of spinning.
                if loop_guard.observe(&node.title, &format!("{route:?}")) {
                    self.store.set_node_status(
                        project_id,
                        &node.id,
                        NodeStatus::Blocked,
                        "loop_guard",
                    )?;
                    steps.push(json!({"node":node.id,"escalated":"loop_detected","route":route}));
                    continue;
                }
                let tier =
                    crate::guard::model_tier(node.kind, attempts, node.strategy_hint.as_deref());
                let outcome = self.act(
                    project_id,
                    &run,
                    &node,
                    route,
                    &mut session,
                    &mut falsified,
                    &mut counterexamples,
                    &mut decomposed,
                    &mut certified,
                    &mut abstained,
                )?;
                steps.push(json!({
                    "node":node.id,"title":node.title,"route":route,
                    "tier":crate::guard::tier_env_suffix(tier),"outcome":outcome
                }));
                if outcome != "noop" {
                    progressed = true;
                }
            }
            if !progressed {
                break;
            }
        }

        // Phase 3 — critique the resulting DAG.
        self.store
            .update_run(project_id, &run, "running", "critique", 0)?;
        let mut critique_findings = 0;
        if tools.model {
            let critic = Critic {
                store: self.store,
                provider: self.provider,
            };
            match critic.critique(project_id) {
                Ok(report) => {
                    critique_findings = report.findings.len();
                    steps.push(json!({"phase":"critique","findings":critique_findings}));
                }
                Err(e) => notes.push(format!("critique failed: {e}")),
            }
        }

        drop(session);
        let state = if certified > 0 {
            "made_progress"
        } else {
            "no_certificate"
        };
        self.store
            .update_run(project_id, &run, state, "complete", 0)?;
        Ok(AgentSummary {
            project_id: project_id.into(),
            run_id: run,
            steps,
            certified,
            abstained,
            critique_findings,
            state: state.into(),
            notes,
        })
    }

    fn node_attempts(&self, project_id: &str, node_id: &str) -> Result<u32> {
        Ok(self
            .store
            .attempts(project_id, 1000)?
            .into_iter()
            .filter(|a| a.node_id.as_deref() == Some(node_id))
            .count() as u32)
    }

    #[allow(clippy::too_many_arguments)]
    fn act(
        &self,
        project_id: &str,
        run: &str,
        node: &Node,
        route: Route,
        session: &mut Option<LeanSession>,
        falsified: &mut HashSet<String>,
        counterexamples: &mut HashSet<String>,
        decomposed: &mut HashSet<String>,
        certified: &mut usize,
        abstained: &mut usize,
    ) -> Result<&'static str> {
        match route {
            Route::Falsify => {
                // The model derives an executable bounded check from THIS node's
                // statement (never a hardcoded example); the generic worker runs
                // it. Numerics only screen — a clean run is not a proof.
                let verdict = crate::falsification::Falsifier {
                    provider: self.provider,
                }
                .falsify(&node.statement)?;
                let details = serde_json::to_value(&verdict)?;
                self.store.add_attempt(
                    project_id,
                    Some(&node.id),
                    Some(run),
                    "falsifier",
                    &json!({ "statement": node.statement }),
                    &details,
                    verdict.verdict != "counterexample",
                )?;
                self.store.add_evidence(
                    project_id,
                    &node.id,
                    "falsification",
                    "falsifier",
                    &verdict.verdict,
                    details,
                )?;
                if verdict.verdict == "counterexample" {
                    // Refuted: the branch is dead — record and let the router
                    // escalate it to a human next pass. Rejecting the node
                    // recomputes three-valued taint in the store, poisoning
                    // every dependent; surface that blast radius as an event.
                    counterexamples.insert(node.id.clone());
                    self.store.set_node_status(
                        project_id,
                        &node.id,
                        NodeStatus::Rejected,
                        "falsifier",
                    )?;
                    let edges = self.store.edges(project_id)?;
                    let tainted = crate::taint::tainted_dependents(&node.id, &edges);
                    if !tainted.is_empty() {
                        self.store.event(
                            Some(project_id),
                            Some(run),
                            "taint.propagated",
                            "falsifier",
                            json!({"root": node.id, "tainted_dependents": tainted}),
                        )?;
                    }
                }
                falsified.insert(node.id.clone());
                Ok("falsify")
            }
            Route::Retrieve => {
                let py = PythonCheck::new();
                if !py.available() {
                    return Ok("noop");
                }
                let root = self
                    .config
                    .resources
                    .join("mathlib4-master/mathlib4-master");
                let result = py.run(json!({
                    "tool":"retrieve","root":root,"imports":["Mathlib"],
                    "query":node.title,"limit":8,"op":"accessible_retrieve",
                    "theorem_module": node.lean_decls.first(),
                }))?;
                let lemmas = parse_lemma_names(&result.stdout);
                if crate::guard::looks_injected(&result.stdout) {
                    self.store.event(
                        Some(project_id),
                        None,
                        "guard.injection_flagged",
                        "librarian",
                        json!({ "node": node.id }),
                    )?;
                }
                if !lemmas.is_empty() {
                    self.store
                        .set_suggested_lemmas(project_id, &node.id, &lemmas, "librarian")?;
                    // Wrap retrieved (untrusted) text before it becomes a hint fed
                    // into later model prompts.
                    let hint =
                        crate::guard::wrap_untrusted("mathlib_retrieval", &lemmas.join(", "));
                    self.store
                        .set_strategy_hint(project_id, &node.id, Some(&hint), "librarian")?;
                }
                self.store.add_evidence(
                    project_id,
                    &node.id,
                    "retrieval",
                    "librarian",
                    if lemmas.is_empty() {
                        "none"
                    } else {
                        "candidates"
                    },
                    serde_json::to_value(&result)?,
                )?;
                Ok("retrieve")
            }
            Route::Formalize => {
                self.formalize(project_id, run, node, session, certified, abstained)
            }
            Route::Verify => {
                if let Some(formal) = &node.formal_statement {
                    let theorem = extract_theorem(formal);
                    let (compiles, axioms_clean) =
                        self.verify_source(formal, theorem.as_deref(), session)?;
                    self.store.add_evidence(
                        project_id,
                        &node.id,
                        "lean_compile",
                        "verifier",
                        if compiles && axioms_clean {
                            "pass"
                        } else {
                            "fail"
                        },
                        json!({"compiles":compiles,"axioms_clean":axioms_clean}),
                    )?;
                    if compiles && axioms_clean {
                        self.certify_k_consecutive(
                            project_id,
                            node,
                            formal,
                            theorem.as_deref(),
                            session,
                            certified,
                            abstained,
                        )?;
                    }
                    Ok("verify")
                } else {
                    Ok("noop")
                }
            }
            Route::Decompose => {
                if self.config.model_command.is_none() {
                    return Ok("noop");
                }
                let obligations = crate::decompose::Decomposer {
                    store: self.store,
                    provider: self.provider,
                }
                .run(
                    project_id,
                    run,
                    &node.statement,
                    self.config.node_granularity,
                )?;
                if obligations.is_empty() {
                    return Ok("noop");
                }
                for ob in obligations {
                    // A decomposed obligation may itself expand into helper
                    // sub-lemmas later; carry the typed-claim / transfer-schema
                    // tags forward as a strategy hint for the prover.
                    let hint = decompose_hint(&ob);
                    let child = self.store.add_node_detailed(
                        project_id,
                        NodeKind::Obligation,
                        NodeTier::Implementation,
                        Some(&node.id),
                        &ob.title,
                        &ob.statement,
                        hint.as_deref(),
                        &[],
                        "agent:decompose",
                    )?;
                    // The conjecture depends on each of its obligations.
                    self.store
                        .add_edge(project_id, &node.id, &child.id, EdgeKind::DependsOn)?;
                }
                decomposed.insert(node.id.clone());
                Ok("decompose")
            }
            Route::Prove => {
                self.prove_via_prover(project_id, run, node, session, certified, abstained)
            }
            _ => Ok("noop"),
        }
    }

    fn prove_via_prover(
        &self,
        project_id: &str,
        run: &str,
        node: &Node,
        session: &mut Option<LeanSession>,
        certified: &mut usize,
        abstained: &mut usize,
    ) -> Result<&'static str> {
        // Non-Lean targets go through the per-system generator + live backend
        // (Coq/Isar are produced and verified natively, not via the Lean path).
        if self.config.target_system != FormalSystem::Lean {
            return self.generate_formal(project_id, run, node, certified);
        }
        let input = json!({
            "statement": node.statement,
            "theorem_name": format!("Theoremata.N{}", node.id.replace('-', "").get(0..8).unwrap_or("ode")),
            "backend": self.config.prover_backend,
        });
        let record = attempt_run::start(
            self.store,
            self.config,
            project_id,
            Some(&node.id),
            input,
        )?;
        let out = attempt_run::run_to_completion(
            self.store,
            self.config,
            &record.id,
            self.config.prover_max_polls,
            Some(self.provider),
        )?;
        self.store.add_attempt(
            project_id,
            Some(&node.id),
            Some(run),
            "external_prover",
            &json!({"attempt_id": record.id, "backend": self.config.prover_backend}),
            &serde_json::to_value(&out)?,
            out.status == crate::prover::model::AttemptRunStatus::Completed,
        )?;
        let Some(result) = out.proof_result else {
            return Ok("prove_failed");
        };
        let Some(lean) = result.formal_code else {
            return Ok("prove_failed");
        };
        // Statement-change guard: if this node already carries a formal statement,
        // the external prover must return a proof of THAT statement — not a weaker
        // or different one (the mock, e.g., returns `: True := trivial`). Reject the
        // drift before trusting/overwriting the node, so a header that goes missing
        // or is weakened can never be certified.
        if let Some(formal) = node.formal_statement.as_deref() {
            let guard = crate::prover::statement_guard::guard_lean_round_trip(formal, &lean);
            if !guard.preserved {
                self.store.add_evidence(
                    project_id,
                    &node.id,
                    "statement_guard",
                    "verifier",
                    "drift",
                    crate::prover::statement_guard::guard_report_json(&guard),
                )?;
                return Ok("prove_statement_drift");
            }
        }
        self.store
            .set_formal_statement(project_id, &node.id, &lean, "external_prover")?;
        let theorem = extract_theorem(&lean);
        let (compiles, axioms_clean) =
            self.verify_source(&lean, theorem.as_deref(), session)?;
        if compiles && axioms_clean {
            let fresh = self.store.nodes(project_id)?.into_iter().find(|n| n.id == node.id).unwrap();
            self.certify_k_consecutive(
                project_id,
                &fresh,
                &lean,
                theorem.as_deref(),
                session,
                certified,
                abstained,
            )?;
            Ok("prove")
        } else {
            Ok("prove_unverified")
        }
    }

    fn formalize(
        &self,
        project_id: &str,
        run: &str,
        node: &Node,
        session: &mut Option<LeanSession>,
        certified: &mut usize,
        abstained: &mut usize,
    ) -> Result<&'static str> {
        // Non-Lean targets are formalized+proved through the per-system generator
        // (system-native Coq/Isar) rather than the Lean-only best-of-N below.
        if self.config.target_system != FormalSystem::Lean {
            return self.generate_formal(project_id, run, node, certified);
        }
        if self.config.model_command.is_none() {
            return Ok("noop");
        }
        let n = 3usize;
        // Best-of-N: each candidate is a model formalization checked by the
        // compiler + axiom gate; the compiler is the acceptance predicate.
        let selection = sampling::best_of_n(
            n,
            |_i| -> Result<Formalization> {
                let lean = self.formalize_once(&node.statement)?;
                let (compiles, axioms_clean) =
                    self.verify_source(&lean, extract_theorem(&lean).as_deref(), session)?;
                Ok(Formalization {
                    lean,
                    compiles,
                    axioms_clean,
                })
            },
            |f: &Formalization| f.compiles && f.axioms_clean,
        )?;

        let Some(sampled) = selection else {
            return Ok("noop");
        };
        self.store.add_attempt(
            project_id,
            Some(&node.id),
            Some(run),
            "formalizer",
            &json!({"statement":node.statement}),
            &json!({"accepted":sampled.accepted,"attempts":sampled.attempts,"index":sampled.index}),
            sampled.accepted,
        )?;
        self.store
            .set_formal_statement(project_id, &node.id, &sampled.value.lean, "formalizer")?;
        if sampled.accepted {
            let theorem = extract_theorem(&sampled.value.lean);
            self.certify_k_consecutive(
                project_id,
                node,
                &sampled.value.lean,
                theorem.as_deref(),
                session,
                certified,
                abstained,
            )?;
        }
        Ok("formalize")
    }

    /// Route a Rocq/Isabelle target through the per-system proof generator: emit
    /// system-native code, verify it through the live 3+1-layer gate (mock
    /// backend offline), and certify on a clean report. Mirrors `formalize` but
    /// the backend gate — not the Lean compiler — is the acceptance selector.
    fn generate_formal(
        &self,
        project_id: &str,
        run: &str,
        node: &Node,
        certified: &mut usize,
    ) -> Result<&'static str> {
        let system = self.config.target_system;
        let (code, report) = crate::formal_generate::generate_and_verify(
            self.store,
            self.config,
            self.provider,
            system,
            &node.statement,
        )?;
        self.store.add_attempt(
            project_id,
            Some(&node.id),
            Some(run),
            "formal_generator",
            &json!({"statement": node.statement, "system": system.as_str()}),
            &json!({"verified": report.lexically_verified}),
            report.lexically_verified,
        )?;
        self.store
            .set_formal_statement(project_id, &node.id, &code, "formal_generator")?;
        self.store.add_evidence(
            project_id,
            &node.id,
            "formal_verify",
            "verifier",
            if report.lexically_verified {
                "pass"
            } else {
                "fail"
            },
            serde_json::to_value(&report)?,
        )?;
        if report.lexically_verified {
            self.store.set_node_status(
                project_id,
                &node.id,
                NodeStatus::FormallyVerified,
                "verifier",
            )?;
            *certified += 1;
            Ok("formal_generate")
        } else {
            Ok("formal_generate_unverified")
        }
    }

    /// Certify a node only after `config.k_consecutive_clean` CONSECUTIVE clean
    /// verifier passes (streak resets on any fail), then hand off to
    /// [`AgentLoop::certify`] — which keeps the authoritative `#print axioms`
    /// gate intact. A hedge against a noisy verifier: one lucky clean pass is
    /// not enough. Returns whether the node was certified.
    fn certify_k_consecutive(
        &self,
        project_id: &str,
        node: &Node,
        lean: &str,
        theorem: Option<&str>,
        session: &mut Option<LeanSession>,
        certified: &mut usize,
        abstained: &mut usize,
    ) -> Result<bool> {
        let k = self.config.k_consecutive_clean;
        let gate = k_consecutive_clean(k, k.max(1), |_round| {
            let (compiles, axioms_clean) = self.verify_source(lean, theorem, session)?;
            Ok(compiles && axioms_clean)
        })?;
        self.store.add_evidence(
            project_id,
            &node.id,
            "k_consecutive_clean",
            "verifier",
            if gate.certified {
                "certified"
            } else {
                "streak_broken"
            },
            json!({
                "k": k,
                "rounds_run": gate.rounds_run,
                "longest_streak": gate.longest_streak,
            }),
        )?;
        // Scored proof-pool + critic meta-verification gate (item #1). The
        // candidate is inserted into the persisted pool regardless of the streak
        // outcome (so the pool is populated during a run); certification then
        // additionally requires the pool's all-pass verdict AND that the critic's
        // meta-verification did not CONFIRM a critical finding on this node. A
        // full clean streak maps to verifier_score 1.0 (the all-pass value).
        let verifier_score = if k == 0 {
            1.0
        } else {
            (gate.longest_streak as f64 / k as f64).min(1.0)
        };
        let meta_gate = super::certification::PoolMetaGate {
            store: self.store,
            provider: self.provider,
            enabled: super::certification::gate_enabled(),
        };
        // Aletheia abstention (item #2): when the abstention threshold env-seam is
        // set, an uncertified low-confidence node ABSTAINS (declines) rather than
        // being scored as a failure. Default (no threshold) keeps the exact prior
        // certify-or-fail behaviour.
        let outcome = match abstain_threshold() {
            Some(threshold) => meta_gate.evaluate_with_abstention(
                project_id,
                &node.id,
                lean,
                verifier_score,
                verifier_score,
                gate.certified,
                threshold,
            )?,
            None => meta_gate.evaluate(
                project_id,
                &node.id,
                lean,
                verifier_score,
                verifier_score,
                gate.certified,
            )?,
        };
        if outcome.certified {
            self.certify(project_id, node, lean, certified)?;
        } else if outcome.abstained {
            // A first-class terminal state distinct from failure: record it and
            // leave the node uncertified (never marked Rejected/Failed).
            *abstained += 1;
            self.store.event(
                Some(project_id),
                None,
                "certify.abstained",
                "certification_gate",
                json!({
                    "node": node.id,
                    "reason": outcome.abstain_reason,
                }),
            )?;
        }
        Ok(outcome.certified)
    }

    fn certify(
        &self,
        project_id: &str,
        node: &Node,
        lean: &str,
        certified: &mut usize,
    ) -> Result<()> {
        self.store.set_node_status(
            project_id,
            &node.id,
            NodeStatus::FormallyVerified,
            "verifier",
        )?;
        *certified += 1;
        // Additional hardening (opt-in: the first workspace build hits the
        // network with no timeout). The `#print axioms` gate is authoritative.
        if !self.config.harden_proofs {
            return Ok(());
        }
        let module = format!("N{}", node.id.replace('-', "").get(0..8).unwrap_or("node"));
        if let Ok(report) =
            hardening::harden(self.store, self.config, project_id, &node.id, &module, lean)
        {
            // Record the honest outcome. Only `Flagged` is a real soundness
            // failure: it taints the node (the `#print axioms` gate remains
            // authoritative for the Inconclusive/Unavailable/Skipped states,
            // so those must NOT hard-reject a formally verified node).
            use hardening::HardeningOutcome as HO;
            let flagged = matches!(report.outcome, HO::Flagged);
            let verdict = match report.outcome {
                HO::Passed => "clean",
                HO::Flagged => "flagged",
                HO::Inconclusive => "inconclusive",
                HO::Unavailable => "unavailable",
                HO::BuildFailed => "build_failed",
                HO::Skipped => "skipped",
            };
            self.store.add_evidence(
                project_id,
                &node.id,
                "hardening",
                "lean_paranoia",
                verdict,
                serde_json::to_value(&report)?,
            )?;
            if flagged {
                self.store.set_node_status(
                    project_id,
                    &node.id,
                    NodeStatus::Rejected,
                    "verifier",
                )?;
                *certified = certified.saturating_sub(1);
            }
        }
        Ok(())
    }

    fn formalize_once(&self, statement: &str) -> Result<String> {
        let response = self.provider.complete(&ModelRequest {
            role: "lean_formalizer".into(),
            task: "Produce a complete, self-contained Lean 4 file proving the statement. Never use sorry, admit, axioms, or unsafe declarations.".into(),
            context: json!({ "statement": statement }),
            output_schema: json!({"type":"object","required":["lean"],"properties":{"lean":{"type":"string"}}}),
        })?;
        Ok(response.content["lean"]
            .as_str()
            .context("missing lean")?
            .to_owned())
    }

    /// Compile a Lean source and audit its axioms, preferring the warm session.
    fn verify_source(
        &self,
        source: &str,
        theorem: Option<&str>,
        session: &mut Option<LeanSession>,
    ) -> Result<(bool, bool)> {
        if let Some(s) = session.as_mut() {
            match s.check(source, theorem) {
                Ok(outcome) => return Ok((outcome.ok, outcome.axioms_clean)),
                Err(_) => {
                    // Session died; drop it and fall back to cold checks.
                    *session = None;
                }
            }
        }
        // Cold path: write to a temp file, compile via LeanCheck, audit via python.
        let dir = self.config.workspace.join("_agent");
        std::fs::create_dir_all(&dir)?;
        let file = dir.join("Candidate.lean");
        std::fs::write(&file, source)?;
        let lean = LeanCheck::new(self.config);
        let compiles = if lean.available() {
            lean.run(json!({ "file": file }))?.success
        } else {
            false
        };
        let axioms_clean = if compiles {
            match theorem {
                Some(thm) => {
                    let py = PythonCheck::new();
                    if py.available() {
                        let audit = py.run(json!({
                            "tool":"check_axioms","source":source,"theorem":thm,
                            "root":self.config.lean_project
                        }))?;
                        serde_json::from_str::<Value>(&audit.stdout)
                            .ok()
                            .and_then(|v| v["output"]["clean"].as_bool())
                            .unwrap_or(false)
                    } else {
                        false
                    }
                }
                None => false,
            }
        } else {
            false
        };
        Ok((compiles, axioms_clean))
    }
}

/// Fold an obligation's typed-claim / transfer-schema tags into a strategy hint
/// (or `None` when the model tagged neither), so the prover sees how the claim
/// should be approached.
fn decompose_hint(ob: &crate::decompose::Obligation) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(kind) = ob.claim_kind {
        parts.push(format!("claim: {kind}"));
    }
    if !ob.ingredients.is_empty() {
        let names: Vec<String> = ob.ingredients.iter().map(|i| i.to_string()).collect();
        parts.push(format!("ingredients: {}", names.join(", ")));
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// Extract the first declared theorem/lemma name from a Lean source.
fn extract_theorem(src: &str) -> Option<String> {
    for line in src.lines() {
        let trimmed = line.trim_start();
        for kw in ["theorem ", "lemma "] {
            if let Some(rest) = trimmed.strip_prefix(kw) {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '\''))
                    .collect();
                if !name.is_empty() {
                    return Some(name);
                }
            }
        }
    }
    None
}

/// Outcome of the k-consecutive-clean acceptance gate.
#[derive(Debug, Clone, Copy)]
pub struct KConsecutiveOutcome {
    /// Whether `k` consecutive clean passes were reached.
    pub certified: bool,
    /// How many verification rounds actually ran.
    pub rounds_run: u32,
    /// The longest clean streak observed (for telemetry).
    pub longest_streak: u32,
}

/// AgentMathOlympiadMedalist's noisy-verifier hedge (`imo_client.py:339-346`):
/// run `check` up to `max_rounds` times and only accept after `k` CONSECUTIVE
/// clean passes, with the streak RESETTING to zero on any failed pass. Stops
/// early the moment the streak reaches `k`. `k == 0` accepts immediately without
/// running `check` (no consecutive requirement).
pub fn k_consecutive_clean(
    k: u32,
    max_rounds: u32,
    mut check: impl FnMut(u32) -> Result<bool>,
) -> Result<KConsecutiveOutcome> {
    if k == 0 {
        return Ok(KConsecutiveOutcome {
            certified: true,
            rounds_run: 0,
            longest_streak: 0,
        });
    }
    let mut streak = 0u32;
    let mut longest = 0u32;
    let mut rounds = 0u32;
    while rounds < max_rounds {
        let clean = check(rounds)?;
        rounds += 1;
        if clean {
            streak += 1;
            longest = longest.max(streak);
            if streak >= k {
                break;
            }
        } else {
            // A single failed pass wipes the streak — the hedge against a
            // verifier that occasionally false-passes.
            streak = 0;
        }
    }
    Ok(KConsecutiveOutcome {
        certified: streak >= k,
        rounds_run: rounds,
        longest_streak: longest,
    })
}

/// Pull ranked lemma names out of the retrieval worker's JSON stdout.
fn parse_lemma_names(stdout: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(stdout) else {
        return Vec::new();
    };
    value["output"]["results"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|r| r["name"].as_str().map(str::to_owned))
                .take(8)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ModelResponse, NodeKind};
    use std::path::Path;

    struct MockProvider;
    impl ModelProvider for MockProvider {
        fn complete(&self, request: &ModelRequest) -> Result<ModelResponse> {
            let content = match request.role.as_str() {
                "object_identification" => {
                    json!({"objects":[{"name":"n","description":"integer"}]})
                }
                "candidate_discovery" => {
                    json!({"candidates":[{"title":"c","statement":"s","type_label":"Invariant","status":"inconclusive"}]})
                }
                "formalization_target" => {
                    json!({"lean_signature":"theorem t : True := by sorry","symbol_dictionary":{}})
                }
                "lean_formalizer" => json!({"lean":"theorem t : True := trivial"}),
                "adversarial_verifier" => json!({"findings":[],"summary":"ok"}),
                _ => json!({}),
            };
            Ok(ModelResponse {
                content,
                model: "mock".into(),
                provider: "mock".into(),
            })
        }
        fn name(&self) -> &str {
            "command"
        }
    }

    #[test]
    fn k_consecutive_requires_a_clean_streak() {
        // Three consecutive clean passes required.
        let all_clean = k_consecutive_clean(3, 5, |_| Ok(true)).unwrap();
        assert!(all_clean.certified);
        assert_eq!(all_clean.rounds_run, 3); // stops early once the streak hits k

        // A fail on round 1 resets the streak; only rounds after it count. With
        // pattern clean,FAIL,clean,clean,clean the streak reaches 3 by round 5.
        let cell = std::cell::Cell::new(0u32);
        let recovers = k_consecutive_clean(3, 5, |_| {
            let i = cell.get();
            cell.set(i + 1);
            Ok(i != 1) // round index 1 fails, all others clean
        })
        .unwrap();
        assert!(recovers.certified);
        assert_eq!(recovers.rounds_run, 5);

        // Never enough consecutive clean passes within the round budget.
        let clean = std::cell::Cell::new(true);
        let never = k_consecutive_clean(3, 6, |_| {
            let c = clean.get();
            clean.set(!c); // alternating clean/fail: streak never reaches 2
            Ok(c)
        })
        .unwrap();
        assert!(!never.certified);
        assert_eq!(never.longest_streak, 1);
    }

    #[test]
    fn loop_runs_without_lean_or_python() {
        // No model_command means research/formalize are skipped, but the loop
        // must complete cleanly and route open nodes to noop.
        std::env::set_var("THEOREMATA_ARISTOTLE_MOCK", "0");
        std::env::set_var("THEOREMATA_ARISTOTLE_API_KEY", "test-disabled");
        let store = Store::open(Path::new(":memory:")).unwrap();
        let config = Config {
            model_command: None,
            prover_backend: "disabled".into(),
            ..Config::default()
        };
        let project = store.create_project("p", "trivial holds").unwrap();
        store
            .add_node(&project.id, NodeKind::Obligation, "o", "s", "test")
            .unwrap();
        let agent = AgentLoop {
            store: &store,
            config: &config,
            provider: &MockProvider,
        };
        let summary = agent.run(&project.id).unwrap();
        assert_eq!(summary.state, "no_certificate");
    }
}
