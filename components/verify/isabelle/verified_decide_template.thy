(*
  Theoremata — Verified code-reflection / `eval` TEMPLATE (Isabelle/HOL)
  =====================================================================

  Isabelle counterpart of `components/verify/lean/verified_decide_template.lean`.
  It lets the model emit a *finite certificate table* that Isabelle's **kernel**
  checks exhaustively — closed by `by eval` (or `by normalization`), the
  code-reflection idiom for decidable goals.

  Trust note: `eval` compiles the goal via the code generator to ML, evaluates
  it, and — crucially — feeds the result back through the `Eval` oracle... but
  the modern `eval` method used here reconstructs a genuine kernel proof by
  *rewriting with the code equations* (the `Code_Evaluation`/`Nbe` path), so a
  proof closed by `by normalization` is FULLY kernel-checked and leaves NO
  oracle in `thm_oracles`. Contrast:
    * `by normalization`  — kernel-checked normalization-by-evaluation; NO oracle.
    * `by eval`           — code-generator evaluation; on the reflective path it
                            is kernel-checked, but be aware some configurations
                            route through the `Eval` oracle. PREFER `normalization`
                            (or `by (simp add: ...)`) when you need a guaranteed
                            oracle-free proof, and ALWAYS confirm with
                            `thm_oracles` (see validate_proof_template.thy).
    * `code_reflect` / a custom `oracle`  — genuine escape hatches: they install
                            an ML function as an axiom-producing oracle, which
                            WILL show up (tagged) in `thm_oracles`. Never in the gate.

  The recipe has four moving parts, mirroring the Lean template one-for-one:

    1. an abstract predicate `is_valid` — what "this certificate is correct"
       *means*, written declaratively (a spec a human/blueprint can read);
    2. a **computable** checker `check_valid :: ... => bool` — an executable
       decision procedure the code generator / `eval` can reduce;
    3. a **soundness bridge** `check_valid c <-> is_valid c` — proving the checker
       computes exactly the abstract predicate (the load-bearing lemma, proved
       once by induction);
    4. rewriting the abstract goal along (3) leaves a closed `bool` computation
       that `by eval`/`by normalization` crunches in the kernel — the finite,
       kernel-checked certificate replay.

  ----------------------------------------------------------------------------
  HOW THE FORMALIZER INSTANTIATES THIS TEMPLATE
  ----------------------------------------------------------------------------
  Replace the toy domain below with the real one, keeping the SAME SHAPE:

    * `is_valid`    -> your abstract correctness predicate on the certificate
                       data (e.g. "these frames cover every incompatible pair").
                       Keep it a `bool`-returning HOL function or a `Prop`-level
                       predicate with an executable mirror.
    * `check_valid` -> the executable decision procedure. It MUST have code
                       equations (be `fun`/`primrec` or have `[code]` lemmas), so
                       the code generator can evaluate it.
    * prove the `..._iff` bridge by induction (usually `induction ... simp`),
                       then reuse the boilerplate.
    * the model then emits `definition my_cert = ...` and closes
      `is_valid my_cert` by `by (simp add: is_valid_iff) eval` (rewrite along the
      bridge, then evaluate) — a finite, kernel-checked certificate. Run
      `thm_oracles my_thm` and confirm the oracle set is EMPTY.

  Toolchain note: syntactically Isabelle/HOL-correct against `Main` only (no AFP
  needed for the toy). Treat this as the SHAPE of the reflection gate; adjust to
  your pinned Isabelle release (studied against Isabelle2025-2).
*)

theory verified_decide_template
  imports Main
begin

subsection \<open>1. Abstract predicate (the SPEC)\<close>

text \<open>
  Toy domain: a certificate is a @{typ "nat list"} ("a table of entries"). It is
  \emph{valid} w.r.t. a @{term bound} iff every entry is @{text "< bound"} and the
  entries are strictly increasing. Both halves are declarative recursive
  functions — the human-readable meaning, kept distinct from the checker.\<close>

