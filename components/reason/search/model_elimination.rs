//! Goal-directed **model-elimination / weak-connection-tableau** proof search over
//! first-order clauses — the backward, refutational counterpart to the forward
//! saturation engine in [`super::inverse_method`].
//!
//! The design follows Harrison's *"Optimizing Proof Search in Model Elimination"*
//! (`me.pdf`) and the closely-related MESON procedure in his *Handbook of Practical
//! Logic and Automated Reasoning*: a chain of goal literals is closed by repeatedly
//!
//! * an **extension (input) step** — resolve the leading goal literal against a
//!   complementary literal of a (renamed-apart) input clause, applying the mgu and
//!   pushing the clause's remaining literals as fresh subgoals, with the closed
//!   literal recorded as an *ancestor* of that new branch; or
//! * a **reduction step** — close the leading goal literal directly against an
//!   ancestor of complementary sign on the current branch (again applying the mgu).
//!
//! All term machinery is **reused verbatim** from [`super::rewriting`]: the
//! [`Term`] model, Robinson [`unify`] (with its sound occurs-check) and
//! [`apply_subst`] over a [`Subst`]. Nothing here re-implements unification.
//!
//! ## Iterative deepening
//!
//! [`prove`] performs **iterative deepening** on a per-branch extension-depth bound:
//! round `d = 0, 1, 2, …` searches for a closed tableau in which no subgoal branch
//! uses more than `d` extension steps, returning the first proof found. This keeps
//! the search complete for the fragment we target while bounding work each round,
//! and a too-small bound provably misses a proof a larger bound finds.
//!
//! ## Continuation caching (lemma memoization)
//!
//! Repeated **ground subgoals** are memoized. When the leading goal literal is
//! ground it is closed as a *self-contained lemma* — solved with an empty ancestor
//! set, so its proof depends only on the clause set and the remaining budget, never
//! on the surrounding branch. The `(signed-atom, budget) → success/failure` result
//! is cached in a deterministic [`BTreeMap`], so a subgoal reached twice (e.g. via
//! two different super-goals) is expanded only once. Because a ground lemma binds
//! no outer variables, splicing a cached proof can never change the outcome — the
//! cache is a pure optimization: cached and uncached runs return the same result,
//! only the number of extension expansions differs.
//!
//! ## Soundness / determinism contract
//!
//! A [`Proof`] is returned only when every goal literal genuinely closes by a
//! real unification (occurs-check included, inherited from [`unify`]) — the engine
//! never reports success otherwise. Every loop is bounded (by the depth budget and
//! the finite clause set), so search always terminates. There is **no** wall-clock
//! and **no** randomness: clause renaming uses a monotonic counter and subgoals are
//! tried in a fixed order, so a run is byte-identical given the same inputs.
//!
//! Reduction against ancestors is fully supported; the ground-lemma cache is a
//! goal-directed optimization that is exact for Horn / reduction-free refutations
//! (which includes the classic unsatisfiable examples). See [`Search`] for the API.

use super::rewriting::{apply_subst, unify, Subst, Term};
use std::collections::BTreeMap;
use std::fmt;

/// A first-order literal: a signed atom. `negated == true` is `¬atom`. The `atom`
/// is a [`Term`] whose head symbol is the predicate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Literal {
    /// `true` for a negative literal `¬atom`, `false` for a positive one.
    pub negated: bool,
    /// The atom (a predicate applied to argument terms).
    pub atom: Term,
}

impl Literal {
    /// A positive literal `atom`.
    pub fn pos(atom: Term) -> Literal {
        Literal { negated: false, atom }
    }

    /// A negative literal `¬atom`.
    pub fn neg(atom: Term) -> Literal {
        Literal { negated: true, atom }
    }

    /// Rename every variable in the atom by appending `suffix` (used to make an
    /// input clause's variables disjoint from the current chain before resolving).
    fn rename(&self, suffix: &str) -> Literal {
        Literal {
            negated: self.negated,
            atom: rename_term(&self.atom, suffix),
        }
    }
}

impl fmt::Display for Literal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.negated {
            write!(f, "~{}", self.atom)
        } else {
            write!(f, "{}", self.atom)
        }
    }
}

/// A first-order clause: a disjunction of [`Literal`]s.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clause {
    /// The clause's literals (an empty vector is the empty clause `⊥`).
    pub literals: Vec<Literal>,
}

impl Clause {
    /// Build a clause from its literals.
    pub fn new(literals: Vec<Literal>) -> Clause {
        Clause { literals }
    }

