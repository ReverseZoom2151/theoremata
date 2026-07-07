/-
  Theoremata — LeanDojo `validateProof` in-kernel SOUNDNESS-GATE template
  ======================================================================

  Ported from LeanDojo's interactive REPL (`Lean4Repl.lean`, `dojo.py`
  `check_proof`): the small but load-bearing routine that decides whether a
  candidate proof term *actually* closes the target theorem before we trust a
  "no goals" outcome. It is the in-Lean analogue of Theoremata's
  `#print axioms` / LeanParanoia soundness gate: rather than replaying `#print
  axioms` out-of-band, it runs four checks *inside* the Lean process on the
  close-path, so a metavariable-riddled or `sorry`-carrying "proof" is rejected
  before it can be reported as verified.

  THE FOUR CHECKS (all must pass to accept a closed goal):

    1. `instantiateMVars` the assigned proof term, so every solved metavariable
       is substituted in — we validate the *actual* term, not a placeholder.
    2. `isDefEq` the term's type against the declared target under `.all`
       transparency (unfold everything): the proof must inhabit *this* goal.
    3. reject `pf.hasSorry` — no `sorryAx`, and reject `pf.hasExprMVar` — no
       unsolved holes left dangling in the term.
    4. kernel re-check via `addDecl (.thmDecl …)`: hand the term to the trusted
       kernel as a theorem declaration. The elaborator is large and heuristic;
       the kernel is the small trusted core. If `addDecl` succeeds, the proof
       typechecks at the axiomatic foundation. (`addDecl` also re-runs the kernel
       so a proof that only "passed" via elaborator defeq still has to survive
       kernel reduction.)

  Only when all four pass do we return `.accepted`. This is deliberately stricter
  than "the tactic block elaborated": it is the gate that lets the search treat a
  `reward = 1.0` terminal as genuinely certified.

  ----------------------------------------------------------------------------
  HOW OUR WARM REPL (`components/verify/lean_session.rs`) CALLS THIS
  ----------------------------------------------------------------------------
  `lean_session.rs` holds ONE warm Mathlib environment and exchanges JSON lines
  with the Python `lean_repl serve` loop. On the CLOSE-PATH — when a candidate
  tactic script drives a goal to `no goals` — the server would call
  `validateProof env target pf` (below) on the elaborated proof term instead of
  trusting the tactic outcome, and fold the boolean into the existing
  `CheckOutcome`:

      { "ok": <result.isAccepted>,
        "axioms_clean": <#print axioms closure ⊆ allowlist>,
        "messages": [ <result reason on reject> ],
        "axioms":   [ … ] }

  So `validateProof` is the *soundness* half (is this term a real, hole-free
  proof of the target?) and the `#print axioms` allowlist check is the *trust*
  half (does it lean only on permitted axioms — no `native_decide`/`sorryAx`?).
  Both must hold; `lean_session.rs::check` already surfaces `ok && axioms_clean`.

  ----------------------------------------------------------------------------
  SAVED-STATE ARRAY (`savedStates`, branch-by-`sid`) — MCTS BACKTRACKING NOTE
  ----------------------------------------------------------------------------
  LeanDojo's REPL stores proof states by integer id and runs each tactic against
  a *saved* state, returning a fresh id for the result. That id-addressable store
  is exactly what MCTS needs: a node in `components/reason/mcts.rs` is a `sid`
  into `savedStates`, expansion runs a tactic against `savedStates[sid]` yielding
  a child `sid`, and BACKTRACKING is O(1) — re-select any earlier `sid` without
  replaying the tactic prefix. `validateProof` is invoked on the close-path of a
  branch (when a tactic reports `no goals` for `savedStates[sid]`) to certify
  that leaf before backpropagating `reward = 1.0`. The sketch below shows the
  store shape; the real REPL would guard concurrent access and cap the array.

  ----------------------------------------------------------------------------
  Toolchain note: this is a syntactically Lean-4 TEMPLATE using `Lean.Meta` /
  `Lean.Elab` APIs. Names track LeanDojo's usage; exact signatures drift across
  Lean/Mathlib versions, so treat this as the SHAPE of the gate — adjust
  `MetaM`/`TermElabM` entry points to your pinned toolchain. It need not compile
  against a specific Mathlib.
