//! Statement-quality ACCUSERS: a layer of the gate that asks whether the
//! STATEMENT says anything, consulted unconditionally as part of
//! [`crate::prover::formal::FormalBackend::verify_with_gates`].
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
//! WHY THERE IS NO INVOCATION SWITCH
//! =================================
//! This layer used to sit behind three default-off environment variables. A gate
//! that defaults off is not a gate; it is dead code with a flag, and it makes
//! production and test diverge silently. The two things the switches were
//! standing in for are now handled directly:
//!
//! **COST is guarded structurally, in three stages, cheapest first.** No detector
//! is allowed to spawn Lean until something free or nearly free has said the
//! statement is in its covered class:
//!
//! 1. A zero-cost Rust precondition ([`triviality_precondition`],
//!    [`opaque_precondition`]) that spawns no process at all. For triviality it
//!    is the observation that `plan_mutation` cannot admit a statement unless the
//!    file declares a `structure` (step 5 of its covered class requires the
//!    return type of every mutated definition to be a same-file structure), so a
//!    source with no `structure` token is a guaranteed withhold. For opaque it is
//!    the layer-2 axiom closure, which the caller has ALREADY computed: see
//!    [`opaque_precondition`] for why `sorryAx` absent there makes an accusation
//!    unreachable.
//! 2. A cheap PYTHON precondition, for triviality only: `plan_mutation`, which is
//!    pure Python, touches no filesystem and runs no Lean. It is reached through
//!    the worker's `op: "plan"` arm and costs one interpreter start.
//! 3. Only then, the expensive Lean path.
//!
//! Measured over the corpora in this repo (see the module tests and the report
//! that accompanied this change): on a 400-file, 7365-statement sample of
//! Mathlib, stage 1 alone rejects 72.1% of statements and stage 2 rejects the
//! remaining 27.9%, so **0.0%** of ordinary mathematics reaches a Lean spawn. On
//! the machine-generated `MaxwellEquations` corpus the detector was built for,
//! 69.2% reach the expensive path, which is the intended behavior: that is where
//! the defect lives.
//!
//! **REPEAT COST is guarded by a cache** keyed on the detector, the exact source
//! and declaration name, and the RESOLVED environment fingerprint
//! ([`crate::checker_cache::EnvironmentFingerprint`]) that already makes a cached
//! verdict honest elsewhere in this crate. See [`detector_cache_key`] for why an
//! unresolved environment refuses the cache outright rather than merely keying
//! differently.
//!
//! ADVISORY VERSUS BLOCKING IS A PROPERTY OF THE ACCUSATION
//! ========================================================
//! There is no enforcement switch. Whether an accusation moves a verdict is
//! decided in exactly one place, [`AccusationPolicy::blocks`], by one documented
//! rule, [`AccusationPolicy::RULE`], applied to the evidence the detector
//! actually returned.

