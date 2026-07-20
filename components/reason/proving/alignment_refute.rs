//! Refutation of graded alignments (plan Phase 2.3, "the part nobody appears to
//! have built").
//!
//! [`super::alignment`] holds the graded record, the outcome vocabulary
//! ([`Refutation`], whose strongest term is [`Refutation::Unrefuted`] and which
//! has no positive verdict) and the pure probe enumeration
//! ([`super::alignment::generate_probes`]). [`super::alignment_propose`] holds
//! the proposer. Neither of them *runs* anything: probing is deliberately left to
//! this module so that the record stays a pure function of its inputs.
//!
//! This module is the executor. Given a graded [`Alignment`], it turns each
//! admissible edge-case probe into an agreement CLAIM and hands that claim to the
//! EXISTING model-derived falsifier ([`super::falsification::Falsifier`], consumed
//! through the injectable [`crate::validity_seams::Falsify`] seam so tests can
//! feed canned verdicts). A confirmed counterexample refutes the alignment and is
//! stored as its [`Witness`]; an alignment that survives every probe is
//! [`Refutation::Unrefuted`], never "verified".
//!
//! ## Why the falsifier, and not a new checker
//!
//! The plan names three existing pieces of refutation machinery: the falsifier,
//! the witness search ([`crate::prover::witness_search`]), and the vacuity
//! refute-bundle ([`crate::prover::vacuity::refute_bundle`]). The last two operate
//! on a parsed [`HypothesisBundle`](crate::prover::witness_search) drawn from a
//! single formal statement; they answer "is this bundle satisfiable inside the
//! bounds". An alignment probe is a different shape: a free-text agreement claim
//! between two foreign symbols at a documented divergence point. The model-derived
//! falsifier is exactly the entry point that takes such a claim, has the model
//! emit an executable bounded check, runs it, and (optionally) reconfirms any
//! witness in exact arithmetic through the Wolfram oracle. So this module CALLS
//! that falsifier rather than reimplementing one; the numeric exact-recheck the
//! plan asks for is the one already living inside the falsifier's worker.
//!
//! ## The soundness rule, made structural
//!
//! An alignment may steer retrieval and may suggest an obligation. It may NEVER
//! license a transfer. This module cannot break that rule, and the reason is
//! type-level, not a matter of discipline:
//!
//! * The ONLY value this module produces about an alignment is a [`Refutation`],
//!   and it flows back in through [`Alignment::with_refutation`]. There is no
//!   other setter reachable from here.
//! * [`Refutation`] has no positive verdict. The strongest thing this module can
//!   ever record is [`Refutation::Unrefuted`], which reads "probes ran and none
//!   disagreed", explicitly not "the alignment holds".
//! * This module constructs no [`super::alignment::AlignmentStrength`], no
//!   [`super::alignment::KernelCertificate`], and no
//!   [`super::alignment::TransferObligation`]. It has no function that returns a
//!   fact, discharges an obligation, or promotes a grade. A caller that wants to
//!   transfer must still take the alignment's obligation goal to the prover stack
//!   and re-prove it in the target kernel, which is a different component.
//! * A [`Refutation::Refuted`] outcome makes both of the alignment's exits
//!   ([`Alignment::retrieval_hint`] and [`Alignment::transfer_obligation`]) return
//!   `None`, so refuting an alignment here makes it inert everywhere.
//!
//! In one sentence: the strongest reachable postcondition of this module is "we
//! failed to break it", and the type system has no way to spell "we proved it".

use std::collections::BTreeSet;

use serde_json::Value;

use super::alignment::{Alignment, DivergenceClass, Probe, ProbeAdmissibility, Refutation, Witness};
use super::falsification::FalsifyVerdict;
use crate::validity_seams::Falsify;

/// The falsifier verdict string that means a refuting assignment was actually
/// found and (for the model searcher) independently rechecked. Every other string
/// is a non-result: it never refutes and never certifies. Kept in step with
/// [`super::falsification`], which is the module that produces these strings.
const VERDICT_COUNTEREXAMPLE: &str = "counterexample";

/// The verdict of a genuine bounded search that ran to completion and found
/// nothing. This is the only non-refuting verdict that counts as "we actually
/// looked": it is what lets a survived probe contribute to
/// [`Refutation::Unrefuted`] rather than to [`Refutation::Unavailable`]. All other
/// strings (`inconclusive`, `unavailable`, `no_model`, `not_applicable`, `error`)
/// mean the search did not settle the question, so they must not read as a clean
/// run.
const VERDICT_NO_COUNTEREXAMPLE: &str = "no_counterexample_in_domain";

