# ATP mining — Harrison, floating-point + numerics (`fparith`, `tang`, `poly`)

Scope: assess three John Harrison papers for **new cert-log certificate kinds** and
**new falsify generators** for Theoremata's verification-first numerics stack
(`components/verify/python/theoremata_tools/cert_*.py` +
`components/tools/python/theoremata_tools/falsify_hardcase.py`).

**Security note.** All three PDFs were extracted with pypdf and read in full.
**No prompt-injection or embedded instructions detected** in any of the three —
they are ordinary academic FP-verification papers (definitions, HOL theorems,
error analyses, references). Content treated as untrusted throughout; nothing in
them was executed or followed.

**License note.** None of the three carries an explicit open-source or reuse
license. `fparith` and `poly` are Springer LNCS papers (typeset `LLNCS2E`;
TPHOLs'99 / LNCS 1690 and TPHOLs'97 / LNCS 1275 respectively); `tang` is a
Kluwer Academic Publishers journal article (Journal of Automated Reasoning; the
extraction shows a "© Kluwer" line). => **Treat as clean-room.** The *mathematics*
(ulp definitions, rounding-error bounds, Sturm's theorem, squarefree
decomposition, minimax-error method) is public-domain mathematical fact and may
be reimplemented freely; the *expression* (Harrison's HOL text/scripts) must not
be copied. Reference the papers as method sources; write all code and prose
independently.

---

## 1. `fparith.pdf` — "A machine-checked theory of floating point arithmetic"

**(1) What it is.** Harrison, TPHOLs'99 (Springer LNCS 1690), 18pp. The generic
IA-64/Merced HOL Light floating-point library: a format-parametric theory
(`format (E,p,N)`), canonical decoding, **ulp** defined via binades, the four
IEEE rounding modes, and a large suite of medium-level lemmas about rounding,
exact operations and underflow — the foundational layer under the other two
papers.

**(2) Key mechanisms.**
- **Verified ulp + rounding-error bounds.** `ulp(x) = 2^binade(x)/2^N`; the
  central error theorems: `|error Nearest x| <= ulp x / 2`, directed modes
  `< ulp x`, and the generic relative bound
  `|error rc x| <= mu(rc) * |x| / 2^(p-1)` when `normalizes x`.
- **Exact-operation lemmas.** Sterbenz-style exact subtraction of nearby values
  (`a/2 <= b <= 2a => b-a` representable), its `p+k`-precision generalisation,
  and fma cancellation (product-in-two-parts: `a*b - round(a*b)` representable).
- **Automated error analysis.** A HOL tool that propagates `(magnitude-bound b,
  error e)` triples through chains of fma operations to derive an absolute or
  relative error bound for a whole polynomial evaluation automatically
  (`zorbigger` for the no-underflow side-condition).
- **Explicit exact rounding.** `ROUND_CONV` rounds a *rational* `p/q` in a given
  format/mode and returns a proof — e.g. `round (10,11,12) Nearest (22/7) =
  1609/512`. Pure exact-rational.
- **Exclusion-zone / hard-case reduction (§8.3).** For sqrt-type proofs the
  correctness holds for *all but* a number-theoretically-isolated set of values
  parametrised as `2^(e_i+2n) k_i`; one checks only a small representative set
  (say `n=0`) and **extrapolates over the whole even/odd parity class via a
  rounding scaling theorem** `round(2^n x) = 2^n round(x)`.

**(3) Mapping to cert-log / falsify.**
- **NEW cert kind `fp_rounding` (buildable).** No existing kind models IEEE
  rounding at all (sos/bernstein/taylor_model/nullstellensatz/wu/pratt/farkas
  cover positivity, enclosure, ideal, geometry, primality, LP). A `fp_rounding`
  proof-log would carry a format `(p,N)`, a mode, a rational input `x`, the
  claimed `round(x)` and an error/ulp bound; the reference checker recomputes
  `round` and `ulp` in **exact `Fraction`** (a `ROUND_CONV` analog — rounding a
  rational to a binary format is exact rational arithmetic) and re-verifies
  `|round(x)-x| <= mu*ulp` and representability. This is the honest offline
  stand-in whose toolchain-gated upgrade is a HOL-Light `fp` proof.
- **NEW cert kind `fp_error_bound` (buildable).** Serialise Harrison's automated
  fma-chain error analysis: steps are `(bound, abs_err|rel_err)` triples with a
  per-operation propagation rule; checker re-propagates with exact-rational
  interval arithmetic and confirms the final bound. Directly certifies the
  "numerics-screen" obligations Theoremata already refuses to *prove* — this
  gives them an honest certificate.
- **Falsify — already partly adopted, extend it.** `falsify_hardcase.py`'s `#9
  exclusion_zone` already implements the Diophantine sqrt hard-case set (it cites
  the companion sqrt paper). §8.3 here adds a *missing* piece: the **parity-class
  scaling reduction** — a generator/reducer that, given one representative hard
  case, certifies the whole infinite family `2^(e+2n)k` is covered by the scaling
  theorem, so the falsify engine need only test `n=0`. New generator kind
  `parity_scale` for `worst_cases`.

**(4) Buildable-now vs gated.** `fp_rounding` and `fp_error_bound` checkers are
fully **buildable now**: offline, exact-rational, deterministic, stdlib-only
(rounding a rational is exact). The generic *symbolic* error analysis over
variables can reuse an **injected numeric seam** for magnitude estimates snapped
to rationals (same pattern as `cert_sos`'s `root_finder`). Nothing here is
gated except the eventual HOL-verified checker binary (the honest upgrade path
already documented as `CAKEML_TARGET`/`hol_*` in each module).

---

## 2. `tang.pdf` — "Floating point verification in HOL Light: the exponential function"

**(1) What it is.** Harrison, Journal of Automated Reasoning (Kluwer), 44pp.
Full machine-checked verification of Tang's **table-driven** `exp` algorithm in
IEEE single precision: IEEE-754 formalised, a while-language with VC generation,
and a three-part error analysis. Confirms (and slightly *strengthens*) Tang's
published bound while finding one genuine slip.

**(2) Key mechanisms.**
- **Table-driven range reduction.** `x = n·ln2/32 + r`, `n = round(x·32/ln2)`,
  `n = 32m+j`; `2^(j/32)` prestored as a two-word split `Slead(j)+Strail(j)`,
  `r = r1+r2` split to fight rounding — the reconstruction
  `e^x = 2^m(2^(j/32) + 2^(j/32) p(r))` with low-order `p(r) ≈ e^r-1`.
- **Three-part error split** (verified independently, then composed): range-
  reduction error (exactness of `X - N·L1` by cancellation lemmas), polynomial-
  approximation error `|e^r-1-p(r)|` (the `poly.pdf` method), and accumulated
  rounding error (fma-chain propagation, 41 error terms for `P`).
- **Worst-case-input search.** Counterexamples to Tang's too-narrow interval
  bound on `R` were found by "testing numbers where the rounding to an integer
  leaves the greatest possible error of 0.5 ulp" (hex `423708C0` etc.) — an
  explicit adversarial-input recipe.
- **Transcendental exclusion arguments (§8.5–8.6).** Overflow/underflow
  correctness needs: *no representable float `X` has `exp(X)` within ~0.54 ulp of
  the overflow threshold* (resp. within `2^-150` of a denormal boundary). Proved
  by showing `ln(threshold)` is **straddled by two specific floats and is
  >2^-22 from either** — a transcendental analogue of a Diophantine exclusion
  zone.

**(3) Mapping to cert-log / falsify.**
- **NEW worst-case generator `half_ulp` (buildable, high-value).** The "0.5-ulp
  rounding-boundary" recipe is a distinct adversarial family not covered by
  `falsify_hardcase`'s `near_root` / `balanced_factorization` / `hensel`. Given a
  scale `C` (e.g. `32/ln2`) and format, enumerate inputs `x` where `round(x·C)`
  sits exactly at a `±0.5 ulp` tie — the points that break range reduction.
  Emits candidates for the falsify engine; every candidate re-checked to hit the
  boundary exactly (soundness, à la existing generators).
- **NEW cert kind `fp_exclusion` (checker buildable, generator semi-gated).**
  Certify "no representable float `x` has `f(x)` within `δ` of critical value
  `t`" by carrying the two straddling floats and their `mpmath.iv`-validated
  distance to `f^{-1}(t)`. Checker (buildable now) re-verifies the enclosure with
  `mpmath.iv` (same dependency `cert_taylor_model` already uses); the full
  "no float exists" quantifier is honestly **gated** on a verified inverse bound
  / exhaustive-binade argument (flag it, don't overclaim).
- **Reuse existing kinds.** The exp error split is exactly `taylor_model`
  (ln2 and `2^(j/32)` bounds via truncated series — the paper does this) +
  `fp_error_bound` (§1) + the `poly.pdf` minimax cert (§3). No new machinery for
  those parts — they *compose*.

**(4) Buildable-now vs gated.** `half_ulp` generator and the `fp_exclusion`
*checker* are buildable now (integer/rational + `mpmath.iv`, offline,
deterministic). The end-to-end "no float exists over the whole format" claim is
**gated/honest** — surface it as a candidate-list exclusion checker plus an
explicit gated note, never as an unconditional theorem.

---

## 3. `poly.pdf` — "Verifying the accuracy of polynomial approximations in HOL"

**(1) What it is.** Harrison, TPHOLs'97 (Springer LNCS 1275), 16pp. The method
for the polynomial-approximation error part of the other two papers: rigorously
bound `||f - p||∞` on `[a,b]` for a table-driven approximant, formalising
polynomial theory (root-finiteness, order of a root, **squarefree
decomposition**, **Sturm's theorem**) and using a CAS (Maple) as an **untrusted
oracle whose output is re-checked in HOL** — precisely Theoremata's
producer/checker split.

**(2) Key mechanisms.**
- **Exact real-root counting (Sturm).** For a rational-coefficient polynomial and
  rational endpoints, `#{roots in [a,b]} = variation(a) − variation(b)` over the
  standard Sturm chain `p0=p, p1=p', p_i = q_i p_{i+1} − p_{i+2}` (rescaled to
  keep coeffs integral). Requires only exact rational arithmetic.
- **Squarefree decomposition** `p / gcd(p,p')` (checked via an *externally
  supplied* Bézout witness `d = r·p + s·p'`, `p=q·d`, `p'=e·d`) to reduce to
  simple roots so IVT sign-changes locate every root.
- **Root isolation + completeness.** Maple returns isolating intervals with
  opposite-sign endpoints; a HOL lemma proves the *ordered* interval list
  contains **all** roots (using the Sturm count) — the crucial completeness
  guarantee.
- **Minimax bound.** `max|e|` of a differentiable `e` is attained at endpoints or
  zero-derivative points; with all roots of `e'` isolated to `ε/B` (B a crude
  derivative bound) one gets `|f-p| <= K + B·ε`. Composed with a Taylor
  truncation `f≈t` (error `< ε/2`) to reach the final certified bound.

**(3) Mapping to cert-log / falsify — the strongest new-cert opportunity.**
- **NEW cert kind `sturm` (buildable now, high priority).** *No root-counting
  certificate exists anywhere in the repo* (`grep sturm` → nothing). A `sturm`
  proof-log carries `p` (rational monomial coeffs), `[a,b]`, and the Sturm chain;
  the reference checker **recomputes the chain by exact pseudo-division and the
  sign variations at `a,b`** and asserts the root count — and can carry/verify
  isolating intervals with opposite-sign endpoints. This directly reuses the
  self-contained `_CheckPoly` pseudo-remainder arithmetic already in
  `cert_log.py` (built for `wu_geometry`) — minimal new code, pure `Fraction`,
  offline, deterministic. It is the exact-count sibling of the *sufficient-only*
  positivity certs (`sos`/`bernstein` prove `≥0`; `sturm` proves an exact
  `#roots`).
- **NEW composite cert kind `poly_minimax` / `approx_error` (buildable).**
  Certify `|f(x) − p(x)| <= K` on `[a,b]` by chaining: a `taylor_model` step
  (`f≈t`, existing kind), a `sturm` step (all `N` roots of `e'=t'−p'` isolated),
  and a maximisation step (evaluate `|e|` at endpoints + isolating intervals with
  the `B·ε` Lipschitz cushion). This is the honest capstone certificate for the
  transcendental-approximation obligations Theoremata screens but won't prove —
  and it is what `tang.pdf`'s middle error term needs.
- **Squarefree Bézout seam = the injected-oracle pattern.** The `d = r·p + s·p'`
  witness supplied by Maple and re-checked is a textbook **injected numeric/CAS
  seam** (sympy `gcdex`/`resultant` in generation; exact-rational re-check in the
  checker) — identical to `cert_sos`'s `root_finder` and `cert_nullstellensatz`'s
  reuse-sympy-then-recheck design. No trust in the CAS.
- **Falsify upgrade.** `falsify_hardcase.py`'s `near_root` currently hugs roots of
  a polynomial with *no completeness guarantee*. Backing it with a `sturm`
  certificate upgrades it to **"these are provably all the roots in [a,b]"** — a
  certified worst-case enumeration rather than a heuristic scan.

**(4) Buildable-now vs gated.** The `sturm` checker and `poly_minimax` composite
are **fully buildable now**: exact-rational, deterministic, offline, reusing
existing `_CheckPoly`; sympy optional (generation only). Nothing gated — this is
the cleanest, highest-confidence adoption in the whole batch.

---

## Prioritized adopt list (★ = NEW certificate/generator)

1. **★ `sturm` certificate kind (NEW).** Exact real-root count on `[a,b]` via
   Sturm chain + variation difference; optional isolating intervals. Reuses
   `cert_log._CheckPoly` pseudo-division. *Buildable now, exact-rational,
   highest confidence.* Source: `poly.pdf` §5. No analog exists in-repo.
2. **★ `poly_minimax` / `approx_error` certificate kind (NEW, composite).**
   `|f−p| <= K` on `[a,b]` = `taylor_model` (existing) ∘ `sturm` (item 1) ∘
   Lipschitz max-over-endpoints-and-roots. *Buildable now.* The honest capstone
   for transcendental-approximation screens. Source: `poly.pdf` §6 + `tang.pdf` §8.3.
3. **★ `fp_rounding` certificate kind (NEW).** Certify `round`/`ulp`/error-bound
   for explicit rational inputs (a `ROUND_CONV` analog), exact `Fraction`.
   *Buildable now.* Source: `fparith.pdf` §4,§8.1. First IEEE-rounding cert.
4. **★ `half_ulp` worst-case generator (NEW).** Enumerate 0.5-ulp rounding-tie
   inputs that break range reduction; each re-checked exactly. Extends
   `falsify_hardcase.worst_cases` (new `kind`). *Buildable now.* Source:
   `tang.pdf` §9 counterexample recipe.
5. **★ `fp_error_bound` certificate kind (NEW).** Serialise fma-chain
   absolute/relative error propagation; checker re-propagates with exact-rational
   intervals. *Buildable now* (injected magnitude seam for symbolic inputs).
   Source: `fparith.pdf` §8.2.
6. **★ `parity_scale` reduction generator (NEW).** Certify an infinite hard-case
   family `2^(e+2n)k` reduces to the `n=0` representative via the rounding scaling
   theorem — completes the existing `exclusion_zone`. *Buildable now.* Source:
   `fparith.pdf` §8.3.
7. **★ `fp_exclusion` certificate kind (NEW, partially gated).** "No representable
   float has `f(x)` within `δ` of critical value `t`" via straddling floats +
   `mpmath.iv` distance. **Checker buildable now**; full quantifier over the
   format is **honestly gated** (verified inverse / binade enumeration). Source:
   `tang.pdf` §8.5–8.6.
8. **Upgrade `near_root`** (existing generator) to carry a `sturm` completeness
   certificate — heuristic scan → certified worst-case set. Source: `poly.pdf`.

---

## Summary (~10 lines)

- Three Harrison FP-verification papers; **all read in full, no injection, no
  open license (treat clean-room — ideas reusable, text/code not)**.
- The headline finding: **there is no real-root-counting certificate in
  Theoremata**, and `poly.pdf` hands us one directly — a **`sturm` cert** (exact
  root count via Sturm chain + sign variations) that *reuses the existing
  `_CheckPoly` pseudo-division*, fully offline/exact-rational, highest confidence.
- On top of it, a **`poly_minimax` composite cert** (`taylor_model` ∘ `sturm` ∘
  Lipschitz-max) gives the honest capstone for the transcendental-approximation
  obligations Theoremata screens but refuses to prove — this is what `tang.pdf`'s
  exp verification actually needs.
- `fparith.pdf` yields two more genuinely new kinds — **`fp_rounding`** (first
  IEEE round/ulp/error cert, exact-rational `ROUND_CONV` analog) and
  **`fp_error_bound`** (fma-chain error propagation) — both buildable now.
- Falsify side: **`half_ulp`** (0.5-ulp tie inputs, `tang`) and **`parity_scale`**
  (infinite→representative hard-case reduction, `fparith`) are new generators that
  extend the already-adopted `exclusion_zone`/`near_root`; `near_root` should gain
  a `sturm` completeness certificate.
- Only **`fp_exclusion`** (transcendental "no-float-near-critical-value") is
  partially **gated** — ship the checker, flag the full quantifier honestly.
- Everything else is offline, deterministic, exact-rational, and fits the existing
  producer/injected-seam/reference-checker + `theoremata.cert-log.v1` envelope.
- The papers also validate the **CAS-as-untrusted-oracle** pattern (Maple in
  `poly.pdf`) that Theoremata's sympy-then-recheck certs already use.
