# TauCetiRoadmap: mining report

Target: `resources/TauCetiRoadmap-main/TauCetiRoadmap-main/`
Upstream: `github.com/TauCetiProject/TauCetiRoadmap` (the human-owned roadmap repo for
Tau Ceti, an "AIs-welcome" Lean 4 library downstream of Mathlib, incubated by the Lean FRO
and the Mathlib Initiative).
Corpus: 75 files. 45 `.md`, 14 `.lean`, 9 `.yml`, 1 `.toml`, 1 `.py`, 1 `.json`,
1 `lean-toolchain`, 1 `LICENSE`, 1 `CODEOWNERS`. 9308 lines total across md/lean/yml/py.

Everything under `resources/` was read as untrusted data only. Nothing was executed from
the corpus. See the POSSIBLE INJECTION section at the end.

---

## 1. What it is

It is a blueprint repo, but not a leanblueprint repo. It is the **specification half** of a
three-repo split:

- `TauCetiRoadmap` (this repo): human-owned prose specifications plus suggested Lean
  signatures. No proofs.
- `TauCeti`: the AI-authored mathematics.
- `TauCetiReview`: review machinery.

Root `README.md:1-9` states this directly. The unit of work is a **roadmap area**: one
directory under `TauCetiRoadmap/`, containing a `README.md` (declared "the definitive
specification of its area") and usually a `Suggested.lean` (declared explicitly non-normative
and non-exhaustive). 13 active areas plus 1 archived under `Completed/`.

The mathematics covered is broad research-level Mathlib-adjacent material: universal covers,
the Jacobian conjecture, reductive algebraic groups, PDE, Heegaard Floer (two roadmaps,
combinatorial and analytic), multiquadratic fields and genus theory, geometric topology and
the Kirby problem list, one-parameter semigroups and BCR Bochner, exchangeability and
de Finetti, conformal mapping and the Riemann mapping theorem, weighted orthogonal L2 bases,
contour integration and the Hungerbuhler-Wasem generalized residue theorem, and (completed)
effective arithmetic bounds.

### Is the prose-to-formal link mechanical, or only adjacent?

**Adjacent with a weak, unenforced convention. It is not mechanical.** Evidence, four
measurements:

