/-
POSITIVE fixture for the laundering case at the admitted/abstract boundary.

`opaque` normally means "sealed implementation", which is legitimate abstraction
and must not be flagged. But `opaque ... := sorry` is not abstraction; the author
wrote `sorry` and the keyword only hides the unfolding. The discriminator is
`sorryAx` in the constant's own closure, not the declaration keyword, so this
file is flagged while `abstract_control.lean` is not.
-/
namespace TheoremataSealedFixture

/-- Legitimate sealing: a real value behind an opaque wall. Must not be flagged. -/
opaque realSeal : Nat := 7

/-- Laundered admission. Must be flagged. -/
opaque admittedSeal : Nat := sorry

theorem seals_agree (h : realSeal = admittedSeal) : realSeal = admittedSeal := h

end TheoremataSealedFixture
