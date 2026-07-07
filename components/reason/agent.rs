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
    model::{ModelRequest, Node, NodeStatus},
    provider::ModelProvider,
    research::ResearchEngine,
    router::{self, NodeSignals, Route, ToolAvailability},
    sampling,
    scheduler,
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
    pub critique_findings: usize,
    pub state: String,
    pub notes: Vec<String>,
}

struct Formalization {
    lean: String,
    compiles: bool,
    axioms_clean: bool,
}

impl AgentLoop<'_> {
    pub fn run(&self, project_id: &str) -> Result<AgentSummary> {
        let _project = self.store.project(project_id)?;
        let run = self.store.begin_run(project_id, "autonomous_agent")?;
        let mut notes = Vec::new();
        let mut steps = Vec::new();

        let tools = ToolAvailability {
            python: PythonCheck::new().available(),
            lean: LeanCheck::new(self.config).available(),
            mathlib_search: MathlibSearch::new(self.config).available(),
            model: self.config.model_command.is_some(),
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
        let mut certified = 0usize;
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
                let tier = crate::guard::model_tier(node.kind, attempts, node.strategy_hint.as_deref());
                let outcome = self.act(
                    project_id,
                    &run,
                    &node,
                    route,
                    &mut session,
                    &mut falsified,
                    &mut counterexamples,
                    &mut certified,
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
        certified: &mut usize,
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
                    // escalate it to a human next pass.
                    counterexamples.insert(node.id.clone());
                    self.store.set_node_status(
                        project_id,
                        &node.id,
                        NodeStatus::Rejected,
                        "falsifier",
                    )?;
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
                    "query":node.title,"limit":8,"op":"retrieve"
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
                    let hint = crate::guard::wrap_untrusted("mathlib_retrieval", &lemmas.join(", "));
                    self.store
                        .set_strategy_hint(project_id, &node.id, Some(&hint), "librarian")?;
                }
                self.store.add_evidence(
                    project_id,
                    &node.id,
                    "retrieval",
                    "librarian",
                    if lemmas.is_empty() { "none" } else { "candidates" },
                    serde_json::to_value(&result)?,
                )?;
                Ok("retrieve")
            }
            Route::Formalize => self.formalize(project_id, run, node, session, certified),
            Route::Verify => {
                if let Some(formal) = &node.formal_statement {
                    let (compiles, axioms_clean) =
                        self.verify_source(formal, extract_theorem(formal).as_deref(), session)?;
                    self.store.add_evidence(
                        project_id,
                        &node.id,
                        "lean_compile",
                        "verifier",
                        if compiles && axioms_clean { "pass" } else { "fail" },
                        json!({"compiles":compiles,"axioms_clean":axioms_clean}),
                    )?;
                    if compiles && axioms_clean {
                        self.certify(project_id, node, formal, certified)?;
                    }
                    Ok("verify")
                } else {
                    Ok("noop")
                }
            }
            _ => Ok("noop"),
        }
    }

    fn formalize(
        &self,
        project_id: &str,
        run: &str,
        node: &Node,
        session: &mut Option<LeanSession>,
        certified: &mut usize,
    ) -> Result<&'static str> {
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
            self.certify(project_id, node, &sampled.value.lean, certified)?;
        }
        Ok("formalize")
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
        if let Ok(report) = hardening::harden(self.store, self.config, project_id, &node.id, &module, lean)
        {
            self.store.add_evidence(
                project_id,
                &node.id,
                "hardening",
                "lean_paranoia",
                if report.clean { "clean" } else { "flagged" },
                serde_json::to_value(&report)?,
            )?;
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
                "object_identification" => json!({"objects":[{"name":"n","description":"integer"}]}),
                "candidate_discovery" => json!({"candidates":[{"title":"c","statement":"s","type_label":"Invariant","status":"inconclusive"}]}),
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
    fn loop_runs_without_lean_or_python() {
        // No model_command means research/formalize are skipped, but the loop
        // must complete cleanly and route open nodes to noop.
        let store = Store::open(Path::new(":memory:")).unwrap();
        let config = Config {
            model_command: None,
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
