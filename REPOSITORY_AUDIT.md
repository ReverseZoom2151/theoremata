# AI-for-Mathematics Workspace Audit

## Scope and method

This audit covers every top-level repository in the workspace. Authored documentation,
configuration, prompts, schemas, runtime source, tests, benchmark definitions, Lean entrypoints,
and representative proof artifacts were reviewed. Bulk artifacts were handled as follows:

- Mathlib was analyzed by module taxonomy, tactic surface, declaration volume, and integration API.
  Its 8,245 library modules contain about 2.27 million lines and 225,000 declarations; individual
  theorem-by-theorem review is outside the useful scope of a harness architecture audit.
- Repeated benchmark JSON, generated result files, compiled artifacts, PDFs duplicating source
  documents, images, and lockfiles were classified and sampled rather than interpreted line-by-line.
- Claims made by a README are distinguished from behavior visible in checked-in source.
- A repository containing a binary but not its source is marked as only partially auditable.

## Agent and harness systems

### AgentMathOlympiadMedalist

The intended design is an informal olympiad solver with initial generation, self-improvement,
repeated verification, bug review, correction, and a five-consecutive-pass acceptance rule.
Prompts encode most of this workflow. The checked-in Python runtime is substantially thinner than
the README implies: it wraps Gemini/OpenAI-compatible models using LangGraph/A2A infrastructure,
has no meaningful tool set, and includes unrelated exchange-rate progress strings. Model access
tests run at import time. This is useful as a prompt-loop specimen, not as a production foundation.

Reusable idea: consecutive independent verification passes. Main weakness: verifier outputs are
model judgments without grounded proof-state evidence.

### DeepMath

DeepMath is the strongest example of training and inference being designed together. A custom
SmolAgents `CodeAgent` emits small Python snippets, executes them under import and timeout
constraints, and returns observations to the model. The repository modifies vLLM generation and
TRL/GRPO integration so tool-using trajectories can be generated during training. Rewards combine
answer correctness with tool-call format/use, and evaluation includes majority voting and output
length.

The execution boundary is more careful than a raw Python REPL but not a complete security sandbox:
hard process termination depends on picklability, otherwise execution falls back to an unkillable
daemon thread. Rewarding the presence of code independently of its necessity creates a possible
reward-hacking incentive.

Reusable ideas: executable reasoning traces, training/inference parity, majority metrics,
temperature scheduling, and concise deterministic computation.

### QED

QED is the most complete natural-language research pipeline. It performs a literature survey,
difficulty classification, proof decomposition, proof generation, structural verification,
detailed verification, regulator-directed retries, and a final proof-effort summary. Its retry
hierarchy distinguishes revising a proof, revising a plan, and replacing the decomposition.
Persistent files provide resumability, token accounting, status, and trajectory logs. Roles can be
routed independently to Codex, Claude, or Gemini CLIs.

Its primary verification remains LLM-mediated. Citations and proof structure are inspected, but the
system does not establish kernel-level truth. File-oriented coordination is transparent but weakly
typed; verdict parsing and fallback behavior can silently convert malformed model behavior into
apparently valid artifacts. The proved-statement folders are valuable trajectories and expert
review examples, not machine-checkable certificates.

Reusable ideas: hierarchical retry policy, artifact persistence, role-specific model routing,
resume detection, structural versus detailed review, and explicit proof-effort reports.

### Alethfeld Legacy

Alethfeld has the strongest explicit intermediate representation. Proofs are semantic DAGs stored
in EDN with typed nodes, dependencies, scopes, discharged assumptions, external references,
verification status, provenance, content hashes, and taint. The Clojure CLI implements atomic
graph mutations, cycle detection, topological sorting, scope validation, taint recomputation,
lemma extraction, node replacement, statistics, and schema validation. Tests cover operations,
boundaries, concurrency, properties, and realistic integrations.

The agent workflow separates adviser, prover, adversarial verifier, reference checker, decomposer,
formalizer, and orchestrator roles. Its main limitation is that a structurally valid graph is not a
mathematically valid graph; semantic acceptance still depends on model or human review until Lean
formalization succeeds. It is archived, but its graph model is the best architectural substrate in
the workspace.

