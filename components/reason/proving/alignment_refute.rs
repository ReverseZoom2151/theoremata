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
//!
//! ## The consumption layer (plan Phase 2.4)
//!
//! The bottom of this file wires the three-part stack into its ONE permitted
//! consumer shape: propose (via [`super::alignment_propose`]), refute (the
//! functions above), then STEER PREMISE RETRIEVAL and nothing else.
//! [`steer_retrieval`] is the only entry point a retrieval caller needs, and it
//! hands back [`AlignedCandidate`], a type whose entire content is a foreign
//! symbol NAME and a ranking weight. That type is the guard:
//!
//! * it carries no statement, no goal, no grade and no obligation, so no caller
//!   can read a fact out of it;
//! * it has no public constructor, so it can only originate from a probe run
//!   that reached [`Refutation::Unrefuted`];
//! * consumption is STRICTLY NARROWER than the record's own exit.
//!   [`Alignment::retrieval_hint`] stays open for [`Refutation::NotProbed`] and
//!   [`Refutation::Unavailable`], which is right for a record but wrong for a
//!   consumer: "we did not look" is no signal at all. [`steer_retrieval`]
//!   therefore demands an outcome that actually probed, which is what makes an
//!   offline falsifier fail safe instead of failing open.
//!
//! Nothing in the layer imports [`super::alignment::TransferObligation`] or
//! [`super::alignment::KernelCertificate`], and nothing calls
//! [`Alignment::transfer_obligation`]. Obligation generation is the other
//! permitted consumer in Phase 2.4 and it is deliberately not built here: a
//! retrieval path has no business minting goals.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Deserialize;
use serde_json::{json, Value};

use super::alignment::{
    Alignment, ConceptShape, DivergenceClass, OperandKind, Probe, ProbeAdmissibility, Refutation,
    Totality, Witness,
};
use super::alignment_propose::{
    propose_alignments, ConstantDecl, LibraryCorpus, MatcherConfig, NormalizedTheorem, PatternToken,
};
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

// ===========================================================================
// Consumption (plan Phase 2.4): steer premise retrieval, license nothing
// ===========================================================================

/// A foreign symbol that an UNREFUTED alignment suggests a retrieval caller
/// might also want to look at, plus how strongly.
///
/// This is deliberately the thinnest possible value. It holds two strings and a
/// number. There is no statement on it, no goal, no grade, no certificate and no
/// obligation, so there is no accessor a caller could mistake for evidence: the
/// worst a wrong one can do is waste a retrieval slot, which is the HOL(y)Hammer
/// bargain the plan cites.
///
/// The fields are private and the struct has no public constructor. Outside this
/// module the ONLY way to obtain one is [`steer_retrieval`], which only ever
/// mints one after a probe run reached [`Refutation::Unrefuted`]. So "an
/// unprobed alignment steered retrieval" is not a thing a caller can express by
/// accident; it would take deliberately reaching past this type into
/// [`super::alignment`] and rebuilding the pipeline by hand.
#[derive(Debug, Clone, PartialEq)]
pub struct AlignedCandidate {
    /// The foreign symbol's bare name, which is what a retrieval list holds.
    name: String,
    /// `system::library::name`, kept for provenance in the recorded evidence so
    /// a human reading the trace can see which corpus the suggestion came from.
    qualified: String,
    /// Ranking multiplier in `(0, 1]`, taken from
    /// [`super::alignment::RetrievalHint::confidence_weight`]. Advisory by
    /// nature: a ranking signal cannot be unsound.
    weight: f64,
}

impl AlignedCandidate {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn qualified(&self) -> &str {
        &self.qualified
    }

    pub fn weight(&self) -> f64 {
        self.weight
    }

    /// The sentence that belongs next to any display of this candidate. Written
    /// here rather than at each call site so that no call site gets to phrase it
    /// more optimistically, matching the discipline
    /// [`Refutation::caveat`] already follows.
    pub fn caveat(&self) -> &'static str {
        "surfaced by an unrefuted alignment: probes ran and none disagreed, which is not \
         agreement; this name is a search suggestion and licenses nothing"
    }

    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "qualified": self.qualified,
            "weight": self.weight,
            "caveat": self.caveat(),
        })
    }
}

