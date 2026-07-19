# peteroupc.github.io: explicit polynomial approximation error bounds

Source: `resources/peteroupc.github.io-master/peteroupc.github.io-master/`, notably
`approxtheory.md` ("Notes on Approximation Theory") and `bernapprox.md`
("Approximations in Bernstein Form"), by Peter Occil.

Verdict: **ADOPT (narrow).** One family of bounds is genuinely usable and has been
implemented in `components/verify/python/theoremata_tools/minimax_bounds.py`, with
tests in `components/verify/tests/test_minimax_bounds.py`. The rest of the site is
correctly out of scope.

## Correction: the earlier SKIP rested on a false premise

An earlier mining pass triaged this resource as SKIP, reasoning that its explicit
polynomial error bounds were worth mining only "IF the minimax-bound path ever becomes
real work". That premise was already false when it was written. `poly_minimax` is one
of the shipping certificate kinds in `components/verify/python/theoremata_tools/cert_sturm.py`,
alongside `sturm`. The revisit trigger the SKIP set for itself had therefore already
fired at the moment the SKIP was recorded, and the resource should have been mined then.

The concrete cost of the miss: `export_poly_minimax_cert` requires the caller to supply
`K`, and nothing in the pipeline told a caller where a defensible `K` might come from.
That gap is what this pass closes.

## Licence (verified, not assumed)

* `resources/peteroupc.github.io-master/peteroupc.github.io-master/LICENSE` is the
  Unlicense ("This is free and unencumbered software released into the public domain").
* `approxtheory.md` and `bernapprox.md` each carry their own `## License` section:
  "Any copyright to this page is released to the Public Domain. In case this is not
  possible, this page is also licensed under Creative Commons Zero."

So the formulas are quotable and usable without constraint, and citation here is a
provenance and error-tracing measure rather than a licensing obligation.

