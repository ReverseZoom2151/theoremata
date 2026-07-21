//! Admission control for proof decomposition (plan Tier 3, items 16-17).
//!
//! Decomposition is the single most dangerous move in a sketch-then-fill
//! architecture, because it lets a *model* redraw the boundary of what must be
//! proved. Two failure modes follow directly from that:
//!
//! * **Authority leak.** The decomposer proposes children *and* asserts they
//!   hold. The parent then "follows", and a proof has been manufactured out of
//!   an assertion. See [`Violation::AssertedChild`] — this is the rule the whole
//!   module exists for.
//! * **Degeneration.** The decomposer restates the goal as its own lemma
//!   ("Lemma: the theorem"), or splits into pieces no simpler than the parent,
//!   and the search burns budget descending a tree that never gets easier.
//!
//! This module is the gate every proposed decomposition must pass *before* any
//! child node is created. It is a pure, deterministic function of the proposal:
//! no IO, no wall-clock, no randomness, std only. It is **fail-closed** — any
//! violation at all means `admitted == false`.
//!
//! ## The seven checks
//!
//! 1. `no_self_child` — no child's canonical hash equals the parent's or a
//!    sibling's ([`Violation::SelfChild`], [`Violation::DuplicateSiblings`]).
//! 2. `acyclic` — the resulting dependency graph stays acyclic
//!    ([`Violation::Cycle`]).
//! 3. `bounded` — child count in `[min_children, max_children]`, resulting depth
//!    `<= max_depth` ([`Violation::ChildCount`], [`Violation::DepthExceeded`]).
//! 4. `complexity_reduction` — at least one child cuts the structural
//!    complexity proxy by `>= min_reduction`, **or** every child isolates a
//!    distinct case/witness/lemma ([`Violation::NoComplexityReduction`]).
//! 5. `parent_composes` — the parent stays *active* as a composition
//!    obligation, and its eventual proof is required to reference every child
//!    ([`Violation::ParentNotComposing`], recorded as [`CompositionObligation`]).
//! 6. `no_asserted_children` — **no child may enter as proved on the
//!    decomposition model's say-so** ([`Violation::AssertedChild`]).
//! 7. `earned_decomposition` — a leaf may only be decomposed after a bounded
//!    discharge probe failed in a *qualifying* way ([`Violation::Unearned`]).
//!
//! ## Canonical hashing
//!
//! This module does **not** invent a second hashing scheme. It reuses
//! `CanonicalGoal::key` from
//! `components/reason/search/subsumption.rs` — the α-renaming- and
//! hypothesis-order-invariant canonical key already used by the goal cache, the
//! inverse method and subsumption dedup. So "restate the goal with the
//! hypotheses swapped and one bound variable renamed" is still caught as a
//! self-child.
//!
//! (`Store` node `content_hash` — `sha256(kind|title|statement)` in
//! `components/graph/db.rs` — is *not* used here: it is title-sensitive and
//! surface-syntactic, so renaming the lemma would defeat the self-child check.)

use super::super::search::subsumption::CanonicalGoal;
use std::collections::{BTreeMap, BTreeSet};

// ===========================================================================
// Configuration
// ===========================================================================

/// Tunable admission bounds. [`Default`] encodes the plan's values.
#[derive(Debug, Clone, PartialEq)]
pub struct AdmissionConfig {
    /// Minimum number of children. A one-child "decomposition" is a rename.
    pub min_children: usize,
    /// Maximum number of children; beyond this the split is a data dump.
    pub max_children: usize,
    /// Maximum depth of a node in the decomposition tree (root is depth 0).
    pub max_depth: usize,
    /// Fraction by which some child must undercut the parent's complexity
    /// proxy, e.g. `0.20` = the child must be at most 80% as complex.
    pub min_reduction: f64,
}

impl Default for AdmissionConfig {
    fn default() -> Self {
        Self {
            min_children: 2,
            max_children: 6,
            max_depth: 6,
            min_reduction: 0.20,
        }
    }
}

// ===========================================================================
// The proposal
// ===========================================================================

/// How a child was *claimed* to have been discharged. The decomposer is allowed
/// to say "I believe this is easy"; it is **never** allowed to say "this is
/// proved". Only [`ChildStatus::Unproved`] is admissible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChildStatus {
    /// The only admissible state: an open obligation for the prover to close.
    #[default]
    Unproved,
    /// The decomposition model asserted this child holds. Rejected — asserting
    /// a child proved is exactly the authority leak this module blocks.
    AssertedProved,
    /// The model cited an external result without a checked reference. Also an
    /// assertion, just laundered through a citation; rejected the same way.
    AssertedByCitation,
}

impl ChildStatus {
    /// True when the status amounts to the model claiming the child is done.
    pub fn is_assertion(self) -> bool {
        !matches!(self, ChildStatus::Unproved)
    }
}

