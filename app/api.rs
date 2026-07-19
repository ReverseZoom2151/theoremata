//! Stable, versioned JSON API surface for the Theoremata core.
//!
//! This module is the single, documented contract an editor / MCP client calls
//! against. Unlike the broad ad-hoc CLI (which exposes every internal operation
//! and evolves freely), this surface is deliberately *small and stable*: a
//! versioned envelope wrapping a tagged request/response pair. Graph requests
//! map one-to-one to existing [`Store`] operations; meta-tool requests expose
//! the existing Rust [`MetaToolRegistry`] as a typed, non-privileged
//! discovery/dispatch surface. The dispatcher is total and never panics on bad
//! input.
//!
//! All request content is treated as UNTRUSTED DATA: raw input is length-capped
//! before parsing, malformed / oversized / unknown input becomes a clean
//! [`ApiResponse::Error`], and no code path panics on bad input. There is no
//! wall-clock or randomness in this module.
//!
//! Wire shape (request):  `{"op":"get_node","project":"…","node":"…"}`
//! Wire shape (response): `{"version":"1","result":"node","node":{…}}`
//! Meta-tool discovery:   `{"op":"list_meta_tools"}`
//! Meta-tool invocation:  `{"op":"invoke_meta_tool","tool":"plan","arguments":{…}}`

use crate::db::Store;
use crate::meta_tools::{MetaToolError, MetaToolKind, MetaToolRegistry, ALL_KINDS};
use crate::model::{Edge, Node, NodeKind, NodeStatus, Project};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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
    /// Discover the built-in orchestration meta-tools in stable name order.
    ListMetaTools,
    /// Invoke one built-in orchestration meta-tool with schema-validated args.
    InvokeMetaTool { tool: String, arguments: Value },
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
    /// Reply to `list_meta_tools`. Names and descriptors share alphabetical order.
    MetaTools {
        names: Vec<String>,
        tools: Vec<Value>,
    },
    /// Reply to `invoke_meta_tool`.
    MetaToolInvoked { tool: String, output: Value },
    /// Uniform failure. `code` is a stable machine token (`not_found`,
    /// `bad_request`, `invalid_arguments`, `unknown_tool`, `tool_error`,
    /// `forbidden`, `too_large`, `store_error`); `message` is human detail.
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

/// Build the stable registry exposed by this API.
///
/// These handlers deliberately produce an orchestration request rather than
/// touching the graph. The privileged work remains in the orchestration seams
/// named by `wraps`; in particular, no meta-tool handler receives a [`Store`]
/// and therefore none can assign a node status. This API is the typed discovery
/// and dispatch boundary used by an editor/agent runtime.
fn built_in_meta_tools() -> MetaToolRegistry {
    let mut registry = MetaToolRegistry::new();
    for kind in ALL_KINDS {
        registry.register_fn(kind, move |arguments| {
            Ok(json!({
                "accepted": true,
                "tool": kind.name(),
                "worker_op": kind.worker_op(),
                "wraps": kind.wraps(),
                "arguments": arguments.clone(),
            }))
        });
    }
    registry
}

/// Reject a direct status assertion anywhere in an invocation or result.
/// Meta-tools may plan, critique, recall, or abstain; certification remains a
/// separate evidence-bearing gate.
fn contains_formally_verified(value: &Value) -> bool {
    match value {
        Value::String(s) => s.eq_ignore_ascii_case("formally_verified"),
        Value::Array(values) => values.iter().any(contains_formally_verified),
        Value::Object(values) => values.values().any(contains_formally_verified),
        _ => false,
    }
}

/// Validate arguments against the JSON-schema subset used by
/// [`MetaToolKind::input_schema`]. Unknown fields are rejected at every schema
/// node that declares `properties`, keeping calls deterministic and typo-safe.
fn validate_meta_arguments(kind: MetaToolKind, arguments: &Value) -> Result<(), String> {
    validate_schema_value(&kind.input_schema(), arguments, "arguments")
}

/// Drop top-level `arguments` keys carrying an explicit JSON `null` for any
/// property whose schema does not admit `null`.
///
/// Models emit `{"max_rounds": null}` to mean "leave this optional parameter
/// unset", but the schema's scalar type check rejects null, so the call failed
/// with a type error that reads like a model failure. Removing the key makes an
/// explicit null behave exactly like an absent key. Nulls the schema *does*
/// permit (`{"type": ["string","null"]}`, or a property with no declared type)
/// are preserved, as are nulls on unknown properties so those still fail the
/// unknown-field check rather than being silently accepted.
fn strip_permitted_nulls(schema: &Value, arguments: &mut Value) {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return;
    };
    let Some(object) = arguments.as_object_mut() else {
        return;
    };
    object.retain(|name, value| {
        if !value.is_null() {
            return true;
        }
        let Some(types) = properties.get(name).and_then(|p| p.get("type")) else {
            // Unknown property, or one with no declared type: leave it alone.
            return true;
        };
        match types {
            Value::String(kind) => kind == "null",
            Value::Array(kinds) => kinds.iter().any(|k| k.as_str() == Some("null")),
            // Malformed `type`: keep the key so validation reports it.
            _ => true,
        }
    });
}

