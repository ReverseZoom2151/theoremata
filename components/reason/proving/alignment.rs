//! Graded, refutable alignment records (plan Phase 2.2 to 2.4).
//!
//! An "alignment" is the claim that a symbol in library A and a symbol in
//! library B are the same concept. It is the single operation that can silently
//! corrupt a dependency graph, because everything downstream of a wrong
//! alignment inherits a falsehood that no kernel ever looked at.
//!
//! `docs/research/alignment.md` surveyed the field and found:
//!
//! * Proposing alignments works acceptably within one logic (roughly 80 to 94
//!   percent precision on HOL4 / HOL Light; recall was never measured).
//! * Certifying them is done nowhere. MMT is explicit that two aligned symbols
//!   "may use entirely different (possibly not even logically equivalent)
//!   definitions" and still be aligned, because an alignment there is a
//!   knowledge-management relation, not a semantic guarantee (review section
//!   3.3).
//! * Where transfer is genuinely relied on (OpenTheory, Isabelle's HOL Light
//!   importer), soundness comes from replaying proofs in the target kernel. The
//!   alignment table itself is hand-written and trusted because it is small
//!   (review section 8).
//! * Nobody attempts REFUTATION of a proposed alignment (review section 10,
//!   item 3). That gap is what this module is built around.
//!
//! So this module holds three things and deliberately no fourth:
//!
//! 1. [`AlignmentStrength`], a graded record modelled on Trocq's parametricity
//!    classes and MMT's unidirectional annotations. There is no boolean "same".
//! 2. [`Refutation`], an outcome vocabulary with **no positive verdict**. The
//!    strongest thing it can say is [`Refutation::Unrefuted`], meaning probes
//!    ran and none disagreed. That is not verification and the vocabulary has
//!    no word that could be misread as verification.
//! 3. [`generate_probes`], a pure enumeration of edge-case probes drawn from the
//!    divergence classes the literature documents. It returns probe
//!    DESCRIPTIONS. Nothing here executes anything: the falsifier, the witness
//!    search and the exact-arithmetic recheck live elsewhere and this module
//!    stays a pure function of its inputs.
//!
//! The fourth thing, the one that is missing on purpose, is any way to turn an
//! alignment into a fact. See "The consumption guard" below.
//!
//! ## The canonical divergence, and the acceptance test for the schema
//!
//! PVS requires the divisor of `/` to be nonzero; HOL Light and Mizar define
//! `x/0 = 0` (review section 3.3). An alignment of the two recorded as "same"
//! is therefore simply false. The honest record is "agrees on the domain
//! `divisor != 0`, usable in one direction only". A schema that can record that
//! pair as plain equality is the wrong schema, which is why
//! [`AlignmentStrength`] has no unconditional-equality variant reachable from a
//! proposal: [`grade_proposal`] tops out at [`AlignmentStrength::AgreesOn`],
//! and the one unconditional variant demands a [`KernelCertificate`] that only
//! a kernel run can produce.
//!
//! ## The consumption guard
//!
//! An alignment may steer retrieval and may suggest an obligation. It may never
//! license a transfer. That property is structural here, not advisory:
//!
//! * There is **no type in this module that denotes an admitted fact**. Nothing
//!   named theorem, fact, premise or rewrite exists, so no method can return
//!   one.
//! * The only two exits are [`Alignment::retrieval_hint`], which yields a
//!   ranked pointer at a foreign symbol, and
//!   [`Alignment::transfer_obligation`], which yields a [`TransferObligation`]:
//!   a GOAL to be re-proved in the target kernel. `TransferObligation` has no
//!   discharge method, no settable status and no accessor that produces a
//!   statement usable as a hypothesis. The only way to discharge it is to hand
//!   its goal to the prover stack, which is a different component.
//! * Both exits return `None` once the alignment is refuted, so a refuted
//!   alignment is inert rather than merely flagged.
//!
//! This is the HOL(y)Hammer discipline: they tolerated a 6 percent
//! false-alignment rate without unsoundness precisely because a wrong alignment
//! cost a failed proof attempt instead of a false theorem (review section 2).
//!
//! ## Scope limit: within one foundation only
//!
//! Sound transfer exists only within a foundation, where the correspondence can
//! be a proved relation (Trocq, Isabelle's Transfer; review section 5).
//! Cross-foundation efforts are real but small: Logipedia moved roughly 300
//! arithmetic lemmas, and the MMT effort found only 33 concepts common to all
//! four of HOL Light, PVS, Mizar and Coq (review sections 6 and 9). So a
//! cross-foundation alignment here is retrieval-only by construction:
//! [`Alignment::transfer_obligation`] returns `None` for it, always.
//!
//! Also out of scope on purpose: the property-pattern proposer (a separate
//! task) and any execution of probes.

use serde_json::{json, Value};

// ===========================================================================
// Symbols and proposals
// ===========================================================================

/// A symbol in some library, tagged with the foundation its library lives in.
///
/// `foundation` is carried on the symbol rather than derived later because the
/// cross-foundation check is a hard gate on consumption, and a gate that has to
/// look something up can be called with the lookup missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolRef {
    pub system: String,
    pub library: String,
    pub foundation: String,
    pub name: String,
}

impl SymbolRef {
    pub fn new(system: &str, library: &str, foundation: &str, name: &str) -> Self {
        Self {
            system: system.to_string(),
            library: library.to_string(),
            foundation: foundation.to_string(),
            name: name.to_string(),
        }
    }

    pub fn qualified(&self) -> String {
        format!("{}::{}::{}", self.system, self.library, self.name)
    }

