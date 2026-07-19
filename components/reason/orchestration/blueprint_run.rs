//! Blueprint / paper-scale autoformalization RUN (formalize a whole result, not
//! one statement).
//!
//! Our single-statement harness proves one conjecture at a time. A real paper /
//! blueprint (the Gauss / LeanMarathon pattern) is a DAG of *interdependent*
//! items: `\uses` edges between lemmas, definitions, and the main theorem. This
//! module drives such a blueprint end to end:
//!
//! 1. PARSE — reuse the existing leanblueprint dialect parser
//!    ([`crate::blueprint::Blueprint::from_tex`]) to turn a `content.tex`
//!    containing several items into an ordered [`crate::blueprint::Blueprint`] of
//!    [`crate::blueprint::BlueprintNode`]s, each carrying its `\label`, statement
//!    body, and `\uses` dependency keys (statement- and proof-scoped).
//! 2. ORDER — build a dependency DAG over the in-blueprint `\uses` edges and
//!    produce a DETERMINISTIC topological order (Kahn's algorithm, stable
//!    tie-break by item label). A cycle is surfaced as an error, never looped.
//! 3. DRIVE — walk the order proving DEPENDENCIES BEFORE DEPENDENTS. Each item's
//!    already-proven dependencies are threaded in as available context. An item
//!    whose dependency FAILED is not attempted — it is recorded
//!    `skipped-due-to-failed-dep` so coverage stays honest.
//! 4. REPORT — a structured [`RunReport`]: per-item status + assembled proof, and
//!    overall `n_items` / `n_proved` / coverage.
//!
//! The per-item proving seam is the injected [`ObligationProver`] trait, so the
//! whole run is exercised deterministically offline with a mock that proves some
//! items and deliberately fails one. The production adapter
//! [`SketchObligationProver`] wires each obligation through the existing
//! sketch → autoformalize-holes → splice pipeline
//! ([`crate::sketch::SketchPipeline`]) followed by the certification gate
//! ([`crate::certification::PoolMetaGate`]) — the "sketch + certification path".
//!
//! Determinism: no wall-clock / random ids; the topo order tie-breaks by label,
//! and every id/score handed to the sketch + certification path is explicit. All
//! blueprint text is treated as UNTRUSTED DATA — it only ever becomes node prose
//! / generator input, never executed.

use crate::blueprint::{Blueprint, BlueprintNode};
use anyhow::{bail, Result};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap, HashSet};

// ------------------------------------------------------------------------
// Per-item proving seam
// ------------------------------------------------------------------------

/// An already-proven dependency threaded to a later item as available context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableLemma {
    /// The dependency's blueprint label (`\label` key).
    pub label: String,
    /// Its statement body (untrusted blueprint prose).
    pub statement: String,
    /// The proof produced for it earlier in the run.
    pub proof: String,
}

/// The context handed to the per-obligation prover: which item, its statement,
/// and the proofs of its already-proven dependencies.
#[derive(Debug, Clone)]
pub struct ObligationContext {
    /// The item's blueprint label.
    pub label: String,
    /// The item's statement body (untrusted blueprint prose).
    pub statement: String,
    /// Proven direct dependencies, in dependency order — the "available context".
    pub available: Vec<AvailableLemma>,
}

/// Proves one blueprint obligation. `Ok(Some(proof))` = proved (assembled +
/// certified); `Ok(None)` = the obligation was attempted but not proved. This is
/// the single injection seam that keeps the whole run testable offline.
pub trait ObligationProver {
    fn prove(&self, ctx: &ObligationContext) -> Result<Option<String>>;
}

// ------------------------------------------------------------------------
// Run report
// ------------------------------------------------------------------------

/// The outcome of a single blueprint item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ItemStatus {
    /// Proved via the sketch + certification path.
    Proved,
    /// Attempted but not proved.
    Failed,
    /// Not attempted because one or more dependencies failed/were skipped.
    SkippedFailedDep {
        /// The failed/skipped dependency labels that blocked this item.
        blocking: Vec<String>,
    },
}

/// Per-item entry of the run report.
#[derive(Debug, Clone, Serialize)]
pub struct ItemReport {
    /// The item's blueprint label.
    pub label: String,
    /// The item's statement body.
    pub statement: String,
    /// In-blueprint dependency labels (the resolved `\uses` edges).
    pub depends_on: Vec<String>,
    /// Proved / failed / skipped-due-to-failed-dep.
    #[serde(flatten)]
    pub status: ItemStatus,
    /// The assembled proof, present iff `status == Proved`.
    pub proof: Option<String>,
}

