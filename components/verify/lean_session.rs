//! Resident warm-Lean client (plan §8).
//!
//! Spawns the Python `lean_repl serve` loop as a long-lived child that holds
//! ONE warm Mathlib environment, and reuses it across many proof checks within
//! a run — turning a ~25-60s cold `lake env lean` per check into ~milliseconds.
//! Every method returns `Result`; on any failure the caller degrades to the
//! cold `LeanCheck` tool.

use crate::config::Config;
use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

#[derive(Debug, Clone, serde::Serialize)]
pub struct CheckOutcome {
    pub ok: bool,
    pub axioms_clean: bool,
    pub messages: Vec<String>,
    pub axioms: Vec<String>,
    /// Goal states recovered from the REPL's infotree and attached to the error
    /// positions they enclose, most-specific first. Empty when the check passed,
    /// when no tree was returned (an older REPL, or the field was not requested),
    /// or when no error position sat inside a goal-bearing node. A model told the
    /// actual hypotheses and goal at the point it got stuck repairs far more often
    /// than one told only "unsolved goals", which is the whole reason to carry it.
    #[serde(default)]
    pub goal_states: Vec<String>,
}

pub struct LeanSession {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    imports: Vec<String>,
    root: Option<PathBuf>,
}

/// Resolve a working Python interpreter (bare names, then a login shell that
/// sources the user's profile) — mirrors the resolution in `tools.rs`.
fn python_command() -> Option<String> {
    for candidate in ["python3", "python"] {
        let ok = Command::new(candidate)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            return Some(candidate.to_owned());
        }
    }
    let out = Command::new("bash")
        .args([
            "-lc",
            r#"for p in python3 python; do if command -v "$p" >/dev/null 2>&1 && "$p" --version >/dev/null 2>&1; then cygpath -w "$(command -v "$p")" 2>/dev/null || command -v "$p"; break; fi; done"#,
        ])
        .output()
        .ok()?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!path.is_empty()).then_some(path)
}