1. **Directory pairing is mechanical and CI-adjacent.** `TauCetiRoadmap/AREA/README.md`
   plus `TauCetiRoadmap/AREA/Suggested.lean`, glob-built by `lakefile.toml:12-14`
   (`globs = ["TauCetiRoadmap.*"]`, with the stated intent that "an orphan or broken target
   is caught"), and rooted at `TauCetiRoadmap.lean:6-17` which imports all 12 non-empty
   `Suggested` modules. `.github/scripts/check_roadmap_areas.py` enforces that the directory
   set matches the README list and two issue-template dropdowns, with `--fix`
   (`sync-roadmap-dropdowns.yml` runs it after every merge). So the **area** level is
   machine-checked.

2. **Declaration-name overlap is partial.** Of 54 named declarations across the Lean files,
   30 appear verbatim in the sibling `README.md`. But this is concentrated: only 3 of 13
   areas name declarations at all (`ContourIntegration` 9/18, `Exchangeability` 8/9,
   `OrthogonalL2Bases` 13/27). The other 10 areas have zero name overlap, mostly because
   they state milestones as anonymous `example`s.

3. **Layer labels are the actual link, and they are prose.** Across the 14 Lean files the
   milestone-to-roadmap attribution is carried either by a bold docstring prefix or by an
   enclosing module-doc section header:
   - docstring prefix: `ConformalMapping/Suggested.lean:39` `**L0 - Hurwitz.**`, and L0
     through L4 at lines 39, 53, 63, 71, 81, 90; `Completed/EffectiveBounds/Suggested.lean:33`
     `**Layer 1, discriminant from an integral basis.**`; `Exchangeability/Suggested.lean`
     labels all 25 docstrings this way.
   - section header: `ContourIntegration/Suggested.lean:158,200,242,257`
     (`/-! ## Layer 1:`, `## Layer 2:`, `## Layer 3:`, `## Layer 4:`);
     `OrthogonalL2Bases/Suggested.lean:38,79,97,118,205,229` (`## Part 0`, `## Part A1`,
     `## Part B1`, `## Part B2`, `## Part A3`, `## Part B3`).
   38 of 80 docstrings carry a layer label as a bold prefix; the remainder are covered by
   section headers. There is **no unique milestone identifier**, no label-to-label reference,
   and **no CI check** that any Lean declaration corresponds to any roadmap item. `ci.yml`
   is `lean-action` with `test: false, lint: false` and, per its own comment, no axiom audit
   and no warning-as-error.

4. **Cross-area links are markdown hyperlinks only.** 12 occurrences of
   `../AREA/README.md`, e.g. `CombinatorialHeegaardFloer/README.md:15,63,80,227,299`.
   One of them, `ContourIntegration/README.md:22`, points at `../ModularForms/README.md`,
   **a directory that does not exist in this repo**. That is a dangling cross-roadmap edge
   and it is exactly the class of error a real dependency graph would catch.

Conclusion for our purposes: the prose names results and the Lean states some of them, but
the correspondence is human-maintained and lossy. It is weaker than leanblueprint's
`\label` plus `\uses`.

---

## 2. The dependency structure

**Headline, and it is a negative result: there is no machine-readable dependency encoding
here that we do not already ingest. Our leanblueprint path is strictly richer.**

What they actually have:

- **A per-roadmap total order over layers.** Every roadmap is organised as
  `### Layer 0`, `### Layer 1`, ... (105 `Layer N` mentions across the md corpus;
  144 `###` and 231 `##` headings), and 5 of 13 roadmaps close with an explicit
  `## Ordering` prose section (e.g. `Exchangeability/README.md:705-715`, which reads as a
  paragraph: Layer 0 first, then Layer 1, then Layer 2, and so on). This is a chain, not
  a DAG. Individual milestones inside a layer carry no per-item edges.

- **Prose dependency vocabulary, unstructured.** Across the md corpus: "depends on" 18,
  "consumes" 22, "prerequisite" 19, "Depends on" 3. All in running sentences, e.g.
  `ConformalMapping/Suggested.lean:42` "Built on the argument principle / residue theory
  from the sibling `ContourIntegration` roadmap, PR #35". Not parseable without a model.

- **Two genuinely structured bold-field conventions.** These are the only machine-readable
  encodings in the repo, and they are field-per-line, not edge-per-item:

  **(a) `**Unlocks.**`**, 11 occurrences, all in `GeometricTopology/README.md`
  (lines 287, 354, 407, 516, 569, 624, 685, 735, 777, 819, 890). A **forward** edge from a
  layer to the open problems that layer would enable. Real example, `GeometricTopology/README.md:516-518`:

  ```
  **Unlocks.** `0`-shake genus vs slice genus, `[Kir97, Problem 1.41]` (Piccirillo, via the
  Conway knot, jointly with the combinatorial Heegaard Floer roadmap's `τ`);
  Alexander-polynomial-one knots are topologically slice, `[Kir97, Problem 1.36]` (Freedman).
  ```

  Note the citation key shape `[Kir97, Problem 1.41]`, which is a stable external identifier
  into the Kirby problem list.

  **(b) The reference-extract header block**, 27 files under
  `TauCetiRoadmap/GeometricTopology/references/`. Every file opens with the same three bold
  fields. Real example, `references/hirsch-collar.md:2-4`:

  ```
  **Source.** M. Hirsch, Differential Topology, Springer GTM 33, 1976.
  **Locus.** Theorem 6.1, Chapter 4 §6 ("Collars and Tubular Neighborhoods of Neat
  Submanifolds"), printed p. 112 (statement) and p. 114 (proof); physical djvu pp. 62-63.
  **Supports.** GeometricTopology roadmap, layer 1 (existence of a collar on the boundary
  ∂M, foundational for gluing/cobordism constructions).
  ```

  followed by `## Summary`, `## Key statements (quoted)` with the theorem quoted verbatim
  and attributed, and `## Notes` which records **locator corrections** against the roadmap,
  e.g. `references/hirsch-disc-theorem.md` "corrects the roadmap's earlier §8.3 citation"
  and `references/lickorish-braids-markov.md` "corrects the roadmap's Ch 10-11".

  `**Supports.**` parses cleanly: 27/27 files match
  `^\*\*Supports\.\*\* (area) roadmap,? layers? (layers)`. This is a
  real back-edge from a literature node to a roadmap layer.

**Verdict.** Reject "ingest their dependency format" as a headline adopt. Our
`Blueprint::from_tex` (`components/reason/proving/blueprint.rs:152`) already parses
`\label`, `\lean`, `\uses` split by statement versus proof, and `\leanok`, and
`Blueprint::import` (blueprint.rs:269-326) materialises real `EdgeKind::DependsOn` edges
with `DepScope::Statement | Proof | Both`, with `blueprint_run.rs:319` doing a real
topological sort with cycle detection. TauCeti's layer chain is a degenerate case of that.

What is genuinely new to us is **(b)**: a citation-grounding node kind. We have nothing like
it (see section 6, adopt A1).

---

## 3. The Lean: sorry/axiom census and REAL compile output

### Census

14 `.lean` files. Stripping all block comments and docstrings first (so doc prose that
merely discusses `sorry` is not counted):

| metric | count |
|---|---|
| `example` (anonymous milestone statements) | 30 |
| `theorem` / `lemma` (named) | 26 |
| `def` / `abbrev` / `structure` / `class` / `instance` | 26 |
| real `sorry` occurrences in code | 67 |
| `admit` | 0 |
| `native_decide` | 0 |
| `decide` | 0 |
| custom `axiom` declarations | 0 |
| `@[implemented_by]` / `unsafe` | 0 |

Per-file (raw grep including doc text, for cross-checking):
`ContourIntegration` 19, `OrthogonalL2Bases` 23, `Exchangeability` 16, `ConformalMapping` 7,
`Multiquadratic` 7, `EffectiveBounds` 4, `ReductiveGroups` 4, `GeometricTopology` 2,
`UniversalCovers` 2, and 1 each in `CombinatorialHeegaardFloer`, `HeegaardFloer`,
`JacobianChallenge`, `PDE`, `TauCetiRoadmap.lean`.

Five of the 13 areas (`CombinatorialHeegaardFloer`, `GeometricTopology`, `HeegaardFloer`,
`JacobianChallenge`, `PDE`, `UniversalCovers`) have **zero declarations**: their
`Suggested.lean` is a docstring plus an empty namespace, e.g.
`UniversalCovers/Suggested.lean:25-29`.

The `sorry` usage is **declared and intentional**, not concealed. `lakefile.toml:8-10`:
"`sorry` is allowed here (these are goals, not proofs), so this library builds without
`warningAsError`". `ci.yml:18-19` says the same and explicitly declines an axiom audit.
Zero custom axioms is a clean result.

### REAL compile output

`lean-toolchain` pins `leanprover/lean4:v4.31.0-rc1`. Our installed toolchains are
`v4.21.0, v4.25.0, v4.31.0, v4.31.tmp, v4.32.0, v4.32.0-rc1`. The pinned `v4.31.0-rc1` is
**not** among them, so `elan` attempted to fetch it. Actual output:

```
$ lean --version
info: downloading https://releases.lean-lang.org/lean4/v4.31.0-rc1/lean-4.31.0-rc1-windows.tar.zst
info: installing C:\Users\adria\.elan\toolchains\leanprover--lean4---v4.31.0-rc1
error: failed to extract package
info: caused by: failed to unpack `lean-4.31.0-rc1-windows/lib/lean/Lean/LibrarySuggestions/SymbolFrequency.olean.private`
info: caused by: There is not enough space on the disk. (os error 112)
```

```
$ df -h /c
Filesystem      Size  Used Avail Use% Mounted on
C:              951G  951G  520M 100% /c
```

The disk is full (520M free), so the pinned toolchain cannot be installed. A partial
`v4.31.tmp` toolchain is now listed by `elan`, an artifact of that failed extraction.

Forcing our own 4.32.0 instead gets past the toolchain and fails on the real blocker:

```
$ lean +leanprover/lean4:v4.32.0 TauCetiRoadmap/ContourIntegration/Suggested.lean
TauCetiRoadmap/ContourIntegration/Suggested.lean:1:0: error: unknown module prefix 'Mathlib'

No directory 'Mathlib' or file 'Mathlib.olean' in the search path entries:
c:\Users\adria\.elan\toolchains\leanprover--lean4---v4.32.0\lib\lean
```

Same for `UniversalCovers/Suggested.lean`. There is no `.lake/` directory in the corpus, so
no vendored dependencies.

**Plain statement of what this means.** Every one of the 12 non-empty files begins with
`import Mathlib` (the whole library). `lake-manifest.json` pins mathlib at rev
`9caeba1000ef8f302920981f4a08651d325abc81` with `inputRev: "master"`, plus batteries, aesop,
Qq, Cli, importGraph, plausible, LeanSearchClient at `v4.31.0-rc1` tags and ProofWidgets4 at
`v0.0.100`. **We do not have that Mathlib revision and cannot build it here**: it requires
the 4.31.0-rc1 toolchain plus a full `lake exe cache get` Mathlib, on a disk with 520M free.
I did not verify elaboration of any statement in this corpus. Everything in section 4 below
is therefore a **syntactic** verdict, and is labelled as such.

Compatibility note for a later wave: 4.31.0-rc1 to 4.32.0 is a one-minor-version gap, and
`inputRev: "master"` means the pin is a moving target that upstream will have advanced.
Reproducing their build means the manifest rev, not `master`.

---

## 4. Triviality check

**Verdict: the corpus is clean of `name_claims_more_than_statement` in its usual form, and
it is clean for a structural reason worth stealing. But it contains a different and
arguably worse laundering pattern, in one file, which violates the repo's own written rule.**

### 4a. Clean of the usual shapes

Scanned all 14 files for: `True` conclusions, bare `rfl` / `trivial` / anonymous-constructor
proofs, and unconstrained existentials.

- Zero occurrences of `True` anywhere in the corpus.
- Zero proofs that are not `sorry`. Every statement terminates in `sorry` or `by sorry`.
  There is no `⟨_, _⟩`, no `rfl`, no `trivial`, no `by decide`, no `by norm_num`. So the
  `exists r : Real, r = (bigExpr).field` closed by an anonymous constructor shape is
  structurally impossible here: nothing is closed at all.
- The existentials that appear are constrained and substantive. The two strongest:
  `ConformalMapping/Suggested.lean:73-77` (Riemann mapping theorem) asks for
  `∃ f, DifferentiableOn ℂ f Ω ∧ Set.InjOn f Ω ∧ f '' Ω = Metric.ball 0 1`, three conjuncts
  including a surjectivity-onto-the-disc equality, from hypotheses `IsOpen`, `IsConnected`,
  `SimplyConnectedSpace`, `Ω ≠ Set.univ`. `ConformalMapping/Suggested.lean:95-101` (Schwarz
  reflection) asks for `∃ F` with holomorphy, an `EqOn` agreement, and the reflection
  symmetry. Neither is satisfiable by a junk witness.
- Statements with real mathematical content and non-vacuous hypotheses, spot-checked:
  `Multiquadratic/Suggested.lean:79-86` (prime splitting iff every `d i` is a QR mod `p`,
  a genuine iff), `Completed/EffectiveBounds/Suggested.lean:47-50` (class number bound
  `h_F ≤ |d_F| · 4^[F:ℚ]`), `ContourIntegration/Suggested.lean:221-227` (argument principle,
  an explicit contour-integral identity against `logDeriv` and `meromorphicOrderAt`).

### 4b. The structural reason: 30 of 56 milestone statements are anonymous `example`s

This is the single cleanest defensive technique in the repo. `ConformalMapping`,
`Multiquadratic`, `EffectiveBounds`, and `Exchangeability` state their milestone theorems as
bare `example ... := sorry`, carrying the human-readable claim only in the docstring
(`ConformalMapping/Suggested.lean:39` `**L0 - Hurwitz.**`, then an unnamed `example` at
line 44). An anonymous declaration **cannot** overclaim by its name, because it has no name.
The name is only minted when the statement is proved and lands in `TauCeti/`.

### 4c. The real defect: opaque `sorry`-bodied constants used as hypotheses and as content

`ContourIntegration/Suggested.lean` defines five load-bearing objects with a `sorry` body:

```
:85  noncomputable def windingNumber (γ : ℝ → ℂ) (a b : ℝ) (z₀ : ℂ) : ℂ := sorry
:90  noncomputable def residue (f : ℂ → ℂ) (z₀ : ℂ) : ℂ := sorry
:97  def HasCauchyPV (γ : ℝ → ℂ) (a b : ℝ) (f : ℂ → ℂ) (v : ℂ) : Prop := sorry
:151 def ConditionAprime (γ : ℝ → ℂ) (a b : ℝ) (f : ℂ → ℂ) (S : Finset ℂ) : Prop := sorry
:156 def ConditionB (γ : ℝ → ℂ) (a b : ℝ) (f : ℂ → ℂ) : Prop := sorry
```

Consequences, all syntactic and checkable without elaboration:

- `IsNullHomologous` (line 143) is defined as
  `∀ w ∉ Ω, windingNumber γ a b w = 0`, i.e. in terms of an opaque constant. It therefore
  asserts nothing about winding numbers.
- The summit theorem `hungerbuhlerWasem_residueTheorem` (line 277) has hypotheses
  `hnull : IsNullHomologous ...`, `hA : ConditionAprime ...`, `hB : ConditionB ...` and a
  conclusion `HasCauchyPV γ a b f (2πi * ∑ s ∈ S, windingNumber γ a b s * residue f s)`.
  **Every mathematically substantive symbol in that statement is a `sorryAx`.** The
  declaration is named for Hungerbuhler-Wasem Theorem 3.3 but its statement is a relation
  among five opaque constants. This is `name_claims_more_than_statement` by a different
  mechanism: not a trivially-true statement, but a **contentless** one behind a
  substance-claiming name.
- `hasCauchyPV_half_residue` (line 300) is the same, plus
  `hwind : windingNumber γ a b s = 1 / 2`, a constraint on an opaque constant.

Crucially, **the repo's own root `README.md` forbids exactly this**, in the "Write Lean code"
bullet (README.md, "Write Lean code" section): it says a condition you cannot yet state is
still a `sorry`, never a `Prop`-typed field or a `def _ : Prop := sorry`, because both assert
nothing (a `Prop` field is satisfiable by `True`, a `sorry` body is `sorryAx Prop`), and it
instructs the author to omit a condition they cannot state rather than name an empty one.
`ContourIntegration/Suggested.lean:97,151,156` are three direct violations of that rule by
the very repo that wrote it.

I am **not** calling this dishonest: the file is transparently a goal-statement file, and the
docstrings at lines 92-96 and 146-156 describe in prose exactly what the missing content is.
But it is the concrete proof that a written prohibition does not survive contact with
authoring, which is our whole argument for mechanical gates. And unlike the `Prop`-field case
the README anticipated, `windingNumber : ℂ := sorry` and `residue : ℂ := sorry` are
**data**-valued opaque constants, which the README does not even mention.

Also worth recording as a near-miss: `Exchangeability/Suggested.lean:88-92` defines a
`ConditionallyIID`-shaped predicate as a genuine explicit formula (a `∃ ν, Measurable ν ∧ ∀ m k, ...`
mixture identity), not as `sorry`. That file is the good example; `ContourIntegration` is the
bad one. Same repo, same review process.

---

## 5. The stated roadmap itself: mechanisms worth stealing

Ignoring aspiration, the described mechanisms are:

1. **The three-repo separation of specification, mathematics, and review**, with the prose
   README declared definitive and the Lean declared advisory. Root `README.md:1-9` and
   repeated in every `Suggested.lean` header ("This file is not the roadmap and is not
   exhaustive").
2. **Anonymous `example` milestones** (section 4b). Cheap, and it directly blocks our
   known adversarial failure mode.
3. **Fair-use reference extracts with exact locators** (section 2b). The stated purpose,
   `references/README.md:6-8`, is so that an agent working a layer can see the precise
   statement it is formalising without re-deriving it from memory. That is an
   anti-hallucination substrate, and the `## Notes` sections show it working: three roadmap
   citations were found wrong and corrected by the extraction pass.
4. **Cooperative expiring claims.** `README.md` "Coordinating work" plus
   `.github/workflows/intentions.yml` (30d default TTL, 90d max, released automatically,
   `claim` / `disclaim` / `claim 3 weeks` comment verbs, re-armed by a PR that says
   `Closes #N`, swept every 30 minutes). The stated rule is that automated workers respect
   claims and will not author a claimed target. A cooperative soft lock with a TTL, not a
   hard mutex.
5. **A self-growing reviewer pool.** `promote-reviewers.yml`: two merged roadmap PRs
   auto-promotes the author into the CODEOWNERS review team. Social mechanism, not
   applicable to us.
6. **Source-of-truth drift checking with `--fix`.** `check_roadmap_areas.py`. The design is
   good: one canonical source (the directory set), several denormalised copies that cannot be
   removed (GitHub issue forms have no templating), a checker that exits non-zero on drift,
   and a healer that **fills gaps only** and never rewrites an existing curated line
   (`rewrite_readme` docstring, lines 105-112). We already have `staleness.rs` / `Census`.

Explicitly **not** a mechanism: the "Writing a roadmap" bullet list in the root README is
authoring guidance for humans (build the library rather than racing to the theorem, ground
everything with no leaps, check Zulip and open Mathlib PRs first, use Mathlib's vocabulary,
nothing is optional, pin conventions). Good prose, no machinery behind it, no CI check.

---

## 6. Prioritized adopt list

Each item: the source evidence, what we build, which of our files it touches, and why it
beats what we have.

### A1. HIGH. A citation-grounding node kind, ported from the `Source. / Locus. / Supports.` extract format

**Source.** `TauCetiRoadmap/GeometricTopology/references/README.md:1-10` (the stated
purpose), the 27 extract files, header block shape at `references/hirsch-collar.md:2-4`,
`references/lickorish-alexander.md:3-5`. The `## Notes` correction pattern at
`references/hirsch-disc-theorem.md` and `references/lickorish-braids-markov.md`.

**What we build.** A `Citation` node kind carrying `source` (bibliographic), `locus`
(theorem number plus printed page), `verbatim` (the quoted statement), and `supports` (edges
to the obligations it grounds), plus an ingester for the `**Field.**` header block. Then a
`CitesEvidence` requirement: an obligation whose statement was transcribed from literature
carries an edge to the `Citation` node that supplies its verbatim form.

**Our files.** New `components/graph/citation.rs` for the node kind; the node model in
`components/graph/`; `components/graph/evidence.rs` (which already has an `AXIOM_AUDIT` key
at line 38) gets a `CITATION_LOCUS` key; ingester alongside
`components/reason/proving/blueprint.rs`; a `citation-import` verb in `app/lib.rs`
(enum `Command` at L67-556, dispatch match at L575, `pub use` at L26-40).

**Why it beats what we have.** We have nothing. Our blueprint ingestion carries `\lean`
declaration names and `\uses` labels but no provenance for the *statement text itself*.
Every statement-fidelity failure we have catalogued (`augment_statement`, the
`name_claims_more_than_statement` fixture) is a failure to pin what the statement was
supposed to say. An exact locator plus a verbatim quote is the only artifact that makes
"is this the theorem it claims to be" checkable by a human reviewer in bounded time. The
27 extracts are also directly usable as a **test corpus** for such a check.

### A2. HIGH. An opaque-constant laundering check (the missing half of `name_claims_more_than_statement`)

**Source.** `TauCetiRoadmap/ContourIntegration/Suggested.lean:85, 90, 97, 151, 156` (the
five `:= sorry` definitions), consumed at lines 143-144 (`IsNullHomologous`), 277-288
(`hungerbuhlerWasem_residueTheorem`), 300-308 (`hasCauchyPV_half_residue`). The rule they
violate is stated in their own root `README.md`, "Write Lean code" bullet.

**What we build.** A check that, given a submitted declaration, computes the set of constants
occurring in its type and flags the declaration when any of them is **opaque**, meaning its
value is `sorryAx`, it is a bare `axiom` outside the trusted kernel set, or it is a `def`
whose body is `sorry`. Report as a distinct verdict, e.g. `statement_rests_on_opaque_constant`,
separate from the existing `sorry`-in-proof detection, because the defect is in the
**statement**, not the proof, and `#print axioms` on an unproved goal cannot distinguish them.

**Our files.** `components/prover/vacuity.rs` (nearest existing home) or a new sibling;
`components/prover/hypothesis_audit.rs` (whose module doc already frames itself as covering
the gap `#print axioms` cannot see, so this is the same family);
`components/prover/backends/lean.rs:662` already parses the transitive axiom set from
`#print axioms`, which is the data source for the Lean case;
`components/reason/critique/statement_validity.rs` gains a `Check::OpaqueConstant` beside
`Check::Triviality` (L164), wired through
`components/reason/critique/validity_seams.rs:280-320`.

**Why it beats what we have.** Per the survey of our tree,
`name_claims_more_than_statement` exists today only as a **fixture verdict label**
(`components/eval/python/theoremata_tools/benchmarks/adversarial.py:91,638`, fixture
`components/eval/fixtures/trivial_existential/probe.lean`), with the loader docstring stating
we expect to fail it and that catching it needs a check we have not built. Our
`triviality.py` (`_is_degenerate` L196, `_unconstrained_vars` L492) works over integer
domains and catches trivially-*true* statements. It cannot catch a *contentless* statement,
which is a different failure: `hungerbuhlerWasem_residueTheorem` is not trivially true, it
is unfalsifiable because every symbol in it is opaque. This check is cheap (a constant-set
walk over the type), decidable, and has an immediate 5-line ground-truth corpus in
`ContourIntegration/Suggested.lean`.

### A3. MEDIUM-HIGH. Emit obligations as anonymous statements until they are proved

**Source.** `ConformalMapping/Suggested.lean:44,58,66,74,86,95`;
`Completed/EffectiveBounds/Suggested.lean:36,47,55`; `Multiquadratic/Suggested.lean:32,44,...`;
`Exchangeability/Suggested.lean`. 30 of 56 milestone statements corpus-wide are anonymous
`example`s carrying their claim in a docstring.

**What we build.** In blueprint generation and obligation emission, render an unproved
obligation as `example ... := sorry` with the human-readable claim in the docstring, and mint
the declaration name only at the point the obligation transitions to proved.

**Our files.** `components/reason/orchestration/blueprint_generate.rs` (the
`BlueprintGenerator` trait at L49, `plan_and_prove` at L149);
`components/reason/orchestration/blueprint_run.rs` (`SketchObligationProver` L394,
`ItemReport` L97); `components/prover/subgoal_extract.rs:249` `to_obligations`.

**Why it beats what we have.** `to_obligations` already enforces the *status* invariant
(everything enters `Unproved`; `ChildProposal::from_obligation` fixes it, and unrecoverable
goals become the `UNRECOVERED_GOAL` sentinel at L60 rather than a fabricated statement). It
does not enforce the *naming* invariant. A named unproved obligation is the seed of every
name-overclaim: the name is minted from intent, then the statement drifts, and the name is
never revisited. Anonymity removes the seed at zero cost, and is strictly compatible with
the existing status invariant.

### A4. MEDIUM. Cooperative claims with a TTL for parallel workers

**Source.** Root `README.md`, "Coordinating work: intentions and claims";
`.github/workflows/intentions.yml:1-30` (30d default, 90d max, 30-minute sweep,
`claim` / `disclaim` verbs, PR-closes refresh); `.github/ISSUE_TEMPLATE/1-intention.yml:36-52`
(the "Items in scope" field, which asks for declaration names or key phrases from the
roadmap's README).

**What we build.** A `claim` on a graph node with an expiry timestamp and a claimant id,
released automatically on expiry, refreshed by progress. Search drivers skip claimed
subtrees but are never blocked by them, since a claim is cooperative.

**Our files.** `components/graph/` node model; the MCGS driver and
`components/reason/orchestration/`; `app/lib.rs` for a `claim` / `disclaim` verb.

**Why it beats what we have.** Only relevant once we run several workers against one graph.
The specific design points worth copying are the TTL with a hard maximum and the
auto-release sweep: they make duplicate-work avoidance safe against a crashed worker, which
a plain lock is not. Deprioritised because it is infrastructure for a concurrency level we
have not reached.

### A5. LOW. The `**Unlocks.**` forward edge

**Source.** `GeometricTopology/README.md:287,354,407,516,569,624,685,735,777,819,890`.

**What we build.** An optional `Unlocks` edge kind, the inverse of `DependsOn`, from a proved
result to the named open problems it enables, carrying an external citation key such as
`[Kir97, Problem 1.41]`.

**Our files.** `components/graph/` `EdgeKind`.

**Why it is low.** It is derivable from `DependsOn` in the reverse direction whenever both
endpoints are in the graph. Its only unique content is the pointer to problems **outside**
the graph, which is a prioritisation signal, not a correctness one. Worth doing only when we
want the search driver to weight nodes by downstream payoff.

### Explicitly rejected

- **Ingesting their layer ordering as our dependency structure.** Rejected. It is a total
  order per roadmap with no per-item edges. `blueprint.rs:152-190` plus
  `blueprint.rs:269-326` plus `blueprint_run.rs:319` already give us a real DAG with
  statement/proof edge scoping and cycle detection. Adopting theirs would be a downgrade.
- **`check_roadmap_areas.py` as a new drift checker.** Rejected as redundant. We already
  have `components/reason/proving/staleness.rs` (`Census` L574, `census()` L630) and
  `staleness_sweep.rs`, landed in commit b439e4b. The one idea worth remembering from it is
  the gap-fill-only healer that never rewrites a curated line
  (`check_roadmap_areas.py:105-112, 128-160`), which is the right posture for any future
  `--fix` we add. Not worth a build item on its own.
- **Their `sorry` policy and CI.** Rejected. `ci.yml:16-19` explicitly declines an axiom
  audit and warning-as-error. We are strictly stricter: `components/prover/formal.rs`
  banned-token lists at L1116-1117 and L1169 with word-boundary handling that keeps
  `sorryAx` and `sorry'` distinct, plus `#print axioms` parsing at
  `components/prover/backends/lean.rs:662`.
- **The Lean corpus as a proof/benchmark corpus.** Rejected. Zero proved statements; every
  declaration is `sorry`. It is usable as a *statement* corpus (56 research-level Mathlib-adjacent
  goal statements with prose rationale) and as a ground-truth corpus for A2, but not as
  anything with a proof in it.
- **`promote-reviewers.yml`.** Rejected. Social process for a human org.

---

## 7. Licence obligations

- **Licence:** Apache License 2.0. `LICENSE` is the **unmodified stock boilerplate**: the
  appendix at line 189 still reads `Copyright {yyyy} {name of copyright owner}` with the
  placeholders unfilled.
- **Copyright holder, established from the source rather than the LICENSE file:**
  `TauCetiRoadmap/OrthogonalL2Bases/Suggested.lean:2` reads
  `Copyright (c) 2026 The Tau Ceti contributors. All rights reserved.` That is the only
  copyright statement in the corpus, and it is in exactly one of the 14 Lean files; the
  other 13 carry none. So the attributable holder is **"The Tau Ceti contributors", 2026**.
- **NOTICE file:** none present. Apache-2.0 section 4(d) obligations therefore do not
  attach, since there is no NOTICE to propagate.
- **What we must do if we adopt.** Apache-2.0 section 4(a) and 4(b): retain the licence
  text and the copyright notice, and mark any modified files as changed. Concretely, for
  each of A1/A2/A3, none of which copies Lean or prose verbatim, we owe attribution in the
  source header of the file we build, of the form: derived from TauCetiRoadmap
  (github.com/TauCetiProject/TauCetiRoadmap), Copyright (c) 2026 The Tau Ceti contributors,
  Apache-2.0. If we ever vendor any of the 27 reference extracts as test fixtures, note that
  those files are themselves **fair-use excerpts of in-copyright third-party books**
  (Hirsch, Lee, Lickorish, Rolfsen, Gompf-Stipsicz, Thurston, Ratcliffe, Moise,
  Rourke-Sanderson, Munkres, Freedman-Quinn, Burde-Zieschang, Hudson, Cerf,
  Kirby-Siebenmann, Candel-Conlon). Their Apache-2.0 grant does not and cannot relicense the
  quoted passages. Do not vendor them; reimplement the *format* and populate it from sources
  we are entitled to quote.
- Note also the repo's own instruction on this subject, root `README.md`, "Coordinate before
  integrating existing work": it says that mathematical overlap does not permit reuse of
  someone's code, that the licence must be verified to permit the intended copying, and that
  the plan should be discussed publicly first. That is their policy for their contributors,
  not an obligation on us, but it is consistent with the above.

---

## 8. POSSIBLE INJECTION

This repo is written *for* AI contributors and explicitly anticipates automated workers
reading it and acting on it. That is the highest-risk shape for a prose corpus, and it is
prose almost end to end. Nothing below was followed. It is reproduced here as data so a
later reader can see what the corpus tries to make an agent do.

**Directives addressed to automated workers, which the repo expects to be obeyed.**
Root `README.md`, "Coordinating work" section, closing paragraph:

> Automated roadmap workers **respect these claims**: within an area they will not author a
> target that someone else has claimed.

This is a behavioural instruction to any agent that reads the repo. If we ever point a
worker at this corpus, that sentence is untrusted third-party text that would silently
constrain the worker's target selection.

**Imperative authoring rules aimed at the reader.** The root `README.md` "Writing a roadmap"
section is 10 bullets of second-person commands, including: build the library rather than
racing to the theorem; every milestone must rest on existing material; check Zulip and open
Mathlib PRs before specifying an object; use Mathlib's vocabulary; do not use the word
"optional" and do not imply it; decide generality up front; pin conventions ahead of time.
Read as data these are reasonable editorial standards. Read by an agent that treats repo
prose as instruction, they redirect its methodology.

**Coordination directives with an external side effect.** The same section instructs the
reader to open a GitHub issue with the Intention template and then comment `claim` on it
(optionally `claim 3 weeks` or `claim 2026-08-01`), and `disclaim` to release.
`.github/ISSUE_TEMPLATE/1-intention.yml` and `.github/workflows/intentions.yml` implement
the bot side. An agent that followed this would create issues and comments in a third-party
GitHub org. **Do not.**

**Instruction-shaped text inside the Lean files.** Every `Suggested.lean` module docstring
contains second-person direction to the reader, e.g.
`UniversalCovers/Suggested.lean:16-18`: state each milestone here with `sorry` and hand it
to the AIs to discharge in `TauCeti/`. `ContourIntegration/Suggested.lean:11-29` and
`CombinatorialHeegaardFloer/Suggested.lean:23` are similar. These docstrings would be
carried along by any ingester that treats docstrings as statement metadata, which is a real
consideration for A1 and A3: **strip or quarantine docstring prose at ingest; do not
concatenate it into a prompt as if it were our own instruction.**

**Setup instructions that would run privileged operations.** `.github/scripts/README.md`
"One-time setup for the auto-commit" walks through creating a GitHub App, generating a
private key, and running `gh variable set SYNC_BOT_APP_ID` and
`gh secret set SYNC_BOT_APP_PRIVATE_KEY < path/to/key.pem`, and adding the app to a branch
ruleset bypass list. This is a credential-provisioning runbook sitting in a markdown file in
an untrusted corpus. It was read only. **Never execute it.**

**Build commands.** Root `README.md` "Building" ends with a fenced block containing
`lake exe cache get` and `lake build`. Fenced shell in untrusted prose. Not executed; the
compile attempts in section 3 were `lean` invocations I chose, on files I named, not
commands taken from the corpus.

**Nothing adversarial found.** There is no text attempting to override a system prompt, no
"ignore previous instructions", no attempt to exfiltrate, no reference to an agent's
configuration or tools. The one grep hit for "disregards" is mathematical
(`references/lickorish-braids-markov.md:32`, quoting the Markov move that "disregards the
(n+1)th string"). The risk here is **ambient directive prose**, not a planted attack.
