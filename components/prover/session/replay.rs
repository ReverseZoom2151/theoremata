//! Checker-backed tactic REPLAY: the missing soundness seam for proof
//! minimization.
//!
//! `BestFirstOutcome::minimized_proof(replay)` and
//! `hybrid_search::multi_alpha_union_minimized(.., replay)` both take a
//! `FnMut(&[String]) -> bool` that must answer one question and only one:
//!
//! > does executing exactly this tactic sequence, from the root goal, against a
//! > REAL checker, leave no open goal?
//!
//! Nothing in the search layer can answer it (the scorer is a proposer, and a
//! state's `is_closed` flag is the scorer's own claim, not a checker verdict), so
//! the closure is a parameter and until now nobody could supply one. This module
//! supplies it once, so every caller can have it.
//!
//! # Why the whole sequence goes through `FormalBackend::verify`, not `step_tactic`
//!
//! The obvious implementation is to walk [`ProofSession::step_tactic`] over the
//! sequence and read the final `StateResult::finished`. That would be UNSOUND
//! against this codebase as it stands. Every `impl ProofSession` present here
//! returns either [`SessionError::Unsupported`] (Isabelle, Candle, Agda/Metamath)
//! or a *placeholder* `finished` computed lexically from the tactic text:
//! `LeanBackend::step_tactic` reports finished when the tactic
//! `contains("trivial")`, and `RocqBackend::step_tactic` when it
//! `contains("Qed")`. Those are stand-ins for the Phase-3 warm drivers, not
//! checker verdicts. Trusting them would let the string `trivial` anywhere in a
//! shrunk sequence certify that sequence as closing, which is exactly the
//! "something that guessed the shrink confirms the shrink" hole the minimizer's
//! doc-comment forbids.
//!
//! So [`GateReplay`] instead ASSEMBLES the tactic sequence into a complete source
//! file under the original statement and runs the full 3+1-layer gate
//! ([`FormalBackend::verify`]: compile, axioms subset of whitelist, kernel
//! re-check, source scan, statement preservation). That gate really compiles, and
//! a Lean declaration whose tactic block leaves a goal open does not compile.
//! It is coarser and slower than per-tactic stepping, but it is the only path here
//! whose `true` means what the minimizer needs it to mean. When a real
//! `step_tactic` lands, a second [`TacticReplay`] impl can be added beside this
//! one without touching any caller.
//!
//! # Fail-closed, everywhere
//!
//! `false` costs nothing: it merely declines the shrink and keeps the original
//! proof, which already passed a gate upstream. `true` on an unverified sequence
//! emits a shorter WRONG proof as though it were proved. So every uncertainty maps
//! to `false`: no tactics, a blank tactic, a mock backend, an unavailable
//! toolchain, a system whose source we cannot assemble, an unparsable statement, a
//! verify error, a non-live report, or a report that did not pass the gate.

use crate::{
    config::Config,
    prover::{
        formal::{backend_for, FormalBackend, FormalSystem},
        model::VerificationReport,
    },
};

/// Replay a tactic sequence against a real checker.
///
/// Implementors MUST return `true` only when a real checker has confirmed that
/// executing `tactics` in order, from the root goal, leaves no open goal. Every
/// other outcome, including every error and every "cannot tell", is `false`.
pub trait TacticReplay {
    /// True ONLY if executing `tactics` in order from the root leaves no open
    /// goal, as confirmed by a real checker.
    fn replays_closed(&mut self, tactics: &[String]) -> bool;
}

/// Adapt any [`TacticReplay`] into the `FnMut(&[String]) -> bool` closure that
/// `minimized_proof` / `multi_alpha_union_minimized` take.
///
/// Free function rather than a trait method because a method returning
/// `impl FnMut` would make [`TacticReplay`] non-object-safe, and callers that hold
/// a `&mut dyn TacticReplay` (picking the checker at runtime) need object safety.
pub fn as_closure<R: TacticReplay + ?Sized>(
    replay: &mut R,
) -> impl FnMut(&[String]) -> bool + '_ {
    move |tactics: &[String]| replay.replays_closed(tactics)
}