/// Propose, refute, and turn the survivors into retrieval suggestions.
///
/// `already_retrieved` is the premise list the caller has in hand. An alignment
/// only steers when one of its two sides is ALREADY in that list: an alignment is
/// a nudge attached to a premise the ordinary retrievers earned, never an
/// independent source of premises. The other side of such a pair is what gets
/// surfaced.
///
/// The admission rule, in order, and every step of it is a narrowing:
///
/// 1. the pair must be anchored on a name already retrieved;
/// 2. its probe run must have reached [`Refutation::Unrefuted`]. Refuted pairs
///    are inert (their [`Alignment::retrieval_hint`] is already `None`), and
///    [`Refutation::NotProbed`] / [`Refutation::Unavailable`] are dropped HERE,
///    because an offline or inconclusive falsifier means we did not look and
///    "we did not look" must never read as "we tried and failed to break it";
/// 3. [`Alignment::retrieval_hint`] must still be open, which is the record's own
///    inertness check re-asked rather than assumed.
///
/// Cross-foundation pairs are admitted, and that is the plan's rule rather than
/// an oversight: cross-foundation alignment is retrieval-only, and retrieval is
/// exactly and only what this function does. It never calls
/// [`Alignment::transfer_obligation`], which is `None` across foundations anyway.
///
/// Deterministic: highest weight first, qualified name breaking ties, truncated
/// to `budget`.
pub fn steer_retrieval(
    left: &LibraryCorpus,
    right: &LibraryCorpus,
    matcher: &MatcherConfig,
    falsifier: &dyn Falsify,
    already_retrieved: &[String],
    budget: usize,
) -> Vec<AlignedCandidate> {
    if budget == 0 || already_retrieved.is_empty() {
        return Vec::new();
    }
    let seeds: BTreeSet<&str> = already_retrieved.iter().map(|s| s.as_str()).collect();
    let mut best: BTreeMap<String, AlignedCandidate> = BTreeMap::new();

    for proposal in propose_alignments(left, right, matcher) {
        let anchored_left = seeds.contains(proposal.left.name.as_str());
        let anchored_right = seeds.contains(proposal.right.name.as_str());
        if !anchored_left && !anchored_right {
            // Nothing in hand to attach this suggestion to, so probing it would
            // spend falsifier runs on a pair no caller asked about.
            continue;
        }

        let alignment = refute_alignment(Alignment::propose(proposal), falsifier);

        // Step 2: only a completed probe run counts. This is the fail-safe.
        if !matches!(alignment.refutation, Refutation::Unrefuted { .. }) {
            continue;
        }
        // Step 3: ask the record itself whether it is still consumable, rather
        // than inferring it from the match above. A refuted alignment closes
        // this exit, so if the two checks ever disagree we take the stricter one.
        let Some(hint) = alignment.retrieval_hint() else {
            continue;
        };

        // Surface the side the caller does NOT already have.
        let other = if anchored_left {
            &alignment.proposal.right
        } else {
            &alignment.proposal.left
        };
        if seeds.contains(other.name.as_str()) {
            // Both sides already retrieved: there is nothing to surface.
            continue;
        }

        let candidate = AlignedCandidate {
            name: other.name.clone(),
            qualified: other.qualified(),
            weight: hint.confidence_weight,
        };
        // Keep the strongest suggestion per foreign symbol. Written as a lookup
        // and a separate insert on purpose: an `entry(..).and_modify(..)` chain
        // would hold a closure borrow of `candidate` across the insert that moves
        // it.
        let keep = match best.get(candidate.qualified.as_str()) {
            Some(existing) => candidate.weight > existing.weight,
            None => true,
        };
        if keep {
            best.insert(candidate.qualified.clone(), candidate);
        }
    }

    let mut out: Vec<AlignedCandidate> = best.into_values().collect();
    out.sort_by(|a, b| {
        b.weight
            .total_cmp(&a.weight)
            .then_with(|| a.qualified.cmp(&b.qualified))
    });
    out.truncate(budget);
    out
}

// ---------------------------------------------------------------------------
// Corpus loading
// ---------------------------------------------------------------------------

/// On-disk shape of the two corpora to align. Kept as a private wire type
/// distinct from [`LibraryCorpus`] so that an operator-authored file can never
/// dictate the in-memory representation, and so that an unrecognised operand
/// kind is a hard error rather than a silent widening of the type gate.
#[derive(Debug, Deserialize)]
struct WireCorpora {
    left: WireCorpus,
    right: WireCorpus,
}

#[derive(Debug, Deserialize)]
struct WireCorpus {
    system: String,
    library: String,
    foundation: String,
    constants: Vec<WireConstant>,
    theorems: Vec<WireTheorem>,
}

