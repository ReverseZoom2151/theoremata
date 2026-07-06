//! Adversarial critic (plan §4).
//!
//! An LLM critic that reviews the *structure* of the proof DAG — not tactic
//! text — for circular dependencies, unjustified logical gaps, over-general
//! statements, and claims marked verified without grounded evidence. Findings
//! are grounded: each one that names a real node is recorded as evidence on
//! that node, so the critique becomes durable, auditable graph state rather
//! than throwaway prose.

use crate::{db::Store, model::ModelRequest, provider::ModelProvider};
use anyhow::Result;
use serde::Serialize;
use serde_json::json;
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize)]
pub struct CritiqueFinding {
    pub node_id: Option<String>,
    pub severity: String,
    pub category: String,
    pub issue: String,
}

#[derive(Debug, Serialize)]
pub struct CritiqueReport {
    pub project_id: String,
    pub findings: Vec<CritiqueFinding>,
    pub summary: String,
}

pub struct Critic<'a> {
    pub store: &'a Store,
    pub provider: &'a dyn ModelProvider,
}

impl Critic<'_> {
    pub fn critique(&self, project_id: &str) -> Result<CritiqueReport> {
        let graph = self.store.export(project_id)?;

        let request = ModelRequest {
            role: "adversarial_verifier".into(),
            task: "You are a meticulous adversarial referee reviewing the STRUCTURE of a \
                   mathematical proof DAG — not the prose of any single step. Inspect the \
                   nodes and dependency edges for: circular dependencies; unjustified logical \
                   gaps (a claim whose stated dependencies do not actually entail it); \
                   over-general statements that claim more than their support; and any node \
                   marked verified without grounded evidence. Report ONLY findings you can tie \
                   to a specific node or edge — no unsupported speculation. Cite the offending \
                   node by its id."
                .into(),
            context: json!({
                "project": graph.project,
                "nodes": graph.nodes,
                "edges": graph.edges,
            }),
            output_schema: json!({
                "type": "object",
                "required": ["findings", "summary"],
                "properties": {
                    "findings": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["severity", "category", "issue"],
                            "properties": {
                                "node_id": {"type": ["string", "null"]},
                                "severity": {"type": "string"},
                                "category": {"type": "string"},
                                "issue": {"type": "string"}
                            }
                        }
                    },
                    "summary": {"type": "string"}
                }
            }),
        };

        let response = self.provider.complete(&request)?;
        let content = &response.content;

        let summary = content["summary"]
            .as_str()
            .unwrap_or("The critic returned no summary.")
            .to_owned();

        let mut findings = Vec::new();
        if let Some(items) = content["findings"].as_array() {
            for item in items {
                let issue = item["issue"].as_str().unwrap_or("").trim().to_owned();
                if issue.is_empty() {
                    continue;
                }
                findings.push(CritiqueFinding {
                    node_id: item["node_id"].as_str().map(str::to_owned),
                    severity: item["severity"].as_str().unwrap_or("info").to_owned(),
                    category: item["category"].as_str().unwrap_or("general").to_owned(),
                    issue,
                });
            }
        }

        // Ground each finding: one that names a real node becomes evidence on
        // that node; anything else is logged as a project-level event so it is
        // never silently dropped but also never attached to a phantom node.
        let node_ids: HashSet<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
        for finding in &findings {
            let payload = json!({
                "severity": finding.severity,
                "category": finding.category,
                "issue": finding.issue,
            });
            match &finding.node_id {
                Some(id) if node_ids.contains(id.as_str()) => {
                    self.store.add_evidence(
                        project_id,
                        id,
                        "critique",
                        "adversarial_verifier",
                        &finding.severity,
                        payload,
                    )?;
                }
                _ => {
                    self.store.event(
                        Some(project_id),
                        None,
                        "critique.finding",
                        "adversarial_verifier",
                        payload,
                    )?;
                }
            }
        }

        Ok(CritiqueReport {
            project_id: project_id.to_owned(),
            findings,
            summary,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ModelResponse, NodeKind};
    use std::path::Path;

    /// A provider that returns a canned critique naming a specific node id.
    struct MockCritic {
        node_id: String,
    }
    impl ModelProvider for MockCritic {
        fn complete(&self, _: &ModelRequest) -> Result<ModelResponse> {
            Ok(ModelResponse {
                provider: "test".into(),
                model: "test".into(),
                content: json!({
                    "findings": [{
                        "node_id": self.node_id,
                        "severity": "major",
                        "category": "gap",
                        "issue": "The dependencies do not entail the conclusion."
                    }],
                    "summary": "One logical gap found."
                }),
            })
        }
        fn name(&self) -> &str {
            "test"
        }
    }

    #[test]
    fn critique_grounds_findings_to_nodes() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "Every even square is even").unwrap();
        let node = store
            .add_node(&project.id, NodeKind::Obligation, "Core step", "S", "test")
            .unwrap();
        let critic = Critic {
            store: &store,
            provider: &MockCritic {
                node_id: node.id.clone(),
            },
        };
        let report = critic.critique(&project.id).unwrap();
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].node_id.as_deref(), Some(node.id.as_str()));
        assert_eq!(report.findings[0].severity, "major");
        assert_eq!(report.summary, "One logical gap found.");
        // The grounded finding was recorded as evidence (which emits an event).
        let events = store.events(&project.id, 50).unwrap();
        assert!(events.iter().any(|e| e.event_type == "evidence.recorded"));
    }

    #[test]
    fn ungrounded_findings_become_events_not_evidence() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "t").unwrap();
        let critic = Critic {
            store: &store,
            provider: &MockCritic {
                node_id: "does-not-exist".into(),
            },
        };
        let report = critic.critique(&project.id).unwrap();
        assert_eq!(report.findings.len(), 1);
        let events = store.events(&project.id, 50).unwrap();
        assert!(events.iter().any(|e| e.event_type == "critique.finding"));
        assert!(!events.iter().any(|e| e.event_type == "evidence.recorded"));
    }
}
