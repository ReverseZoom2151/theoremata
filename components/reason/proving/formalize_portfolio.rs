//! Formalization portfolio: don't commit to ONE Lean/formal rendering of an
//! informal statement — generate N candidate FORMAL statements, screen each, and
//! surface the distinct ones with their differences so the most faithful can be
//! chosen.
//!
//! This is the FORMALIZING analogue of [`portfolio`](crate::portfolio): where
//! [`portfolio_prove`](crate::portfolio::portfolio_prove) fans one conjecture out
//! across prover backends and names the first that CERTIFIES,
//! [`formalize_portfolio`] fans one *informal* statement out into competing formal
//! renderings and names the best-*screened* candidate — never auto-committing.
//!
//! The motivating case (Erdős #728): two competing Lean formulations existed — an
//! older one and a newer one "more in the spirit" of the problem. A portfolio
//! keeps both, screens each (would it compile as a well-formed statement? is it
//! trivially satisfiable — the degenerate-solution check, conceptually the Python
//! `triviality` tool?), DEDUPs canonically-identical renderings via
//! [`crate::subsumption`], and reports the pairwise HYPOTHESIS/constraint
//! differences between candidates. It prefers well-formed, non-trivial candidates
//! but returns ALL with flags so a human (or the next stage) chooses.
//!
//! Both the [`Formalizer`] (candidate generator) and the [`StatementScreen`]
//! (well-formedness + triviality judge) are INJECTED: a deterministic mock in
//! tests, a model / best-of-N generator and the real compiler+triviality gate in
//! production. Generation is threaded a `seed`, never wall-clock or unseeded RNG,
//! so a portfolio run is reproducible.

use crate::{
    concurrent::{run_owned, ConcurrentConfig},
    config::Config,
    prover::{
        formal::{backend_for, FormalSystem},
        model::VerificationReport,
    },
    subsumption::{subsumes_str, CanonicalGoal},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::time::Instant;

/// Produce N candidate FORMAL statements for one informal statement.
///
/// The `seed` is threaded into generation so a run is reproducible (production
/// best-of-N samples the model deterministically from it; the test mock derives
/// its candidates from it). Implementations SHOULD NOT read wall-clock time or an
/// unseeded RNG. May return more or fewer than any target N; the portfolio caps
/// and de-duplicates.
pub trait Formalizer {
    /// Candidate formal renderings of `informal`, generated under `seed`.
    fn formalize(&self, informal: &str, seed: u64) -> Vec<String>;
}

/// The verdict of screening a single candidate formal statement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenResult {
    /// Would this compile as a well-formed *statement* (type-checks as a Prop /
    /// theorem signature, independent of whether it is provable)?
    pub well_formed: bool,
    /// Is the statement trivially satisfiable / degenerate — the triviality check
    /// (conceptually the Python `triviality` tool: e.g. a vacuous hypothesis, a
    /// `True` conclusion, or an existential a trivial witness satisfies)?
    pub trivial: bool,
    /// A short human-readable rationale for the two flags.
    pub note: String,
}

/// Screen a candidate formal statement for well-formedness + triviality.
///
/// Injected: the test mock is deterministic; production wires the real compiler
/// (well-formedness) and the triviality tool (degenerate-solution check).
pub trait StatementScreen {
    /// Judge one `formal` statement.
    fn screen(&self, formal: &str) -> ScreenResult;
}

/// One distinct, screened candidate rendering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenedCandidate {
    /// The formal statement source (trimmed).
    pub formal: String,
    /// Well-formedness verdict from the screen.
    pub well_formed: bool,
    /// Triviality verdict from the screen.
    pub trivial: bool,
    /// The screen's rationale.
    pub note: String,
}

/// A pairwise comparison of two candidates' hypotheses / constraints.
///
/// Candidates are compared via their [`CanonicalGoal`] form so hypothesis
/// reordering and α-renaming do not register as spurious differences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Difference {
    /// Index (into [`FormalizationReport::candidates`]) of the first candidate.
    pub a: usize,
    /// Index of the second candidate.
    pub b: usize,
    /// Canonical hypotheses present in `a` but dropped by `b`.
    pub only_in_a: Vec<String>,
    /// Canonical hypotheses present in `b` but dropped by `a`.
    pub only_in_b: Vec<String>,
    /// Whether the canonical conclusions differ.
    pub conclusion_differs: bool,
    /// Whether `a` subsumes `b` (proves the same conclusion from a subset of the
    /// hypotheses — `a` is the more general / weaker-premise rendering).
    pub a_subsumes_b: bool,
    /// Whether `b` subsumes `a`.
    pub b_subsumes_a: bool,
    /// A one-line human summary of the difference.
    pub summary: String,
}

