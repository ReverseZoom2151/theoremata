//! Phase 1.4: the staleness SWEEP, an honest census over stored verified results.
//!
//! `docs/PLAN-MATH-AT-SCALE.md` Phase 1.4 asks for exactly one thing that did not
//! exist: a verb that walks what the store already recorded, calls the existing
//! [`staleness::assess`] on each result under the CURRENT environment, and reports
//! the Fresh / RepairCandidate / MathematicsMoved / Unknown split. It is a census,
//! not a re-prove loop and not a repair loop: it classifies, it never re-verifies
//! and never patches.
//!
//! ## Where the input comes from
//!
//! `formal_generate` publishes a `verification_provenance` record (schema
//! `theoremata.verification-provenance.v1`) onto every completed verification, on
//! the report's `detail` and, unconditionally, on the `formal_generate.completed`
//! event payload. That record was shaped field-for-field so a sweep can rebuild a
//! [`staleness::VerifiedResult`] with no reinterpretation. This module reads the
//! event stream, so it depends only on what was actually written, never on a live
//! prover.
//!
//! ## The one thing this module must never do
//!
//! Collapse Unknown into Fresh. `assess` already fails toward
//! [`staleness::StalenessVerdict::Unknown`]; this module preserves that by obeying
//! the documented contract of the provenance record: whenever TODAY's environment
//! for a result's system does NOT resolve, we pass `None` as `assess`'s
//! `current_environment`, which yields `Unknown(EnvironmentUnresolved)`. We never
//! synthesize a current fingerprint from an unresolved state, because two
//! "unresolved for the same reason" strings would compare equal and read as Fresh.
//! That is the exact false-clean the plan forbids.
//!
//! ## Re-elaboration (Phase 1.2)
//!
//! The discriminator is wired, and it is OPT-IN. `SweepOptions::reelaborate` is
//! false by default and the default census costs exactly what it cost before: no
//! prover is spawned, `assess` is handed `None`, and the only reachable verdicts
//! from real data are Fresh (exact fingerprint match) and Unknown.
//!
//! With the opt-in set, a node that is BOTH stale and carries a pinned elaborated
//! form is re-elaborated through
//! [`crate::prover::lean::reelaborate_pinned_statement`], which uses the
//! very mechanism that produced the pin. That is what makes RepairCandidate and
//! MathematicsMoved reachable from real data.
//!
//! Three things bound the cost, in this order, so the expensive call is made only
//! when its answer can change the verdict:
//!
//! 1. the opt-in must be set,
//! 2. the current environment must have RESOLVED (an unresolved one is
//!    `Unknown(EnvironmentUnresolved)` whatever a re-elaboration said),
//! 3. the fingerprint must have MOVED and a pinned form must exist (a fresh node
//!    needs no answer, and an unpinned one cannot use one).
//!
//! ## The one thing re-elaboration must never do
//!
//! Turn "we could not elaborate" into "the mathematics moved". A missing import,
//! an absent toolchain, a timeout or a preamble narrower than the pin was
//! elaborated under all produce `Unavailable`, which becomes
//! `Unknown(ReelaborationUnavailable)`. Only an ill-typedness reported by a Lean
//! that has just proved, on a control probe, that it can elaborate in this context
//! becomes a withdrawal. See the backend function for how the two are told apart.
//!
//! This sweep still never repairs, re-verifies or mutates a stored verdict. A
//! re-elaboration is a read: it elaborates a type in a throwaway workspace and
//! reports a string.

use anyhow::Result;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

use super::staleness::{
    self, ArtifactClass, EnvironmentFingerprint, RecheckScope, StalenessReport, VerifiedResult,
};
use crate::checker_cache::EnvironmentFingerprint as CheckerEnvironment;
use crate::config::Config;
use crate::db::Store;
use crate::prover::lean::{
    reelaborate_pinned_statement, LeanReelaboration, DEFAULT_REELABORATION_PREAMBLE,
    REELABORATION_OPT_IN_ENV,
};
use crate::prover::formal::{backend_for, FormalSystem};

/// The schema tag written by `formal_generate::provenance_value` (under the event
/// key `verification_provenance`). Duplicated here (that constant is private to
/// `formal_generate`) rather than imported; the two must stay in step, and this
/// comment is the reminder if either moves. We match on this `schema` field during
/// a bounded walk of every payload rather than on the fixed key, so a provenance
/// record nested inside a portfolio/per-system event is still found.
const PROVENANCE_SCHEMA: &str = "theoremata.verification-provenance.v1";

