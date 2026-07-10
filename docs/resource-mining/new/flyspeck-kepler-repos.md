# Flyspeck & Kepler98 — code-repo mining

Scope: the two *code* repos behind the Kepler conjecture proof. The Flyspeck
*papers* were mined separately (`docs/atp-mining/flyspeck-kepler.md`); this
report is the software architecture — the LP-certificate pipeline, the
Taylor/interval nonlinear verifier, tame-graph enumeration, and blueprint-scale
proof management — mapped onto Theoremata modules.

Sources (read-only, characterized not fully read; 3519 + 2040 files):
- `resources/flyspeck-master/flyspeck-master/` — the completed (2014) **formal**
  proof of Kepler in **HOL Light** + **Isabelle**.
- `resources/kepler98-master/kepler98-master/` — Hales & Ferguson's original
  **1998 informal** proof code (C++ / Java / Mathematica), a historical archive.

---

## 0. Injection + license line

- **INJECTION:** None found. Licenses, READMEs, code comments, and the Isabelle
  headers were scanned; no text attempts to issue instructions to a reader/agent.
  (The Leibniz epigraph in `interval/interval.h` — "unworthy of excellent
  persons to lose hours like slaves in the labor of calculation" — is a quotation,
  not an injection.) All repo content remains treated as UNTRUSTED data.
- **LICENSE — Flyspeck: MIT** (`LICENSE`, Copyright (c) 2014 Thomas C. Hales;
  `formal_ineqs/LICENSE` MIT too). Reusable with attribution — code or ideas.
- **LICENSE — kepler98: NO LICENSE FILE. Effectively "all rights reserved."**
  `interval/interval.h` carries `Copyright (c) 1997, Thomas C. Hales, all rights
  reserved`; 94 files repeat "all rights reserved"; no MIT/permission/redistribution
  grant anywhere; README calls it a "historical record." **=> CLEAN-ROOM: take
  mathematical ideas and algorithm structure only, never copy code.**

---

## 1. What each repo is

