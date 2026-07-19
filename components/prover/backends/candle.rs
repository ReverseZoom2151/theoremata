//! Candle (verified HOL Light on CakeML) external-prover adapter — Phase 1 mock
//! + Phase 2 live gate.
//!
//! Mirrors the `rocq`/`isabelle` backends (Config.prover_mock-driven canned
//! layers + a REAL source scan) and routes verification through the
//! system-agnostic [`FormalBackend`] 3+1-layer gate. Candle proofs are HOL Light
//! OCaml scripts (`.ml`) executed by the Candle binary through the configured
//! [`Runner`].
//!
//! Candle is SPECIAL among the backends: its kernel's soundness is machine-
//! PROVEN (formalized in HOL4, all the way down to the CakeML-compiled machine
//! code). So the layer-3 kernel re-check here is not "a smaller trusted checker
//! re-replaying the proof" (`leanchecker`/`coqchk`) — it is a re-run through a
//! kernel that has itself been *verified*, the strongest layer-3 of any backend.
//! (Caveat: HOL Light's model-generation automation is weaker than Lean/Isabelle,
//! so in practice a Candle proof leans on hammers / cross-system translation to
//! be produced; once produced, the proven kernel makes checking it maximally
//! trustworthy.)
//!
//! HOL Light is script/theory granular (no per-tactic REPL stepping wired here),
//! so the driver's `step_tactic` returns
//! [`crate::prover::formal::SessionError::Unsupported`], like Isabelle.

use crate::{
    config::Config,
    prover::{
        exec::{self, Runner},
        formal::{
            AxiomReport, CompileReport, FormalBackend, FormalSystem, GoalState, ProofSession,
            RecheckReport, ScanReport, SessionError, StateResult, UnitResult, Workspace,
        },
        model::FormalProject,
    },
};
use anyhow::Result;
use serde_json::json;
use std::path::PathBuf;

const SYSTEM: FormalSystem = FormalSystem::Candle;
/// The fixed, OCaml-safe module basename for a generated HOL Light script.
const MODULE: &str = "Generated";

/// Candle [`FormalBackend`]. In mock mode the compile / axiom-audit / kernel
/// re-check layers return canned success; the source scan always runs for real.
/// In live mode every layer runs the real Candle binary through the configured
/// [`Runner`]: `candle Generated.ml` loads and CHECKS the HOL Light proof (the
/// verified kernel accepts every primitive inference). Because that kernel is
/// machine-proven sound, the compile IS the kernel check and a fresh re-run is
/// the independent (proven-kernel) re-check.
pub struct CandleBackend {
    pub mock: bool,
    pub runner: Runner,
    pub candle: String,
}

impl CandleBackend {
    /// The offline mock backend (canned layers; real source scan).
    pub fn mock() -> Self {
        Self {
            mock: true,
            runner: Runner::Native,
            candle: "candle".into(),
        }
    }

    /// The live backend, reading the configured runner + binary (env-overridable
    /// via `THEOREMATA_CANDLE`).
    pub fn live(cfg: &Config) -> Self {
        Self {
            mock: false,
            runner: cfg.formal_runners.for_system(SYSTEM),
            candle: exec::env_or("THEOREMATA_CANDLE", &cfg.candle_bin),
        }
    }

    /// Run the generated HOL Light script through the Candle kernel (shared by
    /// `compile` and `kernel_recheck` — a Candle run both elaborates and, via the
    /// PROVEN kernel, checks every inference).
    fn check(&self, ws: &Workspace) -> exec::ExecOutcome {
        exec::run(
            &self.runner,
            &[&self.candle, &format!("{MODULE}.ml")],
            &ws.root,
        )
    }
}

impl FormalBackend for CandleBackend {
    fn system(&self) -> FormalSystem {
        SYSTEM
    }

    fn compile_success_signal(&self) -> crate::prover::formal::SuccessSignal {
        // The Candle checker sets a correct non-zero exit code on failure.
        crate::prover::formal::SuccessSignal::NonZeroExitIsHonest
    }