    fn to_json(&self) -> Value {
        json!({
            "system": self.system,
            "library": self.library,
            "foundation": self.foundation,
            "name": self.name,
        })
    }
}

/// Who proposed the pair. Recorded because precision differs sharply by
/// proposer: the symbolic property matcher reached roughly 94 percent on the
/// easiest library pair, while the embedding approach managed under 8 percent
/// Top-1 on the same ground truth (review sections 1.2 and 4). A grade is not
/// interpretable without knowing which of those produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proposer {
    PropertyPattern,
    Embedding,
    LanguageModel,
    Human,
    Other(String),
}

impl Proposer {
    fn label(&self) -> &str {
        match self {
            Proposer::PropertyPattern => "property_pattern",
            Proposer::Embedding => "embedding",
            Proposer::LanguageModel => "language_model",
            Proposer::Human => "human",
            Proposer::Other(s) => s.as_str(),
        }
    }
}

/// The kind of value an operand or a result ranges over.
///
/// Coarse on purpose. This drives probe generation, and probe generation only
/// needs to know which documented divergence classes are even applicable to the
/// concept. A finer type language would be a second type system to keep in sync
/// with the real ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperandKind {
    Natural,
    Integer,
    Rational,
    Real,
    Complex,
    Set,
    List,
    Proposition,
    Function,
    Opaque,
}

impl OperandKind {
    fn label(self) -> &'static str {
        match self {
            OperandKind::Natural => "natural",
            OperandKind::Integer => "integer",
            OperandKind::Rational => "rational",
            OperandKind::Real => "real",
            OperandKind::Complex => "complex",
            OperandKind::Set => "set",
            OperandKind::List => "list",
            OperandKind::Proposition => "proposition",
            OperandKind::Function => "function",
            OperandKind::Opaque => "opaque",
        }
    }

    fn is_numeric(self) -> bool {
        matches!(
            self,
            OperandKind::Natural
                | OperandKind::Integer
                | OperandKind::Rational
                | OperandKind::Real
                | OperandKind::Complex
        )
    }

    fn is_signed(self) -> bool {
        matches!(
            self,
            OperandKind::Integer | OperandKind::Rational | OperandKind::Real | OperandKind::Complex
        )
    }

    fn is_container(self) -> bool {
        matches!(self, OperandKind::Set | OperandKind::List)
    }
}

/// Whether a side is total or partial, and on what convention.
///
/// This is the field the division case turns on. PVS's `/` is
/// `PartialOn { excluded: "divisor = 0" }`; HOL Light's is
/// `Total { convention: "x / 0 = 0" }`. Recording the convention as free text
/// is deliberate: it is provenance for a human and a probe point for the
/// falsifier, never something this module interprets logically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Totality {
    Total { convention: String },
    PartialOn { excluded: String },
}

impl Totality {
    pub fn total(convention: &str) -> Self {
        Totality::Total {
            convention: convention.to_string(),
        }
    }

    pub fn partial_on(excluded: &str) -> Self {
        Totality::PartialOn {
            excluded: excluded.to_string(),
        }
    }

    fn excluded_point(&self) -> Option<&str> {
        match self {
            Totality::Total { .. } => None,
            Totality::PartialOn { excluded } => Some(excluded.as_str()),
        }
    }

    fn to_json(&self) -> Value {
        match self {
            Totality::Total { convention } => {
                json!({"totality": "total", "convention": convention})
            }
            Totality::PartialOn { excluded } => {
                json!({"totality": "partial", "excluded": excluded})
            }
        }
    }
}

/// The shape of one side of a proposed pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConceptShape {
    pub operands: Vec<OperandKind>,
    pub result: OperandKind,
    pub totality: Totality,
}

impl ConceptShape {
    pub fn new(operands: Vec<OperandKind>, result: OperandKind, totality: Totality) -> Self {
        Self {
            operands,
            result,
            totality,
        }
    }

    /// Do the two sides even range over the same things?
    ///
    /// A mismatch here is the structural-rigidity case the 2014 matcher could
    /// not handle at all: HOL Light's primitive `complex` against HOL4's pairs
    /// of reals is the same mathematics under a different encoding (review
    /// section 1.3). We do not reject the pair, we downgrade it and emit a
    /// representation probe.
    fn same_signature(&self, other: &ConceptShape) -> bool {
        self.operands == other.operands && self.result == other.result
    }

    fn to_json(&self) -> Value {
        json!({
            "operands": self.operands.iter().map(|k| k.label()).collect::<Vec<_>>(),
            "result": self.result.label(),
            "convention": self.totality.to_json(),
        })
    }
}

/// A pair somebody proposed, before any grading and before any probing.
#[derive(Debug, Clone, PartialEq)]
pub struct ProposedAlignment {
    pub left: SymbolRef,
    pub right: SymbolRef,
    pub left_shape: ConceptShape,
    pub right_shape: ConceptShape,
    pub proposer: Proposer,
    /// The proposer's own score, if it produced one. Kept as raw provenance;
    /// nothing in this module thresholds on it, because the published numbers
    /// say the tail of a ranked list is where the errors live (review 1.2).
    pub score: Option<f64>,
}

impl ProposedAlignment {
    pub fn cross_foundation(&self) -> bool {
        self.left.foundation != self.right.foundation
    }

    fn to_json(&self) -> Value {
        json!({
            "left": self.left.to_json(),
            "right": self.right.to_json(),
            "left_shape": self.left_shape.to_json(),
            "right_shape": self.right_shape.to_json(),
            "proposer": self.proposer.label(),
            "score": self.score,
            "cross_foundation": self.cross_foundation(),
        })
    }
}