/// One proposed child obligation.
///
/// Mirrors [`super::decompose::Obligation`] (title + statement) and adds the
/// fields admission needs. Build one from an `Obligation` with
/// [`ChildProposal::from_obligation`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildProposal {
    /// Stable id used for graph/cycle reasoning (a node id, or the title).
    pub id: String,
    /// Human title.
    pub title: String,
    /// The child's statement, in `hyps ⊢ concl` surface form when available.
    pub statement: String,
    /// How the model claims this child stands. Must be [`ChildStatus::Unproved`].
    pub status: ChildStatus,
    /// The distinct case / witness / lemma this child isolates, if any. Used by
    /// the *second* arm of the complexity check: a case split into equally
    /// complex but genuinely disjoint cases is legitimate even without a
    /// complexity drop, provided every child isolates something *distinct*.
    pub isolates: Option<String>,
    /// Ids this child itself depends on (siblings, or pre-existing nodes).
    pub depends_on: Vec<String>,
}

impl ChildProposal {
    /// A well-formed, unproved child with no declared dependencies.
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        statement: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            statement: statement.into(),
            status: ChildStatus::Unproved,
            isolates: None,
            depends_on: Vec::new(),
        }
    }

    /// Lift a decomposer [`Obligation`](super::decompose::Obligation) into a
    /// child proposal. The obligation carries no proof, so the status is
    /// [`ChildStatus::Unproved`] by construction — a decomposer literally has
    /// no channel through which to assert a child proved.
    pub fn from_obligation(id: impl Into<String>, ob: &super::decompose::Obligation) -> Self {
        Self::new(id, ob.title.clone(), ob.statement.clone())
    }

    /// Mark what distinct case/witness/lemma this child isolates.
    pub fn isolating(mut self, what: impl Into<String>) -> Self {
        self.isolates = Some(what.into());
        self
    }

    /// Declare dependencies on other node ids.
    pub fn depending_on<I, S>(mut self, ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.depends_on = ids.into_iter().map(Into::into).collect();
        self
    }

    /// The canonical key of this child's statement.
    pub fn canonical_key(&self) -> String {
        CanonicalGoal::parse(&self.statement).key()
    }
}

/// The parent being decomposed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParentNode {
    pub id: String,
    pub statement: String,
    /// Depth of the parent in the decomposition tree; the root is `0`.
    pub depth: usize,
    /// Whether the parent remains an *active* composition obligation after the
    /// split. If a proposal retires/supersedes the parent, the composition step
    /// is never proved and the children's conjunction is silently promoted to
    /// the theorem. Rejected.
    pub stays_active: bool,
}

impl ParentNode {
    /// A parent at `depth` that correctly stays active as the composition step.
    pub fn new(id: impl Into<String>, statement: impl Into<String>, depth: usize) -> Self {
        Self {
            id: id.into(),
            statement: statement.into(),
            depth,
            stays_active: true,
        }
    }

    /// The canonical key of the parent's statement.
    pub fn canonical_key(&self) -> String {
        CanonicalGoal::parse(&self.statement).key()
    }
}

/// The full proposal handed to [`admit`].
#[derive(Debug, Clone, PartialEq)]
pub struct DecompositionProposal {
    pub parent: ParentNode,
    pub children: Vec<ChildProposal>,
    /// Pre-existing dependency edges `(dependent, dependency)` in the graph the
    /// children will be spliced into. Needed to catch cycles created only in
    /// combination with edges that already exist.
    pub existing_edges: Vec<(String, String)>,
    /// The bounded discharge probe that (must have) failed before this leaf was
    /// allowed to be decomposed.
    pub probe: DischargeProbe,
}

impl DecompositionProposal {
    /// A proposal with no pre-existing edges.
    pub fn new(parent: ParentNode, children: Vec<ChildProposal>, probe: DischargeProbe) -> Self {
        Self {
            parent,
            children,
            existing_edges: Vec::new(),
            probe,
        }
    }

    /// Attach pre-existing `(dependent, dependency)` edges.
    pub fn with_existing_edges<I>(mut self, edges: I) -> Self
    where
        I: IntoIterator<Item = (String, String)>,
    {
        self.existing_edges = edges.into_iter().collect();
        self
    }
}

// ===========================================================================
// earned_decomposition — the leaf probe
// ===========================================================================

/// Evidence from the bounded discharge probe run on a leaf *before* anyone is
/// permitted to decompose it.
///
/// The rule: **you do not get to change the mathematics because the syntax was
/// wrong.** A failure made only of syntax / elaboration errors routes to
/// REPAIR ([`ProbeVerdict::Repair`]); it never on its own earns a decomposition.
/// Decomposition is earned only by a failure that is genuinely *structural*.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DischargeProbe {
    /// Whether a bounded discharge probe was actually run at all.
    pub ran: bool,
    /// Number of independent unsolved goals the probe left open.
    pub independent_unsolved_goals: usize,
    /// How many *semantic* (non-syntax-error) attempts were made.
    pub semantic_attempts: usize,
    /// Whether the same canonical goal hash survived those semantic attempts.
    pub same_goal_hash_survived: bool,
    /// Tokens of context the goal mandatorily requires.
    pub mandatory_context_tokens: usize,
    /// The context budget available to the prover.
    pub context_budget_tokens: usize,
    /// Number of probe attempts that ended in a timeout.
    pub timeouts: usize,
    /// Number of probe attempts that ended in a syntax/elaboration error.
    /// These are *excluded* from justifying a decomposition.
    pub syntax_errors: usize,
}

