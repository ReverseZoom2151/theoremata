//! Blueprint GENERATION + REFINEMENT (the planning layer above the driver).
//!
//! The named bottleneck in agentic theorem proving is PLANNING: attacking a hard
//! theorem head-on gets trapped in local dead ends. The fix (Goedel-Architect
//! pattern) is an explicit blueprint *planning* layer that (a) decomposes the
//! informal goal into a DAG of verifiable sub-lemmas culminating in the main
//! theorem, and (b) REFINES that plan from proving feedback — when a lemma fails,
//! decompose it further into sub-lemmas wired with `\uses` edges and try again.
//!
//! We already DRIVE a blueprint ([`crate::blueprint_run::BlueprintRun`]); this
//! module adds the two missing halves and the loop that closes them:
//!
//! * [`BlueprintGenerator`] — informal statement → a valid, acyclic
//!   [`crate::blueprint::Blueprint`] (round-trips to `content.tex`).
//! * [`BlueprintRefiner`] — `(blueprint, RunReport)` → a refined blueprint that
//!   decomposes the FAILED items further while keeping the PROVED items intact.
//! * [`plan_and_prove`] — the generate → drive → refine LOOP, bounded by
//!   `max_rounds`, that never emits a cyclic blueprint and whose best coverage is
//!   non-decreasing across rounds.
//!
//! Both model-shaped seams are TRAITS so the whole loop runs deterministically
//! offline with mocks (a real generator/refiner is a language model). Determinism:
//! every generator/refiner call is handed an explicit `seed` (derived per round
//! from `config.seed`); no wall-clock, no unseeded randomness. All informal /
//! blueprint text is treated as UNTRUSTED DATA — it only ever becomes node prose,
//! never executed.
//!
//! Acyclicity is validated by the SAME code path that drives the blueprint:
//! [`crate::blueprint_run::BlueprintRun::from_blueprint`] returns an error on a
//! `\uses` cycle, so a refinement that would introduce a cycle is rejected and the
//! previous (acyclic) blueprint is kept.

use crate::blueprint::Blueprint;
use crate::blueprint_run::{BlueprintRun, ItemStatus, ObligationProver, RunReport};
use anyhow::Result;
use serde::Serialize;

// ------------------------------------------------------------------------
// The two model-shaped seams
// ------------------------------------------------------------------------

/// Decompose an informal theorem into a blueprint: a DAG of sub-lemmas /
/// definitions wired with `\uses` edges culminating in the main theorem.
///
/// The returned [`Blueprint`] MUST be valid and acyclic (it is built from the same
/// [`crate::blueprint::BlueprintNode`] type, so it round-trips to `content.tex` and
/// is topo-orderable by the driver). Injected: a deterministic mock in tests, a
/// language model in production. The `seed` makes any sampling reproducible.
pub trait BlueprintGenerator {
    fn generate(&self, informal_statement: &str, seed: u64) -> Blueprint;
}

/// Given the current blueprint and the [`RunReport`] from driving it, produce a
/// REFINED blueprint: for a FAILED lemma, decompose it further into sub-lemmas (or
/// insert intermediate steps) wired with `\uses` edges, while keeping the
/// already-PROVED items intact. The result MUST stay acyclic (a refinement that
/// introduces a cycle is rejected by [`plan_and_prove`]). Injected: a mock in
/// tests, a language model in production.
pub trait BlueprintRefiner {
    fn refine(&self, blueprint: &Blueprint, report: &RunReport, seed: u64) -> Blueprint;
}

// ------------------------------------------------------------------------
// Configuration + result
// ------------------------------------------------------------------------

/// Loop bounds + determinism seed for [`plan_and_prove`].
#[derive(Debug, Clone)]
pub struct PlanConfig {
    /// Hard cap on generate/refine rounds — the loop is ALWAYS bounded.
    pub max_rounds: usize,
    /// Base seed; each round derives a distinct seed from it deterministically.
    pub seed: u64,
}

impl Default for PlanConfig {
    fn default() -> Self {
        Self {
            max_rounds: 4,
            seed: 0,
        }
    }
}

/// One round of the plan loop: the blueprint size that was driven and its report.
#[derive(Debug, Clone, Serialize)]
pub struct PlanRound {
    /// Number of nodes in the blueprint driven this round.
    pub blueprint_size: usize,
    /// The structured outcome of driving it.
    pub report: RunReport,
}

