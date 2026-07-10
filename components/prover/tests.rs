//! Integration tests for proof jobs, AttemptRun, and Aristotle mock backend.

use crate::{
    config::Config,
    db::Store,
    prover::{
        aristotle,
        attempt_run,
        external,
        formal::{FormalBackend, FormalSystem, ProofSession, SessionError},
        isabelle,
        lean,
        model::{AttemptRunStatus, ProverJobStatus},
        proof_job,
        rocq,
    },
};
use serde_json::json;
use std::path::Path;
use std::str::FromStr;

fn test_config(dir: &Path) -> Config {
    let mut c = Config::default();
    c.database = dir.join("test.db");
    c.artifacts = dir.join("artifacts");
    // Force mock via config, NOT the process-global env — `std::env::set_var`
    // races against every other test's `env::var` read under parallel execution.
    c.prover_mock = true;
    c
}

#[test]
fn proof_task_submit_poll_result_mock() {
    let tmp = tempfile::tempdir().unwrap();
    let config = test_config(tmp.path());
    let store = Store::open(&config.database).unwrap();
    let project = store.create_project("p", "True").unwrap();

    let task = aristotle::build_task(
        Some(project.id.clone()),
        None,
        "True",
        "Theoremata.MockThm",
        &config,
    );
    let job = proof_job::submit(&store, &config, task, None).unwrap();
    assert_eq!(job.status, ProverJobStatus::Submitted);

    let mid = proof_job::poll(&store, &config, &job.id, None).unwrap();
    assert_eq!(mid.status, ProverJobStatus::InProgress);

    let done = proof_job::poll(&store, &config, &job.id, None).unwrap();
    assert_eq!(done.status, ProverJobStatus::Proved);
    assert!(done.result.as_ref().unwrap().formal_code.is_some());
    let verification = done.result.as_ref().unwrap().verification.as_ref().unwrap();
    assert!(verification.lexical_clean);
    assert!(verification.axioms_clean);

    let result = proof_job::result(&store, &job.id).unwrap();
    assert_eq!(result.status, ProverJobStatus::Proved);
}

#[test]
fn attempt_run_start_to_completion_writes_artifacts() {
    let tmp = tempfile::tempdir().unwrap();
    let config = test_config(tmp.path());
    let store = Store::open(&config.database).unwrap();
    let project = store.create_project("p", "True").unwrap();

    let record = attempt_run::start(
        &store,
        &config,
        &project.id,
        None,
        json!({"statement": "True", "theorem_name": "Theoremata.RunThm"}),
    )
    .unwrap();
    assert_eq!(record.status, AttemptRunStatus::Running);
    assert!(record.artifacts_dir.join("input.json").exists());

    let out = attempt_run::run_to_completion(&store, &config, &record.id, 8, None).unwrap();
    assert_eq!(out.status, AttemptRunStatus::Completed);
    assert!(out.proof_result.is_some());
    assert!(out.artifacts_dir.join("output.json").exists());
    assert!(out.artifacts_dir.join("lean/solution.lean").exists());
    assert!(out.artifacts_dir.join("verifier/report.json").exists());
}

#[test]
fn rocq_submit_poll_result_mock() {
    let tmp = tempfile::tempdir().unwrap();
    let config = test_config(tmp.path());
    let store = Store::open(&config.database).unwrap();
    let project = store.create_project("p", "True").unwrap();

    let task = rocq::build_task(
        Some(project.id.clone()),
        None,
        "True",
        "Theoremata.RocqThm",
        &config,
    );
    assert_eq!(task.backend, "rocq");
    assert_eq!(task.system, FormalSystem::Rocq);

    let job = proof_job::submit(&store, &config, task, None).unwrap();
    assert_eq!(job.status, ProverJobStatus::Submitted);

    let mid = proof_job::poll(&store, &config, &job.id, None).unwrap();
    assert_eq!(mid.status, ProverJobStatus::InProgress);

    let done = proof_job::poll(&store, &config, &job.id, None).unwrap();
    assert_eq!(done.status, ProverJobStatus::Proved);
    let result = done.result.as_ref().unwrap();
    let code = result.formal_code.as_ref().unwrap();
    assert!(code.contains("Qed."), "rocq mock must emit a .v proof: {code}");
    let v = result.verification.as_ref().unwrap();
    assert!(v.lexically_verified);
    assert!(v.axioms_clean);
    assert!(v.lexical_clean);
}