use crate::{
    config::Config,
    prover::formal::{AxiomReport, FormalSystem, Workspace},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

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

/// The worker's own withhold verdict, returned by the `op: "plan"` precondition
/// when the statement is outside the covered class.
const VERDICT_WITHHELD: &str = "withheld";

/// The axiom name Lean emits for a `sorry` or `admit`, and the sole thing
/// `opaque_statement` can accuse on. Matched against the layer-2 closure.
const ADMITTED_AXIOM: &str = "sorryAx";

/// Seconds handed to the worker as its own Lean timeout. The Python side turns a
/// timeout into `withheld` / `unknown`, i.e. silence, which is why this being
/// too small can only lose signal and can never manufacture one.
const DEFAULT_TIMEOUT_SECS: f64 = 300.0;

/// Wall-clock ceiling for ONE expensive detector call, enforced on the Rust side.
///
/// [`crate::tools::PythonCheck`] has no timeout of its own: it calls
/// `wait_with_output` and blocks forever. The Python detectors do pass a timeout
/// down to the Lean subprocess they spawn, so the elaborator is bounded, but the
/// worker wrapper around it is not, and a wedged interpreter would hang a whole
/// verification. This budget is what stops that. It is set above the worker's own
/// Lean timeout (five stages at [`DEFAULT_TIMEOUT_SECS`] is the theoretical
/// worst case, but the stages short-circuit) so that in normal operation the
/// PYTHON side times out first and returns a structured withhold, and this
/// ceiling only fires when the worker itself is stuck.
const DETECTOR_BUDGET_SECS: u64 = 900;

/// Wall-clock ceiling for the CHEAP `op: "plan"` precondition. `plan_mutation`
/// is pure Python with no subprocess, so anything beyond a few seconds means the
/// interpreter never started or is wedged, not that the analysis is slow.
const PRECONDITION_BUDGET_SECS: u64 = 60;

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

// --- the single advisory/blocking policy ----------------------------------

/// The ONE place that decides whether an accusation moves a verdict.
///
/// This replaces the former `THEOREMATA_STATEMENT_QUALITY_ENFORCE` switch. A
/// switch made the answer a deployment accident; the answer belongs to the
/// accusation itself, because what makes an accusation safe to act on is a
/// property of the evidence behind it.
pub struct AccusationPolicy;

impl AccusationPolicy {
    /// The rule, in one sentence, stated once and quoted into every report so a
    /// reader never has to reconstruct it from code.
    ///
    /// **An accusation BLOCKS only when it is CORROBORATED by more than one
    /// independent trial within the same run, AND it is not already implied by a
    /// gate layer that ran earlier. Every other accusation is ADVISORY:
    /// published in full, never verdict-moving.**
    pub const RULE: &'static str = "an accusation blocks only when corroborated by more than one \
         independent trial in the same run and not already implied by an earlier gate layer; \
         every other accusation is advisory";

    /// Apply [`AccusationPolicy::RULE`] to one detector outcome.
    ///
    /// Only [`Signal::Accused`] can ever reach a `true` here, so silence and
    /// non-accusation are structurally incapable of failing a verification. That
    /// is the fail-closed-into-silence guarantee, and it does not depend on any
    /// switch.
    ///
    /// * `statement_triviality` CAN block. Its accusation is the outcome of a
    ///   staged experiment that had to succeed four separate times over two
    ///   mutually distinct sentinels, each with its own stage-A elaboration and
    ///   stage-B proof replay, any one of which failing would have produced a
    ///   different verdict. That is corroboration in the sense the rule means: a
    ///   single flaky compile cannot manufacture it. And nothing earlier in the
    ///   gate implies it, which is the entire reason this layer exists; a trivial
    ///   statement is kernel-clean, axiom-clean, lexically clean and preserved.
    ///
    /// * `opaque_statement` NEVER blocks. It is a single probe run parsed once,
    ///   so it is not corroborated; and its accusation is by construction already
    ///   implied by the layer-2 axiom audit, which must have seen `sorryAx` for
    ///   this detector to be consulted at all (see [`opaque_precondition`]) and
    ///   which has therefore ALREADY set `axioms_clean` false. Its value is
    ///   attribution, telling a human which constants are hollow, not a second
    ///   and weaker opinion about a verdict that is already decided.
    ///
    /// Corroboration is CHECKED against the payload, not assumed from the tool
    /// name. If the worker ever stops emitting the evidence of its own staging,
    /// this degrades to advisory rather than silently continuing to block on a
    /// claim we can no longer see.
    pub fn blocks(outcome: &DetectorOutcome) -> bool {
        if !outcome.signal.accuses() {
            return false;
        }
        if outcome.tool != TOOL_TRIVIALITY {
            return false;
        }
        Self::corroborated_by_distinct_sentinels(&outcome.detail)
    }

    /// Whether a triviality payload EXHIBITS its corroboration: at least two
    /// mutually distinct sentinels, and a successful stage B recorded for each.
    ///
    /// Reading the stages rather than trusting the verdict is what makes the
    /// blocking decision auditable. A payload that merely says `trivial` without
    /// showing the trials that earned it is not enough to fail a verification.
    fn corroborated_by_distinct_sentinels(detail: &Value) -> bool {
        let mut passing: Vec<i64> = Vec::new();
        let Some(stages) = detail.get("stages").and_then(Value::as_array) else {
            return false;
        };
        for stage in stages {
            if stage.get("stage").and_then(Value::as_str) != Some("B") {
                continue;
            }
            if stage.get("ok").and_then(Value::as_bool) != Some(true) {
                continue;
            }
            if let Some(s) = stage.get("sentinel").and_then(Value::as_i64) {
                if !passing.contains(&s) {
                    passing.push(s);
                }
            }
        }
        passing.len() >= 2
    }
}

// --- evidence -------------------------------------------------------------

/// Why a detector was not run, when it was not. Distinguishing these matters to
/// a human triaging a report: "outside the covered class" is a fact about the
/// statement, while "worker unavailable" is a fact about the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Rejected by the zero-cost Rust precondition. No process was spawned.
    Precondition,
    /// Rejected by the cheap Python precondition (`plan_mutation`). One
    /// interpreter start, no Lean.
    CheapProbe,
    /// The expensive path ran (or tried to).
    Expensive,
    /// Served from the detector cache, so nothing ran at all this time.
    Cached,
}

