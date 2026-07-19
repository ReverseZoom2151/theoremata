//! Statement VALIDATION as a first-class pipeline stage (plan: promote
//! faithfulness checking ahead of proving).
//!
//! The field's consensus is that the *risky* step in an autoformalization loop is
//! not the proof search — a formal proof is machine-checkable ground truth — but
//! whether the formalized statement faithfully encodes the intended (informal)
//! problem. A proof of the *wrong* statement is worthless. The existing
//! [`crate::prover::statement_guard`] only catches statement DRIFT that happens
//! DURING proving (a prover returning a proof of a weaker/different header); it
//! says nothing about whether the INITIAL formalization was faithful in the first
//! place.
//!
//! This stage fills that gap. When a node first receives a formal statement, it
//! runs an ADVISORY faithfulness check that combines:
//!
//! * a **round-trip** check (`statement_roundtrip` worker) — does the formal
//!   statement, translated back to informal, still mean the original problem?
//! * a **triviality** check (`triviality` worker) — is the formal statement
//!   vacuous / trivially true (e.g. `: True`), so that a proof of it proves
//!   nothing about the intended claim?
//!
//! It is strictly ADVISORY: the formal gate (`#print axioms` +
//! k-consecutive-clean + the pool/meta gate) remains the sole ground truth for
//! certification. This stage NEVER hard-rejects or silently drops a node — a
//! `Reject` verdict only warns/annotates (and can be used to *request* human
//! review); proving always proceeds. The whole stage is gated behind
//! [`validation_enabled`] (`THEOREMATA_VALIDATE_STATEMENTS`); OFF (the default)
//! reproduces the prior behaviour exactly.
//!
//! No wall-clock / randomness: the outcome is a pure function of the two worker
//! outputs (or, when the workers are unavailable, a deterministic neutral
//! outcome).

use crate::{
    config::Config,
    tools::{PythonCheck, Tool},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Advisory verdict of the faithfulness stage. This is NOT a certification
/// verdict — it never blocks proving. `Reject` is the strongest advisory signal
/// (warn / optionally request human review), not a hard failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// The formalization looks faithful and non-trivial — proceed normally.
    Ok,
    /// Something is off (borderline faithfulness) — proceed, but flag it.
    Suspect,
    /// Strong advisory signal that the statement is unfaithful or vacuous —
    /// proceed (advisory!), but warn loudly / request review.
    Reject,
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verdict::Ok => "ok",
            Verdict::Suspect => "suspect",
            Verdict::Reject => "reject",
        }
    }

    /// A `Suspect` or `Reject` verdict warrants surfacing a warning; `Ok` is
    /// silent.
    pub fn is_warning(&self) -> bool {
        !matches!(self, Verdict::Ok)
    }
}

/// The advisory outcome of a statement-faithfulness check. Carries the combined
/// faithfulness score, the triviality flag, the rolled-up [`Verdict`], and any
/// human-readable findings from the workers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationOutcome {
    /// Round-trip faithfulness in `[0, 1]` (1.0 = fully faithful). When the
    /// round-trip worker is unavailable this defaults to `1.0` so the advisory
    /// stage never fabricates a problem it did not observe.
    pub faithful_score: f64,
    /// Whether the triviality worker judged the formal statement vacuous /
    /// trivially true (a proof of it proves nothing about the intended claim).
    pub trivial: bool,
    /// The rolled-up advisory verdict.
    pub verdict: Verdict,
    /// Human-readable notes from the workers (already treated as untrusted data —
    /// see [`crate::guard::wrap_untrusted`] at the point they enter model
    /// prompts; here they are only stored/surfaced, never executed).
    pub findings: Vec<String>,
}

impl ValidationOutcome {
    /// Serialize for storage as node evidence / event payload.
    pub fn to_json(&self) -> Value {
        json!({
            "faithful_score": self.faithful_score,
            "trivial": self.trivial,
            "verdict": self.verdict.as_str(),
            "findings": self.findings,
        })
    }

    /// A neutral, non-blocking outcome used when the workers are unavailable or
    /// error out: faithful, non-trivial, `Ok`. Advisory-safe — it never invents a
    /// problem, so proving proceeds exactly as it would with the stage off.
    pub fn neutral(reason: impl Into<String>) -> Self {
        Self {
            faithful_score: 1.0,
            trivial: false,
            verdict: Verdict::Ok,
            findings: vec![reason.into()],
        }
    }
}

