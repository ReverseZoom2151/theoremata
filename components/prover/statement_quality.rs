//! Statement-quality ACCUSERS: a fifth, advisory layer that consults the two
//! Python detectors shipped under `components/verify/python/theoremata_tools/`
//! and reports what they found. It never blesses a statement.
//!
//! WHAT THIS LAYER IS FOR
//! ======================
//! The 3+1-layer gate in [`crate::prover::formal`] asks, in order, "did it
//! compile", "is its axiom closure inside the whitelist", "does an independent
//! kernel accept it", "is the source free of escape hatches", and "is the
//! submitted declaration the statement we asked for". Every one of those
//! questions is about the DERIVATION. None of them is about whether the
//! statement says anything.
//!
//! Two detectors ask that second question, each in a narrow way:
//!
//! * `statement_triviality` mutates the definitions a statement names and
//!   re-runs the UNCHANGED proof. If the proof still closes, the statement did
//!   not constrain those definitions.
//! * `opaque_statement` attributes a `sorryAx` to the individual constants of
//!   the statement's TYPE. This is not a coverage gap in the layer-2 audit,
//!   which already reports `sorryAx` for these; it is ATTRIBUTION, separating a
//!   contentless statement carrying a complete proof from an honest unfinished
//!   proof of a real statement.
//!
//! THE ONE RULE THAT DECIDES THIS MODULE'S SHAPE
//! =============================================
//! **Both detectors can only ACCUSE.** There are three worker verdicts each, and
//! exactly ONE of the three is a signal:
//!
//! | worker verdict                | meaning here                |
//! |-------------------------------|-----------------------------|
//! | `trivial`                     | [`Signal::Accused`]         |
//! | `opaque_constant_found`       | [`Signal::Accused`]         |
//! | `not_shown_trivial`           | [`Signal::NoAccusation`]    |
//! | `no_opaque_constant_found`    | [`Signal::NoAccusation`]    |
//! | `withheld`                    | [`Signal::Silent`]          |
//! | `unknown`                     | [`Signal::Silent`]          |
//! | anything else, or no reply    | [`Signal::Silent`]          |
//!
//! [`Signal::NoAccusation`] and [`Signal::Silent`] are DIFFERENT facts (the
//! check ran versus the check could not run) but they are the SAME instruction
//! to a caller: do nothing. Surviving a check is not evidence that a statement
//! is meaningful; it is the absence of evidence that it is empty. That is why
//! this type has exactly one predicate, [`Signal::accuses`], and deliberately no
//! `is_clean` / `is_ok` / `passed` / `Into<bool>`: there is no way to spell the
//! backwards reading.
//!
//! SILENCE IS THE DEFAULT ON EVERY FAILURE PATH
//! ============================================
//! An absent Python interpreter, an absent worker tree, a non-zero worker exit,
//! a timeout, a missing Lean toolchain, a non-JSON reply, a JSON reply with a
//! verdict string we do not recognise: every one of these produces
//! [`Signal::Silent`], which by construction cannot fail a verification and
//! cannot pass one. There is no `unwrap`, no index, and no `expect` in this
//! module; every fallible step is an `Option` short-circuit into
//! [`DetectorOutcome::silent`]. A false accusation against honest third-party
//! mathematics is worse than a miss, so the failure direction is fixed.
//!
//! ADVISORY BY DEFAULT
//! ===================
//! [`StatementQualityGates`] carries two INVOCATION switches (these detectors
//! spawn Lean compiles, so no verification pays for them unless asked) and one
//! ENFORCEMENT switch. All three default OFF, read from the environment in the
//! crate's default-off env-seam idiom (cf. [`crate::prover::formal::TierZeroGates::from_env`]).
//! With enforcement off, an accusation is published to the report's `detail` and
//! changes no verdict. With enforcement on, the ONLY thing that can flip a
//! verdict is [`Signal::Accused`] on a detector that was actually consulted; see
//! [`StatementQualityReport::blocks`].