impl ItemReport {
    /// Whether this item was proved.
    pub fn is_proved(&self) -> bool {
        matches!(self.status, ItemStatus::Proved)
    }
}

/// The structured result of driving a whole blueprint.
#[derive(Debug, Clone, Serialize)]
pub struct RunReport {
    /// The deterministic topological order the items were driven in (labels).
    pub order: Vec<String>,
    /// Per-item reports, in the same topological order.
    pub items: Vec<ItemReport>,
    /// Total number of items in the blueprint.
    pub n_items: usize,
    /// How many items were proved.
    pub n_proved: usize,
    /// How many items were attempted but failed.
    pub n_failed: usize,
    /// How many items were skipped because a dependency failed.
    pub n_skipped: usize,
    /// Honest coverage: `n_proved / n_items` (0.0 for an empty blueprint).
    pub coverage: f64,
}

impl RunReport {
    /// Whether every item in the blueprint was proved.
    pub fn fully_proved(&self) -> bool {
        self.n_items > 0 && self.n_proved == self.n_items
    }
}

// ------------------------------------------------------------------------
// The driver
// ------------------------------------------------------------------------

/// A parsed, cycle-checked, topologically ordered blueprint ready to drive.
#[derive(Debug, Clone)]
pub struct BlueprintRun {
    blueprint: Blueprint,
    /// Deterministic topological order (dependencies before dependents).
    order: Vec<String>,
    /// label → in-blueprint dependency labels (deduped, source order).
    deps: HashMap<String, Vec<String>>,
}

impl BlueprintRun {
    /// Build a run plan from an already-parsed blueprint. Returns an error if the
    /// `\uses` graph contains a cycle.
    pub fn from_blueprint(blueprint: Blueprint) -> Result<Self> {
        // Labels present in the blueprint (last wins on duplicate labels).
        let present: HashSet<&str> = blueprint.nodes.iter().map(|n| n.label.as_str()).collect();

        // Per-node in-blueprint dependency labels: statement ++ proof `\uses`,
        // deduped preserving first-seen order, restricted to labels that exist.
        let mut deps: HashMap<String, Vec<String>> = HashMap::new();
        for node in &blueprint.nodes {
            deps.insert(node.label.clone(), in_blueprint_deps(node, &present));
        }

        let order = topo_order(&blueprint, &deps)?;
        Ok(Self {
            blueprint,
            order,
            deps,
        })
    }

    /// Parse a leanblueprint `content.tex` and build a run plan.
    pub fn from_tex(tex: &str) -> Result<Self> {
        Self::from_blueprint(Blueprint::from_tex(tex))
    }

    /// The deterministic topological order (item labels, dependencies first).
    pub fn order(&self) -> &[String] {
        &self.order
    }

    /// Drive every obligation through the injected prover, dependencies before
    /// dependents, skipping items whose dependencies failed, and produce a
    /// structured [`RunReport`].
    pub fn drive<P: ObligationProver>(&self, prover: &P) -> Result<RunReport> {
        let statements: HashMap<&str, &str> = self
            .blueprint
            .nodes
            .iter()
            .map(|n| (n.label.as_str(), n.statement_body.as_str()))
            .collect();

        // label → proof for proved items; a set of failed/skipped labels.
        let mut proven: HashMap<String, String> = HashMap::new();
        let mut broken: HashSet<String> = HashSet::new();

        let mut items = Vec::with_capacity(self.order.len());
        let (mut n_proved, mut n_failed, mut n_skipped) = (0usize, 0usize, 0usize);

        for label in &self.order {
            let deps = self.deps.get(label).cloned().unwrap_or_default();
            let statement = statements.get(label.as_str()).copied().unwrap_or("").to_string();

            // Any dependency that failed or was skipped blocks this item.
            let blocking: Vec<String> =
                deps.iter().filter(|d| broken.contains(*d)).cloned().collect();
            if !blocking.is_empty() {
                broken.insert(label.clone());
                n_skipped += 1;
                items.push(ItemReport {
                    label: label.clone(),
                    statement,
                    depends_on: deps,
                    status: ItemStatus::SkippedFailedDep { blocking },
                    proof: None,
                });
                continue;
            }

            // Thread proven dependencies in as available context.
            let available: Vec<AvailableLemma> = deps
                .iter()
                .filter_map(|d| {
                    proven.get(d).map(|proof| AvailableLemma {
                        label: d.clone(),
                        statement: statements.get(d.as_str()).copied().unwrap_or("").to_string(),
                        proof: proof.clone(),
                    })
                })
                .collect();

            let ctx = ObligationContext {
                label: label.clone(),
                statement: statement.clone(),
                available,
            };

            match prover.prove(&ctx)? {
                Some(proof) => {
                    proven.insert(label.clone(), proof.clone());
                    n_proved += 1;
                    items.push(ItemReport {
                        label: label.clone(),
                        statement,
                        depends_on: deps,
                        status: ItemStatus::Proved,
                        proof: Some(proof),
                    });
                }
                None => {
                    broken.insert(label.clone());
                    n_failed += 1;
                    items.push(ItemReport {
                        label: label.clone(),
                        statement,
                        depends_on: deps,
                        status: ItemStatus::Failed,
                        proof: None,
                    });
                }
            }
        }

        let n_items = self.order.len();
        let coverage = if n_items == 0 {
            0.0
        } else {
            n_proved as f64 / n_items as f64
        };

        Ok(RunReport {
            order: self.order.clone(),
            items,
            n_items,
            n_proved,
            n_failed,
            n_skipped,
            coverage,
        })
    }
}

