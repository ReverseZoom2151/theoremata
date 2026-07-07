//! leanblueprint dialect: emit + ingest (plan Tier 1, item 1).
//!
//! `leanblueprint` (Patrick Massot's plasTeX plugin) is the de-facto proof-DAG
//! interchange format shared by all seven Lean formalization repos we mined
//! (Erdos1196, Sphere-Packing-Lean, RiemannHypothesisCurves, KakeyaFiniteFields,
//! strongpnt, ZkLinalg, FrontierMath-Hypergraphs). Each proof-DAG node is a LaTeX
//! theorem-like environment carrying four load-bearing macros — quoting the exact
//! syntax from `docs/resource-mining/{Erdos1196,Sphere-Packing-Lean,
//! RiemannHypothesisCurves}.md`:
//!
//! ```latex
//! \begin{lemma}[Tail estimate]\label{lem:tail}
//! \lean{PrimitiveSetsAboveX.tailEstimate}   % node -> Lean declaration binding
//! \leanok                                    % the STATEMENT is formalised
//! \uses{lem:mertens}                         % statement-level dependency edges
//! ...
//! \end{lemma}
//! \begin{proof}
//! \leanok                                    % the PROOF is complete (no sorry)
//! \uses{lem:chebyshev}                       % proof-level dependency edges
//! \end{proof}
//! ```
//!
//! The refinement our schema gained for this: `\uses` and `\leanok` are placed
//! **independently on the statement vs inside the proof**, so statement-deps ≠
//! proof-deps and statement-formalized ≠ proof-done (see `model::DepScope`,
//! `Node::stmt_formalized`/`proof_done`).
//!
//! This module (a) EMITs a project's graph as a `content.tex` plus the flat
//! `blueprint/lean_decls` manifest, and (b) INGESTs a `content.tex` back into
//! nodes+edges. EMIT→INGEST is a graph-preserving round-trip.

use crate::{
    db::Store,
    model::{DepScope, EdgeKind, NodeKind, NodeStatus},
};
use anyhow::Result;
use serde::Serialize;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// The theorem-like environments leanblueprint treats as DAG nodes.
const NODE_ENVS: &[&str] = &[
    "theorem",
    "lemma",
    "proposition",
    "corollary",
    "definition",
    "remark",
];

/// One leanblueprint node: a theorem-env with its four schema macros, split
/// across the statement and its proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlueprintNode {
    /// The theorem-like environment (`lemma`, `theorem`, `definition`, …).
    pub env: String,
    /// `\label{kind:slug}` — the node's unique blueprint key (not the Lean name).
    pub label: String,
    /// Optional `\begin{env}[Title]` display title.
    pub title: Option<String>,
    /// `\lean{A,B,...}` — the fully-qualified Lean declaration(s) this node binds.
    pub lean: Vec<String>,
    /// `\uses{...}` appearing on the statement.
    pub statement_uses: Vec<String>,
    /// `\uses{...}` appearing inside `\begin{proof}`.
    pub proof_uses: Vec<String>,
    /// `\leanok` on the statement (statement formalised).
    pub statement_leanok: bool,
    /// `\leanok` inside the proof (proof complete).
    pub proof_leanok: bool,
    /// The LaTeX statement body (prose sans macros).
    pub statement_body: String,
    /// The LaTeX proof body (prose sans macros).
    pub proof_body: String,
}

/// A parsed / to-be-emitted blueprint: an ordered list of nodes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Blueprint {
    pub nodes: Vec<BlueprintNode>,
}

/// EMIT result: the `content.tex` document and the flat `blueprint/lean_decls`
/// manifest (newline-delimited `\lean{}` names in node order).
#[derive(Debug, Clone, Serialize)]
pub struct BlueprintExport {
    pub content_tex: String,
    pub lean_decls: String,
}

/// INGEST result: how many nodes/edges were created and any `\uses` keys that
/// referenced a label not present in the document.
#[derive(Debug, Clone, Serialize)]
pub struct BlueprintImport {
    pub nodes_created: usize,
    pub edges_created: usize,
    pub unresolved_uses: Vec<String>,
}

