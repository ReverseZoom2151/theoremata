/-
  Theoremata — Verified-`decide` finite-certificate TEMPLATE
  ==========================================================

  Pattern mined from FrontierMathOpen-Hypergraphs (`Uniform.lean`) and Math Inc.'s
  `ZkLinalg` (`friRoundBadEvent`). It lets the model emit a *finite certificate
  table* that the Lean **kernel** checks exhaustively — closed by `decide`, never
  by `native_decide` (which trusts compiled code and is rejected by Theoremata's
  `#print axioms` soundness gate).

  The recipe has four moving parts:

    1. an abstract predicate `IsValid` — what "this certificate is correct"
       *means*, written declaratively (a spec a human/blueprint can read);
    2. a **computable** checker `checkValid : ... → Bool` — an executable decision
       procedure the kernel can reduce;
    3. a **soundness bridge** `checkValid_iff_isValid : checkValid c = true ↔ IsValid c`
       proving the checker computes exactly the abstract predicate — this is the
       load-bearing lemma; it is proved once, generically, by induction;
    4. a `Decidable (IsValid ...)` instance built from (3) via `decidable_of_iff`,
       so any *concrete* certificate is closed by kernel `decide`.

  On top of that, a `frame!`-style macro (a smart constructor + auto-discharged
  well-formedness) lets the certificate data read as a plain declarative table:
  the model writes the numbers, `by decide` proves they are well-formed, and the
  kernel replays the whole check.

  ----------------------------------------------------------------------------
  HOW THE FORMALIZER INSTANTIATES THIS TEMPLATE
  ----------------------------------------------------------------------------
  Replace the toy domain below with the real one, keeping the SAME SHAPE:

    * `IsValid`   -> your abstract correctness predicate on the certificate data
                     (e.g. "these frames cover every incompatible pair", "this
                     assignment satisfies every clause", "this list is a valid
                     Hamiltonian cycle"). Keep it a `Prop` and keep it declarative.
    * `checkValid`-> the executable `Bool` decision procedure. It MUST be
                     structurally recursive / computable (no `Classical`, no
                     `native_decide`), so the kernel can evaluate it.
    * prove the `..._iff_...` bridge by induction (usually `simp` + the recursive
       hypotheses + `decide_eq_true_eq`), then reuse the `decidable_of_iff` +
       `frame!` boilerplate verbatim.
    * the model then emits `def myCert := frame! <params> <table>` and the goal
       `IsValid <params> <table>` is discharged by `by decide` — a finite,
       kernel-checked certificate. End the file with `#print axioms myProof` and
       confirm only `[propext, Classical.choice, Quot.sound]` (or fewer) appear.

  Toolchain note: this is a syntactically Lean-4-correct TEMPLATE. It is written
  against core Lean 4 only (no Mathlib import needed for the toy), so the shape is
  portable; swap in Mathlib types/lemmas as your real domain requires.
-/

namespace Theoremata.VerifiedDecide

/-! ### 1. Abstract predicate (the SPEC)

Toy domain: a certificate is a `List Nat` ("a table of entries"). It is *valid*
w.r.t. a `bound` iff every entry is `< bound` and the entries are strictly
increasing. Both halves are written as declarative, recursive `Prop`s — this is
the human-readable meaning, deliberately kept distinct from the checker. -/

/-- Every entry is strictly below `bound`. -/
def AllLt (bound : Nat) : List Nat → Prop
  | []        => True
  | a :: rest => a < bound ∧ AllLt bound rest

/-- Consecutive entries are strictly increasing. -/
def SortedLt : List Nat → Prop
  | []             => True
  | [_]            => True
  | a :: b :: rest => a < b ∧ SortedLt (b :: rest)

/-- The abstract correctness predicate for a certificate `t` at a given `bound`. -/
def IsValid (bound : Nat) (t : List Nat) : Prop :=
  AllLt bound t ∧ SortedLt t

/-! ### 2. Computable checker (the DECISION PROCEDURE)

A `Bool`-valued mirror of the spec. Everything here reduces in the kernel:
`decide (a < b)` on literals evaluates to `true`/`false`, and the recursion is
structural. No `Classical`, no `native_decide`. -/

/-- Executable check that every entry is `< bound`. -/
def allLt (bound : Nat) : List Nat → Bool
  | []        => true
  | a :: rest => decide (a < bound) && allLt bound rest

/-- Executable check that consecutive entries strictly increase. -/
def sortedLt : List Nat → Bool
  | []             => true
  | [_]            => true
  | a :: b :: rest => decide (a < b) && sortedLt (b :: rest)

/-- The computable certificate checker. -/
def checkValid (bound : Nat) (t : List Nat) : Bool :=
  allLt bound t && sortedLt t

/-! ### 3. Soundness bridge (the load-bearing lemma)

Prove the checker computes exactly the abstract predicate. Done once, by
structural induction; each concrete certificate then rides on this. -/

theorem allLt_iff (bound : Nat) : ∀ t, allLt bound t = true ↔ AllLt bound t
  | []        => by simp [allLt, AllLt]
  | a :: rest => by
      simp [allLt, AllLt, Bool.and_eq_true, decide_eq_true_eq, allLt_iff bound rest]

theorem sortedLt_iff : ∀ t, sortedLt t = true ↔ SortedLt t
  | []             => by simp [sortedLt, SortedLt]
  | [_]            => by simp [sortedLt, SortedLt]
  | a :: b :: rest => by
      simp [sortedLt, SortedLt, Bool.and_eq_true, decide_eq_true_eq,
            sortedLt_iff (b :: rest)]

/-- **Soundness**: the computable checker is `true` exactly when the abstract
predicate holds. This is the bridge that makes `decide` trustworthy here. -/
theorem checkValid_iff_isValid (bound : Nat) (t : List Nat) :
    checkValid bound t = true ↔ IsValid bound t := by
  simp [checkValid, IsValid, Bool.and_eq_true, allLt_iff, sortedLt_iff]

/-! ### 4. `Decidable` instance from the bridge

`decidable_of_iff` turns the decidability of the `Bool` equation into decidability
of the abstract predicate. Now `by decide` on any *concrete* certificate reduces
`checkValid`, matches it against `true`, and transports along the bridge — all in
the kernel. -/

instance instDecidableIsValid (bound : Nat) (t : List Nat) :
    Decidable (IsValid bound t) :=
  decidable_of_iff (checkValid bound t = true) (checkValid_iff_isValid bound t)

/-! ### 5. `frame!`-style smart constructor + macro

Bundle the certificate data with its validity proof, and auto-discharge the
well-formedness side-condition with `by decide` so the data table reads
declaratively. Mirrors the `frame!` elaborator from FrontierMath's `Uniform.lean`. -/

/-- A well-formed certificate at a fixed `bound`: the data plus a proof it is
valid. The proof field is what `frame!` fills in by `decide`. -/
structure Cert (bound : Nat) where
  table : List Nat
  valid : IsValid bound table

/-- Smart constructor taking the validity proof explicitly. -/
def mkCert (bound : Nat) (table : List Nat) (h : IsValid bound table) : Cert bound :=
  ⟨table, h⟩

/-- `frame! b t` builds a `Cert b` from a table `t`, discharging the
well-formedness obligation `IsValid b t` by kernel `decide`. -/
macro "frame!" bound:term:max table:term:max : term =>
  `(mkCert $bound $table (by decide))

/-! ### 6. Worked toy instance

Everything below is what the *model emits* per problem: a finite certificate
table and a one-line `decide` proof. The kernel checks it exhaustively. -/

/-- A concrete valid certificate: entries `[1, 3, 7]` are all `< 10` and strictly
increasing. `frame!` proves well-formedness by `decide`. -/
def toyCert : Cert 10 := frame! 10 [1, 3, 7]

/-- The finite certificate check, closed by kernel `decide` (NOT `native_decide`). -/
theorem toyValid : IsValid 10 [1, 3, 7] := by decide

/-- A negative sanity check: `[3, 1]` is not strictly increasing, so it is *not*
valid — also decided by the kernel. -/
theorem toyInvalid : ¬ IsValid 10 [3, 1] := by decide

-- Soundness confirmation the formalizer should run on the real proof; for a
-- pure `decide` proof this reports `[propext]` (or no axioms), never `sorryAx`
-- nor `Lean.ofReduceBool` (the native-decide trust axiom):
--   #print axioms toyValid

end Theoremata.VerifiedDecide
