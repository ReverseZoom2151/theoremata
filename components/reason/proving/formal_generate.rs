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
    checker_cache::{
        CheckerCache, EnvironmentFingerprint, VerificationCacheKey, ELABORATED_STATEMENT_DETAIL_KEY,
    },
    config::Config,
    db::Store,
    model::ModelRequest,
    prover::{
        error_feedback::{render_feedback, FeedbackConfig},
        formal::{backend_for, FormalBackend, FormalSystem},
        model::VerificationReport,
        subgoal_extract::{extract_subgoals, to_obligations},
    },
    provider::ModelProvider,
    sampling,
};
use anyhow::{Context, Result};
// The staleness classifier is pure logic with no callers yet. We do not call it
// here (this module verifies, it does not sweep); we import its vocabulary so the
// provenance we record is already in the shape it consumes.
use crate::reason::proving::staleness;
use serde_json::{json, Value};
use std::cell::RefCell;

/// How many candidate proofs to sample before giving up (best-of-N).
const N: usize = 3;

/// Default number of CORRECTION rounds run after an all-failed initial round.
///
/// Deliberately `0`: correction is strictly opt-in, so the established
/// generate/verify path behaves exactly as it did before this loop existed.
/// Operators enable it with `THEOREMATA_FORMAL_CORRECTION_ROUNDS` (the mined
/// system that measured a gain used 2 rounds).
const DEFAULT_MAX_CORRECTION_ROUNDS: usize = 0;

/// Default sample budget for EACH correction round, once enabled.
///
/// Correction rounds are deliberately much cheaper than round 0 (the mined
/// system used 8 initial samples and 2 per correction round): a corrected
/// candidate starts from concrete checker diagnostics, so it does not need the
/// same blind-sampling width.
const DEFAULT_CORRECTION_SAMPLES: usize = 2;

/// Budget for the feedback-driven correction loop.
///
/// `max_rounds == 0` disables the loop entirely and is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CorrectionConfig {
    /// How many correction rounds may run after an all-failed initial round.
    max_rounds: usize,
    /// How many candidates each correction round may sample.
    samples_per_round: usize,
}

impl Default for CorrectionConfig {
    fn default() -> Self {
        Self {
            max_rounds: DEFAULT_MAX_CORRECTION_ROUNDS,
            samples_per_round: DEFAULT_CORRECTION_SAMPLES,
        }
    }
}

impl CorrectionConfig {
    /// Read the opt-in from the environment.
    ///
    /// This is read locally rather than from [`Config`] on purpose: the field is
    /// not yet part of the shared config struct. See the module report for the
    /// preferred name (`Config::formal_correction_rounds`).
    fn from_env() -> Self {
        let default = Self::default();
        Self {
            max_rounds: usize_from_env("THEOREMATA_FORMAL_CORRECTION_ROUNDS", default.max_rounds),
            samples_per_round: usize_from_env(
                "THEOREMATA_FORMAL_CORRECTION_SAMPLES",
                default.samples_per_round,
            ),
        }
    }

    /// Whether any correction work is possible under this budget.
    fn enabled(&self) -> bool {
        self.max_rounds > 0 && self.samples_per_round > 0
    }
}

/// Parse a non-negative integer setting, falling back on absent/blank/invalid.
fn usize_from_env(key: &str, default: usize) -> usize {
    match std::env::var(key) {
        Ok(raw) => raw.trim().parse::<usize>().unwrap_or(default),
        Err(_) => default,
    }
}

/// A rejected candidate plus the checker feedback rendered for it.
#[derive(Debug, Clone)]
struct FailedCandidate {
    code: String,
    feedback: String,
}

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
    generate_and_verify_inner(
        store,
        config,
        provider,
        system,
        statement,
        ordered_context,
        cache,
        CorrectionConfig::from_env(),
    )
}

/// An injectable hammer seam: given `(config, system, goal)` it returns a
/// complete, system-native candidate proof, or `None` when it declines (worker
/// unavailable, found nothing, or gated off). Production wires this to
/// [`hammer_prove`] guarded by the live backend; tests inject a deterministic
/// stand-in so the fallback's control flow can be exercised without a toolchain.
type HammerSeam<'a> = dyn Fn(&Config, FormalSystem, &str) -> Option<String> + 'a;

/// Implementation of [`generate_and_verify_with_cache`] with an explicit
/// correction budget, so tests can exercise the loop without touching global
/// process environment state.
///
/// With `correction.max_rounds == 0` (the default) this is exactly the original
/// single-round best-of-N: same generation count, same selection, same event.
#[allow(clippy::too_many_arguments)]
fn generate_and_verify_inner(
    store: &Store,
    config: &Config,
    provider: &dyn ModelProvider,
    system: FormalSystem,
    statement: &str,
    ordered_context: &[String],
    cache: Option<&CheckerCache>,
    correction: CorrectionConfig,
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
    // The production hammer seam. Gated on the LIVE backend: only a real
    // 3+1-layer gate keeps a hammer-found proof honest. Under the mock backend
    // the "verification" is canned, so a mock hammer would fabricate a clean
    // pass; return `None` there so the fallback never runs and can never fake.
    let hammer = move |cfg: &Config, sys: FormalSystem, goal: &str| -> Option<String> {
        if used_live {
            hammer_prove(cfg, sys, goal)
        } else {
            None
        }
    };
    generate_and_verify_core(
        store,
        config,
        provider,
        system,
        statement,
        ordered_context,
        cache,
        correction,
        backend.as_ref(),
        used_live,
        &hammer,
    )
}

