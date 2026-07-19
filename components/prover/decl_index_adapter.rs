//! A **real** [`DeclarationIndex`] backed by the Python `decl_index` worker.
//!
//! [`crate::prover::declaration_lookup`] defines the four-valued lookup that
//! cures *environmental scope collapse*, but it ships with only mock indices —
//! the capability seam has no production implementor, so nothing can actually
//! call it. This module is that implementor.
//!
//! # What backs each tier
//!
//! | Tier | Method | Backing call |
//! |---|---|---|
//! | fast | [`DeclarationIndex::lookup_in_manifest`] | `decl_index` dumped under **the problem's own imports** |
//! | slow | [`DeclarationIndex::lookup_in_library`] | `decl_index` dumped under **the wide corpus roots** (default `Mathlib`) |
//!
//! The manifest tier does not post-filter with [`ImportManifest::covers`], and
//! that is deliberate. `covers` is a *prefix* predicate over module paths; the
//! real scope predicate is the transitive **import closure** (the thing
//! `accessible_premises.import_closure` computes over the source import DAG).
//! Lean's own elaborator computes exactly that closure when it imports the
//! manifest's modules, so dumping the environment under those imports *is* the
//! closure — no approximation, no second, stricter filter layered on top. A
//! prefix filter here would demote genuinely-in-scope declarations to
//! "undecided", which is safe but wrong: it would report `add the import` for an
//! import that is already there.
//!
//! # The invariant this file exists to hold
//!
//! **No infrastructure failure may ever become `Ok(None)`.** `Ok(None)` is a
//! claim about mathematics ("this declaration does not exist"); a missing
//! interpreter, an absent Lean toolchain, a blown deadline, or unparseable
//! worker output are claims about *our machine*. Every path below that cannot
//! prove a clean, complete, parsed negative answer returns `Err(IndexError)`:
//!
//! * worker unavailable / would not spawn → [`IndexErrorKind::ToolchainUnavailable`]
//! * `lean` / `lake` not found (worker said so) → [`IndexErrorKind::ToolchainUnavailable`]
//! * `{"ok": false}` envelope, or `{"ok": true}` with a failed inner payload → classified, else [`IndexErrorKind::Other`]
//! * dump produced zero declarations → [`IndexErrorKind::IndexMissing`] (an empty
//!   index is a broken index, **not** an empty library)
//! * stdout that is not the expected JSON envelope → [`IndexErrorKind::Malformed`]
//! * the worker's deadline fired → [`IndexErrorKind::Timeout`]
//! * a search result set truncated by `limit` without an exact hit → [`IndexErrorKind::Other`]
//!   (we did not see the whole result set, so we cannot report absence)
//!
//! `Ok(None)` is reachable from exactly one place: the worker ran to completion,
//! reported success, returned an untruncated result set, and no entry in it had
//! the queried name.
//!
//! # Timeouts
//!
//! [`crate::prover::declaration_lookup`]'s docs put deadline enforcement on the
//! implementor. To be plain about what exists: **[`crate::tools::PythonCheck`]
//! has no timeout of its own.** Its `run` spawns the interpreter and blocks in
//! `wait_with_output()` with no deadline, so this adapter cannot bound the
//! Python process from the Rust side without changing that type (which this file
//! does not own). What it *can* do — and does — is pass a `timeout` field in the
//! request, which `decl_index.dump` enforces around the `lean`/`lake` subprocess
//! (SIGTERM→SIGKILL escalation) and reports as `lean dump timed out after Ns`.
//! That is the deadline that actually fires in practice, since the Lean dump is
//! where all the time goes, and this adapter maps it to
//! [`IndexErrorKind::Timeout`]. A hung *interpreter* (as opposed to a hung Lean)
//! would still block; closing that hole requires a timeout on `PythonCheck`.
//!
//! # Systems other than Lean
//!
//! `decl_index` dumps a **Lean** environment. Asked about any other
//! [`FormalSystem`], this adapter returns
//! [`IndexErrorKind::ToolchainUnavailable`] — "we have no index for this system"
//! — and never `Ok(None)`, which would falsely assert that Rocq's or Isabelle's
//! libraries lack the name.

use crate::model::ToolResult;
use crate::prover::declaration_lookup::{
    normalize, Declaration, DeclarationIndex, ImportManifest, IndexError, IndexErrorKind,
};
use crate::prover::formal::FormalSystem;
use crate::tools::{PythonCheck, Tool};
use serde_json::{json, Value};

