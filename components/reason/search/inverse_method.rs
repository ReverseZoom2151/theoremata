//! Forward-saturation / inverse-method proof search — a second, *sound* search
//! substrate that sits alongside the backward MCGS driver ([`super::driver`]).
//!
//! Where the MCGS driver reasons **backward** (goal → subgoals, PUCT over a DAG),
//! this engine reasons **forward** (axioms → goal): it repeatedly derives new
//! consequences from the clauses it already has and adds them to a saturating
//! set, until the goal is derived (success) or no rule produces anything new
//! (saturation / fixpoint failure). This is the classical *inverse method* /
//! *given-clause loop* described in Pfenning's automated-deduction notes
//! (`docs/paper-mining/automated-theorem-proving-survey.md`, §"Forward search —
//! the Inverse Method"). Three ideas from that text are implemented here:
//!
//! * **Subsumption-based redundancy elimination.** A clause is stored as an
//!   ordered-literal sequent (hypotheses ⊢ conclusion) canonicalised through
//!   [`super::subsumption`]. A newly derived clause is dropped when an existing
//!   clause **subsumes** it (*forward subsumption*), and a newly derived clause
//!   that is stronger evicts the weaker stored clauses it subsumes (*backward
//!   subsumption*). This strictly dominates syntactic-equality dedup.
//! * **Focusing (Andreoli).** Invertible / asynchronous rules can be applied
//!   eagerly without losing completeness. [`focus`] chains a maximal sequence of
//!   invertible inferences into a single deterministic **macro-step**, collapsing
//!   the don't-know-nondeterminism that would otherwise inflate branching.
//! * **Saturation to a fixpoint** via a *given-clause* loop with a documented,
//!   deterministic selection heuristic (smallest sequent first).
//!
//! ## Soundness / determinism contract
//!
//! The engine is a pure algorithm: given the same axioms, goal, rule set and
//! [`SaturationConfig`] (including its `seed`) it returns byte-identical results.
//! There is **no** wall-clock and **no** unseeded randomness anywhere — the given
//! clause is chosen by a total order, not sampled. It never reports `proved`
//! unless a derived clause genuinely subsumes the goal, and it inherits the
//! sound-leaning conservatism of [`super::subsumption::subsumes`] (a false
//! *negative* only costs re-work; it never unsoundly discards or "proves").
//!
//! The [`InferenceRule`] trait is injectable, so the tests drive the whole engine
//! with deterministic mock rule sets and no external prover.

use super::subsumption::{subsumes, CanonicalGoal};

/// A clause / sequent: an (unordered, de-duplicated) set of hypothesis literals
/// entailing a single conclusion literal, `h0 , h1 ⊢ concl`. Internally it is a
/// [`CanonicalGoal`], so α-equivalent clauses and hypothesis reorderings share a
/// canonical form and subsumption is decided by [`super::subsumption::subsumes`].
///
/// `Sequent` is a type alias for `Clause` — the two names denote the same thing
/// (a clause in the inverse-method sense *is* a forward sequent).
#[derive(Clone, Debug)]
pub struct Clause {
    goal: CanonicalGoal,
    /// The original surface text, retained for readable derivations / logs.
    text: String,
}

/// A forward sequent — the same structure as a [`Clause`].
pub type Sequent = Clause;

impl Clause {
    /// Parse a clause from `hyps ⊢ concl` surface text (or just `concl` for an
    /// axiom with no hypotheses). Recognises the Unicode `⊢` and ASCII `|-`.
    pub fn parse(s: &str) -> Clause {
        Clause {
            goal: CanonicalGoal::parse(s),
            text: s.trim().to_string(),
        }
    }

    /// Build a sequent from explicit hypothesis literals and a conclusion.
    pub fn sequent(hyps: &[&str], conclusion: &str) -> Clause {
        let text = if hyps.is_empty() {
            format!("⊢ {conclusion}")
        } else {
            format!("{} ⊢ {conclusion}", hyps.join(" , "))
        };
        Clause::parse(&text)
    }

    /// The canonical (α-renamed, hypothesis-sorted) form.
    pub fn canonical(&self) -> &CanonicalGoal {
        &self.goal
    }

    /// The canonical dedup key — equal for α-equivalent / reordered clauses.
    pub fn key(&self) -> String {
        self.goal.key()
    }

    /// The original surface text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Canonicalised hypothesis literals (sorted, de-duplicated).
    pub fn hypotheses(&self) -> &[String] {
        self.goal.hypotheses()
    }

    /// Canonicalised conclusion literal.
    pub fn conclusion(&self) -> &str {
        self.goal.conclusion()
    }

    /// True if `self` (the more general clause) makes `other` redundant: same
    /// conclusion, and `self`'s hypotheses are a subset of `other`'s. Thin
    /// wrapper over [`super::subsumption::subsumes`] — the redundancy-elimination
    /// primitive used for both forward and backward subsumption.
    pub fn subsumes(&self, other: &Clause) -> bool {
        subsumes(&self.goal, &other.goal)
    }
}

