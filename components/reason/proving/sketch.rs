//! Informal-sketch → autoformalize-holes → splice pipeline (Aristotle / Nexus).
//!
//! The SOTA "sketch-and-fill" pattern: a model proposes an INFORMAL proof as an
//! ordered list of steps; some steps carry a HOLE (a subgoal that still needs a
//! rigorous proof). The pipeline
//!
//! 1. represents the sketch AS a sub-DAG on the existing proof-DAG — a root
//!    `informal_proof` node with one `obligation` node per hole, wired with
//!    `\uses`-style dependency edges between steps (a hole that uses an earlier
//!    hole's result depends on it);
//! 2. dispatches each hole to the injected per-hole prover (in production, the
//!    existing per-obligation prove path); and
//! 3. splices the returned proofs back into one assembled proof — but ONLY once
//!    every hole is closed. If any hole is left open, assembly is refused and the
//!    failure is surfaced (never a partial/fake proof).
//!
//! Both the sketch GENERATOR and the per-hole PROVER are injected as traits, so
//! the whole pipeline is exercised deterministically with mocks. Step ids are
//! caller-supplied and threaded explicitly — no wall-clock/random ids in the
//! assembly logic. All model-produced sketch text is treated as untrusted data:
//! it only ever becomes node prose / strategy hints, never executed.

use crate::{
    db::Store,
    model::{EdgeKind, NodeKind, NodeStatus, NodeTier},
    provider::ModelProvider,
};
use anyhow::Result;
use serde_json::json;
use std::collections::HashMap;

/// A subgoal that a sketch step defers to a rigorous proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hole {
    /// The subgoal statement to be discharged by the per-hole prover.
    pub subgoal: String,
}

/// One ordered step of an informal proof sketch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SketchStep {
    /// Stable, caller-supplied id (threaded explicitly; used as the `\uses` key).
    pub id: String,
    /// The informal prose of this step.
    pub prose: String,
    /// `Some` when this step defers a subgoal that must be proven.
    pub hole: Option<Hole>,
    /// `\uses`-style dependencies: ids of EARLIER steps whose results this step
    /// relies on. Edges are only materialised between hole-bearing steps.
    pub uses: Vec<String>,
}

impl SketchStep {
    /// A prose-only step (no hole, no dependencies).
    pub fn prose(id: impl Into<String>, prose: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            prose: prose.into(),
            hole: None,
            uses: Vec::new(),
        }
    }

    /// A step that defers `subgoal` as a hole.
    pub fn hole(
        id: impl Into<String>,
        prose: impl Into<String>,
        subgoal: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            prose: prose.into(),
            hole: Some(Hole {
                subgoal: subgoal.into(),
            }),
            uses: Vec::new(),
        }
    }

    /// Builder: declare the `\uses` dependencies of this step.
    pub fn using(mut self, uses: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.uses = uses.into_iter().map(Into::into).collect();
        self
    }
}

/// An ordered informal proof sketch for a statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InformalSketch {
    pub statement: String,
    pub steps: Vec<SketchStep>,
}

impl InformalSketch {
    pub fn new(statement: impl Into<String>, steps: Vec<SketchStep>) -> Self {
        Self {
            statement: statement.into(),
            steps,
        }
    }

    /// The steps that carry a hole, in order.
    pub fn holes(&self) -> impl Iterator<Item = &SketchStep> {
        self.steps.iter().filter(|s| s.hole.is_some())
    }
}

/// Produces an informal sketch for a statement (the model/generator seam).
pub trait SketchGenerator {
    fn generate(&self, statement: &str) -> Result<InformalSketch>;
}

/// The degenerate production generator: emit a one-step sketch whose single hole
/// IS the whole statement. With no model-driven decomposition available, a
/// sketch run gracefully degrades to "prove the statement directly" — the hole
/// is then discharged by whatever `HoleProver` is wired (e.g. the portfolio).
/// This is the honest default until a model-backed multi-step generator lands.
pub struct WholeStatementGenerator;

impl SketchGenerator for WholeStatementGenerator {
    fn generate(&self, statement: &str) -> Result<InformalSketch> {
        Ok(InformalSketch::new(
            statement,
            vec![SketchStep::hole("goal", "Prove the statement directly.", statement)],
        ))
    }
}

/// The context handed to the per-hole prover: which sketch step, which DAG node,
/// and the subgoal to discharge.
#[derive(Debug, Clone)]
pub struct HoleContext {
    pub step_id: String,
    pub node_id: String,
    pub subgoal: String,
}

/// A closed hole: the proof text that discharges the subgoal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoleProof {
    pub proof: String,
}

