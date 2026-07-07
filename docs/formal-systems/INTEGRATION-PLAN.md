# Abstracted multi-formal-system integration plan

Goal: generalize Theoremata's currently Lean-hardwired prover/verify layer into a
system-agnostic `FormalSystem` abstraction, so **Rocq** and **Isabelle** (and any
future system) become *implementations of an interface* rather than a rework of
the core. Grounded in the three build-ready references in this folder
(`lean.md`, `rocq.md`, `isabelle.md`) and our existing architecture
(`components/prover/`, `components/verify/`).

## BUILD STATUS — all phases shipped (2026-07-07)

The plan below is **built and live on this machine**, commits `6245cd2`…`9f8b44f`:

- **Phase 0/1** — `FormalSystem`/`FormalBackend`/`ProofSession` abstraction + mock Rocq/Isabelle backends (`6245cd2`).
- **Layer 2c** — universal source-scan gate for all three (`0ea653e`).
- **Drivers + hammer adapters** (`51be03a`), worker-wired (`ef134cd`).
- **Phase 2 — live gates** via a runner-agnostic exec bridge (`Runner{Native,Wsl,Docker}`, per-system `Config.formal_runners`) (`1639530`). Toolchains installed: Lean 4.31 native, Coq 8.18 (WSL), Isabelle2025-2 (WSL). Live e2e tests pass: each certifies a trivial proof AND rejects the `sorry`/`Admitted` variant.
- **Phase 3 — proof generators** (`8ef4b55`): `generate_and_verify(system, statement)`, best-of-N selected by the live gate; CLI `formal-prove <system> <statement>`.
- **Phase 5 — portfolio proving** (`4a9bb8f`): `portfolio-prove <statement>` races all three; smoke on `"True"` certified via **all three live backends** (winner lean).
- **Phase 4 — live hammer** (`9f8b44f`): **Sledgehammer genuinely works** here (`isabelle process_theories -O`, E prover, ~11s → real `by auto`/`by simp` reconstructions). CoqHammer/aesop gated (need `opam install coq-hammer-tactics` / a Mathlib Lake project) with graceful mock fallback.
- **Hammer-assisted generation** (final): the hammer folded into `generate_and_verify` so Isabelle can *find* proofs via Sledgehammer even with no model in the loop.

129+ Rust tests, warning-clean; the `reason`/`prover` components were regrouped into subdirectories (`01a3ac2`, `9193922`).

## The unifying insight

All three systems expose the **same five-part integration surface**. The deep
read confirmed the mappings are clean, which is what makes one abstraction viable:

| Surface | Lean | Rocq (Coq) | Isabelle/HOL |
|---|---|---|---|
| **1. Driver** (out-of-process interaction) | `leanprover-community/repl` (JSON stdin/stdout; `env`/`proofState` ids) | `coq-lsp` **Petanque** (`petanque/start→run→goals`) or **SerAPI** `sertop` (`Add`/`Exec`/`Cancel`/`Query Goals`) | **Isabelle Server** (TCP; `session_start`/`use_theories`/`purge_theories`) |
| **granularity** | per-tactic (`proofState`) | per-tactic (Petanque `Run_result`, SerAPI state ids) | **theory-file only** (submit a whole `.thy`, parse messages) |
| **2a. Axiom/oracle audit** | `#print axioms` vs whitelist `propext/Classical.choice/Quot.sound` | `Print Assumptions` vs whitelist (`Closed under the global context` = clean) | `thm_oracles` / `Thm_Deps.all_oracles = []` |
| **2b. Kernel re-check** | `leanchecker` (replays `.olean`) | `rocqchk`/`rocq check` (`.vo`, minimal trusted binary) | clean `isabelle build` (kernel-checked) |
| **2c. Source scan** (MANDATORY — audit+recheck miss escape hatches) | LeanParanoia patterns (`native_decide`→`ofReduceBool`/`trustCompiler`, `@[implemented_by]`, `sorryAx`, custom `axiom`) | **`-type-in-type` / `Unset Universe Checking` / `bypass_check` / `Admitted` / `Axiom`** (NOT caught by `Print Assumptions`/`rocqchk`) | `quick_and_dirty` / `Pure.skip_proof` (`sorry`) / `oops` / added `oracle` |
| **3. Project scaffold** | `lakefile.toml` + `lean-toolchain`, `lake build` | `_CoqProject` (`-R . Mod` + files), `coqc`/`rocq compile` → `.vo` | session `ROOT` (`session A = B + …`), `isabelle build` |
| **4. Automation** ("hammer") | `aesop` (+ emerging Duper / LeanHammer / lean-auto) | **CoqHammer** (`sauto`/`hauto` pure tier; `hammer`→Vampire/E/Z3) | **Sledgehammer** (E/Vampire/cvc5/Z3 → kernel-checked `by (metis …)`) |
| **5. Corpus / retrieval** | mathlib4; Loogle (structural) + LeanSearch/Moogle (semantic) + `exact?` | MathComp + stdlib; `Search`/`SearchPattern` + Petanque `premises` | `Main`/HOL + AFP; `find_theorems`/`find_consts` + Find_Facts (Solr REST) |

