//! Local verification gate for external-prover output (trust-but-verify).

use crate::{
    config::Config,
    db::Store,
    hardening::{self, HardeningOutcome, HardeningReport},
    prover::{model::VerificationReport, statement_guard},
    tools::{PythonCheck, Tool},
};
use anyhow::Result;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Stable content hash for provenance payloads.
///
/// Lives here rather than in each backend because all three external-prover
/// backends already depend on this module and all three need the SAME hash: a
/// provenance record is only useful if two backends hashing the same Lean agree.
pub fn provenance_hash(text: &str) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(text.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Everything the real [`hardening::harden`] step needs and that a bare
/// `(config, source)` pair cannot supply: a `Store` to write the evidence row
/// into, and the graph coordinates that row is filed under.
///
/// This exists because hardening is not a pure function of the source text. It
/// scaffolds a Lake workspace, builds the module, runs LeanParanoia, and records
/// an evidence row, so a caller that wants the deep battery on external-prover
/// output has to hand us the store. Callers that have no store still get an
/// honest report saying the requested check could not run, never a clean one.
pub struct HardeningContext<'a> {
    pub store: &'a Store,
    pub project_id: &'a str,
    pub node_id: &'a str,
}

/// The three states a hardening layer can be in. Kept explicit (rather than a
/// bare `Option<bool>`) because collapsing "off by config" into "failed" would
/// reject honest proofs, and collapsing "could not run" into "not applicable"
/// would let an unaudited external-prover proof read as fully checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HardeningState {
    /// `harden_proofs` is off. Nothing was asked for, so nothing failed.
    NotRequested,
    /// `harden_proofs` is on but the battery could not be executed (no store,
    /// no workspace, no toolchain, launch error). A requested check that did not
    /// complete is a failure of that check.
    CouldNotRun,
    /// LeanParanoia (or one of its preconditions) produced an actual verdict.
    Ran,
}

impl HardeningState {
    fn as_str(self) -> &'static str {
        match self {
            Self::NotRequested => "not_requested",
            Self::CouldNotRun => "requested_but_could_not_run",
            Self::Ran => "ran",
        }
    }
}

/// Map a completed [`HardeningReport`] onto our three-state view.
///
/// `HardeningReport::ran == false` means the battery never executed (skipped
/// preconditions, scaffold/place failure), which is `CouldNotRun` here even
/// though `harden` itself returned `Ok`: from this call site's perspective a
/// requested check did not complete. Only `ran == true` yields a verdict, and
/// only `HardeningOutcome::Passed` is clean (`report.clean` already encodes
/// that fail-closed rule).
fn state_of(report: &HardeningReport) -> (HardeningState, Option<bool>) {
    if report.ran {
        (HardeningState::Ran, Some(report.clean))
    } else {
        // Not clean: an unexecuted requested check must never report clean.
        (HardeningState::CouldNotRun, Some(false))
    }
}

pub fn verify_lean_output(
    config: &Config,
    lean_code: &str,
    expected_statement: &str,
) -> Result<VerificationReport> {
    verify_lean_round_trip(config, expected_statement, lean_code, expected_statement)
}

/// As [`verify_lean_output`], but with the graph handle the deep hardening step
/// needs. Prefer this from any call site that has a `Store`; the storeless entry
/// points cannot run hardening and say so in the report.
pub fn verify_lean_output_hardened(
    ctx: &HardeningContext<'_>,
    config: &Config,
    lean_code: &str,
    expected_statement: &str,
) -> Result<VerificationReport> {
    verify_lean_round_trip_inner(
        Some(ctx),
        config,
        expected_statement,
        lean_code,
        expected_statement,
    )
}

pub fn verify_lean_round_trip(
    config: &Config,
    before_src: &str,
    after_src: &str,
    expected_statement: &str,
) -> Result<VerificationReport> {
    verify_lean_round_trip_inner(None, config, before_src, after_src, expected_statement)
}

/// As [`verify_lean_round_trip`], with a store so hardening can actually run.
pub fn verify_lean_round_trip_hardened(
    ctx: &HardeningContext<'_>,
    config: &Config,
    before_src: &str,
    after_src: &str,
    expected_statement: &str,
) -> Result<VerificationReport> {
    verify_lean_round_trip_inner(Some(ctx), config, before_src, after_src, expected_statement)
}

