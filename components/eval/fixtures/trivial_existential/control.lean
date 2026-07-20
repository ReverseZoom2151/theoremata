/-
  CONTROL: the same theorem name, over the same definitions, stating the property
  the name actually claims.

  This half matters as much as probe.lean. The defect the probe exhibits is a
  relation between a name and a statement, so a gate could "catch" it by flagging
  anything whose statement is an existential, or anything short, or anything proved
  by an anonymous constructor. Every one of those heuristics fires on honest
  mathematics too. The control is what makes a rejection here readable as a false
  positive instead of a success, which is why it ships in the same directory and is
  registered in the same pair.

  The definitions below are duplicated from probe.lean rather than imported. That is
  intentional: each file must elaborate standalone, with no `import`, no Mathlib and
  no build directory, so the harness can hand a single path to `lean` and get an
  answer. Sharing a module would make the pair depend on LEAN_PATH setup and would
  cost us the property that these are the only fixtures runnable on every commit.
-/

/-- A system's parameters. `radius` is intended to be non-negative. -/
structure Params where
  center : Int
  radius : Int

/-- The two endpoints of a system's spectrum. -/
structure Spectrum where
  lo : Int
  hi : Int

/-- The spectrum of a system, computed from its parameters. -/
def spectrum (p : Params) : Spectrum :=
  { lo := p.center - p.radius, hi := p.center + p.radius }

/--
  The honest statement of the property the name claims: the spectrum's lower
  endpoint really is below its upper endpoint, given the non-negativity of `radius`
  that the parameter's docstring intends.

  Note what the honest version needs and the probe does not: a hypothesis. The
  property is genuinely conditional, so the statement has to carry `0 ≤ p.radius`,
  and the proof has to consume it. A statement with real content constrains its
  inputs; a statement with no content accepts anything, which is the difference the
  pair exists to exhibit.
-/
theorem spectrumIsOrdered (p : Params) (hr : 0 ≤ p.radius) :
    (spectrum p).lo ≤ (spectrum p).hi := by
  -- `spectrum` is a plain structure instance, so its projections reduce
  -- definitionally and `show` can restate the goal without any simp set.
  show p.center - p.radius ≤ p.center + p.radius
  omega
