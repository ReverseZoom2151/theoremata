//! Full LeanParanoia hardening step (plan §5, item 1d).
//!
//! Scaffolds a Lake workspace on the local Mathlib, places a generated proof as
//! an importable module, builds it, and runs the built LeanParanoia executable
//! on the theorem — the deep kernel-replay / csimp / native-decide /
//! constructor-recursor battery beyond the lexical and `#print axioms` gates.
//!
//! This layer now FAILS CLOSED: a proof is only reported `clean:true` when
//! LeanParanoia was actually run and emitted a parseable success verdict
//! (`HardeningOutcome::Passed`). Every other state — `Flagged` (checks failed),
//! `Inconclusive` (paranoia present but no parseable verdict / launch error),
//! `Unavailable` (executable absent), `BuildFailed`, and `Skipped`
//! (preconditions unmet) — is NOT clean. In particular `Inconclusive` and
//! `Unavailable` are never treated as clean: an un-audited proof no longer
//! passes silently.
//!
//! It remains best-effort as a *gate*: missing tooling or unresolved imports
//! yield a non-clean outcome with a clear summary rather than an error, and the
//! caller uses this as *additional* evidence. Only `Flagged` denotes a real
//! soundness failure; the authoritative axiom check remains the workflow's
//! `#print axioms` step.

use crate::{
    config::Config,
    db::Store,
    tools::{PythonCheck, Tool},
};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    process::{Command, Stdio},
};

/// The terminal state of a hardening run. Only `Passed` is `clean`; every other
/// variant is non-clean so an un-audited proof never passes silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HardeningOutcome {
    /// LeanParanoia ran and emitted a parseable success verdict.
    Passed,
    /// LeanParanoia ran and reported one or more failing checks.
    Flagged,
    /// LeanParanoia is present but produced no parseable verdict (import/name
    /// resolution failure) or could not be launched.
    Inconclusive,
    /// The LeanParanoia executable was not present.
    Unavailable,
    /// The generated module (scaffold/place/build) failed to produce an olean.
    BuildFailed,
    /// A precondition was unmet (no Mathlib project, no python worker, no lake).
    Skipped,
}

#[derive(Debug, serde::Serialize)]
pub struct HardeningReport {
    pub ran: bool,
    pub clean: bool,
    pub outcome: HardeningOutcome,
    pub summary: String,
    pub details: Value,
}

impl HardeningReport {
    fn skipped(summary: impl Into<String>) -> Self {
        Self {
            ran: false,
            clean: false,
            outcome: HardeningOutcome::Skipped,
            summary: summary.into(),
            details: Value::Null,
        }
    }
}

/// Resolve an executable to a spawnable command, falling back to a login shell
/// (which sources the user's profile) for the absolute native path — mirrors
/// the resolution in `tools.rs` without depending on its private helpers.
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

/// Parse a python worker `ToolResult` stdout `{"ok":bool,"output":...}` and
/// return the inner `output` when the call succeeded.
fn worker_output(result: &crate::model::ToolResult) -> Option<Value> {
    let v: Value = serde_json::from_str(&result.stdout).ok()?;
    if v.get("ok")?.as_bool()? {
        Some(v.get("output").cloned().unwrap_or(Value::Null))
    } else {
        None
    }
}

/// The paranoia target declaration in a Lean source, or None.
///
/// Robust to leading attributes (`@[...]`, possibly stacked) and stacked
/// declaration modifiers (`private`, `noncomputable`, …). Prefers the LAST
/// `theorem`/`lemma` (the main result is usually last), falling back to the LAST
/// `def`/`abbrev`/`instance` (exploit fixtures sometimes use `def exploit_...`).
fn target_declaration_name(src: &str) -> Option<String> {
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
        "mutual ",
    ];
    const DECLS: &[&str] = &["theorem ", "lemma ", "def ", "abbrev ", "instance "];

    let mut last_theorem: Option<String> = None;
    let mut last_def: Option<String> = None;

    for line in src.lines() {
        let mut rest = line.trim_start();

        // Strip leading attributes (`@[...]`), possibly stacked on one line.
        while let Some(after) = rest.strip_prefix("@[") {
            match after.find(']') {
                Some(close) => rest = after[close + 1..].trim_start(),
                None => break,
            }
        }

        // Strip stacked declaration modifiers.
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

        // Recognize a declaration keyword and collect its name.
        for kw in DECLS {
            if let Some(after) = rest.strip_prefix(kw) {
                let name: String = after
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '\''))
                    .collect();
                if !name.is_empty() {
                    if *kw == "theorem " || *kw == "lemma " {
                        last_theorem = Some(name);
                    } else {
                        last_def = Some(name);
                    }
                }
                break;
            }
        }
    }

    last_theorem.or(last_def)
}

