//! Per-system PROOF GENERATORS (Phase 3): produce — not just verify — a proof
//! for Lean, Rocq, or Isabelle, selected by the live verification gate.
//!
//! This is the model-driven analogue of the agent's Lean `formalize` best-of-N,
//! generalized across every [`FormalSystem`]. Given an informal/formal
//! `statement`, [`generate_and_verify`] prompts the provider with a
//! SYSTEM-SPECIFIC role/task ("Write a complete Lean 4 / Coq / Isabelle-Isar
//! proof of …; no `sorry`/`admit`/`Admitted`"), samples N candidates, and
//! accepts the first that passes the real [`FormalBackend::verify`] 3+1-layer
//! gate — exactly as `formalize` uses the compiler as the acceptance predicate.
//!
//! Mock-provider compatible: when no model is configured (`offline`) the
//! generator falls back to a system-native trivially-true stub, and the backend
//! still runs — the live toolchain when it is present, otherwise the mock
//! backend (canned kernel layers, but a REAL source scan, so a `sorry` /
//! `Admitted` candidate is still rejected).

use crate::{
    checker_cache::{CheckerCache, VerificationCacheKey},
    config::Config,
    db::Store,
    model::ModelRequest,
    prover::{
        error_feedback::{render_feedback, FeedbackConfig},
        formal::{backend_for, FormalBackend, FormalSystem},
        model::VerificationReport,
    },
    provider::ModelProvider,
    sampling,
};
use anyhow::{Context, Result};
use serde_json::{json, Value};

/// How many candidate proofs to sample before giving up (best-of-N).
const N: usize = 3;

/// One generated candidate together with its verification verdict.
struct Candidate {
    code: String,
    report: VerificationReport,
    cache_hit: bool,
}

/// Candidate sources prepared on the caller thread for a later verification
/// stage. This keeps model/provider calls and any hammer invocation out of the
/// owned-worker boundary used by the formal-system portfolio.
///
/// `attempts` includes generation failures, matching [`sampling::best_of_n`].
/// The candidates themselves retain generation order; callers select the first
/// verifier-approved source in that order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedProofCandidates {
    pub candidates: Vec<String>,
    pub attempts: usize,
    pub hammer_candidate: bool,
}

// Process-default cache used by the existing production API. The proving path is
// sequential today, so a thread-local cache avoids global locking while still
// reusing identical candidates across portfolio/agent calls on that thread.
// Callers needing explicit lifetime/isolation use generate_and_verify_with_cache.
thread_local! {
    static DEFAULT_CHECKER_CACHE: CheckerCache = CheckerCache::new();
}

/// Generate a proof of `statement` in `system` and verify it through the live
/// 3+1-layer gate, returning the accepted candidate's `(code, report)` — or, if
/// none of the N samples verify, the last candidate produced (its `report`
/// carries `lexically_verified = false`).
///
/// The acceptance selector is [`FormalBackend::verify`] on the live backend when
/// its toolchain is available, otherwise the mock backend (whose source scan is
/// still real). Emitting system-native output (`.lean` / `.v` / `.thy`).
pub fn generate_and_verify(
    store: &Store,
    config: &Config,
    provider: &dyn ModelProvider,
    system: FormalSystem,
    statement: &str,
) -> Result<(String, VerificationReport)> {
    DEFAULT_CHECKER_CACHE.with(|cache| {
        generate_and_verify_with_cache(store, config, provider, system, statement, &[], Some(cache))
    })
}