// ===========================================================================
// Grade
// ===========================================================================

/// Which way an alignment may be read, following MMT's bidirectional versus
/// unidirectional annotation (review section 3.2, where roughly a fifth of the
/// hand-written alignments were unidirectional).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    LeftToRight,
    RightToLeft,
    Bidirectional,
}

impl Direction {
    fn label(self) -> &'static str {
        match self {
            Direction::LeftToRight => "left_to_right",
            Direction::RightToLeft => "right_to_left",
            Direction::Bidirectional => "bidirectional",
        }
    }
}

/// A stated restriction on where the two sides are claimed to agree.
///
/// Free text, checked by nobody here. Its job is to be narrower than
/// "everywhere" and to be visible in the record, so that the division pair
/// cannot be filed as unconditional agreement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainRestriction {
    pub predicate: String,
}

impl DomainRestriction {
    pub fn new(predicate: &str) -> Self {
        Self {
            predicate: predicate.to_string(),
        }
    }
}

/// Evidence that a kernel checked something.
///
/// Fields are private and the only constructor demands a checker name, a proof
/// identifier and the foundation it was checked in. This module cannot make
/// one out of thin air, and neither can a caller that never ran a kernel. It
/// exists so that [`AlignmentStrength::KernelEquality`] is unreachable from
/// heuristic evidence, which is the whole point of grading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelCertificate {
    checker: String,
    proof_id: String,
    foundation: String,
}

impl KernelCertificate {
    /// Record a certificate. Returns `None` for any empty component, since a
    /// certificate that cannot be traced back to a specific checker run is not
    /// evidence of anything.
    pub fn record(checker: &str, proof_id: &str, foundation: &str) -> Option<Self> {
        if checker.trim().is_empty() || proof_id.trim().is_empty() || foundation.trim().is_empty() {
            return None;
        }
        Some(Self {
            checker: checker.to_string(),
            proof_id: proof_id.to_string(),
            foundation: foundation.to_string(),
        })
    }

    pub fn checker(&self) -> &str {
        &self.checker
    }

    pub fn proof_id(&self) -> &str {
        &self.proof_id
    }

    pub fn foundation(&self) -> &str {
        &self.foundation
    }

    fn to_json(&self) -> Value {
        json!({
            "checker": self.checker,
            "proof_id": self.proof_id,
            "foundation": self.foundation,
        })
    }
}

/// What was actually established about the pair. Never a boolean "same".
///
/// The ordering is a strength lattice in the sense of Trocq's parametricity
/// classes: each level demands more structure than the last, and a consumer
/// asks for the weakest level that suffices (review section 5). The levels:
///
/// * [`Correlated`](AlignmentStrength::Correlated) -- statistical
///   co-occurrence only, the raw output of a matcher. Good for retrieval.
/// * [`RelatedBy`](AlignmentStrength::RelatedBy) -- a stated map between the
///   two sides (an encoding, a coercion), still unchecked.
/// * [`AgreesOn`](AlignmentStrength::AgreesOn) -- claimed pointwise agreement,
///   but only on a stated domain, and possibly in one direction only. This is
///   where the division pair lives.
/// * [`KernelEquality`](AlignmentStrength::KernelEquality) -- a kernel checked
///   it. Unreachable without a [`KernelCertificate`].
///
/// Note what is absent: there is no variant meaning "equal everywhere, because
/// a matcher said so". That absence is the schema's acceptance test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlignmentStrength {
    Correlated {
        note: String,
    },
    RelatedBy {
        relation: String,
        direction: Direction,
    },
    AgreesOn {
        domain: DomainRestriction,
        direction: Direction,
    },
    KernelEquality {
        certificate: KernelCertificate,
    },
}

impl AlignmentStrength {
    /// Machine label for the grade. Note that even the strongest one says
    /// "kernel_equality", naming the evidence rather than asserting sameness.
    pub fn label(&self) -> &'static str {
        match self {
            AlignmentStrength::Correlated { .. } => "correlated",
            AlignmentStrength::RelatedBy { .. } => "related_by",
            AlignmentStrength::AgreesOn { .. } => "agrees_on_domain",
            AlignmentStrength::KernelEquality { .. } => "kernel_equality",
        }
    }

    /// Is this an unrestricted claim of agreement? True only for the
    /// kernel-backed variant, and even then consumption is still guarded.
    pub fn is_unrestricted(&self) -> bool {
        matches!(self, AlignmentStrength::KernelEquality { .. })
    }

    /// The domain the claim is confined to, when there is one. A probe outside
    /// this domain cannot refute the alignment, because the alignment never
    /// said anything there.
    pub fn claimed_domain(&self) -> Option<&DomainRestriction> {
        match self {
            AlignmentStrength::AgreesOn { domain, .. } => Some(domain),
            _ => None,
        }
    }

    fn to_json(&self) -> Value {
        match self {
            AlignmentStrength::Correlated { note } => {
                json!({"grade": "correlated", "note": note})
            }
            AlignmentStrength::RelatedBy {
                relation,
                direction,
            } => json!({
                "grade": "related_by",
                "relation": relation,
                "direction": direction.label(),
            }),
            AlignmentStrength::AgreesOn { domain, direction } => json!({
                "grade": "agrees_on_domain",
                "domain": domain.predicate,
                "direction": direction.label(),
            }),
            AlignmentStrength::KernelEquality { certificate } => json!({
                "grade": "kernel_equality",
                "certificate": certificate.to_json(),
            }),
        }
    }
}