/// Our own output schema tag, so a consumer can tell one sweep format from another.
const SWEEP_SCHEMA: &str = "theoremata.staleness-sweep.v1";

// ===========================================================================
// Parsing the stored provenance into staleness inputs
// ===========================================================================

/// The fields lifted out of one provenance record. Kept separate from
/// [`VerifiedResult`] only so the raw strings needed for reporting (the system
/// tag, the resolved bit, the verbatim `verified_against`) survive alongside the
/// staleness input built from them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedProvenance {
    system: String,
    id: String,
    artifact: ArtifactClass,
    /// The `key_field()` string the verdict was earned against. For a resolved
    /// environment this is `resolved:<kind>:<digest>`; for an unresolved one it is
    /// `unresolved:<reason>`, which can never match a resolved current env.
    verified_against: String,
    environment_resolved: bool,
    pinned_statement_type: Option<String>,
    /// The import header the pin was elaborated under, when the record carries
    /// one. Absent today (the provenance writer does not emit the field yet), and
    /// absence is why [`SweepOptions::reelaboration_preamble`] exists. Read
    /// tolerantly so that a writer adding the field later needs no change here.
    pinned_statement_imports: Option<String>,
    verdict_verified: bool,
}

impl ParsedProvenance {
    /// Rebuild the [`VerifiedResult`] the classifier consumes. Field-for-field
    /// with the writer, no reinterpretation.
    fn to_verified_result(&self) -> VerifiedResult {
        VerifiedResult::new(
            self.id.clone(),
            self.artifact,
            EnvironmentFingerprint::new(self.verified_against.clone()),
            self.pinned_statement_type.clone(),
        )
    }
}

/// Map the stable artifact tag back to an [`ArtifactClass`].
///
/// An unknown or missing tag routes to [`ArtifactClass::TacticScript`], the most
/// expensive route (statement AND proof rechecked). Guessing a certificate here
/// would hand a result the cheap statement-only route it did not earn, so the
/// conservative default is the script.
fn artifact_from_tag(tag: Option<&str>) -> ArtifactClass {
    match tag {
        Some("self_contained_certificate") => ArtifactClass::SelfContainedCertificate,
        Some("proof_term") => ArtifactClass::ProofTerm,
        Some("tactic_script") => ArtifactClass::TacticScript,
        _ => ArtifactClass::TacticScript,
    }
}

/// Parse one JSON object into a [`ParsedProvenance`], or `None` when it is not a
/// recognizable provenance record. A missing `verified_against` becomes an
/// explicit unresolved marker, never an empty string that might accidentally
/// match something.
fn parse_provenance(value: &serde_json::Value) -> Option<ParsedProvenance> {
    let map = value.as_object()?;
    if map.get("schema").and_then(|v| v.as_str()) != Some(PROVENANCE_SCHEMA) {
        return None;
    }
    let system = map
        .get("system")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let id = map.get("id").and_then(|v| v.as_str())?.to_string();
    let artifact = artifact_from_tag(map.get("artifact").and_then(|v| v.as_str()));
    let verified_against = map
        .get("verified_against")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "unresolved:provenance omitted verified_against".to_string());
    let environment_resolved = map
        .get("environment_resolved")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    // `pinned_statement_type` is null today (no backend publishes an elaborated
    // form). A null or absent value is `None`, which keeps a moved result on the
    // `Unknown(NoPinnedStatementType)` path rather than fabricating a pin.
    let pinned_statement_type = map
        .get("pinned_statement_type")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let pinned_statement_imports = map
        .get("pinned_statement_imports")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty());
    // Only a real green is a subject of the census. Failed attempts also emit
    // provenance; they are counted separately and excluded from classification.
    let verdict_verified = map
        .get("verdict_verified")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Some(ParsedProvenance {
        system,
        id,
        artifact,
        verified_against,
        environment_resolved,
        pinned_statement_type,
        pinned_statement_imports,
        verdict_verified,
    })
}