fn str_vec(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| m.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_outcome(resp: &Value) -> CheckOutcome {
    CheckOutcome {
        ok: resp["ok"].as_bool().unwrap_or(false),
        axioms_clean: resp["axioms_clean"].as_bool().unwrap_or(false),
        messages: str_vec(&resp["messages"]),
        axioms: str_vec(&resp["axioms"]),
        goal_states: goal_states_from(resp),
    }
}

/// Build diagnostics from the REPL's structured messages and let the infotree
/// attach a goal state to each error position, returning the attached states.
///
/// This is the caller `error_feedback::Diagnostic::goal_state_slot` documents as
/// the intended filler: a warm REPL is the only place the infotree exists, so the
/// enrichment has to happen here rather than in that pure module.
///
/// Returns empty on every degrade path. A REPL that does not implement the field
/// omits `infotree` (reported by the worker as `null`), and there is nothing to
/// attach; that is a different fact from "the proof had no goals", so we simply
/// carry no states rather than inventing any.
fn goal_states_from(resp: &Value) -> Vec<String> {
    let tree = &resp["infotree"];
    if tree.is_null() {
        return Vec::new();
    }
    let mut diagnostics = error_diagnostics(&resp["messages"]);
    if diagnostics.is_empty() {
        return Vec::new();
    }
    // `attach_goal_states` takes the raw tree JSON so it can parse permissively;
    // hand it the serialized subtree we received.
    let tree_json = tree.to_string();
    crate::prover::infotree::attach_goal_states(
        &mut diagnostics,
        &tree_json,
        crate::prover::infotree::DEFAULT_GOAL_STATE_CAP,
    );
    diagnostics
        .into_iter()
        .filter_map(|d| d.goal_state_slot)
        .collect()
}

/// Convert the REPL's error-severity message objects into
/// [`error_feedback::Diagnostic`]s positioned in the submitted source.
///
/// Only errors are kept: a warning position is not where the proof is stuck. The
/// REPL serializes `Lean.Position`, whose column is 0-based while `Diagnostic`
/// columns are 1-based, so the column is shifted by one; the line is 1-based in
/// both and copied through. A message with no position cannot be matched against
/// a tree node, so it is dropped rather than attached to an arbitrary node.
fn error_diagnostics(messages: &Value) -> Vec<crate::prover::error_feedback::Diagnostic> {
    use crate::prover::error_feedback::{Diagnostic, Severity};
    use crate::prover::formal::FormalSystem;

    let Some(items) = messages.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter(|m| m["severity"].as_str() == Some("error"))
        .filter_map(|m| {
            let line = m["pos"]["line"].as_u64().map(|l| l as usize)?;
            let col0 = m["pos"]["column"].as_u64().map(|c| c as usize);
            Some(Diagnostic {
                system: FormalSystem::Lean,
                severity: Severity::Error,
                line: Some(line),
                end_line: m["endPos"]["line"].as_u64().map(|l| l as usize),
                col_start: col0.map(|c| c + 1),
                col_end: m["endPos"]["column"].as_u64().map(|c| c as usize + 1),
                message: m["data"].as_str().unwrap_or_default().to_string(),
                goal_state_slot: None,
            })
        })
        .collect()
}

impl LeanSession {
    /// Spawn the server and warm it once with `imports` (Mathlib resolves
    /// against `config.lean_project` when set). Returns `Err` if the process
    /// cannot start or warm, so the caller can fall back to cold checks.
    pub fn start(config: &Config, imports: &[String]) -> Result<Self> {
        let python = python_command().ok_or_else(|| anyhow!("no python interpreter found"))?;
        let package_root = PathBuf::from("python").canonicalize()?;
        let bootstrap = format!(
            "import sys;sys.path.insert(0,{:?});from theoremata_tools.lean_repl import serve;serve()",
            package_root.to_string_lossy()
        );
        let mut child = Command::new(python)
            .args(["-E", "-c", &bootstrap])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("spawning lean_repl serve")?;
        let stdin = child.stdin.take().context("child stdin")?;
        let stdout = BufReader::new(child.stdout.take().context("child stdout")?);
        let root = config.lean_project.clone().filter(|p| p.exists());
        let mut session = Self {
            child,
            stdin,
            stdout,
            imports: imports.to_vec(),
            root: root.clone(),
        };
        let warm = session.request(&json!({
            "op": "warm",
            "imports": imports,
            "root": root,
        }))?;
        if !warm["ok"].as_bool().unwrap_or(false) {
            return Err(anyhow!("lean session failed to warm: {warm}"));
        }
        Ok(session)
    }

    fn request(&mut self, req: &Value) -> Result<Value> {
        self.stdin.write_all(req.to_string().as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        let mut line = String::new();
        if self.stdout.read_line(&mut line)? == 0 {
            return Err(anyhow!("lean session closed unexpectedly"));
        }
        Ok(serde_json::from_str(&line)?)
    }

    /// Type-check `source` against the warm environment. When `theorem` is
    /// given, its `#print axioms` closure is checked against the allowlist and
    /// reported as `axioms_clean`.
    pub fn check(&mut self, source: &str, theorem: Option<&str>) -> Result<CheckOutcome> {
        let req = json!({
            "op": "check",
            "imports": self.imports.clone(),
            "root": self.root.clone(),
            "source": source,
            "theorem": theorem,
            // Ask for the elaboration tree so a failed check can carry the goal
            // state at the error. It is advisory output and never moves `ok`; a
            // REPL that does not implement it simply returns no tree.
            "infotree": true,
        });
        Ok(parse_outcome(&self.request(&req)?))
    }
}

impl Drop for LeanSession {
    fn drop(&mut self) {
        // Closing stdin sends EOF so the server exits; then reap/kill.
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_check_response() {
        let resp = json!({
            "ok": true, "axioms_clean": true,
            "messages": [], "axioms": ["propext", "Classical.choice"]
        });
        let outcome = parse_outcome(&resp);
        assert!(outcome.ok && outcome.axioms_clean);
        assert_eq!(outcome.axioms, vec!["propext", "Classical.choice"]);
        assert!(outcome.messages.is_empty());
    }

    #[test]
    fn defaults_on_missing_fields() {
        let outcome = parse_outcome(&json!({}));
        assert!(!outcome.ok && !outcome.axioms_clean);
        assert!(outcome.messages.is_empty() && outcome.axioms.is_empty());
        assert!(outcome.goal_states.is_empty());
    }

    /// The tree the REPL returns under `infotree: "original"`: a goal-bearing node
    /// whose source range encloses the error position.
    const TREE: &str = r#"[
      {"node": {"stx": {"range": {"start": {"line": 1, "column": 0},
                                  "finish": {"line": 10, "column": 0}}},
                "goalsBefore": ["n : Nat\nh : n > 0\n⊢ inner"]},
       "kids": []}
    ]"#;

    #[test]
    fn attaches_goal_state_to_an_error_inside_a_goal_node() {
        // Error at line 5, column 2 (0-based, as Lean.Position serializes it):
        // inside the [1,0]..[10,0] node, so its goal state is recovered.
        let resp = json!({
            "ok": false,
            "axioms_clean": false,
            "messages": [
                {"severity": "error", "pos": {"line": 5, "column": 2}, "data": "unsolved goals"}
            ],
            "axioms": [],
            "infotree": serde_json::from_str::<Value>(TREE).unwrap(),
        });
        let outcome = parse_outcome(&resp);
        assert_eq!(outcome.goal_states.len(), 1);
        assert!(outcome.goal_states[0].contains("inner"));
    }

    #[test]
    fn absent_tree_yields_no_goal_states_even_with_errors() {
        // An older REPL omits the field (worker reports null). An error with no
        // tree is still an error, but there is nothing to attach, and we must not
        // invent a state.
        let resp = json!({
            "ok": false,
            "axioms_clean": false,
            "messages": [
                {"severity": "error", "pos": {"line": 5, "column": 2}, "data": "unsolved goals"}
            ],
            "axioms": [],
            "infotree": Value::Null,
        });
        assert!(parse_outcome(&resp).goal_states.is_empty());
    }

    #[test]
    fn warnings_do_not_produce_goal_states() {
        // A warning is not where the proof is stuck, so it is not a candidate for
        // attachment.
        let resp = json!({
            "ok": true,
            "axioms_clean": true,
            "messages": [
                {"severity": "warning", "pos": {"line": 5, "column": 2}, "data": "unused variable"}
            ],
            "axioms": [],
            "infotree": serde_json::from_str::<Value>(TREE).unwrap(),
        });
        assert!(parse_outcome(&resp).goal_states.is_empty());
    }
}
