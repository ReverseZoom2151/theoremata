use crate::{
    config::Config,
    db::Store,
    model::{EdgeKind, EdgeStrength, ModelRequest, NodeKind, NodeStatus},
    provider::ModelProvider,
    retry::{Decision, RetryLimits, RetryState},
    tools::{LeanCheck, MathlibSearch, PythonCheck, Tool},
};
use anyhow::{Context, Result};
use serde_json::json;
use std::{fs, path::PathBuf};

#[derive(Debug, serde::Serialize)]
pub struct WorkflowSummary {
    pub run_id: String,
    pub project_id: String,
    pub created_nodes: usize,
    pub lean_file: PathBuf,
    pub state: String,
    pub notes: Vec<String>,
}

pub struct ResearchWorkflow<'a> {
    pub store: &'a Store,
    pub config: &'a Config,
    pub provider: &'a dyn ModelProvider,
}

/// Find the first declared theorem/lemma name in a Lean source, so its axiom
/// closure can be audited by name.
fn extract_theorem_name(src: &str) -> Option<String> {
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

/// Pull Lean declaration names out of ripgrep hits over Mathlib, so an
/// obligation node can carry concrete lemmas to try as its `suggested_lemmas`.
fn extract_lemma_names(stdout: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in stdout.lines() {
        for kw in ["theorem ", "lemma "] {
            if let Some(idx) = line.find(kw) {
                let name: String = line[idx + kw.len()..]
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '\''))
                    .collect();
                if !name.is_empty() && !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        if names.len() >= 8 {
            break;
        }
    }
    names
}

impl ResearchWorkflow<'_> {
    pub fn run(&self, project_id: &str) -> Result<WorkflowSummary> {
        let project = self.store.project(project_id)?;
        let run = self.store.begin_run(project_id, "research_to_formal")?;
        let mut created = 0;
        let mut notes = Vec::new();

        self.store
            .update_run(project_id, &run, "running", "normalize", 0)?;
        let conjecture = self.store.add_node(
            project_id,
            NodeKind::Conjecture,
            "Main conjecture",
            project.theorem.trim(),
            "workflow:normalize",
        )?;
        self.store.set_node_status(
            project_id,
            &conjecture.id,
            NodeStatus::Active,
            "orchestrator",
        )?;
        created += 1;

        let strategy=self.store.add_node(project_id,NodeKind::Strategy,
            "Initial proof strategy",
            "Audit assumptions, search for counterexamples, decompose the claim, then formalize only stable obligations.",
            "workflow:strategy")?;
        self.store.add_edge(
            project_id,
            &conjecture.id,
            &strategy.id,
            EdgeKind::DependsOn,
        )?;
        created += 1;

        self.store
            .update_run(project_id, &run, "running", "falsify", 0)?;
        let computation = self.store.add_node(
            project_id,
            NodeKind::Computation,
            "Computational falsification",
            "Run bounded checks when the statement admits an executable interpretation.",
            "workflow:falsify",
        )?;
        self.store.add_edge(
            project_id,
            &strategy.id,
            &computation.id,
            EdgeKind::DependsOn,
        )?;
        created += 1;
        let py = PythonCheck::new();
        // Derive the bounded check from the actual theorem — the model emits the
        // executable falsifier, the generic worker runs it. Numerics only screen.
        let verdict = crate::falsification::Falsifier {
            provider: self.provider,
        }
        .falsify(&project.theorem)?;
        let details = serde_json::to_value(&verdict)?;
        self.store.add_evidence(
            project_id,
            &computation.id,
            "falsification",
            "falsifier",
            &verdict.verdict,
            details.clone(),
        )?;
        self.store.add_attempt(
            project_id,
            Some(&computation.id),
            Some(&run),
            "falsifier",
            &json!({ "statement": project.theorem }),
            &details,
            verdict.verdict != "counterexample",
        )?;
        match verdict.verdict.as_str() {
            "counterexample" => notes.push(format!(
                "Counterexample found for the conjecture: {}",
                verdict.assignment.clone().unwrap_or(serde_json::Value::Null)
            )),
            "no_counterexample_in_domain" => self.store.set_node_status(
                project_id,
                &computation.id,
                NodeStatus::InformallyVerified,
                "falsifier",
            )?,
            other => notes.push(format!(
                "Falsification screen did not run to a verdict ({other}); obligation remains open"
            )),
        }

        self.store
            .update_run(project_id, &run, "running", "decompose", 0)?;
        let obligations = self.decompose_bounded(project_id, &run, &project.theorem, &mut notes)?;
        for (title, statement) in obligations {
            let node = self.store.add_node(
                project_id,
                NodeKind::Obligation,
                &title,
                &statement,
                "workflow:decompose",
            )?;
            self.store
                .add_edge(project_id, &strategy.id, &node.id, EdgeKind::DependsOn)?;
            created += 1;
            self.store.set_strategy_hint(
                project_id,
                &node.id,
                Some("Search Mathlib for a matching lemma before attempting a manual proof."),
                "workflow:decompose",
            )?;
            let search = MathlibSearch::new(self.config);
            if search.available() {
                let terms = title
                    .split_whitespace()
                    .take(3)
                    .collect::<Vec<_>>()
                    .join("|");
                let result = search.run(json!({"query":terms,"limit":8}))?;
                let lemmas = extract_lemma_names(&result.stdout);
                if !lemmas.is_empty() {
                    self.store
                        .set_suggested_lemmas(project_id, &node.id, &lemmas, "workflow:retrieval")?;
                }
                self.store.add_evidence(
                    project_id,
                    &node.id,
                    "retrieval",
                    "mathlib_search",
                    if result.stdout.is_empty() {
                        "none"
                    } else {
                        "candidates"
                    },
                    serde_json::to_value(result)?,
                )?;
            }
        }

        self.store
            .update_run(project_id, &run, "running", "formalize", 0)?;
        let formal=self.formalize(&project.theorem).unwrap_or_else(|error|{
            notes.push(format!("Model formalization unavailable: {error}"));
            format!("-- Formal statement requires a configured model provider.\n-- Informal theorem: {}",project.theorem)
        });
        let formal_node = self.store.add_node(
            project_id,
            NodeKind::FormalStatement,
            "Lean statement",
            &project.theorem,
            "workflow:formalize",
        )?;
        self.store
            .set_formal_statement(project_id, &formal_node.id, &formal, "orchestrator")?;
        self.store.add_edge(
            project_id,
            &conjecture.id,
            &formal_node.id,
            EdgeKind::Formalizes,
        )?;
        created += 1;

        let dir = self.config.workspace.join(project_id);
        fs::create_dir_all(&dir)?;
        let lean_file = dir.join("Main.lean");
        let source = if formal.trim_start().starts_with("import ") {
            formal.clone()
        } else {
            format!("import Mathlib\n\nnamespace Theoremata\n\n{formal}\n\nend Theoremata\n")
        };
        fs::write(&lean_file, source)?;

        self.store
            .update_run(project_id, &run, "running", "verify", 0)?;

        // Cheap deterministic pre-gate: reject sorry/admit/axiom lexically
        // (ignoring comments and strings) before trusting any compilation.
        let mut soundness_clean = true;
        if py.available() {
            let sound = py.run(json!({"tool":"lean_soundness","text":formal}))?;
            soundness_clean = sound.success
                && serde_json::from_str::<serde_json::Value>(&sound.stdout)
                    .ok()
                    .and_then(|v| v["output"]["clean"].as_bool())
                    .unwrap_or(false);
            self.store.add_evidence(
                project_id,
                &formal_node.id,
                "lexical_soundness",
                py.name(),
                if soundness_clean { "clean" } else { "flagged" },
                serde_json::to_value(&sound)?,
            )?;
            if !soundness_clean {
                notes.push(
                    "Lexical soundness gate flagged the formal statement (sorry/admit/axiom)".into(),
                );
            }
        }

        let lean = LeanCheck::new(self.config);
        if lean.available() {
            let lean_input = json!({ "file": lean_file });
            let result = lean.run(lean_input.clone())?;
            let verdict = if result.success && soundness_clean {
                "pass"
            } else if !soundness_clean {
                "unsound"
            } else {
                "fail"
            };
            self.store.add_evidence(
                project_id,
                &formal_node.id,
                "lean_compile",
                "lean",
                verdict,
                serde_json::to_value(&result)?,
            )?;
            self.store.add_attempt(
                project_id,
                Some(&formal_node.id),
                Some(&run),
                "lean",
                &lean_input,
                &serde_json::to_value(&result)?,
                result.success,
            )?;
            // Authoritative gate: certify only when the proof compiles, is
            // lexically clean, AND its axiom closure is within the allowlist.
            let mut certified = false;
            if result.success && soundness_clean {
                match extract_theorem_name(&formal) {
                    Some(thm) if py.available() => {
                        let source = fs::read_to_string(&lean_file).unwrap_or_default();
                        let audit = py.run(json!({
                            "tool":"check_axioms","source":source,"theorem":thm,
                            "root":self.config.lean_project
                        }))?;
                        let axioms_clean = serde_json::from_str::<serde_json::Value>(&audit.stdout)
                            .ok()
                            .and_then(|v| v["output"]["clean"].as_bool())
                            .unwrap_or(false);
                        self.store.add_evidence(
                            project_id,
                            &formal_node.id,
                            "axiom_audit",
                            py.name(),
                            if axioms_clean { "clean" } else { "flagged" },
                            serde_json::to_value(&audit)?,
                        )?;
                        certified = axioms_clean;
                        if !axioms_clean {
                            notes.push("Axiom audit flagged the proof; not certifying".into());
                        }
                    }
                    _ => notes
                        .push("No theorem name to audit; formal statement not certified".into()),
                }
            }
            if certified {
                self.store.set_node_status(
                    project_id,
                    &formal_node.id,
                    NodeStatus::FormallyVerified,
                    "lean",
                )?;
                self.store.set_edge_strength(
                    project_id,
                    &conjecture.id,
                    &formal_node.id,
                    EdgeKind::Formalizes,
                    EdgeStrength::LeanChecked,
                )?;
            } else {
                notes.push(format!(
                    "Lean verification did not certify the statement: {verdict}"
                ));
            }
        } else {
            notes.push("Lean/Lake unavailable; formal verification remains open".into());
        }

        let state = if self
            .store
            .nodes(project_id)?
            .iter()
            .any(|n| n.status == NodeStatus::FormallyVerified)
        {
            "completed_with_certificate"
        } else {
            "completed_with_open_obligations"
        };
        self.store
            .update_run(project_id, &run, state, "complete", 0)?;
        Ok(WorkflowSummary {
            run_id: run,
            project_id: project_id.into(),
            created_nodes: created,
            lean_file,
            state: state.into(),
            notes,
        })
    }

    /// Decompose the theorem into obligations, bounded by the QED-style retry
    /// policy when a model provider is configured. Each model attempt is
    /// recorded; if the provider is offline or the escalation budget is spent,
    /// fall back to a fixed obligation skeleton so the workflow still proceeds.
    fn decompose_bounded(
        &self,
        project_id: &str,
        run: &str,
        theorem: &str,
        notes: &mut Vec<String>,
    ) -> Result<Vec<(String, String)>> {
        let fallback = || {
            vec![
                (
                    "Assumption audit".to_string(),
                    "Make every domain, quantifier, and side condition explicit.".to_string(),
                ),
                (
                    "Core implication".to_string(),
                    "Establish the main mathematical implication from the normalized assumptions."
                        .to_string(),
                ),
                (
                    "Edge-case audit".to_string(),
                    "Check boundary, degenerate, and excluded cases.".to_string(),
                ),
            ]
        };
        if self.provider.name() == "offline" {
            notes.push("Model decomposition unavailable: no model provider configured".into());
            return Ok(fallback());
        }
        let mut state = RetryState::new(RetryLimits::default());
        loop {
            let outcome = self.decompose(theorem);
            match outcome {
                Ok(obligations) if !obligations.is_empty() => {
                    self.store.add_attempt(
                        project_id,
                        None,
                        Some(run),
                        "proof_decomposer",
                        &json!({ "theorem": theorem }),
                        &json!({ "obligations": obligations.len() }),
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
                        Some(run),
                        "proof_decomposer",
                        &json!({ "theorem": theorem }),
                        &json!({ "error": detail }),
                        false,
                    )?;
                    if state.resolve(Decision::ReviseProof) == Decision::Terminate {
                        notes.push(format!("Model decomposition failed after retries: {detail}"));
                        return Ok(fallback());
                    }
                }
            }
        }
    }

    fn decompose(&self, theorem: &str) -> Result<Vec<(String, String)>> {
        let response=self.provider.complete(&ModelRequest{
            role:"proof_decomposer".into(),task:"Decompose into independently verifiable obligations.".into(),
            context:json!({"theorem":theorem}),
            output_schema:json!({"type":"object","required":["obligations"],"properties":{
                "obligations":{"type":"array","items":{"type":"object","required":["title","statement"],
                    "properties":{"title":{"type":"string"},"statement":{"type":"string"}}}}}})
        })?;
        Ok(response.content["obligations"]
            .as_array()
            .context("missing obligations")?
            .iter()
            .map(|x| {
                (
                    x["title"].as_str().unwrap_or("Obligation").into(),
                    x["statement"].as_str().unwrap_or("").into(),
                )
            })
            .collect())
    }
    fn formalize(&self, theorem: &str) -> Result<String> {
        let response=self.provider.complete(&ModelRequest{
            role:"lean_formalizer".into(),task:"Produce a complete Lean 4 file. Never use sorry, admit, axioms, or unsafe declarations.".into(),
            context:json!({"theorem":theorem}),
            output_schema:json!({"type":"object","required":["lean"],"properties":{"lean":{"type":"string"}}})
        })?;
        Ok(response.content["lean"]
            .as_str()
            .context("missing lean")?
            .into())
    }
}
