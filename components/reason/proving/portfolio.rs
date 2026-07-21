//! Portfolio proving (Phase 5): attempt a conjecture across every requested
//! formal system and take whichever backend certifies first.
//!
//! Where [`generate_and_verify`](crate::formal_generate::generate_and_verify)
//! produces-and-verifies a proof for ONE [`FormalSystem`], [`portfolio_prove`]
//! fans the same conjecture out across Lean, Rocq, and Isabelle, records a
//! per-system verdict, and names the WINNER — the first system whose live report
//! passes the verification gate. Mock/source-scan successes remain useful
//! diagnostics, but are never certification.
//!
//! It is sequential and deterministic by default. Two stages are opt-in via the
//! environment and off otherwise: a cheap refutation pre-filter
//! (`THEOREMATA_PORTFOLIO_FAST_REFUTE`) that can skip the whole fan-out when the
//! statement is provably false, and an owned verification stage
//! (`THEOREMATA_PORTFOLIO_VERIFY_THREADS`) that can run already-generated
//! candidates on worker threads;
//! provider calls, database writes, result order, and winner selection remain
//! deterministic on the caller thread. A system whose toolchain is absent
//! contributes an `available: false` entry, and a system that errors out records
//! its error — neither aborts the sibling attempts.

use crate::{
    concurrent::ConcurrentConfig,
    config::Config,
    db::Store,
    falsification::Falsifier,
    formal_generate::{generate_and_verify, generate_candidates_for_verification},
    formalize_portfolio::{
        run_owned_formal_system_verifications, OwnedVerificationTask, VerificationMode,
    },
    prover::{
        formal::{backend_for, FormalSystem},
        model::VerificationReport,
    },
    provider::ModelProvider,
    verification_ladder::{
        CheapRung, KernelRung, KernelVerdict, LadderConfig, LadderOutcome, RefutationWitness,
        RungBudget, RungConfig, RungVerdict, VerificationLadder,
    },
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Instant;

/// The three systems attempted when the caller does not restrict the portfolio.
pub const ALL_SYSTEMS: [FormalSystem; 3] = [
    FormalSystem::Lean,
    FormalSystem::Rocq,
    FormalSystem::Isabelle,
];

/// One system's independent attempt at the conjecture.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemAttempt {
    pub system: FormalSystem,
    /// Whether this is a LIVE, gate-verified proof suitable for certification.
    pub verified: bool,
    /// Whether this system's toolchain was usable (mock mode is always
    /// available; in live mode this reflects a real toolchain probe). An
    /// `available: false` entry did not run and is neither a win nor an error.
    pub available: bool,
    /// The accepted (or last) candidate proof source, when an attempt ran.
    pub code: Option<String>,
    /// The 3+1-layer verdict, when an attempt ran.
    pub report: Option<VerificationReport>,
    /// Wall-clock time spent on this system's attempt.
    pub duration_ms: u128,
    /// A backend/generation fault, if the attempt failed to run to a verdict.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The outcome of a portfolio run across the requested systems.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioResult {
    pub statement: String,
    /// The first system that certified, if any.
    pub winner: Option<FormalSystem>,
    /// True iff at least one system's report is `lexically_verified`.
    pub any_verified: bool,
    pub per_system: Vec<SystemAttempt>,
    /// Set when a cheap [`verification_ladder`](crate::verification_ladder) rung
    /// refuted the statement *before* any backend was consulted. A refutation is
    /// system-independent — if the claim is false, no formal system can certify
    /// it — so `per_system` is empty and N backend calls were never paid for.
    ///
    /// `None` on every default run: both cheap tiers are disabled by
    /// [`LadderConfig::default`], so this field is only ever populated when a
    /// caller has explicitly opted in to pre-filtering — either by setting
    /// `THEOREMATA_PORTFOLIO_FAST_REFUTE` (see [`portfolio_ladder_config`]) or
    /// by calling [`portfolio_prove_with_ladder`] with an enabled tier.
    ///
    /// **Shape note for consumers:** when this is `Some`, `per_system` is
    /// EMPTY — not N entries marked unavailable. Code that assumes
    /// `per_system.len() == systems.len()` must check `refutation` first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refutation: Option<RefutationWitness>,
}

/// Attempt `statement` across each of `systems` (defaulting to all three when
/// empty), taking the first system that certifies as the winner.
///
/// Each system is attempted independently and sequentially:
/// * If its toolchain is absent (live mode only), it contributes an
///   `available: false` entry and is skipped — no error.
/// * Otherwise [`generate_and_verify`] runs. Only a LIVE 3+1-layer gate pass
///   decides whether it verified; mock source-scan results remain diagnostics.
/// * A generation/backend error is recorded on the entry, not propagated — one
///   failing system never aborts the others.
///
/// The cheap pre-filter is **off** unless `THEOREMATA_PORTFOLIO_FAST_REFUTE` opts
/// in (see [`portfolio_ladder_config`]); with it unset this is exactly the
/// sequential portfolio and `refutation` is always `None`. When it IS enabled and
/// the statement is refuted, `per_system` comes back EMPTY — see
/// [`PortfolioResult::refutation`].
pub fn portfolio_prove(
    store: &Store,
    config: &Config,
    provider: &dyn ModelProvider,
    statement: &str,
    systems: &[FormalSystem],
) -> Result<PortfolioResult> {
    portfolio_prove_with_ladder(
        store,
        config,
        provider,
        statement,
        systems,
        &[],
        portfolio_ladder_config(),
    )
}

