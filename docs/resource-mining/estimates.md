# Resource mining: `estimates` (Terence Tao's proof assistant)

Full-pass study of `resources/estimates-main/estimates-main`. This supersedes our earlier
targeted skim. All paths below are relative to that root unless noted.

> **Critical correction to our framing.** The task brief called this "a Lean tactic/framework."
> **It is not.** `estimates` is a **standalone Python proof assistant** built on **SymPy + Z3**.
> There is **no Lean code** in the repo (0 `.lean` files). It only *imitates* Lean's tactic-mode
> UX and *emits* pseudo-Lean proof scripts as strings (never compiled). This matters for
> Theoremata: `estimates` is not a source of Lean tactics to port into our Lean layer — it is a
> reference design for a **Python-side symbolic reasoner / falsifier / estimate DSL** that sits
> *upstream* of Lean formalization. See §6.

---

## 1) What it is (scope, size, structure)

- **Identity.** "A mathematical proof assistant (Version 2.0)" by Terence Tao (`README.md:1`).
  Companion to two blog posts (2025-05-01, 2025-05-09) and an "orders of infinity" post
  (2025-05-04). Deliberately *weaker* than Lean/Isabelle/Rocq, aimed at "short, tedious tasks,
  such as verifying that one inequality or estimate follows from others" with special support for
  **asymptotic estimates** (`README.md:3`).
- **Language / deps.** Pure Python ≥3.13; only two runtime deps: `sympy>=1.14.0` and
  `z3-solver>=4.15.0.0` (`pyproject.toml:7-10`). Installable/runnable via `uv`. Entry point
  `from estimates.main import *`.
- **Size.** Core engine is small — ~3,000 lines of Python across 20 modules in `src/estimates/`
  (largest: `simp.py` 500, `proofassistant.py` 449, `order_of_magnitude.py` 435, `log_linarith.py`
  428, `propositional_tactics.py` 375, `subst.py` 234). Docs ~1,900 lines of Markdown. Plus a
  large **React/TypeScript web UI** (`ui/`, ~80 files) that runs the whole thing in-browser via
  **Pyodide** (Python-in-WASM) with a bundled `z3-built.wasm`.
- **Structure.**
  - `src/estimates/` — the engine (detailed below).
  - `docs/` — prose docs (`asymptotic.md`, `tactics.md`, `tactics/*.md`, `linprog.md`,
    `exercises.md`, `navigation.md`, `lemmas.md`, `littlewood_paley.md`).
  - `tests/test_all.py` — 25 end-to-end "solution" tests asserting stdout ends with
    "Proof complete!".
  - `ui/` — Vite + React + Redux browser front-end; visual proof-DAG editor (React Flow + dagre),
    a code editor, LaTeX↔Python translation, and metadata JSON (`tactics.json`, `lemmas.json`).
  - CI: `.github/workflows/ci.yml` (tests) + `pages.yml` (deploy UI). Bundled wheels
    `estimates-0.3.0-py3-none-any.whl`, `z3-0.2.0-py3-none-any.whl` under `ui/public/`.

**Core architecture (engine):** a Lean-style tactic state machine over SymPy objects.
- `ProofAssistant` (`proofassistant.py`) — top-level; two modes: **assumption mode** (declare
  vars, add hypotheses) and **tactic mode** (prove a goal). Holds a `ProofTree` + `current_node`.
- `ProofState` (`proofstate.py`) — `{goal: Basic, hypotheses: dict[str, Basic]}`. Hypotheses are
  either **variable declarations** (`Type` wrappers) or **predicates** (SymPy `Boolean`/`Relational`).
- `ProofTree` (`prooftree.py`) — n-ary tree; leaves are `sorry` nodes (open goals); a tactic
  replaces a leaf with 0+ children. Navigation = walking sorries. `is_sorry_free()` = proof done.
- `Tactic` (`tactic.py`) — ABC: `activate(state) -> list[ProofState]` plus UI metadata
  (`label`, `description`, `arguments`).
- Backends: `linprog.py` (exact rational LP via Z3, primal+dual Farkas certificates),
  `order_of_magnitude.py` (the `Theta`/`O` algebra), `log_linarith.py` (asymptotic solver),
  `bounded.py` (fixed/bounded predicates), `simp.py`, `subst.py`, `propositional_tactics.py`,
  `lemma.py`, `littlewood_paley.py`, `test.py`.

