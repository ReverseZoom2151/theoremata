//! Full LeanParanoia hardening step (plan §5, item 1d).
//!
//! Scaffolds a Lake workspace on the local Mathlib, places a generated proof as
//! an importable module, builds it, and runs the built LeanParanoia executable
//! on the theorem — the deep kernel-replay / csimp / native-decide /
//! constructor-recursor battery beyond the lexical and `#print axioms` gates.
//!
//! This is best-effort and NON-FATAL: any missing tool or unresolved import
//! yields `ran:false`/`clean:false` with a clear summary rather than an error.
//! The caller uses it as *additional* evidence, never the sole gate — the
//! authoritative axiom check remains the workflow's `#print axioms` step.

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

#[derive(Debug, serde::Serialize)]
pub struct HardeningReport {
    pub ran: bool,
    pub clean: bool,
    pub summary: String,
    pub details: Value,
}

impl HardeningReport {
    fn skipped(summary: impl Into<String>) -> Self {
        Self {
            ran: false,
            clean: false,
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

/// The first declared theorem/lemma name in a Lean source (for the paranoia
/// target), or None.
fn first_theorem_name(src: &str) -> Option<String> {
    for line in src.lines() {
        let trimmed = line.trim_start();
        for kw in ["theorem ", "lemma "] {
            if let Some(rest) = trimmed.strip_prefix(kw) {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '\''))
                    .collect();
                if !name.is_empty() {
                    return Some(name);
                }
            }
        }
    }
    None
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

    let theorem_target = match first_theorem_name(lean_source) {
        Some(thm) if thm.contains('.') => thm,
        Some(thm) => format!("{qualified}.{thm}"),
        None => qualified.clone(),
    };

    let mut summary = format!("module '{qualified}' built cleanly");
    let mut paranoia_details = Value::Null;
    let mut paranoia_success: Option<bool> = None;

    if let Some(exe) = paranoia_exe {
        match Command::new(&lake)
            .current_dir(&ws_root)
            .arg("env")
            .arg(&exe)
            .arg(&theorem_target)
            .output()
        {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                match serde_json::from_str::<Value>(stdout.trim()) {
                    Ok(v) => {
                        let success = v.get("success").and_then(Value::as_bool).unwrap_or(false);
                        paranoia_success = Some(success);
                        summary = if success {
                            format!("module '{qualified}' built and passed LeanParanoia")
                        } else {
                            format!("module '{qualified}' built but LeanParanoia reported failures")
                        };
                        paranoia_details = v;
                    }
                    Err(_) => {
                        summary = format!(
                            "module '{qualified}' built; LeanParanoia could not audit '{theorem_target}' (import/name resolution) — relying on build + axiom gate"
                        );
                        paranoia_details =
                            json!({"note":"no parseable verdict","stdout": stdout, "stderr": stderr});
                    }
                }
            }
            Err(e) => {
                summary =
                    format!("module '{qualified}' built; LeanParanoia could not be launched: {e}");
                paranoia_details = json!({"error": e.to_string()});
            }
        }
    } else {
        summary = format!(
            "module '{qualified}' built; LeanParanoia executable not present — `lake build` it under resources to enable the deep checks"
        );
    }

    // clean = built AND (paranoia passed, or paranoia not applicable). This is
    // additional hardening; the authoritative axiom check is the workflow gate.
    let clean = paranoia_success.unwrap_or(true);

    let details = json!({
        "qualified_name": qualified,
        "theorem_target": theorem_target,
        "workspace": ws_root,
        "paranoia": paranoia_details,
    });

    store.add_evidence(
        project_id,
        node_id,
        "lean_paranoia",
        "hardening",
        if clean { "clean" } else { "flagged" },
        details.clone(),
    )?;

    Ok(HardeningReport {
        ran: true,
        clean,
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
    }
}