### Flyspeck (formal, MIT) — the reusable one
Directory structure maps 1:1 onto the three hard computational pillars, each
made *formal* (machine-checked), which is exactly Theoremata's verification-first
thesis:
- `formal_lp/`   — formal verification of the ~10^5 **linear programs** (HOL Light + a C# tool).
- `formal_ineqs/`— formal verification of **nonlinear inequalities** via multivariate
  **Taylor models + formal interval/floating-point arithmetic** (HOL Light).
  Standalone upstream: `github.com/monadius/formal_ineqs`.
- `formal_graph/isabelle_tame/` — **tame planar-graph classification** in **Isabelle**
  (this is the AFP entry `Flyspeck-Tame`; files here aid the HOL Light statement).
- `text_formalization/` — the ~500 `.hl` "book" proof, built by an explicit
  dependency-ordered build (`text_formalization/build/`) — a **blueprint-scale
  orchestration** precedent.
- `jHOLLight/` — Java front-end for Solovyev's SSReflect mode (tooling, low value).

### kepler98 (informal, clean-room only)
The 1998 pipeline, unmaintained, "all rights reserved":
- `interval/` — C++ interval-arithmetic + Taylor engine (`taylorInterval.cc`,
  `secondDerive.cc`, `recurse.cc` branch-and-bound). **Not rigorous by modern
  standards** (README says so of `numerical/`); superseded by `formal_ineqs`.
- `linear/`   — the LP bulk: `.lp` files (CPLEX format) + logs + `callable/` C.
- `graph/`    — Java applet generating tame planar graphs (`planar`, `arrange*.java`).
- `dodec/`,`honey/`,`samf/` — dodecahedral & honeycomb conjectures, Ferguson's code.

Value of kepler98 = *cross-check reference* for the math (branch-and-bound
strategy, LP formulation, graph properties). Flyspeck is the formal successor of
every kepler98 component; prefer Flyspeck for anything we actually build from.

---

## 2. Key reusable ARCHITECTURE

### 2a. LP-certificate pipeline — VALIDATES/EXTENDS `cert_flyspeck_lp`
This is the highest-value find and directly confirms the "modified-dual LP trick."

Pipeline (`formal_lp/`):
1. **Solve** each LP with GLPK (`glpk/lpproc.ml`, `glpk_link.ml`, `*.mod`) — untrusted, fast.
2. **Extract the dual**: constraint **marginals** (Farkas multipliers) + variable
   reduced-cost marginals from the GLPK solution (`LP.cs` reads
   `sol.ConstraintMarginals`, `sol.VariableMarginals`).
3. **Build a Farkas certificate** (`LP-HL/LP-HL/LP.cs`, the core ~260-line method):
   - form `sum1 = Σ_i marginal_i · ineq_i` (nonneg. combination of constraints);
   - the residual `df = objective − Σ ineq` is absorbed into corrected
     **variable-bound marginals** (`LMarginal`/`UMarginal`) — the "modified dual";
   - **rational rounding for soundness**: lower bounds `RoundDown`, upper bounds
     `RoundUp`, at a fixed `precision`, then compute slack `eps = UpperBound − sum`
     and **reject if `eps < 0`**. This makes a floating dual into a rigorous
     rational witness — exactly the trick to port.
4. **Emit a HOL Light certificate** (`Inequality.cs`/`LinearFunction.cs` print the
   linear combo) that HOL Light re-checks: multiply-and-add the stored inequalities
   and confirm the bound — **no LP solver in the trusted kernel path**.
5. **Serialize** certificates as binary OCaml (`glpk/binary/`, `easy*`/`hard*`
   prefixes); `hypermap/main/lp_certificate.hl` + `verify_all.hl` replay them.
   Full replay ≈ 15 h on a 2GHz Mac mini — offline-checkable, solver-independent.

Two-tier "easy" vs "hard" LP split (`build_all_easy` / `build_all_hard`, terminal-case
cap parameter) = a difficulty-tiered cert store worth mirroring.

**Takeaway for us:** our `cert_flyspeck_lp` already models the Farkas dual; the
*new* pieces to adopt are (i) the explicit directional rational rounding + `eps≥0`
gate that upgrades a float dual to a sound certificate, and (ii) the
solver→marginal→linear-combination→kernel-replay separation, with GLPK/any LP
solver kept strictly untrusted.

### 2b. Nonlinear Taylor/interval verifier — VALIDATES a `cert_taylor_model`/`cert_sos` sibling
`formal_ineqs/` is a self-contained, MIT, **formal** nonlinear-inequality prover:
- `arith/` — formal **floating-point** + **interval arithmetic** built inside the
  logic (`arith_float.hl`, `interval_arith.hl`, `float_theory.hl`, `eval_interval.hl`,
  `arith_cache.hl`), i.e. every rounding is a theorem, with a memo cache.
- `taylor/` — **multivariate Taylor models** (`m_taylor.hl`,
  `theory/multivariate_taylor.vhl`, `taylor_interval.vhl`): first/second-derivative
  interval bounds → rigorous enclosure of a function over a box.
- `verifier/` — a **branch-and-bound driver** over boxes with a **replayable proof
  certificate** (`certificate.hl`): the `result_tree` ADT records the search as
  `Result_pass` (box discharged), `Result_glue` (box split on a variable),
  `Result_mono` (dimension reduced by monotonicity), `Result_false` (counterexample).
  Two layers: an **informal** search (`informal/`) finds the tree fast; the
  **formal** pass (`m_verifier_main.hl`) replays it to produce the theorem.

**Takeaway:** this is the canonical design for a `cert_taylor_model` verifier —
separate a cheap *search* that emits a compact tree certificate from a *formal
replay* that checks it, with interval arithmetic as the trusted numeric core.
The `result_tree`/`P_result_*` grammar is a ready blueprint for our cert-log kind.
Complements (does not duplicate) our existing `cert_taylor_model` and `cert_sos`.

### 2c. Tame-graph combinatorial enumeration
`formal_graph/isabelle_tame/` (Isabelle, from AFP `Flyspeck-Tame`): `Enumerator.thy`
(all face extensions of an unfinished patch), `Plane.thy`/`Plane1.thy`,
`Completeness.thy` (the enumeration is complete vs the archive), `Tame.thy` (the
tameness predicate), `PlaneGraphIso.thy` (iso-dedup so each graph counted once),
`ArchCompAux.thy` (compare generated set against a stored **archive**).
kepler98's `graph/*.java` is the informal ancestor.

**Takeaway:** a verified **exhaustive enumeration + isomorphism-dedup + archive-
comparison** pattern. Relevant to our subsumption/dedup story and to any
"enumerate all cases, prove none survive" abstention/exhaustiveness gate.

### 2d. Blueprint-scale proof management — VALIDATES `blueprint_run`/`proof_import`
- Explicit dependency-ordered build over ~500 `.hl` files
  (`text_formalization/build/ocamlinit_hol_light.ml`) — a real large-proof DAG.
- `.vhl` (SSReflect source) → `-compiled.hl` artifacts throughout `formal_ineqs`
  and `formal_lp` — a **compile/cache step producing content-addressable, replayable
  proof artifacts**, matching our `proof_import` content-addressed store.
- Certificates as serialized binary blobs replayed by a verifier = **import a proof
  you did not search for and re-check it** — exactly `proof_import`'s premise.
- Multi-backend by construction: LP + text in **HOL Light**, tame graphs in
  **Isabelle**, dual-checked against each other — validates our Lean/Rocq/Isabelle
  `FormalSystem` abstraction and cross-backend import.

---

## 3. Mapping to Theoremata modules

| Flyspeck/kepler98 artifact | Theoremata module | Relationship |
|---|---|---|
| `formal_lp/LP-HL` marginal→rounded-Farkas→`eps≥0` | `cert_flyspeck_lp` (cert-log LP/Farkas) | **validates + extends** (add directional rounding + slack gate) |
| GLPK solve kept out of trusted path | cert-log design principle | validates "solver untrusted, kernel replays" |
| `formal_ineqs/verifier/certificate.hl` `result_tree` | `cert_taylor_model` (+ `cert_sos`) | **validates**; adopt the B&B tree-cert grammar |
| `formal_ineqs/arith` formal float+interval | Candle/interval numeric core | reusable design (MIT) for verified computation |
| `formal_ineqs` informal-search + formal-replay split | 3+1 gate / TTC search-then-verify | validates the two-tier pattern |
| `formal_graph/isabelle_tame` enumerate+iso-dedup+archive | subsumption/dedup, exhaustiveness/abstention | pattern to adopt |
| `text_formalization/build` DAG; `.vhl`→`-compiled.hl` | `blueprint_run` | validates dependency-ordered orchestration |
| serialized binary certs replayed | `proof_import` (content-addressed) | validates import-and-recheck |
| HOL Light + Isabelle, cross-checked | `FormalSystem` (Lean/Rocq/Isabelle) | validates multi-backend |

---

## 4. Buildable-now vs toolchain/scale-gated

**Buildable now (offline cert checkers, from the MATH — no HOL Light/Isabelle needed):**
- **Rounded-Farkas LP certificate checker** in our stack: take any LP's dual
  marginals, form the nonneg. combination, round directional (down on LBs, up on
  UBs), verify `eps ≥ 0` in exact rationals. Pure arithmetic; extends `cert_flyspeck_lp`.
  Port the algorithm from `LP.cs` (MIT — but re-implement, don't copy verbatim,
  to keep our stack clean).
- **Taylor/interval B&B certificate schema + offline checker**: adopt the
  `result_tree` (pass/glue/mono/false) grammar and re-check a search tree with our
  own interval arithmetic. Math is public; design is MIT.
- **Isomorphism-dedup + archive-comparison** enumeration utility (ideas are public).
- Ingest a `.lp`/marginal export from any external solver as an untrusted witness.

**Toolchain / scale-gated (do NOT attempt to build/run here — READ-ONLY, no builds):**
- Actually *running* `formal_lp`/`formal_ineqs` needs HOL Light + OCaml 4.01 +
  GLPK + Mono; full LP replay ≈ 15 h; certs are OCaml-4.01 binary blobs (version-locked).
- `formal_graph` needs Isabelle/AFP `Flyspeck-Tame`.
- End-to-end Kepler replay is the blueprint-scale target, not a now-task.

---

## 5. Prioritized adopt-list

1. **Directional-rounding + `eps≥0` slack gate for `cert_flyspeck_lp`** (from
   `LP-HL/LP.cs`). Small, high-value, upgrades a float dual to a sound rational
   certificate. MIT; re-implement in Rust/Python. **Do first.**
2. **`result_tree` B&B certificate grammar for the Taylor/interval verifier**
   (from `formal_ineqs/verifier/certificate.hl`): pass / glue(split) / mono / false.
   Compact, replayable, matches our cert-log; pairs with `cert_taylor_model`.
3. **Solver-untrusted / kernel-replays separation** as an explicit cert-log
   invariant (LP and nonlinear both do this) — bake into the 3+1 gate contract.
4. **Two-tier "easy/hard" difficulty-split cert store** with terminal-case cap
   (from `glpk/build_main`) — mirror in our cert store.
5. **`.vhl`→`-compiled.hl` compile-and-cache → content-addressed artifact** pattern
   for `proof_import`/`blueprint_run`.
6. **Enumerate + iso-dedup + archive-compare** exhaustiveness pattern (from
   `isabelle_tame`) for subsumption/abstention. Lower priority.
7. kepler98: **reference only, clean-room** — use its branch-and-bound strategy and
   LP formulation to sanity-check ours; never copy code (no license).

---

## Summary (~10 lines)
- Two repos: **Flyspeck** (2014 formal Kepler proof, HOL Light + Isabelle, **MIT**)
  and **kepler98** (1998 informal C++/Java code, **no license → clean-room, ideas only**).
- No injection found; all content treated as untrusted data.
- `formal_lp` is a live implementation of the **modified-dual Farkas LP certificate**:
  GLPK gives marginals, they are combined and **directionally rounded** to rationals
  with an **`eps≥0` slack gate**, then re-checked in the kernel — directly
  **validating and extending `cert_flyspeck_lp`**.
- `formal_ineqs` is a MIT **formal Taylor-model + interval-arithmetic** verifier with
  a **replayable branch-and-bound `result_tree` certificate** and an informal-search /
  formal-replay split — a ready blueprint for `cert_taylor_model`.
- `isabelle_tame` gives verified **tame-graph enumeration + iso-dedup + archive-compare**;
  the `text_formalization` build DAG and `.vhl→compiled` artifacts validate
  **`blueprint_run`/`proof_import`**.
- Buildable now (from the math, offline): the **rounded-Farkas LP checker** and the
  **B&B tree-certificate schema**. Running the originals is toolchain/scale-gated
  (HOL Light, Isabelle, GLPK, ~15 h replay) and out of scope for this read-only pass.
- Top adopt: (1) directional-rounding + slack-gate for the LP cert, (2) the
  `result_tree` nonlinear-cert grammar, (3) solver-untrusted/kernel-replays invariant.