Untrusted-data note: both notes were scanned for prompt-injection patterns (imperative
instructions aimed at a reader-agent, fake system prompts, "ignore previous
instructions" variants). **No injection found.** They read as ordinary mathematical
prose. They remain third-party data: every formula below was transcribed by hand from
prose and LaTeX, so a transcription error is a live risk, which is why the
implementation cites the exact table row and the tests pin each formula against an
independently known value.

## What is in the notes

`approxtheory.md` is the general theory: Lemmas 1 through 13 bounding
`|L(f)(x) - f(x)|` for positive linear operators, general linear operators, Peano-kernel
arguments, Lebesgue inequalities. These are stated in terms of moduli of continuity
(`omega_1`, the concave `omega_1`), central and absolute moments of the operator
(`sigma_i`, `tau_i`), operator norms, and quantities like "the least upper bound of
`|L(g)(x)|` over all continuous `g` with sup-norm at most 1". They are explicit in the
sense of having no hidden constants, but almost none of them is *computable* from what
we hold: they require the caller to already know a modulus of continuity or an operator
norm, which is a strictly harder object than the bound being sought.

`bernapprox.md` is the operational half, and it is the useful one. Its section
"Approximations on the Closed Unit Interval" contains a table whose every row is a
fully numeric error bound for a **named, constructible** polynomial (Bernstein
polynomial `B_n(f)`, iterated Boolean sums, Butzer combinations, Lorentz operators)
in terms of a Lipschitz or Hoelder constant of `f` or of a derivative of `f`.

## Bounds extracted (explicit, every constant numeric)

All are for `p = B_n(f)`, the degree-`n` Bernstein polynomial of `f` (Bernstein
coefficients `f(x_j)` at the `n+1` evenly spaced nodes). Section: `bernapprox.md`,
"Approximations by Polynomials" -> "Approximations on the Closed Unit Interval".

| Hypothesis on `f` | Bound on `|f - B_n(f)|` | Attribution in the note |
| --- | --- | --- |
| `f'` Lipschitz, constant `L1` | `L1 / (8n)` | Lorentz (1964) |
| `f'` Hoelder, exponent `alpha`, constant `H1` | `H1 / (4 n**((1+alpha)/2))` | Schurer and Steutel (1975) |
| `f` Hoelder, exponent `alpha`, constant `H0` | `H0 * (1/(4n))**(alpha/2)` | Kac (1938) |
| `f` Lipschitz, constant `L0` | `((4306 + 837*sqrt(6))/5832) * L0 / sqrt(n)`, i.e. `< 1.08989 L0 / sqrt(n)` | Sikkema (1961) |

Two supporting results, also used:

* **Change of interval** ("Approximations Beyond the Closed Unit Interval"): for
  `f(x) = g(a + (b-a)x)`, the `r`-th derivative's Lipschitz constant scales by
  `(b-a)**(r+1)` and its sup-norm by `(b-a)**r`. The note explicitly warns that the
  `H_r`-based rows do **not** transfer off `[0, 1]`; the implementation honours that by
  refusing any other interval for the two Hoelder bounds rather than rescaling them.
* **Coefficient rounding** (Note 1 of the same section): perturbing every Bernstein
  coefficient by at most `delta` moves the polynomial by at most `delta`. This is what
  lets us sample a transcendental `f` at rational nodes, round to exact rationals, and
  fold the rounding into `K` soundly.

Deliberately **not** extracted:

* Everything in `approxtheory.md` that needs a modulus of continuity, an operator norm,
  or a Peano kernel as input (Lemmas 1 to 13). Explicit but not computable here.
* Rows for `U_{n,2}`, `U_{n,3}`, Butzer's `L_{2,n/2}` / `L_{3,n/4}`, and the Lorentz
  operators. Their bounds are equally explicit, but building those polynomials exactly
  is materially more work (iterated Boolean sums, degree elevation, second-derivative
  samples) and buys only a better rate, not a new capability. Recorded as a follow-up,
  not implemented.
* The bulk of the site: Bernoulli factories, randomness extraction, colour and
  encoding utilities. Irrelevant.

## Honest assessment: is any of it usable?

The obvious objection is real and worth stating first. Our checker
(`cert_sturm.check`) re-verifies a bound over exact rational arithmetic on a
**concrete polynomial pair**; these are bounds over a function **class**. There is no
way to make `check` accept "the Lorentz bound says so" as evidence, and it must not.

But the bounds are still usable, for a specific structural reason: **they apply to a
polynomial we can construct exactly.** `B_n(f)` is not an abstract minimiser, it is a
determinate polynomial whose Bernstein coefficients are samples of `f`. So the workflow
is closed:

1. Pick `n`, sample `f` at the `n+1` nodes as exact rationals (rounding down, with the
   rounding error tracked).
2. Compute `p = B_n(f)` in monomial form, exactly, over `Fraction`.
3. Compute a candidate `K` from the table row matching the hypothesis on `f`, plus the
   Note-1 rounding slack.
4. Hand `p` and `K` to `export_poly_minimax_cert`, and let `check` decide.

Step 4 is the whole point. The formula produces a *number*; the checker produces the
*fact*. If the formula were wrong (bad transcription, wrong constant, hypothesis that
does not hold), the checker rejects and the pipeline is no worse off than before. So
this is a **generation aid whose output is fully re-verified**, which is the strongest
thing a bound over a function class can honestly be in this architecture.

Where it stops short, stated plainly:

* **The hypotheses are not machine-checkable.** We cannot verify from rational data
  that `f'` is Lipschitz with constant `L1`, nor that the supplied node values really
  are samples of `f`. Every such hypothesis is emitted verbatim in the candidate's
  `assumptions` list, prefixed `UNCHECKED:`, so nothing is assumed silently. If a
  caller lies about `L1`, the candidate `K` is garbage; the checker will usually still
  catch it, because `check` never consults `L1`, but "usually" is not "always" and the
  assumption record is how a reviewer sees what was taken on faith.
* **A correct bound is not always an acceptable one.** The source bounds are non-strict
  (`|f - p| <= K`) and several are attained. `cert_sturm`'s no-crossing step needs
  `|p - T|` strictly below `K - delta`. So a formula-correct `K` can be rejected. This
  is not papered over: `poly_minimax_candidate` has an explicit `strict_slack`
  parameter that defaults to zero, widening the claim is recorded as an assumption, and
  a test asserts the rejection of the attained bound as intended behaviour.
* **`B_n` converges slowly.** The rate is `O(1/n)` at best for these rows (the note
  cites Voronovskaya 1932 for the barrier). These `K` values are usable but loose; they
  are a starting point for a certificate, not a competitive minimax approximation.

## What was implemented

`components/verify/python/theoremata_tools/minimax_bounds.py` (new file; `cert_sturm.py`
untouched, as it is the trust boundary):

* `bernstein_bound_lipschitz_derivative`, `bernstein_bound_holder_derivative`,
  `bernstein_bound_holder`, `bernstein_bound_lipschitz` -> a frozen `BoundCandidate`
  carrying `K` as an exact `Fraction`, the source citation, and the `UNCHECKED:`
  hypotheses.
* `BoundCandidate.verified` is a non-init field hard-wired to `False`; the dataclass is
  frozen, so no code path can flip it. `as_dict()` always emits `verified: false` and a
  `status` string saying the value is a candidate until `cert_sturm.check` accepts it.
* Irrational constants (`sqrt(6)`, `n**((1+alpha)/2)`, `(1/(4n))**(alpha/2)`) are
  bounded by exact rationals rounded in the **conservative** direction via integer
  `k`-th roots, so a returned `K` is never below the formula's true value. Rounding the
  other way would silently weaken the claim, which is the one failure mode that would
  matter.
* `bernstein_nodes` / `bernstein_monomial_coeffs` build the approximant exactly over
  `Fraction`, including the Bernstein-form-on-`[a, b]` change of basis.
* `poly_minimax_candidate` assembles `p_coeffs` plus the total `K` (class bound +
  node-rounding slack + optional strict slack) in the shape
  `export_poly_minimax_cert` wants. It does **not** import `cert_sturm`: a generator
  must not be able to reach the checker.

Tests (`components/verify/tests/test_minimax_bounds.py`, 21 cases) cover transcription
fidelity against closed forms (`B_n(x**2) = x**2 + x(1-x)/n`, where the Lorentz bound is
exactly attained), the interval rescaling on `[1, 3]`, rounding direction of the
rational root bounds, the Sikkema constant against the decimal the note quotes, and
three round trips through the real `cert_sturm` exporter and checker: attained `K`
rejected, `K` with strict slack accepted, halved `K` rejected. One end-to-end case takes
floored rational samples of `exp` on `[0, 1]`, folds the rounding into `K` via Note 1,
pairs it with a degree-8 Taylor sub-certificate, and gets a `valid: true` out of
`check`.

## Follow-ups worth considering

* The iterated Boolean sum `U_{n,2}` row (`(5L_2 + 4M_2)/(32 n**(3/2))` for a Lipschitz
  second derivative) gives a much better rate for smooth `f`, at the cost of
  constructing `B_n(2f - B_n(f))`. Same machinery, more code.
* `bernapprox.md` also has a Chebyshev-interpolant appendix; unmined so far, and a more
  natural fit than Bernstein for tight minimax work if the certificate ever needs it.
* `cert_taylor_model`'s interval-arithmetic residual spread scales with the subdivision
  width, so `delta` for a non-polynomial `f` must be loose enough for the checker's
  recomputation rather than merely true. Worth documenting near the exporter; it is
  easy to pick a `delta` that is mathematically correct and still rejected.