impl Blueprint {
    /// Serialise to a leanblueprint `content.tex` fragment.
    pub fn to_tex(&self) -> String {
        let mut out = String::new();
        for n in &self.nodes {
            out.push_str(&format!("\\begin{{{}}}", n.env));
            if let Some(t) = &n.title {
                out.push_str(&format!("[{t}]"));
            }
            out.push_str(&format!("\\label{{{}}}\n", n.label));
            if !n.lean.is_empty() {
                out.push_str(&format!("\\lean{{{}}}\n", n.lean.join(",")));
            }
            if n.statement_leanok {
                out.push_str("\\leanok\n");
            }
            if !n.statement_uses.is_empty() {
                out.push_str(&format!("\\uses{{{}}}\n", n.statement_uses.join(",")));
            }
            if !n.statement_body.trim().is_empty() {
                out.push_str(n.statement_body.trim());
                out.push('\n');
            }
            out.push_str(&format!("\\end{{{}}}\n", n.env));

            // Emit a proof environment only when there is proof-level content.
            if n.proof_leanok || !n.proof_uses.is_empty() || !n.proof_body.trim().is_empty() {
                out.push_str("\\begin{proof}\n");
                if n.proof_leanok {
                    out.push_str("\\leanok\n");
                }
                if !n.proof_uses.is_empty() {
                    out.push_str(&format!("\\uses{{{}}}\n", n.proof_uses.join(",")));
                }
                if !n.proof_body.trim().is_empty() {
                    out.push_str(n.proof_body.trim());
                    out.push('\n');
                }
                out.push_str("\\end{proof}\n");
            }
            out.push('\n');
        }
        out
    }

    /// Parse a leanblueprint `content.tex` fragment back into nodes. A `proof`
    /// environment attaches to the theorem-env immediately preceding it.
    pub fn from_tex(tex: &str) -> Blueprint {
        let mut nodes: Vec<BlueprintNode> = Vec::new();
        for block in environment_blocks(tex) {
            if block.env == "proof" {
                if let Some(last) = nodes.last_mut() {
                    last.proof_leanok = has_flag(&block.body, "leanok");
                    last.proof_uses = collect_uses(&block.body);
                    last.proof_body = strip_macros(&block.body);
                }
                continue;
            }
            if !NODE_ENVS.contains(&block.env.as_str()) {
                continue;
            }
            let Some(label) = first_macro_arg(&block.body, "label") else {
                // A theorem-env without a \label is not a DAG node; skip it.
                continue;
            };
            nodes.push(BlueprintNode {
                env: block.env,
                label,
                title: block.title,
                lean: collect_macro_args(&block.body, "lean")
                    .into_iter()
                    .flat_map(|a| split_csv(&a))
                    .collect(),
                statement_uses: collect_uses(&block.body),
                proof_uses: Vec::new(),
                statement_leanok: has_flag(&block.body, "leanok"),
                proof_leanok: false,
                statement_body: strip_macros(&block.body),
                proof_body: String::new(),
            });
        }
        Blueprint { nodes }
    }

    /// All `\lean{}` declaration names across the graph in node order — the
    /// content of the flat `blueprint/lean_decls` manifest.
    pub fn lean_decls(&self) -> Vec<String> {
        self.nodes.iter().flat_map(|n| n.lean.clone()).collect()
    }
}

