(*
  Theoremata — Verified reflection / `decide`-by-computation TEMPLATE (Rocq/Coq)
  =============================================================================

  Rocq counterpart of `components/verify/lean/verified_decide_template.lean`.
  It lets the model emit a *finite certificate table* that the Rocq **kernel**
  checks exhaustively — closed by `vm_compute; reflexivity` (or plain
  `reflexivity`), the kernel-checked reflection idiom.

  Trust note: `vm_compute` (like `native_compute`) is a *conversion strategy*
  the KERNEL itself re-runs when it type-checks the `Qed` — it is NOT an
  external oracle. A proof closed by `vm_compute; reflexivity` reduces to a
  `eq_refl` the kernel verifies, so no extra axiom is incurred and
  `Print Assumptions` reports "Closed under the global context". (This is the
  key difference from Lean's `native_decide`, which trusts compiled code via a
  dedicated axiom — Rocq's `vm_compute` needs no such axiom.)

  The recipe has four moving parts, mirroring the Lean template one-for-one:

    1. an abstract predicate `IsValid` — what "this certificate is correct"
       *means*, written declaratively (a spec a human/blueprint can read);
    2. a **computable** checker `checkValid : ... -> bool` — an executable
       decision procedure the kernel can reduce;
    3. a **soundness bridge** `checkValid_reflect : reflect (IsValid c) (checkValid c)`
       (a `Bool.reflect` two-way bridge) proving the checker computes exactly the
       abstract predicate — this is the load-bearing lemma, proved once by induction;
    4. a `Decidable`-style consumer: given (3), any *concrete* certificate is
       closed by `apply /checkValid_reflect; vm_compute; reflexivity` — or simply
       `by vm_compute` on the boolean equation. The kernel replays the whole check.

  ----------------------------------------------------------------------------
  HOW THE FORMALIZER INSTANTIATES THIS TEMPLATE
  ----------------------------------------------------------------------------
  Replace the toy domain below with the real one, keeping the SAME SHAPE:

    * `IsValid`    -> your abstract correctness predicate on the certificate data
                      (e.g. "these frames cover every incompatible pair", "this
                      assignment satisfies every clause"). Keep it a `Prop` and
                      keep it declarative.
    * `checkValid` -> the executable `bool` decision procedure. It MUST be
                      structurally recursive / computable (no `Classical`, no
                      axioms), so the kernel can evaluate it under `vm_compute`.
    * prove the `..._reflect` bridge by induction (`induction` + `ReflectT`/
                      `ReflectF`, or via `iff_reflect` from a `..._iff` lemma),
                      then reuse the boilerplate verbatim.
    * the model then emits `Definition myCert := ...` and the goal
      `IsValid myCert` is discharged by `apply /checkValid_reflect; vm_compute;
      reflexivity` — a finite, kernel-checked certificate. End the file with
      `Print Assumptions myProof.` and confirm "Closed under the global context"
      (or a whitelisted classical set) appears — never a stray `Axiom`.

  Toolchain note: this is a syntactically Rocq-correct TEMPLATE written against
  the Rocq stdlib only (no MathComp needed for the toy — we hand-roll `reflect`
  so the file is self-contained; swap in `From mathcomp Require Import ssrbool`
  and its `reflect`/`elimT` for the real domain). Treat this as the SHAPE of the
  reflection gate; adjust to your pinned Rocq version.
*)

(* --------------------------------------------------------------------------- *)
(* 0. A self-contained `reflect` (the `Bool.reflect` / ssreflect idiom).        *)
(*    In real code prefer `From mathcomp Require Import ssrbool.` which supplies *)
(*    `reflect`, `elimT`, `introT`, `iffP`, and the `/view` apply syntax.        *)
(* --------------------------------------------------------------------------- *)

Require Import List.
Import ListNotations.
Require Import PeanoNat.       (* Nat.ltb, Nat.ltb_lt *)
Require Import Bool.

Inductive reflect (P : Prop) : bool -> Prop :=
  | ReflectT :   P -> reflect P true
  | ReflectF : ~ P -> reflect P false.

(* Bridge a decidable `iff` into a `reflect` — the standard constructor. *)
Lemma iff_reflect (P : Prop) (b : bool) : (P <-> b = true) -> reflect P b.
Proof.
  intros H. destruct b.
  - apply ReflectT. apply H. reflexivity.
  - apply ReflectF. intro p. discriminate (proj1 H p).
Qed.

(* Eliminate a `reflect` on the `true` side: this is how a concrete certificate
   proof is closed — reduce the boolean to `true`, then read off `P`. *)
Lemma reflect_true (P : Prop) (b : bool) : reflect P b -> b = true -> P.
Proof. intros r Hb; destruct r; [ assumption | discriminate ]. Qed.

Module TheoremataVerifiedDecide.

(* --------------------------------------------------------------------------- *)
(* 1. Abstract predicate (the SPEC).                                            *)
(*                                                                              *)
(* Toy domain: a certificate is a `list nat` ("a table of entries"). It is      *)
(* *valid* w.r.t. a `bound` iff every entry is `< bound` and the entries are    *)
(* strictly increasing. Both halves are declarative recursive `Prop`s — the     *)
(* human-readable meaning, kept deliberately distinct from the checker.         *)
(* --------------------------------------------------------------------------- *)

(* Every entry is strictly below `bound`. *)
Fixpoint AllLt (bound : nat) (t : list nat) : Prop :=
  match t with
  | []        => True
  | a :: rest => a < bound /\ AllLt bound rest
  end.

(* Consecutive entries are strictly increasing. *)
Fixpoint SortedLt (t : list nat) : Prop :=
  match t with
  | []            => True
  | [_]           => True
  | a :: (b :: _) as rest => a < b /\ SortedLt rest
  end.

(* The abstract correctness predicate for a certificate `t` at a given `bound`. *)
Definition IsValid (bound : nat) (t : list nat) : Prop :=
  AllLt bound t /\ SortedLt t.

(* --------------------------------------------------------------------------- *)
(* 2. Computable checker (the DECISION PROCEDURE).                              *)
(*                                                                              *)
(* A `bool`-valued mirror of the spec. Everything here reduces in the kernel:   *)
(* `Nat.ltb a b` on literals evaluates to `true`/`false`, and the recursion is  *)
(* structural. No `Classical`, no axioms — so `vm_compute` can crunch it.       *)
(* --------------------------------------------------------------------------- *)

Fixpoint allLt (bound : nat) (t : list nat) : bool :=
  match t with
  | []        => true
  | a :: rest => Nat.ltb a bound && allLt bound rest
  end.

Fixpoint sortedLt (t : list nat) : bool :=
  match t with
  | []            => true
  | [_]           => true
  | a :: (b :: _) as rest => Nat.ltb a b && sortedLt rest
  end.

Definition checkValid (bound : nat) (t : list nat) : bool :=
  allLt bound t && sortedLt t.

(* --------------------------------------------------------------------------- *)
(* 3. Soundness bridge (the load-bearing lemma).                               *)
(*                                                                              *)
(* Prove the checker decides exactly the abstract predicate. Done once, by      *)
(* structural induction; each concrete certificate then rides on this. We prove *)
(* the `..._iff` form and package it as a `reflect` via `iff_reflect`.          *)
(* --------------------------------------------------------------------------- *)

Lemma allLt_iff (bound : nat) (t : list nat) :
  AllLt bound t <-> allLt bound t = true.
Proof.
  induction t as [| a rest IH]; simpl.
  - split; [ reflexivity | intros _; exact I ].
  - rewrite Bool.andb_true_iff, <- Nat.ltb_lt, IH. reflexivity.
Qed.

Lemma sortedLt_iff (t : list nat) :
  SortedLt t <-> sortedLt t = true.
Proof.
  induction t as [| a rest IH].
  - simpl; split; [ reflexivity | intros _; exact I ].
  - destruct rest as [| b rest'].
    + simpl; split; [ reflexivity | intros _; exact I ].
    + change (SortedLt (a :: b :: rest'))
        with (a < b /\ SortedLt (b :: rest')).
      change (sortedLt (a :: b :: rest'))
        with (Nat.ltb a b && sortedLt (b :: rest')).
      rewrite Bool.andb_true_iff, <- Nat.ltb_lt, IH. reflexivity.
Qed.

(* **Soundness**: the computable checker is `true` exactly when the abstract
   predicate holds. This is the bridge that makes `vm_compute` trustworthy. *)
Lemma checkValid_iff (bound : nat) (t : list nat) :
  IsValid bound t <-> checkValid bound t = true.
Proof.
  unfold IsValid, checkValid.
  rewrite Bool.andb_true_iff, <- allLt_iff, <- sortedLt_iff. reflexivity.
Qed.

(* The `reflect` packaging — the ssreflect-style two-way view. In MathComp this
   `reflect (IsValid bound t) (checkValid bound t)` is what `apply/` consumes. *)
Lemma checkValid_reflect (bound : nat) (t : list nat) :
  reflect (IsValid bound t) (checkValid bound t).
Proof. apply iff_reflect. apply checkValid_iff. Qed.

(* --------------------------------------------------------------------------- *)
(* 4. Worked toy instance.                                                      *)
(*                                                                              *)
(* Everything below is what the *model emits* per problem: a finite certificate *)
(* table and a one-line reflection proof. The kernel checks it exhaustively.    *)
(* --------------------------------------------------------------------------- *)

(* A concrete valid certificate: entries `[1; 3; 7]` are all `< 10` and strictly
   increasing. *)
Definition toyCert : list nat := [1; 3; 7].

(* The finite certificate check, closed by kernel reflection. Two equivalent
   idioms; both reduce `checkValid` in the kernel and transport along the bridge.
   NOTE: `vm_compute` here is kernel-trusted, not an oracle (see header). *)
Theorem toyValid : IsValid 10 toyCert.
Proof.
  apply (reflect_true _ _ (checkValid_reflect 10 toyCert)).
  vm_compute. reflexivity.
Qed.

(* Equivalent one-liner via the `iff` bridge, closed by plain computation: *)
Theorem toyValid' : IsValid 10 toyCert.
Proof. apply checkValid_iff. reflexivity. Qed.

(* A negative sanity check: `[3; 1]` is not strictly increasing, so it is *not*
   valid — also decided by the kernel. *)
Theorem toyInvalid : ~ IsValid 10 [3; 1].
Proof.
  intro H. apply checkValid_iff in H. vm_compute in H. discriminate H.
Qed.

(* Soundness confirmation the formalizer should run on the real proof; for a
   pure reflection proof this reports "Closed under the global context"
   (no axioms), never an `Admitted`-lemma axiom nor a disabled-check escape:
     Print Assumptions toyValid.
   Independent kernel re-check (separate trusted binary):
     rocqchk -R . Gen -o -silent Gen.<ThisFile>            (* exit 0 = trusted *)
*)

End TheoremataVerifiedDecide.
