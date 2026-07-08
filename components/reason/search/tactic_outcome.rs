//! Tri-state tactic edges: classify what a tactic application *did* to a goal so
//! an error-free-but-non-closing tactic becomes a DAG edge to new subgoal nodes
//! rather than being silently dropped.
//!
//! The [`crate::search::driver`] expands a goal by asking a [`TacticExpander`] for
//! candidate steps, then adds a DAG edge per candidate. But a real prover backend
//! sees a *third* outcome the raw `(tactic, next_state)` pair hides: a tactic can
//! (a) **close** the goal, (b) run without error yet leave one or more open
//! subgoals — genuine *progress* that must become new nodes, or (c) **fail** (an
//! error / no-op) and be discarded. Collapsing (b) and (c) together loses the
//! progress edges that make the search a graph.
//!
//! [`TacticOutcome`] names the three cases and [`classify`] is the helper an
//! expander calls to decide which one a tactic produced, given whether it ran
//! error-free and the resulting subgoal states. [`into_tactic_steps`] then turns a
//! progress ([`TacticOutcome::Advances`]) outcome into the [`TacticStep`]s the
//! driver already consumes — one child subgoal node per remaining open goal — so
//! wiring it into an expander needs no driver changes.

use super::driver::{GoalState, TacticStep};

/// What applying a tactic did to a goal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TacticOutcome<S> {
    /// The tactic closed the goal outright (no open subgoals remain).
    Closes,
    /// The tactic ran without error but left open subgoals — genuine progress
    /// that must become new DAG nodes rather than being dropped.
    Advances {
        /// The still-open subgoal states produced (each becomes a child node).
        new_subgoals: Vec<S>,
    },
    /// The tactic errored or made no progress — discard it (no edge).
    Discard,
}

/// Classify a tactic application into its tri-state outcome.
///
/// * `error_free = false` ⇒ [`TacticOutcome::Discard`] (the tactic failed).
/// * error-free and every resulting subgoal is closed (or there are none) ⇒
///   [`TacticOutcome::Closes`].
/// * error-free with one or more still-open subgoals ⇒ [`TacticOutcome::Advances`]
///   carrying exactly those open subgoals.
///
/// A tactic that is error-free but makes *no* change — it returns the original
/// open goal unchanged as its sole subgoal — is treated as [`TacticOutcome::Discard`]
/// to avoid a self-loop edge that would waste search budget.
pub fn classify<S: GoalState>(
    error_free: bool,
    parent: &S,
    resulting: Vec<S>,
) -> TacticOutcome<S> {
    if !error_free {
        return TacticOutcome::Discard;
    }
    let parent_key = parent.dedup_key();
    let open: Vec<S> = resulting
        .into_iter()
        .filter(|s| !s.is_closed())
        .collect();
    if open.is_empty() {
        return TacticOutcome::Closes;
    }
    // No-op guard: a single subgoal identical to the parent made no progress.
    if open.len() == 1 && open[0].dedup_key() == parent_key {
        return TacticOutcome::Discard;
    }
    TacticOutcome::Advances { new_subgoals: open }
}

/// Convert a classified outcome into the driver's [`TacticStep`]s.
///
/// * [`TacticOutcome::Advances`] ⇒ one step per open subgoal (each shares the same
///   `tactic` label and `prior`, and becomes a child node in the DAG).
/// * [`TacticOutcome::Closes`] and [`TacticOutcome::Discard`] ⇒ no steps: a close
///   is detected on the resulting node's own [`GoalState::is_closed`], and a
///   discard adds no edge at all.
pub fn into_tactic_steps<S: GoalState>(
    tactic: &str,
    prior: f64,
    outcome: TacticOutcome<S>,
) -> Vec<TacticStep<S>> {
    match outcome {
        TacticOutcome::Advances { new_subgoals } => new_subgoals
            .into_iter()
            .map(|s| TacticStep::new(tactic, prior, s))
            .collect(),
        TacticOutcome::Closes | TacticOutcome::Discard => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal table-free goal state for classification tests.
    #[derive(Clone, Debug, PartialEq)]
    struct G {
        key: String,
        closed: bool,
    }
    impl G {
        fn open(k: &str) -> Self {
            Self {
                key: k.into(),
                closed: false,
            }
        }
        fn closed(k: &str) -> Self {
            Self {
                key: k.into(),
                closed: true,
            }
        }
    }
    impl GoalState for G {
        fn dedup_key(&self) -> String {
            self.key.clone()
        }
        fn is_closed(&self) -> bool {
            self.closed
        }
    }

    #[test]
    fn errored_tactic_is_discarded() {
        let out = classify(false, &G::open("g"), vec![G::open("h")]);
        assert_eq!(out, TacticOutcome::Discard);
    }

    #[test]
    fn no_remaining_subgoals_closes() {
        // Error-free with no open subgoals => Closes.
        let out = classify(true, &G::open("g"), vec![]);
        assert_eq!(out, TacticOutcome::Closes);
        // Error-free where every produced subgoal is already closed => Closes.
        let out2 = classify(true, &G::open("g"), vec![G::closed("done")]);
        assert_eq!(out2, TacticOutcome::Closes);
    }

    #[test]
    fn error_free_non_closing_tactic_advances_to_new_subgoals() {
        let out = classify(
            true,
            &G::open("g"),
            vec![G::open("sub1"), G::open("sub2"), G::closed("also_done")],
        );
        match &out {
            TacticOutcome::Advances { new_subgoals } => {
                // Only the two OPEN subgoals become nodes; the closed one drops out.
                assert_eq!(new_subgoals.len(), 2);
                assert_eq!(new_subgoals[0].dedup_key(), "sub1");
                assert_eq!(new_subgoals[1].dedup_key(), "sub2");
            }
            other => panic!("expected Advances, got {other:?}"),
        }

        // ...and it yields child subgoal steps for the driver.
        let steps = into_tactic_steps("induction n", 0.7, out);
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].tactic, "induction n");
        assert_eq!(steps[0].next.dedup_key(), "sub1");
        assert_eq!(steps[1].next.dedup_key(), "sub2");
    }

    #[test]
    fn error_free_noop_is_discarded() {
        // A tactic that returns the parent unchanged made no progress.
        let out = classify(true, &G::open("g"), vec![G::open("g")]);
        assert_eq!(out, TacticOutcome::Discard);
    }

    #[test]
    fn closes_and_discard_yield_no_steps() {
        assert!(into_tactic_steps::<G>("x", 1.0, TacticOutcome::Closes).is_empty());
        assert!(into_tactic_steps::<G>("x", 1.0, TacticOutcome::Discard).is_empty());
    }
}