/// EMIT: serialise a project's proof-DAG to a leanblueprint document + manifest.
pub fn export(store: &Store, project_id: &str) -> Result<BlueprintExport> {
    let nodes = store.nodes(project_id)?;
    let edges = store.edges(project_id)?;

    // Assign a stable, unique blueprint label per node.
    let mut labels: HashMap<String, String> = HashMap::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for n in &nodes {
        let base = format!("{}:{}", label_prefix(n.kind), slugify(&n.title));
        let count = seen.entry(base.clone()).or_insert(0);
        let label = if *count == 0 {
            base.clone()
        } else {
            format!("{base}-{count}")
        };
        *count += 1;
        labels.insert(n.id.clone(), label);
    }

    let mut bp = Blueprint::default();
    for n in &nodes {
        // Partition this node's outgoing dependency edges by \uses scope.
        let mut statement_uses = Vec::new();
        let mut proof_uses = Vec::new();
        for e in edges.iter().filter(|e| e.source_id == n.id && e.kind == EdgeKind::DependsOn) {
            let Some(target_label) = labels.get(&e.target_id) else {
                continue;
            };
            match e.dep_scope {
                DepScope::Statement => statement_uses.push(target_label.clone()),
                DepScope::Proof => proof_uses.push(target_label.clone()),
                DepScope::Both => {
                    statement_uses.push(target_label.clone());
                    proof_uses.push(target_label.clone());
                }
            }
        }
        let verified = n.status == NodeStatus::FormallyVerified;
        bp.nodes.push(BlueprintNode {
            env: env_for_kind(n.kind).to_string(),
            label: labels[&n.id].clone(),
            title: Some(n.title.clone()),
            lean: n
                .formal_statement
                .as_deref()
                .map(lean_decl_names)
                .unwrap_or_default(),
            statement_uses,
            proof_uses,
            statement_leanok: n.stmt_formalized || verified,
            proof_leanok: n.proof_done || verified,
            statement_body: n.statement.clone(),
            proof_body: String::new(),
        });
    }

    Ok(BlueprintExport {
        content_tex: bp.to_tex(),
        lean_decls: bp.lean_decls().join("\n"),
    })
}

/// INGEST: parse a `content.tex` and materialise its nodes + edges into a
/// project. Statement `\uses` become `Statement`-scoped edges, proof `\uses`
/// become `Proof`-scoped, and a key appearing in both is merged to `Both`.
pub fn import(store: &Store, project_id: &str, tex: &str) -> Result<BlueprintImport> {
    let bp = Blueprint::from_tex(tex);
    let mut label_to_id: HashMap<String, String> = HashMap::new();

    for bnode in &bp.nodes {
        let kind = kind_for_env(&bnode.env);
        let title = bnode.title.clone().unwrap_or_else(|| bnode.label.clone());
        let node = store.add_node(project_id, kind, &title, &bnode.statement_body, "blueprint:import")?;
        if bnode.statement_leanok || bnode.proof_leanok {
            store.set_verification_flags(
                project_id,
                &node.id,
                bnode.statement_leanok,
                bnode.proof_leanok,
                "blueprint:import",
            )?;
        }
        label_to_id.insert(bnode.label.clone(), node.id);
    }

    let mut edges_created = 0usize;
    let mut unresolved_uses = Vec::new();
    for bnode in &bp.nodes {
        let source = &label_to_id[&bnode.label];
        // Merge statement/proof scopes per referenced label.
        let mut scoped: HashMap<String, DepScope> = HashMap::new();
        for u in &bnode.statement_uses {
            merge_scope(&mut scoped, u, DepScope::Statement);
        }
        for u in &bnode.proof_uses {
            merge_scope(&mut scoped, u, DepScope::Proof);
        }
        for (target_label, scope) in scoped {
            match label_to_id.get(&target_label) {
                Some(target) => {
                    store.add_edge_scoped(
                        project_id,
                        source,
                        target,
                        EdgeKind::DependsOn,
                        scope,
                    )?;
                    edges_created += 1;
                }
                None => unresolved_uses.push(target_label),
            }
        }
    }

    Ok(BlueprintImport {
        nodes_created: bp.nodes.len(),
        edges_created,
        unresolved_uses,
    })
}

fn merge_scope(map: &mut HashMap<String, DepScope>, key: &str, scope: DepScope) {
    map.entry(key.to_string())
        .and_modify(|s| *s = s.merge(scope))
        .or_insert(scope);
}

// ------------------------------------------------------------------------
// checkdecls-style node-binding gate (plan Tier 1, item 3)
// ------------------------------------------------------------------------

/// How the node-binding gate resolved the `\lean{}` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckDeclsMode {
    /// `lake exe checkdecls` ran against the built workspace.
    Lake,
    /// Best-effort "declaration present in source" static scan (lake absent).
    Static,
    /// No work was possible (no workspace / no declarations).
    Skipped,
}

