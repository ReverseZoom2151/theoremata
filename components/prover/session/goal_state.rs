//! Live goal-state extractor (ImProver "chain-of-states"): dump the prover's
//! ground-truth intermediate goal state at a hole and parse it into a structured
//! Chain-of-States value.
//!
//! Two halves, with a hard split between the offline-testable core and the
//! live-gated wrapper:
//!
//! * [`parse_lean_goal_state`] — the PARSER. Pure, total, panic-free. It takes
//!   the raw text Lean prints for a goal state (as emitted by the `trace_state`
//!   tactic or an "unsolved goals" error) and turns it into a [`GoalState`]:
//!   multiple [`Goal`]s (split on blank lines / `case` markers), each a list of
//!   [`Hyp`] hypothesis bindings plus a target. It is robust to the ASCII (`|-`)
//!   and unicode (`⊢`) turnstiles, several names per hypothesis line, multi-goal
//!   output, the empty / "no goals" case, and outright garbage (which degrades
//!   to an empty [`GoalState`] rather than panicking). ALL of this input is
//!   treated as untrusted DATA — it is parsed, never executed. This is the fully
//!   offline-tested core.
//!
//! * [`LeanGoalStateExtractor`] — the live wrapper. It builds a tiny Lean probe
//!   that elaborates a candidate `attempt` against a `subgoal` and prints the
//!   remaining goal state via `trace_state`, invokes Lean through the same
//!   [`Runner`] bridge the rest of the live gate uses, and feeds the captured
//!   stdout to [`parse_lean_goal_state`]. When Lean is not available under the
//!   configured runner it returns `None`, so it is a safe no-op in a toolchain-
//!   less build. It implements [`sketch::GoalStateExtractor`] by rendering the
//!   parsed state back into the readable `Option<String>` the sketch retry loop
//!   threads into the next attempt.

use crate::prover::exec::{self, Runner};
use crate::reason::proving::sketch;

/// One hypothesis binding in a goal: one or more names sharing a type
/// (`n₁ n₂ : ℕ` → names `["n₁", "n₂"]`, ty `"ℕ"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hyp {
    /// The bound names on the left of the `:` (whitespace-separated in Lean).
    pub names: Vec<String>,
    /// The type/proposition on the right of the `:`.
    pub ty: String,
}

/// A single goal: its local hypothesis context and the target under `⊢`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Goal {
    pub hypotheses: Vec<Hyp>,
    pub target: String,
}

/// A parsed Chain-of-States goal state: zero or more goals. Zero goals is the
/// "no goals" / proof-complete case.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GoalState {
    pub goals: Vec<Goal>,
}

impl GoalState {
    /// Whether there are no open goals (proof complete or nothing to show).
    pub fn is_empty(&self) -> bool {
        self.goals.is_empty()
    }

    /// Render the state back to a readable, Lean-shaped string (the inverse of
    /// the parse, up to normalization): each goal's hypotheses one binding per
    /// line, then the `⊢ target` line; goals separated by a blank line and, when
    /// there is more than one, prefixed with a `case N` marker so the output is
    /// unambiguous. Returns an empty string for the empty state.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for (i, goal) in self.goals.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            if self.goals.len() > 1 {
                out.push_str(&format!("case {}\n", i + 1));
            }
            for hyp in &goal.hypotheses {
                out.push_str(&format!("{} : {}\n", hyp.names.join(" "), hyp.ty));
            }
            out.push_str(&format!("⊢ {}\n", goal.target));
        }
        out
    }
}

/// The turnstiles that introduce a goal's target line.
const TURNSTILES: [&str; 2] = ["⊢", "|-"];

/// Lines that signal an explicitly empty state; matched case-insensitively as a
/// whole-content substring so both the tactic output ("no goals") and the REPL
/// banner ("goals accomplished") short-circuit to an empty [`GoalState`].
fn is_no_goals(raw: &str) -> bool {
    let low = raw.to_lowercase();
    low.contains("no goals") || low.contains("goals accomplished")
}

