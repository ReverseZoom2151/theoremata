//! Run-trace observability (agentic-patterns-mining gaps H5 / A5): the unified
//! **span tree**, **failure taxonomy**, and **attempt replay** that every mature
//! agent framework ships and Theoremata was missing.
//!
//! A [`RunTrace`] is a tree of [`Span`]s — one per unit of agent work (a plan, a
//! search, a model call, a tool call, a verification, a repair) — each nested
//! under the span that opened it. It is the single structured record of *what the
//! agent did*, in order, with parent/child structure and per-span status. From it
//! you can:
//!
//! * render a nested JSON tree for a viewer ([`RunTrace::to_tree`]),
//! * reconstruct the exact ordered step sequence of an attempt for replay
//!   ([`replay`]), and
//! * classify any failure into one of six standard classes
//!   ([`FailureTaxonomy::classify`]).
//!
//! ## Determinism contract
//!
//! There is **no wall-clock and no randomness** anywhere. Ordering comes from a
//! single monotonic *sequence counter*: every [`RunTrace::open_span`] and
//! [`RunTrace::close_span`] takes the next `seq` tick, so a span's `start_seq` /
//! `end_seq` are logical timestamps, not real ones. The counter is injectable
//! ([`RunTrace::with_start_seq`]) so a caller can splice a trace into a larger
//! sequence space. The same operations always produce a byte-identical trace,
//! which is what makes traces comparable, replayable, and testable.
//!
//! ## Persistence
//!
//! `trace.rs` deliberately depends on nothing but `serde` / `serde_json` / `std`:
//! it is a pure in-memory data structure. Durability rides on the existing
//! `events` table — a caller persists [`RunTrace::to_tree`] through
//! `Store::event(project, run, "run.trace", actor, tree)` exactly as
//! `plan_history` persists its entries, with no schema migration.

use serde::{Deserialize, Serialize};

/// The kind of work a span represents. `Root` is the synthetic top of a trace;
/// `Other` is the escape hatch for a stage that does not map onto the fixed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanKind {
    /// The whole run (synthetic root).
    Root,
    /// Planning / decomposition of an obligation.
    Plan,
    /// A proof search (best-first, MCGS, …).
    Search,
    /// A single model completion.
    ModelCall,
    /// A single external tool / worker invocation (Lean, Python, retrieval, …).
    ToolCall,
    /// A verification / certification pass (compile + axiom gate).
    Verify,
    /// A repair / retry of a failed artifact.
    Repair,
    /// Retrieval of candidate lemmas.
    Retrieve,
    /// A falsification / counterexample screen.
    Falsify,
    /// Anything not covered above.
    Other,
}

/// The terminal disposition of a span. A span is [`SpanStatus::Open`] until
/// [`RunTrace::close_span`] records its outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanStatus {
    /// Not yet closed.
    Open,
    /// Completed successfully.
    Ok,
    /// Completed with a failure.
    Failed,
    /// Abandoned before completion (e.g. a parent failed, budget exhausted).
    Aborted,
}

/// One node of a [`RunTrace`]: a unit of agent work with parent/child structure,
/// logical start/end sequence numbers, a status, and an optional free-form
/// detail (a short message or serialized signal).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    /// Stable, unique id (equal to the span's `start_seq`).
    pub id: u64,
    /// The id of the span this one was opened under, or `None` for a root.
    pub parent: Option<u64>,
    /// What kind of work this span represents.
    pub kind: SpanKind,
    /// A short human label (e.g. the node title or tool name).
    pub label: String,
    /// The monotonic sequence tick at which the span was opened.
    pub start_seq: u64,
    /// The monotonic sequence tick at which the span was closed, or `None` while
    /// still open.
    pub end_seq: Option<u64>,
    /// The span's disposition; [`SpanStatus::Open`] until closed.
    pub status: SpanStatus,
    /// Optional closing detail (a diagnosis, an error message, a small payload).
    pub detail: Option<String>,
}

/// A run trace: a forest of [`Span`]s built from a single monotonic sequence
/// counter. Spans are stored in creation order; the tree structure lives in each
/// span's `parent` link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunTrace {
    /// The next sequence tick to hand out.
    seq: u64,
    /// All spans, in the order they were opened.
    spans: Vec<Span>,
}

impl Default for RunTrace {
    fn default() -> Self {
        Self::new()
    }
}

