/-
Fixture, half one of the import-drift tamper pair. Compiled to `DriftBase.olean`, then
`DriftClient.lean` is compiled against it, then `DriftBase_v2.lean` is compiled OVER the
same `DriftBase.olean` and the untouched `DriftClient.olean` is replayed.

This models the swapped-dependency attack: hand the checker a prebuilt `.olean` whose
proof was typechecked against a different version of a module it imports. The client file
itself is honest and its source never changes.
-/
def Theoremata.Fixture.baseValue : Nat := 0
