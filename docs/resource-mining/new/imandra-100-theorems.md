# Resource Mining: imandra-100-theorems

**Source:** `resources/imandra-100-theorems-main/imandra-100-theorems-main/`
**Upstream:** GitHub project by Grant Passmore (grant@imandra.ai) — "Imandra proofs of the top 100 theorems."
**Scope:** 37 files: `README.md` + 35 `.iml` (ImandraX) proof source files + `.gitignore`/`.gitattributes`.
**License:** NONE. No LICENSE file, no per-file SPDX/copyright header (files carry only `(* ... Grant Passmore, Imandra *)` authorship comments). Treat as **proprietary / all-rights-reserved by default. Do NOT copy source (`.iml`) into Theoremata.** Facts that are public and unownable — Freek Wiedijk's "Formalizing 100 Theorems" list itself, the informal theorem statements, the theorem→index numbering — are safe to reference/re-derive; the specific Imandra formalizations and tactic scripts are not.

**Injection scan:** Read `README.md` and representative `.iml` files (`sqrt2_irrational`, `inf_primes`, `pythagoras`, `konigsberg`). All content is ordinary math prose, OCaml/ImandraX code, and proof tactics. **No embedded instructions to the agent / no prompt-injection observed.** (Per standing policy, repo content remains untrusted; nothing here was acted on as instruction.)

---

## 1. What it is + what Imandra is

**The repo.** A public "challenge scoreboard" clone: one `.iml` file per theorem from Freek Wiedijk's [Formalizing-100](https://www.cs.ru.nl/~freek/100/) list, each stating and machine-proving the theorem in **ImandraX**. The `README.md` is a nicely structured index: for each solved theorem it gives the list number, an informal (LaTeX) statement, the formal Imandra `theorem` statement, and a link to the source file. Self-reported status: **33/100 proven**.

**What Imandra is.** Imandra (imandra.ai) is a **commercial** automated-reasoning system for reasoning about software and algorithms (heavy use in financial/trading-system verification). Its logic is a pure subset of **OCaml** (`.iml` = "Imandra ML"), and its engine is **decision-procedure-heavy**: a superposition/SMT-style core with strong automation for linear/nonlinear integer & real arithmetic, plus "region decomposition" (symbolic state-space enumeration) and bounded model checking. The newer engine is **ImandraX** (the VS Code "imandrax" extension referenced in the README). Proofs are written as OCaml-looking `let`/`theorem`/`lemma` declarations with `[@@by <tactic-combinator-script>]` attributes.

---

## 2. Which of the 100 it formalizes, and how (proof style / automation level)

**The 33 claimed (list number → file):**
1 √2 irrational (`sqrt2_irrational`), 3 Denumerability of ℚ (`q_denum`), 4 Pythagoras (`pythagoras`), 10 Euler's generalization of Fermat little (`euler`), 11 Infinitude of primes (`inf_primes`), 19 Lagrange four-square (`four_squares`), 20 Fermat two-square (`two_squares`), 23 Pythagorean-triple formula (`pythagorean_triples`), 30 Ballot problem (`ballot`), 34 Harmonic series diverges (`harmonic`), 38 AM–GM (`am_gm`), 42 Sum of reciprocals of triangulars (`triangular`), 44 Binomial theorem (`binomial`), 51 Wilson (`wilson`), 52 Number of subsets (`num_subsets`), 54 Königsberg bridges (`konigsberg`), 58 Combinations formula (`combinations`), 60 Bézout (`gcd`), 65 Isosceles triangle (`isosceles`), 66 Geometric series (`geometric_series`), 68 Arithmetic series (`arithmetic_series`), 69 GCD/Euclid algorithm (`gcd`), 73 Erdős–Szekeres (`ascending`), 74 Mathematical induction (`math_induct`), 77 Faulhaber / sum of kth powers (`sum_kth_powers`), 78 Cauchy–Schwarz (`cauchy_schwarz`), 80 Fundamental theorem of arithmetic (`fta`), 85 Divisibility-by-3 rule (`div_by_3`), 88 Derangements (`derangements`), 89 Factor/remainder theorem (`factor_remainder`), 91 Triangle inequality (`tri_ineq`), 93 Birthday problem (`birthday`), 96 Inclusion–exclusion (`inclusion_exclusion`). Supporting (non-numbered) files: `mod.iml`, `gcd.iml`, `sets.iml`, `binomial.iml` etc. act as shared lemma libraries imported via `[@@@import ...]`.