/// Walk any JSON value and collect every embedded provenance record (matched by
/// its `schema` tag). Recursing rather than reading one fixed key means a record
/// nested inside a portfolio/per-system event payload is still swept. A provenance
/// object is not descended into once matched.
fn collect_provenance(value: &serde_json::Value, out: &mut Vec<ParsedProvenance>) {
    match value {
        serde_json::Value::Object(map) => {
            if map.get("schema").and_then(|v| v.as_str()) == Some(PROVENANCE_SCHEMA) {
                if let Some(parsed) = parse_provenance(value) {
                    out.push(parsed);
                }
                return;
            }
            for v in map.values() {
                collect_provenance(v, out);
            }
        }
        serde_json::Value::Array(items) => {
            for v in items {
                collect_provenance(v, out);
            }
        }
        _ => {}
    }
}

// ===========================================================================
// Current environment resolution
// ===========================================================================

/// TODAY's environment for one system, resolved exactly the way the live
/// verification path resolves it, so the fingerprint is comparable to what a fresh
/// verification would record.
#[derive(Debug, Clone)]
struct CurrentEnvironment {
    /// `Some` only when the environment RESOLVED. `None` is the load-bearing
    /// state: it makes `assess` return `Unknown(EnvironmentUnresolved)` instead of
    /// risking a false Fresh from comparing two unresolved strings.
    fingerprint: Option<EnvironmentFingerprint>,
    describe: String,
    resolved: bool,
}

/// Resolve the current environment for a system tag, mirroring
/// `formal_generate::generate_and_verify_inner`: prefer the live backend when its
/// toolchain is present and the prover is not pinned to mock, otherwise the mock
/// backend; fingerprint against the chosen backend's mock-ness. `.available()`
/// probes the toolchain, the same probe the `route` verb already performs.
fn resolve_current_environment(config: &Config, system_tag: &str) -> CurrentEnvironment {
    let Ok(system) = system_tag.parse::<FormalSystem>() else {
        // Cannot even name the system, so we cannot resolve its environment. Not a
        // failure of the census: it just means these nodes are Unknown, not Fresh.
        return CurrentEnvironment {
            fingerprint: None,
            resolved: false,
            describe: format!("unparseable formal system `{system_tag}`"),
        };
    };
    let live = backend_for(config, system, false);
    let used_live = !config.prover_mock && live.available();
    let backend = if used_live {
        live
    } else {
        backend_for(config, system, true)
    };
    let env = CheckerEnvironment::resolve(
        system,
        backend.is_mock(),
        config.lean_project.as_deref().filter(|p| p.exists()),
    );
    let resolved = env.is_resolved();
    let describe = env.describe();
    CurrentEnvironment {
        // Only a resolved environment becomes a comparable fingerprint. An
        // unresolved one is dropped to `None` on purpose.
        fingerprint: resolved.then(|| EnvironmentFingerprint::new(env.key_field())),
        resolved,
        describe,
    }
}

// ===========================================================================
// Options: the Phase 1.2 opt-in
// ===========================================================================

/// What a sweep is allowed to spend.
///
/// [`Default`] is the CHEAP CENSUS, byte for byte what the sweep did before
/// Phase 1.2 landed: no prover, no re-elaboration. That default is the cost gate
/// the plan asks for. Turning re-elaboration on spawns Lean twice per stale,
/// pinned, resolvable node, which over a large store is minutes to hours.
#[derive(Debug, Clone, Default)]
pub struct SweepOptions {
    /// Run the Phase 1.2 discriminator. **Default false.**
    ///
    /// Even when true, the backend enforces its own opt-in
    /// ([`REELABORATION_OPT_IN_ENV`]) and answers `Unavailable` without it, so a
    /// caller cannot spend prover time by accident from one side alone. Both must
    /// agree, and the disagreement is reported rather than silently downgraded.
    pub reelaborate: bool,
    /// Import header to re-elaborate a pinned form against, when the stored record
    /// does not carry its own.
    ///
    /// The record's `pinned_statement_imports` always wins when present, because
    /// only it is the context the pin was actually elaborated under. This is the
    /// operator's stand-in for that, and `None` falls back to
    /// [`DEFAULT_REELABORATION_PREAMBLE`]. Which of the three was used is reported
    /// per node, so no reader has to guess whether the comparison was made against
    /// the real context or a substitute for it.
    pub reelaboration_preamble: Option<String>,
}

