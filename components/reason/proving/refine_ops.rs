//! Proof-refinement operations (Seed-Prover / Delta-Prover architecture).
//!
//! Seed-Prover's key loop is not "generate once and verify" but *refine*: a
//! failed or partial attempt is not thrown away — its useful residue is distilled
//! and fed back, and the problem is re-decomposed when in-place fixes stall. This
//! module provides three deterministic, model-free refinement primitives that the
//! existing [`crate::repair`] (in-place error fixing) and [`crate::sketch`]
//! (decompose → prove holes → splice) loops can drive:
//!
//! 1. [`summarize_progress`] — the *self-summarization / restart* helper. When an
//!    attempt fails but got part-way, we distill what it *did* establish (the
//!    closed subgoals and the lemmas their proofs leaned on) into a compact
//!    [`Summary`] seed, so the next attempt starts from the progress rather than
//!    from scratch. Pure extraction — no model call.
//!
//! 2. [`reflective_redecompose`] — the *outer-loop* move. Given a sketch whose
//!    subgoal failed and structured [`RedecomposeFeedback`], it produces a new
//!    sketch that splits the failing subgoal into parts, adds a bridging lemma
//!    that recombines them, reorders so the bridge depends on its parts, and
//!    **preserves every other (already-proved) step** — rewiring any downstream
//!    `\uses` of the split step onto the bridge. Deterministic transformation.
//!
//! 3. [`RefinementScheduler`] — alternates an *inner* loop (repair errors in
//!    place, [`crate::repair::repair_proof`]) and an *outer* loop (re-decompose,
//!    [`reflective_redecompose`] + a fresh sketch run). It stays in the inner loop
//!    while repair makes progress and escalates to an outer move only when the
//!    inner loop stalls, all under a hard move budget, returning the exact
//!    schedule of moves it took for audit.
//!
//! Everything here is a pure function of its inputs — no wall-clock, no unseeded
//! randomness — so the whole refinement loop is exercised deterministically. All
//! proof / sketch / feedback text is treated as UNTRUSTED DATA: it is only ever
//! parsed for structure and re-emitted as prose / seed text, never executed.

use super::repair::VerifyError;
use super::sketch::{InformalSketch, SketchStep};

// ===========================================================================
// 1. summarize_progress — self-summarization / restart seed
// ===========================================================================

/// One subgoal of a partial attempt and whether that attempt discharged it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubgoalStatus {
    /// Stable name/id of the subgoal (mirrors a sketch step id).
    pub name: String,
    /// The subgoal statement.
    pub statement: String,
    /// Whether the failed/partial attempt actually closed this subgoal.
    pub closed: bool,
    /// The proof fragment that discharged it, if `closed`.
    pub proof: Option<String>,
}

impl SubgoalStatus {
    /// A subgoal the attempt closed, with the discharging proof fragment.
    pub fn closed(
        name: impl Into<String>,
        statement: impl Into<String>,
        proof: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            statement: statement.into(),
            closed: true,
            proof: Some(proof.into()),
        }
    }

    /// A subgoal the attempt left open.
    pub fn open(name: impl Into<String>, statement: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            statement: statement.into(),
            closed: false,
            proof: None,
        }
    }
}

/// A partial/failed proof attempt: its raw text plus the per-subgoal status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartialProof {
    /// The raw (failing) proof text of the whole attempt.
    pub text: String,
    /// The subgoals this attempt targeted, with closed/open status.
    pub subgoals: Vec<SubgoalStatus>,
}

impl PartialProof {
    pub fn new(text: impl Into<String>, subgoals: Vec<SubgoalStatus>) -> Self {
        Self {
            text: text.into(),
            subgoals,
        }
    }
}

/// The goal state the *next* attempt must reach: the top-level goal plus any
/// subgoals the prover reports still open (merged with the partial's open ones).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalState {
    pub goal: String,
    pub open_subgoals: Vec<String>,
}

impl GoalState {
    pub fn new(goal: impl Into<String>, open_subgoals: Vec<String>) -> Self {
        Self {
            goal: goal.into(),
            open_subgoals,
        }
    }
}