/// The generate/verify body, factored out so the hammer fallback (below) can run
/// against an injectable `hammer` seam. The seam is attempted ONLY after every
/// model attempt failed the gate, so a model success never triggers or pays for
/// it; see the fallback block for the full soundness argument.
#[allow(clippy::too_many_arguments)]
fn generate_and_verify_core(
    store: &Store,
    config: &Config,
    provider: &dyn ModelProvider,
    system: FormalSystem,
    statement: &str,
    ordered_context: &[String],
    cache: Option<&CheckerCache>,
    correction: CorrectionConfig,
    backend: &dyn FormalBackend,
    used_live: bool,
    hammer: &HammerSeam,
) -> Result<(String, VerificationReport)> {
    let checker_identity = checker_identity(config, backend);
    let policy_fingerprint = policy_fingerprint(config, backend);
    // The import closure this verification runs under. A cached PASS is only
    // meaningful relative to an environment: a proof verified under an import
    // list that smuggles in an axiom must never share a cache slot with one
    // verified under a clean list.
    let import_manifest = system.default_imports();
    // The import NAMES above say `Mathlib`; they do not say WHICH Mathlib. This
    // resolves the environment those names actually bind to (for Lean: the
    // lake-manifest revisions and the toolchain pin beside them) so that updating
    // a library in place, at the same path under the same Lean, is a mandatory
    // cache miss rather than a silent reuse of a verdict earned against the old
    // one. When it cannot be resolved the key becomes unusable and the cache goes
    // quiet for this run, which is the safe direction: an extra verification, not
    // a stale green.
    let environment = EnvironmentFingerprint::resolve(
        system,
        backend.is_mock(),
        config.lean_project.as_deref().filter(|p| p.exists()),
    );

    // Every rejected candidate, recorded with its rendered checker feedback so a
    // later correction round can generate against the checker's own words. This
    // is generation-side only: it never reaches `accept` below.
    let failures: RefCell<Vec<FailedCandidate>> = RefCell::new(Vec::new());

    // THE GATE. Identical for an initial candidate and a corrected one: the same
    // key, the same cache policy, the same `FormalBackend::verify` call. Feedback
    // influences only what source we hand the checker, never the verdict.
    let verify_one = |code: String| -> Result<Candidate> {
        let key = VerificationCacheKey {
            system,
            canonical_statement: statement,
            ordered_context,
            proof_source: &code,
            checker_identity: &checker_identity,
            policy_fingerprint: &policy_fingerprint,
            import_manifest: &import_manifest,
            environment: &environment,
        };
        let (mut report, cache_hit) =
            verify_candidate(cache, &key, || backend.verify(config, &code, statement))?;
        attach_error_feedback(system, &code, &mut report);
        attach_declaration_hints(system, config, &mut report);
        // Record WHAT this verdict was earned against, on the verdict itself.
        // Until now the environment was computed for the cache key and dropped,
        // so no stored result could ever be assessed for staleness. Placed after
        // the cache call for the same reason the annotations above are: the cache
        // stores the gate's own report, and provenance must not enter its
        // accept/reject decision or its key.
        attach_verification_provenance(system, statement, &code, &environment, &mut report);
        if !report.lexically_verified {
            failures.borrow_mut().push(FailedCandidate {
                code: code.clone(),
                feedback: feedback_text(&report).unwrap_or_default(),
            });
        }
        Ok(Candidate {
            code,
            report,
            cache_hit,
        })
    };

    // Round 0 is model-only best-of-N. The hammer is no longer a positive
    // candidate here; it runs strictly as a fallback below and only if all of
    // these fail, so a model success never triggers or pays for it.
    let selection = sampling::best_of_n(
        N,
        |_i| -> Result<Candidate> {
            let code = generate_once(provider, system, statement)?;
            verify_one(code)
        },
        |c: &Candidate| c.report.lexically_verified,
    )?;

    let mut sampled = selection.context("no proof candidate could be generated")?;
    let mut attempts = sampled.attempts;
    let mut rounds_run = 0usize;
    let mut corrected_accepted = false;

    // CORRECTION ROUNDS. Retire-on-any-sibling-pass: entered only when the whole
    // initial round failed, and abandoned the moment any candidate verifies, so
    // no correction budget is ever spent on an already-solved problem.
    if correction.enabled() && !sampled.accepted {
        for _ in 0..correction.max_rounds {
            // Per-variant branching: correct each DISTINCT failed candidate, not
            // just the last one, round-robin across this round's budget.
            let variants = distinct_failures(&failures.borrow());
            if variants.is_empty() {
                break;
            }
            rounds_run += 1;
            let round = sampling::best_of_n(
                correction.samples_per_round,
                |i| -> Result<Candidate> {
                    let variant = &variants[i % variants.len()];
                    let prompt = correction_feedback(system, variant);
                    let code =
                        generate_once_with_feedback(provider, system, statement, Some(&prompt))?;
                    verify_one(code)
                },
                |c: &Candidate| c.report.lexically_verified,
            )?;
            let Some(round) = round else {
                // Every generation in this round errored; keep the incumbent
                // fallback and stop spending budget.
                break;
            };
            attempts += round.attempts;
            let accepted = round.accepted;
            // The corrected candidate supersedes the fallback either way: it is
            // the most recent, best-informed attempt.
            sampled.index = round.index;
            sampled.accepted = accepted;
            sampled.value = round.value;
            if accepted {
                corrected_accepted = true;
                break;
            }
        }
    }

    // HAMMER FALLBACK. Reached only when every model attempt (round 0 plus any
    // correction rounds) failed the gate. We give the SAME goal one more real
    // attempt through the SAME gate: the hammer worker (Sledgehammer / CoqHammer
    // / aesop) proposes a candidate and `verify_one` runs the identical
    // key/policy/`backend.verify` path every model candidate ran.
    //
    // Soundness. This can only turn a failure into a gate-accepted success:
    //   * it runs solely when `!sampled.accepted`, so a model success is untouched
    //     and pays nothing;
    //   * the incumbent is replaced ONLY when the hammer candidate itself reports
    //     `lexically_verified` (the same gate accepted it), never merely because a
    //     hammer proof was produced;
    //   * any error from the seam or the gate is swallowed, leaving the original
    //     model failure exactly as it was (fail closed);
    //   * the production seam yields `None` unless the LIVE backend is in use, so a
    //     canned mock verdict can never bless a hammer proof.
    // Nothing here can downgrade an accepted verdict or synthesize one.
    let mut hammer_attempted = false;
    let mut hammer_accepted = false;
    if !sampled.accepted {
        if let Some(hammer_code) = hammer(config, system, statement) {
            hammer_attempted = true;
            attempts += 1;
            // Swallow any gate error: the model failure must stand unchanged.
            if let Ok(candidate) = verify_one(hammer_code) {
                if candidate.report.lexically_verified {
                    hammer_accepted = true;
                    // The hammer candidate is not one of the N model slots; record
                    // it in the slot just past them so `index` stays honest.
                    sampled.index = N;
                    sampled.accepted = true;
                    sampled.value = candidate;
                }
            }
        }
    }

    // Which path produced the artifact being returned. On a model success or a
    // both-failed outcome the returned proof is the model's, hence "model"; only a
    // gate-accepted hammer candidate earns "hammer". Recorded on the report detail
    // so a caller/UI can attribute the proof honestly.
    let proof_path = if hammer_accepted { "hammer" } else { "model" };
    attach_proof_path(&mut sampled.value.report, proof_path, hammer_attempted);

    let mut event = json!({
        "system": system.as_str(),
        "accepted": sampled.accepted,
        "attempts": attempts,
        "index": sampled.index,
        "verified": sampled.value.report.lexically_verified,
        "backend": if used_live { "live" } else { "mock" },
        // A hammer fallback was tried (the model round(s) failed). Retains the old
        // key name; its meaning is now "the fallback ran" rather than "a hammer
        // candidate was included in best-of-N".
        "hammer_candidate": hammer_attempted,
        "hammer_accepted": hammer_accepted,
        "proof_path": proof_path,
        "checker_cache_enabled": cache.is_some(),
        "checker_cache_hit": sampled.value.cache_hit,
        // Surfaced so an operator can see WHY a run never hits the cache: an
        // unresolved environment disables it by design, and that should be
        // visible rather than looking like bad luck.
        "checker_cache_environment": environment.describe(),
        "checker_cache_usable": environment.is_resolved(),
        // The provenance of the SELECTED candidate, so a later staleness sweep
        // can read `verified_against` per result straight off the event stream.
        // Unconditional: a report whose `detail` is not an object cannot carry
        // the same key, and a silently missing environment is the one omission
        // that produces a false `Fresh` later.
        PROVENANCE_KEY: provenance_value(
            system,
            statement,
            &sampled.value.code,
            &environment,
            &sampled.value.report,
        ),
    });
    // Additive only when the opt-in is on, so the default event payload is
    // byte-identical to the pre-correction one.
    if correction.enabled() {
        if let Some(obj) = event.as_object_mut() {
            obj.insert("correction_rounds".into(), json!(rounds_run));
            obj.insert("correction_budget".into(), json!(correction.max_rounds));
            obj.insert(
                "correction_samples".into(),
                json!(correction.samples_per_round),
            );
            obj.insert("corrected_accepted".into(), json!(corrected_accepted));
        }
    }
    store.event(
        None,
        None,
        "formal_generate.completed",
        system.as_str(),
        event,
    )?;

    Ok((sampled.value.code, sampled.value.report))
}

/// Deduplicate recorded failures by source text, preserving generation order and
/// keeping only those that actually carry checker feedback worth acting on.
fn distinct_failures(failures: &[FailedCandidate]) -> Vec<FailedCandidate> {
    let mut seen: Vec<&str> = Vec::new();
    let mut out: Vec<FailedCandidate> = Vec::new();
    for failure in failures {
        if failure.feedback.trim().is_empty() {
            continue;
        }
        if seen.iter().any(|code| *code == failure.code.as_str()) {
            continue;
        }
        seen.push(failure.code.as_str());
        out.push(failure.clone());
    }
    out
}

/// Read the rendered feedback text published by [`attach_error_feedback`].
fn feedback_text(report: &VerificationReport) -> Option<String> {
    report
        .detail
        .get(ERROR_FEEDBACK_KEY)?
        .get("text")?
        .as_str()
        .map(str::to_owned)
}

