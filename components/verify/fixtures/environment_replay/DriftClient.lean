/-
Fixture: the honest client of the drift pair. Its source is never modified. Compiled
against `DriftBase_v1`, it is correct; replayed after `DriftBase.olean` has been rebuilt
from `DriftBase_v2`, its stored proof no longer typechecks and the replay must say so.
-/
import DriftBase

theorem Theoremata.Fixture.baseValue_is_zero : Theoremata.Fixture.baseValue = 0 := rfl