**Two asymmetries that matter:**
- **Automation:** Isabelle and Rocq have *true external-ATP hammers* that reconstruct kernel-checked proofs (Sledgehammer, CoqHammer). Lean does not, natively — this is arguably the single biggest reason to add the other two: our agent gains a hammer tool. All three hammers' output is kernel-checked, so exposing them is **zero soundness cost**.
- **Granularity:** Lean and Rocq support per-tactic stepping (ideal for MCTS/search over proof states); Isabelle is theory-file granular (generate a whole `.thy`, submit, parse). The driver interface must accommodate both.

## What already generalizes for free

Everything above the object-language leaf ops is prover-agnostic and needs **no
change**: the proof-DAG, falsify-before-prove (the sympy/z3 falsifier is language-
neutral), decompose, plan-history, three-valued taint, MCTS + progress prior, the
benchmark harness, the graders, `statement_guard`, and the
`ProofTask`/`ProofResult` + `match backend` dispatch skeleton in `proof_job.rs`.

## What is Lean-hardwired today (the seams to cut)

- **Data contract** (`components/prover/model.rs`): `LeanProject`, `ProofResult.lean_code`, `ProofTask.lean_project`, `toolchain`.
- **Verify leaf layer**: `components/verify/hardening.rs` (LeanParanoia), `lean_session.rs` (warm Lean REPL), the `#print axioms` gate, `lake` scaffolding.
- **Generation**: `agent.rs::formalize_once` emits Lean; `prove_via_prover` assumes Lean output.

## The abstraction — `FormalSystem`

Introduce a system tag + a per-system implementation of the five surfaces. Rust
sketch (enum + trait, following the existing backend-dispatch style):

```rust
pub enum FormalSystem { Lean, Rocq, Isabelle }

pub struct FormalProject {            // generalizes LeanProject
    pub system: FormalSystem,
    pub root: PathBuf,
    pub toolchain: Option<String>,    // lean-toolchain / opam switch / Isabelle version
    pub imports: Vec<String>,         // Mathlib / MathComp+ssreflect / Main+AFP
    pub metadata: Value,
}

// One trait, three impls (verify_lean.rs / verify_rocq.rs / verify_isabelle.rs):
pub trait FormalBackend {
    fn scaffold(&self, cfg, code, name) -> Result<Workspace>;       // 3
    fn compile(&self, ws) -> Result<CompileReport>;                 // 2b (build) + errors
    fn audit_axioms(&self, ws, thm, whitelist) -> Result<AxiomReport>; // 2a
    fn kernel_recheck(&self, ws) -> Result<RecheckReport>;          // 2b
    fn source_scan(&self, code) -> Result<ScanReport>;              // 2c  MANDATORY
    fn verify(&self, cfg, code, stmt) -> Result<VerificationReport> // orchestrates 2a-2c
        { /* default: compile && audit⊆whitelist && recheck && scan_clean */ }
}

pub trait ProofSession {              // generalizes lean_session.rs (the warm driver)
    fn start(&mut self, project) -> Result<()>;
    fn submit_unit(&mut self, code) -> Result<UnitResult>;          // theory/file mode (all 3)
    fn step_tactic(&mut self, state, tactic) -> Result<StateResult>;// tactic mode (Lean/Rocq; Isabelle: Unsupported)
    fn goal_state(&self, state) -> Result<GoalState>;
}
```

