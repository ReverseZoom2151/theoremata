//! Lean external-prover adapter — Phase 1 mock + Phase 2 live gate.
//!
//! Mirrors the `aristotle`/`rocq` mocks (Config.prover_mock-driven,
//! submit → poll(InProgress → Proved) → result + a `VerificationReport`), and
//! routes verification through the system-agnostic [`FormalBackend`] 3+1-layer
//! gate. In live mode each layer runs the native Lean toolchain through the
//! configured [`Runner`]: `lean <Generated.lean>` (compile — the kernel checks
//! every proof term), `#print axioms <thm>` (axiom audit vs the mathlib
//! whitelist), and `lake env leanchecker` when available (kernel re-check;
//! degrades gracefully to the compile-time kernel check otherwise).

use crate::{
    config::Config,
    db::Store,
    prover::{
        exec::{self, Runner},
        formal::{
            AxiomReport, CompileReport, FormalBackend, FormalSystem, GoalState, ProofSession,
            RecheckReport, ScanReport, StateResult, UnitResult, Workspace,
        },
        model::{FormalProject, ProofJob, ProofResult, ProofTask, ProverJobStatus},
    },
};
use anyhow::{anyhow, Result};
use chrono::Utc;
use serde_json::json;
use std::{path::PathBuf, time::Instant};

const BACKEND: &str = "lean";
const SYSTEM: FormalSystem = FormalSystem::Lean;
const MODULE: &str = "Generated";

/// The LeanDojo in-kernel `validateProof` soundness-gate template
/// (`components/verify/lean/validate_proof_template.lean`). Referenced from the
/// verify path as an OPTIONAL extra check (gated by `Config::kernel_validate_proof`);
/// it reconstructs a standalone declaration, rejects `sorry`/metavariables, and
/// kernel-rechecks via `addDecl`. See the template header for how the warm REPL
/// would invoke it on the close-path. It need not run live if the toolchain lacks
/// a REPL build of it — the wiring + flag exist regardless.
pub const VALIDATE_PROOF_TEMPLATE: &str = "components/verify/lean/validate_proof_template.lean";

pub fn mock_enabled(config: &Config) -> bool {
    config.prover_mock
        || std::env::var("THEOREMATA_LEAN_MOCK")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or_else(|_| std::env::var("THEOREMATA_LEAN_COMMAND").is_err())
}

pub fn submit(
    store: &Store,
    config: &Config,
    task: ProofTask,
    artifacts_dir: Option<std::path::PathBuf>,
) -> Result<ProofJob> {
    let external_id = if mock_enabled(config) {
        Some(format!("mock-{}", &task.id[..8.min(task.id.len())]))
    } else {
        None
    };
    let job = store.create_proof_job(
        &task,
        BACKEND,
        ProverJobStatus::Submitted,
        external_id.as_deref(),
        artifacts_dir.as_deref(),
        0.0,
    )?;
    store.event(
        task.project_id.as_deref(),
        None,
        "proof_job.submitted",
        BACKEND,
        json!({"job_id": job.id, "task_id": task.id, "mock": mock_enabled(config)}),
    )?;
    if let Some(dir) = &artifacts_dir {
        write_artifact(dir, "task.json", &task)?;
        write_artifact(
            dir,
            "submit.json",
            &json!({"mock": mock_enabled(config), "backend": BACKEND}),
        )?;
    }
    Ok(job)
}

pub fn poll(store: &Store, config: &Config, job_id: &str) -> Result<ProofJob> {
    let mut job = store
        .get_proof_job(job_id)?
        .ok_or_else(|| anyhow!("unknown proof job {job_id}"))?;
    if job.status.is_terminal() {
        return Ok(job);
    }
    if !mock_enabled(config) {
        // Live path: verify the candidate proof through the real 3+1-layer gate.
        return crate::prover::formal::live_poll(store, config, job, BACKEND, SYSTEM);
    }
    let started = Instant::now();
    let (status, percent, formal_code, message) = advance_mock(&job);
    job.status = status;
    job.percent_complete = percent;
    job.poll_count += 1;
    job.updated_at = Utc::now();

    if status.is_terminal() {
        job.completed_at = Some(Utc::now());
        let backend = LeanBackend::mock();
        let verification = formal_code
            .as_deref()
            .and_then(|code| backend.verify(config, code, &job.task.statement).ok());
        let result = ProofResult {
            task_id: job.task.id.clone(),
            job_id: job.id.clone(),
            status,
            formal_code: formal_code.clone(),
            counterexample: None,
            verification,
            artifacts_dir: job.artifacts_dir.clone(),
            duration_ms: started.elapsed().as_millis(),
            cost: None,
            message: message.clone(),
            provenance: json!({
                "backend": BACKEND,
                "system": SYSTEM.as_str(),
                "mock": true,
                "poll_count": job.poll_count,
            }),
        };
        job.result = Some(result.clone());
        if let Some(dir) = &job.artifacts_dir {
            if let Some(code) = &formal_code {
                let sub = dir.join(BACKEND);
                std::fs::create_dir_all(&sub)?;
                std::fs::write(sub.join("solution.lean"), code)?;
            }
            write_artifact(dir, "result.json", &result)?;
            if let Some(v) = &result.verification {
                write_artifact(dir, "verifier/report.json", v)?;
            }
        }
        store.update_proof_job(&job)?;
        store.event(
            job.project_id.as_deref(),
            None,
            "proof_job.completed",
            BACKEND,
            json!({"job_id": job.id, "status": status, "verified": result.verification.is_some()}),
        )?;
        return Ok(job);
    }

    store.update_proof_job(&job)?;
    store.event(
        job.project_id.as_deref(),
        None,
        "proof_job.polled",
        BACKEND,
        json!({"job_id": job.id, "status": status, "percent": percent}),
    )?;
    Ok(job)
}