---

## 2) Reusable ideas / patterns / code for Theoremata — THE priority

This is the highest-value section. Everything here is concrete and citable.

### 2.1 The `OrderOfMagnitude` algebra — the core asymptotic engine (`order_of_magnitude.py`)

This is the crown jewel and *exactly* the "asymptotic/order-of-magnitude tooling" we drew from.
It builds a formal semiring of "orders of infinity" **inside SymPy** by subclassing SymPy `Expr`
and intercepting all arithmetic.

- **`Theta(expr)`** maps a positive real expression to its order of magnitude. Construction-time
  normalization laws (`order_of_magnitude.py:109-149`):
  - Positive **numeric constants collapse to `Theta(1)`** (`:123-127`) — the key move that makes
    constants disappear.
  - **`Theta` distributes**: `Theta(X+Y) -> OrderMax(Theta(X),Theta(Y))` (`:129-132`);
    `Theta(X*Y) -> OrderMul` (`:134-136`); `Theta(X**q) -> OrderPow` for rational `q` (`:138-144`).
  - Non-positive argument → prints a warning and returns `Undefined()` (`⊥`) rather than raising
    (`:119-121`). (Design choice: total functions returning ⊥, not exceptions.)
- **Custom operators** replace SymPy's native ones because `O` has **no zero and no subtraction**
  (`asymptotic.md:9,21`). `OrderOfMagnitude.__add__` is redefined as **`OrderMax`** (addition =
  max of orders!) (`:36-37`); `__mul__`→`OrderMul`, `__pow__`→`OrderPow`, `__truediv__`→ mul by
  inverse power (`:51-65`). Subtraction is a purely formal `FormalSub` marker with no math content
  (`:14-29,42-49`) — a hack to stop SymPy's simplifier from doing illegal cancellation.
- **`OrderMax`/`OrderMin`/`OrderMul`/`OrderPow`** (`:168-386`) each implement `__new__` +
  `doit()` with: dedup of args, flattening of nested ops, **gathering like terms into powers**
  (`OrderMul.doit` sums exponents of a common base, `:288-319`), and identity laws
  (`X**0=Theta(1)`, `X**1=X`, `Theta(1)**n=Theta(1)`, `:351-357`).
- **Comparison → relation objects**: `<,<=,>,>=` on orders build SymPy `Relational`s wrapping
  the other side in `Theta` (`:69-91`). This is how `X ≲ Y` becomes a first-class hypothesis.
- **Syntactic sugar** injected onto `Expr` (`:388-436`): `lesssim(X,Y) := Theta(|X|)<=Theta(Y)`
  (i.e. `X = O(Y)`), `ll` (`o(Y)`, strict), `gg`, `gtrsim`, `asymp(X,Y) := Eq(Theta(X),Theta(Y))`
  (`Θ`). These are monkey-patched as methods on `Expr` (`Expr.lesssim = lesssim`, etc.).
- **`OrderSymbol(name)`** — an abstract, formal order of magnitude (a symbol that *is* an order,
  positive by construction), for reasoning about magnitudes with no underlying real (`:161-165`).

**Why this is gold for Theoremata:** it is a compact, dependency-light, executable model of
`O`/`o`/`Θ`/`≲` reasoning. Our "asymptotic/feasibility tools" can adopt this near-verbatim as the
Python-side representation for magnitude estimates, *before* we ask Lean to formalize the tight
statement. The "constants collapse, `+`→`max`, gather-exponents" normalization is the reusable
insight.

### 2.2 `LogLinarith` — asymptotic goal solver by "log-linear programming" (`log_linarith.py`)

The headline tactic. It proves/refutes asymptotic inequalities by **taking logs** (conceptually):
a product/power inequality among `Theta(...)` monomials becomes a **linear** inequality among the
exponents, then solved by the same exact LP engine as `Linarith`.

Pipeline (`log_linarith.py:214-418`):
1. Gather hypotheses that yield order inequalities; **negate the goal** and add it (proof by
   contradiction). Strict inequalities become **non-strict** under `Theta` (`:56-63,289-305`).
2. **Integrality gap**: a positive *integer* variable `N` contributes `Theta(N) >= Theta(1)`
   (`:250-253`, commented "the integrality gap!") — i.e. `N ≳ 1`. This is a genuinely clever,
   reusable modeling trick.
