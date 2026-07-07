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
