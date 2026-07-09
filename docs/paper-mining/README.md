# Paper mining — `math-papers/` (20 papers, fully read)

Per-paper mining reports for the 20 PDFs in `math-papers/`. Each report: core
contribution, techniques/architecture, results, novelty-vs-SOTA-2026,
adopt-relevance to Theoremata, and verbatim implementation details. All PDF
content was treated as untrusted data.

## Reports
| Paper | Report | Direct relevance |
|---|---|---|
| AlphaGeometry (Solving Olympiad Geometry) | `alphageometry.md` | High — synthetic-data traceback |
| Aristotle (IMO-level ATP) | `aristotle.md` | High — MCGS spec, validates our gate |
| ATP for Prolog Verification | `atp-for-prolog-verification.md` | Medium — hammer decomposition |
| Automated Theorem Proving (Pfenning notes) | `automated-theorem-proving-survey.md` | Medium — subsumption redundancy |
| DeepSeekMath-V2 | `deepseekmath-v2.md` | **Highest** — trained meta-verifier + flywheel |
| FOTL automata | `first-order-automata-for-foltl.md` | Low — runtime monitoring only |
| ImProver | `improver.md` | High — Chain-of-States goal feedback |
| Lean Copilot | `lean-copilot.md` | Medium — tri-state tactics into MCTS |
| LeanDojo-v2 | `leandojo-v2.md` | Medium — curriculum, proof minimization |
| LEAN-GitHub | `lean-github.md` | High — α-rename canonical node hash |
| LEGO-Prover | `lego-prover.md` | **Highest** — growing verified-lemma library / evolver |
| Machine Learning & ATP (2010) | `ml-and-automated-theorem-proving.md` | Medium — learned backend selector |
| On Mathematical Superintelligence | `on-mathematical-superintelligence.md` | Framing only |
| Quantum ATP | `quantum-automated-theorem-proving.md` | Low — benchmark domain only |
| Reliable NL Proof Eval (PROOFGRADER) | `reliable-nl-proof-eval-proofgrader.md` | **Highest** — marking-scheme grading |
| Towards Robust Math Reasoning (IMO-Bench) | `robust-mathematical-reasoning.md` | High — ref-conditioned grader/benchmark |
| TheoremExplainAgent | `theorem-explain-agent.md` | Medium — structural render as verify signal |
| von Neumann, Theory of Automata (1948) | `von-neumann-general-logical-theory-of-automata.md` | Framing only |
| Towards Autonomous Math Research (Aletheia) | `towards-autonomous-mathematics-research-aletheia.md` | High — abstention as terminal state |
| Advancing Math Research (AlphaProof Nexus) | `advancing-math-research-alphaproof-nexus.md` | High — deep-hash goal cache, Elo fitness |

## Cross-paper convergences (what multiple papers independently point at)

1. **Verifier-as-reward + self-verification flywheel** — DeepSeekMath-V2, Aletheia,
   Aristotle, LEGO-Prover all converge on: an LLM *verifier/grader* trained or
   prompted to score rigor, used as the reward that improves the generator, with
   automated (majority-vote) labeling replacing humans. We have the mock loop;
   the gap is a **graded/trained meta-verifier reward** `R_V = R_format·R_score·R_meta`.
2. **Search-state redundancy control** — LEAN-GitHub (α-rename canonicalization),
   AlphaProof Nexus (deep-hash goal cache), Aristotle (state/action equivalence),
   Pfenning (subsumption) all attack the same problem: our MCGS `dedup_key` should
   move from string-equality to **kernel-order canonicalization → subsumption**.
3. **Growing verified-lemma library** — LEGO-Prover (evolver + skills), Aristotle
   (lemma outer loop), AlphaProof Nexus (sub-proof reuse). Our lemma-cache is a
   stub; the gap is the **evolver** (request-solve backlog + generalize + admit
   only through the 3+1 gate, dedup by proof-DAG subsumption).
4. **Reference/marking-scheme-conditioned grading** — PROOFGRADER and IMO-Bench
   both show conditioning the grader on a reference solution + generated rubric
   lifts human correlation ~0.87 → 0.93–0.96. Directly upgrades our ProofGrader.
5. **Abstention / falsify-inside-search** — Aletheia (abstain as terminal state),
   Aristotle (augment each state with the goal's negation). Reliability lever.

## Also in this folder

- `deepmind-articles.md` — mining of five DeepMind math-AI blog articles
  (AlphaProof/AlphaGeometry 2, AlphaGeometry, FunSearch, AlphaTensor, AI-guided
  pure mathematics). Articles 1/2/4 validate and mirror what we built (DD+AR,
  traceback synthetic data, MCGS, flywheel); **FunSearch (program search)** and
  **conjecture-discovery** are the two genuinely-new capabilities absent from the
  codebase.

See the parent `docs/resource-mining/` for the repo-level (code) mining. The
prioritized cross-source adopt list lives in the session synthesis.
