//! Growing verified-lemma library + evolver (the LEGO-Prover pattern).
//!
//! LEGO-Prover treats proving as *skill accumulation*: every verified lemma is
//! an admitted, reusable skill; open sub-goals become *requests* that double as
//! retrieval queries and an evolver worklist; and a background *evolver* keeps
//! generalising existing skills along four axes so the library grows richer than
//! any single problem required. This module ports that organ to the Rust core,
//! backed by [`Store`] via three logical tables (lemma / request / problem
//! stores added to `graph::db`).
//!
//! Design seams (all injected, so the whole thing runs offline & deterministic):
//! * a [`VerifierFn`] — in production our 3+1 gate, in tests a pattern mock —
//!   decides whether a `(statement, proof)` is admissible;
//! * a [`DedupFn`] — **defaults to exact string equality** and is deliberately
//!   shaped `Fn(candidate, existing) -> is_duplicate` so an external
//!   subsumption-based deduper can be dropped in at integration without touching
//!   this module. Because subsumption is asymmetric, the deduper is consulted in
//!   both directions by the retention rule in [`retained_and_dropped`]: the more
//!   general of two verified lemmas is kept and the more specific one is dropped
//!   with a [`LemmaDrop`] record naming its subsumer;
//! * an [`Evolver`] — proposes generalisations of a lemma along the four
//!   [`EvolveDirection`] axes (and, optionally, solves a request).
//!
//! Determinism: the k-NN retrieval uses a lightweight in-house embedding (a
//! signed hashing-trick token vector with FNV-1a — no RNG, no `DefaultHasher`
//! randomised seeds, no network, no external deps) and cosine similarity, with a
//! stable id tie-break. Every scheduler pick is least-`update_count` then
//! oldest-created then id. All lemma/proof/subgoal text is untrusted data: it is
//! only ever stored and fed to the injected verifier — never executed here.

use crate::db::Store;
use anyhow::Result;

// Re-export the persisted records as this module's domain vocabulary. `Lemma`
// is the LEGO "skill"; requests/problems are the other two stores.
pub use crate::db::{LibraryLemma as Lemma, LibraryProblem as Problem, LibraryRequest};

/// Dimensionality of the hashing-trick embedding used for k-NN retrieval.
pub const EMBED_DIM: usize = 64;

/// The injected verifier: `(statement, proof) -> admitted`. In production this
/// is the 3+1 gate; in tests a deterministic pattern mock.
pub type VerifierFn = Box<dyn Fn(&str, &str) -> bool>;

/// The injected deduper: `(candidate_statement, existing_statement) ->
/// is_duplicate`. Defaults to exact string equality (see
/// [`LemmaLibrary::new`]); shaped to accept an external subsumption impl.
///
/// Read it as the *relation* "the second argument subsumes the first", because
/// the retention rule ([`retained_and_dropped`]) calls it in **both** directions
/// to tell a strictly-more-general lemma from a strictly-more-specific one. It
/// must therefore be transitive, and it must not be symmetrised: an
/// implementation that returns the same answer whichever way round it is called
/// (as exact equality does) simply never reports supersession, which is the
/// correct, conservative behaviour rather than a wrong one.
pub type DedupFn = Box<dyn Fn(&str, &str) -> bool>;

/// The four axes along which the evolver generalises a skill (LEGO-Prover).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvolveDirection {
    /// Turn a concrete constant into a universally-quantified parameter.
    Parameterize,
    /// Surface the key concepts / abstract the shared structure.
    IdentifyKeyConcepts,
    /// Push the same shape to a harder / larger instance.
    ScaleComplexity,
    /// Lift the statement into a higher-arity / multi-dimensional form.
    ExtendDimensions,
}

impl EvolveDirection {
    /// All four directions, in a fixed order (deterministic iteration).
    pub const ALL: [EvolveDirection; 4] = [
        EvolveDirection::Parameterize,
        EvolveDirection::IdentifyKeyConcepts,
        EvolveDirection::ScaleComplexity,
        EvolveDirection::ExtendDimensions,
    ];
}

/// A candidate skill an [`Evolver`] proposes. Directly spliceable —
/// `statement` + `proof` are the same shape the lemma store admits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedLemma {
    pub statement: String,
    pub proof: String,
    pub provenance: String,
}

/// Proposes generalisations of admitted skills (and, optionally, solves an open
/// request). Injected, so the evolve loop is exercised with a deterministic mock.
pub trait Evolver {
    /// Propose a generalisation of `lemma` along `direction`, or `None` if this
    /// direction does not apply to this lemma.
    fn transform(&self, lemma: &Lemma, direction: EvolveDirection)
        -> Result<Option<ProposedLemma>>;

    /// Attempt to discharge an open request's `subgoal` directly, yielding a
    /// candidate skill. Defaults to "no attempt" so a `transform`-only evolver
    /// stays valid; a request-capable evolver overrides it.
    fn solve_request(&self, _subgoal: &str) -> Result<Option<ProposedLemma>> {
        Ok(None)
    }
}

/// The outcome of trying to admit a candidate into the lemma store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdmitOutcome {
    /// Passed the verifier and was not a duplicate — inserted.
    Admitted,
    /// Passed the verifier but an existing lemma already subsumes it.
    Duplicate,
    /// Failed the verifier.
    Rejected,
}