fn verify_lean_round_trip_inner(
    ctx: Option<&HardeningContext<'_>>,
    config: &Config,
    before_src: &str,
    after_src: &str,
    expected_statement: &str,
) -> Result<VerificationReport> {
    let guard = statement_guard::guard_lean_round_trip(before_src, after_src);
    // Statement-guard RESTORE (open-atp / Numina): when a header drifted or was
    // deleted, compute the restored-to-snapshot source rather than only
    // rejecting, so a caller can recover the original statement.
    let restore = if guard.preserved {
        None
    } else {
        Some(statement_guard::restore_statements(before_src, after_src))
    };
    let py = PythonCheck::new();
    let lexical = if py.available() {
        let resp = py.run(json!({"tool": "lean_soundness", "text": after_src}))?;
        let parsed: serde_json::Value =
            serde_json::from_str(&resp.stdout).unwrap_or(json!({"ok": false}));
        // The worker envelope is `{"ok": bool, "output": <payload>}`. Reading
        // top-level `ok` alone (as this did) only asked "did the worker run",
        // never "is the source clean" -- so the pre-screen passed on any
        // successful invocation regardless of what the scan found. Require BOTH:
        // the call succeeded, and the payload's verdict is clean.
        // `lean_soundness` returns `pregate_clean`, with `clean` as a deprecated
        // alias. Absent or unparseable still means false (fail closed).
        let call_ok = parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        let scan_clean = parsed
            .get("output")
            .and_then(|o| o.get("pregate_clean").or_else(|| o.get("clean")))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        call_ok && scan_clean
    } else {
        false
    };

    let low = after_src.to_lowercase();
    let axioms_clean = !low.contains("sorry") && !low.contains("admit");
    let exp_norm: String = expected_statement.split_whitespace().collect();
    let code_norm: String = after_src.split_whitespace().collect();
    let statement_preserved = guard.preserved
        && !exp_norm.is_empty()
        && (code_norm.contains(&exp_norm)
            || expected_statement
                .split(':')
                .next()
                .map(|s| code_norm.contains(&s.split_whitespace().collect::<String>()))
                .unwrap_or(false));

    // Deep hardening (LeanParanoia). Gated exactly like the certify path in
    // `agent.rs`: opt-in via `harden_proofs`, which stays OFF by default because
    // the first workspace build resolves Mathlib's git deps over the network.
    // The switch only decides whether the check is REQUESTED; it never decides
    // the verdict.
    let (hardening_state, hardening_clean, hardening_detail) = run_hardening(ctx, config, after_src);

    // Only `Flagged` is a real soundness failure (same rule as the certify path:
    // Inconclusive/Unavailable/Skipped mean "not audited", not "unsound"), so
    // only a flag is conjoined here. Without this, a proof LeanParanoia actively
    // rejected would still be reported as passing the pre-screen.
    let hardening_flagged = hardening_detail
        .get("outcome")
        .and_then(Value::as_str)
        .is_some_and(|o| o == "flagged");

    // Lexical pre-screen only — the real compile happens at the certify step.
    let lexically_verified = lexical && axioms_clean && !hardening_flagged;

    let report = VerificationReport {
        lexically_verified,
        axioms_clean,
        statement_preserved,
        lexical_clean: lexical,
        hardening_clean,
        // A lexical pre-screen of external-prover output — NOT a live compile.
        // The authoritative certify step does the real toolchain check, so this
        // report must never itself count as a live formal certification.
        live: false,
        detail: json!({
            "expected_statement": expected_statement,
            "hardening_enabled": config.harden_proofs,
            // ALWAYS present, in all three states. An omitted field reads as
            // "not applicable", which would overstate how audited an
            // external-prover result is.
            "hardening": {
                "state": hardening_state.as_str(),
                "clean": hardening_clean,
                "report": hardening_detail,
            },
            "statement_guard": statement_guard::guard_report_json(&guard),
            "statement_restore": restore,
        }),
    };

    // Audit trail: record that the local re-check of external-producer output
    // actually ran, and what it saw.
    //
    // Only the `ctx` path writes. The storeless entry points have no store and,
    // more importantly, no graph coordinates: an evidence row hangs off a node,
    // so filing one without a node id (or against a fabricated one) would put a
    // real audit record on a node that does not exist. Writing nothing is the
    // honest outcome there, and it is also what those callers did before.
    if let Some(ctx) = ctx {
        record_producer_checked(ctx, &report)?;
    }
    Ok(report)
}

