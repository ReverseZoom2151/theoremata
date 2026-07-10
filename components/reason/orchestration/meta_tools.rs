//! Meta-tool layer: the agent's *own* orchestration moves, exposed as callable,
//! inspectable tools (gap #4 from `docs/agentic-patterns-mining/A2`+`H3`).
//!
//! The mining found that the high-level moves an agent makes — *plan*, *critique*,
//! *re-decompose a failed sketch*, *spend/allocate budget*, *recall episodic
//! memory*, *self-review*, *abstain* — are all logic locked INSIDE the
//! orchestration loop ([`crate::agent`], [`crate::blueprint_generate`],
//! [`crate::critic`], [`crate::refine_ops`], [`crate::ttc`], [`crate::memory`]).
//! They can be *run* by the harness but never *chosen* by the model: they are not
//! tools it can invoke, and they cannot be advertised on the MCP server the way
//! the arithmetic/Lean tools are.
//!
//! This module turns each of those moves into a first-class [`MetaTool`]: a
//! `(name, description, JSON input schema, invoke)` quadruple, collected in a
//! [`MetaToolRegistry`]. The registry can [`describe_all`](MetaToolRegistry::describe_all)
//! itself for the model's tool list and for the MCP `tools/list` response, and it
//! can [`invoke`](MetaToolRegistry::invoke) a tool by name from a `tools/call`.
//!
//! ## The injected-seam discipline
//!
//! A meta-tool does NOT re-implement planning/critique/refinement. Each tool wraps
//! an *injected handler* — a closure the wiring layer supplies that calls the real
//! orchestration function (`plan_and_prove`, `Critic::critique`,
//! `reflective_redecompose`, `EpisodicMemory::*`, `TtcController::*`, …). Because
//! the handler is injected, this module:
//!
//! * compiles standalone (it depends only on `std` + `serde_json`, not on the
//!   heavy orchestration modules), and
//! * is unit-testable with deterministic mock handlers — no store, no model, no
//!   Lean toolchain.
//!
//! ## Determinism
//!
//! [`MetaToolKind`] carries a fixed canonical name / description / input schema per
//! kind (compile-time constants). The registry is a `BTreeMap` keyed by name, so
//! [`describe_all`](MetaToolRegistry::describe_all) and
//! [`names`](MetaToolRegistry::names) are stable (alphabetical) regardless of
//! registration order — no wall-clock, no RNG, no ambient state.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fmt;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Why a meta-tool call could not be completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetaToolError {
    /// No tool with this name is registered (analogue of MCP `unknown tool`).
    UnknownTool(String),
    /// The tool's injected handler rejected the call (bad args, or the wrapped
    /// orchestration function returned an error). Carries a human-readable reason.
    Invocation { tool: String, reason: String },
}

impl MetaToolError {
    /// Build an invocation error for `tool` with `reason`.
    pub fn invocation(tool: impl Into<String>, reason: impl Into<String>) -> Self {
        MetaToolError::Invocation {
            tool: tool.into(),
            reason: reason.into(),
        }
    }
}

impl fmt::Display for MetaToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MetaToolError::UnknownTool(name) => write!(f, "unknown meta-tool: {name}"),
            MetaToolError::Invocation { tool, reason } => {
                write!(f, "meta-tool '{tool}' failed: {reason}")
            }
        }
    }
}

impl std::error::Error for MetaToolError {}

/// The result of invoking a meta-tool: a JSON value on success (the wrapped
/// function's structured output), or a [`MetaToolError`].
pub type MetaResult = Result<Value, MetaToolError>;

/// The injected handler signature: `args -> result`. The wiring layer supplies one
/// per registered tool; it is the ONLY place that touches the real orchestration
/// functions, keeping this module free of those dependencies.
pub type Handler = Box<dyn Fn(&Value) -> MetaResult>;

// ---------------------------------------------------------------------------
// The meta-tool taxonomy
// ---------------------------------------------------------------------------