/// Where a re-elaboration preamble came from. Reported per node because a
/// substituted preamble is a weaker comparison than the recorded one, and the
/// difference must be visible rather than inferred.
fn preamble_for(record: &ParsedProvenance, options: &SweepOptions) -> (String, &'static str) {
    if let Some(imports) = record.pinned_statement_imports.as_deref() {
        return (imports.to_string(), "provenance_record");
    }
    if let Some(supplied) = options.reelaboration_preamble.as_deref() {
        return (supplied.to_string(), "operator_supplied");
    }
    (DEFAULT_REELABORATION_PREAMBLE.to_string(), "default")
}

/// Run the Phase 1.2 discriminator for one node, or explain why it was not run.
///
/// Returns the outcome to hand `assess` plus a human-readable note. `None` means
/// NOT ATTEMPTED, which `assess` already turns into
/// `Unknown(ReelaborationUnavailable)` when the fingerprint moved: not attempting
/// is never a road to Fresh.
fn reelaborate_node(
    config: &Config,
    record: &ParsedProvenance,
    current: Option<&EnvironmentFingerprint>,
    options: &SweepOptions,
) -> Option<(staleness::ReelaborationOutcome, String)> {
    if !options.reelaborate {
        return None;
    }
    // An unresolved current environment is `Unknown(EnvironmentUnresolved)` no
    // matter what a re-elaboration says, so paying for one would buy nothing.
    let current = current?;
    // A node whose fingerprint still matches is Fresh and needs no answer; a node
    // with no pin has nothing to re-elaborate. Both are skipped for cost, and both
    // already land on the correct verdict without us.
    if record.verified_against.as_str() == current.as_str() {
        return None;
    }
    let pinned = record.pinned_statement_type.as_deref()?;
    // Only Lean publishes a pinned elaborated form and only Lean can re-elaborate
    // one. Any other system is honestly reported as having no discriminator rather
    // than being run through Lean's.
    if record.system.as_str() != FormalSystem::Lean.as_str() {
        return Some((
            staleness::ReelaborationOutcome::Unavailable {
                reason: format!(
                    "no re-elaboration mechanism exists for `{}`; only lean publishes and \
                     re-elaborates a pinned form",
                    record.system
                ),
            },
            format!("skipped: no re-elaborator for {}", record.system),
        ));
    }

    let (preamble, preamble_source) = preamble_for(record, options);
    let outcome = match reelaborate_pinned_statement(config, &preamble, pinned) {
        LeanReelaboration::Elaborated { form } => {
            staleness::ReelaborationOutcome::Elaborated {
                statement_type: form,
            }
        }
        LeanReelaboration::Rejected { detail } => {
            staleness::ReelaborationOutcome::Rejected { detail }
        }
        LeanReelaboration::Unavailable { reason } => {
            staleness::ReelaborationOutcome::Unavailable { reason }
        }
    };
    Some((outcome, format!("preamble from {preamble_source}")))
}

// ===========================================================================
// Output
// ===========================================================================

/// The bucket counts, mirrored from [`staleness::Census`] into a serializable form.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct CensusOut {
    pub fresh: usize,
    pub repair_candidate: usize,
    pub mathematics_moved: usize,
    pub unknown: usize,
    pub total: usize,
    /// `repair_candidate + mathematics_moved + unknown`. Unknown is counted here,
    /// because an unassessable result is not a verified one.
    pub not_verified: usize,
}

/// One resolved current environment, for the report header.
#[derive(Debug, Clone, Serialize)]
pub struct EnvironmentOut {
    pub resolved: bool,
    pub describe: String,
}

/// One classified node in the census.
#[derive(Debug, Clone, Serialize)]
pub struct NodeVerdict {
    pub id: String,
    pub system: String,
    /// Stable artifact tag.
    pub artifact: String,
    /// `fresh` | `repair_candidate` | `mathematics_moved` | `unknown`. Never folded.
    pub bucket: String,
    /// Human-readable cause, including the specific `UnknownReason` when unknown.
    pub detail: String,
    /// The recheck route this artifact class earns (Phase 1.3): a self-contained
    /// certificate is `statement_only`, everything else is `statement_and_proof`.
    pub recheck_scope: String,
    /// The verbatim environment string the verdict was earned against.
    pub verified_against: String,
    /// Whether that environment was resolved when the result was recorded.
    pub verified_environment_resolved: bool,
    /// Whether this node received a real re-elaboration this sweep. False unless
    /// [`SweepOptions::reelaborate`] was set AND the node was worth spending one
    /// on (stale, pinned, resolvable environment).
    pub reelaborated: bool,
    /// Why the re-elaboration was or was not run, and which preamble it used.
    /// `None` when it was not attempted at all.
    pub reelaboration_note: Option<String>,
}

