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
    informal_defect_prior::{analyze, RiskReport, RoutingHints},
    model::{EdgeKind, NodeKind, NodeStatus, NodeTier},
    prover::{formal::FormalSystem, model::VerificationReport},
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
            vec![SketchStep::hole(
                "goal",
                "Prove the statement directly.",
                statement,
            )],
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
    /// Chain-of-states feedback (ImProver pattern): the EXACT ground-truth local
    /// goal state at this hole, as dumped by the prover on a prior failed attempt.
    /// `None` on the first dispatch (and whenever no state could be extracted) —
    /// the prover then reasons from the subgoal/error alone, exactly as before.
    /// A retry threads the last known goal state in here so the prover reasons
    /// over the real intermediate state rather than only the error text.
    pub goal_state: Option<String>,
}

/// A closed hole: the proof text that discharges the subgoal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoleProof {
    pub proof: String,
}

/// A proof candidate plus the verifier result that authorizes it for use as a
/// certified sketch hole. Proof text alone is deliberately insufficient: a
/// model, mock backend, or lexical screen can produce text that looks valid
/// without a live checker ever accepting it.
#[derive(Debug, Clone)]
pub struct HoleVerification {
    pub system: FormalSystem,
    pub verification: VerificationReport,
}

impl HoleVerification {
    fn is_live_certification(&self) -> bool {
        let report = &self.verification;
        report.live
            && report.lexically_verified
            && report.axioms_clean
            && report.statement_preserved
            && report.lexical_clean
            && report.hardening_clean != Some(false)
    }
}

/// One prover attempt. Candidate text is retained for auditability even when
/// the optional verification is absent or fails the live-certification policy.
#[derive(Debug, Clone)]
pub struct HoleAttempt {
    pub proof: HoleProof,
    pub verification: Option<HoleVerification>,
}

impl HoleAttempt {
    fn is_live_certification(&self) -> bool {
        self.verification
            .as_ref()
            .is_some_and(HoleVerification::is_live_certification)
    }
}

/// A verifier result for the final spliced proof. Individual verified holes do
/// not prove that their composition parses, preserves the parent statement, or
/// has a clean kernel dependency set, so the root needs its own live check.
#[derive(Debug, Clone)]
pub struct VerifiedAssembly {
    pub system: FormalSystem,
    pub verification: VerificationReport,
}

impl VerifiedAssembly {
    fn is_live_certification(&self) -> bool {
        let report = &self.verification;
        report.live
            && report.lexically_verified
            && report.axioms_clean
            && report.statement_preserved
            && report.lexical_clean
            && report.hardening_clean != Some(false)
    }
}

/// Attempts to produce a candidate for one hole (the per-obligation prove-path
/// seam). Candidate text is not a closed hole until `prove_verified` attaches a
/// live, system-native verification report.
pub trait HoleProver {
    fn prove(&self, ctx: &HoleContext) -> Result<Option<HoleProof>>;

    /// Produce a hole proof together with a system-native verifier report.
    ///
    /// The default deliberately treats the legacy [`Self::prove`] result as an
    /// unverified candidate. This preserves deterministic mock/test provers,
    /// but prevents them from becoming `FormallyVerified` by accident.
    fn prove_verified(&self, ctx: &HoleContext) -> Result<Option<HoleAttempt>> {
        Ok(self.prove(ctx)?.map(|proof| HoleAttempt {
            proof,
            verification: None,
        }))
    }

    /// Verify the final source after all hole proofs have been spliced. The
    /// default is fail-closed because no existing injected interface can infer
    /// that independently checked fragments form a valid parent theorem.
    fn verify_assembled(&self, _statement: &str, _proof: &str) -> Result<Option<VerifiedAssembly>> {
        Ok(None)
    }

    /// The last failed attempt text for `ctx` — a failed proof term or the raw
    /// verifier error — used to drive goal-state extraction on a retry. Defaults
    /// to `None`: a prover that reports nothing here degrades the goal-state
    /// retry to today's error-only behaviour (the extractor is handed an empty
    /// attempt and typically returns `None`).
    fn last_attempt(&self, _ctx: &HoleContext) -> Option<String> {
        None
    }
}

