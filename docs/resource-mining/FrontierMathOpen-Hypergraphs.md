# FrontierMathOpen-Hypergraphs — resource-mining report

Repo root: `resources/FrontierMathOpen-Hypergraphs-main/FrontierMathOpen-Hypergraphs-main/`. ~18 content files; ~7,500-line Lean 4 + LaTeX + embedded-Python. Toolchain `leanprover/lean4:v4.28.0`, mathlib `v4.28.0`, plus `checkdecls`. No `sorry`/`admit`/`native_decide` (finite checks via kernel `decide`; two top results end with `#print axioms`).

## 1. Core result & math
Lower bounds for the extremal hypergraph function `H(n)` of Brian–Larson (finitary core of a Ramsey problem on incompatible ideals) — Epoch AI FrontierMath **full-problem OPEN** variant. Results (`Uniform.lean`, `Lubell.lean`): `25·H(n) ≥ 26·k(n)` for `n≥15` (`thm_main_H_lower_bound`) via explicit recursive construction `A(n)`; asymptotic `2·ln2 ≤ liminf H(n)/k(n)` via a Lubell frame. **Partial/lower-bound by design**: resolves the full-problem prompt but stops one vertex short of the single-challenge target (`H(20) ≥ 65`, not `66`); matching upper bound not formalized.

## 2. Blueprint & dependency-graph format (highest priority)
Standard **`leanblueprint`** (`preamble.tex` = amsmath/amssymb/amsthm + `leanblueprint`; `web.tex`/`print.tex` 4-line wrappers). Dep-graph lives in per-item macros in `blueprint/src/content.tex`:
```latex
\begin{theorem}\label{thm:main}
  \lean{HypergraphLowerBound.thm_main}
  \leanok%
  \uses{def:H_n, def:k_n, thm:constructive_An, thm:uniform_26_25}
  There exists a sequence of hypergraphs ...
\end{theorem}
```
- `\label{<kind>:<slug>}` = node id, namespaced (`def:`,`thm:`,`lem:`,`cor:`,`prop:`).
- `\lean{FQN}` = node→Lean binding (one per item).
- `\leanok` = Lean-complete flag (blueprint renders "written" vs "leanok" via node colors).
- `\uses{a,b,c}` = directed dependency edges. Appears on **statement** and separately inside `\begin{proof}...\uses{...}` → statement-deps vs proof-deps as two edge classes (e.g. `substitution_theorem` at line 171).
- `blueprint/lean_decls` = flat list of all 25 `\lean{}` targets, fed to `checkdecls` (`lakefile.toml:17-20`) → automated blueprint↔Lean consistency gate. **Single most portable artifact.**
- Blueprint prose also names auxiliary bundled structs per node (e.g. `WitnessFamily`, `BlockFamily`, `PartitionedBlocks`, `CountedBlocks`, `exactSmallFrames`/`boosters`/`residueGadgets`) → richer node→multiple-decl mapping than the one primary `\lean` tag.

## 3. Reusable patterns / code
- **`checkdecls` + `lean_decls` manifest** — cheap CI gate that every node name resolves.
- **Reflection/`decide`-with-soundness for finite combinatorial checks** (`Uniform.lean`): computable checker `FrameSpec.checkValid`/`IsValid` + `Decidable` instance, proven sound vs the abstract predicate (`isValid_iff_isFrame`, bitmask `testBit` rep `rawWitness`/`rawInterCount`); finite bank closed by kernel `decide` over lists. Template for "LLM emits a finite certificate table, Lean verifies exhaustively **without** `native_decide`" — aligns with LeanParanoia.
- **Custom elaborator `frame!`** (`Uniform.lean:103-106`): `macro_rules | \`(frame! $parts $supports) => \`(mkFrame $parts $supports (by decide))` auto-discharges well-formedness so data tables read declaratively.
- **`#print axioms <thm>`** at file end (Uniform 4611-4612, Lubell 2033-2034) — confirms only `propext/Classical.choice/Quot.sound`.
- **Triple-redundancy validation**: paper (`paper/input.tex`) embeds a stdlib-only constructor `solution(n)->str` AND an independent verifier `verify_hn_26_25.py` — informal paper + Python constructor + Python verifier + Lean proof.
- **Edge-set hypergraph encoding** (`Basic.lean`): `abbrev Hypergraph V := Finset (Finset V)`, `vertexSet := edges.biUnion id` → "no isolated vertices" definitionally implicit (removes a side-condition class).

## 4. What earlier targeted pass missed
The verified-`decide` certificate pattern + `frame!` macro; the triple-redundancy (Python constructor + verifier embedded in paper); the statement-vs-proof edge distinction; the `checkdecls` gate.

## 5. Test-case value
**High.** Self-contained, pinned toolchain + `lake-manifest.json`, single-command build, only mathlib + 2 small deps, no external data. Full pipeline present: GPT-authored paper (`paper/input.tex`, "GPT-5.4 Pro") → blueprint DAG → 4 Lean files → axiom gate. Difficulty bounded: `Substitution.lean` (794 lines self-contained combinatorics) is a good mid-size formalize target; `Uniform.lean` tests decide-certificate generation; `Lubell.lean` tests real-analysis asymptotics. Caveat: it's a lower-bound/partial result → good **honesty test-case** (agent must not overclaim `H(20)=66`). Doc drifts: README says `frontier.tex`/`FrontierMathHypergraphs/` but actual is `paper/input.tex`/`FrontierMathOpenHypergraphs/`.

## 6. New vs. known
Additive:
1. `checkdecls`/`lean_decls` as a first-class node-binding validator.
2. Verified-`decide` finite-certificate pattern with explicit soundness bridge + `frame!` macro — the positive construction pattern for LLM-emitted certificates that pass the kernel without `native_decide`.
3. Statement-deps vs proof-deps as distinct edge classes.