/// An injectable inference rule: given the currently-known premises, generate the
/// clauses that follow. Mock rule sets in the tests implement this; a real prover
/// backend (a saturation calculus over Lean/Rocq goals) would implement the same
/// trait.
///
/// Implementations MUST be pure functions of their `premises` (no wall-clock, no
/// unseeded randomness) so saturation stays reproducible.
pub trait InferenceRule {
    /// A stable name, recorded in derivations.
    fn name(&self) -> &str;

    /// Whether this rule is **invertible / asynchronous**: applicable eagerly
    /// without losing completeness, so [`focus`] chains it into a deterministic
    /// macro-step rather than treating it as a branching (synchronous) choice.
    /// Defaults to `false` (synchronous).
    fn is_invertible(&self) -> bool {
        false
    }

    /// Derive consequences from `premises`. Returning an empty vector means the
    /// rule fired nothing. Duplicates / already-known clauses are fine — the
    /// engine's subsumption layer discards them.
    fn apply(&self, premises: &[Clause]) -> Vec<Clause>;
}

/// Tuning for [`saturate`].
#[derive(Clone, Copy, Debug)]
pub struct SaturationConfig {
    /// Hard cap on given-clause iterations (steps). Guarantees termination even
    /// on an unsaturable rule set — the engine stops without a false proof.
    pub max_steps: usize,
    /// Safety cap on the total number of stored clauses.
    pub max_clauses: usize,
    /// Iteration cap for a single [`focus`] macro-step's invertible fixpoint.
    pub focus_fixpoint_cap: usize,
    /// Seed threaded through the engine for reproducibility. The given-clause
    /// selection is a *total order* (fully deterministic) and consults no
    /// randomness, so results are identical for every seed; the field is threaded
    /// so a future sampling rule set has a deterministic source and to document
    /// that nothing here reads a wall-clock or unseeded RNG.
    pub seed: u64,
}

impl Default for SaturationConfig {
    fn default() -> Self {
        Self {
            max_steps: 1_000,
            max_clauses: 10_000,
            focus_fixpoint_cap: 256,
            seed: 0,
        }
    }
}

/// One line of a reconstructed derivation: the clause, the rule that produced it
/// (`"axiom"`, a rule name, or `"focus:<name>"` for an invertible macro-step),
/// and the premises it was attributed to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivationStep {
    pub clause: String,
    pub rule: String,
    pub premises: Vec<String>,
}

/// The outcome of a saturation run.
#[derive(Clone, Debug)]
pub struct SaturationResult {
    /// Whether a derived clause subsumed the goal (a genuine forward proof).
    pub proved: bool,
    /// Distinct non-axiom clauses added to the store.
    pub derived_count: usize,
    /// Total clauses discarded by subsumption (`forward_subsumed +
    /// backward_subsumed`).
    pub subsumed_count: usize,
    /// New clauses dropped because an existing clause already subsumed them.
    pub forward_subsumed: usize,
    /// Stored clauses evicted because a newly derived stronger clause subsumed
    /// them.
    pub backward_subsumed: usize,
    /// Given-clause iterations actually run (bounded by `max_steps`).
    pub steps: usize,
    /// How many of those steps had a non-empty [`focus`] macro-step (a collapsed
    /// chain of invertible inferences).
    pub macro_steps: usize,
    /// A derivation witness from axioms to the goal-proving clause, present iff
    /// `proved`. Ordered so every clause's premises appear before it.
    pub derivation: Option<Vec<DerivationStep>>,
}

/// Apply the invertible (`is_invertible() == true`) rules in `invertible_rules`
/// to `seeds` and their consequences repeatedly, collapsing a maximal chain of
/// forced/invertible inferences into a **single macro-step**. Returns only the
/// newly derived clauses (not the seeds), in derivation order, with intra-chain
/// forward-subsumption/dedup applied so nothing redundant is emitted.
///
/// This is Andreoli's focusing discipline: because invertible rules never need a
/// don't-know choice, chaining them eagerly loses no completeness while removing
/// the branching a naive step-at-a-time search would create. Deterministic and
/// bounded by [`SaturationConfig::focus_fixpoint_cap`].
pub fn focus(
    seeds: &[Clause],
    invertible_rules: &[&dyn InferenceRule],
    cfg: &SaturationConfig,
) -> Vec<Clause> {
    // `known` = seeds + everything derived so far (used to test redundancy).
    let mut known: Vec<Clause> = seeds.to_vec();
    let mut derived: Vec<Clause> = Vec::new();

    for _ in 0..cfg.focus_fixpoint_cap.max(1) {
        // Snapshot the premises for this fixpoint round.
        let premises: Vec<Clause> = known.clone();
        let mut added = false;
        for rule in invertible_rules {
            for c in rule.apply(&premises) {
                // Forward subsumption + exact-dup guard against what we already know.
                if known.iter().any(|k| k.subsumes(&c) || k.key() == c.key()) {
                    continue;
                }
                known.push(c.clone());
                derived.push(c);
                added = true;
                if known.len() >= cfg.max_clauses {
                    return derived;
                }
            }
        }
        if !added {
            break; // invertible fixpoint reached — the macro-step is complete.
        }
    }
    derived
}

/// A stored clause plus its derivation provenance.
struct Node {
    clause: Clause,
    /// `"axiom"`, a rule name, or `"focus:<name>"`.
    rule: String,
    /// Arena indices of the premises this clause was attributed to.
    parents: Vec<usize>,
    /// Selected as a given clause and generating (processed).
    active: bool,
    /// Still in the store (not evicted by backward subsumption).
    alive: bool,
}