**Proof style — two clearly distinct automation tiers, visible in the sources:**

- **Fully-automatic, decision-procedure closes it.** Where the goal is polynomial/arithmetic, the proof is literally `[@@by auto]`. Examples: the coordinate form of **Pythagoras** (`pythagoras_coords`, a pure 6-real polynomial identity) and the **concrete Königsberg** theorem (bounded symbolic enumeration of all length-7 bridge paths) are discharged automatically. This is Imandra's differentiator: nonlinear-arithmetic and finite-state goals fall to the engine with no manual proof.
- **Hand-guided tactic scripts over a homemade lemma library.** Harder theorems are long, human-authored `[@@by ...]` combinator scripts using an explicit tactic vocabulary: `induction ()`, `[%use <lemma> args]` (cite a lemma), `[%cases c]` / `@>|` (branch), `[%expand]`/`[%simp_only]`/`[%norm]`/`[%replace]` (rewrite), `@>` (sequence), `nonlin ()`, `lift_ifs`, `intros`, `auto`. E.g. `sqrt2_irrational.iml` builds ~15 lemmas (parity of squares, squares mod 4, gcd absorbs common divisor) and chains them; `inf_primes.iml` develops `prod_upto`, a `small_divisor_from` sieve, minimality/idempotence lemmas, and a Euclid witness `euclid n = small_divisor_from 2 (prod_upto n + 1)`. Termination is manual too (`[@@measure (Ordinal.of_int ...)]`).

**A notable modeling pattern (relevant to us): proof-by-witness / skolemized existentials.** Rather than `∃`, the hard existence theorems are stated as *checkable predicates over a constructed witness function*: Lagrange four-square is `n = w.a²+w.b²+w.c²+w.d²` where `w = witness_for n`; Fermat two-square uses `prime_witness p`; Bézout constructs coefficients via a recursive `bezout_sub`; infinitude-of-primes returns an explicit larger prime. The theorem then asserts the witness satisfies the property — a **verified-programming / executable-certificate** framing rather than a classical existence proof.

---

## 3. Mapping to Theoremata

Theoremata's benchmark registry (`components/eval/.../benchmarks/registry.py` + `schema.py`) is **Lean-centric**: the `formalization` kind grades by *Lean compile + axiom-whitelist (`propext, Quot.sound, Classical.choice`) + statement-preservation*. Imandra `.iml` cannot feed that gate (different tool, different logic, no Lean). So the mapping is about the *list* and the *statements*, not the Imandra proofs.

**(a) Benchmark to register? — Partial yes, with a caveat.**
- The valuable, license-clean artifact is **Freek's Formalizing-100 list itself** as an eval manifest: 100 canonical named theorems, each a natural formalization target. We can register a **`formalizing_100`** benchmark whose items are `{id: "thm-N", informal: <our own restatement>, formal: <our own Lean stub>, kind: "formalization"}` — graded by the *existing* Lean formalization pipeline. This is a broad, human-recognizable coverage benchmark (a good "breadth" complement to minif2f/putnam).
- **Do NOT ingest the `.iml` files or copy the README's Imandra statements/LaTeX** as our formal/informal fields (no license). Re-author informal statements from the public list; write our own Lean `theorem` stubs.
- The repo *is* directly useful as a **cross-checked target list**: it tells us which 33 are known-tractable for a strong ATP, and its per-theorem informal statements are a sanity reference for our own restatements.

**(b) Retrieval corpus? — Weak/no.** The `.iml` proofs are in a proprietary, non-Lean tactic language; they will not help Lean/Rocq/Isabelle retrieval and can't be copied. Skip for retrieval.