/// Cache-aware variant of [`generate_and_verify`]. `ordered_context` is part of
/// the verification identity and is never sorted. Passing `None` preserves the
/// pre-cache behavior exactly; an empty cache merely adds a lookup before the
/// same live gate call. Only complete LIVE successes are inserted.
pub fn generate_and_verify_with_cache(
    store: &Store,
    config: &Config,
    provider: &dyn ModelProvider,
    system: FormalSystem,
    statement: &str,
    ordered_context: &[String],
    cache: Option<&CheckerCache>,
) -> Result<(String, VerificationReport)> {
    // Prefer the live backend (real compile/audit/recheck) when its toolchain is
    // present; degrade to the mock backend when it is absent or when
    // `config.prover_mock` forces offline mode. Either way the source scan runs
    // for real, so `sorry`/`Admitted` never passes.
    let live = backend_for(config, system, false);
    let used_live = !config.prover_mock && live.available();
    let backend = if used_live {
        live
    } else {
        backend_for(config, system, true)
    };
    let checker_identity = checker_identity(config, backend.as_ref());
    let policy_fingerprint = policy_fingerprint(config, backend.as_ref());
    // The import closure this verification runs under. A cached PASS is only
    // meaningful relative to an environment: a proof verified under an import
    // list that smuggles in an axiom must never share a cache slot with one
    // verified under a clean list.
    let import_manifest = system.default_imports();

    // An ADDITIONAL best-of-N candidate: a hammer-assisted proof. We ask the
    // `hammer` worker (Sledgehammer / CoqHammer / aesop) to FIND a tactic for the
    // goal and assemble a complete system-native proof around it. The external
    // ATP is only a hint oracle — the acceptance predicate below is unchanged (the
    // real `FormalBackend::verify` 3+1-layer gate), so a hammer proof is trusted
    // only if it genuinely verifies. When the model is offline/mock but the hammer
    // is live (Isabelle on this box), the hammer can still produce a VERIFIED
    // proof; when unavailable it yields `None` and the candidate is simply skipped.
    //
    // Gated on the LIVE backend: only a real 3+1-layer gate keeps a hammer-found
    // proof honest. Under the mock backend the "verification" is canned, so a mock
    // hammer would fabricate a clean pass — we therefore skip it entirely there.
    let hammer_candidate = if used_live {
        hammer_prove(config, system, statement)
    } else {
        None
    };
    let total = N + hammer_candidate.is_some() as usize;

    let selection = sampling::best_of_n(
        total,
        |i| -> Result<Candidate> {
            // Slot 0 is the hammer-assisted candidate when one was produced; every
            // other slot is a fresh model (or offline-stub) generation.
            let code = match (i, &hammer_candidate) {
                (0, Some(h)) => h.clone(),
                _ => generate_once(provider, system, statement)?,
            };
            let key = VerificationCacheKey {
                system,
                canonical_statement: statement,
                ordered_context,
                proof_source: &code,
                checker_identity: &checker_identity,
                policy_fingerprint: &policy_fingerprint,
                import_manifest: &import_manifest,
            };
            let (mut report, cache_hit) =
                verify_candidate(cache, &key, || backend.verify(config, &code, statement))?;
            attach_error_feedback(system, &code, &mut report);
            Ok(Candidate {
                code,
                report,
                cache_hit,
            })
        },
        |c: &Candidate| c.report.lexically_verified,
    )?;

    let sampled = selection.context("no proof candidate could be generated")?;

    store.event(
        None,
        None,
        "formal_generate.completed",
        system.as_str(),
        json!({
            "system": system.as_str(),
            "accepted": sampled.accepted,
            "attempts": sampled.attempts,
            "index": sampled.index,
            "verified": sampled.value.report.lexically_verified,
            "backend": if used_live { "live" } else { "mock" },
            "hammer_candidate": hammer_candidate.is_some(),
            "checker_cache_enabled": cache.is_some(),
            "checker_cache_hit": sampled.value.cache_hit,
        }),
    )?;

    Ok((sampled.value.code, sampled.value.report))
}

/// Generate every candidate needed for a portfolio verification stage without
/// invoking a checker. Generation and any optional hammer call remain on the
/// caller thread; only the returned owned source strings may cross into worker
/// threads.
///
/// This is intentionally not used by [`generate_and_verify_with_cache`]: that
/// API preserves its established sequential best-of-N short-circuit behavior.
/// The portfolio uses this preparation path only after an explicit concurrency
/// opt-in, where running independent verifier jobs in parallel requires all
/// candidate inputs up front.
pub fn generate_candidates_for_verification(
    config: &Config,
    provider: &dyn ModelProvider,
    system: FormalSystem,
    statement: &str,
    live_backend_available: bool,
) -> Result<GeneratedProofCandidates> {
    // This mirrors the candidate ordering in generate_and_verify_with_cache:
    // a live hammer result occupies slot zero, followed by model candidates.
    // Failed generations are skipped but still count as attempts.
    let hammer_candidate = if !config.prover_mock && live_backend_available {
        hammer_prove(config, system, statement)
    } else {
        None
    };
    let total = N + hammer_candidate.is_some() as usize;
    let mut candidates = Vec::with_capacity(total);

    for i in 0..total {
        let code = match (i, &hammer_candidate) {
            (0, Some(h)) => Ok(h.clone()),
            _ => generate_once(provider, system, statement),
        };
        if let Ok(code) = code {
            candidates.push(code);
        }
    }

    if candidates.is_empty() {
        anyhow::bail!("no proof candidate could be generated");
    }

    Ok(GeneratedProofCandidates {
        candidates,
        attempts: total,
        hammer_candidate: hammer_candidate.is_some(),
    })
}

/// Return a cached live verdict or run the real verifier on a miss. The helper is
/// deliberately small so its safety properties are unit-testable without an
/// external prover: no cache/empty cache runs `verify`, failures and mock reports
/// are refused by `CheckerCache`, and only an exact key can hit.
fn verify_candidate<F>(
    cache: Option<&CheckerCache>,
    key: &VerificationCacheKey<'_>,
    verify: F,
) -> Result<(VerificationReport, bool)>
where
    F: FnOnce() -> Result<VerificationReport>,
{
    if let Some(report) = cache.and_then(|cache| cache.get(key)) {
        return Ok((report, true));
    }

    let report = verify()?;
    if let Some(cache) = cache {
        // Defensive insertion policy lives in CheckerCache: incomplete, failed,
        // and mock reports are all refused and therefore never become hits.
        cache.insert_verified(key, report.clone());
    }
    Ok((report, false))
}

/// The `detail` key under which rendered checker feedback is published.
const ERROR_FEEDBACK_KEY: &str = "error_feedback";

