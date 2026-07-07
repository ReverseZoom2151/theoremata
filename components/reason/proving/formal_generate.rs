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
    config::Config,
    db::Store,
    model::ModelRequest,
    prover::{
        formal::{backend_for, FormalSystem},
        model::VerificationReport,
    },
    provider::ModelProvider,
    sampling,
};
use anyhow::{Context, Result};
use serde_json::json;

/// How many candidate proofs to sample before giving up (best-of-N).
const N: usize = 3;

/// One generated candidate together with its verification verdict.
struct Candidate {
    code: String,
    report: VerificationReport,
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

    let selection = sampling::best_of_n(
        N,
        |_i| -> Result<Candidate> {
            let code = generate_once(provider, system, statement)?;
            let report = backend.verify(config, &code, statement)?;
            Ok(Candidate { code, report })
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
        }),
    )?;

    Ok((sampled.value.code, sampled.value.report))
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
    }
}

/// The system-specific generation instruction.
fn task_for(system: FormalSystem, statement: &str) -> String {
    let (lang, banned) = match system {
        FormalSystem::Lean => ("Lean 4", "sorry, admit, or unsafe axioms"),
        FormalSystem::Rocq => ("Coq (Rocq)", "admit, Admitted, or bare Axiom"),
        FormalSystem::Isabelle => ("Isabelle/Isar", "sorry, oops, or an oracle"),
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelResponse;
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
    fn returns_code_and_report_for_each_system() {
        let store = store();
        let config = mock_config();
        for (system, code) in [
            (FormalSystem::Lean, "theorem t : True := trivial"),
            (FormalSystem::Rocq, "Theorem t : True.\nProof. exact I. Qed."),
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
