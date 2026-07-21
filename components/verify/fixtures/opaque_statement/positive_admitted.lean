/-
POSITIVE fixture for `statement_rests_on_opaque_constant`.

This reproduces the gate-passing combination: every load-bearing constant in the
theorem's statement is DEFINED with a `sorry` body, while the theorem's own proof
is complete. A reader who greps the proof for `sorry` finds nothing, and a
mutation-style triviality check cannot move the statement because swapping one
opaque constant for another opaque constant changes nothing.

Mathlib-free on purpose so it compiles against a bare Lean toolchain.
-/
namespace TheoremataOpaqueFixture

/-- Admitted placeholder: named for a winding number, backed by nothing. -/
noncomputable def windingNumber (f : Nat → Nat) (z : Nat) : Nat := sorry

/-- Admitted placeholder: named for a residue, backed by nothing. -/
noncomputable def residue (f : Nat → Nat) (z : Nat) : Nat := sorry

/-- Admitted placeholder: the relation the summit theorem is stated in. -/
def HasContourValue (f : Nat → Nat) (v : Nat) : Prop := sorry

/-- Named for a published theorem. The proof is complete: it is the identity on
the hypothesis. Every substantive symbol in the statement is an admitted
constant, so the statement asserts nothing. -/
theorem residueTheorem (f : Nat → Nat) (z : Nat)
    (h : HasContourValue f (windingNumber f z * residue f z)) :
    HasContourValue f (windingNumber f z * residue f z) := h

end TheoremataOpaqueFixture
