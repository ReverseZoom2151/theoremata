//! Conjecture-and-prove loop that *grows the lemma library* (Seed-Prover's
//! "heavy" engine — our biggest structural gap).
//!
//! Seed-Prover's heavy setting does not just chase a single target: between
//! attempts it **conjectures** auxiliary lemmas, tries to prove or disprove each,
//! and folds the survivors back into a reusable pool so later proofs stand on a
//! taller stack of admitted facts. This module ports that organ to the Rust core,
//! deterministically and offline, on top of the existing growing-library
//! machinery ([`crate::library::LemmaLibrary`]) — so graduation, scoring, k-NN
//! retrieval and subsumption dedup are *reused*, not re-implemented.
//!
//! The loop, per round:
//! 1. **propose** a batch of candidate conjectures from the current lemma pool
//!    (the [`Proposer`] seam — mutate / specialise / generalise, deterministic by
//!    index);
//! 2. **falsify first** — run each candidate through the [`Falsifier`] and *drop*
//!    any [`Refutation::Refuted`] one before spending prover effort (our
//!    falsify-before-prove discipline: a cheap counterexample search screens out
//!    dead branches so the prover only sees survivors);
//! 3. **prove** the survivors with the [`Prover`] seam;
//! 4. **graduate** every [`ProveOutcome::Proved`] conjecture into the lemma
//!    library via [`LemmaLibrary::record_lemma`], which re-verifies it through the
//!    library's own (trusted) gate and **subsumption-dedups** it against the
//!    accumulated pool, so near-duplicate lemmas never pile up.
//!
//! A graduated lemma lands in the store, so the *next* round's [`Proposer`] sees
//! it as a seed and the [`Prover`] may cite it as a premise — verified-lemma
//! caching: the pool a conjecture is proposed from grows monotonically across
//! bounded rounds.
//!
//! ## Seams (all injected, no model here)
//!
//! [`Proposer`], [`Prover`] and [`Falsifier`] are traits, exactly as
//! [`crate::evolve_sketch`]'s `Mutator`/`Evaluator` are: in production they are
//! model-gated subagents (the propose/prove/disprove calls), in tests
//! deterministic mocks. The engine itself adds **no** wall-clock and **no**
//! randomness — any per-candidate variation a `Proposer` wants must be seeded by
//! its `round`/index arguments. All conjecture / proof / counterexample text is
//! untrusted data: it is only ever stored and handed to the injected seams and to
//! the library verifier — never executed here.

use crate::library::{Lemma, LemmaLibrary};
use anyhow::Result;

/// A candidate lemma the [`Proposer`] emits — the same `(statement, provenance)`
/// shape the library admits, minus the proof (which the [`Prover`] supplies).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conjecture {
    /// The conjectured statement (untrusted text; goal-string form, e.g.
    /// `H ⊢ C`, so the library's subsumption deduper can canonicalise it).
    pub statement: String,
    /// Where this candidate came from (which seed lemma / mutation axis), carried
    /// through to the graduated lemma's provenance for audit.
    pub provenance: String,
}

impl Conjecture {
    /// A candidate `statement` with its `provenance` tag.
    pub fn new(statement: impl Into<String>, provenance: impl Into<String>) -> Self {
        Self {
            statement: statement.into(),
            provenance: provenance.into(),
        }
    }
}

/// The [`Prover`] verdict for one conjecture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProveOutcome {
    /// A proof was found — its (untrusted) text is what gets graduated.
    Proved { proof: String },
    /// No proof was found this round (the conjecture is neither admitted nor
    /// discarded — a future round with a richer pool may still discharge it).
    Failed,
}

impl ProveOutcome {
    /// Convenience constructor for [`ProveOutcome::Proved`].
    pub fn proved(proof: impl Into<String>) -> Self {
        Self::Proved {
            proof: proof.into(),
        }
    }
}