3. `extract_monomials()` (`:92-117`) turns each order expression into a `dict[base -> Fraction
   exponent]` — the log-linear coefficient vector.
4. **Max/min handling** (`split_max`, default on): each `OrderMax` object generates a *disjunction*
   — one branch per "this arg is the max" (`arg <= max` always, plus one branch where `arg == max`)
   (`max_objects`/`min_objects` `:159-200`, expansion `:341-352`). This is the exponential-blowup
   source flagged in `linarith.md:89`.
5. **Fixed/bounded injection**: any variable that `is_fixed` gets `Theta(var)=Theta(1)`; any
   `is_bounded` gets `Theta(var)<=Theta(1)` (`:355-367`). This is how "let `k` be bounded" makes
   `k` vanish from the asymptotics.
6. Iterate over the Cartesian product of all disjunction branches (`itertools.product`, `:380`);
   the goal is proved iff **every** branch is infeasible. Certificates: infeasible → "multiply
   inequality *i* raised to power *c_i*"; feasible (counterexample) → assign
   `Theta(var) = X**value` for an unbounded order `X` (`:388-418`).

**`ApplyTheta` tactic** (`:34-89`) — converts an ordinary inequality hypothesis into its
asymptotic (`Theta`) form as a new named hypothesis, letting the user stage a proof manually.

**Reusable for Theoremata:** this is a self-contained *decision procedure for asymptotic
estimates with an explainable certificate* — directly usable both as a solver and as a **falsifier**
(it returns concrete asymptotic counterexamples). The "convert to log-linear, reuse LP, product
over max-disjunctions" structure is the template.

### 2.3 Exact rational linear programming with dual/Farkas certificates (`linprog.py`)

`feasibility(inequalities) -> (bool, certificate)` (`linprog.py:64-174`):
- Builds a Z3 `Solver` over `Real` vars; if **sat** → returns a **satisfying rational assignment**
  (a counterexample / feasibility witness).
- If unsat → solves the **dual** (one dual var per inequality, sign-constrained by sense, primal
  vars force `Σ dual*coeff = 0`, normalization so strict inequalities contribute) → returns
  **Farkas multipliers** proving infeasibility. If neither solves, raises "Farkas lemma violation".
- **Exact**: all coefficients are Python `Fraction`s — *no floating-point roundoff*, and it handles
  **strict + non-strict** inequalities correctly (`linprog.md:10`). `Inequality` class
  (`:13-51`): `{coeffs: dict, sense: leq|lt|geq|gt|eq, rhs}`, with a `dual_name()` and pretty-printer.
- `is_valid_counterexample` (`:176-181`) sanity-checks that a returned model respects `base**exp`
  relations among vars (used by log-linarith where vars are `Theta(x)` and `Theta(x**2)`).

**Reusable:** a clean "feasibility-with-certificate-either-way" primitive — precisely the
**falsify-before-prove** ethos in our pipeline. Both `Linarith` and `LogLinarith` are thin front-ends
over this one function. Worth porting the primal/dual pattern wholesale.

### 2.4 `Linarith` — Lean-style linear arithmetic (`linarith.py`)

`Linarith` (`:24-160`): negate goal, extract inequalities from hypotheses (incl. implicit
positivity/nonnegativity from variable *types*, incl. the **integer integrality gap** `n>=1` for
positive integers, `:69-77`), split `Ne` into a disjunction of two strict inequalities, iterate
scenarios, call `feasibility`. Verbose mode prints the exact certificate ("infeasible by summing …
multiplied by …") or a concrete counterexample. Notably **ignores non-real relations** (`:87-94`) —
it only reasons over ordered fields.

### 2.5 `Bounded`/`Fixed` — a lightweight "external type/attribute" system (`bounded.py`)

Because they wrap SymPy rather than fork it, they can't add `.is_bounded`. Instead they introduce
**`Fixed(expr)` and `Bounded(expr)` as `Boolean` marker hypotheses** (`bounded.py:14-45`) and
compute closure with **whitelist recursion**: `is_fixed`/`is_bounded` walk the expression and are
true iff every arg is, over an *approved operation whitelist* (`Mul, Add, Pow, Max, Min, Theta,
Order*, Abs, Relational…`) (`:63,83-86`). Powers are bounded only if the exponent is
`nonnegative` (`:85-86`).

**Reusable pattern:** "attach semantic attributes to expressions as boolean hypotheses + a
whitelisted structural-closure checker" is a clean, general trick for our node metadata (fixed vs
parameter-dependent quantities) without subclassing the CAS.

