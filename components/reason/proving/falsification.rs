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

/// The verdict both falsifiers use for "a refuting assignment was found". The
/// rest of the codebase keys refutation off this exact string, which is why the
/// Wolfram path must produce it verbatim to have any effect at all.
const VERDICT_COUNTEREXAMPLE: &str = "counterexample";
/// The Wolfram tool's "there is no engine on this machine" answer. It means
/// "we did not look", which is a different fact from `no_counterexample_found`
/// ("we looked, bounded and heuristically, and saw nothing") and from
/// `inconclusive` ("we looked and the answer was unusable"). Collapsing the
/// three would let a missing dependency read as evidence.
const WOLFRAM_UNAVAILABLE: &str = "unavailable";

/// Read the worker envelope and hand back the oracle response, or `None` when
/// the oracle told us nothing at all.
///
/// `None` is returned for an unparseable envelope, a failed call, and for the
/// `unavailable` verdict alike, because in every one of those cases we did not
/// look. Encoding "did not look" as absence is deliberate: when no Wolfram
/// Engine is configured, the caller then records nothing and the existing
/// falsifier's output stays exactly what it is today.
fn wolfram_response(stdout: &str) -> Option<Value> {
    let parsed: Value = serde_json::from_str(stdout).ok()?;
    if !parsed.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    let output = parsed.get("output")?.clone();
    // A response whose verdict we cannot read is a response we refuse to
    // interpret: fail closed rather than guess.
    if output.get("verdict")?.as_str()? == WOLFRAM_UNAVAILABLE {
        return None;
    }
    Some(output)
}

/// The exact numerator/denominator pair the oracle reported for one variable.
///
/// The oracle prints witness values as STRINGS (`"3/2"`), because a witness can
/// be rational. We never parse those strings ourselves and never cast them:
/// integrality is decided from this exact pair instead, so there is no place
/// where a rational could be silently truncated to an integer.
fn exact_pair(entry: Option<&Value>) -> Option<(i64, i64)> {
    let pair = entry?.as_array()?;
    if pair.len() != 2 {
        return None;
    }
    Some((pair[0].as_i64()?, pair[1].as_i64()?))
}

/// Is the oracle's witness a point the model's spec actually quantified over?
///
/// The oracle rechecks that the claim is FALSE at the witness, but it does not
/// know how the spec quantified the variables. The spec declares integer
/// domains, so a rational witness such as 3/2 refutes nothing about the
/// statement: it is not a point the statement talks about. A `step` likewise
/// carves out a residue class (e.g. even `n` only), and a witness outside that
/// class is equally inadmissible.
///
/// The declared `start`/`stop` BOUNDS are deliberately not enforced. Searching
/// outside the cheap searcher's window is the entire reason to consult a second
/// oracle; only the KIND of point is checked here, never its size.
///
/// Every failure path returns false. An unrecognised witness must cost us
/// recall, never soundness.
fn witness_admissible(spec_variables: &Value, exact: &Value) -> bool {
    let Some(declared) = spec_variables.as_object() else {
        return false;
    };
    let Some(point) = exact.as_object() else {
        return false;
    };
    if declared.is_empty() || point.is_empty() {
        return false;
    }
    // Every declared variable must be pinned, or the "witness" is a partial
    // assignment that does not name a point at all.
    if !declared.keys().all(|name| point.contains_key(name)) {
        return false;
    }
    for (name, _) in point {
        let Some(domain) = declared.get(name) else {
            // A variable the spec never declared: we cannot say what it ranges
            // over, so we cannot admit it.
            return false;
        };
        let Some((numerator, denominator)) = exact_pair(point.get(name)) else {
            return false;
        };
        if denominator != 1 {
            return false;
        }
        let start = domain.get("start").and_then(Value::as_i64).unwrap_or(0);
        let step = domain
            .get("step")
            .and_then(Value::as_i64)
            .unwrap_or(1)
            .abs();
        if step > 1 && (numerator - start).rem_euclid(step) != 0 {
            return false;
        }
    }
    true
}