/// How many `decl_index` search hits to request. The worker reports the
/// pre-truncation `count`, so truncation is detectable — and when it happens
/// without an exact hit we error rather than infer absence.
const SEARCH_LIMIT: u64 = 500;

/// Default deadline handed to the worker for one Lean dump, in seconds. Matches
/// `decl_index.run`'s own default.
const DEFAULT_TIMEOUT_SECONDS: f64 = 300.0;

// ---------------------------------------------------------------------------
// The worker seam
// ---------------------------------------------------------------------------

/// The one capability this adapter needs: send a JSON request to the Python
/// worker, get its [`ToolResult`] back. Existing so the failure modes — the
/// whole point of this file — are testable without a Python interpreter.
pub trait DeclWorker {
    /// Whether the worker could plausibly run at all.
    fn available(&self) -> bool;
    /// Run one request. `Err` means the worker did not run; it is never absence.
    fn invoke(&self, request: Value) -> Result<ToolResult, String>;
}

impl DeclWorker for PythonCheck {
    fn available(&self) -> bool {
        Tool::available(self)
    }

    fn invoke(&self, request: Value) -> Result<ToolResult, String> {
        Tool::run(self, request).map_err(|e| e.to_string())
    }
}

// ---------------------------------------------------------------------------
// The index
// ---------------------------------------------------------------------------

/// A [`DeclarationIndex`] over the Python `decl_index` worker.
pub struct PythonDeclIndex<W: DeclWorker = PythonCheck> {
    worker: W,
    /// The Lake project root to resolve imports against (Mathlib's checkout).
    /// `None` runs bare `lean`, which only resolves the core library.
    root: Option<String>,
    /// The wide-corpus roots the slow tier dumps. Default `["Mathlib"]`.
    library_imports: Vec<String>,
    /// Explicit `lean`/`lake` binary, when the worker should not search PATH.
    lean_bin: Option<String>,
    /// Per-dump deadline in seconds, enforced by the worker around Lean.
    timeout_seconds: f64,
}

impl PythonDeclIndex<PythonCheck> {
    /// The production index: the real Python worker, dumping `root`'s Lake
    /// project.
    pub fn new(root: Option<String>) -> Self {
        Self::with_worker(PythonCheck::new(), root)
    }
}

impl<W: DeclWorker> PythonDeclIndex<W> {
    /// Build over an arbitrary worker (the test seam).
    pub fn with_worker(worker: W, root: Option<String>) -> Self {
        Self {
            worker,
            root,
            library_imports: vec!["Mathlib".to_string()],
            lean_bin: None,
            timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
        }
    }

    /// Override the wide-corpus roots the slow tier searches.
    pub fn with_library_imports<I, S>(mut self, imports: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let imports: Vec<String> = imports.into_iter().map(Into::into).collect();
        if !imports.is_empty() {
            self.library_imports = imports;
        }
        self
    }

    pub fn with_lean_bin(mut self, bin: impl Into<String>) -> Self {
        self.lean_bin = Some(bin.into());
        self
    }

    /// Set the per-dump deadline the worker enforces around Lean.
    pub fn with_timeout_seconds(mut self, seconds: f64) -> Self {
        if seconds.is_finite() && seconds > 0.0 {
            self.timeout_seconds = seconds;
        }
        self
    }

    /// The exact request sent to the worker for one lookup.
    fn request(&self, imports: &[String], name: &str) -> Value {
        json!({
            "tool": "decl_index",
            "root": self.root,
            "imports": imports,
            "query": "search",
            "substring": name,
            "limit": SEARCH_LIMIT,
            "lean_bin": self.lean_bin,
            "timeout": self.timeout_seconds,
        })
    }