    fn is_mock(&self) -> bool {
        self.mock
    }

    fn available(&self) -> bool {
        self.mock || exec::probe(&self.runner, &[&self.candle, "--version"])
    }

    fn scaffold(&self, cfg: &Config, code: &str, name: &str) -> Result<Workspace> {
        if self.mock {
            return Ok(Workspace {
                system: SYSTEM,
                root: PathBuf::from("."),
                source_path: PathBuf::from(format!("{name}{}", SYSTEM.source_extension())),
                entry: name.to_string(),
            });
        }
        let entry =
            crate::prover::formal::entry_name(SYSTEM, code).unwrap_or_else(|| name.to_string());
        let root = crate::prover::formal::live_workspace_dir(cfg, SYSTEM)?;
        let src = root.join(format!("{MODULE}.ml"));
        std::fs::write(&src, code)?;
        Ok(Workspace {
            system: SYSTEM,
            root,
            source_path: src,
            entry,
        })
    }

    fn compile(&self, ws: &Workspace) -> Result<CompileReport> {
        if self.mock {
            return Ok(CompileReport {
                compiled: true,
                errors: Vec::new(),
                per_unit: Vec::new(),
                detail: json!({"mock": true}),
            });
        }
        if !self.available() {
            return Ok(CompileReport {
                compiled: false,
                errors: vec!["candle toolchain unavailable".into()],
                per_unit: Vec::new(),
                detail: json!({"unavailable": true, "runner": self.runner.tag()}),
            });
        }
        // The verified Candle kernel accepts/checks the HOL Light proof script.
        let out = self.check(ws);
        let errors = if out.success() {
            Vec::new()
        } else {
            vec![out.stderr.clone(), out.stdout.clone()]
        };
        let code = std::fs::read_to_string(&ws.source_path).unwrap_or_default();
        let per_unit =
            crate::prover::formal::per_declaration_status(SYSTEM, &code, out.success(), &errors);
        Ok(CompileReport {
            compiled: self.compile_success_signal().is_pass(
                out.launched,
                out.success(),
                &out.stdout,
                &out.stderr,
            ),
            errors,
            per_unit,
            detail: json!({
                "runner": self.runner.tag(),
                "code": out.code,
                "stdout": out.stdout,
                "stderr": out.stderr,
                "note": "kernel-checked by the machine-proven Candle/HOL-Light kernel",
            }),
        })
    }

    fn audit_axioms(
        &self,
        _ws: &Workspace,
        _thm: &str,
        whitelist: &[String],
    ) -> Result<AxiomReport> {
        if self.mock {
            return Ok(AxiomReport {
                axioms: Vec::new(),
                within_whitelist: true,
                detail: json!({"mock": true, "whitelist": whitelist}),
            });
        }
        // HOL Light's trusted base is a tiny, FIXED set of three axioms
        // (`ETA_AX`, `SELECT_AX`, `INFINITY_AX`) plus the conservative
        // definitional principles. There is no cheap per-theorem axiom-dependency
        // command over a batch Candle run, so — as with Isabelle's oracle gate —
        // this layer defers to the clean kernel run (compile) combined with the
        // MANDATORY source scan, which is what actually catches an undue axiom:
        // any `new_axiom` (widen the base) or `mk_thm` (fabricate a theorem
        // bypassing the kernel) is flagged by [`source_scan`]. The definitional
        // principles are permitted; anything outside the fixed base is not.
        Ok(AxiomReport {
            axioms: Vec::new(),
            within_whitelist: true,
            detail: json!({
                "runner": self.runner.tag(),
                "note": "HOL Light fixed axiom base + definitional principles allowed; \
                         undue axioms (new_axiom) and mk_thm are rejected by the source scan",
                "whitelist": whitelist,
            }),
        })
    }