/// Why (or why not) a decomposition was earned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeVerdict {
    /// No probe was run — decomposition is unearned by construction.
    NoProbe,
    /// The failure is syntax/elaboration only. Route to REPAIR. A broken
    /// incantation is not evidence that the mathematics needs restructuring.
    Repair,
    /// The probe failed, but in no qualifying way.
    NotQualifying,
    /// Earned: more than two independent unsolved goals remain.
    EarnedIndependentGoals(usize),
    /// Earned: the same canonical goal hash survived >= 2 semantic attempts.
    EarnedGoalHashSurvived,
    /// Earned: mandatory context exceeds the budget `(required, budget)`.
    EarnedContextOverBudget(usize, usize),
    /// Earned: repeated (>= 2) timeouts.
    EarnedRepeatedTimeouts(usize),
}

impl ProbeVerdict {
    /// Whether this verdict permits decomposition.
    pub fn is_earned(&self) -> bool {
        matches!(
            self,
            ProbeVerdict::EarnedIndependentGoals(_)
                | ProbeVerdict::EarnedGoalHashSurvived
                | ProbeVerdict::EarnedContextOverBudget(_, _)
                | ProbeVerdict::EarnedRepeatedTimeouts(_)
        )
    }
}

impl DischargeProbe {
    /// Classify the probe. Qualifying conditions are checked in a fixed order
    /// so the verdict is deterministic when several hold at once.
    pub fn verdict(&self) -> ProbeVerdict {
        if !self.ran {
            return ProbeVerdict::NoProbe;
        }
        // > 2 independent unsolved goals: the goal really is several problems.
        if self.independent_unsolved_goals > 2 {
            return ProbeVerdict::EarnedIndependentGoals(self.independent_unsolved_goals);
        }
        // The same canonical goal hash surviving two semantic attempts: the
        // prover made no progress *on the mathematics*, twice.
        if self.same_goal_hash_survived && self.semantic_attempts >= 2 {
            return ProbeVerdict::EarnedGoalHashSurvived;
        }
        // Mandatory context exceeds the budget: it cannot be attempted whole.
        if self.context_budget_tokens > 0
            && self.mandatory_context_tokens > self.context_budget_tokens
        {
            return ProbeVerdict::EarnedContextOverBudget(
                self.mandatory_context_tokens,
                self.context_budget_tokens,
            );
        }
        // Repeated timeouts: the search space is too large as posed.
        if self.timeouts >= 2 {
            return ProbeVerdict::EarnedRepeatedTimeouts(self.timeouts);
        }
        // EXCLUSION. Nothing structural qualified. If the only thing that went
        // wrong was syntax/elaboration, the correct move is REPAIR, not
        // decomposition: a parse error is not a mathematical obstruction.
        if self.syntax_errors > 0 {
            return ProbeVerdict::Repair;
        }
        ProbeVerdict::NotQualifying
    }
}

// ===========================================================================
// The structural complexity proxy
// ===========================================================================

/// A deterministic structural complexity score for a statement.
///
/// **This is a proxy, not an AST measure.** Theoremata statements are surface
/// text in several dialects (informal prose, Lean, Rocq, Isabelle), so parsing
/// a real AST here would need a per-backend elaborator and would drag IO into a
/// pure gate. Instead we approximate the shape of the AST with three
/// surface-countable signals:
///
/// * **token count** — whitespace/punctuation-separated tokens, an AST-size proxy;
/// * **binder depth** — maximum bracket nesting plus the number of binder
///   tokens (`∀ ∃ λ forall exists fun lambda`), a quantifier-alternation proxy;
/// * **branch count** — logical connectives and case markers
///   (`∧ ∨ → /\ \/ -> and or implies if case match`), a proof-obligation-count
///   proxy.
///
/// The score is `tokens + 3*binder_depth + 2*branch_count`. The weights say a
/// nested binder costs about three tokens of difficulty and a branch about two.
/// They are a heuristic and only ever compared *ratio-wise between a parent and
/// its own children*, never across problems, so their absolute scale carries no
/// meaning. Being a proxy, it can be gamed by verbose restatement — which is
/// precisely why it is one arm of a disjunction and not the whole check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Complexity {
    pub tokens: usize,
    pub binder_depth: usize,
    pub branch_count: usize,
}

impl Complexity {
    /// The combined score: `tokens + 3*binder_depth + 2*branch_count`.
    pub fn score(&self) -> usize {
        self.tokens + 3 * self.binder_depth + 2 * self.branch_count
    }
}