/// Split raw goal-state text into per-goal blocks. A new block starts on a
/// `case …` marker or after a blank line that follows a completed goal (one that
/// already has a turnstile). Blank lines inside a goal's context are ignored, and
/// `case` marker lines are consumed as boundaries (not kept as hypotheses).
fn split_goal_blocks(raw: &str) -> Vec<Vec<String>> {
    let mut blocks: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut current_has_target = false;

    let finish = |current: &mut Vec<String>, blocks: &mut Vec<Vec<String>>, has: &mut bool| {
        if !current.is_empty() {
            blocks.push(std::mem::take(current));
        }
        *has = false;
    };

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            // A blank line closes a goal only once its target has been seen;
            // otherwise it is noise between context lines.
            if current_has_target {
                finish(&mut current, &mut blocks, &mut current_has_target);
            }
            continue;
        }
        if is_case_marker(trimmed) {
            finish(&mut current, &mut blocks, &mut current_has_target);
            continue;
        }
        if has_turnstile(trimmed) {
            current_has_target = true;
        }
        current.push(trimmed.to_string());
    }
    finish(&mut current, &mut blocks, &mut current_has_target);
    blocks
}

/// Whether a trimmed line is a `case`/`case'` goal-tag marker (e.g. `case pos`,
/// `case succ n ih`). A bare `case` with no tag still counts as a boundary.
fn is_case_marker(trimmed: &str) -> bool {
    trimmed == "case"
        || trimmed.starts_with("case ")
        || trimmed == "case'"
        || trimmed.starts_with("case' ")
}

/// Whether a line contains a target turnstile.
fn has_turnstile(line: &str) -> bool {
    TURNSTILES.iter().any(|t| line.contains(t))
}

/// Split a line at its turnstile, returning `(before, target)` where `target` is
/// the text after the marker. `before` is whatever preceded the turnstile on the
/// same line (usually empty).
fn split_at_turnstile(line: &str) -> Option<(&str, &str)> {
    let mut best: Option<(usize, usize)> = None;
    for t in TURNSTILES {
        if let Some(idx) = line.find(t) {
            // Prefer the earliest turnstile occurrence on the line.
            if best.map(|(b, _)| idx < b).unwrap_or(true) {
                best = Some((idx, t.len()));
            }
        }
    }
    best.map(|(idx, len)| (&line[..idx], &line[idx + len..]))
}

/// Parse one hypothesis line (`n₁ n₂ : T`) into a [`Hyp`]. Splits on the FIRST
/// `:` so a `:` inside the type is preserved. Returns `None` for a line with no
/// `:` or with no names on the left (malformed → skipped, never panics).
fn parse_hyp_line(line: &str) -> Option<Hyp> {
    let (lhs, rhs) = line.split_once(':')?;
    let names: Vec<String> = lhs.split_whitespace().map(str::to_string).collect();
    if names.is_empty() {
        return None;
    }
    Some(Hyp {
        names,
        ty: rhs.trim().to_string(),
    })
}

/// Parse one goal block (already stripped of `case` markers) into a [`Goal`].
/// Everything before the turnstile line is a hypothesis binding; the turnstile
/// line's remainder plus any continuation lines form the target. A block with no
/// turnstile is not a goal and yields `None` (so garbage is dropped).
fn parse_goal_block(block: &[String]) -> Option<Goal> {
    let target_idx = block.iter().position(|l| has_turnstile(l))?;
    let mut hypotheses = Vec::new();
    for line in &block[..target_idx] {
        if let Some(hyp) = parse_hyp_line(line) {
            hypotheses.push(hyp);
        }
    }
    // Target: text after the turnstile, plus any trailing continuation lines in
    // the block (multi-line targets), joined by a single space.
    let (_, first) = split_at_turnstile(&block[target_idx])?;
    let mut parts: Vec<String> = Vec::new();
    let first = first.trim();
    if !first.is_empty() {
        parts.push(first.to_string());
    }
    for line in &block[target_idx + 1..] {
        let t = line.trim();
        if !t.is_empty() {
            parts.push(t.to_string());
        }
    }
    Some(Goal {
        hypotheses,
        target: parts.join(" "),
    })
}