/// The saturation engine state (arena of clauses + the passive worklist).
struct Engine {
    arena: Vec<Node>,
    /// Indices awaiting selection as a given clause.
    passive: Vec<usize>,
    goal: Clause,
    proved_by: Option<usize>,
    forward_subsumed: usize,
    backward_subsumed: usize,
    derived_count: usize,
    max_clauses: usize,
}

impl Engine {
    fn new(goal: Clause, max_clauses: usize) -> Self {
        Self {
            arena: Vec::new(),
            passive: Vec::new(),
            goal,
            proved_by: None,
            forward_subsumed: 0,
            backward_subsumed: 0,
            derived_count: 0,
            max_clauses,
        }
    }

    /// The currently active-and-alive clauses (the premises visible to rules).
    fn active_clauses(&self) -> Vec<Clause> {
        self.arena
            .iter()
            .filter(|n| n.active && n.alive)
            .map(|n| n.clause.clone())
            .collect()
    }

    /// Attempt to add a clause, enforcing forward + backward subsumption. Returns
    /// the new arena index, or `None` if the clause was redundant (dropped) or the
    /// store is full. Also records goal closure when the clause subsumes the goal.
    fn add(
        &mut self,
        clause: Clause,
        rule: &str,
        parents: Vec<usize>,
        is_axiom: bool,
    ) -> Option<usize> {
        // Exact-duplicate guard: an α/order-equal live clause already present.
        if self
            .arena
            .iter()
            .any(|n| n.alive && n.clause.key() == clause.key())
        {
            return None;
        }
        // Forward subsumption: some live clause already makes this one redundant.
        if self
            .arena
            .iter()
            .any(|n| n.alive && n.clause.subsumes(&clause))
        {
            self.forward_subsumed += 1;
            return None;
        }
        if self.arena.len() >= self.max_clauses {
            return None;
        }
        // Backward subsumption: this (stronger) clause evicts the weaker live
        // clauses it subsumes. The evicted nodes stay in the arena (so derivation
        // traces remain intact) but are `alive = false`.
        for n in self.arena.iter_mut() {
            if n.alive && clause.subsumes(&n.clause) && n.clause.key() != clause.key() {
                n.alive = false;
                n.active = false;
                self.backward_subsumed += 1;
            }
        }
        let idx = self.arena.len();
        let closes_goal = clause.subsumes(&self.goal);
        self.arena.push(Node {
            clause,
            rule: rule.to_string(),
            parents,
            active: false,
            alive: true,
        });
        if !is_axiom {
            self.derived_count += 1;
        }
        self.passive.push(idx);
        if closes_goal && self.proved_by.is_none() {
            self.proved_by = Some(idx);
        }
        Some(idx)
    }

    /// Select the next given clause: the **smallest sequent first** — fewest
    /// hypotheses, then shortest canonical key, then lexicographically smallest
    /// key. A total order ⇒ fully deterministic, no randomness. Skips dead /
    /// already-active nodes. Removes and returns the chosen passive index.
    fn select_given(&mut self) -> Option<usize> {
        let mut best: Option<(usize, usize)> = None; // (passive position, arena idx)
        for (pos, &idx) in self.passive.iter().enumerate() {
            let n = &self.arena[idx];
            if !n.alive || n.active {
                continue;
            }
            let better = match best {
                None => true,
                Some((_, b)) => {
                    order_key(&self.arena[idx].clause) < order_key(&self.arena[b].clause)
                }
            };
            if better {
                best = Some((pos, idx));
            }
        }
        let (pos, idx) = best?;
        self.passive.swap_remove(pos);
        Some(idx)
    }

    /// Reconstruct a derivation witness for `idx`, parents before children.
    fn derivation(&self, idx: usize) -> Vec<DerivationStep> {
        let mut out = Vec::new();
        let mut seen = vec![false; self.arena.len()];
        self.collect(idx, &mut out, &mut seen);
        out
    }

    fn collect(&self, idx: usize, out: &mut Vec<DerivationStep>, seen: &mut [bool]) {
        if seen[idx] {
            return;
        }
        seen[idx] = true;
        for &p in &self.arena[idx].parents {
            self.collect(p, out, seen);
        }
        let n = &self.arena[idx];
        out.push(DerivationStep {
            clause: n.clause.text().to_string(),
            rule: n.rule.clone(),
            premises: n
                .parents
                .iter()
                .map(|&p| self.arena[p].clause.text().to_string())
                .collect(),
        });
    }
}

/// Total-order key for the given-clause heuristic: (hypothesis count, key length,
/// key). Smaller = selected first.
fn order_key(c: &Clause) -> (usize, usize, String) {
    let key = c.key();
    (c.hypotheses().len(), key.len(), key)
}