Reusable ideas: proof DAG as source of truth, taint propagation, assumption scope, atomic graph
operations, extractable lemmas, explicit provenance, and adversarial verification.

### MathCode

MathCode presents a terminal coding-agent experience specialized for natural-language-to-Lean
formalization. The distributable runtime is binary, so its central orchestration cannot be audited
from this workspace. The visible surface includes a Lean workspace, plugin/skill/MCP extension
model, stored-proof library search, proof statistics, comment/string-aware placeholder detection,
and checks against local axioms, constants, postulates, `sorry`, and `admit`.

Its Python checks are useful fast filters but are lexical rather than kernel-level soundness
audits. They should precede, not replace, compilation, comparator-based challenge checking, and
dependency verification.

### Estimates

Estimates is a narrow symbolic proof assistant implemented over SymPy. It provides explicit proof
states, tactics, proof trees, simplification, substitution, propositional reasoning, linear
arithmetic, logarithmic linear arithmetic, asymptotic orders, boundedness, and Littlewood–Paley
domain primitives. The UI exposes proof construction as a graph.

This demonstrates the value of domain-specific intermediate verifiers: many analytical obligations
are easier to express and automate here than in general Lean. Its trusted base is Python/SymPy and
custom tactic code, so it supplies evidence rather than foundational certification.

## Verification and benchmark systems

### LeanParanoia

LeanParanoia performs transitive dependency analysis and checks for `sorry`, metavariables, unsafe
or partial declarations, unapproved axioms, extern/implemented-by/csimp substitutions, native
computation primitives, constructor and recursor integrity, source patterns, and environment
replay through lean4checker. Its exploit-focused test suite is unusually extensive.

This is the strongest final hardening layer in the workspace, but its own README correctly notes
that it is not a complete soundness guarantee. Challenge/solution isolation with Comparator and an
OS sandbox remains preferable for untrusted submissions.

### FormalQualBench

FormalQualBench contains 23 compact Lean challenge modules spanning graduate qualifying and major
theorem statements. Each has the definitions/imports needed to state one `MainTheorem` followed by
`sorry`. It recommends Comparator plus Landrun, with only `propext`, `Quot.sound`, and
`Classical.choice` permitted.

Strength: fast formal-agent iteration over broad mathematical domains. Limitation: statements vary
greatly in difficulty and may be solvable through existing Mathlib theorems, so results require
contamination and theorem-alias analysis.

### IneqMath

IneqMath reformulates inequality proofs into automatically judged bound-estimation and relation-
prediction tasks. Data records carry problem, choices, answer, solution, theorem annotations,
difficulty, split, and task type. The repository includes provider adapters, answer scoring,
few-shot and retrieval-style hints, critic/self-improvement experiments, reformulation scripts,
training-data enhancement, and extensive saved outputs.

Its most useful contribution is layered informal evaluation: answer correctness plus several
step-quality dimensions. Limitations include LLM judges for fine-grained reasoning, benchmark-
specific prompt engineering, and many checked-in result files that are evidence rather than
reusable runtime components.

### AIME 2024/2025/2026

These repositories contain project pages and PDF reports, not the full machine-readable datasets
advertised on Hugging Face. They are answer-level competition benchmarks with objective numeric
grading, useful for regression testing but weak evidence for proof reliability.

### AutoMathText 2.5

This checkout contains metadata, licensing, and a landing page only. The claimed two-trillion-token
corpus is external and cannot be audited here. It is a potential training/retrieval source, not a
harness component.

## AI-assisted mathematical research and formalization

### MathResearchPrompts

This repository documents a human–AI research workflow: topic exploration, claim sharpening,
counterexample/numerical checks, theorem formulation, proof development, and later Lean
formalization. The included numerical code and roughly 5,000 lines of placeholder-free Lean show
that free-form research can be progressively hardened.

Key lesson: numerical falsification and coordinate/artifact checks should occur before expensive
proof work, and the system should retain the evolution from intuition to formal statement.

### Erdos1196

Approximately 4,000 lines of placeholder-free Lean formalize a primitive-set estimate using
analytic normalization, a sub-Markov chain, first-entry weights, and hit-mass bounds. The source
and blueprint expose a well-factored theorem dependency chain.

### FrontierMathOpen-Hypergraphs

