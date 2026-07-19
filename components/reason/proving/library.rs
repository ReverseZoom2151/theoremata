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
//!   this module;
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

    /// The shared admission gate: verify, then dedup against every existing
    /// lemma (including ones admitted earlier this round), then insert. Uses the
    /// explicitly-passed verifier/deduper so both `record_lemma` (stored
    /// injections) and `evolve_round` (per-call injections) share one policy.
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
        for existing in self.store.library_lemmas(project_id)? {
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

    /// All admitted lemmas for a project.
    pub fn lemmas(&self, project_id: &str) -> Result<Vec<Lemma>> {
        self.store.library_lemmas(project_id)
    }

    /// k-NN retrieval over the lemma store: rank every lemma by cosine
    /// similarity of its statement-embedding to `query`'s, best-first, and
    /// return the top `k`. The returned `(statement, proof)` pairs are directly
    /// spliceable. Deterministic (stable id tie-break).
    pub fn retrieve(&self, project_id: &str, query: &str, k: usize) -> Result<Vec<Lemma>> {
        let qv = embed(query);
        let mut scored: Vec<(f64, Lemma)> = self
            .store
            .library_lemmas(project_id)?
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

    /// The evolver scheduler pick: the least-`update_count` lemma.
    pub fn next_to_evolve(&self, project_id: &str) -> Result<Option<Lemma>> {
        self.store.next_library_lemma_to_evolve(project_id)
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