/// A replay that confirms NOTHING. The correct thing to pass when no checker is
/// wired up: minimization then always falls back to the original proof.
///
/// Prefer this over a hand-rolled `|_| false` at call sites, so the intent
/// ("declined, on purpose") is named rather than inferred, and over
/// `|_| true`, which must never be written anywhere.
#[derive(Debug, Clone, Copy, Default)]
pub struct DenyAllReplay;

impl TacticReplay for DenyAllReplay {
    fn replays_closed(&mut self, _tactics: &[String]) -> bool {
        false
    }
}

/// Checker-backed replay: assemble the sequence into a source file under the
/// original statement and run the full [`FormalBackend::verify`] gate over it.
pub struct GateReplay<'a> {
    cfg: &'a Config,
    /// Boxed so the backend can be chosen at runtime (and stubbed in tests). The
    /// gate is invoked through the trait's default `verify` orchestration, so this
    /// replay inherits every layer the rest of the system relies on.
    backend: Box<dyn FormalBackend + 'a>,
    /// The ROOT GOAL, as a declaration header (`theorem foo (n : Nat) : n = n`),
    /// with no `:=` body. This is what the sequence must close, and what the
    /// gate's statement-preservation layer will hold the assembled source to.
    statement: String,
    /// Preamble (imports) prepended to the assembled source. Defaults to the
    /// system's `default_imports`; overridable because a project may need a
    /// narrower or wider corpus than the default and a preamble that fails to
    /// elaborate would reject every shrink.
    preamble: String,
}

impl<'a> GateReplay<'a> {
    /// Build a replay over an explicit backend.
    pub fn new(cfg: &'a Config, backend: Box<dyn FormalBackend + 'a>, statement: &str) -> Self {
        let preamble = default_preamble(backend.system());
        Self {
            cfg,
            backend,
            statement: statement.trim().to_string(),
            preamble,
        }
    }

    /// Build a replay over the configured backend for `system`.
    ///
    /// Note that this honors `Config::prover_mock`: with mocking on, this
    /// constructs a MOCK backend, and [`replays_closed`](TacticReplay::replays_closed)
    /// will then refuse every sequence. That is deliberate. A mock's canned
    /// compile/axiom/kernel layers are not evidence, so under mocking the only
    /// sound answer is "no shrink accepted".
    pub fn for_system(cfg: &'a Config, system: FormalSystem, statement: &str) -> Self {
        Self::new(cfg, backend_for(cfg, system, cfg.prover_mock), statement)
    }

    /// Replace the assembled source's preamble (one import/require per line).
    pub fn with_preamble(mut self, preamble: impl Into<String>) -> Self {
        self.preamble = preamble.into();
        self
    }

    /// The source this replay would submit for `tactics`, or `None` when it
    /// cannot be assembled soundly. Exposed (and unit-tested) because the exact
    /// text submitted to the checker is what the `true` verdict is about.
    pub fn assemble(&self, tactics: &[String]) -> Option<String> {
        assemble_source(
            self.backend.system(),
            &self.preamble,
            &self.statement,
            tactics,
        )
    }

    /// The gate run itself, split out so `replays_closed` reads as a list of
    /// refusals followed by one positive path.
    fn run_gate(&self, code: &str) -> Option<VerificationReport> {
        // A verify error (spawn failure, timeout surfaced as an error, unreadable
        // workspace, panic-free I/O failure) is an ambiguous result, and ambiguous
        // means no.
        self.backend.verify(self.cfg, code, &self.statement).ok()
    }
}

