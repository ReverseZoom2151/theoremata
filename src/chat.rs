use crate::{
    db::Store,
    model::{EdgeKind, ModelRequest, ModelStreamEvent, NodeKind, NodeStatus},
    provider::ModelProvider,
};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};

pub struct ChatEngine<'a> {
    pub store: &'a Store,
    pub provider: &'a dyn ModelProvider,
}

impl ChatEngine<'_> {
    pub fn send(&self, project_id: &str, text: &str) -> Result<String> {
        self.send_stream(project_id, text, &mut |_| {})
    }

    pub fn send_stream(
        &self,
        project_id: &str,
        text: &str,
        on_event: &mut dyn FnMut(ModelStreamEvent),
    ) -> Result<String> {
        self.store
            .add_message(project_id, "user", text, json!({}))?;
        let graph = self.store.export(project_id)?;
        let history = self.store.messages(project_id, 30)?;
        let request = ModelRequest {
            role: "mathematical_research_orchestrator".into(),
            task: text.into(),
            context: json!({
                "project":graph.project,
                "nodes":graph.nodes,
                "edges":graph.edges,
                "conversation":history,
                "policy":{
                    "graph_is_source_of_truth":true,
                    "never_claim_formal_verification_without_tool_evidence":true,
                    "propose_small_atomic_mutations":true
                }
            }),
            output_schema: json!({
                "type":"object",
                "required":["reply","mutations"],
                "properties":{
                    "reply":{"type":"string"},
                    "mutations":{"type":"array","items":{
                        "type":"object","required":["action"],
                        "properties":{
                            "action":{"enum":["add_node","add_edge","set_status","set_formal_statement"]},
                            "kind":{"type":"string"},"title":{"type":"string"},"statement":{"type":"string"},
                            "source":{"type":"string"},"target":{"type":"string"},"node_id":{"type":"string"},
                            "status":{"type":"string"},"formal_statement":{"type":"string"}
                        }
                    }}
                }
            }),
        };
        let response = self.provider.stream(&request, on_event)?;
        let reply = response.content["reply"]
            .as_str()
            .or_else(|| response.content["message"].as_str())
            .unwrap_or("The provider returned no textual reply.")
            .to_owned();
        let mut proposal_count = 0;
        if let Some(mutations) = response.content["mutations"].as_array() {
            for mutation in mutations {
                self.validate_mutation(mutation)?;
                self.store
                    .add_proposal(project_id, mutation.clone(), "model")?;
                proposal_count += 1;
            }
        }
        self.store.add_message(
            project_id,
            "assistant",
            &reply,
            json!({"provider":response.provider,"model":response.model,"proposals":proposal_count}),
        )?;
        Ok(if proposal_count > 0 {
            format!("{reply}\n\n[{proposal_count} graph mutation proposal(s) awaiting approval]")
        } else {
            reply
        })
    }

    pub fn approve(&self, project_id: &str, proposal_id: &str) -> Result<()> {
        let proposal = self.store.proposal(project_id, proposal_id)?;
        if proposal.status != "pending" {
            return Err(anyhow!("proposal already resolved"));
        }
        self.apply_mutation(project_id, &proposal.action)?;
        self.store
            .resolve_proposal(project_id, proposal_id, "approved", "applied by user")
    }

    pub fn reject(&self, project_id: &str, proposal_id: &str, note: &str) -> Result<()> {
        self.store
            .resolve_proposal(project_id, proposal_id, "rejected", note)
    }

    fn validate_mutation(&self, m: &Value) -> Result<()> {
        match m["action"].as_str().unwrap_or("") {
            "add_node" => {
                required(m, "title")?;
                required(m, "statement")?;
                m["kind"]
                    .as_str()
                    .unwrap_or("obligation")
                    .parse::<NodeKind>()?;
            }
            "add_edge" => {
                required(m, "source")?;
                required(m, "target")?;
                m["kind"]
                    .as_str()
                    .unwrap_or("depends_on")
                    .parse::<EdgeKind>()?;
            }
            "set_status" => {
                required(m, "node_id")?;
                let status = required(m, "status")?.parse::<NodeStatus>()?;
                if status == NodeStatus::FormallyVerified {
                    return Err(anyhow!("models cannot propose formal certification"));
                }
            }
            "set_formal_statement" => {
                required(m, "node_id")?;
                required(m, "formal_statement")?;
            }
            other => return Err(anyhow!("unsupported mutation action: {other}")),
        }
        Ok(())
    }

    fn apply_mutation(&self, project_id: &str, m: &Value) -> Result<()> {
        match m["action"].as_str().unwrap_or("") {
            "add_node" => {
                let kind = m["kind"]
                    .as_str()
                    .unwrap_or("obligation")
                    .parse::<NodeKind>()?;
                self.store.add_node(
                    project_id,
                    kind,
                    required(m, "title")?,
                    required(m, "statement")?,
                    "model",
                )?;
            }
            "add_edge" => {
                self.store.add_edge(
                    project_id,
                    required(m, "source")?,
                    required(m, "target")?,
                    m["kind"]
                        .as_str()
                        .unwrap_or("depends_on")
                        .parse::<EdgeKind>()?,
                )?;
            }
            "set_status" => {
                let status = required(m, "status")?.parse::<NodeStatus>()?;
                // Models may not grant themselves formal verification.
                if status == NodeStatus::FormallyVerified {
                    return Err(anyhow!(
                        "model mutation cannot grant formally_verified status"
                    ));
                }
                self.store
                    .set_node_status(project_id, required(m, "node_id")?, status, "model")?;
            }
            "set_formal_statement" => {
                self.store.set_formal_statement(
                    project_id,
                    required(m, "node_id")?,
                    required(m, "formal_statement")?,
                    "model",
                )?;
            }
            other => return Err(anyhow!("unsupported mutation action: {other}")),
        }
        Ok(())
    }
}
fn required<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .ok_or_else(|| anyhow!("mutation missing {key}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelResponse;
    use std::path::Path;

    struct ProposingProvider;
    impl ModelProvider for ProposingProvider {
        fn complete(&self, _: &ModelRequest) -> Result<ModelResponse> {
            Ok(ModelResponse {
                provider: "test".into(),
                model: "test".into(),
                content: json!({
                    "reply":"I propose an obligation.",
                    "mutations":[{
                        "action":"add_node","kind":"obligation",
                        "title":"Check parity","statement":"Prove the parity implication."
                    }]
                }),
            })
        }
        fn name(&self) -> &str {
            "test"
        }
    }

    #[test]
    fn model_mutations_require_approval() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store
            .create_project("test", "Every even square is even")
            .unwrap();
        let engine = ChatEngine {
            store: &store,
            provider: &ProposingProvider,
        };
        engine.send(&project.id, "decompose it").unwrap();
        assert!(store.nodes(&project.id).unwrap().is_empty());
        let proposals = store.proposals(&project.id, true).unwrap();
        assert_eq!(proposals.len(), 1);
        engine.approve(&project.id, &proposals[0].id).unwrap();
        assert_eq!(store.nodes(&project.id).unwrap().len(), 1);
        assert!(store.proposals(&project.id, true).unwrap().is_empty());
    }
}