impl RunTrace {
    /// A fresh trace whose sequence counter starts at `0`.
    pub fn new() -> Self {
        Self {
            seq: 0,
            spans: Vec::new(),
        }
    }

    /// A fresh trace whose sequence counter starts at `start` — lets a caller
    /// splice this trace into a larger logical sequence space while keeping
    /// determinism (no wall-clock).
    pub fn with_start_seq(start: u64) -> Self {
        Self {
            seq: start,
            spans: Vec::new(),
        }
    }

    /// Take the next monotonic sequence tick.
    fn tick(&mut self) -> u64 {
        let s = self.seq;
        self.seq += 1;
        s
    }

    /// Open a span of `kind` labelled `label` under `parent` (or `None` for a
    /// root). Returns the new span's id. The span starts [`SpanStatus::Open`]
    /// with no end sequence and no detail.
    pub fn open_span(
        &mut self,
        kind: SpanKind,
        label: impl Into<String>,
        parent: Option<u64>,
    ) -> u64 {
        let start_seq = self.tick();
        let id = start_seq; // start_seq is unique per open, so it is a valid id.
        self.spans.push(Span {
            id,
            parent,
            kind,
            label: label.into(),
            start_seq,
            end_seq: None,
            status: SpanStatus::Open,
            detail: None,
        });
        id
    }

    /// Close the span with id `id`, recording its `status` and optional `detail`.
    /// Takes a fresh sequence tick for the span's `end_seq`. A no-op (but the
    /// tick is still consumed) if `id` is unknown, so callers never panic on a
    /// stale id.
    pub fn close_span(&mut self, id: u64, status: SpanStatus, detail: Option<String>) {
        let end_seq = self.tick();
        if let Some(span) = self.spans.iter_mut().find(|s| s.id == id) {
            span.end_seq = Some(end_seq);
            span.status = status;
            span.detail = detail;
        }
    }

    /// All spans, in the order they were opened.
    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    /// Look up a span by id.
    pub fn span(&self, id: u64) -> Option<&Span> {
        self.spans.iter().find(|s| s.id == id)
    }

    /// The direct children of `id`, in open order.
    pub fn children(&self, id: u64) -> Vec<&Span> {
        self.spans
            .iter()
            .filter(|s| s.parent == Some(id))
            .collect()
    }

    /// Every span of the given `kind`, in open order — e.g. `find(SpanKind::Verify)`
    /// to pull all verification passes out of a trace.
    pub fn find(&self, kind: SpanKind) -> Vec<&Span> {
        self.spans.iter().filter(|s| s.kind == kind).collect()
    }

    /// The root spans (those with no parent), in open order.
    pub fn roots(&self) -> Vec<&Span> {
        self.spans.iter().filter(|s| s.parent.is_none()).collect()
    }

    /// Build the nested forest of [`TreeNode`]s: each root span with its children
    /// recursively attached, in open order. This is the structured form behind
    /// [`RunTrace::to_tree`].
    pub fn to_forest(&self) -> Vec<TreeNode> {
        self.roots()
            .into_iter()
            .map(|s| self.subtree(s.id))
            .collect()
    }

    /// Recursively assemble the subtree rooted at `id`.
    fn subtree(&self, id: u64) -> TreeNode {
        // `id` is always a real span here (called from roots()/children()).
        let span = self
            .span(id)
            .expect("subtree called with a live span id")
            .clone();
        let children = self
            .children(id)
            .into_iter()
            .map(|c| self.subtree(c.id))
            .collect();
        TreeNode { span, children }
    }

    /// Render the trace as a nested serde JSON forest for a viewer. Round-trips:
    /// the value deserializes back into `Vec<TreeNode>`.
    pub fn to_tree(&self) -> serde_json::Value {
        // Serializing an owned Vec of plain structs cannot fail.
        serde_json::to_value(self.to_forest()).unwrap_or(serde_json::Value::Null)
    }
}

/// A nested span node: a span plus its child subtrees. The serializable shape of
/// [`RunTrace::to_tree`] / [`RunTrace::to_forest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeNode {
    /// The span at this node.
    #[serde(flatten)]
    pub span: Span,
    /// Child subtrees, in open order.
    #[serde(default)]
    pub children: Vec<TreeNode>,
}

/// Whether a [`Step`] marks entering (opening) or exiting (closing) a span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepPhase {
    /// The span was opened.
    Enter,
    /// The span was closed.
    Exit,
}