-/

import Lean
open Lean Lean.Meta Lean.Elab

namespace Theoremata.ValidateProof

/-- Outcome of the soundness gate: either the proof term is accepted, or it is
rejected with a human-readable reason (surfaced in `CheckOutcome.messages`). -/
inductive ValidateResult where
  | accepted
  | rejected (reason : String)
  deriving Inhabited

def ValidateResult.isAccepted : ValidateResult → Bool
  | .accepted     => true
  | .rejected _   => false

/-- The in-kernel soundness gate.

`target` is the declared proposition (the goal's `Expr`), `pf` is the candidate
proof term the elaborator produced for it. Runs the four checks in order and
short-circuits on the first failure. Intended to be called by the warm REPL on
the close-path, in the theorem-local `MetaM` context. -/
def validateProof (target : Expr) (pf : Expr) (thmName : Name := `_theoremata_candidate) :
    MetaM ValidateResult := do
  -- (1) Substitute solved metavariables so we validate the ACTUAL term.
  let pf ← instantiateMVars pf
  -- (3a) No `sorryAx` may hide inside the term.
  if pf.hasSorry then
    return .rejected "proof term contains `sorry` (sorryAx)"
  -- (3b) No unsolved metavariables may remain.
  if pf.hasExprMVar then
    return .rejected "proof term still has unassigned metavariables"
  -- (2) The term must inhabit THIS goal: its type is defeq to `target` under
  -- `.all` transparency (unfold everything — the strictest defeq check).
  let pfType ← inferType pf
  let defeq ← withTransparency .all (isDefEq pfType target)
  unless defeq do
    return .rejected "proof term's type is not defeq to the target"
  -- (4) Kernel re-check: hand the term to the trusted kernel as a theorem.
  -- `addDecl` typechecks against the axiomatic core; elaborator-only defeq that
  -- the kernel rejects is caught here. We use a throwaway name in a scratch env.
  let decl := Declaration.thmDecl {
    name        := thmName
    levelParams := []
    type        := target
    value       := pf
  }
  try
    let env ← getEnv
    let _ ← ofExceptKernelException (env.addDecl {} decl)
    return .accepted
  catch e =>
    return .rejected s!"kernel rejected the proof term: {← e.toMessageData.toString}"

/-! ### Saved-state store sketch (`savedStates`, branch-by-`sid`)

The id-addressable proof-state store that makes MCTS backtracking O(1). Each
tactic step runs against `savedStates[sid]` and appends a new state, returning
its id; MCTS holds these ids as node states and re-selects any earlier `sid`
without replaying the prefix. `validateProof` runs when a step closes a goal. -/

/-- A single stored proof state (goals + local context live inside `mvarId`'s
`MetavarContext`; kept abstract here). -/
structure SavedState where
  mvars : List MVarId
  deriving Inhabited

/-- The saved-state array, indexed by `sid`. LeanDojo keeps this per-REPL. -/
structure SavedStates where
  states : Array SavedState := #[]

/-- Store a state and return its `sid` (its index) — the branch handle. -/
def SavedStates.push (s : SavedStates) (st : SavedState) : SavedStates × Nat :=
  (⟨s.states.push st⟩, s.states.size)

/-- Fetch the state to branch from by `sid`; `none` if out of range. -/
def SavedStates.get? (s : SavedStates) (sid : Nat) : Option SavedState :=
  s.states[sid]?

/-! Worked shape — what the REPL does on the close-path of `savedStates[sid]`:

```text
run tactic on savedStates[sid]  ⟶  goals empty?
  │ no  ⟶  push resulting state, return fresh sid (MCTS child node)
  │ yes ⟶  validateProof target pf
             ├ .accepted     ⟶ certify leaf, backprop reward = 1.0
             └ .rejected msg ⟶ discard: NOT a sound proof of the target
```
-/

end Theoremata.ValidateProof