use crate::{
    config::Config,
    prover::formal::{env_gate_on, FormalSystem, Workspace},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Worker tool key for the mutation-based triviality detector.
pub const TOOL_TRIVIALITY: &str = "statement_triviality";
/// Worker tool key for the `sorryAx`-attribution detector.
pub const TOOL_OPAQUE: &str = "opaque_statement";

/// The only two verdict strings that are a signal. Kept as constants next to
/// each other so a reader can see at a glance how short this list is.
const VERDICT_TRIVIAL: &str = "trivial";
const VERDICT_OPAQUE_FOUND: &str = "opaque_constant_found";

/// Verdict strings that mean "the check ran and did not accuse". Recognising
/// them buys nothing operationally (they are handled exactly like an unknown
/// string) but it lets the emitted evidence distinguish "ran, found nothing"
/// from "could not run", which is what a human triaging a report needs.
const VERDICT_NOT_SHOWN_TRIVIAL: &str = "not_shown_trivial";
const VERDICT_NO_OPAQUE: &str = "no_opaque_constant_found";

/// Seconds handed to the worker as its own Lean timeout. The Python side turns a
/// timeout into `withheld` / `unknown`, i.e. silence, which is why this being
/// too small can only lose signal and can never manufacture one.
const DEFAULT_TIMEOUT_SECS: f64 = 300.0;

// --- the signal type ------------------------------------------------------

/// What a detector told us. See the module docs for why there is no fourth
/// variant meaning "this statement is good".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Signal {
    /// The detector positively accused this statement. The ONLY actionable
    /// variant.
    Accused,
    /// The detector ran to completion and produced no accusation. This is NOT a
    /// certificate of meaning and must never be read as approval.
    NoAccusation,
    /// No signal at all: withheld, unknown, unavailable, timed out, or
    /// unparseable. Neither approval nor suspicion. Silence.
    Silent,
}

impl Signal {
    /// The single predicate this type exposes. True only for
    /// [`Signal::Accused`].
    ///
    /// There is intentionally no complementary `is_clean`: `!accuses()` is true
    /// for both [`Signal::NoAccusation`] and [`Signal::Silent`], and neither one
    /// licenses any positive conclusion. If you find yourself wanting the
    /// negation, you are about to read absence of evidence as evidence.
    pub fn accuses(self) -> bool {
        matches!(self, Signal::Accused)
    }

    /// Map ONE worker verdict string to a signal, for the named tool.
    ///
    /// The tool key is part of the match on purpose: `trivial` is a signal only
    /// from the triviality detector and `opaque_constant_found` only from the
    /// opaque detector, so a reply that arrived from the wrong dispatch arm (a
    /// mis-routed request, a renamed tool) cannot accuse.
    fn from_verdict(tool: &str, verdict: &str) -> Signal {
        match (tool, verdict) {
            (TOOL_TRIVIALITY, VERDICT_TRIVIAL) => Signal::Accused,
            (TOOL_OPAQUE, VERDICT_OPAQUE_FOUND) => Signal::Accused,
            (TOOL_TRIVIALITY, VERDICT_NOT_SHOWN_TRIVIAL) => Signal::NoAccusation,
            (TOOL_OPAQUE, VERDICT_NO_OPAQUE) => Signal::NoAccusation,
            // `withheld`, `unknown`, and every string we do not know: silence.
            _ => Signal::Silent,
        }
    }
}

// --- switches -------------------------------------------------------------

/// Which detectors to CONSULT, and whether an accusation ENFORCES.
///
/// All three fields default to `false`. The two invocation switches are a cost
/// control: each detector spawns one or more Lean elaborations, so leaving them
/// on unconditionally would make every verification in the system pay for a
/// compile it usually does not need. The enforcement switch is a correctness
/// posture; see [`StatementQualityReport::blocks`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StatementQualityGates {
    /// Consult `statement_triviality`.
    pub triviality: bool,
    /// Consult `opaque_statement`.
    pub opaque: bool,
    /// Conjoin an ACCUSATION into `lexically_verified`. Has no effect at all
    /// unless a detector was consulted and returned [`Signal::Accused`].
    pub enforce: bool,
}

impl StatementQualityGates {
    /// Nothing consulted, nothing enforced: the behavior-preserving default.
    pub const OFF: Self = Self {
        triviality: false,
        opaque: false,
        enforce: false,
    };

    /// Both detectors consulted, advisory only. This is the recommended way to
    /// turn the layer on: it buys the evidence without any risk of a false
    /// accusation failing an honest proof.
    pub const ADVISORY: Self = Self {
        triviality: true,
        opaque: true,
        enforce: false,
    };

    /// Both detectors consulted AND enforcing.
    pub const ENFORCING: Self = Self {
        triviality: true,
        opaque: true,
        enforce: true,
    };

    /// Read the switches from the environment, in the crate's default-off
    /// env-seam idiom (absent / empty / `0` / `false` / `off` means OFF):
    ///
    /// * `THEOREMATA_STATEMENT_TRIVIALITY_GATE` — consult the triviality detector.
    /// * `THEOREMATA_OPAQUE_STATEMENT_GATE` — consult the opaque-constant detector.
    /// * `THEOREMATA_STATEMENT_QUALITY_ENFORCE` — let an accusation fail a verification.
    ///
    /// Deterministic per call: no clock, no RNG, no filesystem.
    pub fn from_env() -> Self {
        Self {
            triviality: env_gate_on("THEOREMATA_STATEMENT_TRIVIALITY_GATE"),
            opaque: env_gate_on("THEOREMATA_OPAQUE_STATEMENT_GATE"),
            enforce: env_gate_on("THEOREMATA_STATEMENT_QUALITY_ENFORCE"),
        }
    }

