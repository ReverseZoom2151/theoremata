//! Goal-state-at-error-position extraction from Lean's **infotree**.
//!
//! [`error_feedback::Diagnostic`] carries a typed but deliberately unpopulated
//! `goal_state_slot`: the proof state at the failure position is the single
//! highest-value enrichment we can add to failure feedback, because a model told
//! "unsolved goals" *plus the actual hypotheses and target* repairs far more
//! often than one told only the message. That slot was left empty because
//! obtaining the state "needs a live REPL".
//!
//! Half of that is true. Producing the infotree needs a live REPL; **parsing it
//! does not**, and this module is the parsing half. It is pure and
//! deterministic: no IO, no process spawning, no clock, no RNG. Feed it the JSON
//! a REPL already emitted and it fills the slot.
//!
//! ## The technique (ported from a mined system)
//!
//! Lean's infotree is a tree of elaboration nodes, each carrying the source
//! range of the syntax it elaborated plus, for tactic nodes, the goal state
//! `goalsBefore` that tactic ran on and the `goalsAfter` it left. To recover the
//! state at an error position you walk the tree, find the **smallest node whose
//! range contains the error span**, and read its goals. The feedback then leads
//! with the real hypothesis context at the failure point rather than only the
//! message text.
//!
//! Node selection follows the mined implementation exactly: among all containing
//! nodes, score each by summed distance from the position to **both** endpoints,
//! weighting a line difference [`LINE_WEIGHT`]× a column difference, and take the
//! minimum (see [`smallest_containing_node`]). Output is capped at
//! [`DEFAULT_GOAL_STATE_CAP`] diagnostics, because goal states are large and the
//! first few errors are the ones a model can act on.
//!
//! ## Producing the tree (out of scope here)
//!
//! The REPL must be asked for it explicitly. For the standard `lean-repl`
//! JSON protocol that means adding [`REPL_INFOTREE_FIELD`] with value
//! [`REPL_INFOTREE_VALUE`] to the command object, i.e.
//! `{"cmd": "...", "infotree": "original"}`. Without that field the response
//! carries no `infotree` key at all and every function here degrades to "no goal
//! state", never an error. Wiring that request is the caller's job.
//!
//! ## Coordinate conventions — read before touching anything here
//!
//! Lean's `Lean.Position` is **1-based in `line` and 0-based in `column`**, and
//! the infotree serializes `Lean.Position` verbatim, so infotree ranges inherit
//! that mixed base. [`Diagnostic`] stores **1-based** columns
//! (`error_feedback::parse_lean` already does the `+1` on Lean's error headers
//! for exactly this reason, and getting it wrong there once opened every
//! `<error>` span one character early). Positions are therefore normalized to
//! Diagnostic's convention at parse time, in [`Pos::from_raw`], so every
//! comparison below happens in one base. The bases are named constants
//! ([`INFOTREE_COLUMN_BASE`], [`DIAGNOSTIC_COLUMN_BASE`], [`INFOTREE_LINE_BASE`],
//! [`DIAGNOSTIC_LINE_BASE`]) rather than inline literals precisely so the
//! off-by-one cannot be reintroduced silently.
//!
//! ## Fail-soft
//!
//! Every entry point is total. Malformed JSON, an empty tree, missing ranges,
//! unknown fields, goal payloads in a shape we did not anticipate — all degrade
//! to "nothing attached" and leave the diagnostics byte-identical. This module
//! is advisory presentation only and never participates in a verdict.

use serde::{Deserialize, Serialize};

use crate::prover::error_feedback::Diagnostic;
use crate::prover::formal::FormalSystem;
use crate::prover::session::goal_state::{parse_lean_goal_state, GoalState};

// --- conventions ----------------------------------------------------------

/// Base of the `column` field in an infotree range. Lean's `Lean.Position` is
/// 0-based in columns and the infotree serializes it verbatim.
pub const INFOTREE_COLUMN_BASE: usize = 0;
/// Base of the `line` field in an infotree range. Lean's `Lean.Position` is
/// 1-based in lines.
pub const INFOTREE_LINE_BASE: usize = 1;
/// Base of [`Diagnostic::col_start`] / [`Diagnostic::col_end`] (1-based).
pub const DIAGNOSTIC_COLUMN_BASE: usize = 1;
/// Base of [`Diagnostic::line`] (1-based).
pub const DIAGNOSTIC_LINE_BASE: usize = 1;