/// Render one failed candidate into the prompt fragment a correction generation
/// is given: the rejected source plus the checker's own diagnostics.
fn correction_feedback(system: FormalSystem, failure: &FailedCandidate) -> String {
    format!(
        "Your previous {system} attempt was REJECTED by the checker.\n\n\
         Previous attempt:\n```\n{code}\n```\n\n\
         Checker diagnostics:\n{feedback}\n\n\
         Fix the specific errors above and output a complete corrected proof. \
         Do not restate the errors; do not weaken or restate the theorem.",
        system = system.as_str(),
        code = failure.code.trim(),
        feedback = failure.feedback.trim(),
    )
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

// ===========================================================================
// Verification provenance (what a verdict was earned against)
// ===========================================================================

/// The `detail` key, and the event field, under which verification provenance is
/// published.
///
/// One name for both surfaces on purpose: a staleness sweep that reads stored
/// reports and one that replays the event stream must not have to know two
/// spellings of the same fact.
const PROVENANCE_KEY: &str = "verification_provenance";

/// Schema tag for the published provenance object.
const PROVENANCE_SCHEMA: &str = "theoremata.verification-provenance.v1";

/// Stable tag for an artifact class.
///
/// Spelled out here rather than derived from the `Debug` form so a rename in
/// `staleness` cannot silently change a value that has already been written to
/// the event log.
fn artifact_tag(class: staleness::ArtifactClass) -> &'static str {
    match class {
        staleness::ArtifactClass::SelfContainedCertificate => "self_contained_certificate",
        staleness::ArtifactClass::TacticScript => "tactic_script",
        staleness::ArtifactClass::ProofTerm => "proof_term",
    }
}

/// Classify the artifact this generator produced, for staleness routing.
///
/// Only two of the three classes are reachable from here, and that is the honest
/// answer rather than a limitation: this module asks a model (or a hammer) for
/// system-native proof SOURCE, which is a program against the library in every
/// system it targets. Nothing on this path produces a self-contained certificate
/// (an SOS witness, an LRAT refutation, an exact-rational bound), so nothing on
/// this path may claim the cheap `StatementOnly` recheck route that class earns.
/// Misreading a script as a certificate would skip the proof replay it needs, so
/// the certificate variant is deliberately unreachable here.
///
/// The remaining split, script versus proof term, is safe to decide
/// heuristically: both route to `StatementAndProof`, so getting it wrong costs a
/// census label and never a recheck.
fn classify_artifact(system: FormalSystem, code: &str) -> staleness::ArtifactClass {
    match system {
        // A Lean proof is a term unless it opens a tactic block.
        FormalSystem::Lean => {
            if has_word(code, "by") {
                staleness::ArtifactClass::TacticScript
            } else {
                staleness::ArtifactClass::ProofTerm
            }
        }
        // Agda proofs are terms by construction: there is no tactic language to
        // rot, only definitions that reference other definitions.
        FormalSystem::Agda => staleness::ArtifactClass::ProofTerm,
        // Isar methods, Rocq tactic scripts, HOL Light tactic combinators and
        // Metamath label sequences are all programs against a library surface.
        FormalSystem::Rocq
        | FormalSystem::Isabelle
        | FormalSystem::Candle
        | FormalSystem::Metamath => staleness::ArtifactClass::TacticScript,
    }
}

/// Whether `word` appears in `text` as a standalone identifier rather than as a
/// substring of a longer one (so `by` matches `:= by` but not `bytes`).
fn has_word(text: &str, word: &str) -> bool {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '\'' || c == '.';
    text.match_indices(word).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + word.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

/// The provenance record for one verified candidate.
///
/// Shaped so a later sweep can build a `staleness::VerifiedResult` field for
/// field, with no reinterpretation:
///
/// * `id`                    -> `VerifiedResult::id`
/// * `artifact`              -> `VerifiedResult::artifact` (tags from [`artifact_tag`])
/// * `verified_against`      -> `staleness::EnvironmentFingerprint::new(..)`
/// * `pinned_statement_type` -> `VerifiedResult::pinned_statement_type` (null = absent)
///
/// `verified_against` is [`EnvironmentFingerprint::key_field`], which is the same
/// exact string the cache key mixes in, and which spells both states. An
/// UNRESOLVED environment is therefore recorded as `unresolved:` plus the
/// verbatim reason, and is
/// never omitted: omitting it would read downstream as "this verdict depended on
/// nothing", which is a different and much more dangerous fact than "we could not
/// tell what it depended on". `environment_resolved` carries the same bit as a
/// boolean so a consumer never has to parse the prefix.
///
/// A sweep must pass `None` as `assess`'s `current_environment` whenever TODAY's
/// environment is unresolved. That is the documented contract of `staleness`
/// (unresolvable current environment yields `Unknown`), and it is what keeps two
/// unresolved-for-the-same-reason strings from comparing equal and reading as
/// `Fresh`.
///
/// `pinned_statement_type` is the checker's own elaborated form when a backend
/// published one under [`ELABORATED_STATEMENT_DETAIL_KEY`], and `null` otherwise.
/// No backend publishes it today, so this is honestly null and the sweep will say
/// `Unknown(NoPinnedStatementType)` rather than guessing. Deriving a stand-in
/// from the source text would hand the discriminator something confident and
/// wrong, which is the exact failure the pin exists to prevent.
/// The verbatim import header of a source file: every leading line up to the
/// first that is neither blank, a comment, a `prelude`, a `set_option`, nor an
/// `import`.
///
/// Verbatim rather than a parsed module list, because this is fed back into a
/// generated file to re-elaborate a pinned statement, and reconstructing a
/// header from parsed names would silently drop anything the parser did not
/// model. Lean's own header ends at the first non-header line, so stopping
/// there cannot pick up an `import` inside a string literal or a doc comment.
fn import_header_of(code: &str) -> String {
    let mut header: Vec<&str> = Vec::new();
    for raw in code.lines() {
        let line = raw.trim();
        if line.is_empty()
            || line.starts_with("--")
            || line == "prelude"
            || line.starts_with("set_option ")
            || line.starts_with("import ")
        {
            header.push(raw);
            continue;
        }
        break;
    }
    // Trailing blank lines carry no information and would only make two equal
    // headers compare unequal.
    while header.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
        header.pop();
    }
    header.join("\n")
}

fn provenance_value(
    system: FormalSystem,
    statement: &str,
    code: &str,
    environment: &EnvironmentFingerprint,
    report: &VerificationReport,
) -> Value {
    let elaborated = report
        .detail
        .get(ELABORATED_STATEMENT_DETAIL_KEY)
        .filter(|node| node.get("form").and_then(Value::as_str).is_some());
    let unresolved_reason = match environment {
        EnvironmentFingerprint::Unresolved { reason } => Some(reason.as_str()),
        EnvironmentFingerprint::Resolved { .. } => None,
    };
    json!({
        "schema": PROVENANCE_SCHEMA,
        "system": system.as_str(),
        "id": statement,
        "artifact": artifact_tag(classify_artifact(system, code)),
        "verified_against": environment.key_field(),
        "environment_resolved": environment.is_resolved(),
        // Verbatim, and only when there is one: "we did not look" and "the
        // project declares nothing" must stay distinguishable in the record.
        "environment_unresolved_reason": unresolved_reason,
        "environment_describe": environment.describe(),
        "pinned_statement_type": elaborated
            .and_then(|node| node.get("form"))
            .cloned()
            .unwrap_or(Value::Null),
        "pinned_statement_provenance": elaborated
            .and_then(|node| node.get("provenance"))
            .cloned()
            .unwrap_or(Value::Null),
        // The import header the pin was elaborated UNDER. Without it a later
        // re-elaboration has to guess a preamble, and a statement re-elaborated
        // against a different import set is not a comparison of the same thing.
        // Only recorded when a pin exists, since it exists to serve the pin.
        "pinned_statement_imports": match elaborated {
            Some(_) => Value::String(import_header_of(code)),
            None => Value::Null,
        },
        // Present exactly when the pin is absent, so a sweep can report WHY a
        // result is unassessable instead of only that it is.
        "pinned_statement_absent_reason": match elaborated {
            Some(_) => Value::Null,
            None => json!(
                "no backend published an elaborated statement form for this verification"
            ),
        },
        // The verdict this provenance describes, copied so a sweep reading the
        // event stream alone can tell a green from a red. Read-only: nothing here
        // writes back into the report's verdict fields.
        "verdict_verified": report.lexically_verified,
        "verdict_live": report.live,
    })
}