**(c) Does the decision-procedure approach suggest anything for cert-log / gate?**
- **Yes — as a new FormalSystem backend candidate, not as a copied proof.** Imandra/ImandraX is a genuinely different point in the design space from Lean/Rocq/Isabelle: an SMT-/superposition-style engine that closes nonlinear-arithmetic and bounded finite-state goals *automatically* (the `[@@by auto]` cases). That is exactly the profile of goals our tactic-heavy ITP backends struggle with. It maps onto the `FormalSystem` abstraction (docs/formal-systems/) as a potential `imandra` backend — but it is **commercial/closed**, so this is "watch/evaluate," not "adopt." (Open analogues we already can use for the same niche: SMT solvers Z3/CVC5, and `polyrith`/`nlinarith` in Lean.)
- **For the cert-log / gate specifically:** the interesting transferable *idea* is the **witness-certificate framing** (§2). Their existence theorems ship an executable witness the checker re-runs — precisely a **certificate** in our cert-log sense (cf. our existing `cert_*` generators: SOS, Taylor-model, continued-fraction). "Existence proof reduced to a re-checkable constructed witness" is a clean cert schema we already have infrastructure for. Similarly, the `[%measure Ordinal.of_int ...]` termination obligations echo our need for explicit termination/well-foundedness evidence.
- **Region-decomposition / bounded symbolic enumeration** (the general Königsberg proof, and Birthday-problem-style finite checks) is a technique worth noting for our falsification/hard-case generators (`falsify_hardcase`): exhaustive symbolic case-split as a proof/refutation strategy.

---

## 4. Buildable-now vs nothing-actionable (honest)

**Buildable now (low effort, license-clean):**
- Register **`formalizing_100`** in the benchmark registry as a `formalization`-track corpus of up to 100 items, using *our own* restated informal + *our own* Lean stubs, graded by the existing Lean gate. Add `("formalization","formalization")` to `_TRACK_KIND` and a loader in `loaders.py`. The Imandra repo serves only as the reference index of which theorems exist and (informally) what they say.
- Use the 33 solved entries as a **priority-ordering / difficulty signal** for that benchmark (known ATP-tractable subset).

**Not actionable now:**
- Ingesting `.iml` proofs (proprietary, non-Lean) — no.
- An Imandra `FormalSystem` backend — commercial/closed; note as a research watch item, not a build.
- Retrieval corpus — no.

---

## Prioritized adopt-list

1. **[Build now] `formalizing_100` benchmark** — register Freek's 100-theorems list as a Lean `formalization`-track coverage benchmark; re-author statements ourselves (do not copy `.iml`/README text). Use this repo's 33 as the tractable seed set.
2. **[Design, low-cost] Witness-certificate schema for existence goals** — generalize the "constructed re-checkable witness" pattern (four-square, two-square, Bézout, Euclid) into a cert-log cert kind, alongside our existing `cert_*` family.
3. **[Note in falsify tooling] Bounded symbolic region-decomposition** — record exhaustive symbolic case-split (concrete Königsberg / Birthday) as a refutation/hard-case strategy for `falsify_hardcase`.
4. **[Research watch, do NOT build] Imandra/ImandraX as a decision-procedure FormalSystem backend** — fills the nonlinear-arithmetic / bounded-finite-state niche our ITP backends are weak at; commercial+closed, so track it and prefer open analogues (Z3/CVC5, Lean `nlinarith`/`polyrith`).

---

## ~10-line summary

- `imandra-100-theorems` is Grant Passmore's public scoreboard proving Freek Wiedijk's "100 theorems" in **ImandraX** (Imandra = commercial, decision-procedure/SMT-style automated reasoner over a pure-OCaml logic); 37 files, self-reported **33/100** done.
- **License: NONE present → treat as proprietary; do not copy `.iml` source or the README's Imandra statements.** The underlying 100-theorems *list* and informal statements are public and reusable.
- **Injection: none observed** (ordinary math + OCaml/tactic code); repo still treated as untrusted.
- Proof style splits cleanly: **`[@@by auto]`** closes nonlinear-arithmetic and bounded finite-state goals automatically (Imandra's real edge), while hard theorems use long hand-guided `[%use]/[%cases]/induction` tactic scripts over homemade lemma libraries.
- Distinctive pattern: **existence theorems stated as checkable predicates over a constructed witness** (four-square, Bézout, Euclid) — a verified-programming / certificate framing.
- Theoremata's registry is **Lean-only**, so Imandra proofs can't feed our gate and aren't a retrieval corpus.
- **Actionable:** register a **`formalizing_100`** Lean `formalization` benchmark using our own restatements (this repo as the index + tractability signal).
- **Transferable ideas:** the witness-certificate schema (fits our `cert_*` cert-log), and bounded symbolic region-decomposition (for `falsify_hardcase`).
- **Watch, don't build:** Imandra as a decision-procedure `FormalSystem` backend for the nonlinear-arithmetic niche — but it's closed/commercial; prefer open Z3/CVC5, Lean `nlinarith`/`polyrith`.