/// One detector's outcome, in the shape that is published to a verification
/// report's `detail`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectorOutcome {
    /// The worker tool key (`statement_triviality` / `opaque_statement`).
    pub tool: String,
    /// How far this detector got. See [`Stage`].
    pub stage: Stage,
    /// Whether the expensive path was actually entered.
    pub consulted: bool,
    pub signal: Signal,
    /// The raw verdict string the worker returned, verbatim and untrusted, or
    /// `None` when there was no reply to read one out of.
    pub verdict: Option<String>,
    /// Whether this outcome moved the verdict, per [`AccusationPolicy`]. Always
    /// false for anything that is not a corroborated accusation.
    pub blocking: bool,
    /// Why this outcome, in one line, for a human reading a failed report.
    pub note: String,
    /// The worker's own payload, or a description of the failure path. Never
    /// interpreted beyond the corroboration check; carried so the accusation can
    /// be checked by hand.
    #[serde(default)]
    pub detail: Value,
}

impl DetectorOutcome {
    /// Silence, with a reason and a stage. Every failure path in this module
    /// ends here, and silence is never blocking.
    fn silent(tool: &str, stage: Stage, note: impl Into<String>, detail: Value) -> Self {
        Self {
            tool: tool.to_string(),
            stage,
            consulted: matches!(stage, Stage::Expensive),
            signal: Signal::Silent,
            verdict: None,
            blocking: false,
            note: note.into(),
            detail,
        }
    }

    /// Convenience for callers: did THIS detector accuse?
    pub fn accuses(&self) -> bool {
        self.signal.accuses()
    }
}

/// Both detectors' outcomes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatementQualityReport {
    /// The blocking rule in force, quoted so a report is self-explaining.
    pub policy: String,
    pub triviality: DetectorOutcome,
    pub opaque: DetectorOutcome,
    /// How the environment resolved, for the cache. `Unresolved` here means no
    /// result was read from or written to the cache on this run.
    pub environment: String,
}

impl StatementQualityReport {
    /// A report in which neither detector ran, for one shared reason. Used for
    /// non-Lean systems, where there is simply no check to run.
    fn all_silent(reason: &str, environment: String) -> Self {
        Self {
            policy: AccusationPolicy::RULE.to_string(),
            triviality: DetectorOutcome::silent(
                TOOL_TRIVIALITY,
                Stage::Precondition,
                reason,
                Value::Null,
            ),
            opaque: DetectorOutcome::silent(TOOL_OPAQUE, Stage::Precondition, reason, Value::Null),
            environment,
        }
    }

    /// Whether ANY detector accused. Note that accusing and blocking are
    /// different questions; see [`StatementQualityReport::blocks`].
    pub fn accuses(&self) -> bool {
        self.triviality.accuses() || self.opaque.accuses()
    }

    /// Whether this report should fail a verification.
    ///
    /// The whole enforcement surface, and it delegates every decision to
    /// [`AccusationPolicy::blocks`]. [`Signal::Silent`] and
    /// [`Signal::NoAccusation`] are both false there, so an unavailable worker, a
    /// timeout, a missing Lean toolchain or a malformed reply leaves the verdict
    /// EXACTLY as it would have been without this layer.
    pub fn blocks(&self) -> bool {
        self.triviality.blocking || self.opaque.blocking
    }

    /// The names of the detectors that accused, for a one-line failure message.
    pub fn accusers(&self) -> Vec<String> {
        [&self.triviality, &self.opaque]
            .iter()
            .filter(|o| o.accuses())
            .map(|o| o.tool.clone())
            .collect()
    }

    /// The names of the detectors whose accusation actually moved the verdict.
    pub fn blockers(&self) -> Vec<String> {
        [&self.triviality, &self.opaque]
            .iter()
            .filter(|o| o.blocking)
            .map(|o| o.tool.clone())
            .collect()
    }
}

// --- zero-cost preconditions ----------------------------------------------

/// Whether `statement_triviality` could POSSIBLY admit this source, decided
/// without spawning anything.
///
/// Step 5 of the detector's covered class requires the return type of every
/// definition it would mutate to name a `structure` declared IN THE SAME FILE,
/// and step 3 requires at least one such definition, so a source that contains no
/// `structure` token at all is a guaranteed withhold. Testing the bare token
/// (rather than parsing declarations) is deliberately over-inclusive: a
/// `structure` mentioned only in a comment costs one cheap Python probe that
/// then withholds, whereas being too clever here could skip a real detection.
///
/// Verified over a 7365-statement Mathlib sample and both in-repo corpora:
/// `plan_mutation` admitted zero statements that this predicate rejects, and the
/// predicate rejected 72.1% of the Mathlib sample.
pub fn triviality_precondition(code: &str) -> bool {
    contains_token(code, "structure")
}