/// Weight applied to a line difference when scoring candidate nodes: one line
/// of distance counts as much as ten columns. Ported verbatim from the mined
/// implementation's `10 * Δline + Δcolumn`.
pub const LINE_WEIGHT: usize = 10;

/// Default ceiling on how many diagnostics get a goal state attached. Goal
/// states are verbose; the mined system capped at three and so do we.
pub const DEFAULT_GOAL_STATE_CAP: usize = 3;

/// The REPL request field that makes Lean emit an infotree at all.
pub const REPL_INFOTREE_FIELD: &str = "infotree";
/// The value [`REPL_INFOTREE_FIELD`] must take: the *original* (source-ranged)
/// tree. Other modes elide the ranges this module navigates by.
pub const REPL_INFOTREE_VALUE: &str = "original";

/// Heading introducing the post-tactic state when a node reports one.
pub const AFTER_HEADING: &str = "-- after the failing step --";

// --- the serde model ------------------------------------------------------
//
// Permissive by construction: every field is optional, unknown fields are
// ignored (no `deny_unknown_fields` anywhere), and shape variation between REPL
// versions is absorbed by aliases rather than by a hard failure.

/// A raw `{"line": L, "column": C}` position, in infotree bases.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawPos {
    #[serde(default)]
    pub line: Option<usize>,
    #[serde(default, alias = "col")]
    pub column: Option<usize>,
}

/// A raw `{"start": …, "finish": …}` range. Some emitters spell the end
/// `"end"`, so both are accepted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawRange {
    #[serde(default)]
    pub start: Option<RawPos>,
    #[serde(default, alias = "end", alias = "stop")]
    pub finish: Option<RawPos>,
}

/// The `stx` (syntax) sub-object, whose `range` is where the range usually
/// lives in `infotree: "original"` output.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawSyntax {
    #[serde(default)]
    pub range: Option<RawRange>,
}

/// A goal payload. The REPL prints goals as strings, but tolerate a single
/// string and any richer shape (objects with a `goal`/`pp`/`type` field).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RawGoals {
    Many(Vec<String>),
    One(String),
    Other(serde_json::Value),
}

impl RawGoals {
    /// Flatten to plain pretty-printed goal strings, dropping anything we cannot
    /// interpret. Never panics.
    fn to_strings(&self) -> Vec<String> {
        match self {
            RawGoals::Many(v) => v.iter().filter(|s| !s.trim().is_empty()).cloned().collect(),
            RawGoals::One(s) if !s.trim().is_empty() => vec![s.clone()],
            RawGoals::One(_) => Vec::new(),
            RawGoals::Other(v) => value_goals(v),
        }
    }
}

/// Best-effort goal extraction from an unanticipated JSON shape.
fn value_goals(v: &serde_json::Value) -> Vec<String> {
    match v {
        serde_json::Value::String(s) if !s.trim().is_empty() => vec![s.clone()],
        serde_json::Value::Array(items) => items.iter().flat_map(value_goals).collect(),
        serde_json::Value::Object(map) => ["goal", "pp", "type", "goalState"]
            .iter()
            .find_map(|k| map.get(*k).and_then(|x| x.as_str()))
            .filter(|s| !s.trim().is_empty())
            .map(|s| vec![s.to_string()])
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// The payload of one infotree node: where it is, and what goals it saw.
///
/// Flattened into [`RawNode`] as well as nested under its `node` key, because
/// emitters differ on whether the payload is wrapped.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RawNodeBody {
    #[serde(default)]
    pub stx: Option<RawSyntax>,
    #[serde(default)]
    pub range: Option<RawRange>,
    #[serde(default, rename = "goalsBefore", alias = "goals_before")]
    pub goals_before: Option<RawGoals>,
    #[serde(default, rename = "goalsAfter", alias = "goals_after")]
    pub goals_after: Option<RawGoals>,
}

impl RawNodeBody {
    fn range(&self) -> Option<&RawRange> {
        self.range
            .as_ref()
            .or_else(|| self.stx.as_ref().and_then(|s| s.range.as_ref()))
    }

    fn is_empty(&self) -> bool {
        self.stx.is_none()
            && self.range.is_none()
            && self.goals_before.is_none()
            && self.goals_after.is_none()
    }
}

/// One raw infotree entry: an optional wrapped body, an inline body, and kids.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RawNode {
    /// The `{"node": {…}}` wrapper used by `infotree: "original"`.
    #[serde(default)]
    pub node: Option<RawNodeBody>,
    /// The same fields when they appear directly on the entry instead.
    #[serde(flatten)]
    pub inline: RawNodeBody,
    #[serde(default)]
    pub kids: Vec<RawNode>,
    #[serde(default)]
    pub children: Vec<RawNode>,
}

