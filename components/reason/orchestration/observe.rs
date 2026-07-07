//! Observability over the event log (plan §1): structured traces, run metrics,
//! and a replay descriptor. Everything is derived from the durable event log and
//! graph snapshot — no new tables. The event log is append-only, so a trace is a
//! faithful record of how the graph's knowledge changed.

use crate::db::Store;
use anyhow::Result;
use serde_json::Value;
use std::collections::BTreeMap;

pub struct Observer<'a> {
    pub store: &'a Store,
}

/// One event rendered as a trace span. `seq` is monotonic (the event id), so
/// spans are totally ordered even across runs.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TraceSpan {
    pub seq: i64,
    pub event_type: String,
    pub actor: String,
    pub run_id: Option<String>,
    pub at: String,
    pub payload: Value,
}

#[derive(Debug, serde::Serialize)]
pub struct Metrics {
    pub project_id: String,
    pub total_nodes: usize,
    pub nodes_by_kind: BTreeMap<String, usize>,
    pub nodes_by_status: BTreeMap<String, usize>,
    /// formally-verified nodes / (obligations + conjectures).
    pub resolve_rate: f64,
    pub attempt_count: usize,
    pub attempt_success_rate: f64,
    /// event counts by actor — a proxy for tool/role usage.
    pub actor_activity: BTreeMap<String, usize>,
    pub runs: usize,
    pub events: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ReplayStep {
    pub seq: i64,
    pub event_type: String,
    pub actor: String,
    pub run_id: Option<String>,
    pub payload: Value,
}

#[derive(Debug, serde::Serialize)]
pub struct ReplayTrace {
    pub project_id: String,
    pub run_id: Option<String>,
    pub steps: Vec<ReplayStep>,
    pub final_nodes: usize,
    pub final_certified: usize,
}

impl Observer<'_> {
    /// The most recent `limit` events as ordered spans (newest first).
    pub fn trace(&self, project_id: &str, limit: usize) -> Result<Vec<TraceSpan>> {
        Ok(self
            .store
            .events(project_id, limit)?
            .into_iter()
            .map(|e| TraceSpan {
                seq: e.id,
                event_type: e.event_type,
                actor: e.actor,
                run_id: e.run_id,
                at: e.created_at.to_rfc3339(),
                payload: e.payload,
            })
            .collect())
    }

    /// Aggregate run metrics for a project.
    pub fn metrics(&self, project_id: &str) -> Result<Metrics> {
        let nodes = self.store.nodes(project_id)?;
        let mut nodes_by_kind: BTreeMap<String, usize> = BTreeMap::new();
        let mut nodes_by_status: BTreeMap<String, usize> = BTreeMap::new();
        let mut verified = 0usize;
        let mut goals = 0usize; // obligations + conjectures = the things we aim to resolve
        for n in &nodes {
            *nodes_by_kind.entry(n.kind.to_string()).or_default() += 1;
            *nodes_by_status.entry(n.status.to_string()).or_default() += 1;
            if matches!(
                n.kind,
                crate::model::NodeKind::Obligation | crate::model::NodeKind::Conjecture
            ) {
                goals += 1;
            }
            if n.status == crate::model::NodeStatus::FormallyVerified {
                verified += 1;
            }
        }
        let resolve_rate = if goals == 0 {
            0.0
        } else {
            verified as f64 / goals as f64
        };

        let attempts = self.store.attempts(project_id, 100_000)?;
        let attempt_count = attempts.len();
        let successes = attempts.iter().filter(|a| a.success).count();
        let attempt_success_rate = if attempt_count == 0 {
            0.0
        } else {
            successes as f64 / attempt_count as f64
        };

        let events = self.store.events(project_id, 1_000_000)?;
        let mut actor_activity: BTreeMap<String, usize> = BTreeMap::new();
        let mut runs = 0usize;
        for e in &events {
            *actor_activity.entry(e.actor.clone()).or_default() += 1;
            if e.event_type == "run.started" {
                runs += 1;
            }
        }

        Ok(Metrics {
            project_id: project_id.to_owned(),
            total_nodes: nodes.len(),
            nodes_by_kind,
            nodes_by_status,
            resolve_rate,
            attempt_count,
            attempt_success_rate,
            actor_activity,
            runs,
            events: events.len(),
        })
    }

