use chrono::{DateTime, Utc};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Conjecture,
    Definition,
    Assumption,
    Strategy,
    Lemma,
    Obligation,
    Computation,
    Counterexample,
    InformalProof,
    FormalStatement,
    FormalProof,
    Evidence,
}

impl fmt::Display for NodeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_value(self).unwrap().as_str().unwrap()
        )
    }
}

impl FromStr for NodeKind {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(serde_json::from_value(serde_json::Value::String(
            s.to_owned(),
        ))?)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    Proposed,
    Active,
    Blocked,
    Rejected,
    InformallyVerified,
    FormallyVerified,
    Superseded,
}

impl fmt::Display for NodeStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_value(self).unwrap().as_str().unwrap()
        )
    }
}

impl FromStr for NodeStatus {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(serde_json::from_value(serde_json::Value::String(
            s.to_owned(),
        ))?)
    }
}

/// Whether a node is a blueprint-visible mathematical step (the human/review/
/// scheduling unit) or an agent-introduced sub-lemma owned by a parent spine
/// node. Completed formalizations show a ~4-5x fan-out of implementation nodes
/// beneath each spine node.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NodeTier {
    Spine,
    Implementation,
}

impl fmt::Display for NodeTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_value(self).unwrap().as_str().unwrap()
        )
    }
}

impl FromStr for NodeTier {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(serde_json::from_value(serde_json::Value::String(
            s.to_owned(),
        ))?)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    DependsOn,
    Supports,
    Contradicts,
    Formalizes,
    Verifies,
    DerivedFrom,
    Supersedes,
}

impl fmt::Display for EdgeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_value(self).unwrap().as_str().unwrap()
        )
    }
}

impl FromStr for EdgeKind {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(serde_json::from_value(serde_json::Value::String(
            s.to_owned(),
        ))?)
    }
}

/// How strongly a dependency edge is backed: numerics only *screen*, prose is a
/// human argument, Lean is machine-checked. Variants are declared ascending so
/// the derived ordering gives `lean_checked > prose_proof > numeric_screen`.
#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum EdgeStrength {
    NumericScreen,
    ProseProof,
    LeanChecked,
}

impl fmt::Display for EdgeStrength {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_value(self).unwrap().as_str().unwrap()
        )
    }
}

impl FromStr for EdgeStrength {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(serde_json::from_value(serde_json::Value::String(
            s.to_owned(),
        ))?)
    }
}

/// leanblueprint places `\uses` independently on a statement and inside its
/// proof, so a dependency edge is either a *statement* dependency (needed even
/// to state the claim), a *proof* dependency (needed only to close the proof),
/// or both (the key appears in both `\uses` lists). Legacy edges — created
/// before the split existed — default to `statement`, the conservative choice.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DepScope {
    Statement,
    Proof,
    Both,
}

impl DepScope {
    /// Merge two scopes: a key used at both statement and proof level becomes
    /// `Both`. Used when ingesting a blueprint node whose statement and proof
    /// both `\uses` the same target.
    pub fn merge(self, other: DepScope) -> DepScope {
        if self == other {
            self
        } else {
            DepScope::Both
        }
    }
}

impl fmt::Display for DepScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_value(self).unwrap().as_str().unwrap()
        )
    }
}

impl FromStr for DepScope {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(serde_json::from_value(serde_json::Value::String(
            s.to_owned(),
        ))?)
    }
}

/// MathResearchPrompts typed-claim taxonomy (arXiv:2512.09443, template §5): the
/// kind of assertion an obligation makes. Attaching it to obligations lets the
/// router/critic reason about how a claim should be checked (e.g. an
/// `Obstruction`/`Counterexample` claim is a disproof, a `Convergence` claim
/// wants a rate argument).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ClaimKind {
    Invariant,
    NormIdentity,
    ScalarRecursion,
    Spectral,
    Convergence,
    Stability,
    NormalForm,
    Obstruction,
    Counterexample,
}

impl ClaimKind {
    /// Parse a free-form label from a model ("norm identity", "Scalar_Recursion",
    /// "spectral property") leniently: normalise spaces/underscores to hyphens,
    /// lowercase, and drop a trailing descriptor word so "convergence guarantee"
    /// and "stability statement" resolve. Returns `None` for unrecognised text.
    pub fn from_label(label: &str) -> Option<ClaimKind> {
        let norm = label.trim().to_ascii_lowercase().replace([' ', '_'], "-");
        let canonical = match norm.as_str() {
            "invariant" => "invariant",
            "norm-identity" | "norm-identities" => "norm-identity",
            "scalar-recursion" | "scalar-recurrence" => "scalar-recursion",
            "spectral" | "spectral-property" => "spectral",
            "convergence" | "convergence-guarantee" => "convergence",
            "stability" | "stability-statement" => "stability",
            "normal-form" => "normal-form",
            "obstruction" => "obstruction",
            "counterexample" | "counterexample-family" => "counterexample",
            _ => return None,
        };
        serde_json::from_value(serde_json::Value::String(canonical.to_owned())).ok()
    }
}

impl fmt::Display for ClaimKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_value(self).unwrap().as_str().unwrap()
        )
    }
}