/// The fixed set of orchestration moves exposed as meta-tools. Each variant maps
/// to one wrapped orchestration seam (documented on [`MetaToolKind::wraps`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MetaToolKind {
    /// Produce a plan (blueprint / sub-lemma DAG) for an informal statement.
    Plan,
    /// Revise an existing plan from proving feedback (a refinement round).
    UpdatePlan,
    /// Run the adversarial critic on a node or on the whole plan/DAG.
    Critique,
    /// Reflectively re-decompose a failed sketch step into parts + a bridge.
    Redecompose,
    /// Query episodic memory (attempt log, taint, proof pool) for a project/node.
    Recall,
    /// Report remaining budget (peek — does not charge).
    Spend,
    /// Allocate compute for a goal from the remaining budget (may charge).
    Budget,
    /// Self-review a candidate/plan before committing to it.
    SelfReview,
    /// Declare a low-confidence abstention (a first-class terminal state).
    Abstain,
}

/// Every meta-tool kind, in canonical order (also the order the wiring layer
/// should register them and the order docs list them). `describe_all` orders by
/// name, not by this array, but this is the authoritative enumeration.
pub const ALL_KINDS: [MetaToolKind; 9] = [
    MetaToolKind::Plan,
    MetaToolKind::UpdatePlan,
    MetaToolKind::Critique,
    MetaToolKind::Redecompose,
    MetaToolKind::Recall,
    MetaToolKind::Spend,
    MetaToolKind::Budget,
    MetaToolKind::SelfReview,
    MetaToolKind::Abstain,
];

