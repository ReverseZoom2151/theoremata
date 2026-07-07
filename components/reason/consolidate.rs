//! Sleep-time consolidation (plan §14).
//!
//! An offline pass over a project's proof graph, run between sessions, that
//! *compresses and strengthens* the graph rather than pushing the frontier: it
//! dedups identical nodes, distils failed dead-ends into reusable "don't try X"
//! lessons, closes obligations that are one-line consequences of already
//! verified results, and reports the remaining frontier. Every step is
//! best-effort and recorded as durable graph state; nothing is deleted, and no
//! step ever grants `formally_verified` (that stays behind the Lean gate).

use crate::{
    db::Store,
    model::{ModelRequest, NodeKind, NodeStatus},
    provider::ModelProvider,
};
use anyhow::Result;
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;

/// Cap on per-obligation model calls in the trivial-closure step so a large
/// graph can't trigger an unbounded number of requests in one pass.
const MAX_SUBLEMMA_CHECKS: usize = 25;

#[derive(Debug, Serialize)]
pub struct ConsolidationReport {
    pub project_id: String,
    pub deduped: usize,
    pub lessons: usize,
    pub trivial_closed: usize,
    pub open_frontier: usize,
    pub notes: Vec<String>,
}

pub struct Consolidator<'a> {
    pub store: &'a Store,
    pub provider: &'a dyn ModelProvider,
}

fn is_open(status: NodeStatus) -> bool {
    matches!(status, NodeStatus::Proposed | NodeStatus::Active)
}