#[test]
fn isabelle_submit_poll_result_mock() {
    let tmp = tempfile::tempdir().unwrap();
    let config = test_config(tmp.path());
    let store = Store::open(&config.database).unwrap();
    let project = store.create_project("p", "True").unwrap();

    let task = isabelle::build_task(
        Some(project.id.clone()),
        None,
        "True",
        "Theoremata.IsabelleThm",
        &config,
    );
    assert_eq!(task.backend, "isabelle");
    assert_eq!(task.system, FormalSystem::Isabelle);

    let job = proof_job::submit(&store, &config, task, None).unwrap();
    assert_eq!(job.status, ProverJobStatus::Submitted);

    let mid = proof_job::poll(&store, &config, &job.id, None).unwrap();
    assert_eq!(mid.status, ProverJobStatus::InProgress);

    let done = proof_job::poll(&store, &config, &job.id, None).unwrap();
    assert_eq!(done.status, ProverJobStatus::Proved);
    let result = done.result.as_ref().unwrap();
    let code = result.formal_code.as_ref().unwrap();
    assert!(code.contains("by simp"), "isabelle mock must emit a .thy proof: {code}");
    let v = result.verification.as_ref().unwrap();
    assert!(v.lexically_verified);
    assert!(v.axioms_clean);
    assert!(v.lexical_clean);
}

#[test]
fn agda_and_metamath_submit_poll_result_mock() {
    let tmp = tempfile::tempdir().unwrap();
    let config = test_config(tmp.path());
    let store = Store::open(&config.database).unwrap();
    let project = store.create_project("p", "True").unwrap();

    for system in [FormalSystem::Agda, FormalSystem::Metamath] {
        let statement = match system {
            FormalSystem::Agda => "generated",
            FormalSystem::Metamath => "ph",
            _ => unreachable!(),
        };
        let task = external::build_task(
            Some(project.id.clone()),
            None,
            statement,
            "Theoremata.ExternalThm",
            &config,
            system,
        );
        let job = proof_job::submit(&store, &config, task, None).unwrap();
        let mid = proof_job::poll(&store, &config, &job.id, None).unwrap();
        assert_eq!(mid.status, ProverJobStatus::InProgress);
        let done = proof_job::poll(&store, &config, &job.id, None).unwrap();
        assert_eq!(done.status, ProverJobStatus::Proved);
        assert_eq!(done.task.system, system);
        assert!(done.result.as_ref().and_then(|r| r.verification.as_ref()).is_some());
    }
}

#[test]
fn isabelle_step_tactic_is_unsupported() {
    let mut backend = isabelle::IsabelleBackend::mock();
    let err = backend.step_tactic(0, "by simp").unwrap_err();
    assert!(
        matches!(err.downcast_ref::<SessionError>(), Some(SessionError::Unsupported(_))),
        "Isabelle step_tactic must return SessionError::Unsupported, got {err}"
    );
    // submit_unit IS supported and accepts the clean mock proof.
    let unit = backend.submit_unit("theorem t: \"True\" by simp").unwrap();
    assert!(unit.ok);
}

#[test]
fn rocq_step_tactic_is_supported() {
    let mut backend = rocq::RocqBackend::mock();
    let step = backend.step_tactic(0, "exact I").unwrap();
    assert!(step.finished);
    assert_eq!(step.state, 1);
}

#[test]
fn source_scan_rejects_escape_hatches() {
    let rocq_b = rocq::RocqBackend::mock();
    assert!(!rocq_b.source_scan("Theorem t: True. Admitted.").unwrap().clean);
    let isa_b = isabelle::IsabelleBackend::mock();
    assert!(!isa_b.source_scan("theorem t: \"True\" sorry").unwrap().clean);
}

#[test]
fn formal_system_from_str_and_display() {
    assert_eq!(FormalSystem::from_str("coq").unwrap(), FormalSystem::Rocq);
    assert_eq!(FormalSystem::from_str("Isabelle/HOL").unwrap(), FormalSystem::Isabelle);
    assert_eq!(FormalSystem::Lean.to_string(), "lean");
    assert_eq!(FormalSystem::default(), FormalSystem::Lean);
}

#[test]
fn lean_backend_precheck_rejects_toolchain_mismatch() {
    use crate::prover::formal::backend_for;
    use crate::prover::model::FormalProject;
    // A Lean backend pinned to one toolchain rejects a project pinned to another
    // BEFORE any compute (open-atp reject-on-mismatch).
    let mut backend = lean::LeanBackend::mock();
    backend.toolchain = Some("leanprover/lean4:v4.9.0".into());
    let project = FormalProject {
        system: FormalSystem::Lean,
        root: std::path::PathBuf::from("."),
        toolchain: Some("leanprover/lean4:v4.10.0".into()),
        imports: Vec::new(),
        metadata: json!({}),
    };
    let report = backend.precheck(Some(&project));
    assert!(!report.compatible);
    assert!(report.reason.contains("mismatch"));

    // A mock backend with no pin (the default) is always compatible — existing
    // behavior is unchanged.
    let cfg = Config::default();
    let plain = backend_for(&cfg, FormalSystem::Lean, true);
    assert!(plain.precheck(Some(&project)).compatible);
}

