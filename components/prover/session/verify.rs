//! Local verification gate for external-prover output (trust-but-verify).

use crate::{
    config::Config,
    prover::{model::VerificationReport, statement_guard},
    tools::{PythonCheck, Tool},
};
use anyhow::Result;
use serde_json::json;

pub fn verify_lean_output(
    config: &Config,
    lean_code: &str,
    expected_statement: &str,
) -> Result<VerificationReport> {
    verify_lean_round_trip(config, expected_statement, lean_code, expected_statement)
}

pub fn verify_lean_round_trip(
    config: &Config,
    before_src: &str,
    after_src: &str,
    expected_statement: &str,
) -> Result<VerificationReport> {
    let guard = statement_guard::guard_lean_round_trip(before_src, after_src);
    // Statement-guard RESTORE (open-atp / Numina): when a header drifted or was
    // deleted, compute the restored-to-snapshot source rather than only
    // rejecting, so a caller can recover the original statement.
    let restore = if guard.preserved {
        None
    } else {
        Some(statement_guard::restore_statements(before_src, after_src))
    };
    let py = PythonCheck::new();
    let lexical = if py.available() {
        let resp = py.run(json!({"tool": "lean_soundness", "text": after_src}))?;
        let parsed: serde_json::Value =
            serde_json::from_str(&resp.stdout).unwrap_or(json!({"ok": false}));
        // The worker envelope is `{"ok": bool, "output": <payload>}`. Reading
        // top-level `ok` alone (as this did) only asked "did the worker run",
        // never "is the source clean" -- so the pre-screen passed on any
        // successful invocation regardless of what the scan found. Require BOTH:
        // the call succeeded, and the payload's verdict is clean.
        // `lean_soundness` returns `pregate_clean`, with `clean` as a deprecated
        // alias. Absent or unparseable still means false (fail closed).
        let call_ok = parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        let scan_clean = parsed
            .get("output")
            .and_then(|o| o.get("pregate_clean").or_else(|| o.get("clean")))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        call_ok && scan_clean
    } else {
        false
    };

    let low = after_src.to_lowercase();
    let axioms_clean = !low.contains("sorry") && !low.contains("admit");
    let exp_norm: String = expected_statement.split_whitespace().collect();
    let code_norm: String = after_src.split_whitespace().collect();
    let statement_preserved = guard.preserved
        && !exp_norm.is_empty()
        && (code_norm.contains(&exp_norm)
            || expected_statement
                .split(':')
                .next()
                .map(|s| code_norm.contains(&s.split_whitespace().collect::<String>()))
                .unwrap_or(false));

    // Lexical pre-screen only — the real compile happens at the certify step.
    let lexically_verified = lexical && axioms_clean;
    let hardening_clean = if config.harden_proofs {
        // Deep hardening requires a graph node + Lake workspace; external-prover
        // verification records the intent and leaves hardening to the certify step.
        Some(false)
    } else {
        None
    };

    Ok(VerificationReport {
        lexically_verified,
        axioms_clean,
        statement_preserved,
        lexical_clean: lexical,
        hardening_clean,
        // A lexical pre-screen of external-prover output — NOT a live compile.
        // The authoritative certify step does the real toolchain check, so this
        // report must never itself count as a live formal certification.
        live: false,
        detail: json!({
            "expected_statement": expected_statement,
            "hardening_enabled": config.harden_proofs,
            "statement_guard": statement_guard::guard_report_json(&guard),
            "statement_restore": restore,
        }),
    })
}