primrec all_lt :: "nat \<Rightarrow> nat list \<Rightarrow> bool" where
  "all_lt bound [] = True"
| "all_lt bound (a # rest) = (a < bound \<and> all_lt bound rest)"

fun sorted_lt :: "nat list \<Rightarrow> bool" where
  "sorted_lt [] = True"
| "sorted_lt [_] = True"
| "sorted_lt (a # b # rest) = (a < b \<and> sorted_lt (b # rest))"

text \<open>The abstract correctness predicate for a certificate @{term t} at @{term bound}.\<close>
definition is_valid :: "nat \<Rightarrow> nat list \<Rightarrow> bool" where
  "is_valid bound t \<longleftrightarrow> all_lt bound t \<and> sorted_lt t"

subsection \<open>2. Computable checker (the DECISION PROCEDURE)\<close>

text \<open>
  A @{typ bool}-valued mirror of the spec. Because @{const all_lt},
  @{const sorted_lt} and @{const check_valid} are defined by @{command primrec}/
  @{command fun}, the code generator derives code equations automatically, so
  @{method eval}/@{method normalization} can reduce them in the kernel. No
  @{command axiomatization}, no oracle.

  (Here the "checker" coincides with the spec functions themselves — in a real
  instance the spec may be a non-executable @{typ bool}/@{typ prop} predicate and
  @{term check_valid} its separate executable refinement; the bridge in \S3 then
  does real work.)\<close>

definition check_valid :: "nat \<Rightarrow> nat list \<Rightarrow> bool" where
  "check_valid bound t \<longleftrightarrow> all_lt bound t \<and> sorted_lt t"

subsection \<open>3. Soundness bridge (the load-bearing lemma)\<close>

text \<open>
  Prove the checker decides exactly the abstract predicate. For this toy the two
  are definitionally equal, so the bridge is immediate; keep the lemma explicit
  because in a real instance it is proved by structural induction and is what
  makes @{method eval} trustworthy.\<close>

lemma check_valid_iff: "check_valid bound t \<longleftrightarrow> is_valid bound t"
  by (simp add: check_valid_def is_valid_def)

subsection \<open>4. Worked toy instance\<close>

text \<open>
  Everything below is what the \emph{model emits} per problem: a finite
  certificate table and a one-line reflection proof. The kernel checks it
  exhaustively.\<close>

definition toy_cert :: "nat list" where
  "toy_cert = [1, 3, 7]"

text \<open>
  A concrete valid certificate: entries @{term "[1, 3, 7]::nat list"} are all
  @{text "< 10"} and strictly increasing. Closed by kernel-checked evaluation.
  We rewrite along the bridge, unfold the definition, then evaluate the closed
  boolean. @{method normalization} is the guaranteed oracle-free closer; a bare
  @{method eval} also works.\<close>

lemma toy_valid: "is_valid 10 toy_cert"
  unfolding check_valid_iff[symmetric] check_valid_def toy_cert_def
  by normalization

text \<open>Equivalent one-liner (evaluate the whole decidable proposition directly):\<close>
lemma toy_valid': "is_valid 10 toy_cert"
  by (simp add: is_valid_def toy_cert_def)

text \<open>
  A negative sanity check: @{term "[3, 1]::nat list"} is not strictly increasing,
  so it is \emph{not} valid — also decided by the kernel.\<close>
lemma toy_invalid: "\<not> is_valid 10 [3, 1]"
  by (simp add: is_valid_def)

text \<open>
  Soundness confirmation the formalizer should run on the real proof: the oracle
  audit must come back EMPTY (no @{text Pure.skip_proof} from @{command sorry}, no
  custom @{command oracle}, no @{method eval} @{text Eval}-oracle):

    thm_oracles toy_valid        \<comment> \<open>expect: (no oracles)\<close>

  Independent kernel re-check (separate clean build in a fresh environment):

    isabelle build -c -o quick_and_dirty=false -d . Theoremata_Scratch   \<comment> \<open>return code 0\<close>
\<close>

end
