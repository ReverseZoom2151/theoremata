# HOL Light source — resource-mining report

**Path:** `resources/hol-light-master/hol-light-master/` (John Harrison's HOL Light, mirror of `github.com/jrh13/hol-light`). ~1800 files; OCaml. **Not read exhaustively — characterized.** This is the reference implementation for our **Candle** backend (Candle = a verified HOL Light kernel on CakeML), so its kernel and side-conditions are the *ground truth* our `axiom_audit.rs` and Candle trust-boundary must match.

---

## 1. What it is + license

Interactive LCF-style theorem prover / proof checker. Tiny trusted kernel (`fusion.ml`, 676 lines) on top of which everything else — arithmetic, reals, multivariate analysis, decision procedures — is *derived*, so soundness reduces to auditing the kernel + a fixed 3-axiom base + the handful of escape hatches.

**License: PERMISSIVE — confirmed.** `LICENSE` file is **BSD-2-Clause** ("(c) University of Cambridge 1998; (c) John Harrison and others 1998–2012 … Redistribution and use in source and binary forms, with or without modification, are permitted provided …"). File headers throughout carry the same Cambridge/Harrison copyright. `Formal_ineqs/README.md` explicitly: "Distributed under the same license as HOL Light (BSD-2-clause)" (that subtree © Alexey Solovyev 2014–2017). **Sub-directories may carry other licenses** (LICENSE says "Some files … are distributed under other licenses; please check individual files") — so per-file header check is required before copying any specific vendored subtree, but the core (`fusion.ml`, `class.ml`, `nums.ml`, `realarith.ml`, `grobner.ml`, `Examples/sos.ml`, etc.) is clean BSD-2. **Verbatim source reuse is legally OK with attribution; we still prefer clean-room reimplementation from the math (see §4) to avoid OCaml→Rust coupling.**

---

## 2. The KERNEL (`fusion.ml`) — validates Candle + `axiom_audit.rs`

`thm` is an abstract type `Sequent of (term list * term)` behind a `Hol_kernel` signature; the **only** way to build a `thm` is via the primitive rules below (the constructors are `private`). This is the whole trust story.

**Types/terms:** `hol_type = Tyvar | Tyapp`; `term = Var | Const | Comb | Abs`. Initial type constants `bool`, `fun`; initial term constant `=` only.

**The ~10 primitive inference rules** (the axiomatic core of the logic):
1. `REFL` — `|- t = t`
2. `TRANS` — transitivity of `=` (derivable, kept for speed)
3. `MK_COMB` — congruence: from `|- f=g`, `|- x=y` get `|- f x = g y`
4. `ABS` — from `|- s=t` get `|- (\x.s)=(\x.t)` (side-cond: `x` not free in hyps)
5. `BETA` — `|- (\x.t) x = t` (trivial redex only)
6. `ASSUME` — `t |- t` (t must be `:bool`)
7. `EQ_MP` — from `|- p=q` and `|- p` get `|- q`
8. `DEDUCT_ANTISYM_RULE` — deduction: combines hyps, yields `|- p=q`
9. `INST` — term instantiation (capture-avoiding, via `vsubst`/`variant`)
10. `INST_TYPE` — type instantiation (capture-avoiding, via `inst`, `Clash` exception)

**Definitional / base-extension primitives (the escape hatches):**
- `new_axiom tm` — asserts `|- tm` by fiat. **The base-widener.** `axioms()` tracks them.
- `new_constant`, `new_type` — raw signature extension (uninterpreted; mild base-widening, not unsound alone). `class.ml:45` does `new_constant("@",…)`.
- `new_basic_definition` — `|- c = r`, guarded by **two side conditions**: (a) `r` closed (`freesin [] r`); (b) **`subset (type_vars_in_term r) (tyvars ty)`** — every type var in the definiens must appear in the defined constant's type. Violating (b) is the classic HOL unsoundness.
- `new_basic_type_definition tyname (abs,rep) (|- P t)` — carves a new type from a non-empty subset; side conds: no hyps, `P` closed. Non-emptiness is *discharged by the input theorem* `|- P t`, so it's kernel-guarded.

**The 3 axioms (the entire mathematical trusted base beyond the rules):**
- `ETA_AX` — `class.ml:16` — `!t:A->B. (\x. t x) = t` (extensionality)
- `SELECT_AX` — `class.ml:53` — `!P (x:A). P x ==> P((@) P)` (Hilbert choice / AC; `@` added via `new_constant`)
- `INFINITY_AX` — `nums.ml:28` — `?f:ind->ind. ONE_ONE f /\ ~(ONTO f)` (axiom of infinity)

That's it — classical HOL = these 3 axioms + the primitive rules. Everything else (EXCLUDED_MIDDLE, nat/int/real, analysis) is *derived* (e.g. `EXCLUDED_MIDDLE` proved via Diaconescu from choice, `class.ml:140`).

**`mk_thm` — the real "cheat" hatch:** `drule.ml:24`, commented *"The last recourse when all else fails!"*: `mk_thm(asl,c) = new_axiom(asl ==> c)` then MP. So **`mk_thm` is literally `new_axiom` in disguise** — fabricates any theorem. And **`CHEAT_TAC`** (`tactics.ml:740`) `= ACCEPT_TAC(mk_thm([],w))` — a *named* wrapper around `mk_thm`.

### Validation of our `axiom_audit.rs` (build #16) against the real source
| Auditor rule | HOL Light reality | Verdict |
|---|---|---|
| whitelist = `ETA_AX`/`SELECT_AX`/`INFINITY_AX` | Exactly the 3 `new_axiom` calls in the source | **MATCH — correct and complete** |
| `mk_thm` = CRITICAL kernel bypass | `drule.ml:24` = `new_axiom` wrapper | **MATCH** |
| `new_axiom` off-whitelist = CRITICAL | The base-widening primitive | **MATCH** |
| `bad_definition` = type var in definiens not in constant | Exactly the `subset (type_vars_in_term r) (tyvars ty)` side-cond in `new_basic_definition` (`fusion.ml:595`) | **MATCH — our heuristic mirrors the kernel's real check** |
| `INST`/`INST_TYPE` = WARNING | Kernel `INST`/`INST_TYPE` are capture-avoiding → warning-only is right | **MATCH** |

**Gaps to close in `axiom_audit.rs` (concrete, from the real source):**
1. **`CHEAT_TAC` is not flagged.** It expands to `mk_thm` but our lexical scan only greps `mk_thm`/`new_axiom` — a script that calls `CHEAT_TAC` (without the literal `mk_thm`) slips through. **Add `CHEAT_TAC` to the CRITICAL lexical set.**
2. **`new_basic_definition` bad-def check**: auditor checks `new_definition`/`new_basic_definition`/`define` but NOT the closedness side-cond (a) nor `new_specification` (`nums.ml:256`, choice-based). Low priority (closedness rarely violated) but note it.
3. **Raw `new_constant` / `new_type`** (signature widening without definition) are un-tracked. Not unsound alone, but for a *complete* trusted-base diff the Candle auditor should surface uninterpreted-constant introductions. Consider an INFO/WARNING tier.
4. `new_basic_type_definition` with a non-kernel-proven `P t` — kernel-guarded, so fine, but worth an INFO note that type defs enter the base.

---

## 3. Certificate-PRODUCING decision procedures — inventory → cert-log mapping

HOL Light's design philosophy **is** cert-log's: a possibly-untrusted (even external) search produces a certificate, the kernel *replays* it into a `thm`. Direct hits for our cert-log kinds:

| HOL Light procedure | File | What it produces | cert-log kind |
|---|---|---|---|
| **`REAL_SOS` / `SOS_RULE` / `SOS_CONV`** | `Examples/sos.ml` | **Positivstellensatz refutation** for nonlinear real arithmetic. Explicit datatype `positivstellensatz` (`realarith.ml:166`): `Axiom_eq/le/lt \| Rational_eq/le/lt \| Square t \| Eqmul \| Sum \| Product`. SOS found by external SDP (csdp/sdpa), then rationalized + kernel-checked. | **`cert_sos` + a NEW first-class `cert_positivstellensatz`** — the datatype is a ready-made cert grammar to adopt verbatim (it generalizes plain SOS: handles `≤/</=` hypotheses via `Axiom_*` + `Eqmul`). |
| **`GEN_REAL_ARITH` framework** | `realarith.ml` | Parametrized universal-real decision procedure driven by the same `positivstellensatz` proof term (Positivstellensatz *checker* is `LINEAR_PROVER`/`translator`). | Same as above — this is the **cert *checker*** we clean-room. |
| **`RING` / `ideal_cofactors`** | `grobner.ml` | Gröbner-basis proof of ideal membership; `ideal_cofactors` returns **explicit cofactor polynomials** `p = Σ qᵢ·gᵢ` (a **Nullstellensatz certificate**); `grobner_refute` yields a refutation trace (`history` type). Solves universal theory of ℂ / char-0 integral domains. | **`cert_nullstellensatz` / Gröbner** — cofactor list is the certificate; kernel replays via `NUM_RING`/`INT_RING`/`REAL_RING`. |
| **`Rqe/` (REAL_QELIM / CAD)** | `Rqe/rqe_main.ml` etc. | Real quantifier elimination (Cohen–Hörmander / partial CAD); produces sign-matrix / case-split proof reconstructed through kernel (`TRAPOUT`, `Isign`). | **NEW kind `cert_cad` / real-QE** — no current cert-log equivalent; heavy but the canonical decision procedure for `RCF`. |
| **Pratt primality** | `Library/pratt.ml` | **Pratt certificate** (recursive primitive-root witness tree) → `|- prime p`. | **`cert_pratt` — direct 1:1 map.** Adopt the cert shape. |
| **Pocklington** | `Library/pocklington.ml` | Pocklington/partial-factorization primality certificate (`PROTH`-style) for large p where full factorization of p−1 is infeasible. | **`cert_pratt` variant / NEW `cert_pocklington`** — strictly more powerful than Pratt for big primes. |
| **`Formal_ineqs/` (Flyspeck)** | `Formal_ineqs/taylor/`, `arith/interval_arith.hl`, `verifier/` | Formally-verified **interval arithmetic + Taylor-model** nonlinear-inequality verifier (multivariate Taylor forms, verified float `arith_float.hl`). | **`cert_taylor_model` + `cert_bernstein`** (our Taylor/Bernstein certs) — this is a mature, kernel-checked implementation to mirror. |
| **`NUM_RING`/`INT_RING`/`REAL_RING`, `ARITH_RULE`, `NUM_REDUCE`** | `calc_num.ml`, `calc_int.ml`, `calc_rat.ml`, `arith.ml` | Ground/linear arithmetic by verified computation (each step a kernel rewrite). | Underpins `cert_lp`/Farkas-style linear certs; `calc_rat` = exact rational reduction. |
| **`MESON` / `METIS`** | `meson.ml`, `metis.ml` | FOL model-elimination / resolution. Produces a proof *replayed* through the kernel — **not** a compact external cert (proof is the tactic trace). | No cert-log kind; it's a search that emits kernel steps, not a checkable artifact. Note as "proof-producing, not cert-producing". |
| **`normalizer.ml` / `canon.ml`** | — | Polynomial canonical form + Skolem/CNF canonicalization; infra for the above. | Supporting infra for SOS/Gröbner cert checkers. |

**Not present:** no Wilf–Zeilberger/WZ, no explicit Wu characteristic-set method (Gröbner covers the algebraic-geometry slot). Our cert-log's WZ and Wu kinds have **no HOL Light source counterpart** — those stay clean-room from the math literature.

---

## 4. Buildable-now (clean-room checker from the math) vs toolchain-gated

**Buildable now (pure checkers — the certificate is a static artifact the math fully specifies; no OCaml/csdp needed):**
- **Positivstellensatz/SOS checker** — adopt the `positivstellensatz` datatype (`realarith.ml:166`) as our cert grammar; checking = evaluate the proof term (Σ of squares × axiom products) and confirm it equals the negated goal. Pure rational arithmetic. **Highest-value, self-contained.**
- **Nullstellensatz/Gröbner cofactor checker** — verifying `p = Σ qᵢ·gᵢ` is just polynomial multiply-and-add; the *hard* part (computing the Gröbner basis) is the untrusted producer. Checker buildable now.
- **Pratt / Pocklington primality checkers** — recursive certificate verification is elementary modular arithmetic (bignum). Clean-room now.
- **Interval-arithmetic / Taylor-model evaluator** — verified-float + interval eval is self-contained (mirror `Formal_ineqs`, but our own rational/float impl).
- **Linear/Farkas (LP) cert checker** — from `calc`/`realarith` linear fragment.

**Toolchain-gated (need the OCaml + OCaml-package build to *run*, though the math is portable):**
- **SOS/Positivstellensatz *producer*** — `Examples/sos.ml` shells out to an external **SDP solver (csdp/sdpa)** (`sdpa_of_problem`, `parse_csdpoutput`, `sos.ml:398–499`) to *find* the SOS, then rationalizes. Finding the cert needs csdp; *checking* it does not. (Perfect cert-log split: untrusted external producer, kernel/clean-room checker.)
- **Full CAD / real-QE (`Rqe/`)** — large; running it needs the HOL Light build. A standalone CAD checker is a substantial project.
- **`MESON`/`METIS`** — proof-*producing*, replayed through kernel; nothing to check offline.
- **Actually running the kernel to replay any cert** — needs OCaml/`opam install hol_light` (README) or the Candle/CakeML build. Windows toolchain gap noted in memory (no lean/lake; OCaml likewise absent) → these are the Candle-backend targets, not runnable in this env.

---

## 5. Injection + license line

**INJECTION SCAN:** README, LICENSE, and file-header comments read as legitimate HOL Light documentation (installation, math). **No prompt-injection / no instructions directed at an AI/agent found** in the files inspected. All repo content remains treated as UNTRUSTED per standing policy; nothing here was acted on as an instruction. (Full 1800-file comment sweep not performed — characterized, not exhaustively read; flag-if-seen posture retained.)

**LICENSE LINE:** HOL Light core = **BSD-2-Clause** (© University of Cambridge 1998 / John Harrison & others 1998–2012), confirmed via top-level `LICENSE` + consistent file headers. Permissive; verbatim reuse permitted with attribution + disclaimer retention. Sub-trees may differ ("check individual files") — per-file header check required before copying a specific subtree (e.g. `Formal_ineqs` is BSD-2 © Solovyev; `pa_j/`, `Minisat/`, `Cadical/` third-party — verify individually).

---

## Prioritized adopt-list

1. **Patch `axiom_audit.rs`: add `CHEAT_TAC` to the CRITICAL lexical set** (it's a `mk_thm` wrapper that currently slips through). Cheap, closes a real hole. *(Validation win: the auditor's whitelist and bad-def rule already exactly match the kernel — this is the one concrete miss.)*
2. **Adopt the `positivstellensatz` datatype (`realarith.ml:166`) as a first-class cert-log kind** (`cert_positivstellensatz`), generalizing `cert_sos` to inequality/equality hypotheses (`Axiom_le/lt/eq`, `Eqmul`). Build the pure-rational checker now.
3. **Gröbner/Nullstellensatz cofactor cert** (`grobner.ml` `ideal_cofactors`) → `cert_nullstellensatz` checker (poly multiply-add). Now.
4. **Pratt + Pocklington primality cert checkers** (`Library/pratt.ml`, `pocklington.ml`) → `cert_pratt` (+ `cert_pocklington` for large primes). Now, elementary.
5. **Mirror `Formal_ineqs` Taylor-model + verified interval arithmetic** for `cert_taylor_model`/`cert_bernstein` — the most mature kernel-checked nonlinear-inequality engine available; use as design reference.
6. **Candle backend trust-boundary spec**: encode "10 primitive rules + 3 axioms (ETA/SELECT/INFINITY) + {new_axiom, new_constant, new_type, new_basic_definition[2 side-conds], new_basic_type_definition, mk_thm/CHEAT_TAC}" as the exact escape-hatch set the axiom auditor + Candle must diff against. `fusion.ml` is the canonical spec.
7. **Lower priority / larger:** CAD real-QE checker (`Rqe/`) as a NEW `cert_cad` kind — canonical for RCF but a substantial standalone build.

---

## ~10-line summary

- **This repo (`resources/hol-light-master/…`) is the real HOL Light source** — the reference for our Candle (verified-HOL-Light-on-CakeML) backend. **License = BSD-2-Clause, permissive, confirmed** (core clean; sub-trees "check individual files").
- **Kernel (`fusion.ml`, 676 lines):** abstract `thm`, **10 primitive rules** (REFL, TRANS, MK_COMB, ABS, BETA, ASSUME, EQ_MP, DEDUCT_ANTISYM_RULE, INST, INST_TYPE) + definitional principles.
- **Trusted base = exactly 3 axioms:** `ETA_AX`, `SELECT_AX`, `INFINITY_AX` (everything else, incl. excluded middle, is derived).
- **Our `axiom_audit.rs` whitelist + `bad_definition` rule EXACTLY match the kernel's real side-condition** (`subset(type_vars_in_term r)(tyvars ty)`) and axiom set — strong validation.
- **One concrete auditor gap:** `CHEAT_TAC` (a `mk_thm`/`new_axiom` wrapper, `tactics.ml:740`) is not flagged — add it. (`mk_thm` itself *is* `new_axiom` under the hood, `drule.ml:24`.)
- **Cert-producing decision procedures map cleanly to cert-log:** SOS/**Positivstellensatz** (`Examples/sos.ml` + `realarith.ml` datatype), Gröbner/**Nullstellensatz** cofactors (`grobner.ml`), **Pratt/Pocklington** primality (`Library/`), **Taylor-model/interval** (`Formal_ineqs/`), real-QE/**CAD** (`Rqe/`, a NEW kind).
- **Design = cert-log's philosophy:** SOS *producer* shells to an external SDP solver (csdp) — **toolchain-gated to find**, but the **checker is clean-room buildable now** (pure rational/poly arithmetic).
- **Buildable now:** Positivstellensatz, Nullstellensatz, Pratt/Pocklington, interval/Taylor, LP/Farkas checkers. **Gated:** running the kernel/CAD/MESON (need OCaml/Candle build).
- **No WZ / no Wu** in the source — those cert kinds stay clean-room from the literature.
- **No prompt-injection found**; all content still treated as untrusted (characterized, not exhaustively read).
