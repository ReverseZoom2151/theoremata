//! Tactic sampling boundary (plan §13) and verbalized sampling (plan §14).
//!
//! `TacticSampler` is the swappable, removable optimization seam that sits
//! strictly *below* the proof representation: a GPU/FlashSampling backend can
//! implement it later for cheap high-branching sampling without the proof DAG,
//! tactic representation, or tool interface knowing anything about it. Every
//! sample it returns is an UNTRUSTED proposal — the Lean checker is the only
//! authority on validity, so a mis-sample can at worst waste a search branch,
//! never corrupt a proof. The correctness test of this abstraction is that the
//! proof-DAG schema and tool interface remain fully definable with any
//! `TacticSampler` removed.
//!
//! `verbalized_sample` addresses post-training mode collapse: when we need N
//! *semantically distinct* strategies (induction vs. contradiction vs. cite-a-
//! lemma) rather than N lexical variants of one template, we ask the model to
//! verbalize a distribution over approaches and sample from that.

use crate::{model::ModelRequest, provider::ModelProvider};
use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::json;
use std::collections::HashSet;

/// The removable sampling backend. `sample` returns `num_samples` candidate
/// continuations for `context`; they are proposals only, validated downstream.
pub trait TacticSampler {
    fn sample(
        &mut self,
        context: &str,
        num_samples: usize,
        temperature: f64,
        seed: u64,
    ) -> Result<Vec<String>>;
    fn name(&self) -> &str;
}

/// Reference (CPU) sampler backed by the model provider — also the test oracle
/// any faster backend must agree with statistically. `temperature`/`seed` are
/// forwarded in the request context so a real backend can honour them; the
/// default provider path ignores them.
pub struct ModelSampler<'a> {
    pub provider: &'a dyn ModelProvider,
    pub role: String,
}

impl ModelSampler<'_> {
    pub fn new(provider: &dyn ModelProvider) -> ModelSampler<'_> {
        ModelSampler {
            provider,
            role: "tactic_sampler".into(),
        }
    }
}

impl TacticSampler for ModelSampler<'_> {
    fn sample(
        &mut self,
        context: &str,
        num_samples: usize,
        temperature: f64,
        seed: u64,
    ) -> Result<Vec<String>> {
        let response = self.provider.complete(&ModelRequest {
            role: self.role.clone(),
            task: format!(
                "Propose {num_samples} distinct candidate next tactics / continuations for the \
                 goal. Favour genuinely different approaches over lexical variants. Return only \
                 the candidates."
            ),
            context: json!({
                "context": context,
                "num_samples": num_samples,
                "temperature": temperature,
                "seed": seed,
            }),
            output_schema: json!({
                "type": "object",
                "required": ["candidates"],
                "properties": {
                    "candidates": {
                        "type": "array",
                        "items": { "type": "string" }
                    }
                }
            }),
        })?;
        let candidates = response.content["candidates"]
            .as_array()
            .context("missing candidates")?
            .iter()
            .filter_map(|c| c.as_str().map(str::to_owned))
            .collect();
        Ok(candidates)
    }

    fn name(&self) -> &str {
        "model"
    }
}

/// A verbalized strategy candidate: a distinct approach with the model's own
/// probability estimate, so the caller can sample from the distribution rather
/// than always take the argmax (which mode-collapses).
#[derive(Debug, Clone, Serialize)]
pub struct VerbalizedCandidate {
    pub strategy: String,
    pub approach: String,
    pub probability: f64,
}

/// Ask the model to verbalize a distribution over `n` semantically distinct
/// proof strategies. Sampling from these (weighted by `probability`) yields
/// real diversity for best-of-N / MCTS branching.
pub fn verbalized_sample(
    provider: &dyn ModelProvider,
    goal: &str,
    n: usize,
) -> Result<Vec<VerbalizedCandidate>> {
    let response = provider.complete(&ModelRequest {
        role: "verbalized_sampler".into(),
        task: format!(
            "Propose {n} SEMANTICALLY DISTINCT strategies to prove the goal (e.g. induction, \
             contradiction, cite a known lemma, case split, direct computation). For each give a \
             short strategy name, a one-line concrete approach, and a probability that it is the \
             right route. The probabilities should form a distribution over genuinely different \
             approaches, not variants of one."
        ),
        context: json!({ "goal": goal }),
        output_schema: json!({
            "type": "object",
            "required": ["candidates"],
            "properties": {
                "candidates": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["strategy", "approach", "probability"],
                        "properties": {
                            "strategy": { "type": "string" },
                            "approach": { "type": "string" },
                            "probability": { "type": "number" }
                        }
                    }
                }
            }
        }),
    })?;
    Ok(response.content["candidates"]
        .as_array()
        .context("missing candidates")?
        .iter()
        .map(|c| VerbalizedCandidate {
            strategy: c["strategy"].as_str().unwrap_or("").to_owned(),
            approach: c["approach"].as_str().unwrap_or("").to_owned(),
            probability: c["probability"].as_f64().unwrap_or(0.0),
        })
        .collect())
}