/// Grade a proposal from the proposal alone.
///
/// The ceiling here is [`AlignmentStrength::AgreesOn`] and that is structural:
/// a heuristic proposal never justifies an unrestricted claim, so this function
/// has no path that constructs [`AlignmentStrength::KernelEquality`]. Raising
/// an alignment to that level requires presenting a [`KernelCertificate`],
/// which only a checker run can produce.
///
/// The rules, all drawn from documented failure modes:
///
/// * Differing totality conventions produce `AgreesOn` restricted to the
///   partial side's domain of definition, directed from the partial side to
///   the total side. Facts on the partial side always carry the definedness
///   hypothesis, so they survive the move; facts on the total side may be about
///   the convention value and have no counterpart. This is exactly MMT's
///   handling of `/` (review section 3.3).
/// * Differing signatures produce `Correlated`, because the two sides are not
///   even pointwise comparable until somebody supplies the encoding map. This
///   is the complex-as-pairs-of-reals case.
/// * Everything else produces `Correlated` as well. Matching shapes are not
///   agreement; they are the absence of a cheap reason to doubt.
pub fn grade_proposal(proposal: &ProposedAlignment) -> AlignmentStrength {
    let left_excluded = proposal.left_shape.totality.excluded_point();
    let right_excluded = proposal.right_shape.totality.excluded_point();

    match (left_excluded, right_excluded) {
        (Some(excluded), None) => AlignmentStrength::AgreesOn {
            domain: DomainRestriction::new(&format!("not ({excluded})")),
            direction: Direction::LeftToRight,
        },
        (None, Some(excluded)) => AlignmentStrength::AgreesOn {
            domain: DomainRestriction::new(&format!("not ({excluded})")),
            direction: Direction::RightToLeft,
        },
        (Some(left), Some(right)) if left != right => AlignmentStrength::AgreesOn {
            domain: DomainRestriction::new(&format!("not ({left}) and not ({right})")),
            direction: Direction::Bidirectional,
        },
        _ => {
            if proposal.left_shape.same_signature(&proposal.right_shape) {
                AlignmentStrength::Correlated {
                    note: "matching signatures; no agreement established".to_string(),
                }
            } else {
                AlignmentStrength::Correlated {
                    note: "signatures differ; an encoding map would have to be supplied"
                        .to_string(),
                }
            }
        }
    }
}

// ===========================================================================
// Refutation
// ===========================================================================

/// The divergence classes the literature documents, plus the encoding mismatch
/// the 2014 matcher could not represent.
///
/// These are the buckets probe generation draws from. They are not a taxonomy
/// of all possible disagreement; they are the cases that are known to have bitten
/// somebody.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DivergenceClass {
    /// Zero as an argument. The most productive single point in the literature.
    Zero,
    /// The empty set, the empty list, the empty product.
    Empty,
    /// Negative arguments, including truncated subtraction on naturals.
    Negative,
    /// The first or last element of a range, index origin, saturation point.
    Boundary,
    /// One side is partial and the other is total with a junk-value convention.
    /// This is the `x/0` class.
    UndefinedVersusTotal,
    /// Same mathematics, different encoding, so the two sides are not even
    /// pointwise comparable without a map.
    Representation,
}

impl DivergenceClass {
    pub fn label(self) -> &'static str {
        match self {
            DivergenceClass::Zero => "zero",
            DivergenceClass::Empty => "empty",
            DivergenceClass::Negative => "negative",
            DivergenceClass::Boundary => "boundary",
            DivergenceClass::UndefinedVersusTotal => "undefined_versus_total",
            DivergenceClass::Representation => "representation",
        }
    }
}

/// A concrete point at which the two sides were observed to disagree, together
/// with what each side gave there and who observed it.
///
/// A refutation without a witness would be an opinion, so the witness is not
/// optional.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Witness {
    pub class: DivergenceClass,
    pub point: Vec<String>,
    pub left_value: String,
    pub right_value: String,
    pub observed_by: String,
}

impl Witness {
    fn to_json(&self) -> Value {
        json!({
            "class": self.class.label(),
            "point": self.point,
            "left_value": self.left_value,
            "right_value": self.right_value,
            "observed_by": self.observed_by,
        })
    }
}

/// The outcome vocabulary. There is no positive verdict in it, on purpose.
///
/// The strongest statement available is [`Refutation::Unrefuted`], which means
/// "probes ran and none of them disagreed". That is a statement about our
/// probes, not about the symbols. The same discipline the search stages already
/// follow when they carry a false verification flag and have no word for
/// success: the absence of a positive term is what stops a downstream reader
/// from promoting evidence it does not have.
///
/// [`Refutation::NotProbed`] and [`Refutation::Unavailable`] both mean "we did
/// not look", kept apart because a missing dependency must not read as a clean
/// run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refutation {
    NotProbed,
    Unavailable {
        reason: String,
    },
    Unrefuted {
        probes_run: usize,
        classes: Vec<DivergenceClass>,
    },
    Refuted {
        witness: Witness,
    },
}