### 2.6 The tactic set (transferable proof-move vocabulary)

Full catalogue (each is a `Tactic` subclass; `__str__` emits a pseudo-Lean token):
- **Propositional** (`propositional_tactics.py`): `SplitGoal`, `SplitHyp`, `Cases`, `ByCases`,
  `Contrapose` (contrapositive *or* proof-by-contradiction, `:147-182`), `Option`, `Claim`
  (Lean's `have`, with auto-discharge of trivial subgoals, `:331-365`).
  - Notable: `get_conjuncts`/`get_disjuncts` (`:30-117`) encode **structural splitting of
    max/min/order relations**, e.g. `x <= Max(y,z)` ⇒ disjuncts `x<=y`, `x<=z`; `x = Max(y,z)` ⇒
    conjuncts `x>=y, x>=z, (x==y)|(x==z)`; and `LittlewoodPaley(a,b,c)` ⇒ disjunction over "which
    one is the max." These lookup tables (also documented as tables in `tactics/propositional.md`
    and `tactics/simplification.md`) are a reusable knowledge base of estimate-manipulation rules.
- **Arithmetic**: `Linarith`, `LogLinarith`, `ApplyTheta`.
- **Substitution** (`subst.py`): `Let`, `Set` (introduce+substitute), `Subst`, `SubstAll`
  (forward/reverse equality rewriting).
- **Simplification** (`simp.py`): `SimpAll` (hypothesis↔hypothesis+goal simplification loop, with
  optional `repeat` to fixpoint and optional `use_sympy`), plus `IsPositive`/`IsNonnegative`/
  `IsNonzero` (upgrade a variable's *type* by proving a sign condition), and `Calc` (Lean-style
  chained-relation blocks with a compatibility checker over the `{-1,0,1}` outcome lattice,
  `:363-499`).
- **Closers**: `Trivial` (`test.py`), lemma application `UseLemma`.

### 2.7 `SimpAll` internal simplifier (`simp.py:23-138`)

A hand-rolled, hypothesis-aware simplifier `rsimp`/`simp` that: recognizes when a goal *is* a
hypothesis (→`true`) or its negation (→`false`); does **relational refinement** via a sign-set
algebra (`{<= : {0,1}}` etc., `:48-66`) to combine `x<=y` with `x>=y` into `Eq`; removes dominated
args from `Max`/`Min` using ordering hypotheses (`:68-79`); and folds `Theta` of a fixed/bounded
integer to `Theta(1)` (`:81-85`). It deliberately **avoids** SymPy's global `simplify` by default
(too aggressive, breaks the order algebra). This "cheap, certificate-free, hypothesis-driven
rewriting that never calls the heavy CAS" is a good model for our fast pre-normalization pass.

### 2.8 Proof-object emission (pseudo-Lean)

`ProofAssistant.proof()` (`proofassistant.py:227-237`) renders the whole tree as a Lean-looking
`example (…) : goal := by` script with `sorry` leaves and `.`-indented case branches
(`prooftree.py:43-80`). It is **never type-checked** — purely a human-readable transcript. For
Theoremata this is a reminder: `estimates`' "proof" is a *plan/trace*, and our Lean-compile +
`#print axioms` gate is the real verification `estimates` lacks.

---

## 3) Schema / format

- **Proof-state schema** (`proofstate.py:13-22`): `hypotheses: dict[str,Basic]` + `goal: Basic`.
  Variable declarations are stored under the variable's own name as `Type(sympy_symbol)`; predicates
  under arbitrary names. This is essentially our node's "context + claim." Fresh-name generation via
  priming (`state.new`, `:40-45`).
- **Variable type vocabulary** (`basic.py:55-96`, `navigation.md:87-104`): `real/pos_real/
  nonneg_real/nonzero_real`, same for `int`, `rat`; `complex/nonzero_complex`; `bool`; **`order`**
  (an `OrderSymbol`). `typeof()` (`basic.py:11-52`) infers the tag back from SymPy assumptions.