    /// A chronological, replayable trace for a run (or the whole project) plus a
    /// snapshot of the final graph, so a past run can be diffed or re-examined.
    pub fn replay(&self, project_id: &str, run_id: Option<&str>) -> Result<ReplayTrace> {
        let mut events = self.store.events(project_id, 1_000_000)?;
        // events() returns newest-first; replay wants chronological order.
        events.reverse();
        let steps: Vec<ReplayStep> = events
            .into_iter()
            .filter(|e| run_id.is_none() || e.run_id.as_deref() == run_id)
            .map(|e| ReplayStep {
                seq: e.id,
                event_type: e.event_type,
                actor: e.actor,
                run_id: e.run_id,
                payload: e.payload,
            })
            .collect();

        let graph = self.store.export(project_id)?;
        let final_certified = graph
            .nodes
            .iter()
            .filter(|n| n.status == crate::model::NodeStatus::FormallyVerified)
            .count();

        Ok(ReplayTrace {
            project_id: project_id.to_owned(),
            run_id: run_id.map(str::to_owned),
            steps,
            final_nodes: graph.nodes.len(),
            final_certified,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NodeKind, NodeStatus};
    use serde_json::json;
    use std::path::Path;

    fn seeded() -> (Store, String) {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "t").unwrap();
        let a = store
            .add_node(&project.id, NodeKind::Obligation, "o1", "s1", "test")
            .unwrap();
        let b = store
            .add_node(&project.id, NodeKind::Obligation, "o2", "s2", "test")
            .unwrap();
        store
            .set_node_status(&project.id, &a.id, NodeStatus::FormallyVerified, "lean")
            .unwrap();
        store
            .add_attempt(
                &project.id,
                Some(&b.id),
                None,
                "formalizer",
                &json!({"x":1}),
                &json!({"ok":false}),
                false,
            )
            .unwrap();
        store
            .add_evidence(
                &project.id,
                &a.id,
                "lean_compile",
                "lean",
                "pass",
                json!({}),
            )
            .unwrap();
        (store, project.id)
    }

    #[test]
    fn trace_returns_ordered_spans() {
        let (store, pid) = seeded();
        let obs = Observer { store: &store };
        let spans = obs.trace(&pid, 100).unwrap();
        assert!(!spans.is_empty());
        // newest-first: seq strictly decreasing
        for w in spans.windows(2) {
            assert!(w[0].seq > w[1].seq);
        }
    }

    #[test]
    fn metrics_counts_and_resolve_rate() {
        let (store, pid) = seeded();
        let obs = Observer { store: &store };
        let m = obs.metrics(&pid).unwrap();
        assert_eq!(m.total_nodes, 2);
        assert_eq!(m.nodes_by_kind.get("obligation"), Some(&2));
        assert_eq!(m.resolve_rate, 0.5); // 1 of 2 obligations verified
        assert_eq!(m.attempt_count, 1);
        assert_eq!(m.attempt_success_rate, 0.0);
        assert!(m.events > 0);
    }

    #[test]
    fn replay_is_chronological_with_snapshot() {
        let (store, pid) = seeded();
        let obs = Observer { store: &store };
        let r = obs.replay(&pid, None).unwrap();
        assert!(!r.steps.is_empty());
        for w in r.steps.windows(2) {
            assert!(w[0].seq < w[1].seq); // chronological
        }
        assert_eq!(r.final_nodes, 2);
        assert_eq!(r.final_certified, 1);
    }
}
