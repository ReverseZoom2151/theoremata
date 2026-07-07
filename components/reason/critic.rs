//! Adversarial critic (plan §4), upgraded with the AgentMathOlympiadMedalist +
//! MathResearchPrompts critic craft.
//!
//! An LLM critic that reviews the *structure* of the proof DAG — not tactic
//! text — for circular dependencies, unjustified logical gaps, over-general
//! statements, and claims marked verified without grounded evidence. Beyond the
//! base pass it now carries:
//!
//! * a **Critical-Error vs Justification-Gap** taxonomy on each finding (a gap
//!   means "assume it true and keep going", so one pass surfaces every
//!   independent issue instead of halting at the first);
//! * a **meta-critic** prune layer that filters out false-positive bug reports
//!   *before* they trigger a wasteful rewrite;
//! * the **7-item failure-mode rubric** and the "every proof carries its own
//!   itemized adversarial-check list" output contract;
//! * a **reparameterization gate** (is the flaw intrinsic, or a coordinate
//!   artifact?) and a **never-fabricate-references** rule (emit an explicit
//!   proof obligation instead of inventing a citation).
//!
//! Findings are grounded: each one that names a real node is recorded as
//! evidence on that node, so the critique becomes durable, auditable graph
//! state. Pruned false positives are logged (never silently dropped).

use crate::{db::Store, model::ModelRequest, provider::ModelProvider};
use anyhow::Result;
use serde::Serialize;
use serde_json::json;
use std::collections::HashSet;

/// Two-class taxonomy (AgentMathOlympiadMedalist `verification_start.md`). A
/// `CriticalError` breaks the logical chain — stop following that line but keep
/// checking independent parts. A `JustificationGap` is a likely-true step with
/// an insufficient argument — *assume it true and continue*, so a single pass
/// yields all independent findings rather than stopping at the first.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FindingClass {
    CriticalError,
    JustificationGap,
}