// --- the normalized model -------------------------------------------------

/// A source position in **[`Diagnostic`]'s convention**: 1-based line, 1-based
/// column. Nothing downstream of [`Pos::from_raw`] ever sees an infotree base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

impl Pos {
    /// Convert a raw infotree position into Diagnostic's convention, rebasing
    /// each coordinate by the difference between the two documented bases. A
    /// position missing either coordinate is not usable and yields `None`.
    fn from_raw(raw: &RawPos) -> Option<Self> {
        let line = raw.line?;
        let col = raw.column?;
        Some(Pos {
            // Both are 1-based; the shift is zero, but it is written as a shift
            // so that a future emitter change is a one-constant edit.
            line: line + (DIAGNOSTIC_LINE_BASE - INFOTREE_LINE_BASE),
            // Infotree columns are 0-based, Diagnostic's are 1-based: +1.
            col: col + (DIAGNOSTIC_COLUMN_BASE - INFOTREE_COLUMN_BASE),
        })
    }

    /// Weighted distance to `other`, per the mined scoring rule.
    fn distance(self, other: Pos) -> usize {
        LINE_WEIGHT * self.line.abs_diff(other.line) + self.col.abs_diff(other.col)
    }
}

/// A half-open-in-spirit but **inclusive-in-practice** source span. Inclusive at
/// both ends: Lean error positions frequently land exactly on a node boundary
/// (the first character after a tactic, say), and excluding the endpoint there
/// would drop the very node that holds the state we want.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: Pos,
    pub end: Pos,
}

impl Span {
    fn contains(&self, at: Pos) -> bool {
        (self.start.line, self.start.col) <= (at.line, at.col)
            && (at.line, at.col) <= (self.end.line, self.end.col)
    }

    /// `10*Δline + Δcolumn` summed over **both** endpoints — the mined score.
    /// Smaller is tighter, so the minimum is the innermost enclosing node.
    fn score(&self, at: Pos) -> usize {
        self.start.distance(at) + self.end.distance(at)
    }
}

/// A normalized infotree node. `span` is `None` for a node whose range was
/// missing or unparseable; such a node is skipped as a candidate but its
/// children are still visited, so one bad node never hides a good subtree.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub span: Option<Span>,
    pub goals_before: Vec<String>,
    pub goals_after: Vec<String>,
    pub kids: Vec<Node>,
}

impl Node {
    /// Whether this node carries any goal state at all.
    pub fn has_goals(&self) -> bool {
        !self.goals_before.is_empty() || !self.goals_after.is_empty()
    }
}

/// A parsed infotree: a forest, because the REPL returns one tree per command.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InfoTree {
    pub roots: Vec<Node>,
}

impl InfoTree {
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }
}

// --- parsing --------------------------------------------------------------

/// Parse infotree JSON into an [`InfoTree`].
///
/// Accepts the three shapes seen in practice: a bare array of trees, a single
/// tree object, or a REPL response object with an `infotree` key. Returns `None`
/// only when the input is not JSON at all or contains no recognizable node;
/// unknown fields, missing ranges and absent goals are all tolerated.
///
/// Total: `serde_json` enforces its own recursion limit, so even a pathological
/// nesting depth returns `None` rather than overflowing the stack.
pub fn parse_infotree(json: &str) -> Option<InfoTree> {
    if json.trim().is_empty() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let raws = extract_raw(&value)?;
    let roots: Vec<Node> = raws.iter().filter_map(normalize).collect();
    if roots.is_empty() {
        return None;
    }
    Some(InfoTree { roots })
}

/// Locate the node array inside whatever wrapper the caller handed us.
fn extract_raw(value: &serde_json::Value) -> Option<Vec<RawNode>> {
    if let Some(obj) = value.as_object() {
        for key in [REPL_INFOTREE_FIELD, "infoTree", "infotrees", "trees"] {
            if let Some(inner) = obj.get(key) {
                if let Some(found) = extract_raw(inner) {
                    return Some(found);
                }
            }
        }
    }
    if value.is_array() {
        return serde_json::from_value::<Vec<RawNode>>(value.clone()).ok();
    }
    serde_json::from_value::<RawNode>(value.clone())
        .ok()
        .map(|n| vec![n])
}