// ---------------------------------------------------------------------------
// The cheap pre-filter: refute once, before N backend calls
// ---------------------------------------------------------------------------

/// The "kernel" seam handed to the pre-filter ladder.
///
/// The portfolio itself IS the expensive kernel — it fans out to Lean/Rocq/
/// Isabelle and only a live gate pass certifies. The ladder is used here purely
/// as the *screening* stage, so this rung always abstains and hands the question
/// back to the per-system loop. It has no way to certify, by construction: a
/// `KernelVerdict::Verified` is never produced on this path.
struct DeferToPortfolio;

impl KernelRung<str> for DeferToPortfolio {
    fn name(&self) -> &str {
        "portfolio"
    }
    fn verify(&self, _statement: &str, _budget: &RungBudget) -> KernelVerdict {
        KernelVerdict::Abstain
    }
}

/// The [`Falsifier`] as a tier-1 [`CheapRung`].
///
/// Mapping (deliberately conservative — only ONE verdict refutes):
/// * `counterexample` ⇒ [`RungVerdict::Refuted`], with the falsifying assignment
///   as the witness instance.
/// * **everything else** — `no_counterexample_in_domain`, `not_applicable`,
///   `no_model`, `unavailable`, `inconclusive`, `error` — ⇒
///   [`RungVerdict::Abstain`]. In particular a bounded sweep that found nothing
///   has said *nothing*: `∀`-claims are refutable by one instance but never
///   provable by finitely many. An `Err` from the falsifier likewise abstains,
///   since a failed probe is not evidence about the statement.
pub struct FalsifierRung<'a> {
    falsifier: Falsifier<'a>,
}

impl<'a> FalsifierRung<'a> {
    /// Wrap `provider`'s falsifier as a refutation-only rung.
    pub fn new(provider: &'a dyn ModelProvider) -> Self {
        Self {
            falsifier: Falsifier { provider },
        }
    }
}

/// Flatten a falsifying assignment into ordered `(variable, value)` bindings.
/// Sorted by variable name so the witness serializes deterministically.
fn assignment_bindings(assignment: Option<&Value>) -> Vec<(String, String)> {
    match assignment {
        Some(Value::Object(map)) => {
            let mut bindings: Vec<(String, String)> = map
                .iter()
                .map(|(key, value)| {
                    let rendered = match value {
                        Value::String(text) => text.clone(),
                        other => other.to_string(),
                    };
                    (key.clone(), rendered)
                })
                .collect();
            bindings.sort_by(|a, b| a.0.cmp(&b.0));
            bindings
        }
        Some(Value::Null) | None => Vec::new(),
        Some(other) => vec![("assignment".to_string(), other.to_string())],
    }
}

impl CheapRung<str> for FalsifierRung<'_> {
    fn name(&self) -> &str {
        "falsifier"
    }

    fn probe(&self, statement: &str, _budget: &RungBudget) -> RungVerdict {
        let Ok(verdict) = self.falsifier.falsify(statement) else {
            // A probe that failed to run is not evidence about the statement.
            return RungVerdict::Abstain;
        };
        if verdict.verdict != "counterexample" {
            return RungVerdict::Abstain;
        }
        let bindings = assignment_bindings(verdict.assignment.as_ref());
        RungVerdict::Refuted(RefutationWitness::counterexample(
            "falsifier",
            format!("bounded numeric check found a counterexample to: {statement}"),
            bindings,
        ))
    }
}

