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

/// The CLOSED set of actions the chat model may request the cockpit to run.
/// The model selects a `tool` from this fixed enum; the TUI maps each variant
/// to a known real function. An unknown tool name is dropped by `parse_actions`
/// and NEVER executed, so the model can never name a shell command, path, or
/// tool of its own. None of these can mark a node formally verified: `Prove`
/// and `Hammer` run the real gate and only RETURN a report; the graph status is
/// changed solely by the verification pipeline, never by a model reply.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatAction {
    /// Formalize + prove + gate. `target` is a node id/index or a raw statement;
    /// the caller resolves which. `system` is lean/rocq/isabelle.
    Prove { system: String, target: String },
    /// Numeric falsification worker: search for a counterexample.
    Falsify { variables: Value, claim: String },
    /// Hammer-assisted proof search for a native goal.
    Hammer { system: String, goal: String },
    /// Staleness census over stored verified results.
    Sweep,
}

impl ChatAction {
    /// Stable tool name, matching the closed schema enum.
    pub fn tool_name(&self) -> &'static str {
        match self {
            ChatAction::Prove { .. } => "prove",
            ChatAction::Falsify { .. } => "falsify",
            ChatAction::Hammer { .. } => "hammer",
            ChatAction::Sweep => "sweep",
        }
    }
}

/// One agentic turn: the model's textual reply, how many graph-mutation
/// proposals it filed, and the (already validated, closed-set) actions it asked
/// the cockpit to run. The cockpit executes the actions; the engine never does.
#[derive(Debug, Clone)]
pub struct ChatTurn {
    pub reply: String,
    pub proposals: usize,
    pub actions: Vec<ChatAction>,
}

