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
        model::{
            FormalProject, ProofJob, ProofResult, ProofTask, ProverJobStatus, VerificationReport,
        },
    },
};
use anyhow::{anyhow, Result};
use chrono::Utc;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    time::Instant,
};

const BACKEND: &str = "lean";
const SYSTEM: FormalSystem = FormalSystem::Lean;
const MODULE: &str = "Generated";

/// `CompileReport::detail` key carrying WHICH LIBRARY THE COMPILE ACTUALLY READ,
/// as opposed to the one the configuration names. See
/// [`LeanBackend::library_resolution`] for why the two can differ.
const LIBRARY_RESOLUTION_KEY: &str = "library_resolution";

/// `CompileReport::detail` key carrying the temp workspace the compile ran in.
/// Published so a later phase of the same verification (the elaborated-statement
/// probe in [`LeanBackend::verify`]) can reach the very files that were compiled
/// instead of scaffolding a second, possibly different, workspace.
const WORKSPACE_ROOT_KEY: &str = "workspace_root";

/// Kill switch for the advisory elaborated-statement probe (see
/// [`LeanBackend::elaborated_statement`]). Set to `0`/`false` to skip the probe.
///
/// It exists because the probe costs one extra `lean` run per SUCCESSFUL
/// verification, and against a real Mathlib that is not free. Skipping it
/// publishes nothing, which `checker_cache` reads as UNAVAILABLE, the correct
/// reading, and the reason no placeholder is ever emitted in its place.
pub const ELABORATION_PROBE_ENV: &str = "THEOREMATA_LEAN_PUBLISH_ELABORATION";

/// Upper bound on a published elaborated form. `pp.all` output is fully explicit
/// and can be enormous (a 200-term arithmetic goal pretty-printed to 600 KB on
/// the machine this was measured on). Past the cap we publish NOTHING rather
/// than a truncation: a truncated form digests to a value that is neither the
/// old statement's nor the new one's, which is exactly the confidently-wrong
/// input a staleness discriminator must never be handed.
const MAX_ELABORATED_FORM_BYTES: usize = 64 * 1024;

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
    /// WHERE [`LeanBackend::designated_inputs`] came from. Recorded so a reader of
    /// the code (and of a log line) can never mistake an operator's assertion for
    /// something this backend worked out for itself. See
    /// [`DesignatedInputsSource`], whose missing `Derived` variant is the whole
    /// point.
    pub designated_inputs_source: DesignatedInputsSource,
}

/// Provenance of a [`LeanBackend::designated_inputs`] allowlist.
///
/// **There is deliberately NO `Derived` variant.** A designated input is a claim
/// that a particular unproved assumption is a legitimate ANTECEDENT of the task
/// rather than smuggled-in mathematics, and this layer has no source it may
/// derive such a claim from:
///
/// * the canonical statement is not a trustworthy source, because in this
///   pipeline it is itself model-authored (`reason::orchestration::agent`'s
///   `formalize` writes it via `set_formal_statement`). Allowlisting whatever
///   binders the statement happens to carry would let the same untrusted producer
///   that writes `(hRH : RiemannHypothesis)` into the statement designate it;
/// * and it would be near-vacuous even if it were trustworthy: a hypothesis
///   binder that is NOT in the canonical statement already fails the (always
///   enforced) statement-preservation layer, so allowlisting every binder that IS
///   in it leaves the audit with nothing left to reject;
/// * `FormalProject::imports` are MODULE imports, i.e. access to *proved*
///   lemmas, which need no allowlist, and not assumed premises;
/// * `Config` carries no trusted-premise field at all.
///
/// So every entry is asserted by a human/task-definer, and the variants below
/// record which one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DesignatedInputsSource {
    /// Nobody declared anything: the allowlist is empty. The audit will therefore
    /// classify every genuine antecedent as `Unaccounted`.
    #[default]
    Unset,
    /// An operator set [`DESIGNATED_INPUTS_ENV`], the process-global escape
    /// hatch. It applies to EVERY statement this backend verifies, so it must be
    /// scoped to a single run whose goals share the same antecedents.
    OperatorEnv,
    /// The party that defined the task populated it programmatically via
    /// [`LeanBackend::with_designated_inputs`], the preferred channel, because it
    /// is scoped to one backend instance rather than to the process.
    TaskDeclared,
}