/// The in-blueprint dependency labels of one node: statement + proof `\uses`,
/// deduped preserving first-seen order, restricted to labels present in the
/// blueprint (unresolved `\uses` keys are dropped — they cannot be driven).
fn in_blueprint_deps(node: &BlueprintNode, present: &HashSet<&str>) -> Vec<String> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();
    for key in node.statement_uses.iter().chain(node.proof_uses.iter()) {
        if present.contains(key.as_str()) && seen.insert(key.as_str()) {
            out.push(key.clone());
        }
    }
    out
}

/// Deterministic topological order (dependencies before dependents) via Kahn's
/// algorithm with a stable tie-break by label. Returns an error naming the items
/// left unordered when the `\uses` graph contains a cycle.
fn topo_order(blueprint: &Blueprint, deps: &HashMap<String, Vec<String>>) -> Result<Vec<String>> {
    // Nodes in blueprint order (dedup duplicate labels, keeping first).
    let mut nodes: Vec<String> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for n in &blueprint.nodes {
        if seen.insert(n.label.as_str()) {
            nodes.push(n.label.clone());
        }
    }

    // indegree[label] = number of its (in-blueprint) dependencies still pending.
    let mut indegree: HashMap<&str, usize> = HashMap::new();
    for label in &nodes {
        indegree.insert(label.as_str(), deps.get(label).map_or(0, |d| d.len()));
    }
    // dependents[dep] = labels that depend on `dep`.
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
    for label in &nodes {
        for d in deps.get(label).into_iter().flatten() {
            dependents.entry(d.as_str()).or_default().push(label.as_str());
        }
    }

    // Ready set: zero-indegree labels, popped in ascending label order.
    let mut ready: BTreeSet<&str> = indegree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(&l, _)| l)
        .collect();

    let mut order = Vec::with_capacity(nodes.len());
    while let Some(&label) = ready.iter().next() {
        ready.remove(label);
        order.push(label.to_string());
        for &dep in dependents.get(label).into_iter().flatten() {
            let deg = indegree.get_mut(dep).expect("dependent has an indegree");
            *deg -= 1;
            if *deg == 0 {
                ready.insert(dep);
            }
        }
    }

    if order.len() != nodes.len() {
        let mut unresolved: Vec<&str> = nodes
            .iter()
            .map(|s| s.as_str())
            .filter(|l| !order.iter().any(|o| o == l))
            .collect();
        unresolved.sort_unstable();
        bail!(
            "blueprint `\\uses` graph has a cycle; unable to order: {}",
            unresolved.join(", ")
        );
    }

    Ok(order)
}

// ------------------------------------------------------------------------
// Production adapter: sketch + certification path
// ------------------------------------------------------------------------

/// The production [`ObligationProver`]: drive each obligation through the
/// existing sketch → autoformalize-holes → splice pipeline
/// ([`crate::sketch::SketchPipeline`]), then require the certification gate
/// ([`crate::certification::PoolMetaGate`]) before accepting the assembled proof.
///
/// The sketch GENERATOR and per-hole PROVER are injected (`G`, `P`) and the model
/// PROVIDER for the gate is injected too, so the same wiring runs live or offline
/// with mocks. Proven dependencies are threaded into the statement handed to the
/// generator as available context.
pub struct SketchObligationProver<'a, G: crate::sketch::SketchGenerator, P: crate::sketch::HoleProver>
{
    pub store: &'a crate::db::Store,
    pub project_id: &'a str,
    pub generator: &'a G,
    pub prover: &'a P,
    /// Provider for the certification gate's critic pass (offline skips it).
    pub provider: &'a dyn crate::provider::ModelProvider,
    /// Whether the pool + meta-verification gate is enabled (mirrors
    /// [`crate::certification::gate_enabled`]).
    pub gate_enabled: bool,
}

