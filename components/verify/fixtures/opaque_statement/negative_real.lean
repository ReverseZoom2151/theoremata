/-
NEGATIVE control for `statement_rests_on_opaque_constant`.

The same theorem shape as `positive_admitted.lean`, but every constant in the
statement has a real definition. Nothing here may be flagged.
-/
namespace TheoremataRealFixture

/-- A real definition, not a placeholder. -/
def windingNumber (f : Nat → Nat) (z : Nat) : Nat := f z - f 0

/-- A real definition, not a placeholder. -/
def residue (f : Nat → Nat) (z : Nat) : Nat := f (z + 1)

/-- A real relation, not a placeholder. -/
def HasContourValue (f : Nat → Nat) (v : Nat) : Prop := f 0 = v

theorem residueTheorem (f : Nat → Nat) (z : Nat)
    (h : HasContourValue f (windingNumber f z * residue f z)) :
    HasContourValue f (windingNumber f z * residue f z) := h

end TheoremataRealFixture