/// Extracts the EXACT local goal state at a hole (ImProver "chain-of-states"):
/// given the subgoal and a failed `attempt`, the production impl asks the
/// Lean/prover to DUMP its ground-truth intermediate state so a retry can reason
/// over the real state, not just the error text. Injected as a trait so the
/// pipeline stays deterministic under test.
pub trait GoalStateExtractor {
    /// Dump the local goal state for `subgoal` given the failed `attempt`.
    /// `None` when no state can be extracted — the retry then degrades to
    /// today's error-only behaviour, unchanged.
    fn extract(&self, subgoal: &str, attempt: &str) -> Option<String>;
}

/// The production goal-state extractor: live extraction dumps the prover's state
/// via a real Lean/prover invocation, which is LIVE-GATED and not wired into this
/// build. It returns `None`, so threading it through is a safe no-op until the
/// live extractor lands — the flow degrades to error-only retries. Documented
/// stub; the mock in tests exercises the populated path.
pub struct StubGoalStateExtractor;

impl GoalStateExtractor for StubGoalStateExtractor {
    fn extract(&self, _subgoal: &str, _attempt: &str) -> Option<String> {
        // Live-gated: would shell out to `lean --run`/the prover to print the
        // goal state. Intentionally inert here.
        None
    }
}

/// The result of dispatching one hole to the prover.
#[derive(Debug, Clone)]
pub struct HoleResult {
    pub step_id: String,
    pub node_id: String,
    pub subgoal: String,
    /// Whether a live system-native verifier certified this hole. A returned
    /// candidate without that report is intentionally represented as open.
    pub closed: bool,
    pub proof: Option<String>,
    /// The last goal state threaded into this hole's (retried) context, if any.
    /// `None` when no retry ran or no state could be extracted.
    pub goal_state: Option<String>,
}

/// The outcome of running a sketch: the sub-DAG root, every hole's result, and
/// the spliced proof only when every hole and the final composition have a live
/// system-native verification result.
#[derive(Debug, Clone)]
pub struct SketchAssembly {
    /// The root `informal_proof` node the holes hang under.
    pub sketch_node_id: String,
    /// Per-hole results in sketch order.
    pub hole_results: Vec<HoleResult>,
    /// `Some` iff every hole and the final spliced source were live-verified.
    /// `None` when either the hole or root certification boundary rejects it.
    pub assembled_proof: Option<String>,
    /// Step ids of holes left open (empty iff assembled).
    pub open_holes: Vec<String>,
    /// The document-level informal-defect scan of this sketch, computed ONCE
    /// before any hole was dispatched. Advisory only: nothing in this pipeline
    /// reads it back, and no route, order, or certification decision depends on
    /// it. See [`SketchAssembly::defect_hints`] for the router-shaped view.
    pub defect_report: RiskReport,
    /// [`Self::defect_report`] split into the two router buckets. A future,
    /// deliberate change may let a router consume these to probe
    /// `falsify_first` regions with the cheap counterexample gate before
    /// spending proof effort; today they are only computed and recorded.
    pub defect_hints: RoutingHints,
}

impl SketchAssembly {
    /// Whether a live verifier certified the complete assembled proof.
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
    /// splice on full closure (refuse otherwise). No goal-state retry — a hole
    /// that fails to close stays open, exactly as before.
    pub fn run_sketch(&self, project_id: &str, sketch: &InformalSketch) -> Result<SketchAssembly> {
        self.run_sketch_inner(project_id, sketch, None, 0)
    }

    /// Chain-of-states variant of [`SketchPipeline::run`]: generate a sketch, then
    /// run it with goal-state feedback. When a hole fails to close, the injected
    /// [`GoalStateExtractor`] dumps the ground-truth local goal state, which is
    /// threaded into the hole context for up to `max_retries` re-dispatches so the
    /// prover reasons over the real intermediate state, not just the error text.
    /// If extraction returns `None`, the flow degrades to the error-only
    /// behaviour of [`SketchPipeline::run`], unchanged.
    pub fn run_with_goal_states(
        &self,
        project_id: &str,
        statement: &str,
        extractor: &dyn GoalStateExtractor,
        max_retries: u32,
    ) -> Result<SketchAssembly> {
        let sketch = self.generator.generate(statement)?;
        self.run_sketch_with_goal_states(project_id, &sketch, extractor, max_retries)
    }

