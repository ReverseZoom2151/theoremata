//! A LIVE, model-editable plan / todo (the promoted form of `plan_history.rs`).
//!
//! `plan_history.rs` is an append-only LOG of strategies already tried. This
//! module is its live counterpart: a mutable, ordered todo list that the agent
//! MAINTAINS and REVISES as it works. The model reads it, and a meta-tool
//! (`update_plan`) mutates it: add a step, mark one in progress, record a
//! failure, or wholesale revise the remaining plan while preserving the work
//! already finished.
//!
//! Design constraints (all enforced here, not by convention):
//!
//! * **Deterministic** — step ids are drawn from a monotonic counter, never a
//!   clock or RNG; ordering is explicit `Vec` order; serialization is stable.
//! * **Bounded** — the number of steps is capped ([`MAX_STEPS`]); every growth
//!   operation is fallible and refuses to exceed the cap.
//! * **Exactly one `InProgress`** — the "what am I doing right now" invariant is
//!   checked after every mutation; an operation that would create a second
//!   in-progress step is rejected and leaves the plan unchanged.
//! * **Revision preserves finished work** — [`LivePlan::revise`] may reorder,
//!   drop, or insert *pending* steps freely, but every `Done` step must survive
//!   the revision (the model cannot silently discard proven work).
//!
//! Observability: [`LivePlan::snapshot`] projects the current plan into the
//! existing [`PlanHistoryEntry`] type so a snapshot can be appended to
//! `plan_history` through the same store-backed log as every other attempt —
//! the live plan and the strategy log share one durable record type.

use crate::plan_history::PlanHistoryEntry;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Hard cap on the number of steps a single plan may hold. Keeps the plan
/// bounded regardless of how the model drives `update_plan`.
pub const MAX_STEPS: usize = 256;

/// Lifecycle state of a single plan step.
///
/// `Pending` and `InProgress` are the two *open* states; `Done`, `Failed`, and
/// `Skipped` are *terminal* — a plan is complete once no step is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    /// Not started yet.
    Pending,
    /// The single step currently being worked on (at most one per plan).
    InProgress,
    /// Finished successfully.
    Done,
    /// Attempted and failed (kept in the plan as a record, like a `do_not_retry`).
    Failed,
    /// Deliberately not attempted (e.g. made unnecessary by another step).
    Skipped,
}

impl StepStatus {
    /// Whether this is a terminal state (nothing more to do on the step).
    pub fn is_terminal(self) -> bool {
        matches!(self, StepStatus::Done | StepStatus::Failed | StepStatus::Skipped)
    }
}

/// One item in the live plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    /// Stable identifier, unique within the plan and never reused.
    pub id: u64,
    /// One-line description of what the step does.
    pub description: String,
    /// Current lifecycle state.
    pub status: StepStatus,
    /// Free-form notes the model may attach (diagnosis, hints, links).
    pub notes: Option<String>,
}

/// A step as proposed by [`LivePlan::revise`]. An entry with `id = Some(existing)`
/// keeps that step (carrying its status and notes forward); an entry with
/// `id = None` inserts a fresh `Pending` step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisedStep {
    /// Existing step to keep, or `None` to insert a new step.
    pub id: Option<u64>,
    /// Description for a new step (ignored for a kept step, whose description
    /// is preserved).
    pub description: String,
}

impl RevisedStep {
    /// Keep an existing step by id.
    pub fn keep(id: u64) -> Self {
        Self {
            id: Some(id),
            description: String::new(),
        }
    }

    /// Insert a new pending step with the given description.
    pub fn new_step(description: impl Into<String>) -> Self {
        Self {
            id: None,
            description: description.into(),
        }
    }
}

/// Errors from a plan mutation. Every variant leaves the plan unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// The referenced step id does not exist in the plan.
    NoSuchStep(u64),
    /// The operation would exceed [`MAX_STEPS`].
    TooManySteps,
    /// The operation would leave two or more steps `InProgress`.
    MultipleInProgress,
    /// A revision omitted a `Done` step (finished work may not be discarded).
    DroppedDoneStep(u64),
    /// A revision referenced the same existing step id twice.
    DuplicateStep(u64),
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanError::NoSuchStep(id) => write!(f, "no step with id {id}"),
            PlanError::TooManySteps => write!(f, "plan exceeds the maximum of {MAX_STEPS} steps"),
            PlanError::MultipleInProgress => {
                write!(f, "at most one step may be InProgress at a time")
            }
            PlanError::DroppedDoneStep(id) => {
                write!(f, "revision dropped Done step {id}; finished work is preserved")
            }
            PlanError::DuplicateStep(id) => write!(f, "revision referenced step {id} twice"),
        }
    }
}

