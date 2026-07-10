//! Stable, versioned JSON API surface for the Theoremata core.
//!
//! This module is the single, documented contract an editor / MCP client calls
//! against. Unlike the broad ad-hoc CLI (which exposes every internal operation
//! and evolves freely), this surface is deliberately *small and stable*: a
//! versioned envelope wrapping a tagged request/response pair. Every request
//! variant maps one-to-one to an existing [`Store`] operation — this file adds
//! no new behaviour, only a fixed schema and a total (never-panicking)
//! dispatcher.
//!
//! All request content is treated as UNTRUSTED DATA: raw input is length-capped
//! before parsing, malformed / oversized / unknown input becomes a clean
//! [`ApiResponse::Error`], and no code path panics on bad input. There is no
//! wall-clock or randomness in this module.
//!
//! Wire shape (request):  `{"op":"get_node","project":"…","node":"…"}`
//! Wire shape (response): `{"version":"1","result":"node","node":{…}}`

use crate::db::Store;
use crate::model::{Edge, Node, NodeKind, NodeStatus, Project};
use serde::{Deserialize, Serialize};

/// The API contract version. Bump only on a breaking change to the request or
/// response schema; additive (backward-compatible) changes keep the version.
pub const API_VERSION: &str = "1";

/// Hard cap on a raw request payload. Anything larger is rejected before it is
/// parsed, so an editor / MCP client cannot force unbounded allocation.
const MAX_REQUEST_BYTES: usize = 256 * 1024;

/// Provenance recorded for graph mutations that arrive over the API.
const API_ACTOR: &str = "api";

/// The stable core request surface. Serde-internally-tagged on `op`; each
/// variant maps to exactly one [`Store`] read/act operation. Keep this small.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ApiRequest {
    /// Liveness probe. Maps to no Store call; always succeeds.
    Health,
    /// List every project, newest-updated first. → [`Store::list_projects`].
    ListProjects,
    /// Fetch a single project by id. → [`Store::project`].
    GetProject { project: String },
    /// List all nodes in a project, creation order. → [`Store::nodes`].
    ListNodes { project: String },
    /// Fetch one node by id within a project. → [`Store::nodes`] + filter.
    GetNode { project: String, node: String },
    /// Create a node (provenance `api`). → [`Store::add_node`].
    AddNode {
        project: String,
        kind: NodeKind,
        title: String,
        statement: String,
    },
    /// Change a node's status. → [`Store::set_node_status`].
    SetStatus {
        project: String,
        node: String,
        status: NodeStatus,
    },
    /// List all dependency edges in a project. → [`Store::edges`].
    ListEdges { project: String },
}

/// The stable core response surface. Serde-internally-tagged on `result`, with
/// one success variant per request plus a single uniform [`ApiResponse::Error`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ApiResponse {
    /// Reply to `health`.
    Health { status: String },
    /// Reply to `list_projects`.
    Projects { projects: Vec<Project> },
    /// Reply to `get_project`.
    Project { project: Project },
    /// Reply to `list_nodes`.
    Nodes { nodes: Vec<Node> },
    /// Reply to `get_node`.
    Node { node: Node },
    /// Reply to `add_node`.
    NodeAdded { node: Node },
    /// Reply to `set_status` — carries the node in its new state.
    StatusSet { node: Node },
    /// Reply to `list_edges`.
    Edges { edges: Vec<Edge> },
    /// Uniform failure. `code` is a stable machine token (`not_found`,
    /// `bad_request`, `too_large`, `store_error`); `message` is human detail.
    Error { code: String, message: String },
}

/// The versioned wrapper serialized back to the caller. `version` is present on
/// every response (success or error); the tagged [`ApiResponse`] is flattened
/// in, so the wire object is e.g. `{"version":"1","result":"projects",...}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiEnvelope {
    pub version: String,
    #[serde(flatten)]
    pub response: ApiResponse,
}

impl ApiEnvelope {
    fn new(response: ApiResponse) -> Self {
        Self {
            version: API_VERSION.to_owned(),
            response,
        }
    }
}

/// Build an [`ApiResponse::Error`] with a stable code and a message.
fn error(code: &str, message: impl Into<String>) -> ApiResponse {
    ApiResponse::Error {
        code: code.to_owned(),
        message: message.into(),
    }
}