    fn kernel_recheck(&self, ws: &Workspace) -> Result<RecheckReport> {
        if self.mock {
            return Ok(RecheckReport {
                rechecked: true,
                detail: json!({
                    "mock": true,
                    "proven_kernel": true,
                }),
            });
        }
        if !self.available() {
            return Ok(RecheckReport {
                rechecked: false,
                detail: json!({"unavailable": true, "runner": self.runner.tag()}),
            });
        }
        // A fresh Candle run re-replays every primitive inference through the
        // kernel. Unlike leanchecker/coqchk (a *smaller* independent checker),
        // Candle's kernel is itself machine-PROVEN sound (HOL4 + CakeML), so this
        // is the strongest layer-3 re-check of any backend.
        let out = self.check(ws);
        Ok(RecheckReport {
            rechecked: out.success(),
            detail: json!({
                "runner": self.runner.tag(),
                "code": out.code,
                "proven_kernel": true,
                "note": "re-checked by the machine-proven Candle/HOL-Light kernel \
                         (strongest layer-3: the checker itself is verified, not merely smaller)",
            }),
        })
    }

    fn source_scan(&self, code: &str) -> Result<ScanReport> {
        // Prefer the shared Python `source_scan` worker; it does not (yet) know
        // the `candle` system and returns `None`, so the built-in lexical pass
        // below is authoritative for HOL Light and the gate still bites offline.
        if let Some(report) = crate::prover::formal::worker_source_scan(SYSTEM, code) {
            return Ok(report);
        }
        // Authoritative HOL Light auditor: flags the escape hatches the proven
        // kernel cannot see because they sidestep it (`mk_thm` fabricates a `thm`,
        // `new_axiom` widens the fixed base) plus unsound definitional extensions
        // and INST/INST_TYPE capture. See `crate::prover::axiom_audit`.
        Ok(
            crate::prover::axiom_audit::audit_hol_light(code, &SYSTEM.axiom_whitelist())
                .into_scan_report(),
        )
    }
}

/// Candle warm-driver session. HOL Light is script granular here (a whole `.ml`
/// is run through the kernel), so `submit_unit` is supported and `step_tactic`
/// returns [`SessionError::Unsupported`], mirroring Isabelle.
impl ProofSession for CandleBackend {
    fn start(&mut self, _project: &FormalProject) -> Result<()> {
        Ok(())
    }

    fn submit_unit(&mut self, code: &str) -> Result<UnitResult> {
        let scan = self.source_scan(code)?;
        Ok(UnitResult {
            ok: scan.clean,
            messages: scan.findings,
            detail: json!({"mock": self.mock, "system": SYSTEM.as_str()}),
        })
    }

    fn step_tactic(&mut self, _state: u64, _tactic: &str) -> Result<StateResult> {
        // HOL Light proofs are run as whole OCaml scripts here; no per-tactic
        // stepping is wired.
        Err(SessionError::Unsupported(
            "Candle/HOL Light is script granular; use submit_unit instead of step_tactic",
        )
        .into())
    }

