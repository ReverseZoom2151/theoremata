//! Cross-attempt strategy memory (QED's regulator-owned `plan_history.md`).
//!
//! An append-only, per-project log of the strategies already tried and — the
//! load-bearing part — an explicit **"Do NOT try again"** list. Before a
//! REVISE_PLAN or REWRITE, the decomposer/retry reads this so the model is
//! steered away from a strategy that already died, instead of re-deriving the
//! same dead end. QED found this the single most valuable idea for stopping a
//! system from looping on a failed approach.
//!
//! Storage is store-backed and needs no schema migration: each entry is one
//! `plan_history.entry` event carrying the structured record as its payload, so
//! the log is durable, ordered, and replayable alongside every other event.

use crate::db::Store;
use anyhow::Result;
use serde::{Deserialize, Serialize};

/// The event type under which plan-history entries are persisted.
const EVENT_TYPE: &str = "plan_history.entry";

/// One append-only strategy record, written whenever the plan is revised or
/// rewritten. Mirrors QED's regulator entry: a one-sentence strategy, the key
/// step statements tried, a diagnosis of why it failed, and the two advisory
/// lists that steer the next attempt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanHistoryEntry {
    /// Which decomposition attempt this entry summarises (1-based).
    pub attempt: u32,
    /// The strategy in one sentence.
    pub strategy: String,
    /// The key step statements that were tried, verbatim.
    pub key_steps: Vec<String>,
    /// Why the strategy failed (the regulator's diagnosis).
    pub diagnosis: String,
    /// Strategies that must NOT be retried — the anti-loop list.
    pub do_not_retry: Vec<String>,
    /// Fragments that may still be reusable in a different plan.
    pub may_reuse: Vec<String>,
    /// An advisory suggestion for what to try next.
    pub next_suggestion: Option<String>,
}

impl PlanHistoryEntry {
    /// A minimal entry recording only a failed strategy and why to avoid it.
    pub fn failed(attempt: u32, strategy: impl Into<String>, diagnosis: impl Into<String>) -> Self {
        let strategy = strategy.into();
        Self {
            attempt,
            do_not_retry: vec![strategy.clone()],
            strategy,
            key_steps: Vec::new(),
            diagnosis: diagnosis.into(),
            may_reuse: Vec::new(),
            next_suggestion: None,
        }
    }
}

/// Store-backed accessor for a project's plan history.
pub struct PlanHistory<'a> {
    pub store: &'a Store,
}

impl<'a> PlanHistory<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// Append an entry (append-only; entries are never mutated or removed).
    pub fn add(&self, project_id: &str, entry: &PlanHistoryEntry) -> Result<()> {
        self.store.event(
            Some(project_id),
            None,
            EVENT_TYPE,
            "regulator",
            serde_json::to_value(entry)?,
        )
    }

    /// Read all entries in chronological (append) order.
    pub fn read(&self, project_id: &str) -> Result<Vec<PlanHistoryEntry>> {
        // Events come back newest-first; reverse to append order.
        let mut entries: Vec<PlanHistoryEntry> = self
            .store
            .events(project_id, 100_000)?
            .into_iter()
            .filter(|e| e.event_type == EVENT_TYPE)
            .filter_map(|e| serde_json::from_value(e.payload).ok())
            .collect();
        entries.reverse();
        Ok(entries)
    }

    /// Compact rendering for prompt injection: the accumulated strategy memory,
    /// foregrounding the "Do NOT try again" list. Returns `None` when the
    /// history is empty (nothing to inject).
    pub fn render(&self, project_id: &str) -> Result<Option<String>> {
        Ok(render(&self.read(project_id)?))
    }
}

/// Pure compact renderer (separated so it is testable without a store).
pub fn render(entries: &[PlanHistoryEntry]) -> Option<String> {
    if entries.is_empty() {
        return None;
    }
    let mut out = String::from("# Plan history (cross-attempt strategy memory)\n");
    let mut do_not_retry: Vec<&str> = Vec::new();
    for e in entries {
        out.push_str(&format!(
            "\n## Attempt {}: {}\n- diagnosis: {}\n",
            e.attempt, e.strategy, e.diagnosis
        ));
        if !e.key_steps.is_empty() {
            out.push_str(&format!("- key steps: {}\n", e.key_steps.join("; ")));
        }
        if let Some(next) = &e.next_suggestion {
            out.push_str(&format!("- suggested next: {next}\n"));
        }
        for s in &e.do_not_retry {
            do_not_retry.push(s.as_str());
        }
    }
    if !do_not_retry.is_empty() {
        out.push_str("\n## Do NOT try again\n");
        for s in do_not_retry {
            out.push_str(&format!("- {s}\n"));
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn round_trips_entries_in_append_order() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let p = store.create_project("p", "t").unwrap();
        let hist = PlanHistory::new(&store);

        let e1 = PlanHistoryEntry {
            attempt: 1,
            strategy: "induct on n".into(),
            key_steps: vec!["base case n=0".into(), "step n->n+1".into()],
            diagnosis: "step case stuck on parity".into(),
            do_not_retry: vec!["plain induction on n".into()],
            may_reuse: vec!["base case".into()],
            next_suggestion: Some("try strong induction".into()),
        };
        let e2 = PlanHistoryEntry::failed(2, "strong induction", "same parity wall");

        hist.add(&p.id, &e1).unwrap();
        hist.add(&p.id, &e2).unwrap();

        let got = hist.read(&p.id).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], e1);
        assert_eq!(got[1], e2);

        let rendered = hist.render(&p.id).unwrap().unwrap();
        assert!(rendered.contains("Do NOT try again"));
        assert!(rendered.contains("plain induction on n"));
        assert!(rendered.contains("strong induction"));
    }

    #[test]
    fn empty_history_renders_nothing() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let p = store.create_project("p", "t").unwrap();
        assert!(PlanHistory::new(&store).render(&p.id).unwrap().is_none());
        assert!(render(&[]).is_none());
    }
}