/// The result of the cheap referential-integrity gate: does every node's
/// `\lean{}` name resolve? This is the leanblueprint `checkdecls` check — a
/// binding-integrity gate *distinct from and cheaper than* the full proof /
/// `#print axioms` gate. It fails gracefully (typed report, never panics) like
/// `verify::hardening`.
#[derive(Debug, Clone, Serialize)]
pub struct CheckDeclsReport {
    pub ran: bool,
    pub mode: CheckDeclsMode,
    pub total: usize,
    pub resolved: usize,
    pub missing: Vec<String>,
    pub summary: String,
}

/// Verify every declaration name in `decls` resolves against a Lean `workspace`.
///
/// Prefers `lake exe checkdecls <manifest>` under the workspace when lake is
/// available and the workspace exposes the checkdecls executable; otherwise it
/// falls back to a static "the declaration appears in a `.lean` source" scan.
/// This is intentionally cheaper than compiling and auditing axioms.
pub fn check_decls(workspace: &Path, decls: &[String]) -> Result<CheckDeclsReport> {
    let decls: Vec<String> = decls
        .iter()
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .collect();
    if decls.is_empty() {
        return Ok(CheckDeclsReport {
            ran: false,
            mode: CheckDeclsMode::Skipped,
            total: 0,
            resolved: 0,
            missing: Vec::new(),
            summary: "no \\lean{} declarations to check".into(),
        });
    }
    if !workspace.exists() {
        return Ok(CheckDeclsReport {
            ran: false,
            mode: CheckDeclsMode::Skipped,
            total: decls.len(),
            resolved: 0,
            missing: decls.clone(),
            summary: format!("workspace {} does not exist", workspace.display()),
        });
    }

    if let Some(report) = try_lake_checkdecls(workspace, &decls) {
        return Ok(report);
    }
    Ok(static_check_decls(workspace, &decls))
}

/// Attempt the real `lake exe checkdecls` gate. Returns `None` (so the caller
/// falls back to the static scan) when lake or the executable is unavailable.
fn try_lake_checkdecls(workspace: &Path, decls: &[String]) -> Option<CheckDeclsReport> {
    // checkdecls only makes sense inside a Lake workspace; without a lakefile the
    // static "declaration present in source" scan is the right (and safe) path.
    if !workspace.join("lakefile.toml").exists() && !workspace.join("lakefile.lean").exists() {
        return None;
    }
    let lake = resolve("lake")?;
    // checkdecls reads a newline-delimited manifest path argument.
    let manifest = workspace.join(".theoremata_lean_decls");
    if std::fs::write(&manifest, decls.join("\n")).is_err() {
        return None;
    }
    let out = Command::new(&lake)
        .current_dir(workspace)
        .args(["exe", "checkdecls"])
        .arg(&manifest)
        .output()
        .ok()?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // The workspace may not declare a `checkdecls` executable at all.
    if combined.contains("unknown executable") || combined.contains("no such executable") {
        return None;
    }
    let (resolved, missing) = if out.status.success() {
        (decls.len(), Vec::new())
    } else {
        // Non-zero exit: any of our decls named in the output is reported missing.
        let missing: Vec<String> = decls
            .iter()
            .filter(|d| combined.contains(d.as_str()))
            .cloned()
            .collect();
        (decls.len() - missing.len(), missing)
    };
    let summary = if missing.is_empty() {
        format!("checkdecls: all {} declaration(s) resolve", decls.len())
    } else {
        format!("checkdecls: {} of {} declaration(s) missing", missing.len(), decls.len())
    };
    Some(CheckDeclsReport {
        ran: true,
        mode: CheckDeclsMode::Lake,
        total: decls.len(),
        resolved,
        missing,
        summary,
    })
}