Approximately 7,500 lines of placeholder-free Lean formalize finite and asymptotic hypergraph lower
bounds. The development separates basic definitions, substitution machinery, uniform finite
bootstrap, and Lubell asymptotics. It originated from an informal frontier-model proof and includes
paper/blueprint artifacts.

### KakeyaFiniteFields

A compact, placeholder-free 315-line Lean formalization of the polynomial-method Kakeya bound.
Its short `step1`–`step4` decomposition is a useful example of blueprint granularity aligned with
formal obligations.

### RiemannHypothesisCurves

About 4,000 placeholder-free Lean lines implement the Bombieri–Stepanov route for hyperelliptic
curves: Hasse derivatives, non-square facts, degree bounds, constraint-space dimension, polynomial
construction, vanishing, and the final point-count estimate.

### ZkLinalg

About 2,100 placeholder-free Lean lines formalize linear-algebraic and probabilistic components of
FRI soundness: zero tests, Reed–Solomon distance, sparsity tests, Kronecker products, decoding
radius, subspace arguments, schedules, and concrete bounds.

### StrongPNT

About 28,000 placeholder-free Lean lines and roughly 1,100 declarations cover complex analysis,
logarithmic derivatives, zeta-function infrastructure, zero-free regions, and the strong prime
number theorem. Some final components adapt prior human work; the large generated middle layers
show that blueprint-driven autoformalization can scale when dependencies and statements are fixed.

### Sphere-Packing-Lean and M4R Thesis

Sphere-Packing-Lean is a live collaborative development with about 18,500 Lean lines, over 1,100
declarations, and 82 remaining placeholders. It spans packing definitions, lattices, Cohn–Elkies,
Fourier analysis, modular forms, and the magic function. The M4R thesis documents the mathematical
roadmap and human formalization experience.

Unlike polished generated repositories, this project exposes the actual frontier: missing library
infrastructure, difficult analytic interfaces, evolving blueprints, and dependencies whose
formalization cost is hard to predict. It is the best testbed for obligation scheduling and
human-escalation design.

### Goldbach/Collatz Proof

This repository contains an informal manuscript asserting proofs of two famous open conjectures,
without formal verification, executable checking, or a substantial peer-review artifact in the
checkout. It should be treated as an adversarial verifier benchmark, not as established
mathematics.

## Infrastructure and supporting repositories

### FlashSampling

FlashSampling fuses categorical sampling into the language-model head matmul and avoids
materializing full logits in HBM. The repository includes CUDA/Triton implementations, tensor-
parallel support, tests, profiling, and benchmark results. It is not a reasoning harness, but it
could reduce the cost of high-branching local-model search. This optimization should remain below
the harness abstraction and must not shape the proof representation.

### Mathlib

The checkout contains 8,245 core Mathlib modules, about 2.27 million lines, 225,000 declarations,
and 356 tactic modules. Largest domains include algebra, category theory, analysis, ring theory,
topology, data, linear algebra, tactics, order, measure theory, number theory, combinatorics, group
theory, geometry, and probability.

For a harness, Mathlib must be exposed through indexed declaration search, import minimization,
type-aware retrieval, tactic capability metadata, dependency graphs, and compiler feedback. Raw
text RAG over all modules is insufficient: theorem names, namespaces, implicit arguments, typeclass
requirements, versioning, and transitive imports are essential retrieval features.

## Cross-repository conclusions

1. No repository unifies research exploration, explicit proof state, computation, retrieval,
   natural-language verification, Lean formalization, and hardened proof checking.
2. Alethfeld supplies the best persistent representation; QED supplies the best research workflow;
   DeepMath supplies the best executable-reasoning loop; MathCode supplies the desired Lean UX;
   LeanParanoia and Comparator-style isolation supply the strongest acceptance boundary.
3. Successful large formalizations are blueprint-driven. They do not demonstrate that an agent can
   reliably invent the blueprint, validate the theorem statement, and formalize it end-to-end.
4. Evaluation must separate discovery, informal validity, formal compilation, dependency
   soundness, efficiency, novelty, and human usefulness. A single pass rate is misleading.
5. The most valuable retained data is not final prose. It is the obligation graph, failed
   strategies, compiler states, retrieved declarations, counterexamples, verifier findings, and
   provenance of accepted mutations.