/// [`portfolio_prove`] with an explicit ladder policy and extra tier-1 rungs.
///
/// The cheap tiers run **once, on the statement**, before any system is touched.
/// A refutation is system-independent, so it skips the entire portfolio rather
/// than paying N backend calls; the witness is recorded on the result. Every
/// other outcome falls through to exactly the pre-existing per-system logic.
///
/// With `ladder_config` at its default both cheap tiers are disabled, no rung is
/// invoked (registration alone never enables a tier), and this is byte-identical
/// to the sequential portfolio.
///
/// Callers that only want the environment's policy should use [`portfolio_prove`];
/// this entry point exists for callers injecting their *own* tier-1 rungs (a
/// domain-specific numeric sweep, an SMT probe) alongside the built-in falsifier.
///
/// # Result shape under a refutation
///
/// A refutation returns `per_system: Vec::new()` and `refutation: Some(_)` — see
/// [`PortfolioResult::refutation`]. Consumers must not assume
/// `per_system.len() == systems.len()` once a cheap tier is enabled.
pub fn portfolio_prove_with_ladder(
    store: &Store,
    config: &Config,
    provider: &dyn ModelProvider,
    statement: &str,
    systems: &[FormalSystem],
    extra_fast_refute: &[&dyn CheapRung<str>],
    ladder_config: LadderConfig,
) -> Result<PortfolioResult> {
    let systems: Vec<FormalSystem> = if systems.is_empty() {
        ALL_SYSTEMS.to_vec()
    } else {
        systems.to_vec()
    };

    // Caller-supplied rungs are tried first, then the built-in falsifier.
    let falsifier_rung = FalsifierRung::new(provider);
    let mut fast_refute: Vec<&dyn CheapRung<str>> = extra_fast_refute.to_vec();
    fast_refute.push(&falsifier_rung);

    let kernel = DeferToPortfolio;
    let screen = VerificationLadder::new(&kernel)
        .with_fast_refute(fast_refute)
        .with_config(ladder_config)
        .run(statement);

    if let LadderOutcome::Refuted {
        rung,
        tier,
        witness,
    } = screen.outcome
    {
        store.event(
            None,
            None,
            "portfolio_prove.refuted",
            &rung,
            json!({
                "statement": statement,
                "rung": rung,
                "tier": tier.as_str(),
                "witness": &witness,
                "systems_skipped": systems.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            }),
        )?;
        return Ok(PortfolioResult {
            statement: statement.to_string(),
            winner: None,
            any_verified: false,
            // No system was attempted: a false statement cannot be certified by
            // any of them, so every backend call would have been pure waste.
            per_system: Vec::new(),
            refutation: Some(witness),
        });
    }

    let concurrency = portfolio_verification_concurrency();
    if concurrency.enabled {
        return portfolio_prove_with_owned_verification(
            store,
            config,
            provider,
            statement,
            systems,
            &concurrency,
        );
    }

    let mut per_system = Vec::with_capacity(systems.len());
    let mut winner: Option<FormalSystem> = None;

    for system in systems {
        // In mock mode every backend is available; in live mode probe the real
        // toolchain so an absent system degrades to `unavailable` rather than
        // silently running the mock backend and falsely certifying.
        let available = config.prover_mock || backend_for(config, system, false).available();
        if !available {
            per_system.push(SystemAttempt {
                system,
                verified: false,
                available: false,
                code: None,
                report: None,
                duration_ms: 0,
                error: None,
            });
            continue;
        }

        let started = Instant::now();
        let attempt = match generate_and_verify(store, config, provider, system, statement) {
            Ok((code, report)) => {
                let verified = report.live && report.lexically_verified;
                if verified && winner.is_none() {
                    winner = Some(system);
                }
                SystemAttempt {
                    system,
                    verified,
                    available: true,
                    code: Some(code),
                    report: Some(report),
                    duration_ms: started.elapsed().as_millis(),
                    error: None,
                }
            }
            Err(e) => SystemAttempt {
                system,
                verified: false,
                available: true,
                code: None,
                report: None,
                duration_ms: started.elapsed().as_millis(),
                error: Some(e.to_string()),
            },
        };
        per_system.push(attempt);
    }

    let any_verified = winner.is_some();

    store.event(
        None,
        None,
        "portfolio_prove.completed",
        winner.map(|s| s.as_str()).unwrap_or("none"),
        json!({
            "statement": statement,
            "winner": winner.map(|s| s.as_str()),
            "any_verified": any_verified,
            "attempts": per_system
                .iter()
                .map(|a| json!({
                    "system": a.system.as_str(),
                    "verified": a.verified,
                    "available": a.available,
                    "duration_ms": a.duration_ms,
                }))
                .collect::<Vec<_>>(),
        }),
    )?;

    Ok(PortfolioResult {
        statement: statement.to_string(),
        winner,
        any_verified,
        per_system,
        // Unreachable-with-a-refutation by construction: a refuting screen
        // returns above, before any system is attempted.
        refutation: None,
    })
}

/// The ladder policy [`portfolio_prove`] runs under.
///
/// `THEOREMATA_PORTFOLIO_FAST_REFUTE` is the explicit, environment-only opt-in to
/// the tier-1 pre-filter. Absent it, this is [`LadderConfig::default`] and
/// [`portfolio_prove`] is exactly the sequential portfolio: the registered
/// falsifier rung is never invoked, because registering a rung does not enable
/// its tier. There is deliberately no implicit default — the established API
/// pays for N backends unless an operator opts out of that.
///
/// Tier 2 (`cheap_decide`) stays disabled: the portfolio registers no tier-2
/// rung, so enabling it would only add an empty pass.
fn portfolio_ladder_config() -> LadderConfig {
    ladder_config_from_value(
        std::env::var("THEOREMATA_PORTFOLIO_FAST_REFUTE")
            .ok()
            .as_deref(),
    )
}