/// The portfolio outcome: every distinct candidate with its screen flags, the
/// pairwise differences, and an OPTIONAL recommendation — never an auto-commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormalizationReport {
    /// The informal statement that was formalized.
    pub informal: String,
    /// Distinct candidates (canonically-identical renderings collapsed to one),
    /// in first-seen order.
    pub candidates: Vec<ScreenedCandidate>,
    /// Pairwise differences over the distinct candidates (`a < b`).
    pub differences: Vec<Difference>,
    /// Index of the recommended candidate — the FIRST well-formed, non-trivial
    /// one — or `None` when no candidate is both. Advisory only: the caller
    /// inspects every candidate and chooses.
    pub recommended: Option<usize>,
}

/// Knobs for a portfolio run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormalizeConfig {
    /// Seed threaded into [`Formalizer::formalize`] for reproducibility.
    pub seed: u64,
    /// Upper bound on candidates screened (generation is capped before dedup so a
    /// runaway generator cannot blow up the screen budget).
    pub max_candidates: usize,
}

impl Default for FormalizeConfig {
    fn default() -> Self {
        FormalizeConfig {
            seed: 0,
            max_candidates: 8,
        }
    }
}

// ---------------------------------------------------------------------------
// Owned verification seam for the formal-system portfolio
// ---------------------------------------------------------------------------

/// Which backend an owned verification task must use.
///
/// Selection is explicit and happens before the task crosses the worker
/// boundary. A live task never falls back to a mock backend when its toolchain is
/// unavailable; it returns an unavailable result instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationMode {
    Live,
    Mock,
}

/// A fully owned formal-system verification job.
///
/// This is the concurrency boundary for portfolio proving. Model generation,
/// database access, and event persistence stay on the caller thread. Once a
/// candidate proof has been generated, the caller moves its owned config, source,
/// and statement into this value; only backend construction and the independent
/// 3+1-layer verification gate run in a worker.
///
/// There are intentionally no references or service handles here. In particular,
/// neither `Store` nor `dyn ModelProvider` can be carried by this type.
#[derive(Debug, Clone)]
pub struct OwnedVerificationTask {
    pub system: FormalSystem,
    pub code: String,
    pub statement: String,
    config: Config,
    mode: VerificationMode,
}

impl OwnedVerificationTask {
    /// Prepare a task for a live backend. If the toolchain is unavailable at
    /// execution time, the result is `available: false`; no mock fallback occurs.
    pub fn live(config: Config, system: FormalSystem, code: String, statement: String) -> Self {
        Self {
            system,
            code,
            statement,
            config,
            mode: VerificationMode::Live,
        }
    }

    /// Prepare an explicitly mocked verification task for deterministic tests.
    /// Mock reports remain stamped `live: false` by the backend gate.
    pub fn mock(config: Config, system: FormalSystem, code: String, statement: String) -> Self {
        Self {
            system,
            code,
            statement,
            config,
            mode: VerificationMode::Mock,
        }
    }

    pub fn mode(&self) -> VerificationMode {
        self.mode
    }
}

/// Result of one [`OwnedVerificationTask`], retained in input-system order by
/// [`run_owned_formal_system_verifications`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnedVerificationResult {
    pub system: FormalSystem,
    pub available: bool,
    pub code: String,
    pub report: Option<VerificationReport>,
    pub duration_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl OwnedVerificationResult {
    /// Whether all configured gate layers passed. This deliberately does not
    /// imply live certification; use [`live_verified`](Self::live_verified) for
    /// that stronger predicate.
    pub fn gate_passed(&self) -> bool {
        self.report
            .as_ref()
            .is_some_and(|report| report.lexically_verified)
    }

    /// Whether a live backend, rather than a mock, passed every gate layer.
    pub fn live_verified(&self) -> bool {
        self.report
            .as_ref()
            .is_some_and(|report| report.live && report.lexically_verified)
    }
}