/// Run forward saturation from `axioms` toward `goal` using `rules`.
///
/// The loop is the classical given-clause procedure:
/// 1. Seed the passive set with the axioms.
/// 2. Each step: select the smallest passive clause as the *given* clause, move
///    it to the active set, then
///    a. run a [`focus`] **macro-step** — eagerly close the active set under the
///       invertible rules (one deterministic collapse, counted as part of this
///       one step), and
///    b. apply the synchronous (non-invertible) rules to the active set.
/// 3. Every newly derived clause is filtered through forward subsumption (drop if
///    an existing clause subsumes it) and triggers backward subsumption (evict
///    the weaker stored clauses it subsumes).
/// 4. Stop as soon as a clause subsumes the goal (`proved`), when the passive set
///    empties (saturation / fixpoint — no proof exists in this calculus), or when
///    `max_steps` is hit (bounded give-up, never a false proof).
pub fn saturate(
    axioms: &[Clause],
    goal: &Clause,
    rules: &[Box<dyn InferenceRule>],
    cfg: &SaturationConfig,
) -> SaturationResult {
    let invertible: Vec<&dyn InferenceRule> = rules
        .iter()
        .filter(|r| r.is_invertible())
        .map(|r| r.as_ref())
        .collect();
    let synchronous: Vec<&dyn InferenceRule> = rules
        .iter()
        .filter(|r| !r.is_invertible())
        .map(|r| r.as_ref())
        .collect();

    let mut eng = Engine::new(goal.clone(), cfg.max_clauses);

    // Seed with axioms (goal closure is checked as each is added).
    for ax in axioms {
        eng.add(ax.clone(), "axiom", Vec::new(), true);
    }

    let mut steps = 0usize;
    let mut macro_steps = 0usize;

    while steps < cfg.max_steps && eng.proved_by.is_none() {
        let given = match eng.select_given() {
            Some(idx) => idx,
            None => break, // passive empty ⇒ saturated (fixpoint, no proof).
        };
        steps += 1;
        eng.arena[given].active = true;

        // (a) Focus macro-step: eagerly close the active set under invertible
        //     rules, chaining a whole invertible sequence into this single step.
        if !invertible.is_empty() && eng.proved_by.is_none() {
            let mut produced_in_macro = false;
            // Local buffer of macro-derived clauses so the chain sees its own
            // consequences without prematurely activating them (they stay passive
            // and are still processed as ordinary givens later).
            let mut extra: Vec<Clause> = Vec::new();
            for _ in 0..cfg.focus_fixpoint_cap.max(1) {
                let mut premises = eng.active_clauses();
                premises.extend(extra.iter().cloned());
                let mut added = false;
                for rule in &invertible {
                    let name = format!("focus:{}", rule.name());
                    for c in rule.apply(&premises) {
                        if let Some(_idx) = eng.add(c.clone(), &name, vec![given], false) {
                            extra.push(c);
                            added = true;
                            produced_in_macro = true;
                        }
                    }
                    if eng.proved_by.is_some() {
                        break;
                    }
                }
                if !added || eng.proved_by.is_some() {
                    break;
                }
            }
            if produced_in_macro {
                macro_steps += 1;
            }
        }

        // (b) Synchronous generation from the active set.
        if eng.proved_by.is_none() {
            let premises = eng.active_clauses();
            for rule in &synchronous {
                for c in rule.apply(&premises) {
                    eng.add(c, rule.name(), vec![given], false);
                    if eng.proved_by.is_some() {
                        break;
                    }
                }
                if eng.proved_by.is_some() {
                    break;
                }
            }
        }
    }

    let proved = eng.proved_by.is_some();
    let derivation = eng.proved_by.map(|idx| eng.derivation(idx));

    SaturationResult {
        proved,
        derived_count: eng.derived_count,
        subsumed_count: eng.forward_subsumed + eng.backward_subsumed,
        forward_subsumed: eng.forward_subsumed,
        backward_subsumed: eng.backward_subsumed,
        steps,
        macro_steps,
        derivation,
    }
}

// ---------------------------------------------------------------------------
// CLI entry point
// ---------------------------------------------------------------------------

/// A caller-supplied forward rule, in the only shape a CLI can express: "when a
/// known clause has conclusion `trigger`, the clauses in `emit` follow".
///
/// The engine's [`InferenceRule`] trait is injectable precisely so a rule set can
/// come from outside; this is the data-driven instance used by [`saturate_spec`].
/// The rules are **assumptions supplied by the caller**: nothing here checks that
/// they are valid inferences in any calculus, which is a large part of why a run's
/// output is never a proof (see [`SaturationSummary::needs_verification`]).
pub struct RuleSpec {
    name: String,
    trigger: String,
    emit: Vec<Clause>,
    invertible: bool,
}

impl RuleSpec {
    /// Parse `name : trigger => emit1 ; emit2`. A `*` prefix on the name marks the
    /// rule invertible, so [`saturate`] folds it into a [`focus`] macro-step.
    pub fn parse(spec: &str) -> anyhow::Result<RuleSpec> {
        let (head, body) = spec
            .split_once("=>")
            .ok_or_else(|| anyhow::anyhow!("rule spec needs `=>`: {spec}"))?;
        let (name, trigger) = head
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("rule spec needs `name : trigger`: {spec}"))?;
        let name = name.trim();
        let (invertible, name) = match name.strip_prefix('*') {
            Some(rest) => (true, rest.trim()),
            None => (false, name),
        };
        if name.is_empty() {
            anyhow::bail!("rule spec has an empty name: {spec}");
        }
        let trigger = trigger.trim();
        if trigger.is_empty() {
            anyhow::bail!("rule spec has an empty trigger: {spec}");
        }
        let emit: Vec<Clause> = body
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(Clause::parse)
            .collect();
        if emit.is_empty() {
            anyhow::bail!("rule spec emits nothing: {spec}");
        }
        Ok(RuleSpec {
            name: name.to_string(),
            trigger: trigger.to_string(),
            emit,
            invertible,
        })
    }
}