/// Faithfulness score at or above this is considered clearly faithful.
const FAITHFUL_OK: f64 = 0.75;
/// Faithfulness score strictly below this is considered clearly unfaithful.
const FAITHFUL_REJECT: f64 = 0.4;

/// Combine the two advisory sub-signals into a single [`ValidationOutcome`].
/// Pure and deterministic. A trivial statement is always at least `Suspect` and,
/// if also unfaithful, `Reject` — a vacuous formalization cannot faithfully
/// encode a non-trivial problem.
pub fn combine(faithful_score: f64, trivial: bool, findings: Vec<String>) -> ValidationOutcome {
    let score = faithful_score.clamp(0.0, 1.0);
    let verdict = if score < FAITHFUL_REJECT {
        // Clearly unfaithful — the strongest advisory signal.
        Verdict::Reject
    } else if trivial {
        // A vacuous statement never faithfully encodes a real problem, but a
        // borderline-faithful score alone is not enough to Reject.
        if score < FAITHFUL_OK {
            Verdict::Reject
        } else {
            Verdict::Suspect
        }
    } else if score < FAITHFUL_OK {
        Verdict::Suspect
    } else {
        Verdict::Ok
    };
    ValidationOutcome {
        faithful_score: score,
        trivial,
        verdict,
        findings,
    }
}

/// Injectable faithfulness checker. Kept behind a trait so the production impl
/// ([`ToolStatementValidator`], which shells out to the Python workers) can be
/// swapped for a deterministic mock in tests. Advisory: `validate` is infallible
/// (it degrades to a neutral outcome rather than erroring) because it must never
/// be able to block the pipeline.
pub trait StatementValidator {
    /// Judge whether `formal` faithfully encodes `informal`. Never blocks: on any
    /// internal failure it returns [`ValidationOutcome::neutral`].
    fn validate(&self, informal: &str, formal: &str) -> ValidationOutcome;
}

/// Production [`StatementValidator`]: calls the `statement_roundtrip` and
/// `triviality` Python worker tools through the existing tool bridge
/// ([`PythonCheck`]) and folds their advisory outputs together via [`combine`].
///
/// Mirrors how other tools are invoked from the orchestration layer (e.g. the
/// `retrieve` / `check_axioms` calls in `agent.rs`): build a `{"tool": …}`
/// request, run it through [`PythonCheck::run`], and parse the worker's
/// `{"ok":…, "output":…}` envelope. All worker text is untrusted data.
pub struct ToolStatementValidator<'a> {
    #[allow(dead_code)]
    config: &'a Config,
}

impl<'a> ToolStatementValidator<'a> {
    pub fn new(config: &'a Config) -> Self {
        Self { config }
    }

    /// Run one worker tool and return its parsed `output` object, or `None` when
    /// the worker is unavailable / errored / produced non-JSON. Never panics.
    fn run_tool(py: &PythonCheck, request: Value) -> Option<Value> {
        let result = py.run(request).ok()?;
        if !result.success {
            return None;
        }
        let value: Value = serde_json::from_str(&result.stdout).ok()?;
        // Honour the worker envelope: {"ok": bool, "output": …}.
        if value.get("ok").and_then(Value::as_bool) == Some(false) {
            return None;
        }
        Some(value.get("output").cloned().unwrap_or(value))
    }

    /// Pull a `[0,1]` faithfulness score out of a round-trip worker's output,
    /// tolerating a few field spellings and a boolean `faithful`/`equivalent`
    /// fallback. Defaults to `1.0` (neutral) when nothing is present.
    fn parse_score(output: &Value) -> f64 {
        for key in ["faithful_score", "score", "faithfulness", "similarity"] {
            if let Some(v) = output.get(key).and_then(Value::as_f64) {
                return v.clamp(0.0, 1.0);
            }
        }
        for key in ["faithful", "equivalent", "preserved"] {
            if let Some(b) = output.get(key).and_then(Value::as_bool) {
                return if b { 1.0 } else { 0.0 };
            }
        }
        1.0
    }

    /// Collect human-readable findings out of a worker output under any of a few
    /// common field names.
    fn parse_findings(output: &Value, prefix: &str) -> Vec<String> {
        let mut out = Vec::new();
        for key in ["findings", "issues", "reasons", "notes"] {
            if let Some(arr) = output.get(key).and_then(Value::as_array) {
                for item in arr {
                    let text = item
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| item.to_string());
                    out.push(format!("{prefix}: {text}"));
                }
            }
        }
        out
    }
}