#[test]
fn lean_mock_compile_reports_per_declaration_status() {
    use crate::prover::formal::FormalBackend;
    let backend = lean::LeanBackend::mock();
    let cfg = Config::default();
    // The mock compile is a whole-file success; per_unit stays empty (single-unit
    // semantics), and the report round-trips through serde with the new field.
    let ws = backend
        .scaffold(&cfg, "theorem t : True := trivial", "t")
        .unwrap();
    let report = backend.compile(&ws).unwrap();
    assert!(report.compiled);
    let json = serde_json::to_string(&report).unwrap();
    let back: crate::prover::formal::CompileReport = serde_json::from_str(&json).unwrap();
    assert!(back.per_unit.is_empty());
}

#[test]
fn kernel_validate_flag_plumbs_and_template_exists() {
    // The config flag flows into the live Lean backend, and the referenced
    // LeanDojo template is present on disk (Task 4 wiring).
    let mut cfg = Config::default();
    assert!(!cfg.kernel_validate_proof, "off by default");
    cfg.kernel_validate_proof = true;
    let backend = lean::LeanBackend::live(&cfg);
    assert!(backend.kernel_validate);
    assert!(
        std::path::Path::new(lean::VALIDATE_PROOF_TEMPLATE).exists(),
        "validate_proof_template.lean must exist at {}",
        lean::VALIDATE_PROOF_TEMPLATE
    );
}

#[test]
fn lean_project_alias_deserializes() {
    // Old serialized ProofTask used `lean_project` and had no `system` field.
    let legacy = json!({
        "id": "t1",
        "project_id": null,
        "node_id": null,
        "theorem": {"repo": null, "commit": null, "file": null, "full_name": "T", "line": null},
        "lean_project": {"root": "/tmp/x", "toolchain": null, "imports": ["Mathlib"], "metadata": {}},
        "statement": "True",
        "stub": null,
        "prompt": null,
        "backend": "aristotle",
        "metadata": {}
    });
    let task: crate::prover::model::ProofTask = serde_json::from_value(legacy).unwrap();
    assert_eq!(task.system, FormalSystem::Lean);
    assert_eq!(task.formal_project.imports, vec!["Mathlib".to_string()]);

    // Old ProofResult used `lean_code`.
    let legacy_res = json!({
        "task_id": "t1", "job_id": "j1", "status": "proved",
        "lean_code": "theorem T : True := trivial",
        "counterexample": null, "verification": null, "artifacts_dir": null,
        "duration_ms": 1, "cost": null, "message": null, "provenance": {}
    });
    let res: crate::prover::model::ProofResult = serde_json::from_value(legacy_res).unwrap();
    assert!(res.formal_code.unwrap().contains("trivial"));
}

// --- Phase 2 live gate tests -------------------------------------------------
//
// These exercise the REAL toolchains through the configured runner. Each probes
// availability first and skips cleanly (returns) when the toolchain is absent,
// so the suite stays green on machines without Lean/Rocq/Isabelle. On this
// machine the toolchains are installed, so they run and must pass. Every system
// gets a POSITIVE case (trivial proof certifies) and a NEGATIVE case (a
// sorry/admit proof is rejected — the source scan bites even if it compiles).

/// A Config whose live formal workspaces land under a throwaway temp dir.
fn live_config(tmp: &Path) -> Config {
    let mut c = Config::default();
    c.workspace = tmp.join("workspaces");
    c
}

#[test]
fn lean_live_verifies_trivial_and_rejects_sorry() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = live_config(tmp.path());
    let backend = lean::LeanBackend::live(&cfg);
    if !backend.available() {
        eprintln!("SKIP lean_live: lean toolchain unavailable via configured runner");
        return;
    }
    let ok = backend
        .verify(&cfg, "theorem t : True := trivial\n", "theorem t : True")
        .unwrap();
    assert!(ok.lexically_verified, "trivial Lean proof must certify: {ok:?}");
    assert!(ok.axioms_clean, "trivial Lean proof is axiom-clean: {ok:?}");

    let bad = backend
        .verify(&cfg, "theorem t : True := by sorry\n", "theorem t : True")
        .unwrap();
    assert!(
        !bad.lexically_verified,
        "a `sorry` Lean proof must be rejected even if it compiles: {bad:?}"
    );
}