impl Refutation {
    /// A short machine label. Every one of these is neutral or negative; none
    /// of them can be read as a certification.
    pub fn label(&self) -> &'static str {
        match self {
            Refutation::NotProbed => "not_probed",
            Refutation::Unavailable { .. } => "probing_unavailable",
            Refutation::Unrefuted { .. } => "unrefuted",
            Refutation::Refuted { .. } => "refuted",
        }
    }

    pub fn is_refuted(&self) -> bool {
        matches!(self, Refutation::Refuted { .. })
    }

    pub fn witness(&self) -> Option<&Witness> {
        match self {
            Refutation::Refuted { witness } => Some(witness),
            _ => None,
        }
    }

    /// The sentence that goes next to the label wherever a human reads this.
    /// Written out here rather than at each call site so that no call site gets
    /// to phrase it more optimistically.
    pub fn caveat(&self) -> &'static str {
        match self {
            Refutation::NotProbed => "no probe has been run; nothing is known about this pair",
            Refutation::Unavailable { .. } => {
                "probing did not run, so this pair is in the same state as an unprobed one"
            }
            Refutation::Unrefuted { .. } => {
                "probes ran and none disagreed; agreement is not established and this pair \
                 may still be wrong"
            }
            Refutation::Refuted { .. } => "a probe found a point where the two sides disagree",
        }
    }

    fn to_json(&self) -> Value {
        let mut base = match self {
            Refutation::NotProbed => json!({}),
            Refutation::Unavailable { reason } => json!({"reason": reason}),
            Refutation::Unrefuted {
                probes_run,
                classes,
            } => json!({
                "probes_run": probes_run,
                "classes": classes.iter().map(|c| c.label()).collect::<Vec<_>>(),
            }),
            Refutation::Refuted { witness } => json!({"witness": witness.to_json()}),
        };
        let obj = base.as_object_mut().expect("json object");
        obj.insert("outcome".to_string(), json!(self.label()));
        obj.insert("caveat".to_string(), json!(self.caveat()));
        base
    }
}

// ===========================================================================
// Probe generation (pure; nothing here runs)
// ===========================================================================

/// A probe description: where to look and what question to ask there.
///
/// It carries no executor and no result field. Running it is somebody else's
/// job (the falsifier, the witness search, the exact-arithmetic recheck), and
/// keeping the result out of this struct is what stops a probe from being
/// mistaken for an observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub class: DivergenceClass,
    /// One entry per operand, in order. Surface syntax, deliberately: this
    /// module has no term representation and should not grow one.
    pub point: Vec<String>,
    pub question: String,
    pub rationale: &'static str,
}

impl Probe {
    fn new(
        class: DivergenceClass,
        point: Vec<String>,
        question: String,
        rationale: &'static str,
    ) -> Self {
        Self {
            class,
            point,
            question,
            rationale,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "class": self.class.label(),
            "point": self.point,
            "question": self.question,
            "rationale": self.rationale,
        })
    }
}

/// Whether a probe, if it disagreed, would actually refute the graded claim.
///
/// This is the payoff of grading. Once the division pair is graded as agreeing
/// only where the divisor is nonzero, a disagreement at zero is not a
/// refutation: the alignment never claimed anything there. A schema that had
/// recorded the pair as plain equality would have had no way to say that, and
/// would have had to either accept a false claim or throw away a usable one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeAdmissibility {
    /// Inside the claimed domain: disagreement here refutes the alignment.
    Refuting,
    /// Outside the claimed domain: disagreement here is expected and refutes
    /// nothing.
    OutsideClaimedDomain,
}

/// Enumerate edge-case probes for a proposed pair. Pure: no IO, no clock, no
/// randomness, and no execution.
///
/// Coverage follows the divergence classes above, gated on what the shapes make
/// applicable, so a set-valued concept does not get asked about negative
/// numbers.
pub fn generate_probes(proposal: &ProposedAlignment) -> Vec<Probe> {
    let mut probes: Vec<Probe> = Vec::new();
    let left_name = proposal.left.qualified();
    let right_name = proposal.right.qualified();
    let operands = &proposal.left_shape.operands;

    // The partiality class first, because it is the one with a documented
    // real-world instance (PVS `/` against HOL Light `/`).
    for (side, shape) in [
        ("left", &proposal.left_shape),
        ("right", &proposal.right_shape),
    ] {
        if let Some(excluded) = shape.totality.excluded_point() {
            probes.push(Probe::new(
                DivergenceClass::UndefinedVersusTotal,
                vec![excluded.to_string()],
                format!(
                    "at the point where the {side} side is undefined ({excluded}), what does the \
                     other side give?"
                ),
                "one side is partial and the other is total with a junk-value convention; this \
                 is the documented x/0 divergence",
            ));
        }
    }

    for (index, kind) in operands.iter().enumerate() {
        if kind.is_numeric() {
            probes.push(Probe::new(
                DivergenceClass::Zero,
                point_with(operands.len(), index, "0"),
                format!("do {left_name} and {right_name} agree when argument {index} is zero?"),
                "zero is the single most productive divergence point in the surveyed libraries",
            ));
        }
        if kind.is_signed() {
            probes.push(Probe::new(
                DivergenceClass::Negative,
                point_with(operands.len(), index, "-1"),
                format!("do the two sides agree when argument {index} is negative?"),
                "sign conventions and branch choices diverge on negative arguments",
            ));
        }
        if *kind == OperandKind::Natural {
            probes.push(Probe::new(
                DivergenceClass::Boundary,
                point_with(operands.len(), index, "0 with a predecessor taken"),
                format!(
                    "does argument {index} underflow, and do both sides truncate the same way?"
                ),
                "truncated subtraction on naturals is a boundary convention, not a theorem",
            ));
        }
        if kind.is_container() {
            probes.push(Probe::new(
                DivergenceClass::Empty,
                point_with(operands.len(), index, "the empty container"),
                format!("do the two sides agree when argument {index} is empty?"),
                "the empty case is where fold and quantifier conventions separate",
            ));
            probes.push(Probe::new(
                DivergenceClass::Boundary,
                point_with(operands.len(), index, "a one-element container"),
                format!("do the two sides agree on a singleton at argument {index}?"),
                "index origin and off-by-one conventions show up first on singletons",
            ));
        }
        if *kind == OperandKind::Complex {
            probes.push(Probe::new(
                DivergenceClass::Boundary,
                point_with(operands.len(), index, "on the branch cut"),
                format!("do the two sides pick the same branch at argument {index}?"),
                "branch cuts are a convention each library fixes independently",
            ));
        }
    }

    if !proposal.left_shape.same_signature(&proposal.right_shape) {
        probes.push(Probe::new(
            DivergenceClass::Representation,
            vec!["any point".to_string()],
            format!(
                "which map carries {left_name} arguments to {right_name} arguments, and is it \
                 total?"
            ),
            "the surveyed matcher could not relate a primitive type to its encoding as a pair; \
             without the map there is nothing to compare",
        ));
    }

    probes
}