/// Look up a single node by id within a project, mapping absence and store
/// failures to typed errors.
fn find_node(store: &Store, project: &str, node: &str) -> Result<Node, ApiResponse> {
    match store.nodes(project) {
        Ok(nodes) => nodes
            .into_iter()
            .find(|n| n.id == node)
            .ok_or_else(|| error("not_found", format!("node not found: {node}"))),
        Err(e) => Err(error("store_error", e.to_string())),
    }
}

/// Dispatch a typed request against the store, returning a typed response.
///
/// Total by construction: every Store error is converted into
/// [`ApiResponse::Error`] rather than propagated, so this never panics on bad
/// input (including references to non-existent projects / nodes).
pub fn handle(store: &Store, req: ApiRequest) -> ApiResponse {
    match req {
        ApiRequest::Health => ApiResponse::Health {
            status: "ok".to_owned(),
        },
        ApiRequest::ListProjects => match store.list_projects() {
            Ok(projects) => ApiResponse::Projects { projects },
            Err(e) => error("store_error", e.to_string()),
        },
        ApiRequest::GetProject { project } => match store.project(&project) {
            Ok(project) => ApiResponse::Project { project },
            Err(e) => error("not_found", e.to_string()),
        },
        ApiRequest::ListNodes { project } => {
            // Validate the project exists so a missing project is `not_found`
            // rather than a silently-empty node list.
            if let Err(e) = store.project(&project) {
                return error("not_found", e.to_string());
            }
            match store.nodes(&project) {
                Ok(nodes) => ApiResponse::Nodes { nodes },
                Err(e) => error("store_error", e.to_string()),
            }
        }
        ApiRequest::GetNode { project, node } => match find_node(store, &project, &node) {
            Ok(node) => ApiResponse::Node { node },
            Err(resp) => resp,
        },
        ApiRequest::AddNode {
            project,
            kind,
            title,
            statement,
        } => match store.add_node(&project, kind, &title, &statement, API_ACTOR) {
            Ok(node) => ApiResponse::NodeAdded { node },
            Err(e) => error("store_error", e.to_string()),
        },
        ApiRequest::SetStatus {
            project,
            node,
            status,
        } => {
            // Trust boundary: `FormallyVerified` asserts a machine-checked proof
            // and may ONLY be reached through the verification pipeline (which
            // attaches evidence). The plain status-mutation API must not be able
            // to fabricate it by fiat.
            if matches!(status, NodeStatus::FormallyVerified) {
                return error(
                    "forbidden",
                    "formally_verified is set only by the verification pipeline \
                     (with proof evidence), not via set_status",
                );
            }
            if let Err(e) = store.set_node_status(&project, &node, status, API_ACTOR) {
                // The only documented failure is a missing node.
                return error("not_found", e.to_string());
            }
            match find_node(store, &project, &node) {
                Ok(node) => ApiResponse::StatusSet { node },
                Err(resp) => resp,
            }
        }
        ApiRequest::ListEdges { project } => {
            if let Err(e) = store.project(&project) {
                return error("not_found", e.to_string());
            }
            match store.edges(&project) {
                Ok(edges) => ApiResponse::Edges { edges },
                Err(e) => error("store_error", e.to_string()),
            }
        }
    }
}