/// Best-effort static binding check: a declaration resolves if its fully
/// qualified name — or its final `.`-separated component preceded by a decl
/// keyword — appears in any `.lean` source under the workspace.
fn static_check_decls(workspace: &Path, decls: &[String]) -> CheckDeclsReport {
    let mut sources = String::new();
    collect_lean_sources(workspace, &mut sources, 0);
    let mut missing = Vec::new();
    for d in decls {
        let last = d.rsplit('.').next().unwrap_or(d.as_str());
        let present = sources.contains(d.as_str())
            || ["theorem", "lemma", "def", "abbrev", "instance", "noncomputable def"]
                .iter()
                .any(|kw| sources.contains(&format!("{kw} {last}")));
        if !present {
            missing.push(d.clone());
        }
    }
    let resolved = decls.len() - missing.len();
    let summary = if missing.is_empty() {
        format!("static check: all {} declaration(s) found in source", decls.len())
    } else {
        format!(
            "static check: {} of {} declaration(s) not found in source",
            missing.len(),
            decls.len()
        )
    };
    CheckDeclsReport {
        ran: true,
        mode: CheckDeclsMode::Static,
        total: decls.len(),
        resolved,
        missing,
        summary,
    }
}

/// Recursively concatenate `.lean` source text under `dir` (bounded depth,
/// skipping the `.lake` build directory). Best-effort — I/O errors are ignored.
fn collect_lean_sources(dir: &Path, out: &mut String, depth: usize) {
    if depth > 24 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().and_then(|n| n.to_str()) == Some(".lake") {
                continue;
            }
            collect_lean_sources(&path, out, depth + 1);
        } else if path.extension().and_then(|e| e.to_str()) == Some("lean") {
            if let Ok(text) = std::fs::read_to_string(&path) {
                out.push_str(&text);
                out.push('\n');
            }
        }
    }
}

/// Resolve an executable to a spawnable command (direct, else via login shell).
/// Mirrors `verify::hardening::resolve` without depending on its private helper.
fn resolve(cmd: &str) -> Option<String> {
    if Command::new(cmd)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
    {
        return Some(cmd.to_owned());
    }
    let script = format!(
        "if command -v {cmd} >/dev/null 2>&1 && {cmd} --version >/dev/null 2>&1; then \
         cygpath -w \"$(command -v {cmd})\" 2>/dev/null || command -v {cmd}; fi"
    );
    let out = Command::new("bash").args(["-lc", &script]).output().ok()?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!path.is_empty()).then_some(path)
}

// ------------------------------------------------------------------------
// Convenience: build a workspace-relative default manifest path.
// ------------------------------------------------------------------------

/// The conventional leanblueprint manifest location under a project directory.
pub fn lean_decls_path(blueprint_root: &Path) -> PathBuf {
    blueprint_root.join("blueprint").join("lean_decls")
}

// ------------------------------------------------------------------------
// LaTeX macro scanning helpers (regex-free, brace-matched)
// ------------------------------------------------------------------------

struct EnvBlock {
    env: String,
    title: Option<String>,
    body: String,
}

/// Split a document into `\begin{env}...\end{env}` blocks in source order.
/// Nesting is not expected in leanblueprint node/proof environments, so the
/// nearest matching `\end{env}` delimits each block.
fn environment_blocks(tex: &str) -> Vec<EnvBlock> {
    let bytes = tex.as_bytes();
    let mut blocks = Vec::new();
    let mut i = 0;
    while let Some(begin) = find_from(tex, "\\begin{", i) {
        let name_start = begin + "\\begin{".len();
        let Some(name_end) = find_from(tex, "}", name_start) else {
            break;
        };
        let env = tex[name_start..name_end].to_string();
        let after = name_end + 1;
        // Optional [Title] immediately after \begin{env}.
        let (title, body_start) = if bytes.get(after) == Some(&b'[') {
            match find_from(tex, "]", after + 1) {
                Some(close) => (Some(tex[after + 1..close].to_string()), close + 1),
                None => (None, after),
            }
        } else {
            (None, after)
        };
        let end_marker = format!("\\end{{{env}}}");
        let Some(end_pos) = find_from(tex, &end_marker, body_start) else {
            break;
        };
        blocks.push(EnvBlock {
            env,
            title,
            body: tex[body_start..end_pos].to_string(),
        });
        i = end_pos + end_marker.len();
    }
    blocks
}

/// Byte index of `needle` in `hay` at or after `from`.
fn find_from(hay: &str, needle: &str, from: usize) -> Option<usize> {
    hay.get(from..).and_then(|s| s.find(needle)).map(|p| p + from)
}