/// One reconstructed step of an attempt replay: an enter or exit of a span at a
/// specific logical sequence tick. Ordering the full list by `seq` reproduces the
/// exact interleaving of the original run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// The logical sequence tick of this step (`start_seq` for an enter,
    /// `end_seq` for an exit).
    pub seq: u64,
    /// The span this step belongs to.
    pub span_id: u64,
    /// The span's kind (carried for convenience).
    pub kind: SpanKind,
    /// The span's label (carried for convenience).
    pub label: String,
    /// Whether the span was entered or exited at this step.
    pub phase: StepPhase,
    /// The span's status — only meaningful (and populated) on an [`StepPhase::Exit`].
    pub status: Option<SpanStatus>,
}

/// Reconstruct the ordered step sequence of a trace — attempt replay. Emits an
/// [`StepPhase::Enter`] at every span's `start_seq` and an [`StepPhase::Exit`] at
/// every closed span's `end_seq`, then sorts by `seq`. Because `seq` is the
/// monotonic tick assigned at open/close time, the result is the exact original
/// execution order, interleaving nested spans faithfully. Open (never-closed)
/// spans contribute only their enter step.
pub fn replay(trace: &RunTrace) -> Vec<Step> {
    let mut steps: Vec<Step> = Vec::with_capacity(trace.spans().len() * 2);
    for span in trace.spans() {
        steps.push(Step {
            seq: span.start_seq,
            span_id: span.id,
            kind: span.kind,
            label: span.label.clone(),
            phase: StepPhase::Enter,
            status: None,
        });
        if let Some(end) = span.end_seq {
            steps.push(Step {
                seq: end,
                span_id: span.id,
                kind: span.kind,
                label: span.label.clone(),
                phase: StepPhase::Exit,
                status: Some(span.status),
            });
        }
    }
    // Sequence ticks are unique across all opens and closes, so this total order
    // is unambiguous.
    steps.sort_by_key(|s| s.seq);
    steps
}

/// The six standard failure classes an agent framework buckets errors into. The
/// bucket, not the raw message, is what dashboards, retry policy, and the failure
/// taxonomy reason over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    /// The planner / decomposer produced no usable plan.
    Planning,
    /// A tool / worker execution failed (Lean crash, Python error, missing exe).
    ToolingExec,
    /// The verifier / kernel rejected the artifact (compile error, dirty axioms).
    VerificationReject,
    /// A budget was exhausted — a timeout or a resource limit.
    TimeoutResource,
    /// The model produced malformed / unparseable / schema-violating output.
    ModelFormat,
    /// Not attributable to any of the above.
    Unknown,
}

impl FailureClass {
    /// A stable snake_case tag for event payloads / dashboards.
    pub fn as_str(&self) -> &'static str {
        match self {
            FailureClass::Planning => "planning",
            FailureClass::ToolingExec => "tooling_exec",
            FailureClass::VerificationReject => "verification_reject",
            FailureClass::TimeoutResource => "timeout_resource",
            FailureClass::ModelFormat => "model_format",
            FailureClass::Unknown => "unknown",
        }
    }
}

/// Which layer of the agent a failure surfaced in — one of the structured signals
/// [`FailureTaxonomy::classify`] reads. Kept separate from [`SpanKind`] because a
/// failure's *attributed* layer is a coarser judgement than a span's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    /// The planner / decomposer.
    Plan,
    /// A tool / worker.
    Tool,
    /// A model completion.
    Model,
    /// The verifier / kernel.
    Verify,
    /// The search driver.
    Search,
    /// A repair / retry.
    Repair,
    /// Unattributed.
    Unknown,
}

/// The structured signals of a failure, from which a [`FailureClass`] is derived.
/// Callers set the booleans they can observe and the `layer` the failure came
/// from; the free-text `message` is retained for the record but is *not* pattern-
/// matched (classification is signal-driven, not string-driven).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorContext {
    /// Which layer the failure surfaced in.
    pub layer: Layer,
    /// A budget (time or resource) was exhausted.
    pub timed_out: bool,
    /// The model output failed to parse / violated its schema.
    pub format_error: bool,
    /// The verifier / kernel rejected the artifact.
    pub kernel_rejected: bool,
    /// The original error text, kept for the record (not matched against).
    pub message: String,
}