impl MetaToolKind {
    /// The stable tool name advertised to the model and used for dispatch.
    pub const fn name(self) -> &'static str {
        match self {
            MetaToolKind::Plan => "plan",
            MetaToolKind::UpdatePlan => "update_plan",
            MetaToolKind::Critique => "critique",
            MetaToolKind::Redecompose => "redecompose",
            MetaToolKind::Recall => "recall",
            MetaToolKind::Spend => "spend",
            MetaToolKind::Budget => "budget",
            MetaToolKind::SelfReview => "self_review",
            MetaToolKind::Abstain => "abstain",
        }
    }

    /// The recommended Python-worker op name (and the `worker.dispatch` selector)
    /// when this meta-tool is also surfaced through the tool worker / MCP server.
    /// Prefixed `meta_` so it never collides with an existing arithmetic/Lean op.
    pub const fn worker_op(self) -> &'static str {
        match self {
            MetaToolKind::Plan => "meta_plan",
            MetaToolKind::UpdatePlan => "meta_update_plan",
            MetaToolKind::Critique => "meta_critique",
            MetaToolKind::Redecompose => "meta_redecompose",
            MetaToolKind::Recall => "meta_recall",
            MetaToolKind::Spend => "meta_spend",
            MetaToolKind::Budget => "meta_budget",
            MetaToolKind::SelfReview => "meta_self_review",
            MetaToolKind::Abstain => "meta_abstain",
        }
    }

    /// Which orchestration seam this meta-tool wraps (for docs / provenance).
    pub const fn wraps(self) -> &'static str {
        match self {
            MetaToolKind::Plan => "orchestration::blueprint_generate::plan_and_prove (generate)",
            MetaToolKind::UpdatePlan => {
                "orchestration::blueprint_generate::BlueprintRefiner::refine"
            }
            MetaToolKind::Critique => "critique::critic::Critic::critique",
            MetaToolKind::Redecompose => "proving::refine_ops::reflective_redecompose",
            MetaToolKind::Recall => "critique::memory::EpisodicMemory::{recall_attempts,snapshot}",
            MetaToolKind::Spend => "search::ttc::TtcController::{spent,remaining}",
            MetaToolKind::Budget => "search::ttc::TtcController::{allocate,take}",
            MetaToolKind::SelfReview => "critique::critic (self-review pass)",
            MetaToolKind::Abstain => "orchestration::agent abstention (THEOREMATA_ABSTAIN_THRESHOLD)",
        }
    }

    /// One-line, model-facing description of what invoking this tool does.
    pub const fn description(self) -> &'static str {
        match self {
            MetaToolKind::Plan => {
                "Produce a PLAN for an informal statement: decompose it into a DAG of \
                 verifiable sub-lemmas culminating in the main theorem. Returns the plan \
                 and its coverage. Use this before attacking a hard theorem head-on."
            }
            MetaToolKind::UpdatePlan => {
                "REVISE an existing plan using proving feedback: decompose the lemmas that \
                 failed into sub-lemmas while keeping proved items intact. Returns the \
                 refined plan. Use when a plan partially failed and you want to try again."
            }
            MetaToolKind::Critique => {
                "Run the adversarial CRITIC on a node or the whole proof DAG: surface \
                 circular dependencies, unjustified gaps, over-general claims, and \
                 verified-without-evidence nodes, classified as critical-error vs \
                 justification-gap. Returns the findings."
            }
            MetaToolKind::Redecompose => {
                "Reflectively RE-DECOMPOSE a failed sketch step: split its subgoal into \
                 parts, add a bridging lemma that recombines them, and preserve every \
                 other (already-proved) step. Returns the new sketch. Use when in-place \
                 repair of a subgoal has stalled."
            }
            MetaToolKind::Recall => {
                "RECALL episodic memory for a project/node: prior plan attempts (the \
                 'do NOT try again' log), the node's taint verdict, and the ranked proof \
                 pool. Returns the unified snapshot. Use to avoid repeating a dead strategy."
            }
            MetaToolKind::Spend => {
                "Report the remaining test-time-compute BUDGET (spent, remaining, global \
                 cap) WITHOUT charging it. Use to decide whether an expensive move is \
                 affordable before committing to it."
            }
            MetaToolKind::Budget => {
                "ALLOCATE compute for a goal from the remaining budget as a function of its \
                 difficulty and prior attempts (width / rollouts / depth). May charge the \
                 allocation against the running total. Use to size the next search."
            }
            MetaToolKind::SelfReview => {
                "SELF-REVIEW a candidate proof or plan before committing: re-read it \
                 adversarially against the failure-mode rubric and report whether to \
                 proceed, revise, or abstain."
            }
            MetaToolKind::Abstain => {
                "Declare a low-confidence ABSTENTION: decline to certify rather than risk a \
                 wrong answer. Returns the recorded abstention (never marks the node \
                 rejected/failed). Use when confidence is below the abstention threshold."
            }
        }
    }

    /// The JSON-schema-ish `inputSchema` for this tool's `arguments`, matching the
    /// MCP tool-descriptor shape so it can be advertised and registered verbatim.
    pub fn input_schema(self) -> Value {
        match self {
            MetaToolKind::Plan => object_schema(
                json!({
                    "statement": {"type": "string", "description": "The informal statement to plan a proof for."},
                    "max_rounds": {"type": "integer", "description": "Generate/refine round cap (default policy value).", "minimum": 1},
                    "seed": {"type": "integer", "description": "Determinism seed for reproducible planning."}
                }),
                &["statement"],
            ),
            MetaToolKind::UpdatePlan => object_schema(
                json!({
                    "plan": {"type": "object", "description": "The current plan/blueprint to revise."},
                    "feedback": {"description": "The proving feedback (run report) driving the revision."},
                    "seed": {"type": "integer", "description": "Determinism seed for the refinement round."}
                }),
                &["plan", "feedback"],
            ),
            MetaToolKind::Critique => object_schema(
                json!({
                    "project_id": {"type": "string", "description": "Project whose DAG to critique."},
                    "node_id": {"type": ["string", "null"], "description": "Optional single node to focus on; omit for the whole DAG."},
                    "target": {"type": "string", "enum": ["node", "plan", "dag"], "description": "What to critique (default 'dag')."}
                }),
                &["project_id"],
            ),
            MetaToolKind::Redecompose => object_schema(
                json!({
                    "sketch": {"type": "object", "description": "The failed sketch (steps + holes)."},
                    "failing_step_id": {"type": "string", "description": "Id of the step whose subgoal failed to close."},
                    "subparts": {"type": "array", "items": {"type": "string"}, "description": "Optional explicit split; omit to derive it structurally."}
                }),
                &["sketch", "failing_step_id"],
            ),
            MetaToolKind::Recall => object_schema(
                json!({
                    "project_id": {"type": "string", "description": "Project to recall memory for."},
                    "node_id": {"type": ["string", "null"], "description": "Node whose taint verdict to include; omit for project-only recall."},
                    "n_best": {"type": "integer", "description": "How many ranked proof candidates to return (default policy value).", "minimum": 0}
                }),
                &["project_id"],
            ),
            MetaToolKind::Spend => object_schema(json!({}), &[]),
            MetaToolKind::Budget => object_schema(
                json!({
                    "difficulty": {"type": "number", "description": "Goal difficulty in [0,1]; harder gets more compute.", "minimum": 0.0, "maximum": 1.0},
                    "prior_attempts": {"type": "integer", "description": "How many times this goal was already attempted (retry escalation).", "minimum": 0},
                    "charge": {"type": "boolean", "description": "If true, charge the allocation against the running total; if false, peek only (default false)."}
                }),
                &["difficulty"],
            ),
            MetaToolKind::SelfReview => object_schema(
                json!({
                    "project_id": {"type": "string", "description": "Project the candidate belongs to."},
                    "node_id": {"type": ["string", "null"], "description": "Node under review; omit for a plan-level review."},
                    "candidate": {"description": "The candidate proof/plan text or object to review."}
                }),
                &["project_id"],
            ),
            MetaToolKind::Abstain => object_schema(
                json!({
                    "project_id": {"type": "string", "description": "Project the abstention is recorded against."},
                    "node_id": {"type": ["string", "null"], "description": "Node being abstained on; omit for a run-level abstention."},
                    "reason": {"type": "string", "description": "Why confidence is too low to certify."},
                    "confidence": {"type": "number", "description": "The (low) confidence that triggered the abstention.", "minimum": 0.0, "maximum": 1.0}
                }),
                &["reason"],
            ),
        }
    }

    /// Parse a tool name back into its kind (inverse of [`name`](MetaToolKind::name)).
    pub fn from_name(name: &str) -> Option<MetaToolKind> {
        ALL_KINDS.into_iter().find(|k| k.name() == name)
    }

    /// The full MCP-style descriptor `{name, description, inputSchema}` for this
    /// kind — the shape both the model's tool list and MCP `tools/list` consume.
    pub fn descriptor(self) -> Value {
        json!({
            "name": self.name(),
            "description": self.description(),
            "inputSchema": self.input_schema(),
        })
    }
}

