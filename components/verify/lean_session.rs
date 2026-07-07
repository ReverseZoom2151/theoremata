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
        .map(|a| a.iter().filter_map(|m| m.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

fn parse_outcome(resp: &Value) -> CheckOutcome {
    CheckOutcome {
        ok: resp["ok"].as_bool().unwrap_or(false),
        axioms_clean: resp["axioms_clean"].as_bool().unwrap_or(false),
        messages: str_vec(&resp["messages"]),
        axioms: str_vec(&resp["axioms"]),
    }
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
    }
}