/// A subgoal the failed attempt already closed — carried into the next attempt so
/// it is not re-proven.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosedSubgoal {
    pub name: String,
    pub statement: String,
    pub proof: String,
}

/// The distilled residue of a failed/partial attempt: what it established, the
/// lemmas it leaned on, what remains, and a compact textual `seed` rendering the
/// three for the next attempt's context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    /// The goal the next attempt targets (carried through verbatim).
    pub goal: String,
    /// Subgoals already discharged — reusable, do not re-prove.
    pub closed_subgoals: Vec<ClosedSubgoal>,
    /// Lemma / identifier names the closed fragments used — good premises to seed
    /// or retrieve next time. First-seen order, de-duplicated.
    pub useful_lemmas: Vec<String>,
    /// Subgoals still open — what the next attempt must still establish.
    pub open_subgoals: Vec<String>,
    /// A compact text seed rendering the above for the next attempt.
    pub seed: String,
}

/// Distill a failed/partial attempt into a compact restart [`Summary`]:
/// partition its subgoals into closed vs. open, harvest the lemma names the
/// closed proofs used, merge in any still-open subgoals the prover reports, and
/// render a compact seed. Pure, deterministic extraction — never a model call.
pub fn summarize_progress(partial: &PartialProof, goal_state: &GoalState) -> Summary {
    let mut closed_subgoals: Vec<ClosedSubgoal> = Vec::new();
    let mut open_subgoals: Vec<String> = Vec::new();
    let mut useful_lemmas: Vec<String> = Vec::new();

    for sg in &partial.subgoals {
        if sg.closed {
            let proof = sg.proof.clone().unwrap_or_default();
            for lem in extract_lemmas(&proof) {
                if !useful_lemmas.iter().any(|e| e == &lem) {
                    useful_lemmas.push(lem);
                }
            }
            closed_subgoals.push(ClosedSubgoal {
                name: sg.name.clone(),
                statement: sg.statement.clone(),
                proof,
            });
        } else if !open_subgoals.iter().any(|e| e == &sg.statement) {
            open_subgoals.push(sg.statement.clone());
        }
    }

    // Merge in any open subgoals the prover reports that we did not already list.
    for og in &goal_state.open_subgoals {
        if !open_subgoals.iter().any(|e| e == og) {
            open_subgoals.push(og.clone());
        }
    }

    let seed = render_seed(
        &goal_state.goal,
        &closed_subgoals,
        &useful_lemmas,
        &open_subgoals,
    );
    Summary {
        goal: goal_state.goal.clone(),
        closed_subgoals,
        useful_lemmas,
        open_subgoals,
        seed,
    }
}

/// Harvest lemma / premise names a proof fragment leaned on: qualified dotted
/// identifiers (`Nat.succ_le_succ`, `List.getLast?`) anywhere, plus the bare
/// identifier immediately after an explicit citation tactic (`exact`/`apply`/
/// `refine`/`exacts`). De-duplicated, first-seen order — fully deterministic.
fn extract_lemmas(proof: &str) -> Vec<String> {
    const CITES: [&str; 4] = ["exact", "apply", "refine", "exacts"];
    let toks: Vec<&str> = proof
        .split_whitespace()
        .map(|raw| {
            raw.trim_matches(|c: char| {
                !(c.is_alphanumeric() || c == '.' || c == '_' || c == '?' || c == '\'')
            })
        })
        .collect();

    let mut out: Vec<String> = Vec::new();
    for (i, &tok) in toks.iter().enumerate() {
        if tok.is_empty() {
            continue;
        }
        let starts_alpha = tok.chars().next().is_some_and(|c| c.is_alphabetic());
        let looks_ident = starts_alpha
            && tok
                .chars()
                .all(|c| c.is_alphanumeric() || c == '.' || c == '_' || c == '?' || c == '\'');
        if !looks_ident {
            continue;
        }
        let is_dotted = tok.contains('.');
        let after_cite = i > 0 && CITES.contains(&toks[i - 1]);
        if (is_dotted || after_cite) && !out.iter().any(|e| e == tok) {
            out.push(tok.to_string());
        }
    }
    out
}