/// Assemble a JSON-Schema object node from its `properties` and `required` names.
fn object_schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}

// ---------------------------------------------------------------------------
// The MetaTool trait + a closure-backed implementor
// ---------------------------------------------------------------------------

/// A single callable, inspectable meta-tool. Implementors carry the canonical
/// name / description / schema of their [`MetaToolKind`] and an [`invoke`] that
/// dispatches to the wrapped orchestration seam.
pub trait MetaTool {
    /// The kind this tool exposes.
    fn kind(&self) -> MetaToolKind;

    /// The advertised tool name.
    fn name(&self) -> &'static str {
        self.kind().name()
    }

    /// The model-facing description.
    fn description(&self) -> &'static str {
        self.kind().description()
    }

    /// The `arguments` JSON schema.
    fn input_schema(&self) -> Value {
        self.kind().input_schema()
    }

    /// The MCP-style `{name, description, inputSchema}` descriptor.
    fn descriptor(&self) -> Value {
        self.kind().descriptor()
    }

    /// Invoke the tool with `args`, dispatching to the wrapped seam.
    fn invoke(&self, args: &Value) -> MetaResult;
}

/// A [`MetaTool`] whose behaviour is an injected closure — the wiring layer's seam
/// onto the real orchestration function. This is what keeps the module standalone.
pub struct FnMetaTool {
    kind: MetaToolKind,
    handler: Handler,
}