/// File the `external_producer_checked` row for one completed local re-check.
///
/// This is a RECORD that the trust-but-verify step ran, not a certification.
/// The verdict is deliberately scoped to the lexical pre-screen, because that is
/// all this function computes: `report.live` is always false here and the
/// authoritative compile happens later at the certify step. A verdict of
/// "verified"/"passed" would read as a gate result that was never taken.
fn record_producer_checked(ctx: &HardeningContext<'_>, report: &VerificationReport) -> Result<()> {
    // `HardeningContext` makes both ids non-optional, but an empty string is not
    // a node id. Treat it exactly like an absent id: no node to attach to, so no
    // row, rather than a row filed against nothing.
    if ctx.project_id.is_empty() || ctx.node_id.is_empty() {
        return Ok(());
    }
    let verdict = if report.lexically_verified {
        "lexical_screen_clean"
    } else {
        "lexical_screen_flagged"
    };
    ctx.store.add_evidence(
        ctx.project_id,
        ctx.node_id,
        // Written as a literal, not as `evidence::EXTERNAL_PRODUCER_CHECKED`,
        // because the drift guard in `components/graph/evidence.rs` only counts
        // a statically visible `kind` argument. The test below pins the literal
        // to the declared constant so the two cannot drift apart.
        "external_producer_checked",
        "prover_verify",
        verdict,
        json!({
            "lexically_verified": report.lexically_verified,
            "axioms_clean": report.axioms_clean,
            "statement_preserved": report.statement_preserved,
            "lexical_clean": report.lexical_clean,
            "hardening_clean": report.hardening_clean,
            "hardening": report.detail.get("hardening"),
            "live": report.live,
            "note": "local re-check of external-producer output: lexical pre-screen \
                     plus statement guard, NOT a live compile",
        }),
    )?;
    Ok(())
}

/// Resolve the hardening layer into (state, `hardening_clean`, detail).
///
/// Split out of the main body so each of the three states is reachable from a
/// test without a Lean toolchain.
fn run_hardening(
    ctx: Option<&HardeningContext<'_>>,
    config: &Config,
    source: &str,
) -> (HardeningState, Option<bool>, Value) {
    if !config.harden_proofs {
        // NOT a failure. A check nobody asked for cannot reject a proof, so
        // `hardening_clean` stays `None` (the codebase's "no verdict" value)
        // while the detail block still records that it did not run.
        return (
            HardeningState::NotRequested,
            None,
            json!({"reason": "harden_proofs disabled; hardening not requested"}),
        );
    }
    let Some(ctx) = ctx else {
        // Hardening was requested and we cannot even attempt it: `harden` needs
        // a `Store` to file its evidence row, which the storeless entry points
        // do not have. Requested-and-not-completed is a failure of the check.
        return (
            HardeningState::CouldNotRun,
            Some(false),
            json!({
                "reason": "no HardeningContext: this call site has no Store, so \
                           hardening could not be attempted",
                "remedy": "call verify_lean_output_hardened / \
                           verify_lean_round_trip_hardened with a HardeningContext",
            }),
        );
    };
    // Same module-name derivation as the certify path so repeat runs on a node
    // reuse one workspace module instead of accumulating one per attempt.
    let module = format!(
        "N{}",
        ctx.node_id.replace('-', "").get(0..8).unwrap_or("node")
    );
    match hardening::harden(
        ctx.store,
        config,
        ctx.project_id,
        ctx.node_id,
        &module,
        source,
    ) {
        Ok(report) => {
            let (state, clean) = state_of(&report);
            let outcome = outcome_str(report.outcome);
            (
                state,
                clean,
                json!({
                    "outcome": outcome,
                    "summary": report.summary,
                    "details": report.details,
                }),
            )
        }
        // An error is a requested check that did not complete, never a pass.
        Err(e) => (
            HardeningState::CouldNotRun,
            Some(false),
            json!({"reason": "hardening returned an error", "error": e.to_string()}),
        ),
    }
}