impl InferenceRule for RuleSpec {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_invertible(&self) -> bool {
        self.invertible
    }

    fn apply(&self, premises: &[Clause]) -> Vec<Clause> {
        if premises.iter().any(|p| p.conclusion() == self.trigger) {
            self.emit.clone()
        } else {
            Vec::new()
        }
    }
}

/// The JSON summary of a saturation run.
///
/// Every field that could be mistaken for a verification verdict is stated
/// negatively on purpose: a forward derivation under caller-supplied rules is a
/// *search outcome*, and nothing downstream may treat it as a checked proof.
#[derive(Debug, serde::Serialize)]
pub struct SaturationSummary {
    pub project_id: String,
    pub node_id: String,
    pub evidence_id: String,
    pub goal: String,
    /// `"goal_derived"`, `"step_bound_reached"` or `"saturated"`.
    pub outcome: String,
    /// A stored clause subsumed the goal. This is a search hit, not a proof.
    pub goal_derived: bool,
    /// Always `false`: this engine checks nothing against a formal system.
    pub verified: bool,
    /// Always `true`: any derivation below must be re-checked by a prover backend
    /// before it may be relied on.
    pub needs_verification: bool,
    /// The run stopped because `max_steps` was hit, so the search space was left
    /// partly unexplored and no conclusion at all may be drawn from a miss.
    pub hit_step_bound: bool,
    /// The passive set emptied before the bound. Note this is *not* a proof that
    /// the goal is underivable: it only means the caller's rule set derived
    /// nothing further, and that rule set is not known to be complete for
    /// anything.
    pub saturated: bool,
    pub steps: usize,
    pub max_steps: usize,
    pub max_clauses: usize,
    pub macro_steps: usize,
    pub derived_count: usize,
    pub forward_subsumed: usize,
    pub backward_subsumed: usize,
    /// The derivation witness when `goal_derived`, as `rule | clause | premises`
    /// lines. Unchecked.
    pub derivation: Vec<String>,
    pub caveats: Vec<String>,
}