/// The one stable entrypoint an editor / MCP client calls: parse a raw JSON
/// request, dispatch it, and serialize the versioned response.
///
/// Failure modes are total and typed: oversized input → `too_large`, unparseable
/// or unknown-`op` input → `bad_request`. Serialization of the response cannot
/// realistically fail (all fields are plain data), but if it did we fall back to
/// a hand-written error envelope so this function always returns valid JSON.
pub fn handle_json(store: &Store, raw: &str) -> String {
    let envelope = if raw.len() > MAX_REQUEST_BYTES {
        ApiEnvelope::new(error(
            "too_large",
            format!(
                "request of {} bytes exceeds limit of {MAX_REQUEST_BYTES}",
                raw.len()
            ),
        ))
    } else {
        match serde_json::from_str::<ApiRequest>(raw) {
            Ok(req) => ApiEnvelope::new(handle(store, req)),
            Err(e) => ApiEnvelope::new(error("bad_request", e.to_string())),
        }
    };
    serde_json::to_string(&envelope).unwrap_or_else(|_| {
        format!(r#"{{"version":"{API_VERSION}","result":"error","code":"internal","message":"failed to serialize response"}}"#)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn store() -> Store {
        Store::open(Path::new(":memory:")).unwrap()
    }

    /// Every response carries a `version` field equal to `API_VERSION`.
    fn assert_versioned(raw: &str) -> serde_json::Value {
        let v: serde_json::Value = serde_json::from_str(raw).expect("valid JSON response");
        assert_eq!(v["version"], API_VERSION, "version present on every response");
        v
    }

    #[test]
    fn list_projects_empty_returns_ok_empty_list_with_version() {
        let s = store();
        let out = handle_json(&s, r#"{"op":"list_projects"}"#);
        let v = assert_versioned(&out);
        assert_eq!(v["result"], "projects");
        assert_eq!(v["projects"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn add_node_then_get_node_round_trips() {
        let s = store();
        let project = s.create_project("P", "T").unwrap();
        let add = format!(
            r#"{{"op":"add_node","project":"{}","kind":"lemma","title":"L","statement":"1=1"}}"#,
            project.id
        );
        let added = assert_versioned(&handle_json(&s, &add));
        assert_eq!(added["result"], "node_added");
        let node_id = added["node"]["id"].as_str().unwrap().to_owned();

        let get = format!(
            r#"{{"op":"get_node","project":"{}","node":"{}"}}"#,
            project.id, node_id
        );
        let got = assert_versioned(&handle_json(&s, &get));
        assert_eq!(got["result"], "node");
        assert_eq!(got["node"]["id"], node_id);
        assert_eq!(got["node"]["title"], "L");
        assert_eq!(got["node"]["statement"], "1=1");
        assert_eq!(got["node"]["provenance"], API_ACTOR);
    }

    #[test]
    fn malformed_json_returns_clean_error_without_panic() {
        let s = store();
        let v = assert_versioned(&handle_json(&s, "{not json"));
        assert_eq!(v["result"], "error");
        assert_eq!(v["code"], "bad_request");
    }

    #[test]
    fn unknown_op_returns_error() {
        let s = store();
        let v = assert_versioned(&handle_json(&s, r#"{"op":"delete_everything"}"#));
        assert_eq!(v["result"], "error");
        assert_eq!(v["code"], "bad_request");
    }

    #[test]
    fn set_status_changes_status_reflected_by_get_node() {
        let s = store();
        let project = s.create_project("P", "T").unwrap();
        let node = s
            .add_node(&project.id, NodeKind::Lemma, "L", "1=1", "user")
            .unwrap();

        let set = format!(
            r#"{{"op":"set_status","project":"{}","node":"{}","status":"active"}}"#,
            project.id, node.id
        );
        let set_out = assert_versioned(&handle_json(&s, &set));
        assert_eq!(set_out["result"], "status_set");
        assert_eq!(set_out["node"]["status"], "active");

        let get = format!(
            r#"{{"op":"get_node","project":"{}","node":"{}"}}"#,
            project.id, node.id
        );
        let got = assert_versioned(&handle_json(&s, &get));
        assert_eq!(got["node"]["status"], "active");
    }

    #[test]
    fn missing_node_is_clean_not_found_error() {
        let s = store();
        let project = s.create_project("P", "T").unwrap();
        let get = format!(
            r#"{{"op":"get_node","project":"{}","node":"nope"}}"#,
            project.id
        );
        let v = assert_versioned(&handle_json(&s, &get));
        assert_eq!(v["result"], "error");
        assert_eq!(v["code"], "not_found");
    }

    #[test]
    fn health_is_ok_and_versioned() {
        let s = store();
        let v = assert_versioned(&handle_json(&s, r#"{"op":"health"}"#));
        assert_eq!(v["result"], "health");
        assert_eq!(v["status"], "ok");
    }

    #[test]
    fn oversized_request_is_rejected() {
        let s = store();
        let big = format!(r#"{{"op":"get_project","project":"{}"}}"#, "x".repeat(300_000));
        let v = assert_versioned(&handle_json(&s, &big));
        assert_eq!(v["result"], "error");
        assert_eq!(v["code"], "too_large");
    }
}