impl ErrorContext {
    /// A context carrying only a layer and message, with every boolean signal
    /// cleared — the common case where the classifier leans on the layer alone.
    pub fn from_layer(layer: Layer, message: impl Into<String>) -> Self {
        Self {
            layer,
            timed_out: false,
            format_error: false,
            kernel_rejected: false,
            message: message.into(),
        }
    }
}

/// The failure taxonomy: a pure classifier from structured [`ErrorContext`]
/// signals to a [`FailureClass`]. Stateless — a zero-sized policy object.
#[derive(Debug, Clone, Copy, Default)]
pub struct FailureTaxonomy;

impl FailureTaxonomy {
    /// Classify a failure. Specific cross-cutting signals win over the layer, in
    /// priority order:
    ///
    /// 1. `timed_out` ⇒ [`FailureClass::TimeoutResource`] (a budget failure looks
    ///    the same whatever layer it hit).
    /// 2. `kernel_rejected` ⇒ [`FailureClass::VerificationReject`].
    /// 3. `format_error` ⇒ [`FailureClass::ModelFormat`].
    /// 4. otherwise, the `layer`: Plan⇒Planning, Tool⇒ToolingExec,
    ///    Verify⇒VerificationReject, Model⇒ModelFormat, Repair⇒ToolingExec,
    ///    Search⇒Planning, Unknown⇒Unknown.
    pub fn classify(ctx: &ErrorContext) -> FailureClass {
        if ctx.timed_out {
            return FailureClass::TimeoutResource;
        }
        if ctx.kernel_rejected {
            return FailureClass::VerificationReject;
        }
        if ctx.format_error {
            return FailureClass::ModelFormat;
        }
        match ctx.layer {
            Layer::Plan => FailureClass::Planning,
            // A search that dead-ends is a planning-level failure (no path found).
            Layer::Search => FailureClass::Planning,
            Layer::Tool => FailureClass::ToolingExec,
            // A repair is a tool-driven rewrite; its raw failure is execution-class.
            Layer::Repair => FailureClass::ToolingExec,
            Layer::Verify => FailureClass::VerificationReject,
            Layer::Model => FailureClass::ModelFormat,
            Layer::Unknown => FailureClass::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a small representative trace:
    ///   root(Plan) ─ search(Search) ─ model(ModelCall)
    ///                              └─ verify(Verify)
    /// Returns the trace and the ids so tests can assert structure.
    fn sample_trace() -> (RunTrace, u64, u64, u64, u64) {
        let mut t = RunTrace::new();
        let root = t.open_span(SpanKind::Plan, "prove main", None);
        let search = t.open_span(SpanKind::Search, "best-first", Some(root));
        let model = t.open_span(SpanKind::ModelCall, "formalize", Some(search));
        t.close_span(model, SpanStatus::Ok, Some("compiled".into()));
        let verify = t.open_span(SpanKind::Verify, "axiom gate", Some(search));
        t.close_span(verify, SpanStatus::Failed, Some("dirty axioms".into()));
        t.close_span(search, SpanStatus::Failed, None);
        t.close_span(root, SpanStatus::Failed, None);
        (t, root, search, model, verify)
    }

    #[test]
    fn nested_spans_build_a_correct_tree() {
        let (t, root, search, model, verify) = sample_trace();

        // Parent/child links.
        assert_eq!(t.roots().len(), 1);
        assert_eq!(t.roots()[0].id, root);
        let root_children = t.children(root);
        assert_eq!(root_children.len(), 1);
        assert_eq!(root_children[0].id, search);
        let search_children = t.children(search);
        assert_eq!(search_children.len(), 2);
        assert_eq!(search_children[0].id, model); // open order preserved
        assert_eq!(search_children[1].id, verify);
        assert!(t.children(model).is_empty());

        // find(kind) pulls every span of a kind.
        assert_eq!(t.find(SpanKind::Verify).len(), 1);
        assert_eq!(t.find(SpanKind::Verify)[0].id, verify);
        assert_eq!(t.find(SpanKind::ModelCall)[0].label, "formalize");

        // The forest mirrors the tree shape.
        let forest = t.to_forest();
        assert_eq!(forest.len(), 1);
        assert_eq!(forest[0].span.id, root);
        assert_eq!(forest[0].children.len(), 1);
        assert_eq!(forest[0].children[0].children.len(), 2);
    }

    #[test]
    fn close_records_status_and_detail() {
        let (t, root, _search, model, verify) = sample_trace();

        let m = t.span(model).unwrap();
        assert_eq!(m.status, SpanStatus::Ok);
        assert_eq!(m.detail.as_deref(), Some("compiled"));
        assert!(m.end_seq.is_some());
        assert!(m.end_seq.unwrap() > m.start_seq);

        let v = t.span(verify).unwrap();
        assert_eq!(v.status, SpanStatus::Failed);
        assert_eq!(v.detail.as_deref(), Some("dirty axioms"));

        // The root closed too, with no detail.
        let r = t.span(root).unwrap();
        assert_eq!(r.status, SpanStatus::Failed);
        assert_eq!(r.detail, None);
    }

    #[test]
    fn open_span_leaves_span_open_until_closed() {
        let mut t = RunTrace::new();
        let id = t.open_span(SpanKind::ToolCall, "lean", None);
        assert_eq!(t.span(id).unwrap().status, SpanStatus::Open);
        assert_eq!(t.span(id).unwrap().end_seq, None);
        t.close_span(id, SpanStatus::Ok, None);
        assert_eq!(t.span(id).unwrap().status, SpanStatus::Ok);
        assert!(t.span(id).unwrap().end_seq.is_some());
    }

    #[test]
    fn closing_an_unknown_id_is_a_noop_not_a_panic() {
        let mut t = RunTrace::new();
        let id = t.open_span(SpanKind::Plan, "p", None);
        t.close_span(9999, SpanStatus::Ok, None); // stale id: ignored
        assert_eq!(t.span(id).unwrap().status, SpanStatus::Open);
    }

    #[test]
    fn to_tree_round_trips_json() {
        let (t, _r, _s, _m, _v) = sample_trace();

        // Structured forest -> JSON string -> back into Vec<TreeNode>.
        let forest = t.to_forest();
        let json = serde_json::to_string(&forest).unwrap();
        let back: Vec<TreeNode> = serde_json::from_str(&json).unwrap();
        assert_eq!(forest, back);

        // to_tree() is the same content as a serde_json::Value.
        let tree_value = t.to_tree();
        let from_value: Vec<TreeNode> = serde_json::from_value(tree_value).unwrap();
        assert_eq!(forest, from_value);

        // And the whole RunTrace round-trips too (it derives Serialize/Deserialize).
        let trace_json = serde_json::to_string(&t).unwrap();
        let trace_back: RunTrace = serde_json::from_str(&trace_json).unwrap();
        assert_eq!(t, trace_back);
    }

    #[test]
    fn classify_maps_representative_failures_to_the_right_class() {
        // Timeout wins regardless of layer.
        let mut ctx = ErrorContext::from_layer(Layer::Model, "took too long");
        ctx.timed_out = true;
        assert_eq!(FailureTaxonomy::classify(&ctx), FailureClass::TimeoutResource);

        // Kernel rejection.
        let mut ctx = ErrorContext::from_layer(Layer::Verify, "unknown identifier");
        ctx.kernel_rejected = true;
        assert_eq!(
            FailureTaxonomy::classify(&ctx),
            FailureClass::VerificationReject
        );

        // Malformed model output.
        let mut ctx = ErrorContext::from_layer(Layer::Model, "missing field 'lean'");
        ctx.format_error = true;
        assert_eq!(FailureTaxonomy::classify(&ctx), FailureClass::ModelFormat);

        // Plain layer-driven classifications (no cross-cutting signal set).
        assert_eq!(
            FailureTaxonomy::classify(&ErrorContext::from_layer(Layer::Plan, "no plan")),
            FailureClass::Planning
        );
        assert_eq!(
            FailureTaxonomy::classify(&ErrorContext::from_layer(Layer::Search, "no path")),
            FailureClass::Planning
        );
        assert_eq!(
            FailureTaxonomy::classify(&ErrorContext::from_layer(Layer::Tool, "python crashed")),
            FailureClass::ToolingExec
        );
        assert_eq!(
            FailureTaxonomy::classify(&ErrorContext::from_layer(Layer::Repair, "patch failed")),
            FailureClass::ToolingExec
        );
        assert_eq!(
            FailureTaxonomy::classify(&ErrorContext::from_layer(Layer::Verify, "compile error")),
            FailureClass::VerificationReject
        );
        assert_eq!(
            FailureTaxonomy::classify(&ErrorContext::from_layer(Layer::Model, "bad json")),
            FailureClass::ModelFormat
        );
        assert_eq!(
            FailureTaxonomy::classify(&ErrorContext::from_layer(Layer::Unknown, "???")),
            FailureClass::Unknown
        );

        // Priority: timeout beats a kernel rejection that is also flagged.
        let ctx = ErrorContext {
            layer: Layer::Verify,
            timed_out: true,
            format_error: true,
            kernel_rejected: true,
            message: "everything at once".into(),
        };
        assert_eq!(FailureTaxonomy::classify(&ctx), FailureClass::TimeoutResource);

        // Tags are stable.
        assert_eq!(FailureClass::VerificationReject.as_str(), "verification_reject");
    }

    #[test]
    fn replay_reproduces_the_step_order() {
        let (t, root, search, model, verify) = sample_trace();
        let steps = replay(&t);

        // Every open produced an enter; every close produced an exit.
        let enters = steps.iter().filter(|s| s.phase == StepPhase::Enter).count();
        let exits = steps.iter().filter(|s| s.phase == StepPhase::Exit).count();
        assert_eq!(enters, 4);
        assert_eq!(exits, 4);

        // The (span_id, phase) order reconstructs the exact run:
        //   enter root, enter search, enter model, exit model,
        //   enter verify, exit verify, exit search, exit root.
        let order: Vec<(u64, StepPhase)> =
            steps.iter().map(|s| (s.span_id, s.phase)).collect();
        assert_eq!(
            order,
            vec![
                (root, StepPhase::Enter),
                (search, StepPhase::Enter),
                (model, StepPhase::Enter),
                (model, StepPhase::Exit),
                (verify, StepPhase::Enter),
                (verify, StepPhase::Exit),
                (search, StepPhase::Exit),
                (root, StepPhase::Exit),
            ]
        );

        // seq is strictly increasing across the replay (a total logical order).
        for w in steps.windows(2) {
            assert!(w[0].seq < w[1].seq);
        }

        // Exit steps carry the closing status; enters do not.
        let model_exit = steps
            .iter()
            .find(|s| s.span_id == model && s.phase == StepPhase::Exit)
            .unwrap();
        assert_eq!(model_exit.status, Some(SpanStatus::Ok));
    }

    #[test]
    fn replay_of_an_open_span_emits_only_its_enter() {
        let mut t = RunTrace::new();
        let id = t.open_span(SpanKind::Search, "ongoing", None);
        let steps = replay(&t);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].span_id, id);
        assert_eq!(steps[0].phase, StepPhase::Enter);
        assert_eq!(steps[0].status, None);
    }