/// Publish the provenance record onto a verification report's `detail`.
///
/// Strictly additive and verdict-neutral, exactly like [`attach_error_feedback`]:
/// it writes one new key and reads nothing it could act on. It runs for passes
/// AND failures, because a sweep that only ever saw greens could not tell an
/// unrecorded result from an absent one.
///
/// A `detail` that is not a JSON object cannot carry the key; the event payload
/// is written unconditionally and is the surface that is guaranteed to carry the
/// record.
fn attach_verification_provenance(
    system: FormalSystem,
    statement: &str,
    code: &str,
    environment: &EnvironmentFingerprint,
    report: &mut VerificationReport,
) {
    let value = provenance_value(system, statement, code, environment, report);
    if let Some(detail) = report.detail.as_object_mut() {
        detail.insert(PROVENANCE_KEY.to_string(), value);
    }
}

/// The `detail` key under which the accepted proof's PATH provenance is
/// published: which generator actually produced the proof being returned, the
/// model or the hammer fallback. Named distinctly from [`PROVENANCE_KEY`] (which
/// records what a verdict was earned AGAINST) because this records who produced
/// it, and a caller/UI must be able to read either without the other.
const PROOF_PATH_KEY: &str = "proof_path";

/// Schema tag for the published proof-path record.
const PROOF_PATH_SCHEMA: &str = "theoremata.proof-path.v1";

/// Publish, onto a report's `detail`, which path produced the returned proof.
///
/// Strictly additive and verdict-neutral, like the other `attach_*` passes: it
/// writes one key and reads nothing it could act on. `path` is `"model"` or
/// `"hammer"`; `hammer_attempted` records whether the fallback ran at all, so a
/// consumer can tell "hammer was never needed" (model passed first) apart from
/// "hammer was tried and failed" (both `path == "model"`). A `detail` that is not
/// a JSON object cannot carry the key and is left untouched; the event payload is
/// the surface guaranteed to carry the record.
fn attach_proof_path(report: &mut VerificationReport, path: &str, hammer_attempted: bool) {
    if let Some(detail) = report.detail.as_object_mut() {
        detail.insert(
            PROOF_PATH_KEY.to_string(),
            json!({
                "schema": PROOF_PATH_SCHEMA,
                "path": path,
                "hammer_attempted": hammer_attempted,
            }),
        );
    }
}

/// The `detail` key under which rendered checker feedback is published.
const ERROR_FEEDBACK_KEY: &str = "error_feedback";

/// Detail key for subgoals lifted off a failed attempt. Separate from
/// [`ERROR_FEEDBACK_KEY`] so a consumer of the rendered text never has to skip
/// past obligations, and so absent-because-clean stays distinguishable.
const SUBGOALS_KEY: &str = "extracted_subgoals";

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

    // Every failed attempt exposes subgoals -- explicit holes plus the positions
    // the checker actually complained about -- and until now they were thrown
    // away with the attempt. Lifting them here costs one pass over text we have
    // already parsed.
    //
    // These are published as OBLIGATIONS, never as results. `to_obligations`
    // enters them unproved without exception; a subgoal is a hypothesis about
    // where the proof has to go, and a failed compile is not evidence that any
    // of them holds.
    let subgoals = extract_subgoals(system, code, &rendered.diagnostics);
    if !subgoals.is_empty() {
        detail.insert(
            SUBGOALS_KEY.to_string(),
            json!({
                "schema": "theoremata.extracted-subgoals.v1",
                "system": system.as_str(),
                "subgoals": serde_json::to_value(&subgoals).unwrap_or(Value::Null),
                "obligations": serde_json::to_value(to_obligations(&subgoals))
                    .unwrap_or(Value::Null),
                "proved": false,
            }),
        );
    }
}

/// Detail key under which declaration-existence hints are published.
const DECL_HINTS_KEY: &str = "declaration_hints";

/// Environment opt-in for the declaration-hint pass.
///
/// Off by default because the pass dumps the environment through the Lean
/// worker, which costs real time per failed candidate. It is a live-toolchain
/// aid, not something CI or an offline run should pay for.
const DECL_HINTS_ENV: &str = "THEOREMATA_DECL_HINTS";

/// Resolve the identifiers a failed attempt named but the checker could not
/// find, and record whether each one exists elsewhere in the library.
///
/// The point is to separate two failures the model conflates. `Nat.sub_one_lt`
/// coming back `unknown identifier` can mean the name is wrong (abandon it) or
/// that it is real but unimported (add the import and keep the branch). Only the
/// index can tell them apart, and until now nothing asked it.
///
/// Strictly advisory and strictly additive, mirroring [`attach_error_feedback`]:
/// it never touches a verdict field, never runs on a passing report, and gates
/// itself off unless a live Lean project is configured AND the operator opted
/// in. The lookup uses the FAST path only (the candidate's own import manifest),
/// never the wide-library dump, so the cost stays one bounded query per unknown
/// name rather than a full Mathlib scan.
///
/// Every non-`UnknownDeclaration` verdict is reported as-is. In particular an
/// `EnvironmentError` is published verbatim and never rewritten into "absent":
/// a broken worker is evidence of nothing, and letting it read as a missing
/// declaration is exactly the false negative the index type exists to prevent.
fn attach_declaration_hints(
    system: FormalSystem,
    config: &Config,
    report: &mut VerificationReport,
) {
    use crate::prover::declaration_lookup::{check, ImportManifest};

    if report.lexically_verified {
        return;
    }
    // Only Lean has a decl_index worker behind it today. Other systems would
    // resolve every name to ToolchainUnavailable, which is noise, not a hint.
    if system != FormalSystem::Lean {
        return;
    }
    // Both conditions are required: a real checkout to dump, and an explicit
    // opt-in, since the dump is not free.
    let Some(root) = config.lean_project.as_ref() else {
        return;
    };
    if !env_flag_on(DECL_HINTS_ENV) {
        return;
    }

    let Some(detail) = report.detail.as_object_mut() else {
        return;
    };
    let names = unknown_identifier_names(system, detail.get(ERROR_FEEDBACK_KEY));
    if names.is_empty() {
        return;
    }

    let index = crate::prover::decl_index_adapter::PythonDeclIndex::new(Some(
        root.to_string_lossy().into_owned(),
    ));
    let manifest = ImportManifest::new(system, system.default_imports());

    let hints: Vec<Value> = names
        .iter()
        .map(|name| {
            // deep = false: consult only the manifest tier. A wide-library dump
            // per candidate would dwarf the proof attempt it is meant to inform.
            let verdict = check(&index, system, name, &manifest, false);
            json!({
                "name": name,
                "verdict": verdict.tag(),
                "exists_somewhere": verdict.exists_somewhere(),
                "add_import": match &verdict {
                    crate::prover::declaration_lookup::Verdict::NotInCurrentImportScope {
                        add_import,
                        ..
                    } => add_import.clone(),
                    _ => None,
                },
                // Surfaced so a consumer never treats a lookup failure as a
                // decision about the library.
                "is_evidence_of_absence": verdict.is_evidence_of_absence(),
            })
        })
        .collect();

    detail.insert(
        DECL_HINTS_KEY.to_string(),
        json!({
            "schema": "theoremata.declaration-hints.v1",
            "system": system.as_str(),
            "hints": hints,
        }),
    );
}

