/-
Fixture: an honest, self-contained module. Replaying it must succeed.

Deliberately imports nothing, so the replay's reconstructed base environment is the empty
environment and the whole result depends only on these two declarations. That keeps the
positive case fast and keeps it from passing for an accidental reason.
-/
def Theoremata.Fixture.answer : Nat := 42

theorem Theoremata.Fixture.answer_eq : Theoremata.Fixture.answer = 42 := rfl