impl DesignatedInputsSource {
    /// Stable tag for log lines / JSON detail.
    pub fn tag(self) -> &'static str {
        match self {
            DesignatedInputsSource::Unset => "unset",
            DesignatedInputsSource::OperatorEnv => "operator_env",
            DesignatedInputsSource::TaskDeclared => "task_declared",
        }
    }
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
///
/// **It is process-global, so its entries are designated for EVERY statement any
/// live backend verifies.** Scope it to a single run whose goals share the same
/// antecedents, or prefer [`LeanBackend::with_designated_inputs`], which is scoped
/// to one backend instance. The preferred long-term home remains a task-level
/// field authored by whoever defined the task. See
/// [`FormalBackend::designated_inputs`] as implemented below for exactly what
/// would need to be threaded in.
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
            designated_inputs_source: DesignatedInputsSource::Unset,
        }
    }

    /// The live backend, reading the configured runner + binary (env-overridable).
    pub fn live(cfg: &Config) -> Self {
        // The env var is the OPERATOR ESCAPE HATCH and the only populated source
        // that exists today. An entry that parses to nothing (`""`, `" , ; "`)
        // leaves the allowlist empty, and an empty allowlist is `Unset` however it
        // arose: provenance describes what the audit will actually see, so an
        // operator who set the var to a blank value cannot read `OperatorEnv` and
        // believe the channel is populated.
        let designated_inputs = std::env::var(DESIGNATED_INPUTS_ENV)
            .ok()
            .map(|raw| parse_designated_inputs(&raw))
            .unwrap_or_default();
        let designated_inputs_source = if designated_inputs.is_empty() {
            DesignatedInputsSource::Unset
        } else {
            DesignatedInputsSource::OperatorEnv
        };
        Self {
            mock: false,
            runner: cfg.formal_runners.for_system(SYSTEM),
            lean: exec::env_or("THEOREMATA_LEAN", &cfg.lean_bin),
            lake: exec::env_or("THEOREMATA_LAKE", "lake"),
            toolchain: std::env::var("THEOREMATA_LEAN_TOOLCHAIN")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            kernel_validate: cfg.kernel_validate_proof,
            designated_inputs,
            designated_inputs_source,
        }
    }

    /// Declare the task's DESIGNATED INPUTS on this backend instance: the
    /// non-global way to populate the layer-2d channel.
    ///
    /// Each entry names a hypothesis the caller asserts is a legitimate antecedent
    /// of the goals this backend will verify, by BINDER name (`hRH`) or by TYPE
    /// HEAD (`RiemannHypothesis`). Only the party that defined the task may call
    /// this, because only that party can tell a genuine "assuming RH, …" result
    /// from a proof dodging its own obligations.
    ///
    /// Prefer this over [`DESIGNATED_INPUTS_ENV`]: the env var is process-global
    /// and so designates its entries for every statement any live backend touches,
    /// while this is scoped to one instance. Entries are deduplicated and trimmed
    /// exactly as the env seam does; an all-empty input leaves the channel `Unset`
    /// rather than claiming a populated one.
    pub fn with_designated_inputs<I, S>(mut self, entries: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let joined = entries
            .into_iter()
            .map(|e| e.as_ref().to_string())
            .collect::<Vec<_>>()
            .join(",");
        self.designated_inputs = parse_designated_inputs(&joined);
        self.designated_inputs_source = if self.designated_inputs.is_empty() {
            DesignatedInputsSource::Unset
        } else {
            DesignatedInputsSource::TaskDeclared
        };
        self
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

    /// WHAT THE COMPILE ACTUALLY RESOLVED ITS IMPORTS AGAINST, measured rather
    /// than assumed.
    ///
    /// # Why this is not the same as the configured project
    ///
    /// [`FormalBackend::compile`] runs bare `lean Generated.lean` in a fresh
    /// per-verification temp workspace
    /// (`formal::live_workspace_dir`), NOT `lake env lean` inside
    /// `Config::lean_project`. A bare `lean` builds its search path from exactly
    /// two sources: the toolchain's own `--print-libdir`, and the ambient
    /// `LEAN_PATH`. The configured Lake project contributes NOTHING unless
    /// something outside this process already put it on `LEAN_PATH`.
    ///
    /// That was verified directly against the installed toolchain (lean 4.32.0):
    /// with two directories each holding a different `Foo.olean`, the identical
    /// source `import Foo` elaborated against whichever one `LEAN_PATH` named, and
    /// with `LEAN_PATH` unset the import failed with the search path listed as the
    /// libdir alone. So the environment fingerprint, which pins the CONFIGURED
    /// Lake project, can be pinning a library the compile never read.
    ///
    /// # What is captured
    ///
    /// `lean --deps FILE` prints the ABSOLUTE resolved `.olean` path of every
    /// header import without elaborating anything, so it is both cheap and
    /// authoritative: it is the resolver's own answer, obtained through the same
    /// [`Runner`] as the compile, so it also holds under a container runner where
    /// this process's own `LEAN_PATH` would be irrelevant. From those paths the
    /// library ROOTS are recovered by stripping each module's own path tail.
    ///
    /// This is a DETAIL FIELD only. It changes no verdict, and a failed probe
    /// records `provenance: "unavailable"` with the reason rather than a guess.
    fn library_resolution(&self, ws: &Workspace, code: &str) -> Value {
        let file = format!("{MODULE}.lean");
        let deps = exec::run(&self.runner, &[&self.lean, "--deps", &file], &ws.root);
        if !deps.success() {
            return json!({
                "provenance": "unavailable",
                "runner": self.runner.tag(),
                "reason": if deps.launched {
                    "lean --deps exited non-zero (unresolvable import, or a malformed header)"
                } else {
                    "lean --deps could not be launched"
                },
                "stderr": deps.stderr,
            });
        }
        let resolved = parse_dep_paths(&deps.stdout);
        let modules = parse_import_modules(code);
        let roots = library_roots(&resolved, &modules);
        // The toolchain's built-in library directory, asked of the same binary
        // through the same runner. It is always one of the roots; naming it
        // separately lets a consumer subtract it and see what came from
        // elsewhere, which is the part `LEAN_PATH` controls.
        let libdir = exec::run(&self.runner, &[&self.lean, "--print-libdir"], &ws.root);
        json!({
            "provenance": "lean.deps",
            "runner": self.runner.tag(),
            "imports": modules,
            "resolved_imports": resolved,
            "library_roots": roots,
            "toolchain_libdir": libdir.success().then(|| libdir.stdout.trim().to_string()),
            "note": "roots the bare `lean` compile actually read; the configured lake \
                     project contributes only via an ambient LEAN_PATH",
        })
    }

    /// THE CHECKER'S OWN ELABORATED FORM of the accepted statement, published for
    /// [`checker_cache`](crate::checker_cache)'s
    /// `ELABORATED_STATEMENT_DETAIL_KEY`. Advisory provenance; never a verdict.
    ///
    /// # What is actually obtainable, and what this is
    ///
    /// Determined against the installed toolchain (lean 4.32.0) rather than
    /// assumed. `lean --json FILE` reports every message as one JSON object per
    /// line, so a `#check @thm` appended to the already-compiled source yields the
    /// elaborated type of the declaration the kernel accepted, cleanly delimited
    /// and attributable by source position. Under
    ///
    /// ```text
    /// set_option pp.deepTerms true in
    /// set_option pp.maxSteps 10000000 in
    /// set_option pp.all true in
    /// ```
    ///
    /// what comes back is FULLY EXPLICIT: universe levels, implicit arguments and
    /// instance arguments are all printed (`@HAdd.hAdd.{0,0,0} Nat Nat Nat
    /// (@instHAdd.{0} Nat instAddNat) n m`). That matters for the motivating case:
    /// when a library re-routes `x ^ (1/3)` through `Real.rpow`, the SURFACE
    /// notation is unchanged and default pretty-printing would digest identically,
    /// while the explicit form's head constant and instance change. `pp.deepTerms`
    /// and a large `pp.maxSteps` are set so the printer cannot elide a subterm to
    /// `⋯`, which would make two different types print the same.
    ///
    /// **It is a PRETTY-PRINTED type, not a kernel term hash**, and the provenance
    /// string says exactly that (`lean.check.pp_all`). It is sensitive to the
    /// pretty-printer, so a toolchain upgrade can move it without the mathematics
    /// moving; it is a hint about WHY a cached green went stale, never evidence
    /// that one did. A kernel-level identity would need a REPL or a plugin that
    /// hands back the `Expr`/term hash; no such build is wired here, and inventing
    /// a stronger-sounding label for a weaker artifact is the one thing that would
    /// make this field harmful.
    ///
    /// Returns `None`, i.e. publish NOTHING, on every doubt: no declaration name, a
    /// probe that fails to elaborate, output past [`MAX_ELABORATED_FORM_BYTES`], or
    /// the [`ELABORATION_PROBE_ENV`] kill switch. `checker_cache` reads an absent
    /// field as UNAVAILABLE, which is the honest reading of all four.
    fn elaborated_statement(&self, root: &Path, decl: &str) -> Option<Value> {
        if decl.trim().is_empty() {
            return None;
        }
        let base = std::fs::read_to_string(root.join(format!("{MODULE}.lean"))).ok()?;
        // Shared with the staleness re-elaboration path
        // ([`reelaborate_pinned_statement`]) so the two forms being compared can
        // never have been produced by two different mechanisms. See
        // [`append_elaboration_probe`].
        let (content, first_appended_line) = append_elaboration_probe(&base, decl);
        let probe_file = format!("{MODULE}_elab.lean");
        std::fs::write(root.join(&probe_file), content).ok()?;
        let out = exec::run(&self.runner, &[&self.lean, "--json", &probe_file], root);
        if !out.success() {
            return None;
        }
        let form = probe_outcome_from_json(&out.stdout, first_appended_line).into_form()?;
        if form.len() > MAX_ELABORATED_FORM_BYTES {
            return None;
        }
        Some(json!({
            "provenance": ELABORATED_PROVENANCE,
            "form": form,
            "declaration": decl,
            "options": ["pp.all", "pp.deepTerms", "pp.maxSteps"],
        }))
    }
}

/// Provenance label for the published elaborated form. It names the MECHANISM
/// (`#check` under `pp.all`), so no consumer can read it as a kernel-level term
/// identity, which it is not. See [`LeanBackend::elaborated_statement`].
const ELABORATED_PROVENANCE: &str = "lean.check.pp_all";

/// The lines appended to a compiled source to print its elaborated type.
///
/// `pp.deepTerms`/`pp.maxSteps` suppress the `⋯` elision the printer applies to
/// deep or long terms: an elided form is a form two DIFFERENT types can share, so
/// leaving elision on would make the discriminator quietly blind. Both options
/// were confirmed to exist and be accepted by lean 4.32.0.
fn elaboration_probe_block(decl: &str) -> String {
    format!(
        "set_option pp.deepTerms true in\n\
         set_option pp.maxSteps 10000000 in\n\
         set_option pp.all true in\n\
         #check @{decl}\n"
    )
}

/// Whether the advisory elaborated-statement probe should run. Default ON; the
/// [`ELABORATION_PROBE_ENV`] kill switch turns it off for runs that will not pay
/// one extra `lean` invocation per success.
fn elaboration_probe_enabled() -> bool {
    match std::env::var(ELABORATION_PROBE_ENV) {
        Ok(v) => {
            let v = v.trim();
            !(v == "0" || v.eq_ignore_ascii_case("false") || v.eq_ignore_ascii_case("off"))
        }
        Err(_) => true,
    }
}

/// Append the probe block to a source and report the 1-based line the appended
/// block starts on.
///
/// THE SINGLE PLACE a probe is assembled. Both the pin path
/// ([`LeanBackend::elaborated_statement`]) and the staleness re-elaboration path
/// ([`reelaborate_pinned_statement`]) go through it, because a comparison between
/// a form produced by one mechanism and a form produced by another mechanism is
/// meaningless: it would report pretty-printer differences as moved mathematics.
/// Factoring it here means the two cannot drift without a compile error.
fn append_elaboration_probe(base: &str, decl: &str) -> (String, usize) {
    let mut content = base.to_string();
    if !content.ends_with('\n') {
        content.push('\n');
    }
    // 1-based line of the first line we append, so a `#check` that was already in
    // the submitted source cannot be mistaken for ours.
    let first_appended_line = content.lines().count() + 1;
    content.push_str(&elaboration_probe_block(decl));
    (content, first_appended_line)
}

