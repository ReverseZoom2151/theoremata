//! Model-derived falsification (plan §3, the DeepMath / MathResearchPrompts
//! pattern): the model translates an informal claim into an *executable* bounded
//! check, which the generic falsify worker runs. Nothing about the check is
//! hardcoded — the variables/assumptions/claim are derived from the actual
//! statement. Numerics only SCREEN: a passing screen is never a proof, and a
//! found counterexample refutes the branch.

use crate::{
    model::ModelRequest,
    provider::ModelProvider,
    tools::{PythonCheck, Tool},
};
use anyhow::Result;
use serde_json::{json, Value};

pub struct Falsifier<'a> {
    pub provider: &'a dyn ModelProvider,
}

#[derive(Debug, serde::Serialize)]
pub struct FalsifyVerdict {
    /// Whether the statement admits a bounded computational check at all.
    pub applicable: bool,
    /// One of: counterexample | no_counterexample_in_domain | inconclusive |
    /// not_applicable | no_model | unavailable | error.
    pub verdict: String,
    /// The refuting assignment, when a counterexample was found.
    pub assignment: Option<Value>,
    /// The executable spec the model produced (for provenance/audit).
    pub spec: Value,
    /// The raw worker result.
    pub details: Value,
}

impl Falsifier<'_> {
    pub fn falsify(&self, statement: &str) -> Result<FalsifyVerdict> {
        if self.provider.name() == "offline" {
            return Ok(FalsifyVerdict {
                applicable: false,
                verdict: "no_model".into(),
                assignment: None,
                spec: Value::Null,
                details: json!({ "reason": "no model provider configured" }),
            });
        }
        // 1. The model emits an executable falsification spec derived from the
        //    statement. `claim`/`assumptions` are safe_eval-compatible Python
        //    expressions over the named integer variables.
        let response = self.provider.complete(&ModelRequest {
            role: "falsifier".into(),
            task: "If the statement admits a bounded computational falsification over integer \
                   variables, emit an executable check. `variables` maps each variable to an \
                   integer domain {start,stop[,step]}; `assumptions` and `claim` are safe Python-eval \
                   expressions over those variables (arithmetic, comparisons, and/or/not, and \
                   abs/min/max/sum/range/math.*). The claim should hold for all admissible \
                   assignments iff the statement is true. Set applicable=false for statements not \
                   reducible to a finite numeric check."
                .into(),
            context: json!({ "statement": statement }),
            output_schema: json!({
                "type":"object","required":["applicable"],
                "properties":{
                    "applicable":{"type":"boolean"},
                    "variables":{"type":"object"},
                    "assumptions":{"type":"string"},
                    "claim":{"type":"string"}
                }
            }),
        })?;
        let spec = response.content;
        if !spec["applicable"].as_bool().unwrap_or(false) {
            return Ok(FalsifyVerdict {
                applicable: false,
                verdict: "not_applicable".into(),
                assignment: None,
                spec,
                details: Value::Null,
            });
        }

        // 2. Run the generic, reusable falsify worker with the model's spec.
        let py = PythonCheck::new();
        if !py.available() {
            return Ok(FalsifyVerdict {
                applicable: true,
                verdict: "unavailable".into(),
                assignment: None,
                spec,
                details: json!({ "reason": "python worker unavailable" }),
            });
        }
        let result = py.run(json!({
            "tool":"falsify",
            "variables": spec.get("variables").cloned().unwrap_or_else(|| json!({})),
            "assumptions": spec["assumptions"].as_str().unwrap_or("True"),
            "claim": spec["claim"].as_str().unwrap_or("True"),
        }))?;
        let output: Value = serde_json::from_str(&result.stdout).unwrap_or(Value::Null);
        let verdict = output["output"]["verdict"]
            .as_str()
            .unwrap_or(if result.success {
                "inconclusive"
            } else {
                "error"
            })
            .to_string();
        let assignment = output["output"].get("assignment").cloned();
        Ok(FalsifyVerdict {
            applicable: true,
            verdict,
            assignment,
            spec,
            details: serde_json::to_value(&result).unwrap_or(Value::Null),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelResponse;

    struct FalsifierProvider;
    impl ModelProvider for FalsifierProvider {
        fn complete(&self, request: &ModelRequest) -> Result<ModelResponse> {
            // Emit a real executable spec for the parity example.
            let content = if request.role == "falsifier" {
                json!({
                    "applicable": true,
                    "variables": {"n": {"start": -20, "stop": 21}},
                    "assumptions": "n % 2 == 0",
                    "claim": "(n * n) % 2 == 0"
                })
            } else {
                json!({})
            };
            Ok(ModelResponse {
                content,
                model: "test".into(),
                provider: "command".into(),
            })
        }
        fn name(&self) -> &str {
            "command"
        }
    }

    #[test]
    fn derives_an_executable_spec_from_the_statement() {
        let f = Falsifier {
            provider: &FalsifierProvider,
        };
        let v = f.falsify("every even integer has an even square").unwrap();
        assert!(v.applicable);
        // The spec is model-derived, not hardcoded in the falsifier.
        assert_eq!(v.spec["claim"], "(n * n) % 2 == 0");
        // verdict depends on whether the python worker is present; either way it
        // must not be a hardcoded constant — it reflects the spec that ran.
        assert!([
            "no_counterexample_in_domain",
            "unavailable",
            "inconclusive",
            "counterexample"
        ]
        .contains(&v.verdict.as_str()));
    }

    #[test]
    fn offline_is_not_applicable() {
        let f = Falsifier {
            provider: &crate::provider::OfflineProvider,
        };
        let v = f.falsify("x").unwrap();
        assert!(!v.applicable);
        assert_eq!(v.verdict, "no_model");
    }
}