impl Consolidator<'_> {
    pub fn run(&self, project_id: &str) -> Result<ConsolidationReport> {
        // Validate the project up front so a bad id fails cleanly.
        self.store.project(project_id)?;
        let run = self.store.begin_run(project_id, "consolidation")?;
        let has_model = self.provider.name() != "offline";
        let mut notes = Vec::new();

        // 1. Dedup ------------------------------------------------------------
        self.store
            .update_run(project_id, &run, "running", "dedup", 0)?;
        let deduped = self.dedup(project_id).unwrap_or_else(|e| {
            notes.push(format!("dedup step failed: {e}"));
            0
        });

        // 2. Dead-end summarization ------------------------------------------
        self.store
            .update_run(project_id, &run, "running", "lessons", 0)?;
        let lessons = self.summarize_dead_ends(project_id, has_model).unwrap_or_else(|e| {
            notes.push(format!("dead-end summarization failed: {e}"));
            0
        });
        if !has_model {
            notes.push("no model provider: dead-end lessons and sub-lemma closure skipped".into());
        }

        // 3. Obvious sub-lemma closure ---------------------------------------
        self.store
            .update_run(project_id, &run, "running", "sublemmas", 0)?;
        let trivial_closed = self
            .close_trivial(project_id, has_model)
            .unwrap_or_else(|e| {
                notes.push(format!("sub-lemma closure failed: {e}"));
                0
            });

        // 4. Frontier count --------------------------------------------------
        let open_frontier = self
            .store
            .nodes(project_id)?
            .iter()
            .filter(|n| n.kind == NodeKind::Obligation && is_open(n.status))
            .count();

        self.store
            .update_run(project_id, &run, "completed", "complete", 0)?;
        Ok(ConsolidationReport {
            project_id: project_id.to_owned(),
            deduped,
            lessons,
            trivial_closed,
            open_frontier,
            notes,
        })
    }

    /// Supersede later duplicates that share an identical content hash. Nodes
    /// are returned oldest-first, so the earliest occurrence is kept.
    fn dedup(&self, project_id: &str) -> Result<usize> {
        let nodes = self.store.nodes(project_id)?;
        let mut seen: HashMap<String, String> = HashMap::new();
        let mut deduped = 0;
        for node in &nodes {
            // Never touch a node that is already retired or already a duplicate.
            if matches!(
                node.status,
                NodeStatus::Superseded | NodeStatus::Rejected
            ) {
                continue;
            }
            match seen.get(&node.content_hash) {
                Some(kept) => {
                    self.store.set_node_status(
                        project_id,
                        &node.id,
                        NodeStatus::Superseded,
                        "consolidation",
                    )?;
                    self.store.event(
                        Some(project_id),
                        None,
                        "consolidation.dedup",
                        "consolidation",
                        json!({"superseded": node.id, "kept": kept, "hash": node.content_hash}),
                    )?;
                    deduped += 1;
                }
                None => {
                    seen.insert(node.content_hash.clone(), node.id.clone());
                }
            }
        }
        Ok(deduped)
    }

    /// Distil rejected/blocked nodes and their failed attempts into reusable
    /// "don't try X for goals like Y" lessons (persisted as Strategy nodes when
    /// a model is available, else recorded as events).
    fn summarize_dead_ends(&self, project_id: &str, has_model: bool) -> Result<usize> {
        let nodes = self.store.nodes(project_id)?;
        let dead_ends: Vec<_> = nodes
            .iter()
            .filter(|n| matches!(n.status, NodeStatus::Rejected | NodeStatus::Blocked))
            .collect();
        if dead_ends.is_empty() {
            return Ok(0);
        }

        if !has_model {
            for node in &dead_ends {
                self.store.event(
                    Some(project_id),
                    None,
                    "consolidation.dead_end",
                    "consolidation",
                    json!({"node": node.id, "title": node.title, "status": node.status}),
                )?;
            }
            return Ok(0);
        }

        // Gather each dead-end plus a few of its most recent failed attempts.
        let attempts = self.store.attempts(project_id, 500)?;
        let clusters: Vec<_> = dead_ends
            .iter()
            .map(|n| {
                let fails: Vec<_> = attempts
                    .iter()
                    .filter(|a| a.node_id.as_deref() == Some(n.id.as_str()) && !a.success)
                    .take(3)
                    .map(|a| json!({"actor": a.actor, "output": a.output}))
                    .collect();
                json!({
                    "node_id": n.id,
                    "title": n.title,
                    "statement": n.statement,
                    "status": n.status,
                    "failed_attempts": fails,
                })
            })
            .collect();

        let request = ModelRequest {
            role: "consolidation_summarizer".into(),
            task: "You are consolidating a proof project offline. From these dead-ends \
                   (rejected/blocked claims and their failed attempts), write a few short, \
                   reusable lessons of the form 'don't try X for goals like Y', so the agent \
                   avoids repeating them. Be specific and terse."
                .into(),
            context: json!({ "dead_ends": clusters }),
            output_schema: json!({
                "type": "object",
                "required": ["lessons"],
                "properties": {
                    "lessons": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["pattern", "advice"],
                            "properties": {
                                "pattern": {"type": "string"},
                                "advice": {"type": "string"}
                            }
                        }
                    }
                }
            }),
        };

        let response = self.provider.complete(&request)?;
        let mut count = 0;
        if let Some(items) = response.content["lessons"].as_array() {
            for item in items {
                let pattern = item["pattern"].as_str().unwrap_or("").trim();
                let advice = item["advice"].as_str().unwrap_or("").trim();
                if pattern.is_empty() && advice.is_empty() {
                    continue;
                }
                self.store.add_node(
                    project_id,
                    NodeKind::Strategy,
                    &format!("Lesson: {pattern}"),
                    advice,
                    "consolidation:lesson",
                )?;
                count += 1;
            }
        }
        Ok(count)
    }

    /// Close open obligations that the model judges to be one-line consequences
    /// of an already-verified node — set `informally_verified` only (never
    /// certified: that requires the Lean gate).
    fn close_trivial(&self, project_id: &str, has_model: bool) -> Result<usize> {
        if !has_model {
            return Ok(0);
        }
        let nodes = self.store.nodes(project_id)?;
        let verified: Vec<_> = nodes
            .iter()
            .filter(|n| {
                matches!(
                    n.status,
                    NodeStatus::InformallyVerified | NodeStatus::FormallyVerified
                )
            })
            .map(|n| json!({"id": n.id, "statement": n.statement}))
            .collect();
        if verified.is_empty() {
            return Ok(0);
        }

        let open: Vec<_> = nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Obligation && is_open(n.status))
            .take(MAX_SUBLEMMA_CHECKS)
            .collect();

        let mut closed = 0;
        for node in open {
            let request = ModelRequest {
                role: "sublemma_prover".into(),
                task: "Decide whether the obligation is a one-line, immediate consequence of \
                       one of the already-verified results (e.g. a direct instance, rewrite, or \
                       specialization). Only answer trivial:true when it is genuinely immediate."
                    .into(),
                context: json!({
                    "obligation": {"id": node.id, "statement": node.statement},
                    "verified": verified,
                }),
                output_schema: json!({
                    "type": "object",
                    "required": ["trivial", "justification"],
                    "properties": {
                        "trivial": {"type": "boolean"},
                        "justification": {"type": "string"}
                    }
                }),
            };

            let response = match self.provider.complete(&request) {
                Ok(r) => r,
                Err(_) => continue, // best-effort: skip on model error
            };
            if response.content["trivial"].as_bool() == Some(true) {
                self.store.set_node_status(
                    project_id,
                    &node.id,
                    NodeStatus::InformallyVerified,
                    "consolidation:trivial",
                )?;
                self.store.add_evidence(
                    project_id,
                    &node.id,
                    "consolidation_trivial",
                    "sublemma_prover",
                    "trivial",
                    json!({"justification": response.content["justification"]}),
                )?;
                closed += 1;
            }
        }
        Ok(closed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelResponse;
    use std::path::Path;

    /// A provider that answers each consolidation role with canned content.
    struct MockConsolidator;
    impl ModelProvider for MockConsolidator {
        fn complete(&self, request: &ModelRequest) -> Result<ModelResponse> {
            let content = match request.role.as_str() {
                "consolidation_summarizer" => json!({
                    "lessons": [{
                        "pattern": "induction on the raw sum",
                        "advice": "reduce to the closed form first"
                    }]
                }),
                "sublemma_prover" => json!({
                    "trivial": false,
                    "justification": "not an immediate consequence"
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
    fn dedups_and_summarizes() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "every even square is even").unwrap();
        // Two identical nodes (same kind/title/statement -> same content_hash).
        let a = store
            .add_node(&project.id, NodeKind::Lemma, "dup", "S", "test")
            .unwrap();
        let b = store
            .add_node(&project.id, NodeKind::Lemma, "dup", "S", "test")
            .unwrap();
        assert_eq!(a.content_hash, b.content_hash);
        // A dead-end to summarize.
        let dead = store
            .add_node(&project.id, NodeKind::Obligation, "bad idea", "T", "test")
            .unwrap();
        store
            .set_node_status(&project.id, &dead.id, NodeStatus::Rejected, "test")
            .unwrap();

        let consolidator = Consolidator {
            store: &store,
            provider: &MockConsolidator,
        };
        let report = consolidator.run(&project.id).unwrap();

        assert_eq!(report.deduped, 1, "exactly one duplicate superseded");
        assert_eq!(report.lessons, 1, "one lesson persisted from the dead-end");
        assert_eq!(report.trivial_closed, 0);

        let nodes = store.nodes(&project.id).unwrap();
        // The later duplicate (b) is superseded; the first (a) is untouched.
        let got_b = nodes.iter().find(|n| n.id == b.id).unwrap();
        assert_eq!(got_b.status, NodeStatus::Superseded);
        let got_a = nodes.iter().find(|n| n.id == a.id).unwrap();
        assert_ne!(got_a.status, NodeStatus::Superseded);
        // The lesson landed as a Strategy node.
        assert!(nodes
            .iter()
            .any(|n| n.kind == NodeKind::Strategy && n.provenance == "consolidation:lesson"));
    }

    #[test]
    fn offline_skips_model_steps() {
        use crate::provider::OfflineProvider;
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "t").unwrap();
        let x = store
            .add_node(&project.id, NodeKind::Lemma, "dup", "S", "test")
            .unwrap();
        store
            .add_node(&project.id, NodeKind::Lemma, "dup", "S", "test")
            .unwrap();
        let _ = x;
        let consolidator = Consolidator {
            store: &store,
            provider: &OfflineProvider,
        };
        let report = consolidator.run(&project.id).unwrap();
        // Dedup still runs offline; model-dependent steps are no-ops.
        assert_eq!(report.deduped, 1);
        assert_eq!(report.lessons, 0);
        assert_eq!(report.trivial_closed, 0);
    }
}