/// Falsey (absent, empty, `0`, `false`, `off`, `no`) and every unrecognized value
/// leave the ladder at its kernel-only default; only `1`/`true`/`on`/`yes` enable
/// tier 1. Unrecognized input is treated as *off* rather than as an error: a
/// typo'd variable must not silently start skipping backend attempts.
fn ladder_config_from_value(value: Option<&str>) -> LadderConfig {
    let Some(value) = value.map(str::trim) else {
        return LadderConfig::default();
    };
    let normalized = value.to_ascii_lowercase();
    if matches!(normalized.as_str(), "1" | "true" | "on" | "yes") {
        return LadderConfig {
            fast_refute: RungConfig::enabled(RungBudget::default()),
            ..LadderConfig::default()
        };
    }
    LadderConfig::default()
}

/// Parse the explicit environment-only opt-in for owned verifier workers.
///
/// `THEOREMATA_PORTFOLIO_VERIFY_THREADS` is disabled when absent, empty, false,
/// or zero. `true`/`on`/`1` uses the machine parallelism; a positive integer
/// selects an explicit worker cap. There is deliberately no implicit platform
/// default: the established portfolio API stays sequential unless an operator
/// opts in.
fn portfolio_verification_concurrency() -> ConcurrentConfig {
    concurrency_from_value(
        std::env::var("THEOREMATA_PORTFOLIO_VERIFY_THREADS")
            .ok()
            .as_deref(),
    )
}

fn concurrency_from_value(value: Option<&str>) -> ConcurrentConfig {
    let Some(value) = value.map(str::trim) else {
        return ConcurrentConfig::sequential();
    };
    let normalized = value.to_ascii_lowercase();
    if matches!(normalized.as_str(), "" | "0" | "false" | "off" | "no") {
        return ConcurrentConfig::sequential();
    }
    if matches!(normalized.as_str(), "1" | "true" | "on" | "yes") {
        return ConcurrentConfig::with_threads(ConcurrentConfig::default_parallelism());
    }
    match normalized.parse::<usize>() {
        Ok(threads) if threads > 0 => ConcurrentConfig::with_threads(threads),
        _ => ConcurrentConfig::sequential(),
    }
}