#[derive(Debug, Deserialize)]
struct WireConstant {
    name: String,
    operands: Vec<String>,
    result: String,
    /// The junk-value convention, when this side is total. Free provenance text.
    #[serde(default)]
    convention: Option<String>,
    /// The excluded point, when this side is partial. Presence of this field is
    /// what makes the side partial, which is the distinction
    /// [`super::alignment::grade_proposal`] turns the whole `x/0` case on.
    #[serde(default)]
    partial_on: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireTheorem {
    id: String,
    /// A leading `@` marks a library-local constant; everything else is
    /// vocabulary already shared between the two libraries. Same convention the
    /// proposer's own fixtures use.
    tokens: Vec<String>,
}

/// Parse an operand kind name. Unknown names are an ERROR rather than a fallback
/// to [`OperandKind::Opaque`], because `Opaque` is compatible with everything and
/// a typo that silently widened the proposer's type gate would manufacture
/// candidates out of a spelling mistake.
fn parse_kind(raw: &str) -> anyhow::Result<OperandKind> {
    Ok(match raw {
        "natural" => OperandKind::Natural,
        "integer" => OperandKind::Integer,
        "rational" => OperandKind::Rational,
        "real" => OperandKind::Real,
        "complex" => OperandKind::Complex,
        "set" => OperandKind::Set,
        "list" => OperandKind::List,
        "proposition" => OperandKind::Proposition,
        "function" => OperandKind::Function,
        "opaque" => OperandKind::Opaque,
        other => anyhow::bail!("unknown operand kind {other:?} in the alignment corpora file"),
    })
}

impl WireCorpus {
    fn into_corpus(self) -> anyhow::Result<LibraryCorpus> {
        let mut constants: Vec<ConstantDecl> = Vec::new();
        for c in self.constants {
            let mut operands: Vec<OperandKind> = Vec::new();
            for raw in &c.operands {
                operands.push(parse_kind(raw)?);
            }
            let result = parse_kind(&c.result)?;
            let totality = match &c.partial_on {
                Some(excluded) => Totality::partial_on(excluded),
                None => Totality::total(c.convention.as_deref().unwrap_or("total")),
            };
            constants.push(ConstantDecl::new(
                &c.name,
                ConceptShape::new(operands, result, totality),
            ));
        }
        let theorems: Vec<NormalizedTheorem> = self
            .theorems
            .into_iter()
            .map(|t| {
                let tokens: Vec<PatternToken> = t
                    .tokens
                    .iter()
                    .map(|raw| match raw.strip_prefix('@') {
                        Some(name) => PatternToken::Constant(name.to_string()),
                        None => PatternToken::Shared(raw.clone()),
                    })
                    .collect();
                NormalizedTheorem::new(&t.id, tokens)
            })
            .collect();
        Ok(LibraryCorpus::new(
            &self.system,
            &self.library,
            &self.foundation,
            constants,
            theorems,
        ))
    }
}

/// Load the pair of corpora to align from `path`.
///
/// `Ok(None)` when the file does not exist, which is the ordinary case and the
/// fail-safe one: no corpora means no alignments means no signal, never an
/// error the caller has to interpret. A malformed file IS an error, because a
/// file that is present and unreadable is an operator mistake worth surfacing.
pub fn load_corpora_pair(path: &Path) -> anyhow::Result<Option<(LibraryCorpus, LibraryCorpus)>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(path)?;
    let wire: WireCorpora = serde_json::from_str(&raw)?;
    let left = wire.left.into_corpus()?;
    let right = wire.right.into_corpus()?;
    Ok(Some((left, right)))
}

#[cfg(test)]
mod tests {
    use super::super::alignment::{
        AlignmentStrength, ConceptShape, OperandKind, ProposedAlignment, Proposer, SymbolRef,
        Totality,
    };
    use super::*;
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
        let witness = refuted
            .refutation
            .witness()
            .expect("a refuted alignment carries a witness");
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
            Refutation::Unrefuted {
                probes_run,
                classes,
            } => {
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
        assert!(matches!(
            alignment.strength,
            AlignmentStrength::AgreesOn { .. }
        ));

        let outcome = probe_alignment(&alignment, &refuting_partiality);
        assert!(
            !outcome.is_refuted(),
            "an out-of-domain disagreement must not refute a domain-restricted grade"
        );
    }

    // -----------------------------------------------------------------
    // The consumption layer
    // -----------------------------------------------------------------

    /// One constant per side, one theorem per side, identical pattern, identical
    /// shape. The smallest corpus pair that the matcher must propose on.
    fn steer_corpora() -> (LibraryCorpus, LibraryCorpus) {
        let shape = || {
            ConceptShape::new(
                vec![OperandKind::List],
                OperandKind::Natural,
                Totality::total("fold with unit 0"),
            )
        };
        let thm = |id: &str, constant: &str| {
            NormalizedTheorem::new(
                id,
                vec![
                    PatternToken::Shared("forall".to_string()),
                    PatternToken::Constant(constant.to_string()),
                    PatternToken::Shared("eq".to_string()),
                ],
            )
        };
        (
            LibraryCorpus::new(
                "hol4",
                "list",
                "hol",
                vec![ConstantDecl::new("SUM", shape())],
                vec![thm("left_1", "SUM")],
            ),
            LibraryCorpus::new(
                "hol_light",
                "lists",
                "hol",
                vec![ConstantDecl::new("ITLIST_ADD", shape())],
                vec![thm("right_1", "ITLIST_ADD")],
            ),
        )
    }