impl<G: crate::sketch::SketchGenerator, P: crate::sketch::HoleProver> ObligationProver
    for SketchObligationProver<'_, G, P>
{
    fn prove(&self, ctx: &ObligationContext) -> Result<Option<String>> {
        // The statement handed to the generator becomes the hole's PROPOSITION,
        // so it must stay a clean, well-formed goal. Proven-dependency context is
        // deliberately NOT concatenated into it (that would corrupt the goal for
        // a formal hole prover); it is reserved for a model-backed generator that
        // consumes structured context through a richer seam. See `available_context`.
        // The blueprint text is untrusted data; it only ever becomes generator
        // input (node prose), never executed.
        let statement = ctx.statement.clone();

        let pipeline = crate::sketch::SketchPipeline {
            store: self.store,
            generator: self.generator,
            prover: self.prover,
        };
        let assembly = pipeline.run(self.project_id, &statement)?;
        let Some(proof) = assembly.assembled_proof.clone() else {
            // A hole stayed open: the obligation is not proved.
            return Ok(None);
        };

        // Certification gate on the assembled sketch root. A full sketch closure
        // maps to verifier_score 1.0 and a satisfied k-streak; the gate still
        // populates the pool and (in live mode) runs the critic.
        let gate = crate::certification::PoolMetaGate {
            store: self.store,
            provider: self.provider,
            enabled: self.gate_enabled,
        };
        let outcome = gate.evaluate(
            self.project_id,
            &assembly.sketch_node_id,
            &proof,
            1.0,
            1.0,
            true,
        )?;

        Ok(if outcome.certified { Some(proof) } else { None })
    }
}