    #[test]
    fn traces_are_deterministic_seq_based_no_clock() {
        // The same operation sequence yields byte-identical traces every time —
        // there is no wall-clock or randomness to perturb them.
        let build = || {
            let mut t = RunTrace::new();
            let r = t.open_span(SpanKind::Plan, "p", None);
            let a = t.open_span(SpanKind::ToolCall, "lean", Some(r));
            t.close_span(a, SpanStatus::Ok, Some("ok".into()));
            let b = t.open_span(SpanKind::ModelCall, "m", Some(r));
            t.close_span(b, SpanStatus::Failed, None);
            t.close_span(r, SpanStatus::Failed, None);
            t
        };
        let t1 = build();
        let t2 = build();
        assert_eq!(t1, t2);
        assert_eq!(t1.to_tree(), t2.to_tree());
        assert_eq!(replay(&t1), replay(&t2));

        // Sequence ticks are exactly 0..n over 3 opens + 3 closes = 6 ticks.
        let max_seq = t1
            .spans()
            .iter()
            .flat_map(|s| [Some(s.start_seq), s.end_seq])
            .flatten()
            .max()
            .unwrap();
        assert_eq!(max_seq, 5);
    }

    #[test]
    fn injected_start_seq_shifts_the_logical_clock() {
        let mut t = RunTrace::with_start_seq(100);
        let id = t.open_span(SpanKind::Plan, "p", None);
        assert_eq!(id, 100);
        assert_eq!(t.span(id).unwrap().start_seq, 100);
        t.close_span(id, SpanStatus::Ok, None);
        assert_eq!(t.span(id).unwrap().end_seq, Some(101));
    }
}