/// Fold a [`RawNode`] into a [`Node`]. Returns `None` for an entry that carries
/// neither payload nor children (pure noise).
fn normalize(raw: &RawNode) -> Option<Node> {
    // Prefer the wrapped body; fall back to the flattened one.
    let wrapped = raw.node.as_ref().filter(|b| !b.is_empty());
    let body = wrapped.unwrap_or(&raw.inline);

    let span = body.range().and_then(|r| {
        let start = Pos::from_raw(r.start.as_ref()?)?;
        let end = Pos::from_raw(r.finish.as_ref()?)?;
        // A reversed range is corrupt; skip it rather than "containing" nothing
        // or, worse, everything.
        if (end.line, end.col) < (start.line, start.col) {
            None
        } else {
            Some(Span { start, end })
        }
    });

    let kids: Vec<Node> = raw
        .kids
        .iter()
        .chain(raw.children.iter())
        .filter_map(normalize)
        .collect();

    let goals_before = body
        .goals_before
        .as_ref()
        .map(RawGoals::to_strings)
        .unwrap_or_default();
    let goals_after = body
        .goals_after
        .as_ref()
        .map(RawGoals::to_strings)
        .unwrap_or_default();

    if span.is_none() && goals_before.is_empty() && goals_after.is_empty() && kids.is_empty() {
        return None;
    }
    Some(Node {
        span,
        goals_before,
        goals_after,
        kids,
    })
}

// --- node selection -------------------------------------------------------

/// Find the smallest node whose range contains `(line, col)`.
///
/// `line` and `col` are in [`Diagnostic`]'s 1-based convention — pass
/// `d.line` / `d.col_start` straight through.
///
/// "Smallest" is decided by the mined distance score: among all containing
/// nodes, the winner minimizes `10*Δline + Δcolumn` summed over both endpoints,
/// which is exactly the tightest enclosing range. Ties break first toward the
/// **deeper** node (an inner elaboration is more specific than the outer syntax
/// that shares its range) and then toward the **earlier** node in document
/// order, so the result is fully deterministic for any input.
pub fn smallest_containing_node(tree: &InfoTree, line: usize, col: usize) -> Option<&Node> {
    select(tree, Pos { line, col }, false)
}

/// As [`smallest_containing_node`], but only nodes that actually carry goals are
/// eligible. This is what [`attach_goal_states`] uses: the tightest node is
/// often a bare syntax node with no state, and the useful answer is the tightest
/// *goal-bearing* enclosing node rather than nothing at all.
pub fn smallest_containing_node_with_goals(
    tree: &InfoTree,
    line: usize,
    col: usize,
) -> Option<&Node> {
    select(tree, Pos { line, col }, true)
}

fn select(tree: &InfoTree, at: Pos, require_goals: bool) -> Option<&Node> {
    // (score, depth, order) of the incumbent; lower score wins, then greater
    // depth, then lower order.
    let mut best: Option<(&Node, usize, usize, usize)> = None;
    let mut order = 0usize;

    // Explicit stack rather than recursion: the tree is attacker-influenced
    // input and must not be able to smash our stack.
    let mut stack: Vec<(&Node, usize)> = tree.roots.iter().rev().map(|n| (n, 0usize)).collect();
    while let Some((node, depth)) = stack.pop() {
        let this_order = order;
        order += 1;
        for kid in node.kids.iter().rev() {
            stack.push((kid, depth + 1));
        }
        let Some(span) = node.span else { continue };
        if !span.contains(at) {
            continue;
        }
        if require_goals && !node.has_goals() {
            continue;
        }
        let score = span.score(at);
        let better = match best {
            None => true,
            Some((_, b_score, b_depth, b_order)) => {
                (score, std::cmp::Reverse(depth), this_order)
                    < (b_score, std::cmp::Reverse(b_depth), b_order)
            }
        };
        if better {
            best = Some((node, score, depth, this_order));
        }
    }
    best.map(|(n, _, _, _)| n)
}

// --- rendering + attachment ----------------------------------------------