impl std::error::Error for PlanError {}

/// A live, ordered, model-editable todo list.
///
/// Construct with [`LivePlan::new`], grow it with [`add_step`](LivePlan::add_step),
/// drive it with [`mark_in_progress`](LivePlan::mark_in_progress) /
/// [`update_status`](LivePlan::update_status), and reshape it with
/// [`revise`](LivePlan::revise). All mutations keep the exactly-one-`InProgress`
/// and bounded invariants; a rejected mutation leaves the plan untouched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LivePlan {
    /// Steps in execution order.
    steps: Vec<PlanStep>,
    /// Monotonic id source — only ever increases, so ids are never reused even
    /// across removals (keeping references in the log stable).
    next_id: u64,
    /// How many times the plan has been revised (also used as the snapshot's
    /// `attempt` number).
    revision: u32,
}

impl Default for LivePlan {
    fn default() -> Self {
        Self::new()
    }
}

impl LivePlan {
    /// An empty plan.
    pub fn new() -> Self {
        Self {
            steps: Vec::new(),
            next_id: 1,
            revision: 0,
        }
    }

    // -- Observers ----------------------------------------------------------

    /// The steps in execution order.
    pub fn steps(&self) -> &[PlanStep] {
        &self.steps
    }

    /// Number of steps.
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// Whether the plan has no steps.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// How many revisions have been applied.
    pub fn revision(&self) -> u32 {
        self.revision
    }

    /// The step with the given id, if any.
    pub fn get(&self, id: u64) -> Option<&PlanStep> {
        self.steps.iter().find(|s| s.id == id)
    }

    /// The single `InProgress` step, if one exists.
    pub fn in_progress(&self) -> Option<&PlanStep> {
        self.steps.iter().find(|s| s.status == StepStatus::InProgress)
    }

    /// The first `Pending` step in execution order — what to pick up next.
    pub fn next_pending(&self) -> Option<&PlanStep> {
        self.steps.iter().find(|s| s.status == StepStatus::Pending)
    }

    /// Whether the plan is complete: it has at least one step and none remain
    /// open (`Pending`/`InProgress`), i.e. every step is `Done`/`Failed`/`Skipped`.
    pub fn is_complete(&self) -> bool {
        !self.steps.is_empty() && self.steps.iter().all(|s| s.status.is_terminal())
    }

    // -- Mutators -----------------------------------------------------------