/// Whether an environment flag is set to an affirmative value.
fn env_flag_on(key: &str) -> bool {
    std::env::var(key)
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// Pull the identifiers out of `unknown identifier 'X'` diagnostics in the
/// already-rendered error-feedback payload.
///
/// Reads the structured diagnostics rather than re-parsing raw checker text, so
/// it stays in step with whatever `error_feedback` produced. Deduplicated,
/// order preserved, so the same missing name is looked up once.
fn unknown_identifier_names(system: FormalSystem, feedback: Option<&Value>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let Some(diags) = feedback
        .and_then(|f| f.get("diagnostics"))
        .and_then(|d| d.as_array())
    else {
        return out;
    };
    for diag in diags {
        let Some(message) = diag.get("message").and_then(|m| m.as_str()) else {
            continue;
        };
        if let Some(name) = quoted_unknown_identifier(system, message) {
            if !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

/// Extract the quoted name from a Lean `unknown identifier 'X'` message.
///
/// Returns `None` for any other message, including the sibling
/// `unknown constant` and `unknown namespace` forms, which name things a
/// declaration index does not resolve the same way.
fn quoted_unknown_identifier(system: FormalSystem, message: &str) -> Option<String> {
    if system != FormalSystem::Lean {
        return None;
    }
    let lowered = message.to_ascii_lowercase();
    if !lowered.contains("unknown identifier") {
        return None;
    }
    let start = message.find('\'')?;
    let rest = &message[start + 1..];
    let end = rest.find('\'')?;
    let name = rest[..end].trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
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
///
/// Everything here is REQUESTED configuration: paths, a runner tag, the toolchain
/// string a backend expects, a manually-set epoch. None of it moves when a library
/// at one of those paths is updated underneath us, which is why the cache key also
/// carries a resolved
/// [`EnvironmentFingerprint`](crate::checker_cache::EnvironmentFingerprint). The
/// manual epoch is retained unchanged: it remains a useful override, and it was
/// never a detector.
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
    generate_once_with_feedback(provider, system, statement, None)
}

/// [`generate_once`] with an optional checker-feedback channel.
///
/// `feedback` is prompt material ONLY: it is appended to the task and published
/// in the request context so the model can repair a specific rejection. It has
/// no effect on verification — the caller runs the result through exactly the
/// same gate as any other candidate. `None` reproduces `generate_once` verbatim.
fn generate_once_with_feedback(
    provider: &dyn ModelProvider,
    system: FormalSystem,
    statement: &str,
    feedback: Option<&str>,
) -> Result<String> {
    if provider.name() == "offline" {
        return Ok(stub_for(system));
    }
    let task = match feedback {
        Some(feedback) if !feedback.trim().is_empty() => {
            format!("{}\n\n{}", task_for(system, statement), feedback.trim())
        }
        _ => task_for(system, statement),
    };
    let mut context = json!({ "statement": statement, "system": system.as_str() });
    if let (Some(feedback), Some(obj)) = (feedback, context.as_object_mut()) {
        if !feedback.trim().is_empty() {
            obj.insert(ERROR_FEEDBACK_KEY.to_string(), json!(feedback));
        }
    }
    let response = provider.complete(&ModelRequest {
        role: role_for(system).into(),
        task,
        context,
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

    /// A fixed RESOLVED environment for cache tests that are about other fields.
    /// Borrowed for the process lifetime because a key borrows its environment.
    fn test_env() -> &'static EnvironmentFingerprint {
        static CELL: std::sync::OnceLock<EnvironmentFingerprint> = std::sync::OnceLock::new();
        CELL.get_or_init(|| {
            EnvironmentFingerprint::from_parts(
                "test",
                "unit-test environment",
                &[("env.fixture", "formal-generate".to_string())],
            )
        })
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
            environment: test_env(),
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
            environment: test_env(),
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
            environment: test_env(),
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

    /// A live verdict whose environment we could not resolve is re-verified every
    /// time. Preferring the redundant call is the whole point: we cannot say what
    /// the earlier green was earned against.
    #[test]
    fn an_unresolved_environment_disables_the_cache() {
        let cache = CheckerCache::new();
        let context = Vec::new();
        let unresolved = EnvironmentFingerprint::unresolved("test: environment not inspected");
        let key = VerificationCacheKey {
            system: FormalSystem::Lean,
            canonical_statement: "P",
            ordered_context: &context,
            proof_source: "theorem p : P := h",
            checker_identity: "lean:live:v4.19",
            policy_fingerprint: "gate-v1",
            import_manifest: &[],
            environment: &unresolved,
        };
        let calls = Cell::new(0);
        for _ in 0..2 {
            let (_, hit) = verify_candidate(Some(&cache), &key, || {
                calls.set(calls.get() + 1);
                Ok(live_verified_report("fresh"))
            })
            .unwrap();
            assert!(!hit, "an unresolved environment can never hit");
        }
        assert_eq!(calls.get(), 2);
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
        attach_error_feedback(
            FormalSystem::Lean,
            "theorem t : True := trivial",
            &mut report,
        );
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
        attach_error_feedback(
            FormalSystem::Lean,
            "theorem t : True := trivial",
            &mut noisy,
        );
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

    /// A provider whose first `fail_first` responses are checker-rejected
    /// (`sorry`) and whose later responses are clean, recording every call and
    /// the feedback it was given.
    struct ScriptedProvider {
        fail_first: usize,
        calls: Cell<usize>,
        feedback_seen: RefCell<Vec<Option<String>>>,
    }

    impl ScriptedProvider {
        fn new(fail_first: usize) -> Self {
            Self {
                fail_first,
                calls: Cell::new(0),
                feedback_seen: RefCell::new(Vec::new()),
            }
        }
    }

    impl ModelProvider for ScriptedProvider {
        fn complete(&self, req: &ModelRequest) -> Result<ModelResponse> {
            let n = self.calls.get();
            self.calls.set(n + 1);
            self.feedback_seen.borrow_mut().push(
                req.context
                    .get(ERROR_FEEDBACK_KEY)
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            );
            let code = if n < self.fail_first {
                "theorem t : True := by sorry"
            } else {
                "theorem t : True := trivial"
            };
            Ok(ModelResponse {
                content: json!({ "code": code }),
                model: "mock".into(),
                provider: "mock".into(),
            })
        }
        fn name(&self) -> &str {
            "command"
        }
    }

    #[test]
    fn default_correction_budget_reproduces_todays_behavior() {
        // The opt-in is off by default: no extra generations, no feedback in any
        // request, and the same all-failed outcome as before the loop existed.
        assert_eq!(CorrectionConfig::default().max_rounds, 0);
        assert!(!CorrectionConfig::default().enabled());

        let store = store();
        let config = mock_config();
        let provider = ScriptedProvider::new(usize::MAX);
        let (code, report) = generate_and_verify_inner(
            &store,
            &config,
            &provider,
            FormalSystem::Lean,
            "True",
            &[],
            None,
            CorrectionConfig::default(),
        )
        .unwrap();

        assert_eq!(provider.calls.get(), N, "default must sample exactly N");
        assert!(!report.lexically_verified);
        assert!(code.contains("sorry"), "fallback is the last candidate");
        assert!(
            provider.feedback_seen.borrow().iter().all(Option::is_none),
            "no request may carry feedback at the default budget"
        );
    }

    #[test]
    fn correction_round_receives_feedback_and_verifies() {
        let store = store();
        let config = mock_config();
        // All N initial candidates are rejected; the first corrected one is clean.
        let provider = ScriptedProvider::new(N);
        let (code, report) = generate_and_verify_inner(
            &store,
            &config,
            &provider,
            FormalSystem::Lean,
            "True",
            &[],
            None,
            CorrectionConfig {
                max_rounds: 2,
                samples_per_round: 2,
            },
        )
        .unwrap();

        assert!(report.lexically_verified, "corrected candidate must verify");
        assert!(code.contains("trivial"));
        // Round 0 spent N; the correction round accepted on its first sample and
        // retired immediately rather than spending the rest of its budget.
        assert_eq!(provider.calls.get(), N + 1);

        let seen = provider.feedback_seen.borrow();
        assert!(seen[..N].iter().all(Option::is_none));
        let corrective = seen[N].as_deref().expect("correction must carry feedback");
        assert!(!corrective.trim().is_empty());
        assert!(corrective.contains("REJECTED"), "{corrective}");
        // The failed source itself is handed back to the model.
        assert!(corrective.contains("sorry"), "{corrective}");
    }

    #[test]
    fn a_first_round_pass_spends_zero_correction_budget() {
        let store = store();
        let config = mock_config();
        let provider = ScriptedProvider::new(0);
        let (_, report) = generate_and_verify_inner(
            &store,
            &config,
            &provider,
            FormalSystem::Lean,
            "True",
            &[],
            None,
            CorrectionConfig {
                max_rounds: 2,
                samples_per_round: 2,
            },
        )
        .unwrap();

        assert!(report.lexically_verified);
        assert_eq!(
            provider.calls.get(),
            1,
            "an early sibling pass must retire the problem"
        );
    }

    #[test]
    fn exhausted_correction_rounds_stay_bounded_and_fail_closed() {
        let store = store();
        let config = mock_config();
        // Nothing ever passes: 3 initial + 2 rounds x 2 samples = 7 generations.
        let provider = ScriptedProvider::new(usize::MAX);
        let (_, report) = generate_and_verify_inner(
            &store,
            &config,
            &provider,
            FormalSystem::Lean,
            "True",
            &[],
            None,
            CorrectionConfig {
                max_rounds: 2,
                samples_per_round: 2,
            },
        )
        .unwrap();

        assert!(!report.lexically_verified, "correction never fakes a pass");
        assert_eq!(provider.calls.get(), N + 4);
    }

    #[test]
    fn distinct_failures_dedups_by_source_and_drops_empty_feedback() {
        let failures = vec![
            FailedCandidate {
                code: "a".into(),
                feedback: "err a".into(),
            },
            FailedCandidate {
                code: "a".into(),
                feedback: "err a again".into(),
            },
            FailedCandidate {
                code: "b".into(),
                feedback: "   ".into(),
            },
            FailedCandidate {
                code: "c".into(),
                feedback: "err c".into(),
            },
        ];
        let distinct = distinct_failures(&failures);
        assert_eq!(distinct.len(), 2);
        assert_eq!(distinct[0].code, "a");
        assert_eq!(distinct[1].code, "c");
    }

    #[test]
    fn correction_config_reads_bounded_values_from_env() {
        // Absent/blank/invalid all fall back to the (disabled) default.
        assert_eq!(usize_from_env("THEOREMATA_NO_SUCH_VAR_XYZ", 7), 7);
        assert!(!CorrectionConfig {
            max_rounds: 2,
            samples_per_round: 0,
        }
        .enabled());
        assert!(CorrectionConfig {
            max_rounds: 1,
            samples_per_round: 1,
        }
        .enabled());
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

    #[test]
    fn quoted_unknown_identifier_extracts_lean_names() {
        assert_eq!(
            quoted_unknown_identifier(FormalSystem::Lean, "unknown identifier 'Nat.sub_one_lt'"),
            Some("Nat.sub_one_lt".to_string())
        );
        // Case-insensitive on the phrase, exact on the name.
        assert_eq!(
            quoted_unknown_identifier(FormalSystem::Lean, "Unknown Identifier 'foo'"),
            Some("foo".to_string())
        );
    }

    #[test]
    fn quoted_unknown_identifier_ignores_sibling_and_foreign_messages() {
        // A different diagnostic entirely.
        assert_eq!(
            quoted_unknown_identifier(FormalSystem::Lean, "unsolved goals"),
            None
        );
        // The sibling forms name things the index does not resolve alike.
        assert_eq!(
            quoted_unknown_identifier(FormalSystem::Lean, "unknown constant 'Foo.bar'"),
            None
        );
        assert_eq!(
            quoted_unknown_identifier(FormalSystem::Lean, "unknown namespace 'Foo'"),
            None
        );
        // Only Lean is backed by a decl index; other systems opt out here.
        assert_eq!(
            quoted_unknown_identifier(FormalSystem::Rocq, "unknown identifier 'foo'"),
            None
        );
    }

    #[test]
    fn unknown_identifier_names_dedupes_and_preserves_order() {
        let feedback = json!({
            "diagnostics": [
                {"message": "unknown identifier 'b'"},
                {"message": "unsolved goals"},
                {"message": "unknown identifier 'a'"},
                {"message": "unknown identifier 'b'"},
            ]
        });
        assert_eq!(
            unknown_identifier_names(FormalSystem::Lean, Some(&feedback)),
            vec!["b".to_string(), "a".to_string()]
        );
    }

    #[test]
    fn unknown_identifier_names_empty_without_diagnostics() {
        assert!(unknown_identifier_names(FormalSystem::Lean, None).is_empty());
        assert!(unknown_identifier_names(FormalSystem::Lean, Some(&json!({}))).is_empty());
    }

    #[test]
    fn declaration_hints_skipped_on_a_passing_report() {
        // A lexically-verified report is never annotated, regardless of config.
        let mut report = live_verified_report("pass");
        report.detail = json!({ ERROR_FEEDBACK_KEY: {"diagnostics": []} });
        let config = Config::default();
        attach_declaration_hints(FormalSystem::Lean, &config, &mut report);
        assert!(report.detail.get(DECL_HINTS_KEY).is_none());
    }

    // =======================================================================
    // Verification provenance
    // =======================================================================

    /// A resolved Lake-shaped environment distinct from [`test_env`].
    fn resolved_env(manifest: &str) -> EnvironmentFingerprint {
        EnvironmentFingerprint::from_parts(
            "lake",
            "lake-manifest.json (1 package pin(s)), toolchain leanprover/lean4:v4.19.0",
            &[("env.manifest", manifest.to_string())],
        )
    }

    fn provenance_of(report: &VerificationReport) -> &Value {
        report
            .detail
            .get(PROVENANCE_KEY)
            .expect("every verified candidate must carry its provenance")
    }

    #[test]
    fn a_resolved_environment_is_recorded_with_its_digest() {
        let env = resolved_env("{\"packages\":[]}");
        let mut report = live_verified_report("clean");
        attach_verification_provenance(
            FormalSystem::Lean,
            "True",
            "theorem t : True := by trivial",
            &env,
            &mut report,
        );

        let p = provenance_of(&report);
        assert_eq!(p["schema"], PROVENANCE_SCHEMA);
        assert_eq!(p["environment_resolved"], true);
        assert_eq!(p["environment_unresolved_reason"], Value::Null);

        // The recorded pin is the digest-bearing key field, not a lossy summary:
        // a sweep compares these for exact equality.
        let recorded = p["verified_against"].as_str().unwrap();
        assert_eq!(recorded, env.key_field());
        assert!(recorded.starts_with("resolved:lake:"), "{recorded}");
        let EnvironmentFingerprint::Resolved { digest, .. } = &env else {
            panic!("from_parts yields a resolved fingerprint");
        };
        assert!(recorded.ends_with(digest.as_str()), "{recorded}");

        // A different environment must record a different pin, or the comparison
        // downstream is worthless.
        let mut other_report = live_verified_report("clean");
        attach_verification_provenance(
            FormalSystem::Lean,
            "True",
            "theorem t : True := by trivial",
            &resolved_env("{\"packages\":[{\"name\":\"mathlib\"}]}"),
            &mut other_report,
        );
        assert_ne!(
            provenance_of(&other_report)["verified_against"],
            p["verified_against"]
        );
    }

    #[test]
    fn an_unresolved_environment_is_recorded_with_its_reason_not_dropped() {
        const REASON: &str =
            "lean: no lake project configured, so no dependency revision can be pinned";
        let mut report = live_verified_report("clean");
        attach_verification_provenance(
            FormalSystem::Lean,
            "True",
            "theorem t : True := by trivial",
            &EnvironmentFingerprint::unresolved(REASON),
            &mut report,
        );

        let p = provenance_of(&report);
        // Omission is the bug: an absent environment reads as "depended on
        // nothing", which is the fact that produces a false Fresh later.
        assert!(
            p.get("verified_against").is_some_and(|v| !v.is_null()),
            "an unresolved environment must still be recorded"
        );
        assert_eq!(p["environment_resolved"], false);
        assert_eq!(p["environment_unresolved_reason"], REASON);
        let recorded = p["verified_against"].as_str().unwrap();
        assert!(recorded.starts_with("unresolved:"), "{recorded}");
        assert!(recorded.contains(REASON), "{recorded}");
        assert!(p["environment_describe"].as_str().unwrap().contains(REASON));
    }

    #[test]
    fn provenance_leaves_every_verdict_field_byte_identical() {
        for before in [live_verified_report("clean"), failed_report()] {
            let mut after = before.clone();
            attach_verification_provenance(
                FormalSystem::Lean,
                "True",
                "theorem t : True := by trivial",
                &resolved_env("{}"),
                &mut after,
            );

            assert_eq!(after.lexically_verified, before.lexically_verified);
            assert_eq!(after.axioms_clean, before.axioms_clean);
            assert_eq!(after.statement_preserved, before.statement_preserved);
            assert_eq!(after.lexical_clean, before.lexical_clean);
            assert_eq!(after.hardening_clean, before.hardening_clean);
            assert_eq!(after.live, before.live);

            // `detail` differs by exactly the one added key and nothing else.
            let mut stripped = after.detail.clone();
            stripped.as_object_mut().unwrap().remove(PROVENANCE_KEY);
            assert_eq!(stripped, before.detail);
        }

        // A non-object detail is left exactly as it was; the event payload is the
        // surface that always carries the record.
        let mut scalar = VerificationReport {
            detail: json!("opaque"),
            ..failed_report()
        };
        attach_verification_provenance(
            FormalSystem::Lean,
            "True",
            "x",
            &resolved_env("{}"),
            &mut scalar,
        );
        assert_eq!(scalar.detail, json!("opaque"));
    }

    #[test]
    fn the_real_generate_path_publishes_provenance_without_changing_the_verdict() {
        let store = store();
        let config = mock_config();
        let provider = CannedProvider {
            code: "theorem t : True := trivial".into(),
        };
        let (_, report) =
            generate_and_verify(&store, &config, &provider, FormalSystem::Lean, "True").unwrap();

        // Same verdict `returns_code_and_report_for_each_system` asserts.
        assert!(report.lexically_verified);
        assert!(!report.live);

        let p = provenance_of(&report);
        assert_eq!(p["id"], "True");
        // A mock backend consults no library, which is a RESOLVED environment
        // rather than an unknown one.
        assert_eq!(p["environment_resolved"], true);
        assert!(p["verified_against"]
            .as_str()
            .unwrap()
            .starts_with("resolved:mock:"));
        // No backend publishes an elaborated form yet, so the pin is honestly
        // absent and says why.
        assert_eq!(p["pinned_statement_type"], Value::Null);
        assert!(p["pinned_statement_absent_reason"].is_string());
    }

    #[test]
    fn artifact_classification_never_claims_a_self_contained_certificate() {
        use staleness::ArtifactClass;

        // Nothing this module generates is a certificate, and claiming one would
        // buy the cheap statement-only recheck route it has not earned.
        for system in [
            FormalSystem::Lean,
            FormalSystem::Rocq,
            FormalSystem::Isabelle,
            FormalSystem::Candle,
            FormalSystem::Agda,
            FormalSystem::Metamath,
        ] {
            assert_ne!(
                classify_artifact(system, stub_for(system).as_str()),
                ArtifactClass::SelfContainedCertificate,
                "{system}"
            );
        }

        assert_eq!(
            classify_artifact(FormalSystem::Lean, "theorem t : True := by simp"),
            ArtifactClass::TacticScript
        );
        assert_eq!(
            classify_artifact(FormalSystem::Lean, "theorem t : True := trivial"),
            ArtifactClass::ProofTerm
        );
        // `by` as a substring of an identifier is not a tactic block.
        assert_eq!(
            classify_artifact(FormalSystem::Lean, "theorem t : P := byte_lemma"),
            ArtifactClass::ProofTerm
        );
        assert!(has_word("theorem t : True :=\n  by\n  simp", "by"));
        assert!(!has_word("Nat.byte", "by"));
    }

    #[test]
    fn the_emitted_shape_is_what_staleness_consumes() {
        use staleness::{
            assess, ArtifactClass, EnvironmentFingerprint as PinnedEnv, StalenessVerdict,
            UnknownReason, VerifiedResult,
        };

        let env = resolved_env("{\"packages\":[]}");
        let mut report = live_verified_report("clean");
        attach_verification_provenance(
            FormalSystem::Lean,
            "theorem p : P",
            "theorem p : P := by simp",
            &env,
            &mut report,
        );
        let p = provenance_of(&report).clone();

        // Rebuild the staleness input straight off the record, field for field.
        let recovered = VerifiedResult::new(
            p["id"].as_str().unwrap(),
            match p["artifact"].as_str().unwrap() {
                "tactic_script" => ArtifactClass::TacticScript,
                "proof_term" => ArtifactClass::ProofTerm,
                "self_contained_certificate" => ArtifactClass::SelfContainedCertificate,
                other => panic!("unknown artifact tag {other}"),
            },
            PinnedEnv::new(p["verified_against"].as_str().unwrap()),
            p["pinned_statement_type"].as_str().map(str::to_string),
        );
        assert_eq!(recovered.artifact, ArtifactClass::TacticScript);
        assert_eq!(recovered.id, "theorem p : P");

        // Same environment today: Fresh, with no reinterpretation anywhere.
        let today = PinnedEnv::new(env.key_field());
        assert_eq!(
            assess(&recovered, Some(&today), None),
            StalenessVerdict::Fresh
        );

        // A moved environment with no elaborated pin is the honest Unknown. This
        // is the state of the world until a backend publishes an elaborated form.
        let moved = PinnedEnv::new(resolved_env("{\"packages\":[1]}").key_field());
        assert!(matches!(
            assess(&recovered, Some(&moved), None),
            StalenessVerdict::Unknown(UnknownReason::NoPinnedStatementType)
        ));
    }

    #[test]
    fn a_published_elaborated_form_becomes_the_pinned_statement_type() {
        use staleness::{
            assess, EnvironmentFingerprint as PinnedEnv, ReelaborationOutcome, VerifiedResult,
        };

        // The shape a backend that CAN pretty-print an elaborated type publishes.
        let mut report = VerificationReport {
            detail: json!({
                ELABORATED_STATEMENT_DETAIL_KEY: {
                    "form": "forall (x : Real), Real.nnrpow x (1/3) = 2",
                    "provenance": "lean.repl.elaborated_type",
                }
            }),
            ..live_verified_report("clean")
        };
        let env = resolved_env("{\"packages\":[]}");
        attach_verification_provenance(
            FormalSystem::Lean,
            "algebra_5778",
            "theorem algebra_5778 : P := by norm_num",
            &env,
            &mut report,
        );

        let p = provenance_of(&report);
        assert_eq!(
            p["pinned_statement_type"],
            "forall (x : Real), Real.nnrpow x (1/3) = 2"
        );
        assert_eq!(
            p["pinned_statement_provenance"],
            "lean.repl.elaborated_type"
        );
        assert_eq!(p["pinned_statement_absent_reason"], Value::Null);

        let recovered = VerifiedResult::new(
            p["id"].as_str().unwrap(),
            classify_artifact(
                FormalSystem::Lean,
                "theorem algebra_5778 : P := by norm_num",
            ),
            PinnedEnv::new(p["verified_against"].as_str().unwrap()),
            p["pinned_statement_type"].as_str().map(str::to_string),
        );
        // With the pin present the discriminator actually runs: same type under a
        // moved environment is a repair candidate, not a silent green.
        let verdict = assess(
            &recovered,
            Some(&PinnedEnv::new(
                resolved_env("{\"packages\":[1]}").key_field(),
            )),
            Some(&ReelaborationOutcome::Elaborated {
                statement_type: "forall (x : Real), Real.nnrpow x (1/3) = 2".to_string(),
            }),
        );
        assert!(verdict.is_repairable());
    }

    #[test]
    fn an_unresolved_pin_is_never_swept_up_as_fresh() {
        use staleness::{
            assess, ArtifactClass, EnvironmentFingerprint as PinnedEnv, StalenessVerdict,
            UnknownReason, VerifiedResult,
        };

        let mut report = live_verified_report("clean");
        attach_verification_provenance(
            FormalSystem::Lean,
            "True",
            "theorem t : True := by trivial",
            &EnvironmentFingerprint::unresolved("lean: no lake project configured"),
            &mut report,
        );
        let p = provenance_of(&report);
        let recovered = VerifiedResult::new(
            p["id"].as_str().unwrap(),
            ArtifactClass::TacticScript,
            PinnedEnv::new(p["verified_against"].as_str().unwrap()),
            None,
        );

        // The sweep contract: an unresolvable environment TODAY is passed as
        // `None`, which is what keeps two unresolved records from comparing equal
        // and reading as Fresh.
        assert!(matches!(
            assess(&recovered, None, None),
            StalenessVerdict::Unknown(UnknownReason::EnvironmentUnresolved { .. })
        ));
        // And it can never collide with a resolved environment either.
        let resolved_today = PinnedEnv::new(resolved_env("{}").key_field());
        assert!(!assess(&recovered, Some(&resolved_today), None).is_fresh());
    }

    #[test]
    fn declaration_hints_skipped_for_non_lean_systems() {
        // Rocq/Isabelle have no decl index; the pass must not annotate them even
        // when it would otherwise run.
        let mut report = failed_report();
        let config = Config::default();
        attach_declaration_hints(FormalSystem::Rocq, &config, &mut report);
        assert!(report.detail.get(DECL_HINTS_KEY).is_none());
    }

    // =======================================================================
    // Hammer fallback (model fails the gate -> a second real attempt via hammer)
    // =======================================================================

    /// Read the proof-path record the fallback publishes on the returned report.
    fn proof_path_of(report: &VerificationReport) -> &Value {
        report
            .detail
            .get(PROOF_PATH_KEY)
            .expect("every returned proof must carry its path provenance")
    }

    /// Drive `generate_and_verify_core` against the REAL mock backend (real source
    /// scan, canned kernel) with an injected hammer seam. `used_live` is `false`
    /// here on purpose: the production seam's live gate is bypassed so the
    /// fallback's control flow can be exercised without a toolchain, while the
    /// mock backend still runs a genuine source scan on whatever code it verifies.
    fn run_core_with_hammer(
        provider: &dyn ModelProvider,
        hammer: &HammerSeam,
    ) -> (String, VerificationReport) {
        let store = store();
        let config = mock_config();
        let backend = backend_for(&config, FormalSystem::Lean, true);
        generate_and_verify_core(
            &store,
            &config,
            provider,
            FormalSystem::Lean,
            "True",
            &[],
            None,
            CorrectionConfig::default(),
            backend.as_ref(),
            false,
            hammer,
        )
        .unwrap()
    }

    #[test]
    fn model_success_returns_immediately_with_model_provenance_and_no_hammer_call() {
        // A clean model proof must short-circuit: the fallback never runs, so the
        // hammer seam is never touched and the path is honestly "model".
        let provider = CannedProvider {
            code: "theorem t : True := trivial".into(),
        };
        let calls = Cell::new(0usize);
        let hammer = |_c: &Config, _s: FormalSystem, _g: &str| -> Option<String> {
            calls.set(calls.get() + 1);
            Some("theorem t : True := trivial".into())
        };
        let (code, report) = run_core_with_hammer(&provider, &hammer);

        assert!(report.lexically_verified, "model proof must verify");
        assert!(code.contains("trivial"));
        assert_eq!(calls.get(), 0, "a model success must never call the hammer");
        let p = proof_path_of(&report);
        assert_eq!(p["path"], "model");
        assert_eq!(p["hammer_attempted"], false);
    }

    #[test]
    fn model_failure_then_hammer_success_returns_hammer_result_with_hammer_provenance() {
        // Every model candidate is rejected (the source scan flags `sorry`); the
        // hammer proposes a clean proof of the SAME goal, the SAME gate accepts it,
        // and that gate-accepted proof is what comes back, tagged "hammer".
        let provider = CannedProvider {
            code: "theorem mfail : True := by sorry".into(),
        };
        let calls = Cell::new(0usize);
        let hammer = |_c: &Config, _s: FormalSystem, _g: &str| -> Option<String> {
            calls.set(calls.get() + 1);
            Some("theorem hammered : True := trivial".into())
        };
        let (code, report) = run_core_with_hammer(&provider, &hammer);

        assert!(report.lexically_verified, "the hammer proof must verify");
        assert_eq!(calls.get(), 1, "the hammer runs exactly once on failure");
        assert!(
            code.contains("hammered"),
            "the accepted (hammer) proof is returned, not the model failure: {code}"
        );
        let p = proof_path_of(&report);
        assert_eq!(p["path"], "hammer");
        assert_eq!(p["hammer_attempted"], true);
    }

    #[test]
    fn model_failure_and_hammer_failure_returns_the_original_model_failure() {
        // Both paths fail the gate. The result must be the ORIGINAL model failure,
        // never a synthesized success and never silently swapped for the hammer's
        // own (equally failed) candidate.
        let provider = CannedProvider {
            code: "theorem mfail : True := by sorry".into(),
        };
        let hammer = |_c: &Config, _s: FormalSystem, _g: &str| -> Option<String> {
            Some("theorem hfail : True := by sorry".into())
        };
        let (code, report) = run_core_with_hammer(&provider, &hammer);

        assert!(
            !report.lexically_verified,
            "neither path passed, so nothing may be marked verified"
        );
        assert!(
            code.contains("mfail"),
            "the original model failure is returned, not the hammer's: {code}"
        );
        let p = proof_path_of(&report);
        assert_eq!(p["path"], "model");
        // The fallback WAS attempted; that it failed is recorded, not hidden.
        assert_eq!(p["hammer_attempted"], true);
    }

    /// A backend that fails model code cleanly but ERRORS when asked to verify the
    /// hammer's candidate, so the fallback's error-swallowing can be exercised.
    struct GateErrorBackend;
    impl FormalBackend for GateErrorBackend {
        fn system(&self) -> FormalSystem {
            FormalSystem::Lean
        }
        fn compile_success_signal(&self) -> crate::prover::formal::SuccessSignal {
            crate::prover::formal::SuccessSignal::NonZeroExitIsHonest
        }
        fn is_mock(&self) -> bool {
            true
        }
        // Model code fails cleanly; the hammer's marked candidate makes the gate
        // itself error, standing in for a worker/exec failure mid-verification.
        fn verify(&self, _cfg: &Config, code: &str, _stmt: &str) -> Result<VerificationReport> {
            if code.contains("HAMMER_ERROR") {
                anyhow::bail!("simulated hammer gate error");
            }
            Ok(failed_report())
        }
        // Unreachable: `verify` is overridden, so the layer methods never run. They
        // exist only because the trait requires them.
        fn scaffold(
            &self,
            _cfg: &Config,
            _code: &str,
            _name: &str,
        ) -> Result<crate::prover::formal::Workspace> {
            unreachable!("verify is overridden")
        }
        fn compile(
            &self,
            _ws: &crate::prover::formal::Workspace,
        ) -> Result<crate::prover::formal::CompileReport> {
            unreachable!("verify is overridden")
        }
        fn audit_axioms(
            &self,
            _ws: &crate::prover::formal::Workspace,
            _thm: &str,
            _whitelist: &[String],
        ) -> Result<crate::prover::formal::AxiomReport> {
            unreachable!("verify is overridden")
        }
        fn kernel_recheck(
            &self,
            _ws: &crate::prover::formal::Workspace,
        ) -> Result<crate::prover::formal::RecheckReport> {
            unreachable!("verify is overridden")
        }
        fn source_scan(&self, _code: &str) -> Result<crate::prover::formal::ScanReport> {
            unreachable!("verify is overridden")
        }
    }

    #[test]
    fn a_hammer_gate_error_is_swallowed_and_the_model_failure_stands() {
        // Fail closed: an error thrown while verifying the hammer candidate must
        // not panic and must leave the original model failure exactly as it was.
        let store = store();
        let config = mock_config();
        let provider = CannedProvider {
            code: "theorem model_only : True := by sorry".into(),
        };
        let hammer = |_c: &Config, _s: FormalSystem, _g: &str| -> Option<String> {
            Some("theorem t : True := by HAMMER_ERROR".into())
        };
        let backend = GateErrorBackend;
        let (code, report) = generate_and_verify_core(
            &store,
            &config,
            &provider,
            FormalSystem::Lean,
            "True",
            &[],
            None,
            CorrectionConfig::default(),
            &backend,
            false,
            &hammer,
        )
        .unwrap();

        assert!(
            !report.lexically_verified,
            "a swallowed hammer error can never produce a verified verdict"
        );
        assert!(
            code.contains("model_only"),
            "the untouched model failure is returned: {code}"
        );
        let p = proof_path_of(&report);
        assert_eq!(p["path"], "model");
        // The attempt happened (and threw); that is recorded honestly.
        assert_eq!(p["hammer_attempted"], true);
    }
}