/// Render a node's goals into the human-readable block that goes into
/// [`Diagnostic::goal_state_slot`].
///
/// Goal text is round-tripped through
/// [`crate::prover::session::goal_state::parse_lean_goal_state`] and
/// [`GoalState::render`] so the state is normalized exactly like every other
/// goal state in the system — this module deliberately does **not** define a
/// second goal representation. When the text does not parse as a goal state
/// (no turnstile, say) the original text is passed through verbatim rather than
/// dropped, because the checker's own words are never worth losing.
///
/// Returns `None` when the node carries no goals at all.
pub fn render_node_goals(node: &Node) -> Option<String> {
    let before = render_goals(&node.goals_before);
    let after = render_goals(&node.goals_after);
    match (before, after) {
        (None, None) => None,
        (Some(b), None) => Some(b),
        (None, Some(a)) => Some(format!("{AFTER_HEADING}\n{a}")),
        (Some(b), Some(a)) => Some(format!("{b}\n{AFTER_HEADING}\n{a}")),
    }
}

fn render_goals(goals: &[String]) -> Option<String> {
    if goals.is_empty() {
        return None;
    }
    let joined = goals.join("\n\n");
    let parsed: GoalState = parse_lean_goal_state(&joined);
    let rendered = if parsed.is_empty() {
        joined.trim_end().to_string()
    } else {
        parsed.render().trim_end().to_string()
    };
    if rendered.is_empty() {
        None
    } else {
        Some(rendered)
    }
}