/// Attempts to close a single hole (the per-obligation prove path seam).
/// `Ok(Some(proof))` = closed; `Ok(None)` = the hole remains open (unproven).
pub trait HoleProver {
    fn prove(&self, ctx: &HoleContext) -> Result<Option<HoleProof>>;
}

/// The result of dispatching one hole to the prover.
#[derive(Debug, Clone)]
pub struct HoleResult {
    pub step_id: String,
    pub node_id: String,
    pub subgoal: String,
    pub closed: bool,
    pub proof: Option<String>,
}

/// The outcome of running a sketch: the sub-DAG root, every hole's result, and —
/// only when EVERY hole closed — the spliced assembled proof.
#[derive(Debug, Clone)]
pub struct SketchAssembly {
    /// The root `informal_proof` node the holes hang under.
    pub sketch_node_id: String,
    /// Per-hole results in sketch order.
    pub hole_results: Vec<HoleResult>,
    /// `Some` iff all holes closed — the spliced proof. `None` when refused.
    pub assembled_proof: Option<String>,
    /// Step ids of holes left open (empty iff assembled).
    pub open_holes: Vec<String>,
}

impl SketchAssembly {
    /// Whether assembly succeeded (all holes closed and a proof was spliced).
    pub fn is_assembled(&self) -> bool {
        self.assembled_proof.is_some()
    }
}

/// Drives the sketch → holes → splice pipeline against a proof-DAG store, with an
/// injected sketch generator and per-hole prover.
pub struct SketchPipeline<'a, G: SketchGenerator, P: HoleProver> {
    pub store: &'a Store,
    pub generator: &'a G,
    pub prover: &'a P,
}

impl<G: SketchGenerator, P: HoleProver> SketchPipeline<'_, G, P> {
    /// Generate a sketch for `statement`, then run it end to end.
    pub fn run(&self, project_id: &str, statement: &str) -> Result<SketchAssembly> {
        let sketch = self.generator.generate(statement)?;
        self.run_sketch(project_id, &sketch)
    }

    /// Run a pre-built sketch: materialise the sub-DAG, dispatch every hole, and
    /// splice on full closure (refuse otherwise).
    pub fn run_sketch(&self, project_id: &str, sketch: &InformalSketch) -> Result<SketchAssembly> {
        // 1. Root node representing the informal sketch.
        let root = self.store.add_node_detailed(
            project_id,
            NodeKind::InformalProof,
            NodeTier::Spine,
            None,
            "Proof sketch",
            &sketch.statement,
            None,
            &[],
            "sketch:root",
        )?;

        // 2. One obligation node per hole; record step-id → node-id.
        let mut node_for_step: HashMap<String, String> = HashMap::new();
        for step in sketch.holes() {
            let hole = step.hole.as_ref().expect("filtered to hole-bearing steps");
            // The step prose is untrusted model text; carry it only as a hint.
            let hint = crate::guard::wrap_untrusted("sketch_step", &step.prose);
            let child = self.store.add_node_detailed(
                project_id,
                NodeKind::Obligation,
                NodeTier::Implementation,
                Some(&root.id),
                &format!("Hole: {}", step.id),
                &hole.subgoal,
                Some(&hint),
                &[],
                "sketch:hole",
            )?;
            // The sketch root depends on each of its hole obligations.
            self.store
                .add_edge(project_id, &root.id, &child.id, EdgeKind::DependsOn)?;
            node_for_step.insert(step.id.clone(), child.id);
        }

        // 3. `\uses` edges between hole steps (a hole depends on earlier holes it
        //    uses). Non-hole `uses` targets and unknown ids are skipped.
        for step in sketch.holes() {
            let Some(src) = node_for_step.get(&step.id) else {
                continue;
            };
            for used in &step.uses {
                if let Some(dst) = node_for_step.get(used) {
                    if dst != src {
                        self.store
                            .add_edge(project_id, src, dst, EdgeKind::DependsOn)?;
                    }
                }
            }
        }

        // 4. Dispatch every hole to the per-hole prover, recording the outcome on
        //    its node. A closed hole is certified; an open hole is left blocked.
        let mut hole_results = Vec::new();
        let mut open_holes = Vec::new();
        for step in sketch.holes() {
            let node_id = node_for_step[&step.id].clone();
            let subgoal = step.hole.as_ref().unwrap().subgoal.clone();
            let ctx = HoleContext {
                step_id: step.id.clone(),
                node_id: node_id.clone(),
                subgoal: subgoal.clone(),
            };
            let proved = self.prover.prove(&ctx)?;
            match &proved {
                Some(HoleProof { proof }) => {
                    self.store
                        .set_formal_statement(project_id, &node_id, proof, "sketch:hole")?;
                    self.store.add_evidence(
                        project_id,
                        &node_id,
                        "sketch_hole",
                        "sketch_prover",
                        "closed",
                        json!({ "step_id": step.id }),
                    )?;
                    self.store.set_node_status(
                        project_id,
                        &node_id,
                        NodeStatus::FormallyVerified,
                        "sketch_prover",
                    )?;
                }
                None => {
                    self.store.add_evidence(
                        project_id,
                        &node_id,
                        "sketch_hole",
                        "sketch_prover",
                        "open",
                        json!({ "step_id": step.id }),
                    )?;
                    self.store.set_node_status(
                        project_id,
                        &node_id,
                        NodeStatus::Blocked,
                        "sketch_prover",
                    )?;
                    open_holes.push(step.id.clone());
                }
            }
            hole_results.push(HoleResult {
                step_id: step.id.clone(),
                node_id,
                subgoal,
                closed: proved.is_some(),
                proof: proved.map(|p| p.proof),
            });
        }

        // 5. Splice — but only when every hole closed. Otherwise refuse and
        //    surface the open holes (no partial/fake assembly).
        let assembled_proof = if open_holes.is_empty() {
            let proof = self.splice(sketch, &hole_results);
            self.store
                .set_formal_statement(project_id, &root.id, &proof, "sketch:assembly")?;
            self.store.set_node_status(
                project_id,
                &root.id,
                NodeStatus::FormallyVerified,
                "sketch_assembly",
            )?;
            self.store.event(
                Some(project_id),
                None,
                "sketch.assembled",
                "sketch_assembly",
                json!({ "sketch_node": root.id, "holes": hole_results.len() }),
            )?;
            Some(proof)
        } else {
            self.store.event(
                Some(project_id),
                None,
                "sketch.assembly_refused",
                "sketch_assembly",
                json!({ "sketch_node": root.id, "open_holes": open_holes }),
            )?;
            None
        };

        Ok(SketchAssembly {
            sketch_node_id: root.id,
            hole_results,
            assembled_proof,
            open_holes,
        })
    }

    /// Splice the closed-hole proofs back into the ordered sketch: each step's
    /// prose in order, with a hole step's proof inlined beneath it. Only called
    /// once every hole is closed, so every hole result carries a proof.
    fn splice(&self, sketch: &InformalSketch, results: &[HoleResult]) -> String {
        let proof_for: HashMap<&str, &str> = results
            .iter()
            .filter_map(|r| r.proof.as_deref().map(|p| (r.step_id.as_str(), p)))
            .collect();
        let mut out = String::new();
        out.push_str(&format!("-- Proof of: {}\n", sketch.statement));
        for step in &sketch.steps {
            out.push_str(&format!("-- Step {}: {}\n", step.id, step.prose));
            if step.hole.is_some() {
                if let Some(proof) = proof_for.get(step.id.as_str()) {
                    out.push_str(proof);
                    if !proof.ends_with('\n') {
                        out.push('\n');
                    }
                }
            }
        }
        out
    }
}