impl FindingClass {
    /// Lenient parse; anything not clearly a critical error defaults to the
    /// safe "assume-and-continue" gap class.
    pub fn from_label(label: &str) -> FindingClass {
        match label
            .trim()
            .to_ascii_lowercase()
            .replace([' ', '-'], "_")
            .as_str()
        {
            "critical_error" | "critical" | "error" | "fatal" => FindingClass::CriticalError,
            _ => FindingClass::JustificationGap,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CritiqueFinding {
    pub node_id: Option<String>,
    pub severity: String,
    pub category: String,
    /// Critical-error vs justification-gap classification.
    pub class: FindingClass,
    pub issue: String,
    /// The itemized adversarial checks the critic attached to this finding
    /// (degenerate cases, boundary values, citation-status probes, …).
    pub adversarial_checks: Vec<String>,
}

/// A finding the meta-critic judged a false positive, kept with its reason so
/// the prune is auditable rather than a silent drop.
#[derive(Debug, Clone, Serialize)]
pub struct PrunedFinding {
    pub finding: CritiqueFinding,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct CritiqueReport {
    pub project_id: String,
    /// Findings that survived the meta-critic prune.
    pub findings: Vec<CritiqueFinding>,
    /// False positives the meta-critic removed (kept for audit).
    pub pruned: Vec<PrunedFinding>,
    pub summary: String,
}

pub struct Critic<'a> {
    pub store: &'a Store,
    pub provider: &'a dyn ModelProvider,
}

/// The 7-item failure-mode rubric (MathResearchPrompts `prompt_templates.md`
/// §10) the critic must screen every claim against.
const FAILURE_MODE_RUBRIC: &str = "Screen every claim against this 7-item failure-mode rubric: \
    (1) notation drift; (2) hidden assumption changes; (3) unsupported claims; \
    (4) fabricated or vague references; (5) boundary cases omitted; \
    (6) numerical/heuristic evidence overstated as proof; \
    (7) output too generic to verify (missing domain-specific substance).";

impl Critic<'_> {
    pub fn critique(&self, project_id: &str) -> Result<CritiqueReport> {
        let graph = self.store.export(project_id)?;

        let request = ModelRequest {
            role: "adversarial_verifier".into(),
            task: format!(
                "You are a meticulous, adversarial referee reviewing the STRUCTURE of a \
                 mathematical proof DAG — not the prose of any single step. Your primary failure \
                 mode is ACCEPTING A FALSE result, so be strict: a proof that is 'almost right' \
                 still FAILS. Inspect the nodes and dependency edges for: circular dependencies; \
                 unjustified logical gaps (a claim whose stated dependencies do not entail it); \
                 over-general statements that claim more than their support; and any node marked \
                 verified without grounded evidence. {FAILURE_MODE_RUBRIC} \
                 Classify each finding as either 'critical_error' (breaks the logical chain — do \
                 not follow that line further, but DO check independent parts) or \
                 'justification_gap' (the conclusion is likely true but the argument is \
                 insufficient — ASSUME it true and continue, so you surface every independent \
                 issue in one pass). Apply a reparameterization gate: before reporting a flaw, \
                 ask whether it is intrinsic to the mathematics or merely a coordinate/parameter \
                 artifact that survives a change of coordinates — report only intrinsic flaws. \
                 NEVER fabricate a reference or lemma to close a gap: if a required fact is not \
                 supplied or standard, flag it as an explicit proof obligation instead. Report \
                 ONLY findings you can tie to a specific node or edge; cite the offending node by \
                 its id. For each finding, attach an itemized list of adversarial checks a reader \
                 could run (degenerate cases, boundary values, special choices, citation-status \
                 probes)."
            ),
            context: json!({
                "project": graph.project,
                "nodes": graph.nodes,
                "edges": graph.edges,
            }),
            output_schema: json!({
                "type": "object",
                "required": ["findings", "summary"],
                "properties": {
                    "findings": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["severity", "category", "issue"],
                            "properties": {
                                "node_id": {"type": ["string", "null"]},
                                "severity": {"type": "string"},
                                "category": {"type": "string"},
                                "class": {"type": "string", "enum": ["critical_error", "justification_gap"]},
                                "issue": {"type": "string"},
                                "adversarial_checks": {"type": "array", "items": {"type": "string"}}
                            }
                        }
                    },
                    "summary": {"type": "string"}
                }
            }),
        };

        let response = self.provider.complete(&request)?;
        let content = &response.content;

        let summary = content["summary"]
            .as_str()
            .unwrap_or("The critic returned no summary.")
            .to_owned();

        let mut findings = Vec::new();
        if let Some(items) = content["findings"].as_array() {
            for item in items {
                let issue = item["issue"].as_str().unwrap_or("").trim().to_owned();
                if issue.is_empty() {
                    continue;
                }
                let class = item["class"]
                    .as_str()
                    .map(FindingClass::from_label)
                    .unwrap_or(FindingClass::JustificationGap);
                let adversarial_checks = item["adversarial_checks"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|c| c.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                findings.push(CritiqueFinding {
                    node_id: item["node_id"].as_str().map(str::to_owned),
                    severity: item["severity"].as_str().unwrap_or("info").to_owned(),
                    category: item["category"].as_str().unwrap_or("general").to_owned(),
                    class,
                    issue,
                    adversarial_checks,
                });
            }
        }

        // Meta-critic: prune false-positive bug reports before they drive a
        // rewrite. Only real (surviving) findings are grounded onto nodes.
        let (retained, pruned) = self.meta_review(findings)?;

        // Ground each surviving finding: one that names a real node becomes
        // evidence on that node; anything else is logged as a project-level
        // event so it is never silently dropped but also never attached to a
        // phantom node.
        let node_ids: HashSet<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
        for finding in &retained {
            let payload = json!({
                "severity": finding.severity,
                "category": finding.category,
                "class": finding.class,
                "issue": finding.issue,
                "adversarial_checks": finding.adversarial_checks,
            });
            match &finding.node_id {
                Some(id) if node_ids.contains(id.as_str()) => {
                    self.store.add_evidence(
                        project_id,
                        id,
                        "critique",
                        "adversarial_verifier",
                        &finding.severity,
                        payload,
                    )?;
                }
                _ => {
                    self.store.event(
                        Some(project_id),
                        None,
                        "critique.finding",
                        "adversarial_verifier",
                        payload,
                    )?;
                }
            }
        }
        // Record each pruned false positive so the de-noising is auditable.
        for p in &pruned {
            self.store.event(
                Some(project_id),
                None,
                "critique.pruned",
                "meta_critic",
                json!({
                    "node_id": p.finding.node_id,
                    "issue": p.finding.issue,
                    "reason": p.reason,
                }),
            )?;
        }

        Ok(CritiqueReport {
            project_id: project_id.to_owned(),
            findings: retained,
            pruned,
            summary,
        })
    }

    /// Meta-critic prune layer (AgentMathOlympiadMedalist `bug_report_review_*`):
    /// a critic-of-the-critic that reviews the verifier's own findings and
    /// removes false positives before they cause a rewrite. Conservative — a
    /// finding is only dropped when the meta-critic explicitly marks it a false
    /// positive; anything unreviewed is retained.
    fn meta_review(
        &self,
        findings: Vec<CritiqueFinding>,
    ) -> Result<(Vec<CritiqueFinding>, Vec<PrunedFinding>)> {
        if findings.is_empty() {
            return Ok((findings, Vec::new()));
        }
        let indexed: Vec<serde_json::Value> = findings
            .iter()
            .enumerate()
            .map(|(i, f)| {
                json!({
                    "index": i,
                    "node_id": f.node_id,
                    "severity": f.severity,
                    "class": f.class,
                    "issue": f.issue,
                })
            })
            .collect();

        let response = self.provider.complete(&ModelRequest {
            role: "meta_critic".into(),
            task: "You are a meta-critic reviewing another verifier's bug reports before they \
                   trigger an expensive rewrite. For each finding decide whether it is a genuine \
                   issue ('confirmed') or a false positive the verifier misread ('false_positive'), \
                   with a one-line reason. Be conservative: only mark 'false_positive' when you are \
                   confident the reported issue is not real."
                .into(),
            context: json!({ "findings": indexed }),
            output_schema: json!({
                "type": "object",
                "required": ["reviews"],
                "properties": {
                    "reviews": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["index", "verdict"],
                            "properties": {
                                "index": {"type": "integer"},
                                "verdict": {"type": "string", "enum": ["confirmed", "false_positive"]},
                                "reason": {"type": "string"}
                            }
                        }
                    }
                }
            }),
        })?;