fn validate_schema_value(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    if let Some(types) = schema.get("type") {
        let allowed: Vec<&str> = match types {
            Value::String(kind) => vec![kind.as_str()],
            Value::Array(kinds) => kinds.iter().filter_map(Value::as_str).collect(),
            _ => return Err(format!("invalid built-in schema at {path}: 'type'")),
        };
        if !allowed.iter().any(|kind| value_matches_type(value, kind)) {
            return Err(format!(
                "{path} must be {}; got {}",
                allowed.join(" or "),
                json_type_name(value)
            ));
        }
    }

    if let Some(options) = schema.get("enum").and_then(Value::as_array) {
        if !options.iter().any(|candidate| candidate == value) {
            return Err(format!("{path} is not one of the allowed values"));
        }
    }

    if let Some(number) = value.as_f64() {
        if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64) {
            if number < minimum {
                return Err(format!("{path} must be >= {minimum}"));
            }
        }
        if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64) {
            if number > maximum {
                return Err(format!("{path} must be <= {maximum}"));
            }
        }
    }

    if let Some(object) = value.as_object() {
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for name in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(name) {
                    return Err(format!("{path} is missing required field '{name}'"));
                }
            }
        }
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for (name, child) in object {
                let Some(child_schema) = properties.get(name) else {
                    return Err(format!("{path} contains unknown field '{name}'"));
                };
                validate_schema_value(child_schema, child, &format!("{path}.{name}"))?;
            }
        }
    }

    if let (Some(values), Some(item_schema)) = (
        value.as_array(),
        schema.get("items").filter(|v| v.is_object()),
    ) {
        for (index, item) in values.iter().enumerate() {
            validate_schema_value(item_schema, item, &format!("{path}[{index}]"))?;
        }
    }

    Ok(())
}