    /// One tier: dump under `imports`, search for `name`, resolve to an exact
    /// match. Every non-clean path is an `Err`.
    fn lookup(
        &self,
        system: FormalSystem,
        name: &str,
        imports: &[String],
        tier: &str,
    ) -> Result<Option<Declaration>, IndexError> {
        // `decl_index` dumps a LEAN environment. For any other system we have no
        // index — say so; do not answer "absent".
        if system != FormalSystem::Lean {
            return Err(IndexError::toolchain_unavailable(format!(
                "the decl_index worker indexes Lean only; no declaration index is configured for \
                 {} (this is a gap in our tooling, not evidence about {}'s library)",
                system.as_str(),
                system.as_str()
            )));
        }

        let name = normalize(system, name);
        if name.is_empty() {
            return Err(IndexError::new(
                IndexErrorKind::Other,
                "refusing to query the declaration index for an empty name",
            ));
        }

        if !self.worker.available() {
            return Err(IndexError::toolchain_unavailable(format!(
                "the python decl_index worker is unavailable (interpreter or \
                 components/tools/python/theoremata_tools/worker.py missing); {tier} tier not run"
            )));
        }

        let result = self
            .worker
            .invoke(self.request(imports, &name))
            .map_err(|e| {
                IndexError::toolchain_unavailable(format!(
                    "could not run the python decl_index worker ({tier} tier): {}",
                    clip(&e)
                ))
            })?;

        let payload = self.parse_envelope(&result, tier)?;
        self.resolve(&payload, &name, tier)
    }

    /// Unwrap `{"ok": bool, "output": …}` and then the inner worker payload.
    /// Both `ok` flags are load-bearing: either being false is an `Err`.
    fn parse_envelope(&self, result: &ToolResult, tier: &str) -> Result<Value, IndexError> {
        let Some(line) = result
            .stdout
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .next_back()
        else {
            // No stdout at all. The process may also have exited non-zero; either
            // way we have no answer, so we report the failure, not an absence.
            return Err(classify(
                &format!("{} {}", result.summary, result.stderr),
                IndexErrorKind::Malformed,
                format!("the decl_index worker ({tier} tier) produced no output"),
            ));
        };

        let envelope: Value = serde_json::from_str(line).map_err(|e| {
            IndexError::new(
                IndexErrorKind::Malformed,
                format!(
                    "the decl_index worker ({tier} tier) returned unparseable JSON: {e}; \
                     stdout was {:?}",
                    clip(line)
                ),
            )
        })?;

        // Outer envelope. `{"ok": false}` carries the dispatch error string.
        if envelope.get("ok") != Some(&Value::Bool(true)) {
            let detail = envelope
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("(no error field)");
            return Err(classify(
                detail,
                IndexErrorKind::Other,
                format!("the decl_index worker ({tier} tier) reported failure: {}", clip(detail)),
            ));
        }

        let Some(output) = envelope.get("output").filter(|o| o.is_object()) else {
            return Err(IndexError::new(
                IndexErrorKind::Malformed,
                format!(
                    "the decl_index worker ({tier} tier) returned ok:true with no object payload"
                ),
            ));
        };

        // Inner payload. `decl_index.run` sets ok:false for a failed dump — a
        // missing toolchain, a timeout, OR a dump that yielded zero declarations.
        // An empty index is a BROKEN index, never an empty library.
        if output.get("ok") != Some(&Value::Bool(true)) {
            let stderr = output.get("stderr").and_then(Value::as_str).unwrap_or("");
            let count = output.get("count").and_then(Value::as_u64);
            if stderr.trim().is_empty() && count == Some(0) {
                return Err(IndexError::index_missing(format!(
                    "the decl_index dump ({tier} tier) yielded zero declarations — the index is \
                     not built or the imports resolved to nothing. This says nothing about \
                     whether the declaration exists."
                )));
            }
            return Err(classify(
                stderr,
                IndexErrorKind::IndexMissing,
                format!("the decl_index dump ({tier} tier) failed: {}", clip(stderr)),
            ));
        }

        Ok(output.clone())
    }

    /// Find the exact-name hit in a successful `query: "search"` payload.
    fn resolve(
        &self,
        payload: &Value,
        name: &str,
        tier: &str,
    ) -> Result<Option<Declaration>, IndexError> {
        let Some(matches) = payload.get("matches").and_then(Value::as_array) else {
            return Err(IndexError::new(
                IndexErrorKind::Malformed,
                format!("the decl_index worker ({tier} tier) returned no `matches` array"),
            ));
        };

        // Search is a case-insensitive substring scan; we want the exact name.
        for entry in matches {
            let Some(found) = entry.get("name").and_then(Value::as_str) else {
                return Err(IndexError::new(
                    IndexErrorKind::Malformed,
                    format!("a decl_index match ({tier} tier) has no string `name`"),
                ));
            };
            if normalize(FormalSystem::Lean, found) == name {
                return Ok(Some(declaration_from(entry, found)));
            }
        }

        // Truncation: we did not see the whole result set, so "not among the ones
        // we saw" is not "not there". Error rather than fabricate an absence.
        let count = payload.get("count").and_then(Value::as_u64).unwrap_or(0);
        if count > matches.len() as u64 {
            return Err(IndexError::new(
                IndexErrorKind::Other,
                format!(
                    "the decl_index search ({tier} tier) matched {count} names but returned only \
                     {} — the result set was truncated, so `{name}` cannot be ruled out",
                    matches.len()
                ),
            ));
        }

        // The one legitimate absence: complete, successful, untruncated, no hit.
        Ok(None)
    }
}

