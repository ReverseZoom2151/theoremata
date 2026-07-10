# HOL Light Manuals — cert-log / Candle mining report

Sources (both by John Harrison et al., `github.com/jrh13/hol-light`, 20 May 2026 / 29 Oct 2025 revisions):

- `atp/reference.pdf` — **The HOL Light System REFERENCE**, 946pp. An alphabetical dictionary of *essentially all pre-defined ML identifiers* (functions, term/type constructors, forward inference rules, tactics/tacticals) in `Synopsis / Description / Failure / Example / Comments / See also` form, generated from the same DB as the online `help` system. Chapter 2 (p.827+) lists the pre-proved theorem library grouped by subject (logic, arithmetic, reals, integers, sets, iterated ops, cartesian powers).
- `atp/tutorial.pdf` — **HOL Light Tutorial** (Harrison + Juneyoung Lee), 230pp. A graded, example-driven "quick start". Directly relevant chapters: **§9 HOL's number systems** (9.1 arithmetical decision procedures, 9.2 nonlinear reasoning, 9.3 quantifier elimination), §18 number theory, §19 real analysis, §25 custom inference rules (Knuth–Bendix/LPO), **§26 linking external tools** (the oracle-plus-checker pattern).

---

## 1. Decision-procedure / automated-tactic inventory

For each: what it does, *what its underlying certificate is*, and the cert-log mapping. "Rule" = `term -> thm`, "Tac" = tactic form, "Conv" = `term -> thm` equational.

### Linear arithmetic — maps to **LP/Farkas** cert-log
| Identifier | Domain | Method | Checkable certificate? |
|---|---|---|---|
| `REAL_ARITH` / `REAL_ARITH_TAC` / `ASM_REAL_ARITH_TAC` | ℝ | Normalize (kill `max`/`min`/`abs`/conditionals via case-split, put atoms in `p(x) ⋈ 0` form via `REAL_POLY_CONV`), then **Fourier–Motzkin** linear refutation | **YES** — a *Positivstellensatz refutation* object (see below); linear ⇒ a Farkas/LP combination |
| `ARITH_RULE` / `ARITH_TAC` / `ASM_ARITH_TAC` | ℕ | Basic algebraic normalization + linear inequality reasoning; also handles `DIV`/`MOD`/`EXP` by constants | Same refutation machinery; ℕ discreteness only *partially* exploited (documented weakness: `~(2m+1=2n)` fails) |
| `INT_ARITH` / `INT_ARITH_TAC` | ℤ | As above over ℤ | Same |

Core insight (`GEN_REAL_ARITH`, `REAL_LINEAR_PROVER`): `REAL_ARITH ≡ GEN_REAL_ARITH REAL_LINEAR_PROVER`. The wrapper hands the prover a triple of theorem lists — equalities `A_i ⊢ p_i = 0`, non-strict `B_j ⊢ q_j ≥ 0`, strict `C_k ⊢ r_k > 0` — and a *reconstruction function*. The prover must return a **`positivstellensatz`** value: "a representation of how to add and multiply equalities/inequalities from the list to reach a trivially false `0 > 0`." **This `positivstellensatz` datatype is exactly a cert-log proof object** and the linear case is our LP/Farkas kind.

### Nonlinear equational (ideal membership) — maps to **Nullstellensatz / Gröbner** cert-log
| Identifier | Domain | Method | Certificate |
|---|---|---|---|
| `RING` (generic) | any integral domain/semiring | Parameterized ring procedure; needs an integrality thm + optional field thm + `POLY_CONV` | Produces the proof by ideal-membership cofactors |
| `REAL_RING`, `NUM_RING`, `INT_RING` | ℝ/ℕ/ℤ | Instantiations of `RING`; prove any Boolean combination of equations valid in all integral domains | **YES** |
| `ideal_cofactors` / `real_ideal_cofactors` / `int_ideal_cofactors` | — | Given `[p1..pn]`, `p`, computes cofactors `[q1..qn]` with `p = Σ pi*qi` as an algebraic identity, **via a Gröbner-basis procedure**. Explicit ideal-membership certificate. | **YES — the literal Nullstellensatz cofactor certificate** |
| `REAL_IDEAL_CONV` / `INT_IDEAL_CONV` / `RING_AND_IDEAL_CONV` | — | Return `⊢ p = q1*p1 + … + qn*pn` (the cofactor identity *as a theorem*) | **YES** |
| `REAL_FIELD` / `INT_ARITH`+ | ℝ | `REAL_RING` preceded by case-splitting `inv(t)`: for each `t`, split `t=0` vs `t*inv t=1`; on failure also tries `REAL_ARITH` | Ring cert + linear side conditions |