impl StatementValidator for ToolStatementValidator<'_> {
    fn validate(&self, informal: &str, formal: &str) -> ValidationOutcome {
        let py = PythonCheck::new();
        if !py.available() {
            return ValidationOutcome::neutral(
                "statement validation unavailable: no python worker",
            );
        }

        let mut findings = Vec::new();

        // Round-trip faithfulness.
        let faithful_score = match Self::run_tool(
            &py,
            json!({ "tool": "statement_roundtrip", "informal": informal, "formal": formal }),
        ) {
            Some(output) => {
                findings.extend(Self::parse_findings(&output, "roundtrip"));
                Self::parse_score(&output)
            }
            None => {
                findings.push("roundtrip: worker unavailable (assumed faithful)".into());
                1.0
            }
        };

        // Triviality.
        let trivial = match Self::run_tool(
            &py,
            json!({ "tool": "triviality", "informal": informal, "formal": formal }),
        ) {
            Some(output) => {
                findings.extend(Self::parse_findings(&output, "triviality"));
                output
                    .get("trivial")
                    .and_then(Value::as_bool)
                    .or_else(|| output.get("is_trivial").and_then(Value::as_bool))
                    .unwrap_or(false)
            }
            None => {
                findings.push("triviality: worker unavailable (assumed non-trivial)".into());
                false
            }
        };

        combine(faithful_score, trivial, findings)
    }
}

/// Whether the statement-validation stage is enabled. Absent / empty / an
/// explicit `0`/`false`/`off` means OFF — the pipeline keeps its exact prior
/// behaviour (no validator call, no evidence, no event). Any other value turns
/// the advisory stage ON. Read once per stage invocation; deterministic.
pub fn validation_enabled() -> bool {
    match std::env::var("THEOREMATA_VALIDATE_STATEMENTS") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "off"
        ),
        Err(_) => false,
    }
}

/// Shared lock serializing every test that mutates the process-global
/// `THEOREMATA_VALIDATE_STATEMENTS` env var (this module's tests AND the agent
/// stage tests, which live in a sibling module). Test-only.
#[cfg(test)]
pub(crate) fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combine_rolls_up_verdicts() {
        // Faithful + non-trivial ⇒ Ok.
        assert_eq!(combine(0.9, false, vec![]).verdict, Verdict::Ok);
        // Borderline faithfulness ⇒ Suspect.
        assert_eq!(combine(0.6, false, vec![]).verdict, Verdict::Suspect);
        // Clearly unfaithful ⇒ Reject.
        assert_eq!(combine(0.2, false, vec![]).verdict, Verdict::Reject);
        // Trivial but otherwise faithful ⇒ at least Suspect (a vacuous statement
        // never faithfully encodes a non-trivial problem).
        assert_eq!(combine(0.95, true, vec![]).verdict, Verdict::Suspect);
        // Trivial AND borderline ⇒ Reject.
        assert_eq!(combine(0.6, true, vec![]).verdict, Verdict::Reject);
    }

    #[test]
    fn neutral_is_non_blocking() {
        let n = ValidationOutcome::neutral("x");
        assert_eq!(n.verdict, Verdict::Ok);
        assert!(!n.trivial);
        assert_eq!(n.faithful_score, 1.0);
    }

    #[test]
    fn flag_defaults_off_and_parses() {
        let _guard = super::env_lock();
        std::env::remove_var("THEOREMATA_VALIDATE_STATEMENTS");
        assert!(!validation_enabled());
        std::env::set_var("THEOREMATA_VALIDATE_STATEMENTS", "0");
        assert!(!validation_enabled());
        std::env::set_var("THEOREMATA_VALIDATE_STATEMENTS", "1");
        assert!(validation_enabled());
        std::env::set_var("THEOREMATA_VALIDATE_STATEMENTS", "off");
        assert!(!validation_enabled());
        std::env::remove_var("THEOREMATA_VALIDATE_STATEMENTS");
    }

    #[test]
    fn outcome_serializes_verdict_as_lowercase() {
        let v = combine(0.9, false, vec!["roundtrip: ok".into()]).to_json();
        assert_eq!(v["verdict"], "ok");
        assert_eq!(v["trivial"], false);
    }
}
