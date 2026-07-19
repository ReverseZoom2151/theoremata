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

use crate::config::Config;
use crate::db::Store;
use crate::library::{Lemma, LemmaLibrary};
use crate::model::ModelRequest;
use crate::portfolio::{portfolio_prove, PortfolioResult};
use crate::provider::ModelProvider;
use crate::symmetry_dedup::{dedup_candidates, SymmetryGroup};
use anyhow::Result;
use serde_json::json;

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
    /// Candidates set aside as exact duplicates of an earlier candidate in the
    /// same round's batch, before any falsify/prove effort was spent on them. A
    /// pure efficiency screen: these are never counted as refuted, failed, proved
    /// or subsumed, and the survivor they collapsed onto is still processed
    /// normally. Additive field, so existing counters keep their exact meaning.
    pub n_deduped: usize,
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

            // Dedup BEFORE the expensive falsify/prove step: never pay twice to
            // decide the same conjecture. We use the EMPTY symmetry group on the
            // conjecture's statement string, which makes `dedup_candidates`
            // degrade to exact statement-string equality (identical statements
            // only). That is the one equivalence sound to assert here: in
            // verification software a wrong symmetry generator could merge two
            // genuinely different conjectures and set aside the one that would
            // have graduated, whereas identical statement strings are provably the
            // same proof obligation. Anything cleverer (variable relabelling,
            // reflections) is NOT justified for arbitrary goal strings, so we
            // deliberately do not supply generators. Graduation's own subsumption
            // dedup in the library is untouched; this is only an upstream screen.
            let outcome = dedup_candidates(
                batch,
                &SymmetryGroup::<String>::new(),
                |c: &Conjecture| c.statement.clone(),
                |c: &Conjecture| c.provenance.clone(),
            );
            // Surface the count so no candidate is silently discarded: the deduper
            // retains every dropped candidate in `outcome.dropped` (recoverable),
            // and we fold its size into the report.
            report.n_deduped += outcome.report.dropped_count;
            let batch = outcome.into_kept();

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

// ---------------------------------------------------------------------------
// CLI entry point: model-backed seams over the deterministic engine
// ---------------------------------------------------------------------------

/// A [`Proposer`] that asks an injected model for conjectures seeded from the
/// current pool. An offline or erroring provider proposes nothing, so the engine
/// terminates immediately with an empty report: no conjecture is invented without
/// a model.
struct ModelProposer<'a> {
    provider: &'a dyn ModelProvider,
}