/// Whether `opaque_statement` could POSSIBLY accuse, decided from the layer-2
/// axiom closure the caller has ALREADY computed. Costs nothing.
///
/// The detector accuses exactly when some constant of the theorem's TYPE has
/// `sorryAx` in its own axiom closure. The layer-2 audit runs `#print axioms` on
/// the theorem, whose closure is the union over everything the theorem's type AND
/// value depend on. So the type's constants' closures are a SUBSET of what the
/// audit reported: if `sorryAx` is not in the audit's list, no constant of the
/// type can carry it, and the detector cannot reach its accusing verdict. Running
/// it anyway would spend a full Lean elaboration to prove a foregone conclusion.
///
/// This is the detector's own stated purpose read as a precondition: it exists to
/// ATTRIBUTE a `sorryAx` the audit already reported, not to find a new one.
///
/// A degraded or unavailable audit reports an empty closure, which skips the
/// detector. That direction is correct: it costs a miss, never a false
/// accusation, and it leaves the verdict where the earlier layers put it.
pub fn opaque_precondition(axioms: &AxiomReport) -> bool {
    axioms.axioms.iter().any(|a| a.contains(ADMITTED_AXIOM))
}

/// Substring search that will not match inside a longer identifier, so
/// `structures` or `my_structure` do not count as a `structure` declaration.
fn contains_token(haystack: &str, token: &str) -> bool {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '\'';
    haystack.match_indices(token).any(|(idx, _)| {
        let before_ok = haystack[..idx].chars().next_back().is_none_or(|c| !is_ident(c));
        let after_ok = haystack[idx + token.len()..]
            .chars()
            .next()
            .is_none_or(|c| !is_ident(c));
        before_ok && after_ok
    })
}

// --- the detector cache ---------------------------------------------------

/// Process-local memo of detector outcomes.
///
/// Deliberately in-memory and process-scoped: the key commits to an environment
/// fingerprint measured in THIS process, and nothing here is worth persisting
/// past the run that measured it.
fn detector_cache() -> &'static Mutex<HashMap<String, DetectorOutcome>> {
    static CACHE: OnceLock<Mutex<HashMap<String, DetectorOutcome>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The USABLE cache key for one detector call, or `None` when there is none.
///
/// `None` is returned exactly when the environment did not RESOLVE, mirroring
/// [`crate::checker_cache::cache_key`] rather than introducing a second notion of
/// identity. Caching against an unresolved environment is the stale-green bug
/// this project already fixed once: a verdict earned against one Mathlib would be
/// reused against another we never looked at. A miss costs one redundant
/// detector run; a hit on an unknown environment costs the meaning of the result.
///
/// Three things are hashed, each length-framed and domain-separated so no two
/// fields can be confused for one another:
///
/// * the detector's tool key, so the two detectors can never share an entry;
/// * the exact statement identity: the source bytes VERBATIM (never normalized,
///   because `statement_triviality` slices the file by byte offset and whitespace
///   changes what it mutates) plus the declaration name the detector was pointed
///   at;
/// * [`crate::checker_cache::EnvironmentFingerprint::key_field`], which is a
///   digest of the resolved Lake manifest content, the toolchain pin and the
///   canonical project path.
///
/// So an edited proof, a renamed target, an in-place library update, a toolchain
/// bump or a different project all miss. There is nothing left that can change a
/// detector's answer while the key stays fixed.
pub fn detector_cache_key(
    tool: &str,
    code: &str,
    target: &str,
    environment: &crate::checker_cache::EnvironmentFingerprint,
) -> Option<String> {
    if !environment.is_resolved() {
        return None;
    }
    let mut hasher = Sha256::new();
    for (tag, data) in [
        (&b"theoremata.statement_quality.v1"[..], &b""[..]),
        (b"tool", tool.as_bytes()),
        (b"source", code.as_bytes()),
        (b"target", target.as_bytes()),
        (b"environment", environment.key_field().as_bytes()),
    ] {
        hasher.update((tag.len() as u64).to_be_bytes());
        hasher.update(tag);
        hasher.update((data.len() as u64).to_be_bytes());
        hasher.update(data);
    }
    Some(hex_lower(hasher.finalize()))
}