    /// Read the switches from [`Config`] when the fields exist there, else from
    /// the environment.
    ///
    /// `Config` lives in `app/config.rs`, which this module does not own, so
    /// until the three fields land there this delegates to
    /// [`StatementQualityGates::from_env`]. The signature is the one the
    /// Tier-0 gates use, so the eventual swap is a body-only change.
    pub fn from_config(_cfg: &Config) -> Self {
        Self::from_env()
    }

    /// Whether either detector would be consulted. Callers use this to skip the
    /// whole layer, including the interpreter probe, when nothing is switched on.
    pub fn any(self) -> bool {
        self.triviality || self.opaque
    }
}

// --- evidence -------------------------------------------------------------

/// One detector's outcome, in the shape that is published to a verification
/// report's `detail`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectorOutcome {
    /// The worker tool key (`statement_triviality` / `opaque_statement`).
    pub tool: String,
    /// Whether the detector was switched on AND actually invoked.
    pub consulted: bool,
    pub signal: Signal,
    /// The raw verdict string the worker returned, verbatim and untrusted, or
    /// `None` when there was no reply to read one out of.
    pub verdict: Option<String>,
    /// Why this outcome, in one line, for a human reading a failed report.
    pub note: String,
    /// The worker's own payload, or a description of the failure path. Never
    /// interpreted; carried so the accusation can be checked by hand.
    #[serde(default)]
    pub detail: Value,
}

impl DetectorOutcome {
    /// Silence, with a reason. Every failure path in this module ends here.
    fn silent(tool: &str, consulted: bool, note: impl Into<String>, detail: Value) -> Self {
        Self {
            tool: tool.to_string(),
            consulted,
            signal: Signal::Silent,
            verdict: None,
            note: note.into(),
            detail,
        }
    }

    /// Not consulted at all, because the switch was off.
    fn switched_off(tool: &str) -> Self {
        Self::silent(
            tool,
            false,
            "detector not consulted: switch is off (default)",
            Value::Null,
        )
    }

    /// Convenience for callers: did THIS detector accuse?
    pub fn accuses(&self) -> bool {
        self.signal.accuses()
    }
}

/// Both detectors' outcomes plus the switches that produced them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatementQualityReport {
    pub gates: StatementQualityGates,
    pub triviality: DetectorOutcome,
    pub opaque: DetectorOutcome,
}

impl StatementQualityReport {
    /// Nothing consulted. Used for a system with no detector, and as the value
    /// when both switches are off.
    pub fn not_consulted(gates: StatementQualityGates) -> Self {
        Self {
            gates,
            triviality: DetectorOutcome::switched_off(TOOL_TRIVIALITY),
            opaque: DetectorOutcome::switched_off(TOOL_OPAQUE),
        }
    }

    /// Whether ANY detector accused.
    pub fn accuses(&self) -> bool {
        self.triviality.accuses() || self.opaque.accuses()
    }

    /// Whether this report should fail a verification.
    ///
    /// This is the whole enforcement surface, and it is one line on purpose:
    /// blocking requires the enforcement switch AND a positive accusation.
    /// [`Signal::Silent`] and [`Signal::NoAccusation`] are both false here, so
    /// an unavailable worker, a timeout, a missing Lean toolchain or a malformed
    /// reply leaves the verdict EXACTLY as it would have been without this
    /// layer, whatever the switch says.
    pub fn blocks(&self) -> bool {
        self.gates.enforce && self.accuses()
    }

    /// The names of the detectors that accused, for a one-line failure message.
    pub fn accusers(&self) -> Vec<String> {
        [&self.triviality, &self.opaque]
            .iter()
            .filter(|o| o.accuses())
            .map(|o| o.tool.clone())
            .collect()
    }
}

// --- invocation -----------------------------------------------------------