impl Proposer for ModelProposer<'_> {
    fn propose(&self, seeds: &[Lemma], round: usize, batch_size: usize) -> Vec<Conjecture> {
        // Skip the model call entirely when offline; the engine treats an empty
        // batch as deterministic early termination.
        if self.provider.name() == "offline" {
            return Vec::new();
        }
        let pool: Vec<&str> = seeds.iter().map(|l| l.statement.as_str()).collect();
        let request = ModelRequest {
            role: "conjecture_proposer".into(),
            task: "Propose auxiliary lemmas worth proving next, seeded from the \
                   current pool. Each is a goal-string statement (H |- C form) plus \
                   a short provenance tag naming the seed or mutation it came from. \
                   Do not prove them."
                .into(),
            context: json!({ "pool": pool, "round": round, "batch_size": batch_size }),
            output_schema: json!({
                "type":"object","required":["conjectures"],"properties":{
                    "conjectures":{"type":"array","items":{"type":"object",
                        "required":["statement","provenance"],
                        "properties":{
                            "statement":{"type":"string"},
                            "provenance":{"type":"string"}}}}}
            }),
        };
        let Ok(response) = self.provider.complete(&request) else {
            return Vec::new();
        };
        response.content["conjectures"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .take(batch_size)
                    .map(|c| {
                        Conjecture::new(
                            c["statement"].as_str().unwrap_or("").trim(),
                            c["provenance"].as_str().unwrap_or("conjecture_proposer"),
                        )
                    })
                    // Drop empty statements: an unparseable proposal is no
                    // proposal, not a blank conjecture handed to the prover.
                    .filter(|c| !c.statement.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// A [`Falsifier`] backed by the model-derived bounded numeric check
/// ([`crate::falsification`]). Only a real counterexample refutes; every other
/// verdict (including a clean bounded sweep, which proves nothing) abstains so the
/// conjecture proceeds to the prover. Mirrors the portfolio's `FalsifierRung`.
struct NumericFalsifier<'a> {
    provider: &'a dyn ModelProvider,
}

impl Falsifier for NumericFalsifier<'_> {
    fn refute(&self, conjecture: &Conjecture) -> Refutation {
        let falsifier = crate::falsification::Falsifier {
            provider: self.provider,
        };
        match falsifier.falsify(&conjecture.statement) {
            Ok(verdict) if verdict.verdict == "counterexample" => {
                let witness = verdict
                    .assignment
                    .map(|a| a.to_string())
                    .unwrap_or_else(|| "counterexample".into());
                Refutation::refuted(witness)
            }
            // A failed probe, or any non-refuting verdict, is not evidence that
            // the conjecture is false: abstain and let the prover decide.
            _ => Refutation::Unknown,
        }
    }
}

/// A [`Prover`] that dispatches each survivor through the existing portfolio
/// prove path and admits a proof ONLY when a live, fully clean 3+1-layer gate
/// closed it (identical discipline to sketch's `PortfolioHoleProver`). A mock or
/// lexical-only pass is never a proof here. The pool `premises` are not yet
/// threaded into the portfolio call; premise reuse is a future enhancement whose
/// absence only makes proving harder, never unsound.
struct PortfolioProver<'a> {
    store: &'a Store,
    config: &'a Config,
    provider: &'a dyn ModelProvider,
}

impl Prover for PortfolioProver<'_> {
    fn attempt(&self, conjecture: &Conjecture, _premises: &[Lemma]) -> ProveOutcome {
        match portfolio_prove(
            self.store,
            self.config,
            self.provider,
            &conjecture.statement,
            &crate::portfolio::ALL_SYSTEMS,
        ) {
            Ok(result) => match live_closed_proof(&result) {
                Some(code) => ProveOutcome::proved(code),
                None => ProveOutcome::Failed,
            },
            // A backend/generation fault is not a proof.
            Err(_) => ProveOutcome::Failed,
        }
    }
}

/// The verified proof source from the first system whose report is a LIVE, fully
/// clean gate pass, if any. This is the trust boundary for graduation: nothing
/// else counts as proved, so a mock or lexical-only pass never grows the library.
fn live_closed_proof(result: &PortfolioResult) -> Option<String> {
    result
        .per_system
        .iter()
        .find(|attempt| {
            attempt.code.is_some()
                && attempt.report.as_ref().is_some_and(|report| {
                    report.live
                        && report.lexically_verified
                        && report.axioms_clean
                        && report.statement_preserved
                        && report.lexical_clean
                        && report.hardening_clean != Some(false)
                })
        })
        .and_then(|attempt| attempt.code.clone())
}