    /// Chain-of-states variant of [`SketchPipeline::run_sketch`]. See
    /// [`SketchPipeline::run_with_goal_states`].
    pub fn run_sketch_with_goal_states(
        &self,
        project_id: &str,
        sketch: &InformalSketch,
        extractor: &dyn GoalStateExtractor,
        max_retries: u32,
    ) -> Result<SketchAssembly> {
        self.run_sketch_inner(project_id, sketch, Some(extractor), max_retries)
    }

    /// The shared sketch driver. `extractor`/`max_retries` are `None`/`0` for the
    /// legacy path (byte-for-byte prior behaviour) and populated for the
    /// chain-of-states path.
    fn run_sketch_inner(
        &self,
        project_id: &str,
        sketch: &InformalSketch,
        extractor: Option<&dyn GoalStateExtractor>,
        max_retries: u32,
    ) -> Result<SketchAssembly> {
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

        // 1b. Scan the informal text ONCE, document-level, before any hole node
        //     exists and long before any hole is dispatched — the hints have to
        //     be available to a future router at dispatch time, and a per-step
        //     scan would both miss cross-step spans and land too late.
        //
        //     This pass is advisory: it COMPUTES and RECORDS, and nothing below
        //     reads it back. Acting on the hints is a separate, deliberate step.
        let defect_text = defect_scan_text(sketch);
        let defect_report = analyze(&defect_text);
        let defect_hints = defect_report.to_routing_hints();
        // Recorded on the sketch root the same way per-hole metadata is recorded
        // on its hole node: an evidence row carrying the full structured payload.
        // Offsets in `findings`/regions index `defect_text` (the statement, then
        // each step's prose, newline-joined), NOT any single step's prose.
        self.store.add_evidence(
            project_id,
            &root.id,
            "sketch_defect_prior",
            "informal_defect_prior",
            "scanned",
            json!({
                "overall_risk": defect_hints.overall_risk,
                "findings": defect_report.findings,
                "falsify_first": defect_hints.falsify_first,
                "decompose_first": defect_hints.decompose_first,
                "scanned_bytes": defect_text.len(),
            }),
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
        //    its node. Candidate text is only certified after a live verifier
        //    report; otherwise it is retained as evidence and left blocked.
        let mut hole_results = Vec::new();
        let mut open_holes = Vec::new();
        for step in sketch.holes() {
            let node_id = node_for_step[&step.id].clone();
            let subgoal = step.hole.as_ref().unwrap().subgoal.clone();
            let mut ctx = HoleContext {
                step_id: step.id.clone(),
                node_id: node_id.clone(),
                subgoal: subgoal.clone(),
                goal_state: None,
            };
            let mut proved = self.prover.prove_verified(&ctx)?;

            // Chain-of-states retry: while the hole is open and a retry budget
            // remains, dump the ground-truth goal state from the last failed
            // attempt and thread it into the context before re-dispatching. If no
            // state can be extracted the loop stops immediately — degrading to the
            // error-only behaviour of the legacy path.
            if let Some(extractor) = extractor {
                let mut retries_left = max_retries;
                while proved.is_none() && retries_left > 0 {
                    let attempt = self.prover.last_attempt(&ctx).unwrap_or_default();
                    let Some(state) = extractor.extract(&subgoal, &attempt) else {
                        break;
                    };
                    ctx.goal_state = Some(state.clone());
                    // Record the recovered goal state as auditable evidence.
                    self.store.add_evidence(
                        project_id,
                        &node_id,
                        "sketch_hole_goal_state",
                        "goal_state_extractor",
                        "extracted",
                        json!({ "step_id": step.id, "goal_state": state }),
                    )?;
                    retries_left -= 1;
                    proved = self.prover.prove_verified(&ctx)?;
                }
            }

            let goal_state = ctx.goal_state.clone();
            let certified = proved
                .as_ref()
                .is_some_and(HoleAttempt::is_live_certification);
            let candidate = proved.as_ref().map(|attempt| &attempt.proof.proof);
            match (candidate, certified) {
                (Some(proof), true) => {
                    self.store
                        .set_formal_statement(project_id, &node_id, proof, "sketch:hole")?;
                    self.store.add_evidence(
                        project_id,
                        &node_id,
                        "sketch_hole",
                        "sketch_prover",
                        "live_verified",
                        json!({
                            "step_id": step.id,
                            "system": proved.as_ref().and_then(|attempt| attempt.verification.as_ref()).map(|verification| verification.system.as_str()),
                            "verification": proved.as_ref().and_then(|attempt| attempt.verification.as_ref()).map(|verification| &verification.verification.detail),
                        }),
                    )?;
                    self.store.set_node_status(
                        project_id,
                        &node_id,
                        NodeStatus::FormallyVerified,
                        "sketch_prover",
                    )?;
                }
                (Some(proof), false) => {
                    self.store.add_evidence(
                        project_id,
                        &node_id,
                        "sketch_hole",
                        "sketch_prover",
                        "unverified_candidate",
                        json!({
                            "step_id": step.id,
                            "proof": proof,
                            "verification_present": proved.as_ref().and_then(|attempt| attempt.verification.as_ref()).is_some(),
                        }),
                    )?;
                    self.store.set_node_status(
                        project_id,
                        &node_id,
                        NodeStatus::Blocked,
                        "sketch_prover_unverified",
                    )?;
                    open_holes.push(step.id.clone());
                }
                (None, _) => {
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
                closed: certified,
                proof: proved.map(|attempt| attempt.proof.proof),
                goal_state,
            });
        }

        // 5. Splice only after every hole was live-certified, then verify the
        //    complete source independently. Otherwise retain evidence but refuse
        //    certification (no partial/fake assembly).
        let assembled_proof = if open_holes.is_empty() {
            let proof = self.splice(sketch, &hole_results);
            let assembly_verification = self.prover.verify_assembled(&sketch.statement, &proof)?;
            if assembly_verification
                .as_ref()
                .is_some_and(VerifiedAssembly::is_live_certification)
            {
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
                    json!({
                        "sketch_node": root.id,
                        "holes": hole_results.len(),
                        "system": assembly_verification.as_ref().map(|verification| verification.system.as_str()),
                    }),
                )?;
                Some(proof)
            } else {
                self.store.add_evidence(
                    project_id,
                    &root.id,
                    "sketch_assembly",
                    "sketch_assembly",
                    "unverified_candidate",
                    json!({
                        "proof": proof,
                        "verification_present": assembly_verification.is_some(),
                    }),
                )?;
                self.store.set_node_status(
                    project_id,
                    &root.id,
                    NodeStatus::Blocked,
                    "sketch_assembly_unverified",
                )?;
                self.store.event(
                    Some(project_id),
                    None,
                    "sketch.assembly_unverified",
                    "sketch_assembly",
                    json!({ "sketch_node": root.id, "holes": hole_results.len() }),
                )?;
                None
            }
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
            defect_report,
            defect_hints,
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

/// The exact text handed to the informal-defect scanner: the statement followed
/// by every step's prose, newline-joined, in sketch order.
///
/// Document-level on purpose. Scanning per step would miss a notion introduced
/// in one step and never reused in a later one (the `IntroducedNotion`
/// detector's whole premise), and would produce a report per step rather than
/// one ordering over the sketch. The join is a plain `\n` so byte offsets stay
/// meaningful and the concatenation cannot fabricate a match across a boundary
/// that a space would have allowed.
fn defect_scan_text(sketch: &InformalSketch) -> String {
    let mut text = String::with_capacity(sketch.statement.len() + 64);
    text.push_str(&sketch.statement);
    for step in &sketch.steps {
        text.push('\n');
        text.push_str(&step.prose);
    }
    text
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
        Ok(self.prove_verified(ctx)?.map(|attempt| attempt.proof))
    }

    fn prove_verified(&self, ctx: &HoleContext) -> Result<Option<HoleAttempt>> {
        let result = crate::portfolio::portfolio_prove(
            self.store,
            self.config,
            self.provider,
            &ctx.subgoal,
            &self.systems,
        )?;
        // A portfolio's lexical/mock win is never a sketch certification. Only
        // an explicitly live, fully clean report may close a hole here.
        let closed = result.per_system.into_iter().find(|attempt| {
            attempt.report.as_ref().is_some_and(|report| {
                report.live
                    && report.lexically_verified
                    && report.axioms_clean
                    && report.statement_preserved
                    && report.lexical_clean
                    && report.hardening_clean != Some(false)
            }) && attempt.code.is_some()
        });
        Ok(closed.map(|attempt| HoleAttempt {
            proof: HoleProof {
                proof: attempt.code.expect("filtered to a code-bearing attempt"),
            },
            verification: attempt.report.map(|verification| HoleVerification {
                system: attempt.system,
                verification,
            }),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NodeStatus;
    use std::cell::RefCell;
    use std::path::Path;

    fn live_report() -> VerificationReport {
        VerificationReport {
            lexically_verified: true,
            axioms_clean: true,
            statement_preserved: true,
            lexical_clean: true,
            hardening_clean: Some(true),
            live: true,
            detail: json!({"fixture": "live-system-native"}),
        }
    }

    fn live_attempt(proof: HoleProof) -> HoleAttempt {
        HoleAttempt {
            proof,
            verification: Some(HoleVerification {
                system: FormalSystem::Lean,
                verification: live_report(),
            }),
        }
    }

    fn live_assembly() -> Option<VerifiedAssembly> {
        Some(VerifiedAssembly {
            system: FormalSystem::Lean,
            verification: live_report(),
        })
    }

    /// A generator that returns a fixed 3-step sketch with 2 holes (step `s2`
    /// uses `s1`).
    struct FixedGenerator;
    impl SketchGenerator for FixedGenerator {
        fn generate(&self, statement: &str) -> Result<InformalSketch> {
            Ok(InformalSketch::new(
                statement,
                vec![
                    SketchStep::hole("s1", "Establish the base case", "base : P 0"),
                    SketchStep::hole("s2", "Induction step", "step : ∀ n, P n → P (n+1)")
                        .using(["s1"]),
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

        fn prove_verified(&self, ctx: &HoleContext) -> Result<Option<HoleAttempt>> {
            Ok(self.prove(ctx)?.map(live_attempt))
        }

        fn verify_assembled(
            &self,
            _statement: &str,
            _proof: &str,
        ) -> Result<Option<VerifiedAssembly>> {
            Ok(live_assembly())
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

        fn prove_verified(&self, ctx: &HoleContext) -> Result<Option<HoleAttempt>> {
            Ok(self.prove(ctx)?.map(live_attempt))
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
    fn mock_hole_text_never_directly_certifies_a_hole_or_root() {
        struct TextOnlyProver;
        impl HoleProver for TextOnlyProver {
            fn prove(&self, _ctx: &HoleContext) -> Result<Option<HoleProof>> {
                Ok(Some(HoleProof {
                    proof: "by exact True.intro".into(),
                }))
            }
        }

        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "True").unwrap();
        let pipeline = SketchPipeline {
            store: &store,
            generator: &OneHoleGenerator,
            prover: &TextOnlyProver,
        };
        let assembly = pipeline.run(&project.id, "True").unwrap();

        assert!(!assembly.is_assembled());
        assert_eq!(assembly.open_holes, vec!["h1"]);
        assert_eq!(
            assembly.hole_results[0].proof.as_deref(),
            Some("by exact True.intro")
        );
        assert!(!assembly.hole_results[0].closed);
        assert!(store
            .nodes(&project.id)
            .unwrap()
            .iter()
            .all(|node| node.status != NodeStatus::FormallyVerified));
    }

    #[test]
    fn verified_holes_do_not_certify_an_unverified_spliced_root() {
        struct HolesOnlyProver;
        impl HoleProver for HolesOnlyProver {
            fn prove(&self, _ctx: &HoleContext) -> Result<Option<HoleProof>> {
                Ok(Some(HoleProof {
                    proof: "by exact True.intro".into(),
                }))
            }

            fn prove_verified(&self, ctx: &HoleContext) -> Result<Option<HoleAttempt>> {
                Ok(self.prove(ctx)?.map(live_attempt))
            }
        }

        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "True").unwrap();
        let pipeline = SketchPipeline {
            store: &store,
            generator: &OneHoleGenerator,
            prover: &HolesOnlyProver,
        };
        let assembly = pipeline.run(&project.id, "True").unwrap();

        assert!(!assembly.is_assembled());
        assert!(assembly.open_holes.is_empty());
        let nodes = store.nodes(&project.id).unwrap();
        assert_eq!(
            nodes
                .iter()
                .find(|node| node.kind == NodeKind::Obligation)
                .unwrap()
                .status,
            NodeStatus::FormallyVerified
        );
        assert_eq!(
            nodes
                .iter()
                .find(|node| node.id == assembly.sketch_node_id)
                .unwrap()
                .status,
            NodeStatus::Blocked
        );
        assert!(store
            .events(&project.id, 100)
            .unwrap()
            .iter()
            .any(|event| event.event_type == "sketch.assembly_unverified"));
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
        assert_eq!(
            prover.seen.into_inner(),
            vec!["s1".to_string(), "s2".to_string()]
        );
    }

    /// A generator with a single hole (keeps the goal-state tests focused).
    struct OneHoleGenerator;
    impl SketchGenerator for OneHoleGenerator {
        fn generate(&self, statement: &str) -> Result<InformalSketch> {
            Ok(InformalSketch::new(
                statement,
                vec![SketchStep::hole("h1", "discharge the goal", "goal : P 0")],
            ))
        }
    }

    /// A prover that CANNOT close a hole from the error alone (first attempt fails
    /// with `goal_state == None`), but succeeds once the ground-truth goal state
    /// has been threaded in. It records every `goal_state` it observed so the test
    /// can assert the extractor's state reached the retry context.
    struct GoalStateDependentProver {
        seen_states: RefCell<Vec<Option<String>>>,
    }
    impl HoleProver for GoalStateDependentProver {
        fn prove(&self, ctx: &HoleContext) -> Result<Option<HoleProof>> {
            self.seen_states.borrow_mut().push(ctx.goal_state.clone());
            match &ctx.goal_state {
                Some(state) => Ok(Some(HoleProof {
                    proof: format!("-- closed using state: {state}\nby simp"),
                })),
                None => Ok(None),
            }
        }

        fn prove_verified(&self, ctx: &HoleContext) -> Result<Option<HoleAttempt>> {
            Ok(self.prove(ctx)?.map(live_attempt))
        }

        fn verify_assembled(
            &self,
            _statement: &str,
            _proof: &str,
        ) -> Result<Option<VerifiedAssembly>> {
            Ok(live_assembly())
        }

        fn last_attempt(&self, _ctx: &HoleContext) -> Option<String> {
            Some("by simp -- failed: unsolved goals".into())
        }
    }

    /// A mock extractor that dumps a canned goal state (production would shell out
    /// to the prover). Asserts it is handed the failed attempt text.
    struct MockGoalStateExtractor {
        state: String,
    }
    impl GoalStateExtractor for MockGoalStateExtractor {
        fn extract(&self, _subgoal: &str, attempt: &str) -> Option<String> {
            assert!(
                !attempt.is_empty(),
                "the failed attempt is threaded to the extractor"
            );
            Some(self.state.clone())
        }
    }

    #[test]
    fn mock_extractors_goal_state_is_surfaced_into_the_retry_context() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "P 0").unwrap();
        let prover = GoalStateDependentProver {
            seen_states: RefCell::new(Vec::new()),
        };
        let pipeline = SketchPipeline {
            store: &store,
            generator: &OneHoleGenerator,
            prover: &prover,
        };
        let extractor = MockGoalStateExtractor {
            state: "⊢ P 0".into(),
        };
        let assembly = pipeline
            .run_with_goal_states(&project.id, "P 0", &extractor, 2)
            .unwrap();

        // The retry closed the hole and assembled the proof.
        assert!(assembly.is_assembled());
        assert_eq!(assembly.hole_results.len(), 1);
        let hole = &assembly.hole_results[0];
        assert!(hole.closed);
        // The EXACT extracted goal state was surfaced into the hole result...
        assert_eq!(hole.goal_state.as_deref(), Some("⊢ P 0"));
        // ...and into the retry context the prover actually saw (first attempt
        // None, second attempt carries the extractor's state).
        let seen = prover.seen_states.into_inner();
        assert_eq!(seen, vec![None, Some("⊢ P 0".to_string())]);
    }

    #[test]
    fn extraction_none_degrades_to_error_only_behaviour_unchanged() {
        // An extractor that never yields a state: the hole must stay open exactly
        // as it does on the legacy (no-goal-state) path — no retry closes it.
        struct EmptyExtractor;
        impl GoalStateExtractor for EmptyExtractor {
            fn extract(&self, _subgoal: &str, _attempt: &str) -> Option<String> {
                None
            }
        }
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "P 0").unwrap();
        let prover = GoalStateDependentProver {
            seen_states: RefCell::new(Vec::new()),
        };
        let pipeline = SketchPipeline {
            store: &store,
            generator: &OneHoleGenerator,
            prover: &prover,
        };
        let assembly = pipeline
            .run_with_goal_states(&project.id, "P 0", &EmptyExtractor, 3)
            .unwrap();

        // No state extracted ⇒ no retry succeeds ⇒ the hole is refused, identical
        // to the legacy error-only flow.
        assert!(!assembly.is_assembled());
        assert_eq!(assembly.open_holes, vec!["h1".to_string()]);
        assert_eq!(assembly.hole_results[0].goal_state, None);
        // The prover was dispatched exactly once (no goal-state retry fired).
        assert_eq!(prover.seen_states.into_inner(), vec![None]);
    }

    #[test]
    fn legacy_run_sketch_never_retries_with_goal_state() {
        // The stable `run` path must behave as before: one dispatch, hole open.
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "P 0").unwrap();
        let prover = GoalStateDependentProver {
            seen_states: RefCell::new(Vec::new()),
        };
        let pipeline = SketchPipeline {
            store: &store,
            generator: &OneHoleGenerator,
            prover: &prover,
        };
        let assembly = pipeline.run(&project.id, "P 0").unwrap();
        assert!(!assembly.is_assembled());
        assert_eq!(prover.seen_states.into_inner(), vec![None]);
    }

    // ---- informal-defect prior -------------------------------------------

    use crate::informal_defect_prior::DefectCategory;
    use crate::router::Route;

    /// A one-hole generator whose step prose is supplied by the test, so the
    /// defect scanner sees exactly the informal text under examination.
    struct ProseGenerator {
        prose: String,
    }
    impl SketchGenerator for ProseGenerator {
        fn generate(&self, statement: &str) -> Result<InformalSketch> {
            Ok(InformalSketch::new(
                statement,
                vec![SketchStep::hole("h1", self.prose.clone(), "goal : P 0")],
            ))
        }
    }

    /// The prose from the case study's headline defect.
    const RISKY_PROSE: &str =
        "The remaining cases may be checked directly for small n, so we are done.";

    /// Rigorous prose with nothing to flag.
    const CLEAN_PROSE: &str =
        "Multiplying both sides by the inverse of a modulo p yields the identity, \
         and the bound then follows from Lemma 3.2 with parameter t = 1/2.";

    fn run_with_prose(prose: &str) -> (Store, String, SketchAssembly) {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "P 0").unwrap();
        let generator = ProseGenerator {
            prose: prose.into(),
        };
        let assembly = {
            let pipeline = SketchPipeline {
                store: &store,
                generator: &generator,
                prover: &AllClosingProver,
            };
            pipeline.run(&project.id, "P 0").unwrap()
        };
        (store, project.id, assembly)
    }

    #[test]
    fn hand_waved_finite_check_in_prose_is_routed_to_falsification() {
        let (_store, _project, assembly) = run_with_prose(RISKY_PROSE);

        // The finding is present...
        assert!(
            assembly
                .defect_report
                .findings
                .iter()
                .any(|f| f.category == DefectCategory::HandWavedFiniteCheck),
            "no finite-check finding in {:?}",
            assembly.defect_report.findings
        );
        assert!(assembly.defect_report.score > 0.0);

        // ...and it lands in the falsify-first bucket, not the decompose one.
        let hints = &assembly.defect_hints;
        assert!(
            hints
                .falsify_first
                .iter()
                .any(|r| r.categories.contains(&DefectCategory::HandWavedFiniteCheck)),
            "finite check was not routed to falsification: {hints:?}"
        );
        for region in &hints.falsify_first {
            assert_eq!(region.route, Route::Falsify);
        }
        assert!(!hints
            .decompose_first
            .iter()
            .any(|r| r.categories.contains(&DefectCategory::HandWavedFiniteCheck)));
        assert_eq!(hints.overall_risk, assembly.defect_report.score);
    }

    #[test]
    fn clean_prose_produces_a_zero_risk_report_and_empty_hints() {
        let (_store, _project, assembly) = run_with_prose(CLEAN_PROSE);

        assert!(
            assembly.defect_report.findings.is_empty(),
            "clean prose produced findings: {:?}",
            assembly.defect_report.findings
        );
        assert_eq!(assembly.defect_report.score, 0.0);
        assert!(assembly.defect_hints.falsify_first.is_empty());
        assert!(assembly.defect_hints.decompose_first.is_empty());
        assert_eq!(assembly.defect_hints.overall_risk, 0.0);
    }

    #[test]
    fn defect_hints_are_recorded_on_the_sketch_root_before_any_hole_runs() {
        let (store, project, assembly) = run_with_prose(RISKY_PROSE);

        // The scan is recorded as evidence on the ROOT, and `add_evidence`
        // surfaces every row as an auditable event. Events come back newest
        // first, so the defect-prior row must appear AFTER (i.e. later in the
        // returned vector than) every per-hole row — it was written first, hence
        // before any hole was dispatched.
        let events = store.events(&project, 200).unwrap();
        let recorded: Vec<_> = events
            .iter()
            .filter(|e| e.event_type == "evidence.recorded")
            .collect();
        let prior_pos = recorded
            .iter()
            .position(|e| e.payload["evidence_type"] == "sketch_defect_prior")
            .expect("the defect scan was recorded as evidence");
        let hole_pos = recorded
            .iter()
            .position(|e| e.payload["evidence_type"] == "sketch_hole")
            .expect("the hole outcome was recorded as evidence");
        assert!(
            prior_pos > hole_pos,
            "defect hints were not recorded before the hole was dispatched"
        );
        assert_eq!(
            recorded[prior_pos].payload["node_id"],
            serde_json::Value::String(assembly.sketch_node_id.clone())
        );
    }

    #[test]
    fn recording_hints_does_not_change_what_the_pipeline_proves() {
        // Two runs identical in every way that can affect proving — same
        // statement, same subgoal, same prover — differing only in whether the
        // informal prose trips the defect scanner. The scan must be inert: the
        // holes, their proofs, the open set, and the node statuses must match.
        let (risky_store, risky_project, risky) = run_with_prose(RISKY_PROSE);
        let (clean_store, clean_project, clean) = run_with_prose(CLEAN_PROSE);

        // The scan DID see a difference...
        assert!(risky.defect_report.score > clean.defect_report.score);
        assert!(!risky.defect_hints.falsify_first.is_empty());
        assert!(clean.defect_hints.falsify_first.is_empty());

        // ...and it changed nothing about the proving.
        assert_eq!(risky.is_assembled(), clean.is_assembled());
        assert!(risky.is_assembled());
        assert_eq!(risky.open_holes, clean.open_holes);
        assert!(risky.open_holes.is_empty());
        assert_eq!(risky.hole_results.len(), clean.hole_results.len());
        for (r, c) in risky.hole_results.iter().zip(&clean.hole_results) {
            assert_eq!(r.step_id, c.step_id);
            assert_eq!(r.subgoal, c.subgoal);
            assert_eq!(r.closed, c.closed);
            assert_eq!(r.proof, c.proof);
            assert_eq!(r.goal_state, c.goal_state);
        }

        // Same DAG shape and same statuses on both sides.
        let statuses = |store: &Store, project: &str| {
            let mut out: Vec<_> = store
                .nodes(project)
                .unwrap()
                .iter()
                .map(|n| (n.kind, n.status))
                .collect();
            out.sort_by_key(|(kind, status)| (format!("{kind}"), format!("{status}")));
            out
        };
        assert_eq!(
            statuses(&risky_store, &risky_project),
            statuses(&clean_store, &clean_project)
        );
        assert_eq!(
            risky_store.edges(&risky_project).unwrap().len(),
            clean_store.edges(&clean_project).unwrap().len()
        );

        // The only prose-driven difference in the assembled proof is the prose
        // line itself; the spliced hole proof is byte-identical.
        let risky_proof = risky.assembled_proof.unwrap();
        let clean_proof = clean.assembled_proof.unwrap();
        assert!(risky_proof.contains("theorem h1 := by decide"));
        assert_eq!(
            risky_proof.replace(RISKY_PROSE, ""),
            clean_proof.replace(CLEAN_PROSE, "")
        );
    }
}