/// How far a candidate proof got through the layered verifier (QED's
/// structural-gate → detailed-check pipeline). Declared ascending so the derived
/// ordering is `Certified > Detailed > Structural > Rejected` — a candidate that
/// reached a deeper phase is a better bet even if none is fully certified yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationPhase {
    /// Failed the first gate (or never ran).
    Rejected,
    /// Passed the cheap structural gate.
    Structural,
    /// Passed the expensive step-by-step detailed check.
    Detailed,
    /// Fully certified (compiled + axiom-clean).
    Certified,
}

/// A candidate tagged with the verification phase it reached.
#[derive(Debug, Clone)]
pub struct PhasedCandidate<T> {
    pub value: T,
    pub phase: VerificationPhase,
}

/// QED's phase-prior selector: bias candidate selection by which verification
/// phase each candidate reached. Among `candidates`, pick the one that got
/// furthest through verification (ties broken by original order — a stable
/// preference for earlier, cheaper candidates). Returns `None` for an empty
/// slate. This is a *prior*, not a verdict: it ranks partial progress so
/// best-of-N spends its next effort on the most promising branch rather than a
/// uniformly-random one.
pub fn select_by_phase<T>(candidates: Vec<PhasedCandidate<T>>) -> Option<PhasedCandidate<T>> {
    candidates.into_iter().reduce(|best, c| {
        // Strictly-greater keeps the earlier candidate on ties (stable).
        if c.phase > best.phase {
            c
        } else {
            best
        }
    })
}

/// Lexical-diversity score in [0, 1]: unique tokens / total tokens across the
/// candidates' `approach` strings. Low values signal mode collapse (the model
/// returned near-identical approaches).
pub fn distinctness(candidates: &[VerbalizedCandidate]) -> f64 {
    let mut total = 0usize;
    let mut unique: HashSet<String> = HashSet::new();
    for candidate in candidates {
        for token in candidate
            .approach
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
        {
            total += 1;
            unique.insert(token.to_lowercase());
        }
    }
    if total == 0 {
        0.0
    } else {
        unique.len() as f64 / total as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelResponse;

    struct MockSampler;
    impl ModelProvider for MockSampler {
        fn complete(&self, request: &ModelRequest) -> Result<ModelResponse> {
            let content = match request.role.as_str() {
                "verbalized_sampler" => json!({
                    "candidates": [
                        {"strategy": "induction", "approach": "induct on n then apply succ lemma", "probability": 0.5},
                        {"strategy": "contradiction", "approach": "assume odd and derive parity clash", "probability": 0.3},
                        {"strategy": "cite", "approach": "reduce to Nat.even_mul existing result", "probability": 0.2}
                    ]
                }),
                _ => json!({ "candidates": ["exact?", "simp", "ring", "omega"] }),
            };
            Ok(ModelResponse {
                content,
                model: "test".into(),
                provider: "test".into(),
            })
        }
        fn name(&self) -> &str {
            "test"
        }
    }

    #[test]
    fn model_sampler_returns_candidates() {
        let provider = MockSampler;
        let mut sampler = ModelSampler::new(&provider);
        let out = sampler.sample("goal", 4, 0.8, 7).unwrap();
        assert_eq!(out, vec!["exact?", "simp", "ring", "omega"]);
        assert_eq!(sampler.name(), "model");
    }

    #[test]
    fn verbalized_sample_parses_distribution() {
        let provider = MockSampler;
        let out = verbalized_sample(&provider, "n^2 even implies n even", 3).unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].strategy, "induction");
        assert!((out.iter().map(|c| c.probability).sum::<f64>() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn phase_prior_prefers_the_deepest_candidate() {
        let candidates = vec![
            PhasedCandidate {
                value: "a",
                phase: VerificationPhase::Rejected,
            },
            PhasedCandidate {
                value: "b",
                phase: VerificationPhase::Detailed,
            },
            PhasedCandidate {
                value: "c",
                phase: VerificationPhase::Structural,
            },
        ];
        let picked = select_by_phase(candidates).unwrap();
        assert_eq!(picked.value, "b");
        assert_eq!(picked.phase, VerificationPhase::Detailed);
    }

    #[test]
    fn phase_prior_breaks_ties_toward_earlier() {
        let candidates = vec![
            PhasedCandidate {
                value: 1,
                phase: VerificationPhase::Structural,
            },
            PhasedCandidate {
                value: 2,
                phase: VerificationPhase::Structural,
            },
        ];
        assert_eq!(select_by_phase(candidates).unwrap().value, 1);
        assert!(select_by_phase::<()>(Vec::new()).is_none());
    }

    #[test]
    fn distinctness_detects_mode_collapse() {
        let varied = vec![
            VerbalizedCandidate {
                strategy: "a".into(),
                approach: "induct on the natural number".into(),
                probability: 0.5,
            },
            VerbalizedCandidate {
                strategy: "b".into(),
                approach: "assume contradiction derive parity".into(),
                probability: 0.5,
            },
        ];
        let collapsed = vec![
            VerbalizedCandidate {
                strategy: "a".into(),
                approach: "simp the goal".into(),
                probability: 0.5,
            },
            VerbalizedCandidate {
                strategy: "b".into(),
                approach: "simp the goal".into(),
                probability: 0.5,
            },
        ];
        assert!(distinctness(&varied) > distinctness(&collapsed));
    }
}