/// Run the opt-in portfolio shape. Candidate generation and all store writes
/// remain sequential; only owned backend verification jobs cross to workers.
/// Results retain the requested system order, and each system selects the first
/// source that passed the gate in generation order.
fn portfolio_prove_with_owned_verification(
    store: &Store,
    config: &Config,
    provider: &dyn ModelProvider,
    statement: &str,
    systems: Vec<FormalSystem>,
    concurrency: &ConcurrentConfig,
) -> Result<PortfolioResult> {
    struct PreparedSystem {
        output_index: usize,
        task_start: usize,
        task_end: usize,
        generation_duration_ms: u128,
    }

    let mut attempts: Vec<Option<SystemAttempt>> = vec![None; systems.len()];
    let mut prepared = Vec::new();
    let mut tasks = Vec::new();

    // Provider/model calls are intentionally in requested-system order and stay
    // on this thread. The worker receives only the owned verification input.
    for (output_index, system) in systems.iter().copied().enumerate() {
        let live_available = !config.prover_mock && backend_for(config, system, false).available();
        if !config.prover_mock && !live_available {
            attempts[output_index] = Some(SystemAttempt {
                system,
                verified: false,
                available: false,
                code: None,
                report: None,
                duration_ms: 0,
                error: None,
            });
            continue;
        }

        let generation_started = Instant::now();
        let generated = match generate_candidates_for_verification(
            config,
            provider,
            system,
            statement,
            live_available,
        ) {
            Ok(generated) => generated,
            Err(error) => {
                attempts[output_index] = Some(SystemAttempt {
                    system,
                    verified: false,
                    available: true,
                    code: None,
                    report: None,
                    duration_ms: generation_started.elapsed().as_millis(),
                    error: Some(error.to_string()),
                });
                continue;
            }
        };
        let generation_duration_ms = generation_started.elapsed().as_millis();
        let task_start = tasks.len();
        let mode = if config.prover_mock {
            VerificationMode::Mock
        } else {
            VerificationMode::Live
        };
        for code in generated.candidates {
            tasks.push(match mode {
                VerificationMode::Live => {
                    OwnedVerificationTask::live(config.clone(), system, code, statement.to_string())
                }
                VerificationMode::Mock => {
                    OwnedVerificationTask::mock(config.clone(), system, code, statement.to_string())
                }
            });
        }
        prepared.push(PreparedSystem {
            output_index,
            task_start,
            task_end: tasks.len(),
            generation_duration_ms,
        });
    }

    let results = run_owned_formal_system_verifications(tasks, concurrency);
    for prepared_system in prepared {
        let system_results = &results[prepared_system.task_start..prepared_system.task_end];
        // This matches best_of_n: accept the first passing candidate, otherwise
        // retain the final candidate that produced a verifier report. A worker
        // error is skipped rather than becoming a false verification result.
        let selected = system_results
            .iter()
            .find(|result| result.gate_passed())
            .or_else(|| {
                system_results
                    .iter()
                    .rev()
                    .find(|result| result.report.is_some())
            });

        let attempt = match selected {
            Some(selected) => SystemAttempt {
                system: selected.system,
                verified: selected.live_verified(),
                available: selected.available,
                code: Some(selected.code.clone()),
                report: selected.report.clone(),
                duration_ms: prepared_system.generation_duration_ms
                    + system_results
                        .iter()
                        .map(|result| result.duration_ms)
                        .max()
                        .unwrap_or(0),
                error: selected.error.clone(),
            },
            None => SystemAttempt {
                system: systems[prepared_system.output_index],
                verified: false,
                available: system_results.iter().any(|result| result.available),
                code: None,
                report: None,
                duration_ms: prepared_system.generation_duration_ms
                    + system_results
                        .iter()
                        .map(|result| result.duration_ms)
                        .max()
                        .unwrap_or(0),
                error: Some("no proof candidate reached a verification verdict".into()),
            },
        };
        attempts[prepared_system.output_index] = Some(attempt);
    }

    let per_system: Vec<SystemAttempt> = attempts
        .into_iter()
        .map(|attempt| attempt.expect("every requested system receives one portfolio result"))
        .collect();
    let winner = per_system
        .iter()
        .find(|attempt| attempt.verified)
        .map(|attempt| attempt.system);
    let any_verified = winner.is_some();

    store.event(
        None,
        None,
        "portfolio_prove.completed",
        winner.map(|system| system.as_str()).unwrap_or("none"),
        json!({
            "statement": statement,
            "winner": winner.map(|system| system.as_str()),
            "any_verified": any_verified,
            "verification_concurrency": {
                "enabled": concurrency.enabled,
                "max_threads": concurrency.max_threads,
            },
            "attempts": per_system
                .iter()
                .map(|attempt| json!({
                    "system": attempt.system.as_str(),
                    "verified": attempt.verified,
                    "available": attempt.available,
                    "duration_ms": attempt.duration_ms,
                    "live": attempt.report.as_ref().map(|report| report.live),
                }))
                .collect::<Vec<_>>(),
        }),
    )?;

    Ok(PortfolioResult {
        statement: statement.to_string(),
        winner,
        any_verified,
        per_system,
        refutation: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ModelRequest, ModelResponse};
    use crate::provider::OfflineProvider;
    use std::path::Path;

    fn store() -> Store {
        Store::open(Path::new(":memory:")).unwrap()
    }

    /// Force the mock backend (no toolchain assumed) so the portfolio is exercised
    /// deterministically offline.
    fn mock_config() -> Config {
        Config {
            prover_mock: true,
            ..Config::default()
        }
    }

    /// A provider that injects a system-native ESCAPE HATCH (read from the
    /// request context's `system` tag) so the real source scan rejects every
    /// candidate — used to prove "no system certifies ⇒ no winner".
    struct EscapeHatchProvider;
    impl ModelProvider for EscapeHatchProvider {
        fn complete(&self, req: &ModelRequest) -> Result<ModelResponse> {
            let system = req.context["system"].as_str().unwrap_or("lean");
            let code = match system {
                "rocq" => "Theorem t : True.\nProof.\n  exact I.\nAdmitted.",
                "isabelle" => {
                    "theory Scratch\n imports Main\nbegin\ntheorem t: \"True\" sorry\nend"
                }
                _ => "theorem t : True := by sorry",
            };
            Ok(ModelResponse {
                content: json!({ "code": code }),
                model: "mock".into(),
                provider: "mock".into(),
            })
        }
        fn name(&self) -> &str {
            // Non-"offline" so the provider path (not the stub) is exercised.
            "command"
        }
    }

    use crate::verification_ladder::WitnessKind;
    use std::cell::Cell;

    /// A tier-1 rung that always refutes, counting its invocations.
    struct MockRefuter {
        calls: Cell<usize>,
    }
    impl MockRefuter {
        fn new() -> Self {
            Self {
                calls: Cell::new(0),
            }
        }
    }
    impl CheapRung<str> for MockRefuter {
        fn name(&self) -> &str {
            "mock-refuter"
        }
        fn probe(&self, _statement: &str, _budget: &RungBudget) -> RungVerdict {
            self.calls.set(self.calls.get() + 1);
            RungVerdict::Refuted(RefutationWitness::counterexample(
                "mock-refuter",
                "claim fails at n = 4",
                vec![("n".to_string(), "4".to_string())],
            ))
        }
    }

    /// A tier-1 rung that always abstains, counting its invocations.
    struct MockAbstainer {
        calls: Cell<usize>,
    }
    impl MockAbstainer {
        fn new() -> Self {
            Self {
                calls: Cell::new(0),
            }
        }
    }
    impl CheapRung<str> for MockAbstainer {
        fn name(&self) -> &str {
            "mock-abstainer"
        }
        fn probe(&self, _statement: &str, _budget: &RungBudget) -> RungVerdict {
            self.calls.set(self.calls.get() + 1);
            RungVerdict::Abstain
        }
    }

    /// Only tier 1 enabled — the shape a caller opting in to fast refutation uses.
    /// Derived from the production parser so the tests below exercise exactly the
    /// config an operator gets, not a hand-rolled look-alike.
    fn fast_refute_enabled() -> LadderConfig {
        ladder_config_from_value(Some("1"))
    }

    /// The observable content of a portfolio run, for parity comparisons.
    /// `duration_ms` is deliberately excluded: it is wall-clock and not part of
    /// the behavior under test.
    type Fingerprint = (
        String,
        Option<FormalSystem>,
        bool,
        Vec<(FormalSystem, bool, bool, bool, Option<bool>, Option<bool>)>,
    );

    fn fingerprint(result: &PortfolioResult) -> Fingerprint {
        (
            result.statement.clone(),
            result.winner,
            result.any_verified,
            result
                .per_system
                .iter()
                .map(|attempt| {
                    (
                        attempt.system,
                        attempt.verified,
                        attempt.available,
                        attempt.error.is_some(),
                        attempt.report.as_ref().map(|report| report.live),
                        attempt
                            .report
                            .as_ref()
                            .map(|report| report.lexically_verified),
                    )
                })
                .collect(),
        )
    }

    #[test]
    fn the_ladder_at_defaults_is_byte_identical_to_the_sequential_portfolio() {
        // Registering a rung must NOT enable its tier: the default path has to
        // stay exactly today's portfolio — same winner, same order, same
        // SystemAttempt records — even with a rung that would refute everything.
        //
        // The config compared against below IS the one `portfolio_prove` uses when
        // the opt-in env var is unset, so this parity claim covers the real
        // default path and not just a hand-written `LadderConfig::default()`.
        assert_eq!(
            ladder_config_from_value(None),
            LadderConfig::default(),
            "an unset opt-in must resolve to the kernel-only default"
        );
        for &(name, use_escape_hatch) in &[("offline", false), ("escape-hatch", true)] {
            let provider: &dyn ModelProvider = if use_escape_hatch {
                &EscapeHatchProvider
            } else {
                &OfflineProvider
            };

            let baseline =
                portfolio_prove(&store(), &mock_config(), provider, "True", &[]).unwrap();

            let refuter = MockRefuter::new();
            let with_ladder = portfolio_prove_with_ladder(
                &store(),
                &mock_config(),
                provider,
                "True",
                &[],
                &[&refuter],
                LadderConfig::default(),
            )
            .unwrap();

            assert_eq!(
                refuter.calls.get(),
                0,
                "{name}: a disabled tier must never invoke a registered rung"
            );
            assert!(
                with_ladder.refutation.is_none(),
                "{name}: no refutation on the default path"
            );
            assert!(
                baseline.refutation.is_none(),
                "{name}: portfolio_prove never refutes at defaults"
            );
            assert_eq!(
                fingerprint(&baseline),
                fingerprint(&with_ladder),
                "{name}: default ladder diverged from the sequential portfolio"
            );
            assert_eq!(with_ladder.per_system.len(), 3, "{name}: all three ran");
        }
    }

    #[test]
    fn an_enabled_refuting_rung_skips_every_backend_attempt() {
        let refuter = MockRefuter::new();
        let result = portfolio_prove_with_ladder(
            &store(),
            &mock_config(),
            &OfflineProvider,
            "every integer is even",
            &[],
            &[&refuter],
            fast_refute_enabled(),
        )
        .unwrap();

        assert_eq!(refuter.calls.get(), 1, "the cheap rung ran exactly once");
        // The whole point: N backend calls were never paid for.
        assert!(
            result.per_system.is_empty(),
            "no backend attempt may be recorded after a refutation"
        );
        assert_eq!(result.winner, None);
        assert!(!result.any_verified);

        // ...and the witness that routes the repair survives onto the result.
        let witness = result.refutation.expect("the refutation is surfaced");
        assert_eq!(witness.rung, "mock-refuter");
        assert_eq!(witness.kind, WitnessKind::Counterexample);
        assert_eq!(witness.instance, vec![("n".to_string(), "4".to_string())]);
        assert!(witness.is_actionable());
    }

    #[test]
    fn an_enabled_abstaining_rung_produces_todays_exact_result() {
        let baseline =
            portfolio_prove(&store(), &mock_config(), &OfflineProvider, "True", &[]).unwrap();

        let abstainer = MockAbstainer::new();
        let result = portfolio_prove_with_ladder(
            &store(),
            &mock_config(),
            &OfflineProvider,
            "True",
            &[],
            &[&abstainer],
            fast_refute_enabled(),
        )
        .unwrap();

        assert_eq!(abstainer.calls.get(), 1, "the enabled tier ran the rung");
        assert!(
            result.refutation.is_none(),
            "abstention is not a refutation"
        );
        assert_eq!(
            fingerprint(&baseline),
            fingerprint(&result),
            "an abstaining ladder must fall through to today's per-system loop"
        );
    }

    #[test]
    fn only_a_counterexample_verdict_refutes() {
        // The falsifier adapter is deliberately asymmetric: an exhausted bounded
        // sweep has proved nothing, so every non-`counterexample` verdict — and
        // an outright probe failure — must abstain.
        let rung = FalsifierRung::new(&OfflineProvider);
        // OfflineProvider yields `no_model`, which is NOT a refutation.
        assert_eq!(
            rung.probe(
                "every even integer has an even square",
                &RungBudget::default()
            ),
            RungVerdict::Abstain
        );
        // Nor is a spec the model declares inapplicable (`not_applicable`).
        let rung = FalsifierRung::new(&EscapeHatchProvider);
        assert_eq!(
            rung.probe("True", &RungBudget::default()),
            RungVerdict::Abstain
        );
    }

    #[test]
    fn a_falsifying_assignment_becomes_ordered_witness_bindings() {
        // Sorted by variable name so the witness serializes deterministically.
        assert_eq!(
            assignment_bindings(Some(&json!({ "n": 4, "k": -1, "s": "x" }))),
            vec![
                ("k".to_string(), "-1".to_string()),
                ("n".to_string(), "4".to_string()),
                ("s".to_string(), "x".to_string()),
            ]
        );
        assert!(assignment_bindings(None).is_empty());
        assert!(assignment_bindings(Some(&Value::Null)).is_empty());
        // A non-object assignment is still carried, not silently dropped.
        assert_eq!(
            assignment_bindings(Some(&json!(7))),
            vec![("assignment".to_string(), "7".to_string())]
        );
    }

    #[test]
    fn mock_gate_successes_are_not_portfolio_certifications() {
        let store = store();
        let config = mock_config();
        // OfflineProvider → each system generates its native trivially-true stub,
        // which the mock backend accepts lexically. That is diagnostic only.
        let result = portfolio_prove(&store, &config, &OfflineProvider, "True", &[]).unwrap();

        assert_eq!(result.per_system.len(), 3, "all three systems attempted");
        for attempt in &result.per_system {
            assert!(
                attempt.available,
                "{}: mock backend is available",
                attempt.system
            );
            assert!(!attempt.verified, "mock proof must not certify");
            assert!(!attempt.report.as_ref().unwrap().live);
        }
        assert!(!result.any_verified);
        assert_eq!(result.winner, None);
    }

    #[test]
    fn no_winner_when_every_candidate_has_an_escape_hatch() {
        let store = store();
        let config = mock_config();
        let result = portfolio_prove(&store, &config, &EscapeHatchProvider, "True", &[]).unwrap();

        assert_eq!(result.per_system.len(), 3, "all three still attempted");
        for attempt in &result.per_system {
            assert!(attempt.available);
            assert!(
                !attempt.verified,
                "{}: escape-hatch proof must NOT certify",
                attempt.system
            );
            // The mandatory source scan flags the escape hatch.
            let report = attempt.report.as_ref().unwrap();
            assert!(
                !report.lexical_clean,
                "{}: scan must flag it",
                attempt.system
            );
        }
        assert!(!result.any_verified);
        assert_eq!(result.winner, None);
    }

    #[test]
    fn respects_an_explicit_system_subset() {
        let store = store();
        let config = mock_config();
        let result = portfolio_prove(
            &store,
            &config,
            &OfflineProvider,
            "True",
            &[FormalSystem::Rocq],
        )
        .unwrap();

        assert_eq!(result.per_system.len(), 1);
        assert_eq!(result.per_system[0].system, FormalSystem::Rocq);
        assert!(!result.any_verified);
        assert_eq!(result.winner, None);
    }

    #[test]
    fn owned_verification_keeps_system_order_and_mock_provenance() {
        let store = store();
        let config = mock_config();
        let result = portfolio_prove_with_owned_verification(
            &store,
            &config,
            &OfflineProvider,
            "True",
            vec![FormalSystem::Rocq, FormalSystem::Lean],
            &ConcurrentConfig::with_threads(2),
        )
        .unwrap();

        assert_eq!(
            result
                .per_system
                .iter()
                .map(|attempt| attempt.system)
                .collect::<Vec<_>>(),
            vec![FormalSystem::Rocq, FormalSystem::Lean],
            "worker completion order must never affect portfolio order"
        );
        assert_eq!(result.winner, None);
        for attempt in &result.per_system {
            assert!(!attempt.verified);
            assert!(
                !attempt.report.as_ref().unwrap().live,
                "mock mode must remain visibly mock after owned verification"
            );
        }
    }

    #[test]
    fn verification_thread_opt_in_is_disabled_unless_explicitly_enabled() {
        for value in [
            None,
            Some(""),
            Some("0"),
            Some("false"),
            Some("off"),
            Some("bogus"),
        ] {
            assert!(
                !concurrency_from_value(value).enabled,
                "{value:?} must preserve the sequential default"
            );
        }
        for value in [Some("1"), Some("true"), Some("on"), Some("3")] {
            assert!(
                concurrency_from_value(value).enabled,
                "{value:?} must explicitly enable owned verifier workers"
            );
        }
        assert_eq!(concurrency_from_value(Some("3")).max_threads, 3);
    }

    #[test]
    fn fast_refute_opt_in_is_disabled_unless_explicitly_enabled() {
        // Absent / empty / falsey / unrecognized all leave the ladder kernel-only.
        // A typo'd variable must never silently start skipping backend attempts.
        for value in [
            None,
            Some(""),
            Some("   "),
            Some("0"),
            Some("false"),
            Some("off"),
            Some("no"),
            Some("bogus"),
            Some("2"),
        ] {
            let config = ladder_config_from_value(value);
            assert_eq!(
                config,
                LadderConfig::default(),
                "{value:?} must preserve the kernel-only default"
            );
            assert!(
                !config.fast_refute.enabled,
                "{value:?} must not enable tier 1"
            );
        }
        // Only the explicit truthy set opts in, case- and whitespace-insensitively.
        for value in [
            Some("1"),
            Some("true"),
            Some("TRUE"),
            Some("on"),
            Some("yes"),
            Some(" 1 "),
        ] {
            let config = ladder_config_from_value(value);
            assert!(
                config.fast_refute.enabled,
                "{value:?} must enable the fast_refute tier"
            );
            // Tier 2 is never enabled: the portfolio registers no tier-2 rung.
            assert!(
                !config.cheap_decide.enabled,
                "{value:?} must not enable tier 2"
            );
            assert_eq!(config.fast_refute.budget, RungBudget::default());
        }
    }

    #[test]
    fn the_env_derived_opt_in_actually_fires_a_refuting_rung() {
        // The bug this closes: before the opt-in existed, `portfolio_prove` always
        // passed `LadderConfig::default()`, so no registered rung could ever fire.
        // With the enabled config the ladder reaches tier 1 and short-circuits.
        let refuter = MockRefuter::new();
        let result = portfolio_prove_with_ladder(
            &store(),
            &mock_config(),
            &OfflineProvider,
            "every integer is even",
            &[],
            &[&refuter],
            ladder_config_from_value(Some("true")),
        )
        .unwrap();

        assert_eq!(refuter.calls.get(), 1, "the enabled tier ran the rung");
        assert!(result.refutation.is_some(), "the refutation is surfaced");
        assert!(result.per_system.is_empty(), "no backend call was paid for");

        // ...and the same rung under the unset-env config stays untouched, which is
        // what keeps `portfolio_prove` byte-identical to today by default.
        let quiet = MockRefuter::new();
        let unchanged = portfolio_prove_with_ladder(
            &store(),
            &mock_config(),
            &OfflineProvider,
            "every integer is even",
            &[],
            &[&quiet],
            ladder_config_from_value(None),
        )
        .unwrap();
        assert_eq!(quiet.calls.get(), 0, "the unset env must not invoke a rung");
        assert!(unchanged.refutation.is_none());
        assert_eq!(unchanged.per_system.len(), 3, "all three still attempted");
    }

    #[test]
    fn live_portfolio_certifies_a_trivial_statement_when_a_toolchain_is_present() {
        let config = Config::default();
        // Probe: only meaningful if at least one live backend is available.
        let any_live = ALL_SYSTEMS
            .iter()
            .any(|&s| backend_for(&config, s, false).available());
        if !any_live {
            eprintln!("skip: no live formal toolchain present");
            return;
        }
        let store = store();
        // OfflineProvider → per-system trivially-true stubs; live gates compile
        // them, unavailable systems degrade to `available: false` (not errors).
        let result = portfolio_prove(&store, &config, &OfflineProvider, "True", &[]).unwrap();

        assert_eq!(result.per_system.len(), 3);
        assert!(
            result.any_verified,
            "at least one live backend should certify a trivial statement"
        );
        assert!(result.winner.is_some());
        // No available system should have errored on a trivial statement.
        for attempt in &result.per_system {
            if attempt.available {
                assert!(
                    attempt.error.is_none(),
                    "{}: available backend errored: {:?}",
                    attempt.system,
                    attempt.error
                );
            }
        }
    }
}