/// What one probe run said. Three states, not two: "the elaborator ran and
/// refused" and "we got no answer" are the distinction the whole staleness
/// discriminator rests on, so the parser must not collapse them into `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ProbeOutcome {
    /// `#check` printed a type.
    Form(String),
    /// The elaborator reported an error. Carries its text verbatim; the CALLER
    /// decides whether that text is evidence of moved mathematics or merely of a
    /// context that does not provide what the form names.
    Failed(String),
    /// The run produced neither. No answer is not a negative answer.
    NoAnswer,
}

impl ProbeOutcome {
    /// The form, discarding WHY there is none. Used by the pin path, which
    /// publishes nothing on every doubt and so needs no reason.
    fn into_form(self) -> Option<String> {
        match self {
            ProbeOutcome::Form(f) => Some(f),
            ProbeOutcome::Failed(_) | ProbeOutcome::NoAnswer => None,
        }
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
            Some(t) => (split_lean_idents(name_part), t.iter().collect::<String>()),
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
/// names). `_` is preserved here -- the caller renames it, because a bundle field
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
    while i < chars.len()
        && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '\'' || chars[i] == '.')
    {
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
    "=", "≠", "<", ">", "≤", "≥", "∈", "∉", "⊆", "⊂", "⊇", "∣", "∤", "∧", "∨", "↔", "¬", "≡", "≅",
    "∃", "≫", "≪",
];

/// Type heads that are `Prop`-valued in Lean core / the logical prelude.
///
/// **This is a list of LOGICAL CONNECTIVES AND ORDER/ALGEBRA CLASS PROJECTIONS,
/// not of mathematical facts.** Nothing domain-specific belongs here: a
/// domain-specific opaque assumption (`RiemannHypothesis`) is the
/// [`crate::prover::hypothesis_audit`] layer's business, allowlisted via
/// [`DESIGNATED_INPUTS_ENV`], not something this bundle parser should guess at.
const PROP_HEADS: &[&str] = &[
    "Eq",
    "Ne",
    "Not",
    "And",
    "Or",
    "Iff",
    "Xor",
    "True",
    "False",
    "Exists",
    "LT.lt",
    "LE.le",
    "GT.gt",
    "GE.ge",
    "Dvd.dvd",
    "Membership.mem",
    "Nonempty",
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

// ===========================================================================
// Elaborated-statement publication + resolved-library capture
// ===========================================================================

/// The module names a Lean file's header imports, in source order, plus `Init`,
/// which every file imports implicitly and which therefore always shows up in
/// `lean --deps` output.
///
/// Header-only by construction: scanning stops at the first line that is neither
/// blank, a comment, nor an `import`, because Lean's own header ends there. A
/// later `import` inside a string literal or a doc comment is thus never picked
/// up as a module.
fn parse_import_modules(code: &str) -> Vec<String> {
    let mut out: Vec<String> = vec!["Init".to_string()];
    for raw in code.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("--") {
            continue;
        }
        let Some(rest) = line.strip_prefix("import ") else {
            // `prelude` and `set_option` may legally precede imports; anything
            // else ends the header.
            if line == "prelude" || line.starts_with("set_option ") {
                continue;
            }
            break;
        };
        // `import all Foo` / `import Foo`: take the last whitespace-separated
        // token, which is the module name in either spelling.
        if let Some(module) = rest.split_whitespace().last() {
            if !module.is_empty() && !out.iter().any(|m| m == module) {
                out.push(module.to_string());
            }
        }
    }
    out
}

/// Normalize a path for suffix comparison: `\` to `/`, and lowercased.
///
/// Lean emits MIXED separators on Windows (a search-path entry keeps the
/// separator style it was given, and only the final component is appended with
/// the native one), so a naive `ends_with` on the raw text misses. Lowercasing is
/// for the same platform: the search-path entry's drive letter and directory case
/// come from the environment, not from the module name.
fn normalize_path_for_match(p: &str) -> String {
    p.replace('\\', "/").to_ascii_lowercase()
}

/// Given a resolved `.olean` path and the module name it resolves, recover the
/// SEARCH-PATH ROOT it was found under, i.e. the library root: strip the
/// `A/B/C.olean` tail that the module name `A.B.C` dictates.
///
/// `None` when the path does not end in that tail, which means this path did not
/// come from this module. Recovering the root rather than guessing it is the
/// point: the root is the thing a fingerprint can compare against the project it
/// believes it pinned.
fn olean_root_for(path: &str, module: &str) -> Option<String> {
    let tail = format!("/{}.olean", module.replace('.', "/"));
    let hay = normalize_path_for_match(path);
    let needle = normalize_path_for_match(&tail);
    let cut = hay.len().checked_sub(needle.len())?;
    if !hay.ends_with(&needle) {
        return None;
    }
    // Cut the ORIGINAL string at the same byte offset: the normalized form is
    // byte-for-byte length-preserving (separator swap and ASCII lowercasing both
    // are), so the offset transfers, and the caller sees the real on-disk casing.
    Some(path[..cut].to_string())
}

/// The distinct library roots behind a set of resolved `.olean` paths.
///
/// Sorted and deduplicated so the value is stable run to run (an unstable detail
/// field would show up as spurious environment churn). A path that matches no
/// known module contributes no root; it is still reported verbatim under
/// `resolved_imports`, so nothing is silently dropped.
fn library_roots(deps: &[String], modules: &[String]) -> Vec<String> {
    let mut roots: Vec<String> = Vec::new();
    for dep in deps {
        for module in modules {
            if let Some(root) = olean_root_for(dep, module) {
                if !roots.iter().any(|r| r == &root) {
                    roots.push(root);
                }
                break;
            }
        }
    }
    roots.sort();
    roots
}

/// Split `lean --deps` stdout into resolved paths, one per line.
fn parse_dep_paths(stdout: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in stdout.lines() {
        let p = line.trim();
        // `--deps` repeats a path when two imports resolve to the same file;
        // dedup here so the detail lists each resolved artifact once.
        if !p.is_empty() && !out.iter().any(|e| e == p) {
            out.push(p.to_string());
        }
    }
    out
}

/// The text Lean's `#check` prints for a declaration is `name : type`. Return the
/// TYPE half.
///
/// The name is dropped ON PURPOSE. A published elaborated form exists to tell
/// "the script needs a rename" apart from "the mathematics moved"; if the
/// declaration's own name were part of the digested form, every rename would move
/// the digest and the two cases would be indistinguishable again. The separator
/// is the first `" : "`, which is unambiguous because the printed name (possibly
/// with universe parameters, `foo.{u_1}`) contains no whitespace.
fn strip_check_prefix(printed: &str) -> Option<String> {
    let (name, ty) = printed.split_once(" : ")?;
    if name.trim().is_empty() || name.contains(char::is_whitespace) {
        return None;
    }
    let ty = ty.trim();
    if ty.is_empty() {
        None
    } else {
        Some(ty.to_string())
    }
}

/// Pull the elaborated type out of `lean --json` output for a probe whose
/// appended block starts at 1-based line `first_appended_line`.
///
/// Fails closed to `None` on ANY error message in the run: an error means the
/// probe file did not elaborate, so whatever else it printed says nothing about
/// the accepted statement.
fn elaborated_form_from_json(stdout: &str, first_appended_line: usize) -> Option<String> {
    probe_outcome_from_json(stdout, first_appended_line).into_form()
}

/// The parser both probe paths share. Fails closed on ANY error message in the
/// run: an error means the probe file did not elaborate, so whatever else it
/// printed says nothing about the accepted statement. The error TEXT is kept
/// rather than discarded, because the staleness discriminator has to read it to
/// tell "this term is ill-typed here" (moved mathematics) from "this context does
/// not provide what the term names" (no answer at all).
fn probe_outcome_from_json(stdout: &str, first_appended_line: usize) -> ProbeOutcome {
    let mut found: Option<String> = None;
    for line in stdout.lines() {
        let Ok(msg) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let severity = msg.get("severity").and_then(Value::as_str).unwrap_or("");
        if severity == "error" {
            return ProbeOutcome::Failed(
                msg.get("data")
                    .and_then(Value::as_str)
                    .unwrap_or("lean reported an error with no message body")
                    .to_string(),
            );
        }
        if severity != "information" {
            continue;
        }
        let pos = msg
            .get("pos")
            .and_then(|p| p.get("line"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        // Only messages from the block WE appended may be read. A `#check`
        // already present in the submitted source prints an information message
        // too, and attributing that to the statement would be a lie.
        if pos < first_appended_line {
            continue;
        }
        if let Some(ty) = msg
            .get("data")
            .and_then(Value::as_str)
            .and_then(strip_check_prefix)
        {
            found = Some(ty);
        }
    }
    match found {
        Some(form) => ProbeOutcome::Form(form),
        None => ProbeOutcome::NoAnswer,
    }
}

// ===========================================================================
// Phase 1.2: re-elaborating a PINNED statement under the CURRENT environment
// ===========================================================================

/// Import header used when neither the stored record nor the operator supplies
/// one. `import Mathlib` is the maximal Mathlib context, so it is a SUPERSET of
/// whatever a Mathlib-derived statement was originally elaborated under, which is
/// the safe direction: a too-narrow context would make a perfectly good statement
/// fail to resolve, and this file must never read that as moved mathematics.
pub const DEFAULT_REELABORATION_PREAMBLE: &str = "import Mathlib";

/// Name the re-elaborated statement is introduced under. The name is irrelevant
/// to the result: [`strip_check_prefix`] drops it before anything is compared, on
/// exactly the same grounds as on the pin side.
const REELABORATION_DECL: &str = "theoremataReelaboratedStatement";

/// Name of the control declaration. See [`reelaborate_pinned_statement`] step 1.
const REELABORATION_CONTROL_DECL: &str = "theoremataReelaborationControl";

/// Options turned off around the re-elaborated declaration.
///
/// `autoImplicit` is the load-bearing one and was confirmed live on lean 4.32.0.
/// With it ON, a constant the current environment no longer provides is silently
/// auto-bound as a fresh implicit variable, and the probe then fails with a
/// downstream complaint ("function expected at f") that is indistinguishable from
/// a genuine type error. With it OFF the same case reports `Unknown identifier`,
/// which is precisely the marker [`names_are_unresolved`] needs to route the node
/// to "no answer" instead of to a withdrawal.
const AUTO_IMPLICIT_OFF: &str =
    "set_option autoImplicit false\nset_option relaxedAutoImplicit false\n";

/// What re-elaborating a pinned statement produced.
///
/// Deliberately the same three-way shape as the reason layer's
/// `staleness::ReelaborationOutcome`, and deliberately NOT that type: this file
/// is a prover backend and should not depend on the reason layer. The caller
/// (`reason::proving::staleness_sweep`) does the one-to-one conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeanReelaboration {
    /// The statement elaborated, to this form. Produced by the SAME mechanism
    /// that produced the pin, so the two are comparable by exact string equality.
    Elaborated { form: String },
    /// The elaborator ran, in a context proved healthy by the control probe, and
    /// refused the statement as ILL-TYPED. A checked negative.
    Rejected { detail: String },
    /// We got no usable answer. Never evidence of anything.
    Unavailable { reason: String },
}

/// Lean error markers meaning THE CONTEXT DOES NOT PROVIDE A NAME the pinned form
/// uses.
///
/// This is the "unrelated reason" case the plan warns about: a missing import, a
/// narrower preamble than the pin was elaborated under, or a constant that was
/// deleted. Those are not distinguishable from each other from here, and none of
/// them is evidence that the mathematics moved, so all of them degrade to
/// `Unavailable` and therefore to `Unknown`. Withdrawing a green on a missing
/// import would be the most damaging error this component can make.
const UNRESOLVED_NAME_MARKERS: [&str; 4] = [
    "unknown identifier",
    "unknown constant",
    "unknown namespace",
    "unknown package",
];

/// Lean error markers meaning THE TERM IS ILL-TYPED IN A CONTEXT THAT RESOLVED.
///
/// An ALLOWLIST, not a denylist, and that direction is deliberate: only an error
/// we positively recognize as ill-typedness may produce the loud
/// `MathematicsMoved` verdict. Every unrecognized error falls through to
/// `Unavailable`, so a new or reworded Lean diagnostic costs us an `Unknown`
/// (recoverable) rather than a wrongful withdrawal (not).
const ILL_TYPED_MARKERS: [&str; 4] = [
    "type mismatch",
    "function expected",
    "has type",
    "is not definitionally equal",
];

/// True when a Lean error is about a name the context does not provide.
fn names_are_unresolved(detail: &str) -> bool {
    let lowered = detail.to_lowercase();
    UNRESOLVED_NAME_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker))
}

