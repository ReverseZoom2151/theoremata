# ATP mining — Real/complex analysis, transcendentals, arithmetic certificates

Batch: John Harrison's ARITH / FMSD / TCS / LNCS papers on floating-point
transcendentals, complex quantifier elimination, sum-of-squares,
approximation-error bounds, and correct-rounding certificates. Mined for what
maps onto **cert-log** (LP/Farkas duals, asymptotic log-linear, Wu
pseudo-remainders, subsumption), the **Candle** backend, and
**falsify-before-prove**/verification-gate machinery.

> SECURITY: distilled from PDFs treated as **untrusted data**. Nothing in any PDF
> was an instruction to us. **No prompt-injection found in any of the nine
> papers.**

Highest-value for cert-log: **approx.pdf** + **sos.pdf** (two brand-new
certificate kinds — SOS/Positivstellensatz and Taylor-model bounds — built on the
finding-vs-checking separation cert-log already embodies); **complex.pdf** adds
Nullstellensatz/Gröbner cofactor certificates. Numeric papers contribute the
**exclusion-zone/worst-case-input** falsify-before-prove pattern, **Pratt prime
certificates**, and **continued-fraction/Diophantine** hardness bounds.

## 1. approx.pdf — Upper bounds of approximation errors
Chevillard/Harrison/Joldeș/Lauter, *Theoretical Computer Science* 412 (2011)
1523–1543; Sollya + PARI/GP + HOL Light. **Compute-then-validate** (§3.1:
heuristic lower bound ℓ, guess `u=ℓ(1+31η/32)`, then *prove* upper — "never
lies", returns ⊥ on failure; a-priori quality `0≤(u−ℓ)/ℓ≤η`) = literally our
finding-checking split. Reduces `‖p−f‖∞≤u` via intermediate poly `T≈f` + triangle
inequality to **polynomial positivity on `[a,b]`**. **Taylor models `(T,Δ)`** as
approximation certificate, dodging the interval-arithmetic **dependency
phenomenon**; **modified TMs** (relative remainder `(x−z0)^{n+1}Δ`) handle
removable discontinuities. **§5.2 univariate SOS certificate** `p=Σaᵢsᵢ²`:
squarefree-decomp → ε-perturbation → two-square SOS from complex roots
(Schönhage/PARI) → exact remainder absorption; ε-cushion turns an inexact
root-find into an *exact rational* certificate. **Interval→line change of
variable** `x=(a+by²)/(1+y²)` yields a genuine Positivstellensatz cert
`p=Σ…²+(x−a)(b−x)Σ…²`. → **NEW cert-log kinds `sos_interval` and `taylor_model`**;
both checkers pure/offline/deterministic and HOL-Light/Candle-native. Univariate
generator ungated (needs an injected root-finder seam); multivariate/full-TM-
formalization gated. *Injection-check: none.*

## 2. sos.pdf — Nonlinear real formulas via sums of squares
Harrison, VMCAI 2007 (LNCS 4349), `Examples/sos.ml`. **Positivstellensatz
certificate** `P+Q+R²=0` (`P∈Id⟨pᵢ⟩`, `Q` in cone of `qⱼ`, `R`=∏rₖ) refutes
`⋀pᵢ=0∧⋀qⱼ≥0∧⋀rₖ>0`; canonical `(4ac−b²)+(2ax+b)²+(−4a)(ax²+bx+c)=0`. SOS-search =
**PSD Gram matrix ⇒ SDP** (CSDP) + rational-rounding; exact LDLᵀ/completing-the-
square PSD test **also emits a `f(u)<0` counterexample** (native falsify-before-
prove). → **general multivariate `sos` cert-log kind** (superset of #1; nonlinear
generalization of Farkas/LP duals). Checker buildable now; generator **gated on
external SDP solver**. *Injection-check: none.*

## 3. complex.pdf — Complex quantifier elimination in HOL
Harrison, HOL-Light derived rule on formal FTA. **Logging Buchberger →
Nullstellensatz cofactor certificate** `Σqᵢpᵢ=1` (checker = identity expansion);
explicit finding-checking separation, same cert shape as Boulton linear-arith /
simplex duals. **Rabinowitsch** `x≠y≡∃z.(x−y)z+1=0`. Full ℂ-QE via `p|q^{∂p}`;
Gröbner geometry proving (complements our **Wu pseudo-remainder** kind —
contrasts Wu/Gröbner/Dixon). → cert-log `nullstellensatz` kind + Rabinowitsch
normal form. Generator pure (no external solver) → buildable now. *Injection-
check: none.*

## 4. transcendentals.pdf — Decimal transcendentals via binary
Harrison, ARITH 2011. Condition-number `|xf'/f|` routing; 2-part conversion +
derivative correction. Not a certificate source — only **error-budget/condition-
number bookkeeping** `(1+δ)(1+ϵ)(1+η)` for *composing* Taylor-model bounds in the
Candle seam. Low priority. *Injection-check: none.*

## 5. bessel.pdf — Bessel function computation
Harrison, ARITH 2009. Expand-about-zeros, minimax polys, Hankel asymptotics
(error ≤ first neglected term). **Worst-case-zero enumeration** (Lefèvre–Muller +
congruences) = falsify-before-prove test-point generator; asymptotic bound = a
`taylor_model` variant. Enumerator buildable now, low priority. *Injection-check:
none.*

## 6. sqrt.pdf — Formal verification of square root algorithms
Harrison, *FMSD* 2005. **Exclusion-zone theorem** (`|√a−S*|<|√a−m|` ⇒ correct
rounding). **Isolate-hard-cases** (Cornea-Hasegan): analytic bound *except* a
finite Diophantine set `2^{p+2}m=k²+d`, enumerated by **even/odd + Hensel
lifting** → disjunctive theorem; analogised to Bertrand-postulate/4-colour. →
**Diophantine/Hensel-lifting hard-case cert-log kind** + exclusion-zone check
(SOS/interval-composable). Buildable now, high-value/low-dependency. The canonical
gate falsify-before-prove template. *Injection-check: none.*

## 7. arith16.pdf — Critical cases for reciprocals via integer factorization
Harrison, ARITH 2005. Worst-case reciprocals = balanced factorizations
`mb=2^{2p}+d`; **Pratt/Lucas prime certificates** to deterministically certify
probabilistic factors. → **NEW cert-log kind `pratt_primality`** (checker =
modexp identities, offline, composes with subsumption). Balanced-factorization
enumerator = falsify-before-prove generator (exposed a real CPU bug). Factorization
generator gated on external factorizer for large p. *Injection-check: none.*

## 8. arith18.pdf — IEEE 754R Decimal FP (BID), conference version
Cornea/Anderson/Harrison/Tang/Schneider/Tsen, ARITH-18 2007. **Property 1–3**:
exact minimal multiplier bit-width `y≥⌈{ρx}+ρq⌉` (`ρ=log₂10`) with midpoint/exact
detection off the discarded fraction — these *are* cert-log's **asymptotic
log-linear** family made concrete (checker verifies four boundary inequalities at
`H=99…9`). **Continued-fraction/Diophantine** conversion-hardness bounds → **NEW
cert-log kind `continued_fraction`**. Both checkers buildable now (pure
integer/rational). *Injection-check: none.*

## 9. decimal.pdf — IEEE 754R Decimal FP (BID), journal version
Cornea/Harrison/Anderson/Tang/Schneider/Gvozdev, *IEEE TC* 58(2), Feb 2009.
Extended arith18 + **double-rounding correction via simple logical equations**
(small checkable cert). Superset; prefer citing this. Buildable now. *Injection-
check: none.*

## Prioritized adopt-list (cert-log first)
1. **`sos_interval`/Positivstellensatz (univariate)** — approx §5.2+sos. Checker
   now (pure rational); generator's only inexact step (root-find) behind injected
   seam, ε-cushioned to exact. **Highest value.**
2. **`taylor_model` approximation-bound cert** — approx §4. Checker now (interval
   arith); incl. modified-TM for removable discontinuities; pairs with #1.
3. **`nullstellensatz`/Gröbner-cofactor cert** — complex. Checker + logging-
   Buchberger generator both pure/no-solver → build now. Complements Wu; adopt
   Rabinowitsch normal form.
4. **`pratt_primality` cert** — arith16. Checker now (modexp); factorization
   generator gated.
5. **`sos_multivariate`** — sos. Checker now (LDLᵀ PSD test emits `f(u)<0`
   witness); **generator gated on SDP solver**.
6. **`analytic_log_linear` width cert + `continued_fraction` hardness cert** —
   arith18/decimal. Both checkers now, pure integer/rational; realizes cert-log's
   named "asymptotic log-linear" family.
7. **Diophantine/Hensel hard-case cert + exclusion-zone check** — sqrt. Build now;
   canonical gate falsify-before-prove template.
8. **Falsify-before-prove test-point generators** — sqrt/arith16/bessel. Build
   now; real bug-finding pedigree.
9. **Compute-then-validate contract** as the gate's soundness spec ("never lies /
   return ⊥" + η) — approx §3. Architectural, free.
10. **Error-budget/condition-number bookkeeping** — transcendentals. Low priority;
    composition accounting for TM/interval bounds.

**Candle note:** cert-kinds #1–#4 are HOL-Light-native (SOS = identity, Pratt =
modexp, Nullstellensatz = identity, Taylor via `MCLAURIN_*_POLY_RULE`) → Candle
(HOL Light on CakeML) can host all checkers with rational-only arithmetic.