/// Enrich a FAILED verification's `detail` with prompt-ready checker feedback.
///
/// Until now the checker's own words reached the model nowhere: backends bury
/// stdout/stderr in `detail["compile"]["detail"]`, and no `reason/` site reads
/// it. This renders that raw text through [`render_feedback`] and republishes it
/// under `detail["error_feedback"]`, so a retry/repair caller can hand the model
/// the positional diagnostics instead of nothing.
///
/// **Advisory only, and strictly additive.** It never inspects or changes any
/// verdict field, never runs on a passing report, and cannot fail: absent or
/// unparseable checker output degrades inside `error_feedback` to a bounded raw
/// passthrough. A `detail` that is not a JSON object is left untouched.
fn attach_error_feedback(system: FormalSystem, code: &str, report: &mut VerificationReport) {
    if report.lexically_verified {
        return;
    }
    let Some(detail) = report.detail.as_object_mut() else {
        return;
    };
    let raw = raw_checker_output(detail.get("compile"));
    let rendered = render_feedback(system, &raw, code, &FeedbackConfig::default());
    detail.insert(
        ERROR_FEEDBACK_KEY.to_string(),
        json!({
            "schema": "theoremata.error-feedback.v1",
            "system": system.as_str(),
            "text": rendered.text,
            "parsed": rendered.parsed,
            "omitted": rendered.omitted,
            "diagnostics": serde_json::to_value(&rendered.diagnostics).unwrap_or(Value::Null),
        }),
    );
}

/// Recover the checker's raw text from a serialized `CompileReport`.
///
/// The shape is `{"compiled":…, "errors":[…], "per_unit":[…], "detail":{…}}`;
/// every live backend puts the checker's words in `detail.stderr`/`detail.stdout`
/// (see `backends/lean.rs::compile`) and duplicates them into `errors`. Prefer
/// the nested detail, fall back to `errors`, and return an empty string when
/// neither exists — the renderer handles that case on its own.
fn raw_checker_output(compile: Option<&Value>) -> String {
    let Some(compile) = compile else {
        return String::new();
    };
    let mut parts: Vec<&str> = Vec::new();
    if let Some(inner) = compile.get("detail") {
        for key in ["stderr", "stdout"] {
            if let Some(s) = inner.get(key).and_then(Value::as_str) {
                if !s.trim().is_empty() {
                    parts.push(s);
                }
            }
        }
    }
    if parts.is_empty() {
        if let Some(errors) = compile.get("errors").and_then(Value::as_array) {
            parts.extend(
                errors
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|s| !s.trim().is_empty()),
            );
        }
    }
    parts.join("\n")
}

/// Stable identity for the checker installation and corpus selected by config.
/// The optional epoch is an explicit cache-buster for an in-place tool/corpus
/// replacement whose path and configured toolchain string did not change.
fn checker_identity(config: &Config, backend: &dyn FormalBackend) -> String {
    let system = backend.system();
    let env_or = crate::prover::exec::env_or;
    let binaries = match system {
        FormalSystem::Lean => json!({
            "lean": env_or("THEOREMATA_LEAN", &config.lean_bin),
            "lake": env_or("THEOREMATA_LAKE", "lake"),
        }),
        FormalSystem::Rocq => json!({
            "coqc": env_or("THEOREMATA_COQC", &config.coqc_bin),
            "coqchk": env_or("THEOREMATA_COQCHK", &config.coqchk_bin),
        }),
        FormalSystem::Isabelle => json!({
            "isabelle": env_or("THEOREMATA_ISABELLE", &config.isabelle_bin),
        }),
        FormalSystem::Candle => json!({
            "candle": env_or("THEOREMATA_CANDLE", &config.candle_bin),
        }),
        FormalSystem::Agda => json!({
            "agda": env_or("THEOREMATA_AGDA", &config.agda_bin),
        }),
        FormalSystem::Metamath => json!({
            "metamath": env_or("THEOREMATA_METAMATH", &config.metamath_bin),
            "secondary": std::env::var("THEOREMATA_METAMATH_SECONDARY").ok(),
        }),
    };
    json!({
        "schema": "theoremata.checker-identity.v1",
        "system": system.as_str(),
        "mode": if backend.is_mock() { "mock" } else { "live" },
        "runner": config.formal_runners.for_system(system).tag(),
        "binaries": binaries,
        "expected_toolchain": backend.expected_toolchain(),
        "lean_project": config.lean_project.as_ref().map(|p| p.display().to_string()),
        "resources": config.resources.display().to_string(),
        "cache_epoch": std::env::var("THEOREMATA_CHECKER_CACHE_EPOCH").ok(),
    })
    .to_string()
}

/// Fingerprint policy that can change gate acceptance independently of the
/// candidate text or checker installation.
fn policy_fingerprint(config: &Config, backend: &dyn FormalBackend) -> String {
    let limits = crate::prover::exec::ResourceLimits::from_env();
    json!({
        "schema": "theoremata.formal-gate-policy.v1",
        "crate_version": env!("CARGO_PKG_VERSION"),
        "system": backend.system().as_str(),
        "axiom_whitelist": backend.system().axiom_whitelist(),
        "success_signal": format!("{:?}", backend.compile_success_signal()),
        "kernel_validate_proof": config.kernel_validate_proof,
        "timeout_secs": limits.timeout.as_secs(),
        "max_output_bytes": limits.max_output_bytes,
        "source_policy": "mandatory-scan+statement-preservation+no-suggestion-tactics",
    })
    .to_string()
}