/// A lemma the retention rule dropped, together with the lemma that displaced it.
///
/// Every drop is materialised as one of these so the library can explain its own
/// contents: a lemma that is in the store but not in [`LemmaLibrary::lemmas`]
/// always has a matching record here naming its subsumer. A library that
/// silently discards cannot be debugged, so nothing is dropped without a record.
// No `Eq`: `Lemma` (db::LibraryLemma) is only `PartialEq`.
#[derive(Debug, Clone, PartialEq)]
pub struct LemmaDrop {
    /// The lemma that lost. Still present in the underlying store (the store has
    /// no delete), just not part of the retained library view.
    pub dropped: Lemma,
    /// Id of the lemma that subsumed it.
    pub subsumed_by_id: String,
    /// Statement of the lemma that subsumed it, denormalised so a drop record is
    /// readable on its own without a second store round-trip.
    pub subsumed_by_statement: String,
}

/// The tally an [`LemmaLibrary::evolve_round`] returns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EvolveSummary {
    /// Candidates the evolver produced this round.
    pub n_proposed: usize,
    /// Candidates admitted (verifier-passed, non-duplicate).
    pub n_admitted: usize,
    /// Candidates rejected as duplicates of an existing lemma.
    pub n_deduped: usize,
}

/// The growing verified-lemma library. Holds a borrowed [`Store`] plus the
/// injected verifier and deduper used by [`record_lemma`](Self::record_lemma).
pub struct LemmaLibrary<'a> {
    store: &'a Store,
    verifier: VerifierFn,
    dedup: DedupFn,
}

impl<'a> LemmaLibrary<'a> {
    /// Wrap a store with an injected verifier and deduper. Prefer this when the
    /// caller wants a custom (e.g. subsumption-based) deduper.
    pub fn new(store: &'a Store, verifier: VerifierFn, dedup: DedupFn) -> Self {
        Self {
            store,
            verifier,
            dedup,
        }
    }

    /// Wrap a store with an injected verifier and the **default deduper: exact
    /// string equality**. Swap to [`new`](Self::new) with a subsumption deduper
    /// when one is available — the injection shape is identical.
    pub fn with_exact_dedup(store: &'a Store, verifier: VerifierFn) -> Self {
        Self::new(store, verifier, Box::new(|a, b| a == b))
    }

    /// Wrap a store with an injected verifier and the **subsumption deduper**
    /// (`search::subsumption`): a candidate lemma is a duplicate when an existing
    /// lemma subsumes it (α-equivalent, hypothesis-reordered, or strictly more
    /// general). This is the production deduper — it dominates exact-equality by
    /// also collapsing renamed/reordered/weaker restatements of the same skill.
    ///
    /// It is also asymmetric, so with this deduper the retention rule is live in
    /// both directions: a candidate that is strictly more general than a stored
    /// lemma is admitted and *displaces* it (see [`retained_and_dropped`]).
    pub fn with_subsumption_dedup(store: &'a Store, verifier: VerifierFn) -> Self {
        Self::new(
            store,
            verifier,
            // dedup(candidate, existing) => existing subsumes candidate.
            Box::new(|candidate, existing| crate::subsumption::subsumes_str(existing, candidate)),
        )
    }

    // --- lemma store ------------------------------------------------------

    /// Admit `(statement, proof)` as a skill iff it passes the injected
    /// verifier AND no existing lemma is a duplicate/subsumer. Returns whether
    /// it was admitted.
    pub fn record_lemma(
        &self,
        project_id: &str,
        statement: &str,
        proof: &str,
        provenance: &str,
    ) -> Result<bool> {
        let outcome = self.try_admit(
            project_id,
            statement,
            proof,
            provenance,
            self.verifier.as_ref(),
            self.dedup.as_ref(),
        )?;
        Ok(outcome == AdmitOutcome::Admitted)
    }

    /// The shared admission gate: verify, then dedup against every *retained*
    /// existing lemma (including ones admitted earlier this round), then insert.
    /// Uses the explicitly-passed verifier/deduper so both `record_lemma` (stored
    /// injections) and `evolve_round` (per-call injections) share one policy.
    ///
    /// Only the *forward* direction is decided here (an existing lemma subsumes
    /// the candidate, so the candidate adds nothing). The backward direction (the
    /// candidate subsumes an existing lemma) is deliberately *not* decided here:
    /// it is settled by the retention rule in [`retained_and_dropped`], which
    /// recomputes the retained view from the stored set on every read. Keeping
    /// one rule in one place means admission order can never leave the library in
    /// a state the rule would not have produced.
    fn try_admit(
        &self,
        project_id: &str,
        statement: &str,
        proof: &str,
        provenance: &str,
        verifier: &dyn Fn(&str, &str) -> bool,
        dedup: &dyn Fn(&str, &str) -> bool,
    ) -> Result<AdmitOutcome> {
        if !verifier(statement, proof) {
            return Ok(AdmitOutcome::Rejected);
        }
        // Compare against the retained view, not the raw store: an already-dropped
        // lemma must not be able to block a new candidate. That is safe because
        // the dedup relations we support are transitive, so whatever subsumed the
        // dropped lemma also subsumes anything the dropped lemma would subsume.
        for existing in self.retained_lemmas(project_id, dedup)? {
            if dedup(statement, &existing.statement) {
                return Ok(AdmitOutcome::Duplicate);
            }
        }
        self.store.add_library_lemma(
            project_id,
            statement,
            proof,
            provenance,
            &embedding_key(statement),
        )?;
        Ok(AdmitOutcome::Admitted)
    }

