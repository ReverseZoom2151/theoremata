(*
  Theoremata — Rocq/Coq SOUNDNESS-GATE template (`Print Assumptions` + `rocqchk`)
  ==============================================================================

  Rocq counterpart of `components/verify/lean/validate_proof_template.lean`.
  Where the Lean template runs the LeanDojo `validateProof` checks *inside* the
  Lean process (instantiate mvars, defeq, reject sorry/mvars, kernel `addDecl`),
  Rocq's soundness gate is naturally SPLIT across the language and two external
  tools — because Rocq is an LCF-style kernel with an out-of-band checker:

    * The `Qed`/`Defined` step ALREADY hands the proof term to the trusted
      kernel and type-checks it (the analogue of Lean's `addDecl` kernel
      re-check happens automatically at `Qed`). A proof that "elaborated" but
      does not kernel-check simply fails at `Qed` — you never get a `thm`.
    * `Print Assumptions <thm>.` is the in-Rocq axiom audit — the `#print axioms`
      analogue: it walks the FULL transitive dependency graph and reports every
      axiom / parameter / section variable the term rests on.
    * `rocqchk` / `coqchk` re-runs the kernel over the compiled `.vo` in a
      SEPARATE, minimal trusted-base binary that plugins / `Ltac` / vernacular
      cannot taint — the LeanParanoia independent-replay analogue.
    * A source scan catches the escape hatches that are NOT axioms and so do NOT
      appear in `Print Assumptions` (`Unset Guard/Positivity/Universe Checking`,
      `-type-in-type`, `bypass_check`).

  This file is a WORKED REFERENCE: a genuine `Theorem ... Qed.`, then a doc block
  showing exactly what the gate runs and what a PASS looks like. It is the
  reference the live gate (each system's CLI) mirrors — it is not itself wired
  into the runtime.

  THE FOUR CHECKS (all must pass to accept a closed goal — parity with Lean):

    1. It must be a real `thm`: closed by `Qed` (opaque) or `Defined`
       (transparent) — NOT `Admitted` (which declares the goal as an axiom) and
       with no `admit`/`give_up` left in the script. Reaching `Qed` means the
       kernel already type-checked the term (checks 1+2+3 of Lean's in-process
       gate collapse into the `Qed` step here).
    2. `Print Assumptions <thm>.` ⇒ "Closed under the global context"
       (or a set ⊆ the classical-logic whitelist). This is the axiom/trust half.
    3. Independent kernel replay: compile to `.vo` and run
       `rocqchk -o -silent` — exit 0 in the untainted checker binary.
    4. Source scan rejects the silent kernel-disabling escape hatches.

  Only when all four pass is a `reward = 1.0` terminal treated as certified.

  ----------------------------------------------------------------------------
  HOW A WARM Rocq DRIVER WOULD CALL THIS (parity with lean_session.rs)
  ----------------------------------------------------------------------------
  A Rocq step loop (coq-lsp `petanque/run` or SerAPI `Add`/`Exec`) drives tactics
  until `proof_finished = true`, then closes with `Qed`. On that close-path it
  would fold the gate into the same `CheckOutcome` the Lean session emits:

      { "ok":           <Qed succeeded && no admit/Admitted in script>,
        "axioms_clean": <Print Assumptions ⊆ whitelist>,
        "messages":     [ <rocqchk stderr / scan hits on reject> ],
        "axioms":       [ <names parsed from Print Assumptions> ] }

  So `ok` is the *soundness* half (is this a real, hole-free kernel-checked proof
  of the target?) and `axioms_clean` is the *trust* half (does it lean only on
  permitted axioms — no admits, no disabled checks?). Both must hold.

  Toolchain note: syntactically Rocq-correct against the stdlib; the vernacular
  in the doc block (`Print Assumptions`, `rocqchk` invocation) is exact, but
  treat the whole thing as the SHAPE of the gate — pin to your Rocq version.
*)

Require Import PeanoNat.
Require Import List.
Import ListNotations.

Module TheoremataValidateProof.

(* --------------------------------------------------------------------------- *)
(* 1. A worked, genuinely-closed theorem — the ACCEPT case.                     *)
(*                                                                              *)
(* Closed by `Qed`, so the kernel has already type-checked the proof term. It   *)
(* uses only constructive stdlib lemmas, so its assumption set is empty. This   *)
(* is the shape the model emits and the gate certifies.                         *)
(* --------------------------------------------------------------------------- *)

Theorem add_comm_example : forall n m : nat, n + m = m + n.
Proof.
  intros n m. induction n as [| n IH]; simpl.
  - rewrite Nat.add_0_r. reflexivity.
  - rewrite IH, Nat.add_succ_r. reflexivity.
Qed.

(* A second worked example closing a decidable goal by computation — still a
   real `thm`, still axiom-free (kernel-checked at `Qed`). *)
Theorem concrete_fact : 2 + 3 = 5.
Proof. reflexivity. Qed.

(* --------------------------------------------------------------------------- *)
(* 2. The axiom audit — `Print Assumptions` (the `#print axioms` analogue).     *)
(* --------------------------------------------------------------------------- *)

(* Run this on the target. A clean, constructive proof prints EXACTLY:

     Print Assumptions add_comm_example.
     (* ==> Closed under the global context *)

   The gate PASSES iff the output is either that green-light string, or a listed
   `Axioms:` set that is a subset of the approved classical whitelist, commonly:

     classic
     functional_extensionality  functional_extensionality_dep
     proof_irrelevance
     Eqdep.Eq_rect_eq.eq_rect_eq
     constructive_indefinite_description

   ANY other name — an unexpected `Axiom`, a `Parameter`, or a section
   `Variable` — FAILS the gate. Crucially, an `Admitted` lemma surfaces here as
   an axiom named after the lemma, so admits cannot hide from this audit.

   Finer-grained companions (same vernacular family) for provenance:
     Print Opaque Dependencies add_comm_example.       (* Qed-sealed constants *)
     Print Transparent Dependencies add_comm_example.  (* unfoldable constants *)
     Print All Dependencies add_comm_example.          (* axioms + all constants *)
*)

(* --------------------------------------------------------------------------- *)
(* 3. Independent kernel re-check — `rocqchk` / `coqchk`.                        *)
(* --------------------------------------------------------------------------- *)

(*
   Compile this file to a `.vo`, then re-check it in the standalone kernel
   binary — a minimal trusted base that plugins / Ltac / vernac cannot taint
   (the LeanParanoia replay analogue):

     rocq compile -R . Gen ValidateProof.v          # or: coqc -R . Gen ValidateProof.v
     rocqchk -R . Gen -o -silent Gen.ValidateProof  # exit 0 = trusted; -o prints verified context

   Exit code 0 = every requested module re-type-checked in the untainted kernel.
   `-o/--output-context` enumerates exactly what entered the trusted context.
   IMPORTANT: gate on a REAL `.vo` (full proofs), never on a `.vos` interface
   object (proofs elided — fast but NOT kernel-sound).

   CAVEAT (why check 4 is mandatory): `rocqchk` honors the SAME relaxed flags the
   `.vo` was built with. A `.vo` compiled with `-type-in-type` still passes
   `rocqchk`. Disabled kernel checks are not axioms and do NOT appear in
   `Print Assumptions` either — hence the source scan below.
*)

(* --------------------------------------------------------------------------- *)
(* 4. Source scan for the silent escape hatches (NOT caught by 2 or 3).         *)
(* --------------------------------------------------------------------------- *)

(*
   Regex/AST-scan the generated `.v` and REJECT on any of:

     Admitted\.                      (* closes a goal as an axiom *)
     \badmit\b | \bgive_up\b         (* admit tactic / ssreflect give_up: leaves a hole *)
     \b(Axiom|Axioms|Conjecture|Conjectures|Parameter|Parameters
        |Hypothesis|Hypotheses|Variable|Variables)\b     (* postulates *)
     Admit\s+Obligations | Obligation\s+Tactic\s*:=\s*admit
     Unset\s+(Guard|Positivity|Universe)\s+Checking      (* the dangerous silent ones *)
     #\[\s*bypass_check\s*\(         (* per-command kernel-check bypass attribute *)
     -type-in-type | -impredicative-set | -allow-sprop   (* CLI trust-relaxing flags *)

   Rationale: `Print Assumptions` + `rocqchk` catch axioms and type errors, but
   the `Unset ... Checking` / `-type-in-type` family DISABLE kernel checks
   silently — a non-strictly-positive inductive lets you prove `False`, yet no
   axiom is recorded. Only a source scan closes that hole.

   Note on opacity: `Qed` (opaque) vs `Defined` (transparent) is NOT a soundness
   distinction — both are fully kernel-verified. Only axioms and disabled checks
   are trust risks.
*)

(* --------------------------------------------------------------------------- *)
(* 5. Negative reference — what the gate MUST reject (kept commented so this     *)
(*    file still compiles clean; uncommenting must trip the corresponding check).*)
(* --------------------------------------------------------------------------- *)

(*
   (* REJECT (check 1 & 2): `Admitted` declares the goal as an axiom. It would
      show up in `Print Assumptions bad_admitted` as `bad_admitted : ...`. *)
   Theorem bad_admitted : forall n : nat, n = n + 0.
   Proof. Admitted.

   (* REJECT (check 2): an explicit postulate — appears under `Axioms:`. *)
   Axiom bad_axiom : forall P : Prop, P.

   (* REJECT (check 4): silently disabling positivity → `False` becomes provable,
      yet NOTHING appears in `Print Assumptions`. Only the source scan catches it. *)
   Unset Positivity Checking.
   Inductive Bad := mkBad (_ : Bad -> False).
   Set Positivity Checking.
*)

End TheoremataValidateProof.