fn render_seed(goal: &str, closed: &[ClosedSubgoal], lemmas: &[String], open: &[String]) -> String {
    let mut s = String::new();
    s.push_str(&format!("Progress summary for goal: {goal}\n"));

    s.push_str("Already established (reuse, do not re-prove):\n");
    if closed.is_empty() {
        s.push_str("  (none)\n");
    } else {
        for c in closed {
            s.push_str(&format!("  - {}: {}\n", c.name, c.statement));
        }
    }

    if lemmas.is_empty() {
        s.push_str("Useful lemmas: (none)\n");
    } else {
        s.push_str(&format!("Useful lemmas: {}\n", lemmas.join(", ")));
    }

    s.push_str("Remaining subgoals:\n");
    if open.is_empty() {
        s.push_str("  (none)\n");
    } else {
        for o in open {
            s.push_str(&format!("  - {o}\n"));
        }
    }
    s
}

// ===========================================================================
// 2. reflective_redecompose — outer-loop re-decomposition
// ===========================================================================

/// Structured feedback pointing at the sketch step whose hole failed, plus the
/// (optional) verifier error and an (optional) explicit split.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedecomposeFeedback {
    /// Id of the sketch step whose subgoal failed to close.
    pub failing_step_id: String,
    /// The verifier error, carried through for provenance (unused by the split
    /// itself, which is structural).
    pub error: Option<VerifyError>,
    /// Explicit sub-subgoals to split the failing step into. When empty, a
    /// structural split is derived from the subgoal (top-level `∧`/`↔`), falling
    /// back to a single hard part wrapped by a bridging lemma.
    pub subparts: Vec<String>,
}

impl RedecomposeFeedback {
    /// Feedback that lets the split be derived structurally from the subgoal.
    pub fn on_step(failing_step_id: impl Into<String>) -> Self {
        Self {
            failing_step_id: failing_step_id.into(),
            error: None,
            subparts: Vec::new(),
        }
    }

    /// Attach an explicit split (caller-driven sub-subgoals).
    pub fn with_subparts(mut self, subparts: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.subparts = subparts.into_iter().map(Into::into).collect();
        self
    }

    /// Attach the verifier error for provenance.
    pub fn with_error(mut self, error: VerifyError) -> Self {
        self.error = Some(error);
        self
    }
}

/// The re-decomposed sketch plus what changed, for audit and downstream wiring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSketch {
    /// The new sketch: failing step replaced by parts + bridge, others preserved.
    pub sketch: InformalSketch,
    /// Ids of the steps introduced (the split parts then the bridge).
    pub added_steps: Vec<String>,
    /// Id of the step that was split out.
    pub split_step_id: String,
    /// Id of the bridging-lemma step, if one was introduced.
    pub bridge_step_id: Option<String>,
}

impl NewSketch {
    /// Whether a re-decomposition actually happened (the failing step was found).
    pub fn changed(&self) -> bool {
        self.bridge_step_id.is_some()
    }
}