    /// The library's contents: every admitted lemma the retention rule keeps.
    ///
    /// Lemmas a more general lemma has displaced are excluded here and surface in
    /// [`drops`](Self::drops) instead; [`all_lemmas`](Self::all_lemmas) is the
    /// unfiltered store view.
    pub fn lemmas(&self, project_id: &str) -> Result<Vec<Lemma>> {
        self.retained_lemmas(project_id, self.dedup.as_ref())
    }

    /// Every lemma ever admitted, retained or dropped, in store order. Use this
    /// to audit; use [`lemmas`](Self::lemmas) as the library.
    pub fn all_lemmas(&self, project_id: &str) -> Result<Vec<Lemma>> {
        self.store.library_lemmas(project_id)
    }

    /// The drop ledger: every lemma the retention rule dropped, each naming the
    /// lemma that subsumed it. `lemmas() + drops()` partitions `all_lemmas()`.
    pub fn drops(&self, project_id: &str) -> Result<Vec<LemmaDrop>> {
        Ok(self.partition(project_id, self.dedup.as_ref())?.1)
    }

    /// The retained view under an explicitly-passed deduper (so `evolve_round`,
    /// which takes a per-call deduper, applies the same rule it was handed).
    fn retained_lemmas(
        &self,
        project_id: &str,
        dedup: &dyn Fn(&str, &str) -> bool,
    ) -> Result<Vec<Lemma>> {
        Ok(self.partition(project_id, dedup)?.0)
    }

    /// Recompute `(retained, dropped)` from the store under `dedup`.
    ///
    /// Recomputing rather than persisting a "retired" flag is what makes the
    /// library reproducible: the retained set is a pure function of the stored
    /// lemmas and the injected relation, so it cannot drift from the rule and
    /// does not depend on the order calls happened to arrive in.
    fn partition(
        &self,
        project_id: &str,
        dedup: &dyn Fn(&str, &str) -> bool,
    ) -> Result<(Vec<Lemma>, Vec<LemmaDrop>)> {
        Ok(retained_and_dropped(
            self.store.library_lemmas(project_id)?,
            dedup,
        ))
    }