/// The [`Falsifier`] verdict for one conjecture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refutation {
    /// A counterexample was found — the conjecture is false and is dropped before
    /// the prover ever sees it.
    Refuted { counterexample: String },
    /// No counterexample found in the searched domain — *not* a proof; the
    /// conjecture survives to the prover.
    Unknown,
}

impl Refutation {
    /// Convenience constructor for [`Refutation::Refuted`].
    pub fn refuted(counterexample: impl Into<String>) -> Self {
        Self::Refuted {
            counterexample: counterexample.into(),
        }
    }
}

/// Proposes candidate conjectures from the current lemma pool. Injected so the
/// loop runs deterministically with a mock; a real implementation is a
/// model-gated subagent that mutates / specialises / generalises seed lemmas.
pub trait Proposer {
    /// Emit up to `batch_size` candidates for `round`, given the current pool of
    /// admitted `seeds`. Must be deterministic in its arguments (seed any
    /// variation by `round`/index — no clock, no RNG).
    fn propose(&self, seeds: &[Lemma], round: usize, batch_size: usize) -> Vec<Conjecture>;
}

/// Attempts to prove a conjecture, citing already-admitted lemmas as premises
/// (verified-lemma caching). Injected — a model-gated prover in production, a
/// deterministic mock in tests.
pub trait Prover {
    /// Try to prove `conjecture`; `premises` are the lemmas already in the pool,
    /// reusable as axioms.
    fn attempt(&self, conjecture: &Conjecture, premises: &[Lemma]) -> ProveOutcome;
}

/// Attempts to *disprove* a conjecture by bounded counterexample search (the
/// falsify-before-prove screen). Injected.
pub trait Falsifier {
    /// Try to refute `conjecture`. Return [`Refutation::Unknown`] when no
    /// counterexample is found (the conservative answer — the conjecture then
    /// proceeds to the prover).
    fn refute(&self, conjecture: &Conjecture) -> Refutation;
}

/// Bounds and batch size for [`ConjectureEngine::run`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConjectureConfig {
    /// Hard cap on rounds (the loop also stops early when a round proposes
    /// nothing) — guarantees termination.
    pub rounds: usize,
    /// How many candidates a [`Proposer`] is asked for per round.
    pub batch_size: usize,
}

impl Default for ConjectureConfig {
    fn default() -> Self {
        Self {
            rounds: 4,
            batch_size: 16,
        }
    }
}

/// The tally [`ConjectureEngine::run`] returns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConjectureReport {
    /// Rounds actually executed (≤ `config.rounds`; fewer if the proposer dried
    /// up early).
    pub rounds: usize,
    /// Candidates the proposer produced across all rounds.
    pub n_proposed: usize,
    /// Candidates dropped by the falsify-first screen (a counterexample found).
    pub n_refuted: usize,
    /// Survivors the prover proved.
    pub n_proved: usize,
    /// Proved candidates admitted into the library (novel, verifier-passed).
    pub n_graduated: usize,
    /// Proved candidates the library did **not** admit — subsumed by an existing
    /// lemma (or rejected by the library's own verifier gate); either way they do
    /// not swell the pool.
    pub n_subsumed: usize,
    /// Survivors the prover failed to prove this run.
    pub n_failed: usize,
}

/// The conjecture-and-prove engine: drives propose → falsify → prove → graduate
/// over a bounded number of rounds, growing an injected [`LemmaLibrary`].
///
/// The library is the single source of truth for admission: it owns the trusted
/// verifier and the (subsumption) deduper, so this engine never re-decides
/// admissibility — it only routes proved candidates into
/// [`LemmaLibrary::record_lemma`]. Build the library with
/// [`LemmaLibrary::with_subsumption_dedup`] to get near-duplicate collapsing.
pub struct ConjectureEngine<'a, P: Proposer, V: Prover, F: Falsifier> {
    library: LemmaLibrary<'a>,
    proposer: P,
    prover: V,
    falsifier: F,
    config: ConjectureConfig,
}

