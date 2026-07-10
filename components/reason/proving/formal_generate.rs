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
use serde_json::{json, Value};

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
            "hammer_candidate": hammer_candidate.is_some(),
        }),
    )?;

    Ok((sampled.value.code, sampled.value.report))
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
    let mode: Option<&str> = if config.prover_mock { Some("mock") } else { None };
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
    if !output.get("success").and_then(Value::as_bool).unwrap_or(false) {
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
        FormalSystem::Isabelle => format!(
            "theory T\n  imports Main\nbegin\n\ntheorem t: \"{goal}\"\n  {tactic}\n\nend\n"
        ),
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
        FormalSystem::Metamath => ("Metamath", "unverified proof shortcuts or malformed $p declarations"),
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