impl<W: DeclWorker> DeclarationIndex for PythonDeclIndex<W> {
    fn lookup_in_manifest(
        &self,
        system: FormalSystem,
        name: &str,
        manifest: &ImportManifest,
    ) -> Result<Option<Declaration>, IndexError> {
        // The manifest's own imports. `Init` when nothing is imported: Lean's
        // prelude is always in scope, and it is what `decl_index` defaults to.
        let mut imports: Vec<String> = manifest.imports().to_vec();
        if imports.is_empty() {
            imports.push("Init".to_string());
        }
        self.lookup(system, name, &imports, "manifest")
    }

    fn lookup_in_library(
        &self,
        system: FormalSystem,
        name: &str,
    ) -> Result<Option<Declaration>, IndexError> {
        let imports = self.library_imports.clone();
        self.lookup(system, name, &imports, "library")
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a [`Declaration`] from one `{"name","kind","module","is_axiom"}` record.
///
/// `decl_index` does not emit a type/statement, so `signature` stays `None`.
fn declaration_from(entry: &Value, name: &str) -> Declaration {
    let module = entry
        .get("module")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|m| !m.is_empty());
    let mut decl = match module {
        Some(module) => Declaration::in_module(FormalSystem::Lean, name, module),
        None => Declaration::new(FormalSystem::Lean, name),
    };
    let kind = entry
        .get("kind")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_string)
        .or_else(|| {
            (entry.get("is_axiom") == Some(&Value::Bool(true))).then(|| "axiom".to_string())
        });
    if let Some(kind) = kind {
        decl = decl.with_kind(kind);
    }
    decl
}

/// Pick an [`IndexErrorKind`] from a worker diagnostic string, falling back to
/// `default`. Every arm produces an `Err`; the classification only affects
/// retry advice, never whether absence is inferred.
fn classify(diagnostic: &str, default: IndexErrorKind, detail: String) -> IndexError {
    let lower = diagnostic.to_ascii_lowercase();
    let kind = if lower.contains("timed out") || lower.contains("timeout") {
        IndexErrorKind::Timeout
    } else if lower.contains("not found")
        || lower.contains("no such file")
        || lower.contains("no python interpreter")
    {
        IndexErrorKind::ToolchainUnavailable
    } else {
        default
    };
    IndexError::new(kind, detail)
}

