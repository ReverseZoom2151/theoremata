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

use crate::{
    db::Store,
    model::{ModelRequest, Node, NodeStatus},
    provider::ModelProvider,
};
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
    /// DeepSeek-Math-V2 `R_meta`: whether the "verify-the-verifier" meta pass
    /// AFFIRMED that this claimed defect actually exists. A finding only gates a
    /// node rejection/taint when `confirmed` — an unconfirmed finding is retained
    /// for audit but treated as a possible critic hallucination.
    #[serde(default)]
    pub confirmed: bool,
    /// The meta-verifier's one-line reason for the confirm/deny decision.
    #[serde(default)]
    pub meta_reason: Option<String>,
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

impl CritiqueReport {
    /// The findings whose defect the meta-verifier CONFIRMED — the only findings
    /// allowed to drive a node rejection/taint (DeepSeek `R_V` gate).
    pub fn confirmed_findings(&self) -> Vec<&CritiqueFinding> {
        self.findings.iter().filter(|f| f.confirmed).collect()
    }

    /// Whether `node_id` should be REJECTED: it carries at least one confirmed
    /// `CriticalError`. Justification gaps (assume-and-continue) and unconfirmed
    /// findings never reject a node on their own.
    pub fn should_reject_node(&self, node_id: &str) -> bool {
        self.findings.iter().any(|f| {
            f.confirmed
                && f.class == FindingClass::CriticalError
                && f.node_id.as_deref() == Some(node_id)
        })
    }
}

/// Category tag carried on the advisory finding emitted by
/// [`short_proof_for_hard_target`]. A stable string so downstream consumers can
/// filter for it.
pub const SUSPICIOUS_SHORT_PROOF_CATEGORY: &str = "suspicious-short-proof";

/// Tuning for the "suspiciously short proof for a hard target" heuristic
/// (maxwells-daemon: *if you expect a problem to be hard and the proof is very
/// short, you've probably missed something* — usually a mis-stated statement or
/// a trivial witness). A node ESTIMATED hard yet certified by a TRIVIALLY SHORT
/// proof earns an ADVISORY finding. It never rejects a node — a passed formal
/// check is ground truth — it only warns that the STATEMENT may be wrong.
#[derive(Debug, Clone, Copy)]
pub struct ShortProofCfg {
    /// A node whose difficulty estimate (in `[0, 1]`, `1.0` = hardest — the
    /// convention used by the TTC controller) is `>=` this is a "hard target".
    pub hard_threshold: f64,
    /// A proof whose size (see [`proof_size`]: non-comment tactic-step count) is
    /// `<=` this is "trivially short".
    pub trivial_len: usize,
}

impl Default for ShortProofCfg {
    fn default() -> Self {
        ShortProofCfg {
            hard_threshold: 0.7,
            trivial_len: 2,
        }
    }
}

/// Documented proof-"size" metric: the number of **non-comment tactic steps**.
///
/// The proof text is normalised deterministically (no wall-clock/rand):
/// 1. block comments are removed — Lean `/- … -/` and Rocq/Isabelle `(* … *)`
///    (an unterminated block drops the remainder);
/// 2. per line, a Lean/Haskell `--` line comment is stripped to end-of-line;
/// 3. the tactic separators newline, `;`, and the Lean combinator `<;>` are all
///    normalised to newlines;
/// 4. the count is the number of remaining non-blank segments.
///
/// A term-mode one-liner (`… := trivial`) has size `1`; a multi-tactic `by`
/// block scales with its steps. Comment-only lines contribute nothing.
pub fn proof_size(proof: &str) -> usize {
    let no_block = strip_block_comments(proof);
    let mut normalized = String::new();
    for line in no_block.lines() {
        // Drop a Lean/Haskell `--` line comment (Rocq uses `(* *)`, handled
        // above), keeping only the code before it.
        let code = match line.split_once("--") {
            Some((before, _)) => before,
            None => line,
        };
        normalized.push_str(code);
        normalized.push('\n');
    }
    normalized
        .replace("<;>", "\n")
        .replace(';', "\n")
        .lines()
        .filter(|seg| !seg.trim().is_empty())
        .count()
}