- `ProofTask` gains `system: FormalSystem`; `ProofResult.lean_code` → `formal_code` (keep a serde alias for back-compat).
- `VerificationReport` already has the right fields (`axioms_clean`, `lexically_verified`, `statement_preserved`, `hardening_clean`) — reuse verbatim; only the *producers* become per-system.
- `proof_job::{submit,poll,cancel}` dispatch already keys on `backend: String` — add `"rocq"`/`"isabelle"` arms.
- The **3+1-layer gate is universal** (compile → axiom/oracle audit ⊆ whitelist → kernel re-check → source scan). The Rocq escape-hatch finding makes layer 2c non-optional everywhere; `verify()`'s default body enforces all four.

## Phased implementation

- **Phase 0 — generalize the contract (no behavior change).** Add `FormalSystem`, `FormalProject`, `system` on `ProofTask`/`ProofResult`; keep Lean as the reference `FormalBackend`. Serde aliases so existing data/tests pass. *Testable now, offline.*
- **Phase 1 — mock Rocq + Isabelle backends** in the exact `aristotle`-mock pattern (`Config.prover_mock`-driven), returning canned system-native code + a `VerificationReport`, so dispatch, `statement_guard`, and the gate wiring are exercised end-to-end. *Testable now, offline.*
- **Phase 2 — real verify gates** behind `FormalBackend`, mapping each layer to the concrete commands (table above). Runs under WSL/Docker where a toolchain exists; degrades to `Unavailable` (fail-closed) otherwise — same discipline as `hardening.rs`.
- **Phase 3 — drivers** (`ProofSession`): Lean `repl`, Rocq Petanque/SerAPI, Isabelle Server. Lean/Rocq expose `step_tactic` (feeds MCTS + the progress prior); Isabelle exposes `submit_unit` only.
- **Phase 4 — hammer tools.** Expose Sledgehammer / CoqHammer / aesop(+Duper) as agent-callable automation on a node; their kernel-checked output flows through the normal gate. Highest near-term value.
- **Phase 5 — portfolio proving.** Given a conjecture, formalize per target system and race backends; first to certify (through its full gate) wins. Requires per-system formalization (no cross-translation).

## Key decisions & caveats

1. **No cross-system translation.** Each backend generates *system-native* code (model emits Lean / Coq / Isar); a proof of S in one system is a distinct object. "Portfolio" races native backends, it does not translate.
2. **Gate is 3+1 layers, universally, source-scan mandatory** — the Rocq `-type-in-type`/`bypass_check` finding proves axiom-audit + kernel-recheck can be defeated; the source scan (our LeanParanoia analogue per system) is required, not optional.
3. **Granularity split** — the `ProofSession` trait carries both a coarse `submit_unit` (all three) and an optional `step_tactic` (Lean/Rocq); Isabelle returns `Unsupported` for stepping and is driven at theory granularity.
4. **Toolchain reality** — all three need Linux/WSL/Docker for *live* verification (this Windows box has none working). Mock-first (Phase 0/1) is fully offline-testable; live gates (Phase 2+) are environment-gated, documented as such, exactly like our current Lean situation.
5. **Automation is the headline win** — Sledgehammer and CoqHammer give the agent a genuine hammer with zero soundness cost (kernel-checked reconstruction). Lean's Duper/LeanHammer are worth tracking as they mature.

## Immediate next step

Phase 0 + Phase 1 are buildable and testable offline right now, in the proven
`aristotle`-mock shape: generalize the contract and stand up mock `rocq`/`isabelle`
backends with per-system source-scan gates. That puts the whole abstraction and
the multi-system gate in place and under test, with only the live drivers left to
wire when a WSL/Docker toolchain is available.