/// Truncate a diagnostic so an error message stays loggable.
fn clip(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() <= 400 {
        return s.to_string();
    }
    let head: String = s.chars().take(400).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prover::declaration_lookup::{deep_check, fast_check, Verdict};
    use std::cell::RefCell;

    // -- a scriptable worker --------------------------------------------------

    /// Canned worker responses, plus a record of every request sent.
    struct FakeWorker {
        available: bool,
        /// One scripted response per call, in order; the last one repeats.
        responses: Vec<Result<ToolResult, String>>,
        requests: RefCell<Vec<Value>>,
        calls: RefCell<usize>,
    }

    impl FakeWorker {
        fn stdout(lines: &str) -> Self {
            Self::responses(vec![Ok(tool_result(lines, true, ""))])
        }

        fn responses(responses: Vec<Result<ToolResult, String>>) -> Self {
            Self {
                available: true,
                responses,
                requests: RefCell::new(Vec::new()),
                calls: RefCell::new(0),
            }
        }

        fn unavailable() -> Self {
            Self {
                available: false,
                responses: Vec::new(),
                requests: RefCell::new(Vec::new()),
                calls: RefCell::new(0),
            }
        }

        fn call_count(&self) -> usize {
            *self.calls.borrow()
        }

        fn request(&self, i: usize) -> Value {
            self.requests.borrow()[i].clone()
        }
    }

    impl DeclWorker for FakeWorker {
        fn available(&self) -> bool {
            self.available
        }

        fn invoke(&self, request: Value) -> Result<ToolResult, String> {
            self.requests.borrow_mut().push(request);
            let i = *self.calls.borrow();
            *self.calls.borrow_mut() = i + 1;
            let idx = i.min(self.responses.len().saturating_sub(1));
            match self.responses.get(idx) {
                Some(Ok(r)) => Ok(r.clone()),
                Some(Err(e)) => Err(e.clone()),
                None => Err("no scripted response".to_string()),
            }
        }
    }

    fn tool_result(stdout: &str, success: bool, stderr: &str) -> ToolResult {
        ToolResult {
            tool: "python_check".into(),
            success,
            summary: if success {
                "completed".into()
            } else {
                "exited with 2".into()
            },
            stdout: stdout.into(),
            stderr: stderr.into(),
            duration_ms: 1,
            metadata: json!({}),
        }
    }

    /// A healthy `{"ok":true,"output":{...search payload...}}` envelope.
    fn healthy(matches: Value) -> String {
        let count = matches.as_array().map(Vec::len).unwrap_or(0);
        json!({
            "ok": true,
            "output": {"ok": true, "query": "search", "count": count, "matches": matches},
        })
        .to_string()
    }

    fn succ_le_succ() -> Value {
        json!([{
            "name": "Nat.succ_le_succ",
            "kind": "theorem",
            "module": "Mathlib.Data.Nat.Basic",
            "is_axiom": false,
        }])
    }

    fn manifest() -> ImportManifest {
        ImportManifest::new(FormalSystem::Lean, ["Mathlib.Data.Nat"])
    }

    fn index(worker: FakeWorker) -> PythonDeclIndex<FakeWorker> {
        PythonDeclIndex::with_worker(worker, Some("resources/mathlib4".into()))
    }

    /// Every failure mode must be `Err`, and must NEVER be `Ok(None)`.
    #[track_caller]
    fn assert_error_kind(
        result: Result<Option<Declaration>, IndexError>,
        expected: IndexErrorKind,
    ) -> IndexError {
        match result {
            Err(e) => {
                assert_eq!(e.kind, expected, "wrong kind: {e:?}");
                assert!(!e.detail.is_empty());
                e
            }
            Ok(None) => panic!(
                "an infrastructure failure was laundered into Ok(None) — that is the claim \
                 'this declaration does not exist', which the lookup never established"
            ),
            Ok(Some(d)) => panic!("expected an error, got {d:?}"),
        }
    }

    // -- the happy paths -------------------------------------------------------

    #[test]
    fn a_found_declaration_parses_into_a_declaration_with_module_and_kind() {
        let idx = index(FakeWorker::stdout(&healthy(succ_le_succ())));
        let found = idx
            .lookup_in_manifest(FormalSystem::Lean, "Nat.succ_le_succ", &manifest())
            .expect("healthy worker")
            .expect("the name is in the index");
        assert_eq!(found.name, "Nat.succ_le_succ");
        assert_eq!(found.module.as_deref(), Some("Mathlib.Data.Nat.Basic"));
        assert_eq!(found.kind.as_deref(), Some("theorem"));
        assert_eq!(found.system, FormalSystem::Lean);
        // decl_index emits no type, so signature is honestly absent.
        assert_eq!(found.signature, None);
        assert_eq!(
            found.effective_module().as_deref(),
            Some("Mathlib.Data.Nat.Basic")
        );
    }

    #[test]
    fn an_axiom_record_without_a_kind_is_labelled_axiom() {
        let idx = index(FakeWorker::stdout(&healthy(json!([{
            "name": "Classical.choice", "module": "Init.Core", "is_axiom": true,
        }]))));
        let found = idx
            .lookup_in_library(FormalSystem::Lean, "Classical.choice")
            .unwrap()
            .unwrap();
        assert_eq!(found.kind.as_deref(), Some("axiom"));
    }

    #[test]
    fn a_name_absent_from_a_healthy_index_is_ok_none() {
        // The ONLY route to Ok(None): worker ran, ok:true at both layers, the
        // result set was complete, and nothing matched.
        let idx = index(FakeWorker::stdout(&healthy(json!([]))));
        let out = idx
            .lookup_in_library(FormalSystem::Lean, "Nat.totally_made_up_lemma")
            .expect("a healthy worker must not error");
        assert_eq!(out, None);
    }

    #[test]
    fn a_substring_hit_that_is_not_the_exact_name_is_not_a_match() {
        // `search` is a substring scan: querying `Nat.succ_le` must not resolve
        // to `Nat.succ_le_succ`.
        let idx = index(FakeWorker::stdout(&healthy(succ_le_succ())));
        assert_eq!(
            idx.lookup_in_library(FormalSystem::Lean, "Nat.succ_le")
                .unwrap(),
            None
        );
    }

    // -- THE requirement: every infra failure is Err, never Ok(None) ----------

    #[test]
    fn a_missing_worker_is_toolchain_unavailable_never_ok_none() {
        let idx = index(FakeWorker::unavailable());
        for name in ["Nat.succ_le_succ", "Nat.totally_made_up_lemma"] {
            assert_error_kind(
                idx.lookup_in_manifest(FormalSystem::Lean, name, &manifest()),
                IndexErrorKind::ToolchainUnavailable,
            );
            assert_error_kind(
                idx.lookup_in_library(FormalSystem::Lean, name),
                IndexErrorKind::ToolchainUnavailable,
            );
        }
    }

    #[test]
    fn a_worker_that_will_not_spawn_is_toolchain_unavailable_never_ok_none() {
        let idx = index(FakeWorker::responses(vec![Err(
            "no python interpreter found (tried python3, python)".into(),
        )]));
        assert_error_kind(
            idx.lookup_in_library(FormalSystem::Lean, "Nat.nope"),
            IndexErrorKind::ToolchainUnavailable,
        );
    }

    #[test]
    fn an_ok_false_envelope_is_an_error_never_ok_none() {
        // Non-zero exit AND ok:false — the worker's own dispatch failure.
        let body = json!({"ok": false, "error": "unknown tool: decl_index"}).to_string();
        let idx = index(FakeWorker::responses(vec![Ok(tool_result(&body, false, ""))]));
        let e = assert_error_kind(
            idx.lookup_in_library(FormalSystem::Lean, "Nat.nope"),
            IndexErrorKind::Other,
        );
        assert!(e.detail.contains("unknown tool"), "{e:?}");
    }

    #[test]
    fn an_inner_ok_false_payload_is_an_error_never_ok_none() {
        // The envelope succeeded; the DUMP failed. Still not absence.
        let body = json!({
            "ok": true,
            "output": {"ok": false, "count": 0, "decls": [], "stderr": "lean not found"},
        })
        .to_string();
        let idx = index(FakeWorker::stdout(&body));
        assert_error_kind(
            idx.lookup_in_library(FormalSystem::Lean, "Nat.succ_le_succ"),
            IndexErrorKind::ToolchainUnavailable,
        );
    }

    #[test]
    fn an_empty_dump_is_index_missing_not_absence() {
        // `decl_index` sets ok = (count > 0), so a zero-declaration dump arrives
        // looking exactly like "nothing matched". It is a broken index.
        let body = json!({
            "ok": true,
            "output": {"ok": false, "count": 0, "decls": [], "stderr": ""},
        })
        .to_string();
        let idx = index(FakeWorker::stdout(&body));
        let e = assert_error_kind(
            idx.lookup_in_library(FormalSystem::Lean, "Nat.succ_le_succ"),
            IndexErrorKind::IndexMissing,
        );
        assert!(e.detail.contains("zero declarations"), "{e:?}");
    }

    #[test]
    fn malformed_json_is_a_malformed_error_never_ok_none() {
        for stdout in [
            "this is not json at all",
            "{\"ok\": true, \"output\": ",
            "{\"ok\": true, \"output\": \"a string, not an object\"}",
            "{\"ok\": true, \"output\": {\"ok\": true, \"count\": 0}}", // no `matches`
            "{\"ok\": true, \"output\": {\"ok\": true, \"count\": 1, \"matches\": [{\"kind\": \"theorem\"}]}}",
        ] {
            let idx = index(FakeWorker::stdout(stdout));
            assert_error_kind(
                idx.lookup_in_library(FormalSystem::Lean, "Nat.nope"),
                IndexErrorKind::Malformed,
            );
        }
    }

    #[test]
    fn no_output_at_all_is_an_error_never_ok_none() {
        let idx = index(FakeWorker::responses(vec![Ok(tool_result("   \n", false, "Traceback"))]));
        assert_error_kind(
            idx.lookup_in_library(FormalSystem::Lean, "Nat.nope"),
            IndexErrorKind::Malformed,
        );
    }

    #[test]
    fn a_timeout_is_a_timeout_error_never_ok_none() {
        // The worker's own deadline around the Lean dump.
        let body = json!({
            "ok": true,
            "output": {
                "ok": false, "count": 0, "decls": [],
                "stderr": "lean dump timed out after 300.0s",
            },
        })
        .to_string();
        let idx = index(FakeWorker::stdout(&body));
        let e = assert_error_kind(
            idx.lookup_in_library(FormalSystem::Lean, "Nat.succ_le_succ"),
            IndexErrorKind::Timeout,
        );
        assert!(e.kind.is_transient(), "a timeout is worth retrying");

        // And a timeout surfaced through the outer envelope instead.
        let outer = json!({"ok": false, "error": "Rust meta-tool API timed out"}).to_string();
        let idx = index(FakeWorker::stdout(&outer));
        assert_error_kind(
            idx.lookup_in_library(FormalSystem::Lean, "Nat.succ_le_succ"),
            IndexErrorKind::Timeout,
        );
    }

    #[test]
    fn a_truncated_result_set_is_an_error_never_ok_none() {
        // 900 matches, 500 returned, none exact: we did not see them all.
        let body = json!({
            "ok": true,
            "output": {"ok": true, "query": "search", "count": 900, "matches": succ_le_succ()},
        })
        .to_string();
        let idx = index(FakeWorker::stdout(&body));
        let e = assert_error_kind(
            idx.lookup_in_library(FormalSystem::Lean, "Nat.add_comm"),
            IndexErrorKind::Other,
        );
        assert!(e.detail.contains("truncated"), "{e:?}");
    }

    #[test]
    fn a_non_lean_system_errors_rather_than_denying_its_library() {
        let idx = index(FakeWorker::stdout(&healthy(json!([]))));
        for system in [
            FormalSystem::Rocq,
            FormalSystem::Isabelle,
            FormalSystem::Candle,
            FormalSystem::Agda,
            FormalSystem::Metamath,
        ] {
            assert_error_kind(
                idx.lookup_in_library(system, "some_name"),
                IndexErrorKind::ToolchainUnavailable,
            );
        }
        // And the worker was never even asked.
        assert_eq!(idx.worker.call_count(), 0);
    }

    #[test]
    fn an_empty_name_errors_rather_than_reporting_absence() {
        let idx = index(FakeWorker::stdout(&healthy(json!([]))));
        assert_error_kind(
            idx.lookup_in_library(FormalSystem::Lean, "  `` "),
            IndexErrorKind::Other,
        );
    }

    // -- every failure mode lands on EnvironmentError end-to-end ---------------

    #[test]
    fn every_failure_mode_yields_environment_error_never_unknown_declaration() {
        let cases: Vec<(&str, Vec<Result<ToolResult, String>>)> = vec![
            ("spawn failure", vec![Err("python missing".into())]),
            (
                "ok:false envelope",
                vec![Ok(tool_result(
                    &json!({"ok": false, "error": "boom"}).to_string(),
                    false,
                    "",
                ))],
            ),
            ("malformed json", vec![Ok(tool_result("<html>500</html>", true, ""))]),
            (
                "timeout",
                vec![Ok(tool_result(
                    &json!({"ok": true, "output": {"ok": false, "count": 0, "decls": [],
                            "stderr": "lean dump timed out after 300.0s"}})
                    .to_string(),
                    true,
                    "",
                ))],
            ),
        ];
        for (label, responses) in cases {
            let idx = index(FakeWorker::responses(responses));
            let verdict = deep_check(
                &idx,
                FormalSystem::Lean,
                "Nat.totally_made_up_lemma",
                &manifest(),
            );
            assert!(
                matches!(verdict, Verdict::EnvironmentError(_)),
                "{label}: expected EnvironmentError, got {verdict:?}"
            );
            assert!(
                !verdict.is_evidence_of_absence(),
                "{label}: an infra failure must be evidence of NOTHING"
            );
            assert!(verdict.action().contains("evidence of NOTHING"));
        }
    }

    #[test]
    fn a_healthy_index_still_reaches_unknown_declaration_for_a_real_absence() {
        // The control: when nothing is broken, absence IS reportable. Otherwise
        // the invariant above would be vacuous.
        let idx = index(FakeWorker::stdout(&healthy(json!([]))));
        let verdict = deep_check(
            &idx,
            FormalSystem::Lean,
            "Nat.totally_made_up_lemma",
            &manifest(),
        );
        assert!(verdict.is_evidence_of_absence(), "got {verdict:?}");
        assert_eq!(idx.worker.call_count(), 2, "both tiers were consulted");
    }

    // -- tier separation -------------------------------------------------------

    #[test]
    fn the_manifest_tier_does_not_consult_the_library_tier() {
        let idx = index(FakeWorker::stdout(&healthy(succ_le_succ())));
        let verdict = fast_check(&idx, FormalSystem::Lean, "Nat.succ_le_succ", &manifest());
        assert!(matches!(verdict, Some(Verdict::Found(_))));

        // Exactly one worker call, and it dumped the MANIFEST's imports — not
        // the wide corpus.
        assert_eq!(idx.worker.call_count(), 1);
        let req = idx.worker.request(0);
        assert_eq!(req["imports"], json!(["Mathlib.Data.Nat"]));
        assert_ne!(req["imports"], json!(["Mathlib"]));
    }

    #[test]
    fn the_library_tier_dumps_the_wide_corpus_and_ignores_the_manifest() {
        let idx = PythonDeclIndex::with_worker(
            FakeWorker::stdout(&healthy(succ_le_succ())),
            Some("resources/mathlib4".into()),
        )
        .with_library_imports(["Mathlib", "Std"]);
        idx.lookup_in_library(FormalSystem::Lean, "Nat.succ_le_succ")
            .unwrap()
            .unwrap();
        assert_eq!(idx.worker.request(0)["imports"], json!(["Mathlib", "Std"]));
    }

    #[test]
    fn an_empty_manifest_still_dumps_leans_always_in_scope_prelude() {
        let idx = index(FakeWorker::stdout(&healthy(json!([]))));
        let _ = idx.lookup_in_manifest(
            FormalSystem::Lean,
            "Nat.succ_le_succ",
            &ImportManifest::empty(FormalSystem::Lean),
        );
        assert_eq!(idx.worker.request(0)["imports"], json!(["Init"]));
    }

    // -- the request wire format ----------------------------------------------

    #[test]
    fn the_request_is_the_decl_index_search_contract() {
        let idx = index(FakeWorker::stdout(&healthy(json!([]))))
            .with_lean_bin("/opt/elan/bin/lean")
            .with_timeout_seconds(45.0);
        let _ = idx.lookup_in_manifest(FormalSystem::Lean, " `Nat.succ_le_succ` ", &manifest());
        assert_eq!(
            idx.worker.request(0),
            json!({
                "tool": "decl_index",
                "root": "resources/mathlib4",
                "imports": ["Mathlib.Data.Nat"],
                "query": "search",
                // Normalized before dispatch: backticks and whitespace stripped.
                "substring": "Nat.succ_le_succ",
                "limit": 500,
                "lean_bin": "/opt/elan/bin/lean",
                "timeout": 45.0,
            })
        );
    }

    #[test]
    fn a_null_root_and_lean_bin_are_sent_explicitly() {
        let idx = PythonDeclIndex::with_worker(FakeWorker::stdout(&healthy(json!([]))), None);
        let _ = idx.lookup_in_library(FormalSystem::Lean, "Nat.add");
        let req = idx.worker.request(0);
        assert_eq!(req["root"], Value::Null);
        assert_eq!(req["lean_bin"], Value::Null);
        assert_eq!(req["timeout"], json!(300.0));
    }

    #[test]
    fn a_nonsense_timeout_is_rejected_rather_than_sent() {
        for bad in [0.0, -1.0, f64::NAN] {
            let idx = index(FakeWorker::stdout(&healthy(json!([])))).with_timeout_seconds(bad);
            let _ = idx.lookup_in_library(FormalSystem::Lean, "Nat.add");
            assert_eq!(idx.worker.request(0)["timeout"], json!(300.0));
        }
    }

    // -- classification --------------------------------------------------------

    #[test]
    fn diagnostics_classify_to_the_right_kind() {
        let cases = [
            ("lean dump timed out after 300.0s", IndexErrorKind::Timeout),
            ("lake not found", IndexErrorKind::ToolchainUnavailable),
            ("lean not found", IndexErrorKind::ToolchainUnavailable),
            ("no such file or directory", IndexErrorKind::ToolchainUnavailable),
            ("something else entirely", IndexErrorKind::IndexMissing),
        ];
        for (diagnostic, expected) in cases {
            let e = classify(diagnostic, IndexErrorKind::IndexMissing, "d".into());
            assert_eq!(e.kind, expected, "{diagnostic}");
        }
    }

    #[test]
    fn long_diagnostics_are_clipped() {
        let long = "x".repeat(5_000);
        assert!(clip(&long).chars().count() <= 401);
        assert_eq!(clip("  short  "), "short");
    }
}