impl TacticReplay for GateReplay<'_> {
    fn replays_closed(&mut self, tactics: &[String]) -> bool {
        // Empty sequence: REFUSED, deliberately. An empty sequence closes a goal
        // only if the goal was already closed, which is not a proof of the root
        // statement and not something we can establish from here. Worse, in Lean an
        // empty `:= by` block is a syntax error, so we would be asking the checker
        // a malformed question and reading its answer as a verdict. Callers see the
        // original proof kept, which is always safe.
        if tactics.is_empty() {
            return false;
        }
        // A blank tactic means the caller handed us a sequence we cannot faithfully
        // render (it would silently vanish into whitespace, so the checker would be
        // asked about a DIFFERENT, shorter sequence than the one being accepted).
        if tactics.iter().any(|t| t.trim().is_empty()) {
            return false;
        }
        // A mock backend must never confirm a replay. Its compile/axiom/kernel
        // layers are canned constants, so a `true` here would be the "mock
        // certifies" hole in its purest form. This is checked BEFORE any work, and
        // it is also enforced a second time below via `report.live`, which
        // `verify` stamps as `!is_mock()`. Two independent checks because this is
        // the one mistake with no recovery.
        if self.backend.is_mock() {
            return false;
        }
        // No toolchain means no evidence. `verify` would already fail closed (an
        // unavailable backend reports `compiled: false`), but refusing up front
        // keeps the intent explicit and skips the scaffolding I/O.
        if !self.backend.available() {
            return false;
        }
        let Some(code) = self.assemble(tactics) else {
            // No sound rendering for this system or this statement: refuse rather
            // than submit a guess. See `assemble_source`.
            return false;
        };
        let Some(report) = self.run_gate(&code) else {
            return false;
        };
        // `live` rejects any report a mock produced; `lexically_verified` is the
        // gate's conjunction of all layers (compile AND axioms AND kernel re-check
        // AND source scan AND statement preservation). A shrunk sequence that
        // leaves a goal open does not compile, so it cannot reach `true` here.
        report.live && report.lexically_verified
    }
}

/// The default preamble for a system: one import/require directive per line.
/// Kept minimal on purpose. Every extra dependency is another way for a correct
/// shrink to be rejected because the PREAMBLE failed rather than the proof.
///
/// Only Lean has one, because Lean is the only system [`assemble_source`] renders.
/// Note that this is NOT `FormalSystem::default_imports()`: that list is the
/// RETRIEVAL corpus (the premises a model may draw on), and the only thing in the
/// tree that turns it into source lines is the Python search helper
/// (`components/retrieval/python/theoremata_tools/rocq_retrieval.py::_require_lines`),
/// which builds a throwaway `Search` file, not a proof. Reusing a retrieval corpus
/// as a proof preamble would silently add dependencies the checker must resolve.
fn default_preamble(system: FormalSystem) -> String {
    match system {
        FormalSystem::Lean => "import Mathlib\n".to_string(),
        // Every other system is refused by `assemble_source` anyway, so there is
        // nothing honest to put here.
        _ => String::new(),
    }
}