impl FnMetaTool {
    /// Wrap `handler` (a call into the real orchestration function) as the given
    /// meta-tool `kind`.
    pub fn new<F>(kind: MetaToolKind, handler: F) -> Self
    where
        F: Fn(&Value) -> MetaResult + 'static,
    {
        Self {
            kind,
            handler: Box::new(handler),
        }
    }
}

impl MetaTool for FnMetaTool {
    fn kind(&self) -> MetaToolKind {
        self.kind
    }

    fn invoke(&self, args: &Value) -> MetaResult {
        (self.handler)(args)
    }
}

// ---------------------------------------------------------------------------
// The registry
// ---------------------------------------------------------------------------

/// The set of meta-tools the model may call. Keyed by name in a `BTreeMap` so
/// enumeration ([`describe_all`](Self::describe_all), [`names`](Self::names)) is
/// deterministic (alphabetical) regardless of registration order.
///
/// Built by the wiring layer, which registers one injected handler per tool. This
/// module never constructs the handlers itself — that is the seam that keeps it
/// free of the heavy orchestration dependencies and unit-testable with mocks.
#[derive(Default)]
pub struct MetaToolRegistry {
    tools: BTreeMap<&'static str, Box<dyn MetaTool>>,
}

impl MetaToolRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self {
            tools: BTreeMap::new(),
        }
    }

    /// Register `tool`, returning the previous tool of the same name if one was
    /// already present (last registration wins).
    pub fn register(&mut self, tool: Box<dyn MetaTool>) -> Option<Box<dyn MetaTool>> {
        self.tools.insert(tool.name(), tool)
    }

    /// Register a closure-backed tool for `kind` (the common case).
    pub fn register_fn<F>(&mut self, kind: MetaToolKind, handler: F) -> &mut Self
    where
        F: Fn(&Value) -> MetaResult + 'static,
    {
        self.register(Box::new(FnMetaTool::new(kind, handler)));
        self
    }

    /// Builder form of [`register_fn`](Self::register_fn) for fluent wiring.
    pub fn with<F>(mut self, kind: MetaToolKind, handler: F) -> Self
    where
        F: Fn(&Value) -> MetaResult + 'static,
    {
        self.register_fn(kind, handler);
        self
    }

    /// Whether a tool with this name is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// How many tools are registered.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// The registered tool names, sorted (deterministic).
    pub fn names(&self) -> Vec<&'static str> {
        self.tools.keys().copied().collect()
    }

    /// Look up a tool by name.
    pub fn get(&self, name: &str) -> Option<&dyn MetaTool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    /// Invoke the named tool with `args`. Returns [`MetaToolError::UnknownTool`]
    /// when no such tool is registered (the model asked for a tool that is not
    /// advertised), otherwise the tool's own result.
    pub fn invoke(&self, name: &str, args: &Value) -> MetaResult {
        match self.tools.get(name) {
            Some(tool) => tool.invoke(args),
            None => Err(MetaToolError::UnknownTool(name.to_owned())),
        }
    }

    /// The MCP-style descriptors for every registered tool, ordered by name.
    /// Drops straight into the model's tool list or an MCP `tools/list` response.
    /// Deterministic: same registry ⇒ identical output every call.
    pub fn describe_all(&self) -> Vec<Value> {
        self.tools.values().map(|t| t.descriptor()).collect()
    }

    /// `describe_all` wrapped as a single JSON array value, for the exact
    /// `{"tools": [...]}` payload shape of an MCP `tools/list` result.
    pub fn tools_list(&self) -> Value {
        Value::Array(self.describe_all())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A registry with every kind wired to a trivial echo handler — the shape the
    /// wiring layer produces, but with mock seams.
    fn echo_registry() -> MetaToolRegistry {
        let mut reg = MetaToolRegistry::new();
        for kind in ALL_KINDS {
            reg.register_fn(kind, move |args| {
                Ok(json!({ "tool": kind.name(), "echo": args.clone() }))
            });
        }
        reg
    }

    #[test]
    fn registry_lists_all_meta_tools_with_schemas() {
        let reg = echo_registry();
        assert_eq!(reg.len(), ALL_KINDS.len());

        // Every canonical kind is present, advertised, and carries a full schema.
        for kind in ALL_KINDS {
            assert!(reg.contains(kind.name()), "missing {}", kind.name());
            let tool = reg.get(kind.name()).expect("registered");
            let desc = tool.descriptor();
            assert_eq!(desc["name"], kind.name());
            assert!(desc["description"].as_str().is_some_and(|s| !s.is_empty()));
            assert_eq!(desc["inputSchema"]["type"], "object");
            assert!(desc["inputSchema"]["properties"].is_object());
            assert!(desc["inputSchema"]["required"].is_array());
        }

        // The advertised catalog covers exactly the registered tools.
        let advertised: Vec<String> = reg
            .describe_all()
            .iter()
            .map(|d| d["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(advertised.len(), ALL_KINDS.len());
        // Names cover the full canonical set (order is alphabetical, not enum).
        for kind in ALL_KINDS {
            assert!(advertised.iter().any(|n| n == kind.name()));
        }
    }

    #[test]
    fn each_invoke_dispatches_to_its_injected_handler() {
        let reg = echo_registry();
        for kind in ALL_KINDS {
            let out = reg
                .invoke(kind.name(), &json!({ "x": 1 }))
                .expect("invoke ok");
            // The result came from THIS tool's handler (name round-trips), and the
            // args were passed through untouched.
            assert_eq!(out["tool"], kind.name());
            assert_eq!(out["echo"], json!({ "x": 1 }));
        }
    }

    #[test]
    fn unknown_tool_errors() {
        let reg = echo_registry();
        let err = reg.invoke("does_not_exist", &json!({})).unwrap_err();
        assert_eq!(err, MetaToolError::UnknownTool("does_not_exist".into()));
        assert!(err.to_string().contains("does_not_exist"));
    }

    #[test]
    fn handler_errors_propagate_as_invocation_errors() {
        let mut reg = MetaToolRegistry::new();
        reg.register_fn(MetaToolKind::Plan, |_args| {
            Err(MetaToolError::invocation("plan", "generator refused"))
        });
        let err = reg.invoke("plan", &json!({})).unwrap_err();
        assert_eq!(
            err,
            MetaToolError::Invocation {
                tool: "plan".into(),
                reason: "generator refused".into(),
            }
        );
    }

    #[test]
    fn describe_all_is_deterministic_and_sorted() {
        let reg = echo_registry();
        let a = reg.describe_all();
        let b = reg.describe_all();
        assert_eq!(a, b, "same registry ⇒ identical catalog");

        // Ordering is alphabetical by name and independent of registration order.
        let names: Vec<&str> = a.iter().map(|d| d["name"].as_str().unwrap()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted, "describe_all must be name-sorted");

        // A registry built in a DIFFERENT registration order yields the SAME
        // catalog — determinism does not depend on insertion order.
        let mut rev = MetaToolRegistry::new();
        for kind in ALL_KINDS.into_iter().rev() {
            rev.register_fn(kind, move |args| {
                Ok(json!({ "tool": kind.name(), "echo": args.clone() }))
            });
        }
        assert_eq!(rev.describe_all(), a);
    }

    #[test]
    fn tools_list_matches_mcp_shape() {
        let reg = echo_registry();
        let list = reg.tools_list();
        let arr = list.as_array().expect("tools/list is an array");
        assert_eq!(arr.len(), ALL_KINDS.len());
        assert!(arr.iter().all(|d| d["inputSchema"]["type"] == "object"));
    }

    #[test]
    fn from_name_round_trips_every_kind() {
        for kind in ALL_KINDS {
            assert_eq!(MetaToolKind::from_name(kind.name()), Some(kind));
        }
        assert_eq!(MetaToolKind::from_name("nope"), None);
    }

    #[test]
    fn worker_ops_are_meta_prefixed_and_unique() {
        let mut ops: Vec<&str> = ALL_KINDS.iter().map(|k| k.worker_op()).collect();
        assert!(ops.iter().all(|op| op.starts_with("meta_")));
        ops.sort_unstable();
        let n = ops.len();
        ops.dedup();
        assert_eq!(ops.len(), n, "worker op names must be unique");
    }

    // -- A mock plan / critique / abstain round-trip through injected seams -----

    /// A stand-in for the real orchestration functions: deterministic, no store,
    /// no model. Records how many times each seam was called so the test can
    /// assert dispatch actually reached the injected handler.
    #[derive(Default)]
    struct MockSeams {
        planned: Cell<u32>,
        critiqued: Cell<u32>,
        abstained: Cell<u32>,
    }

    #[test]
    fn mock_plan_critique_abstain_round_trip() {
        // The wiring layer owns the seams; the closures borrow them. We use Rc so
        // the closures and the assertions can both see the call counts.
        use std::rc::Rc;
        let seams = Rc::new(MockSeams::default());

        let mut reg = MetaToolRegistry::new();
        {
            let s = Rc::clone(&seams);
            reg.register_fn(MetaToolKind::Plan, move |args| {
                s.planned.set(s.planned.get() + 1);
                let stmt = args["statement"]
                    .as_str()
                    .ok_or_else(|| MetaToolError::invocation("plan", "missing 'statement'"))?;
                // Mimics plan_and_prove: a tiny DAG + coverage.
                Ok(json!({
                    "plan": {"nodes": [format!("main: {stmt}")]},
                    "coverage": 0.0,
                    "fully_proved": false,
                }))
            });
        }
        {
            let s = Rc::clone(&seams);
            reg.register_fn(MetaToolKind::Critique, move |args| {
                s.critiqued.set(s.critiqued.get() + 1);
                let node = args["node_id"].as_str().unwrap_or("<dag>");
                Ok(json!({ "findings": [], "summary": format!("clean: {node}") }))
            });
        }
        {
            let s = Rc::clone(&seams);
            reg.register_fn(MetaToolKind::Abstain, move |args| {
                s.abstained.set(s.abstained.get() + 1);
                let reason = args["reason"]
                    .as_str()
                    .ok_or_else(|| MetaToolError::invocation("abstain", "missing 'reason'"))?;
                Ok(json!({ "abstained": true, "reason": reason }))
            });
        }

        // plan
        let plan = reg
            .invoke("plan", &json!({ "statement": "n + 0 = n" }))
            .unwrap();
        assert_eq!(plan["plan"]["nodes"][0], "main: n + 0 = n");
        assert_eq!(plan["fully_proved"], false);

        // critique
        let crit = reg
            .invoke("critique", &json!({ "project_id": "p", "node_id": "N1" }))
            .unwrap();
        assert_eq!(crit["summary"], "clean: N1");

        // abstain
        let ab = reg
            .invoke("abstain", &json!({ "reason": "confidence 0.2 < 0.5" }))
            .unwrap();
        assert_eq!(ab["abstained"], true);
        assert_eq!(ab["reason"], "confidence 0.2 < 0.5");

        // Each seam was reached exactly once via dispatch.
        assert_eq!(seams.planned.get(), 1);
        assert_eq!(seams.critiqued.get(), 1);
        assert_eq!(seams.abstained.get(), 1);

        // A bad-args call is surfaced as an invocation error, not a panic.
        let bad = reg.invoke("plan", &json!({})).unwrap_err();
        assert!(matches!(bad, MetaToolError::Invocation { .. }));
    }

    #[test]
    fn last_registration_wins() {
        let mut reg = MetaToolRegistry::new();
        reg.register_fn(MetaToolKind::Spend, |_| Ok(json!({ "v": 1 })));
        let prev = reg.register(Box::new(FnMetaTool::new(MetaToolKind::Spend, |_| {
            Ok(json!({ "v": 2 }))
        })));
        assert!(prev.is_some(), "the first Spend tool was replaced");
        assert_eq!(reg.len(), 1);
        assert_eq!(reg.invoke("spend", &json!({})).unwrap(), json!({ "v": 2 }));
    }
}