    /// k-NN retrieval over the lemma store: rank every lemma by cosine
    /// similarity of its statement-embedding to `query`'s, best-first, and
    /// return the top `k`. The returned `(statement, proof)` pairs are directly
    /// spliceable. Deterministic (stable id tie-break).
    pub fn retrieve(&self, project_id: &str, query: &str, k: usize) -> Result<Vec<Lemma>> {
        let qv = embed(query);
        // Retrieval serves the retained view only: answering a query with a
        // lemma that a more general one has displaced is exactly the redundancy
        // the retention rule exists to remove.
        let mut scored: Vec<(f64, Lemma)> = self
            .lemmas(project_id)?
            .into_iter()
            .map(|l| (cosine(&embed(&l.statement), &qv), l))
            .collect();
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.id.cmp(&b.1.id))
        });
        Ok(scored.into_iter().take(k).map(|(_, l)| l).collect())
    }

    /// The evolver scheduler pick: the least-`update_count` retained lemma, ties
    /// broken oldest-created then id. Dropped lemmas are skipped because
    /// evolving a displaced statement spends the budget re-deriving what its
    /// subsumer already covers.
    pub fn next_to_evolve(&self, project_id: &str) -> Result<Option<Lemma>> {
        let mut lemmas = self.lemmas(project_id)?;
        lemmas.sort_by(|a, b| {
            a.update_count
                .cmp(&b.update_count)
                .then_with(|| a.created_at.cmp(&b.created_at))
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(lemmas.into_iter().next())
    }

    // --- request store ----------------------------------------------------

    /// Enqueue an open hole (from a sketch) as a request — both a retrieval
    /// query and an evolver worklist item.
    pub fn enqueue_request(
        &self,
        project_id: &str,
        subgoal: &str,
        provenance: &str,
    ) -> Result<LibraryRequest> {
        self.store
            .add_library_request(project_id, subgoal, provenance)
    }

    /// All requests for a project.
    pub fn requests(&self, project_id: &str) -> Result<Vec<LibraryRequest>> {
        self.store.library_requests(project_id)
    }

    /// The oldest still-open request (least `update_count`, then oldest-created).
    pub fn next_open_request(&self, project_id: &str) -> Result<Option<LibraryRequest>> {
        self.store.oldest_open_library_request(project_id)
    }

    // --- problem store ----------------------------------------------------

    /// Record a target statement that biases evolution.
    pub fn add_problem(
        &self,
        project_id: &str,
        statement: &str,
        provenance: &str,
    ) -> Result<Problem> {
        self.store
            .add_library_problem(project_id, statement, provenance)
    }

    /// All target problems for a project.
    pub fn problems(&self, project_id: &str) -> Result<Vec<Problem>> {
        self.store.library_problems(project_id)
    }

    // --- the evolver loop -------------------------------------------------

    /// One evolution round (LEGO-Prover's growth step): pop the least-updated
    /// lemma, run the four directional transforms, then request-solve the oldest
    /// open request; verify + dedup-admit every proposal; bump the `update_count`
    /// of both the evolved lemma and the worked request. `verifier`/`dedup` are
    /// passed explicitly so an integrator can vary policy per round (a
    /// subsumption deduper slots straight in here). Returns the round tally.
    pub fn evolve_round(
        &self,
        project_id: &str,
        verifier: &dyn Fn(&str, &str) -> bool,
        evolver: &dyn Evolver,
        dedup: &dyn Fn(&str, &str) -> bool,
    ) -> Result<EvolveSummary> {
        let mut summary = EvolveSummary::default();

        // 1. Generalise the least-updated skill along all four axes.
        if let Some(lemma) = self.next_to_evolve(project_id)? {
            for direction in EvolveDirection::ALL {
                if let Some(proposed) = evolver.transform(&lemma, direction)? {
                    summary.n_proposed += 1;
                    self.tally_admit(project_id, &proposed, verifier, dedup, &mut summary)?;
                }
            }
            self.store
                .bump_library_lemma_update(project_id, &lemma.id)?;
        }

        // 2. Attempt the oldest open request; on admission, mark it solved.
        if let Some(request) = self.next_open_request(project_id)? {
            if let Some(proposed) = evolver.solve_request(&request.subgoal)? {
                summary.n_proposed += 1;
                let outcome = self.try_admit(
                    project_id,
                    &proposed.statement,
                    &proposed.proof,
                    &proposed.provenance,
                    verifier,
                    dedup,
                )?;
                match outcome {
                    AdmitOutcome::Admitted => {
                        summary.n_admitted += 1;
                        self.store
                            .mark_library_request_solved(project_id, &request.id)?;
                    }
                    AdmitOutcome::Duplicate => summary.n_deduped += 1,
                    AdmitOutcome::Rejected => {}
                }
            }
            self.store
                .bump_library_request_update(project_id, &request.id)?;
        }

        Ok(summary)
    }

    /// Admit a proposal and fold the outcome into the running summary.
    fn tally_admit(
        &self,
        project_id: &str,
        proposed: &ProposedLemma,
        verifier: &dyn Fn(&str, &str) -> bool,
        dedup: &dyn Fn(&str, &str) -> bool,
        summary: &mut EvolveSummary,
    ) -> Result<()> {
        match self.try_admit(
            project_id,
            &proposed.statement,
            &proposed.proof,
            &proposed.provenance,
            verifier,
            dedup,
        )? {
            AdmitOutcome::Admitted => summary.n_admitted += 1,
            AdmitOutcome::Duplicate => summary.n_deduped += 1,
            AdmitOutcome::Rejected => {}
        }
        Ok(())
    }
}

// --- the retention rule ----------------------------------------------------

/// Split `lemmas` (in store order, oldest first) into the retained library and
/// the drop ledger under the dedup relation `dedup(a, b) == "b subsumes a"`.
///
/// # The retention rule
///
/// For two already-verified lemmas `A` and `B`, with
/// `a_sub_b = dedup(b, a)` ("A subsumes B") and `b_sub_a = dedup(a, b)`:
///
/// * `a_sub_b && !b_sub_a`: A is strictly more general. **Keep A, drop B.**
/// * `b_sub_a && !a_sub_b`: B is strictly more general. **Keep B, drop A.**
/// * both: mutual subsumption (α-equivalent / hypothesis-reordered restatements
///   of the same skill). **Keep the incumbent**, i.e. the one that reached the
///   store first (oldest `created_at`, id as tie-break); drop the newcomer.
/// * neither: unrelated. **Keep both.**
///
/// ## Why keeping the more general one is the sound direction
///
/// Subsumption is asymmetric: a proof of a more general statement discharges a
/// more specific one, never the reverse. If A subsumes B, then every query B
/// legitimately answers is also answered by A, so dropping B loses no reachable
/// conclusion. Dropping A instead would leave the library answering a query that
/// matches A with B, whose extra hypotheses (or narrower conclusion) were never
/// established for that query. That is unsound, not untidy, which is why the
/// direction is spelled out here rather than left implicit in a comparison.
///
/// Graduation is untouched: both lemmas in every pair above already passed the
/// verifier. This rule only chooses which of two verified lemmas the library
/// keeps.
///
/// ## Why oldest-wins for mutual subsumption
///
/// Mutual subsumption means the two are interchangeable, so soundness does not
/// pick for us and something else must, deterministically. The incumbent wins:
/// it is the one downstream retrieval may already have cited, it carries the
/// accumulated `update_count` the evolver scheduler reads, and "oldest wins" is
/// stable under replay of the same admission sequence. A rule like "prefer the
/// shorter statement" would instead let an unrelated edit reshuffle the library.
///
/// The dropped lemma is never lost silently: each drop yields a [`LemmaDrop`]
/// naming its subsumer. Chains are reported honestly, so if C displaced B and B
/// had already displaced A, the ledger holds both `A <- B` and `B <- C`.
///
/// Cost is O(n^2) `dedup` calls; the lemma store is small by construction (it
/// holds distinct skills, not attempts), and recomputation buys reproducibility.
pub fn retained_and_dropped(
    lemmas: Vec<Lemma>,
    dedup: &dyn Fn(&str, &str) -> bool,
) -> (Vec<Lemma>, Vec<LemmaDrop>) {
    let mut retained: Vec<Lemma> = Vec::new();
    let mut drops: Vec<LemmaDrop> = Vec::new();

    for lemma in lemmas {
        // Index of a retained lemma that subsumes this one, and the retained
        // lemmas this one strictly supersedes.
        let mut subsumer: Option<usize> = None;
        let mut superseded: Vec<usize> = Vec::new();
        for (i, r) in retained.iter().enumerate() {
            if dedup(&lemma.statement, &r.statement) {
                // r subsumes lemma. Checked first, so the mutual case resolves
                // in favour of r, the incumbent.
                subsumer = Some(i);
                break;
            }
            if dedup(&r.statement, &lemma.statement) {
                superseded.push(i);
            }
        }

        match subsumer {
            Some(i) => drops.push(LemmaDrop {
                subsumed_by_id: retained[i].id.clone(),
                subsumed_by_statement: retained[i].statement.clone(),
                dropped: lemma,
            }),
            None => {
                // Remove back-to-front so the earlier indices stay valid.
                for i in superseded.into_iter().rev() {
                    let loser = retained.remove(i);
                    drops.push(LemmaDrop {
                        subsumed_by_id: lemma.id.clone(),
                        subsumed_by_statement: lemma.statement.clone(),
                        dropped: loser,
                    });
                }
                retained.push(lemma);
            }
        }
    }

    // Ledger order is the dropped lemmas' own store order, not the order the
    // scan happened to remove them in, so the ledger reads the same every time.
    drops.sort_by(|a, b| {
        a.dropped
            .created_at
            .cmp(&b.dropped.created_at)
            .then_with(|| a.dropped.id.cmp(&b.dropped.id))
    });
    (retained, drops)
}

// --- deterministic hashing-trick embedding --------------------------------

/// A stable, deterministic fingerprint of a statement, stored alongside a lemma.
/// The retrieval vector is recomputed from the statement at query time; this key
/// is a compact audit handle (sorted-token FNV-1a hash).
pub fn embedding_key(statement: &str) -> String {
    let mut tokens = tokenize(statement);
    tokens.sort();
    format!("emb1:{:016x}", fnv1a(&tokens.join(" ")))
}

/// Split into lowercased alphanumeric tokens (everything else is a separator).
fn tokenize(s: &str) -> Vec<String> {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

/// FNV-1a 64-bit — a fixed, deterministic hash (unlike `DefaultHasher`, which is
/// seeded randomly per process).
fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Signed hashing-trick embedding: each token lands in one of [`EMBED_DIM`]
/// buckets with a sign bit, so distinct tokens rarely cancel. Deterministic.
fn embed(statement: &str) -> [f64; EMBED_DIM] {
    let mut v = [0.0f64; EMBED_DIM];
    for token in tokenize(statement) {
        let h = fnv1a(&token);
        let bucket = (h % EMBED_DIM as u64) as usize;
        let sign = if (h >> 63) & 1 == 1 { -1.0 } else { 1.0 };
        v[bucket] += sign;
    }
    v
}

/// Cosine similarity in `[-1, 1]`; `0.0` when either side is the zero vector.
fn cosine(a: &[f64], b: &[f64]) -> f64 {
    let dot: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f64 = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let nb: f64 = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sketch::{InformalSketch, SketchStep};
    use std::path::Path;

    fn store_with_project() -> (Store, String) {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let p = store.create_project("p", "t").unwrap();
        (store, p.id)
    }

    /// Verifier mock: a proof is admissible iff it contains the token `by`
    /// (stands in for "the 3+1 gate passed").
    fn pattern_verifier() -> VerifierFn {
        Box::new(|_stmt: &str, proof: &str| proof.contains("by"))
    }

    /// A deterministic evolver: each direction prefixes the statement with a
    /// fixed generalisation tag, producing a distinct, verifier-passing skill.
    /// `solve_request` closes any subgoal with a canned proof.
    struct MockEvolver;
    impl Evolver for MockEvolver {
        fn transform(
            &self,
            lemma: &Lemma,
            direction: EvolveDirection,
        ) -> Result<Option<ProposedLemma>> {
            let tag = match direction {
                EvolveDirection::Parameterize => "forall n,",
                EvolveDirection::IdentifyKeyConcepts => "key:",
                EvolveDirection::ScaleComplexity => "scaled:",
                EvolveDirection::ExtendDimensions => "nd:",
            };
            Ok(Some(ProposedLemma {
                statement: format!("{tag} {}", lemma.statement),
                proof: format!("{} by evolve", lemma.proof),
                provenance: "evolver".to_owned(),
            }))
        }

        fn solve_request(&self, subgoal: &str) -> Result<Option<ProposedLemma>> {
            Ok(Some(ProposedLemma {
                statement: subgoal.to_owned(),
                proof: "solved by evolve".to_owned(),
                provenance: "evolver:request".to_owned(),
            }))
        }
    }

    #[test]
    fn subsumption_dedup_collapses_reordered_and_more_specific_restatements() {
        // The production deduper (search::subsumption) must reject a lemma that an
        // existing one subsumes — restatements that exact-equality would wrongly
        // admit. This is the Unit-1 → Unit-2 splice, demonstrated end to end.
        let (store, pid) = store_with_project();
        let lib = LemmaLibrary::with_subsumption_dedup(&store, pattern_verifier());

        // General lemma admitted.
        assert!(lib
            .record_lemma(&pid, "P x ⊢ R z", "by tac", "seed")
            .unwrap());
        // A strictly MORE SPECIFIC restatement (extra hypothesis) is subsumed →
        // rejected. Exact-equality dedup would have (wrongly) admitted it.
        assert!(!lib
            .record_lemma(&pid, "Q y, P x ⊢ R z", "by tac", "seed2")
            .unwrap());
        // An α-equivalent restatement of a fresh lemma is also collapsed.
        assert!(lib
            .record_lemma(&pid, "⊢ ∀ x, P x", "by intro", "s3")
            .unwrap());
        assert!(!lib
            .record_lemma(&pid, "⊢ ∀ y, P y", "by intro", "s4")
            .unwrap());
        // Exactly the two genuinely-distinct lemmas survived.
        assert_eq!(lib.lemmas(&pid).unwrap().len(), 2);
    }

    #[test]
    fn general_lemma_displaces_the_specific_one_it_subsumes() {
        // Backward direction: the specific lemma lands first, then a strictly
        // more general one arrives. The general one is admitted AND the specific
        // one leaves the library, because every query the specific one answered
        // is answered by the general one.
        let (store, pid) = store_with_project();
        let lib = LemmaLibrary::with_subsumption_dedup(&store, pattern_verifier());

        assert!(lib
            .record_lemma(&pid, "P x, Q y ⊢ R z", "by tac", "specific")
            .unwrap());
        assert!(lib
            .record_lemma(&pid, "P x ⊢ R z", "by tac", "general")
            .unwrap());

        let kept = lib.lemmas(&pid).unwrap();
        assert_eq!(kept.len(), 1, "only the general lemma is retained");
        assert_eq!(kept[0].statement, "P x ⊢ R z");
        // The store still holds both; it is the library *view* that narrowed.
        assert_eq!(lib.all_lemmas(&pid).unwrap().len(), 2);
    }

    #[test]
    fn specific_lemma_never_displaces_the_general_one() {
        // The asymmetry, and the soundness test. Same pair as above in the other
        // order: the specific lemma must not be admitted, and must certainly not
        // evict the general lemma. Answering a query that matches "P x ⊢ R z"
        // with "P x, Q y ⊢ R z" would assume Q y, which was never established.
        let (store, pid) = store_with_project();
        let lib = LemmaLibrary::with_subsumption_dedup(&store, pattern_verifier());

        assert!(lib
            .record_lemma(&pid, "P x ⊢ R z", "by tac", "general")
            .unwrap());
        assert!(
            !lib.record_lemma(&pid, "P x, Q y ⊢ R z", "by tac", "specific")
                .unwrap(),
            "a more specific lemma adds nothing and must not be admitted"
        );

        let kept = lib.lemmas(&pid).unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(
            kept[0].statement, "P x ⊢ R z",
            "the general lemma must survive"
        );
        assert_eq!(
            kept[0].provenance, "general",
            "the surviving lemma is the original general one, not a replacement"
        );
    }

    #[test]
    fn mutual_subsumption_resolves_to_the_incumbent() {
        // α-equivalent restatements subsume each other, so soundness does not
        // pick a winner and the documented tie-break must: oldest wins.
        let (store, pid) = store_with_project();
        let lib = LemmaLibrary::with_subsumption_dedup(&store, pattern_verifier());

        assert!(lib
            .record_lemma(&pid, "⊢ ∀ x, P x", "by intro", "first")
            .unwrap());
        assert!(!lib
            .record_lemma(&pid, "⊢ ∀ y, P y", "by intro", "second")
            .unwrap());

        let kept = lib.lemmas(&pid).unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].provenance, "first", "the incumbent wins ties");

        // And the same input in the opposite order keeps the other statement:
        // the rule is order-stable, not statement-preferential.
        let (store2, pid2) = store_with_project();
        let lib2 = LemmaLibrary::with_subsumption_dedup(&store2, pattern_verifier());
        lib2.record_lemma(&pid2, "⊢ ∀ y, P y", "by intro", "first")
            .unwrap();
        lib2.record_lemma(&pid2, "⊢ ∀ x, P x", "by intro", "second")
            .unwrap();
        let kept2 = lib2.lemmas(&pid2).unwrap();
        assert_eq!(kept2.len(), 1);
        assert_eq!(kept2[0].statement, "⊢ ∀ y, P y");
    }

    #[test]
    fn dropped_lemmas_are_recorded_with_their_subsumer() {
        // Nothing is lost silently: both a rejected-at-admission drop and a
        // displaced-after-the-fact drop appear in the ledger with a subsumer.
        let (store, pid) = store_with_project();
        let lib = LemmaLibrary::with_subsumption_dedup(&store, pattern_verifier());

        lib.record_lemma(&pid, "P x, Q y ⊢ R z", "by tac", "specific")
            .unwrap();
        lib.record_lemma(&pid, "P x ⊢ R z", "by tac", "general")
            .unwrap();

        let drops = lib.drops(&pid).unwrap();
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].dropped.statement, "P x, Q y ⊢ R z");
        assert_eq!(drops[0].subsumed_by_statement, "P x ⊢ R z");
        let general = lib.lemmas(&pid).unwrap()[0].clone();
        assert_eq!(drops[0].subsumed_by_id, general.id);

        // retained + dropped partitions the store: nothing falls off the edge.
        assert_eq!(
            lib.lemmas(&pid).unwrap().len() + drops.len(),
            lib.all_lemmas(&pid).unwrap().len()
        );
    }

    #[test]
    fn a_chain_of_generalisations_leaves_one_survivor_and_an_explained_ledger() {
        // Increasingly general lemmas arrive in turn. Each is admitted (nothing
        // stored subsumes it yet) and displaces its predecessor, so the library
        // converges on the most general one while the ledger records the chain.
        let (store, pid) = store_with_project();
        let lib = LemmaLibrary::with_subsumption_dedup(&store, pattern_verifier());

        assert!(lib
            .record_lemma(&pid, "A, B, C ⊢ R", "by tac", "s0")
            .unwrap());
        assert!(lib.record_lemma(&pid, "A, B ⊢ R", "by tac", "s1").unwrap());
        assert!(lib.record_lemma(&pid, "A ⊢ R", "by tac", "s2").unwrap());

        let kept = lib.lemmas(&pid).unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].statement, "A ⊢ R");

        // Each loser names the lemma that displaced it, so following
        // `subsumed_by` from any dropped lemma terminates at a retained one.
        let drops = lib.drops(&pid).unwrap();
        assert_eq!(drops.len(), 2);
        assert_eq!(drops[0].dropped.statement, "A, B, C ⊢ R");
        assert_eq!(drops[0].subsumed_by_statement, "A, B ⊢ R");
        assert_eq!(drops[1].dropped.statement, "A, B ⊢ R");
        assert_eq!(drops[1].subsumed_by_statement, "A ⊢ R");
    }

    #[test]
    fn unrelated_lemmas_both_survive() {
        // Different conclusions: neither subsumes the other, so the rule must
        // keep both. A retention rule that over-drops is as useless as one that
        // never drops.
        let (store, pid) = store_with_project();
        let lib = LemmaLibrary::with_subsumption_dedup(&store, pattern_verifier());

        assert!(lib.record_lemma(&pid, "P x ⊢ Q x", "by tac", "s1").unwrap());
        assert!(lib.record_lemma(&pid, "P x ⊢ R x", "by tac", "s2").unwrap());
        assert_eq!(lib.lemmas(&pid).unwrap().len(), 2);
        assert!(lib.drops(&pid).unwrap().is_empty());
    }

    #[test]
    fn displaced_lemma_is_not_retrieved_or_evolved() {
        // The retained view is what the rest of the system sees: a displaced
        // lemma must not come back through retrieval or the evolver scheduler.
        let (store, pid) = store_with_project();
        let lib = LemmaLibrary::with_subsumption_dedup(&store, pattern_verifier());

        lib.record_lemma(&pid, "P x, Q y ⊢ R z", "by tac", "specific")
            .unwrap();
        lib.record_lemma(&pid, "P x ⊢ R z", "by tac", "general")
            .unwrap();

        let hits = lib.retrieve(&pid, "P x, Q y ⊢ R z", 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].statement, "P x ⊢ R z");

        let pick = lib.next_to_evolve(&pid).unwrap().unwrap();
        assert_eq!(pick.statement, "P x ⊢ R z");
    }

    #[test]
    fn exact_dedup_never_displaces() {
        // Exact equality is symmetric, so it can only ever report the mutual
        // case, and duplicates are refused at admission. An exact-dedup library
        // therefore retains everything it stored: the retention rule is inert.
        let (store, pid) = store_with_project();
        let lib = LemmaLibrary::with_exact_dedup(&store, pattern_verifier());

        lib.record_lemma(&pid, "P x ⊢ R z", "by tac", "s1").unwrap();
        lib.record_lemma(&pid, "P x, Q y ⊢ R z", "by tac", "s2")
            .unwrap();
        assert_eq!(lib.lemmas(&pid).unwrap().len(), 2);
        assert!(lib.drops(&pid).unwrap().is_empty());
    }

    #[test]
    fn admits_verified_non_dup_and_rejects_dup() {
        let (store, pid) = store_with_project();
        let lib = LemmaLibrary::with_exact_dedup(&store, pattern_verifier());

        // Verified, novel -> admitted.
        assert!(lib
            .record_lemma(&pid, "a + b = b + a", "commutativity by ring", "seed")
            .unwrap());
        // Exact duplicate statement -> rejected by the default exact-equality dedup.
        assert!(!lib
            .record_lemma(&pid, "a + b = b + a", "commutativity by ring", "seed2")
            .unwrap());
        // Fails the verifier (no `by`) -> not admitted.
        assert!(!lib
            .record_lemma(&pid, "c * d = d * c", "handwave", "seed3")
            .unwrap());

        // Only the first landed in the store.
        assert_eq!(lib.lemmas(&pid).unwrap().len(), 1);
    }

    #[test]
    fn retrieve_ranks_relevant_above_irrelevant() {
        let (store, pid) = store_with_project();
        let lib = LemmaLibrary::with_exact_dedup(&store, pattern_verifier());

        lib.record_lemma(&pid, "sum of two even numbers is even", "by parity", "s1")
            .unwrap();
        lib.record_lemma(&pid, "a topological space is compact", "by cover", "s2")
            .unwrap();

        let hits = lib
            .retrieve(&pid, "the sum of even numbers is even", 2)
            .unwrap();
        assert_eq!(hits.len(), 2);
        assert!(
            hits[0].statement.contains("even"),
            "the parity lemma must rank first, got {:?}",
            hits[0].statement
        );
    }

    #[test]
    fn next_to_evolve_returns_least_updated() {
        let (store, pid) = store_with_project();
        let lib = LemmaLibrary::with_exact_dedup(&store, pattern_verifier());

        lib.record_lemma(&pid, "lemma one", "by x", "s1").unwrap();
        lib.record_lemma(&pid, "lemma two", "by y", "s2").unwrap();
        // Bump the first lemma's update_count so the second is least-updated.
        let first = lib.lemmas(&pid).unwrap()[0].clone();
        store.bump_library_lemma_update(&pid, &first.id).unwrap();

        let pick = lib.next_to_evolve(&pid).unwrap().unwrap();
        assert_eq!(pick.statement, "lemma two");
        assert_eq!(pick.update_count, 0);
    }

    #[test]
    fn enqueue_request_and_next_open_request() {
        let (store, pid) = store_with_project();
        let lib = LemmaLibrary::with_exact_dedup(&store, pattern_verifier());

        lib.enqueue_request(&pid, "prove base case", "hole:s1")
            .unwrap();
        lib.enqueue_request(&pid, "prove step case", "hole:s2")
            .unwrap();
        assert_eq!(lib.requests(&pid).unwrap().len(), 2);

        // Oldest open first; after bumping it, the second becomes least-updated.
        let first = lib.next_open_request(&pid).unwrap().unwrap();
        assert_eq!(first.subgoal, "prove base case");
        store.bump_library_request_update(&pid, &first.id).unwrap();
        let second = lib.next_open_request(&pid).unwrap().unwrap();
        assert_eq!(second.subgoal, "prove step case");
    }

    #[test]
    fn one_evolve_round_admits_generalized_lemma_and_updates_counts() {
        let (store, pid) = store_with_project();
        let lib = LemmaLibrary::with_exact_dedup(&store, pattern_verifier());

        lib.record_lemma(&pid, "a + 0 = a", "identity by simp", "seed")
            .unwrap();
        lib.enqueue_request(&pid, "n + 1 > n", "hole:x").unwrap();

        let verifier = pattern_verifier();
        let dedup: DedupFn = Box::new(|a, b| a == b);
        let summary = lib
            .evolve_round(&pid, verifier.as_ref(), &MockEvolver, dedup.as_ref())
            .unwrap();

        // 4 directional proposals + 1 request-solve = 5 proposed; all admitted.
        assert_eq!(summary.n_proposed, 5);
        assert!(summary.n_admitted >= 1);
        assert_eq!(summary.n_admitted, 5);
        assert_eq!(summary.n_deduped, 0);

        // Seed (1) + 4 generalisations + 1 request-solve = 6 lemmas.
        assert_eq!(lib.lemmas(&pid).unwrap().len(), 6);

        // The seed's update_count was bumped, and the request is now solved.
        let seed = lib
            .lemmas(&pid)
            .unwrap()
            .into_iter()
            .find(|l| l.statement == "a + 0 = a")
            .unwrap();
        assert_eq!(seed.update_count, 1);
        assert!(lib.next_open_request(&pid).unwrap().is_none());
    }

    #[test]
    fn evolve_round_dedups_repeat_proposals() {
        // A second round re-proposes the same generalisations of the (now
        // least-updated) generalised lemmas; exact-equality dedup rejects repeats.
        let (store, pid) = store_with_project();
        let lib = LemmaLibrary::with_exact_dedup(&store, pattern_verifier());
        lib.record_lemma(&pid, "base", "by tac", "seed").unwrap();

        let verifier = pattern_verifier();
        let dedup: DedupFn = Box::new(|a, b| a == b);
        lib.evolve_round(&pid, verifier.as_ref(), &MockEvolver, dedup.as_ref())
            .unwrap();
        // Re-run: "forall n, forall n, base" etc. are new, but "forall n, base"
        // from evolving the seed again would collide — at least one dedup fires
        // across the accumulated store over a second round.
        let before = lib.lemmas(&pid).unwrap().len();
        let s2 = lib
            .evolve_round(&pid, verifier.as_ref(), &MockEvolver, dedup.as_ref())
            .unwrap();
        let after = lib.lemmas(&pid).unwrap().len();
        assert_eq!(after - before, s2.n_admitted);
        assert!(s2.n_proposed >= s2.n_admitted);
    }

    #[test]
    fn sketch_holes_flow_in_as_requests() {
        // The requests worklist is fed from sketch holes (sketch.rs boundary):
        // build a sketch, and enqueue each open hole's subgoal as a request.
        let (store, pid) = store_with_project();
        let lib = LemmaLibrary::with_exact_dedup(&store, pattern_verifier());

        let sketch = InformalSketch::new(
            "P holds for all n",
            vec![
                SketchStep::hole("s1", "base case", "P 0"),
                SketchStep::hole("s2", "step case", "forall n, P n -> P (n+1)").using(["s1"]),
                SketchStep::prose("s3", "conclude"),
            ],
        );
        for hole in sketch.holes() {
            let subgoal = &hole.hole.as_ref().unwrap().subgoal;
            lib.enqueue_request(&pid, subgoal, &format!("hole:{}", hole.id))
                .unwrap();
        }

        let reqs = lib.requests(&pid).unwrap();
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0].subgoal, "P 0");
        assert_eq!(reqs[1].subgoal, "forall n, P n -> P (n+1)");
    }
}
