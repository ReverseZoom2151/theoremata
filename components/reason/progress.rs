//! Proof-progress heuristic (a LeanProgress-style search prior).
//!
//! LeanProgress ("Guiding Search for Neural Theorem Proving via Proof Progress
//! Prediction") trains a model to predict *how many tactic steps remain* until a
//! proof state reaches `no goals`, and feeds that prediction back into search as
//! a value/reward signal (the LeanDojo-v2 API returns *negative predicted
//! remaining steps* as a reward, so states closer to done rank higher).
//!
//! We cannot train that model here, so this module is a **principled,
//! deterministic, offline stand-in**: it estimates "how close is this proof
//! state to done?" from cheap structural features of the pretty-printed goal
//! state — goal count, hypothesis count, conclusion term size, and subgoal /
//! term nesting depth — and squashes them into a value in `[0, 1]` where `1.0`
//! means "no goals" (done) and smaller values mean farther away. It approximates
//! LeanProgress's steps-remaining signal monotonically (fewer goals and a smaller
//! goal term ⇒ closer to done ⇒ higher value) without any learned weights.
//!
//! When a learned progress model becomes available it should replace
//! [`progress_value`] behind the same signature; everything downstream (the MCTS
//! prior in `mcts.rs`, the phase/progress selector in `sampler.rs`) consumes the
//! `[0, 1]` value and is agnostic to how it was produced.

/// Default blend weight for the progress prior when it is mixed into another
/// score (PUCT selection in `mcts.rs`, phase/progress ranking in `sampler.rs`).
/// A knob, not a constant of nature: `0.0` disables the prior (recovering the
/// prior-free behaviour), larger values trust the progress estimate more.
pub const PROGRESS_PRIOR_WEIGHT: f64 = 0.5;

// Feature-decay scales: how many units of each feature halve its contribution.
// Chosen so a "typical" small goal scores high and large/deep goals decay
// gently rather than falling off a cliff.
const TERM_SCALE: f64 = 32.0;
const HYP_SCALE: f64 = 8.0;
const DEPTH_SCALE: f64 = 6.0;

// Relative weights of the four features (sum to 1). Goal count dominates — the
// number of open goals is the strongest coarse signal of remaining work — with
// term size next and structural depth / hypothesis count as finer corrections.
const W_GOALS: f64 = 0.50;
const W_TERM: f64 = 0.25;
const W_DEPTH: f64 = 0.15;
const W_HYPS: f64 = 0.10;

/// Structural features of a proof state, extracted from its pretty-printed form.
/// Every field is "more ⇒ farther from done", so [`progress_value`] is decreasing
/// in each of them (and `goal_count == 0` is the terminal "no goals" state).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProgressFeatures {
    /// Number of open goals (`0` ⇒ `no goals`, i.e. proof complete).
    pub goal_count: usize,
    /// Total local hypotheses across the displayed goals.
    pub hypothesis_count: usize,
    /// Approximate size of the goal conclusion(s), in whitespace tokens.
    pub term_size: usize,
    /// Maximum bracket-nesting depth of the state text — a proxy for how deeply
    /// structured the remaining goal term is.
    pub subgoal_depth: usize,
}

impl ProgressFeatures {
    /// Parse features from a pretty-printed Lean goal state such as:
    ///
    /// ```text
    /// case succ
    /// n : ℕ
    /// ih : n + 0 = n
    /// ⊢ n + 1 + 0 = n + 1
    /// ```
    ///
    /// Each goal contributes one turnstile (`⊢`, or ASCII `|-`); the text after
    /// it is the conclusion (counted toward `term_size`). Non-turnstile,
    /// non-`case` lines that read like `name : type` count as hypotheses. The
    /// empty state or the literal `no goals` yields all-zero features (done).
    ///
    /// This is a deliberately forgiving text parser (a compatibility shim in the
    /// spirit of LeanDojo's `parse_goals.py`): continuation lines of a long
    /// conclusion may be mis-attributed, which only perturbs the heuristic, never
    /// soundness — the Lean checker remains the only authority on validity.
    pub fn parse(state: &str) -> Self {
        let trimmed = state.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("no goals") {
            return Self::default();
        }

        let mut goal_count = 0;
        let mut hypothesis_count = 0;
        let mut term_size = 0;

        for raw in state.lines() {
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(rest) = turnstile_conclusion(line) {
                goal_count += 1;
                term_size += rest.split_whitespace().count();
            } else if line.starts_with("case") {
                // goal label / focus marker — neither a hypothesis nor a goal.
                continue;
            } else if is_hypothesis_line(line) {
                hypothesis_count += 1;
            }
        }

        Self {
            goal_count,
            hypothesis_count,
            term_size,
            subgoal_depth: max_nesting_depth(state),
        }
    }
}

/// If `line` contains a goal turnstile, return the conclusion text after it.
fn turnstile_conclusion(line: &str) -> Option<&str> {
    if let Some(idx) = line.find('⊢') {
        return Some(line[idx + '⊢'.len_utf8()..].trim_start());
    }
    if let Some(idx) = line.find("|-") {
        return Some(line[idx + 2..].trim_start());
    }
    None
}