/// True when a Lean error is a recognized ill-typedness AND is not about an
/// unresolved name. Order matters: an "unknown identifier" message can also carry
/// a `has type` clause further down, and the unresolved reading must win.
fn is_ill_typed(detail: &str) -> bool {
    if names_are_unresolved(detail) {
        return false;
    }
    let lowered = detail.to_lowercase();
    ILL_TYPED_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker))
}

/// The probe source for one re-elaboration: a preamble, then the pinned form
/// introduced as a declaration, then the SHARED probe block.
///
/// `axiom` is used to introduce the type because we want the type ELABORATED and
/// nothing else; a `theorem` would need a proof and a `def` would need a value,
/// and both would make the probe fail for reasons that have nothing to do with
/// the statement. The declaration exists only inside a throwaway file that is
/// never imported, never compiled into a workspace that backs a verdict, and
/// never consulted for anything but the string `#check` prints, so it introduces
/// no axiom anywhere a proof could reach it.
fn reelaboration_probe_source(preamble: &str, pinned_form: &str) -> String {
    format!("{preamble}\n{AUTO_IMPLICIT_OFF}\naxiom {REELABORATION_DECL} :\n{pinned_form}\n")
}

/// The control probe source: the same preamble and the same options, carrying a
/// declaration whose type is `True` and therefore cannot depend on the library.
fn reelaboration_control_source(preamble: &str) -> String {
    format!("{preamble}\n{AUTO_IMPLICIT_OFF}\naxiom {REELABORATION_CONTROL_DECL} : True\n")
}

/// Run one probe file under the same runner and the same `lean --json`
/// invocation the pin path uses.
fn run_probe(
    runner: &Runner,
    lean: &str,
    root: &Path,
    file_name: &str,
    body: &str,
    decl: &str,
) -> ProbeOutcome {
    let (content, first_appended_line) = append_elaboration_probe(body, decl);
    if std::fs::write(root.join(file_name), content).is_err() {
        return ProbeOutcome::NoAnswer;
    }
    let out = exec::run(runner, &[lean, "--json", file_name], root);
    if !out.launched {
        return ProbeOutcome::NoAnswer;
    }
    probe_outcome_from_json(&out.stdout, first_appended_line)
}

