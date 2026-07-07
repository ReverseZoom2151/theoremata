# Resource mining — synthesis & adopt list

Exhaustive full-pass study of every repo in `resources/` (code + prose + data), done for the Theoremata harness. One report per repo in this folder. This index rolls up the **cross-cutting themes** and a single **prioritized adopt list**.

Coverage: 21 repos studied in full (5 previously-unmined + 16 previously-only-skimmed). `mathlib4` excluded (it is the retrieval corpus, not a study target). `aime24/25/26` are benchmark datasets (problem PDFs), characterized inline.

## Cross-cutting themes (what recurred)

1. **`leanblueprint` is the de-facto proof-DAG standard, and it *is* our core.** Seven formalization repos (Erdos1196, FrontierMath-Hypergraphs, RiemannHypothesisCurves, KakeyaFiniteFields, Sphere-Packing-Lean, strongpnt, ZkLinalg) all use the identical schema: theorem-env `\label{kind:slug}` = node, `\lean{FQN}` = node→Lean binding (a *separate* namespace from the label), `\uses{...}` = directed dependency edges, `\leanok` = verified flag. Two refinements our DAG lacks: `\uses` and `\leanok` are placed **independently on the statement vs inside the proof** → statement-deps ≠ proof-deps, and statement-formalized ≠ proof-done. Richer statuses exist too (`\mathlibok`/`\notready`/`\discussion`).

2. **The blueprint DAG is a *skeleton*; the executor invents 40–100% more hidden helper decls.** Measured fan-out: Kakeya 5 nodes→10 decls (2×), RHCurves 31→~55 (1.8×), strongpnt 615→1,118 (~1.8×), ZkLinalg 34→55 (~1.6×). Node granularity is a **dial** (strongpnt = trivial micro-lemmas, DAG does the reasoning; ZkLinalg = coarse paper-sized nodes). Our decomposer/executor must budget for un-blueprinted sub-node helpers rather than expect 1:1.

3. **`checkdecls` binding gate — a cheap check we're missing.** All seven blueprint repos emit `blueprint/lean_decls` (flat decl list) and run `lake exe checkdecls` in CI to verify every `\lean{}` name actually compiles — a referential-integrity gate *distinct from and cheaper than* the full proof/`#print axioms` gate. Erdos1196 even showed real drift (`lean_decls` 19 vs `decls.json` 16).

4. **Answer-matching ≠ sound proof — our central thesis, externally confirmed.** IneqMath: top models score 66–76% *answer* accuracy but **<10% overall (proof-sound)** accuracy, and scaling flattens the overall curve. Direct evidence for step/DAG verification over answer checking.

5. **Falsify-before-prove is validated everywhere, and always as *executable* code.** MathResearchPrompts (two-branch prove-or-disprove, disproof gets the same tooling), IneqMath (bound/relation reformulation, "(F) none of the above" decided by counterexample), ZkLinalg (`friRoundBadEvent` = bad event as a `Set Ω` with a measure bound), estimates (`LogLinarith` returns asymptotic counterexamples), alethfeld (`COUNTEREXAMPLE:` node + `native_decide` Lean file), DeepMath (sandboxed code execution). None hardcode; all run code.

6. **Verifier discipline / anti-sycophancy is a first-class design axis.** Recurring devices: k-consecutive-clean-verification acceptance (AgentMathMedalist: 5 clean passes, reset on any failure), Critical-Error-vs-Justification-Gap taxonomy, meta-critic that prunes false-positive bugs (AgentMathMedalist, QED Phase-4f), 7-item critic failure-mode rubric + "every proof ends with its own adversarial-check list" (MathResearchPrompts), anti-sycophancy prompts *derived from measured BrokenMath failures* (alethfeld), two-phase gated LLM-judge (QED).

7. **Math Inc's "Gauss" is the closest external precedent to Theoremata** (Kakeya, RHCurves, strongpnt, ZkLinalg, Sphere-Packing are all Gauss output). Pattern: hand-written LaTeX blueprint → agent autoformalizes → `lake build` + `checkdecls` gate, zero `sorry`. Observed infra: self-hosted `morph` CI runners, comment-free Lean (all prose in the blueprint), blueprint proofs pre-name exact Mathlib lemmas as retrieval anchors and embed anti-counterexample "Note/Key insight" warnings.

## Prioritized adopt list

### P0 — fix real bugs the full pass found in *our* code (`components/verify/hardening.rs`)
- **Replay check silently no-ops**: we shell out to `paranoia.exe` but never pass `--trust-modules Std,Mathlib`, so its kernel Replay re-verifies all of Mathlib and times out. The deepest check isn't actually running. → pass the flag.
- **Fail-open**: on unparseable paranoia output we default `clean = true`. → fail closed.
- **Target-selection bug**: `first_theorem_name` only grabs the first `theorem`/`lemma`; fixtures using `def exploit_theorem` audit the wrong constant. → resolve the actual target constant.
- **Discarded taxonomy**: we read only `success` and drop the per-check `failures` list (repair signal). → surface it.
- Wire LeanParanoia's **66-file adversarial corpus** (15 attack families + 6 valid negatives) as a CI regression suite for the gate.