/// Run the conjecture-and-prove loop for `project_id` with model-backed seams,
/// growing the project's verified-lemma library, and return a JSON tally.
///
/// Seams: the [`Proposer`] is the model; the [`Falsifier`] is the model-derived
/// bounded numeric check; the [`Prover`] is the existing portfolio path, which
/// yields a proof ONLY on a live, fully clean formal gate. The library is built
/// with subsumption dedup.
///
/// Trust boundary: graduation is gated upstream by that live formal verification
/// in the [`Prover`] seam (via [`live_closed_proof`]). The library's own
/// [`VerifierFn`](crate::library::VerifierFn) is a secondary floor that only
/// rejects an empty proof; it is deliberately NOT the sole authority, because the
/// string-only verifier seam cannot re-run a system-specific formal check. If the
/// [`Prover`] seam ever returned a non-live proof, that would be the bug to fix.
///
/// Offline (no model) the proposer is empty, so the loop runs zero rounds, calls
/// no backend, and admits nothing.
///
/// Emits a `conjecture_engine.completed` store event and closes a run row.
pub fn run(
    store: &Store,
    config: &Config,
    provider: &dyn ModelProvider,
    project_id: &str,
) -> Result<serde_json::Value> {
    let run_id = store.begin_run(project_id, "conjecture_engine")?;

    // The authoritative proof gate lives in the Prover seam; this verifier is a
    // defensive floor that rejects an empty proof and nothing more.
    let verifier: crate::library::VerifierFn = Box::new(|_stmt, proof: &str| !proof.trim().is_empty());
    let library = LemmaLibrary::with_subsumption_dedup(store, verifier);

    let engine = ConjectureEngine::new(
        library,
        ModelProposer { provider },
        PortfolioProver {
            store,
            config,
            provider,
        },
        NumericFalsifier { provider },
        ConjectureConfig::default(),
    );
    let report = engine.run(project_id)?;

    let state = if provider.name() == "offline" {
        "completed_no_model"
    } else {
        "completed"
    };
    let summary = json!({
        "project_id": project_id,
        "run_id": run_id,
        "model": provider.name(),
        "rounds": report.rounds,
        "n_proposed": report.n_proposed,
        "n_deduped": report.n_deduped,
        "n_refuted": report.n_refuted,
        "n_proved": report.n_proved,
        "n_graduated": report.n_graduated,
        "n_subsumed": report.n_subsumed,
        "n_failed": report.n_failed,
        "pool_size": engine.library().lemmas(project_id)?.len(),
    });

    store.event(
        Some(project_id),
        Some(&run_id),
        "conjecture_engine.completed",
        "conjecture_engine",
        json!({
            "rounds": report.rounds,
            "n_graduated": report.n_graduated,
            "model": provider.name(),
        }),
    )?;
    store.update_run(project_id, &run_id, state, "complete", 0)?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// A prover that records every statement it is asked to prove, so a test can
    /// assert an exact-duplicate candidate reached the prover only once.
    struct CountingProver {
        seen: std::cell::RefCell<Vec<String>>,
    }
    impl Prover for CountingProver {
        fn attempt(&self, conjecture: &Conjecture, _premises: &[Lemma]) -> ProveOutcome {
            self.seen.borrow_mut().push(conjecture.statement.clone());
            ProveOutcome::proved("qed")
        }
    }

    #[test]
    fn exact_duplicate_in_batch_is_deduped_before_proving() {
        // Two candidates with an identical statement string are the same proof
        // obligation: the pre-prove dedup must drop the second so the prover is
        // charged for it once, and the drop must be reported (not lost).
        let (store, pid) = store_with_project();
        let lib = LemmaLibrary::with_subsumption_dedup(&store, qed_verifier());
        let prover = CountingProver {
            seen: std::cell::RefCell::new(Vec::new()),
        };
        let proposer = ScriptedProposer {
            rounds: vec![vec![
                Conjecture::new("⊢ dup", "first"),
                Conjecture::new("⊢ dup", "second"),
                Conjecture::new("⊢ other", "third"),
            ]],
        };
        let engine = ConjectureEngine::new(
            lib,
            proposer,
            prover,
            PatternFalsifier,
            ConjectureConfig {
                rounds: 1,
                batch_size: 8,
            },
        );

        let report = engine.run(&pid).unwrap();
        assert_eq!(report.n_proposed, 3, "all three were proposed");
        assert_eq!(report.n_deduped, 1, "the exact duplicate was set aside");
        // The prover saw the duplicate statement exactly once, plus the distinct one.
        let seen = engine.prover.seen.borrow();
        assert_eq!(
            seen.iter().filter(|s| s.as_str() == "⊢ dup").count(),
            1,
            "the prover is charged for the duplicate statement only once"
        );
        assert_eq!(seen.len(), 2, "prover ran on the two distinct statements");
        // The survivor (first-seen provenance) is what graduated for that orbit.
        let pool = engine.library().lemmas(&pid).unwrap();
        let dup = pool.iter().find(|l| l.statement == "⊢ dup").unwrap();
        assert_eq!(dup.provenance, "first", "the first member of the orbit survives");
    }

    #[test]
    fn all_distinct_batch_is_untouched_by_the_dedup_screen() {
        // Guard against a future over-eager equivalence: when every statement is
        // distinct, the screen must drop NOTHING and leave every downstream count
        // exactly as it was before this screen existed.
        let (store, pid) = store_with_project();
        let lib = LemmaLibrary::with_subsumption_dedup(&store, qed_verifier());
        let proposer = ScriptedProposer {
            rounds: vec![vec![
                Conjecture::new("⊢ a", "s"),
                Conjecture::new("⊢ b", "s"),
                Conjecture::new("⊢ c", "s"),
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
        assert_eq!(report.n_deduped, 0, "nothing collapses when all are distinct");
        assert_eq!(report.n_proposed, 3);
        assert_eq!(report.n_proved, 3);
        assert_eq!(report.n_graduated, 3);
        assert_eq!(engine.library().lemmas(&pid).unwrap().len(), 3);
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

    #[test]
    fn offline_run_grows_nothing_and_reports_zero() {
        use crate::provider::OfflineProvider;
        let (store, pid) = store_with_project();
        let config = Config::default();
        // Offline: the model proposer yields nothing, so the loop terminates in
        // zero rounds without ever touching a backend, and admits nothing.
        let summary = super::run(&store, &config, &OfflineProvider, &pid).unwrap();
        assert_eq!(summary["rounds"], 0);
        assert_eq!(summary["n_proposed"], 0);
        assert_eq!(summary["n_graduated"], 0);
        assert_eq!(summary["pool_size"], 0);
        assert_eq!(summary["model"], "offline");

        let events = store.events(&pid, 100).unwrap();
        assert!(events
            .iter()
            .any(|e| e.event_type == "conjecture_engine.completed"));
    }

    /// Build a [`VerificationReport`](crate::prover::model::VerificationReport)
    /// with the given liveness, all other layers clean.
    fn report(live: bool) -> crate::prover::model::VerificationReport {
        crate::prover::model::VerificationReport {
            lexically_verified: true,
            axioms_clean: true,
            statement_preserved: true,
            lexical_clean: true,
            hardening_clean: None,
            live,
            detail: serde_json::Value::Null,
        }
    }

    fn attempt(
        code: Option<&str>,
        report: Option<crate::prover::model::VerificationReport>,
    ) -> crate::portfolio::SystemAttempt {
        crate::portfolio::SystemAttempt {
            system: crate::prover::formal::FormalSystem::Lean,
            verified: report.as_ref().is_some_and(|r| r.live && r.lexically_verified),
            available: true,
            code: code.map(str::to_owned),
            report,
            duration_ms: 0,
            error: None,
        }
    }

    fn result(attempts: Vec<crate::portfolio::SystemAttempt>) -> PortfolioResult {
        PortfolioResult {
            statement: "s".into(),
            winner: None,
            any_verified: false,
            per_system: attempts,
            refutation: None,
        }
    }

    #[test]
    fn only_a_live_clean_attempt_counts_as_a_closed_proof() {
        // A live, clean, code-bearing attempt is the only thing that graduates.
        let live = result(vec![attempt(Some("theorem t := by decide"), Some(report(true)))]);
        assert_eq!(
            live_closed_proof(&live).as_deref(),
            Some("theorem t := by decide")
        );

        // A mock (live=false) pass is NOT a proof, even though every other layer
        // is clean and code is present.
        let mock = result(vec![attempt(Some("mock proof"), Some(report(false)))]);
        assert!(live_closed_proof(&mock).is_none(), "a mock pass never graduates");

        // A live pass with no code cannot graduate (nothing to admit).
        let no_code = result(vec![attempt(None, Some(report(true)))]);
        assert!(live_closed_proof(&no_code).is_none());

        // A failed hardening layer vetoes it even when live.
        let mut hardened = report(true);
        hardened.hardening_clean = Some(false);
        let vetoed = result(vec![attempt(Some("code"), Some(hardened))]);
        assert!(live_closed_proof(&vetoed).is_none());

        // An unavailable-toolchain run (no report) is neither a win nor an error.
        let unavailable = result(vec![attempt(None, None)]);
        assert!(live_closed_proof(&unavailable).is_none());
    }
}