/// The full census result. Serializable and `Debug`, so a CLI arm can hand it
/// straight to the shared `print_value` like every other verb.
#[derive(Debug, Clone, Serialize)]
pub struct SweepOutcome {
    pub schema: &'static str,
    /// The one-line summary from [`StalenessReport::summary`].
    pub summary: String,
    pub census: CensusOut,
    /// Repair candidates that need no prover work at all (certificate whose
    /// statement already re-elaborated unchanged). Zero today.
    pub statement_only_repairs: usize,
    /// Non-fresh counts split by artifact class.
    pub drift_by_artifact: BTreeMap<String, usize>,
    /// True iff any node got a real re-elaboration, so the census can never be
    /// mistaken for one backed by live re-elaboration when it was not.
    pub any_reelaborated: bool,
    /// Whether the caller ASKED for re-elaboration. Distinct from
    /// `any_reelaborated`: asking and getting nothing (no stale node had a pin, or
    /// the backend opt-in was unset) is a different fact from not asking.
    pub reelaboration_requested: bool,
    /// How many nodes a re-elaboration was actually spent on.
    pub reelaborations_attempted: usize,
    /// The backend env var that must ALSO be set for a re-elaboration to run, and
    /// whether it was. Reported so an operator who asked for the discriminator and
    /// got no answers can see why in the output rather than in the source.
    pub reelaboration_opt_in_env: &'static str,
    pub reelaboration_opt_in_set: bool,
    /// The current environment resolved per system encountered.
    pub current_environments: BTreeMap<String, EnvironmentOut>,
    /// Every withdrawal's audit line (mathematics moved). Empty today.
    pub withdrawals: Vec<String>,
    /// Per-node classification.
    pub nodes: Vec<NodeVerdict>,
    /// Recorded results that were NOT verified greens, excluded from the census.
    pub skipped_unverified: usize,
    /// How many projects the sweep walked.
    pub projects_scanned: usize,
}

/// Stable string for a recheck scope.
fn scope_tag(scope: RecheckScope) -> &'static str {
    match scope {
        RecheckScope::StatementOnly => "statement_only",
        RecheckScope::StatementAndProof => "statement_and_proof",
    }
}

/// Stable string for an artifact class (kept local so a rename in `staleness`
/// surfaces as a compile error here rather than a silently changed report value).
fn artifact_tag(artifact: ArtifactClass) -> &'static str {
    match artifact {
        ArtifactClass::SelfContainedCertificate => "self_contained_certificate",
        ArtifactClass::TacticScript => "tactic_script",
        ArtifactClass::ProofTerm => "proof_term",
    }
}

/// A human-readable cause for a verdict, spelling out the specific Unknown reason
/// so "we could not look" never reads like "clean".
fn verdict_detail(verdict: &staleness::StalenessVerdict) -> String {
    use staleness::{StalenessVerdict as V, UnknownReason as U};
    match verdict {
        V::Fresh => "fingerprint matches the current environment".to_string(),
        V::RepairCandidate(plan) => format!(
            "environment moved; statement re-elaborates unchanged (`{}`) so this is a repair, not a withdrawal",
            plan.confirmed_statement_type()
        ),
        V::MathematicsMoved(withdrawal) => withdrawal.explain(),
        V::Unknown(reason) => match reason {
            U::EnvironmentUnresolved { detail } => {
                format!("unknown: current environment could not be resolved ({detail})")
            }
            U::NoPinnedStatementType => "unknown: environment moved and no elaborated statement type \
                 was pinned at verification time, so staleness cannot be discriminated"
                .to_string(),
            U::ReelaborationUnavailable { reason } => {
                format!("unknown: environment moved and no re-elaboration was available ({reason})")
            }
        },
    }
}

// ===========================================================================
// The sweep
// ===========================================================================