/// A hypothesis line looks like `name : type` (a ` : ` separator that is not the
/// leading token). Turnstile lines are handled earlier, so they never reach here.
fn is_hypothesis_line(line: &str) -> bool {
    match line.find(" : ") {
        Some(pos) => pos > 0,
        None => false,
    }
}

/// Maximum nesting depth across the usual bracket pairs, over the whole state.
fn max_nesting_depth(state: &str) -> usize {
    let mut depth: i32 = 0;
    let mut max: i32 = 0;
    for c in state.chars() {
        match c {
            '(' | '[' | '{' | '⟨' => {
                depth += 1;
                if depth > max {
                    max = depth;
                }
            }
            ')' | ']' | '}' | '⟩' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            _ => {}
        }
    }
    max as usize
}

/// Estimate proof progress in `[0, 1]`: `1.0` for `no goals` (done), decreasing
/// as the state has more goals, more hypotheses, a larger conclusion, or deeper
/// structure. A stand-in for LeanProgress's learned steps-remaining prediction.
///
/// Monotonicity (holding the other features fixed): strictly decreasing in
/// `goal_count` and `term_size`, and non-increasing in `hypothesis_count` and
/// `subgoal_depth` — so "fewer goals / smaller term ⇒ higher value".
pub fn progress_value(features: &ProgressFeatures) -> f64 {
    if features.goal_count == 0 {
        return 1.0;
    }
    let goals = 1.0 / (1.0 + features.goal_count as f64);
    let term = 1.0 / (1.0 + features.term_size as f64 / TERM_SCALE);
    let hyps = 1.0 / (1.0 + features.hypothesis_count as f64 / HYP_SCALE);
    let depth = 1.0 / (1.0 + features.subgoal_depth as f64 / DEPTH_SCALE);
    let value = W_GOALS * goals + W_TERM * term + W_DEPTH * depth + W_HYPS * hyps;
    value.clamp(0.0, 1.0)
}

/// Convenience: parse a pretty-printed goal state and score it directly.
pub fn progress_value_from_state(state: &str) -> f64 {
    progress_value(&ProgressFeatures::parse(state))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_goals_is_done() {
        assert_eq!(progress_value_from_state("no goals"), 1.0);
        assert_eq!(progress_value_from_state("   "), 1.0);
        assert_eq!(ProgressFeatures::parse("no goals"), ProgressFeatures::default());
    }

    #[test]
    fn parses_a_goal_state() {
        let state = "case succ\nn : ℕ\nih : n + 0 = n\n⊢ n + 1 + 0 = n + 1";
        let f = ProgressFeatures::parse(state);
        assert_eq!(f.goal_count, 1);
        assert_eq!(f.hypothesis_count, 2, "n and ih are hypotheses; case is not");
        assert!(f.term_size > 0);
    }

    #[test]
    fn counts_multiple_goals_via_turnstiles() {
        let ascii = "n : Nat\n|- n = n\n\nm : Nat\n|- m = m";
        let f = ProgressFeatures::parse(ascii);
        assert_eq!(f.goal_count, 2);
    }

    #[test]
    fn fewer_goals_scores_higher() {
        let two = ProgressFeatures {
            goal_count: 2,
            ..ProgressFeatures::default()
        };
        let one = ProgressFeatures {
            goal_count: 1,
            ..ProgressFeatures::default()
        };
        assert!(progress_value(&one) > progress_value(&two));
        // and both are below the terminal "done" value.
        assert!(progress_value(&one) < 1.0);
    }

    #[test]
    fn smaller_term_scores_higher() {
        let big = ProgressFeatures {
            goal_count: 1,
            term_size: 80,
            ..ProgressFeatures::default()
        };
        let small = ProgressFeatures {
            goal_count: 1,
            term_size: 4,
            ..ProgressFeatures::default()
        };
        assert!(progress_value(&small) > progress_value(&big));
    }

    #[test]
    fn deeper_and_more_hyps_score_no_higher() {
        let base = ProgressFeatures {
            goal_count: 1,
            term_size: 10,
            hypothesis_count: 1,
            subgoal_depth: 1,
        };
        let deeper = ProgressFeatures {
            subgoal_depth: 6,
            ..base
        };
        let more_hyps = ProgressFeatures {
            hypothesis_count: 12,
            ..base
        };
        assert!(progress_value(&deeper) < progress_value(&base));
        assert!(progress_value(&more_hyps) < progress_value(&base));
    }

    #[test]
    fn stays_in_unit_interval() {
        let huge = ProgressFeatures {
            goal_count: 50,
            hypothesis_count: 200,
            term_size: 5000,
            subgoal_depth: 40,
        };
        let v = progress_value(&huge);
        assert!((0.0..=1.0).contains(&v));
    }

    #[test]
    fn nesting_depth_tracks_brackets() {
        let f = ProgressFeatures::parse("⊢ f (g (h (i x))) = ⟨a, b⟩");
        assert_eq!(f.subgoal_depth, 3);
    }
}