/// Build an argument tuple that is the wildcard everywhere except at `index`.
fn point_with(arity: usize, index: usize, value: &str) -> Vec<String> {
    (0..arity)
        .map(|i| {
            if i == index {
                value.to_string()
            } else {
                "_".to_string()
            }
        })
        .collect()
}

// ===========================================================================
// The record, and the two ways out of it
// ===========================================================================

/// A pointer at a foreign symbol, for ranking retrieval candidates.
///
/// Carries no statement and no truth. The worst a wrong one can do is waste a
/// retrieval slot.
#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalHint {
    pub from: SymbolRef,
    pub to: SymbolRef,
    /// Multiplier in the range zero to one, lower where the evidence is weaker.
    /// Advisory by nature: this is a ranking signal, and ranking signals cannot
    /// be unsound.
    pub confidence_weight: f64,
    pub caveat: &'static str,
}

/// A goal to be re-proved in the target system.
///
/// This type is the whole consumption guard. It has:
///
/// * no discharge method,
/// * no mutable status,
/// * no accessor that yields anything usable as a hypothesis or a rewrite,
/// * and no sibling type in this module that denotes an established fact.
///
/// So the strongest thing a caller can do with an alignment is obtain some text
/// and hand it to a prover. If the prover fails, nothing entered the graph. A
/// wrong alignment therefore costs compute, which is the HOL(y)Hammer bargain
/// (review sections 2 and 10, item 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferObligation {
    goal: String,
    target_system: String,
    scope_note: String,
}

impl TransferObligation {
    /// The statement that must be proved in the target system before anything
    /// crosses. Returned as a string rather than as a term precisely so that it
    /// cannot be spliced into a proof context by accident.
    pub fn goal(&self) -> &str {
        &self.goal
    }

    pub fn target_system(&self) -> &str {
        &self.target_system
    }

    pub fn scope_note(&self) -> &str {
        &self.scope_note
    }

    /// Constant. There is no code path anywhere that makes this anything else,
    /// because discharging an obligation is the prover's business and the
    /// result of that lives in the prover's own records, not here.
    pub fn status(&self) -> &'static str {
        "undischarged"
    }

    pub fn to_json(&self) -> Value {
        json!({
            "goal": self.goal,
            "target_system": self.target_system,
            "scope_note": self.scope_note,
            "status": self.status(),
        })
    }
}

/// A proposed pair, its grade, and what refutation has found so far.
#[derive(Debug, Clone, PartialEq)]
pub struct Alignment {
    pub proposal: ProposedAlignment,
    pub strength: AlignmentStrength,
    pub refutation: Refutation,
}

impl Alignment {
    /// Take a proposal and grade it. The result starts unprobed, which is the
    /// honest state for something nobody has looked at.
    pub fn propose(proposal: ProposedAlignment) -> Self {
        let strength = grade_proposal(&proposal);
        Self {
            proposal,
            strength,
            refutation: Refutation::NotProbed,
        }
    }

    /// Raise the grade. Kept separate from `propose` so that every strengthening
    /// is an explicit act by a caller who has the evidence in hand.
    pub fn with_strength(mut self, strength: AlignmentStrength) -> Self {
        self.strength = strength;
        self
    }

    pub fn with_refutation(mut self, refutation: Refutation) -> Self {
        self.refutation = refutation;
        self
    }

    pub fn cross_foundation(&self) -> bool {
        self.proposal.cross_foundation()
    }

    /// The probes worth running, each tagged with whether a disagreement there
    /// would actually refute the graded claim.
    pub fn probes(&self) -> Vec<(Probe, ProbeAdmissibility)> {
        let has_domain = self.strength.claimed_domain().is_some();
        generate_probes(&self.proposal)
            .into_iter()
            .map(|probe| {
                // A pair graded as agreeing only off the exceptional point has
                // said nothing at that point, so the partiality probe there
                // cannot refute it. Every other class remains refuting.
                let admissibility =
                    if has_domain && probe.class == DivergenceClass::UndefinedVersusTotal {
                        ProbeAdmissibility::OutsideClaimedDomain
                    } else {
                        ProbeAdmissibility::Refuting
                    };
                (probe, admissibility)
            })
            .collect()
    }