/// A per-hole prover that dispatches each subgoal through the existing portfolio
/// prove path ([`crate::portfolio::portfolio_prove`]) — the "existing
/// per-obligation prove path" in production. Injected like any other
/// [`HoleProver`], so the pipeline stays testable with mocks.
pub struct PortfolioHoleProver<'a> {
    pub store: &'a Store,
    pub config: &'a crate::config::Config,
    pub provider: &'a dyn ModelProvider,
    /// The formal systems to attempt per hole (empty = all three).
    pub systems: Vec<crate::prover::formal::FormalSystem>,
}

impl HoleProver for PortfolioHoleProver<'_> {
    fn prove(&self, ctx: &HoleContext) -> Result<Option<HoleProof>> {
        let result = crate::portfolio::portfolio_prove(
            self.store,
            self.config,
            self.provider,
            &ctx.subgoal,
            &self.systems,
        )?;
        // Take the winning system's verified source, if any.
        let closed = result
            .per_system
            .into_iter()
            .find(|a| a.verified)
            .and_then(|a| a.code);
        Ok(closed.map(|proof| HoleProof { proof }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NodeStatus;
    use std::cell::RefCell;
    use std::path::Path;

    /// A generator that returns a fixed 3-step sketch with 2 holes (step `s2`
    /// uses `s1`).
    struct FixedGenerator;
    impl SketchGenerator for FixedGenerator {
        fn generate(&self, statement: &str) -> Result<InformalSketch> {
            Ok(InformalSketch::new(
                statement,
                vec![
                    SketchStep::hole("s1", "Establish the base case", "base : P 0"),
                    SketchStep::hole("s2", "Induction step", "step : ∀ n, P n → P (n+1)").using(["s1"]),
                    SketchStep::prose("s3", "Conclude by induction"),
                ],
            ))
        }
    }

    /// A prover that closes every hole with a canned proof.
    struct AllClosingProver;
    impl HoleProver for AllClosingProver {
        fn prove(&self, ctx: &HoleContext) -> Result<Option<HoleProof>> {
            Ok(Some(HoleProof {
                proof: format!("theorem {} := by decide", ctx.step_id),
            }))
        }
    }

    /// A prover that closes every hole EXCEPT one named step id.
    struct FailingProver {
        fail_step: String,
    }
    impl HoleProver for FailingProver {
        fn prove(&self, ctx: &HoleContext) -> Result<Option<HoleProof>> {
            if ctx.step_id == self.fail_step {
                Ok(None)
            } else {
                Ok(Some(HoleProof {
                    proof: format!("theorem {} := by decide", ctx.step_id),
                }))
            }
        }
    }

    #[test]
    fn all_holes_close_then_proof_is_assembled_and_subnodes_certified() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "P holds for all n").unwrap();
        let pipeline = SketchPipeline {
            store: &store,
            generator: &FixedGenerator,
            prover: &AllClosingProver,
        };
        let assembly = pipeline.run(&project.id, "P holds for all n").unwrap();

        // Two holes closed, an assembled proof returned.
        assert!(assembly.is_assembled());
        assert!(assembly.open_holes.is_empty());
        assert_eq!(assembly.hole_results.len(), 2);
        assert!(assembly.hole_results.iter().all(|r| r.closed));
        let proof = assembly.assembled_proof.unwrap();
        // The splice inlines both hole proofs, in order, under the prose.
        assert!(proof.contains("theorem s1"));
        assert!(proof.contains("theorem s2"));
        assert!(proof.contains("Step s3"));

        // Every hole sub-node is certified, and so is the sketch root.
        let nodes = store.nodes(&project.id).unwrap();
        let obligations: Vec<_> = nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Obligation)
            .collect();
        assert_eq!(obligations.len(), 2);
        assert!(obligations
            .iter()
            .all(|n| n.status == NodeStatus::FormallyVerified));
        let root = nodes
            .iter()
            .find(|n| n.id == assembly.sketch_node_id)
            .unwrap();
        assert_eq!(root.status, NodeStatus::FormallyVerified);
        assert_eq!(root.formal_statement.as_deref(), Some(proof.as_str()));

        // The `\uses` edge (s2 → s1) plus the two root→hole edges = 3 edges.
        let edges = store.edges(&project.id).unwrap();
        assert_eq!(edges.len(), 3);
    }

    #[test]
    fn one_open_hole_refuses_assembly_and_surfaces_the_failure() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "P holds for all n").unwrap();
        let pipeline = SketchPipeline {
            store: &store,
            generator: &FixedGenerator,
            prover: &FailingProver {
                fail_step: "s2".into(),
            },
        };
        let assembly = pipeline.run(&project.id, "P holds for all n").unwrap();

        // Assembly is refused; the open hole is surfaced.
        assert!(!assembly.is_assembled());
        assert!(assembly.assembled_proof.is_none());
        assert_eq!(assembly.open_holes, vec!["s2".to_string()]);

        // The closed hole is certified; the open one is Blocked (not certified),
        // and the sketch root is never certified.
        let nodes = store.nodes(&project.id).unwrap();
        let root = nodes
            .iter()
            .find(|n| n.id == assembly.sketch_node_id)
            .unwrap();
        assert_ne!(root.status, NodeStatus::FormallyVerified);
        let s1 = &assembly.hole_results[0];
        let s2 = &assembly.hole_results[1];
        assert!(s1.closed);
        assert!(!s2.closed);

        // The refusal was surfaced as an auditable event.
        let events = store.events(&project.id, 100).unwrap();
        assert!(events
            .iter()
            .any(|e| e.event_type == "sketch.assembly_refused"));
    }

    #[test]
    fn prover_is_dispatched_once_per_hole_in_order() {
        // Verifies each hole is dispatched to the per-hole prove path exactly once.
        struct RecordingProver {
            seen: RefCell<Vec<String>>,
        }
        impl HoleProver for RecordingProver {
            fn prove(&self, ctx: &HoleContext) -> Result<Option<HoleProof>> {
                self.seen.borrow_mut().push(ctx.step_id.clone());
                Ok(Some(HoleProof {
                    proof: "trivial".into(),
                }))
            }
        }
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "t").unwrap();
        let prover = RecordingProver {
            seen: RefCell::new(Vec::new()),
        };
        let pipeline = SketchPipeline {
            store: &store,
            generator: &FixedGenerator,
            prover: &prover,
        };
        pipeline.run(&project.id, "t").unwrap();
        assert_eq!(prover.seen.into_inner(), vec!["s1".to_string(), "s2".to_string()]);
    }
}