/// All `\uses{...}` keys in `text`, comma-split and flattened across multiple
/// `\uses` occurrences (leanblueprint allows either form).
fn collect_uses(text: &str) -> Vec<String> {
    collect_macro_args(text, "uses")
        .into_iter()
        .flat_map(|a| split_csv(&a))
        .collect()
}

/// The brace argument of the first `\name{...}` in `text`.
fn first_macro_arg(text: &str, name: &str) -> Option<String> {
    collect_macro_args(text, name).into_iter().next()
}

/// The brace arguments of every `\name{...}` occurrence in `text`, in order.
fn collect_macro_args(text: &str, name: &str) -> Vec<String> {
    let marker = format!("\\{name}");
    let bytes = text.as_bytes();
    let mut args = Vec::new();
    let mut i = 0;
    while let Some(pos) = find_from(text, &marker, i) {
        let after = pos + marker.len();
        // Reject a longer macro name (e.g. \leanok when scanning \lean).
        if let Some(&c) = bytes.get(after) {
            if (c as char).is_ascii_alphabetic() {
                i = after;
                continue;
            }
        }
        if bytes.get(after) == Some(&b'{') {
            if let Some((arg, end)) = read_braces(text, after) {
                args.push(arg);
                i = end;
                continue;
            }
        }
        i = after;
    }
    args
}

/// True if the no-argument flag macro `\name` occurs in `text` (not as a prefix
/// of a longer macro name).
fn has_flag(text: &str, name: &str) -> bool {
    let marker = format!("\\{name}");
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(pos) = find_from(text, &marker, i) {
        let after = pos + marker.len();
        let ok = match bytes.get(after) {
            Some(&c) => !(c as char).is_ascii_alphabetic(),
            None => true,
        };
        if ok {
            return true;
        }
        i = after;
    }
    false
}

/// Read a `{...}` group starting at `open` (index of the `{`), returning the
/// inner text and the index just past the closing `}`. Handles one level of
/// nested braces.
fn read_braces(text: &str, open: usize) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    if bytes.get(open) != Some(&b'{') {
        return None;
    }
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((text[open + 1..i].to_string(), i + 1));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Split a comma-separated macro argument, trimming whitespace/newlines.
fn split_csv(arg: &str) -> Vec<String> {
    arg.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Remove leanblueprint macro lines / calls, leaving the prose body.
fn strip_macros(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    // Drop the four schema macros (label/lean/uses/proves + the leanok flag)
    // wherever they appear, keeping the surrounding prose.
    while !rest.is_empty() {
        if let Some(pos) = rest.find('\\') {
            out.push_str(&rest[..pos]);
            let tail = &rest[pos..];
            let mut matched = false;
            for name in ["label", "lean", "uses", "proves"] {
                let marker = format!("\\{name}{{");
                if tail.starts_with(&marker) {
                    if let Some((_, end)) = read_braces(tail, marker.len() - 1) {
                        rest = &tail[end..];
                        matched = true;
                        break;
                    }
                }
            }
            if matched {
                continue;
            }
            if tail.starts_with("\\leanok") {
                rest = &tail["\\leanok".len()..];
                continue;
            }
            // An unrelated backslash: keep it and advance one char.
            out.push('\\');
            rest = &tail[1..];
        } else {
            out.push_str(rest);
            break;
        }
    }
    out.trim().to_string()
}

// ------------------------------------------------------------------------
// Kind <-> environment/label mappings
// ------------------------------------------------------------------------

fn env_for_kind(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Conjecture | NodeKind::FormalStatement => "theorem",
        NodeKind::Lemma | NodeKind::Computation => "lemma",
        NodeKind::Obligation => "proposition",
        NodeKind::Definition | NodeKind::Assumption => "definition",
        _ => "remark",
    }
}

fn kind_for_env(env: &str) -> NodeKind {
    match env {
        "theorem" => NodeKind::Conjecture,
        "lemma" | "corollary" => NodeKind::Lemma,
        "proposition" => NodeKind::Obligation,
        "definition" => NodeKind::Definition,
        _ => NodeKind::Strategy,
    }
}