/// Parse Lean's goal-state text into a structured [`GoalState`].
///
/// Handles: a single goal (hypotheses + `⊢ target`); multiple goals separated by
/// blank lines and/or `case` markers; the empty / "no goals" case; several names
/// per hypothesis line; both `⊢` and `|-` turnstiles; and arbitrary garbage,
/// which degrades to an empty [`GoalState`]. Total and panic-free — the input is
/// untrusted data.
pub fn parse_lean_goal_state(raw: &str) -> GoalState {
    if raw.trim().is_empty() || is_no_goals(raw) {
        return GoalState::default();
    }
    let goals = split_goal_blocks(raw)
        .iter()
        .filter_map(|b| parse_goal_block(b))
        .collect();
    GoalState { goals }
}

/// The live goal-state extractor: shells out to Lean through a [`Runner`] to dump
/// the intermediate goal state, then parses it with [`parse_lean_goal_state`].
///
/// Safe in a toolchain-less build: [`LeanGoalStateExtractor::dump`] returns `None`
/// whenever Lean is not available under the configured runner (or the process
/// cannot be launched), so the sketch retry loop degrades to error-only behaviour
/// exactly as with the [`sketch::StubGoalStateExtractor`].
pub struct LeanGoalStateExtractor {
    /// The runner bridge (Native / WSL / Docker) — mirrors `LeanBackend`.
    pub runner: Runner,
    /// The `lean` binary name/path (env-overridable via the caller).
    pub lean: String,
}

impl LeanGoalStateExtractor {
    /// Construct with an explicit runner and lean binary.
    pub fn new(runner: Runner, lean: impl Into<String>) -> Self {
        Self {
            runner,
            lean: lean.into(),
        }
    }

    /// Native runner reading the `THEOREMATA_LEAN` env override (default `lean`),
    /// matching how [`crate::prover::lean::LeanBackend::live`] resolves the binary.
    #[allow(dead_code)] // convenience ctor for the live wiring the parent adds.
    pub fn native() -> Self {
        Self::new(Runner::Native, exec::env_or("THEOREMATA_LEAN", "lean"))
    }

    /// Whether Lean can be launched under the configured runner.
    pub fn available(&self) -> bool {
        exec::probe(&self.runner, &[&self.lean, "--version"])
    }

    /// The exact Lean probe source: import the default corpus, then a throwaway
    /// theorem whose type is `subgoal`, proved by the candidate `attempt`,
    /// followed by the `trace_state` tactic which prints the REMAINING goal state
    /// to stdout. If `attempt` closes the goal, Lean prints "no goals" (parsed to
    /// an empty state); if it fails, the "unsolved goals" / error text still
    /// carries the goal state, which we parse all the same.
    ///
    /// The `subgoal` and `attempt` are written into the source as Lean DATA and
    /// only ever elaborated by Lean — never interpolated into a shell (argv is
    /// passed verbatim through the [`Runner`]).
    fn probe_source(subgoal: &str, attempt: &str) -> String {
        use crate::prover::formal::FormalSystem;
        let mut src = String::new();
        for imp in FormalSystem::Lean.default_imports() {
            src.push_str(&format!("import {imp}\n"));
        }
        src.push('\n');
        // Indent every line of the (possibly multi-line) attempt into the tactic
        // block so a multi-tactic attempt elaborates correctly.
        src.push_str(&format!("theorem __theoremata_probe__ : {subgoal} := by\n"));
        for line in attempt.lines() {
            src.push_str(&format!("  {line}\n"));
        }
        src.push_str("  trace_state\n");
        src
    }