/// Ask the `hammer` worker (Sledgehammer / CoqHammer / aesop) to FIND a proof of
/// `goal` in `system`, and — if it returns a kernel-checked `reconstructed_tactic`
/// — assemble a complete, system-native proof around it (see [`assemble_proof`]).
///
/// Returns `None` when the worker is unavailable, the hammer finds nothing, or the
/// tactic is empty. Never errors: a failed hammer just means "skip this candidate".
///
/// The mode is auto-resolved by the worker (live when the toolchain is probeable,
/// else mock), except that `config.prover_mock` forces the offline mock hammer so
/// mock-mode callers stay deterministic.
pub fn hammer_prove(config: &Config, system: FormalSystem, goal: &str) -> Option<String> {
    use crate::tools::{PythonCheck, Tool};
    let py = PythonCheck::new();
    if !py.available() {
        return None;
    }
    // `null` mode = auto (live if the toolchain is present); force `mock` only when
    // the whole prover is pinned to mock so offline runs are deterministic.
    let mode: Option<&str> = if config.prover_mock {
        Some("mock")
    } else {
        None
    };
    let result = py
        .run(json!({
            "tool": "hammer",
            "system": system.as_str(),
            "goal": goal,
            "mode": mode,
        }))
        .ok()?;
    // The worker wraps the tool result: `{"ok": true, "output": {<hammer dict>}}`.
    let v: Value = serde_json::from_str(&result.stdout).ok()?;
    if !v.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    let output = v.get("output")?;
    if !output
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }
    let tactic = output
        .get("reconstructed_tactic")
        .and_then(Value::as_str)?
        .trim();
    if tactic.is_empty() {
        return None;
    }
    Some(assemble_proof(system, goal, tactic))
}

/// Splice a hammer's reconstructed `tactic` into a complete, verifiable,
/// system-native proof of `goal`.
///
/// * Isabelle — the tactic is already a full Isar method (`by (metis …)` /
///   `by auto` / `using … by …`), so it drops straight after the goal:
///   `theory T imports Main begin theorem t: "<goal>" <tactic> end`.
/// * Rocq — the tactic is a proof-script body (`sauto` / `hauto use: …`); wrap it
///   in `Theorem t : <goal>. Proof. <tactic>. Qed.` (trailing `.` de-duplicated so
///   a tactic that already ends in `.` is not doubled).
/// * Lean — the tactic is a tactic-block body (`aesop`, `simp`, …):
///   `theorem t : <goal> := by <tactic>`.
pub fn assemble_proof(system: FormalSystem, goal: &str, tactic: &str) -> String {
    let goal = goal.trim();
    let tactic = tactic.trim();
    match system {
        FormalSystem::Isabelle => {
            format!("theory T\n  imports Main\nbegin\n\ntheorem t: \"{goal}\"\n  {tactic}\n\nend\n")
        }
        FormalSystem::Rocq => {
            // The Rocq reconstruction is a tactic script; normalize a single
            // terminating `.` so `sauto` and `sauto.` both yield one period.
            let body = tactic.trim_end_matches('.').trim_end();
            format!("Theorem t : {goal}.\nProof.\n  {body}.\nQed.\n")
        }
        FormalSystem::Lean => format!("theorem t : {goal} := by\n  {tactic}\n"),
        // Candle/HOL Light: an OCaml let-binding whose body is a `prove` call
        // combining the goal term with the tactic script.
        FormalSystem::Candle => format!("let t = prove(`{goal}`,\n  {tactic});;\n"),
        FormalSystem::Agda => format!("module Generated where\n\n-- {goal}\n{tactic}\n"),
        FormalSystem::Metamath => format!("$c {goal} $.\n"),
    }
}

/// Produce ONE candidate proof: ask the provider (system-specific role/task), or
/// fall back to a system-native trivially-true stub when offline.
fn generate_once(
    provider: &dyn ModelProvider,
    system: FormalSystem,
    statement: &str,
) -> Result<String> {
    if provider.name() == "offline" {
        return Ok(stub_for(system));
    }
    let response = provider.complete(&ModelRequest {
        role: role_for(system).into(),
        task: task_for(system, statement),
        context: json!({ "statement": statement, "system": system.as_str() }),
        output_schema: json!({
            "type": "object",
            "required": ["code"],
            "properties": { "code": { "type": "string" } }
        }),
    })?;
    // Lenient extraction: accept `code`, or the system-native field name a model
    // might use (`lean` / `proof` / `source`).
    let content = &response.content;
    for key in ["code", "lean", "proof", "source"] {
        if let Some(s) = content[key].as_str() {
            if !s.trim().is_empty() {
                return Ok(s.to_owned());
            }
        }
    }
    anyhow::bail!("model response for {system} proof generation had no `code` field")
}

