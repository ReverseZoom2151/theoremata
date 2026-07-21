/-
Fixture, half two of the import-drift tamper pair. See `DriftBase_v1.lean`.

Same module name, same declaration name, same type, different value. `DriftClient`'s
`rfl` proof was checked against the `0` here and is false against the `1`.
-/
def Theoremata.Fixture.baseValue : Nat := 1