    /// Rename every variable in every literal by appending `suffix`.
    fn rename(&self, suffix: &str) -> Clause {
        Clause {
            literals: self.literals.iter().map(|l| l.rename(suffix)).collect(),
        }
    }
}

impl fmt::Display for Clause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.literals.is_empty() {
            return write!(f, "⊥");
        }
        for (i, l) in self.literals.iter().enumerate() {
            if i > 0 {
                write!(f, " ∨ ")?;
            }
            write!(f, "{l}")?;
        }
        Ok(())
    }
}

/// One inference in a reconstructed proof.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProofStep {
    /// Closed the goal literal by resolving against `clause` (index into the input
    /// clause set); `resolved_literal` is the complementary clause literal used.
    Extension {
        /// The (instantiated) goal literal that was closed.
        goal: String,
        /// Index of the input clause used.
        clause: usize,
        /// The clause literal resolved against (renamed / instantiated form).
        resolved_literal: String,
    },
    /// Closed the goal literal directly against an ancestor of complementary sign.
    Reduction {
        /// The (instantiated) goal literal that was closed.
        goal: String,
        /// The ancestor literal it was closed against.
        ancestor: String,
    },
}

impl fmt::Display for ProofStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProofStep::Extension { goal, clause, resolved_literal } => {
                write!(f, "extend {goal} with clause #{clause} on {resolved_literal}")
            }
            ProofStep::Reduction { goal, ancestor } => {
                write!(f, "reduce {goal} against ancestor {ancestor}")
            }
        }
    }
}

/// A successful refutation: the ordered inference steps and the depth bound at which
/// it was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proof {
    /// The inference steps, in the order they closed subgoals.
    pub steps: Vec<ProofStep>,
    /// The extension-depth bound the proof was found at (its "cost").
    pub depth: usize,
}

impl fmt::Display for Proof {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "proof (depth {}):", self.depth)?;
        for (i, s) in self.steps.iter().enumerate() {
            writeln!(f, "  {}. {s}", i + 1)?;
        }
        Ok(())
    }
}

/// A pending goal literal together with the ancestor literals on its branch (used
/// for reduction steps) and a flag suppressing the lemma cache for a lemma's own
/// root (to keep the memoized solver from recursing into itself).
#[derive(Clone, Debug)]
struct Pending {
    lit: Literal,
    ancestors: Vec<Literal>,
    skip_lemma: bool,
}

/// A cached ground-subgoal outcome.
#[derive(Clone, Debug)]
enum CacheEntry {
    /// Proved as a self-contained lemma, with the spliceable step sequence.
    Proved(Vec<ProofStep>),
    /// Not provable as a lemma within the recorded budget.
    Failed,
}

/// The model-elimination search state: the (immutable) input clauses, the
/// continuation cache, and deterministic bookkeeping counters. Construct with
/// [`Search::new`] and drive a single bounded round with [`Search::run`]; most
/// callers should instead use the iterative-deepening [`prove`] wrapper.
pub struct Search<'a> {
    clauses: &'a [Clause],
    use_cache: bool,
    cache: BTreeMap<(String, usize), CacheEntry>,
    /// Monotonic source of unique variable-renaming suffixes (no randomness).
    rename_counter: usize,
    /// Number of goal literals for which the extension clause-loop was actually run
    /// (real search work; cache hits do not count).
    expansions: usize,
    /// Number of times a ground subgoal was served from the cache instead of
    /// re-expanded.
    cache_hits: usize,
}

