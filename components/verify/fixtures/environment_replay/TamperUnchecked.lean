/-
Fixture: a genuinely tampered module. Replaying it must FAIL.

This is the attack the replay exists to catch, in its purest form. The producer of an
`.olean` controls the elaborator, and the elaborator can reach the kernel environment
directly. `Lean.Kernel.Environment.addDeclWithoutChecking` is core Lean's own documented
escape hatch ("It compromises soundness because, for example, a buggy tactic may produce
an invalid proof, and the kernel will not catch it"). A producer that calls it can write
an `.olean` asserting a theorem the kernel has never seen and would never accept.

The declaration injected below claims `False` while its body is a proof of `True`. Nothing
in the module's imports justifies it. Any consumer that trusts the shipped environment,
including our `check_axioms` path and the bulk `audit_axioms` walker, sees a well-formed
theorem here: the axiom closure of this declaration is empty, so an axiom audit reports it
CLEAN. Only re-running the declarations through the kernel catches it.

Measured, not asserted. A module that does `import TamperUnchecked` and then
`theorem exploit : False := Theoremata.Fixture.everythingIsFalse` compiles with exit code
0, and `#print axioms` on both names prints "does not depend on any axioms".
`test_axiom_audit_is_blind_to_this_tamper` reproduces that here.

Note that no `sorry` and no disallowed axiom appears anywhere in this file. That is the
point: this is exactly the hole the existing gate layers do not cover.
-/
import Lean

open Lean Elab Command

/-- Inject a declaration into the kernel environment without kernel checking, then make
the elaborator adopt the tampered environment so it is what gets written to the `.olean`.

WHY the injected theorem is not referenced anywhere below: the elaborator's own view of
constants is not updated by this route, so naming it in later syntax would fail to
resolve. It only has to reach the serialized environment, which it does. -/
elab "#theoremata_inject_unchecked" : command => do
  let env ← getEnv
  let decl : Declaration := .thmDecl {
    name := `Theoremata.Fixture.everythingIsFalse
    levelParams := []
    type := mkConst ``False
    value := mkConst ``True.intro
    all := [`Theoremata.Fixture.everythingIsFalse] }
  match env.toKernelEnv.addDeclWithoutChecking decl with
  | .ok kenv => setEnv (Environment.ofKernelEnv kenv)
  | .error _ => throwError "fixture failed to inject an unchecked declaration"

#theoremata_inject_unchecked