This is the highest-ROI find: HOL Light already emits **explicit, kernel-checkable cofactor certificates** (`p = Σ qi·pi`) that map 1:1 onto our Nullstellensatz cert-log kind. `REAL_IDEAL_CONV` even returns the certificate *as a theorem*, ready to re-check.

### Nonlinear inequalities (SOS / Positivstellensatz) — maps to **SOS** cert-log (toolchain-gated)
- `SOS_RULE`, `REAL_SOS`, `INT_SOS`, `PURE_SOS`, `SOS_CONV` — **not in core build**; live in `Examples/sos.ml` (Parrilo 2003 method). Plug a **semidefinite-programming** prover into `GEN_REAL_ARITH` in place of `REAL_LINEAR_PROVER`. `REAL_SOS` "searches for real Nullstellensatz certificates involving sums of squares"; `SOS_CONV` returns an explicit SOS decomposition `p = Σ cᵢ·mᵢ²`; `PURE_SOS` proves `p ≥ 0` by pure SOS. **Requires external CSDP** to *find* the certificate — but HOL then *checks* it through the kernel. Certificate = SOS/Positivstellensatz multipliers ⇒ our SOS + Positivstellensatz cert-log kinds.

### Number-theoretic divisibility — candidate **NEW cert-log kind** (or reuse Nullstellensatz-over-ℤ + gcd)
- `NUMBER_RULE`/`NUMBER_TAC` (ℕ), `INTEGER_RULE`/`INTEGER_TAC` (ℤ) — "partly heuristic" provers for Boolean combinations of `divides`, `coprime`, `gcd`, congruences `(x==y)(mod n)`; also some existentials (linear-congruence solvability, 2-number CRT). Built on `int_ideal_cofactors` + Bézout/gcd witnesses. Certificate is reconstructible (cofactors + gcd cofactors) but not surfaced as a single object → **worth defining a "divisibility/congruence (Bézout-cofactor) certificate" cert-log kind**.

### Quantifier elimination — mostly toolchain-gated
- `COOPER_RULE`/`INT_COOPER`/`COOPER_CONV` (`Examples/cooper.ml`) — **Presburger** (Cooper 1972) over ℤ/ℕ, full quantifier alternation + divisibility; returns quantifier-free equivalent. Certificate = the QE trace; complete but slow.
- `REAL_QELIM_CONV` (`Rqe/` — McLaughlin & Harrison 2005) — real closed field QE (a proof-producing Cohen–Hörmander, *not* CAD); handles quantifier alternation (`∀x. 0≤x ⇒ ∃y. y²=x`). Minutes-slow; produces a checkable HOL proof.
- Complex-number QE in the `Complex/` subdirectory (algebraically-closed-field theory).

### First-order / logical — no independent checkable certificate (kernel-replayed)
- `MESON`/`MESON_TAC`/`ASM_MESON_TAC` — model-elimination first-order search (iterative deepening); purely logical, exploits only equality + logical primitives, needs supporting lemmas passed explicitly.
- `METIS`/`METIS_TAC` — Metis (better on harder FO problems).
- `LEANCOP`/`NANOCOP` (+`_TAC`) — leanCoP / nanoCoP connection provers.
- `SIMP_TAC`/`REWRITE_TAC`/`GEN_REWRITE_*` — conditional/ordered rewriting; `TAUT` for propositional.
These reconstruct a proof through the kernel (so the *theorem* is trusted) but do **not** emit a compact standalone certificate to log — map to a generic "kernel-replay" evidence record, not a decision-procedure cert.

### Supporting normalizers (useful primitives to port)
`REAL_POLY_CONV` (canonical multiplied-out polynomial normal form), `REAL_POLY_{ADD,MUL,NEG,SUB,POW}_CONV`, `NUM_REDUCE_CONV` / `REAL_RAT_REDUCE_CONV` (ground reduction with arbitrary-precision rationals — "constants manipulated by proof internally, no machine-int overflow"), `SEMIRING_NORMALIZERS_CONV`.

---

## 2. Kernel primitives + Trusted Computing Base (for Candle / axiom auditor)

HOL Light is an **LCF-style** system: the only way to build a value of type `thm` is via a small trusted kernel; everything above is derived and cannot forge theorems.