/// Reflectively re-decompose a sketch whose subgoal failed. Splits the failing
/// step's subgoal into parts (explicit `subparts`, else a structural split on a
/// top-level `∧`/`↔`, else a single part), introduces a **bridging lemma** step
/// that `\uses` those parts to recover the original subgoal, reorders so the
/// bridge follows its parts, and **preserves every other step** — rewiring any
/// downstream `\uses` of the split step onto the bridge. Deterministic.
///
/// If `failing_step_id` is not in the sketch, the sketch is returned unchanged
/// (`changed() == false`).
pub fn reflective_redecompose(
    failed_sketch: &InformalSketch,
    feedback: &RedecomposeFeedback,
) -> NewSketch {
    let Some(fail_idx) = failed_sketch
        .steps
        .iter()
        .position(|s| s.id == feedback.failing_step_id)
    else {
        return NewSketch {
            sketch: failed_sketch.clone(),
            added_steps: Vec::new(),
            split_step_id: feedback.failing_step_id.clone(),
            bridge_step_id: None,
        };
    };

    let failing = &failed_sketch.steps[fail_idx];
    let base = failing.id.clone();
    // The subgoal to split: the hole's subgoal, or the prose if it carries none.
    let subgoal = failing
        .hole
        .as_ref()
        .map(|h| h.subgoal.clone())
        .unwrap_or_else(|| failing.prose.clone());

    // Determine split parts.
    let parts: Vec<String> = if feedback.subparts.is_empty() {
        split_subgoal(&subgoal)
    } else {
        feedback
            .subparts
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    };
    let parts = if parts.is_empty() {
        vec![subgoal.trim().to_string()]
    } else {
        parts
    };

    // Build the split part steps and the bridging-lemma step.
    let mut new_steps: Vec<SketchStep> = Vec::new();
    let mut added_steps: Vec<String> = Vec::new();
    let mut part_ids: Vec<String> = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let pid = format!("{base}_part{i}");
        new_steps.push(SketchStep::hole(
            pid.clone(),
            format!("Subpart {} of {base}", i + 1),
            part.clone(),
        ));
        part_ids.push(pid.clone());
        added_steps.push(pid);
    }

    let bridge_id = format!("{base}_bridge");
    // The bridge recombines the parts (and keeps the original step's deps) to
    // recover the original subgoal.
    let bridge_uses: Vec<String> = part_ids
        .iter()
        .cloned()
        .chain(failing.uses.iter().cloned())
        .collect();
    new_steps.push(
        SketchStep::hole(
            bridge_id.clone(),
            format!("Bridging lemma: combine subparts to obtain {base}"),
            subgoal.clone(),
        )
        .using(bridge_uses),
    );
    added_steps.push(bridge_id.clone());

    // Assemble: preserve every other step (rewiring `\uses` of the split step
    // onto the bridge), replacing the failing step with parts + bridge.
    let mut steps: Vec<SketchStep> = Vec::new();
    for (i, s) in failed_sketch.steps.iter().enumerate() {
        if i == fail_idx {
            steps.extend(new_steps.iter().cloned());
        } else {
            let mut s2 = s.clone();
            if s2.uses.iter().any(|u| u == &base) {
                s2.uses = s2
                    .uses
                    .iter()
                    .map(|u| {
                        if u == &base {
                            bridge_id.clone()
                        } else {
                            u.clone()
                        }
                    })
                    .collect();
            }
            steps.push(s2);
        }
    }

    NewSketch {
        sketch: InformalSketch::new(failed_sketch.statement.clone(), steps),
        added_steps,
        split_step_id: feedback.failing_step_id.clone(),
        bridge_step_id: Some(bridge_id),
    }
}

/// Structurally split a subgoal: on a top-level conjunction `A ∧ B` → `[A, B]`;
/// on a top-level `A ↔ B` → the two implications; otherwise the whole subgoal
/// (which still gets wrapped by a bridging lemma). Bracket-depth aware so nested
/// connectives are not split.
fn split_subgoal(subgoal: &str) -> Vec<String> {
    if let Some(parts) = split_top_level(subgoal, '∧') {
        return parts;
    }
    if let Some(parts) = split_top_level(subgoal, '↔') {
        if parts.len() == 2 {
            return vec![
                format!("{} → {}", parts[0], parts[1]),
                format!("{} → {}", parts[1], parts[0]),
            ];
        }
    }
    vec![subgoal.trim().to_string()]
}

/// Split `s` on `sep` only at bracket depth 0. Returns `None` if `sep` does not
/// occur at top level or fewer than two non-empty parts result.
fn split_top_level(s: &str, sep: char) -> Option<Vec<String>> {
    let mut depth: i32 = 0;
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut found = false;
    for c in s.chars() {
        match c {
            '(' | '[' | '{' | '⟨' => {
                depth += 1;
                cur.push(c);
            }
            ')' | ']' | '}' | '⟩' => {
                depth -= 1;
                cur.push(c);
            }
            _ if c == sep && depth == 0 => {
                found = true;
                parts.push(cur.trim().to_string());
                cur = String::new();
            }
            _ => cur.push(c),
        }
    }
    if !found {
        return None;
    }
    parts.push(cur.trim().to_string());
    let parts: Vec<String> = parts.into_iter().filter(|p| !p.is_empty()).collect();
    if parts.len() < 2 {
        None
    } else {
        Some(parts)
    }
}

