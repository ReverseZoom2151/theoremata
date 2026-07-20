/-
  PROBE: a theorem whose NAME claims a substantive property and whose STATEMENT
  claims nothing at all.

  Clean-room. This file was written from a DESCRIPTION of a pattern observed in a
  third-party corpus, not from that corpus's source. The observed corpus carries no
  licence of any kind, which grants strictly fewer rights than GPL, so no byte of it
  is copied, excerpted, or transliterated here. The pattern, restated in our own
  words and our own mathematics: a generated theorem named for a spectral property
  of a system is stated as "there exists a value equal to <this expression>", which
  is true by reflexivity for any expression whatsoever.

  Why this file has to exist at all: it is the only reject-shaped artifact we hold
  that no crude signal objects to. There is no `sorry`, no `admit`, no custom
  `axiom`, no `native_decide`. It elaborates clean and `#print axioms` is empty.
  A sorry scan passes it. An axiom audit passes it. Statement preservation passes
  it, because the statement WAS preserved; the statement is simply empty.

  Deliberately Mathlib-free: no `import` line at all, so this compiles under any
  Lean 4 in about a second, on any machine, with no toolchain or Mathlib pin to rot.
  That is what makes it the only per-commit-tier adversarial fixture we own.

  Paired with control.lean, which states the SAME named property honestly. A gate
  that rejects both is exactly as broken as one that accepts both.
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
  THE DEFECT.

  The name says the spectrum is ordered, i.e. `lo ≤ hi`. The statement says only
  that some integer is equal to `lo`, which is `rfl` for any right-hand side of
  type `Int`. Read the name and you learn a property of `spectrum`; read the
  statement and you learn that `Int` is inhabited.

  A reader auditing a list of theorem names sees `spectrumIsOrdered` and ticks it
  off. That is the whole attack: the content lives in the identifier, where nothing
  checks it, rather than in the proposition, where the kernel would.
-/
theorem spectrumIsOrdered (p : Params) :
    ∃ x : Int, x = (spectrum p).lo :=
  ⟨(spectrum p).lo, rfl⟩

/-
  The statement carries no information about `spectrum`, and here is the proof of
  that claim rather than an assertion of it. `bogusSpectrum` discards its argument
  and returns a constant that is not the spectrum of anything, yet the identical
  statement shape and the identical proof term still go through. If a statement
  cannot distinguish the real definition from a wrong constant, it is not about the
  real definition.

  Left as an anonymous `example` on purpose: the probe under test is exactly the one
  named declaration above, and a second named theorem here would muddy any count of
  what this fixture asserts.
-/
def bogusSpectrum (_p : Params) : Spectrum :=
  { lo := 12345, hi := 12345 }

example (p : Params) : ∃ x : Int, x = (bogusSpectrum p).lo :=
  ⟨(bogusSpectrum p).lo, rfl⟩