/// Lowercase hex of a digest. `sha2` 0.11 returns an `Array` that does not
/// implement `LowerHex`, so `{:x}` does not compile against it; this mirrors the
/// private helper of the same name in `checker_cache`.
fn hex_lower(bytes: impl AsRef<[u8]>) -> String {
    let mut out = String::with_capacity(bytes.as_ref().len() * 2);
    for byte in bytes.as_ref() {
        use std::fmt::Write;
        // Writing into a String is infallible; the result is discarded rather
        // than unwrapped so this module keeps its no-unwrap property.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Look up a previously computed outcome, restamped as [`Stage::Cached`].
fn cache_get(key: Option<&String>) -> Option<DetectorOutcome> {
    let key = key?;
    let guard = detector_cache().lock().ok()?;
    let mut hit = guard.get(key).cloned()?;
    hit.stage = Stage::Cached;
    hit.note = format!("{} [served from the detector cache]", hit.note);
    Some(hit)
}

/// Store an outcome, if it is worth storing.
///
/// [`Signal::Silent`] is NEVER stored. Silence means the check could not run:
/// the interpreter was missing, the toolchain was absent, the worker timed out.
/// Every one of those is a property of the MOMENT rather than of the input, and
/// caching it would freeze a transient outage into a permanent one for the rest
/// of the process. This is the same asymmetry as `checker_cache`'s refusal to
/// cache failures, for the same reason.
fn cache_put(key: Option<&String>, outcome: &DetectorOutcome) {
    if outcome.signal == Signal::Silent {
        return;
    }
    let Some(key) = key else { return };
    if let Ok(mut guard) = detector_cache().lock() {
        guard.insert(key.clone(), outcome.clone());
    }
}

// --- invocation -----------------------------------------------------------

/// Run one worker tool under a wall-clock budget and return the parsed `output`
/// object, or `None`.
///
/// Follows the precedent already in this component,
/// `crate::prover::formal::worker_source_scan`: build a `{"tool": …}` request,
/// push it through [`crate::tools::PythonCheck`] (which shells the interpreter
/// with the `components/*/python` bootstrap and writes the request on stdin),
/// then honour the worker's `{"ok": …, "output": …}` envelope. All worker text
/// is untrusted data and is only ever compared, never executed.
///
/// The budget is imposed HERE because `PythonCheck::run` has none: it blocks in
/// `wait_with_output` indefinitely. The call is moved to a helper thread and the
/// result collected with `recv_timeout`, so a wedged worker costs this thread
/// nothing but the budget. The caveat, stated plainly because it is a real
/// limitation and not a fixable one from this file: on timeout the helper thread
/// is NOT joined and the Python child is NOT killed, because `PythonCheck` hands
/// back no handle to kill. The child is left to exit on its own. Fixing that
/// properly means giving `PythonCheck` a timeout, in `components/tools/mod.rs`.
fn run_worker(request: Value, budget_secs: u64) -> Option<Value> {
    use std::sync::mpsc;
    use std::time::Duration;

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        use crate::tools::{PythonCheck, Tool};
        let py = PythonCheck::new();
        // The availability probe is inside the thread so that even it cannot
        // block the caller past the budget.
        let reply = if py.available() {
            py.run(request).ok()
        } else {
            None
        };
        // A send failure means the receiver already gave up on the budget. That
        // is expected, not an error, and there is nothing to do about it.
        let _ = tx.send(reply);
    });

    let result = rx.recv_timeout(Duration::from_secs(budget_secs)).ok()??;
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
                Stage::Expensive,
                "worker reply carried no verdict string",
                payload,
            )
        }
    };
    let signal = Signal::from_verdict(tool, &verdict);
    let mut outcome = DetectorOutcome {
        tool: tool.to_string(),
        stage: Stage::Expensive,
        consulted: true,
        signal,
        verdict: Some(verdict.clone()),
        blocking: false,
        note: String::new(),
        detail: payload,
    };
    // The policy is consulted exactly once, here, and its answer is recorded on
    // the outcome so every downstream reader sees the same decision rather than
    // re-deriving it.
    outcome.blocking = AccusationPolicy::blocks(&outcome);
    outcome.note = match signal {
        Signal::Accused if outcome.blocking => format!(
            "{tool} ACCUSES this statement (verdict {verdict:?}) and the accusation is \
             CORROBORATED, so it fails this verification; this is evidence about the \
             STATEMENT, not about the proof's soundness"
        ),
        Signal::Accused => format!(
            "{tool} ACCUSES this statement (verdict {verdict:?}); ADVISORY only under the \
             blocking rule, so it did not move the verdict"
        ),
        Signal::NoAccusation => format!(
            "{tool} ran and did not accuse (verdict {verdict:?}); this is NOT a certificate \
             that the statement is meaningful"
        ),
        Signal::Silent => format!(
            "{tool} withheld (verdict {verdict:?}); no signal, neither approval nor suspicion"
        ),
    };
    outcome
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

/// Stage 2 for the triviality detector: ask the worker's pure-Python
/// `plan_mutation` whether this statement is in the covered class at all.
///
/// Returns `Ok(())` to proceed to Lean, or `Err(outcome)` carrying the silence
/// to report. A worker that cannot be reached yields `Err`, so an unavailable
/// interpreter costs a skipped detector rather than an unguarded Lean spawn.
fn triviality_cheap_probe(code: &str, short_name: &str) -> Result<(), DetectorOutcome> {
    let request = json!({
        "tool": TOOL_TRIVIALITY,
        "op": "plan",
        "source": code,
        "theorem_name": short_name,
    });
    let Some(plan) = run_worker(request, PRECONDITION_BUDGET_SECS) else {
        return Err(DetectorOutcome::silent(
            TOOL_TRIVIALITY,
            Stage::CheapProbe,
            "cheap precondition (plan_mutation) could not be reached; no Lean was spawned",
            Value::Null,
        ));
    };
    // A plan carries `verdict: null`; a refusal carries `withheld`. Treat
    // anything that is not an explicit plan as a refusal, so a worker reply we
    // do not understand cannot buy an expensive run.
    let is_plan = plan.get("verdict").map(Value::is_null).unwrap_or(false)
        && plan.get("mutated_defs").is_some();
    if is_plan {
        return Ok(());
    }
    let reason = plan
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("no reason given")
        .to_string();
    let withheld = plan.get("verdict").and_then(Value::as_str) == Some(VERDICT_WITHHELD);
    let note = if withheld {
        format!("statement is outside the covered class, so no Lean was spawned: {reason}")
    } else {
        "cheap precondition returned neither a plan nor a withhold; declining to spend Lean"
            .to_string()
    };
    Err(DetectorOutcome::silent(
        TOOL_TRIVIALITY,
        Stage::CheapProbe,
        note,
        plan,
    ))
}