/// The result of the generate → drive → refine loop.
#[derive(Debug, Clone, Serialize)]
pub struct PlanResult {
    /// The last blueprint that was actually driven (always acyclic).
    pub final_blueprint: Blueprint,
    /// Per-round records, in loop order.
    pub rounds: Vec<PlanRound>,
    /// The best (maximum) coverage observed across all rounds — non-decreasing.
    pub best_coverage: f64,
    /// Whether the final round proved every item in its blueprint.
    pub fully_proved: bool,
}

impl PlanResult {
    /// How many generate/refine rounds actually ran.
    pub fn rounds_run(&self) -> usize {
        self.rounds.len()
    }
}

// ------------------------------------------------------------------------
// Acyclicity gate (shared with the driver)
// ------------------------------------------------------------------------

/// A blueprint is acceptable to drive iff the driver can build a run plan from it
/// — i.e. its `\uses` graph is acyclic. We reuse the driver's own cycle check so
/// "valid to drive" and "valid to plan" are exactly the same predicate.
fn is_acyclic(blueprint: &Blueprint) -> bool {
    BlueprintRun::from_blueprint(blueprint.clone()).is_ok()
}

/// Whether the report shows at least one item that could benefit from refinement
/// (something Failed or Skipped). If nothing is broken there is nothing to refine.
fn has_refinable_failure(report: &RunReport) -> bool {
    report.items.iter().any(|i| {
        matches!(
            i.status,
            ItemStatus::Failed | ItemStatus::SkippedFailedDep { .. }
        )
    })
}

// ------------------------------------------------------------------------
// The generate -> drive -> refine loop
// ------------------------------------------------------------------------

