# HOL Light mining: reflection, CAS-as-oracle, model theory, program verification

Source: four John Harrison papers (`atp/reflect.pdf`, `atp/cas.pdf`, `atp/model.pdf`,
`atp/dijkstra.pdf`). Read in full via pypdf extraction. Assessed for what maps to
**Theoremata** (verification-first Rust+Python harness; Lean/Rocq/Isabelle + Candle
backends; cert-log proof-log with 13 kinds; 3+1 verification gate; MCGS search;
"search-anywhere, check-in-kernel" — untrusted oracle emits a certificate, only the
kernel-check is trusted).

**Bottom line:** `cas.pdf` is the literal charter for the cert-log philosophy (find
untrusted, check in kernel); `reflect.pdf` is the deep survey that both *justifies*
the "check-in-kernel" trust model and answers the "is reflection a way to make our
checkers kernel-trusted?" question (yes, but usually not worth it — prefer
certificates); `model.pdf` is a reusable metatheory toolkit yielding a Herbrand
certificate kind; `dijkstra.pdf` is the canonical case study that a *failed* proof
attempt localizes a real bug (our falsify/critic loop).

---

## 1. cas.pdf — "A Sceptic's Approach to Combining HOL and Maple"

**(1) What it is.** J. Harrison & L. Théry, Univ. Cambridge / INRIA, 1997
(circulated 19 Aug 1997; published in *Journal of Automated Reasoning*). 17 pp. The
canonical statement of **CAS-as-untrusted-oracle**: contrast theorem provers vs
computer algebra systems, then synthesize them by **systematically separating search
for a solution from checking the solution**, over a physical link (a "software bus":
HOL ⇄ bridge ⇄ Maple). Maple is trusted *not at all*; every answer is re-derived by
HOL inference.

**(2) Key mechanisms.**
- **Find/check separation = the cert-log made concrete.** The untrusted CAS produces
  a *certificate* (usually just "the answer"); HOL confirms it by rigorous reduction
  to its ~10 primitive rules. Explicitly framed via **NP / co-NP**: a certificate is
  a piece of data that makes checking cheap even when finding was expensive.
- **Certificate catalogue (directly portable to cert-log kinds):**
  - *Factorization* — certificate is the factors (check by multiplying out).
  - *GCD* — the richer-certificate trick: don't return `gcd = x+1`; call `gcdex` to
    get **Bézout cofactors** `r,s` with `d = p·r + q·s`, then "greatest" follows from
    an easy lemma. **The lesson: pick the algorithm variant that emits a
    kernel-checkable certificate**, not just the bare answer.
  - *Antiderivatives* — check by differentiating (Fundamental Theorem of Calculus);
    worked trig-integral example uses a *cascaded* second find/check pass (Maple
    factorizes the residual, HOL expands to confirm `sin²+cos²−1` divides it).
  - *Closed-form summations* — check by differencing/induction.
  - *Equation solving, root isolation / Sturm sequences* — cited from the FP-verif
    work: Maple supplies squarefree decomp + isolating intervals, HOL checks.
  - *Ground numeric values* — assert `|π − 3.14159…| < 10⁻¹⁸` with an **explicit
    accuracy bound** rather than trusting the CAS's digits.