    /// Steer retrieval. `None` once refuted: a refuted alignment is inert, not
    /// merely annotated.
    pub fn retrieval_hint(&self) -> Option<RetrievalHint> {
        if self.refutation.is_refuted() {
            return None;
        }
        let weight = match (&self.strength, &self.refutation) {
            (AlignmentStrength::KernelEquality { .. }, _) => 1.0,
            (AlignmentStrength::AgreesOn { .. }, Refutation::Unrefuted { .. }) => 0.8,
            (AlignmentStrength::AgreesOn { .. }, _) => 0.6,
            (AlignmentStrength::RelatedBy { .. }, _) => 0.5,
            (AlignmentStrength::Correlated { .. }, Refutation::Unrefuted { .. }) => 0.4,
            (AlignmentStrength::Correlated { .. }, _) => 0.3,
        };
        Some(RetrievalHint {
            from: self.proposal.left.clone(),
            to: self.proposal.right.clone(),
            confidence_weight: weight,
            caveat: "ranking signal only; this pointer never licenses a transfer",
        })
    }

    /// Suggest what would have to be proved for anything to cross.
    ///
    /// `None` when refuted, and `None` across foundations. The second case is
    /// the scope limit made structural: sound transfer exists only within a
    /// foundation, so this module refuses to even phrase the obligation across
    /// one.
    pub fn transfer_obligation(&self) -> Option<TransferObligation> {
        if self.refutation.is_refuted() || self.cross_foundation() {
            return None;
        }
        let left = self.proposal.left.qualified();
        let right = self.proposal.right.qualified();
        let goal = match &self.strength {
            AlignmentStrength::AgreesOn { domain, .. } => format!(
                "for all arguments satisfying ({}), {left} and {right} give the same value",
                domain.predicate
            ),
            AlignmentStrength::RelatedBy { relation, .. } => {
                format!("{left} and {right} are related by ({relation}) at every argument")
            }
            _ => format!("for all arguments, {left} and {right} give the same value"),
        };
        Some(TransferObligation {
            goal,
            target_system: self.proposal.right.system.clone(),
            scope_note: "must be discharged by the target kernel before any dependent result is \
                         recorded; this record is not itself evidence"
                .to_string(),
        })
    }

