# Resource Mining: MathResearchPrompts

Full-pass extraction of `resources/MathResearchPrompts-main/MathResearchPrompts-main`.
This is a curated prompt library accompanying two arXiv papers by the PKU/BICMR group
(Lai, An, Hu, Wen et al.):

- Case study: *A Grover-compatible Manifold Optimization Algorithm for Quantum Search*, arXiv:2512.08432.
- Methodology: *Advancing Mathematical Research via Human–AI Interactive Theorem Proving*, arXiv:2512.09443.

All file paths below are under
`C:\Users\adria\Downloads\math-agent\resources\MathResearchPrompts-main\MathResearchPrompts-main\`.

---

## 1. What it is (scope, structure, prompt organization)

A small, prompt-heavy repo (no build system of its own — the Lean folder is a Lakefile
project used only to compile the case-study proof). Non-artifact contents:

- `README.md` — the **actual, verbatim prompts** used in the Grover case study, organized
  by research phase, plus four "nano-banana" cartoons narrating the human–AI loop.
- `prompt_templates.md` — the **domain-neutral, reusable template library** (12 templates)
  distilled from those case-study prompts. This is the reusable IP.
- `materials/numerical/retraction_checks.py` — the executable finite-difference verifier
  (P2/P3 checks) that screens synthesized constructions before proof.
- `materials/formal/…` — a Lean 4 + Mathlib project that formally verifies the paper's
  key algebraic lemmas (the "Lean compile + axioms gate" analog).

Two-layer structure worth internalizing: **README = concrete instantiated prompts**;
**prompt_templates.md = the same prompts abstracted into slot-filled templates** with an
explicit instruction that slots must be filled with *substantive expert knowledge*, not
left vague. This concrete→template pairing is itself a design lesson: keep a canonical
worked example beside every abstract role prompt.

The whole library encodes one workflow the papers call **propose → check → distill**
(also "propose-check-distill"), executed as a human–AI loop with a web platform that
**executes generated Python and feeds results back to the model** (README lines 382–383).

### The Grover case study as a concrete end-to-end trace
The prompts are not abstract — they walk one real research project, which is a perfect
model of a Theoremata run:
1. Scope the quantum-algorithms × Riemannian-optimization interface.
2. Frame Grover's algorithm as Riemannian gradient flow on U(N); objective
   `f(U)=Tr(H U ψ₀ U†)`.
3. Prove the gradient identity `grad f(U) = [H, ψ_U]` and the stationarity⇔extremal claim.
4. **Discovery**: model surfaces the invariant 2D "Grover plane" `span{|ψ₀⟩, H|ψ₀⟩}` →
   reduces dynamics to SU(2) → yields closed-form scalar recursions (the "aha" theorem).
5. **Property-constrained synthesis**: build the 5-factor product retraction satisfying
   (P1) product form, (P2) identity at t=0, (P3) correct initial velocity — screened
   numerically, then proved.
6. Constant discovery (retraction bounds α, β), rate refinement (sublinear → linear via PL).
7. Formalize the load-bearing lemmas in Lean.

---

## 2. Reusable ideas/patterns — catalogue mapped to our roles

Priority section. Each entry: the technique, verbatim key prompt text, and the Theoremata
role(s) it feeds. Our roles: **conjecturer, decomposer, formalizer, critic, falsifier**
(plus retrieval and the orchestration loop).

### 2.1 Cross-domain scoping / direction generation → CONJECTURER
`prompt_templates.md` §1 "Scoping and Topic Conceptualization"; README "Early-stage Exploration".
Generates 5–8 analyzable candidate directions at the interface of two domains, each with
*feasibility notes* including how it might fail.

Verbatim (template §1, lines 21–27):
> "Tasks:
> 1. Restate the relevant objects on both sides using the supplied domain notation.
> 2. Propose 5-8 candidate research directions. For each direction, list the core idea,
>    objects, assumptions, nearest known results, possible gap, and likely proof tools.
> 3. For each direction, include feasibility notes: degenerate cases, boundary examples,
>    expected theorem or complexity guarantee, and reasons the statement might fail.
> 4. Rank the directions by mathematical clarity, novelty, and verifiability.
> Output: a compact numbered list. Avoid speculative claims disconnected from the supplied
> domain theory."

Design signal: every candidate carries its own failure-mode + verifiability ranking. Our
conjecturer should emit directions already scored for falsifiability, not just novelty.

### 2.2 Sharpened claims + validation plan → CONJECTURER → CRITIC handoff
`prompt_templates.md` §2; README "Sharpened Claims and Validation Plans: Example 1 & 2".
Turns a rough idea into checkable theorem candidates and, critically, classifies each.

Verbatim (template §2, lines 46–48):
> "3. Propose candidate theorem statements and validation checks that distinguish
>    intrinsic structure from coordinate artifacts or implementation artifacts.
> 4. Classify each candidate as ready for proof, suitable as a conjecture, or likely too broad.
> Output: theorem candidates, validation checks, and a proof-feasibility assessment."

The "intrinsic vs coordinate/implementation artifact" check (README Ex.2 line 94:
"ensure the observed structure is intrinsic to the geometry (coordinate-independent)
rather than an artifact of a specific parameterization") is a reusable **critic** gate:
does the claimed structure survive reparameterization?

### 2.3 Rigorous direct proof, no fabricated references → FORMALIZER / prover
`prompt_templates.md` §3; README "Prompts and Reply of Section 4.1".
The core prover system prompt. This is the single most portable text in the repo.

Verbatim (README lines 107–111, the system message):
> "You are a careful mathematical assistant collaborating with human experts.
> When you are asked to prove a statement, you must:
> 1. restate all assumptions, notation, and domains of all variables;
> 2. give a numbered, line-by-line argument in which every non-trivial inference is
>    explicitly justified;
> 3. cite standard results by their usual names (e.g. spectral theorem, Cauchy–Schwarz
>    inequality) and never fabricate new references."

Output-format clamp (README lines 126–131):
> "Return only: (1) a LaTeX `proof` environment with a numbered, step-by-step derivation;
> (2) a short LaTeX `itemize` list of adversarial checks a reader could perform (for
> example: degenerate spectrum of H, special choices of ψ₀, boundary cases where H=0 or
> H=I, etc.). Do not output any text outside these two parts."

Two reusable mechanisms: **anti-hallucination clause** ("never fabricate new references";
template §3 line 55 strengthens it: "If a required fact is not supplied or standard, flag
it as a proof obligation instead of inventing a citation") and **proof-carries-its-own-
adversarial-checks** (every proof must end with an itemized list of stress tests the critic
runs). Both belong in our formalizer output contract.

### 2.4 Prove-or-disprove two-branch workflow → FALSIFIER (this is our "falsify-before-prove")
`prompt_templates.md` §4; README "Clear target, uncertain truth".
The single most important match to our architecture: forces a **proof branch and a disproof
branch in parallel**, then a verdict.

Verbatim (README lines 141–146):
> "2. Run two explicit branches:
>    - Proof branch: Try to show that [statement]… Use concrete constructions, basis
>      expansions, or dimension arguments.
>    - Disproof branch: In parallel, look for structural obstructions or counterexamples.
>      Analyse the subspace spanned by such commutators, check invariants (e.g. trace,
>      eigenvalues, rank), and probe whether some elements … might fail…
> …
> 5. …provide a final verdict: decide whether the proposition is true or false. If it is
>    true, present a concise but fully rigorous proof; if it is false, present an explicit
>    counterexample and explain why it fails."

Template §4 (lines 84–88) generalizes the disproof toolkit: "search for obstructions,
counterexamples, boundary cases, hidden assumptions, and incompatible known results" and
"identify any step whose validity depends on an extra assumption. End with a final verdict:
proved, disproved, or unresolved under the stated assumptions."

Key design note (template §4 line 77): *"The disproof branch should be as domain-informed
as the proof branch."* Our falsifier must be given the same domain tooling as the prover,
not a weaker generic one.

### 2.5 Candidate-conclusion discovery (fixed assumptions, unknown conclusions) → CONJECTURER
`prompt_templates.md` §5; README "AI-Guided Proposal of Candidate Conclusions".
Assumptions fixed, conclusions open. Generates *typed* candidate claims each with a quick
falsifier, then distills a coherent subset into a numbered proposition.

Verbatim (template §5, lines 105–109):
> "2. Propose concise candidate statements. Tag each as invariant, norm identity, scalar
>    recursion, spectral property, convergence guarantee, stability statement, normal form,
>    obstruction, counterexample family, or another useful type.
> 3. For each candidate, explain why it might be useful and what quick check could falsify it.
> 4. Select a coherent subset of surviving statements and package them into a theorem-style
>    proposition with numbered parts.
> …
> Return: candidate list with pass/fail/inconclusive labels, final proposition, proof
> sketch, and adversarial checks."

README version (lines 230–233) adds the local/global spread requirement: candidates must
cover "both local information (e.g. behavior in the Grover plane, small-step expansions)
and global information (e.g. long-time dynamics, extremal behavior…)". The **type-label
taxonomy** here is directly reusable as node/claim tags in our proof-DAG (see §3).

### 2.6 Transfer-schema extraction / cross-setting transfer → DECOMPOSER + CONJECTURER
`prompt_templates.md` §6; README "Human Abstraction and Cross-Setting Transfer".
Extract the *reusable structural mechanism* of a proven result (not its surface wording),
mark essential vs relaxable assumptions, and re-instantiate in a nearby setting.

Verbatim (template §6, lines 123–128):
> "1. Extract the reusable structural schema from the baseline result, such as invariant
>    subspaces, conserved quantities, tangent or feasible directions, scalar progress
>    coordinates, admissible updates, comparison inequalities, and proof mechanism.
> 2. State which assumptions appear essential and which may be relaxed.
> 3. Transfer the schema to the new setting by proposing analogous statements.
> 4. For each proposed transferred statement, list required checks and likely failure modes.
> 5. Provide a theorem-ready statement only for claims that survive the stated checks."

README instantiation (lines 283–288) names the four schema ingredients explicitly:
"(1) the working invariant subspace; (2) the associated 'gradient plane' or tangent plane;
(3) the scalar progress coordinate; (4) the structured local update." This is a strong
**decomposition primitive**: reduce a theorem to (invariant subspace, progress coordinate,
local update, comparison inequality) — a schema our decomposer can reuse as sub-node types.

### 2.7 Property-constrained object synthesis (construct-then-verify) → CONJECTURER + FALSIFIER + verifier tool
`prompt_templates.md` §7; README "Property-Constrained Synthesis".
Build an object meeting a fixed property list (P1)…(P3), propose candidates of increasing
length, **write and RUN executable code** to screen them, keep the shortest that passes,
then give a symbolic proof plan.

Verbatim (template §7, lines 147–155):
> "2. Propose candidate constructions of different sizes or complexities, including short
>    candidates that may fail.
> 3. For each candidate, write executable Python, MATLAB, or symbolic-check code for finite
>    tests on random and structured seed cases.
> 4. Include edge cases supplied by the domain expert: [insert edge cases].
> 5. Report pass/fail outcomes and explain failures.
> 6. Select the simplest candidate that passes the tests and provide a symbolic proof plan
>    for (P1)-(P3).
> Numerical tests are screening evidence only. A symbolic proof or a clearly stated proof
> obligation is required before acceptance."

README shows the concrete finite-difference falsifier the model wrote and the platform ran
(lines 356–367): check (P2) exactly at t=0, check (P3) by
`‖(γ(h;x,y)−I)/h − (xX₀+yY₀)‖_F ≤ ε`, report pass/fail per seed including edges (x,0),(0,y).
`materials/numerical/retraction_checks.py` is the cleaned-up executable version — a template
for a **model-derived numerical falsifier** (matches our "model-derived falsification,
executable check" commit history exactly).

### 2.8 Constant discovery + numerical stress testing → CONJECTURER + FALSIFIER
`prompt_templates.md` §8; README "AI-assisted coefficient exploration".
Find sharp constants in an inequality via a **mandated order**: propose → test → prove →
discuss tightness.

Verbatim (template §8 required order, lines 172–177):
> "1. Propose candidate constants and explain the heuristic using the supplied domain structure.
> 2. Design and perform numerical or symbolic stress tests over random and structured cases.
> 3. Adjust the constants if the tests reveal violations.
> 4. Give a theoretical proof using the explicit construction and standard inequalities.
> 5. Discuss tightness and list worst-case or near-worst-case examples."

README enforces the ordering as a hard contract (line 505): *"It is enough to clearly follow
the order 'propose constants → perform numerical tests → give a theoretical proof → discuss
tightness'."* The **propose→test→prove→tightness pipeline** is a reusable loop shape.

### 2.9 Structure-specific rate/complexity refinement → CONJECTURER (sharpening) + prover
`prompt_templates.md` §9; README "Refining the Theorem".
Take a baseline (generic) bound and beat it by exploiting problem-specific structure
(PL / KL / error-bound / sharpness / invariant-region inequality).

Verbatim (template §9, lines 191–198):
> "2. Search for a structure-specific inequality relating the progress measure to the target
>    gap or residual.
> 3. State the exact region where the refined inequality is expected to hold.
> 4. Use the baseline descent/ascent/smoothness inequality and the refined inequality to
>    derive the improved iteration or complexity bound.
> 5. List boundary cases and stress tests that could falsify the proposed constants.
> Return a theorem-ready statement and a proof skeleton. Do not hide local assumptions
> behind vague phrases such as 'good initialization'."

README instance (lines 542–545) asks: does the objective's algebraic structure imply a
"stronger geometric property than simple smoothness" → linear instead of sublinear rate.
The **anti-vagueness clause** ("Do not hide local assumptions behind vague phrases") is a
reusable critic rule.

### 2.10 Prompt-sensitivity / ablation harness → CRITIC (meta-evaluation)
`prompt_templates.md` §10 "Prompt Variation and Failure-Mode Check".
Run the *same task* under three prompt variants (A full, B missing-assumption, C evidence-
only) and record failure modes. A built-in self-eval / regression check.

Verbatim failure-mode checklist (lines 222–229):
> "1. notation drift; 2. hidden assumption changes; 3. unsupported claims; 4. fabricated or
> vague references; 5. boundary cases omitted; 6. whether numerical evidence is overstated
> as proof; 7. whether the absence of domain-specific expert knowledge makes the output too
> generic to verify."

This 7-item list is a ready-made **critic rubric** / automated grading schema for any proof
or claim node. Item 6 ("numerical evidence overstated as proof") is exactly the gate that
separates our falsifier's screening evidence from the formalizer's accepted proof.

### 2.11 Computational environment / provenance log → orchestration + reproducibility
`prompt_templates.md` §11. A required metadata record per run (task name, prompt family +
version, full prompt text, domain info inserted, model name+snapshot, API/local, temperature
+ decoding, hardware, tool/package versions, accepted output, rejected/failure notes, human
verification steps, unresolved proof obligations). This is a schema for our **run/node
provenance** — see §3.

### 2.12 Second-case-study scoping / generalization protocol → orchestration
`prompt_templates.md` §12. Meta-template for deciding whether the whole workflow transfers to
a new problem; requires author-supplied objects/known results/admissible ops/deliverable/
verification tools before claiming validation. Encodes "do not claim validated until prompts
are actually run and outputs verified" (line 285) — a discipline our loop should enforce
before marking an open-problem node solved.

### 2.13 The "aha" mechanism worth capturing → CONJECTURER discovery pattern
README "Rich proofs and aha-moment" (lines 176–184) records the *model output* that triggered
the key theorem: recognizing evolution is confined to an invariant 2D subspace ("Jordan/Grover
plane"), reducing to SU(2), yielding closed-form scalar recurrences. The reusable heuristic:
**look for a low-dimensional invariant subspace that collapses high-dimensional matrix dynamics
to scalar recursions.** Worth encoding as a conjecturer prompt hint ("search for invariant
subspaces / conserved quantities that reduce dimensionality").

---

## 3. Taxonomy / schema

Two explicit schemas are reusable directly as DAG node/claim metadata:

**(a) Claim/statement type labels** (template §5 line 106; README line 231):
`invariant · norm identity · scalar recursion · spectral property · convergence guarantee ·
stability statement · normal form · obstruction · counterexample family`. → adopt as an
enum tag on conjecture/claim nodes in the proof-DAG.

**(b) Transfer schema ingredients** (template §6; README lines 284–287):
`working invariant subspace · gradient/tangent plane · scalar progress coordinate ·
structured local update · comparison inequality · admissible updates`. → sub-node types a
decomposer emits when reducing a convergence/optimality theorem.

**(c) Verdict enum** (template §4 line 88): `proved · disproved · unresolved (under stated
assumptions)`. → the falsifier/critic verdict field.

**(d) Evidence-status enum** implicit throughout: `screening evidence (numerical) → proof
obligation → symbolic proof / formal (Lean)`. Template §7 line 155 and §10 item 6 make the
numerical-vs-proof distinction load-bearing. → node acceptance ladder.

**(e) Formalization decomposition** (Lean docstrings): the informal proof was broken into
numbered **"Formalization Targets" (Target 1–5)** in an (unshipped) `FormalizationTargets.md`
/ `formal_verification_statements.md`, each an independently Lean-provable substatement
(commutator trace identity; invariant plane; Frobenius norm bridge; five-factor zero-step;
Hermitian commutator inclusion; PL scalar convergence). → this is a concrete **decomposer→
formalizer contract**: reduce a paper theorem to a list of small, individually-compilable
Lean targets before attempting formalization.

**(f) Run provenance schema** (template §11): the 13-field log above → our run/experiment
record.

---

## 4. What our earlier TARGETED pass MISSED

The earlier skim caught the high-level roles. The full pass surfaces these concretes that
were not previously extracted:

1. **The verbatim prover system prompt** (README 107–111) with its exact anti-fabrication
   wording and the "flag as proof obligation instead of inventing a citation" escalation
   (template §3 line 55). This is copy-paste-ready.
2. **The full 12-template library** in `prompt_templates.md` — earlier notes referenced the
   README prompts but not the abstracted, slot-filled templates, which are the actually
   reusable form and include four templates with no README counterpart (§8 constants, §9
   rate refinement partially, §10 ablation harness, §11 env log, §12 second-case scoping).
3. **The 7-item failure-mode rubric** (§10, lines 222–229) — a ready critic grading schema.
4. **The claim-type-label taxonomy and transfer-schema ingredient list** (§3 above) — usable
   as DAG enums.
5. **The executable model-derived falsifier** (`retraction_checks.py` + README 383) and the
   platform pattern of *running generated code and feeding results back* — direct match to
   our "model-derived falsification (executable check, not hardcoded)" work, previously not
   tied to this concrete example.
6. **The Lean "Formalization Targets" decomposition discipline** — the informal proof was
   split into 5+ named, individually-compilable Lean targets (docstrings across
   `materials/formal/MathlibProject/GlobalOptimality/*.lean`). Concrete blueprint for our
   decomposer→formalizer→compile-gate chain, including the axioms-clean style (theorems built
   only from Mathlib lemmas, no `sorry`, no fabricated axioms).
7. **The mandated-ordering contract** ("propose → test → prove → tightness", §8/README 505;
   "propose constants → numerical → proof", §9) — an explicit sequencing constraint we can
   bake into loop stage-gating rather than leaving to the model.
8. **The "intrinsic vs coordinate/implementation artifact" critic check** (README 94,
   template §2) — a reparameterization-invariance gate not previously noted.
9. **The concrete–template pairing convention** (README = worked example, prompt_templates =
   abstraction) as a documentation/prompt-authoring pattern.

---

## 5. Which prompts to PORT into provider role-routing

Recommended concrete ports (source → target role prompt):

| Source | Target role | What to port |
|---|---|---|
| `prompt_templates.md` §3 + README 107–131 | **formalizer / prover** | System prompt: restate assumptions → numbered line-by-line proof → cite standard results by name, **never fabricate**, flag missing facts as proof obligations. Output contract: proof + itemized adversarial checks, nothing else. |
| `prompt_templates.md` §4 + README 137–169 | **falsifier** | Two-branch proof/disproof with parallel obstruction search; disproof branch gets the *same* domain tooling as proof; verdict enum {proved, disproved, unresolved}. This is our falsify-before-prove core. |
| `prompt_templates.md` §1 + README 47–66 | **conjecturer** | 5–8 directions, each with objects/assumptions/nearest results/gap/proof-tools/feasibility + failure reason, ranked by clarity·novelty·verifiability. |
| `prompt_templates.md` §5 + README 188–241 | **conjecturer** | Fixed-assumption candidate-conclusion discovery with type labels + per-candidate falsifier + distill-to-proposition; require local+global coverage. |
| `prompt_templates.md` §6 + README 243–305 | **decomposer** | Transfer-schema extraction: reduce theorem to (invariant subspace, progress coordinate, local update, comparison inequality); mark essential vs relaxable assumptions. |
| `prompt_templates.md` §7 + `retraction_checks.py` | **falsifier + synthesis tool** | Property-constrained synthesis: propose candidates of increasing length → model writes & runs finite-difference screening code → keep shortest passing → symbolic proof plan. Wire the executable-check-feedback loop. |
| `prompt_templates.md` §10 (7-item list) | **critic** | Failure-mode rubric as the critic's grading schema for any proof/claim node. |
| `prompt_templates.md` §2 + README 84–96 | **critic** | Intrinsic-vs-artifact / coordinate-independence gate; classify each claim ready-for-proof / conjecture / too-broad. |
| `prompt_templates.md` §8, §9 | **conjecturer (sharpening) + prover** | Mandated propose→test→prove→tightness ordering; structure-specific inequality search (PL/KL/error-bound); anti-vagueness clause on local assumptions. |
| `prompt_templates.md` §11 | **orchestrator** | Per-node run provenance schema (model snapshot, temperature, tool versions, accepted/rejected, open proof obligations). |

Cross-cutting output conventions to bake into ALL role prompts:
- **"Never fabricate references/lemmas; flag unknown facts as explicit proof obligations."**
- **Every proof/claim ends with an itemized adversarial-check list** (degenerate spectrum,
  boundary cases H=0/H=I, special ψ₀, notation checks, citation-status checks).
- **Numerical/heuristic evidence is screening only; never accept it as proof** (feeds the
  falsifier↔formalizer gate).
- **Restate all assumptions, notation, and variable domains before reasoning.**
- **Fill domain slots with substantive expert knowledge**, or the checks degrade to unusable
  generic advice (prompt_templates.md lines 3–5).

---

## 6. New vs. already-in-our-design

**Already central to our design (this repo confirms/validates):**
- Falsify-before-prove → their two-branch prove-or-disprove (§4). Strong external validation.
- Model-derived executable falsification → their run-generated-code-and-feed-back loop and
  `retraction_checks.py` (§7). Matches our recent commits precisely.
- Formalize best-of-N → Lean compile + axioms gate → their `FormalizationTargets` decomposition
  + `sorry`-free Mathlib proofs (`materials/formal/…`).
- Role split conjecturer/decomposer/formalizer/critic/falsifier → all five appear.
- Retrieve/anchor to known results, no fabricated citations → §3 anti-fabrication clause.

**New / refinements to absorb:**
- **Claim-type-label taxonomy** and **transfer-schema ingredient list** as DAG enums (§3) —
  gives our nodes richer typed structure than we currently have.
- **The 7-item critic failure-mode rubric** — a concrete grading schema we lack.
- **Mandated stage-ordering contracts** (propose→test→prove→tightness) enforced by the loop
  rather than the model.
- **Intrinsic-vs-coordinate-artifact / reparameterization gate** — a novel critic check.
- **Slot-filled template + concrete worked-example pairing** as a prompt-authoring convention
  for our provider role prompts (keep a canonical instantiation beside each abstract role).
- **Proof-carries-its-own-adversarial-checks output contract** — make it a hard schema field
  on formalizer/prover outputs, consumed by the critic.
- **Decompose informal proof into named, individually-Lean-compilable "Targets"** before
  formalization — an explicit decomposer→formalizer protocol.
- **Structure-specific rate refinement** (§9: beat a generic smoothness bound via PL/KL/
  error-bound) — a concrete conjecturer sharpening move for optimization/analysis problems.
- **Per-run provenance schema** (§11) — reproducibility metadata for each node/run.

---

### File index
- `README.md` — verbatim case-study prompts (scoping, sharpened claims, prover, prove-or-
  disprove, candidate discovery, transfer, synthesis, coefficient/constant exploration,
  theorem refinement) + the "aha" model output.
- `prompt_templates.md` — 12 reusable domain-neutral templates (§1–§12).
- `materials/numerical/retraction_checks.py` — executable (P2)/(P3) finite-difference falsifier.
- `materials/formal/MathlibProject/GlobalOptimality/*.lean` — Lean/Mathlib formalization of
  the case-study lemmas (commutator trace identity, invariant Grover plane, Frobenius norm
  bridge, five-factor zero-step retraction, Hermitian commutator inclusion, PL scalar
  convergence, stationarity↔extremal), decomposed from `FormalizationTargets.md` /
  `formal_verification_statements.md` (referenced, not shipped).
- `fig/1–4.png` — narrative cartoons of the human–AI loop (no prompt content).
