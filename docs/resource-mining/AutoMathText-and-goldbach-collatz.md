# Resource Mining: AutoMathText-2.5 & goldbach-collatz-proof

Full-pass study of two small reference repos under `resources/`. Every file in both
repos was read in full (build artifacts / `.git` excluded). Findings are for the
Theoremata graph-first agentic math harness.

---

## Repo 1 — AutoMathText-2.5

Path: `C:\Users\adria\Downloads\math-agent\resources\AutoMathText-2.5-main\AutoMathText-2.5-main\`

### 1) What it is (scope, structure)

A **project landing page for a dataset**, not a code repo. Four files total:

- `README.md` — badges + links to the HuggingFace dataset, the arXiv paper, and license; two BibTeX entries.
- `index.html` — a single self-contained marketing website (GitHub Pages) describing the dataset. All content, no logic beyond nav/scroll/copy-to-clipboard JS.
- `LICENSE` — a custom "AutoMathText Data Agreement for Model Training" (a restrictive EULA, not OSS).
- `.nojekyll` — empty GitHub Pages marker.

The dataset it advertises: **AutoMathText-2.5**, "a foundational high-quality STEM training dataset," 2T+ tokens, 7.11 TB, 50+ sources, English + Chinese, hosted at `huggingface.co/datasets/math-ai/AutoMathText-2.5`. Authors: Yifan Zhang and the Math-AI team.

**Critical structural finding:** this repo contains **NO scoring code, NO prompts, NO score function, and NO dataset samples**. It is pure marketing/metadata. The actual autonomous-data-selection methodology lives in the referenced papers, not here:
- arXiv **2402.07625** — *"AutoMathText: Autonomous Data Selection with Language Models for Mathematical Texts"* (`README.md` line 5, 29; `index.html` line 459).
- ACL 2025 Findings — *"Autonomous Data Selection with Zero-shot Generative Classifiers for Mathematical Texts"*, Zhang, Luo, Yuan, Yao (`README.md` lines 28-33).

The only method description the repo itself gives is a vague 4-stage pipeline in `index.html` (lines 466-477): `01 Deduplicate → 02 Detect Contamination → 03 Clean Text → 04 Quality Score`, plus prose (lines 428-429, 470): "50+ premium data sources with semantic deduplication, contamination detection, and intelligent text cleaning," "three-tier deduplication pipeline and AI-powered quality assessment."

### 2) Reusable ideas / patterns for Theoremata — THE priority

The reusable IP is the **"language-model-as-scorer" (zero-shot generative classifier)** method named in the citations. It is NOT reproduced in these files — it must be pulled from the arXiv/ACL papers. What the repo gives us is only the *name of the technique* and the *pipeline skeleton*. From the papers' well-known formulation (flagged clearly as external, since it is not in-repo):

- **LM-Score technique:** Prompt a base LM with a meta-prompt asking whether a passage has mathematical/educational value, then read the model's **token probability for the affirmative answer** (e.g. `P("YES")` normalized against `P("NO")`) and use that scalar in [0,1] as the quality score. No fine-tuned classifier or human labels — hence "autonomous" / "zero-shot generative classifier."
- The score is a soft, calibrated proxy for "is this text worth training on," applied at corpus scale to rank/filter.

Because the concrete prompt is **not present in this repo**, we cannot quote real code/prompts from it — that is itself a finding (see §4). To actually reuse the method we must fetch arXiv 2402.07625 (the paper contains the exact meta-prompt and the logit-to-score formula).

How this maps onto Theoremata:
- **Retrieval ranking:** reuse LM-Score as a reranker over mathlib retrieval candidates — score each candidate lemma/passage by an LM's affirmative-token probability for "is this relevant/useful to the current goal," giving a cheap, model-agnostic relevance score through our LiteLLM provider (works with logprobs).
- **Training-data / SFT curation:** score our own generated proof traces, retrieved snippets, and synthesized SFT examples with the same technique to filter the training-data scaffolds before they enter the SFT set.
- **Pipeline shape to copy:** the `Deduplicate → Detect Contamination → Clean → Quality-Score` ordering is a sensible template for any corpus-ingestion path we build (esp. contamination detection = keeping benchmark theorems out of training data).

### 3) Schema / format

None in-repo. No JSON schema, no row schema, no scoring rubric fields. The only "schema" is the HF dataset card metadata paraphrased in `index.html` (size bucket `10B<n<100B`, tasks = text-generation/QA, modality = text, languages = en/zh). The BibTeX blocks (`README.md` 19-34) are the only structured data.

### 4) What our earlier targeted pass MISSED

- The earlier skim likely treated this as "the AutoMathText scoring repo." The full pass confirms the opposite: **the scoring method is absent from the repo** — it is a landing page. Any plan that assumed we could lift a prompt/score-function directly from these files is wrong; we must go to arXiv 2402.07625.
- The **LICENSE is a restrictive data-use EULA**, not OSS (custom "Data Agreement for Model Training," Delaware/California jurisdiction, no redistribution, internal-training-only, terminates on litigation). This matters if anyone assumed the dataset/method was freely reusable — the *dataset* is encumbered; the *method* (from the papers) is what we reuse, not the data.
- Two distinct papers are cited (the 2024 arXiv "with Language Models" and the 2025 ACL "with Zero-shot Generative Classifiers") — the second is the refined framing worth reading for the exact classifier construction.
- The pipeline explicitly includes **contamination detection** as a first-class stage — easy to overlook but directly relevant to our benchmark hygiene.

### 5) Test / benchmark value

Low as a test artifact (no code, no data samples to run against). Value is **methodological**: (a) a citable, model-agnostic quality-scoring technique to implement in our retrieval reranker and SFT curation; (b) a corpus-ingestion pipeline template. Not a functional benchmark.

### 6) New vs. already-in-our-design

- **New:** explicit LM-as-scorer (affirmative-token-probability) for *ranking retrieval candidates and curating training data* — our design has mathlib retrieval and SFT scaffolds but (per memory notes) no LM-driven quality score on either. This is a concrete, cheap addition.
- **New:** contamination-detection stage as an explicit gate in the training-data path.
- **Already in our design:** model-agnostic LLM access (LiteLLM) — the technique slots directly onto it via logprobs; training-data/SFT scaffolds already exist as the consumer.

---

## Repo 2 — goldbach-collatz-proof

Path: `C:\Users\adria\Downloads\math-agent\resources\goldbach-collatz-proof-main\goldbach-collatz-proof-main\`

### 1) What it is (scope, structure)

A **claimed "constructive complete proof" of BOTH the Goldbach and Collatz conjectures** — an AI-collaboration artifact, bilingual (English/Japanese). Author "M. Koide (小井手 将基)", "in collaboration with GPT-based AI systems," CC BY 4.0. Files actually present:

- `README.md` — extensive claims: "fully constructive proof" of Goldbach ("every even number > 2 is the sum of two primes") and Collatz ("every positive integer eventually reaches 1"). Badges include `status: peer-review-ready` and `AI-assisted: yes`. Lists methods: "A-type primes, T-sequences, block decomposition," "Reduction and Elimination Functions," "Formalized Lemmas and Structural Induction."
- `README.txt` — a **single sentence**: "This folder is reserved for future figures or diagrams illustrating the proof structure." (i.e. a placeholder, NOT a text mirror of the README as its own file tree claims.)
- `CHANGELOG.md` — "v1.0.0 2025-06-XX," announces the "unified constructive proof."
- `main.tex` — **essentially EMPTY**. It is a LaTeX shell: preamble, `\title`, `\author`, `\maketitle`, then the comment `% 内容省略：先に記述したものと一致` ("content omitted: matches what was described earlier"), then `\end{document}`. **There is no actual mathematics in the file.**

**Critical structural finding:** the README advertises a rich file tree — `sections/proof_goldbach.tex`, `proof_collatz.tex`, `definitions.tex`, `theorem.tex`, `introduction.tex`, `conclusion.tex`, a `proof_assets/` dir, and a `final_proof.pdf`. **None of these exist in the repo.** The Quick-Nav links (`README.md` 5-9) and the "View the Full Paper → final_proof.pdf" link (line 60) are all dead. The only `.tex` file, `main.tex`, is a stub with the body explicitly omitted.

### 2) Reusable ideas / patterns for Theoremata — artifact type & test-case value

This is a **cautionary flawed-proof artifact**, not a usable proof and not a reusable technique.

- Both Goldbach and Collatz are **famously open** (unproven as of the knowledge cutoff). A repo claiming a "constructive complete proof" of both simultaneously, "peer-review-ready," "AI-assisted," with **no actual proof content on disk**, is the textbook signature of an AI-hallucinated / crank proof artifact.
- Every hallmark is present: grandiose unified claim, invented terminology with no definitions supplied ("A-type primes," "T-sequences," "block decomposition"), status badges asserting rigor, a promised PDF that isn't there, and a `main.tex` whose mathematical body is literally the comment "content omitted."
- **Value to Theoremata = negative example / test fixture for the falsify-before-prove gate.** This is exactly the kind of input our harness must *reject*. It is a real-world specimen of "confident natural-language proof with zero formal content," useful for:
  - a **red-team / regression fixture**: feed the README's claims + `main.tex` into the pipeline and assert the harness refuses to certify (no Lean, so `#print axioms` gate can never pass; falsifier should flag unbounded universal claims over ℕ with no base/induction structure).
  - calibrating our **falsification stage**: the correct behavior is to demand the formal artifact, find none, and fail closed — never trust the prose badges.