// ===========================================================================
// 3. RefinementScheduler — inner (repair) / outer (re-decompose) alternation
// ===========================================================================

/// A refinement move: an inner-loop in-place repair, or an outer-loop
/// re-decomposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefinementMove {
    /// Inner loop: fix errors in place (drive [`crate::repair::repair_proof`]).
    InnerRepair,
    /// Outer loop: re-decompose the sketch ([`reflective_redecompose`] + re-run).
    OuterRedecompose,
}

/// The outcome of executing one refinement move, driving what the scheduler does
/// next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveOutcome {
    /// The move produced a verifier-passing proof — the schedule stops.
    Solved,
    /// The move made progress but the proof still fails — stay in this loop.
    Progressed,
    /// The move made no progress — escalate/switch to the other loop.
    Stalled,
}

/// One executed move in the schedule, for audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledMove {
    /// 0-based position in the schedule.
    pub index: usize,
    /// Which loop this move belonged to.
    pub kind: RefinementMove,
    /// What the move produced.
    pub outcome: MoveOutcome,
}

/// The record of a bounded refinement run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefinementSchedule {
    pub moves: Vec<ScheduledMove>,
    /// Whether a move solved the proof (some move returned [`MoveOutcome::Solved`]).
    pub solved: bool,
}

impl RefinementSchedule {
    /// Number of moves actually taken.
    pub fn len(&self) -> usize {
        self.moves.len()
    }

    /// Whether no move was taken.
    pub fn is_empty(&self) -> bool {
        self.moves.is_empty()
    }

    /// How many moves of `kind` were taken.
    pub fn count_of(&self, kind: RefinementMove) -> usize {
        self.moves.iter().filter(|m| m.kind == kind).count()
    }
}

/// Alternates inner-loop repair and outer-loop re-decomposition under a bounded
/// move budget. It **stays in the current loop while that loop makes progress**
/// ([`MoveOutcome::Progressed`]) and **switches loops when it stalls**
/// ([`MoveOutcome::Stalled`]) — so a proof that keeps improving under in-place
/// repair is never needlessly re-decomposed, while a stuck one escalates to the
/// outer loop. Bounded, deterministic, and returns the exact schedule taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefinementScheduler {
    /// Hard cap on the number of moves.
    pub max_moves: usize,
    /// Whether to begin with an inner (repair) move. Default `true`: try to fix
    /// in place first, re-decompose only when repair stalls.
    pub start_inner: bool,
}

impl Default for RefinementScheduler {
    fn default() -> Self {
        Self {
            max_moves: 6,
            start_inner: true,
        }
    }
}

impl RefinementScheduler {
    pub fn new(max_moves: usize, start_inner: bool) -> Self {
        Self {
            max_moves,
            start_inner,
        }
    }