- **`Inequality` LP schema** (`linprog.py:13-24`): `{coeffs: dict[var,Fraction], sense, rhs}`.
- **Tactic/lemma UI metadata schema** (`tactic.py:24-53`, materialized in
  `ui/src/metadata/tactics.json`, `lemmas.json`): each tactic exposes `id/label/description/
  className/arguments`, where `arguments ⊆ {variables, hypotheses, verbose, this, expressions}`.
  This is a compact, machine-readable **tool-catalogue schema** — directly relevant if Theoremata
  exposes tactics/tools to an LLM or a UI.
- **Problem schema** (`ui/src/metadata/sampleProblems.ts:3-9`): `{variables:[{name,type}],
  assumptions:[{name,input}], goal:{input}, label, description}` — a clean, serializable
  "estimate problem" record. 15 sample problems encoded this way.
- **Surface DSL**: goals/assumptions are just Python/SymPy expression strings (`x < 2*y`,
  `lesssim(x*y, N**4)`, `Or(And(P,R),…)`, `Bounded(k)`, `Theta(...)`). The UI adds a thin
  **LaTeX↔Python** mapping (`ui/src/features/pyodide/latexToPython.ts`): `\lor→|`, `\land→&`,
  `\neg→~`, `\implies→->`, `\iff→<->`, `\in→in`, `\cup→union`, etc. No custom parser — SymPy's
  `sympify` is the parser.

---

## 4) What our earlier targeted pass MISSED

Things a skim would not have surfaced, now confirmed by the full read:

1. **It's not Lean.** Zero Lean; the "Lean tactics/APIs" we thought we were mining are Python
   tactic classes emitting *cosmetic* pseudo-Lean. The real reusable asset is a **Python symbolic
   estimate engine**, not Lean code. (Re-scopes how we cite this resource.)
2. **The dual/Farkas certificate machinery** (`linprog.py:136-174`) — infeasibility isn't a bare
   "unsat"; it returns *explicit multipliers*. That's a ready-made explainable-proof / audit trail.
3. **`LogLinarith` returns asymptotic counterexamples** (`log_linarith.py:388-400`,
   `linarith.md:125-131`): `Theta(x)=X**1/2` etc. for an unbounded `X`. This is a concrete
   **falsifier for order-of-magnitude conjectures**, not just a prover — squarely our
   "falsify-before-prove."
4. **The "integrality gap" trick** (`n` positive integer ⇒ `n ≳ 1` / `n ≥ 1`) appears in *both*
   `Linarith` and `LogLinarith` — a small but load-bearing modeling detail for estimate soundness.
5. **`Fixed`/`Bounded` as boolean-marker hypotheses + whitelist closure** (`bounded.py`) — a whole
   sub-system for "which quantities count as constants," feeding the asymptotics. Easy to miss.
6. **Structural max/min/LittlewoodPaley split tables** (`propositional_tactics.py:30-117`; tables
   in `tactics/propositional.md`, `simplification.md`) — a reusable rule library for decomposing
   estimates. `LittlewoodPaley(*args)` collapses to `asymp` for 2 args (`littlewood_paley.py:29-32`).
7. **Two real research-grade worked examples**: the complex Littlewood–Paley estimate is adapted
   from Tao's paper *arXiv:math/0005001* eq. (51) (`main.py:309`), with an explicit note that brute
   `Cases+LogLinarith` costs ~a minute of CPU and how `SubstAll`+`SimpAll` pre-reduction speeds it up
   (`exercises.md:484`). Real performance/strategy guidance.
8. **`Calc`'s outcome-lattice soundness checker** (`simp.py:396-469`) — it *verifies* the chain of
   relations actually implies the goal over `{-1,0,1}` before splitting. Non-obvious rigor.
9. **The entire browser deployment**: Pyodide + Z3-in-WASM (`ui/public/z3-built.wasm`,
   `z3-built.js`), a React-Flow proof-DAG visual editor (`ui/src/components/VisualEditor/…`,
   `features/proof/dagre.ts`), and auto-generated tactic/lemma metadata via
   `ui/build_tactics_lemmas.py`. Evidence that the engine is small/portable enough to run client-side.
10. **`Undefined`/`FormalSub`/warning-not-exception** design for partiality of the order algebra
    (`order_of_magnitude.py:4-29`) — the deliberate choice to keep operations total.

---

## 5) Test / benchmark value (usable as formalize targets?)

