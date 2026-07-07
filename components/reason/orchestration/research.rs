//! Research-to-formal stage engine (plan §10).
//!
//! Drives the informal→formal ladder with a model provider, persisting the
//! typed claim-DAG: object/Setting nodes, candidate-claim nodes with a
//! screening status, and a formalization-target node carrying the Lean
//! signature stub and symbol dictionary. Enforces the hard rule that numerics
//! only *screen* (a passing screen sets `informally_verified` at most, never
//! `formally_verified`).

use crate::{
    db::Store,
    model::{ModelRequest, NodeKind, NodeStatus, NodeTier},
    provider::ModelProvider,
};
use anyhow::{Context, Result};
use serde_json::json;

pub struct ResearchEngine<'a> {
    pub store: &'a Store,
    pub provider: &'a dyn ModelProvider,
}

#[derive(Debug, serde::Serialize)]
pub struct ResearchSummary {
    pub project_id: String,
    pub run_id: String,
    pub created_nodes: usize,
    pub stages_run: Vec<String>,
    pub notes: Vec<String>,
}

struct Candidate {
    title: String,
    statement: String,
    type_label: String,
    status: String,
}

impl ResearchEngine<'_> {
    pub fn run(&self, project_id: &str) -> Result<ResearchSummary> {
        let project = self.store.project(project_id)?;
        let run = self
            .store
            .begin_run(project_id, "research_to_formal_stages")?;
        let mut created = 0usize;
        let mut stages_run = Vec::new();
        let mut notes = Vec::new();

        // Stage: object identification -> Definition/Setting nodes.
        self.store
            .update_run(project_id, &run, "running", "objects", 0)?;
        match self.stage_objects(&project.theorem) {
            Ok(objects) => {
                for (name, desc) in objects {
                    self.store.add_node_detailed(
                        project_id,
                        NodeKind::Definition,
                        NodeTier::Spine,
                        None,
                        &name,
                        &desc,
                        None,
                        &[],
                        "research:objects",
                    )?;
                    created += 1;
                }
                stages_run.push("objects".into());
            }
            Err(e) => notes.push(format!("object identification unavailable: {e}")),
        }

        // Stage: candidate discovery -> Conjecture nodes with a screening status.
        // Numerics screen only: a "pass" is informally_verified, never certified.
        self.store
            .update_run(project_id, &run, "running", "candidates", 0)?;
        match self.stage_candidates(&project.theorem) {
            Ok(candidates) => {
                for c in candidates {
                    let node = self.store.add_node_detailed(
                        project_id,
                        NodeKind::Conjecture,
                        NodeTier::Spine,
                        None,
                        &c.title,
                        &c.statement,
                        Some(&c.type_label),
                        &[],
                        "research:candidates",
                    )?;
                    match c.status.as_str() {
                        "pass" => self.store.set_node_status(
                            project_id,
                            &node.id,
                            NodeStatus::InformallyVerified,
                            "research:numeric_screen",
                        )?,
                        "fail" => self.store.set_node_status(
                            project_id,
                            &node.id,
                            NodeStatus::Rejected,
                            "research:numeric_screen",
                        )?,
                        _ => {}
                    }
                    created += 1;
                }
                stages_run.push("candidates".into());
            }
            Err(e) => notes.push(format!("candidate discovery unavailable: {e}")),
        }

        // Stage: formalization target -> FormalStatement node with a Lean stub.
        self.store
            .update_run(project_id, &run, "running", "formalization_target", 0)?;
        match self.stage_formalization_target(&project.theorem) {
            Ok((signature, dictionary)) => {
                let node = self.store.add_node_detailed(
                    project_id,
                    NodeKind::FormalStatement,
                    NodeTier::Spine,
                    None,
                    "Formalization target",
                    &project.theorem,
                    None,
                    &[],
                    "research:formalization_target",
                )?;
                self.store
                    .set_formal_statement(project_id, &node.id, &signature, "research")?;
                self.store.add_evidence(
                    project_id,
                    &node.id,
                    "symbol_dictionary",
                    "research",
                    "recorded",
                    dictionary,
                )?;
                created += 1;
                stages_run.push("formalization_target".into());
            }
            Err(e) => notes.push(format!("formalization target unavailable: {e}")),
        }

        let state = if stages_run.is_empty() {
            "completed_no_model"
        } else {
            "completed"
        };
        self.store
            .update_run(project_id, &run, state, "complete", 0)?;
        Ok(ResearchSummary {
            project_id: project_id.into(),
            run_id: run,
            created_nodes: created,
            stages_run,
            notes,
        })
    }

    fn stage_objects(&self, theorem: &str) -> Result<Vec<(String, String)>> {
        let response = self.provider.complete(&ModelRequest {
            role: "object_identification".into(),
            task: "Identify the precise mathematical objects (spaces, functions, constants) named on both sides of the statement.".into(),
            context: json!({ "theorem": theorem }),
            output_schema: json!({"type":"object","required":["objects"],"properties":{
                "objects":{"type":"array","items":{"type":"object","required":["name","description"],
                    "properties":{"name":{"type":"string"},"description":{"type":"string"}}}}}}),
        })?;
        Ok(response.content["objects"]
            .as_array()
            .context("missing objects")?
            .iter()
            .map(|o| {
                (
                    o["name"].as_str().unwrap_or("object").into(),
                    o["description"].as_str().unwrap_or("").into(),
                )
            })
            .collect())
    }

    fn stage_candidates(&self, theorem: &str) -> Result<Vec<Candidate>> {
        let response = self.provider.complete(&ModelRequest {
            role: "candidate_discovery".into(),
            task: "Propose candidate claims, each with a type label and a pass/fail/inconclusive screening status. Numerics only SCREEN; never mark a claim proved.".into(),
            context: json!({ "theorem": theorem }),
            output_schema: json!({"type":"object","required":["candidates"],"properties":{
                "candidates":{"type":"array","items":{"type":"object",
                    "required":["title","statement","type_label","status"],
                    "properties":{"title":{"type":"string"},"statement":{"type":"string"},
                        "type_label":{"type":"string"},"status":{"type":"string"}}}}}}),
        })?;
        Ok(response.content["candidates"]
            .as_array()
            .context("missing candidates")?
            .iter()
            .map(|c| Candidate {
                title: c["title"].as_str().unwrap_or("Candidate").into(),
                statement: c["statement"].as_str().unwrap_or("").into(),
                type_label: c["type_label"].as_str().unwrap_or("claim").into(),
                status: c["status"].as_str().unwrap_or("inconclusive").into(),
            })
            .collect())
    }

    fn stage_formalization_target(&self, theorem: &str) -> Result<(String, serde_json::Value)> {
        let response = self.provider.complete(&ModelRequest {
            role: "formalization_target".into(),
            task: "Produce a Lean 4 theorem signature stub (statement only, ending in `:= by sorry`) and a symbol dictionary mapping paper symbols to Lean definitions.".into(),
            context: json!({ "theorem": theorem }),
            output_schema: json!({"type":"object","required":["lean_signature","symbol_dictionary"],
                "properties":{"lean_signature":{"type":"string"},"symbol_dictionary":{"type":"object"}}}),
        })?;
        Ok((
            response.content["lean_signature"]
                .as_str()
                .context("missing lean_signature")?
                .into(),
            response.content["symbol_dictionary"].clone(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelResponse;
    use std::path::Path;

    struct StageProvider;
    impl ModelProvider for StageProvider {
        fn complete(&self, request: &ModelRequest) -> Result<ModelResponse> {
            let content = match request.role.as_str() {
                "object_identification" => {
                    json!({"objects":[{"name":"n","description":"an integer"}]})
                }
                "candidate_discovery" => json!({"candidates":[
                    {"title":"Parity","statement":"n even","type_label":"Invariant","status":"pass"}
                ]}),
                "formalization_target" => json!({
                    "lean_signature":"theorem t : True := by sorry",
                    "symbol_dictionary":{"n":"Nat"}
                }),
                _ => json!({}),
            };
            Ok(ModelResponse {
                content,
                model: "test".into(),
                provider: "test".into(),
            })
        }
        fn name(&self) -> &str {
            "test"
        }
    }

    #[test]
    fn runs_stages_and_persists_nodes() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store
            .create_project("p", "every even square is even")
            .unwrap();
        let engine = ResearchEngine {
            store: &store,
            provider: &StageProvider,
        };
        let summary = engine.run(&project.id).unwrap();
        assert_eq!(summary.stages_run.len(), 3);
        let nodes = store.nodes(&project.id).unwrap();
        assert!(nodes.iter().any(|n| n.kind == NodeKind::Definition));
        assert!(nodes
            .iter()
            .any(|n| n.kind == NodeKind::Conjecture && n.status == NodeStatus::InformallyVerified));
        assert!(nodes
            .iter()
            .any(|n| n.kind == NodeKind::FormalStatement && n.formal_statement.is_some()));
    }
}