/// Consult both detectors for one verified artifact.
///
/// `code` is the submitted source, `ws` the scaffolded workspace whose
/// `source_path` the compile already used, `short_name` the theorem's
/// declaration name AS WRITTEN IN THE SOURCE, and `axioms` the layer-2 audit
/// result, which [`opaque_precondition`] reads so that detector costs nothing on
/// the overwhelming majority of proofs.
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
    system: FormalSystem,
    is_mock: bool,
    ws: &Workspace,
    code: &str,
    short_name: &str,
    axioms: &AxiomReport,
) -> StatementQualityReport {
    let environment = crate::checker_cache::EnvironmentFingerprint::resolve(
        system,
        is_mock,
        cfg.lean_project.as_deref(),
    );
    let env_note = environment.describe();

    // Both detectors are Lean-specific: one drives `lean`/`lake env lean` over a
    // mutated Lean file, the other elaborates a Lean `run_cmd` probe. For any
    // other system there is no check to run, which is silence, not a pass.
    if system != FormalSystem::Lean {
        return StatementQualityReport::all_silent(
            &format!(
                "detector is Lean-only; system is {}, so nothing was checked",
                system.as_str()
            ),
            env_note,
        );
    }
    // A mock backend's `source_path` and entry are scaffolding, not a real
    // elaboration target. Accusing on the strength of a mock is meaningless, and
    // its reports are already permanently non-live.
    if is_mock {
        return StatementQualityReport::all_silent(
            "backend is a mock, so there is no real elaboration to interrogate",
            env_note,
        );
    }

    let workspace = lake_workspace(cfg);

    // --- triviality ------------------------------------------------------
    let triviality = {
        let key = detector_cache_key(TOOL_TRIVIALITY, code, short_name, &environment);
        if let Some(hit) = cache_get(key.as_ref()) {
            hit
        } else if !triviality_precondition(code) {
            DetectorOutcome::silent(
                TOOL_TRIVIALITY,
                Stage::Precondition,
                "source declares no `structure`, so the covered class cannot apply; nothing ran",
                Value::Null,
            )
        } else {
            match triviality_cheap_probe(code, short_name) {
                Err(skipped) => skipped,
                Ok(()) => {
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
                    let outcome = match run_worker(request, DETECTOR_BUDGET_SECS) {
                        Some(payload) => outcome_from_payload(TOOL_TRIVIALITY, payload),
                        None => DetectorOutcome::silent(
                            TOOL_TRIVIALITY,
                            Stage::Expensive,
                            "worker unavailable, timed out, or returned an unreadable reply; \
                             treated as silence",
                            Value::Null,
                        ),
                    };
                    cache_put(key.as_ref(), &outcome);
                    outcome
                }
            }
        }
    };

    // --- opaque ----------------------------------------------------------
    let opaque = {
        let key = detector_cache_key(TOOL_OPAQUE, code, &ws.entry, &environment);
        if let Some(hit) = cache_get(key.as_ref()) {
            hit
        } else if !opaque_precondition(axioms) {
            DetectorOutcome::silent(
                TOOL_OPAQUE,
                Stage::Precondition,
                "layer-2 axiom closure carries no `sorryAx`, so no constant of the statement \
                 can be an admitted placeholder; nothing ran",
                Value::Null,
            )
        } else {
            let mut request = json!({
                "tool": TOOL_OPAQUE,
                "source": code,
                "theorem_name": ws.entry.clone(),
                "timeout": DEFAULT_TIMEOUT_SECS,
            });
            if let Some(root) = workspace {
                request["lake_workspace"] = Value::String(root);
            }
            let outcome = match run_worker(request, DETECTOR_BUDGET_SECS) {
                Some(payload) => outcome_from_payload(TOOL_OPAQUE, payload),
                None => DetectorOutcome::silent(
                    TOOL_OPAQUE,
                    Stage::Expensive,
                    "worker unavailable, timed out, or returned an unreadable reply; treated \
                     as silence",
                    Value::Null,
                ),
            };
            cache_put(key.as_ref(), &outcome);
            outcome
        }
    };

    StatementQualityReport {
        policy: AccusationPolicy::RULE.to_string(),
        triviality,
        opaque,
        environment: env_note,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A payload shaped like a real corroborated `trivial` finding.
    fn corroborated_trivial() -> Value {
        json!({
            "verdict": "trivial",
            "stages": [
                {"stage": "baseline", "ok": true},
                {"stage": "A", "sentinel": 424242, "ok": true},
                {"stage": "B", "sentinel": 424242, "ok": true},
                {"stage": "A", "sentinel": 909091, "ok": true},
                {"stage": "B", "sentinel": 909091, "ok": true}
            ]
        })
    }

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
        assert_eq!(Signal::from_verdict(TOOL_OPAQUE, "trivial"), Signal::Silent);
        assert_eq!(
            Signal::from_verdict(TOOL_TRIVIALITY, "opaque_constant_found"),
            Signal::Silent
        );
    }

    #[test]
    fn nothing_but_a_corroborated_accusation_ever_blocks() {
        // Silence and non-accusation, from either detector, never block.
        for outcome in [
            DetectorOutcome::silent(TOOL_TRIVIALITY, Stage::Expensive, "worker died", Value::Null),
            DetectorOutcome::silent(TOOL_TRIVIALITY, Stage::Precondition, "no struct", Value::Null),
            outcome_from_payload(TOOL_TRIVIALITY, json!({"verdict": "not_shown_trivial"})),
            outcome_from_payload(TOOL_OPAQUE, json!({"verdict": "unknown"})),
            outcome_from_payload(TOOL_OPAQUE, json!({"verdict": "no_opaque_constant_found"})),
        ] {
            assert!(!outcome.blocking, "{} must not block", outcome.note);
            assert!(!AccusationPolicy::blocks(&outcome));
        }
        let blocking = outcome_from_payload(TOOL_TRIVIALITY, corroborated_trivial());
        assert!(blocking.accuses());
        assert!(blocking.blocking);
    }

    #[test]
    fn an_opaque_accusation_is_always_advisory() {
        // It is already implied by the layer-2 audit, which had to have seen
        // `sorryAx` for this detector to run at all.
        let outcome = outcome_from_payload(
            TOOL_OPAQUE,
            json!({"verdict": "opaque_constant_found", "opaque_constants": [{"name": "f"}]}),
        );
        assert!(outcome.accuses(), "it must still accuse");
        assert!(!outcome.blocking, "but it must never block");
    }

    #[test]
    fn a_trivial_verdict_without_visible_corroboration_is_advisory() {
        // The blocking decision rests on evidence, not on the verdict string. A
        // worker that stops showing its staging degrades to advisory.
        for payload in [
            json!({"verdict": "trivial"}),
            json!({"verdict": "trivial", "stages": []}),
            // One sentinel only: not corroborated.
            json!({"verdict": "trivial", "stages": [
                {"stage": "B", "sentinel": 424242, "ok": true}
            ]}),
            // Two stage-B entries but the SAME sentinel: not independent.
            json!({"verdict": "trivial", "stages": [
                {"stage": "B", "sentinel": 424242, "ok": true},
                {"stage": "B", "sentinel": 424242, "ok": true}
            ]}),
            // Two sentinels, but one stage B failed, so the verdict is not
            // even internally consistent; refuse to block on it.
            json!({"verdict": "trivial", "stages": [
                {"stage": "B", "sentinel": 424242, "ok": true},
                {"stage": "B", "sentinel": 909091, "ok": false}
            ]}),
        ] {
            let outcome = outcome_from_payload(TOOL_TRIVIALITY, payload);
            assert!(outcome.accuses());
            assert!(!outcome.blocking, "must not block without corroboration");
        }
    }

    #[test]
    fn a_reply_with_no_verdict_is_silence() {
        let outcome = outcome_from_payload(TOOL_TRIVIALITY, json!({"ok": true, "stages": []}));
        assert_eq!(outcome.signal, Signal::Silent);
        assert!(outcome.verdict.is_none());
        assert!(!outcome.blocking);
    }

    #[test]
    fn report_blocking_agrees_with_the_policy() {
        let mut report = StatementQualityReport::all_silent("test", "mock".to_string());
        assert!(!report.blocks());
        assert!(!report.accuses());
        report.opaque = outcome_from_payload(TOOL_OPAQUE, json!({"verdict": "opaque_constant_found"}));
        assert!(report.accuses(), "an advisory accusation still accuses");
        assert!(!report.blocks(), "but it does not block");
        assert_eq!(report.accusers(), vec![TOOL_OPAQUE.to_string()]);
        assert!(report.blockers().is_empty());
        report.triviality = outcome_from_payload(TOOL_TRIVIALITY, corroborated_trivial());
        assert!(report.blocks());
        assert_eq!(report.blockers(), vec![TOOL_TRIVIALITY.to_string()]);
    }

    #[test]
    fn the_zero_cost_triviality_precondition_needs_a_structure_declaration() {
        assert!(triviality_precondition("structure S where\n  x : Int\n"));
        assert!(triviality_precondition("theorem t : True := trivial\n-- structure\n"));
        assert!(!triviality_precondition("theorem t : True := trivial\n"));
        // Must not match inside a longer identifier.
        assert!(!triviality_precondition("def structures := 1\n"));
        assert!(!triviality_precondition("def my_structure := 1\n"));
        assert!(!triviality_precondition("def structure' := 1\n"));
    }

    #[test]
    fn the_opaque_precondition_follows_the_layer_two_closure() {
        let report = |axs: &[&str]| AxiomReport {
            axioms: axs.iter().map(|s| s.to_string()).collect(),
            within_whitelist: false,
            detail: Value::Null,
        };
        assert!(opaque_precondition(&report(&["sorryAx"])));
        assert!(opaque_precondition(&report(&["propext", "sorryAx"])));
        // No sorryAx anywhere means the detector provably cannot accuse.
        assert!(!opaque_precondition(&report(&[])));
        assert!(!opaque_precondition(&report(&[
            "propext",
            "Classical.choice",
            "Quot.sound"
        ])));
    }

    #[test]
    fn an_unresolved_environment_refuses_the_cache_entirely() {
        use crate::checker_cache::EnvironmentFingerprint;
        let unresolved = EnvironmentFingerprint::unresolved("no lake project");
        assert_eq!(
            detector_cache_key(TOOL_TRIVIALITY, "src", "T", &unresolved),
            None,
            "an unresolved environment must produce NO key, so nothing is stored or served"
        );
        let resolved = EnvironmentFingerprint::mock();
        assert!(detector_cache_key(TOOL_TRIVIALITY, "src", "T", &resolved).is_some());
    }

    #[test]
    fn the_cache_key_separates_every_input_that_can_change_an_answer() {
        use crate::checker_cache::EnvironmentFingerprint;
        let env_a = EnvironmentFingerprint::from_parts("lake", "a", &[("m", "rev-a".into())]);
        let env_b = EnvironmentFingerprint::from_parts("lake", "b", &[("m", "rev-b".into())]);
        let k = |tool, code, target, env| detector_cache_key(tool, code, target, env).unwrap();
        let base = k(TOOL_TRIVIALITY, "source", "Thm", &env_a);
        assert_ne!(base, k(TOOL_OPAQUE, "source", "Thm", &env_a), "tool");
        assert_ne!(base, k(TOOL_TRIVIALITY, "source ", "Thm", &env_a), "source");
        assert_ne!(base, k(TOOL_TRIVIALITY, "source", "Thm2", &env_a), "target");
        assert_ne!(base, k(TOOL_TRIVIALITY, "source", "Thm", &env_b), "library");
        // Length framing: the concatenation of two fields must not collide with
        // a different split of the same bytes.
        assert_ne!(
            k(TOOL_TRIVIALITY, "ab", "c", &env_a),
            k(TOOL_TRIVIALITY, "a", "bc", &env_a)
        );
        // Stable across calls, or the cache never hits.
        assert_eq!(base, k(TOOL_TRIVIALITY, "source", "Thm", &env_a));
    }

    #[test]
    fn silence_is_never_cached() {
        // A transient outage must not be frozen into a permanent one.
        let key = Some("silence-test-key".to_string());
        let silent =
            DetectorOutcome::silent(TOOL_TRIVIALITY, Stage::Expensive, "worker died", Value::Null);
        cache_put(key.as_ref(), &silent);
        assert!(
            cache_get(key.as_ref()).is_none(),
            "silence must not be stored"
        );
        // A real verdict is.
        let real = outcome_from_payload(TOOL_TRIVIALITY, json!({"verdict": "not_shown_trivial"}));
        cache_put(key.as_ref(), &real);
        let hit = cache_get(key.as_ref()).expect("a real verdict is cached");
        assert_eq!(hit.signal, Signal::NoAccusation);
        assert_eq!(hit.stage, Stage::Cached, "a hit is labelled as a hit");
    }

    #[test]
    fn a_cached_accusation_keeps_its_blocking_decision() {
        // The policy is evaluated once and recorded; a cache round trip must not
        // silently upgrade or downgrade it.
        let key = Some("blocking-roundtrip-key".to_string());
        let blocking = outcome_from_payload(TOOL_TRIVIALITY, corroborated_trivial());
        assert!(blocking.blocking);
        cache_put(key.as_ref(), &blocking);
        let hit = cache_get(key.as_ref()).expect("cached");
        assert!(hit.blocking);
        assert!(AccusationPolicy::blocks(&hit), "still corroborated");
    }
}