/// Fill [`Diagnostic::goal_state_slot`] for up to `cap` diagnostics from
/// `tree_json`, returning how many were filled.
///
/// Diagnostics are considered in order. A diagnostic is a candidate when it came
/// from Lean (the infotree is Lean's), reports a line and a start column, and
/// has an empty slot; a candidate whose position lands inside a goal-bearing
/// node gets that node's state. Once `cap` slots are filled the rest are left
/// **byte-identical** — as is every diagnostic when `tree_json` is empty,
/// malformed, or contains no usable node.
///
/// Pass [`DEFAULT_GOAL_STATE_CAP`] for the ported system's behaviour.
pub fn attach_goal_states(diagnostics: &mut [Diagnostic], tree_json: &str, cap: usize) -> usize {
    if cap == 0 {
        return 0;
    }
    let Some(tree) = parse_infotree(tree_json) else {
        return 0;
    };
    let mut filled = 0usize;
    for d in diagnostics.iter_mut() {
        if filled >= cap {
            break;
        }
        if d.system != FormalSystem::Lean || d.goal_state_slot.is_some() {
            continue;
        }
        let (Some(line), Some(col)) = (d.line, d.col_start) else {
            continue;
        };
        let Some(node) = smallest_containing_node_with_goals(&tree, line, col) else {
            continue;
        };
        let Some(rendered) = render_node_goals(node) else {
            continue;
        };
        d.goal_state_slot = Some(rendered);
        filled += 1;
    }
    filled
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prover::error_feedback::{parse_diagnostics, Severity};

    /// A three-level tree over lines 1..=10. Ranges are in INFOTREE bases
    /// (0-based columns), exactly as Lean would emit them.
    const NESTED: &str = r#"[
      {"node": {"stx": {"range": {"start": {"line": 1, "column": 0},
                                  "finish": {"line": 10, "column": 0}}},
                "goalsBefore": ["⊢ outer"]},
       "kids": [
         {"node": {"stx": {"range": {"start": {"line": 4, "column": 0},
                                     "finish": {"line": 6, "column": 20}}},
                   "goalsBefore": ["n : Nat\n⊢ middle"]},
          "kids": [
            {"node": {"stx": {"range": {"start": {"line": 5, "column": 2},
                                        "finish": {"line": 5, "column": 8}}},
                      "goalsBefore": ["n : Nat\nh : n > 0\n⊢ inner"],
                      "goalsAfter": ["⊢ done"]},
             "kids": []}
          ]}
       ]}
    ]"#;

    fn lean_diag(line: usize, col: usize, msg: &str) -> Diagnostic {
        Diagnostic {
            system: FormalSystem::Lean,
            severity: Severity::Error,
            line: Some(line),
            end_line: None,
            col_start: Some(col),
            col_end: None,
            message: msg.to_string(),
            goal_state_slot: None,
        }
    }

    #[test]
    fn columns_are_rebased_from_zero_to_one() {
        // Lean emits column 2; Diagnostic's convention is 3.
        let tree = parse_infotree(NESTED).expect("fixture must parse");
        let innermost = &tree.roots[0].kids[0].kids[0];
        let span = innermost.span.expect("range present");
        assert_eq!(span.start, Pos { line: 5, col: 3 });
        assert_eq!(span.end, Pos { line: 5, col: 9 });
        // Lines pass through unchanged - both conventions are 1-based.
        assert_eq!(INFOTREE_LINE_BASE, DIAGNOSTIC_LINE_BASE);
        assert_eq!(DIAGNOSTIC_COLUMN_BASE - INFOTREE_COLUMN_BASE, 1);
    }

    #[test]
    fn innermost_containing_node_wins() {
        let tree = parse_infotree(NESTED).unwrap();
        // Line 5, column 4 (1-based) is inside all three nested ranges.
        let n = smallest_containing_node(&tree, 5, 4).expect("a node must contain 5:4");
        assert_eq!(
            n.goals_before,
            vec!["n : Nat\nh : n > 0\n⊢ inner".to_string()]
        );
        assert!(n.kids.is_empty(), "the innermost node is the leaf");

        // Line 4 is outside the leaf but inside the middle node.
        let n = smallest_containing_node(&tree, 4, 1).unwrap();
        assert_eq!(n.goals_before, vec!["n : Nat\n⊢ middle".to_string()]);

        // Line 9 is only inside the root.
        let n = smallest_containing_node(&tree, 9, 1).unwrap();
        assert_eq!(n.goals_before, vec!["⊢ outer".to_string()]);
    }

    #[test]
    fn ties_break_by_the_documented_distance_score() {
        // Two roots both contain 5:3 (1-based). The first is enormous and two
        // levels deep; the second is a shallow, exact-fit range. The distance
        // score must beat depth, so the tight shallow node wins.
        let json = r#"[
          {"node": {"range": {"start": {"line": 1, "column": 0},
                              "finish": {"line": 100, "column": 0}},
                    "goalsBefore": ["⊢ wide-outer"]},
           "kids": [{"node": {"range": {"start": {"line": 1, "column": 0},
                                        "finish": {"line": 100, "column": 0}},
                              "goalsBefore": ["⊢ wide-inner"]}, "kids": []}]},
          {"node": {"range": {"start": {"line": 5, "column": 1},
                              "finish": {"line": 5, "column": 3}},
                    "goalsBefore": ["⊢ tight"]}, "kids": []}
        ]"#;
        let tree = parse_infotree(json).unwrap();
        let n = smallest_containing_node(&tree, 5, 3).unwrap();
        assert_eq!(n.goals_before, vec!["⊢ tight".to_string()]);

        // Deeper wins only when the scores are equal: the two wide nodes have
        // identical ranges, so at a position the tight node does not cover the
        // deeper of the pair is chosen.
        let n = smallest_containing_node(&tree, 50, 1).unwrap();
        assert_eq!(n.goals_before, vec!["⊢ wide-inner".to_string()]);

        // And the score itself is the documented formula.
        let span = Span {
            start: Pos { line: 4, col: 1 },
            end: Pos { line: 6, col: 1 },
        };
        let at = Pos { line: 5, col: 3 };
        assert_eq!(span.score(at), (LINE_WEIGHT + 2) + (LINE_WEIGHT + 2));
    }

    #[test]
    fn a_position_outside_every_range_yields_none() {
        let tree = parse_infotree(NESTED).unwrap();
        assert!(smallest_containing_node(&tree, 999, 1).is_none());
        assert!(
            smallest_containing_node(&tree, 10, 5).is_none(),
            "past the end column"
        );
        // Line 0 cannot exist in a 1-based convention and must not match.
        assert!(smallest_containing_node(&tree, 0, 1).is_none());
    }

    #[test]
    fn nodes_without_ranges_are_skipped_but_their_kids_are_not() {
        let json = r#"[
          {"node": {"goalsBefore": ["⊢ rangeless"]},
           "kids": [{"node": {"range": {"start": {"line": 2, "column": 0},
                                        "finish": {"line": 2, "column": 5}},
                              "goalsBefore": ["⊢ kid"]}, "kids": []}]}
        ]"#;
        let tree = parse_infotree(json).unwrap();
        assert!(tree.roots[0].span.is_none(), "missing range => no span");
        let n = smallest_containing_node(&tree, 2, 2).expect("the kid is still reachable");
        assert_eq!(n.goals_before, vec!["⊢ kid".to_string()]);
    }

    #[test]
    fn unknown_fields_and_alternate_shapes_are_tolerated() {
        // Unwrapped body, `end` instead of `finish`, plus junk fields.
        let json = r#"{"infotree": [
          {"range": {"start": {"line": 3, "column": 4}, "end": {"line": 3, "column": 9}},
           "goalsBefore": "h : p\n⊢ q",
           "elaborator": "Lean.Elab.Tactic.evalExact",
           "mystery": {"deeply": ["nested", 1, null]},
           "children": []}
        ]}"#;
        let tree = parse_infotree(json).expect("wrapper + aliases must parse");
        let n = smallest_containing_node(&tree, 3, 6).expect("3:6 is inside 3:5-3:10");
        assert_eq!(n.goals_before, vec!["h : p\n⊢ q".to_string()]);
    }

    #[test]
    fn malformed_and_empty_json_never_panics_and_changes_nothing() {
        let mut diags = vec![lean_diag(5, 4, "unsolved goals")];
        let before = diags.clone();
        for bad in [
            "",
            "   ",
            "not json at all",
            "{",
            "[{\"node\":}]",
            "null",
            "[]",
            "[{}]",
            "{\"infotree\": []}",
            "\u{1f4a5}\u{ff}",
        ] {
            assert_eq!(attach_goal_states(&mut diags, bad, 3), 0, "input: {bad:?}");
            assert_eq!(diags, before, "diagnostics must be untouched for {bad:?}");
        }
        assert!(parse_infotree("[]").is_none());
        assert!(parse_infotree("garbage").is_none());
    }

    #[test]
    fn goals_before_without_goals_after_still_populates() {
        let json = r#"[{"node": {"range": {"start": {"line": 2, "column": 0},
                                            "finish": {"line": 2, "column": 10}},
                                  "goalsBefore": ["n : Nat\n⊢ n + 0 = n"]},
                        "kids": []}]"#;
        let mut diags = vec![lean_diag(2, 3, "unsolved goals")];
        assert_eq!(
            attach_goal_states(&mut diags, json, DEFAULT_GOAL_STATE_CAP),
            1
        );
        let slot = diags[0].goal_state_slot.as_deref().expect("slot filled");
        assert!(slot.contains("n : Nat"), "{slot}");
        assert!(slot.contains("⊢ n + 0 = n"), "{slot}");
        assert!(
            !slot.contains(AFTER_HEADING),
            "no after-state to show: {slot}"
        );

        // The mirror case: goalsAfter alone also populates, under its heading.
        let json_after = json.replace("goalsBefore", "goalsAfter");
        let mut diags2 = vec![lean_diag(2, 3, "unsolved goals")];
        assert_eq!(attach_goal_states(&mut diags2, &json_after, 3), 1);
        let slot2 = diags2[0].goal_state_slot.as_deref().unwrap();
        assert!(slot2.starts_with(AFTER_HEADING), "{slot2}");
    }

    #[test]
    fn the_cap_is_respected_and_the_first_cap_diagnostics_are_filled() {
        // One wide node covering everything, so every diagnostic is fillable.
        let json = r#"[{"node": {"range": {"start": {"line": 1, "column": 0},
                                            "finish": {"line": 50, "column": 0}},
                                  "goalsBefore": ["⊢ everywhere"]}, "kids": []}]"#;
        let mut diags: Vec<Diagnostic> = (1..=6).map(|i| lean_diag(i, 1, "boom")).collect();
        let filled = attach_goal_states(&mut diags, json, DEFAULT_GOAL_STATE_CAP);
        assert_eq!(filled, 3);
        assert_eq!(DEFAULT_GOAL_STATE_CAP, 3);
        for d in &diags[..3] {
            assert!(d.goal_state_slot.is_some(), "the first 3 are filled");
        }
        for d in &diags[3..] {
            assert!(d.goal_state_slot.is_none(), "the rest are untouched");
        }
        // cap 0 fills nothing at all.
        let mut none = vec![lean_diag(1, 1, "boom")];
        assert_eq!(attach_goal_states(&mut none, json, 0), 0);
        assert!(none[0].goal_state_slot.is_none());
    }

    #[test]
    fn only_lean_positioned_unfilled_diagnostics_are_candidates() {
        let json = r#"[{"node": {"range": {"start": {"line": 1, "column": 0},
                                            "finish": {"line": 50, "column": 0}},
                                  "goalsBefore": ["⊢ everywhere"]}, "kids": []}]"#;
        let mut rocq = lean_diag(1, 1, "boom");
        rocq.system = FormalSystem::Rocq;
        let mut unpositioned = lean_diag(1, 1, "boom");
        unpositioned.line = None;
        let mut prefilled = lean_diag(1, 1, "boom");
        prefilled.goal_state_slot = Some("preserve me".into());

        let mut diags = vec![rocq, unpositioned, prefilled, lean_diag(2, 1, "boom")];
        assert_eq!(attach_goal_states(&mut diags, json, 3), 1);
        assert!(diags[0].goal_state_slot.is_none(), "not Lean");
        assert!(diags[1].goal_state_slot.is_none(), "no position");
        assert_eq!(diags[2].goal_state_slot.as_deref(), Some("preserve me"));
        assert!(diags[3].goal_state_slot.is_some());
    }

    #[test]
    fn a_rangeless_or_goalless_tightest_node_falls_back_to_the_nearest_state() {
        // The tightest containing node has NO goals; the useful answer is the
        // tightest goal-bearing enclosing node, not nothing.
        let json = r#"[{"node": {"range": {"start": {"line": 1, "column": 0},
                                            "finish": {"line": 9, "column": 0}},
                                  "goalsBefore": ["⊢ outer"]},
                        "kids": [{"node": {"range": {"start": {"line": 5, "column": 0},
                                                     "finish": {"line": 5, "column": 4}}},
                                  "kids": []}]}]"#;
        let tree = parse_infotree(json).unwrap();
        let tightest = smallest_containing_node(&tree, 5, 2).unwrap();
        assert!(!tightest.has_goals(), "the leaf is a bare syntax node");
        let with_goals = smallest_containing_node_with_goals(&tree, 5, 2).unwrap();
        assert_eq!(with_goals.goals_before, vec!["⊢ outer".to_string()]);
    }

    #[test]
    fn attachment_is_deterministic_and_feeds_the_feedback_renderer() {
        // End to end over the real Lean error parser: parse a Lean error header
        // (0-based column 2 => Diagnostic column 3), attach, and confirm the
        // rendered slot carries the hypothesis context.
        let raw = "Generated.lean:5:2: error: unsolved goals";
        let mut a = parse_diagnostics(FormalSystem::Lean, raw);
        assert_eq!(a[0].col_start, Some(3), "the parser rebases columns");
        let mut b = a.clone();
        assert_eq!(attach_goal_states(&mut a, NESTED, 3), 1);
        assert_eq!(attach_goal_states(&mut b, NESTED, 3), 1);
        assert_eq!(a, b, "attachment must be byte-deterministic");
        let slot = a[0].goal_state_slot.as_deref().unwrap();
        assert!(slot.contains("h : n > 0"), "{slot}");
        assert!(slot.contains("⊢ inner"), "{slot}");
        assert!(slot.contains(AFTER_HEADING), "{slot}");
        assert!(slot.contains("⊢ done"), "{slot}");
    }

    #[test]
    fn unparseable_goal_text_is_passed_through_rather_than_dropped() {
        // No turnstile anywhere => GoalState parses empty => keep Lean's words.
        let json = r#"[{"node": {"range": {"start": {"line": 1, "column": 0},
                                            "finish": {"line": 1, "column": 9}},
                                  "goalsBefore": ["some unstructured prover chatter"]},
                        "kids": []}]"#;
        let mut diags = vec![lean_diag(1, 2, "boom")];
        assert_eq!(attach_goal_states(&mut diags, json, 3), 1);
        assert_eq!(
            diags[0].goal_state_slot.as_deref(),
            Some("some unstructured prover chatter")
        );
    }

    #[test]
    fn reversed_and_partial_ranges_are_discarded() {
        let json = r#"[
          {"node": {"range": {"start": {"line": 9, "column": 0},
                              "finish": {"line": 2, "column": 0}},
                    "goalsBefore": ["⊢ reversed"]}, "kids": []},
          {"node": {"range": {"start": {"line": 3}},
                    "goalsBefore": ["⊢ partial"]}, "kids": []}
        ]"#;
        let tree = parse_infotree(json).expect("nodes still exist, just spanless");
        assert!(tree.roots.iter().all(|n| n.span.is_none()));
        assert!(smallest_containing_node(&tree, 3, 1).is_none());
    }
}