fn value_matches_type(value: &Value, expected: &str) -> bool {
    match expected {
        "null" => value.is_null(),
        "boolean" => value.is_boolean(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "string" => value.is_string(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => false,
    }
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(number) if number.is_i64() || number.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn invoke_meta_tool(registry: &MetaToolRegistry, tool: String, mut arguments: Value) -> ApiResponse {
    let Some(kind) = MetaToolKind::from_name(&tool) else {
        return error("unknown_tool", format!("unknown meta-tool: {tool}"));
    };
    if !registry.contains(&tool) {
        return error(
            "unknown_tool",
            format!("meta-tool is not registered: {tool}"),
        );
    }
    if contains_formally_verified(&arguments) {
        return error(
            "forbidden",
            "meta-tools cannot assign formally_verified; certification requires proof evidence",
        );
    }
    strip_permitted_nulls(&kind.input_schema(), &mut arguments);
    if let Err(message) = validate_meta_arguments(kind, &arguments) {
        return error("invalid_arguments", message);
    }

    match registry.invoke(&tool, &arguments) {
        Ok(output) if contains_formally_verified(&output) => error(
            "forbidden",
            "meta-tool output attempted to assign formally_verified",
        ),
        Ok(output) => ApiResponse::MetaToolInvoked { tool, output },
        Err(MetaToolError::UnknownTool(name)) => {
            error("unknown_tool", format!("unknown meta-tool: {name}"))
        }
        Err(MetaToolError::Invocation { tool, reason }) => {
            error("tool_error", format!("meta-tool '{tool}' failed: {reason}"))
        }
    }
}

/// Dispatch a typed request against the store, returning a typed response.
///
/// Total by construction: every Store error is converted into
/// [`ApiResponse::Error`] rather than propagated, so this never panics on bad
/// input (including references to non-existent projects / nodes).
pub fn handle(store: &Store, req: ApiRequest) -> ApiResponse {
    let registry = built_in_meta_tools();
    handle_with_meta_tools(store, req, &registry)
}

/// Dispatch with an explicit registry. The stable entrypoint uses the built-in
/// registry; this seam keeps invocation testable and lets an embedding replace
/// handlers without changing the wire contract.
pub fn handle_with_meta_tools(
    store: &Store,
    req: ApiRequest,
    registry: &MetaToolRegistry,
) -> ApiResponse {
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
        ApiRequest::ListMetaTools => ApiResponse::MetaTools {
            names: registry.names().into_iter().map(str::to_owned).collect(),
            tools: registry.describe_all(),
        },
        ApiRequest::InvokeMetaTool { tool, arguments } => {
            invoke_meta_tool(registry, tool, arguments)
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
    handle_json_dispatch(raw, |req| handle(store, req))
}

fn handle_json_dispatch(raw: &str, dispatch: impl FnOnce(ApiRequest) -> ApiResponse) -> String {
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
            Ok(req) => ApiEnvelope::new(dispatch(req)),
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
        assert_eq!(
            v["version"], API_VERSION,
            "version present on every response"
        );
        v
    }

    fn handle_json_with_registry(store: &Store, raw: &str, registry: &MetaToolRegistry) -> String {
        handle_json_dispatch(raw, |req| handle_with_meta_tools(store, req, registry))
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
    fn meta_tool_discovery_is_complete_sorted_and_schema_backed() {
        let s = store();
        let v = assert_versioned(&handle_json(&s, r#"{"op":"list_meta_tools"}"#));
        assert_eq!(v["result"], "meta_tools");

        let names: Vec<&str> = v["names"]
            .as_array()
            .unwrap()
            .iter()
            .map(|name| name.as_str().unwrap())
            .collect();
        let mut expected: Vec<&str> = ALL_KINDS.iter().map(|kind| kind.name()).collect();
        expected.sort_unstable();
        assert_eq!(
            names, expected,
            "discovery names must be stable and alphabetical"
        );

        let tools = v["tools"].as_array().unwrap();
        assert_eq!(tools.len(), ALL_KINDS.len());
        let described: Vec<&str> = tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            described, names,
            "names and descriptors must have identical order"
        );
        assert!(tools.iter().all(|tool| {
            tool["description"].as_str().is_some_and(|s| !s.is_empty())
                && tool["inputSchema"]["type"] == "object"
                && tool["inputSchema"]["properties"].is_object()
                && tool["inputSchema"]["required"].is_array()
        }));
    }

    #[test]
    fn meta_tool_invocation_dispatches_through_the_registry() {
        let s = store();
        let out = handle_json(
            &s,
            r#"{"op":"invoke_meta_tool","tool":"plan","arguments":{"statement":"n + 0 = n","max_rounds":2,"seed":7}}"#,
        );
        let v = assert_versioned(&out);
        assert_eq!(v["result"], "meta_tool_invoked");
        assert_eq!(v["tool"], "plan");
        assert_eq!(v["output"]["accepted"], true);
        assert_eq!(v["output"]["tool"], "plan");
        assert_eq!(v["output"]["worker_op"], "meta_plan");
        assert_eq!(v["output"]["arguments"]["statement"], "n + 0 = n");
    }

    #[test]
    fn meta_tool_arguments_are_strictly_schema_validated() {
        let s = store();
        let cases = [
            // The call-level `arguments` must be an object.
            r#"{"op":"invoke_meta_tool","tool":"plan","arguments":"not-an-object"}"#,
            // Required field omitted.
            r#"{"op":"invoke_meta_tool","tool":"plan","arguments":{"seed":1}}"#,
            // Wrong integer type.
            r#"{"op":"invoke_meta_tool","tool":"recall","arguments":{"project_id":"p","n_best":"many"}}"#,
            // Numeric range violation.
            r#"{"op":"invoke_meta_tool","tool":"budget","arguments":{"difficulty":1.5}}"#,
            // Unknown fields are rejected rather than silently ignored.
            r#"{"op":"invoke_meta_tool","tool":"spend","arguments":{"typo":true}}"#,
        ];
        for raw in cases {
            let v = assert_versioned(&handle_json(&s, raw));
            assert_eq!(v["result"], "error", "request should fail: {raw}");
            assert_eq!(v["code"], "invalid_arguments", "request: {raw}");
            assert!(v["message"].as_str().is_some_and(|m| !m.is_empty()));
        }
    }

    #[test]
    fn explicit_null_for_an_optional_scalar_is_treated_as_absent() {
        let s = store();
        // A model emitting `"max_rounds": null` means "leave it unset"; that
        // must dispatch exactly as if the key were omitted, not fail the
        // integer type check.
        let v = assert_versioned(&handle_json(
            &s,
            r#"{"op":"invoke_meta_tool","tool":"plan","arguments":{"statement":"n + 0 = n","max_rounds":null,"seed":null}}"#,
        ));
        assert_eq!(v["result"], "meta_tool_invoked", "message: {}", v["message"]);
        assert_eq!(v["tool"], "plan");
        assert_eq!(v["output"]["arguments"]["statement"], "n + 0 = n");
        // The null keys reach the handler as absent, so `.get(k).unwrap_or(d)`
        // style reads see the default.
        let args = v["output"]["arguments"].as_object().expect("arguments object");
        assert!(!args.contains_key("max_rounds"), "null key should be stripped");
        assert!(!args.contains_key("seed"), "null key should be stripped");
    }

    #[test]
    fn null_is_preserved_where_the_schema_declares_it() {
        let s = store();
        // `node_id` is `["string","null"]`, so an explicit null is meaningful
        // and must survive to the handler.
        let v = assert_versioned(&handle_json(
            &s,
            r#"{"op":"invoke_meta_tool","tool":"critique","arguments":{"project_id":"p","node_id":null}}"#,
        ));
        assert_eq!(v["result"], "meta_tool_invoked", "message: {}", v["message"]);
        let args = v["output"]["arguments"].as_object().expect("arguments object");
        assert!(args.contains_key("node_id"), "schema-permitted null must be kept");
        assert!(args["node_id"].is_null());
    }

    #[test]
    fn null_on_a_required_or_unknown_field_still_errors() {
        let s = store();
        let cases = [
            // Nulling a required field is not a way to smuggle it past the check.
            r#"{"op":"invoke_meta_tool","tool":"plan","arguments":{"statement":null}}"#,
            // Unknown fields stay rejected even when null-valued.
            r#"{"op":"invoke_meta_tool","tool":"spend","arguments":{"typo":null}}"#,
        ];
        for raw in cases {
            let v = assert_versioned(&handle_json(&s, raw));
            assert_eq!(v["result"], "error", "request should fail: {raw}");
            assert_eq!(v["code"], "invalid_arguments", "request: {raw}");
        }
    }

    #[test]
    fn unknown_meta_tool_and_handler_failure_are_structured_errors() {
        let s = store();
        let unknown = assert_versioned(&handle_json(
            &s,
            r#"{"op":"invoke_meta_tool","tool":"certify","arguments":{}}"#,
        ));
        assert_eq!(unknown["result"], "error");
        assert_eq!(unknown["code"], "unknown_tool");

        let mut registry = MetaToolRegistry::new();
        registry.register_fn(MetaToolKind::Plan, |_arguments| {
            Err(MetaToolError::invocation("plan", "planner unavailable"))
        });
        let failed = assert_versioned(&handle_json_with_registry(
            &s,
            r#"{"op":"invoke_meta_tool","tool":"plan","arguments":{"statement":"T"}}"#,
            &registry,
        ));
        assert_eq!(failed["result"], "error");
        assert_eq!(failed["code"], "tool_error");
        assert!(failed["message"]
            .as_str()
            .unwrap()
            .contains("planner unavailable"));
    }

    #[test]
    fn meta_tools_cannot_grant_formally_verified_from_input_or_output() {
        let s = store();
        let project = s.create_project("P", "T").unwrap();
        let node = s
            .add_node(&project.id, NodeKind::Lemma, "L", "1=1", "user")
            .unwrap();

        // A normal abstention is accepted but remains a pure orchestration
        // request: it cannot mutate the node's status.
        let abstain = format!(
            r#"{{"op":"invoke_meta_tool","tool":"abstain","arguments":{{"project_id":"{}","node_id":"{}","reason":"low confidence","confidence":0.2}}}}"#,
            project.id, node.id
        );
        let accepted = assert_versioned(&handle_json(&s, &abstain));
        assert_eq!(accepted["result"], "meta_tool_invoked");
        assert_eq!(
            find_node(&s, &project.id, &node.id).unwrap().status,
            NodeStatus::Proposed
        );

        // An input-side attempt is rejected before handler dispatch.
        let input = assert_versioned(&handle_json(
            &s,
            r#"{"op":"invoke_meta_tool","tool":"self_review","arguments":{"project_id":"p","candidate":{"status":"formally_verified"}}}"#,
        ));
        assert_eq!(input["result"], "error");
        assert_eq!(input["code"], "forbidden");

        // A compromised/injected handler cannot smuggle the status back in its
        // structured output either.
        let mut registry = MetaToolRegistry::new();
        registry.register_fn(MetaToolKind::Abstain, |_arguments| {
            Ok(serde_json::json!({"status": "formally_verified"}))
        });
        let output = assert_versioned(&handle_json_with_registry(
            &s,
            r#"{"op":"invoke_meta_tool","tool":"abstain","arguments":{"reason":"no confidence"}}"#,
            &registry,
        ));
        assert_eq!(output["result"], "error");
        assert_eq!(output["code"], "forbidden");
        assert_eq!(
            find_node(&s, &project.id, &node.id).unwrap().status,
            NodeStatus::Proposed
        );
    }

    #[test]
    fn oversized_request_is_rejected() {
        let s = store();
        let big = format!(
            r#"{{"op":"get_project","project":"{}"}}"#,
            "x".repeat(300_000)
        );
        let v = assert_versioned(&handle_json(&s, &big));
        assert_eq!(v["result"], "error");
        assert_eq!(v["code"], "too_large");
    }
}