impl<'a> Search<'a> {
    /// A fresh search over `clauses`. `use_cache` toggles the continuation cache
    /// (disable it to measure the caching effect, or for a pure reference run).
    pub fn new(clauses: &'a [Clause], use_cache: bool) -> Search<'a> {
        Search {
            clauses,
            use_cache,
            cache: BTreeMap::new(),
            rename_counter: 0,
            expansions: 0,
            cache_hits: 0,
        }
    }

    /// Attempt to close every literal of `goal` within a per-branch extension depth
    /// of `budget`. Returns the proof steps on success.
    pub fn run(&mut self, goal: &[Literal], budget: usize) -> Option<Vec<ProofStep>> {
        let goals: Vec<Pending> = goal
            .iter()
            .map(|l| Pending {
                lit: l.clone(),
                ancestors: Vec::new(),
                skip_lemma: false,
            })
            .collect();
        self.solve(Subst::new(), &goals, budget, Vec::new())
            .map(|(_, steps)| steps)
    }

    /// Extension expansions performed so far.
    pub fn expansions(&self) -> usize {
        self.expansions
    }

    /// Cache hits (ground subgoals served without re-expansion) so far.
    pub fn cache_hits(&self) -> usize {
        self.cache_hits
    }

    /// Core depth-first tableau search. Closes the first goal in `goals` (by
    /// reduction against its ancestors, or by extension against an input clause),
    /// then recurses on the remainder, threading the substitution `env` and the
    /// accumulated `proof`. Returns the final env + proof on success.
    fn solve(
        &mut self,
        env: Subst,
        goals: &[Pending],
        budget: usize,
        proof: Vec<ProofStep>,
    ) -> Option<(Subst, Vec<ProofStep>)> {
        let g = match goals.first() {
            None => return Some((env, proof)), // all subgoals closed ⇒ success
            Some(g) => g.clone(),
        };
        let rest = &goals[1..];
        let g_atom = apply_subst(&env, &g.lit.atom);
        let g_shown = Literal {
            negated: g.lit.negated,
            atom: g_atom.clone(),
        };

        // (A) Reduction: close g against a complementary ancestor on this branch.
        for anc in &g.ancestors {
            if anc.negated == g.lit.negated {
                continue; // same sign ⇒ not complementary
            }
            if let Some(env2) = unify_under(&env, &g.lit.atom, &anc.atom) {
                let anc_shown = Literal {
                    negated: anc.negated,
                    atom: apply_subst(&env, &anc.atom),
                };
                let mut p2 = proof.clone();
                p2.push(ProofStep::Reduction {
                    goal: g_shown.to_string(),
                    ancestor: anc_shown.to_string(),
                });
                if let Some(r) = self.solve(env2, rest, budget, p2) {
                    return Some(r);
                }
            }
        }

        // (B') Ground goal: close it as a self-contained (memoized) lemma. A ground
        //      lemma binds no outer variables, so any proof of it is valid here and
        //      the choice cannot affect the sibling subgoals in `rest`.
        if is_ground(&g_atom) && !g.skip_lemma {
            let lemma_lit = Literal {
                negated: g.lit.negated,
                atom: g_atom,
            };
            return match self.solve_lemma(&lemma_lit, budget) {
                Some(steps) => {
                    let mut p2 = proof.clone();
                    p2.extend(steps);
                    self.solve(env.clone(), rest, budget, p2)
                }
                None => None,
            };
        }

        // (B) Extension: resolve g against a complementary literal of an input
        //     clause (renamed apart), pushing the clause's other literals as new
        //     subgoals with g recorded as their ancestor.
        if budget == 0 {
            return None; // no extension budget left on this branch
        }
        self.expansions += 1;
        let n = self.clauses.len();
        for ci in 0..n {
            let clause = self.clauses[ci].clone();
            self.rename_counter += 1;
            let suffix = format!("_v{}", self.rename_counter);
            let renamed = clause.rename(&suffix);
            for li in 0..renamed.literals.len() {
                let klit = &renamed.literals[li];
                if klit.negated == g.lit.negated {
                    continue; // need a complementary clause literal
                }
                if let Some(env2) = unify_under(&env, &g.lit.atom, &klit.atom) {
                    // Remaining clause literals become new subgoals; g joins the
                    // ancestor path for that new branch.
                    let mut child_ancestors = g.ancestors.clone();
                    child_ancestors.push(g.lit.clone());
                    let mut combined: Vec<Pending> = Vec::new();
                    for (lj, other) in renamed.literals.iter().enumerate() {
                        if lj == li {
                            continue;
                        }
                        combined.push(Pending {
                            lit: other.clone(),
                            ancestors: child_ancestors.clone(),
                            skip_lemma: false,
                        });
                    }
                    combined.extend_from_slice(rest);
                    let mut p2 = proof.clone();
                    p2.push(ProofStep::Extension {
                        goal: g_shown.to_string(),
                        clause: ci,
                        resolved_literal: klit.to_string(),
                    });
                    if let Some(r) = self.solve(env2, &combined, budget - 1, p2) {
                        return Some(r);
                    }
                }
            }
        }
        None
    }

    /// Close a single **ground** literal as a self-contained lemma (empty ancestor
    /// set), memoizing the `(signed-atom, budget)` outcome. The lemma root sets
    /// `skip_lemma` so the memoized solver expands it directly instead of recursing
    /// back into the cache; nested ground subgoals still reuse the cache.
    fn solve_lemma(&mut self, lit: &Literal, budget: usize) -> Option<Vec<ProofStep>> {
        let key = (lit.to_string(), budget);
        if self.use_cache {
            if let Some(entry) = self.cache.get(&key) {
                self.cache_hits += 1;
                return match entry {
                    CacheEntry::Proved(steps) => Some(steps.clone()),
                    CacheEntry::Failed => None,
                };
            }
        }
        let pend = vec![Pending {
            lit: lit.clone(),
            ancestors: Vec::new(),
            skip_lemma: true,
        }];
        let outcome = self
            .solve(Subst::new(), &pend, budget, Vec::new())
            .map(|(_, steps)| steps);
        if self.use_cache {
            let entry = match &outcome {
                Some(steps) => CacheEntry::Proved(steps.clone()),
                None => CacheEntry::Failed,
            };
            self.cache.insert(key, entry);
        }
        outcome
    }
}

/// Unify `a` and `b` under the existing substitution `env`, returning the extended
/// substitution or `None`. Both terms are first fully instantiated by `env`, so the
/// fresh bindings involve only variables still free under `env`; because
/// [`apply_subst`] resolves chains recursively, merging those bindings into `env`
/// needs no further composition.
fn unify_under(env: &Subst, a: &Term, b: &Term) -> Option<Subst> {
    let a2 = apply_subst(env, a);
    let b2 = apply_subst(env, b);
    let delta = unify(&a2, &b2)?;
    let mut merged = env.clone();
    for (k, v) in delta {
        merged.insert(k, v);
    }
    Some(merged)
}

/// True if `term` contains no variables.
fn is_ground(term: &Term) -> bool {
    match term {
        Term::Var(_) => false,
        Term::App(_, args) => args.iter().all(is_ground),
    }
}

/// Rename every variable in `term` by appending `suffix`.
fn rename_term(term: &Term, suffix: &str) -> Term {
    match term {
        Term::Var(x) => Term::Var(format!("{x}{suffix}")),
        Term::App(f, args) => Term::App(
            f.clone(),
            args.iter().map(|a| rename_term(a, suffix)).collect(),
        ),
    }
}

/// Prove `goal` (close every literal) from `clauses` by **iterative deepening** on
/// the per-branch extension-depth bound, from `0` up to and including `max_bound`.
/// Returns the first (shallowest) [`Proof`], or `None` if no proof exists within
/// `max_bound`. The continuation cache is enabled.
pub fn prove(clauses: &[Clause], goal: &[Literal], max_bound: usize) -> Option<Proof> {
    for depth in 0..=max_bound {
        let mut search = Search::new(clauses, true);
        if let Some(steps) = search.run(goal, depth) {
            return Some(Proof { steps, depth });
        }
    }
    None
}

/// Refute an (allegedly unsatisfiable) clause set: iterative-deepen as in [`prove`],
/// trying each clause in turn as the start chain. Returns the first refutation.
pub fn refute(clauses: &[Clause], max_bound: usize) -> Option<Proof> {
    for depth in 0..=max_bound {
        for start in clauses {
            let mut search = Search::new(clauses, true);
            if let Some(steps) = search.run(&start.literals, depth) {
                return Some(Proof { steps, depth });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- readable term / literal constructors ----
    fn v(name: &str) -> Term {
        Term::var(name)
    }
    fn c(name: &str) -> Term {
        Term::constant(name)
    }
    fn app(sym: &str, args: Vec<Term>) -> Term {
        Term::app(sym, args)
    }
    /// A positive/negative unary atom `pred(arg)`.
    fn pos1(pred: &str, arg: Term) -> Literal {
        Literal::pos(app(pred, vec![arg]))
    }
    fn neg1(pred: &str, arg: Term) -> Literal {
        Literal::neg(app(pred, vec![arg]))
    }

    /// The classic unsatisfiable set: { P(a) }, { ¬P(x) ∨ Q(x) }, { ¬Q(a) }.
    fn pq_clauses() -> Vec<Clause> {
        vec![
            Clause::new(vec![pos1("P", c("a"))]),
            Clause::new(vec![neg1("P", v("x")), pos1("Q", v("x"))]),
            Clause::new(vec![neg1("Q", c("a"))]),
        ]
    }

    #[test]
    fn refutes_small_unsatisfiable_set_within_a_bound() {
        // Starting from the goal literal P(a): extend with ¬P(x)∨Q(x) (x↦a) to get
        // the subgoal Q(a), then extend with ¬Q(a) to close. A refutation exists
        // within a generous bound.
        let clauses = pq_clauses();
        let goal = [pos1("P", c("a"))];
        let proof = prove(&clauses, &goal, 5).expect("the set is unsatisfiable");
        assert_eq!(proof.steps.len(), 2, "two extension steps close the chain");
        assert!(matches!(proof.steps[0], ProofStep::Extension { clause: 1, .. }));
        assert!(matches!(proof.steps[1], ProofStep::Extension { clause: 2, .. }));

        // `refute` finds it too, starting from a clause of the set.
        assert!(refute(&clauses, 5).is_some());
    }

    #[test]
    fn iterative_deepening_finds_what_a_too_small_bound_misses() {
        let clauses = pq_clauses();
        let goal = [pos1("P", c("a"))];
        // The proof needs two extension steps along the P(a)→Q(a) branch.
        assert!(
            prove(&clauses, &goal, 1).is_none(),
            "a depth-1 bound cannot close the two-step chain"
        );
        let proof = prove(&clauses, &goal, 2).expect("depth 2 suffices");
        assert_eq!(proof.depth, 2, "deepening stops at the first depth that works");
        // A larger ceiling still returns the shallowest proof (depth 2).
        assert_eq!(prove(&clauses, &goal, 9).unwrap().depth, 2);
    }

    /// A set with a subgoal `Common` reached via two distinct super-goals A and B,
    /// so it is a candidate for re-expansion (and thus for the cache):
    ///   goal Top; Top ⇒ A ∧ B; A ⇒ Common; B ⇒ Common; Common ⇒ Deep; Deep closes.
    fn shared_subgoal_clauses() -> Vec<Clause> {
        vec![
            Clause::new(vec![
                neg1("Top", c("o")),
                pos1("A", c("o")),
                pos1("B", c("o")),
            ]),
            Clause::new(vec![neg1("A", c("o")), pos1("Common", c("o"))]),
            Clause::new(vec![neg1("B", c("o")), pos1("Common", c("o"))]),
            Clause::new(vec![neg1("Common", c("o")), pos1("Deep", c("o"))]),
            Clause::new(vec![neg1("Deep", c("o"))]),
        ]
    }

    #[test]
    fn cache_avoids_reexpanding_a_repeated_subgoal() {
        let clauses = shared_subgoal_clauses();
        let goal = [pos1("Top", c("o"))];
        let budget = 6; // enough for the Top→A→Common→Deep branch (depth 4)

        let mut cached = Search::new(&clauses, true);
        let with_cache = cached.run(&goal, budget);

        let mut plain = Search::new(&clauses, false);
        let without_cache = plain.run(&goal, budget);

        // Same result either way — the cache is a pure optimization.
        assert!(with_cache.is_some(), "the set must be refutable");
        assert!(without_cache.is_some());

        // The cache genuinely fired and saved expansions.
        assert!(cached.cache_hits() >= 1, "the repeated `Common` must be a hit");
        assert!(
            cached.expansions() < plain.expansions(),
            "caching must reduce expansions: cached={}, uncached={}",
            cached.expansions(),
            plain.expansions()
        );
    }

    #[test]
    fn occurs_check_is_respected() {
        // ¬Eq(x,x) is the only clause. Closing Eq(z, f(z)) would need z ↦ f(z),
        // which the occurs-check in `unify` rejects, so there is no proof.
        let clauses = vec![Clause::new(vec![Literal::neg(app("Eq", vec![v("x"), v("x")]))])];
        let cyclic = [Literal::pos(app("Eq", vec![v("z"), app("f", vec![v("z")])]))];
        assert!(
            prove(&clauses, &cyclic, 5).is_none(),
            "x = f(x) must fail the occurs-check ⇒ no refutation"
        );
        // But the sound instance Eq(a,a) closes immediately.
        let reflexive = [Literal::pos(app("Eq", vec![c("a"), c("a")]))];
        assert!(prove(&clauses, &reflexive, 5).is_some());
    }

    #[test]
    fn search_is_deterministic() {
        let clauses = pq_clauses();
        let goal = [pos1("P", c("a"))];
        let p1 = prove(&clauses, &goal, 5);
        let p2 = prove(&clauses, &goal, 5);
        assert_eq!(p1, p2, "identical inputs must give byte-identical proofs");

        // The shared-subgoal refutation is deterministic too.
        let shared = shared_subgoal_clauses();
        let g = [pos1("Top", c("o"))];
        assert_eq!(prove(&shared, &g, 6), prove(&shared, &g, 6));
    }

    #[test]
    fn unprovable_goal_returns_none_within_bound() {
        // No clause can close R(a): the search exhausts the bound without a proof.
        let clauses = pq_clauses();
        let goal = [pos1("R", c("a"))];
        assert!(prove(&clauses, &goal, 4).is_none());
    }
}