/// The system-specific generator role.
fn role_for(system: FormalSystem) -> &'static str {
    match system {
        FormalSystem::Lean => "lean_proof_generator",
        FormalSystem::Rocq => "rocq_proof_generator",
        FormalSystem::Isabelle => "isabelle_proof_generator",
        FormalSystem::Candle => "candle_proof_generator",
        FormalSystem::Agda => "agda_proof_generator",
        FormalSystem::Metamath => "metamath_proof_generator",
    }
}

/// The system-specific generation instruction.
fn task_for(system: FormalSystem, statement: &str) -> String {
    let (lang, banned) = match system {
        FormalSystem::Lean => ("Lean 4", "sorry, admit, or unsafe axioms"),
        FormalSystem::Rocq => ("Coq (Rocq)", "admit, Admitted, or bare Axiom"),
        FormalSystem::Isabelle => ("Isabelle/Isar", "sorry, oops, or an oracle"),
        FormalSystem::Candle => ("HOL Light (Candle)", "mk_thm or new_axiom"),
        FormalSystem::Agda => ("Agda", "postulate, unsafe, or unsolved metas"),
        FormalSystem::Metamath => (
            "Metamath",
            "unverified proof shortcuts or malformed $p declarations",
        ),
    };
    format!(
        "Write a complete, self-contained {lang} proof of: {statement}. \
         Output only the proof source. Never use {banned}, or any other unsound \
         escape hatch."
    )
}