/// Run forward saturation from textual axioms toward a textual goal under the
/// caller's [`RuleSpec`] rule set, recording the outcome as node evidence.
///
/// Thin adapter over [`saturate`]: it parses the surface text, calls the engine
/// unchanged, and reports the result. It never sets a node status, because a
/// forward search hit is not a verification result and the graph's status field
/// is reserved for things that are.
pub fn saturate_spec(
    store: &crate::db::Store,
    project_id: &str,
    node_id: &str,
    axioms: &[String],
    goal: &str,
    rule_specs: &[String],
    cfg: &SaturationConfig,
) -> anyhow::Result<SaturationSummary> {
    if goal.trim().is_empty() {
        anyhow::bail!("goal clause is empty");
    }
    let parsed_axioms: Vec<Clause> = axioms
        .iter()
        .map(|a| a.trim())
        .filter(|a| !a.is_empty())
        .map(Clause::parse)
        .collect();
    let goal_clause = Clause::parse(goal);
    let rules: Vec<Box<dyn InferenceRule>> = rule_specs
        .iter()
        .map(|s| RuleSpec::parse(s).map(|r| Box::new(r) as Box<dyn InferenceRule>))
        .collect::<anyhow::Result<_>>()?;

    let res = saturate(&parsed_axioms, &goal_clause, &rules, cfg);

    let hit_step_bound = !res.proved && res.steps >= cfg.max_steps;
    let saturated = !res.proved && !hit_step_bound;
    let outcome = if res.proved {
        "goal_derived"
    } else if hit_step_bound {
        "step_bound_reached"
    } else {
        "saturated"
    };

    let mut caveats = vec![
        "search outcome only: no formal system checked this derivation".to_string(),
        "the inference rules were supplied by the caller and were not validated"
            .to_string(),
    ];
    if hit_step_bound {
        caveats.push(format!(
            "stopped at the max_steps bound ({}); the search was not exhaustive",
            cfg.max_steps
        ));
    }
    if saturated {
        caveats.push(
            "saturation exhausted this rule set, which is not a completeness claim: \
             the goal may still be derivable under other rules"
                .to_string(),
        );
    }
    // The engine reports no flag for the store cap, so a run that silently stopped
    // storing clauses is indistinguishable from one that did not. Say so rather
    // than imply the cap was not reached.
    caveats.push(format!(
        "clauses may also have been dropped at the max_clauses cap ({}), which the \
         engine does not report",
        cfg.max_clauses
    ));

    let derivation: Vec<String> = res
        .derivation
        .as_ref()
        .map(|steps| {
            steps
                .iter()
                .map(|s| format!("{} | {} | {}", s.rule, s.clause, s.premises.join(" , ")))
                .collect()
        })
        .unwrap_or_default();

    let payload = serde_json::json!({
        "goal": goal_clause.text(),
        "outcome": outcome,
        "goal_derived": res.proved,
        "verified": false,
        "needs_verification": true,
        "hit_step_bound": hit_step_bound,
        "saturated": saturated,
        "steps": res.steps,
        "max_steps": cfg.max_steps,
        "max_clauses": cfg.max_clauses,
        "macro_steps": res.macro_steps,
        "derived_count": res.derived_count,
        "forward_subsumed": res.forward_subsumed,
        "backward_subsumed": res.backward_subsumed,
        "derivation": derivation,
        "caveats": caveats,
    });
    // Verdict is "unverified" even on a hit, so an evidence scan can never read a
    // forward derivation as a pass.
    let evidence_id = store.add_evidence(
        project_id,
        node_id,
        "inverse_method_search",
        "inverse_method",
        "unverified",
        payload,
    )?;

    Ok(SaturationSummary {
        project_id: project_id.to_string(),
        node_id: node_id.to_string(),
        evidence_id,
        goal: goal_clause.text().to_string(),
        outcome: outcome.to_string(),
        goal_derived: res.proved,
        verified: false,
        needs_verification: true,
        hit_step_bound,
        saturated,
        steps: res.steps,
        max_steps: cfg.max_steps,
        max_clauses: cfg.max_clauses,
        macro_steps: res.macro_steps,
        derived_count: res.derived_count,
        forward_subsumed: res.forward_subsumed,
        backward_subsumed: res.backward_subsumed,
        derivation,
        caveats,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic mock rule: when a premise's *conclusion* equals `trigger`,
    /// emit the (pre-parsed) clauses in `emit`. `invertible` marks it async so the
    /// engine folds it into a [`focus`] macro-step. Pure over its premises.
    struct EmitRule {
        name: String,
        trigger: String,
        emit: Vec<Clause>,
        invertible: bool,
    }

    impl EmitRule {
        fn new(name: &str, trigger: &str, emit: &[&str], invertible: bool) -> Self {
            Self {
                name: name.into(),
                trigger: trigger.into(),
                emit: emit.iter().map(|s| Clause::parse(s)).collect(),
                invertible,
            }
        }
    }

    impl InferenceRule for EmitRule {
        fn name(&self) -> &str {
            &self.name
        }
        fn is_invertible(&self) -> bool {
            self.invertible
        }
        fn apply(&self, premises: &[Clause]) -> Vec<Clause> {
            if premises.iter().any(|p| p.conclusion() == self.trigger) {
                self.emit.clone()
            } else {
                Vec::new()
            }
        }
    }

    /// An intentionally unsaturable rule: from the highest `s<k>` fact present it
    /// emits `s<k+1>`, so the store grows without bound and never derives the goal.
    struct CounterRule;
    impl InferenceRule for CounterRule {
        fn name(&self) -> &str {
            "succ"
        }
        fn apply(&self, premises: &[Clause]) -> Vec<Clause> {
            let mut max_k: Option<u64> = None;
            for p in premises {
                let concl = p.conclusion();
                if let Some(rest) = concl.strip_prefix('s') {
                    if let Ok(k) = rest.trim().parse::<u64>() {
                        max_k = Some(max_k.map_or(k, |m| m.max(k)));
                    }
                }
            }
            match max_k {
                Some(k) => vec![Clause::parse(&format!("⊢ s{}", k + 1))],
                None => Vec::new(),
            }
        }
    }

    fn cfg() -> SaturationConfig {
        SaturationConfig::default()
    }

    #[test]
    fn clause_and_sequent_share_type_and_canonicalize() {
        let a = Clause::sequent(&["P x", "Q y"], "R z");
        let b: Sequent = Clause::parse("Q y , P x ⊢ R z");
        assert_eq!(a.key(), b.key(), "hyp order must not affect the key");
        assert!(a.subsumes(&Clause::parse("P x , Q y , S w ⊢ R z")));
    }

    #[test]
    fn solvable_mock_is_proved_with_a_derivation() {
        // Forward chain a -> b -> c -> goal via synchronous rules.
        let axioms = [Clause::parse("⊢ a")];
        let goal = Clause::parse("⊢ goal");
        let rules: Vec<Box<dyn InferenceRule>> = vec![
            Box::new(EmitRule::new("r1", "a", &["⊢ b"], false)),
            Box::new(EmitRule::new("r2", "b", &["⊢ c"], false)),
            Box::new(EmitRule::new("r3", "c", &["⊢ goal"], false)),
        ];
        let res = saturate(&axioms, &goal, &rules, &cfg());
        assert!(res.proved, "the forward chain should reach the goal");
        let deriv = res.derivation.expect("a proof must carry a derivation");
        // Axiom first, goal-proving clause last.
        assert_eq!(deriv.first().unwrap().rule, "axiom");
        assert_eq!(deriv.last().unwrap().clause, "⊢ goal");
        // The whole chain a,b,c,goal appears in order.
        let clauses: Vec<&str> = deriv.iter().map(|d| d.clause.as_str()).collect();
        assert_eq!(clauses, vec!["⊢ a", "⊢ b", "⊢ c", "⊢ goal"]);
    }

    #[test]
    fn forward_subsumption_drops_a_redundant_weaker_clause() {
        // Axiom `⊢ q` (general). A rule tries to derive `p ⊢ q` (weaker: needs an
        // extra hypothesis). The stored general clause subsumes it, so it is
        // dropped rather than stored. Goal is unreachable ⇒ run saturates.
        let axioms = [Clause::parse("⊢ q")];
        let goal = Clause::parse("⊢ unreachable");
        let rules: Vec<Box<dyn InferenceRule>> =
            vec![Box::new(EmitRule::new("weaken", "q", &["p ⊢ q"], false))];
        let res = saturate(&axioms, &goal, &rules, &cfg());
        assert!(!res.proved);
        assert!(
            res.forward_subsumed >= 1,
            "the weaker `p ⊢ q` must be forward-subsumed (got {})",
            res.forward_subsumed
        );
        // Nothing redundant was stored.
        assert_eq!(res.derived_count, 0);
    }

    #[test]
    fn backward_subsumption_evicts_a_now_subsumed_clause() {
        // Axiom `p ⊢ q` (weaker) is stored first. A rule then derives the stronger
        // `⊢ q`, which subsumes the stored clause and must evict it.
        let axioms = [Clause::parse("p ⊢ q")];
        let goal = Clause::parse("⊢ unreachable");
        let rules: Vec<Box<dyn InferenceRule>> =
            vec![Box::new(EmitRule::new("strengthen", "q", &["⊢ q"], false))];
        let res = saturate(&axioms, &goal, &rules, &cfg());
        assert!(!res.proved);
        assert!(
            res.backward_subsumed >= 1,
            "the stronger `⊢ q` must backward-subsume `p ⊢ q` (got {})",
            res.backward_subsumed
        );
    }

    #[test]
    fn focus_collapses_an_invertible_chain_into_one_macro_step() {
        // Direct test of the macro-step: a -> b -> c -> d via three invertible
        // rules. A single `focus` call must derive b, c AND d (the whole chain),
        // not just the first step.
        let seeds = [Clause::parse("⊢ a")];
        let r1 = EmitRule::new("i1", "a", &["⊢ b"], true);
        let r2 = EmitRule::new("i2", "b", &["⊢ c"], true);
        let r3 = EmitRule::new("i3", "c", &["⊢ d"], true);
        let inv: Vec<&dyn InferenceRule> = vec![&r1, &r2, &r3];
        let derived = focus(&seeds, &inv, &cfg());
        let texts: Vec<&str> = derived.iter().map(|c| c.text()).collect();
        assert_eq!(
            texts,
            vec!["⊢ b", "⊢ c", "⊢ d"],
            "one focus macro-step must collapse the entire invertible chain"
        );
    }

    #[test]
    fn saturate_uses_focus_and_reports_a_macro_step() {
        // Same invertible chain inside the engine: it should prove the goal and
        // record at least one macro-step, closing in a single given-clause step.
        let axioms = [Clause::parse("⊢ a")];
        let goal = Clause::parse("⊢ goal");
        let rules: Vec<Box<dyn InferenceRule>> = vec![
            Box::new(EmitRule::new("i1", "a", &["⊢ b"], true)),
            Box::new(EmitRule::new("i2", "b", &["⊢ c"], true)),
            Box::new(EmitRule::new("i3", "c", &["⊢ goal"], true)),
        ];
        let res = saturate(&axioms, &goal, &rules, &cfg());
        assert!(res.proved);
        assert!(
            res.macro_steps >= 1,
            "the invertible chain must be counted as a macro-step"
        );
        // Collapsed: the whole chain closes within the first given clause.
        assert_eq!(res.steps, 1, "focusing should close the goal in one step");
    }

    #[test]
    fn unsaturable_run_stops_at_max_steps_without_a_false_proof() {
        // The CounterRule generates s1, s2, s3, ... forever and never derives the
        // goal. A bounded run must stop at max_steps and report NOT proved.
        let axioms = [Clause::parse("⊢ s0")];
        let goal = Clause::parse("⊢ target");
        let rules: Vec<Box<dyn InferenceRule>> = vec![Box::new(CounterRule)];
        let bounded = SaturationConfig {
            max_steps: 10,
            ..cfg()
        };
        let res = saturate(&axioms, &goal, &rules, &bounded);
        assert!(!res.proved, "an unsaturable run must never falsely prove");
        assert_eq!(res.steps, 10, "it must run exactly up to the step bound");
        assert!(res.derivation.is_none());
    }

    #[test]
    fn saturation_terminates_when_no_rule_fires() {
        // No rule matches ⇒ the passive set empties and the run stops well before
        // max_steps, with no proof.
        let axioms = [Clause::parse("⊢ a")];
        let goal = Clause::parse("⊢ b");
        let rules: Vec<Box<dyn InferenceRule>> =
            vec![Box::new(EmitRule::new("noop", "zzz", &["⊢ nope"], false))];
        let res = saturate(&axioms, &goal, &rules, &cfg());
        assert!(!res.proved);
        assert!(res.steps < 1_000, "should saturate, not exhaust the bound");
    }

    #[test]
    fn saturation_is_deterministic() {
        let build = || -> Vec<Box<dyn InferenceRule>> {
            vec![
                Box::new(EmitRule::new("r1", "a", &["⊢ b"], false)),
                Box::new(EmitRule::new("r2", "b", &["⊢ c"], false)),
                Box::new(EmitRule::new("r3", "c", &["⊢ goal"], false)),
            ]
        };
        let axioms = [Clause::parse("⊢ a")];
        let goal = Clause::parse("⊢ goal");
        let r1 = saturate(
            &axioms,
            &goal,
            &build(),
            &SaturationConfig { seed: 1, ..cfg() },
        );
        let r2 = saturate(
            &axioms,
            &goal,
            &build(),
            &SaturationConfig { seed: 999, ..cfg() },
        );
        // Deterministic regardless of seed: identical everything.
        assert_eq!(r1.proved, r2.proved);
        assert_eq!(r1.steps, r2.steps);
        assert_eq!(r1.derived_count, r2.derived_count);
        assert_eq!(r1.subsumed_count, r2.subsumed_count);
        assert_eq!(r1.derivation, r2.derivation);
    }

    // ---- entry point ----

    use crate::db::Store;
    use crate::model::{NodeKind, NodeTier};
    use std::path::Path;

    /// An in-memory store with one project and one node to hang evidence on.
    fn fixture() -> (Store, String, String) {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "a toy claim").unwrap();
        let node = store
            .add_node_detailed(
                &project.id,
                NodeKind::Conjecture,
                NodeTier::Spine,
                None,
                "target",
                "goal",
                None,
                &[],
                "test",
            )
            .unwrap();
        (store, project.id, node.id)
    }

    #[test]
    fn rule_spec_parses_name_trigger_and_invertibility() {
        let r = RuleSpec::parse("* step : a => ⊢ b ; ⊢ c").unwrap();
        assert_eq!(r.name(), "step");
        assert!(r.is_invertible());
        let fired = r.apply(&[Clause::parse("⊢ a")]);
        assert_eq!(fired.len(), 2);
        // Not triggered by an unrelated conclusion.
        assert!(r.apply(&[Clause::parse("⊢ z")]).is_empty());
        assert!(RuleSpec::parse("no arrow here").is_err());
        assert!(RuleSpec::parse("r : a =>").is_err());
    }

    #[test]
    fn entry_point_reports_a_hit_as_unverified_with_a_derivation() {
        let (store, project, node) = fixture();
        let summary = saturate_spec(
            &store,
            &project,
            &node,
            &["⊢ a".to_string()],
            "⊢ goal",
            &[
                "r1 : a => ⊢ b".to_string(),
                "r2 : b => ⊢ goal".to_string(),
            ],
            &cfg(),
        )
        .unwrap();
        assert!(summary.goal_derived);
        assert_eq!(summary.outcome, "goal_derived");
        // A hit must never present itself as checked.
        assert!(!summary.verified);
        assert!(summary.needs_verification);
        assert!(!summary.derivation.is_empty());
        assert!(!summary.hit_step_bound);

        // Evidence is recorded against the node with an unverified verdict.
        let evidence = store.evidence(&project, &node).unwrap();
        let row = evidence
            .iter()
            .find(|e| e.evidence_type == "inverse_method_search")
            .expect("the run must be recorded");
        assert_eq!(row.verdict, "unverified");
        assert_eq!(row.payload["verified"], serde_json::json!(false));
        assert_eq!(row.payload["needs_verification"], serde_json::json!(true));
    }

    #[test]
    fn entry_point_distinguishes_a_bound_stop_from_saturation() {
        let (store, project, node) = fixture();
        // No rule fires, so the passive set empties: saturated, not bounded out.
        let saturated = saturate_spec(
            &store,
            &project,
            &node,
            &["⊢ a".to_string()],
            "⊢ goal",
            &["r : zzz => ⊢ nope".to_string()],
            &cfg(),
        )
        .unwrap();
        assert!(!saturated.goal_derived);
        assert_eq!(saturated.outcome, "saturated");
        assert!(saturated.saturated);
        assert!(!saturated.hit_step_bound);
        // Saturation must still refuse to claim anything.
        assert!(saturated
            .caveats
            .iter()
            .any(|c| c.contains("not a completeness claim")));

        // A one-step budget on an endlessly generating rule set stops at the bound.
        let bounded = saturate_spec(
            &store,
            &project,
            &node,
            &["⊢ a".to_string()],
            "⊢ goal",
            &["r : a => ⊢ b".to_string(), "r2 : b => ⊢ c".to_string()],
            &SaturationConfig {
                max_steps: 1,
                ..cfg()
            },
        )
        .unwrap();
        assert!(!bounded.goal_derived);
        assert_eq!(bounded.outcome, "step_bound_reached");
        assert!(bounded.hit_step_bound);
        assert!(!bounded.saturated);
        assert!(bounded.derivation.is_empty());
        assert!(bounded.caveats.iter().any(|c| c.contains("max_steps")));
    }

    #[test]
    fn entry_point_rejects_malformed_input() {
        let (store, project, node) = fixture();
        assert!(saturate_spec(&store, &project, &node, &[], "  ", &[], &cfg()).is_err());
        assert!(saturate_spec(
            &store,
            &project,
            &node,
            &["⊢ a".to_string()],
            "⊢ goal",
            &["broken".to_string()],
            &cfg()
        )
        .is_err());
    }
}