/// Re-elaborate a PINNED elaborated statement form under the CURRENT environment
/// (plan Phase 1.2).
///
/// # Why the pinned FORM is the input, and not the original source text
///
/// The pin published by [`LeanBackend::elaborated_statement`] is `pp.all` output:
/// fully explicit, with every universe level, implicit argument and instance
/// argument named. That makes it re-parseable Lean, and it makes it SENSITIVE in
/// exactly the way the discriminator needs. Verified live on lean 4.32.0 against
/// the built Mathlib: an `abbrev T := Nat` changed to `abbrev T := Int` keeps the
/// surface statement byte-identical and keeps the DEFAULT pretty-printing
/// identical, while the `pp.all` form names `instAddNat` explicitly and so fails
/// to re-elaborate. Surface text would have said "nothing changed".
///
/// # The three answers, and how they are kept apart
///
/// 1. **Control probe first.** A declaration of type `True` under the same
///    preamble and the same options. If that does not elaborate, the workspace,
///    the toolchain or the preamble is broken, and NOTHING we learn from the
///    statement probe means anything. Returns `Unavailable`.
/// 2. **Statement probe.** With the control green:
///    - it printed a form -> `Elaborated`. The caller compares strings; equal is a
///      repair task, different is a withdrawal.
///    - it errored with an ill-typedness we recognize ([`ILL_TYPED_MARKERS`]) ->
///      `Rejected`. This is the only path to a withdrawal-by-non-elaboration.
///    - it errored about a name the context does not provide
///      ([`UNRESOLVED_NAME_MARKERS`]) -> `Unavailable`. A missing import and a
///      deleted constant are indistinguishable from here, and the plan's own
///      honest risk (a statement whose local definitions also moved) lands here.
///    - anything else, including no output at all -> `Unavailable`.
///
/// Every early return in this function is `Unavailable`. There is no path from a
/// failure of any kind to `Elaborated`, so the caller can never be handed
/// something that assesses to `Fresh` because we did not manage to look.
///
/// # Cost is the caller's to bound, not this function's
///
/// Two Lean spawns per call is genuinely expensive, and this function used to
/// guard that with an env opt-in of its own. That was the wrong place: a
/// backend primitive that refuses to run unless an env var is set is a primitive
/// nothing calls. The bounding belongs where the fan-out is known, which is the
/// sweep: it skips every node whose answer could not change the verdict, it
/// remembers an answer per (pinned form, preamble, resolved environment), and it
/// caps how many nodes may spawn Lean in one run. See
/// `reason::proving::staleness_sweep`.
pub fn reelaborate_pinned_statement(
    cfg: &Config,
    preamble: &str,
    pinned_form: &str,
) -> LeanReelaboration {
    let unavailable = |reason: String| LeanReelaboration::Unavailable { reason };

    if pinned_form.trim().is_empty() {
        return unavailable("pinned elaborated statement form is empty".to_string());
    }
    if preamble.trim().is_empty() {
        return unavailable(
            "no import preamble is available to re-elaborate the pinned form against".to_string(),
        );
    }
    if cfg.prover_mock {
        // A mock consults no library, so it cannot say anything about whether the
        // library moved. Silence, not a verdict.
        return unavailable(
            "prover is pinned to mock, which elaborates against no library".to_string(),
        );
    }

    let backend = LeanBackend::live(cfg);
    if !backend.available() {
        return unavailable(format!(
            "lean toolchain unavailable through runner {}",
            backend.runner.tag()
        ));
    }
    let root = match crate::prover::formal::live_workspace_dir(cfg, SYSTEM) {
        Ok(root) => root,
        Err(err) => {
            return unavailable(format!(
                "could not scaffold a re-elaboration workspace: {err}"
            ))
        }
    };

    // 1. Control probe. Proves the preamble, the runner and the toolchain can
    //    elaborate ANYTHING here, so a later failure is about the statement.
    let control = run_probe(
        &backend.runner,
        &backend.lean,
        &root,
        "ReelaborationControl.lean",
        &reelaboration_control_source(preamble),
        REELABORATION_CONTROL_DECL,
    );
    let outcome = match control {
        ProbeOutcome::Form(_) => {
            // 2. Statement probe, in a context now known to be healthy.
            let probe = run_probe(
                &backend.runner,
                &backend.lean,
                &root,
                "Reelaboration.lean",
                &reelaboration_probe_source(preamble, pinned_form),
                REELABORATION_DECL,
            );
            match probe {
                ProbeOutcome::Form(form) => LeanReelaboration::Elaborated { form },
                ProbeOutcome::Failed(detail) if is_ill_typed(&detail) => {
                    LeanReelaboration::Rejected { detail }
                }
                ProbeOutcome::Failed(detail) if names_are_unresolved(&detail) => {
                    unavailable(format!(
                        "the pinned form names something this context does not provide, which a \
                         missing import and a deleted constant share, so this is not evidence \
                         that the mathematics moved: {detail}"
                    ))
                }
                ProbeOutcome::Failed(detail) => unavailable(format!(
                    "lean refused the pinned form with an error this build does not recognize as \
                     ill-typedness, so it is not read as moved mathematics: {detail}"
                )),
                ProbeOutcome::NoAnswer => unavailable(
                    "the statement probe printed neither a type nor an error".to_string(),
                ),
            }
        }
        ProbeOutcome::Failed(detail) => unavailable(format!(
            "control probe failed, so the re-elaboration context itself is unusable and nothing \
             about the statement can be concluded: {detail}"
        )),
        ProbeOutcome::NoAnswer => unavailable(
            "control probe produced no output, so the re-elaboration context could not be \
             confirmed usable"
                .to_string(),
        ),
    };

    // Best effort: a sweep can create one of these per node, and leaving them all
    // behind is how a census fills a disk. A failure to clean up is not a reason
    // to change the verdict.
    let _ = std::fs::remove_dir_all(&root);
    outcome
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
    // The token list is the SHARED, ALIAS-EXPANDED table in `formal.rs`
    // ([`crate::prover::formal::escape_hatch_tokens`]), matched on word
    // boundaries. It lives there rather than here because a per-file list is how
    // `native_decide` ended up banned in one place and its exact alias
    // `decide +native` banned in none: one table means one edit covers every
    // backend, and it mirrors the Python worker's rules so offline and online
    // reject the same set.
    let findings = crate::prover::formal::escape_hatch_findings(SYSTEM, code);
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
                // WHERE the compile ran and WHAT LIBRARY it reached. The second is
                // not implied by the first, nor by `Config::lean_project`: see
                // `LeanBackend::library_resolution`.
                WORKSPACE_ROOT_KEY: ws.root.display().to_string(),
                LIBRARY_RESOLUTION_KEY: self.library_resolution(ws, &code),
            }),
        })
    }

    /// [`FormalBackend::verify`]'s default behaviour, plus the ADVISORY
    /// elaborated-statement publication.
    ///
    /// The gate itself is untouched: `verify_with_gates` produces the report and
    /// every verdict field is passed through byte-for-byte. All this override does
    /// is add one PROVENANCE key to `detail`, and only when the gate already said
    /// yes. Restricting the probe to an accepted, live report is deliberate on
    /// three counts: a failed compile has no accepted statement to elaborate, a
    /// mock never touched a library, and the probe costs one extra `lean` run that
    /// should not be spent on a proof that was rejected anyway.
    ///
    /// If anything about the probe is doubtful the key is simply absent, which
    /// `checker_cache` reads as UNAVAILABLE. Nothing here can turn a green red or
    /// a red green.
    fn verify(&self, cfg: &Config, code: &str, stmt: &str) -> Result<VerificationReport> {
        let mut report = self.verify_with_gates(
            cfg,
            code,
            stmt,
            crate::prover::formal::TierZeroGates::from_config(cfg),
        )?;
        if self.mock || !report.live || !report.lexically_verified || !elaboration_probe_enabled() {
            return Ok(report);
        }
        // The workspace `verify_with_gates` scaffolded is not returned to us, so
        // it is read back out of the compile detail this backend just wrote. That
        // keeps the probe on the EXACT files the kernel accepted; re-scaffolding
        // would elaborate a second copy, and a second copy is a second chance to
        // differ from what was verified.
        let Some(root) = report
            .detail
            .get("compile")
            .and_then(|c| c.get("detail"))
            .and_then(|d| d.get(WORKSPACE_ROOT_KEY))
            .and_then(Value::as_str)
            .map(PathBuf::from)
        else {
            return Ok(report);
        };
        let decl = crate::prover::formal::entry_name(SYSTEM, code)
            .unwrap_or_else(|| crate::prover::formal::theorem_name_hint(stmt));
        if let Some(elaborated) = self.elaborated_statement(&root, &decl) {
            if let Some(detail) = report.detail.as_object_mut() {
                detail.insert(
                    crate::checker_cache::ELABORATED_STATEMENT_DETAIL_KEY.to_string(),
                    elaborated,
                );
            }
        }
        Ok(report)
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

    /// Tier-0 layer 2d channel. See [`LeanBackend::designated_inputs`] (the
    /// field), [`DesignatedInputsSource`], [`LeanBackend::with_designated_inputs`]
    /// and [`DESIGNATED_INPUTS_ENV`].
    ///
    /// Sourced ONLY from the caller-populated field: `live()` fills it from the
    /// operator env var, `with_designated_inputs` from the task definer, `mock()`
    /// leaves it empty. **Nothing here is derived, and the following is why**:
    /// this is the investigated answer, not an unexamined default.
    ///
    /// # Why the list cannot be derived at this layer
    ///
    /// 1. **The signature forbids it.** `designated_inputs(&self)` receives no
    ///    statement. One `LeanBackend` verifies every goal of a run
    ///    (`formal::backend_for` builds it from `Config` alone), so anything
    ///    returned here is designated for ALL of them. A per-statement answer
    ///    cannot even be expressed without changing the trait.
    /// 2. **The obvious source is untrusted.** The tempting derivation is "take
    ///    the hypothesis binders of the canonical statement". But in this pipeline
    ///    the canonical statement is model-authored (`reason::orchestration::agent`
    ///    formalizes it and stores it with `set_formal_statement`), so that rule
    ///    lets the same producer that writes `(hRH : RiemannHypothesis)` into the
    ///    statement designate it. That is precisely the OVER-BROAD direction: it
    ///    admits a proof dodging its own obligations.
    /// 3. **And it would gut the layer even if the statement were trusted.** A
    ///    hypothesis binder present in the submission but absent from the canonical
    ///    statement is already rejected by statement preservation, which is
    ///    conjoined unconditionally in `verify_with_gates`. Allowlisting every
    ///    binder the canonical statement carries would therefore leave
    ///    `hypothesis_audit` with nothing it can still reject: the gate would
    ///    become a no-op that reads like a check.
    /// 4. **No other local source qualifies.** `FormalProject` carries `imports`,
    ///    which are MODULE imports granting access to *proved* lemmas and need no
    ///    allowlist; `Config` has no trusted-premise field. Hardcoding names
    ///    (`RiemannHypothesis`, …) would be the list of mathematical facts this
    ///    must never become.
    ///
    /// # What would un-inert the layer
    ///
    /// A designated-inputs field on the TASK, authored by whoever defined the
    /// task rather than by the model that formalized it, threaded to here:
    /// `ProofTask`/`FormalProject` (`prover/model.rs`) or the graph node that
    /// produces them, plumbed either into `LeanBackend::with_designated_inputs`
    /// at construction (`formal::backend_for`) or, for a per-goal answer, by
    /// giving `FormalBackend::designated_inputs` a `&str` statement parameter the
    /// way `hypothesis_bundle`/`satisfiability_witness` already have one. Until
    /// then the honest state is empty, and `THEOREMATA_HYPOTHESIS_GATE` must stay
    /// off: with an empty allowlist the audit rejects every genuine conditional
    /// theorem. Empty is NOT a pass. See
    /// `empty_designated_inputs_fail_the_gate_they_do_not_silence_it`.
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

        let sig =
            crate::prover::statement_preservation::check_statement_preserved(stmt, "").canonical?;
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

    /// ALIAS EXPANSION. `decide +native` is `native_decide` under Lean's tactic
    /// CONFIG syntax, and `sorryAx` is the axiom `sorry` elaborates to. Banning
    /// only the base spelling was a ban a rename walked straight past.
    #[test]
    fn renamed_lean_hatches_are_caught() {
        for (code, expected) in [
            ("theorem t : P := by decide +native\n", "+native"),
            ("theorem t : P := by decide +kernel +native\n", "+native"),
            ("theorem t : P := sorryAx _ false\n", "sorryAx"),
            (
                "theorem t : P := by exact Lean.ofReduceNat h\n",
                "ofReduceNat",
            ),
        ] {
            let report = fallback_source_scan(code);
            assert!(!report.clean, "alias must be caught: {code:?}");
            assert!(
                report.findings.iter().any(|f| f == expected),
                "expected `{expected}` in {:?}",
                report.findings
            );
        }
    }

    /// The boundary trade-off, asserted in the OVER-matching direction. A plain
    /// `decide` is kernel-checked and legitimate, and an identifier that merely
    /// CONTAINS a banned token is ordinary Lean; substring matching flagged
    /// these and every false flag costs a retry.
    #[test]
    fn identifiers_containing_a_hatch_token_are_not_flagged() {
        for code in [
            "theorem t : 2 + 2 = 4 := by decide\n",
            "instance : DecidableEq Foo := decidable_eq_foo\n",
            "theorem admits_a_root (f : R) : True := trivial\n",
            "theorem sorry_free' : True := trivial\n",
            "theorem t (x native : Nat) : x + native = native + x := by ring\n",
            "open Nat Finset in\ntheorem t : True := trivial\n",
        ] {
            let report = fallback_source_scan(code);
            assert!(
                report.clean,
                "innocent source must not be flagged ({code:?}): {:?}",
                report.findings
            );
        }
    }

    /// `open private` reaches a declaration a module deliberately did not
    /// export, and nothing in the kernel or the axiom audit reports it.
    #[test]
    fn open_private_is_flagged() {
        let report = fallback_source_scan(
            "open private secretLemma in\ntheorem t : P := by\n  exact secretLemma\n",
        );
        assert!(!report.clean);
        assert!(report.findings.iter().any(|f| f == "open private"));
        // Line-broken between the two words: whitespace normalization means the
        // pattern still bites.
        let split =
            fallback_source_scan("open\n  private secretLemma in\ntheorem t : P := trivial\n");
        assert!(split.findings.iter().any(|f| f == "open private"));
    }

    /// It inherits the comment policy of the scan it lives in: a MENTION of the
    /// hatch in a comment is never seen by the kernel and must not gate. A plain
    /// `open` of a public namespace is ordinary Lean and stays allowed.
    #[test]
    fn commented_or_plain_open_is_not_flagged() {
        let commented =
            fallback_source_scan("-- open private secretLemma in\ntheorem t : True := trivial\n");
        assert!(commented.clean, "findings: {:?}", commented.findings);
        let plain = fallback_source_scan("open Nat Finset\ntheorem t : True := trivial\n");
        assert!(plain.clean, "findings: {:?}", plain.findings);
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
        assert!(
            !b.is_trivial(),
            "a Prop hypothesis makes the bundle non-trivial"
        );

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
        let b =
            bundle("theorem id_eq (α : Type) (f : Nat → Nat) (x : α) : x = x").expect("must parse");
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
            assert!(
                bundle(stmt).is_none(),
                "must not guess a bundle for `{stmt}`"
            );
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
        let b =
            bundle("theorem cond (hR : RiemannHypothesis) (n : Nat) : n = n").expect("must parse");
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

    /// A theorem conditional on a `Prop` that is STATED and never PROVED: the
    /// mechanism-(a) shape `hypothesis_audit` exists to catch, and the shape a
    /// GENUINE "assuming Glaisher3, …" task also has. Only the task definer can
    /// tell the two apart, which is what the allowlist is for.
    const COND_STMT: &str = "theorem phi3 (hG : Glaisher3) : True";
    const COND_CODE: &str = "\
def Glaisher3 : Prop := True

theorem phi3 (hG : Glaisher3) : True := trivial
";

    fn cond_report(
        backend: &LeanBackend,
        hypothesis_discharge: bool,
    ) -> crate::prover::model::VerificationReport {
        backend
            .verify_with_gates(
                &Config::default(),
                COND_CODE,
                COND_STMT,
                crate::prover::formal::TierZeroGates {
                    hypothesis_discharge,
                    vacuity: false,
                },
            )
            .expect("the mock backend's verify must succeed")
    }

    /// **The inertness pin.** With nobody having declared a designated input the
    /// allowlist is empty, and an empty allowlist is not a quiet pass: the audit
    /// reports UN-clean, and switching the gate on rejects the submission. This is
    /// pinned so that the layer's current silence can never be misread as "the
    /// check ran and found nothing".
    #[test]
    fn empty_designated_inputs_fail_the_gate_they_do_not_silence_it() {
        let backend = LeanBackend::mock();
        assert!(backend.designated_inputs().is_empty());

        // Gate OFF (the shipped default): the verdict is computed and published,
        // and explicitly marked NOT enforced. This is the inert state.
        let off = cond_report(&backend, false);
        let t = &off.detail["tier0"]["hypothesis_audit"];
        assert_eq!(t["clean"], json!(false), "detail: {t}");
        assert_eq!(t["enforced"], json!(false));
        assert_eq!(t["designated_inputs"], json!([]));
        assert!(
            off.lexically_verified,
            "an unenforced Tier-0 verdict must not move the classic gate"
        );

        // Gate ON with the channel still empty: the genuine antecedent reads as
        // Unaccounted and the submission FAILS. Un-inerting the layer without
        // populating the channel costs a rejection, never a false accept.
        let on = cond_report(&backend, true);
        assert!(
            !on.lexically_verified,
            "an empty allowlist must fail closed, not pass silently: {:#?}",
            on.detail
        );
        assert_eq!(
            on.detail["tier0"]["hypothesis_audit"]["enforced"],
            json!(true)
        );
    }

    /// The channel is not broken, only unpopulated: declaring the antecedent on
    /// the backend instance clears the very gate the empty list fails.
    #[test]
    fn task_declared_designated_inputs_clear_the_gate() {
        // By TYPE HEAD...
        let by_head = LeanBackend::mock().with_designated_inputs(["Glaisher3"]);
        assert!(cond_report(&by_head, true).lexically_verified);
        // ...and by BINDER name.
        let by_binder = LeanBackend::mock().with_designated_inputs(["hG"]);
        assert!(cond_report(&by_binder, true).lexically_verified);
        // An unrelated entry designates nothing: the allowlist is not a wildcard.
        let unrelated = LeanBackend::mock().with_designated_inputs(["SomeOtherProp"]);
        assert!(!cond_report(&unrelated, true).lexically_verified);
    }

    /// Provenance is recorded, and `Derived` is not one of the options: every
    /// entry is asserted by a person, none is worked out by this backend.
    #[test]
    fn designated_inputs_provenance_is_asserted_never_derived() {
        assert_eq!(
            LeanBackend::mock().designated_inputs_source,
            DesignatedInputsSource::Unset
        );

        let declared = LeanBackend::mock().with_designated_inputs(["Glaisher3", "hRH"]);
        assert_eq!(
            declared.designated_inputs_source,
            DesignatedInputsSource::TaskDeclared
        );
        assert_eq!(declared.designated_inputs, vec!["Glaisher3", "hRH"]);
        assert_eq!(
            declared.designated_inputs_source.tag(),
            "task_declared",
            "the tag is what reaches a log line"
        );

        // A declaration that parses to nothing stays Unset: provenance describes
        // what the audit will actually see, so a blank declaration can never read
        // as a populated channel.
        let blank = LeanBackend::mock().with_designated_inputs([" ", ","]);
        assert!(blank.designated_inputs.is_empty());
        assert_eq!(
            blank.designated_inputs_source,
            DesignatedInputsSource::Unset
        );
    }

    /// Nothing about the statement changes the allowlist: the answer is a property
    /// of the backend instance alone. This pins point (1) of the
    /// `designated_inputs` docs: a per-statement answer is not expressible here,
    /// so no future edit may quietly start deriving one from the (model-authored)
    /// statement.
    #[test]
    fn designated_inputs_do_not_depend_on_the_statement() {
        let backend = LeanBackend::mock();
        // A statement stuffed with plausible-looking antecedents designates none
        // of them.
        assert!(backend.designated_inputs().is_empty());
        assert_eq!(
            crate::prover::hypothesis_audit::audit_hypotheses(
                SYSTEM,
                COND_STMT,
                COND_CODE,
                &backend.designated_inputs(),
            )
            .unaccounted_count(),
            1,
            "a binder appearing in the canonical statement is NOT self-designating"
        );
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

#[cfg(test)]
mod elaboration_tests {
    use super::*;

    /// One real `lean --json` line, captured verbatim from lean 4.32.0 on the
    /// machine this was developed on, for
    /// `theorem algebra_5778 (x : Nat) (h : x > 0) : x >= 1 := h` with the probe
    /// block appended at lines 4..7. It is the evidence that the fully explicit
    /// form is obtainable at all, so it is pinned here rather than paraphrased.
    const REAL_CHECK_JSON: &str = concat!(
        r#"{"caption":"","data":"algebra_5778 : ∀ (x : Nat)\n  (h : @GT.gt.{0} Nat instLTNat x "#,
        r#"(@OfNat.ofNat.{0} Nat (nat_lit 0) (instOfNatNat (nat_lit 0)))),\n  @GE.ge.{0} Nat "#,
        r#"instLENat x (@OfNat.ofNat.{0} Nat (nat_lit 1) (instOfNatNat (nat_lit 1)))","#,
        r#""endPos":{"column":6,"line":7},"fileName":"Generated_elab.lean","isSilent":false,"#,
        r#""keepFullRange":false,"kind":"[anonymous]","pos":{"column":0,"line":7},"#,
        r#""severity":"information"}"#,
    );

    /// The captured run parses, and what is published is the TYPE, fully
    /// explicit: instances (`instLTNat`) and universe levels (`.{0}`) are present,
    /// which is exactly what a notation-preserving library change (nth roots
    /// re-routed through `rpow`) moves while the source text does not.
    #[test]
    fn a_real_lean_json_check_yields_the_explicit_type() {
        let form = elaborated_form_from_json(REAL_CHECK_JSON, 4)
            .expect("the captured lean 4.32.0 output must parse");
        assert!(form.starts_with("∀ (x : Nat)"), "{form}");
        assert!(form.contains("@GT.gt.{0} Nat instLTNat"), "{form}");
        assert!(
            !form.contains("algebra_5778"),
            "the name must not be in the form: {form}"
        );
    }

    /// A RENAME must not move the form, or the discriminator cannot tell "your
    /// script needs a rename" from "the mathematics moved", the entire purpose.
    #[test]
    fn the_declaration_name_is_not_part_of_the_form() {
        let a = strip_check_prefix("algebra_5778 : @Eq.{1} Nat x y").unwrap();
        let b = strip_check_prefix("algebra_5778' : @Eq.{1} Nat x y").unwrap();
        assert_eq!(a, b);
        assert_eq!(a, "@Eq.{1} Nat x y");
        // Universe-parameterized names still split at the first ` : `.
        assert_eq!(
            strip_check_prefix("foo.{u_1} : Type u_1").unwrap(),
            "Type u_1"
        );
        assert!(strip_check_prefix("no separator here").is_none());
    }

    /// Only the block WE appended may be read, and any error voids the probe.
    /// Both are fail-to-absent, which `checker_cache` reads as UNAVAILABLE.
    #[test]
    fn a_foreign_check_or_any_error_publishes_nothing() {
        let foreign = concat!(
            r#"{"data":"userCheck : Nat","severity":"information","pos":{"line":2,"column":0}}"#,
            "\n",
        );
        assert_eq!(elaborated_form_from_json(foreign, 4), None);

        let errored = concat!(
            r#"{"data":"t : True","severity":"information","pos":{"line":9,"column":0}}"#,
            "\n",
            r#"{"data":"unknown identifier","severity":"error","pos":{"line":9,"column":7}}"#,
            "\n",
        );
        assert_eq!(elaborated_form_from_json(errored, 4), None);
        assert_eq!(elaborated_form_from_json("not json at all\n", 1), None);
    }

    /// The probe block's line count is what makes position attribution correct,
    /// and the anti-elision options are what keep two different types from
    /// printing the same.
    #[test]
    fn probe_block_is_four_lines_and_pins_the_pp_options() {
        let block = elaboration_probe_block("algebra_5778");
        assert_eq!(block.lines().count(), 4);
        assert!(block.contains("set_option pp.all true in"));
        assert!(block.contains("set_option pp.deepTerms true in"));
        assert!(block.contains("set_option pp.maxSteps"));
        assert!(block.trim_end().ends_with("#check @algebra_5778"));
    }

    #[test]
    fn the_probe_defaults_on_and_the_kill_switch_turns_it_off() {
        std::env::remove_var(ELABORATION_PROBE_ENV);
        assert!(elaboration_probe_enabled());
        for off in ["0", "false", "OFF"] {
            std::env::set_var(ELABORATION_PROBE_ENV, off);
            assert!(!elaboration_probe_enabled(), "{off} must disable the probe");
        }
        std::env::set_var(ELABORATION_PROBE_ENV, "1");
        assert!(elaboration_probe_enabled());
        std::env::remove_var(ELABORATION_PROBE_ENV);
    }

    /// The provenance label names the mechanism and never claims a kernel type.
    #[test]
    fn provenance_is_honest_about_being_a_pretty_printed_type() {
        assert_eq!(ELABORATED_PROVENANCE, "lean.check.pp_all");
        assert!(!ELABORATED_PROVENANCE.contains("kernel"));
    }

    // -- Phase 1.2: re-elaboration -----------------------------------------

    /// The pin and the re-elaboration must be produced by ONE mechanism. This
    /// pins the shared pieces: the same probe block, appended by the same helper,
    /// parsed by the same parser, with the name stripped the same way.
    #[test]
    fn the_pin_and_the_re_elaboration_share_one_probe_mechanism() {
        let (pin_content, pin_line) =
            append_elaboration_probe("import Mathlib\ntheorem t : True := trivial\n", "t");
        let (re_content, re_line) = append_elaboration_probe(
            &reelaboration_probe_source("import Mathlib", "True"),
            REELABORATION_DECL,
        );
        // Same trailing block, differing only in the declaration name, which
        // `strip_check_prefix` removes before anything is compared.
        assert!(pin_content.ends_with(&elaboration_probe_block("t")));
        assert!(re_content.ends_with(&elaboration_probe_block(REELABORATION_DECL)));
        // Same line accounting on both sides.
        assert_eq!(pin_line, pin_content.lines().count() - 3);
        assert_eq!(re_line, re_content.lines().count() - 3);
    }

    /// The re-elaborated declaration's own name never reaches a comparison, for
    /// the same reason the pin strips its theorem name: a name inside the form
    /// would make every rename look like moved mathematics.
    #[test]
    fn the_re_elaborated_declaration_name_is_stripped_like_the_pin() {
        let printed = format!("{REELABORATION_DECL} : ∀ (x : Nat), @Eq.{{1}} Nat x x");
        assert_eq!(
            strip_check_prefix(&printed).as_deref(),
            Some("∀ (x : Nat), @Eq.{1} Nat x x")
        );
    }

    /// `autoImplicit` OFF is what makes a missing constant say `Unknown
    /// identifier` instead of silently becoming a fresh variable and failing
    /// later with something that reads like a type error.
    #[test]
    fn the_re_elaboration_probe_disables_auto_implicit() {
        let src = reelaboration_probe_source("import Mathlib", "True");
        assert!(src.contains("set_option autoImplicit false"));
        assert!(src.contains("set_option relaxedAutoImplicit false"));
        assert!(src.contains(&format!("axiom {REELABORATION_DECL} :")));
        assert!(reelaboration_control_source("import Mathlib")
            .contains("set_option autoImplicit false"));
    }

    /// The parser must keep "ran and refused" apart from "no answer". Collapsing
    /// them into `None` is what would let a timeout withdraw a good green.
    #[test]
    fn the_probe_parser_separates_a_refusal_from_silence() {
        let ok = r#"{"severity":"information","pos":{"line":4},"data":"t : True"}"#;
        assert_eq!(
            probe_outcome_from_json(ok, 4),
            ProbeOutcome::Form("True".to_string())
        );
        let err = r#"{"severity":"error","pos":{"line":4},"data":"type mismatch"}"#;
        assert_eq!(
            probe_outcome_from_json(err, 4),
            ProbeOutcome::Failed("type mismatch".to_string())
        );
        assert_eq!(probe_outcome_from_json("", 4), ProbeOutcome::NoAnswer);
        assert_eq!(
            probe_outcome_from_json("not json\n", 1),
            ProbeOutcome::NoAnswer
        );
        // And the pin path's wrapper still sees exactly `None` for both failures.
        assert_eq!(elaborated_form_from_json(err, 4), None);
        assert_eq!(elaborated_form_from_json("", 4), None);
    }

    /// The most damaging possible error is withdrawing a green because an import
    /// was missing. Real lean 4.32.0 message text, captured live.
    #[test]
    fn an_unresolved_name_is_never_read_as_ill_typedness() {
        // Live text from lean 4.32.0 with `autoImplicit` off.
        let unknown = "Unknown identifier `f`\n\nNote: It is not possible to treat `f` as an \
                       implicitly bound variable here because the `autoImplicit` option is set";
        assert!(names_are_unresolved(unknown));
        assert!(!is_ill_typed(unknown));
        for text in [
            "unknown constant 'Real.nnrpow'",
            "unknown namespace 'Mathlib.Analysis'",
        ] {
            assert!(names_are_unresolved(text));
            assert!(!is_ill_typed(text));
        }
    }

    /// And the converse: a genuine ill-typedness, also live text, is recognized.
    #[test]
    fn a_genuine_ill_typedness_is_recognized() {
        // Live text from lean 4.32.0, `def f (x : Nat)` changed to `def f (x : Int)`
        // with the pinned `pp.all` form re-elaborated against it.
        let moved = "Application type mismatch: The argument\n  f ↑x\nhas type\n  ℤ but is \
                     expected to have type\n  ℕ in the application\n  Eq (f ↑x)";
        assert!(is_ill_typed(moved));
        assert!(!names_are_unresolved(moved));
        // Live text from the `abbrev T := Nat` -> `abbrev T := Int` fixture.
        let abbrev_moved = "Application type mismatch: The argument\n  instAddNat\nhas type\n  \
                            Add ℕ\nbut is expected to have type\n  Add T";
        assert!(is_ill_typed(abbrev_moved));
    }

    /// An error we do not recognize must cost an `Unknown`, not a withdrawal.
    /// This is the allowlist direction, asserted rather than assumed.
    #[test]
    fn an_unrecognized_error_is_not_ill_typedness() {
        for text in [
            "(deterministic) timeout at `whnf`, maximum number of heartbeats (200000) has been reached",
            "maximum recursion depth has been reached",
            "failed to synthesize\n  Add T",
            "",
        ] {
            assert!(!is_ill_typed(text), "must not license a withdrawal: {text}");
        }
    }

    /// Every refusal path out of the entry point is `Unavailable`. There is no
    /// input that turns a failure into an `Elaborated`.
    ///
    /// This function used to carry its own env opt-in on top of the sweep's
    /// flag, so two switches had to agree before anything ran. Both are gone:
    /// cost is the CALLER's bound now (the sweep's skip gates, its cache and its
    /// per-sweep budget), and this function's only job is to be unable to lie.
    /// No env var is read here, so this test also no longer mutates the process
    /// environment, which is what made it race under the parallel harness.
    #[test]
    fn every_refusal_out_of_the_entry_point_is_unavailable() {
        let cfg = Config::default();
        // Empty pin, empty preamble.
        assert!(matches!(
            reelaborate_pinned_statement(&cfg, "import Mathlib", "  "),
            LeanReelaboration::Unavailable { .. }
        ));
        assert!(matches!(
            reelaborate_pinned_statement(&cfg, "", "True"),
            LeanReelaboration::Unavailable { .. }
        ));
        // A mock prover consults no library and so can say nothing.
        let mut mock = Config::default();
        mock.prover_mock = true;
        assert!(matches!(
            reelaborate_pinned_statement(&mock, "import Mathlib", "True"),
            LeanReelaboration::Unavailable { .. }
        ));
    }

    /// Header-only import parsing: a later `import` line (or one inside a
    /// string) is not a header import and must not become a module.
    #[test]
    fn imports_are_header_only_plus_the_implicit_init() {
        let code = "import Mathlib\nimport Mathlib.Tactic\ntheorem t : True := by\n  trivial\nimport Evil\n";
        assert_eq!(
            parse_import_modules(code),
            vec![
                "Init".to_string(),
                "Mathlib".to_string(),
                "Mathlib.Tactic".to_string()
            ]
        );
        // Every file imports `Init`, which is why it is always in the list.
        assert_eq!(
            parse_import_modules("theorem t : True := trivial\n"),
            vec!["Init".to_string()]
        );
    }

    /// Root recovery on the exact path shapes lean 4.32.0 printed here,
    /// including the mixed separators Windows produces.
    #[test]
    fn library_roots_are_recovered_from_resolved_olean_paths() {
        let mixed = "C:/Users/x/scratch/libA\\Foo.olean";
        assert_eq!(
            olean_root_for(mixed, "Foo").unwrap(),
            "C:/Users/x/scratch/libA",
            "the on-disk casing and separators are preserved"
        );
        assert_eq!(
            olean_root_for("/x/Mathlib/Analysis/Foo.olean", "Mathlib.Analysis.Foo").unwrap(),
            "/x"
        );
        // A path that is not this module's yields nothing rather than a guess.
        assert!(olean_root_for("/x/Mathlib/Analysis/Foo.olean", "Init").is_none());
    }

    /// THE POINT OF TASK 2, in miniature: a second library reached through an
    /// ambient `LEAN_PATH` shows up as a second root, one the configured Lake
    /// project never named. Verified live: with two directories each holding a
    /// different `Foo.olean`, the identical `import Foo` elaborated against
    /// whichever `LEAN_PATH` named, and `lean --deps` reported that one.
    #[test]
    fn an_ambient_library_shows_up_as_its_own_root() {
        let deps = vec![
            "c:\\lean\\lib\\lean\\Init.olean".to_string(),
            "c:\\lean\\lib\\lean\\Init.olean".to_string(),
            "D:/somewhere-else/.lake/build/lib/Mathlib.olean".to_string(),
        ];
        let modules = parse_import_modules("import Mathlib\n");
        assert_eq!(
            library_roots(&deps, &modules),
            vec![
                "D:/somewhere-else/.lake/build/lib".to_string(),
                "c:\\lean\\lib\\lean".to_string()
            ],
            "sorted and deduplicated so the detail field is stable run to run"
        );
    }

    #[test]
    fn dep_paths_are_deduplicated_in_order() {
        assert_eq!(
            parse_dep_paths("a.olean\na.olean\n\n b.olean \n"),
            vec!["a.olean".to_string(), "b.olean".to_string()]
        );
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