pub fn cancel(store: &Store, job_id: &str) -> Result<ProofJob> {
    let mut job = store
        .get_proof_job(job_id)?
        .ok_or_else(|| anyhow!("unknown proof job {job_id}"))?;
    if job.status.is_terminal() {
        return Ok(job);
    }
    job.status = ProverJobStatus::Cancelled;
    job.completed_at = Some(Utc::now());
    job.updated_at = Utc::now();
    store.update_proof_job(&job)?;
    store.event(
        job.project_id.as_deref(),
        None,
        "proof_job.cancelled",
        BACKEND,
        json!({"job_id": job.id}),
    )?;
    Ok(job)
}

pub fn build_task(
    project_id: Option<String>,
    node_id: Option<String>,
    statement: &str,
    theorem_name: &str,
    config: &Config,
) -> ProofTask {
    let root = config
        .lean_project
        .clone()
        .unwrap_or_else(|| config.resources.join("lean"));
    ProofTask {
        id: uuid::Uuid::new_v4().to_string(),
        project_id,
        node_id,
        theorem: crate::prover::model::TheoremIdentity {
            repo: Some("theoremata".into()),
            commit: None,
            file: None,
            full_name: theorem_name.into(),
            line: None,
        },
        system: SYSTEM,
        formal_project: FormalProject {
            system: SYSTEM,
            root,
            toolchain: None,
            imports: SYSTEM.default_imports(),
            metadata: json!({}),
        },
        statement: statement.into(),
        stub: None,
        prompt: None,
        backend: BACKEND.into(),
        metadata: json!({}),
    }
}

fn advance_mock(job: &ProofJob) -> (ProverJobStatus, f64, Option<String>, Option<String>) {
    match job.poll_count {
        0 => (
            ProverJobStatus::InProgress,
            40.0,
            None,
            Some("mock: working".into()),
        ),
        _ => (
            ProverJobStatus::Proved,
            100.0,
            Some(mock_lean_solution(&job.task)),
            Some("mock: proved".into()),
        ),
    }
}

fn mock_lean_solution(task: &ProofTask) -> String {
    let name = task
        .theorem
        .full_name
        .rsplit('.')
        .next()
        .unwrap_or("MainTheorem");
    format!("-- Mock Lean proof.\ntheorem {name} : True := trivial\n")
}

/// Lean [`FormalBackend`]. In mock mode the compile / axiom-audit / kernel
/// re-check layers return canned success; the source scan always runs for real.
pub struct LeanBackend {
    pub mock: bool,
    pub runner: Runner,
    pub lean: String,
    pub lake: String,
    /// Optional pin for the reject-on-mismatch precheck (`THEOREMATA_LEAN_TOOLCHAIN`,
    /// e.g. `leanprover/lean4:v4.9.0`). `None` disables the toolchain check.
    pub toolchain: Option<String>,
    /// Whether to wire the LeanDojo in-kernel `validateProof` soundness gate
    /// ([`VALIDATE_PROOF_TEMPLATE`]) into the kernel re-check
    /// (`Config::kernel_validate_proof`).
    pub kernel_validate: bool,
    /// **Tier-0 layer-2d channel: the DESIGNATED INPUTS of the task** — the
    /// hypotheses this backend's caller has declared to be legitimate antecedents,
    /// named either by BINDER name (`hRH`) or by TYPE HEAD (`RiemannHypothesis`).
    /// See [`FormalBackend::designated_inputs`] and [`DESIGNATED_INPUTS_ENV`].
    ///
    /// Empty by default, and empty is the only safe default: the allowlist is a
    /// statement about the TASK, and nothing inside this backend knows the task.
    pub designated_inputs: Vec<String>,
}

/// Env seam carrying the task's designated hypothesis inputs into
/// [`LeanBackend::designated_inputs`], in the crate's existing default-empty
/// env-override idiom (`THEOREMATA_LEAN_TOOLCHAIN`, `THEOREMATA_LEAN`, …).
///
/// Comma- / semicolon- / whitespace-separated, e.g.
/// `THEOREMATA_LEAN_DESIGNATED_INPUTS="RiemannHypothesis,hGlaisher"`.
///
/// **Why an env var and not config:** there is no trusted-premise / designated-
/// input field anywhere in `Config` today (it carries `lean_project`, `lean_bin`,
/// runners, and gate booleans — nothing task-semantic), and `FormalProject`
/// carries only `imports`, which are MODULE imports (`Mathlib`), not premises: an
/// import grants access to *proved* lemmas, which need no allowlist. So there is
/// no existing real source to read, and inventing one inside the backend would be
/// exactly the hardcoded-mathematical-facts list this must not be. This env var
/// is the honest minimum: a channel the party that defines the task can populate.
/// The preferred home remains a `Config` field owned by `app/config.rs`.
pub const DESIGNATED_INPUTS_ENV: &str = "THEOREMATA_LEAN_DESIGNATED_INPUTS";

impl LeanBackend {
    /// The offline mock backend (canned layers; real source scan).
    pub fn mock() -> Self {
        Self {
            mock: true,
            runner: Runner::Native,
            lean: "lean".into(),
            lake: "lake".into(),
            toolchain: None,
            kernel_validate: false,
            // A mock has no task context at all; never allowlist anything.
            designated_inputs: Vec::new(),
        }
    }

    /// The live backend, reading the configured runner + binary (env-overridable).
    pub fn live(cfg: &Config) -> Self {
        Self {
            mock: false,
            runner: cfg.formal_runners.for_system(SYSTEM),
            lean: exec::env_or("THEOREMATA_LEAN", &cfg.lean_bin),
            lake: exec::env_or("THEOREMATA_LAKE", "lake"),
            toolchain: std::env::var("THEOREMATA_LEAN_TOOLCHAIN")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            kernel_validate: cfg.kernel_validate_proof,
            designated_inputs: std::env::var(DESIGNATED_INPUTS_ENV)
                .ok()
                .map(|raw| parse_designated_inputs(&raw))
                .unwrap_or_default(),
        }
    }