/// Remove every `/- … -/` and `(* … *)` block comment span.
fn strip_block_comments(text: &str) -> String {
    let once = remove_spans(text, "/-", "-/");
    remove_spans(&once, "(*", "*)")
}

/// Delete every `open`…`close` span (an unterminated `open` drops the rest).
fn remove_spans(text: &str, open: &str, close: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find(open) {
        out.push_str(&rest[..i]);
        let after = &rest[i + open.len()..];
        match after.find(close) {
            Some(j) => rest = &after[j + close.len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The maxwells-daemon heuristic as a pure function: when a target is estimated
/// HARD (`difficulty >= cfg.hard_threshold`) yet its certified proof is
/// TRIVIALLY SHORT (`proof_size(proof) <= cfg.trivial_len`), return an ADVISORY
/// [`CritiqueFinding`] flagging that the statement may be trivially satisfiable
/// or mis-formalized. Returns `None` otherwise.
///
/// The finding is deliberately a [`FindingClass::JustificationGap`] with
/// `confirmed: false`, so it can NEVER drive [`CritiqueReport::should_reject_node`]
/// (which rejects only on a confirmed critical error): a proof that passed the
/// formal check is ground truth and is never overturned — this only warns.
///
/// An empty/whitespace proof yields size `0` and returns `None` (no proof text
/// to judge, rather than a false alarm on missing data). The returned finding
/// has `node_id: None`; the caller sets it when grounding to a specific node.
pub fn short_proof_for_hard_target(
    difficulty: f64,
    proof: &str,
    cfg: &ShortProofCfg,
) -> Option<CritiqueFinding> {
    if difficulty < cfg.hard_threshold {
        return None;
    }
    let size = proof_size(proof);
    if size == 0 || size > cfg.trivial_len {
        return None;
    }
    Some(CritiqueFinding {
        node_id: None,
        severity: "advisory".to_owned(),
        category: SUSPICIOUS_SHORT_PROOF_CATEGORY.to_owned(),
        class: FindingClass::JustificationGap,
        issue: format!(
            "This target is estimated HARD (difficulty {difficulty:.2} >= {:.2}) yet was \
             certified by a very short proof ({size} tactic step(s) <= {}). A hard result with a \
             trivial proof is a classic smell: the statement may be trivially satisfiable, \
             vacuous, or mis-formalized (a trivial witness, a weakened conclusion, or a \
             contradictory hypothesis). ADVISORY ONLY — the formal check is ground truth and is \
             NOT overturned; review the STATEMENT for a formalization error.",
            cfg.hard_threshold, cfg.trivial_len
        ),
        adversarial_checks: vec![
            "Re-read the formal statement: does it capture the intended claim, or only a trivial \
             special case?"
                .to_owned(),
            "Check for a vacuous/contradictory hypothesis — an unsatisfiable premise makes any \
             conclusion trivially provable."
                .to_owned(),
            "Check for a trivial witness or degenerate instance that satisfies the statement \
             without its intended content."
                .to_owned(),
            "Confirm the quantifiers and the direction of the claim were not weakened during \
             formalization."
                .to_owned(),
        ],
        confirmed: false,
        meta_reason: None,
    })
}

/// Estimate a node's difficulty in `[0, 1]` from the signals present on the node
/// itself (the graph stores no numeric difficulty). We read the `strategy_hint`:
/// a hint flagging the work as hard/deep/open maps to a high difficulty,
/// mirroring `guard::model_tier`'s existing "hard" escalation. With no signal we
/// return a neutral `0.5` — below the default `hard_threshold`, so an
/// un-annotated node never triggers the advisory.
fn estimate_node_difficulty(node: &Node) -> f64 {
    match node.strategy_hint.as_deref() {
        Some(hint) => {
            let h = hint.to_ascii_lowercase();
            const HARD_MARKERS: [&str; 7] = [
                "hard",
                "difficult",
                "deep",
                "open problem",
                "nontrivial",
                "non-trivial",
                "major",
            ];
            if HARD_MARKERS.iter().any(|m| h.contains(m)) {
                0.9
            } else {
                0.5
            }
        }
        None => 0.5,
    }
}

/// A node is CERTIFIED (its formal check passed) when its proof is complete
/// (`proof_done`) or it is marked `FormallyVerified`. Only certified nodes are
/// eligible for the short-proof advisory — the heuristic is about a *passed*
/// check whose brevity is suspicious.
fn node_is_certified(node: &Node) -> bool {
    node.proof_done || node.status == NodeStatus::FormallyVerified
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
                    confirmed: false,
                    meta_reason: None,
                });
            }
        }

        // Meta-critic: prune false-positive bug reports before they drive a
        // rewrite. Only real (surviving) findings are grounded onto nodes.
        let (retained, pruned) = self.meta_review(findings)?;

        // Verify-the-verifier (DeepSeek-Math-V2 `R_meta`): before any surviving
        // finding is allowed to reject/taint a node, a META pass audits whether
        // the claimed defect ACTUALLY exists. A finding is marked `confirmed`
        // only when the meta-check affirms it; unconfirmed findings stay for
        // audit but do not gate node rejection.
        let retained = self.meta_verify_findings(retained)?;

        // maxwells-daemon advisory: a target ESTIMATED HARD that was certified by
        // a TRIVIALLY SHORT proof is a smell (usually a mis-stated statement or a
        // trivial witness). Derived deterministically from graph state (no model
        // call) and appended AFTER the meta passes, so it is never pruned/gated.
        // It is ADVISORY — a justification gap with `confirmed: false` — so it can
        // NEVER reject a node: a passed formal check remains ground truth.
        let mut retained = retained;
        let short_cfg = ShortProofCfg::default();
        for node in &graph.nodes {
            if !node_is_certified(node) {
                continue;
            }
            let Some(proof) = node.formal_statement.as_deref() else {
                continue;
            };
            if let Some(mut finding) =
                short_proof_for_hard_target(estimate_node_difficulty(node), proof, &short_cfg)
            {
                finding.node_id = Some(node.id.clone());
                retained.push(finding);
            }
        }

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
                "confirmed": finding.confirmed,
                "meta_reason": finding.meta_reason,
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

    /// Verify-the-verifier meta pass (DeepSeek-Math-V2 §2.2, `R_meta`).
    ///
    /// A second audit over the *surviving* findings that checks, for each one,
    /// whether the claimed defect ACTUALLY exists and justifies its severity —
    /// exactly the exploit the paper closes (a verifier that reports a fake issue
    /// to look rigorous). Each finding is returned with `confirmed` set only when
    /// the meta-check affirms the defect; unconfirmed findings are kept (never
    /// silently dropped — that is `meta_review`'s job) but must not gate a node
    /// rejection. Fail-safe: when the meta pass returns nothing parseable, every
    /// finding stays UNCONFIRMED, so an unverifiable critic can never reject a
    /// node — the anti-hallucination direction.
    fn meta_verify_findings(&self, findings: Vec<CritiqueFinding>) -> Result<Vec<CritiqueFinding>> {
        if findings.is_empty() {
            return Ok(findings);
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
            role: "meta_verifier".into(),
            task: "You are verifying a verifier (meta-verification). For EACH claimed finding \
                   below, audit ONLY whether the described defect ACTUALLY exists in the proof \
                   structure and whether it justifies the stated severity — do NOT introduce new \
                   issues, and do NOT re-check parts the finding does not claim are wrong. Set \
                   `defect_exists` true only when the claimed defect is real. A finding is \
                   `confirmed` iff its defect exists AND (for a critical_error) the error truly \
                   breaks the logical chain. Give a one-line reason. Be strict: a hallucinated or \
                   overstated issue must be marked defect_exists=false so it cannot reject a valid \
                   node."
                .into(),
            context: json!({ "findings": indexed }),
            output_schema: json!({
                "type": "object",
                "required": ["verifications"],
                "properties": {
                    "verifications": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["index", "defect_exists"],
                            "properties": {
                                "index": {"type": "integer"},
                                "defect_exists": {"type": "boolean"},
                                "justifies_severity": {"type": "boolean"},
                                "reason": {"type": "string"}
                            }
                        }
                    }
                }
            }),
        })?;

        // index -> (confirmed, reason). Absent indices stay unconfirmed.
        let mut verdicts: std::collections::HashMap<usize, (bool, Option<String>)> =
            std::collections::HashMap::new();
        if let Some(items) = response.content["verifications"].as_array() {
            for v in items {
                let Some(idx) = v["index"].as_u64().map(|n| n as usize) else {
                    continue;
                };
                let exists = v["defect_exists"].as_bool().unwrap_or(false);
                // `justifies_severity` defaults to true when omitted (the
                // verifier is only asked to override it downward).
                let justifies = v["justifies_severity"].as_bool().unwrap_or(true);
                let reason = v["reason"].as_str().map(str::to_owned);
                verdicts.insert(idx, (exists && justifies, reason));
            }
        }

        let confirmed = findings
            .into_iter()
            .enumerate()
            .map(|(i, mut f)| {
                if let Some((ok, reason)) = verdicts.remove(&i) {
                    f.confirmed = ok;
                    f.meta_reason = reason;
                } else {
                    f.confirmed = false;
                    f.meta_reason =
                        Some("meta-verifier returned no verdict; treated as unconfirmed".into());
                }
                f
            })
            .collect();
        Ok(confirmed)
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
        let project = store
            .create_project("p", "Every even square is even")
            .unwrap();
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
        assert_eq!(
            report.findings[0].node_id.as_deref(),
            Some(node.id.as_str())
        );
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

    /// A provider whose findings all survive the false-positive prune, but whose
    /// meta-verifier CONFIRMS the real critical error and DENIES the hallucinated
    /// one — exercising the `R_meta` rejection gate.
    struct MetaVerifyCritic {
        real_node: String,
        fake_node: String,
    }
    impl ModelProvider for MetaVerifyCritic {
        fn complete(&self, request: &ModelRequest) -> Result<ModelResponse> {
            let content = match request.role.as_str() {
                "adversarial_verifier" => json!({
                    "findings": [
                        {"node_id": self.real_node, "severity": "major", "category": "gap",
                         "class": "critical_error", "issue": "A genuine circular dependency."},
                        {"node_id": self.fake_node, "severity": "major", "category": "gap",
                         "class": "critical_error", "issue": "A hallucinated contradiction."}
                    ],
                    "summary": "Two findings."
                }),
                // Nothing is pruned as an outright false positive here...
                "meta_critic" => json!({"reviews": []}),
                // ...but the meta-verifier only CONFIRMS the real defect.
                "meta_verifier" => json!({
                    "verifications": [
                        {"index": 0, "defect_exists": true, "justifies_severity": true,
                         "reason": "the cycle is real"},
                        {"index": 1, "defect_exists": false, "justifies_severity": false,
                         "reason": "no such contradiction exists"}
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
    fn meta_verify_gates_node_rejection_on_confirmed_findings() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "t").unwrap();
        let real = store
            .add_node(&project.id, NodeKind::Lemma, "real", "R", "test")
            .unwrap();
        let fake = store
            .add_node(&project.id, NodeKind::Lemma, "fake", "F", "test")
            .unwrap();
        let critic = Critic {
            store: &store,
            provider: &MetaVerifyCritic {
                real_node: real.id.clone(),
                fake_node: fake.id.clone(),
            },
        };
        let report = critic.critique(&project.id).unwrap();
        // Both findings are retained (nothing pruned as a false positive)...
        assert_eq!(report.findings.len(), 2);
        // ...but only the meta-confirmed one gates a rejection.
        assert_eq!(report.confirmed_findings().len(), 1);
        assert!(report.should_reject_node(&real.id));
        assert!(!report.should_reject_node(&fake.id));
        // The confirmed finding carries the meta-verifier's reason.
        let confirmed = report.confirmed_findings();
        assert_eq!(confirmed[0].node_id.as_deref(), Some(real.id.as_str()));
        assert_eq!(
            confirmed[0].meta_reason.as_deref(),
            Some("the cycle is real")
        );
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
        assert_eq!(
            report.findings[0].node_id.as_deref(),
            Some(real.id.as_str())
        );
        assert_eq!(report.findings[0].class, FindingClass::CriticalError);
        assert_eq!(report.pruned.len(), 1);
        assert_eq!(
            report.pruned[0].finding.node_id.as_deref(),
            Some(bogus.id.as_str())
        );
        // The prune was logged, and no evidence was written for the bogus node.
        let events = store.events(&project.id, 50).unwrap();
        assert!(events.iter().any(|e| e.event_type == "critique.pruned"));
    }

    // ----- suspiciously-short-proof-for-a-hard-target heuristic ---------------

    /// A short multi-tactic `by` block (2 non-comment steps).
    const SHORT_PROOF: &str = "theorem t : P := by\n  simp";
    /// A substantial proof (well over a handful of steps).
    const LONG_PROOF: &str = "theorem t : P := by\n  intro x\n  induction x with\n  \
                              | zero => simp\n  | succ n ih => rw [foo]; exact ih\n  ring";

    #[test]
    fn proof_size_counts_non_comment_tactic_steps() {
        // Term-mode one-liner is a single step.
        assert_eq!(proof_size("theorem t : True := trivial"), 1);
        // `by` block: header line + one tactic.
        assert_eq!(proof_size(SHORT_PROOF), 2);
        // Separators `;` and `<;>` split steps; empty proof is size 0.
        assert_eq!(proof_size("intro x; simp <;> ring"), 3);
        assert_eq!(proof_size("   \n  \n"), 0);
        // Comment-only lines / trailing comments contribute nothing.
        assert_eq!(proof_size("simp -- trivial\n-- just a note\nring"), 2);
        assert_eq!(proof_size("/- a\n block -/\nsimp\n(* rocq *) ring"), 2);
        assert!(proof_size(LONG_PROOF) > ShortProofCfg::default().trivial_len);
    }

    #[test]
    fn hard_target_with_tiny_proof_is_flagged_advisory() {
        let cfg = ShortProofCfg::default();
        let finding = short_proof_for_hard_target(0.9, SHORT_PROOF, &cfg)
            .expect("hard target + tiny proof must flag");
        assert_eq!(finding.category, SUSPICIOUS_SHORT_PROOF_CATEGORY);
        assert_eq!(finding.severity, "advisory");
        // Advisory ⇒ a justification gap, never a critical error; unconfirmed.
        assert_eq!(finding.class, FindingClass::JustificationGap);
        assert!(!finding.confirmed);
        assert!(!finding.adversarial_checks.is_empty());
    }

    #[test]
    fn easy_target_with_short_proof_is_not_flagged() {
        let cfg = ShortProofCfg::default();
        // Difficulty below the hard threshold: a short proof is expected, no smell.
        assert!(short_proof_for_hard_target(0.3, SHORT_PROOF, &cfg).is_none());
    }

    #[test]
    fn hard_target_with_substantial_proof_is_not_flagged() {
        let cfg = ShortProofCfg::default();
        assert!(short_proof_for_hard_target(0.95, LONG_PROOF, &cfg).is_none());
    }

    #[test]
    fn empty_proof_is_not_flagged() {
        let cfg = ShortProofCfg::default();
        // No proof text to judge ⇒ no false alarm.
        assert!(short_proof_for_hard_target(0.99, "   \n\n", &cfg).is_none());
    }

    #[test]
    fn config_thresholds_are_respected() {
        // A lenient hard_threshold flags a mid-difficulty node; a strict one does not.
        let lenient = ShortProofCfg {
            hard_threshold: 0.5,
            trivial_len: 2,
        };
        let strict = ShortProofCfg {
            hard_threshold: 0.8,
            trivial_len: 2,
        };
        assert!(short_proof_for_hard_target(0.6, SHORT_PROOF, &lenient).is_some());
        assert!(short_proof_for_hard_target(0.6, SHORT_PROOF, &strict).is_none());

        // trivial_len controls the size cutoff at exactly the boundary.
        let three_steps = "intro x; simp; ring"; // size 3
        assert_eq!(proof_size(three_steps), 3);
        let cutoff_2 = ShortProofCfg {
            hard_threshold: 0.7,
            trivial_len: 2,
        };
        let cutoff_3 = ShortProofCfg {
            hard_threshold: 0.7,
            trivial_len: 3,
        };
        assert!(short_proof_for_hard_target(0.9, three_steps, &cutoff_2).is_none());
        assert!(short_proof_for_hard_target(0.9, three_steps, &cutoff_3).is_some());
    }

    #[test]
    fn advisory_finding_never_rejects_a_node() {
        let mut finding = short_proof_for_hard_target(0.9, SHORT_PROOF, &ShortProofCfg::default())
            .expect("should flag");
        finding.node_id = Some("n1".to_owned());
        // Even if a downstream meta pass were to mark it confirmed, a
        // justification gap can never drive a rejection.
        finding.confirmed = true;
        let report = CritiqueReport {
            project_id: "p".to_owned(),
            findings: vec![finding],
            pruned: Vec::new(),
            summary: String::new(),
        };
        assert!(!report.should_reject_node("n1"));
    }

    #[test]
    fn helper_is_deterministic() {
        let cfg = ShortProofCfg::default();
        let a = short_proof_for_hard_target(0.9, SHORT_PROOF, &cfg).unwrap();
        let b = short_proof_for_hard_target(0.9, SHORT_PROOF, &cfg).unwrap();
        assert_eq!(a.category, b.category);
        assert_eq!(a.issue, b.issue);
        assert_eq!(a.class, b.class);
    }

    /// A critic provider that reports NO structural findings — so the only
    /// finding a `critique()` can surface is the deterministic short-proof
    /// advisory wired into the finding-production path.
    struct SilentCritic;
    impl ModelProvider for SilentCritic {
        fn complete(&self, _req: &ModelRequest) -> Result<ModelResponse> {
            Ok(ModelResponse {
                content: json!({ "findings": [], "summary": "no structural findings" }),
                model: "test".into(),
                provider: "test".into(),
            })
        }
        fn name(&self) -> &str {
            "test"
        }
    }

    #[test]
    fn critique_emits_short_proof_advisory_for_a_hard_certified_node() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "t").unwrap();
        let node = store
            .add_node(&project.id, NodeKind::Lemma, "hard lemma", "S", "test")
            .unwrap();
        // Mark it hard (strategy hint), give it a trivial certified proof.
        store
            .set_strategy_hint(
                &project.id,
                &node.id,
                Some("this is a hard, deep step"),
                "test",
            )
            .unwrap();
        store
            .set_formal_statement(&project.id, &node.id, "theorem t : P := by simp", "test")
            .unwrap();
        store
            .set_verification_flags(&project.id, &node.id, true, true, "test")
            .unwrap();

        let critic = Critic {
            store: &store,
            provider: &SilentCritic,
        };
        let report = critic.critique(&project.id).unwrap();
        let advisory: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.category == SUSPICIOUS_SHORT_PROOF_CATEGORY)
            .collect();
        assert_eq!(
            advisory.len(),
            1,
            "one hard+short certified node ⇒ one advisory"
        );
        assert_eq!(advisory[0].node_id.as_deref(), Some(node.id.as_str()));
        // It is advisory: it does not reject the node.
        assert!(!report.should_reject_node(&node.id));
        // It was grounded onto the node as durable evidence.
        let events = store.events(&project.id, 50).unwrap();
        assert!(events.iter().any(|e| e.event_type == "evidence.recorded"));
    }

    #[test]
    fn critique_does_not_flag_an_easy_or_uncertified_short_proof() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "t").unwrap();

        // Easy (no hard hint) but certified with a short proof: not flagged.
        let easy = store
            .add_node(&project.id, NodeKind::Lemma, "easy", "S", "test")
            .unwrap();
        store
            .set_formal_statement(&project.id, &easy.id, "theorem t : P := by simp", "test")
            .unwrap();
        store
            .set_verification_flags(&project.id, &easy.id, true, true, "test")
            .unwrap();

        // Hard + short proof but NOT certified (no proof_done): not flagged.
        let uncertified = store
            .add_node(&project.id, NodeKind::Lemma, "hard-open", "S", "test")
            .unwrap();
        store
            .set_strategy_hint(&project.id, &uncertified.id, Some("hard"), "test")
            .unwrap();
        store
            .set_formal_statement(
                &project.id,
                &uncertified.id,
                "theorem t : P := by simp",
                "test",
            )
            .unwrap();

        let critic = Critic {
            store: &store,
            provider: &SilentCritic,
        };
        let report = critic.critique(&project.id).unwrap();
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.category == SUSPICIOUS_SHORT_PROOF_CATEGORY),
            "neither an easy node nor an uncertified node should be flagged"
        );
    }
}