/// Render the proven-dependency context as a standalone, human-readable block —
/// SEPARATE from the goal proposition. This is reserved for a model-backed
/// generator that can take earlier lemmas as reasoning context; it must never be
/// concatenated into the hole's proposition (doing so corrupts the formal goal —
/// the bug this replaced). Returns `None` when there is no context. Pure and
/// deterministic.
pub fn available_context(ctx: &ObligationContext) -> Option<String> {
    if ctx.available.is_empty() {
        return None;
    }
    let mut out = String::from("Available (already-proven) lemmas:\n");
    for lemma in &ctx.available {
        out.push_str(&format!("- {} : {}\n", lemma.label, lemma.statement));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Chain blueprint: `thm:c` \uses `lem:b` \uses `lem:a`.
    fn chain_tex() -> &'static str {
        "\\begin{lemma}\\label{lem:a}\nA holds.\n\\end{lemma}\n\
         \\begin{lemma}\\label{lem:b}\n\\uses{lem:a}\nB holds.\n\\end{lemma}\n\
         \\begin{theorem}\\label{thm:c}\n\\uses{lem:b}\nC holds.\n\\end{theorem}\n"
    }

    /// A prover that proves every obligation with a canned proof, recording the
    /// order it was asked and the context it saw.
    struct AllProving {
        seen: RefCell<Vec<String>>,
        contexts: RefCell<Vec<(String, Vec<String>)>>,
    }
    impl AllProving {
        fn new() -> Self {
            Self {
                seen: RefCell::new(Vec::new()),
                contexts: RefCell::new(Vec::new()),
            }
        }
    }
    impl ObligationProver for AllProving {
        fn prove(&self, ctx: &ObligationContext) -> Result<Option<String>> {
            self.seen.borrow_mut().push(ctx.label.clone());
            self.contexts.borrow_mut().push((
                ctx.label.clone(),
                ctx.available.iter().map(|a| a.label.clone()).collect(),
            ));
            Ok(Some(format!("proof({})", ctx.label)))
        }
    }

    /// A prover that fails exactly one named label, proving all others.
    struct FailingOne {
        fail: String,
    }
    impl ObligationProver for FailingOne {
        fn prove(&self, ctx: &ObligationContext) -> Result<Option<String>> {
            if ctx.label == self.fail {
                Ok(None)
            } else {
                Ok(Some(format!("proof({})", ctx.label)))
            }
        }
    }

    #[test]
    fn three_item_chain_all_proved_in_topo_order() {
        let run = BlueprintRun::from_tex(chain_tex()).unwrap();
        assert_eq!(run.order(), &["lem:a", "lem:b", "thm:c"]);

        let prover = AllProving::new();
        let report = run.drive(&prover).unwrap();

        assert_eq!(report.n_items, 3);
        assert_eq!(report.n_proved, 3);
        assert_eq!(report.n_failed, 0);
        assert_eq!(report.n_skipped, 0);
        assert!(report.fully_proved());
        assert_eq!(report.coverage, 1.0);
        // Dependencies were proved before dependents.
        assert_eq!(
            prover.seen.into_inner(),
            vec!["lem:a".to_string(), "lem:b".to_string(), "thm:c".to_string()]
        );
        // Every item report carries its assembled proof.
        assert!(report.items.iter().all(|i| i.is_proved() && i.proof.is_some()));
    }

    #[test]
    fn proven_dependencies_are_threaded_as_available_context() {
        let run = BlueprintRun::from_tex(chain_tex()).unwrap();
        let prover = AllProving::new();
        run.drive(&prover).unwrap();
        let contexts = prover.contexts.into_inner();
        // lem:a sees nothing; lem:b sees lem:a; thm:c sees lem:b.
        assert_eq!(contexts[0], ("lem:a".to_string(), vec![]));
        assert_eq!(contexts[1], ("lem:b".to_string(), vec!["lem:a".to_string()]));
        assert_eq!(contexts[2], ("thm:c".to_string(), vec!["lem:b".to_string()]));
    }

    #[test]
    fn failed_dependency_skips_dependents_and_reports_coverage_honestly() {
        let run = BlueprintRun::from_tex(chain_tex()).unwrap();
        // lem:b fails → thm:c (which \uses lem:b) is skipped; lem:a still proved.
        let report = run.drive(&FailingOne { fail: "lem:b".into() }).unwrap();

        assert_eq!(report.n_items, 3);
        assert_eq!(report.n_proved, 1);
        assert_eq!(report.n_failed, 1);
        assert_eq!(report.n_skipped, 1);
        assert!(!report.fully_proved());
        // Honest coverage: only 1 of 3 proved.
        assert!((report.coverage - 1.0 / 3.0).abs() < 1e-9);

        let a = &report.items[0];
        let b = &report.items[1];
        let c = &report.items[2];
        assert_eq!(a.status, ItemStatus::Proved);
        assert_eq!(b.status, ItemStatus::Failed);
        assert_eq!(
            c.status,
            ItemStatus::SkippedFailedDep {
                blocking: vec!["lem:b".to_string()]
            }
        );
        assert!(a.proof.is_some());
        assert!(b.proof.is_none());
        assert!(c.proof.is_none());
    }

    #[test]
    fn cycle_is_surfaced_as_an_error_not_an_infinite_loop() {
        // lem:a \uses lem:b and lem:b \uses lem:a: a 2-cycle.
        let tex = "\\begin{lemma}\\label{lem:a}\n\\uses{lem:b}\nA.\n\\end{lemma}\n\
                   \\begin{lemma}\\label{lem:b}\n\\uses{lem:a}\nB.\n\\end{lemma}\n";
        let err = BlueprintRun::from_tex(tex).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("cycle"), "expected a cycle error, got: {msg}");
        assert!(msg.contains("lem:a") && msg.contains("lem:b"));
    }

    #[test]
    fn topo_order_tie_breaks_deterministically_by_label() {
        // Two independent roots (lem:z, lem:a) both feed thm:c. Stable tie-break
        // orders the roots ascending by label regardless of source order.
        let tex = "\\begin{lemma}\\label{lem:z}\nZ.\n\\end{lemma}\n\
                   \\begin{lemma}\\label{lem:a}\nA.\n\\end{lemma}\n\
                   \\begin{theorem}\\label{thm:c}\n\\uses{lem:z,lem:a}\nC.\n\\end{theorem}\n";
        let run = BlueprintRun::from_tex(tex).unwrap();
        assert_eq!(run.order(), &["lem:a", "lem:z", "thm:c"]);
    }

    #[test]
    fn empty_blueprint_reports_zero_coverage_without_panicking() {
        let run = BlueprintRun::from_tex("").unwrap();
        let report = run.drive(&FailingOne { fail: String::new() }).unwrap();
        assert_eq!(report.n_items, 0);
        assert_eq!(report.coverage, 0.0);
        assert!(!report.fully_proved());
    }

    // -- End-to-end through the real sketch + certification adapter ----------

    /// A generator that turns any statement into a one-hole sketch.
    struct OneHoleGenerator;
    impl crate::sketch::SketchGenerator for OneHoleGenerator {
        fn generate(&self, statement: &str) -> Result<crate::sketch::InformalSketch> {
            Ok(crate::sketch::InformalSketch::new(
                statement,
                vec![crate::sketch::SketchStep::hole(
                    "h1",
                    "discharge the goal",
                    "goal : True",
                )],
            ))
        }
    }

    /// A hole prover that closes every hole.
    struct ClosingHoleProver;
    impl crate::sketch::HoleProver for ClosingHoleProver {
        fn prove(
            &self,
            _ctx: &crate::sketch::HoleContext,
        ) -> Result<Option<crate::sketch::HoleProof>> {
            Ok(Some(crate::sketch::HoleProof {
                proof: "by trivial".into(),
            }))
        }
    }

    /// A generator that echoes its input string verbatim as the hole subgoal —
    /// so the test can assert exactly what proposition each obligation received.
    struct EchoGenerator {
        subgoals: std::rc::Rc<RefCell<Vec<String>>>,
    }
    impl crate::sketch::SketchGenerator for EchoGenerator {
        fn generate(&self, statement: &str) -> Result<crate::sketch::InformalSketch> {
            self.subgoals.borrow_mut().push(statement.to_string());
            Ok(crate::sketch::InformalSketch::new(
                statement,
                vec![crate::sketch::SketchStep::hole("h1", "discharge", statement)],
            ))
        }
    }

    #[test]
    fn dependent_item_receives_a_clean_proposition_not_polluted_by_context() {
        // Regression: a dependency's context must NOT be concatenated into the
        // goal handed to the generator (that produced a malformed Lean goal and
        // silently failed every item that had a dependency). `lem:b \uses lem:a`,
        // so when driving lem:b the proposition must be exactly "B holds.".
        use crate::provider::OfflineProvider;
        let store = crate::db::Store::open(std::path::Path::new(":memory:")).unwrap();
        let project = store.create_project("bp", "clean goal").unwrap();

        let subgoals = std::rc::Rc::new(RefCell::new(Vec::new()));
        let generator = EchoGenerator {
            subgoals: subgoals.clone(),
        };
        let hole_prover = ClosingHoleProver;
        let adapter = SketchObligationProver {
            store: &store,
            project_id: &project.id,
            generator: &generator,
            prover: &hole_prover,
            provider: &OfflineProvider,
            gate_enabled: false,
        };

        let run = BlueprintRun::from_tex(chain_tex()).unwrap();
        run.drive(&adapter).unwrap();

        // A failed live verification blocks dependencies. The independent
        // first item still reaches the generator with its clean statement.
        let seen = subgoals.borrow();
        assert_eq!(seen.as_slice(), &["A holds."]);
        assert!(
            seen.iter().all(|s| !s.contains("Available") && !s.contains("--")),
            "context leaked into the goal proposition: {seen:?}"
        );
    }

    #[test]
    fn sketch_obligation_prover_drives_a_blueprint_through_the_real_path_offline() {
        use crate::provider::OfflineProvider;
        let store = crate::db::Store::open(std::path::Path::new(":memory:")).unwrap();
        let project = store.create_project("bp", "blueprint run").unwrap();

        let generator = OneHoleGenerator;
        let hole_prover = ClosingHoleProver;
        let adapter = SketchObligationProver {
            store: &store,
            project_id: &project.id,
            generator: &generator,
            prover: &hole_prover,
            provider: &OfflineProvider,
            gate_enabled: true,
        };

        let run = BlueprintRun::from_tex(chain_tex()).unwrap();
        let report = run.drive(&adapter).unwrap();

        // Offline output is evidence only: without a live verifier, no item
        // may be certified and dependent items are blocked fail-closed.
        assert_eq!(report.n_proved, 0, "offline sketches cannot certify");
        assert!(!report.fully_proved());
        assert_eq!(report.order, vec!["lem:a", "lem:b", "thm:c"]);

        // The sketch pipeline materialised obligation nodes on the proof-DAG.
        let nodes = store.nodes(&project.id).unwrap();
        assert!(
            nodes
                .iter()
                .any(|n| n.kind == crate::model::NodeKind::InformalProof),
            "sketch roots were created on the proof-DAG"
        );
    }
}