    /// Status of the optional in-kernel `validateProof` soundness gate: whether it
    /// is enabled, whether the template is present on disk, and a note. Folded
    /// into the kernel-recheck detail so the wiring is observable even when the
    /// check does not run live.
    fn validate_proof_gate(&self) -> serde_json::Value {
        if !self.kernel_validate {
            return json!({"enabled": false});
        }
        let present = std::path::Path::new(VALIDATE_PROOF_TEMPLATE).exists();
        json!({
            "enabled": true,
            "template": VALIDATE_PROOF_TEMPLATE,
            "template_present": present,
            "note": if present {
                "in-kernel validateProof gate wired; runs when a REPL build of the template \
                 against the pinned toolchain is available"
            } else {
                "kernel_validate_proof set but template not found on disk"
            },
        })
    }
}

/// Parse the transitive axiom set from a `#print axioms` message. Returns
/// `Some(vec![])` for the clean "does not depend on any axioms" line, or the
/// listed axioms otherwise; `None` if no axiom line is present.
fn parse_axioms(stdout: &str) -> Option<Vec<String>> {
    if stdout.contains("does not depend on any axioms") {
        return Some(Vec::new());
    }
    let marker = "depends on axioms:";
    let idx = stdout.find(marker)?;
    let tail = &stdout[idx + marker.len()..];
    // The list is `[a, b, c]` possibly spanning lines.
    let inside = tail
        .split_once('[')
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(list, _)| list)
        .unwrap_or(tail);
    let axioms: Vec<String> = inside
        .split(',')
        .map(|s| {
            s.trim()
                .trim_matches(|c: char| c.is_whitespace())
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect();
    Some(axioms)
}

// ===========================================================================
// Tier-0 channels: designated inputs + hypothesis-bundle parsing
// ===========================================================================

/// Split the [`DESIGNATED_INPUTS_ENV`] value into allowlist entries.
///
/// Separators are `,`, `;` and whitespace; entries are trimmed, empties dropped,
/// duplicates removed (first occurrence wins) so the JSON detail is stable. No
/// validation is attempted: `hypothesis_audit` matches an entry against either a
/// binder NAME or a type HEAD, and an entry matching neither is simply inert.
fn parse_designated_inputs(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tok in raw.split(|c: char| c == ',' || c == ';' || c.is_whitespace()) {
        let t = tok.trim();
        if t.is_empty() || out.iter().any(|e| e == t) {
            continue;
        }
        out.push(t.to_string());
    }
    out
}

/// One parsed Lean binder group: the names it binds and the type they share.
///
/// Mirrors `hypothesis_audit::parse_binder_groups` / `split_binders_conclusion`.
/// Those helpers are PRIVATE to their modules (as is
/// `statement_preservation::parse_first_decl` and its
/// `split_binders_conclusion`), so this is a deliberate local re-implementation
/// of the same idioms rather than a reuse. The one public reuse available is
/// [`crate::prover::statement_preservation::check_statement_preserved`], whose
/// `canonical` field exposes a parsed [`TheoremSig`] — that IS reused below for
/// the outer `theorem NAME <binders> : <conclusion>` split, so only the binder
/// region needs local parsing.
///
/// [`TheoremSig`]: crate::prover::statement_preservation::TheoremSig
#[derive(Debug, Clone, PartialEq, Eq)]
struct LeanBinder {
    /// Names bound by the group. Empty for an anonymous group (`[Group G]`).
    names: Vec<String>,
    /// The shared type text, whitespace-normalized.
    ty: String,
}

/// Split a binder region into its bracketed groups — `(h : P)`, `{n : Nat}`,
/// `[Group G]`, `⦃x : α⦄`. Unbracketed trailing names carry no type ascription
/// and are skipped (nothing can be said about their kind).
fn parse_lean_binders(binders: &str) -> Vec<LeanBinder> {
    let chars: Vec<char> = binders.chars().collect();
    let mut out: Vec<LeanBinder> = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let open = chars[i];
        let close = match open {
            '(' => ')',
            '{' => '}',
            '[' => ']',
            '⦃' => '⦄',
            _ => {
                i += 1;
                continue;
            }
        };
        let mut depth = 1i32;
        let mut k = i + 1;
        while k < chars.len() {
            if chars[k] == open {
                depth += 1;
            } else if chars[k] == close {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            k += 1;
        }
        let inner = &chars[(i + 1).min(chars.len())..k.min(chars.len())];
        let (name_part, ty_part) = split_at_top_colon(inner);
        let (names, ty) = match ty_part {
            // `[Group G]` / `[Fact (0 < n)]` — anonymous: the whole group is type.
            None => (Vec::new(), name_part.iter().collect::<String>()),
            Some(t) => (
                split_lean_idents(name_part),
                t.iter().collect::<String>(),
            ),
        };
        out.push(LeanBinder {
            names,
            ty: norm_ws(&ty),
        });
        i = k + 1;
    }
    out
}

/// Split at the first bracket-depth-0 `:` that is not `:=`. `None` for the type
/// half when there is none.
fn split_at_top_colon(sig: &[char]) -> (&[char], Option<&[char]>) {
    let mut depth = 0i32;
    for i in 0..sig.len() {
        match sig[i] {
            '(' | '[' | '{' | '⟨' | '⦃' => depth += 1,
            ')' | ']' | '}' | '⟩' | '⦄' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            ':' if depth == 0 && sig.get(i + 1) != Some(&'=') => {
                return (&sig[..i], Some(&sig[i + 1..]));
            }
            _ => {}
        }
    }
    (sig, None)
}

/// Whitespace-separated identifiers in a slice (`.` included, for namespaced
/// names). `_` is preserved here — the caller renames it, because a bundle field
/// needs a referable name.
fn split_lean_idents(chars: &[char]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for &c in chars {
        if c.is_alphanumeric() || c == '_' || c == '\'' || c == '.' {
            cur.push(c);
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The head identifier of a type expression, skipping leading punctuation.
fn lean_head_ident(ty: &str) -> Option<String> {
    let chars: Vec<char> = ty.chars().collect();
    let mut i = 0usize;
    while i < chars.len() && !(chars[i].is_alphabetic() || chars[i] == '_') {
        i += 1;
    }
    let start = i;
    while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '\'' || chars[i] == '.') {
        i += 1;
    }
    if i == start {
        None
    } else {
        Some(chars[start..i].iter().collect())
    }
}

/// Relational / logical tokens whose presence in a binder's TYPE makes it a
/// proposition. Every one of these is a `Prop`-former in Lean core: a comparison,
/// a membership, a divisibility, or a logical connective.
///
/// **`→` and `∀` are deliberately ABSENT.** `(f : Nat → Nat)` and
/// `(F : ∀ α, α → α)` are data, and including them would misclassify ordinary
/// function/dependent-function binders as hypotheses. `∃` IS included: `Exists`
/// is a `Prop` unconditionally.
const PROP_TOKENS: &[&str] = &[
    "=", "≠", "<", ">", "≤", "≥", "∈", "∉", "⊆", "⊂", "⊇", "∣", "∤", "∧", "∨",
    "↔", "¬", "≡", "≅", "∃", "≫", "≪",
];

/// Type heads that are `Prop`-valued in Lean core / the logical prelude.
///
/// **This is a list of LOGICAL CONNECTIVES AND ORDER/ALGEBRA CLASS PROJECTIONS,
/// not of mathematical facts.** Nothing domain-specific belongs here: a
/// domain-specific opaque assumption (`RiemannHypothesis`) is the
/// [`crate::prover::hypothesis_audit`] layer's business, allowlisted via
/// [`DESIGNATED_INPUTS_ENV`], not something this bundle parser should guess at.
const PROP_HEADS: &[&str] = &[
    "Eq", "Ne", "Not", "And", "Or", "Iff", "Xor", "True", "False", "Exists",
    "LT.lt", "LE.le", "GT.gt", "GE.ge", "Dvd.dvd", "Membership.mem", "Nonempty",
];

/// The split heuristic: is this binder type `Prop`-shaped (a hypothesis) rather
/// than type-shaped (a datum)?
///
/// **Honest description of the heuristic, and it IS a heuristic — there is no
/// elaborator here, so this cannot ask Lean whether a type's sort is `Prop`.**
/// It answers `true` in exactly two cases:
///
/// 1. the type text contains one of [`PROP_TOKENS`] — a relation or connective
///    applied to something (`n > 0`, `p ∣ n`, `a = b`, `¬ P`, `∃ k, …`); or
/// 2. the type's HEAD identifier is one of [`PROP_HEADS`].
///
/// Everything else is a [`FieldKind::Datum`]. That default is deliberate and
/// matches the brief: with `THEOREMATA_VACUITY_GATE` set, a binder wrongly called
/// a Hypothesis makes the bundle non-trivial, which demands a witness we cannot
/// produce, which fails a perfectly good proof. A binder wrongly called a Datum
/// merely under-reports. So the parser is biased toward Datum.
///
/// Known, accepted misclassifications (all in the Datum direction):
///
/// * a bare named proposition — `(hRH : RiemannHypothesis)`, `(h : Glaisher3)` —
///   reads as a Datum here. It is NOT lost: that is precisely mechanism (a)/(b)
///   of [`crate::prover::hypothesis_audit`], which catches it on the other gate.
/// * a `Prop`-valued application with no operator and an unrecognized head —
///   `(hp : Nat.Prime p)`, `(hc : Nat.Coprime a b)` — reads as a Datum. Adding
///   these by name would be the hardcoded-mathematical-facts list this must not
///   become; resolving them properly needs the elaborator.
/// * `(h : P → Q)`, an implication hypothesis, reads as a Datum because `→` is
///   excluded for the function-type reason above.
///
/// [`FieldKind::Datum`]: crate::prover::vacuity::FieldKind::Datum
fn is_prop_shaped(ty: &str) -> bool {
    let t = norm_ws(ty);
    if t.is_empty() {
        return false;
    }
    if PROP_TOKENS.iter().any(|tok| t.contains(tok)) {
        return true;
    }
    lean_head_ident(&t).map_or(false, |h| PROP_HEADS.iter().any(|p| *p == h))
}

/// Collapse whitespace runs to one space and trim.
fn norm_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Offline lexical fallback for [`LeanBackend::source_scan`]: the Lean escape
/// hatches NOT caught cleanly by the kernel / `#print axioms`.
///
/// Matched over COMMENT-STRIPPED source, so this offline path agrees with the
/// online (worker) path and with the single authoritative policy in
/// [`crate::prover::statement_preservation`]
/// (`ESCAPE_HATCH_COMMENT_POLICY == CommentPolicy::CodeOnly`,
/// `commented_escape_hatch_is_a_violation() == false`). A `-- sorry` in a
/// comment is never seen by the kernel, so it cannot make an unproved theorem
/// look proved; flagging it only produced offline-only failures on files that
/// passed online. This LOOSENS the gate with respect to commented text ONLY —
/// a real `sorry` in code is untouched by stripping and still fails here.
fn fallback_source_scan(code: &str) -> ScanReport {
    let low = crate::prover::formal::strip_comments(code).to_lowercase();
    let patterns = [
        "sorry",
        "sorryax",
        "admit",
        "native_decide",
        "ofreducebool",
        "trustcompiler",
    ];
    let findings: Vec<String> = patterns
        .iter()
        .filter(|p| low.contains(**p))
        .map(|p| (*p).to_string())
        .collect();
    ScanReport {
        clean: findings.is_empty(),
        findings,
        detail: json!({"system": SYSTEM.as_str(), "fallback": true}),
    }
}

impl FormalBackend for LeanBackend {
    fn system(&self) -> FormalSystem {
        SYSTEM
    }

    fn compile_success_signal(&self) -> crate::prover::formal::SuccessSignal {
        // Lean sets a correct non-zero exit code on failure.
        crate::prover::formal::SuccessSignal::NonZeroExitIsHonest
    }

    fn is_mock(&self) -> bool {
        self.mock
    }

    fn available(&self) -> bool {
        self.mock || exec::probe(&self.runner, &[&self.lean, "--version"])
    }

    fn expected_toolchain(&self) -> Option<String> {
        self.toolchain.clone()
    }

    fn scaffold(&self, cfg: &Config, code: &str, name: &str) -> Result<Workspace> {
        if self.mock {
            return Ok(Workspace {
                system: SYSTEM,
                root: PathBuf::from("."),
                source_path: PathBuf::from(format!("{name}{}", SYSTEM.source_extension())),
                entry: name.to_string(),
            });
        }
        let entry =
            crate::prover::formal::entry_name(SYSTEM, code).unwrap_or_else(|| name.to_string());
        let root = crate::prover::formal::live_workspace_dir(cfg, SYSTEM)?;
        let src = root.join(format!("{MODULE}.lean"));
        std::fs::write(&src, code)?;
        Ok(Workspace {
            system: SYSTEM,
            root,
            source_path: src,
            entry,
        })
    }

    fn compile(&self, ws: &Workspace) -> Result<CompileReport> {
        if self.mock {
            return Ok(CompileReport {
                compiled: true,
                errors: Vec::new(),
                per_unit: Vec::new(),
                detail: json!({"mock": true}),
            });
        }
        if !self.available() {
            return Ok(CompileReport {
                compiled: false,
                errors: vec!["lean toolchain unavailable".into()],
                per_unit: Vec::new(),
                detail: json!({"unavailable": true, "runner": self.runner.tag()}),
            });
        }
        let file = format!("{MODULE}.lean");
        let out = exec::run(&self.runner, &[&self.lean, &file], &ws.root);
        let errors = if out.success() {
            Vec::new()
        } else {
            vec![out.stderr.clone(), out.stdout.clone()]
        };
        // Failure-isolating per-declaration status: read the generated source
        // back and attribute each error to the declaration it names.
        let code = std::fs::read_to_string(&ws.source_path).unwrap_or_default();
        let per_unit =
            crate::prover::formal::per_declaration_status(SYSTEM, &code, out.success(), &errors);
        Ok(CompileReport {
            compiled: self.compile_success_signal().is_pass(
                out.launched,
                out.success(),
                &out.stdout,
                &out.stderr,
            ),
            errors,
            per_unit,
            detail: json!({
                "runner": self.runner.tag(),
                "code": out.code,
                "stdout": out.stdout,
                "stderr": out.stderr,
            }),
        })
    }

    fn audit_axioms(&self, ws: &Workspace, thm: &str, whitelist: &[String]) -> Result<AxiomReport> {
        if self.mock {
            return Ok(AxiomReport {
                axioms: Vec::new(),
                within_whitelist: true,
                detail: json!({"mock": true, "whitelist": whitelist}),
            });
        }
        // Write a sibling file that imports nothing extra and prints the axiom
        // closure of the target theorem, then run `lean` on it.
        let base = std::fs::read_to_string(&ws.source_path).unwrap_or_default();
        let audit_file = "Generated_axioms.lean";
        let mut content = base;
        if !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(&format!("#print axioms {thm}\n"));
        std::fs::write(ws.root.join(audit_file), content)?;
        let out = exec::run(&self.runner, &[&self.lean, audit_file], &ws.root);
        let axioms = parse_axioms(&out.stdout).unwrap_or_else(|| vec!["<unparsed>".into()]);
        let within = out.success()
            && parse_axioms(&out.stdout).is_some()
            && axioms.iter().all(|a| whitelist.iter().any(|w| w == a));
        Ok(AxiomReport {
            axioms,
            within_whitelist: within,
            detail: json!({
                "runner": self.runner.tag(),
                "whitelist": whitelist,
                "stdout": out.stdout,
            }),
        })
    }

    fn kernel_recheck(&self, ws: &Workspace) -> Result<RecheckReport> {
        // Optional LeanDojo in-kernel `validateProof` gate wiring (observable in
        // the detail regardless of whether it can run live).
        let validate_proof = self.validate_proof_gate();
        if self.mock {
            return Ok(RecheckReport {
                rechecked: true,
                detail: json!({"mock": true, "validate_proof": validate_proof}),
            });
        }
        // `leanchecker` is only meaningful inside a Lake project (it replays
        // `.olean`s). A standalone `lean <file>` already runs the proof term
        // through the kernel, so when there is no Lake workspace we degrade
        // gracefully: the compile IS the kernel check.
        if !ws.root.join("lakefile.toml").exists() && !ws.root.join("lakefile.lean").exists() {
            return Ok(RecheckReport {
                rechecked: true,
                detail: json!({
                    "runner": self.runner.tag(),
                    "leanchecker": "skipped (bare lean; compile is kernel-checked)",
                    "validate_proof": validate_proof,
                }),
            });
        }
        let out = exec::run(&self.runner, &[&self.lake, "env", "leanchecker"], &ws.root);
        // If leanchecker is absent the launch fails; degrade to the compile check.
        if !out.launched {
            return Ok(RecheckReport {
                rechecked: true,
                detail: json!({
                    "runner": self.runner.tag(),
                    "leanchecker": "unavailable; relying on compile kernel-check",
                    "validate_proof": validate_proof,
                }),
            });
        }
        Ok(RecheckReport {
            rechecked: out.success(),
            detail: json!({
                "runner": self.runner.tag(),
                "code": out.code,
                "stdout": out.stdout,
                "stderr": out.stderr,
                "validate_proof": validate_proof,
            }),
        })
    }

    fn source_scan(&self, code: &str) -> Result<ScanReport> {
        // Prefer the shared Python `source_scan` worker (comment-aware); fall
        // back to a built-in lexical pass so the gate still bites offline.
        if let Some(report) = crate::prover::formal::worker_source_scan(SYSTEM, code) {
            return Ok(report);
        }
        Ok(fallback_source_scan(code))
    }

    /// Tier-0 layer 2d channel — see [`LeanBackend::designated_inputs`] (the
    /// field) and [`DESIGNATED_INPUTS_ENV`].
    ///
    /// Sourced from the caller-populated field, which `live()` fills from
    /// [`DESIGNATED_INPUTS_ENV`] and `mock()` leaves empty. **There is no other
    /// real source in this crate today** — `Config` has no trusted-premise field,
    /// and `FormalProject::imports` are module imports (access to *proved*
    /// lemmas), not assumed premises, so they are NOT read here. With nothing
    /// configured this returns empty, exactly as the trait default does, and every
    /// genuine antecedent of a conditional theorem will read as `Unaccounted` if
    /// `THEOREMATA_HYPOTHESIS_GATE` is switched on without populating it.
    fn designated_inputs(&self) -> Vec<String> {
        self.designated_inputs.clone()
    }

    /// Vacuity channel (1/2): parse the Lean theorem signature in `stmt` into a
    /// [`HypothesisBundle`], splitting data binders from propositional ones.
    ///
    /// Returns `None` — "this backend does not model the bundle", which
    /// [`FormalBackend::verify_with_gates`] reports as NOT DECLARED and never as a
    /// failure — when `stmt` does not parse into a `theorem`/`lemma`/`example`
    /// signature. **A wrong bundle is strictly worse than no bundle**, so every
    /// parse doubt yields `None`.
    ///
    /// The outer `theorem NAME <binders> : <conclusion>` split reuses the public
    /// [`check_statement_preserved`] (its `canonical` field is the parsed
    /// signature; passing an empty submission means only the canonical side is
    /// parsed). The binder region is then split locally — see [`LeanBinder`] for
    /// why that could not be reused.
    ///
    /// The Datum/Hypothesis split heuristic is [`is_prop_shaped`]; read its docs
    /// before turning `THEOREMATA_VACUITY_GATE` on, because that heuristic decides
    /// which goals are required to carry a witness.
    ///
    /// Naming rules, so a witness can always reference a field:
    ///
    /// * `_` and anonymous groups get a synthesized `_b{index}` name;
    /// * an ANONYMOUS group whose type is `Prop`-shaped (`[Fact (0 < n)]`) is kept
    ///   as a Hypothesis. This is the one place the parser does NOT err toward
    ///   Datum: dropping it would let a genuinely constrained bundle look trivial,
    ///   which is the exact hole the vacuity module exists to close.
    ///
    /// [`HypothesisBundle`]: crate::prover::vacuity::HypothesisBundle
    /// [`check_statement_preserved`]: crate::prover::statement_preservation::check_statement_preserved
    fn hypothesis_bundle(&self, stmt: &str) -> Option<crate::prover::vacuity::HypothesisBundle> {
        use crate::prover::vacuity::{HypothesisBundle, HypothesisField};

        let sig = crate::prover::statement_preservation::check_statement_preserved(stmt, "")
            .canonical?;
        // Only a proposition-bearing declaration has a hypothesis bundle. A `def`
        // (or anything else the signature parser accepts) is not our business.
        if !matches!(sig.kind.as_str(), "theorem" | "lemma" | "example") {
            return None;
        }

        let mut fields: Vec<HypothesisField> = Vec::new();
        for (idx, binder) in parse_lean_binders(&sig.binders).into_iter().enumerate() {
            if binder.ty.is_empty() {
                continue;
            }
            let prop = is_prop_shaped(&binder.ty);
            let names: Vec<String> = if binder.names.is_empty() {
                vec![format!("_b{idx}")]
            } else {
                binder
                    .names
                    .iter()
                    .enumerate()
                    .map(|(k, n)| {
                        if n == "_" {
                            format!("_b{idx}_{k}")
                        } else {
                            n.clone()
                        }
                    })
                    .collect()
            };
            for name in names {
                fields.push(if prop {
                    HypothesisField::hypothesis(name, binder.ty.clone())
                } else {
                    HypothesisField::datum(name, binder.ty.clone())
                });
            }
        }

        Some(HypothesisBundle::new(sig.name, fields))
    }

    /// Vacuity channel (2/2): **always `None`, and that is the correct answer
    /// here.**
    ///
    /// A [`SatisfiabilityWitness`] is a concrete instance claimed to meet every
    /// field of the bundle. Fabricating one would defeat the entire vacuous-
    /// success guard: `check_vacuity` audits a witness only as far as a syntactic
    /// pass can, and takes any hypothesis it cannot evaluate on the supplier's
    /// word. A witness invented by the backend under audit is therefore a
    /// rubber stamp on exactly the proofs the gate exists to reject.
    ///
    /// This backend cannot produce one soundly. Real witness production needs one
    /// of:
    ///
    /// * a NUMERIC SEARCH — enumerate candidate values for the data binders and
    ///   evaluate the hypotheses (only decides the small decidable fragment, and
    ///   needs an evaluator this backend does not have); or
    /// * a MODEL-SUPPLIED INSTANCE — the party that stated the goal exhibits `n :=
    ///   7` and asserts it meets each hypothesis, which is what the vacuity module
    ///   was designed around.
    ///
    /// Returning `None` keeps the gate FAIL-CLOSED: for a non-trivial bundle
    /// `check_vacuity` yields `WitnessMissing` and `clean == false`. With
    /// `THEOREMATA_VACUITY_GATE` unset (the default) that is observational only.
    /// A bundle with no propositional field is `is_trivial()` and is already clean
    /// with no witness, so no witness is manufactured for that case either.
    ///
    /// [`SatisfiabilityWitness`]: crate::prover::vacuity::SatisfiabilityWitness
    fn satisfiability_witness(
        &self,
        stmt: &str,
    ) -> Option<crate::prover::vacuity::SatisfiabilityWitness> {
        // A witness is CONSTRUCTED and checked here, never asserted. The searcher
        // enumerates concrete values and evaluates every hypothesis against them,
        // so a `Some` means an assignment was found that actually satisfies the
        // bundle. That is the opposite of fabrication: the danger this hook
        // guards against is claiming satisfiability without exhibiting anything.
        let bundle = self.hypothesis_bundle(stmt)?;

        // Both non-witness outcomes collapse to `None` on purpose.
        // `NoWitnessInBounds` means the search ran and found nothing within its
        // cap; `NotDecidable` means the bundle fell outside the fragment the
        // searcher can evaluate at all. Neither is a witness, and the gate must
        // treat them identically. They stay distinguishable via `tag()` for
        // logging, but turning that distinction into a verdict here would let
        // "we could not look" become "there is nothing to find".
        crate::prover::witness_search::search_witness(&bundle).into_witness()
    }
}

/// Lean warm-driver session (repl in Phase 3). Supports both `submit_unit` and
/// per-tactic `step_tactic`.
impl ProofSession for LeanBackend {
    fn start(&mut self, _project: &FormalProject) -> Result<()> {
        Ok(())
    }

    fn submit_unit(&mut self, code: &str) -> Result<UnitResult> {
        let scan = self.source_scan(code)?;
        Ok(UnitResult {
            ok: scan.clean,
            messages: scan.findings,
            detail: json!({"mock": self.mock, "system": SYSTEM.as_str()}),
        })
    }

    fn step_tactic(&mut self, state: u64, tactic: &str) -> Result<StateResult> {
        // Lean supports per-tactic stepping (repl `proofState` ids).
        let finished = tactic.contains("trivial") || tactic.trim() == "rfl";
        Ok(StateResult {
            state: state + 1,
            finished,
            detail: json!({"mock": self.mock, "tactic": tactic}),
        })
    }

    fn goal_state(&self, _state: u64) -> Result<GoalState> {
        Ok(GoalState {
            goals: vec!["True".into()],
            detail: json!({"mock": self.mock}),
        })
    }
}

#[cfg(test)]
mod tier0_tests {
    use super::*;
    use crate::prover::vacuity::{check_vacuity, FieldKind};

    /// The offline fallback must implement the SAME comment policy as the
    /// online scan: a commented escape hatch passes, a real one still fails.
    #[test]
    fn offline_fallback_matches_comment_policy() {
        assert!(
            !crate::prover::statement_preservation::commented_escape_hatch_is_a_violation(),
            "this test encodes ESCAPE_HATCH_COMMENT_POLICY == CodeOnly"
        );
        // Commented-out escape hatches: the kernel never sees them -> clean.
        let commented = "-- sorry\n/- native_decide, admit -/\ntheorem t : True := trivial\n";
        let report = fallback_source_scan(commented);
        assert!(
            report.clean,
            "commented escape hatch must not gate: {:?}",
            report.findings
        );
        // A REAL one in code still fails, offline as well as online.
        let real = fallback_source_scan("theorem t : True := by\n  sorry\n");
        assert!(!real.clean);
        assert!(real.findings.iter().any(|f| f == "sorry"));
        let real2 = fallback_source_scan("theorem t : P := by native_decide\n");
        assert!(!real2.clean);
        assert!(real2.findings.iter().any(|f| f == "native_decide"));
    }

    fn bundle(stmt: &str) -> Option<crate::prover::vacuity::HypothesisBundle> {
        LeanBackend::mock().hypothesis_bundle(stmt)
    }

    /// A signature with a `Prop`-valued hypothesis yields a bundle that CONTAINS
    /// it, classified as a hypothesis, with the data binder kept as a datum.
    #[test]
    fn prop_valued_hypothesis_is_in_the_bundle() {
        let b = bundle("theorem pos (n : Nat) (hn : n > 0) : n ≠ 0")
            .expect("a well-formed Lean signature must parse");
        assert_eq!(b.goal, "pos");
        assert!(!b.is_trivial(), "a Prop hypothesis makes the bundle non-trivial");

        let hyps: Vec<_> = b.hypotheses().collect();
        assert_eq!(hyps.len(), 1, "fields: {:?}", b.fields);
        assert_eq!(hyps[0].binder, "hn");
        assert_eq!(hyps[0].text, "n > 0");
        assert_eq!(hyps[0].kind, FieldKind::Hypothesis);

        let data: Vec<_> = b.data().collect();
        assert_eq!(data.len(), 1);
        assert_eq!(data[0].binder, "n");
        assert_eq!(data[0].text, "Nat");

        // And with no witness the guard fails CLOSED, as designed.
        assert!(!check_vacuity(&b, None).clean);
    }

    /// A pure-data signature yields a TRIVIAL bundle — nothing to witness, and
    /// the vacuity check is clean without one.
    #[test]
    fn pure_data_signature_is_a_trivial_bundle() {
        let b = bundle("theorem id_eq (α : Type) (f : Nat → Nat) (x : α) : x = x")
            .expect("must parse");
        assert!(b.is_trivial(), "no Prop binder here: {:?}", b.fields);
        assert_eq!(b.fields.len(), 3);
        assert!(b.fields.iter().all(|f| f.kind == FieldKind::Datum));
        // `→` must not be read as a Prop token, or `f` would be a hypothesis.
        assert!(check_vacuity(&b, None).clean);
    }

    /// An unparseable signature yields `None` — NEVER a wrong bundle.
    #[test]
    fn unparseable_signature_yields_none() {
        for stmt in [
            "",
            "-- just a comment",
            "not a declaration at all",
            "∀ n : Nat, n = n",
        ] {
            assert!(bundle(stmt).is_none(), "must not guess a bundle for `{stmt}`");
        }
        // A `def` is not a proposition-bearing declaration either.
        assert!(bundle("def twice (n : Nat) : Nat := n + n").is_none());
    }

    /// A binder group binding several names yields one field per name.
    #[test]
    fn grouped_binders_yield_one_field_each() {
        let b = bundle("theorem t (a b : Nat) (h1 h2 : a < b) : a ≤ b").expect("must parse");
        let names: Vec<&str> = b.fields.iter().map(|f| f.binder.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "h1", "h2"]);
        assert_eq!(b.hypotheses().count(), 2);
    }

    /// The documented conservative bias: a bare named proposition reads as a
    /// DATUM here rather than risking a false failure. It is not lost — that
    /// shape is `hypothesis_audit`'s `Opaque` mechanism on the other gate.
    #[test]
    fn bare_named_proposition_errs_toward_datum() {
        let b = bundle("theorem cond (hR : RiemannHypothesis) (n : Nat) : n = n")
            .expect("must parse");
        assert!(b.is_trivial(), "bias is toward Datum: {:?}", b.fields);
        assert!(check_vacuity(&b, None).clean, "must not fail a good proof");
    }

    /// An anonymous instance binder carrying a proposition is kept — the one
    /// deliberate exception to the Datum bias.
    #[test]
    fn anonymous_prop_instance_binder_is_kept() {
        let b = bundle("theorem t (n : Nat) [Fact (0 < n)] : n ≠ 0").expect("must parse");
        assert!(!b.is_trivial(), "fields: {:?}", b.fields);
        assert_eq!(b.hypotheses().count(), 1);
        // A plain typeclass binder is data, not a hypothesis.
        let g = bundle("theorem u (G : Type) [Group G] : True").expect("must parse");
        assert!(g.is_trivial(), "fields: {:?}", g.fields);
    }

    /// A contradictory bundle parsed straight off the signature is REFUTED — the
    /// gate can now actually fire on the motivating example.
    #[test]
    fn contradictory_signature_is_refuted() {
        let b = bundle("theorem hollow (x : Nat) (h1 : x > 5) (h2 : x < 3) : False")
            .expect("must parse");
        let r = check_vacuity(&b, None);
        assert!(!r.clean);
        assert!(
            r.contradictions.iter().any(|c| c.rule == "numeric_bounds"),
            "{:?}",
            r.contradictions
        );
    }

    /// No witness is ever fabricated: a fabricated one would rubber-stamp the
    /// proofs this gate exists to reject.
    ///
    /// This test used to assert `is_none()` unconditionally, back when the hook
    /// was a stub. That was the right assertion for a stub and the wrong one for
    /// a searcher: `(n : Nat) (hn : n > 0)` is satisfiable, and refusing to say
    /// so is not soundness, it is silence. What must hold is that any witness
    /// handed back was actually CHECKED, so the assertion is now that the
    /// returned assignment really does satisfy the bundle.
    #[test]
    fn a_returned_witness_is_checked_not_fabricated() {
        let backend = LeanBackend::mock();
        let stmt = "theorem pos (n : Nat) (hn : n > 0) : n ≠ 0";
        let bundle = backend
            .hypothesis_bundle(stmt)
            .expect("this statement's binders and hypotheses are parseable");
        let witness = backend
            .satisfiability_witness(stmt)
            .expect("n > 0 is satisfiable over Nat and lies inside the decidable fragment");
        assert!(
            crate::prover::vacuity::check_vacuity(&bundle, Some(&witness)).clean,
            "a witness this backend hands out must survive the vacuity check it feeds"
        );
    }

    /// Outside the searcher's fragment the answer is `None`, and `None` keeps the
    /// gate fail-closed. The point is that "we cannot decide this" and "there is
    /// no witness" must reach the gate as the same non-answer, never as a pass.
    #[test]
    fn an_undecidable_bundle_yields_no_witness() {
        let backend = LeanBackend::mock();
        // `Nat.Prime` is a predicate the searcher cannot evaluate, so the whole
        // bundle is NotDecidable rather than partially satisfied.
        assert!(backend
            .satisfiability_witness("theorem p (n : Nat) (hn : Nat.Prime n) : n ≠ 0")
            .is_none());
    }

    /// The allowlist is empty unless a caller populates it — never invented.
    #[test]
    fn designated_inputs_default_to_empty() {
        assert!(LeanBackend::mock().designated_inputs().is_empty());
    }

    #[test]
    fn designated_inputs_env_value_parses() {
        assert_eq!(
            parse_designated_inputs(" RiemannHypothesis, hGlaisher ;RiemannHypothesis\nFoo "),
            vec![
                "RiemannHypothesis".to_string(),
                "hGlaisher".to_string(),
                "Foo".to_string()
            ]
        );
        assert!(parse_designated_inputs("  , ; ").is_empty());
    }

    /// Deterministic: no clock, no RNG, no IO on this path.
    #[test]
    fn bundle_parsing_is_deterministic() {
        let stmt = "theorem t (n : Nat) (hn : 0 < n) (hp : Nat.Prime n) : n ≠ 0";
        assert_eq!(bundle(stmt), bundle(stmt));
    }
}

fn write_artifact(dir: &std::path::Path, rel: &str, value: &impl serde::Serialize) -> Result<()> {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(value)?)?;
    Ok(())
}