    fn goal_state(&self, _state: u64) -> Result<GoalState> {
        Ok(GoalState {
            goals: vec!["T".into()],
            detail: json!({"mock": self.mock}),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prover::formal::FormalBackend;

    /// Under the mock backend, a trivial HOL Light theorem certifies through the
    /// whole 3+1-layer gate (canned kernel layers + the REAL source scan), and
    /// the reported system is Candle.
    #[test]
    fn candle_mock_certifies_trivial_theorem() {
        let backend = CandleBackend::mock();
        assert_eq!(backend.system(), FormalSystem::Candle);
        let cfg = Config::default();
        // `TRUTH : thm = |- T` — a trivial, kernel-checked HOL Light theorem.
        let ok = backend
            .verify(&cfg, "let TRUTH_THM = TRUTH;;\n", "TRUTH_THM")
            .unwrap();
        assert!(
            ok.lexically_verified,
            "trivial HOL Light proof must certify: {ok:?}"
        );
        assert!(
            ok.axioms_clean,
            "fixed-axiom-base proof is axiom-clean: {ok:?}"
        );
        assert!(ok.lexical_clean, "no escape hatch present: {ok:?}");
        // The 3+1 mapping is recorded in the detail (system + gate + layers).
        // `verify()` nests each layer's whole report, so the recheck's own
        // `detail` object sits under ["kernel_recheck"]["detail"].
        assert_eq!(ok.detail["system"], "candle");
        assert_eq!(ok.detail["gate"], "3+1-layer");
        assert_eq!(ok.detail["kernel_recheck"]["detail"]["proven_kernel"], true);
    }

    /// The layer-4 source scan / axiom audit fires on the HOL Light escape
    /// hatches: `mk_thm` (fabricated theorem) and `new_axiom` (undue axiom).
    #[test]
    fn candle_mock_rejects_mk_thm_and_new_axiom() {
        let backend = CandleBackend::mock();
        let cfg = Config::default();

        let mk_thm = backend
            .verify(&cfg, "let FAKE = mk_thm([], `p /\\ ~p`);;\n", "FAKE")
            .unwrap();
        assert!(
            !mk_thm.lexically_verified && !mk_thm.lexical_clean,
            "a `mk_thm` fabrication must be rejected by the source scan: {mk_thm:?}"
        );

        let new_axiom = backend
            .verify(&cfg, "let AX = new_axiom `!x. P x`;;\n", "AX")
            .unwrap();
        assert!(
            !new_axiom.lexically_verified && !new_axiom.lexical_clean,
            "a `new_axiom` must be rejected by the source scan: {new_axiom:?}"
        );

        // Direct source-scan spot checks (clean vs. flagged).
        assert!(backend.source_scan("let X = TRUTH;;").unwrap().clean);
        assert!(
            !backend
                .source_scan("let X = mk_thm([], `T`);;")
                .unwrap()
                .clean
        );
        assert!(!backend.source_scan("new_axiom `P`").unwrap().clean);
    }

    /// The 3+1 layers are individually exercisable on the mock backend.
    #[test]
    fn candle_mock_layers_map_3_plus_1() {
        let backend = CandleBackend::mock();
        let cfg = Config::default();
        let ws = backend.scaffold(&cfg, "let X = TRUTH;;", "X").unwrap();
        assert!(backend.compile(&ws).unwrap().compiled); // layer 2b (build)
        let whitelist = FormalSystem::Candle.axiom_whitelist();
        assert!(
            backend
                .audit_axioms(&ws, &ws.entry, &whitelist)
                .unwrap()
                .within_whitelist
        ); // 2a
        assert!(backend.kernel_recheck(&ws).unwrap().rechecked); // layer 3 (proven kernel)
        assert!(backend.source_scan("let X = TRUTH;;").unwrap().clean); // layer 2c
    }

    /// A LIVE Candle gate. Marked `#[ignore]` so a normal `cargo test` run never
    /// pays the (slow, WSL cold-start) toolchain probe — it only runs on demand
    /// via `cargo test -- --ignored` on a machine with the HOL4/PolyML/CakeML
    /// toolchain. It ALSO self-skips if the `candle` binary is still absent when
    /// invoked that way, so `--ignored` stays green without the toolchain.
    #[test]
    #[ignore = "requires the candle/HOL Light (HOL4/CakeML) toolchain; run with: cargo test -- --ignored"]
    fn candle_live_verifies_trivial_and_rejects_mk_thm() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.workspace = tmp.path().join("workspaces");
        let backend = CandleBackend::live(&cfg);
        if !backend.available() {
            eprintln!("SKIP candle_live: candle binary unavailable via configured runner");
            return;
        }
        let ok = backend
            .verify(&cfg, "let T_THM = TRUTH;;\n", "T_THM")
            .unwrap();
        assert!(
            ok.lexically_verified,
            "trivial HOL Light proof must certify live: {ok:?}"
        );
        let bad = backend
            .verify(&cfg, "let FAKE = mk_thm([], `p`);;\n", "FAKE")
            .unwrap();
        assert!(
            !bad.lexically_verified,
            "a `mk_thm` fabrication must be rejected live: {bad:?}"
        );
    }
}