- **10 primitive inference rules** (the manual repeatedly tags entries "one of HOL Light's 10 primitive inference rules"): `REFL`, `TRANS`, `MK_COMB`, `ABS`, `BETA`, `ASSUME`, `EQ_MP`, `DEDUCT_ANTISYM_RULE`, `INST`, `INST_TYPE`. (All work modulo α-conversion **except `BETA`**; `BETA_CONV` generalizes it.)
- **Primitive definitional principles** (conservative, consistency-preserving): `new_basic_definition` (`c = t`, with the type-variable-occurrence restriction that blocks the `trivial <=> !x y:A. x=y` inconsistency) and `new_basic_type_definition` (new type in bijection with a nonempty subset, returns the `mk∘dest=id` / `P r <=> dest(mk r)=r` pair). All higher-level `define`, `define_type`, `new_inductive_definition`, `new_specification` reduce to these.
- **3 mathematical axioms** on top of the logic: `ETA_AX` (η/extensionality), `SELECT_AX` (Hilbert choice ε), `INFINITY_AX` (an infinite type `:ind`). Classical math is otherwise definitional.
- **Trust escape hatches (must be audited)**: `mk_thm(asms,c)` and `new_axiom tm` both fabricate arbitrary theorems (`mk_thm([],‘F‘)` → `⊢ F`); `CHEAT_TAC` is the tactic form. `new_constant` + `new_axiom` is the un-audited path.
- **Built-in auditor**: `axioms()` returns every theorem asserted via `mk_thm`/`new_axiom` — **this is precisely the hook our axiom auditor / resource guard should call** to confirm a proof used no cheats. Candle (HOL Light kernel on CakeML) gives the same TCB with a verified-down-to-machine-code checker; our auditor should (a) assert `axioms()` contains only the 3 expected axioms, (b) reject any proof whose trace touches `mk_thm`/`CHEAT_TAC`/`new_axiom`.

**Relevance to cert-log**: the LCF discipline *is* the trust story — a logged certificate only needs the ~10 primitive rules + 3 axioms to be believed. Re-checking a Nullstellensatz cofactor identity or an SOS decomposition reduces to `REAL_RING`/`REAL_POLY_CONV`, themselves derived from the kernel, so a Candle backend can independently replay any cert-log entry we import.

---

## 3. Buildable-now vs toolchain-gated (honest)

**Buildable now (ideas/algorithms, no HOL install needed):**
- Nullstellensatz/Gröbner **cofactor certificate** format `p = Σ qi·pi` — clean-room re-implement the `ideal_cofactors` contract; verifier is trivial polynomial-identity check. Highest ROI.
- LP/Farkas **Positivstellensatz refutation** object (the triple-of-lists + add/multiply combination reaching `0>0`) — directly models our LP/Farkas kind and generalizes it.
- Presburger (Cooper) and the divisibility/congruence Bézout-cofactor certificate — algorithmic, implementable in Rust/Python.
- Polynomial normal-form (`REAL_POLY_CONV`) + exact-rational ground reduction — needed infrastructure for several cert kinds.
- The **oracle+checker pattern** (§26): let an untrusted engine *find* the answer, verify cheaply. Directly aligns with cert-log philosophy.

**Toolchain-gated (need external solvers / a HOL install):**
- `REAL_SOS`/`SOS_CONV`/`PURE_SOS` — need a **CSDP** (semidefinite) solver to *produce* the SOS multipliers (checking is cheap; we can accept SOS certs from any SDP source).
- `REAL_QELIM_CONV` (`Rqe/`), complex QE — heavy, minutes-slow; port the *certificate format*, not a live dependency.
- `PRIME_CONV` primality — uses an external **factoring** engine (PARI/GP) then builds a **Pratt certificate** checked in HOL ⇒ maps to our Pratt cert-log kind; the checker is buildable now, the factor-finder is the gated part.
- `MESON`/`METIS`/`LEANCOP`/`NANOCOP` — logical search; no compact cert to adopt (replay-only).
- Anything requiring the actual OCaml HOL Light / Candle build to *run* (this environment has no OCaml/Lean/lake).

---

## 4. Prioritized adopt-list (ranked by cert-log ROI)

