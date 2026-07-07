//! Portfolio proving (Phase 5): attempt a conjecture across every requested
//! formal system and take whichever backend certifies first.
//!
//! Where [`generate_and_verify`](crate::formal_generate::generate_and_verify)
//! produces-and-verifies a proof for ONE [`FormalSystem`], [`portfolio_prove`]
//! fans the same conjecture out across Lean, Rocq, and Isabelle, records a
//! per-system verdict, and names the WINNER — the first system whose report is
//! `lexically_verified`.
//!
//! It is deliberately simple and SEQUENTIAL (a real race across prover processes
//! is out of scope; sequential is deterministic and reproducible), but each
//! system's attempt is INDEPENDENT: a system whose toolchain is absent
//! contributes an `available: false` entry, and a system that errors out records
//! its error — neither aborts the sibling attempts.

use crate::{
    config::Config,
    db::Store,
    formal_generate::generate_and_verify,
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
pub const ALL_SYSTEMS: [FormalSystem; 3] =
    [FormalSystem::Lean, FormalSystem::Rocq, FormalSystem::Isabelle];

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
        let result =
            portfolio_prove(&store, &config, &OfflineProvider, "True", &[]).unwrap();

        assert_eq!(result.per_system.len(), 3, "all three systems attempted");
        for attempt in &result.per_system {
            assert!(attempt.available, "{}: mock backend is available", attempt.system);
            assert!(attempt.verified, "{}: trivial stub should certify", attempt.system);
        }
        assert!(result.any_verified);
        // The winner is the FIRST verifying system in ALL_SYSTEMS order (Lean).
        assert_eq!(result.winner, Some(FormalSystem::Lean));
    }

    #[test]
    fn no_winner_when_every_candidate_has_an_escape_hatch() {
        let store = store();
        let config = mock_config();
        let result =
            portfolio_prove(&store, &config, &EscapeHatchProvider, "True", &[]).unwrap();

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
            assert!(!report.lexical_clean, "{}: scan must flag it", attempt.system);
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
        let result =
            portfolio_prove(&store, &config, &OfflineProvider, "True", &[]).unwrap();

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
