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
//! ## Re-elaboration (Phase 1.2): what the sweep DOES, not what it is asked to do
//!
//! A node that is BOTH stale and carries a pinned elaborated form is re-elaborated
//! through [`crate::prover::lean::reelaborate_pinned_statement`], which uses the
//! very mechanism that produced the pin. That is what makes RepairCandidate and
//! MathematicsMoved reachable from real data, so it is the discriminator, and a
//! discriminator that has to be switched on is one nothing ever runs. There used
//! to be TWO switches here (a `SweepOptions::reelaborate` flag and an env var the
//! backend read separately, both defaulting off, both required to agree). Both are
//! gone. The cost they were standing in for is now bounded by three mechanisms
//! that are always in force:
//!
//! 1. **The skip gates.** The expensive call is made only where its answer can
//!    change the verdict:
//!    * the current environment must have RESOLVED (an unresolved one is
//!      `Unknown(EnvironmentUnresolved)` whatever a re-elaboration said),
//!    * the fingerprint must have MOVED (a matching one is Fresh without us),
//!    * a pinned form must exist (an unpinned node is `Unknown(NoPinnedStatementType)`
//!      and has nothing to re-elaborate),
//!    * the system must be Lean (the only one that publishes and re-elaborates a
//!      pin; every other system gets an explicit `Unavailable` for free).
//!
//!    Those gates are almost everything on a real store. Measured on this
//!    repository's own store (169 events): ZERO nodes reach the expensive path,
//!    because no event predates the provenance writer, so there is not one record
//!    to classify. Reasoning through the gates for a populated store:
//!
//!    * Every NON-LEAN green is free. Either its environment does not resolve (so
//!      it is `Unknown(EnvironmentUnresolved)` and is skipped before any cost), or
//!      it resolves and the system gate answers `Unavailable` without a prover.
//!    * Every LEAN green whose fingerprint still MATCHES is free, and in a stable
//!      environment that is all of them. This is the state a store is in almost
//!      all of the time, and in it the sweep costs exactly what the old
//!      flag-defaulted-off census cost: nothing.
//!    * After the environment moves (a `lake update`, a toolchain bump, a
//!      re-pointed project) the eligible set is exactly the Lean greens that carry
//!      a pin. Pins ARE published today (the Lean backend writes
//!      `elaborated_statement` for live, lexically-verified runs unless
//!      `THEOREMATA_LEAN_PUBLISH_ELABORATION` turns the probe off), so this set is
//!      the real cost, and it is what the cache and the bound below exist for.
//!      The FIRST sweep after a move pays min(eligible, bound); every later sweep
//!      in that same environment pays zero.
//!
//! 2. **A cache.** A re-elaboration is a pure function of (pinned form, pinned
//!    imports, current environment). The environment component is
//!    `checker_cache::EnvironmentFingerprint`, carried here verbatim as its
//!    `key_field()` string, so this introduces NO second notion of environment
//!    identity. An entry is written only for a RESOLVED environment, and only for
//!    an answer that was actually obtained; see [`CachedReelaboration`].
//!
//! 3. **A per-sweep bound.** [`SweepOptions::max_reelaborations`] caps how many
//!    nodes may spawn Lean in one run, oldest-recorded first. Past the cap an
//!    eligible node is handed an explicit `Unavailable`, which is
//!    `Unknown(ReelaborationUnavailable)`: bounded work costs recall, never a
//!    wrong verdict. A CLI flag may RAISE this bound. There is no flag that turns
//!    the discriminator on, because it is not off.
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
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use super::staleness::{
    self, ArtifactClass, EnvironmentFingerprint, RecheckScope, StalenessReport, VerifiedResult,
};
use crate::checker_cache::EnvironmentFingerprint as CheckerEnvironment;
use crate::config::Config;
use crate::db::Store;
use crate::prover::lean::{
    reelaborate_pinned_statement, LeanReelaboration, DEFAULT_REELABORATION_PREAMBLE,
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
// Options and bounds
// ===========================================================================

/// How many nodes one sweep may spawn Lean for, unless a caller raises it.
///
/// Chosen to be a number an operator can wait through: at a few seconds per node
/// against a real Mathlib this is a sweep measured in a minute or two, not one
/// measured in hours. It exists so an unbounded store cannot produce an unbounded
/// run; it is not a statement about how many nodes are worth checking.
pub const DEFAULT_MAX_REELABORATIONS: usize = 25;

/// File under the artifact root where settled re-elaborations are remembered.
const REELABORATION_CACHE_FILE: &str = "staleness-reelaborations.json";

/// Prefix `checker_cache::EnvironmentFingerprint::key_field` uses for a RESOLVED
/// environment. Matched defensively before anything is cached, so that even if a
/// future caller managed to hand this module an unresolved fingerprint, nothing
/// would be stored or served under it. Caching against an unresolved environment
/// is the stale-green bug this project already fixed once.
const RESOLVED_ENV_PREFIX: &str = "resolved:";

/// What a sweep is allowed to spend.
///
/// [`Default`] is a REAL sweep: the discriminator runs, bounded by
/// [`DEFAULT_MAX_REELABORATIONS`] and by the skip gates in
/// [`ReelaborationRun::attempt`]. There is no "off" here, because a
/// discriminator that defaults off is a discriminator nothing runs.
#[derive(Debug, Clone)]
pub struct SweepOptions {
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
    /// Ceiling on how many nodes may SPAWN LEAN in this sweep. A remembered answer
    /// does not draw on it, because replaying one costs nothing.
    ///
    /// Raising this is the only knob: a caller may ask for more work, never for
    /// less honesty. Zero is legal and means "classify from what is already known",
    /// which yields `Unknown(ReelaborationUnavailable)` for every eligible node
    /// with no cached answer, never `Fresh`.
    pub max_reelaborations: usize,
}

impl Default for SweepOptions {
    fn default() -> Self {
        Self {
            reelaboration_preamble: None,
            max_reelaborations: DEFAULT_MAX_REELABORATIONS,
        }
    }
}

/// A re-elaboration answer worth remembering.
///
/// Only the two ANSWERS are here. `Unavailable` is deliberately not
/// representable: it means we did not get an answer (no toolchain, a control
/// probe that failed, a name the context does not provide, a budget that ran
/// out), and remembering "we did not look" would freeze a transient failure into
/// a permanent `Unknown` and would make the budget refusal sticky across sweeps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome")]
enum CachedReelaboration {
    Elaborated { statement_type: String },
    Rejected { detail: String },
}

impl CachedReelaboration {
    fn into_outcome(self) -> staleness::ReelaborationOutcome {
        match self {
            CachedReelaboration::Elaborated { statement_type } => {
                staleness::ReelaborationOutcome::Elaborated { statement_type }
            }
            CachedReelaboration::Rejected { detail } => {
                staleness::ReelaborationOutcome::Rejected { detail }
            }
        }
    }
}

/// The cache key for one re-elaboration: the environment, the preamble and the
/// pinned form, length-framed so no two different triples can be re-split into
/// the same pre-image (the same anti-ambiguity framing `checker_cache` uses).
///
/// `environment` is `checker_cache::EnvironmentFingerprint::key_field()` carried
/// through unchanged. This module invents no identity of its own: if that field
/// ever stops distinguishing two environments, this cache stops distinguishing
/// them too, which is the correct coupling.
fn reelaboration_cache_key(environment: &str, preamble: &str, pinned: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for (tag, data) in [
        ("theoremata.staleness_reelaboration.v1", ""),
        ("environment", environment),
        ("preamble", preamble),
        ("pinned", pinned),
    ] {
        hasher.update((tag.len() as u64).to_be_bytes());
        hasher.update(tag.as_bytes());
        hasher.update((data.len() as u64).to_be_bytes());
        hasher.update(data.as_bytes());
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The budget and the memory a single sweep carries.
///
/// Holds no verdicts and no store handle: it decides only whether to pay for one
/// re-elaboration and what the answer was. Classification stays in `assess`.
struct ReelaborationRun {
    cache: BTreeMap<String, CachedReelaboration>,
    path: Option<std::path::PathBuf>,
    /// Nodes that may still be charged to the bound.
    remaining: usize,
    /// Nodes charged to the bound (an upper bound on Lean processes started).
    spawned: usize,
    replayed: usize,
    /// Eligible nodes the bound refused. Reported, because a sweep that hit its
    /// ceiling is a different fact from one that had nothing left to do.
    refused_at_bound: usize,
    learned: usize,
}

impl ReelaborationRun {
    /// Seed from the artifact root. Any IO or parse failure degrades to an empty
    /// cache: a cache is an optimization, and an optimization must never be able
    /// to change a verdict or fail a sweep.
    fn open(config: &Config, options: &SweepOptions) -> Self {
        let path = config.artifacts.join(REELABORATION_CACHE_FILE);
        let cache = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<BTreeMap<String, CachedReelaboration>>(&raw).ok())
            .unwrap_or_default();
        Self {
            cache,
            path: Some(path),
            remaining: options.max_reelaborations,
            spawned: 0,
            replayed: 0,
            refused_at_bound: 0,
            learned: 0,
        }
    }

    /// Run the Phase 1.2 discriminator for one node, or explain why it was not
    /// run.
    ///
    /// Returns the outcome to hand `assess` plus a human-readable note. `None`
    /// means NOT ATTEMPTED, which `assess` already turns into
    /// `Unknown(ReelaborationUnavailable)` when the fingerprint moved: not
    /// attempting is never a road to Fresh.
    fn attempt(
        &mut self,
        config: &Config,
        record: &ParsedProvenance,
        current: Option<&EnvironmentFingerprint>,
        options: &SweepOptions,
    ) -> Option<(staleness::ReelaborationOutcome, String)> {
        // An unresolved current environment is `Unknown(EnvironmentUnresolved)` no
        // matter what a re-elaboration says, so paying for one would buy nothing.
        let current = current?;
        // A node whose fingerprint still matches is Fresh and needs no answer; a
        // node with no pin has nothing to re-elaborate. Both are skipped for cost,
        // and both already land on the correct verdict without us.
        if record.verified_against.as_str() == current.as_str() {
            return None;
        }
        let pinned = record.pinned_statement_type.as_deref()?;
        // Only Lean publishes a pinned elaborated form and only Lean can
        // re-elaborate one. Any other system is honestly reported as having no
        // discriminator rather than being run through Lean's. Free: no prover.
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
        // A mock prover consults no library, so it can say nothing about whether
        // the library moved. Answered here as well as inside the backend so the
        // whole offline case is FREE: a machine with the prover pinned to mock,
        // sweeping a store of live-verified greens, would otherwise charge its
        // entire bound to a backend that was always going to decline.
        if config.prover_mock {
            return Some((
                staleness::ReelaborationOutcome::Unavailable {
                    reason: "prover is pinned to mock, which elaborates against no library"
                        .to_string(),
                },
                "skipped: mock prover has no library to re-elaborate against".to_string(),
            ));
        }

        let (preamble, preamble_source) = preamble_for(record, options);
        // Only a RESOLVED environment may back a cache entry. `current` is
        // `Some` only for a resolved one by construction (see
        // `resolve_current_environment`); the prefix check is the belt to that
        // brace, because a cache served against an unknown environment is the
        // stale green this project already paid for once.
        let key = current
            .as_str()
            .starts_with(RESOLVED_ENV_PREFIX)
            .then(|| reelaboration_cache_key(current.as_str(), &preamble, pinned));

        if let Some(hit) = key.as_ref().and_then(|k| self.cache.get(k)).cloned() {
            self.replayed += 1;
            return Some((
                hit.into_outcome(),
                format!(
                    "preamble from {preamble_source}; answer remembered from an earlier sweep in \
                     this exact environment"
                ),
            ));
        }

        if self.remaining == 0 {
            self.refused_at_bound += 1;
            return Some((
                staleness::ReelaborationOutcome::Unavailable {
                    reason: format!(
                        "this sweep's re-elaboration bound ({}) is spent; no answer was obtained \
                         for this node, which is not evidence about it",
                        options.max_reelaborations
                    ),
                },
                "skipped: sweep re-elaboration bound reached".to_string(),
            ));
        }
        self.remaining -= 1;
        self.spawned += 1;

        let outcome = match reelaborate_pinned_statement(config, &preamble, pinned) {
            LeanReelaboration::Elaborated { form } => {
                if let Some(key) = key {
                    self.cache.insert(
                        key,
                        CachedReelaboration::Elaborated {
                            statement_type: form.clone(),
                        },
                    );
                    self.learned += 1;
                }
                staleness::ReelaborationOutcome::Elaborated {
                    statement_type: form,
                }
            }
            LeanReelaboration::Rejected { detail } => {
                if let Some(key) = key {
                    self.cache.insert(
                        key,
                        CachedReelaboration::Rejected {
                            detail: detail.clone(),
                        },
                    );
                    self.learned += 1;
                }
                staleness::ReelaborationOutcome::Rejected { detail }
            }
            // Never cached: see `CachedReelaboration`.
            LeanReelaboration::Unavailable { reason } => {
                staleness::ReelaborationOutcome::Unavailable { reason }
            }
        };
        Some((outcome, format!("preamble from {preamble_source}")))
    }

    /// Persist anything newly learned. Best effort: a failed write costs the next
    /// sweep some Lean spawns, nothing more.
    fn close(&self) {
        if self.learned == 0 {
            return;
        }
        if let Some(path) = &self.path {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(raw) = serde_json::to_string(&self.cache) {
                let _ = std::fs::write(path, raw);
            }
        }
    }
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
    /// Whether an answer (from Lean or from the cache) was fed to `assess` for
    /// this node. False for every node the skip gates dropped, which is the
    /// overwhelming majority.
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
    /// True iff any node got a re-elaboration answer at all, so the census can
    /// never be mistaken for one backed by the discriminator when it was not.
    pub any_reelaborated: bool,
    /// How many nodes an answer was fed to `assess` for (spawned plus replayed
    /// plus the free "no re-elaborator for this system" ones).
    pub reelaborations_attempted: usize,
    /// How many nodes were CHARGED to the bound. A node is charged just before
    /// the backend is called, so a backend that declines cheaply (no toolchain on
    /// this machine) also consumes one; the number is therefore an upper bound on
    /// the Lean processes started, never an under-count.
    pub reelaborations_spawned: usize,
    /// How many answers came from the cache instead of from Lean. A second sweep
    /// in an unchanged environment should be all cache and no spawns.
    pub reelaborations_replayed: usize,
    /// Eligible nodes the per-sweep bound refused. Non-zero means the census is
    /// incomplete by design: those nodes are `Unknown`, never `Fresh`, and raising
    /// the bound is what finishes them.
    pub reelaborations_refused_at_bound: usize,
    /// The bound that was in force.
    pub max_reelaborations: usize,
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
///
/// This runs the Phase 1.2 discriminator, bounded by
/// [`DEFAULT_MAX_REELABORATIONS`] and by the skip gates. In a stable environment
/// that costs nothing at all, because no node's fingerprint has moved.
pub fn sweep(
    store: &Store,
    config: &Config,
    project: Option<&str>,
    limit: usize,
) -> Result<SweepOutcome> {
    sweep_with_options(store, config, project, limit, &SweepOptions::default())
}

/// [`sweep`] with the bounds exposed. `SweepOptions::default()` is exactly what
/// [`sweep`] runs; a caller may RAISE [`SweepOptions::max_reelaborations`] or
/// supply a preamble, and can do nothing else.
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

    // Phase 1.2, run as its own pass so the ORDER the budget is spent in is a
    // decision rather than an accident of the classification loop. `records` is
    // newest-first (the store returns events DESC), so walking it in reverse
    // spends on the OLDEST-RECORDED greens first: those have had the longest for
    // the library to move underneath them, and if the bound bites they are the
    // ones worth the Lean spawns. The order is observable only when the bound
    // bites; every other node is unaffected.
    let mut run = ReelaborationRun::open(config, options);
    let mut attempts: Vec<Option<(staleness::ReelaborationOutcome, String)>> =
        vec![None; records.len()];
    for index in (0..records.len()).rev() {
        let current = env_cache
            .get(&records[index].system)
            .and_then(|c| c.fingerprint.as_ref());
        attempts[index] = run.attempt(config, &records[index], current, options);
    }
    run.close();

    // Classify. `StalenessReport` is the existing census model; we feed it the same
    // verdicts we render, so the counts and the per-node list can never disagree.
    let mut report = StalenessReport::new();
    let mut nodes: Vec<NodeVerdict> = Vec::new();
    let mut any_reelaborated = false;
    let mut reelaborations_attempted = 0usize;

    for (index, record) in records.iter().enumerate() {
        let verified = record.to_verified_result();
        let current = env_cache
            .get(&record.system)
            .and_then(|c| c.fingerprint.as_ref());

        // `None` (not attempted) still means `assess` returns
        // `Unknown(ReelaborationUnavailable)` for a moved fingerprint, so a node
        // the skip gates or the bound dropped can never read as Fresh.
        let attempt = &attempts[index];
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
        reelaborations_attempted,
        reelaborations_spawned: run.spawned,
        reelaborations_replayed: run.replayed,
        reelaborations_refused_at_bound: run.refused_at_bound,
        max_reelaborations: options.max_reelaborations,
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

    /// A run with no disk behind it, so a test never reads or writes a cache
    /// file and never depends on one left behind by another test.
    fn detached_run(budget: usize) -> ReelaborationRun {
        ReelaborationRun {
            cache: BTreeMap::new(),
            path: None,
            remaining: budget,
            spawned: 0,
            replayed: 0,
            refused_at_bound: 0,
            learned: 0,
        }
    }

    #[test]
    fn the_skip_gates_are_what_keeps_a_sweep_cheap() {
        // The cost control is now structural, so it must hold with a FULL budget:
        // a node whose answer could not change its verdict never reaches Lean.
        let opts = SweepOptions::default();
        assert!(opts.max_reelaborations > 0, "the discriminator is not off");
        let config = Config::default();
        let current = EnvironmentFingerprint::new("resolved:lake:new");
        let mut run = detached_run(opts.max_reelaborations);

        // Environment unresolved: `assess` is Unknown(EnvironmentUnresolved)
        // regardless, so no prover time is spent.
        assert!(run
            .attempt(
                &config,
                &parsed("lean", "resolved:lake:old", Some("T")),
                None,
                &opts
            )
            .is_none());
        // Fingerprint still matches: the node is Fresh without us.
        assert!(run
            .attempt(
                &config,
                &parsed("lean", "resolved:lake:new", Some("T")),
                Some(&current),
                &opts
            )
            .is_none());
        // No pin: nothing to re-elaborate, and `assess` says NoPinnedStatementType.
        assert!(run
            .attempt(
                &config,
                &parsed("lean", "resolved:lake:old", None),
                Some(&current),
                &opts
            )
            .is_none());
        // Not one of those three spent a spawn.
        assert_eq!(run.spawned, 0);
        assert_eq!(run.remaining, opts.max_reelaborations);
    }

    /// The bound is a bound on WORK, and hitting it produces silence about the
    /// node, never a verdict about it.
    #[test]
    fn the_bound_refuses_rather_than_guesses() {
        let opts = SweepOptions {
            reelaboration_preamble: None,
            max_reelaborations: 0,
        };
        let config = Config::default();
        let current = EnvironmentFingerprint::new("resolved:lake:new");
        let mut run = detached_run(0);
        let record = parsed("lean", "resolved:lake:old", Some("T"));

        let (outcome, note) = run
            .attempt(&config, &record, Some(&current), &opts)
            .expect("an explicit outcome, not silence");
        assert!(matches!(
            outcome,
            staleness::ReelaborationOutcome::Unavailable { .. }
        ));
        assert!(note.contains("bound"));
        assert_eq!(run.spawned, 0);
        assert_eq!(run.refused_at_bound, 1);

        // And the verdict it produces is Unknown: not Fresh, not a withdrawal.
        let verdict = staleness::assess(&record.to_verified_result(), Some(&current), Some(&outcome));
        assert!(!verdict.is_fresh());
        assert!(verdict.withdrawal().is_none());
    }

    /// A remembered answer is replayed, costs no spawn, and is exactly the answer
    /// that was stored. Unresolved environments cannot participate at all.
    #[test]
    fn a_remembered_answer_replaces_a_spawn_and_only_for_a_resolved_environment() {
        let opts = SweepOptions::default();
        let config = Config::default();
        let record = parsed("lean", "resolved:lake:old", Some("T"));
        let current = EnvironmentFingerprint::new("resolved:lake:new");
        let (preamble, _) = preamble_for(&record, &opts);

        let mut run = detached_run(0); // zero budget: only a cache hit can answer
        run.cache.insert(
            reelaboration_cache_key(current.as_str(), &preamble, "T"),
            CachedReelaboration::Elaborated {
                statement_type: "T".to_string(),
            },
        );
        let (outcome, note) = run
            .attempt(&config, &record, Some(&current), &opts)
            .expect("the remembered answer");
        assert_eq!(
            outcome,
            staleness::ReelaborationOutcome::Elaborated {
                statement_type: "T".to_string()
            }
        );
        assert!(note.contains("remembered"));
        assert_eq!(run.replayed, 1);
        assert_eq!(run.spawned, 0);
        assert_eq!(run.refused_at_bound, 0);

        // The same pin under a DIFFERENT environment is a different key, so the
        // remembered answer cannot be served for it: it falls through to the
        // budget, which is spent, and answers Unavailable.
        let moved_again = EnvironmentFingerprint::new("resolved:lake:newer");
        let (fallthrough, _) = run
            .attempt(&config, &record, Some(&moved_again), &opts)
            .expect("an explicit outcome");
        assert!(matches!(
            fallthrough,
            staleness::ReelaborationOutcome::Unavailable { .. }
        ));

        // An unresolved environment key is never even formed: the guard is the
        // prefix check, and its absence is what caching a stale green looks like.
        assert!(!"unresolved: no lake project".starts_with(RESOLVED_ENV_PREFIX));
        assert!("resolved:lake:abcd".starts_with(RESOLVED_ENV_PREFIX));
    }

    /// The key is a pure function of the three inputs and separates all of them.
    #[test]
    fn the_cache_key_separates_environment_preamble_and_pin() {
        let base = reelaboration_cache_key("resolved:lake:a", "import Mathlib", "T");
        assert_eq!(base, reelaboration_cache_key("resolved:lake:a", "import Mathlib", "T"));
        assert_eq!(base.len(), 64);
        for other in [
            reelaboration_cache_key("resolved:lake:b", "import Mathlib", "T"),
            reelaboration_cache_key("resolved:lake:a", "import Mathlib.Order.Basic", "T"),
            reelaboration_cache_key("resolved:lake:a", "import Mathlib", "U"),
            // Length framing: no re-split of the same bytes can collide.
            reelaboration_cache_key("resolved:lake:aimport", " Mathlib", "T"),
        ] {
            assert_ne!(base, other);
        }
    }

    #[test]
    fn a_system_with_no_re_elaborator_is_unavailable_and_never_moved() {
        let opts = SweepOptions::default();
        let config = Config::default();
        let current = EnvironmentFingerprint::new("resolved:x:new");
        let mut run = detached_run(opts.max_reelaborations);
        let (outcome, _note) = run
            .attempt(
                &config,
                &parsed("rocq", "resolved:x:old", Some("T")),
                Some(&current),
                &opts,
            )
            .expect("an explicit outcome, not silence");
        // Free: an unsupported system never draws on the budget.
        assert_eq!(run.spawned, 0);
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
            reelaboration_preamble: Some("import Mathlib.Order.Basic".to_string()),
            ..SweepOptions::default()
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