### 3) Schema / format

Only the (aspirational, mostly non-existent) modular LaTeX layout described in `README.md` 105-122: a `main.tex` including per-concern `sections/*.tex` (introduction, definitions, theorem, proof_goldbach, proof_collatz, conclusion). The intent — one root file including modular section files split "by function" for "transparency and reproducibility" — is a reasonable *document-structuring* convention, but here it is unimplemented. No machine-readable schema.

### 4) What our earlier targeted pass MISSED

- The earlier pass likely recorded "a Goldbach/Collatz proof attempt." The full pass reveals **there is no proof at all** — `main.tex` body is the comment `% 内容省略` and the entire advertised `sections/` tree + `final_proof.pdf` are **missing from the repo**. The artifact is hollow.
- `README.txt` is **not** a text version of the README (as the README's own file tree claims, line 111); it is a one-line placeholder about "future figures." The repo's self-description is internally inconsistent — another crank/hallucination tell.
- The bilingual EN/JP framing and the explicit "in collaboration with GPT-based AI systems" attribution confirm provenance: an LLM-assisted artifact presented as a finished result.

### 5) Test / benchmark value

**High, as a negative fixture.** It is a compact, real specimen of the failure mode our gates exist to catch. Concrete uses:
- Falsify-before-prove regression: assert the harness never advances this past the falsification stage.
- Formalize/compile gate: there is nothing to compile → the Lean `#print axioms` gate can never be satisfied → the artifact must be marked unverified. Good end-to-end "fails closed" test.
- Provenance/claims-vs-content check: a test that a document asserting theorems must actually contain the referenced files/proofs (dead-link and empty-body detection).

Zero value as a *positive* proof or as mathlib retrieval material.

### 6) New vs. already-in-our-design

- **Already in our design (validates it):** our falsify-before-prove → Lean-compile → `#print axioms` gate → hardening pipeline is precisely the defense against this class of artifact. This repo is confirmation that the gate is necessary; it is a ready-made adversarial test case rather than a new capability.
- **New (small):** the idea of an explicit **claims-vs-artifact consistency check** (does the prose's referenced `sections/*.tex` / PDF actually exist and contain math?) as a cheap pre-filter before the expensive falsify/formalize stages. Worth adding as a lightweight guardrail.
- **New (fixtures):** add this repo to a curated corpus of "known-bad" proof artifacts for regression testing the guardrails.

---

## Cross-repo summary

- AutoMathText-2.5 = a *dataset landing page* whose real value is the cited **LM-as-scorer / zero-shot generative-classifier** method (fetch arXiv 2402.07625 for the actual prompt + logit-to-score formula) — reusable for retrieval reranking and SFT-data curation; the method is NOT in-repo and the dataset itself is under a restrictive EULA.
- goldbach-collatz-proof = a *hollow, AI-assisted crank artifact* claiming to prove two open conjectures with no actual proof on disk — worthless as math, valuable as a **negative regression fixture** that validates our falsify → Lean-gate pipeline.