/// MathResearchPrompts transfer-schema ingredients (template §6): the structural
/// pieces a decomposer emits when reducing a convergence/optimality theorem —
/// reduce a theorem to (invariant subspace, progress coordinate, local update,
/// comparison inequality) and re-instantiate in a nearby setting.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TransferIngredient {
    InvariantSubspace,
    GradientPlane,
    ScalarProgressCoordinate,
    StructuredLocalUpdate,
    ComparisonInequality,
    AdmissibleUpdates,
}

impl TransferIngredient {
    /// Lenient parse of a free-form ingredient label (see `ClaimKind::from_label`).
    pub fn from_label(label: &str) -> Option<TransferIngredient> {
        let norm = label.trim().to_ascii_lowercase().replace([' ', '_'], "-");
        let canonical = match norm.as_str() {
            "invariant-subspace" | "working-invariant-subspace" => "invariant-subspace",
            "gradient-plane" | "tangent-plane" | "gradient-tangent-plane" => "gradient-plane",
            "scalar-progress-coordinate" | "progress-coordinate" => "scalar-progress-coordinate",
            "structured-local-update" | "local-update" => "structured-local-update",
            "comparison-inequality" => "comparison-inequality",
            "admissible-updates" => "admissible-updates",
            _ => return None,
        };
        serde_json::from_value(serde_json::Value::String(canonical.to_owned())).ok()
    }
}

impl fmt::Display for TransferIngredient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_value(self).unwrap().as_str().unwrap()
        )
    }
}

/// How finely the decomposer cuts a statement into obligations. The blueprint
/// DAG is a *dial*: strongpnt uses trivial micro-lemmas (the DAG does the
/// reasoning), ZkLinalg uses coarse paper-sized nodes. This controls both the
/// prompt guidance and the hidden-helper fan-out budget.
#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum Granularity {
    /// Few, paper-sized obligations (ZkLinalg-style, ~1.6x fan-out).
    Coarse,
    /// Balanced (the default, ~1.8x fan-out).
    #[default]
    Medium,
    /// Many micro-lemmas (strongpnt/Kakeya-style, ~2x fan-out).
    Fine,
}

impl Granularity {
    /// Measured un-blueprinted helper-decl fan-out multiplier per obligation:
    /// executors invent ~1.8x extra helper decls (Kakeya 2x, RHCurves/strongpnt
    /// 1.8x, ZkLinalg 1.6x). Finer granularity ⇒ more helpers per node.
    pub fn fanout_multiplier(self) -> f64 {
        match self {
            Granularity::Coarse => 1.6,
            Granularity::Medium => 1.8,
            Granularity::Fine => 2.0,
        }
    }
}

impl fmt::Display for Granularity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_value(self).unwrap().as_str().unwrap()
        )
    }
}

impl FromStr for Granularity {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(serde_json::from_value(serde_json::Value::String(
            s.to_owned(),
        ))?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub theorem: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub project_id: String,
    pub kind: NodeKind,
    pub status: NodeStatus,
    pub title: String,
    pub statement: String,
    pub formal_statement: Option<String>,
    pub provenance: String,
    pub content_hash: String,
    pub tainted: bool,
    pub tier: NodeTier,
    pub parent_id: Option<String>,
    pub strategy_hint: Option<String>,
    pub suggested_lemmas: Vec<String>,
    /// leanblueprint `\leanok` on the *statement*: the claim has been formalised
    /// in Lean (its type compiles), independent of whether its proof is done.
    pub stmt_formalized: bool,
    /// leanblueprint `\leanok` inside the *proof*: the proof is complete (no
    /// `sorry`). A node can be `stmt_formalized` but not `proof_done` — exactly
    /// the state a blueprint→formalize pipeline lives in.
    pub proof_done: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub id: i64,
    pub project_id: String,
    pub source_id: String,
    pub target_id: String,
    pub kind: EdgeKind,
    pub evidence_strength: EdgeStrength,
    /// Whether this dependency is required to *state* the target, to *prove* it,
    /// or both — the leanblueprint statement-vs-proof `\uses` split.
    pub dep_scope: DepScope,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: i64,
    pub project_id: Option<String>,
    pub run_id: Option<String>,
    pub event_type: String,
    pub actor: String,
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attempt {
    pub id: String,
    pub project_id: String,
    pub node_id: Option<String>,
    pub run_id: Option<String>,
    pub actor: String,
    pub input: serde_json::Value,
    pub output: serde_json::Value,
    pub success: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lemma {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub statement: String,
    pub source_node_id: String,
    pub taint: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: i64,
    pub project_id: String,
    pub role: String,
    pub content: String,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    pub id: String,
    pub project_id: String,
    pub action: serde_json::Value,
    pub status: String,
    pub proposed_by: String,
    pub resolution_note: Option<String>,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool: String,
    pub success: bool,
    pub summary: String,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u128,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRequest {
    pub role: String,
    pub task: String,
    pub context: serde_json::Value,
    pub output_schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelResponse {
    pub content: serde_json::Value,
    pub model: String,
    pub provider: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelStreamEvent {
    Started {
        provider: String,
    },
    Delta {
        text: String,
    },
    ToolIntent {
        name: String,
        input: serde_json::Value,
    },
    Completed {
        response: ModelResponse,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphExport {
    pub project: Project,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub events: Vec<Event>,
}