### P1 — schema & gate upgrades (high leverage, low risk)
- Adopt the **`leanblueprint` dialect** (`\lean`/`\uses`/`\leanok`/`\proves` + `lean_decls`) as our blueprint interchange format → free interop with the Lean-community tooling, PDF/web dep-graph render, and a second existence check.
- Add the **statement-vs-proof split** to the DAG edge/status schema (statement-deps vs proof-deps; statement-formalized vs proof-done).
- Add a **`checkdecls`-style node-binding gate** between blueprint and compile.
- Make the decomposer **budget for hidden-helper fan-out** (expect ~1.8× decls per node); expose node granularity as a config dial.

### P2 — reasoning/verification craft (prompts + control)
- **QED**: `plan_history.md` append-only cross-attempt strategy memory ("Do NOT try again"), verification-phase prior into the selector, mechanical budget-exhaustion escalation separate from the semantic regulator, structured `<cite>`/`<key-original-step>` tags.
- **AgentMathMedalist**: k-consecutive-clean acceptance rule, Critical-Error/Justification-Gap taxonomy, meta-critic prune layer, "report significant partial results, never guess" solver contract.
- **MathResearchPrompts**: typed-claim & transfer-schema enums for the decomposer, 7-item critic rubric, reparameterization ("intrinsic vs coordinate-artifact") gate, proof-carries-its-own-adversarial-checks output contract, prove-or-disprove two-branch, "never fabricate references — flag as an explicit obligation".
- **alethfeld**: three-valued **taint** (`clean`/`tainted`/`self-admitted`) with executable propagation over the subtree, 25-keyword fixed justification vocabulary, flaws encoded in the graph's own vocabulary (rejected-QED + verified COUNTEREXAMPLE node, no special error field), ready graph algorithms (topo-sort, scope algebra, acyclicity-with-cycle-path, lemma-extraction benefit metric).

### P3 — new capability modules
- **estimates**: port the `Θ`/`OrderOfMagnitude` SymPy semiring + `LogLinarith` (asymptotic goal → LP over exponents, integrality-gap trick, returns counterexamples) + `linprog.py` exact-rational LP with **Farkas/dual certificates either way** into our asymptotic/feasibility layer. Deterministic, explainable.
- **DeepMath**: harden our executable-falsifier sandbox with its subprocess hard-kill + pickle→thread fallback, import allow-list + "Import not allowed" hinting, and a **global token-budget governor** (decouple #turns from total tokens, graceful termination).
- **AutoMathText**: LM-as-scorer (affirmative-token logprob) as a retrieval reranker / SFT-data curator via our LiteLLM logprobs path (method is citation-only — fetch arXiv 2402.07625 to implement). Dataset is EULA-restricted.
- **mathcode**: per-stage model routing (formalize_plan/formalize/formalize_eval/prove_plan/prove each independently routable), plan-level (not just proof-level) best-of-N, `-- @stored-theorem` compilable lemma-cache, proof telemetry (tactic histogram) for ranking DAG nodes; lift `_lean_masking.py` (nested comment/string masker) and the YAML-frontmatter tool contract verbatim.

### P4 — benchmarks & eval fixtures to ingest
- **Formalization track** (blueprint→Lean, graded by compile + axiom-whitelist `[propext,Quot.sound,Classical.choice]` + statement-match): FormalQualBench (23 qual stubs), Sphere-Packing (82 live `sorry`s), ZkLinalg (34 nodes, fast smoke), strongpnt (615 nodes, scale stress), Kakeya, RiemannHypothesisCurves, FrontierMath-Hypergraphs (honesty test — partial result), Erdos1196. Same tooling across domains = a clean eval matrix.
- **NL / answer track**: IneqMath (dev 100 / train 1,252, bound+relation, deterministic-rubric grader), AIME 24/25/26, estimates (~30 named problems with informal↔SymPy↔expected).
- **Falsification / critic track**: alethfeld `brokenmath/` (10 problems, ground-truth JSON, baseline 5/10 @ 0 false-positives), goldbach-collatz (negative fixture — empty crank artifact the pipeline must reject).
- **Hardening track**: LeanParanoia 66-file adversarial corpus.

## Per-repo reports
Erdos1196 · FrontierMathOpen-Hypergraphs · RiemannHypothesisCurves · KakeyaFiniteFields · M4R_Thesis · DeepMath · QED · estimates · LeanParanoia · mathcode · MathResearchPrompts · AgentMathOlympiadMedalist · FlashSampling · ineqmath · alethfeld · Sphere-Packing-Lean · strongpnt-and-ZkLinalg · AutoMathText-and-goldbach-collatz · FormalQualBench
