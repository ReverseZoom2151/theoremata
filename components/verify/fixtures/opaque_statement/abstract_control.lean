/-
ABSTRACT control for `statement_rests_on_opaque_constant`, the false-positive guard.

Two shapes of legitimate abstraction that must never be accused:

1. `assoc_four` is a normal algebra lemma. Its statement quantifies over a type
   variable, a typeclass parameter and four section variables. Those are binders,
   not constants, so they are outside the check by construction.

2. `chosenPoint_self` is stated over a constant the author declared as an
   `axiom` on purpose. A declared assumption is visible, intentional, and owned
   by the layer-2 axiom audit. It is not an admitted placeholder and this check
   does not flag it.
-/
namespace TheoremataAbstractFixture

class Semigroupish (G : Type) where
  op : G → G → G
  assoc : ∀ a b c : G, op (op a b) c = op a (op b c)

open Semigroupish

variable {G : Type} [Semigroupish G]

theorem assoc_four (a b c d : G) :
    op (op (op a b) c) d = op a (op b (op c d)) := by
  rw [assoc, assoc]

/-- A deliberately declared assumption, not an admitted placeholder. -/
axiom chosenPoint : Nat

theorem chosenPoint_self : chosenPoint = chosenPoint := rfl

end TheoremataAbstractFixture