    #[test]
    fn an_unrefuted_alignment_surfaces_a_name_and_nothing_else() {
        let falsifier = CannedFalsifier {
            refute_when_contains: None,
            otherwise: VERDICT_NO_COUNTEREXAMPLE,
        };
        let (left, right) = steer_corpora();
        let seeds = vec!["SUM".to_string()];
        let out = steer_retrieval(
            &left,
            &right,
            &MatcherConfig::default(),
            &falsifier,
            &seeds,
            4,
        );

        assert_eq!(
            out.len(),
            1,
            "the anchored pair must surface its other side"
        );
        assert_eq!(out[0].name(), "ITLIST_ADD");
        assert_eq!(out[0].qualified(), "hol_light::lists::ITLIST_ADD");
        assert!(out[0].weight() > 0.0 && out[0].weight() <= 1.0);
        // The caveat is the only prose a caller can display, and it says the
        // opposite of "verified".
        assert!(out[0].caveat().contains("licenses nothing"));
        for banned in ["verif", "proved", "proven", "valid", "confirm"] {
            assert!(!out[0].caveat().contains(banned));
        }
        // Everything the candidate can say is a name, a name, and a number.
        let rendered = out[0].to_json();
        let mut keys: Vec<String> = rendered
            .as_object()
            .expect("object")
            .keys()
            .cloned()
            .collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "caveat".to_string(),
                "name".to_string(),
                "qualified".to_string(),
                "weight".to_string(),
            ]
        );
    }

    #[test]
    fn an_unanchored_pair_never_steers_anything() {
        let falsifier = CannedFalsifier {
            refute_when_contains: None,
            otherwise: VERDICT_NO_COUNTEREXAMPLE,
        };
        let (left, right) = steer_corpora();
        // Nothing the caller holds names either side of the pair.
        let seeds = vec!["Nat.add_comm".to_string()];
        assert!(steer_retrieval(
            &left,
            &right,
            &MatcherConfig::default(),
            &falsifier,
            &seeds,
            4
        )
        .is_empty());
        // And an empty retrieval list spends no falsifier runs at all.
        assert!(
            steer_retrieval(&left, &right, &MatcherConfig::default(), &falsifier, &[], 4)
                .is_empty()
        );
    }

    #[test]
    fn an_offline_falsifier_yields_no_signal_rather_than_a_suggestion() {
        // `no_model` probes to `Unavailable`, whose `retrieval_hint` is still
        // OPEN on the record. Consumption must be stricter than the record: an
        // alignment we never actually probed steers nothing.
        let offline = CannedFalsifier {
            refute_when_contains: None,
            otherwise: "no_model",
        };
        let (left, right) = steer_corpora();
        let seeds = vec!["SUM".to_string()];

        let probed = probe_alignment(
            &Alignment::propose(
                propose_alignments(&left, &right, &MatcherConfig::default())
                    .into_iter()
                    .next()
                    .expect("the fixture must propose a pair"),
            ),
            &offline,
        );
        assert_eq!(probed.label(), "probing_unavailable");

        assert!(
            steer_retrieval(
                &left,
                &right,
                &MatcherConfig::default(),
                &offline,
                &seeds,
                4
            )
            .is_empty(),
            "an unavailable falsifier must be no signal, never a licence"
        );
    }

    #[test]
    fn a_refuted_alignment_is_inert_for_retrieval_too() {
        let refuting = CannedFalsifier {
            refute_when_contains: Some(("same value", json!({ "arg0": 0 }))),
            otherwise: VERDICT_NO_COUNTEREXAMPLE,
        };
        let (left, right) = steer_corpora();
        let seeds = vec!["SUM".to_string()];
        assert!(steer_retrieval(
            &left,
            &right,
            &MatcherConfig::default(),
            &refuting,
            &seeds,
            4
        )
        .is_empty());
    }

    #[test]
    fn the_corpora_loader_fails_safe_and_rejects_a_bad_kind() {
        // A missing file is the ordinary case: no corpora, no signal, no error.
        let missing = std::path::PathBuf::from("no_such_alignment_corpora_file.json");
        assert!(load_corpora_pair(&missing)
            .expect("absence is not an error")
            .is_none());

        // An unrecognised operand kind must NOT degrade to `opaque`, which is
        // compatible with everything and would widen the proposer's type gate.
        let wire: WireCorpus = serde_json::from_value(json!({
            "system": "hol4",
            "library": "list",
            "foundation": "hol",
            "constants": [{"name": "SUM", "operands": ["lst"], "result": "natural"}],
            "theorems": [],
        }))
        .expect("the wire shape parses");
        assert!(wire.into_corpus().is_err());
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
        assert_eq!(
            after.strength, before.strength,
            "the grade is never promoted"
        );
    }
}