/// Parse the model's `actions` array into the closed [`ChatAction`] set.
/// Unknown tool names and malformed entries are dropped, never executed. Pure
/// (no store/network), so it is unit-tested directly.
pub fn parse_actions(content: &Value) -> Vec<ChatAction> {
    let mut out = Vec::new();
    let Some(items) = content["actions"].as_array() else {
        return out;
    };
    for a in items {
        match a["tool"].as_str().unwrap_or("") {
            "prove" => {
                let system = a["system"].as_str().unwrap_or("lean").to_string();
                // Accept either an explicit statement or a node reference; both
                // are carried as `target` and resolved at execution time.
                let target = a["statement"]
                    .as_str()
                    .or_else(|| a["node"].as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if !target.is_empty() {
                    out.push(ChatAction::Prove { system, target });
                }
            }
            "falsify" => {
                let variables = a["variables"].clone();
                let claim = a["claim"].as_str().unwrap_or("").trim().to_string();
                // Falsify needs a variable->domain object and a claim; skip if
                // either is absent rather than run a malformed worker request.
                if !claim.is_empty() && variables.is_object() {
                    out.push(ChatAction::Falsify { variables, claim });
                }
            }
            "hammer" => {
                let system = a["system"].as_str().unwrap_or("lean").to_string();
                let goal = a["goal"].as_str().unwrap_or("").trim().to_string();
                if !goal.is_empty() {
                    out.push(ChatAction::Hammer { system, goal });
                }
            }
            "sweep" => out.push(ChatAction::Sweep),
            // Unknown tool: ignored on purpose (closed set, fail safe).
            _ => {}
        }
    }
    out
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

    /// One agentic turn for the cockpit. Like `send_stream`, but the output
    /// schema also carries a CLOSED `actions` array the model may use to request
    /// real work (prove/falsify/hammer/sweep). This engine only PARSES and
    /// returns those actions plus its usual reply and proposals; the TUI runs
    /// them, appends the results to the conversation as `tool` messages, and
    /// may call this again so the model can react.
    ///
    /// `record_user` adds `task` to the conversation as a user message on the
    /// FIRST turn of a user request; continuation turns pass `false` so the
    /// growing tool-result history drives the next turn without a fake user
    /// line. Soundness is unchanged: the same policy is sent, mutations are
    /// still validated (a model can never propose `formally_verified`), and no
    /// action here can write a verified status.
    pub fn send_turn(
        &self,
        project_id: &str,
        task: &str,
        record_user: bool,
        on_event: &mut dyn FnMut(ModelStreamEvent),
    ) -> Result<ChatTurn> {
        if record_user {
            self.store.add_message(project_id, "user", task, json!({}))?;
        }
        let graph = self.store.export(project_id)?;
        let history = self.store.messages(project_id, 30)?;
        let request = ModelRequest {
            role: "mathematical_research_orchestrator".into(),
            task: task.into(),
            context: json!({
                "project":graph.project,
                "nodes":graph.nodes,
                "edges":graph.edges,
                "conversation":history,
                "policy":{
                    "graph_is_source_of_truth":true,
                    "never_claim_formal_verification_without_tool_evidence":true,
                    "propose_small_atomic_mutations":true,
                    "a_reply_is_text_not_a_verdict":true
                },
                // The cockpit can RUN these; describing them lets the model
                // request real work instead of only asserting results. This is
                // advisory context; the executable set is the closed schema enum
                // below, and anything else the model emits is ignored.
                "available_actions":{
                    "prove":"formalize + prove + gate a node or statement (fields: system, statement | node)",
                    "falsify":"search for a numeric counterexample (fields: variables object, claim)",
                    "hammer":"hammer-assisted native proof search (fields: system, goal)",
                    "sweep":"staleness census over stored verified results (no fields)"
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
                    }},
                    "actions":{"type":"array","items":{
                        "type":"object","required":["tool"],
                        "properties":{
                            "tool":{"enum":["prove","falsify","hammer","sweep"]},
                            "system":{"type":"string"},"statement":{"type":"string"},
                            "node":{"type":"string"},"goal":{"type":"string"},
                            "claim":{"type":"string"},"variables":{"type":"object"}
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
                // Same validation gate as `send_stream`: a model can never
                // propose a `formally_verified` status change.
                self.validate_mutation(mutation)?;
                self.store
                    .add_proposal(project_id, mutation.clone(), "model")?;
                proposal_count += 1;
            }
        }
        let actions = parse_actions(&response.content);
        self.store.add_message(
            project_id,
            "assistant",
            &reply,
            json!({
                "provider":response.provider,"model":response.model,
                "proposals":proposal_count,"actions":actions.len()
            }),
        )?;
        Ok(ChatTurn {
            reply,
            proposals: proposal_count,
            actions,
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

    /// Under a tiered approval policy, auto-approve the pending proposals that
    /// are low-risk (ordinary node/edge additions), leaving consequential ones
    /// (status changes, formal statements, formal/verified nodes) for a human.
    /// Returns how many were auto-approved; a no-op when the policy is off.
    /// Callers invoke this after `send`/`send_stream`.
    pub fn resolve_auto_approvals(
        &self,
        project_id: &str,
        auto_approve_safe: bool,
    ) -> Result<usize> {
        if !auto_approve_safe {
            return Ok(0);
        }
        let mut approved = 0;
        for proposal in self.store.proposals(project_id, true)? {
            if is_low_risk(&proposal.action) {
                self.approve(project_id, &proposal.id)?;
                approved += 1;
            }
        }
        Ok(approved)
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

/// Classify a proposed graph mutation for tiered approval. Low-risk mutations
/// (adding an ordinary node or a dependency edge) may be auto-approved;
/// everything that changes status, sets a formal statement, or introduces a
/// formal/verified node stays gated for a human.
pub fn is_low_risk(action: &Value) -> bool {
    match action["action"].as_str().unwrap_or("") {
        "add_node" => matches!(
            action["kind"].as_str().unwrap_or("obligation"),
            "conjecture"
                | "definition"
                | "assumption"
                | "strategy"
                | "lemma"
                | "obligation"
                | "computation"
        ),
        "add_edge" => true,
        _ => false,
    }
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

    #[test]
    fn parse_actions_keeps_only_known_tools() {
        // A mix of valid actions and one unknown/forged tool name. The unknown
        // one must be dropped: the cockpit executes a closed set only.
        let content = json!({
            "actions":[
                {"tool":"prove","system":"lean","statement":"1 + 1 = 2"},
                {"tool":"prove","node":"3"},
                {"tool":"falsify","variables":{"n":"int"},"claim":"n*n >= 0"},
                {"tool":"hammer","system":"isabelle","goal":"1 + 1 = (2::nat)"},
                {"tool":"sweep"},
                {"tool":"rm","path":"/etc/passwd"},
                {"tool":"exec","cmd":"shutdown"}
            ]
        });
        let actions = parse_actions(&content);
        assert_eq!(actions.len(), 5);
        assert_eq!(
            actions[0],
            ChatAction::Prove {
                system: "lean".into(),
                target: "1 + 1 = 2".into()
            }
        );
        // A bare node reference is carried as the target (default system lean).
        assert_eq!(
            actions[1],
            ChatAction::Prove {
                system: "lean".into(),
                target: "3".into()
            }
        );
        assert_eq!(actions[3].tool_name(), "hammer");
        assert_eq!(actions[4], ChatAction::Sweep);
    }

    #[test]
    fn parse_actions_drops_malformed_entries() {
        // Missing required fields: prove without target, falsify without a
        // variables object, hammer without a goal. None should be executed.
        let content = json!({
            "actions":[
                {"tool":"prove","system":"lean"},
                {"tool":"falsify","claim":"x > 0"},
                {"tool":"hammer","system":"lean"}
            ]
        });
        assert!(parse_actions(&content).is_empty());
        // No actions key at all is simply an empty list, not an error.
        assert!(parse_actions(&json!({"reply":"hi"})).is_empty());
    }

    #[test]
    fn tiers_auto_approval_by_risk() {
        assert!(is_low_risk(&json!({"action":"add_node","kind":"lemma"})));
        assert!(is_low_risk(&json!({"action":"add_edge"})));
        assert!(!is_low_risk(
            &json!({"action":"add_node","kind":"formal_statement"})
        ));
        assert!(!is_low_risk(&json!({"action":"set_status"})));
        assert!(!is_low_risk(&json!({"action":"set_formal_statement"})));

        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("t", "x").unwrap();
        let engine = ChatEngine {
            store: &store,
            provider: &ProposingProvider,
        };
        engine.send(&project.id, "decompose it").unwrap();
        assert_eq!(store.proposals(&project.id, true).unwrap().len(), 1);
        let approved = engine.resolve_auto_approvals(&project.id, true).unwrap();
        assert_eq!(approved, 1);
        assert!(store.proposals(&project.id, true).unwrap().is_empty());
        assert_eq!(store.nodes(&project.id).unwrap().len(), 1);
    }
}