/// Fold one oracle response into `(refutes, record)`.
///
/// `refutes` is true only for a counterexample that the oracle's own exact
/// recheck confirmed AND that lands on an admissible point. Nothing else in
/// this function can ever return true: there is no path here from "found
/// nothing" to a pass, because a bounded heuristic search over a domain we
/// chose cannot establish a universal claim.
fn wolfram_record(spec_variables: &Value, oracle: &Value) -> (bool, Value) {
    let oracle_verdict = oracle["verdict"]
        .as_str()
        .unwrap_or("inconclusive")
        .to_string();
    // All three conditions come from the oracle itself; we require the explicit
    // `independently_verified` flag so that a refutation is only ever admitted
    // when the Python side re-evaluated the ORIGINAL claim in exact arithmetic.
    let confirmed = oracle_verdict == VERDICT_COUNTEREXAMPLE
        && oracle["refuted"].as_bool().unwrap_or(false)
        && oracle["independently_verified"].as_bool().unwrap_or(false);
    let witness = oracle.get("assignment").cloned().unwrap_or(Value::Null);
    let exact = oracle
        .get("assignment_numerator_denominator")
        .cloned()
        .unwrap_or(Value::Null);
    let admissible = confirmed && witness_admissible(spec_variables, &exact);

    let mut record = json!({
        "oracle": "wolfram",
        // Restated here so a reader of the stored evidence never has to go
        // looking for whether this oracle is authoritative. It is not.
        "trusted": false,
        "verdict": oracle_verdict,
        "refuted": admissible,
        "independently_verified": confirmed,
        "witness_admissible": admissible,
        // Witness values are kept EXACTLY as the oracle printed them, so "3/2"
        // stays "3/2". Truncating it to 1 would manufacture a witness that
        // refutes nothing.
        "assignment": witness,
        "assignment_numerator_denominator": exact,
        "response": oracle.clone(),
    });
    if confirmed && !admissible {
        record["note"] = json!(
            "witness passed the oracle's exact recheck but is not a point the \
             spec quantified over (non-integral or wrong residue class); \
             discarded, NOT a refutation"
        );
    }
    if !confirmed {
        record["note"] = json!(
            "no confirmed counterexample. This is NOT verification and NOT a \
             pass: the Wolfram search is bounded, heuristic and untrusted."
        );
    }
    (admissible, record)
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
        let mut verdict = verdict;
        let mut assignment = output["output"].get("assignment").cloned();
        let mut details = serde_json::to_value(&result).unwrap_or(Value::Null);

        // 3. Optional second falsifier. The cheap bounded search above always
        //    runs FIRST and the untrusted oracle is consulted only when that
        //    search did not settle the question, mirroring the falsify-before-
        //    prove cost discipline: once we already hold a refutation there is
        //    nothing left to buy. When no Wolfram Engine is configured the tool
        //    answers `unavailable`, `wolfram_response` maps that to `None`, and
        //    the verdict/assignment/details below are exactly what they are
        //    without this step.
        if verdict != VERDICT_COUNTEREXAMPLE {
            if let Some(oracle) = self.consult_wolfram(&py, &spec) {
                let (refutes, record) = wolfram_record(&spec["variables"], &oracle);
                if refutes {
                    verdict = VERDICT_COUNTEREXAMPLE.to_string();
                    // The rational-capable, string-valued map from the oracle,
                    // stored verbatim. It is deliberately shaped differently
                    // from the bounded searcher's integer-valued map so the two
                    // oracles' witnesses stay distinguishable downstream, and so
                    // a reader that expects an integer gets `None` rather than a
                    // truncated number.
                    assignment = record.get("assignment").cloned();
                }
                // Recorded whether or not it refuted: `inconclusive` and
                // `no_counterexample_found` are facts worth keeping, but neither
                // touches `verdict`, so neither can upgrade a status or be read
                // as a pass.
                match details.as_object_mut() {
                    Some(map) => {
                        map.insert("wolfram".into(), record);
                    }
                    None => details = json!({ "python": details, "wolfram": record }),
                }
            }
        }

        Ok(FalsifyVerdict {
            applicable: true,
            verdict,
            assignment,
            spec,
            details,
        })
    }

    /// Ask the untrusted Wolfram oracle for a counterexample, or `None` when we
    /// did not get an answer we are willing to interpret.
    ///
    /// `None` covers every "we did not look" case at once: no engine configured,
    /// the worker unavailable, a transport failure, unparseable output, or the
    /// oracle's own `unavailable` verdict. Collapsing them here is deliberate,
    /// because the caller's only correct response to all of them is identical:
    /// leave the bounded searcher's verdict exactly as it was. What must never
    /// collapse is "did not look" into "looked and found nothing", and that
    /// distinction is preserved because a genuine `no_counterexample_found`
    /// comes back as `Some` and is recorded without touching the verdict.
    ///
    /// The oracle re-verifies any witness it proposes in exact arithmetic on the
    /// Python side before reporting it, so what arrives here is already a
    /// confirmed counterexample or nothing.
    fn consult_wolfram(&self, py: &PythonCheck, spec: &Value) -> Option<Value> {
        let result = py
            .run(json!({
                "tool": "wolfram_falsify",
                "op": "falsify",
                "variables": spec.get("variables").cloned().unwrap_or_else(|| json!({})),
                "assumptions": spec["assumptions"].as_str().unwrap_or("True"),
                "claim": spec["claim"].as_str().unwrap_or("True"),
            }))
            .ok()?;
        wolfram_response(&result.stdout)
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