#[test]
fn rocq_live_verifies_trivial_and_rejects_admitted() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = live_config(tmp.path());
    let backend = rocq::RocqBackend::live(&cfg);
    if !backend.available() {
        eprintln!("SKIP rocq_live: coqc unavailable via configured runner");
        return;
    }
    let ok = backend
        .verify(
            &cfg,
            "Theorem t : True.\nProof.\n  exact I.\nQed.\n",
            "Theorem t : True",
        )
        .unwrap();
    assert!(ok.lexically_verified, "trivial Rocq proof must certify: {ok:?}");
    assert!(ok.axioms_clean, "trivial Rocq proof is axiom-clean: {ok:?}");

    let bad = backend
        .verify(
            &cfg,
            "Theorem t : True.\nProof.\nAdmitted.\n",
            "Theorem t : True",
        )
        .unwrap();
    assert!(
        !bad.lexically_verified,
        "an `Admitted` Rocq proof must be rejected: {bad:?}"
    );
}

#[test]
fn isabelle_live_verifies_trivial_and_rejects_sorry() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = live_config(tmp.path());
    let backend = isabelle::IsabelleBackend::live(&cfg);
    if !backend.available() {
        eprintln!("SKIP isabelle_live: isabelle unavailable via configured runner");
        return;
    }
    let ok = backend
        .verify(&cfg, "theorem t: \"True\"\n  by simp", "\"True\"")
        .unwrap();
    assert!(
        ok.lexically_verified,
        "trivial Isabelle proof must certify: {ok:?}"
    );

    let bad = backend
        .verify(&cfg, "theorem t: \"True\"\n  sorry", "\"True\"")
        .unwrap();
    assert!(
        !bad.lexically_verified,
        "a `sorry` Isabelle proof must be rejected: {bad:?}"
    );
}

#[test]
fn agda_live_verifies_trivial_and_rejects_postulate() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = live_config(tmp.path());
    let backend = external::ExternalBackend::new(&cfg, FormalSystem::Agda, false);
    if !backend.available() {
        eprintln!("SKIP agda_live: agda unavailable via configured runner");
        return;
    }
    let ok = backend
        .verify(
            &cfg,
            "module Generated where\nopen import Agda.Builtin.Unit\ntrivial : \u{22a4}\ntrivial = tt\n",
            "trivial : \u{22a4}",
        )
        .unwrap();
    assert!(ok.lexically_verified, "trivial Agda proof must certify: {ok:?}");

    let bad = backend
        .verify(
            &cfg,
            "module Generated where\nopen import Agda.Builtin.Unit\npostulate bad : \u{22a4}\n",
            "bad : \u{22a4}",
        )
        .unwrap();
    assert!(!bad.lexically_verified, "Agda postulates must be rejected: {bad:?}");
}

#[test]
fn metamath_live_verifies_trivial_and_rejects_malformed_proof() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = live_config(tmp.path());
    let backend = external::ExternalBackend::new(&cfg, FormalSystem::Metamath, false);
    if !backend.available() {
        eprintln!("SKIP metamath_live: metamath unavailable via configured runner");
        return;
    }
    // A genuinely VALID minimal Metamath proof. The RPN proof of `th : |- ph`
    // is `wph id`: `wph` (the floating hypothesis) pushes `wff ph`, then `id`
    // (which has mandatory hypothesis `wph`) consumes it and yields `|- ph`. The
    // previous fixture used `$= id $.`, which metamath actually REJECTS ("id
    // requires a hypothesis but the RPN stack is empty"); it only "certified"
    // because the old backend trusted metamath's exit code, which is 0 even on a
    // failed `verify proof *` -- the soundness bug now fixed.
    let ok = backend
        .verify(
            &cfg,
            "$c wff |- $.\n$v ph $.\nwph $f wff ph $.\nid $a |- ph $.\nth $p |- ph $= wph id $.\n",
            "th",
        )
        .unwrap();
    assert!(ok.lexically_verified, "trivial Metamath proof must certify: {ok:?}");

    let bad = backend
        .verify(&cfg, "$c wff $.\nthis is not Metamath\n", "bad")
        .unwrap();
    assert!(!bad.lexically_verified, "malformed Metamath must be rejected: {bad:?}");
}

#[test]
fn proof_job_cancel_is_terminal() {
    let tmp = tempfile::tempdir().unwrap();
    let config = test_config(tmp.path());
    let store = Store::open(&config.database).unwrap();
    let project = store.create_project("p", "True").unwrap();
    let task = aristotle::build_task(Some(project.id), None, "True", "T", &config);
    let job = proof_job::submit(&store, &config, task, None).unwrap();
    let cancelled = proof_job::cancel(&store, &job.id).unwrap();
    assert_eq!(cancelled.status, ProverJobStatus::Cancelled);
}
