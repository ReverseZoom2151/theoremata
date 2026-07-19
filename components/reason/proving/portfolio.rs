//! Portfolio proving (Phase 5): attempt a conjecture across every requested
//! formal system and take whichever backend certifies first.
//!
//! Where [`generate_and_verify`](crate::formal_generate::generate_and_verify)
//! produces-and-verifies a proof for ONE [`FormalSystem`], [`portfolio_prove`]
//! fans the same conjecture out across Lean, Rocq, and Isabelle, records a
//! per-system verdict, and names the WINNER — the first system whose report is
//! `lexically_verified`.
//!
//! It is sequential and deterministic by default. An explicitly enabled owned
//! verification stage can run already-generated candidates on worker threads;
//! provider calls, database writes, result order, and winner selection remain
//! deterministic on the caller thread. A system whose toolchain is absent
//! contributes an `available: false` entry, and a system that errors out records
//! its error — neither aborts the sibling attempts.

use crate::{
    concurrent::ConcurrentConfig,
    config::Config,
    db::Store,
    formal_generate::{generate_and_verify, generate_candidates_for_verification},
    formalize_portfolio::{
        run_owned_formal_system_verifications, OwnedVerificationTask, VerificationMode,
    },
    prover::{
        formal::{backend_for, FormalSystem},
        model::VerificationReport,
    },
    provider::ModelProvider,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;
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
    /// Whether the attempt's report is `lexically_verified`.
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
}

/// Attempt `statement` across each of `systems` (defaulting to all three when
/// empty), taking the first system that certifies as the winner.
///
/// Each system is attempted independently and sequentially:
/// * If its toolchain is absent (live mode only), it contributes an
///   `available: false` entry and is skipped — no error.
/// * Otherwise [`generate_and_verify`] runs; its `lexically_verified` verdict
///   (from the mock backend's real source-scan gate offline, or the live
///   3+1-layer gate when the toolchain is present) decides whether it verified.
/// * A generation/backend error is recorded on the entry, not propagated — one
///   failing system never aborts the others.
pub fn portfolio_prove(
    store: &Store,
    config: &Config,
    provider: &dyn ModelProvider,
    statement: &str,
    systems: &[FormalSystem],
) -> Result<PortfolioResult> {
    let systems: Vec<FormalSystem> = if systems.is_empty() {
        ALL_SYSTEMS.to_vec()
    } else {
        systems.to_vec()
    };

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
                let verified = report.lexically_verified;
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
    })
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
                verified: selected.gate_passed(),
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

    #[test]
    fn attempts_all_three_and_picks_a_winner_offline() {
        let store = store();
        let config = mock_config();
        // OfflineProvider → each system generates its native trivially-true stub,
        // which the mock backend (canned kernel + REAL source scan) certifies.
        let result = portfolio_prove(&store, &config, &OfflineProvider, "True", &[]).unwrap();

        assert_eq!(result.per_system.len(), 3, "all three systems attempted");
        for attempt in &result.per_system {
            assert!(
                attempt.available,
                "{}: mock backend is available",
                attempt.system
            );
            assert!(
                attempt.verified,
                "{}: trivial stub should certify",
                attempt.system
            );
        }
        assert!(result.any_verified);
        // The winner is the FIRST verifying system in ALL_SYSTEMS order (Lean).
        assert_eq!(result.winner, Some(FormalSystem::Lean));
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
        assert!(result.any_verified);
        assert_eq!(result.winner, Some(FormalSystem::Rocq));
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
        assert_eq!(result.winner, Some(FormalSystem::Rocq));
        for attempt in &result.per_system {
            assert!(attempt.verified);
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
