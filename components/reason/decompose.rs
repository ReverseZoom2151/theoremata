//! Model-driven proof decomposition with the QED-style retry policy: turn a
//! statement into independently-verifiable obligations. Returns an empty vec
//! (not a canned skeleton) when no model is configured or the model fails after
//! retries — no hardcoded fallback.

use crate::{
    db::Store,
    model::ModelRequest,
    provider::ModelProvider,
    retry::{Decision, RetryLimits, RetryState},
};
use anyhow::{Context, Result};
use serde_json::json;

pub struct Decomposer<'a> {
    pub store: &'a Store,
    pub provider: &'a dyn ModelProvider,
}

impl Decomposer<'_> {
    /// Decompose `statement` into `(title, statement)` obligations, bounded by
    /// the QED retry policy. Each model attempt is recorded. Empty vec when
    /// offline or after the retry budget is spent.
    pub fn run(
        &self,
        project_id: &str,
        run_id: &str,
        statement: &str,
    ) -> Result<Vec<(String, String)>> {
        if self.provider.name() == "offline" {
            return Ok(Vec::new());
        }
        let mut state = RetryState::new(RetryLimits::default());
        loop {
            match self.decompose(statement) {
                Ok(obligations) if !obligations.is_empty() => {
                    self.store.add_attempt(
                        project_id,
                        None,
                        Some(run_id),
                        "proof_decomposer",
                        &json!({ "statement": statement }),
                        &json!({ "obligations": obligations.len() }),
                        true,
                    )?;
                    return Ok(obligations);
                }
                other => {
                    let detail = match &other {
                        Ok(_) => "empty decomposition".to_string(),
                        Err(e) => e.to_string(),
                    };
                    self.store.add_attempt(
                        project_id,
                        None,
                        Some(run_id),
                        "proof_decomposer",
                        &json!({ "statement": statement }),
                        &json!({ "error": detail }),
                        false,
                    )?;
                    if state.resolve(Decision::ReviseProof) == Decision::Terminate {
                        return Ok(Vec::new());
                    }
                }
            }
        }
    }

    fn decompose(&self, statement: &str) -> Result<Vec<(String, String)>> {
        let response = self.provider.complete(&ModelRequest {
            role: "proof_decomposer".into(),
            task: "Decompose the statement into independently verifiable obligations.".into(),
            context: json!({ "statement": statement }),
            output_schema: json!({"type":"object","required":["obligations"],"properties":{
                "obligations":{"type":"array","items":{"type":"object","required":["title","statement"],
                    "properties":{"title":{"type":"string"},"statement":{"type":"string"}}}}}}),
        })?;
        Ok(response.content["obligations"]
            .as_array()
            .context("missing obligations")?
            .iter()
            .map(|x| {
                (
                    x["title"].as_str().unwrap_or("Obligation").into(),
                    x["statement"].as_str().unwrap_or("").into(),
                )
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelResponse;
    use std::path::Path;

    struct DecomposeProvider;
    impl ModelProvider for DecomposeProvider {
        fn complete(&self, _: &ModelRequest) -> Result<ModelResponse> {
            Ok(ModelResponse {
                content: json!({"obligations":[
                    {"title":"Step 1","statement":"first obligation"},
                    {"title":"Step 2","statement":"second obligation"}
                ]}),
                model: "test".into(),
                provider: "command".into(),
            })
        }
        fn name(&self) -> &str {
            "command"
        }
    }

    #[test]
    fn decomposes_via_model_with_retry() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "t").unwrap();
        let run = store.begin_run(&project.id, "test").unwrap();
        let obligations = Decomposer {
            store: &store,
            provider: &DecomposeProvider,
        }
        .run(&project.id, &run, "some theorem")
        .unwrap();
        assert_eq!(obligations.len(), 2);
    }

    #[test]
    fn offline_returns_empty_not_a_skeleton() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "t").unwrap();
        let run = store.begin_run(&project.id, "test").unwrap();
        let obligations = Decomposer {
            store: &store,
            provider: &crate::provider::OfflineProvider,
        }
        .run(&project.id, &run, "t")
        .unwrap();
        assert!(obligations.is_empty());
    }
}