/// Verify already-generated, fully owned proof candidates, optionally in
/// parallel, while returning results in the exact input order.
///
/// `ConcurrentConfig::default()` is sequential, preserving the existing
/// portfolio behavior until a caller explicitly opts in. The worker receives no
/// database or provider handle; the remaining production caller hook is to split
/// sequential candidate generation/event persistence from verification in
/// `proving/portfolio.rs` and feed the generated candidates through this seam.
pub fn run_owned_formal_system_verifications(
    tasks: Vec<OwnedVerificationTask>,
    concurrency: &ConcurrentConfig,
) -> Vec<OwnedVerificationResult> {
    run_owned(tasks, execute_owned_verification, concurrency)
}

fn execute_owned_verification(task: OwnedVerificationTask) -> OwnedVerificationResult {
    let OwnedVerificationTask {
        system,
        code,
        statement,
        config,
        mode,
    } = task;
    let started = Instant::now();
    let backend = backend_for(&config, system, mode == VerificationMode::Mock);

    if mode == VerificationMode::Live && !backend.available() {
        return OwnedVerificationResult {
            system,
            available: false,
            code,
            report: None,
            duration_ms: started.elapsed().as_millis(),
            error: None,
        };
    }

    match backend.verify(&config, &code, &statement) {
        Ok(report) => OwnedVerificationResult {
            system,
            available: true,
            code,
            report: Some(report),
            duration_ms: started.elapsed().as_millis(),
            error: None,
        },
        Err(error) => OwnedVerificationResult {
            system,
            available: true,
            code,
            report: None,
            duration_ms: started.elapsed().as_millis(),
            error: Some(error.to_string()),
        },
    }
}

/// Generate candidate formalizations of `informal`, screen each, DEDUP
/// canonically-identical renderings, and return a ranked report.
///
/// Pipeline:
/// 1. `formalizer.formalize(informal, config.seed)` → raw candidates, capped at
///    `config.max_candidates` and with empties dropped.
/// 2. DEDUP by [`CanonicalGoal::key`]: two renderings that differ only in
///    hypothesis order or bound-variable names collapse to the first seen.
/// 3. `screen.screen(..)` each distinct candidate for well-formedness +
///    triviality.
/// 4. Compute pairwise [`Difference`]s (dropped hypotheses, differing
///    conclusions, subsumption) via [`crate::subsumption`].
/// 5. RECOMMEND the first well-formed, non-trivial candidate (or `None`).
///
/// Nothing is auto-picked or discarded on the caller's behalf beyond exact
/// canonical duplicates — every distinct rendering is returned with its flags.
pub fn formalize_portfolio(
    informal: &str,
    formalizer: &dyn Formalizer,
    screen: &dyn StatementScreen,
    config: &FormalizeConfig,
) -> FormalizationReport {
    // 1 + 2: generate, cap, and de-duplicate by canonical key.
    let raw = formalizer.formalize(informal, config.seed);
    let mut seen_keys: Vec<String> = Vec::new();
    let mut candidates: Vec<ScreenedCandidate> = Vec::new();

    for formal in raw.into_iter().take(config.max_candidates) {
        let formal = formal.trim().to_string();
        if formal.is_empty() {
            continue;
        }
        let key = CanonicalGoal::parse(&formal).key();
        if seen_keys.iter().any(|k| k == &key) {
            continue; // a canonically-identical rendering already kept.
        }
        seen_keys.push(key);

        // 3: screen this distinct candidate.
        let verdict = screen.screen(&formal);
        candidates.push(ScreenedCandidate {
            formal,
            well_formed: verdict.well_formed,
            trivial: verdict.trivial,
            note: verdict.note,
        });
    }

    // 4: pairwise differences over the distinct candidates.
    let differences = pairwise_differences(&candidates);

    // 5: recommend the first well-formed, non-trivial candidate — never a
    // trivial or ill-formed one, and never silently when none qualifies.
    let recommended = candidates
        .iter()
        .position(|c| c.well_formed && !c.trivial);

    FormalizationReport {
        informal: informal.to_string(),
        candidates,
        differences,
        recommended,
    }
}