fn label_prefix(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Conjecture | NodeKind::FormalStatement => "thm",
        NodeKind::Lemma | NodeKind::Computation => "lem",
        NodeKind::Obligation => "prop",
        NodeKind::Definition | NodeKind::Assumption => "def",
        _ => "nd",
    }
}

/// A kebab-case slug from a node title for use as a blueprint label suffix.
fn slugify(title: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = false;
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    let s = slug.trim_matches('-').to_string();
    if s.is_empty() {
        "node".into()
    } else {
        s
    }
}

/// Top-level declaration names in a Lean source, in order — what a node's
/// `\lean{}` binds. Robust to leading attributes and stacked modifiers.
fn lean_decl_names(src: &str) -> Vec<String> {
    const MODIFIERS: &[&str] = &[
        "private ",
        "protected ",
        "noncomputable ",
        "nonrec ",
        "partial ",
        "unsafe ",
        "public ",
        "scoped ",
        "local ",
    ];
    const DECLS: &[&str] = &["theorem ", "lemma ", "def ", "abbrev ", "instance "];
    let mut names = Vec::new();
    for line in src.lines() {
        let mut rest = line.trim_start();
        while let Some(after) = rest.strip_prefix("@[") {
            match after.find(']') {
                Some(close) => rest = after[close + 1..].trim_start(),
                None => break,
            }
        }
        loop {
            let mut stripped = false;
            for m in MODIFIERS {
                if let Some(after) = rest.strip_prefix(m) {
                    rest = after.trim_start();
                    stripped = true;
                    break;
                }
            }
            if !stripped {
                break;
            }
        }
        for kw in DECLS {
            if let Some(after) = rest.strip_prefix(kw) {
                let name: String = after
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '\''))
                    .collect();
                if !name.is_empty() {
                    names.push(name);
                }
                break;
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn sample_blueprint() -> Blueprint {
        Blueprint {
            nodes: vec![
                BlueprintNode {
                    env: "lemma".into(),
                    label: "lem:mertens".into(),
                    title: Some("Mertens estimate".into()),
                    lean: vec!["PrimitiveSetsAboveX.mertens".into()],
                    statement_uses: vec![],
                    proof_uses: vec![],
                    statement_leanok: true,
                    proof_leanok: true,
                    statement_body: "A Mertens-type bound holds.".into(),
                    proof_body: "By partial summation.".into(),
                },
                BlueprintNode {
                    env: "lemma".into(),
                    label: "lem:tail".into(),
                    title: Some("Tail estimate".into()),
                    lean: vec!["PrimitiveSetsAboveX.tailEstimate".into()],
                    // Statement needs mertens; the proof additionally uses chebyshev.
                    statement_uses: vec!["lem:mertens".into()],
                    proof_uses: vec!["lem:mertens".into(), "lem:chebyshev".into()],
                    statement_leanok: true,
                    proof_leanok: false,
                    statement_body: "The tail is small.".into(),
                    proof_body: "Combine the bounds.".into(),
                },
                BlueprintNode {
                    env: "theorem".into(),
                    label: "thm:main".into(),
                    title: None,
                    lean: vec!["Erdos1196.main".into(), "Erdos1196.main'".into()],
                    statement_uses: vec!["lem:tail".into()],
                    proof_uses: vec![],
                    statement_leanok: false,
                    proof_leanok: false,
                    statement_body: "The main result.".into(),
                    proof_body: String::new(),
                },
            ],
        }
    }

    #[test]
    fn tex_round_trip_preserves_the_graph() {
        let bp = sample_blueprint();
        let tex = bp.to_tex();
        // Quote-match the exact leanblueprint macro syntax.
        assert!(tex.contains("\\begin{lemma}[Tail estimate]\\label{lem:tail}"));
        assert!(tex.contains("\\lean{PrimitiveSetsAboveX.tailEstimate}"));
        assert!(tex.contains("\\uses{lem:mertens,lem:chebyshev}"));
        assert!(tex.contains("\\leanok"));
        let reparsed = Blueprint::from_tex(&tex);
        assert_eq!(reparsed, bp, "emit -> ingest must reproduce the graph");
    }

    #[test]
    fn manifest_lists_every_lean_binding() {
        let bp = sample_blueprint();
        assert_eq!(
            bp.lean_decls(),
            vec![
                "PrimitiveSetsAboveX.mertens".to_string(),
                "PrimitiveSetsAboveX.tailEstimate".to_string(),
                "Erdos1196.main".to_string(),
                "Erdos1196.main'".to_string(),
            ]
        );
    }

    #[test]
    fn store_export_import_round_trip() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let p = store.create_project("p", "t").unwrap();
        let a = store.add_node(&p.id, NodeKind::Lemma, "Alpha bound", "A holds", "test").unwrap();
        let b = store.add_node(&p.id, NodeKind::Lemma, "Beta bound", "B holds", "test").unwrap();
        let main = store
            .add_node(&p.id, NodeKind::Conjecture, "Main theorem", "T holds", "test")
            .unwrap();
        // main's statement depends on a; its proof additionally depends on b.
        store
            .add_edge_scoped(&p.id, &main.id, &a.id, EdgeKind::DependsOn, DepScope::Statement)
            .unwrap();
        store
            .add_edge_scoped(&p.id, &main.id, &b.id, EdgeKind::DependsOn, DepScope::Proof)
            .unwrap();
        store.set_verification_flags(&p.id, &a.id, true, true, "test").unwrap();
        store.set_verification_flags(&p.id, &main.id, true, false, "test").unwrap();

        let exported = export(&store, &p.id).unwrap();

        // Ingest into a fresh project and compare the reconstructed graph.
        let q = store.create_project("q", "t").unwrap();
        let summary = import(&store, &q.id, &exported.content_tex).unwrap();
        assert_eq!(summary.nodes_created, 3);
        assert_eq!(summary.edges_created, 2);
        assert!(summary.unresolved_uses.is_empty());

        let qnodes = store.nodes(&q.id).unwrap();
        assert_eq!(qnodes.len(), 3);
        let qmain = qnodes.iter().find(|n| n.title == "Main theorem").unwrap();
        assert!(qmain.stmt_formalized && !qmain.proof_done);
        let qa = qnodes.iter().find(|n| n.title == "Alpha bound").unwrap();
        assert!(qa.stmt_formalized && qa.proof_done);

        let qedges = store.edges(&q.id).unwrap();
        let to_a = qedges.iter().find(|e| e.target_id == qa.id).unwrap();
        assert_eq!(to_a.dep_scope, DepScope::Statement);
        let qb = qnodes.iter().find(|n| n.title == "Beta bound").unwrap();
        let to_b = qedges.iter().find(|e| e.target_id == qb.id).unwrap();
        assert_eq!(to_b.dep_scope, DepScope::Proof);
    }

    #[test]
    fn ingest_flags_unresolved_uses() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let p = store.create_project("p", "t").unwrap();
        let tex = "\\begin{lemma}\\label{lem:a}\n\\uses{lem:missing}\nBody.\n\\end{lemma}\n";
        let summary = import(&store, &p.id, tex).unwrap();
        assert_eq!(summary.nodes_created, 1);
        assert_eq!(summary.edges_created, 0);
        assert_eq!(summary.unresolved_uses, vec!["lem:missing".to_string()]);
    }

    #[test]
    fn check_decls_static_finds_and_misses() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Main.lean"),
            "theorem Foo.bar : True := trivial\nlemma baz : True := trivial\n",
        )
        .unwrap();
        let report = check_decls(
            dir.path(),
            &["Foo.bar".to_string(), "Missing.qux".to_string()],
        )
        .unwrap();
        assert!(report.ran);
        // On a machine without lake this is the static path.
        assert_eq!(report.mode, CheckDeclsMode::Static);
        assert_eq!(report.total, 2);
        assert_eq!(report.resolved, 1);
        assert_eq!(report.missing, vec!["Missing.qux".to_string()]);
    }

    #[test]
    fn check_decls_skips_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let report = check_decls(dir.path(), &[]).unwrap();
        assert!(!report.ran);
        assert_eq!(report.mode, CheckDeclsMode::Skipped);
    }
}