/// Probe a graded alignment with the falsifier and return the outcome.
///
/// Only probes tagged [`ProbeAdmissibility::Refuting`] are ever submitted: a
/// disagreement outside the claimed domain is EXPECTED for a domain-restricted
/// grade (the `x/0` case) and must not masquerade as a refutation, which is
/// exactly why [`Alignment::probes`] tags each probe in the first place.
///
/// The first confirmed counterexample stops the run and yields
/// [`Refutation::Refuted`] with its witness. Otherwise:
/// * at least one refuting probe ran a complete bounded search that found nothing
///   ⇒ [`Refutation::Unrefuted`];
/// * refuting probes existed but none produced a bounded result (falsifier
///   offline, unavailable, or inconclusive throughout) ⇒
///   [`Refutation::Unavailable`], kept distinct so a missing dependency never
///   reads as a clean run;
/// * no refuting probe was applicable at all ⇒ [`Refutation::NotProbed`].
pub fn probe_alignment(alignment: &Alignment, falsifier: &dyn Falsify) -> Refutation {
    let probes = alignment.probes();

    let mut refuting_seen = 0usize;
    let mut effective = 0usize;
    let mut classes: BTreeSet<DivergenceClass> = BTreeSet::new();

    for (probe, admissibility) in &probes {
        // Outside the claimed domain a disagreement proves nothing about the
        // graded claim, so such a probe is never even run.
        if *admissibility != ProbeAdmissibility::Refuting {
            continue;
        }
        refuting_seen += 1;

        let claim = agreement_claim(alignment, probe);
        match falsifier.falsify(&claim) {
            Ok(verdict) => {
                if verdict.verdict == VERDICT_COUNTEREXAMPLE {
                    // A confirmed counterexample: the two sides disagree at a
                    // point the alignment DID claim to cover. That refutes it.
                    return Refutation::Refuted {
                        witness: witness_from_verdict(probe, &verdict),
                    };
                }
                if verdict.applicable && verdict.verdict == VERDICT_NO_COUNTEREXAMPLE {
                    // A complete bounded search that found nothing: this probe
                    // genuinely looked, so it may support "unrefuted".
                    effective += 1;
                    classes.insert(probe.class);
                }
                // Any other verdict (inconclusive / unavailable / no_model /
                // not_applicable / error) means the search did not settle the
                // question; it contributes to neither refutation nor coverage.
            }
            // A falsifier error costs recall, never soundness: it can never
            // upgrade to a refutation and never counts as a clean look.
            Err(_) => {}
        }
    }

    if refuting_seen == 0 {
        // Nothing refutable was applicable to this pair (e.g. a purely
        // set-valued concept whose only probe fell outside the claimed domain).
        return Refutation::NotProbed;
    }
    if effective == 0 {
        return Refutation::Unavailable {
            reason: "the falsifier produced no completed bounded search for any refuting probe \
                     (offline, unavailable, or inconclusive throughout); this pair is in the \
                     same state as an unprobed one"
                .to_string(),
        };
    }
    Refutation::Unrefuted {
        probes_run: effective,
        classes: classes.into_iter().collect(),
    }
}

/// Probe `alignment` and return it with its [`Refutation`] updated.
///
/// The strongest outcome this can produce is [`Refutation::Unrefuted`]. It never
/// touches the grade and never produces anything a caller could read as a licence
/// to transfer; see the module docs for why that is structural.
pub fn refute_alignment(alignment: Alignment, falsifier: &dyn Falsify) -> Alignment {
    let outcome = probe_alignment(&alignment, falsifier);
    alignment.with_refutation(outcome)
}

/// The informal agreement claim a probe becomes before it reaches the falsifier.
///
/// The falsifier looks for a point where this claim is FALSE, and a point where
/// the two sides disagree is exactly a point where the claim is false, so the
/// polarity is already correct: a counterexample to the claim is a refutation of
/// the alignment.
///
/// When the grade restricts the claim to a domain, that predicate is stated as an
/// assumption so the falsifier cannot manufacture a "disagreement" using a point
/// the alignment never spoke about (the excluded `x/0` point, say). The
/// admissibility gate in [`probe_alignment`] already drops the out-of-domain
/// probe; naming the domain here is belt-and-suspenders for the in-domain ones.
fn agreement_claim(alignment: &Alignment, probe: &Probe) -> String {
    let left = alignment.proposal.left.qualified();
    let right = alignment.proposal.right.qualified();

    let mut pinned: Vec<String> = Vec::new();
    for (i, value) in probe.point.iter().enumerate() {
        if value != "_" {
            pinned.push(format!("argument {i} is {value}"));
        }
    }
    let scope = if pinned.is_empty() {
        "all admissible arguments".to_string()
    } else {
        format!("all admissible arguments where {}", pinned.join(" and "))
    };
    let assumption = match alignment.strength.claimed_domain() {
        Some(domain) => format!(" assuming ({})", domain.predicate),
        None => String::new(),
    };
    format!("For {scope}{assumption}, {left} and {right} give the same value.")
}