- **Trust ladder** (choose per domain): (a) check everything (no trust — their
  default); (b) trust the CAS wholesale (imports its bugs); (c) **Mike Gordon's
  oracle-tagging**: tag the theorem with an extra assumption naming the tool, e.g.
  `MAPLE ⊢ φ`, so every downstream result *inherits* the assumption and its
  dependence on the untrusted tool is explicit (this became Isabelle's `oracle`
  mechanism); (d) **lazy/deferred checking** — batch the checks and run overnight
  (Boulton's lazy theorems).

**(3) Mapping to Theoremata.** This *is* the "search-anywhere, check-in-kernel"
principle. Adopt-worthy directly:
- New **cert-log kinds**: `factorization`, `gcd_bezout`, `antiderivative_by_diff`,
  `summation_by_differencing`, `root_isolation_sturm`, `cas_numeric_error_bound`.
  Theoremata already ships SymPy/`symbolic.py` and `safe_eval` workers — these become
  *proposers*; the kernel/Lean check is the *acceptor*. The GCD→Bézout move is the
  template: prefer algorithm variants whose output is a certificate.
- **Oracle-tagging → EdgeStrength + taint + axiom-audit.** Gordon's `TOOL ⊢ φ` tag is
  exactly Theoremata's taint propagation and `EdgeStrength` tiers
  (`numeric_screen < prose_proof < lean_checked`). Any result that trusted an
  unchecked oracle should carry a taint tier and be surfaced, never silently accepted.

**(4) Buildable now vs gated.** *Now:* the certificate kinds above (SymPy proposes,
Lean/kernel checks); the oracle-taint tier. The physical "software bus" and stateful
CAS dialogue are **not** needed — Theoremata's JSON-lines worker protocol already is
the bridge. *Gated:* nothing significant.

**(5) Injection + license.** No injected instructions found. No license stated
(author preprint) → **clean-room, ideas only**.

---

## 2. reflect.pdf — "Metatheory and Reflection in Theorem Proving: A Survey and Critique"

**(1) What it is.** John Harrison, SRI Cambridge Technical Report CRC-053, 15 Feb 1995
(44 pp). The definitive survey/critique. **Thesis:** the fully-expansive **LCF
approach** (force every proof to decompose to a handful of primitive kernel rules) is
*not* as inefficient as folklore claims, and **reflection** (verify a decision
procedure's code, then install it as trusted) is intellectually attractive but has
"only once been used in a major practical prover" (NQTHM) and rarely pays off. Directly
answers our brief's question *"is reflection a way to make our checkers
kernel-trusted?"*.

**(2) Key mechanisms.**
- **Three ways to extend a prover:** (1) add a primitive rule (risky, unsound-prone);
  (2) **LCF derived rules** — arbitrary ML programs that ultimately decompose to
  primitives, so an unsound rule can *fail* but can never mint a false `thm`
  (theorems are an abstract datatype whose only constructors are the primitive
  rules); (3) **reflection** — prove the new rule's code correct *inside the prover*,
  then trust it.
- **Two very different "reflections" (he insists on separating them):**
  - **Logical reflection** — reflection *principles* that genuinely *strengthen* the
    logic: Gödel numbering + provability predicate `Pr(⌜φ⌝)`, the derivability
    conditions, consistency statements `Con`, Turing/Feferman **transfinite
    progressions** (local/uniform reflection schema), and set-theoretic reflection in
    ZF. *Not what we want* — these change provability.
  - **Computational reflection** — does **not** strengthen the logic, only speeds
    deduction. Recipe: define a denotation/`MEANING` function on encoded formulas;
    prove once the correctness metatheorem `⊢ DP(⌜φ⌝) ⇒ Pr(⌜φ⌝)` for a decision
    procedure `DP`; thereafter deriving `⊢ DP(⌜φ⌝)` licenses concluding `⊢ φ` *without
    expanding the proof*. Realized in **NQTHM** (Boyer–Moore "metafunctions": prove
    `MEANING(A,F)=MEANING(A,fn F)`, then install compiled code), **Nuprl** (Howe's
    verified rewriter; reflection rules stratified by level), and *proposed* for HOL
    (**Slind**: don't fully verify — just prove *program equivalence*
    `dest_thm ∘ f = g`, then `mk_thm ∘ g` is a safe new rule).
- **Why LCF is usually enough (the efficiency toolkit):**
  - **Proforma theorems** — prove a general schema once (e.g. "every polynomial is
    differentiable" over a list encoding), then each use is a cheap instantiation +
    a few primitive steps. HOL's higher-order logic *is* a functional language, so
    much "meta" work internalizes with only a *linear/constant-factor* slowdown.
  - **Separate search from inference** (the cas.pdf point again): resolution/tableau
    search over millions of clauses needs no inference; only the short found path is
    replayed as primitive steps.
  - **Memoize/cache** repeated trivial theorems; **lazy theorems** (defer inference);
    **partial evaluation** of derived rules.
- **The genuinely hard cases for LCF** (where reflection *might* pay): **BDDs**
  (imperative, shared structure — ~40× slowdown), **Wu's method** (huge-degree
  polynomials needing FFT multiply), and **bignum arithmetic** (duplicating a
  hardware facility in the logic — `O(log² n)` per op). All three want *imperative*
  code, which is exactly what's hard to verify — hence his scepticism about
  reflection's practicality, plus: reflection is hard to combine with a *checkable
  proof log*.

**(3) Mapping to Theoremata.**
- **Validates the whole trust model.** "Anything of type `thm` must have arisen from
  the primitive rules" *is* Theoremata's check-in-kernel principle; **Candle**
  (verified HOL Light on CakeML) is the extreme, machine-verified endpoint of the LCF
  argument Harrison makes here. This paper is the intellectual justification for the
  design.
- **Answer to "reflection → kernel-trusted checkers?"** *Yes in principle, prefer
  certificates in practice.* Computational reflection can make a Theoremata checker
  kernel-trusted (prove it correct once, then trust its output), but Harrison's
  verdict — used seriously only once, and the payoff cases need un-verifiable
  imperative code — says the certificate route (untrusted oracle → kernel-checkable
  certificate, i.e. our cert-log) dominates. **Reflection is our gated/frontier
  option**, warranted only for a checker that is (a) a proven bottleneck run
  enormously often and (b) has a clean pure-functional core.
- **Concrete audit rule.** The backends *already ship* reflection: Lean's
  `decide`/`native_decide`, Isabelle's `Eval`/code-generation, Rocq's
  `vm_compute`/`native_compute`. `native_decide` in particular **enlarges the TCB**
  (it adds `Lean.ofReduceBool` to `#print axioms`) — precisely Harrison's caution.
  Theoremata's **3+1 gate** should treat reflective tactics as a **distinct, tainted
  trust tier** in the axiom-audit whitelist and source-scan, not as a free pass.
- **Efficiency levers to document/adopt:** proforma-theorem instantiation, the
  existing Lean result cache (= his memoization), lazy-checking, and separate
  search/inference (= our MCGS + falsify-before-prove) are all endorsed here.

**(4) Buildable now vs gated.** *Now:* (a) an explicit **reflection-tier taint** so
`native_decide`/`vm_compute`/`Eval`-backed certificates are flagged in the audit;
(b) a proforma-theorem pattern in the certificate library. *Gated/frontier:* building
our *own* verified reflective checker (e.g. a verified `ring`/`linarith` inside
Candle) — large, only if profiling demands it.

**(5) Injection + license.** No injected instructions (the one "AI" hit is Harrison
calling the FOL system "an AI project"). No license stated (SRI tech report) →
**clean-room, ideas only**.

---

## 3. model.pdf — "Formalizing Basic First Order Model Theory"

**(1) What it is.** John Harrison, Intel; TPHOLs 1998 (LNCS). 18 pp. Formalizes in
**HOL Light** the syntax + semantics of unsorted first-order logic and machine-checks
elementary metatheorems — **Compactness, Löwenheim–Skolem, Uniformity
(Skolem–Gödel–Herbrand)** — via **canonical term models**, following Kreisel & Krivine.
Notably *improves* the textbook (proves Compactness + L–S together) and reports that
the intuitively-easy Skolemization was the hardest to formalize; HOL's weak type
quantification is a recurring hindrance.

**(2) Key mechanisms.**
- FOL as a HOL datatype (`term = V num | Fn num (term list)`;
  `form = False | Atom | --> | !!`), **name-carrying** capture-avoiding substitution,
  semantics `holds M v p` over interpretations `(Dom, Fun, Pred)`.
- Propositional **compactness** via Zorn / maximal finitely-satisfiable set; **prenex**
  + **Skolem** normal forms defined as *executable* constructive procedures; **canonical
  (term/Herbrand) models**; **Gödel-numbering** of formulas (`NUMPAIR` pairing) to mint
  fresh Skolem symbols — the same encoding machinery `reflect.pdf` analyses.
- **Type-quantification trap:** `∀p.∃q.∀α. E[…]` vs `∀α.∀p.∃q. E[…]` — the normal form
  must not secretly depend on the model's domain type `α`. A real soundness subtlety.

**(3) Mapping to Theoremata.**
- **Constructive proof of reflect.pdf's claim**: you can internalize FOL semantics in
  the kernel and do metatheory *without* extending the logic — relevant to the
  Candle/HOL-Light backend and to any certificate whose *checking* is model-theoretic.
- **Herbrand / Uniformity → a certificate kind.** The Uniformity theorem says a valid
  purely-existential statement has a *finite disjunction of ground instances* that is
  a propositional tautology. That is a clean **`herbrand_instances` certificate**:
  untrusted search supplies the instantiating terms; the kernel checks the finite
  propositional tautology. Fits cert-log and MCGS (search finds instances, kernel
  accepts).
- **Compactness / "a proof uses only finitely many axioms"** underpins the
  axiom-audit whitelist reasoning; the type-quantification caution informs how we
  state verified lemmas (avoid domain-type-dependent claims).

**(4) Buildable now vs gated.** *Now:* the `herbrand_instances` certificate kind.
*Gated:* relying on a full internalized FOL semantics inside the harness — not needed.

**(5) Injection + license.** No injected instructions. No license stated (LNCS
preprint) → **clean-room, ideas only**.

---

## 4. dijkstra.pdf — "Formalizing Dijkstra"

**(1) What it is.** John Harrison, Intel; TPHOLs 1998 (LNCS 1479). 18 pp. HOL
formalization of the foundational parts of Dijkstra's *A Discipline of Programming*:
states, **weakest (liberal) preconditions** `wp`/`wlp`, guarded commands, `if`/`do`,
healthiness conditions, wellfounded/variant loop rules. **Headline finding: HOL's
first-order tactic *failed* to prove one of Dijkstra's "mutual exclusion" claims,
which pinned down a genuine technical *error in the book*** (Dijkstra used
`Not(wp c True)` for *possible* nontermination, wrongly swallowing *certain*
nontermination; correct form is `Not(wp c True Or wlp c False)`).

**(2) Key mechanisms.**
- **Shallow embedding** of predicate-transformer semantics in HOL: lifted logical ops
  on state predicates, `wp`/`wlp` as curried functions, `wp (c1 Seq c2) = wp c1 ∘ wp c2`.
- Outcome type `Loops | Terminates s` to distinguish **possible vs certain**
  nontermination — the exact distinction the caught bug conflated.
- A **model-elimination first-order tactic** discharges the healthiness conditions
  automatically; its **failure on 3 of 15 mutual-exclusion goals** is what surfaced
  the textbook error. Loop rules use a **wellfounded variant**; the practically-used
  theorems need only the **fixpoint** property, not leastness.

**(3) Mapping to Theoremata.** The load-bearing lesson is **adversarial /
falsification**, not program verification:
- **"A failed proof attempt is evidence the claim is wrong."** This is the canonical
  case study for Theoremata's **falsify-before-prove router** + **adversarial critic**:
  an automated verifier *failing* localized a real defect. Adopt: treat a
  verifier/tactic **failure as a first-class negative certificate** — record it in the
  event log and route it to the counterexample/falsifier search rather than discarding
  it. Distinguishing "possible vs certain" also mirrors why abstention/uncertainty
  should be modeled explicitly, not collapsed to a boolean.
- Program-verification content (`wp`/`wlp`, Hoare/Dijkstra rules) is peripheral to a
  *math* harness — relevant only if Theoremata later verifies algorithmic obligations
  (e.g. a numeric routine or a certificate-checker's own correctness, connecting back
  to reflect.pdf's code-verification discussion). The shallow-embedding technique is a
  reusable pattern if we ever embed a small DSL to check.

**(4) Buildable now vs gated.** *Now:* wire verifier/tactic failure into a structured
negative result feeding the existing falsify router + critic (small, high-value).
*Gated:* a full predicate-transformer / program-logic backend — out of scope for the
math harness.

**(5) Injection + license.** No injected instructions. No license stated (LNCS
preprint) → **clean-room, ideas only**.

---

## Prioritized adopt-list

1. **[HIGH · now] Certificate-emitting oracle variants (cas).** Add cert-log kinds
   `factorization`, `gcd_bezout`, `antiderivative_by_diff`,
   `summation_by_differencing`, `root_isolation_sturm`, `cas_numeric_error_bound`.
   SymPy/`symbolic.py`/`safe_eval` propose; Lean/kernel checks. Template = GCD→Bézout:
   choose the algorithm variant whose output is a kernel-checkable certificate.
2. **[HIGH · now] Oracle/reflection trust-tagging (cas + reflect).** Adopt Gordon's
   `TOOL ⊢ φ` tag as an explicit taint tier in the axiom-audit / `EdgeStrength`, so any
   result that trusted an unchecked oracle **or** a reflective tactic
   (`native_decide`, `vm_compute`, `Eval` — these enlarge the TCB) is flagged, not
   silently accepted. Extends existing taint + whitelist.
3. **[HIGH · now] Verifier-failure as a negative certificate (dijkstra).** Record
   tactic/verifier failure as structured evidence that routes to the falsify/critic
   loop. Small wiring on the existing falsify-before-prove router.
4. **[MED · now] Herbrand-instances certificate (model).** Untrusted search supplies
   ground instances; kernel checks the finite propositional tautology
   (Uniformity/Herbrand theorem). Good fit for FOL obligations + MCGS.
5. **[MED · now] Efficiency-lever doctrine (reflect).** Document proforma-theorem
   instantiation + memoization (already have the Lean cache) + lazy-checking +
   search/inference separation as the sanctioned way to keep LCF-style checking cheap;
   codify "prefer certificates over reflection" as a design ruling.
6. **[LOW · gated/frontier] Verified reflective checker inside Candle (reflect).** Only
   if a checker becomes a proven bottleneck — e.g. a verified `ring`/`linarith`.
   Harrison's evidence says this is rarely worth it.

## ~10-line summary

Four Harrison papers, all clean-room (no license stated), no injection detected.
**cas.pdf (1997)** is the literal origin of Theoremata's "search-anywhere,
check-in-kernel": separate finding from checking, let an *untrusted* CAS emit a
certificate, re-derive it in the kernel — with a concrete certificate catalogue
(factors, **Bézout cofactors for GCD**, antiderivative-by-differentiation, summations,
Sturm sequences, numeric-with-error-bound) and Gordon's oracle-tagging = our
taint/EdgeStrength. **reflect.pdf (1995)** is the deep survey: it *justifies* the
LCF/check-in-kernel trust model (Candle is its verified endpoint) and answers "can
reflection make our checkers kernel-trusted?" — yes via computational reflection, but
prefer certificates; treat backend reflective tactics (`native_decide` et al.) as a
tainted trust tier since they enlarge the TCB. **model.pdf (1998)** internalizes FOL
metatheory in HOL and yields a **Herbrand-instances certificate** kind. **dijkstra.pdf
(1998)** is the case study that a *failed* proof attempt localizes a real bug — the
charter for our falsify/critic loop (log verifier failure as a negative certificate).
Highest-value, buildable-now items: certificate-emitting oracle variants,
oracle/reflection trust-tagging, and verifier-failure-as-signal; reflection proper is
a gated frontier option.