    /// Run the alternation. `execute` is the injected driver: given the move to
    /// perform and its 0-based index, it performs it (inner repair or outer
    /// re-decompose) and reports a [`MoveOutcome`]. The scheduler stays in a loop
    /// on `Progressed`, switches loops on `Stalled`, and stops on `Solved` or the
    /// move budget. Deterministic given a deterministic `execute`.
    pub fn run(
        &self,
        mut execute: impl FnMut(RefinementMove, usize) -> MoveOutcome,
    ) -> RefinementSchedule {
        let mut moves: Vec<ScheduledMove> = Vec::new();
        let mut inner = self.start_inner;
        let mut solved = false;

        for index in 0..self.max_moves {
            let kind = if inner {
                RefinementMove::InnerRepair
            } else {
                RefinementMove::OuterRedecompose
            };
            let outcome = execute(kind, index);
            moves.push(ScheduledMove {
                index,
                kind,
                outcome,
            });
            match outcome {
                MoveOutcome::Solved => {
                    solved = true;
                    break;
                }
                MoveOutcome::Stalled => inner = !inner, // escalate/switch loops
                MoveOutcome::Progressed => {}           // stay in this loop
            }
        }

        RefinementSchedule { moves, solved }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    // -- summarize_progress --------------------------------------------------

    fn sample_partial() -> PartialProof {
        PartialProof::new(
            "whole failing attempt text",
            vec![
                SubgoalStatus::closed("base", "P 0", "exact Nat.zero_le n"),
                SubgoalStatus::closed(
                    "step",
                    "∀ n, P n → P (n+1)",
                    "intro n ih; apply Nat.succ_le_succ; exact ih",
                ),
                SubgoalStatus::open("final", "P n for all n"),
            ],
        )
    }

    #[test]
    fn summarize_extracts_closed_subgoals_and_partitions_open() {
        let partial = sample_partial();
        let gs = GoalState::new("P n for all n", vec![]);
        let summary = summarize_progress(&partial, &gs);

        // Exactly the two closed subgoals are carried, in order.
        assert_eq!(summary.closed_subgoals.len(), 2);
        assert_eq!(summary.closed_subgoals[0].name, "base");
        assert_eq!(summary.closed_subgoals[0].statement, "P 0");
        assert_eq!(summary.closed_subgoals[1].name, "step");
        // The open subgoal is surfaced as remaining work.
        assert_eq!(summary.open_subgoals, vec!["P n for all n".to_string()]);
        assert_eq!(summary.goal, "P n for all n");
    }

    #[test]
    fn summarize_harvests_lemmas_from_closed_proofs_deduped_in_order() {
        let partial = sample_partial();
        let gs = GoalState::new("P n for all n", vec![]);
        let summary = summarize_progress(&partial, &gs);

        // Dotted lemmas + the identifier cited after `exact`/`apply`, first-seen,
        // de-duplicated (Nat.succ_le_succ appears via `apply`, then `ih` via
        // `exact`).
        assert_eq!(
            summary.useful_lemmas,
            vec![
                "Nat.zero_le".to_string(),
                "Nat.succ_le_succ".to_string(),
                "ih".to_string(),
            ]
        );
        // The compact seed mentions closed work and the lemmas.
        assert!(summary.seed.contains("base: P 0"));
        assert!(summary.seed.contains("Nat.zero_le"));
        assert!(summary.seed.contains("Remaining subgoals"));
    }

    #[test]
    fn summarize_merges_prover_reported_open_subgoals_without_duplicates() {
        let partial = PartialProof::new("attempt", vec![SubgoalStatus::open("g", "already open")]);
        // One overlapping, one new open subgoal reported by the prover.
        let gs = GoalState::new("goal", vec!["already open".into(), "extra open".into()]);
        let summary = summarize_progress(&partial, &gs);
        assert_eq!(
            summary.open_subgoals,
            vec!["already open".to_string(), "extra open".to_string()]
        );
        assert!(summary.closed_subgoals.is_empty());
        assert!(summary.useful_lemmas.is_empty());
    }

    #[test]
    fn summarize_is_deterministic() {
        let partial = sample_partial();
        let gs = GoalState::new("P n for all n", vec!["x".into()]);
        let a = summarize_progress(&partial, &gs);
        let b = summarize_progress(&partial, &gs);
        assert_eq!(a, b);
    }

    // -- reflective_redecompose ---------------------------------------------

    /// A 3-step sketch: two holes (s1, s2 uses s1) + a prose conclusion s3 that
    /// uses s2. s2 is the failing step.
    fn sample_sketch() -> InformalSketch {
        InformalSketch::new(
            "T",
            vec![
                SketchStep::hole("s1", "base", "P 0"),
                SketchStep::hole("s2", "hard step", "A ∧ B").using(["s1"]),
                SketchStep::prose("s3", "conclude").using(["s2"]),
            ],
        )
    }

    #[test]
    fn redecompose_splits_conjunction_and_adds_bridge_preserving_others() {
        let sketch = sample_sketch();
        let fb = RedecomposeFeedback::on_step("s2");
        let out = reflective_redecompose(&sketch, &fb);

        assert!(out.changed());
        assert_eq!(out.split_step_id, "s2");
        assert_eq!(out.bridge_step_id.as_deref(), Some("s2_bridge"));
        // A ∧ B split into two parts + a bridge.
        assert_eq!(
            out.added_steps,
            vec![
                "s2_part0".to_string(),
                "s2_part1".to_string(),
                "s2_bridge".to_string()
            ]
        );

        let ids: Vec<&str> = out.sketch.steps.iter().map(|s| s.id.as_str()).collect();
        // s1 preserved; s2 replaced in place by part0, part1, bridge; s3 preserved.
        assert_eq!(ids, vec!["s1", "s2_part0", "s2_part1", "s2_bridge", "s3"]);

        // The two parts carry the conjuncts.
        let part0 = out
            .sketch
            .steps
            .iter()
            .find(|s| s.id == "s2_part0")
            .unwrap();
        let part1 = out
            .sketch
            .steps
            .iter()
            .find(|s| s.id == "s2_part1")
            .unwrap();
        assert_eq!(part0.hole.as_ref().unwrap().subgoal, "A");
        assert_eq!(part1.hole.as_ref().unwrap().subgoal, "B");

        // The bridge recombines the parts (and keeps s2's original dep on s1),
        // and its subgoal is the original A ∧ B.
        let bridge = out
            .sketch
            .steps
            .iter()
            .find(|s| s.id == "s2_bridge")
            .unwrap();
        assert_eq!(bridge.hole.as_ref().unwrap().subgoal, "A ∧ B");
        assert_eq!(bridge.uses, vec!["s2_part0", "s2_part1", "s1"]);

        // Downstream s3's `\uses` was rewired from s2 onto the bridge.
        let s3 = out.sketch.steps.iter().find(|s| s.id == "s3").unwrap();
        assert_eq!(s3.uses, vec!["s2_bridge"]);
    }

    #[test]
    fn redecompose_honours_explicit_subparts() {
        let sketch = sample_sketch();
        let fb =
            RedecomposeFeedback::on_step("s2").with_subparts(["lemma X", "residual Y", "tail Z"]);
        let out = reflective_redecompose(&sketch, &fb);
        // Three explicit parts + bridge.
        assert_eq!(
            out.added_steps,
            vec![
                "s2_part0".to_string(),
                "s2_part1".to_string(),
                "s2_part2".to_string(),
                "s2_bridge".to_string(),
            ]
        );
        let subgoals: Vec<String> = out
            .sketch
            .steps
            .iter()
            .filter(|s| s.id.starts_with("s2_part"))
            .map(|s| s.hole.as_ref().unwrap().subgoal.clone())
            .collect();
        assert_eq!(subgoals, vec!["lemma X", "residual Y", "tail Z"]);
    }

    #[test]
    fn redecompose_non_splittable_subgoal_still_wraps_in_a_bridge() {
        let sketch =
            InformalSketch::new("T", vec![SketchStep::hole("g", "atomic", "Irreducible a")]);
        let out = reflective_redecompose(&sketch, &RedecomposeFeedback::on_step("g"));
        assert!(out.changed());
        // One part carrying the whole subgoal + a bridge.
        assert_eq!(
            out.added_steps,
            vec!["g_part0".to_string(), "g_bridge".to_string()]
        );
        let part = out.sketch.steps.iter().find(|s| s.id == "g_part0").unwrap();
        assert_eq!(part.hole.as_ref().unwrap().subgoal, "Irreducible a");
    }

    #[test]
    fn redecompose_unknown_step_returns_sketch_unchanged() {
        let sketch = sample_sketch();
        let out = reflective_redecompose(&sketch, &RedecomposeFeedback::on_step("nope"));
        assert!(!out.changed());
        assert!(out.added_steps.is_empty());
        assert_eq!(out.sketch, sketch);
    }

    #[test]
    fn split_top_level_respects_bracket_depth() {
        // The inner ∧ is bracketed, so only the top-level ∧ splits.
        let parts = split_subgoal("(A ∧ B) ∧ C");
        assert_eq!(parts, vec!["(A ∧ B)".to_string(), "C".to_string()]);
    }

    #[test]
    fn redecompose_is_deterministic() {
        let sketch = sample_sketch();
        let fb = RedecomposeFeedback::on_step("s2");
        let a = reflective_redecompose(&sketch, &fb);
        let b = reflective_redecompose(&sketch, &fb);
        assert_eq!(a, b);
    }

    // -- RefinementScheduler -------------------------------------------------

    #[test]
    fn scheduler_alternates_inner_and_outer_on_stall_within_bounds() {
        let sched = RefinementScheduler::new(4, true);
        // Every move stalls, forcing a switch each time.
        let schedule = sched.run(|_kind, _i| MoveOutcome::Stalled);

        assert_eq!(schedule.len(), 4); // bounded by max_moves
        assert!(!schedule.solved);
        let kinds: Vec<RefinementMove> = schedule.moves.iter().map(|m| m.kind).collect();
        assert_eq!(
            kinds,
            vec![
                RefinementMove::InnerRepair,
                RefinementMove::OuterRedecompose,
                RefinementMove::InnerRepair,
                RefinementMove::OuterRedecompose,
            ]
        );
        assert_eq!(schedule.count_of(RefinementMove::InnerRepair), 2);
        assert_eq!(schedule.count_of(RefinementMove::OuterRedecompose), 2);
    }

    #[test]
    fn scheduler_stays_in_inner_loop_while_repair_progresses_then_escalates() {
        // Progress twice in the inner loop, then stall (→ outer), then solve.
        let outcomes = RefinementCell::new(vec![
            MoveOutcome::Progressed,
            MoveOutcome::Progressed,
            MoveOutcome::Stalled,
            MoveOutcome::Solved,
        ]);
        let sched = RefinementScheduler::new(10, true);
        let schedule = sched.run(|kind, _i| {
            outcomes.record_kind(kind);
            outcomes.next()
        });

        assert!(schedule.solved);
        let kinds: Vec<RefinementMove> = schedule.moves.iter().map(|m| m.kind).collect();
        // Three inner moves (progress, progress, stall) then one outer (solve).
        assert_eq!(
            kinds,
            vec![
                RefinementMove::InnerRepair,
                RefinementMove::InnerRepair,
                RefinementMove::InnerRepair,
                RefinementMove::OuterRedecompose,
            ]
        );
        // Stopped exactly at the solving move — the 10-move budget was not spent.
        assert_eq!(schedule.len(), 4);
    }

    #[test]
    fn scheduler_can_start_outer_and_is_deterministic() {
        let sched = RefinementScheduler::new(3, false);
        let a = sched.run(|_k, _i| MoveOutcome::Stalled);
        let b = sched.run(|_k, _i| MoveOutcome::Stalled);
        assert_eq!(a, b);
        assert_eq!(a.moves[0].kind, RefinementMove::OuterRedecompose);
    }

    #[test]
    fn scheduler_zero_budget_takes_no_moves() {
        let sched = RefinementScheduler::new(0, true);
        let schedule = sched.run(|_k, _i| MoveOutcome::Solved);
        assert!(schedule.is_empty());
        assert!(!schedule.solved);
    }

    /// A tiny deterministic outcome queue for driving the scheduler in tests.
    struct RefinementCell {
        outcomes: Vec<MoveOutcome>,
        cursor: RefCell<usize>,
        kinds: RefCell<Vec<RefinementMove>>,
    }
    impl RefinementCell {
        fn new(outcomes: Vec<MoveOutcome>) -> Self {
            Self {
                outcomes,
                cursor: RefCell::new(0),
                kinds: RefCell::new(Vec::new()),
            }
        }
        fn next(&self) -> MoveOutcome {
            let mut c = self.cursor.borrow_mut();
            let o = self.outcomes[*c];
            *c += 1;
            o
        }
        fn record_kind(&self, kind: RefinementMove) {
            self.kinds.borrow_mut().push(kind);
        }
    }
}