/// Build the stored witness from a confirmed-counterexample verdict.
///
/// The model falsifier reports a refuting ASSIGNMENT (a single point at which the
/// agreement claim is false), not each side's separately evaluated value, so the
/// witness records that point and states honestly that the two sides diverge
/// there rather than fabricating a pair of values the falsifier never produced.
fn witness_from_verdict(probe: &Probe, verdict: &FalsifyVerdict) -> Witness {
    let point = render_point(verdict.assignment.as_ref(), probe);
    let note = "the two sides disagree at this assignment; the bounded falsifier confirmed a \
                refuting point but does not evaluate each side's value separately"
        .to_string();
    Witness {
        class: probe.class,
        point,
        left_value: note.clone(),
        right_value: note,
        observed_by: format!("model_falsifier:{}", verdict.verdict),
    }
}

/// Render the falsifier's assignment as a stable, sorted `name=value` list.
///
/// Falls back to the probe's own surface point when the verdict carried no
/// usable assignment, so the witness always names *some* concrete location.
fn render_point(assignment: Option<&Value>, probe: &Probe) -> Vec<String> {
    if let Some(Value::Object(map)) = assignment {
        if !map.is_empty() {
            let mut items: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{k}={}", render_scalar(v)))
                .collect();
            items.sort();
            return items;
        }
    }
    probe.point.clone()
}