        // Map index -> prune reason for findings the meta-critic rejected.
        let mut prune_reason: std::collections::HashMap<usize, String> =
            std::collections::HashMap::new();
        if let Some(reviews) = response.content["reviews"].as_array() {
            for r in reviews {
                let idx = r["index"].as_u64().map(|n| n as usize);
                let is_fp = r["verdict"].as_str() == Some("false_positive");
                if let (Some(idx), true) = (idx, is_fp) {
                    let reason = r["reason"]
                        .as_str()
                        .unwrap_or("flagged as false positive")
                        .to_owned();
                    prune_reason.insert(idx, reason);
                }
            }
        }

        let mut retained = Vec::new();
        let mut pruned = Vec::new();
        for (i, finding) in findings.into_iter().enumerate() {
            match prune_reason.remove(&i) {
                Some(reason) => pruned.push(PrunedFinding { finding, reason }),
                None => retained.push(finding),
            }
        }
        Ok((retained, pruned))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ModelResponse, NodeKind};
    use std::path::Path;

    /// A provider that returns a canned critique naming a specific node id, and
    /// (for the meta-critic role) confirms every finding — nothing is pruned.
    struct MockCritic {
        node_id: String,
    }
    impl ModelProvider for MockCritic {
        fn complete(&self, _: &ModelRequest) -> Result<ModelResponse> {
            Ok(ModelResponse {
                provider: "test".into(),
                model: "test".into(),
                content: json!({
                    "findings": [{
                        "node_id": self.node_id,
                        "severity": "major",
                        "category": "gap",
                        "class": "justification_gap",
                        "issue": "The dependencies do not entail the conclusion.",
                        "adversarial_checks": ["try the n=0 boundary case"]
                    }],
                    "summary": "One logical gap found."
                }),
            })
        }
        fn name(&self) -> &str {
            "test"
        }
    }

    #[test]
    fn critique_grounds_findings_to_nodes() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "Every even square is even").unwrap();
        let node = store
            .add_node(&project.id, NodeKind::Obligation, "Core step", "S", "test")
            .unwrap();
        let critic = Critic {
            store: &store,
            provider: &MockCritic {
                node_id: node.id.clone(),
            },
        };
        let report = critic.critique(&project.id).unwrap();
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].node_id.as_deref(), Some(node.id.as_str()));
        assert_eq!(report.findings[0].severity, "major");
        assert_eq!(report.findings[0].class, FindingClass::JustificationGap);
        assert_eq!(report.summary, "One logical gap found.");
        // The grounded finding was recorded as evidence (which emits an event).
        let events = store.events(&project.id, 50).unwrap();
        assert!(events.iter().any(|e| e.event_type == "evidence.recorded"));
    }

    #[test]
    fn ungrounded_findings_become_events_not_evidence() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "t").unwrap();
        let critic = Critic {
            store: &store,
            provider: &MockCritic {
                node_id: "does-not-exist".into(),
            },
        };
        let report = critic.critique(&project.id).unwrap();
        assert_eq!(report.findings.len(), 1);
        let events = store.events(&project.id, 50).unwrap();
        assert!(events.iter().any(|e| e.event_type == "critique.finding"));
        assert!(!events.iter().any(|e| e.event_type == "evidence.recorded"));
    }

    /// A provider that reports two findings and, as meta-critic, marks the
    /// second one a false positive.
    struct PruningCritic {
        real_node: String,
        bogus_node: String,
    }
    impl ModelProvider for PruningCritic {
        fn complete(&self, request: &ModelRequest) -> Result<ModelResponse> {
            let content = match request.role.as_str() {
                "adversarial_verifier" => json!({
                    "findings": [
                        {"node_id": self.real_node, "severity": "major", "category": "gap",
                         "class": "critical_error", "issue": "A genuine circular dependency."},
                        {"node_id": self.bogus_node, "severity": "minor", "category": "style",
                         "class": "justification_gap", "issue": "A misread non-issue."}
                    ],
                    "summary": "Two findings."
                }),
                // The meta-critic prunes the second (index 1) as a false positive.
                "meta_critic" => json!({
                    "reviews": [
                        {"index": 0, "verdict": "confirmed", "reason": "real cycle"},
                        {"index": 1, "verdict": "false_positive", "reason": "verifier misread the step"}
                    ]
                }),
                _ => json!({}),
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
    fn meta_critic_prunes_a_false_positive() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "t").unwrap();
        let real = store
            .add_node(&project.id, NodeKind::Lemma, "real", "R", "test")
            .unwrap();
        let bogus = store
            .add_node(&project.id, NodeKind::Lemma, "bogus", "B", "test")
            .unwrap();
        let critic = Critic {
            store: &store,
            provider: &PruningCritic {
                real_node: real.id.clone(),
                bogus_node: bogus.id.clone(),
            },
        };
        let report = critic.critique(&project.id).unwrap();
        // Only the genuine finding survives; the false positive is pruned.
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].node_id.as_deref(), Some(real.id.as_str()));
        assert_eq!(report.findings[0].class, FindingClass::CriticalError);
        assert_eq!(report.pruned.len(), 1);
        assert_eq!(report.pruned[0].finding.node_id.as_deref(), Some(bogus.id.as_str()));
        // The prune was logged, and no evidence was written for the bogus node.
        let events = store.events(&project.id, 50).unwrap();
        assert!(events.iter().any(|e| e.event_type == "critique.pruned"));
    }
}
