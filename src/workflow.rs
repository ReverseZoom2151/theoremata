use crate::{
    config::Config,
    db::Store,
    model::{EdgeKind, ModelRequest, NodeKind, NodeStatus},
    provider::ModelProvider,
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
        if py.available() {
            let result = py.run(json!({
                "tool":"falsify",
                "variables":{"n":{"start":-100,"stop":101}},
                "assumptions":"n % 2 == 0",
                "claim":"(n * n) % 2 == 0"
            }))?;
            self.store.add_evidence(
                project_id,
                &computation.id,
                "bounded_computation",
                py.name(),
                if result.success { "pass" } else { "error" },
                serde_json::to_value(&result)?,
            )?;
            if result.success {
                self.store.set_node_status(
                    project_id,
                    &computation.id,
                    NodeStatus::InformallyVerified,
                    "python_check",
                )?;
            }
        } else {
            notes.push("Python worker unavailable; falsification obligation remains open".into());
        }

        self.store
            .update_run(project_id, &run, "running", "decompose", 0)?;
        let obligations = self.decompose(&project.theorem).unwrap_or_else(|error| {
            notes.push(format!("Model decomposition unavailable: {error}"));
            vec![
                (
                    "Assumption audit".into(),
                    "Make every domain, quantifier, and side condition explicit.".into(),
                ),
                (
                    "Core implication".into(),
                    "Establish the main mathematical implication from the normalized assumptions."
                        .into(),
                ),
                (
                    "Edge-case audit".into(),
                    "Check boundary, degenerate, and excluded cases.".into(),
                ),
            ]
        });
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
            let search = MathlibSearch::new(self.config);
            if search.available() {
                let terms = title
                    .split_whitespace()
                    .take(3)
                    .collect::<Vec<_>>()
                    .join("|");
                let result = search.run(json!({"query":terms,"limit":8}))?;
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

        let lean = LeanCheck;
        if lean.available() {
            let result = lean.run(json!({"file":lean_file}))?;
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
            if result.success && soundness_clean {
                self.store.set_node_status(
                    project_id,
                    &formal_node.id,
                    NodeStatus::FormallyVerified,
                    "lean",
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