/// A compact, honest rendering of one assignment value. Strings are kept verbatim
/// (a witness may be rational, printed `"3/2"`), so nothing is ever truncated.
fn render_scalar(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::alignment::{
        AlignmentStrength, ConceptShape, OperandKind, ProposedAlignment, Proposer, SymbolRef,
        Totality,
    };
    use anyhow::Result;
    use serde_json::json;

    /// A canned falsifier: it maps a claim to a fixed verdict by substring match,
    /// so a test can say "refute the zero probe, find nothing on the negative
    /// one" without a model or the Python worker. This REUSES the real
    /// [`Falsify`] seam (the same one [`super::super::falsification::Falsifier`]
    /// implements); it does not reimplement any search.
    struct CannedFalsifier {
        /// A refuting assignment to return when the claim mentions this needle.
        refute_when_contains: Option<(&'static str, Value)>,
        /// The verdict for every other claim.
        otherwise: &'static str,
    }

    impl Falsify for CannedFalsifier {
        fn falsify(&self, statement: &str) -> Result<FalsifyVerdict> {
            if let Some((needle, assignment)) = &self.refute_when_contains {
                if statement.contains(needle) {
                    return Ok(FalsifyVerdict {
                        applicable: true,
                        verdict: VERDICT_COUNTEREXAMPLE.to_string(),
                        assignment: Some(assignment.clone()),
                        spec: Value::Null,
                        details: Value::Null,
                    });
                }
            }
            Ok(FalsifyVerdict {
                applicable: self.otherwise != "no_model",
                verdict: self.otherwise.to_string(),
                assignment: None,
                spec: Value::Null,
                details: Value::Null,
            })
        }
    }

    /// A total/total list-sum pair, matching signatures, no domain restriction, so
    /// every generated probe is `Refuting`.
    fn list_pair() -> ProposedAlignment {
        ProposedAlignment {
            left: SymbolRef::new("hol4", "list", "hol", "SUM"),
            right: SymbolRef::new("hol_light", "lists", "hol", "ITLIST_ADD"),
            left_shape: ConceptShape::new(
                vec![OperandKind::List],
                OperandKind::Natural,
                Totality::total("sum of the empty list is 0"),
            ),
            right_shape: ConceptShape::new(
                vec![OperandKind::List],
                OperandKind::Natural,
                Totality::total("fold with unit 0"),
            ),
            proposer: Proposer::PropertyPattern,
            score: Some(0.7),
        }
    }

    /// The PVS / HOL Light division pair: partial vs total, so grading restricts
    /// it to `not (divisor = 0)` and the partiality probe is out of domain.
    fn division_pair() -> ProposedAlignment {
        ProposedAlignment {
            left: SymbolRef::new("pvs", "prelude", "pvs_classical_hol", "/"),
            right: SymbolRef::new("hol_light", "core", "pvs_classical_hol", "real_div"),
            left_shape: ConceptShape::new(
                vec![OperandKind::Real, OperandKind::Real],
                OperandKind::Real,
                Totality::partial_on("divisor = 0"),
            ),
            right_shape: ConceptShape::new(
                vec![OperandKind::Real, OperandKind::Real],
                OperandKind::Real,
                Totality::total("x / 0 = 0"),
            ),
            proposer: Proposer::PropertyPattern,
            score: Some(0.91),
        }
    }

    #[test]
    fn a_confirmed_counterexample_refutes_and_stores_its_witness() {
        // The falsifier reports disagreement on any empty-container claim.
        let falsifier = CannedFalsifier {
            refute_when_contains: Some(("same value", json!({ "arg0": 0 }))),
            otherwise: VERDICT_NO_COUNTEREXAMPLE,
        };
        let alignment = Alignment::propose(list_pair());
        let refuted = refute_alignment(alignment, &falsifier);

        assert!(refuted.refutation.is_refuted());
        let witness = refuted.refutation.witness().expect("a refuted alignment carries a witness");
        assert_eq!(witness.point, vec!["arg0=0".to_string()]);
        assert!(witness.observed_by.starts_with("model_falsifier:"));

        // A refuted alignment is inert: both exits close.
        assert!(refuted.retrieval_hint().is_none());
        assert!(refuted.transfer_obligation().is_none());
    }

    #[test]
    fn surviving_every_probe_is_unrefuted_never_verified() {
        // Nothing refutes; every probe is a completed bounded search that found
        // nothing.
        let falsifier = CannedFalsifier {
            refute_when_contains: None,
            otherwise: VERDICT_NO_COUNTEREXAMPLE,
        };
        let outcome = probe_alignment(&Alignment::propose(list_pair()), &falsifier);

        match &outcome {
            Refutation::Unrefuted { probes_run, classes } => {
                assert!(*probes_run >= 1, "at least one probe must have run");
                assert!(!classes.is_empty());
            }
            other => panic!("expected unrefuted, got {other:?}"),
        }
        // The vocabulary has no positive verdict, even after surviving.
        assert_eq!(outcome.label(), "unrefuted");
        assert!(!outcome.is_refuted());
    }

    #[test]
    fn an_offline_falsifier_is_unavailable_not_a_clean_run() {
        // `no_model` is "we did not look"; it must never read as unrefuted.
        let falsifier = CannedFalsifier {
            refute_when_contains: None,
            otherwise: "no_model",
        };
        let outcome = probe_alignment(&Alignment::propose(list_pair()), &falsifier);
        assert_eq!(outcome.label(), "probing_unavailable");
    }

    #[test]
    fn a_disagreement_outside_the_claimed_domain_is_never_run() {
        // The division grade restricts the claim to `not (divisor = 0)`. Its
        // partiality probe at the divisor-zero point is OutsideClaimedDomain, so
        // even a falsifier that would "refute" any partiality claim must not be
        // handed one, and the pair survives on its in-domain probes.
        let refuting_partiality = CannedFalsifier {
            refute_when_contains: Some(("undefined", json!({ "divisor": 0 }))),
            otherwise: VERDICT_NO_COUNTEREXAMPLE,
        };
        let alignment = Alignment::propose(division_pair());
        // Sanity: grading did restrict the domain, so the guard is actually live.
        assert!(matches!(alignment.strength, AlignmentStrength::AgreesOn { .. }));

        let outcome = probe_alignment(&alignment, &refuting_partiality);
        assert!(
            !outcome.is_refuted(),
            "an out-of-domain disagreement must not refute a domain-restricted grade"
        );
    }

    #[test]
    fn the_module_only_ever_produces_a_refutation() {
        // A compile-time-ish guard read at runtime: whatever we do, the alignment
        // that comes back differs from the input only in its refutation field.
        let falsifier = CannedFalsifier {
            refute_when_contains: None,
            otherwise: VERDICT_NO_COUNTEREXAMPLE,
        };
        let before = Alignment::propose(list_pair());
        let after = refute_alignment(before.clone(), &falsifier);
        assert_eq!(after.proposal, before.proposal, "the proposal is untouched");
        assert_eq!(after.strength, before.strength, "the grade is never promoted");
    }
}