/// Compute the pairwise [`Difference`]s between every distinct candidate pair.
fn pairwise_differences(candidates: &[ScreenedCandidate]) -> Vec<Difference> {
    let mut out = Vec::new();
    for i in 0..candidates.len() {
        let ga = CanonicalGoal::parse(&candidates[i].formal);
        let ha: BTreeSet<&str> = ga.hypotheses().iter().map(String::as_str).collect();
        for j in (i + 1)..candidates.len() {
            let gb = CanonicalGoal::parse(&candidates[j].formal);
            let hb: BTreeSet<&str> = gb.hypotheses().iter().map(String::as_str).collect();

            let only_in_a: Vec<String> =
                ha.difference(&hb).map(|s| s.to_string()).collect();
            let only_in_b: Vec<String> =
                hb.difference(&ha).map(|s| s.to_string()).collect();
            let conclusion_differs = ga.conclusion() != gb.conclusion();

            let a_subsumes_b =
                subsumes_str(&candidates[i].formal, &candidates[j].formal);
            let b_subsumes_a =
                subsumes_str(&candidates[j].formal, &candidates[i].formal);

            let summary = difference_summary(
                i,
                j,
                &only_in_a,
                &only_in_b,
                conclusion_differs,
                a_subsumes_b,
                b_subsumes_a,
            );

            out.push(Difference {
                a: i,
                b: j,
                only_in_a,
                only_in_b,
                conclusion_differs,
                a_subsumes_b,
                b_subsumes_a,
                summary,
            });
        }
    }
    out
}

