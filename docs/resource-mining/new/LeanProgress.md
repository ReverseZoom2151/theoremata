# LeanProgress — Full Resource-Mining Pass

Source: `resources/LeanProgress-main/LeanProgress-main/`
Pass type: full over README, dataset builders, data-adjustment utilities, training/eval scripts, and model config files.

---

## 1) What it is

LeanProgress is the research code for “Guiding Search for Neural Theorem Proving via Proof Progress Prediction.” It trains a model to predict proof progress, typically expressed as the number of remaining proof steps until `no goals`.

The repo is intentionally minimal and research-oriented. The README notes that an updated version is shared as part of LeanDojo-v2. The main value is the data idea:

```text
proof-search trace
-> successful path ending in `no goals`
-> states labeled by steps_to_no_goals / relative_progress
-> chat-style SFT dataset
-> model predicts remaining steps
-> prover uses prediction as value/reward signal
```

## 2) Core files and flow

Main files inspected:

- `README.md` — paper framing and note that LeanDojo-v2 contains the updated version.
- `utils/collect_steps_data.py` — converts proof-search JSON traces into state/proof/progress examples.
- `utils/adjust_steps.py` — creates SFT chat formats, plots distributions, down-samples ranges, train/test splits.
- `create_datesets/create_full_datasets.py` — creates 2k balanced/unbalanced datasets from a combined JSONL source.
- `create_datesets/create_unbalanced_dataset.py` — raw-distribution train/test SFT data.
- `create_datesets/create_exact_paper_dataset.py` — scaled paper-ratio dataset builder.
- `models/steps_*.py` — XTuner/MMEngine config files for DeepSeek-style supervised finetuning.
- `utils/train_deepseek_simple.py` — simpler Hugging Face Trainer path.
- `utils/train_qwen_epochs.py` — Qwen trainer path with W&B.
- `utils/eval.py` — vLLM evaluation, numeric extraction, accuracy/MAE by step range.
- `utils/train.sh`, `utils/test.sh` — wrapper scripts with required path placeholders.

## 3) Reusable ideas and code patterns

**Progress labels from search traces.** `collect_steps_data.py` reads proof-search records containing `searched_states`, `queue`, `state_before`, `state_after`, and `proof`. It builds a parent graph from `state_after -> state_before`, finds a chain ending in `no goals`, and labels each state by distance to completion.

**Sibling sampling.** The extractor can add sibling branches from the same parent state. This is useful because a value model should learn not only successful-path states but also nearby alternatives that make different progress.

**Two progress targets.**

- `steps_to_no_goals` — integer distance to completion.
- `relative_progress` — normalized progress from 0 to 1.

Theoremata should support both. Integer remaining steps are easier to evaluate; relative progress may be smoother across proofs of different lengths.

**Multiple prompt formats.** `adjust_steps.py` creates variants:

- state before -> steps;
- state after -> steps;
- state before + proof prefix -> steps;
- full state/proof -> steps + state after;
- relative-progress target.

The most useful for search is likely: current goal/state + candidate proof prefix/tactic -> predicted remaining steps.

**Distribution-aware evaluation.** The scripts bucket examples into `1-5`, `6-10`, `11-15`, `16-20`, and `21+` steps and report accuracy/MAE by bucket. This matters because short proofs dominate raw traces, while long proofs are the ones where search guidance is most valuable.

**Progress as reward.** LeanDojo-v2’s external API uses the same concept by returning negative predicted remaining steps as a reward. That is the correct integration point: progress prediction should rank/prune tactics during search, not merely be an offline metric.

## 4) Benchmark and evaluation value

LeanProgress suggests a clean Theoremata eval track:

- collect search traces from successful proof attempts;
- label each state/tactic prefix with remaining steps;
- split by theorem/file/repository, not just random state rows;
- train a progress/value model;
- evaluate MAE by step bucket;
- ablate proof search with and without the value model.

The strongest evaluation is not “can the model predict a number?” but “does the predictor reduce search nodes/time or improve proof rate under the same tactic-generator budget?”

## 5) Gaps and risks

- Many scripts contain placeholder paths such as empty `base_path`, `data_root_path`, `XTUNER_DIR`, `EVAL_SCRIPT`, and `EVAL_INPUT_FILE`.
- Several files are experiment scripts rather than package-quality modules; imports are duplicated and configuration is hardcoded.
- Random train/test splits over state rows risk leakage between train and test if states from the same theorem appear in both.
- Balanced dataset builders may repeat examples when a bucket lacks enough data.
- XTuner config files have empty `data_files` in several variants and hardcoded model/data choices in others.
- `train_qwen_epochs.py` says “10 epochs” in names/comments but sets `num_train_epochs=3`.
- Evaluation extracts the first number from model output; this is practical but weakly structured.
- The repo does not contain the full integrated search loop; it depends on proof-search traces generated elsewhere.
- Training/eval assumes large model infrastructure: vLLM, HF models, XTuner/MMEngine, GPUs, and optional W&B.

## 6) Adopt list for Theoremata

P0:

- Add a **progress/value trace schema** to proof search: every edge should record parent state, tactic, child state, proof prefix, status, and eventually distance-to-success if the branch reaches a proof.
- Train progress predictors on theorem-level or repository-level splits to avoid leakage.
- Use progress predictions as a tactic ranking/pruning signal and measure search-node reduction, not only MAE.

P1:

- Support both `steps_to_no_goals` and `relative_progress`.
- Include sibling/failed-branch examples so the value model learns discriminative search guidance.
- Report MAE by proof-length bucket and separately for long proofs.

P2:

- Prefer the LeanDojo-v2 `ProgressTrainer` integration path over these raw scripts for production.
- Reimplement dataset generation as typed, configurable code; treat the current scripts as paper-reproduction references.