1. **Nullstellensatz cofactor certificate** (`ideal_cofactors` / `REAL_IDEAL_CONV`): explicit `p = Σ qi·pi`, Gröbner-found, trivially re-checkable. Direct 1:1 with our Nullstellensatz kind; `REAL_IDEAL_CONV` returns it *as a theorem*. **Do first.**
2. **`positivstellensatz` refutation datatype** (`GEN_REAL_ARITH`/`REAL_LINEAR_PROVER`): unify our LP/Farkas kind under HOL's add/multiply-to-`0>0` representation; gives a clean extension point where linear → SOS is just swapping the sub-prover.
3. **SOS / Positivstellensatz certificate** (`Examples/sos.ml`, `SOS_CONV`): accept SOS multiplier certs (from any SDP solver) for nonlinear inequalities; checking is a polynomial identity + squares-nonneg. Strengthens our SOS/Taylor-model story.
4. **Divisibility/congruence (Bézout-cofactor) cert** — *new cert-log kind* generalizing `NUMBER_RULE`/`INTEGER_RULE` (divides/coprime/gcd/mod, incl. CRT and linear-congruence solvability). Good coverage of number-theory goals.
5. **Presburger (Cooper) QE trace** (`Examples/cooper.ml`) for ℤ/ℕ with alternation + divisibility.
6. **Pratt primality certificate** via the `PRIME_CONV` external-factor-then-check pattern — maps to existing Pratt cert-log kind.
7. **axioms() auditor contract** — port the "assert only {ETA,SELECT,INFINITY}, reject mk_thm/CHEAT_TAC/new_axiom" check into our axiom auditor / resource guard; this is the trust anchor for every imported cert and for the Candle backend.
8. **RCF QE certificate format** (`Rqe/`, Cohen–Hörmander) — lower priority (slow, complex), adopt format not dependency.

---

## 5. Injection & license

- **Injection scan: CLEAN.** Full-text scan of both manuals for instruction-injection patterns ("ignore previous", "you are", "system prompt", role directives, etc.) found only benign technical prose and installation instructions ("If you are using Windows…"). **No prompt-injection detected.** All PDF content was treated as untrusted data and only characterized, never executed.
- **License: not stated in the manuals.** No explicit copyright/license line appears in either front matter (the only "GNU Public License" string refers to bundled *Maxima/CLISP* banners, not HOL Light). Preface notes the reference is *derived from the HOL88 REFERENCE* (Cambridge/SRI/DSTO, managed by Mike Gordon). HOL Light source is generally BSD-2-Clause, but since no license is asserted in these documents, **treat as clean-room: adopt ideas/algorithms/certificate *formats* only; do not copy manual or source text.**

---

## Summary (~10 lines)

- Two Harrison manuals: `reference.pdf` (946pp alphabetical ML-identifier dictionary + theorem library) and `tutorial.pdf` (230pp graded examples; §9/§26 most relevant).
- HOL Light's automation is a graded ladder of decision procedures, several of which **already emit explicit, kernel-checkable certificates** — the top prize for cert-log.
- **Nullstellensatz cofactor certs** (`ideal_cofactors`/`REAL_IDEAL_CONV`, Gröbner) give `p = Σ qi·pi` directly; **`positivstellensatz`** is a first-class refutation datatype (`GEN_REAL_ARITH`), linear via Fourier–Motzkin (`REAL_ARITH`) and nonlinear via SDP-backed **SOS** (`Examples/sos.ml`).
- `REAL_RING`/`NUM_RING`/`INT_RING`/`REAL_FIELD` = ring/field decision procedures; `NUMBER_RULE`/`INTEGER_RULE` = divisibility/congruence (candidate new cert-log kind); `COOPER`/`REAL_QELIM` = Presburger/RCF quantifier elimination (gated).
- `MESON`/`METIS`/`LEANCOP`/`NANOCOP` = first-order search with no compact cert (kernel-replay only).
- **TCB**: LCF kernel = 10 primitive rules + 2 definitional principles + 3 axioms (ETA/SELECT/INFINITY); `mk_thm`/`new_axiom`/`CHEAT_TAC` are the only escape hatches, and `axioms()` is the built-in auditor hook — directly reusable for our axiom auditor and the Candle backend.
- Buildable now: cofactor/LP/Positivstellensatz/Bézout/Presburger/Pratt certificate *formats* + polynomial normalizers + the oracle-plus-checker pattern. Gated: live SOS (needs CSDP), RCF QE, factoring, and anything needing an OCaml/Candle build.
- Adopt order by ROI: (1) Nullstellensatz cofactors, (2) positivstellensatz datatype unifying LP/Farkas, (3) SOS certs, (4) divisibility cert kind, (5) Presburger, (6) Pratt, (7) axioms() auditor contract.
- Injection: CLEAN. License: unstated in the manuals → clean-room, ideas/formats only.