    pub fn to_json(&self) -> Value {
        json!({
            "proposal": self.proposal.to_json(),
            "strength": self.strength.to_json(),
            "refutation": self.refutation.to_json(),
            "consumption": {
                "retrieval": self.retrieval_hint().is_some(),
                "obligation": self.transfer_obligation().map(|o| o.to_json()),
                "note": "an alignment steers retrieval and suggests obligations; it never \
                         licenses a transfer",
            },
        })
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The PVS / HOL Light division pair from the review, section 3.3.
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

    /// Recursively collect every object key in a JSON value.
    fn keys(value: &Value, out: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (k, v) in map {
                    out.push(k.clone());
                    keys(v, out);
                }
            }
            Value::Array(items) => items.iter().for_each(|v| keys(v, out)),
            _ => {}
        }
    }

    #[test]
    fn x_over_zero_cannot_be_recorded_as_plain_equality() {
        let alignment = Alignment::propose(division_pair());
        match &alignment.strength {
            AlignmentStrength::AgreesOn { domain, direction } => {
                assert!(
                    domain.predicate.contains("divisor = 0"),
                    "the domain must name the exceptional point, got {domain:?}"
                );
                // PVS facts carry the definedness hypothesis, so they move to
                // HOL Light. HOL Light facts about x/0 have no counterpart.
                assert_eq!(*direction, Direction::LeftToRight);
            }
            other => panic!("division must grade as domain-restricted agreement, got {other:?}"),
        }
        assert!(!alignment.strength.is_unrestricted());

        // The only unrestricted variant demands a kernel certificate, and the
        // grader has no path that builds one. Both halves of that are checked.
        assert!(KernelCertificate::record("", "p1", "hol").is_none());
        assert!(KernelCertificate::record("lean", "  ", "hol").is_none());
        for proposal in [division_pair(), list_pair()] {
            assert!(
                !grade_proposal(&proposal).is_unrestricted(),
                "grading from a proposal must never reach an unrestricted claim"
            );
        }

        // And a disagreement at zero does not refute this record, because the
        // record never claimed anything at zero.
        let at_zero = alignment
            .probes()
            .into_iter()
            .find(|(p, _)| p.class == DivergenceClass::UndefinedVersusTotal)
            .expect("the partiality probe must be generated");
        assert_eq!(at_zero.1, ProbeAdmissibility::OutsideClaimedDomain);
    }

    #[test]
    fn refuted_alignment_carries_its_witness_and_cannot_be_consumed() {
        let witness = Witness {
            class: DivergenceClass::Empty,
            point: vec!["the empty container".to_string()],
            left_value: "0".to_string(),
            right_value: "1".to_string(),
            observed_by: "bounded_evaluation".to_string(),
        };
        let alignment = Alignment::propose(list_pair()).with_refutation(Refutation::Refuted {
            witness: witness.clone(),
        });

        assert_eq!(alignment.refutation.witness(), Some(&witness));
        assert!(alignment.retrieval_hint().is_none());
        assert!(alignment.transfer_obligation().is_none());

        let rendered = alignment.to_json();
        assert_eq!(rendered["refutation"]["outcome"], json!("refuted"));
        assert_eq!(rendered["refutation"]["witness"]["right_value"], json!("1"));
        assert_eq!(rendered["consumption"]["retrieval"], json!(false));
        assert_eq!(rendered["consumption"]["obligation"], Value::Null);
    }

    #[test]
    fn unrefuted_never_reads_as_verified() {
        let states = [
            Refutation::NotProbed,
            Refutation::Unavailable {
                reason: "no evaluator for this concept".to_string(),
            },
            Refutation::Unrefuted {
                probes_run: 6,
                classes: vec![DivergenceClass::Zero, DivergenceClass::Empty],
            },
        ];
        // Nothing in the vocabulary is a positive verdict.
        for state in &states {
            assert!(!state.is_refuted());
            for banned in ["verif", "proved", "proven", "same", "valid", "confirm"] {
                assert!(
                    !state.label().contains(banned),
                    "outcome label {} contains {banned}",
                    state.label()
                );
            }
        }
        assert_eq!(states[2].label(), "unrefuted");
        assert!(states[2].caveat().contains("not established"));

        // No serialized key anywhere may suggest a positive verdict.
        let alignment = Alignment::propose(list_pair()).with_refutation(Refutation::Unrefuted {
            probes_run: 6,
            classes: vec![DivergenceClass::Zero, DivergenceClass::Empty],
        });
        let rendered = alignment.to_json();
        let mut found = Vec::new();
        keys(&rendered, &mut found);
        assert!(!found.is_empty());
        for key in &found {
            for banned in ["verif", "proved", "proven", "same", "valid", "confirm"] {
                assert!(
                    !key.contains(banned),
                    "serialized key {key} contains {banned}"
                );
            }
        }
        assert_eq!(rendered["refutation"]["outcome"], json!("unrefuted"));
        assert_eq!(rendered["refutation"]["probes_run"], json!(6));
    }

    #[test]
    fn probe_generation_covers_the_documented_divergence_classes() {
        let division = generate_probes(&division_pair());
        let division_classes: Vec<_> = division.iter().map(|p| p.class).collect();
        assert!(division_classes.contains(&DivergenceClass::UndefinedVersusTotal));
        assert!(division_classes.contains(&DivergenceClass::Zero));
        assert!(division_classes.contains(&DivergenceClass::Negative));
        // Real operands are not containers, so no empty-case probe is invented.
        assert!(!division_classes.contains(&DivergenceClass::Empty));
        // Matching signatures, so no encoding question either.
        assert!(!division_classes.contains(&DivergenceClass::Representation));
        // The zero probe pins the argument that is zero and wildcards the rest.
        let zero = division
            .iter()
            .find(|p| p.class == DivergenceClass::Zero && p.point[1] == "0")
            .expect("a probe placing zero in the divisor position");
        assert_eq!(zero.point, vec!["_".to_string(), "0".to_string()]);

        let lists = generate_probes(&list_pair());
        let list_classes: Vec<_> = lists.iter().map(|p| p.class).collect();
        assert!(list_classes.contains(&DivergenceClass::Empty));
        assert!(list_classes.contains(&DivergenceClass::Boundary));

        // The encoding mismatch case: a primitive complex against a pair of
        // reals, which the surveyed matcher could not relate at all.
        let mut encoded = list_pair();
        encoded.left_shape = ConceptShape::new(
            vec![OperandKind::Complex],
            OperandKind::Complex,
            Totality::total("primitive"),
        );
        encoded.right_shape = ConceptShape::new(
            vec![OperandKind::Real, OperandKind::Real],
            OperandKind::Real,
            Totality::total("pair of reals"),
        );
        let encoded_classes: Vec<_> = generate_probes(&encoded).iter().map(|p| p.class).collect();
        assert!(encoded_classes.contains(&DivergenceClass::Representation));
        assert!(encoded_classes.contains(&DivergenceClass::Boundary));

        // Every probe is a description, never a result: nothing was executed.
        for probe in &division {
            assert!(!probe.question.is_empty());
            assert!(!probe.rationale.is_empty());
        }
    }

    #[test]
    fn the_consumption_guard_makes_a_transfer_unexpressible() {
        // Within one foundation the strongest exit is an obligation, and it is
        // permanently undischarged as far as this module is concerned.
        let alignment =
            Alignment::propose(division_pair()).with_refutation(Refutation::Unrefuted {
                probes_run: 4,
                classes: vec![DivergenceClass::Zero],
            });
        let obligation = alignment
            .transfer_obligation()
            .expect("same-foundation pairs get an obligation");
        assert_eq!(obligation.status(), "undischarged");
        assert!(obligation.goal().contains("divisor = 0"));
        assert_eq!(obligation.target_system(), "hol_light");
        assert!(obligation.scope_note().contains("target kernel"));

        // Even a kernel-backed grade exits as an obligation, not as a fact:
        // the certificate is evidence about a specific checker run, and turning
        // evidence into a licence is the verification layer's job.
        let certificate =
            KernelCertificate::record("lean4", "thm_div_agree", "pvs_classical_hol").unwrap();
        let promoted = alignment
            .clone()
            .with_strength(AlignmentStrength::KernelEquality { certificate });
        let promoted_obligation = promoted.transfer_obligation().unwrap();
        assert_eq!(promoted_obligation.status(), "undischarged");

        // Retrieval is always available while unrefuted, and is weighted, never
        // authoritative.
        let hint = alignment.retrieval_hint().expect("retrieval stays open");
        assert!(hint.confidence_weight > 0.0 && hint.confidence_weight <= 1.0);
        assert!(hint.caveat.contains("never licenses a transfer"));

        // Cross-foundation: retrieval only, by construction.
        let mut cross = division_pair();
        cross.right.foundation = "mizar_set_theory".to_string();
        let cross_alignment = Alignment::propose(cross);
        assert!(cross_alignment.cross_foundation());
        assert!(cross_alignment.transfer_obligation().is_none());
        assert!(cross_alignment.retrieval_hint().is_some());
    }
}