/// Generate an initial blueprint, drive it, and — while not fully proved and
/// rounds remain — refine it from the [`RunReport`] and re-drive. Bounded by
/// `config.max_rounds`; never emits a cyclic blueprint (each refinement is
/// validated and a cycle-introducing refinement is rejected, keeping the previous
/// acyclic blueprint); best coverage is non-decreasing across rounds.
///
/// Returns `Err` only if the injected prover errors, or if the generator hands
/// back a blueprint the driver cannot plan (a cyclic initial blueprint — the
/// generator contract forbids this).
pub fn plan_and_prove<G, R, P>(
    informal_statement: &str,
    generator: &G,
    refiner: &R,
    prover: &P,
    config: &PlanConfig,
) -> Result<PlanResult>
where
    G: BlueprintGenerator,
    R: BlueprintRefiner,
    P: ObligationProver,
{
    // Round 0: the generated plan. Its acyclicity is the generator's contract; if
    // it is cyclic the driver refuses it and we surface that as an error rather
    // than looping.
    let mut current = generator.generate(informal_statement, config.seed);

    let mut rounds: Vec<PlanRound> = Vec::new();
    let mut best_coverage = 0.0f64;
    let mut fully_proved = false;
    // The last blueprint we actually drove — what we return. Seeded to the initial
    // plan so an empty `max_rounds` still yields the generated blueprint.
    let mut final_blueprint = current.clone();

    let max_rounds = config.max_rounds.max(1);
    for round in 0..max_rounds {
        // Build a run plan (this is also our acyclicity gate). `current` is always
        // an accepted, acyclic blueprint by construction.
        let run = BlueprintRun::from_blueprint(current.clone())?;
        let report = run.drive(prover)?;

        best_coverage = best_coverage.max(report.coverage);
        fully_proved = report.fully_proved();
        final_blueprint = current.clone();
        rounds.push(PlanRound {
            blueprint_size: current.nodes.len(),
            report: report.clone(),
        });

        // Stop early: everything proved, nothing left to refine, or no more rounds.
        if fully_proved || !has_refinable_failure(&report) || round + 1 == max_rounds {
            break;
        }

        // Refine from feedback with a per-round-distinct, deterministic seed.
        let round_seed = config.seed.wrapping_add(round as u64).wrapping_add(1);
        let refined = refiner.refine(&current, &report, round_seed);

        // Reject a refinement that would introduce a cycle (or is otherwise
        // unplannable): keep the previous acyclic blueprint. Re-driving the same
        // blueprint would only reproduce the same report, so we stop here.
        if is_acyclic(&refined) {
            current = refined;
        } else {
            break;
        }
    }

    Ok(PlanResult {
        final_blueprint,
        rounds,
        best_coverage,
        fully_proved,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blueprint::BlueprintNode;
    use crate::blueprint_run::ObligationContext;

    // -- Blueprint construction helpers -------------------------------------

    /// A bare lemma node with a statement body and the given `\uses` deps.
    fn lemma(label: &str, body: &str, uses: &[&str]) -> BlueprintNode {
        BlueprintNode {
            env: "lemma".into(),
            label: label.into(),
            title: None,
            lean: vec![],
            statement_uses: uses.iter().map(|s| s.to_string()).collect(),
            proof_uses: vec![],
            statement_leanok: false,
            proof_leanok: false,
            statement_body: body.into(),
            proof_body: String::new(),
        }
    }

    fn theorem(label: &str, body: &str, uses: &[&str]) -> BlueprintNode {
        let mut n = lemma(label, body, uses);
        n.env = "theorem".into();
        n
    }

    // -- Deterministic mock generator ---------------------------------------

    /// Decompose any statement into `lem:easy`, `lem:hard`, and a `thm:main` that
    /// `\uses` both. A valid, acyclic `\uses` DAG. Seeded (the seed is echoed into
    /// prose so determinism is observable), never random.
    struct MockGenerator;
    impl BlueprintGenerator for MockGenerator {
        fn generate(&self, informal_statement: &str, seed: u64) -> Blueprint {
            Blueprint {
                nodes: vec![
                    lemma("lem:easy", &format!("easy part of: {informal_statement}"), &[]),
                    lemma(
                        "lem:hard",
                        &format!("hard part [seed={seed}]: {informal_statement}"),
                        &[],
                    ),
                    theorem(
                        "thm:main",
                        &format!("main: {informal_statement}"),
                        &["lem:easy", "lem:hard"],
                    ),
                ],
            }
        }
    }

    // -- Deterministic mock refiner -----------------------------------------

    /// Split the FIRST failed item into two sub-lemmas and wire the failed item's
    /// proof to use them (`<label>-sub1`, `<label>-sub2` with sub2 \uses sub1).
    /// Every already-present node is preserved verbatim; the sub-lemmas are
    /// inserted before the failed node so the topo order can reach them first.
    struct SplitFailedRefiner;
    impl BlueprintRefiner for SplitFailedRefiner {
        fn refine(&self, blueprint: &Blueprint, report: &RunReport, _seed: u64) -> Blueprint {
            let failed: Option<String> = report
                .items
                .iter()
                .find(|i| matches!(i.status, ItemStatus::Failed))
                .map(|i| i.label.clone());

            let Some(failed_label) = failed else {
                return blueprint.clone();
            };
            let sub1 = format!("{failed_label}-sub1");
            let sub2 = format!("{failed_label}-sub2");

            let mut nodes = Vec::new();
            for node in &blueprint.nodes {
                if node.label == failed_label {
                    // Insert the two sub-lemmas immediately before the failed node.
                    nodes.push(lemma(&sub1, "sub-lemma 1", &[]));
                    nodes.push(lemma(&sub2, "sub-lemma 2", &[&sub1]));
                    // The failed lemma now depends on its sub-lemmas.
                    let mut refined = node.clone();
                    refined.proof_uses = vec![sub1.clone(), sub2.clone()];
                    nodes.push(refined);
                } else {
                    nodes.push(node.clone());
                }
            }
            Blueprint { nodes }
        }
    }

    /// A refiner that ALWAYS wires the two split sub-lemmas into a 2-cycle — every
    /// refinement it produces is cyclic and must be rejected.
    struct CyclingRefiner;
    impl BlueprintRefiner for CyclingRefiner {
        fn refine(&self, blueprint: &Blueprint, _report: &RunReport, _seed: u64) -> Blueprint {
            let mut nodes = blueprint.nodes.clone();
            // a \uses b and b \uses a — a cycle the driver must reject.
            nodes.push(lemma("lem:cyc-a", "x", &["lem:cyc-b"]));
            nodes.push(lemma("lem:cyc-b", "y", &["lem:cyc-a"]));
            Blueprint { nodes }
        }
    }

    /// A refiner that returns the blueprint unchanged — models a refiner that never
    /// makes progress, used to check the loop is bounded.
    struct NoOpRefiner;
    impl BlueprintRefiner for NoOpRefiner {
        fn refine(&self, blueprint: &Blueprint, _report: &RunReport, _seed: u64) -> Blueprint {
            blueprint.clone()
        }
    }

    // -- Deterministic mock provers -----------------------------------------

    /// Proves every obligation whose label does NOT contain "hard". A "hard" lemma
    /// is only provable once it has available (already-proven) dependency context —
    /// i.e. once refinement has decomposed it into sub-lemmas. This is the seam
    /// that lets refinement turn a failure into a success across rounds.
    struct HardNeedsSupport;
    impl ObligationProver for HardNeedsSupport {
        fn prove(&self, ctx: &ObligationContext) -> Result<Option<String>> {
            let hard = ctx.label.contains("hard") && !ctx.label.contains("sub");
            if hard && ctx.available.is_empty() {
                Ok(None)
            } else {
                Ok(Some(format!("proof({})", ctx.label)))
            }
        }
    }

    /// Proves everything except any label containing "hard" — a hard lemma is never
    /// provable regardless of context, so the plan never fully closes.
    struct HardAlwaysFails;
    impl ObligationProver for HardAlwaysFails {
        fn prove(&self, ctx: &ObligationContext) -> Result<Option<String>> {
            if ctx.label.contains("hard") && !ctx.label.contains("sub") {
                Ok(None)
            } else {
                Ok(Some(format!("proof({})", ctx.label)))
            }
        }
    }

    // -- Tests ---------------------------------------------------------------

    #[test]
    fn generator_yields_a_valid_acyclic_uses_dag() {
        let bp = MockGenerator.generate("Every group has an identity", 7);
        // Topo-orderable via the existing driver == acyclic + resolvable.
        let run = BlueprintRun::from_blueprint(bp.clone()).unwrap();
        assert_eq!(run.order(), &["lem:easy", "lem:hard", "thm:main"]);
        // Round-trips to content.tex and back (reuses blueprint.rs types).
        let reparsed = Blueprint::from_tex(&bp.to_tex());
        assert_eq!(reparsed.nodes.len(), 3);
        assert!(reparsed.nodes.iter().any(|n| n.label == "thm:main"));
    }

    #[test]
    fn refiner_splits_a_failed_lemma_and_preserves_proved_items() {
        let bp = MockGenerator.generate("stmt", 0);
        let run = BlueprintRun::from_blueprint(bp.clone()).unwrap();
        let report = run.drive(&HardAlwaysFails).unwrap();
        // Precondition: lem:easy proved, lem:hard failed.
        assert!(report
            .items
            .iter()
            .any(|i| i.label == "lem:easy" && i.is_proved()));
        assert!(report
            .items
            .iter()
            .any(|i| i.label == "lem:hard" && matches!(i.status, ItemStatus::Failed)));

        let refined = SplitFailedRefiner.refine(&bp, &report, 1);

        // The two sub-lemmas appear, wired with a \uses edge (sub2 -> sub1).
        let sub1 = refined.nodes.iter().find(|n| n.label == "lem:hard-sub1");
        let sub2 = refined.nodes.iter().find(|n| n.label == "lem:hard-sub2");
        assert!(sub1.is_some(), "sub-lemma 1 was inserted");
        assert_eq!(sub2.unwrap().statement_uses, vec!["lem:hard-sub1"]);
        // The failed lemma now \uses its sub-lemmas.
        let hard = refined.nodes.iter().find(|n| n.label == "lem:hard").unwrap();
        assert_eq!(hard.proof_uses, vec!["lem:hard-sub1", "lem:hard-sub2"]);
        // The already-PROVED item is preserved verbatim.
        let easy_before = bp.nodes.iter().find(|n| n.label == "lem:easy").unwrap();
        let easy_after = refined.nodes.iter().find(|n| n.label == "lem:easy").unwrap();
        assert_eq!(easy_before, easy_after, "proved item preserved intact");
        // The refined blueprint is still acyclic / drivable.
        assert!(is_acyclic(&refined));
    }

    #[test]
    fn loop_improves_coverage_across_rounds_when_refinement_helps() {
        let cfg = PlanConfig {
            max_rounds: 3,
            seed: 42,
        };
        let result = plan_and_prove(
            "stmt",
            &MockGenerator,
            &SplitFailedRefiner,
            &HardNeedsSupport,
            &cfg,
        )
        .unwrap();

        assert!(result.rounds.len() >= 2, "at least an initial + refined round");
        let cov1 = result.rounds[0].report.coverage;
        let cov2 = result.rounds[1].report.coverage;
        // Round 1: lem:hard fails, thm:main skipped -> 1/3 covered.
        assert!((cov1 - 1.0 / 3.0).abs() < 1e-9, "cov1 = {cov1}");
        // Round 2: refinement made lem:hard provable -> coverage strictly improves.
        assert!(cov2 > cov1, "coverage must improve: {cov1} -> {cov2}");
        assert!(result.fully_proved, "the refined plan fully closes");
        assert!((result.best_coverage - 1.0).abs() < 1e-9);
        // best_coverage is non-decreasing (== max over rounds).
        let max = result
            .rounds
            .iter()
            .map(|r| r.report.coverage)
            .fold(0.0, f64::max);
        assert!((result.best_coverage - max).abs() < 1e-9);
    }

    #[test]
    fn cycle_introducing_refinement_is_rejected_and_blueprint_stays_acyclic() {
        let cfg = PlanConfig {
            max_rounds: 4,
            seed: 1,
        };
        let result = plan_and_prove(
            "stmt",
            &MockGenerator,
            &CyclingRefiner,
            &HardAlwaysFails,
            &cfg,
        )
        .unwrap();

        // The cyclic refinement was rejected; the final blueprint is the acyclic
        // generated one (no lem:cyc-* nodes leaked in).
        assert!(is_acyclic(&result.final_blueprint));
        assert!(BlueprintRun::from_blueprint(result.final_blueprint.clone()).is_ok());
        assert!(!result
            .final_blueprint
            .nodes
            .iter()
            .any(|n| n.label.starts_with("lem:cyc")));
        // Loop terminated (did not run forever) and never fully proved.
        assert!(!result.fully_proved);
        assert!(result.rounds.len() <= cfg.max_rounds);
    }

    #[test]
    fn loop_is_bounded_when_refinement_never_helps() {
        let cfg = PlanConfig {
            max_rounds: 3,
            seed: 0,
        };
        // NoOpRefiner never changes the plan; HardAlwaysFails never closes it.
        let result =
            plan_and_prove("stmt", &MockGenerator, &NoOpRefiner, &HardAlwaysFails, &cfg).unwrap();
        assert!(!result.fully_proved);
        // Bounded: exactly max_rounds attempts, never more.
        assert_eq!(result.rounds.len(), cfg.max_rounds);
        // Coverage never improved but stayed honest and non-negative.
        assert!(result.best_coverage >= 0.0 && result.best_coverage < 1.0);
    }

    #[test]
    fn seeded_determinism_same_seed_same_result() {
        let cfg = PlanConfig {
            max_rounds: 3,
            seed: 99,
        };
        let run = || {
            plan_and_prove(
                "stmt",
                &MockGenerator,
                &SplitFailedRefiner,
                &HardNeedsSupport,
                &cfg,
            )
            .unwrap()
        };
        let a = run();
        let b = run();
        // Identical plans, identical coverage, identical round count.
        assert_eq!(a.final_blueprint, b.final_blueprint);
        assert_eq!(a.rounds.len(), b.rounds.len());
        assert_eq!(a.best_coverage.to_bits(), b.best_coverage.to_bits());
        assert_eq!(a.fully_proved, b.fully_proved);
        // Generator is deterministic in isolation too.
        assert_eq!(
            MockGenerator.generate("stmt", 99),
            MockGenerator.generate("stmt", 99)
        );
    }

    #[test]
    fn max_rounds_zero_is_treated_as_a_single_generate_and_drive() {
        let cfg = PlanConfig {
            max_rounds: 0,
            seed: 0,
        };
        let result =
            plan_and_prove("stmt", &MockGenerator, &NoOpRefiner, &HardNeedsSupport, &cfg).unwrap();
        // Never loops forever: at least the generated plan is driven exactly once.
        assert_eq!(result.rounds.len(), 1);
        assert_eq!(result.final_blueprint.nodes.len(), 3);
    }
}