/// Run one worker tool and return the parsed `output` object, or `None`.
///
/// Follows the precedent already in this component,
/// `crate::prover::formal::worker_source_scan`: build a `{"tool": …}` request,
/// push it through [`crate::tools::PythonCheck`] (which shells the interpreter
/// with the `components/*/python` bootstrap and writes the request on stdin),
/// then honour the worker's `{"ok": …, "output": …}` envelope. All worker text
/// is untrusted data and is only ever compared, never executed.
fn run_worker(request: Value) -> Option<Value> {
    use crate::tools::{PythonCheck, Tool};
    let py = PythonCheck::new();
    if !py.available() {
        return None;
    }
    let result = py.run(request).ok()?;
    if !result.success {
        return None;
    }
    let value: Value = serde_json::from_str(&result.stdout).ok()?;
    if value.get("ok").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    // Some worker arms answer bare, others inside the envelope. Prefer the
    // envelope and fall back to the whole object, exactly as
    // `ToolStatementValidator::run_tool` does.
    Some(value.get("output").cloned().unwrap_or(value))
}

/// Turn a worker payload into an outcome. Anything unexpected is silence.
fn outcome_from_payload(tool: &str, payload: Value) -> DetectorOutcome {
    let verdict = match payload.get("verdict").and_then(Value::as_str) {
        Some(v) => v.to_string(),
        None => {
            return DetectorOutcome::silent(
                tool,
                true,
                "worker reply carried no verdict string",
                payload,
            )
        }
    };
    let signal = Signal::from_verdict(tool, &verdict);
    let note = match signal {
        Signal::Accused => format!(
            "{tool} ACCUSES this statement (verdict {verdict:?}); this is evidence about the \
             STATEMENT, not about the proof's soundness"
        ),
        Signal::NoAccusation => format!(
            "{tool} ran and did not accuse (verdict {verdict:?}); this is NOT a certificate \
             that the statement is meaningful"
        ),
        Signal::Silent => format!(
            "{tool} withheld (verdict {verdict:?}); no signal, neither approval nor suspicion"
        ),
    };
    DetectorOutcome {
        tool: tool.to_string(),
        consulted: true,
        signal,
        verdict: Some(verdict),
        note,
        detail: payload,
    }
}

/// The Lake workspace to elaborate inside, when one is configured and present.
/// `None` makes both detectors fall back to a bare `lean` invocation, which is
/// correct for Mathlib-free sources and simply withholds for the rest.
fn lake_workspace(cfg: &Config) -> Option<String> {
    cfg.lean_project
        .clone()
        .filter(|p| p.exists())
        .map(|p| p.to_string_lossy().into_owned())
}

