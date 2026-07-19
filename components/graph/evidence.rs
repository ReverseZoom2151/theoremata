//! Canonical evidence-type strings for `Store::add_evidence`.

#![allow(dead_code)]

pub const LEAN_COMPILE: &str = "lean_compile";
pub const AXIOM_AUDIT: &str = "axiom_audit";
pub const K_CONSECUTIVE_CLEAN: &str = "k_consecutive_clean";
pub const HARDENING: &str = "hardening";
pub const FALSIFICATION: &str = "falsification";
pub const RETRIEVAL: &str = "retrieval";
pub const EXTERNAL_PROVER_ARTIFACT: &str = "external_prover_artifact";
pub const EXTERNAL_PRODUCER_CHECKED: &str = "external_producer_checked";
pub const REFORMULATION_CHECK: &str = "reformulation_check";
pub const REPAIR_LOOP: &str = "repair_loop";

/// Build a provenance payload for externally generated Lean (Putnam/Aristotle pattern).
pub fn external_prover_payload(
    service: &str,
    request_id: Option<&str>,
    input_hash: Option<&str>,
    output_hash: Option<&str>,
    duration_ms: Option<u128>,
    cost: Option<f64>,
    extra: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "service": service,
        "request_id": request_id,
        "input_hash": input_hash,
        "output_hash": output_hash,
        "duration_ms": duration_ms,
        "cost_usd": cost,
        "extra": extra,
    })
}