/// A system-native trivially-true stub used offline (no model). It passes the
/// mock backend's canned kernel layers and the real source scan, but is only
/// `lexically_verified` when the statement is itself trivial.
fn stub_for(system: FormalSystem) -> String {
    match system {
        FormalSystem::Lean => "theorem generated : True := trivial\n".into(),
        FormalSystem::Rocq => {
            "Theorem generated : True.\nProof.\n  exact I.\nQed.\n".into()
        }
        FormalSystem::Isabelle => "theory Scratch\n  imports Main\nbegin\n\n\
             theorem generated: \"True\"\n  by simp\n\nend\n"
            .into(),
        // A trivially-true HOL Light theorem: `TRUTH : thm = |- T`.
        FormalSystem::Candle => "let generated = TRUTH;;\n".into(),
        FormalSystem::Agda => "module Generated where\n\nopen import Agda.Builtin.Unit\n\ngenerated : ⊤\ngenerated = tt\n".into(),
        FormalSystem::Metamath => "$c wff |- $.\n$v ph $.\nph $f wff ph $.\n$( trivial mock artifact $)\n".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelResponse;
    use std::cell::Cell;
    use std::path::Path;

    /// A mock provider that returns a caller-chosen proof body for every role.
    struct CannedProvider {
        code: String,
    }
    impl ModelProvider for CannedProvider {
        fn complete(&self, _req: &ModelRequest) -> Result<ModelResponse> {
            Ok(ModelResponse {
                content: json!({ "code": self.code }),
                model: "mock".into(),
                provider: "mock".into(),
            })
        }
        fn name(&self) -> &str {
            // Non-"offline" so `generate_once` exercises the provider path.
            "command"
        }
    }

    fn store() -> Store {
        Store::open(Path::new(":memory:")).unwrap()
    }

    fn mock_config() -> Config {
        // Force the mock backend (no toolchain assumed) and mock prover mode.
        Config {
            prover_mock: true,
            ..Config::default()
        }
    }

    #[test]
    fn preparation_generates_ordered_candidates_without_invoking_a_checker() {
        let config = mock_config();
        let provider = CannedProvider {
            code: "theorem t : True := trivial".into(),
        };

        let prepared = generate_candidates_for_verification(
            &config,
            &provider,
            FormalSystem::Lean,
            "True",
            false,
        )
        .unwrap();

        assert_eq!(prepared.attempts, N);
        assert!(
            !prepared.hammer_candidate,
            "mock preparation never invokes hammer"
        );
        assert_eq!(prepared.candidates.len(), N);
        assert!(prepared
            .candidates
            .iter()
            .all(|candidate| candidate == "theorem t : True := trivial"));
    }

    fn live_verified_report(marker: &str) -> VerificationReport {
        VerificationReport {
            lexically_verified: true,
            axioms_clean: true,
            statement_preserved: true,
            lexical_clean: true,
            hardening_clean: Some(true),
            live: true,
            detail: json!({"marker": marker}),
        }
    }

    fn failed_report() -> VerificationReport {
        VerificationReport {
            lexically_verified: false,
            axioms_clean: false,
            statement_preserved: false,
            lexical_clean: false,
            hardening_clean: Some(false),
            live: true,
            detail: json!({"marker": "failed"}),
        }
    }

    fn mock_verified_report() -> VerificationReport {
        VerificationReport {
            live: false,
            ..live_verified_report("mock")
        }
    }

    #[test]
    fn exact_live_success_is_reused_without_reinvoking_the_verifier() {
        let cache = CheckerCache::new();
        let context = vec!["h : P".to_string()];
        let key = VerificationCacheKey {
            system: FormalSystem::Lean,
            canonical_statement: "P",
            ordered_context: &context,
            proof_source: "theorem p : P := h",
            checker_identity: "lean:live:v4.19",
            policy_fingerprint: "gate-v1",
            import_manifest: &[],
        };
        let calls = Cell::new(0);

        let (first, first_hit) = verify_candidate(Some(&cache), &key, || {
            calls.set(calls.get() + 1);
            Ok(live_verified_report("fresh"))
        })
        .unwrap();
        let (second, second_hit) = verify_candidate(Some(&cache), &key, || {
            calls.set(calls.get() + 1);
            Ok(live_verified_report("must-not-run"))
        })
        .unwrap();

        assert!(!first_hit);
        assert!(second_hit);
        assert_eq!(calls.get(), 1);
        assert_eq!(first.detail, second.detail);
    }

    #[test]
    fn absent_cache_preserves_verifier_behavior() {
        let context = Vec::new();
        let key = VerificationCacheKey {
            system: FormalSystem::Lean,
            canonical_statement: "True",
            ordered_context: &context,
            proof_source: "theorem t : True := trivial",
            checker_identity: "lean:live",
            policy_fingerprint: "gate-v1",
            import_manifest: &[],
        };
        let calls = Cell::new(0);
        for marker in ["first", "second"] {
            let (report, hit) = verify_candidate(None, &key, || {
                calls.set(calls.get() + 1);
                Ok(live_verified_report(marker))
            })
            .unwrap();
            assert!(!hit);
            assert_eq!(report.detail["marker"], marker);
        }
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn failures_and_mock_successes_are_rechecked_not_cached() {
        let cache = CheckerCache::new();
        let context = Vec::new();
        let live_key = VerificationCacheKey {
            system: FormalSystem::Lean,
            canonical_statement: "False",
            ordered_context: &context,
            proof_source: "theorem bad : False := candidate",
            checker_identity: "lean:live",
            policy_fingerprint: "gate-v1",
            import_manifest: &[],
        };
        let mock_key = VerificationCacheKey {
            checker_identity: "lean:mock",
            ..live_key
        };
        let calls = Cell::new(0);

        for _ in 0..2 {
            let (_, hit) = verify_candidate(Some(&cache), &live_key, || {
                calls.set(calls.get() + 1);
                Ok(failed_report())
            })
            .unwrap();
            assert!(!hit);
        }
        for _ in 0..2 {
            let (_, hit) = verify_candidate(Some(&cache), &mock_key, || {
                calls.set(calls.get() + 1);
                Ok(mock_verified_report())
            })
            .unwrap();
            assert!(!hit);
        }
        assert_eq!(calls.get(), 4);
        assert!(cache.is_empty());
    }

    #[test]
    fn checker_identity_and_policy_separate_mode_and_gate_configuration() {
        let base = Config::default();
        let live = backend_for(&base, FormalSystem::Lean, false);
        let mock_backend = backend_for(&base, FormalSystem::Lean, true);
        assert_ne!(
            checker_identity(&base, live.as_ref()),
            checker_identity(&base, mock_backend.as_ref())
        );

        let stricter = Config {
            kernel_validate_proof: !base.kernel_validate_proof,
            ..base.clone()
        };
        assert_ne!(
            policy_fingerprint(&base, live.as_ref()),
            policy_fingerprint(&stricter, live.as_ref())
        );
    }

    #[test]
    fn real_mock_candidate_path_never_populates_the_checker_cache() {
        let store = store();
        let config = mock_config();
        let cache = CheckerCache::new();
        let provider = CannedProvider {
            code: "theorem t : True := trivial".into(),
        };
        let (_, report) = generate_and_verify_with_cache(
            &store,
            &config,
            &provider,
            FormalSystem::Lean,
            "True",
            &[],
            Some(&cache),
        )
        .unwrap();
        assert!(report.lexically_verified);
        assert!(!report.live);
        assert!(cache.is_empty());
    }

    #[test]
    fn returns_code_and_report_for_each_system() {
        let store = store();
        let config = mock_config();
        for (system, code) in [
            (FormalSystem::Lean, "theorem t : True := trivial"),
            (
                FormalSystem::Rocq,
                "Theorem t : True.\nProof. exact I. Qed.",
            ),
            (
                FormalSystem::Isabelle,
                "theory Scratch\n imports Main\nbegin\ntheorem t: \"True\" by simp\nend",
            ),
        ] {
            let provider = CannedProvider { code: code.into() };
            let (out, report) =
                generate_and_verify(&store, &config, &provider, system, "True").unwrap();
            assert!(!out.trim().is_empty(), "{system}: empty code");
            // The mock backend's canned kernel layers + a clean source scan on a
            // trivial statement verify cleanly.
            assert!(report.lexically_verified, "{system}: expected clean verify");
        }
    }

    #[test]
    fn sorry_or_admitted_candidate_is_not_accepted() {
        let store = store();
        let config = mock_config();
        // Each carries a system-native escape hatch the REAL source scan catches
        // even though the mock backend's kernel layers are canned-clean.
        for (system, code) in [
            (FormalSystem::Lean, "theorem t : True := by sorry"),
            (
                FormalSystem::Rocq,
                "Theorem t : True.\nProof.\n  exact I.\nAdmitted.",
            ),
            (
                FormalSystem::Isabelle,
                "theory Scratch\n imports Main\nbegin\ntheorem t: \"True\" sorry\nend",
            ),
        ] {
            let provider = CannedProvider { code: code.into() };
            let (out, report) =
                generate_and_verify(&store, &config, &provider, system, "True").unwrap();
            assert!(!out.trim().is_empty());
            assert!(
                !report.lexically_verified,
                "{system}: escape-hatch proof must NOT be accepted"
            );
            assert!(
                !report.lexical_clean,
                "{system}: source scan must flag the escape hatch"
            );
        }
    }

    #[test]
    fn offline_yields_a_stub_and_still_verifies_via_mock_backend() {
        let store = store();
        let config = mock_config();
        let (code, report) = generate_and_verify(
            &store,
            &config,
            &crate::provider::OfflineProvider,
            FormalSystem::Lean,
            "True",
        )
        .unwrap();
        assert!(code.contains("trivial"));
        assert!(report.lexically_verified);
    }

    /// A failed report whose compile layer carries real Lean checker output, in
    /// exactly the shape `FormalBackend::verify` publishes it.
    fn failed_report_with_compile(stderr: &str) -> VerificationReport {
        VerificationReport {
            detail: json!({
                "system": "lean",
                "gate": "3+1-layer",
                "compile": {
                    "compiled": false,
                    "errors": [stderr, ""],
                    "per_unit": [],
                    "detail": {"runner": "direct", "code": 1, "stdout": "", "stderr": stderr},
                },
            }),
            ..failed_report()
        }
    }

    #[test]
    fn failed_verification_gains_rendered_checker_feedback() {
        let code = "theorem foo (n : Nat) : n + 0 = n := by\n  exact bogus\n";
        let mut report =
            failed_report_with_compile("Generated.lean:2:8: error: unknown identifier 'bogus'");
        attach_error_feedback(FormalSystem::Lean, code, &mut report);

        let fb = &report.detail[ERROR_FEEDBACK_KEY];
        assert_eq!(fb["schema"], "theoremata.error-feedback.v1");
        assert_eq!(fb["parsed"], true, "a standard Lean header must parse");
        let text = fb["text"].as_str().expect("text is a string");
        assert!(!text.is_empty());
        // The checker's own words AND the offending source both reach the model.
        assert!(text.contains("unknown identifier 'bogus'"), "{text}");
        assert!(text.contains("<error>bogus</error>"), "{text}");
        assert_eq!(fb["diagnostics"][0]["line"], 2);
        // Verdict fields are untouched: this is presentation only.
        assert!(!report.lexically_verified);
        assert_eq!(report.detail["compile"]["compiled"], false);
    }

    #[test]
    fn passing_verification_detail_is_unchanged() {
        let mut report = live_verified_report("clean");
        let before = report.detail.clone();
        attach_error_feedback(FormalSystem::Lean, "theorem t : True := trivial", &mut report);
        assert_eq!(report.detail, before, "a pass must never gain feedback");
        assert!(report.detail.get(ERROR_FEEDBACK_KEY).is_none());
        assert!(report.lexically_verified);
    }

    #[test]
    fn absent_or_unparseable_checker_text_never_panics() {
        // No `compile` key at all (mock-shaped detail).
        let mut bare = failed_report();
        attach_error_feedback(FormalSystem::Lean, "", &mut bare);
        assert!(!bare.detail[ERROR_FEEDBACK_KEY]["text"]
            .as_str()
            .unwrap()
            .is_empty());
        assert_eq!(bare.detail[ERROR_FEEDBACK_KEY]["parsed"], false);

        // A `compile` layer present but carrying no checker words.
        let mut empty = failed_report_with_compile("");
        attach_error_feedback(FormalSystem::Rocq, "Theorem t : True.", &mut empty);
        assert_eq!(empty.detail[ERROR_FEEDBACK_KEY]["parsed"], false);

        // Garbage falls back to the module's bounded raw passthrough.
        let mut noisy = failed_report_with_compile("\u{1f4a5} segfault at 0xdeadbeef");
        attach_error_feedback(FormalSystem::Lean, "theorem t : True := trivial", &mut noisy);
        let text = noisy.detail[ERROR_FEEDBACK_KEY]["text"].as_str().unwrap();
        assert!(text.contains("segfault at 0xdeadbeef"), "{text}");

        // A non-object `detail` is left exactly as it was.
        let mut scalar = VerificationReport {
            detail: json!("opaque"),
            ..failed_report()
        };
        attach_error_feedback(FormalSystem::Lean, "", &mut scalar);
        assert_eq!(scalar.detail, json!("opaque"));
    }

    #[test]
    fn raw_checker_output_falls_back_to_the_errors_array() {
        // Backends that leave `detail` empty still duplicate the text into
        // `errors`; that must not be lost.
        let compile = json!({
            "compiled": false,
            "errors": ["", "Generated.lean:1:1: error: fallback path"],
            "detail": {"runner": "direct"},
        });
        let raw = raw_checker_output(Some(&compile));
        assert_eq!(raw, "Generated.lean:1:1: error: fallback path");
        assert!(raw_checker_output(None).is_empty());
    }

    #[test]
    fn a_rejected_candidate_carries_feedback_through_the_real_generate_path() {
        // End-to-end through generate_and_verify: the mock backend's source scan
        // rejects `sorry`, and the returned report must carry the feedback key.
        let store = store();
        let config = mock_config();
        let provider = CannedProvider {
            code: "theorem t : True := by sorry".into(),
        };
        let (_, report) =
            generate_and_verify(&store, &config, &provider, FormalSystem::Lean, "True").unwrap();
        assert!(!report.lexically_verified);
        let text = report.detail[ERROR_FEEDBACK_KEY]["text"]
            .as_str()
            .expect("a rejected candidate must publish feedback text");
        assert!(!text.is_empty());
    }

    #[test]
    fn assemble_proof_is_well_formed_per_system() {
        // Isabelle: the tactic is already a full Isar method (`by …`).
        let isa = assemble_proof(FormalSystem::Isabelle, "1 + 1 = (2::nat)", "by auto");
        assert!(isa.starts_with("theory T"));
        assert!(isa.contains("imports Main"));
        assert!(isa.contains("theorem t: \"1 + 1 = (2::nat)\""));
        assert!(isa.contains("by auto"));
        assert!(isa.trim_end().ends_with("end"));

        // Rocq: a tactic-script body wrapped in Theorem/Proof/Qed with exactly
        // one terminating period (no doubling when the tactic already has one).
        let rocq = assemble_proof(FormalSystem::Rocq, "1 + 1 = 2", "sauto");
        assert!(rocq.starts_with("Theorem t : 1 + 1 = 2."));
        assert!(rocq.contains("Proof."));
        assert!(rocq.contains("  sauto.\n"));
        assert!(!rocq.contains("sauto..")); // no doubled period
        assert!(rocq.trim_end().ends_with("Qed."));
        let rocq_dotted = assemble_proof(FormalSystem::Rocq, "True", "exact I.");
        assert!(rocq_dotted.contains("  exact I.\n"));
        assert!(!rocq_dotted.contains("exact I.."));

        // Lean: a tactic-block body after `:= by`.
        let lean = assemble_proof(FormalSystem::Lean, "1 + 1 = 2", "decide");
        assert_eq!(lean, "theorem t : 1 + 1 = 2 := by\n  decide\n");
    }

    #[test]
    fn mock_hammer_assembles_a_proof_when_worker_present() {
        // With the whole prover pinned to mock, `hammer_prove` forces the offline
        // mock hammer, which returns a reconstruction for a (provable-looking)
        // goal. Guard on the Python worker being present so the suite still passes
        // where it is absent.
        use crate::tools::{PythonCheck, Tool};
        if !PythonCheck::new().available() {
            eprintln!("skip: no Python worker for the hammer tool");
            return;
        }
        let config = mock_config();
        let assembled = hammer_prove(&config, FormalSystem::Isabelle, "1 + 1 = (2::nat)");
        let code = assembled.expect("mock hammer should assemble a proof for a trivial goal");
        assert!(code.contains("theorem t: \"1 + 1 = (2::nat)\""));
        // The mock Sledgehammer reconstruction is `by (metis)`.
        assert!(code.contains("by"));
    }

    #[test]
    fn live_isabelle_hammer_finds_and_verifies_end_to_end() {
        use crate::prover::formal::FormalBackend;
        use crate::tools::{PythonCheck, Tool};
        if !PythonCheck::new().available() {
            eprintln!("skip: no Python worker for the hammer tool");
            return;
        }
        let config = Config::default();
        let backend = crate::prover::isabelle::IsabelleBackend::live(&config);
        if !backend.available() {
            eprintln!("skip: no live Isabelle toolchain");
            return;
        }
        // Live (auto) mode: Sledgehammer FINDS a tactic; we assemble a native
        // theory and the live 3+1-layer gate must VERIFY it end-to-end.
        let goal = "1 + 1 = (2::nat)";
        let code = match hammer_prove(&config, FormalSystem::Isabelle, goal) {
            Some(c) => c,
            None => {
                eprintln!("skip: live hammer produced no reconstruction");
                return;
            }
        };
        let report = backend.verify(&config, &code, goal).unwrap();
        assert!(
            report.lexically_verified,
            "live Isabelle Sledgehammer-assisted proof should verify:\n{code}\n{report:?}"
        );
    }

    #[test]
    fn live_lean_generates_and_verifies_when_toolchain_present() {
        use crate::prover::formal::FormalBackend;
        let config = Config::default();
        let backend = crate::prover::lean::LeanBackend::live(&config);
        if !backend.available() {
            eprintln!("skip: no live Lean toolchain");
            return;
        }
        let store = store();
        // Offline provider → trivially-true Lean stub; the live gate compiles it.
        let (code, report) = generate_and_verify(
            &store,
            &config,
            &crate::provider::OfflineProvider,
            FormalSystem::Lean,
            "True",
        )
        .unwrap();
        assert!(code.contains("trivial"));
        assert!(
            report.lexically_verified,
            "live Lean gate should verify a trivial proof"
        );
    }
}