fn outcome_str(outcome: HardeningOutcome) -> &'static str {
    match outcome {
        HardeningOutcome::Passed => "passed",
        HardeningOutcome::Flagged => "flagged",
        HardeningOutcome::Inconclusive => "inconclusive",
        HardeningOutcome::Unavailable => "unavailable",
        HardeningOutcome::BuildFailed => "build_failed",
        HardeningOutcome::Skipped => "skipped",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NodeKind;
    use std::path::Path;

    const PROOF: &str = "theorem t : True := trivial\n";

    fn disabled() -> Config {
        let mut config = Config::default();
        config.harden_proofs = false;
        config.lean_project = None;
        config
    }

    fn enabled() -> Config {
        let mut config = disabled();
        config.harden_proofs = true;
        config
    }

    fn report_detail(report: &VerificationReport) -> &Value {
        report
            .detail
            .get("hardening")
            .expect("the hardening block is always present, in every state")
    }

    // --- state 1: not requested ------------------------------------------

    #[test]
    fn hardening_disabled_is_not_a_failure() {
        let report = verify_lean_output(&disabled(), PROOF, "theorem t : True").unwrap();
        assert_eq!(
            report.hardening_clean, None,
            "a check nobody asked for must not reject the proof"
        );
        assert_eq!(
            report_detail(&report).get("state").and_then(Value::as_str),
            Some("not_requested")
        );
    }

    #[test]
    fn disabled_hardening_does_not_drag_down_the_lexical_verdict() {
        let with = verify_lean_output(&disabled(), PROOF, "theorem t : True").unwrap();
        // Whatever the lexical screen decides offline, hardening being off must
        // not be the thing that changed it.
        assert_eq!(with.lexically_verified, with.lexical_clean && with.axioms_clean);
    }

    // --- state 2: requested but could not run -----------------------------

    #[test]
    fn enabled_hardening_without_a_store_reports_could_not_run() {
        let report = verify_lean_output(&enabled(), PROOF, "theorem t : True").unwrap();
        assert_eq!(
            report.hardening_clean,
            Some(false),
            "a requested check that never ran must not report clean"
        );
        assert_eq!(
            report_detail(&report).get("state").and_then(Value::as_str),
            Some("requested_but_could_not_run")
        );
    }

    #[test]
    fn enabled_hardening_without_a_workspace_reports_could_not_run() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "t").unwrap();
        let node = store
            .add_node(&project.id, NodeKind::FormalStatement, "f", "s", "test")
            .unwrap();
        let ctx = HardeningContext {
            store: &store,
            project_id: &project.id,
            node_id: &node.id,
        };
        // `lean_project` is None, so `harden` skips its preconditions.
        let report =
            verify_lean_output_hardened(&ctx, &enabled(), PROOF, "theorem t : True").unwrap();
        assert_eq!(report.hardening_clean, Some(false));
        assert_eq!(
            report_detail(&report).get("state").and_then(Value::as_str),
            Some("requested_but_could_not_run")
        );
    }

    // --- state 3: ran, with a verdict -------------------------------------

    #[test]
    fn a_passed_run_is_the_only_clean_verdict() {
        let passed = HardeningReport {
            ran: true,
            clean: true,
            outcome: HardeningOutcome::Passed,
            summary: "passed".into(),
            details: Value::Null,
        };
        assert_eq!(state_of(&passed), (HardeningState::Ran, Some(true)));

        for outcome in [
            HardeningOutcome::Flagged,
            HardeningOutcome::Inconclusive,
            HardeningOutcome::Unavailable,
            HardeningOutcome::BuildFailed,
        ] {
            let ran = HardeningReport {
                ran: true,
                clean: false,
                outcome,
                summary: "not passed".into(),
                details: Value::Null,
            };
            assert_eq!(
                state_of(&ran),
                (HardeningState::Ran, Some(false)),
                "{outcome:?} ran but is not clean"
            );
        }
    }

    #[test]
    fn an_unexecuted_run_is_could_not_run_even_when_harden_returns_ok() {
        // `harden` returns Ok(Skipped) when a precondition is unmet. From this
        // call site that is still a requested check that did not complete.
        let skipped = HardeningReport {
            ran: false,
            clean: false,
            outcome: HardeningOutcome::Skipped,
            summary: "no Mathlib".into(),
            details: Value::Null,
        };
        assert_eq!(state_of(&skipped), (HardeningState::CouldNotRun, Some(false)));
    }

    // --- the invariant this whole path exists to protect -------------------

    #[test]
    fn external_prover_output_never_reports_hardening_clean_when_it_did_not_run() {
        // A proof that sails through every cheap layer: no sorry, no admit, and
        // the expected statement verbatim. The lexical gates cannot see that
        // LeanParanoia never looked at it, so the report must.
        let clean_looking = "theorem t : True := trivial\n";
        for config in [disabled(), enabled()] {
            let report = verify_lean_output(&config, clean_looking, "theorem t : True").unwrap();
            assert_ne!(
                report.hardening_clean,
                Some(true),
                "hardening did not run here, so it can never be reported clean"
            );
            let state = report_detail(&report)
                .get("state")
                .and_then(Value::as_str)
                .unwrap();
            assert_ne!(state, "ran", "nothing ran without a toolchain");
            assert!(!report.live, "a lexical pre-screen is never a live proof");
        }
    }

    #[test]
    fn round_trip_entry_point_reports_hardening_too() {
        let report =
            verify_lean_round_trip(&enabled(), PROOF, PROOF, "theorem t : True").unwrap();
        assert_eq!(
            report_detail(&report).get("state").and_then(Value::as_str),
            Some("requested_but_could_not_run")
        );
    }

    // --- the external_producer_checked audit row -------------------------

    fn node(store: &Store) -> (String, String) {
        let project = store.create_project("p", "t").unwrap();
        let node = store
            .add_node(&project.id, NodeKind::FormalStatement, "f", "s", "test")
            .unwrap();
        (project.id, node.id)
    }

    #[test]
    fn a_row_is_written_when_both_graph_ids_are_present() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let (project_id, node_id) = node(&store);
        let ctx = HardeningContext {
            store: &store,
            project_id: &project_id,
            node_id: &node_id,
        };
        let report =
            verify_lean_output_hardened(&ctx, &disabled(), PROOF, "theorem t : True").unwrap();
        let rows = store
            .evidence_of_kind(&project_id, &node_id, "external_producer_checked")
            .unwrap();
        assert_eq!(rows.len(), 1, "the re-check that ran must be recorded once");
        assert_eq!(rows[0].source, "prover_verify");
        assert_eq!(
            rows[0].payload.get("live").and_then(Value::as_bool),
            Some(false),
            "the payload must keep saying this was not a live compile"
        );
        // The verdict names the pre-screen it came from, so it cannot be read as
        // a certification that never happened.
        assert!(
            rows[0].verdict.starts_with("lexical_screen_"),
            "verdict {:?} must stay scoped to the lexical screen",
            rows[0].verdict
        );
        assert_eq!(
            rows[0]
                .payload
                .get("lexically_verified")
                .and_then(Value::as_bool),
            Some(report.lexically_verified),
            "the row must report what the screen actually decided"
        );
    }

    #[test]
    fn an_empty_id_writes_no_row_and_does_not_panic() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let (project_id, node_id) = node(&store);
        // An empty id is not a node. Both halves are required together, so each
        // of these must behave exactly like the storeless path: no row.
        for (p, n) in [
            ("", node_id.as_str()),
            (project_id.as_str(), ""),
            ("", ""),
        ] {
            let ctx = HardeningContext {
                store: &store,
                project_id: p,
                node_id: n,
            };
            let report =
                verify_lean_output_hardened(&ctx, &disabled(), PROOF, "theorem t : True").unwrap();
            assert!(!report.live);
        }
        assert!(
            store.project_evidence(&project_id).unwrap().is_empty(),
            "a missing id must never be papered over with a placeholder row"
        );
    }

    #[test]
    fn the_storeless_paths_still_write_nothing() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let (project_id, _node_id) = node(&store);
        // These entry points have no store at all; they must stay silent rather
        // than file a weaker row.
        verify_lean_output(&disabled(), PROOF, "theorem t : True").unwrap();
        verify_lean_output(&enabled(), PROOF, "theorem t : True").unwrap();
        verify_lean_round_trip(&enabled(), PROOF, PROOF, "theorem t : True").unwrap();
        assert!(store.project_evidence(&project_id).unwrap().is_empty());
    }

    #[test]
    fn the_emitted_kind_matches_the_declared_constant() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let (project_id, node_id) = node(&store);
        let ctx = HardeningContext {
            store: &store,
            project_id: &project_id,
            node_id: &node_id,
        };
        verify_lean_output_hardened(&ctx, &disabled(), PROOF, "theorem t : True").unwrap();
        let rows = store.evidence(&project_id, &node_id).unwrap();
        assert_eq!(
            rows.iter()
                .filter(|e| e.evidence_type
                    == crate::graph::evidence::EXTERNAL_PRODUCER_CHECKED)
                .count(),
            1,
            "the literal `kind` and the registry constant must not drift apart"
        );
    }

    #[test]
    fn outcome_names_are_stable() {
        assert_eq!(outcome_str(HardeningOutcome::Passed), "passed");
        assert_eq!(outcome_str(HardeningOutcome::Flagged), "flagged");
        assert_eq!(outcome_str(HardeningOutcome::Skipped), "skipped");
    }
}