**Yes — high value as a curated estimate benchmark suite.** `tests/test_all.py` + `main.py` provide
~30 named, self-contained problems with reference solutions and expected outcomes. Each is a
candidate **Theoremata formalization target** (Python statement → Lean `theorem` → best-of-N →
compile + `#print axioms`). Highlights:

- Linear-arithmetic (solvable + provably-impossible-with-counterexample) — good for exercising the
  falsifier path (`linarith_exercise`, `linarith_impossible_example`).
- Asymptotic: `loglinarith_exercise` (`x≤2N², y<3kN, k bounded ⊢ xy ≲ N⁴`), `loglinarith_hard_*`
  (needs staged `ApplyTheta`+`Claim`), and a *negative* one (`loglinarith_imposssible_example`) with
  an explicit asymptotic counterexample — ideal to test both prove and falsify.
- AM–GM via lemma application; min/max; bracket submultiplicativity `⟨xy⟩ ≲ ⟨x⟩⟨y⟩` (needs
  case-splitting on vanishing); Littlewood–Paley (`min·max² ≲ N₁N₂N₃`) and the paper-derived complex
  LP estimate.
- Propositional: case-split, pigeonhole, trichotomy.

These come with the "informal version" natural-language statement in `docs/exercises.md` — so they
double as an **NL→formal benchmark** (each has {informal English, Python/SymPy encoding, expected
result, reference tactic sequence}). That triple is exactly what our formalize-and-check loop wants
for eval. Caveat: they are *estimate/inequality* problems (Tao's niche), not general Mathlib-style
theorems — a focused, not broad, benchmark. Also each carries a Lean-ish `proof()` string we could
diff against, though it's uncompiled.

---

## 6) New vs. already-in-our-design

**Already in our design (confirms/validates our approach):**
- Falsify-before-prove: their `feasibility()` returning a counterexample, and `LogLinarith`'s
  asymptotic counterexamples, are precisely this. Validation that a **model-derived executable
  falsifier** is the right first gate.
- Tactic/proof-DAG core: their `ProofTree` (sorry-leaves, n-ary branching, navigation) mirrors our
  proof-DAG. Our design already has this; theirs is a compact reference impl.
- Model-agnostic separation: N/A here (no LLM), but the clean `Tactic` ABC + JSON tool-metadata is a
  good template for exposing tools to our LiteLLM-driven agent.
- Exact rational LP + Z3: matches our "feasibility tools" intent.

**New / worth adopting (not yet in our design, or under-specified there):**
1. **The `Theta`/`OrderOfMagnitude` SymPy algebra** (§2.1) — a concrete, portable implementation of
   `O`/`o`/`Θ`/`≲` we can lift into Theoremata's Python asymptotic layer. We had the *idea*; this is
   a working *artifact* with the normalization laws worked out.
2. **Log-linear reduction** (`estimate ⇒ take logs ⇒ linear program over exponents`, §2.2) — a
   specific, reusable algorithm for discharging/refuting order-of-magnitude goals, incl. the
   **max/min → disjunction-product** expansion and the **integrality-gap** injection.
3. **`Fixed`/`Bounded` marker-hypothesis + whitelist-closure** attribute system (§2.5) — a clean way
   to tag "constant vs parameter-dependent" quantities in our node metadata without patching a CAS.
4. **Dual-LP Farkas certificate emission** as a first-class output (not just sat/unsat) — an
   explainability primitive for our audit trail.
5. **Structural decomposition rule tables** for `Max/Min/⟨·⟩/LittlewoodPaley` (§2.6) — a starter
   knowledge base of estimate-manipulation rewrites.
6. **`Calc` outcome-lattice soundness check** — a pattern for validating multi-step chained-estimate
   plans before committing to them.
7. **The problem/tactic JSON schemas** (§3) — ready-made serialization for estimate problems and a
   tool catalogue.

**Caveats / what NOT to take:** `estimates` has **no formal verification** (proofs are uncompiled
strings), leans on SymPy's fragile global simplifier when `use_sympy=True` (documented footguns re:
division-by-zero and subtraction, `simplification.md:22`), has **exponential blowup** on
max/min-heavy goals (`linarith.md:89`), and warns of **non-determinism** from Python `set`/`dict`
ordering in `SimpAll` (`simplification.md:24`). Its role for us is the *upstream symbolic
reasoner/falsifier and estimate DSL*, with Lean compilation + `#print axioms` remaining our actual
soundness gate.