/// Assemble `tactics` into a complete source file proving `statement`, or `None`
/// when that cannot be done soundly for this system.
///
/// `None` (which the caller turns into `false`) is returned when:
///
/// * the system is not one whose tactic-block syntax is rendered here. Isabelle,
///   Candle, Agda and Metamath are all whole-unit checkers whose `step_tactic`
///   returns `Unsupported`, and their proof languages are not a list of tactic
///   lines under a `by` block. Guessing a rendering for them would mean
///   asking the checker about something that is not the sequence under test;
/// * the system is Rocq. See the `FormalSystem::Rocq` arm below: the gate cannot
///   presently certify ANY Rocq source, so a rendering would be untestable
///   decoration;
/// * the statement does not parse as a declaration header for the system;
/// * the statement already carries a `:=` body, so appending a proof would either
///   produce a second body or silently reuse the old one.
fn assemble_source(
    system: FormalSystem,
    preamble: &str,
    statement: &str,
    tactics: &[String],
) -> Option<String> {
    let statement = statement.trim();
    if statement.is_empty() || tactics.is_empty() {
        return None;
    }
    // `entry_name` returning Some means the statement really opens a declaration
    // this system recognizes, which is also what the gate's preservation layer
    // will look for.
    crate::prover::formal::entry_name(system, statement)?;
    match system {
        FormalSystem::Lean => {
            // A statement that already has a body is not a header we may extend.
            if statement.contains(":=") {
                return None;
            }
            let mut src = String::new();
            src.push_str(preamble);
            if !preamble.is_empty() && !preamble.ends_with('\n') {
                src.push('\n');
            }
            src.push('\n');
            src.push_str(statement);
            src.push_str(" := by\n");
            for tactic in tactics {
                // Indent every line of a possibly multi-line tactic into the
                // tactic block, so a `induction ... with | ...` step elaborates as
                // one step rather than falling out of the block.
                for line in tactic.lines() {
                    src.push_str("  ");
                    src.push_str(line.trim_end());
                    src.push('\n');
                }
            }
            Some(src)
        }
        // Rocq: DECLINED, on evidence rather than on caution.
        //
        // Rendering a `Theorem <name> : <stmt>. Proof. <tactics> Qed.` file here
        // would be dead decoration, because the gate this replay runs cannot
        // return `lexically_verified` for a live Rocq backend at all:
        //
        // * `verify_with_gates` conjoins `statement_preserved`, which for a LIVE
        //   backend requires `check_entry_signature(system, stmt, code).preserved`
        //   (`formal.rs`, the `mentioned && (preservation.preserved || is_mock())`
        //   line);
        // * `check_entry_signature` routes Rocq into `check_statement_preserved`,
        //   whose `parse_all_decls` recognizes only the LOWERCASE Lean keywords
        //   `theorem` / `lemma` / `example` / `def`. Rocq vernacular is
        //   capitalized (`Theorem`, `Lemma`, ...), so the canonical statement never
        //   parses and the verdict is always `CanonicalUnparsable`, whose
        //   `preserved` is `false`.
        //
        // So every Rocq source, correct or not, fails the gate. A rendering here
        // would therefore be untestable against a real verdict, and the only thing
        // it could do is invite a future reader to "fix the preamble" and conclude
        // that Rocq shrinks are being checked when they are being rejected for an
        // unrelated reason.
        //
        // The preamble question is unresolvable here for the same reason there is
        // nothing to match: the Rocq backend does NOT own a preamble convention.
        // `RocqBackend::scaffold` writes the submitted `code` verbatim and appends
        // only `Print Assumptions <entry>.`; the sole Rocq source the backend
        // itself emits (`mock_rocq_solution`) carries no `Require` line at all and
        // leans on the auto-loaded Prelude. And `FormalSystem::Rocq::default_imports()`
        // (`["Stdlib", "mathcomp.ssreflect.ssreflect"]`) is a retrieval corpus, not
        // a source preamble: `Stdlib` is the stdlib's dotted NAMESPACE root
        // (`Stdlib.Arith.Arith`, per `docs/formal-systems/rocq.md`), not a module
        // that `Require Import Stdlib.` resolves to, and mathcomp is an opam
        // package that need not be installed. Both a too-thin and a too-fat
        // preamble reject correct shrinks; neither is a convention to copy.
        //
        // TO ADD ROCQ BACK, all three must hold:
        // 1. `statement_preservation::parse_all_decls` (or a Rocq-specific parser
        //    behind `check_entry_signature`) understands capitalized Rocq
        //    vernacular and period-terminated declarations, so a correct Rocq proof
        //    can reach `preserved == true`;
        // 2. the Rocq backend (or `FormalProject`) exposes the preamble it
        //    actually compiles against, so this module copies a convention instead
        //    of inventing one;
        // 3. a test asserts an assembled source is byte-comparable with what the
        //    backend compiles, the way `lean_assembly_...` does for Lean.
        //
        // Until then, declining is free: a refused shrink keeps the original proof,
        // which already passed a gate upstream.
        FormalSystem::Rocq
        // See the doc comment: no guessed rendering for the whole-unit systems.
        | FormalSystem::Isabelle
        | FormalSystem::Candle
        | FormalSystem::Agda
        | FormalSystem::Metamath => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prover::formal::{
        AxiomReport, CompileReport, RecheckReport, ScanReport, SuccessSignal, Workspace,
    };
    use anyhow::Result;
    use serde_json::json;
    use std::path::PathBuf;

    /// A backend that MODELS a real checker: it "compiles" the assembled source
    /// only when the source contains `closes_on`, which stands in for "the tactic
    /// block actually discharged the goal". Nothing here touches the disk or a
    /// toolchain, so the test is hermetic, but every layer the gate conjoins runs
    /// for real over the assembled text.
    struct StubChecker {
        /// Reported by `is_mock`, and thus what `verify` stamps into `live`.
        mock: bool,
        available: bool,
        /// Make `scaffold` fail, standing in for a spawn failure / timeout /
        /// session death: `verify` then returns `Err`.
        erroring: bool,
        /// The marker whose presence in the source means "closed".
        closes_on: String,
    }

    impl Default for StubChecker {
        fn default() -> Self {
            Self {
                mock: false,
                available: true,
                erroring: false,
                closes_on: "rfl".to_string(),
            }
        }
    }

    impl FormalBackend for StubChecker {
        fn system(&self) -> FormalSystem {
            FormalSystem::Lean
        }
        fn compile_success_signal(&self) -> SuccessSignal {
            SuccessSignal::NonZeroExitIsHonest
        }
        fn is_mock(&self) -> bool {
            self.mock
        }
        fn available(&self) -> bool {
            self.available
        }
        fn scaffold(&self, _cfg: &Config, code: &str, name: &str) -> Result<Workspace> {
            if self.erroring {
                return Err(anyhow::anyhow!("stub checker session died"));
            }
            // No filesystem: the entry is read straight out of the assembled
            // source, so the stub stays hermetic.
            Ok(Workspace {
                system: FormalSystem::Lean,
                root: PathBuf::from("."),
                source_path: PathBuf::from("./Generated.lean"),
                entry: crate::prover::formal::entry_name(FormalSystem::Lean, code)
                    .unwrap_or_else(|| name.to_string()),
            })
        }
        fn compile(&self, _ws: &Workspace) -> Result<CompileReport> {
            // Whether the goal was closed is decided in `source_scan`, the one
            // layer that receives the raw source; `compile` cannot see it, so it
            // defers rather than guessing.
            Ok(CompileReport {
                compiled: true,
                errors: Vec::new(),
                per_unit: Vec::new(),
                detail: json!({"stub": true}),
            })
        }
        fn audit_axioms(
            &self,
            _ws: &Workspace,
            _thm: &str,
            _whitelist: &[String],
        ) -> Result<AxiomReport> {
            Ok(AxiomReport {
                axioms: Vec::new(),
                within_whitelist: true,
                detail: json!({}),
            })
        }
        fn kernel_recheck(&self, _ws: &Workspace) -> Result<RecheckReport> {
            Ok(RecheckReport {
                rechecked: true,
                detail: json!({}),
            })
        }
        /// The stand-in for "did the tactic block close the goal". `source_scan`
        /// is the one layer that receives the raw source, so the model lives here:
        /// a source without the closing marker is reported unclean, which fails
        /// `lexically_verified` exactly as a real non-closing proof would.
        fn source_scan(&self, code: &str) -> Result<ScanReport> {
            let closed = code.contains(&self.closes_on);
            Ok(ScanReport {
                clean: closed,
                findings: if closed {
                    Vec::new()
                } else {
                    vec!["unsolved goals".to_string()]
                },
                detail: json!({"stub": true}),
            })
        }
    }

    const STMT: &str = "theorem t (n : Nat) : n = n";

    fn replay_with<'a>(cfg: &'a Config, stub: StubChecker) -> GateReplay<'a> {
        GateReplay::new(cfg, Box::new(stub), STMT)
    }

    fn seq(tactics: &[&str]) -> Vec<String> {
        tactics.iter().map(|t| t.to_string()).collect()
    }

    #[test]
    fn a_genuinely_closing_sequence_is_confirmed() {
        let cfg = Config::default();
        let mut r = replay_with(&cfg, StubChecker::default());
        assert!(
            r.replays_closed(&seq(&["rfl"])),
            "a sequence the checker says closes the goal must be accepted"
        );
    }

    #[test]
    fn a_non_closing_sequence_is_refused() {
        let cfg = Config::default();
        let mut r = replay_with(&cfg, StubChecker::default());
        assert!(
            !r.replays_closed(&seq(&["simp", "omega"])),
            "the checker left a goal open, so the shrink must be declined"
        );
    }

    #[test]
    fn a_mock_backend_never_confirms() {
        // Same sequence the live stub accepts, on a backend that only differs by
        // being a mock. This is the "mock certifies" hole, and it must stay shut.
        let cfg = Config::default();
        let mut r = replay_with(
            &cfg,
            StubChecker {
                mock: true,
                ..Default::default()
            },
        );
        assert!(!r.replays_closed(&seq(&["rfl"])));
    }

    #[test]
    fn an_unavailable_toolchain_never_confirms() {
        let cfg = Config::default();
        let mut r = replay_with(
            &cfg,
            StubChecker {
                available: false,
                ..Default::default()
            },
        );
        assert!(!r.replays_closed(&seq(&["rfl"])));
    }

    #[test]
    fn a_backend_error_returns_false() {
        // Session death / spawn failure / timeout all surface as an `Err` from
        // `verify`, which is ambiguous, which is `false`.
        let cfg = Config::default();
        let mut r = replay_with(
            &cfg,
            StubChecker {
                erroring: true,
                ..Default::default()
            },
        );
        assert!(!r.replays_closed(&seq(&["rfl"])));
    }

    #[test]
    fn an_empty_sequence_returns_false() {
        let cfg = Config::default();
        let mut r = replay_with(&cfg, StubChecker::default());
        assert!(
            !r.replays_closed(&[]),
            "an empty sequence closes nothing we can establish from here"
        );
        // ...and the assembler refuses it too, so no malformed source is ever built.
        assert!(r.assemble(&[]).is_none());
    }

    #[test]
    fn a_blank_tactic_returns_false() {
        let cfg = Config::default();
        let mut r = replay_with(&cfg, StubChecker::default());
        // The marker is present, so only the blank entry can decide this.
        assert!(!r.replays_closed(&seq(&["rfl", "   "])));
    }

    #[test]
    fn a_system_without_a_tactic_rendering_returns_false() {
        // Isabelle/Candle/Agda/Metamath are whole-unit checkers whose `step_tactic`
        // is `Unsupported`; we refuse rather than invent a tactic block for them.
        for system in [
            FormalSystem::Isabelle,
            FormalSystem::Candle,
            FormalSystem::Agda,
            FormalSystem::Metamath,
        ] {
            assert!(
                assemble_source(system, "", "theorem t : True", &seq(&["simp"])).is_none(),
                "{system} must not get a guessed tactic-block rendering"
            );
        }
    }

    #[test]
    fn a_statement_that_is_not_a_declaration_header_returns_false() {
        // No `theorem`/`lemma` keyword: nothing to attach a proof to.
        assert!(assemble_source(FormalSystem::Lean, "", "n = n", &seq(&["rfl"])).is_none());
        // Already has a body: appending one would produce a different declaration
        // than the sequence under test.
        assert!(assemble_source(
            FormalSystem::Lean,
            "",
            "theorem t : True := trivial",
            &seq(&["rfl"])
        )
        .is_none());
        assert!(assemble_source(FormalSystem::Lean, "", "   ", &seq(&["rfl"])).is_none());
    }

    #[test]
    fn lean_assembly_puts_every_tactic_in_the_block_under_the_statement() {
        let src = assemble_source(
            FormalSystem::Lean,
            "import Mathlib\n",
            STMT,
            &seq(&["intro h", "induction n with\n| zero => rfl"]),
        )
        .expect("a Lean header assembles");
        assert!(src.starts_with("import Mathlib\n"));
        assert!(src.contains("theorem t (n : Nat) : n = n := by\n"));
        assert!(src.contains("  intro h\n"));
        // Multi-line tactics are indented line by line into the block.
        assert!(src.contains("  induction n with\n  | zero => rfl\n"));
    }

    #[test]
    fn rocq_is_declined_rather_than_rendered() {
        // A well-formed Rocq header, which `entry_name` DOES recognize, still gets
        // no rendering: see the `FormalSystem::Rocq` arm of `assemble_source`.
        assert!(
            crate::prover::formal::entry_name(FormalSystem::Rocq, "Theorem t : True").is_some(),
            "the refusal must come from the Rocq arm, not from an unrecognized header"
        );
        assert!(
            assemble_source(
                FormalSystem::Rocq,
                "Require Import Stdlib.\n",
                "Theorem t : True",
                &seq(&["auto", "trivial"])
            )
            .is_none(),
            "Rocq shrinks are declined, so the original proof is kept"
        );
        // Also declined through the public seam (whose backend really is Rocq),
        // with whatever preamble a caller supplies: no preamble can rescue it.
        let cfg = Config::default();
        let mut r = GateReplay::for_system(&cfg, FormalSystem::Rocq, "Theorem t : True")
            .with_preamble("From mathcomp Require Import ssreflect.\n");
        assert!(r.assemble(&seq(&["auto"])).is_none());
        assert!(!r.replays_closed(&seq(&["auto"])));
    }

    /// Pins the REASON Rocq is declined, so this stops being true loudly rather
    /// than silently: the preservation layer the gate conjoins parses only
    /// lowercase Lean vernacular, so a live Rocq report can never be
    /// `lexically_verified`. When this assertion flips, revisit the Rocq arm of
    /// `assemble_source` (its comment lists what else must land).
    #[test]
    fn the_gate_still_cannot_certify_any_rocq_source() {
        let report = crate::prover::statement_preservation::check_entry_signature(
            FormalSystem::Rocq,
            "Theorem t : True",
            "Theorem t : True.\nProof.\n  exact I.\nQed.\n",
        );
        assert!(
            !report.preserved,
            "Rocq preservation now parses; `assemble_source` may be able to gate Rocq"
        );
    }

    #[test]
    fn deny_all_confirms_nothing() {
        let mut deny = DenyAllReplay;
        assert!(!deny.replays_closed(&[]));
        assert!(!deny.replays_closed(&seq(&["rfl"])));
    }

    #[test]
    fn as_closure_plugs_into_the_minimizer_signature() {
        // The whole point of the module: produce the exact `FnMut(&[String]) -> bool`
        // that `minimized_proof` takes, and prove it can be called that way.
        fn takes_replay_closure<F: FnMut(&[String]) -> bool>(mut f: F, seq: &[String]) -> bool {
            f(seq)
        }
        let cfg = Config::default();
        let mut r = replay_with(&cfg, StubChecker::default());
        assert!(takes_replay_closure(as_closure(&mut r), &seq(&["rfl"])));

        let mut deny = DenyAllReplay;
        assert!(!takes_replay_closure(
            as_closure(&mut deny),
            &seq(&["rfl"])
        ));
    }

    #[test]
    fn a_mocking_config_yields_a_replay_that_confirms_nothing() {
        // `for_system` honors `Config::prover_mock`, so a mocked run declines every
        // shrink instead of certifying one from canned layers.
        let mut cfg = Config::default();
        cfg.prover_mock = true;
        let mut r = GateReplay::for_system(&cfg, FormalSystem::Lean, STMT);
        assert!(!r.replays_closed(&seq(&["rfl"])));
    }
}