/// Consult both detectors for one verified artifact.
///
/// `code` is the submitted source, `ws` the scaffolded workspace whose
/// `source_path` the compile already used, and `short_name` the theorem's
/// declaration name AS WRITTEN IN THE SOURCE.
///
/// The two detectors want different spellings of the name and that is not an
/// accident: `statement_triviality` slices the source text and matches a
/// top-level `theorem NAME`, so it needs the short name; `opaque_statement`
/// looks the constant up in the elaborated environment, so it needs the
/// fully-qualified one ([`Workspace::entry`]). Handing either the other's
/// spelling costs only silence (a "found 0 theorems" withhold, or a
/// `THEOREMATA_OPAQUE_MISSING` unknown), never a false accusation.
pub fn consult(
    cfg: &Config,
    gates: StatementQualityGates,
    system: FormalSystem,
    ws: &Workspace,
    code: &str,
    short_name: &str,
) -> StatementQualityReport {
    if !gates.any() {
        return StatementQualityReport::not_consulted(gates);
    }
    // Both detectors are Lean-specific: one drives `lean`/`lake env lean` over a
    // mutated Lean file, the other elaborates a Lean `run_cmd` probe. For any
    // other system there is no check to run, which is silence, not a pass.
    if system != FormalSystem::Lean {
        let note = format!(
            "detector is Lean-only; system is {}, so nothing was checked",
            system.as_str()
        );
        return StatementQualityReport {
            gates,
            triviality: DetectorOutcome::silent(TOOL_TRIVIALITY, false, note.clone(), Value::Null),
            opaque: DetectorOutcome::silent(TOOL_OPAQUE, false, note, Value::Null),
        };
    }

    let workspace = lake_workspace(cfg);

    let triviality = if gates.triviality {
        let mut request = json!({
            "tool": TOOL_TRIVIALITY,
            "op": "check",
            "source_path": ws.source_path.to_string_lossy().into_owned(),
            "theorem_name": short_name,
            "timeout": DEFAULT_TIMEOUT_SECS,
        });
        if let Some(root) = workspace.clone() {
            request["lake_workspace"] = Value::String(root);
        }
        match run_worker(request) {
            Some(payload) => outcome_from_payload(TOOL_TRIVIALITY, payload),
            None => DetectorOutcome::silent(
                TOOL_TRIVIALITY,
                true,
                "worker unavailable, failed, or returned an unreadable reply; treated as silence",
                Value::Null,
            ),
        }
    } else {
        DetectorOutcome::switched_off(TOOL_TRIVIALITY)
    };

    let opaque = if gates.opaque {
        let mut request = json!({
            "tool": TOOL_OPAQUE,
            "source": code,
            "theorem_name": ws.entry.clone(),
            "timeout": DEFAULT_TIMEOUT_SECS,
        });
        if let Some(root) = workspace {
            request["lake_workspace"] = Value::String(root);
        }
        match run_worker(request) {
            Some(payload) => outcome_from_payload(TOOL_OPAQUE, payload),
            None => DetectorOutcome::silent(
                TOOL_OPAQUE,
                true,
                "worker unavailable, failed, or returned an unreadable reply; treated as silence",
                Value::Null,
            ),
        }
    } else {
        DetectorOutcome::switched_off(TOOL_OPAQUE)
    };

    StatementQualityReport {
        gates,
        triviality,
        opaque,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_two_accusing_verdicts_are_signals() {
        assert_eq!(
            Signal::from_verdict(TOOL_TRIVIALITY, "trivial"),
            Signal::Accused
        );
        assert_eq!(
            Signal::from_verdict(TOOL_OPAQUE, "opaque_constant_found"),
            Signal::Accused
        );
        for (tool, verdict) in [
            (TOOL_TRIVIALITY, "not_shown_trivial"),
            (TOOL_OPAQUE, "no_opaque_constant_found"),
        ] {
            assert_eq!(Signal::from_verdict(tool, verdict), Signal::NoAccusation);
        }
        for (tool, verdict) in [
            (TOOL_TRIVIALITY, "withheld"),
            (TOOL_OPAQUE, "unknown"),
            (TOOL_TRIVIALITY, ""),
            (TOOL_OPAQUE, "ok"),
            (TOOL_TRIVIALITY, "TRIVIAL"),
        ] {
            assert_eq!(Signal::from_verdict(tool, verdict), Signal::Silent);
        }
    }

    #[test]
    fn a_verdict_from_the_wrong_detector_cannot_accuse() {
        // A mis-routed reply must not be actionable.
        assert_eq!(
            Signal::from_verdict(TOOL_OPAQUE, "trivial"),
            Signal::Silent
        );
        assert_eq!(
            Signal::from_verdict(TOOL_TRIVIALITY, "opaque_constant_found"),
            Signal::Silent
        );
    }

    #[test]
    fn nothing_but_an_accusation_ever_blocks() {
        let mut report = StatementQualityReport::not_consulted(StatementQualityGates::ENFORCING);
        // Switched off / silent / no-accusation, all with enforcement ON.
        assert!(!report.blocks());
        report.triviality = DetectorOutcome::silent(TOOL_TRIVIALITY, true, "worker died", Value::Null);
        assert!(!report.blocks());
        report.triviality =
            outcome_from_payload(TOOL_TRIVIALITY, json!({"verdict": "not_shown_trivial"}));
        assert!(!report.blocks());
        report.opaque = outcome_from_payload(TOOL_OPAQUE, json!({"verdict": "unknown"}));
        assert!(!report.blocks());
        // Only a real accusation.
        report.opaque = outcome_from_payload(TOOL_OPAQUE, json!({"verdict": "opaque_constant_found"}));
        assert!(report.accuses());
        assert!(report.blocks());
        assert_eq!(report.accusers(), vec![TOOL_OPAQUE.to_string()]);
    }

    #[test]
    fn an_accusation_is_advisory_unless_enforcement_is_switched_on() {
        let mut report = StatementQualityReport::not_consulted(StatementQualityGates::ADVISORY);
        report.triviality = outcome_from_payload(TOOL_TRIVIALITY, json!({"verdict": "trivial"}));
        assert!(report.accuses());
        assert!(!report.blocks(), "advisory mode must not flip a verdict");
    }

    #[test]
    fn a_reply_with_no_verdict_is_silence() {
        let outcome = outcome_from_payload(TOOL_TRIVIALITY, json!({"ok": true, "stages": []}));
        assert_eq!(outcome.signal, Signal::Silent);
        assert!(outcome.verdict.is_none());
        assert!(outcome.consulted);
    }

    #[test]
    fn defaults_are_off() {
        assert_eq!(StatementQualityGates::default(), StatementQualityGates::OFF);
        assert!(!StatementQualityGates::OFF.any());
        assert!(StatementQualityGates::ADVISORY.any());
        assert!(!StatementQualityGates::ADVISORY.enforce);
        assert!(StatementQualityGates::ENFORCING.enforce);
    }
}