/// Run the deep hardening battery on a generated proof.
///
/// `module_name` should be a Lean-identifier-safe name (typically derived from
/// the node id); `lean_source` is the full generated proof file (it is placed
/// as `Theoremata.<module_name>` in the workspace).
pub fn harden(
    store: &Store,
    config: &Config,
    project_id: &str,
    node_id: &str,
    module_name: &str,
    lean_source: &str,
) -> Result<HardeningReport> {
    // Preconditions: a Mathlib project, the python worker, and a Lean toolchain.
    let mathlib_root = match config.lean_project.clone().filter(|p| p.exists()) {
        Some(p) => p,
        None => {
            return Ok(HardeningReport::skipped(
                "no Mathlib lean_project configured; hardening skipped",
            ))
        }
    };
    let python = PythonCheck::new();
    if !python.available() {
        return Ok(HardeningReport::skipped(
            "python worker unavailable; hardening skipped",
        ));
    }
    let lake = match resolve("lake") {
        Some(l) => l,
        None => {
            return Ok(HardeningReport::skipped(
                "lake not found; hardening skipped",
            ))
        }
    };

    // Workspace under the state dir (e.g. `.theoremata/lean`).
    let ws_root = config
        .workspace
        .parent()
        .map(|p| p.join("lean"))
        .unwrap_or_else(|| PathBuf::from(".theoremata/lean"));

    // 1. Scaffold once (skip if a lakefile already exists).
    if !ws_root.join("lakefile.toml").exists() {
        let scaffold = python.run(json!({
            "tool": "lean_workspace_scaffold",
            "target_dir": ws_root,
            "mathlib_root": mathlib_root,
        }))?;
        let ok = worker_output(&scaffold)
            .and_then(|o| o.get("ok").and_then(Value::as_bool))
            .unwrap_or(false);
        if !ok {
            return Ok(HardeningReport {
                ran: false,
                clean: false,
                outcome: HardeningOutcome::BuildFailed,
                summary: "workspace scaffold failed".into(),
                details: json!({"stage":"scaffold","stdout": scaffold.stdout, "stderr": scaffold.stderr}),
            });
        }
    }

    // 2. Place the proof as an importable module.
    let place = python.run(json!({
        "tool": "lean_workspace_place",
        "workspace_dir": ws_root,
        "module_name": module_name,
        "source": lean_source,
    }))?;
    let placed = match worker_output(&place) {
        Some(o) if o.get("ok").and_then(Value::as_bool).unwrap_or(false) => o,
        _ => {
            return Ok(HardeningReport {
                ran: false,
                clean: false,
                outcome: HardeningOutcome::BuildFailed,
                summary: "placing the proof module failed".into(),
                details: json!({"stage":"place","stdout": place.stdout, "stderr": place.stderr}),
            })
        }
    };
    let qualified = placed
        .get("qualified_name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if qualified.is_empty() {
        return Ok(HardeningReport::skipped(
            "workspace place returned no qualified module name",
        ));
    }

    // 3. Build the module so its olean exists and can be imported. NOTE: the
    //    first build in a fresh workspace resolves Mathlib's transitive git deps
    //    (network); callers should invoke hardening judiciously (e.g. only at
    //    final certification), not on every attempt. Mathlib oleans are reused.
    let build = Command::new(&lake)
        .current_dir(&ws_root)
        .arg("build")
        .output()?;
    if !build.status.success() {
        return Ok(HardeningReport {
            ran: true,
            clean: false,
            outcome: HardeningOutcome::BuildFailed,
            summary: format!("module '{qualified}' failed to build"),
            details: json!({
                "stage": "build",
                "stdout": String::from_utf8_lossy(&build.stdout),
                "stderr": String::from_utf8_lossy(&build.stderr),
            }),
        });
    }

    // 4. Run the built LeanParanoia executable against the theorem, in OUR
    //    workspace env (so `import` of our module + Mathlib resolve). paranoia
    //    expects a fully-qualified theorem name; we pass `<module>.<theorem>`.
    //    If paranoia cannot resolve/import it (cross-project or naming), we say
    //    so honestly and fall back to "compiled cleanly" — the axiom gate is the
    //    authoritative soundness check elsewhere.
    let paranoia_exe = {
        let base = config
            .resources
            .join("LeanParanoia-main/LeanParanoia-main/.lake/build/bin");
        let exe = base.join("paranoia.exe");
        if exe.exists() {
            Some(exe)
        } else {
            let bare = base.join("paranoia");
            bare.exists().then_some(bare)
        }
    };

    let theorem_target = match target_declaration_name(lean_source) {
        Some(thm) if thm.contains('.') => thm,
        Some(thm) => format!("{qualified}.{thm}"),
        None => qualified.clone(),
    };

    let mut summary = format!("module '{qualified}' built cleanly");
    let mut paranoia_details = Value::Null;
    // The failing-check taxonomy (CheckName -> reasons) from the verdict.
    let mut failures = Value::Null;
    let outcome: HardeningOutcome;

    if let Some(exe) = paranoia_exe {
        // `--trust-modules Std,Mathlib,Init` MUST precede the theorem target:
        // without it the kernel Replay check re-verifies the entire transitive
        // Mathlib closure and times out, so the deepest check silently no-ops.
        match Command::new(&lake)
            .current_dir(&ws_root)
            .arg("env")
            .arg(&exe)
            .arg("--trust-modules")
            .arg("Std,Mathlib,Init")
            .arg(&theorem_target)
            .output()
        {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                match serde_json::from_str::<Value>(stdout.trim()) {
                    Ok(v) => {
                        let success = v.get("success").and_then(Value::as_bool).unwrap_or(false);
                        if let Some(f) = v.get("failures") {
                            failures = f.clone();
                        }
                        if success {
                            outcome = HardeningOutcome::Passed;
                            summary =
                                format!("module '{qualified}' built and passed LeanParanoia");
                        } else {
                            outcome = HardeningOutcome::Flagged;
                            let names: Vec<&str> = failures
                                .as_object()
                                .map(|m| m.keys().map(String::as_str).collect())
                                .unwrap_or_default();
                            summary = if names.is_empty() {
                                format!(
                                    "module '{qualified}' built but LeanParanoia reported failures"
                                )
                            } else {
                                format!("LeanParanoia flagged: {}", names.join(", "))
                            };
                        }
                        paranoia_details = v;
                    }
                    Err(_) => {
                        outcome = HardeningOutcome::Inconclusive;
                        summary = format!(
                            "module '{qualified}' built; LeanParanoia could not audit '{theorem_target}' (import/name resolution) — INCONCLUSIVE, not clean; relying on axiom gate"
                        );
                        paranoia_details =
                            json!({"note":"no parseable verdict","stdout": stdout, "stderr": stderr});
                    }
                }
            }
            Err(e) => {
                outcome = HardeningOutcome::Inconclusive;
                summary = format!(
                    "module '{qualified}' built; LeanParanoia could not be launched: {e} — INCONCLUSIVE, not clean"
                );
                paranoia_details = json!({"error": e.to_string()});
            }
        }
    } else {
        outcome = HardeningOutcome::Unavailable;
        summary = format!(
            "module '{qualified}' built; LeanParanoia executable not present — UNAVAILABLE, not clean; `lake build` it under resources to enable the deep checks"
        );
    }

    // Fail closed: only an actual parseable success verdict is clean.
    let clean = matches!(outcome, HardeningOutcome::Passed);

    let details = json!({
        "qualified_name": qualified,
        "theorem_target": theorem_target,
        "workspace": ws_root,
        "outcome": outcome,
        "failures": failures,
        "paranoia": paranoia_details,
    });

    let evidence_verdict = match outcome {
        HardeningOutcome::Passed => "passed",
        HardeningOutcome::Flagged => "flagged",
        HardeningOutcome::Inconclusive => "inconclusive",
        HardeningOutcome::Unavailable => "unavailable",
        HardeningOutcome::BuildFailed => "build_failed",
        HardeningOutcome::Skipped => "skipped",
    };
    store.add_evidence(
        project_id,
        node_id,
        "lean_paranoia",
        "hardening",
        evidence_verdict,
        details.clone(),
    )?;

    Ok(HardeningReport {
        ran: true,
        clean,
        outcome,
        summary,
        details,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NodeKind;
    use std::path::Path;

    #[test]
    fn degrades_gracefully_without_lean_project() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "t").unwrap();
        let node = store
            .add_node(&project.id, NodeKind::FormalStatement, "f", "s", "test")
            .unwrap();
        let mut config = Config::default();
        config.lean_project = None;
        let report = harden(
            &store,
            &config,
            &project.id,
            &node.id,
            "Demo",
            "theorem t : True := trivial",
        )
        .unwrap();
        assert!(!report.ran);
        assert!(!report.clean);
        assert_eq!(report.outcome, HardeningOutcome::Skipped);
    }

    #[test]
    fn target_prefers_last_theorem_over_helper_lemma() {
        let src = "lemma aux (n : Nat) : n = n := rfl\ntheorem main : True := trivial\n";
        assert_eq!(
            target_declaration_name(src).as_deref(),
            Some("main"),
            "the main result is the last theorem, not the helper lemma"
        );
    }

    #[test]
    fn target_falls_back_to_def_exploit() {
        let src = "def exploit_theorem : True := trivial\n";
        assert_eq!(
            target_declaration_name(src).as_deref(),
            Some("exploit_theorem")
        );
    }

    #[test]
    fn target_passes_through_qualified_name() {
        let src = "theorem Foo.bar : True := trivial\n";
        assert_eq!(target_declaration_name(src).as_deref(), Some("Foo.bar"));
    }

    #[test]
    fn target_strips_attributes_and_modifiers() {
        let src = "@[simp]\nprivate theorem t : True := trivial\n";
        assert_eq!(target_declaration_name(src).as_deref(), Some("t"));
    }
}