const BINDER_WORDS: &[&str] = &["forall", "exists", "fun", "lambda", "∀", "∃", "λ", "Σ", "Π"];
const BRANCH_WORDS: &[&str] = &[
    "∧", "∨", "→", "/\\", "\\/", "->", "=>", "and", "or", "implies", "if", "case", "match", "|",
];

/// Compute the structural complexity proxy of a statement. Pure and
/// deterministic; see [`Complexity`] for what the numbers mean.
pub fn complexity(statement: &str) -> Complexity {
    // Normalise: strip a leading turnstile so `⊢ P` and `P` score the same.
    let text = statement.replace('⊢', " ").replace("|-", " ");

    // Tokens: split on whitespace, then peel bracket/punctuation characters so
    // `(f x)` counts as `f` and `x` rather than as two glued tokens.
    let tokens = text
        .split_whitespace()
        .flat_map(|w| w.split(|c: char| "()[]{},".contains(c)))
        .filter(|t| !t.trim().is_empty())
        .count();

    // Binder depth: max bracket nesting + number of binder tokens.
    let mut depth = 0usize;
    let mut max_depth = 0usize;
    for c in text.chars() {
        match c {
            '(' | '[' | '{' => {
                depth += 1;
                max_depth = max_depth.max(depth);
            }
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    let lower = text.to_ascii_lowercase();
    let binders = BINDER_WORDS
        .iter()
        .map(|w| count_occurrences(&lower, &w.to_ascii_lowercase()))
        .sum::<usize>();
    let binder_depth = max_depth + binders;

    let branch_count = BRANCH_WORDS
        .iter()
        .map(|w| count_occurrences(&lower, &w.to_ascii_lowercase()))
        .sum::<usize>();

    Complexity {
        tokens,
        binder_depth,
        branch_count,
    }
}

/// Non-overlapping occurrence count of `needle` in `haystack`.
fn count_occurrences(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    haystack.matches(needle).count()
}

// ===========================================================================
// Violations and the report
// ===========================================================================

/// A specific reason a decomposition was refused. Every variant carries enough
/// detail to be actionable in a log without re-deriving the analysis.
///
/// (`PartialEq` only, not `Eq`: [`Violation::NoComplexityReduction`] carries the
/// configured `f64` ratio.)
#[derive(Debug, Clone, PartialEq)]
pub enum Violation {
    /// A child's canonical key equals the parent's — "restate the goal as its
    /// own lemma". The degenerate move.
    SelfChild { child_id: String, key: String },
    /// Two children share a canonical key.
    DuplicateSiblings {
        first_id: String,
        second_id: String,
        key: String,
    },
    /// The resulting dependency graph contains a cycle.
    Cycle { nodes: Vec<String> },
    /// Child count outside `[min_children, max_children]`.
    ChildCount {
        found: usize,
        min: usize,
        max: usize,
    },
    /// The children would sit deeper than `max_depth`.
    DepthExceeded { child_depth: usize, max: usize },
    /// No child cuts the complexity proxy enough, and the children do not each
    /// isolate a distinct case/witness/lemma either.
    NoComplexityReduction {
        parent_score: usize,
        best_child_score: usize,
        required_ratio: f64,
    },
    /// The parent would not remain active as a composition obligation.
    ParentNotComposing { parent_id: String },
    /// **The authority leak.** A child was submitted as already proved on the
    /// decomposition model's assertion.
    AssertedChild {
        child_id: String,
        status: ChildStatus,
    },
    /// The leaf had not earned the right to be decomposed.
    Unearned { verdict: ProbeVerdict },
}

/// The obligation recorded when a decomposition is admitted: the parent stays
/// open as a COMPOSITION step, and its eventual proof must reference every
/// child theorem. Discharging the children does not discharge the parent — the
/// composition argument is itself a proof obligation, and a checker downstream
/// is expected to enforce `must_reference`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionObligation {
    /// The parent node that stays active.
    pub parent_id: String,
    /// Every child id the parent's proof is REQUIRED to reference.
    pub must_reference: Vec<String>,
    /// Canonical keys of the children, for hash-level reference checking.
    pub child_keys: Vec<String>,
}

/// The verdict. Fail-closed: `admitted` is true only when `violations` is empty.
#[derive(Debug, Clone, PartialEq)]
pub struct AdmissionReport {
    pub admitted: bool,
    pub violations: Vec<Violation>,
    /// The recorded composition obligation — present only when admitted.
    pub obligation: Option<CompositionObligation>,
    /// The probe verdict that was (or was not) sufficient to earn the split.
    pub probe_verdict: ProbeVerdict,
}

impl AdmissionReport {
    /// True when a violation of the given shape was recorded.
    pub fn has(&self, pred: impl Fn(&Violation) -> bool) -> bool {
        self.violations.iter().any(pred)
    }
}

// ===========================================================================
// admit — the gate
// ===========================================================================

/// Run every admission check with [`AdmissionConfig::default`].
pub fn admit(proposal: &DecompositionProposal) -> AdmissionReport {
    admit_with(proposal, &AdmissionConfig::default())
}

/// Run every admission check against `cfg`.
///
/// All checks run (they do not short-circuit) so a caller sees the full set of
/// problems in one pass. **Fail-closed**: a single violation refuses the
/// decomposition and no [`CompositionObligation`] is emitted.
pub fn admit_with(proposal: &DecompositionProposal, cfg: &AdmissionConfig) -> AdmissionReport {
    let mut violations = Vec::new();

    // --- 6. no_asserted_children (checked first: the gravest failure) --------
    // NO child may be admitted as proved on the decomposition model's
    // assertion. Children enter as unproved obligations, always.
    for child in &proposal.children {
        if child.status.is_assertion() {
            violations.push(Violation::AssertedChild {
                child_id: child.id.clone(),
                status: child.status,
            });
        }
    }

    // --- 7. earned_decomposition -------------------------------------------
    let probe_verdict = proposal.probe.verdict();
    if !probe_verdict.is_earned() {
        violations.push(Violation::Unearned {
            verdict: probe_verdict.clone(),
        });
    }

    // --- 3. bounded ---------------------------------------------------------
    let n = proposal.children.len();
    if n < cfg.min_children || n > cfg.max_children {
        violations.push(Violation::ChildCount {
            found: n,
            min: cfg.min_children,
            max: cfg.max_children,
        });
    }
    let child_depth = proposal.parent.depth + 1;
    if child_depth > cfg.max_depth {
        violations.push(Violation::DepthExceeded {
            child_depth,
            max: cfg.max_depth,
        });
    }

    // --- 1. no_self_child ---------------------------------------------------
    let parent_key = proposal.parent.canonical_key();
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for child in &proposal.children {
        let key = child.canonical_key();
        if key == parent_key {
            violations.push(Violation::SelfChild {
                child_id: child.id.clone(),
                key: key.clone(),
            });
        }
        if let Some(first) = seen.get(&key) {
            violations.push(Violation::DuplicateSiblings {
                first_id: first.clone(),
                second_id: child.id.clone(),
                key: key.clone(),
            });
        } else {
            seen.insert(key, child.id.clone());
        }
    }

    // --- 2. acyclic ---------------------------------------------------------
    if let Some(cycle) = find_cycle(proposal) {
        violations.push(Violation::Cycle { nodes: cycle });
    }

    // --- 4. complexity_reduction -------------------------------------------
    let parent_score = complexity(&proposal.parent.statement).score();
    let best_child_score = proposal
        .children
        .iter()
        .map(|c| complexity(&c.statement).score())
        .min()
        .unwrap_or(parent_score);
    // Arm A: some child undercuts the parent by at least `min_reduction`.
    let reduces = !proposal.children.is_empty()
        && (best_child_score as f64) <= (parent_score as f64) * (1.0 - cfg.min_reduction);
    // Arm B: every child isolates a *distinct*, non-empty case/witness/lemma.
    // A genuine case split need not get simpler, only disjoint.
    let tags: BTreeSet<&str> = proposal
        .children
        .iter()
        .filter_map(|c| c.isolates.as_deref())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    let isolates_distinctly =
        !proposal.children.is_empty() && tags.len() == proposal.children.len();
    if !reduces && !isolates_distinctly {
        violations.push(Violation::NoComplexityReduction {
            parent_score,
            best_child_score,
            required_ratio: cfg.min_reduction,
        });
    }

    // --- 5. parent_composes -------------------------------------------------
    if !proposal.parent.stays_active {
        violations.push(Violation::ParentNotComposing {
            parent_id: proposal.parent.id.clone(),
        });
    }

    let admitted = violations.is_empty();
    let obligation = admitted.then(|| CompositionObligation {
        parent_id: proposal.parent.id.clone(),
        must_reference: proposal.children.iter().map(|c| c.id.clone()).collect(),
        child_keys: proposal
            .children
            .iter()
            .map(ChildProposal::canonical_key)
            .collect(),
    });

    AdmissionReport {
        admitted,
        violations,
        obligation,
        probe_verdict,
    }
}

/// Detect a cycle in the graph that *would result* from admitting the proposal:
/// pre-existing edges, plus `parent -> child` for every child, plus each child's
/// declared dependencies. Edges point dependent -> dependency. Returns the
/// participating node ids in a deterministic (sorted) order, or `None`.
fn find_cycle(proposal: &DecompositionProposal) -> Option<Vec<String>> {
    let mut adj: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut edge = |from: &str, to: &str| {
        adj.entry(from.to_string())
            .or_default()
            .insert(to.to_string());
    };
    for (dependent, dependency) in &proposal.existing_edges {
        edge(dependent, dependency);
    }
    for child in &proposal.children {
        edge(&proposal.parent.id, &child.id);
        for dep in &child.depends_on {
            edge(&child.id, dep);
        }
    }

    // Iterative DFS with a three-colour marking, over a deterministic node order.
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        White,
        Grey,
        Black,
    }
    let nodes: BTreeSet<String> = adj
        .iter()
        .flat_map(|(k, vs)| std::iter::once(k.clone()).chain(vs.iter().cloned()))
        .collect();
    let mut mark: BTreeMap<String, Mark> = nodes.iter().map(|n| (n.clone(), Mark::White)).collect();

    for root in &nodes {
        if mark[root] != Mark::White {
            continue;
        }
        // (node, whether we are entering or leaving it)
        let mut stack: Vec<(String, bool)> = vec![(root.clone(), false)];
        let mut path: Vec<String> = Vec::new();
        while let Some((node, leaving)) = stack.pop() {
            if leaving {
                mark.insert(node.clone(), Mark::Black);
                path.pop();
                continue;
            }
            match mark[&node] {
                Mark::Grey => {
                    // Back edge: the cycle is the path suffix from `node` on.
                    let start = path.iter().position(|p| *p == node).unwrap_or(0);
                    let mut cycle: Vec<String> = path[start..].to_vec();
                    cycle.sort();
                    cycle.dedup();
                    return Some(cycle);
                }
                Mark::Black => continue,
                Mark::White => {}
            }
            mark.insert(node.clone(), Mark::Grey);
            path.push(node.clone());
            stack.push((node.clone(), true));
            if let Some(succs) = adj.get(&node) {
                for s in succs.iter().rev() {
                    if mark.get(s).copied().unwrap_or(Mark::White) != Mark::Black {
                        stack.push((s.clone(), false));
                    }
                }
            }
        }
    }
    None
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A probe that qualifies via the surviving-goal-hash condition.
    fn earned_probe() -> DischargeProbe {
        DischargeProbe {
            ran: true,
            semantic_attempts: 2,
            same_goal_hash_survived: true,
            ..Default::default()
        }
    }

    fn parent() -> ParentNode {
        ParentNode::new("P", "hA , hB ⊢ forall x , (f x) ∧ (g x) → (h x) ∨ (k x)", 1)
    }

    /// Two genuinely simpler children — the happy path.
    fn good_children() -> Vec<ChildProposal> {
        vec![
            ChildProposal::new("c1", "Left half", "hA ⊢ f x"),
            ChildProposal::new("c2", "Right half", "hB ⊢ g x"),
        ]
    }

    fn proposal(children: Vec<ChildProposal>) -> DecompositionProposal {
        DecompositionProposal::new(parent(), children, earned_probe())
    }

    #[test]
    fn well_formed_decomposition_is_admitted() {
        let report = admit(&proposal(good_children()));
        assert!(report.admitted, "violations: {:?}", report.violations);
        let ob = report.obligation.expect("composition obligation recorded");
        assert_eq!(ob.parent_id, "P");
        // The parent's proof must reference EVERY child.
        assert_eq!(ob.must_reference, vec!["c1".to_string(), "c2".to_string()]);
        assert_eq!(ob.child_keys.len(), 2);
    }

    #[test]
    fn child_identical_to_parent_is_rejected() {
        let p = parent();
        let children = vec![
            ChildProposal::new("c1", "The theorem, again", p.statement.clone()),
            ChildProposal::new("c2", "Right half", "hB ⊢ g x"),
        ];
        let report = admit(&proposal(children));
        assert!(!report.admitted);
        assert!(
            report.has(|v| matches!(v, Violation::SelfChild { child_id, .. } if child_id == "c1"))
        );
    }

    #[test]
    fn self_child_is_caught_through_hypothesis_reordering() {
        // Same goal, hypotheses swapped: the canonical key still matches, so the
        // restatement dodge does not work.
        let children = vec![
            ChildProposal::new(
                "c1",
                "Restated",
                "hB , hA ⊢ forall x , (f x) ∧ (g x) → (h x) ∨ (k x)",
            ),
            ChildProposal::new("c2", "Right half", "hB ⊢ g x"),
        ];
        let report = admit(&proposal(children));
        assert!(report.has(|v| matches!(v, Violation::SelfChild { .. })));
    }

    #[test]
    fn sibling_duplicates_are_rejected() {
        let children = vec![
            ChildProposal::new("c1", "Half", "hA ⊢ f x"),
            // Same goal, reordered hypotheses / extra whitespace.
            ChildProposal::new("c2", "Half again", "hA  ⊢  f x"),
        ];
        let report = admit(&proposal(children));
        assert!(!report.admitted);
        assert!(report.has(|v| matches!(v, Violation::DuplicateSiblings { .. })));
    }

    #[test]
    fn a_cycle_is_rejected() {
        // c1 depends on c2, and a pre-existing edge makes c2 depend on c1.
        let children = vec![
            ChildProposal::new("c1", "Left", "hA ⊢ f x").depending_on(["c2"]),
            ChildProposal::new("c2", "Right", "hB ⊢ g x"),
        ];
        let p = proposal(children).with_existing_edges([("c2".to_string(), "c1".to_string())]);
        let report = admit(&p);
        assert!(!report.admitted);
        assert!(report.has(|v| matches!(v, Violation::Cycle { .. })));
    }

    #[test]
    fn acyclic_diamond_is_not_flagged() {
        let children = vec![
            ChildProposal::new("c1", "Left", "hA ⊢ f x").depending_on(["c2"]),
            ChildProposal::new("c2", "Right", "hB ⊢ g x"),
        ];
        let report = admit(&proposal(children));
        assert!(!report.has(|v| matches!(v, Violation::Cycle { .. })));
        assert!(report.admitted, "violations: {:?}", report.violations);
    }

    #[test]
    fn out_of_range_child_counts_are_rejected() {
        // One child is a rename, not a decomposition.
        let one = vec![ChildProposal::new("c1", "Only", "hA ⊢ f x")];
        assert!(admit(&proposal(one)).has(|v| matches!(v, Violation::ChildCount { found: 1, .. })));

        // Seven children exceeds the max of 6.
        let many: Vec<ChildProposal> = (0..7)
            .map(|i| ChildProposal::new(format!("c{i}"), format!("Part {i}"), format!("hA ⊢ p{i}")))
            .collect();
        assert!(admit(&proposal(many)).has(|v| matches!(v, Violation::ChildCount { found: 7, .. })));

        // Zero children.
        assert!(admit(&proposal(Vec::new()))
            .has(|v| matches!(v, Violation::ChildCount { found: 0, .. })));
    }

    #[test]
    fn depth_limit_is_enforced() {
        let mut p = proposal(good_children());
        p.parent.depth = 6; // children would land at depth 7 > max 6
        let report = admit(&p);
        assert!(!report.admitted);
        assert!(report.has(|v| matches!(
            v,
            Violation::DepthExceeded {
                child_depth: 7,
                max: 6
            }
        )));
    }

    #[test]
    fn no_complexity_reduction_and_no_distinct_isolation_is_rejected() {
        // Children as complex as the parent (padded restatements), with no
        // isolation tags: neither arm of the disjunction is satisfied.
        let bloated = "hA , hB ⊢ forall x , (f x) ∧ (g x) → (h x) ∨ (k x) ∧ (m x)";
        let children = vec![
            ChildProposal::new("c1", "Variant one", bloated),
            ChildProposal::new("c2", "Variant two", format!("{bloated} ∨ (n x)")),
        ];
        let report = admit(&proposal(children));
        assert!(!report.admitted);
        assert!(report.has(|v| matches!(v, Violation::NoComplexityReduction { .. })));
    }

    #[test]
    fn equally_complex_children_pass_when_each_isolates_a_distinct_case() {
        let bloated = "hA , hB ⊢ forall x , (f x) ∧ (g x) → (h x) ∨ (k x) ∧ (m x)";
        let children = vec![
            ChildProposal::new("c1", "Case n = 0", bloated).isolating("case n = 0"),
            ChildProposal::new("c2", "Case n > 0", format!("{bloated} ∨ (n x)"))
                .isolating("case n > 0"),
        ];
        let report = admit(&proposal(children));
        assert!(report.admitted, "violations: {:?}", report.violations);
    }

    #[test]
    fn identical_isolation_tags_do_not_rescue_a_non_reducing_split() {
        // Both children claim to isolate the *same* case — that is not a split.
        let bloated = "hA , hB ⊢ forall x , (f x) ∧ (g x) → (h x) ∨ (k x) ∧ (m x)";
        let children = vec![
            ChildProposal::new("c1", "Case A", bloated).isolating("case A"),
            ChildProposal::new("c2", "Case A too", format!("{bloated} ∨ (n x)"))
                .isolating("case A"),
        ];
        assert!(admit(&proposal(children))
            .has(|v| matches!(v, Violation::NoComplexityReduction { .. })));
    }

    #[test]
    fn child_marked_proved_by_assertion_is_rejected() {
        let mut children = good_children();
        children[0].status = ChildStatus::AssertedProved;
        let report = admit(&proposal(children));
        assert!(
            !report.admitted,
            "asserting a child proved must never be admitted"
        );
        assert!(report.has(
            |v| matches!(v, Violation::AssertedChild { child_id, status }
                if child_id == "c1" && *status == ChildStatus::AssertedProved)
        ));
    }

    #[test]
    fn child_asserted_by_bare_citation_is_also_rejected() {
        let mut children = good_children();
        children[1].status = ChildStatus::AssertedByCitation;
        assert!(admit(&proposal(children)).has(|v| matches!(v, Violation::AssertedChild { .. })));
    }

    #[test]
    fn parent_must_remain_an_active_composition_obligation() {
        let mut p = proposal(good_children());
        p.parent.stays_active = false;
        let report = admit(&p);
        assert!(!report.admitted);
        assert!(report.has(|v| matches!(v, Violation::ParentNotComposing { .. })));
        assert!(report.obligation.is_none());
    }

    #[test]
    fn syntax_error_only_failure_does_not_earn_decomposition() {
        let probe = DischargeProbe {
            ran: true,
            syntax_errors: 5,
            semantic_attempts: 0,
            ..Default::default()
        };
        assert_eq!(probe.verdict(), ProbeVerdict::Repair);
        let p = DecompositionProposal::new(parent(), good_children(), probe);
        let report = admit(&p);
        assert!(
            !report.admitted,
            "syntax errors must route to REPAIR, not decomposition"
        );
        assert!(report.has(
            |v| matches!(v, Violation::Unearned { verdict } if *verdict == ProbeVerdict::Repair)
        ));
    }

    #[test]
    fn repeated_goal_hash_failure_earns_decomposition() {
        let probe = DischargeProbe {
            ran: true,
            semantic_attempts: 2,
            same_goal_hash_survived: true,
            // Syntax errors alongside a qualifying structural failure do not
            // veto it; they only fail to justify one on their own.
            syntax_errors: 3,
            ..Default::default()
        };
        assert_eq!(probe.verdict(), ProbeVerdict::EarnedGoalHashSurvived);
        let p = DecompositionProposal::new(parent(), good_children(), probe);
        assert!(admit(&p).admitted);
    }

    #[test]
    fn other_qualifying_probe_conditions() {
        let goals = DischargeProbe {
            ran: true,
            independent_unsolved_goals: 3,
            ..Default::default()
        };
        assert_eq!(goals.verdict(), ProbeVerdict::EarnedIndependentGoals(3));
        // Exactly two is NOT ">2".
        let two = DischargeProbe {
            ran: true,
            independent_unsolved_goals: 2,
            ..Default::default()
        };
        assert!(!two.verdict().is_earned());

        let ctx = DischargeProbe {
            ran: true,
            mandatory_context_tokens: 9000,
            context_budget_tokens: 8000,
            ..Default::default()
        };
        assert_eq!(
            ctx.verdict(),
            ProbeVerdict::EarnedContextOverBudget(9000, 8000)
        );

        let timeouts = DischargeProbe {
            ran: true,
            timeouts: 2,
            ..Default::default()
        };
        assert_eq!(timeouts.verdict(), ProbeVerdict::EarnedRepeatedTimeouts(2));

        // A single semantic attempt with a surviving hash is not yet enough.
        let once = DischargeProbe {
            ran: true,
            semantic_attempts: 1,
            same_goal_hash_survived: true,
            ..Default::default()
        };
        assert!(!once.verdict().is_earned());
    }

    #[test]
    fn no_probe_at_all_is_unearned() {
        let p = DecompositionProposal::new(parent(), good_children(), DischargeProbe::default());
        let report = admit(&p);
        assert!(!report.admitted);
        assert_eq!(report.probe_verdict, ProbeVerdict::NoProbe);
    }

    #[test]
    fn complexity_proxy_is_deterministic_and_ordered() {
        let simple = complexity("hA ⊢ f x");
        let hard = complexity("hA , hB ⊢ forall x , (f x) ∧ (g x) → (h x) ∨ (k x)");
        assert!(hard.score() > simple.score());
        // Determinism: same input, same score.
        assert_eq!(complexity("hA ⊢ f x"), simple);
        assert!(hard.binder_depth > 0 && hard.branch_count > 0);
    }

    #[test]
    fn config_is_tunable() {
        let cfg = AdmissionConfig {
            min_children: 3,
            ..Default::default()
        };
        // Two children pass the default but fail a min of 3.
        assert!(admit(&proposal(good_children())).admitted);
        assert!(!admit_with(&proposal(good_children()), &cfg).admitted);
    }

    #[test]
    fn all_violations_are_reported_in_one_pass() {
        let p_stmt = parent().statement;
        let mut children = vec![ChildProposal::new("c1", "Self", p_stmt)];
        children[0].status = ChildStatus::AssertedProved;
        let mut p = DecompositionProposal::new(parent(), children, DischargeProbe::default());
        p.parent.stays_active = false;
        let report = admit(&p);
        assert!(!report.admitted);
        // asserted child + unearned + child count + self child + parent not composing
        assert!(report.violations.len() >= 5, "got {:?}", report.violations);
    }

    #[test]
    fn from_obligation_never_yields_an_asserted_child() {
        let ob = super::super::decompose::Obligation {
            title: "Step 1".into(),
            statement: "hA ⊢ f x".into(),
            claim_kind: None,
            ingredients: Vec::new(),
        };
        let child = ChildProposal::from_obligation("c1", &ob);
        assert_eq!(child.status, ChildStatus::Unproved);
        assert!(!child.status.is_assertion());
    }
}