    /// Invoke Lean on the probe and return the parsed goal state. `None` when Lean
    /// is unavailable or the process could not be launched; `Some(state)` (which
    /// may be empty) once Lean has run and its output has been parsed.
    pub fn dump(&self, subgoal: &str, attempt: &str) -> Option<GoalState> {
        if !self.available() {
            return None;
        }
        // A fixed workspace dir under the system temp — deterministic (no rand /
        // wall-clock), created if missing.
        let workspace = std::env::temp_dir().join("theoremata_goal_probe");
        if std::fs::create_dir_all(&workspace).is_err() {
            return None;
        }
        let file = "Probe.lean";
        if std::fs::write(workspace.join(file), Self::probe_source(subgoal, attempt)).is_err() {
            return None;
        }
        let out = exec::run(&self.runner, &[&self.lean, file], &workspace);
        if !out.launched {
            return None;
        }
        // The goal state lands on stdout (`trace_state`) or stderr (error path);
        // parse whichever carries a turnstile, preferring stdout.
        let raw = if has_turnstile(&out.stdout) {
            out.stdout
        } else if has_turnstile(&out.stderr) {
            out.stderr
        } else {
            out.stdout
        };
        Some(parse_lean_goal_state(&raw))
    }
}

/// Bridge to the sketch retry loop: dump + parse + render. A non-empty state
/// renders to `Some(readable string)`; an unavailable prover or an empty state
/// yields `None`, so the retry degrades to error-only behaviour unchanged.
impl sketch::GoalStateExtractor for LeanGoalStateExtractor {
    fn extract(&self, subgoal: &str, attempt: &str) -> Option<String> {
        let state = self.dump(subgoal, attempt)?;
        if state.is_empty() {
            return None;
        }
        Some(state.render())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reason::proving::sketch::GoalStateExtractor;

    #[test]
    fn single_goal_two_hyps_and_target() {
        let raw = "n : ℕ\nh : n > 0\n⊢ n ≥ 1\n";
        let state = parse_lean_goal_state(raw);
        assert_eq!(state.goals.len(), 1);
        let g = &state.goals[0];
        assert_eq!(
            g.hypotheses,
            vec![
                Hyp {
                    names: vec!["n".into()],
                    ty: "ℕ".into()
                },
                Hyp {
                    names: vec!["h".into()],
                    ty: "n > 0".into()
                },
            ]
        );
        assert_eq!(g.target, "n ≥ 1");
    }

    #[test]
    fn multiple_names_on_one_hyp_line() {
        let raw = "a b c : ℤ\n⊢ a + b + c = c + b + a\n";
        let state = parse_lean_goal_state(raw);
        assert_eq!(state.goals.len(), 1);
        assert_eq!(
            state.goals[0].hypotheses,
            vec![Hyp {
                names: vec!["a".into(), "b".into(), "c".into()],
                ty: "ℤ".into()
            }]
        );
        assert_eq!(state.goals[0].target, "a + b + c = c + b + a");
    }

    #[test]
    fn multi_goal_split_on_case_markers() {
        let raw = "case pos\nh : p\n⊢ q\n\ncase neg\nh : ¬p\n⊢ r\n";
        let state = parse_lean_goal_state(raw);
        assert_eq!(state.goals.len(), 2);
        assert_eq!(state.goals[0].target, "q");
        assert_eq!(state.goals[1].target, "r");
        assert_eq!(state.goals[0].hypotheses[0].ty, "p");
        assert_eq!(state.goals[1].hypotheses[0].ty, "¬p");
    }

    #[test]
    fn multi_goal_split_on_blank_lines_without_case() {
        let raw = "n : ℕ\n⊢ P n\n\nm : ℕ\n⊢ Q m\n";
        let state = parse_lean_goal_state(raw);
        assert_eq!(state.goals.len(), 2);
        assert_eq!(state.goals[0].target, "P n");
        assert_eq!(state.goals[1].target, "Q m");
    }

    #[test]
    fn ascii_turnstile_is_accepted() {
        let raw = "h : True\n|- False → True\n";
        let state = parse_lean_goal_state(raw);
        assert_eq!(state.goals.len(), 1);
        assert_eq!(state.goals[0].hypotheses[0].names, vec!["h".to_string()]);
        assert_eq!(state.goals[0].target, "False → True");
    }

    #[test]
    fn no_goals_and_empty_parse_to_empty_state() {
        assert!(parse_lean_goal_state("").is_empty());
        assert!(parse_lean_goal_state("   \n  \n").is_empty());
        assert!(parse_lean_goal_state("no goals").is_empty());
        assert!(parse_lean_goal_state("Goals accomplished!").is_empty());
    }

    #[test]
    fn garbage_input_degrades_to_empty_state_without_panic() {
        // No turnstile anywhere => no goals recovered => empty state.
        assert!(parse_lean_goal_state("asdf qwer zxcv").is_empty());
        assert!(parse_lean_goal_state("}{ ][ random :: ::: garbage").is_empty());
        // A colon-bearing garbage line with no turnstile is still dropped.
        assert!(parse_lean_goal_state("foo : bar\nbaz qux").is_empty());
    }

    #[test]
    fn target_only_goal_has_no_hypotheses() {
        let state = parse_lean_goal_state("⊢ 2 + 2 = 4\n");
        assert_eq!(state.goals.len(), 1);
        assert!(state.goals[0].hypotheses.is_empty());
        assert_eq!(state.goals[0].target, "2 + 2 = 4");
    }

    #[test]
    fn hyp_type_may_contain_colons() {
        // Only the FIRST colon splits names from type.
        let state = parse_lean_goal_state("f : A → B : Type\n⊢ True\n");
        assert_eq!(state.goals[0].hypotheses[0].names, vec!["f".to_string()]);
        assert_eq!(state.goals[0].hypotheses[0].ty, "A → B : Type");
    }

    #[test]
    fn render_round_trips_shape_for_single_goal() {
        let raw = "n : ℕ\nh : n > 0\n⊢ n ≥ 1\n";
        let rendered = parse_lean_goal_state(raw).render();
        // Re-parsing the render yields the same structure.
        assert_eq!(parse_lean_goal_state(&rendered), parse_lean_goal_state(raw));
        assert!(rendered.contains("n : ℕ"));
        assert!(rendered.contains("⊢ n ≥ 1"));
    }

    #[test]
    fn render_multi_goal_tags_cases_and_reparses() {
        let raw = "h : p\n⊢ q\n\nh : ¬p\n⊢ r\n";
        let parsed = parse_lean_goal_state(raw);
        let rendered = parsed.render();
        assert!(rendered.contains("case 1"));
        assert!(rendered.contains("case 2"));
        assert_eq!(parse_lean_goal_state(&rendered), parsed);
    }

    #[test]
    fn extractor_renders_nonempty_state_to_some_and_empty_to_none() {
        // Exercise the sketch::GoalStateExtractor rendering seam directly, without
        // any live Lean, by driving the pure state → string path.
        let nonempty = parse_lean_goal_state("h : p\n⊢ q\n");
        assert!(!nonempty.is_empty());
        let rendered = nonempty.render();
        assert!(rendered.contains("⊢ q"));

        let empty = parse_lean_goal_state("no goals");
        assert!(empty.is_empty());
    }

    #[test]
    fn extractor_is_none_when_lean_unavailable() {
        // A runner pointed at a binary that cannot exist must degrade to None,
        // never panic — this is the safety property that keeps the live extractor
        // inert in a toolchain-less build.
        let ex =
            LeanGoalStateExtractor::new(Runner::Native, "theoremata-nonexistent-lean-binary-xyz");
        assert!(!ex.available());
        assert_eq!(ex.dump("True", "trivial"), None);
        assert_eq!(GoalStateExtractor::extract(&ex, "True", "trivial"), None);
    }

    #[test]
    fn probe_source_embeds_subgoal_and_attempt_as_data() {
        let src = LeanGoalStateExtractor::probe_source("P 0", "intro h\napply h");
        assert!(src.contains("import Mathlib"));
        assert!(src.contains("theorem __theoremata_probe__ : P 0 := by"));
        // Multi-line attempts are indented into the tactic block.
        assert!(src.contains("  intro h"));
        assert!(src.contains("  apply h"));
        assert!(src.contains("  trace_state"));
    }
}