impl<'a, P: Proposer, V: Prover, F: Falsifier> ConjectureEngine<'a, P, V, F> {
    /// Assemble an engine over a prepared [`LemmaLibrary`] and the three seams.
    pub fn new(
        library: LemmaLibrary<'a>,
        proposer: P,
        prover: V,
        falsifier: F,
        config: ConjectureConfig,
    ) -> Self {
        Self {
            library,
            proposer,
            prover,
            falsifier,
            config,
        }
    }

    /// Borrow the underlying library (e.g. to inspect / retrieve the grown pool
    /// after a run).
    pub fn library(&self) -> &LemmaLibrary<'a> {
        &self.library
    }

    /// Run the bounded conjecture-and-prove loop for `project_id`, growing the
    /// library, and return the round tally.
    ///
    /// Each round snapshots the current pool as both the proposer's seeds and the
    /// prover's premises (so within a round every candidate is judged against the
    /// same pool; graduations become visible to the *next* round). The loop is
    /// deterministic given deterministic seams, and terminates after at most
    /// `config.rounds` rounds — earlier if a round proposes nothing.
    pub fn run(&self, project_id: &str) -> Result<ConjectureReport> {
        let mut report = ConjectureReport::default();

        for round in 0..self.config.rounds {
            // The pool as it stands: seeds for proposal, premises for proving.
            let pool = self.library.lemmas(project_id)?;
            let batch = self.proposer.propose(&pool, round, self.config.batch_size);

            // No candidates left to explore -> deterministic early termination.
            if batch.is_empty() {
                break;
            }
            report.rounds = round + 1;
            report.n_proposed += batch.len();

            for conjecture in &batch {
                // Falsify FIRST: a refuted candidate is dropped before the prover
                // is ever invoked on it.
                if let Refutation::Refuted { .. } = self.falsifier.refute(conjecture) {
                    report.n_refuted += 1;
                    continue;
                }

                // Survivor -> attempt a proof, citing the current pool as premises.
                match self.prover.attempt(conjecture, &pool) {
                    ProveOutcome::Failed => report.n_failed += 1,
                    ProveOutcome::Proved { proof } => {
                        report.n_proved += 1;
                        // Graduate: the library re-verifies and subsumption-dedups.
                        let admitted = self.library.record_lemma(
                            project_id,
                            &conjecture.statement,
                            &proof,
                            &conjecture.provenance,
                        )?;
                        if admitted {
                            report.n_graduated += 1;
                        } else {
                            report.n_subsumed += 1;
                        }
                    }
                }
            }
        }

        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Store;
    use crate::library::VerifierFn;
    use std::path::Path;

    fn store_with_project() -> (Store, String) {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let p = store.create_project("p", "t").unwrap();
        (store, p.id)
    }

    /// Library gate: admit any proof carrying the `qed` marker (stands in for the
    /// production 3+1 gate). Paired with subsumption dedup so near-duplicate
    /// graduations collapse.
    fn qed_verifier() -> VerifierFn {
        Box::new(|_stmt: &str, proof: &str| proof.contains("qed"))
    }

    /// A proposer that replays a fixed per-round script (seed-independent), so a
    /// test can dictate exactly which candidates appear each round. Returning an
    /// empty list once the script is exhausted exercises early termination.
    struct ScriptedProposer {
        rounds: Vec<Vec<Conjecture>>,
    }
    impl Proposer for ScriptedProposer {
        fn propose(&self, _seeds: &[Lemma], round: usize, _batch: usize) -> Vec<Conjecture> {
            self.rounds.get(round).cloned().unwrap_or_default()
        }
    }

    /// A prover that discharges everything except statements marked `unprovable`,
    /// citing the premise count so premise-reuse is observable in the proof text.
    struct PatternProver;
    impl Prover for PatternProver {
        fn attempt(&self, conjecture: &Conjecture, premises: &[Lemma]) -> ProveOutcome {
            if conjecture.statement.contains("unprovable") {
                ProveOutcome::Failed
            } else {
                ProveOutcome::proved(format!("by premises({}) qed", premises.len()))
            }
        }
    }

    /// A falsifier that refutes any statement marked `refute-me`.
    struct PatternFalsifier;
    impl Falsifier for PatternFalsifier {
        fn refute(&self, conjecture: &Conjecture) -> Refutation {
            if conjecture.statement.contains("refute-me") {
                Refutation::refuted("witness")
            } else {
                Refutation::Unknown
            }
        }
    }

    #[test]
    fn refuted_dropped_proved_graduate_failed_counted() {
        // One round with a provable candidate, a refutable one, and an unprovable
        // one: only the provable one graduates.
        let (store, pid) = store_with_project();
        let lib = LemmaLibrary::with_subsumption_dedup(&store, qed_verifier());
        let proposer = ScriptedProposer {
            rounds: vec![vec![
                Conjecture::new("⊢ good", "seed"),
                Conjecture::new("⊢ refute-me claim", "bad"),
                Conjecture::new("⊢ unprovable claim", "hard"),
            ]],
        };
        let engine = ConjectureEngine::new(
            lib,
            proposer,
            PatternProver,
            PatternFalsifier,
            ConjectureConfig {
                rounds: 1,
                batch_size: 8,
            },
        );

        let report = engine.run(&pid).unwrap();
        assert_eq!(report.rounds, 1);
        assert_eq!(report.n_proposed, 3);
        assert_eq!(report.n_refuted, 1, "refute-me is dropped before proving");
        assert_eq!(report.n_failed, 1, "unprovable is attempted but fails");
        assert_eq!(report.n_proved, 1);
        assert_eq!(report.n_graduated, 1);
        assert_eq!(report.n_subsumed, 0);

        // Exactly the one proved conjecture landed in the pool.
        let pool = engine.library().lemmas(&pid).unwrap();
        assert_eq!(pool.len(), 1);
        assert_eq!(pool[0].statement, "⊢ good");
        assert_eq!(pool[0].provenance, "seed");
    }

    #[test]
    fn proved_but_subsumed_conjecture_does_not_swell_the_pool() {
        // Pre-seed a general lemma, then conjecture a strictly more-specific
        // restatement: it proves, but subsumption dedup rejects graduation.
        let (store, pid) = store_with_project();
        let lib = LemmaLibrary::with_subsumption_dedup(&store, qed_verifier());
        // Seed "⊢ target" directly through the library interface.
        assert!(lib
            .record_lemma(&pid, "⊢ target", "by tac qed", "root")
            .unwrap());

        let proposer = ScriptedProposer {
            // "H ⊢ target": same conclusion, an extra hypothesis -> subsumed by the
            // existing "⊢ target".
            rounds: vec![vec![Conjecture::new("H ⊢ target", "specialise")]],
        };
        let engine = ConjectureEngine::new(
            lib,
            proposer,
            PatternProver,
            PatternFalsifier,
            ConjectureConfig {
                rounds: 1,
                batch_size: 8,
            },
        );

        let report = engine.run(&pid).unwrap();
        assert_eq!(report.n_proved, 1, "the restatement is provable");
        assert_eq!(report.n_graduated, 0, "but it is subsumed, so not admitted");
        assert_eq!(report.n_subsumed, 1);
        // The pool still holds only the original seed.
        assert_eq!(engine.library().lemmas(&pid).unwrap().len(), 1);
    }

    #[test]
    fn terminates_within_bounded_rounds_even_when_proposer_never_dries_up() {
        // A proposer that yields a fresh (distinct) provable candidate every round
        // would run forever without the cap; the cap must stop it at `rounds`.
        struct EndlessProposer;
        impl Proposer for EndlessProposer {
            fn propose(&self, _s: &[Lemma], round: usize, _b: usize) -> Vec<Conjecture> {
                // Distinct per round (seeded by index), so each one graduates.
                vec![Conjecture::new(format!("⊢ lemma_{round}"), "endless")]
            }
        }
        let (store, pid) = store_with_project();
        let lib = LemmaLibrary::with_subsumption_dedup(&store, qed_verifier());
        let engine = ConjectureEngine::new(
            lib,
            EndlessProposer,
            PatternProver,
            PatternFalsifier,
            ConjectureConfig {
                rounds: 5,
                batch_size: 4,
            },
        );

        let report = engine.run(&pid).unwrap();
        assert_eq!(report.rounds, 5, "runs exactly the capped number of rounds");
        assert_eq!(report.n_proposed, 5);
        assert_eq!(report.n_graduated, 5);
        assert_eq!(engine.library().lemmas(&pid).unwrap().len(), 5);
    }

    #[test]
    fn stops_early_when_proposer_dries_up() {
        // Script has candidates for round 0 only; round 1 returns empty -> break.
        let (store, pid) = store_with_project();
        let lib = LemmaLibrary::with_subsumption_dedup(&store, qed_verifier());
        let proposer = ScriptedProposer {
            rounds: vec![vec![Conjecture::new("⊢ once", "seed")]],
        };
        let engine = ConjectureEngine::new(
            lib,
            proposer,
            PatternProver,
            PatternFalsifier,
            ConjectureConfig {
                rounds: 100,
                batch_size: 8,
            },
        );

        let report = engine.run(&pid).unwrap();
        assert_eq!(report.rounds, 1, "stopped as soon as the proposer emptied");
        assert_eq!(report.n_graduated, 1);
    }

    #[test]
    fn graduated_lemma_is_available_as_a_seed_to_a_later_round() {
        // Round 0 proves a root lemma; round 1's proposer ONLY fires when it can
        // see that root in its seeds, deriving a child from it. The child's
        // presence in the pool proves the round-0 graduate reached round 1.
        struct LineageProposer;
        impl Proposer for LineageProposer {
            fn propose(&self, seeds: &[Lemma], round: usize, _b: usize) -> Vec<Conjecture> {
                if round == 0 {
                    vec![Conjecture::new("⊢ base", "root")]
                } else {
                    // Derive from the round-0 lemma iff it is visible as a seed.
                    seeds
                        .iter()
                        .filter(|s| s.statement == "⊢ base")
                        .map(|s| {
                            Conjecture::new(
                                format!("⊢ derived_from({})", s.statement),
                                format!("child-of:{}", s.id),
                            )
                        })
                        .collect()
                }
            }
        }
        let (store, pid) = store_with_project();
        let lib = LemmaLibrary::with_subsumption_dedup(&store, qed_verifier());
        let engine = ConjectureEngine::new(
            lib,
            LineageProposer,
            PatternProver,
            PatternFalsifier,
            ConjectureConfig {
                rounds: 2,
                batch_size: 8,
            },
        );

        let report = engine.run(&pid).unwrap();
        assert_eq!(
            report.n_graduated, 2,
            "root (r0) + child (r1) both graduate"
        );
        let pool = engine.library().lemmas(&pid).unwrap();
        assert!(
            pool.iter().any(|l| l.statement == "⊢ derived_from(⊢ base)"),
            "the child lemma exists, so the r0 graduate was a seed in r1: {:?}",
            pool.iter().map(|l| &l.statement).collect::<Vec<_>>()
        );
    }

    #[test]
    fn run_is_deterministic() {
        // Two independent runs with identical inputs produce identical reports.
        let script = || ScriptedProposer {
            rounds: vec![
                vec![
                    Conjecture::new("⊢ a", "s"),
                    Conjecture::new("⊢ refute-me b", "s"),
                ],
                vec![Conjecture::new("⊢ c", "s")],
            ],
        };
        let run = || {
            let (store, pid) = store_with_project();
            let lib = LemmaLibrary::with_subsumption_dedup(&store, qed_verifier());
            let engine = ConjectureEngine::new(
                lib,
                script(),
                PatternProver,
                PatternFalsifier,
                ConjectureConfig {
                    rounds: 3,
                    batch_size: 8,
                },
            );
            engine.run(&pid).unwrap()
        };

        assert_eq!(run(), run(), "same seams + inputs -> same report");
    }
}
