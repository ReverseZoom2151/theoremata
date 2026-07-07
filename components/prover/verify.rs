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
    let py = PythonCheck::new();
    let lexical = if py.available() {
        let resp = py.run(json!({"tool": "lean_soundness", "text": after_src}))?;
        let parsed: serde_json::Value =
            serde_json::from_str(&resp.stdout).unwrap_or(json!({"ok": false}));
        parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false)
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
        detail: json!({
            "expected_statement": expected_statement,
            "hardening_enabled": config.harden_proofs,
            "statement_guard": statement_guard::guard_report_json(&guard),
        }),
    })
}