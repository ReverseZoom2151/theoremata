/-
CONTROL separating this check from the layer-2 axiom audit.

The statement is made of real mathematics; only the PROOF is `sorry`. The axiom
audit rightly reports `sorryAx` here. This check must report no opaque constant,
because nothing is wrong with the statement. `#print axioms` alone cannot tell
this file apart from `positive_admitted.lean`: both report `[sorryAx]`.
-/
namespace TheoremataHonestSorryFixture

def double (n : Nat) : Nat := n + n

theorem double_eq_two_mul (n : Nat) : double n = 2 * n := by
  sorry

end TheoremataHonestSorryFixture