/// Build the one-line human summary for a [`Difference`].
fn difference_summary(
    i: usize,
    j: usize,
    only_in_a: &[String],
    only_in_b: &[String],
    conclusion_differs: bool,
    a_subsumes_b: bool,
    b_subsumes_a: bool,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !only_in_a.is_empty() {
        parts.push(format!(
            "candidate {j} drops hypotheses [{}] kept by {i}",
            only_in_a.join(", ")
        ));
    }
    if !only_in_b.is_empty() {
        parts.push(format!(
            "candidate {i} drops hypotheses [{}] kept by {j}",
            only_in_b.join(", ")
        ));
    }
    if conclusion_differs {
        parts.push("conclusions differ".to_string());
    }
    if a_subsumes_b && b_subsumes_a {
        parts.push("equivalent (mutually subsuming)".to_string());
    } else if a_subsumes_b {
        parts.push(format!("candidate {i} subsumes {j}"));
    } else if b_subsumes_a {
        parts.push(format!("candidate {j} subsumes {i}"));
    }
    if parts.is_empty() {
        parts.push("no canonical difference in hypotheses or conclusion".to_string());
    }
    parts.join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic mock generator returning a fixed slate that exercises the
    /// whole pipeline: a full-hypothesis rendering, a dropped-hypothesis variant,
    /// a hypothesis-REORDERED duplicate of the first (must dedup), a trivial
    /// rendering, and an ill-formed one. Ignores the seed (its determinism is
    /// unconditional); seed threading is covered by [`SeedEchoFormalizer`].
    struct SlateFormalizer;
    impl Formalizer for SlateFormalizer {
        fn formalize(&self, _informal: &str, _seed: u64) -> Vec<String> {
            vec![
                "P x, Q y ⊢ R z".to_string(),   // 0: full hypotheses
                "P x ⊢ R z".to_string(),        // 1: drops `Q y`
                "Q y, P x ⊢ R z".to_string(),   // dup of 0 (reordered) → collapses
                "⊢ True".to_string(),           // trivial
                "garbage".to_string(),          // ill-formed (no turnstile)
            ]
        }
    }

    /// A mock screen: a statement is well-formed iff it carries a turnstile
    /// (a sequent), and trivial iff its conclusion is `True`.
    struct MarkerScreen;
    impl StatementScreen for MarkerScreen {
        fn screen(&self, formal: &str) -> ScreenResult {
            let well_formed = formal.contains('⊢') || formal.contains("|-");
            let concl = CanonicalGoal::parse(formal).conclusion().to_string();
            let trivial = concl == "True" || concl == "0 = 0";
            ScreenResult {
                well_formed,
                trivial,
                note: format!("wf={well_formed} trivial={trivial} concl={concl:?}"),
            }
        }
    }

    /// A generator whose output DEPENDS on the seed (deterministically, no RNG),
    /// used to prove the seed is threaded and runs are reproducible.
    struct SeedEchoFormalizer;
    impl Formalizer for SeedEchoFormalizer {
        fn formalize(&self, informal: &str, seed: u64) -> Vec<String> {
            let tag = if seed % 2 == 0 { "A" } else { "B" };
            vec![
                format!("H{seed} ⊢ {informal}"),
                format!("{tag} x ⊢ {informal}"),
            ]
        }
    }

    fn report() -> FormalizationReport {
        formalize_portfolio(
            "there exist infinitely many ...",
            &SlateFormalizer,
            &MarkerScreen,
            &FormalizeConfig::default(),
        )
    }

    #[test]
    fn generates_and_screens_each_distinct_candidate() {
        let r = report();
        // 5 generated, one is a reordered duplicate → 4 distinct screened.
        assert_eq!(r.candidates.len(), 4, "one reordered duplicate must collapse");
        // Every distinct candidate carries a screen verdict (note is non-empty).
        for c in &r.candidates {
            assert!(!c.note.is_empty(), "candidate {c:?} was not screened");
        }
    }

    #[test]
    fn canonically_identical_candidates_dedup_to_one() {
        let r = report();
        // The reordered `Q y, P x ⊢ R z` shares a canonical key with `P x, Q y ⊢ R z`.
        let key = CanonicalGoal::parse("P x, Q y ⊢ R z").key();
        let matches = r
            .candidates
            .iter()
            .filter(|c| CanonicalGoal::parse(&c.formal).key() == key)
            .count();
        assert_eq!(matches, 1, "canonically-identical renderings must collapse to one");
    }

    #[test]
    fn trivial_candidate_is_flagged_and_de_preferred() {
        let r = report();
        let trivial = r
            .candidates
            .iter()
            .find(|c| c.formal.contains("True"))
            .expect("the trivial candidate survives dedup");
        assert!(trivial.trivial, "`⊢ True` must be flagged trivial");
        // The recommendation must never point at the trivial candidate.
        let rec = r.recommended.expect("a well-formed non-trivial candidate exists");
        assert!(!r.candidates[rec].trivial, "recommended must not be trivial");
    }

    #[test]
    fn recommends_a_well_formed_non_trivial_candidate() {
        let r = report();
        let rec = r.recommended.expect("should recommend one");
        let c = &r.candidates[rec];
        assert!(c.well_formed, "recommended must be well-formed");
        assert!(!c.trivial, "recommended must be non-trivial");
        // It is the FIRST such candidate — the full-hypothesis rendering (index 0).
        assert_eq!(rec, 0);
        assert_eq!(c.formal, "P x, Q y ⊢ R z");
    }

    #[test]
    fn ill_formed_candidate_is_flagged_but_retained() {
        let r = report();
        let garbage = r
            .candidates
            .iter()
            .find(|c| c.formal == "garbage")
            .expect("the ill-formed candidate is retained, not silently dropped");
        assert!(!garbage.well_formed, "`garbage` has no turnstile → not well-formed");
    }

    #[test]
    fn differences_report_a_dropped_hypothesis() {
        let r = report();
        // Candidate 0 = `P x, Q y ⊢ R z`, candidate 1 = `P x ⊢ R z`: candidate 1
        // drops the `Q y` hypothesis. The canonical form of `Q y` is `Q y`.
        let diff = r
            .differences
            .iter()
            .find(|d| d.a == 0 && d.b == 1)
            .expect("a (0,1) pairwise difference exists");
        assert!(
            diff.only_in_a.iter().any(|h| h == "Q y"),
            "candidate 1 must be reported as dropping hypothesis `Q y`, got {:?}",
            diff.only_in_a
        );
        assert!(diff.only_in_b.is_empty(), "candidate 1 adds no hypothesis");
        assert!(!diff.conclusion_differs, "both conclude `R z`");
        // The dropped-hypothesis (weaker-premise) rendering subsumes the fuller one.
        assert!(diff.b_subsumes_a, "the P-only rendering is the more general one");
        assert!(diff.summary.contains("Q y"), "summary names the dropped hypothesis");
    }

    #[test]
    fn seeded_generation_is_deterministic_and_threads_the_seed() {
        let screen = MarkerScreen;
        let cfg7 = FormalizeConfig { seed: 7, max_candidates: 8 };
        let a = formalize_portfolio("phi", &SeedEchoFormalizer, &screen, &cfg7);
        let b = formalize_portfolio("phi", &SeedEchoFormalizer, &screen, &cfg7);
        assert_eq!(a, b, "same seed must yield an identical report");

        // A different seed threads through to different candidate text (7 -> odd
        // -> "B x", 8 -> even -> "A x"), proving the seed is actually used.
        let cfg8 = FormalizeConfig { seed: 8, max_candidates: 8 };
        let c = formalize_portfolio("phi", &SeedEchoFormalizer, &screen, &cfg8);
        assert_ne!(a.candidates, c.candidates, "distinct seeds must diverge");
        assert!(a.candidates.iter().any(|x| x.formal.contains("H7")));
        assert!(c.candidates.iter().any(|x| x.formal.contains("H8")));
    }

    #[test]
    fn max_candidates_caps_generation_before_dedup() {
        // Cap at 2 raw candidates: only `P x, Q y ⊢ R z` and `P x ⊢ R z` are seen.
        let cfg = FormalizeConfig { seed: 0, max_candidates: 2 };
        let r = formalize_portfolio("x", &SlateFormalizer, &MarkerScreen, &cfg);
        assert_eq!(r.candidates.len(), 2);
        assert!(r.candidates.iter().all(|c| c.well_formed && !c.trivial));
    }

    fn mock_verification_tasks(workspace: &std::path::Path) -> Vec<OwnedVerificationTask> {
        let mut config = Config {
            prover_mock: true,
            ..Config::default()
        };
        config.workspace = workspace.join("workspaces");

        vec![
            OwnedVerificationTask::mock(
                config.clone(),
                FormalSystem::Lean,
                "theorem generated : True := by trivial\n".to_string(),
                "True".to_string(),
            ),
            OwnedVerificationTask::mock(
                config.clone(),
                FormalSystem::Rocq,
                "Theorem generated : True.\nProof. exact I. Qed.\n".to_string(),
                "True".to_string(),
            ),
            OwnedVerificationTask::mock(
                config,
                FormalSystem::Isabelle,
                "theory Scratch\n  imports Main\nbegin\n\
                 theorem generated: \"True\" by simp\nend\n"
                    .to_string(),
                "True".to_string(),
            ),
        ]
    }

    #[test]
    fn owned_verification_seam_is_send_and_static() {
        fn assert_send_static<T: Send + 'static>() {}
        assert_send_static::<OwnedVerificationTask>();
        assert_send_static::<OwnedVerificationResult>();
    }

    #[test]
    fn owned_verifications_match_sequential_and_keep_system_order() {
        let temp = tempfile::tempdir().unwrap();
        let sequential = run_owned_formal_system_verifications(
            mock_verification_tasks(temp.path()),
            &ConcurrentConfig::default(),
        );
        let parallel = run_owned_formal_system_verifications(
            mock_verification_tasks(temp.path()),
            &ConcurrentConfig::with_threads(3),
        );

        let shape = |results: &[OwnedVerificationResult]| {
            results
                .iter()
                .map(|result| {
                    (
                        result.system,
                        result.available,
                        result.gate_passed(),
                        result.live_verified(),
                        result.error.is_none(),
                    )
                })
                .collect::<Vec<_>>()
        };
        let expected_systems = vec![
            FormalSystem::Lean,
            FormalSystem::Rocq,
            FormalSystem::Isabelle,
        ];

        assert_eq!(
            sequential
                .iter()
                .map(|result| result.system)
                .collect::<Vec<_>>(),
            expected_systems
        );
        assert_eq!(shape(&parallel), shape(&sequential));
        assert!(sequential.iter().all(OwnedVerificationResult::gate_passed));
        assert!(
            sequential.iter().all(|result| !result.live_verified()),
            "mock verification must never be presented as live certification"
        );
    }
}