    /// Append a new `Pending` step and return its id. Fails if the plan is full.
    pub fn add_step(&mut self, description: impl Into<String>) -> Result<u64, PlanError> {
        if self.steps.len() >= MAX_STEPS {
            return Err(PlanError::TooManySteps);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.steps.push(PlanStep {
            id,
            description: description.into(),
            status: StepStatus::Pending,
            notes: None,
        });
        Ok(id)
    }

    /// Attach or replace the notes on a step.
    pub fn set_notes(&mut self, id: u64, notes: impl Into<String>) -> Result<(), PlanError> {
        let step = self
            .steps
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or(PlanError::NoSuchStep(id))?;
        step.notes = Some(notes.into());
        Ok(())
    }

    /// Set a step's status. Rejected (leaving the plan unchanged) if it would
    /// create a second `InProgress` step.
    pub fn update_status(&mut self, id: u64, status: StepStatus) -> Result<(), PlanError> {
        let idx = self
            .steps
            .iter()
            .position(|s| s.id == id)
            .ok_or(PlanError::NoSuchStep(id))?;

        if status == StepStatus::InProgress
            && self
                .steps
                .iter()
                .enumerate()
                .any(|(i, s)| i != idx && s.status == StepStatus::InProgress)
        {
            return Err(PlanError::MultipleInProgress);
        }
        self.steps[idx].status = status;
        Ok(())
    }

    /// Mark a step `InProgress`. Any *other* step currently `InProgress` is first
    /// demoted back to `Pending`, so this always succeeds in establishing a
    /// single active step (the invariant is maintained by construction).
    pub fn mark_in_progress(&mut self, id: u64) -> Result<(), PlanError> {
        let idx = self
            .steps
            .iter()
            .position(|s| s.id == id)
            .ok_or(PlanError::NoSuchStep(id))?;
        for (i, s) in self.steps.iter_mut().enumerate() {
            if i != idx && s.status == StepStatus::InProgress {
                s.status = StepStatus::Pending;
            }
        }
        self.steps[idx].status = StepStatus::InProgress;
        Ok(())
    }

    /// Replace the plan with a revised ordering. Semantics:
    ///
    /// * Each [`RevisedStep`] with an id keeps that existing step verbatim
    ///   (status and notes carried forward); each without an id inserts a fresh
    ///   `Pending` step. The order of `desired` becomes the new execution order.
    /// * **Every `Done` step must appear** in `desired`, or the revision is
    ///   rejected ([`PlanError::DroppedDoneStep`]) — finished work is preserved.
    /// * An existing id may appear at most once ([`PlanError::DuplicateStep`]),
    ///   an unknown id is [`PlanError::NoSuchStep`], and the result must respect
    ///   [`MAX_STEPS`] and the single-`InProgress` invariant.
    ///
    /// On any error the plan is left unchanged. On success `revision` increments.
    pub fn revise(&mut self, desired: Vec<RevisedStep>) -> Result<(), PlanError> {
        if desired.len() > MAX_STEPS {
            return Err(PlanError::TooManySteps);
        }

        // Validate kept ids: each must exist and be referenced at most once.
        let mut seen: Vec<u64> = Vec::new();
        for item in &desired {
            if let Some(id) = item.id {
                if self.get(id).is_none() {
                    return Err(PlanError::NoSuchStep(id));
                }
                if seen.contains(&id) {
                    return Err(PlanError::DuplicateStep(id));
                }
                seen.push(id);
            }
        }

        // Every currently-Done step must be kept.
        for step in &self.steps {
            if step.status == StepStatus::Done && !seen.contains(&step.id) {
                return Err(PlanError::DroppedDoneStep(step.id));
            }
        }

        // Build the new ordering, allocating ids for inserted steps.
        let mut next_id = self.next_id;
        let mut new_steps: Vec<PlanStep> = Vec::with_capacity(desired.len());
        for item in desired {
            match item.id {
                Some(id) => {
                    // Safe: existence checked above.
                    if let Some(existing) = self.steps.iter().find(|s| s.id == id) {
                        new_steps.push(existing.clone());
                    }
                }
                None => {
                    let id = next_id;
                    next_id += 1;
                    new_steps.push(PlanStep {
                        id,
                        description: item.description,
                        status: StepStatus::Pending,
                        notes: None,
                    });
                }
            }
        }

        // Enforce the single-InProgress invariant on the result.
        if new_steps
            .iter()
            .filter(|s| s.status == StepStatus::InProgress)
            .count()
            > 1
        {
            return Err(PlanError::MultipleInProgress);
        }

        self.steps = new_steps;
        self.next_id = next_id;
        self.revision += 1;
        Ok(())
    }

    // -- Observability ------------------------------------------------------

    /// Project the current plan into a [`PlanHistoryEntry`] for logging.
    ///
    /// This reuses the existing store-backed strategy-log record so a snapshot
    /// can be appended to `plan_history` alongside every other attempt. The
    /// mapping: `attempt` = revision number; `key_steps` = every step's
    /// description; `may_reuse` = `Done` step descriptions (reusable work);
    /// `do_not_retry` = `Failed` step descriptions (the anti-loop list);
    /// `next_suggestion` = the next pending step's description.
    pub fn snapshot(&self) -> PlanHistoryEntry {
        let done = self
            .steps
            .iter()
            .filter(|s| s.status == StepStatus::Done)
            .count();
        PlanHistoryEntry {
            attempt: self.revision,
            strategy: format!("live plan: {done}/{} step(s) done", self.steps.len()),
            key_steps: self.steps.iter().map(|s| s.description.clone()).collect(),
            diagnosis: self.status_line(),
            do_not_retry: self
                .steps
                .iter()
                .filter(|s| s.status == StepStatus::Failed)
                .map(|s| s.description.clone())
                .collect(),
            may_reuse: self
                .steps
                .iter()
                .filter(|s| s.status == StepStatus::Done)
                .map(|s| s.description.clone())
                .collect(),
            next_suggestion: self.next_pending().map(|s| s.description.clone()),
        }
    }

    /// A compact one-line status tally, e.g. `2 done, 1 in progress, 3 pending`.
    fn status_line(&self) -> String {
        let mut pending = 0usize;
        let mut in_progress = 0usize;
        let mut done = 0usize;
        let mut failed = 0usize;
        let mut skipped = 0usize;
        for s in &self.steps {
            match s.status {
                StepStatus::Pending => pending += 1,
                StepStatus::InProgress => in_progress += 1,
                StepStatus::Done => done += 1,
                StepStatus::Failed => failed += 1,
                StepStatus::Skipped => skipped += 1,
            }
        }
        format!(
            "{done} done, {in_progress} in progress, {pending} pending, {failed} failed, {skipped} skipped"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_with(descs: &[&str]) -> (LivePlan, Vec<u64>) {
        let mut p = LivePlan::new();
        let ids = descs.iter().map(|d| p.add_step(*d).unwrap()).collect();
        (p, ids)
    }

    #[test]
    fn add_step_assigns_monotonic_ids_and_pending_status() {
        let (p, ids) = plan_with(&["a", "b", "c"]);
        assert_eq!(ids, vec![1, 2, 3]);
        assert_eq!(p.len(), 3);
        assert!(p.steps().iter().all(|s| s.status == StepStatus::Pending));
        assert_eq!(p.get(2).unwrap().description, "b");
    }

    #[test]
    fn update_status_and_notes() {
        let (mut p, ids) = plan_with(&["a", "b"]);
        p.update_status(ids[0], StepStatus::Done).unwrap();
        p.set_notes(ids[0], "worked").unwrap();
        assert_eq!(p.get(ids[0]).unwrap().status, StepStatus::Done);
        assert_eq!(p.get(ids[0]).unwrap().notes.as_deref(), Some("worked"));
        assert_eq!(p.update_status(999, StepStatus::Done), Err(PlanError::NoSuchStep(999)));
        assert_eq!(p.set_notes(999, "x"), Err(PlanError::NoSuchStep(999)));
    }

    #[test]
    fn exactly_one_in_progress_is_enforced() {
        let (mut p, ids) = plan_with(&["a", "b", "c"]);
        // update_status refuses a second InProgress.
        p.update_status(ids[0], StepStatus::InProgress).unwrap();
        assert_eq!(
            p.update_status(ids[1], StepStatus::InProgress),
            Err(PlanError::MultipleInProgress)
        );
        // The rejected op left the plan unchanged.
        assert_eq!(p.get(ids[1]).unwrap().status, StepStatus::Pending);
        assert_eq!(p.in_progress().unwrap().id, ids[0]);

        // mark_in_progress moves the active step by demoting the previous one.
        p.mark_in_progress(ids[2]).unwrap();
        assert_eq!(p.get(ids[0]).unwrap().status, StepStatus::Pending);
        assert_eq!(p.in_progress().unwrap().id, ids[2]);
        // Never more than one InProgress at any time.
        assert_eq!(
            p.steps().iter().filter(|s| s.status == StepStatus::InProgress).count(),
            1
        );
    }

    #[test]
    fn next_pending_follows_execution_order() {
        let (mut p, ids) = plan_with(&["a", "b", "c"]);
        assert_eq!(p.next_pending().unwrap().id, ids[0]);
        p.update_status(ids[0], StepStatus::Done).unwrap();
        p.mark_in_progress(ids[1]).unwrap();
        // b is InProgress (not pending), so the next pending is c.
        assert_eq!(p.next_pending().unwrap().id, ids[2]);
    }

    #[test]
    fn is_complete_only_when_no_open_steps() {
        let mut p = LivePlan::new();
        assert!(!p.is_complete(), "empty plan is not complete");
        let a = p.add_step("a").unwrap();
        let b = p.add_step("b").unwrap();
        assert!(!p.is_complete());
        p.update_status(a, StepStatus::Done).unwrap();
        p.update_status(b, StepStatus::Skipped).unwrap();
        assert!(p.is_complete(), "all terminal -> complete");
        // A failed step still counts as terminal/complete.
        let (mut q, ids) = plan_with(&["x"]);
        q.update_status(ids[0], StepStatus::Failed).unwrap();
        assert!(q.is_complete());
    }

    #[test]
    fn revise_preserves_done_and_reorders_pending() {
        let (mut p, ids) = plan_with(&["a", "b", "c"]);
        p.update_status(ids[0], StepStatus::Done).unwrap(); // a is Done
        // Reorder: keep a (Done), drop c, keep b after a, insert a new step.
        p.revise(vec![
            RevisedStep::keep(ids[0]),
            RevisedStep::new_step("d"),
            RevisedStep::keep(ids[1]),
        ])
        .unwrap();
        let order: Vec<u64> = p.steps().iter().map(|s| s.id).collect();
        assert_eq!(order.len(), 3);
        // a preserved (still Done, same id/description).
        assert_eq!(p.get(ids[0]).unwrap().status, StepStatus::Done);
        assert_eq!(p.get(ids[0]).unwrap().description, "a");
        // c was dropped (it was only Pending, so that is allowed).
        assert!(p.get(ids[2]).is_none());
        // The new step got a fresh id and is Pending.
        let new = p.steps().iter().find(|s| s.description == "d").unwrap();
        assert_eq!(new.status, StepStatus::Pending);
        assert!(new.id > ids[2]);
        assert_eq!(p.revision(), 1);
    }

    #[test]
    fn revise_rejects_dropping_a_done_step() {
        let (mut p, ids) = plan_with(&["a", "b"]);
        p.update_status(ids[0], StepStatus::Done).unwrap();
        // Omitting the Done step a is rejected; plan unchanged.
        let err = p.revise(vec![RevisedStep::keep(ids[1])]).unwrap_err();
        assert_eq!(err, PlanError::DroppedDoneStep(ids[0]));
        assert_eq!(p.len(), 2);
        assert_eq!(p.revision(), 0);
    }

    #[test]
    fn revise_rejects_duplicate_and_unknown_ids() {
        let (mut p, ids) = plan_with(&["a", "b"]);
        assert_eq!(
            p.revise(vec![RevisedStep::keep(ids[0]), RevisedStep::keep(ids[0])]),
            Err(PlanError::DuplicateStep(ids[0]))
        );
        assert_eq!(
            p.revise(vec![RevisedStep::keep(4242)]),
            Err(PlanError::NoSuchStep(4242))
        );
        // Plan untouched by both rejected revisions.
        assert_eq!(p.len(), 2);
        assert_eq!(p.revision(), 0);
    }

    #[test]
    fn bounded_by_max_steps() {
        let mut p = LivePlan::new();
        for _ in 0..MAX_STEPS {
            p.add_step("x").unwrap();
        }
        assert_eq!(p.add_step("overflow"), Err(PlanError::TooManySteps));
        assert_eq!(p.len(), MAX_STEPS);
    }

    #[test]
    fn snapshot_maps_into_plan_history_entry() {
        let (mut p, ids) = plan_with(&["a", "b", "c"]);
        p.update_status(ids[0], StepStatus::Done).unwrap();
        p.update_status(ids[1], StepStatus::Failed).unwrap();
        // c stays pending -> it is the next suggestion.
        let snap = p.snapshot();
        assert_eq!(snap.attempt, p.revision());
        assert_eq!(snap.key_steps, vec!["a", "b", "c"]);
        assert_eq!(snap.may_reuse, vec!["a"]); // Done
        assert_eq!(snap.do_not_retry, vec!["b"]); // Failed
        assert_eq!(snap.next_suggestion.as_deref(), Some("c"));
        assert!(snap.strategy.contains("1/3"));
    }

    #[test]
    fn json_round_trip() {
        let (mut p, ids) = plan_with(&["a", "b"]);
        p.mark_in_progress(ids[1]).unwrap();
        p.set_notes(ids[0], "n").unwrap();
        let json = serde_json::to_string(&p).unwrap();
        let back: LivePlan = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
        // The recovered plan still honours its invariants (one InProgress).
        assert_eq!(back.in_progress().unwrap().id, ids[1]);
    }

    #[test]
    fn deterministic_construction_is_reproducible() {
        let build = || {
            let mut p = LivePlan::new();
            let a = p.add_step("a").unwrap();
            let b = p.add_step("b").unwrap();
            p.update_status(a, StepStatus::Done).unwrap();
            p.mark_in_progress(b).unwrap();
            p.revise(vec![
                RevisedStep::keep(a),
                RevisedStep::new_step("c"),
                RevisedStep::keep(b),
            ])
            .unwrap();
            p
        };
        let x = build();
        let y = build();
        assert_eq!(x, y);
        // Same serialized bytes too (stable field order, no clock/RNG).
        assert_eq!(
            serde_json::to_string(&x).unwrap(),
            serde_json::to_string(&y).unwrap()
        );
    }
}