/// Walk stored verified results and classify each under the current environment.
///
/// `project` restricts the walk to one project; `None` sweeps every project.
/// `limit` bounds how many events are read per project (newest first). The most
/// recent green per `(system, id)` wins, so re-verifications do not double-count.
pub fn sweep(
    store: &Store,
    config: &Config,
    project: Option<&str>,
    limit: usize,
) -> Result<SweepOutcome> {
    sweep_with_options(store, config, project, limit, &SweepOptions::default())
}

/// [`sweep`] with the Phase 1.2 opt-in exposed. `SweepOptions::default()` is the
/// cheap census and is exactly what `sweep` runs.
pub fn sweep_with_options(
    store: &Store,
    config: &Config,
    project: Option<&str>,
    limit: usize,
    options: &SweepOptions,
) -> Result<SweepOutcome> {
    // Which projects to walk. An explicit project is validated by asking the store
    // for its events (a missing project errors there); `None` fans out over all.
    let project_ids: Vec<String> = match project {
        Some(p) => vec![p.to_string()],
        None => store
            .list_projects()?
            .into_iter()
            .map(|p| p.id)
            .collect(),
    };

    // Gather provenance records newest-first, deduplicating to the latest green per
    // (system, id). Events come back DESC by id, so the first record seen for a key
    // is the most recent.
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    let mut records: Vec<ParsedProvenance> = Vec::new();
    let mut skipped_unverified = 0usize;

    for project_id in &project_ids {
        let events = store.events(project_id, limit)?;
        for event in &events {
            let mut found: Vec<ParsedProvenance> = Vec::new();
            collect_provenance(&event.payload, &mut found);
            for parsed in found {
                let key = (parsed.system.clone(), parsed.id.clone());
                if seen.contains(&key) {
                    continue;
                }
                seen.insert(key);
                if !parsed.verdict_verified {
                    // A recorded red is not a green to census. Counted for honesty.
                    skipped_unverified += 1;
                    continue;
                }
                records.push(parsed);
            }
        }
    }

    // Resolve each system's current environment once (the probe is not free).
    let mut env_cache: BTreeMap<String, CurrentEnvironment> = BTreeMap::new();
    for record in &records {
        env_cache
            .entry(record.system.clone())
            .or_insert_with(|| resolve_current_environment(config, &record.system));
    }

    // Classify. `StalenessReport` is the existing census model; we feed it the same
    // verdicts we render, so the counts and the per-node list can never disagree.
    let mut report = StalenessReport::new();
    let mut nodes: Vec<NodeVerdict> = Vec::new();
    let mut any_reelaborated = false;
    let mut reelaborations_attempted = 0usize;

    for record in &records {
        let verified = record.to_verified_result();
        let current = env_cache
            .get(&record.system)
            .and_then(|c| c.fingerprint.as_ref());

        // Phase 1.2. `None` (not attempted) still means `assess` returns
        // `Unknown(ReelaborationUnavailable)` for a moved fingerprint, so the
        // cheap default cannot produce a false Fresh. Bound to a local because
        // `assess` borrows it and the `NodeVerdict` below reads it back.
        let attempt = reelaborate_node(config, record, current, options);
        let reelaboration = attempt.as_ref().map(|(outcome, _)| outcome);
        if reelaboration.is_some() {
            any_reelaborated = true;
            reelaborations_attempted += 1;
        }

        let verdict = staleness::assess(&verified, current, reelaboration);

        nodes.push(NodeVerdict {
            id: record.id.clone(),
            system: record.system.clone(),
            artifact: artifact_tag(record.artifact).to_string(),
            bucket: verdict.bucket().to_string(),
            detail: verdict_detail(&verdict),
            recheck_scope: scope_tag(record.artifact.recheck_scope()).to_string(),
            verified_against: record.verified_against.clone(),
            verified_environment_resolved: record.environment_resolved,
            reelaborated: reelaboration.is_some(),
            reelaboration_note: attempt.as_ref().map(|(_, note)| note.clone()),
        });

        // Record consumes the verdict; the strings above were taken first.
        report.record(record.id.clone(), record.artifact, verdict);
    }

    let census = report.census();
    let drift_by_artifact = report
        .drift_by_artifact()
        .into_iter()
        .map(|(artifact, count)| (artifact_tag(artifact).to_string(), count))
        .collect();
    let withdrawals = report.withdrawals().iter().map(|w| w.explain()).collect();

    let current_environments = env_cache
        .into_iter()
        .map(|(system, env)| {
            (
                system,
                EnvironmentOut {
                    resolved: env.resolved,
                    describe: env.describe,
                },
            )
        })
        .collect();

    Ok(SweepOutcome {
        schema: SWEEP_SCHEMA,
        summary: report.summary(),
        census: CensusOut {
            fresh: census.fresh,
            repair_candidate: census.repair_candidate,
            mathematics_moved: census.mathematics_moved,
            unknown: census.unknown,
            total: census.total(),
            not_verified: census.not_verified(),
        },
        statement_only_repairs: report.statement_only_repairs(),
        drift_by_artifact,
        any_reelaborated,
        reelaboration_requested: options.reelaborate,
        reelaborations_attempted,
        reelaboration_opt_in_env: REELABORATION_OPT_IN_ENV,
        reelaboration_opt_in_set: crate::prover::lean::reelaboration_opt_in(),
        current_environments,
        withdrawals,
        nodes,
        skipped_unverified,
        projects_scanned: project_ids.len(),
    })
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn provenance(
        system: &str,
        id: &str,
        artifact: &str,
        verified_against: &str,
        resolved: bool,
        verified: bool,
    ) -> serde_json::Value {
        json!({
            "schema": PROVENANCE_SCHEMA,
            "system": system,
            "id": id,
            "artifact": artifact,
            "verified_against": verified_against,
            "environment_resolved": resolved,
            "pinned_statement_type": serde_json::Value::Null,
            "verdict_verified": verified,
        })
    }

    #[test]
    fn artifact_tag_defaults_unknown_to_the_expensive_route() {
        assert_eq!(
            artifact_from_tag(Some("self_contained_certificate")),
            ArtifactClass::SelfContainedCertificate
        );
        assert_eq!(artifact_from_tag(Some("proof_term")), ArtifactClass::ProofTerm);
        assert_eq!(artifact_from_tag(Some("tactic_script")), ArtifactClass::TacticScript);
        // Unknown / missing must NOT become a certificate (the cheap route).
        assert_eq!(artifact_from_tag(Some("mystery")), ArtifactClass::TacticScript);
        assert_eq!(artifact_from_tag(None), ArtifactClass::TacticScript);
    }

    #[test]
    fn parse_reads_the_fields_and_null_pin_is_none() {
        let v = provenance(
            "lean",
            "thm",
            "proof_term",
            "resolved:lake:abcd",
            true,
            true,
        );
        let p = parse_provenance(&v).expect("recognized record");
        assert_eq!(p.system, "lean");
        assert_eq!(p.id, "thm");
        assert_eq!(p.artifact, ArtifactClass::ProofTerm);
        assert_eq!(p.verified_against, "resolved:lake:abcd");
        assert!(p.environment_resolved);
        assert!(p.pinned_statement_type.is_none());
        assert!(p.verdict_verified);
    }

    #[test]
    fn non_provenance_object_is_ignored() {
        assert!(parse_provenance(&json!({"schema": "something.else", "id": "x"})).is_none());
        assert!(parse_provenance(&json!({"id": "x"})).is_none());
    }

    #[test]
    fn missing_verified_against_becomes_an_unresolved_marker_not_empty() {
        let v = json!({
            "schema": PROVENANCE_SCHEMA,
            "system": "lean",
            "id": "thm",
            "artifact": "tactic_script",
            "verdict_verified": true,
        });
        let p = parse_provenance(&v).expect("record");
        assert!(p.verified_against.starts_with("unresolved:"));
    }

    fn parsed(system: &str, verified_against: &str, pinned: Option<&str>) -> ParsedProvenance {
        ParsedProvenance {
            system: system.to_string(),
            id: "thm".to_string(),
            artifact: ArtifactClass::TacticScript,
            verified_against: verified_against.to_string(),
            environment_resolved: true,
            pinned_statement_type: pinned.map(|s| s.to_string()),
            pinned_statement_imports: None,
            verdict_verified: true,
        }
    }

    #[test]
    fn the_default_sweep_never_spends_a_re_elaboration() {
        // The cost gate. `SweepOptions::default()` must not reach the prover for
        // ANY node, including one that is stale, pinned and lean.
        let opts = SweepOptions::default();
        assert!(!opts.reelaborate);
        let record = parsed("lean", "resolved:lake:old", Some("T"));
        let current = EnvironmentFingerprint::new("resolved:lake:new");
        let config = Config::default();
        assert!(reelaborate_node(&config, &record, Some(&current), &opts).is_none());
    }

    #[test]
    fn re_elaboration_is_skipped_where_its_answer_could_not_change_the_verdict() {
        let opts = SweepOptions {
            reelaborate: true,
            reelaboration_preamble: None,
        };
        let config = Config::default();
        let current = EnvironmentFingerprint::new("resolved:lake:new");

        // Environment unresolved: `assess` is Unknown(EnvironmentUnresolved)
        // regardless, so no prover time is spent.
        assert!(
            reelaborate_node(&config, &parsed("lean", "resolved:lake:old", Some("T")), None, &opts)
                .is_none()
        );
        // Fingerprint still matches: the node is Fresh without us.
        assert!(reelaborate_node(
            &config,
            &parsed("lean", "resolved:lake:new", Some("T")),
            Some(&current),
            &opts
        )
        .is_none());
        // No pin: nothing to re-elaborate, and `assess` says NoPinnedStatementType.
        assert!(reelaborate_node(
            &config,
            &parsed("lean", "resolved:lake:old", None),
            Some(&current),
            &opts
        )
        .is_none());
    }

    #[test]
    fn a_system_with_no_re_elaborator_is_unavailable_and_never_moved() {
        let opts = SweepOptions {
            reelaborate: true,
            reelaboration_preamble: None,
        };
        let config = Config::default();
        let current = EnvironmentFingerprint::new("resolved:x:new");
        let (outcome, _note) = reelaborate_node(
            &config,
            &parsed("rocq", "resolved:x:old", Some("T")),
            Some(&current),
            &opts,
        )
        .expect("an explicit outcome, not silence");
        assert!(matches!(
            outcome,
            staleness::ReelaborationOutcome::Unavailable { .. }
        ));
        // And it must classify as Unknown, never as a withdrawal.
        let verdict = staleness::assess(
            &parsed("rocq", "resolved:x:old", Some("T")).to_verified_result(),
            Some(&current),
            Some(&outcome),
        );
        assert!(!verdict.is_fresh());
        assert!(verdict.withdrawal().is_none());
    }

    #[test]
    fn the_recorded_preamble_beats_the_operators_and_both_beat_the_default() {
        let mut record = parsed("lean", "resolved:lake:old", Some("T"));
        let operator = SweepOptions {
            reelaborate: true,
            reelaboration_preamble: Some("import Mathlib.Order.Basic".to_string()),
        };

        assert_eq!(
            preamble_for(&record, &SweepOptions::default()),
            (DEFAULT_REELABORATION_PREAMBLE.to_string(), "default")
        );
        assert_eq!(
            preamble_for(&record, &operator),
            ("import Mathlib.Order.Basic".to_string(), "operator_supplied")
        );
        record.pinned_statement_imports = Some("import Mathlib.Analysis.Basic".to_string());
        assert_eq!(
            preamble_for(&record, &operator),
            (
                "import Mathlib.Analysis.Basic".to_string(),
                "provenance_record"
            )
        );
    }

    #[test]
    fn an_absent_or_blank_imports_field_reads_as_absent() {
        let mut v = provenance("lean", "t", "tactic_script", "resolved:lake:a", true, true);
        assert!(parse_provenance(&v)
            .expect("record")
            .pinned_statement_imports
            .is_none());
        v["pinned_statement_imports"] = json!("   ");
        assert!(parse_provenance(&v)
            .expect("record")
            .pinned_statement_imports
            .is_none());
        v["pinned_statement_imports"] = json!("import Mathlib");
        assert_eq!(
            parse_provenance(&v).expect("record").pinned_statement_imports,
            Some("import Mathlib".to_string())
        );
    }

    #[test]
    fn collect_finds_nested_records() {
        // A portfolio-style payload nesting provenance inside an array of systems.
        let payload = json!({
            "statement": "P",
            "per_system": [
                {"system": "lean", "verification_provenance":
                    provenance("lean", "P", "tactic_script", "resolved:lake:aa", true, true)},
                {"system": "rocq", "verification_provenance":
                    provenance("rocq", "P", "tactic_script", "unresolved:rocq", false, false)},
            ],
        });
        let mut out = Vec::new();
        collect_provenance(&payload, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].system, "lean");
        assert_eq!(out[1].system, "rocq");
    }
}
