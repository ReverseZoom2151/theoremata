//! The property-pattern proposer: the PROPOSING half of alignment.
//!
//! [`super::alignment`] holds the graded record, the refutation vocabulary and
//! the probe enumeration. It deliberately contains no proposer. This module is
//! that proposer, and it is a reimplementation of the Gauthier and Kaliszyk
//! property matcher (CICM 2014, `docs/research/alignment.md` section 1.1),
//! chosen over an embedding because the review is unambiguous about the
//! comparison: the symbolic matcher reached roughly 80 to 94 percent precision
//! on the same task where the joint-embedding approach (JEFL) managed under 8
//! percent Top-1 against a 1,000-pair ground truth (review sections 1.2 and 4).
//! The review's adoption list says it outright: "Use the property-pattern
//! matcher, not embeddings, as the proposer" (section 10, item 5).
//!
//! ## The technique, in four steps
//!
//! 1. Normalize each theorem into a token skeleton. That step is the CALLER's,
//!    not ours (see "What a caller must extract" below), because normalization
//!    is a property of the exporter and of the source logic.
//! 2. Turn each theorem into a PATTERN per constant: replace the constant of
//!    interest by a hole and leave the rest fixed. A constant is then
//!    characterized by the set of patterns it participates in.
//! 3. Score a cross-library constant pair by its shared patterns, weighting
//!    rare patterns far above common ones.
//! 4. Accept the single best type-compatible pair, substitute BOTH sides by a
//!    shared fresh symbol, re-index and repeat. This bootstrapping is the actual
//!    engine (review section 1.1, step 4): it starts from the logical constants
//!    alone and each accepted pair makes further patterns line up that were
//!    invisible before.
//!
//! ## Purity
//!
//! No IO, no clock, no randomness, no global state. The corpora live in
//! gitignored `resources/` and a live extractor belongs in the export layer;
//! this module is a function from normalized data to a ranked list.
//!
//! ## What this module may and may not say
//!
//! A proposal is a HYPOTHESIS. The strongest thing emitted here is a scored
//! candidate pair, and the type it is emitted in is [`ProposedAlignment`], which
//! is exactly what [`super::alignment::grade_proposal`] consumes. There is no
//! way to skip that step: nothing here constructs an [`AlignmentStrength`], a
//! [`super::alignment::KernelCertificate`] or anything that reads as a fact.
//!
//! There is also, deliberately, no threshold, no `best_match`, no
//! `is_confident`, and no boolean accessor anywhere in this file. The published
//! numbers are 80 to 94 percent precision WITHIN one logic, worse across logics,
//! with recall never measured at all (review sections 1.2 and 1.3). A wrong
//! proposal is a normal event, not an anomaly, so the API offers a caller no
//! affordance for treating a high score as a decision. See
//! [`PatternEvidence::caveat`], which is the only interpretation this module
//! ships.
//!
//! ## The known blind spot, kept rather than papered over
//!
//! Type compatibility is a hard gate here ([`shapes_compatible`]), so this
//! proposer cannot suggest HOL Light's primitive `complex` against HOL4's
//! encoding as a pair of reals. That is the "structural rigidity" limitation the
//! original authors report (review section 1.3), and it is preserved on purpose:
//! relaxing the gate would mostly manufacture pairs that
//! [`super::alignment::grade_proposal`] can only grade as `Correlated` anyway,
//! at the cost of contaminating the substitution loop. Encoding-level alignment
//! needs a different mechanism (a supplied representation map), not a looser
//! matcher.
//!
//! ## Error propagation, stated up front
//!
//! Because step 4 substitutes accepted pairs into both libraries, an early wrong
//! acceptance contaminates every later score. The review flags this (section
//! 1.3, last bullet). This module does not fix it. What it does instead is make
//! it visible: [`PatternEvidence::round`] and [`PatternEvidence::enabled_by`]
//! record which earlier acceptances a given proposal depended on, so a caller
//! that later refutes proposal `k` can find every proposal downstream of it.

use std::collections::{BTreeMap, BTreeSet};

use super::alignment::{ConceptShape, OperandKind, ProposedAlignment, Proposer, SymbolRef};

// ===========================================================================
// Input: normalized corpora supplied by the caller
// ===========================================================================

/// One token of a normalized theorem skeleton.
///
/// The split is the only thing the matcher needs from the caller's term
/// language: which tokens are library-local constants (the things we are trying
/// to align) and which are already shared vocabulary (the logical operators,
/// application structure, bound-variable markers, numerals). Everything else
/// about the term representation is the caller's business, which is what keeps
/// this module from growing a second term language.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PatternToken {
    /// A constant belonging to this library, named as the exporter names it.
    Constant(String),
    /// Vocabulary already common to both libraries. The review's `norm_1` level
    /// abstracts over forall, exists, and, or, implies, not and equals, and
    /// applies the AC and negation-pushing rewrites; `norm_2` adds a list of
    /// application-specific AC constants (review section 1.1, step 2). All of
    /// that work has already happened by the time a token reaches here.
    Shared(String),
}

/// One normalized theorem. `id` is provenance only; nothing scores on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedTheorem {
    pub id: String,
    pub tokens: Vec<PatternToken>,
}

impl NormalizedTheorem {
    pub fn new(id: &str, tokens: Vec<PatternToken>) -> Self {
        Self {
            id: id.to_string(),
            tokens,
        }
    }
}

/// A constant that is eligible to be aligned, with the shape that gates it.
///
/// The shape is [`ConceptShape`], the same type the graded record uses, so the
/// caller supplies it once and it travels into the proposal unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstantDecl {
    pub name: String,
    pub shape: ConceptShape,
}

impl ConstantDecl {
    pub fn new(name: &str, shape: ConceptShape) -> Self {
        Self {
            name: name.to_string(),
            shape,
        }
    }
}

/// One side of the matching problem.
///
/// A constant that appears in `constants` but in no theorem simply never scores,
/// which is the honest outcome: the whole signal here is statistical
/// co-occurrence of properties, so a constant with no properties has no evidence
/// attached to it (review section 1.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryCorpus {
    pub system: String,
    pub library: String,
    pub foundation: String,
    pub constants: Vec<ConstantDecl>,
    pub theorems: Vec<NormalizedTheorem>,
}

impl LibraryCorpus {
    pub fn new(
        system: &str,
        library: &str,
        foundation: &str,
        constants: Vec<ConstantDecl>,
        theorems: Vec<NormalizedTheorem>,
    ) -> Self {
        Self {
            system: system.to_string(),
            library: library.to_string(),
            foundation: foundation.to_string(),
            constants,
            theorems,
        }
    }

    /// Prefix used to keep an unaligned constant of this library distinct from
    /// any constant of the other one. Two unaligned constants must never render
    /// to the same pattern token, or the matcher would invent agreement it has
    /// no evidence for.
    fn tag(&self) -> String {
        format!("{}::{}", self.system, self.library)
    }

    fn symbol(&self, name: &str) -> SymbolRef {
        SymbolRef::new(&self.system, &self.library, &self.foundation, name)
    }

    fn declared(&self) -> BTreeMap<String, ConceptShape> {
        self.constants
            .iter()
            .map(|c| (c.name.clone(), c.shape.clone()))
            .collect()
    }
}

// ===========================================================================
// Configuration
// ===========================================================================

/// Knobs, all deterministic. There is no threshold knob on purpose: thresholding
/// is a decision, and decisions about alignments belong to grading and
/// refutation, not to the proposer (review section 10, items 1 to 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatcherConfig {
    /// Hard cap on substitution rounds. Each round consumes one constant from
    /// each side, so the loop also terminates on pool exhaustion; the cap exists
    /// so that a caller can bound the work explicitly and so that a single-pass
    /// run (`max_rounds = 1`) is expressible.
    pub max_rounds: usize,
    /// Divide by `ln(2 + n_left * n_right)`, where `n` is how many patterns each
    /// constant participates in. This is the review's `score_2`, which penalizes
    /// constants that appear in so many theorems that sharing patterns with them
    /// is unsurprising. `score_2` with iteration was the configuration that
    /// reached rank 113 before its first incorrect match on the HOL Light / HOL4
    /// pair, the best reported ranking quality (review section 1.2).
    pub penalize_prolific: bool,
}

impl Default for MatcherConfig {
    fn default() -> Self {
        Self {
            max_rounds: 64,
            penalize_prolific: true,
        }
    }
}

// ===========================================================================
// Output
// ===========================================================================

/// A proposal plus why it was made.
///
/// The proposal itself is a [`ProposedAlignment`] and is the only thing a
/// consumer should act on, by handing it to
/// [`super::alignment::grade_proposal`] or to
/// [`super::alignment::Alignment::propose`]. Everything else on this struct is
/// provenance for a human reading the ranked list, which is what the original
/// method assumes happens (review section 1.4: "It needs a human to read the
/// ranked list").
#[derive(Debug, Clone, PartialEq)]
pub struct PatternEvidence {
    pub proposal: ProposedAlignment,
    /// Which substitution round accepted this pair, counting from 1. This is
    /// also the rank: see [`propose_with_evidence`] for why the returned list is
    /// in acceptance order rather than sorted by score.
    pub round: usize,
    /// How many patterns the two constants had in common when accepted.
    pub shared_patterns: usize,
    /// The weight of the single rarest shared pattern. A pair carried by one
    /// rare property is a different kind of evidence from a pair carried by many
    /// common ones, and collapsing both into the score would hide that.
    pub rarest_shared_weight: f64,
    /// Aliases of earlier accepted pairs that appear inside the shared patterns,
    /// so this proposal exists only because those were accepted first. Empty for
    /// a pair that a single pass would also have found. This is the audit trail
    /// for the error-propagation problem in the module docs.
    pub enabled_by: Vec<String>,
}

impl PatternEvidence {
    /// The sentence that belongs next to any display of this proposal. Written
    /// here rather than at each call site so that no call site gets to phrase it
    /// more optimistically, matching the discipline
    /// [`super::alignment::Refutation::caveat`] already follows.
    pub fn caveat(&self) -> &'static str {
        "a scored candidate, not a claim: this proposer's published precision is 80 to 94 percent \
         within one logic and its recall was never measured, so this pair must be graded and \
         probed before anything depends on it"
    }
}

// ===========================================================================
// The matcher
// ===========================================================================

/// Propose alignments between two normalized corpora. Pure.
///
/// The list is in the matcher's own acceptance order, which is its ranking.
/// See [`propose_with_evidence`] for the full record.
pub fn propose_alignments(
    left: &LibraryCorpus,
    right: &LibraryCorpus,
    config: &MatcherConfig,
) -> Vec<ProposedAlignment> {
    propose_with_evidence(left, right, config)
        .into_iter()
        .map(|e| e.proposal)
        .collect()
}

/// Propose alignments, keeping the evidence for each.
///
/// ## Why the result is in acceptance order and not sorted by score
///
/// Scores from different rounds are not comparable. Round `k + 1` is computed
/// over a pattern set that round `k`'s substitution changed, and over a smaller
/// pool, so a later pair can carry a numerically larger score while resting on
/// strictly more assumptions. Sorting by score would put the most derived
/// proposals at the top of the list, which inverts the property the original
/// evaluation measured (rank of the first incorrect match, review section 1.2).
/// Acceptance order is greedy-best-first within each round and is exactly the
/// rank the published numbers describe.
pub fn propose_with_evidence(
    left: &LibraryCorpus,
    right: &LibraryCorpus,
    config: &MatcherConfig,
) -> Vec<PatternEvidence> {
    let left_declared = left.declared();
    let right_declared = right.declared();

    // Active constants: not yet accepted into a pair. Accepted constants leave
    // the pool because the method substitutes them away in both libraries.
    let mut left_active: BTreeSet<String> = left_declared.keys().cloned().collect();
    let mut right_active: BTreeSet<String> = right_declared.keys().cloned().collect();

    // name -> shared fresh symbol, for pairs accepted so far.
    let mut left_alias: BTreeMap<String, String> = BTreeMap::new();
    let mut right_alias: BTreeMap<String, String> = BTreeMap::new();

    let mut out: Vec<PatternEvidence> = Vec::new();

    for round in 1..=config.max_rounds {
        if left_active.is_empty() || right_active.is_empty() {
            break;
        }

        let left_index = build_index(left, &left_active, &left_declared, &left_alias);
        let right_index = build_index(right, &right_active, &right_declared, &right_alias);

        let Some(best) = best_pair(
            &left_index,
            &right_index,
            &left_declared,
            &right_declared,
            config,
        ) else {
            // No type-compatible pair carries any weighted evidence at all.
            // Stopping here rather than emitting the least-bad pair is the
            // degenerate-corpus guarantee: no evidence yields no proposal.
            break;
        };

        let alias = format!("#a{round}");
        let evidence = PatternEvidence {
            proposal: ProposedAlignment {
                left: left.symbol(&best.left_name),
                right: right.symbol(&best.right_name),
                left_shape: left_declared[&best.left_name].clone(),
                right_shape: right_declared[&best.right_name].clone(),
                proposer: Proposer::PropertyPattern,
                score: Some(best.score),
            },
            round,
            shared_patterns: best.shared.len(),
            rarest_shared_weight: best.rarest_weight,
            enabled_by: aliases_mentioned(&best.shared),
        };
        out.push(evidence);

        left_active.remove(&best.left_name);
        right_active.remove(&best.right_name);
        left_alias.insert(best.left_name.clone(), alias.clone());
        right_alias.insert(best.right_name.clone(), alias);
    }

    out
}

// ===========================================================================
// Patterns
// ===========================================================================

/// The pattern set of a library at one point in the iteration.
struct PatternIndex {
    /// pattern -> the constants participating in it.
    by_pattern: BTreeMap<String, BTreeSet<String>>,
    /// constant -> the patterns it participates in.
    by_constant: BTreeMap<String, BTreeSet<String>>,
    /// How many constants participate in at least one pattern. This, not the
    /// declared count, is the population a rarity ratio is taken against: a
    /// constant that appears in no theorem cannot make a pattern look rarer.
    population: usize,
}

/// Render one theorem as the pattern of one constant.
///
/// Three token fates, and the distinction between the last two is the whole
/// bootstrap:
///
/// * the constant of interest becomes a hole, so the pattern describes a
///   PROPERTY rather than a fact;
/// * an already-aligned constant becomes the shared fresh symbol, so the two
///   libraries now agree here and downstream patterns can match;
/// * anything else stays library-qualified, so two unaligned constants can never
///   accidentally look identical across libraries.
fn render_pattern(
    tokens: &[PatternToken],
    focus: &str,
    tag: &str,
    alias: &BTreeMap<String, String>,
) -> String {
    let parts: Vec<String> = tokens
        .iter()
        .map(|token| match token {
            PatternToken::Shared(s) => format!("s:{s}"),
            PatternToken::Constant(name) if name == focus => "_".to_string(),
            PatternToken::Constant(name) => match alias.get(name) {
                Some(a) => format!("a:{a}"),
                None => format!("c:{tag}:{name}"),
            },
        })
        .collect();
    parts.join("|")
}

fn build_index(
    corpus: &LibraryCorpus,
    active: &BTreeSet<String>,
    declared: &BTreeMap<String, ConceptShape>,
    alias: &BTreeMap<String, String>,
) -> PatternIndex {
    let tag = corpus.tag();
    let mut by_pattern: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut by_constant: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for theorem in &corpus.theorems {
        // Distinct constants of this theorem that are still candidates. Sorted,
        // because a BTreeSet is the only iteration order this module trusts.
        let focuses: BTreeSet<&String> = theorem
            .tokens
            .iter()
            .filter_map(|t| match t {
                PatternToken::Constant(name) => Some(name),
                PatternToken::Shared(_) => None,
            })
            .filter(|name| active.contains(*name) && declared.contains_key(*name))
            .collect();

        for focus in focuses {
            let pattern = render_pattern(&theorem.tokens, focus, &tag, alias);
            by_pattern
                .entry(pattern.clone())
                .or_default()
                .insert(focus.clone());
            by_constant
                .entry(focus.clone())
                .or_default()
                .insert(pattern);
        }
    }

    let population = by_constant.len();
    PatternIndex {
        by_pattern,
        by_constant,
        population,
    }
}

/// Collect the aliases of earlier accepted pairs mentioned by a pattern set.
fn aliases_mentioned(patterns: &BTreeSet<String>) -> Vec<String> {
    let mut found: BTreeSet<String> = BTreeSet::new();
    for pattern in patterns {
        for part in pattern.split('|') {
            if let Some(alias) = part.strip_prefix("a:") {
                found.insert(alias.to_string());
            }
        }
    }
    found.into_iter().collect()
}

// ===========================================================================
// Weighting and scoring
// ===========================================================================

/// The weight of one shared pattern.
///
/// Base rule, the review's `w_1` (section 1.1, step 3) applied to both sides:
/// `1 / (|C_left(p)| * |C_right(p)|)`, where `C_lib(p)` is the set of constants
/// of that library participating in `p`. The justification is a coincidence
/// argument rather than an aesthetic one. If a pattern is held by `n_l`
/// constants on the left and `n_r` on the right, then the number of pairs it is
/// consistent with grows as `n_l * n_r`, so the evidence any single one of those
/// pairs gets from it falls off as the reciprocal. A property held by exactly
/// one constant on each side points at exactly one pair and scores 1. A property
/// held by ten on each side is consistent with a hundred pairs and scores 0.01.
///
/// The zero rule on top of it: a pattern that EVERY constant of a library
/// participates in distinguishes nothing there, so when that holds on both sides
/// the weight is exactly zero rather than merely small. Reflexivity is the
/// canonical case. Making it exactly zero rather than `1/(n*n)` is what stops a
/// degenerate corpus (every constant with the same single property) from
/// producing a full ranked list of arbitrary pairs, which would be pure noise
/// wearing a score. The rule is suppressed when a side has fewer than two
/// participating constants, since with one constant there is nothing to
/// discriminate between and universality carries no information either way.
///
/// This is a sharpening of the original weighting, which the authors themselves
/// call ad hoc (review section 1.3, "Weighting is ad hoc"). It is stated here as
/// a rule rather than tuned, because there is no held-out ground truth in this
/// repository to tune against and a tuned constant with no data behind it would
/// be worse than an explicit one.
fn pattern_weight(left: &PatternIndex, right: &PatternIndex, pattern: &str) -> f64 {
    let left_holders = left.by_pattern.get(pattern).map_or(0, |s| s.len());
    let right_holders = right.by_pattern.get(pattern).map_or(0, |s| s.len());
    if left_holders == 0 || right_holders == 0 {
        return 0.0;
    }

    let universal_left = left.population >= 2 && left_holders >= left.population;
    let universal_right = right.population >= 2 && right_holders >= right.population;
    if universal_left && universal_right {
        return 0.0;
    }

    1.0 / ((left_holders * right_holders) as f64)
}

struct Candidate {
    left_name: String,
    right_name: String,
    score: f64,
    shared: BTreeSet<String>,
    rarest_weight: f64,
}

/// Score every type-compatible pair and return the best, or `None` when no pair
/// carries a positive weight.
///
/// Ties are broken by name, lexicographically, so the result does not depend on
/// the order the caller listed constants or theorems in.
fn best_pair(
    left: &PatternIndex,
    right: &PatternIndex,
    left_declared: &BTreeMap<String, ConceptShape>,
    right_declared: &BTreeMap<String, ConceptShape>,
    config: &MatcherConfig,
) -> Option<Candidate> {
    let mut best: Option<Candidate> = None;

    for (left_name, left_patterns) in &left.by_constant {
        let Some(left_shape) = left_declared.get(left_name) else {
            continue;
        };
        for (right_name, right_patterns) in &right.by_constant {
            let Some(right_shape) = right_declared.get(right_name) else {
                continue;
            };
            // The hard gate. A pair that fails it is never proposed, never
            // substituted, and never scored, so it cannot contaminate a later
            // round either.
            if !shapes_compatible(left_shape, right_shape) {
                continue;
            }

            let shared: BTreeSet<String> = left_patterns
                .intersection(right_patterns)
                .cloned()
                .collect();
            if shared.is_empty() {
                continue;
            }

            let mut score = 0.0_f64;
            let mut rarest = 0.0_f64;
            for pattern in &shared {
                let weight = pattern_weight(left, right, pattern);
                score += weight;
                if weight > rarest {
                    rarest = weight;
                }
            }
            if score <= 0.0 {
                continue;
            }
            if config.penalize_prolific {
                let breadth = (left_patterns.len() * right_patterns.len()) as f64;
                score /= (2.0 + breadth).ln();
            }

            let candidate = Candidate {
                left_name: left_name.clone(),
                right_name: right_name.clone(),
                score,
                shared,
                rarest_weight: rarest,
            };
            let better = match &best {
                None => true,
                Some(current) => match candidate.score.total_cmp(&current.score) {
                    std::cmp::Ordering::Greater => true,
                    std::cmp::Ordering::Less => false,
                    std::cmp::Ordering::Equal => {
                        (&candidate.left_name, &candidate.right_name)
                            < (&current.left_name, &current.right_name)
                    }
                },
            };
            if better {
                best = Some(candidate);
            }
        }
    }

    best
}

// ===========================================================================
// Type compatibility
// ===========================================================================

/// Are two operand kinds allowed to sit opposite each other?
///
/// Equal kinds, or either side [`OperandKind::Opaque`]. `Opaque` is the
/// exporter saying "I could not classify this", and refusing every such pair
/// would discard candidates over a data-quality artifact rather than over
/// mathematics. It widens the gate but not the claim: whatever comes through
/// still leaves here as a hypothesis that must be graded and probed.
fn kinds_compatible(a: OperandKind, b: OperandKind) -> bool {
    a == b || a == OperandKind::Opaque || b == OperandKind::Opaque
}

/// The gate: same arity, compatible operand kinds, compatible result kind.
///
/// Totality is deliberately NOT part of this check. A partial `/` against a
/// total `/` is precisely the pair the review holds up as the canonical honest
/// alignment (section 3.3), and rejecting it here would throw away the one case
/// [`super::alignment::grade_proposal`] exists to record properly. Compatibility
/// is about whether the two sides range over the same things; agreement is the
/// grader's question.
pub fn shapes_compatible(left: &ConceptShape, right: &ConceptShape) -> bool {
    left.operands.len() == right.operands.len()
        && left
            .operands
            .iter()
            .zip(right.operands.iter())
            .all(|(a, b)| kinds_compatible(*a, *b))
        && kinds_compatible(left.result, right.result)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::alignment::{grade_proposal, Alignment, Refutation, Totality};

    /// Token helper: a leading "@" marks a library-local constant.
    fn tok(raw: &str) -> PatternToken {
        match raw.strip_prefix('@') {
            Some(name) => PatternToken::Constant(name.to_string()),
            None => PatternToken::Shared(raw.to_string()),
        }
    }

    fn thm(id: &str, raw: &[&str]) -> NormalizedTheorem {
        NormalizedTheorem::new(id, raw.iter().map(|t| tok(t)).collect())
    }

    /// A binary operation on naturals, total. The shape most constants in these
    /// fixtures have, so that shape never accidentally does the matching.
    fn binop() -> ConceptShape {
        ConceptShape::new(
            vec![OperandKind::Natural, OperandKind::Natural],
            OperandKind::Natural,
            Totality::total("total"),
        )
    }

    fn decls(names: &[&str]) -> Vec<ConstantDecl> {
        names
            .iter()
            .map(|n| ConstantDecl::new(n, binop()))
            .collect()
    }

    #[test]
    fn a_rare_shared_pattern_outranks_a_common_one() {
        // c1, c2, c3 all share one common property ("comm"); c4 alone has a rare
        // one ("nilpotent"). Same on the other side. Every pair is
        // type-compatible, so shape cannot be doing the work. c5 and d5 hold
        // properties that match nothing across the libraries; they exist only so
        // that "comm" is not held by literally every constant, which would make
        // it uninformative by the zero rule and take it out of the comparison
        // this test is about.
        let left = LibraryCorpus::new(
            "hol4",
            "core",
            "hol",
            decls(&["c1", "c2", "c3", "c4", "c5"]),
            vec![
                thm("l1", &["comm", "@c1", "@c1"]),
                thm("l2", &["comm", "@c2", "@c2"]),
                thm("l3", &["comm", "@c3", "@c3"]),
                thm("l4", &["nilpotent", "@c4"]),
                thm("l5", &["assoc", "@c5"]),
            ],
        );
        let right = LibraryCorpus::new(
            "hol_light",
            "core",
            "hol",
            decls(&["d1", "d2", "d3", "d4", "d5"]),
            vec![
                thm("r1", &["comm", "@d1", "@d1"]),
                thm("r2", &["comm", "@d2", "@d2"]),
                thm("r3", &["comm", "@d3", "@d3"]),
                thm("r4", &["nilpotent", "@d4"]),
                thm("r5", &["distrib", "@d5"]),
            ],
        );

        let found = propose_with_evidence(&left, &right, &MatcherConfig::default());
        assert!(found.len() >= 2, "expected several proposals, got {found:?}");

        // The rare property wins the first round outright.
        assert_eq!(found[0].proposal.left.name, "c4");
        assert_eq!(found[0].proposal.right.name, "d4");
        assert_eq!(found[0].round, 1);

        // And it is not a near-tie: one property held by one constant on each
        // side outweighs one held by three on each side by a factor of nine
        // before the breadth penalty.
        let rare = found[0].proposal.score.unwrap();
        let common = found[1].proposal.score.unwrap();
        assert!(
            rare > common * 4.0,
            "rare {rare} should dominate common {common}"
        );
        assert!((found[0].rarest_shared_weight - 1.0).abs() < 1e-12);
        assert!(found[1].rarest_shared_weight < 0.2);

        // A pattern every constant holds is worth exactly zero, so the common
        // group is carried by rarity only relative to the whole pool.
        assert_eq!(found[0].enabled_by, Vec::<String>::new());
    }

    /// Two libraries where the second pair is invisible until the first is
    /// substituted: `sq` is characterized only by a theorem that also mentions
    /// `add`, and `square` only by one that also mentions `plus`.
    fn bootstrap_corpora() -> (LibraryCorpus, LibraryCorpus) {
        let left = LibraryCorpus::new(
            "hol4",
            "arith",
            "hol",
            vec![
                ConstantDecl::new("add", binop()),
                ConstantDecl::new("sq", binop()),
            ],
            vec![
                thm("l_comm", &["comm", "@add", "@add"]),
                thm("l_sq", &["eq", "@sq", "@add"]),
            ],
        );
        let right = LibraryCorpus::new(
            "hol_light",
            "arith",
            "hol",
            vec![
                ConstantDecl::new("plus", binop()),
                ConstantDecl::new("square", binop()),
            ],
            vec![
                thm("r_comm", &["comm", "@plus", "@plus"]),
                thm("r_sq", &["eq", "@square", "@plus"]),
            ],
        );
        (left, right)
    }

    #[test]
    fn iterative_substitution_finds_a_pair_a_single_pass_misses() {
        let (left, right) = bootstrap_corpora();

        let single_pass = propose_with_evidence(
            &left,
            &right,
            &MatcherConfig {
                max_rounds: 1,
                ..MatcherConfig::default()
            },
        );
        assert_eq!(single_pass.len(), 1, "one pass sees one pair");
        assert_eq!(single_pass[0].proposal.left.name, "add");
        assert_eq!(single_pass[0].proposal.right.name, "plus");

        let iterated = propose_with_evidence(&left, &right, &MatcherConfig::default());
        assert_eq!(iterated.len(), 2, "iteration reveals the second pair");
        assert_eq!(iterated[1].proposal.left.name, "sq");
        assert_eq!(iterated[1].proposal.right.name, "square");
        assert_eq!(iterated[1].round, 2);
        // And the record says why it became visible.
        assert_eq!(iterated[1].enabled_by, vec!["#a1".to_string()]);
        assert!(iterated[0].enabled_by.is_empty());

        // Termination: the pool is exhausted, so a huge cap changes nothing.
        let uncapped = propose_with_evidence(
            &left,
            &right,
            &MatcherConfig {
                max_rounds: 10_000,
                ..MatcherConfig::default()
            },
        );
        assert_eq!(uncapped.len(), 2);
    }

    #[test]
    fn type_incompatible_pairs_are_never_proposed() {
        // Identical property structure on both sides, so the pattern evidence is
        // as strong as it gets. Only the shapes disagree.
        let set_op = ConceptShape::new(
            vec![OperandKind::Set],
            OperandKind::Natural,
            Totality::total("total"),
        );
        let real_op = ConceptShape::new(
            vec![OperandKind::Real],
            OperandKind::Real,
            Totality::total("total"),
        );
        let left = LibraryCorpus::new(
            "hol4",
            "sets",
            "hol",
            vec![ConstantDecl::new("card", set_op)],
            vec![thm("l", &["idem", "@card"])],
        );
        let right = LibraryCorpus::new(
            "hol_light",
            "reals",
            "hol",
            vec![ConstantDecl::new("abs", real_op)],
            vec![thm("r", &["idem", "@abs"])],
        );
        assert!(propose_alignments(&left, &right, &MatcherConfig::default()).is_empty());

        // Arity is part of the gate too: a unary against a binary is out even
        // when the kinds line up.
        let unary = ConceptShape::new(
            vec![OperandKind::Natural],
            OperandKind::Natural,
            Totality::total("total"),
        );
        let left_unary = LibraryCorpus::new(
            "hol4",
            "arith",
            "hol",
            vec![ConstantDecl::new("neg", unary)],
            vec![thm("l", &["idem", "@neg"])],
        );
        let right_binary = LibraryCorpus::new(
            "hol_light",
            "arith",
            "hol",
            vec![ConstantDecl::new("add", binop())],
            vec![thm("r", &["idem", "@add"])],
        );
        assert!(propose_alignments(&left_unary, &right_binary, &MatcherConfig::default()).is_empty());

        // Opaque is the exporter's "unknown" and is allowed through, still only
        // as a hypothesis.
        let opaque = ConceptShape::new(
            vec![OperandKind::Opaque],
            OperandKind::Opaque,
            Totality::total("total"),
        );
        let left_opaque = LibraryCorpus::new(
            "hol4",
            "sets",
            "hol",
            vec![ConstantDecl::new("card", opaque)],
            vec![thm("l", &["idem", "@card"])],
        );
        assert_eq!(
            propose_alignments(&left_opaque, &right_binary, &MatcherConfig::default()).len(),
            0,
            "arity still gates an opaque unary against a binary"
        );
    }

    #[test]
    fn output_is_deterministic_under_input_reordering() {
        let (left, right) = bootstrap_corpora();
        let config = MatcherConfig::default();

        let baseline = propose_with_evidence(&left, &right, &config);
        assert_eq!(baseline, propose_with_evidence(&left, &right, &config));

        // Reverse the order the caller happened to list constants and theorems
        // in. Nothing mathematical changed, so nothing may change.
        let mut shuffled_left = left.clone();
        shuffled_left.constants.reverse();
        shuffled_left.theorems.reverse();
        let mut shuffled_right = right.clone();
        shuffled_right.constants.reverse();
        shuffled_right.theorems.reverse();
        assert_eq!(
            baseline,
            propose_with_evidence(&shuffled_left, &shuffled_right, &config)
        );

        // A genuine tie must also resolve the same way every time. Two
        // indistinguishable constants on each side, listed in both orders.
        let tied_left = LibraryCorpus::new(
            "hol4",
            "core",
            "hol",
            decls(&["a1", "a2"]),
            vec![thm("t1", &["comm", "@a1"]), thm("t2", &["comm", "@a2"])],
        );
        let tied_right = LibraryCorpus::new(
            "hol_light",
            "core",
            "hol",
            decls(&["b1", "b2"]),
            vec![thm("u1", &["comm", "@b1"]), thm("u2", &["comm", "@b2"])],
        );
        let tied = propose_with_evidence(&tied_left, &tied_right, &config);
        let mut reversed_left = tied_left.clone();
        reversed_left.constants.reverse();
        reversed_left.theorems.reverse();
        assert_eq!(tied, propose_with_evidence(&reversed_left, &tied_right, &config));
        if let Some(first) = tied.first() {
            assert_eq!(first.proposal.left.name, "a1");
            assert_eq!(first.proposal.right.name, "b1");
        }
    }

    #[test]
    fn degenerate_corpora_yield_no_proposals_rather_than_noise() {
        let config = MatcherConfig::default();
        let empty = LibraryCorpus::new("hol4", "core", "hol", vec![], vec![]);
        assert!(propose_alignments(&empty, &empty, &config).is_empty());

        // Constants declared, no theorems: no properties, so no evidence.
        let no_theorems_left =
            LibraryCorpus::new("hol4", "core", "hol", decls(&["c1", "c2"]), vec![]);
        let no_theorems_right =
            LibraryCorpus::new("hol_light", "core", "hol", decls(&["d1", "d2"]), vec![]);
        assert!(propose_alignments(&no_theorems_left, &no_theorems_right, &config).is_empty());

        // The reflexivity case: every constant on both sides holds exactly the
        // same single property. That distinguishes nothing, so the honest output
        // is nothing at all rather than an arbitrary ranked list of the nine
        // possible pairs.
        let flat_left = LibraryCorpus::new(
            "hol4",
            "core",
            "hol",
            decls(&["c1", "c2", "c3"]),
            vec![
                thm("l1", &["refl", "@c1"]),
                thm("l2", &["refl", "@c2"]),
                thm("l3", &["refl", "@c3"]),
            ],
        );
        let flat_right = LibraryCorpus::new(
            "hol_light",
            "core",
            "hol",
            decls(&["d1", "d2", "d3"]),
            vec![
                thm("r1", &["refl", "@d1"]),
                thm("r2", &["refl", "@d2"]),
                thm("r3", &["refl", "@d3"]),
            ],
        );
        assert!(propose_alignments(&flat_left, &flat_right, &config).is_empty());

        // One side with no shared vocabulary at all: patterns cannot coincide.
        let disjoint_right = LibraryCorpus::new(
            "hol_light",
            "core",
            "hol",
            decls(&["d1"]),
            vec![thm("r1", &["assoc", "@d1"])],
        );
        let one_sided = LibraryCorpus::new(
            "hol4",
            "core",
            "hol",
            decls(&["c1"]),
            vec![thm("l1", &["refl", "@c1"])],
        );
        assert!(propose_alignments(&one_sided, &disjoint_right, &config).is_empty());
    }

    #[test]
    fn every_proposal_goes_through_grading_and_stays_a_hypothesis() {
        let (left, right) = bootstrap_corpora();
        for proposal in propose_alignments(&left, &right, &MatcherConfig::default()) {
            assert_eq!(proposal.proposer, Proposer::PropertyPattern);
            assert!(proposal.score.is_some(), "the raw score travels as provenance");

            // The output type is the one the grader consumes, and grading a
            // proposal can never reach an unrestricted claim.
            let strength = grade_proposal(&proposal);
            assert!(!strength.is_unrestricted());

            // Entering the record leaves it unprobed, which is where refutation
            // starts. Nothing here shortcuts that.
            let alignment = Alignment::propose(proposal);
            assert_eq!(alignment.refutation, Refutation::NotProbed);
            assert!(alignment.retrieval_hint().is_some());
        }
    }

    #[test]
    fn the_api_offers_no_way_to_treat_a_score_as_a_decision() {
        let (left, right) = bootstrap_corpora();
        let found = propose_with_evidence(&left, &right, &MatcherConfig::default());
        let top = found.first().expect("at least one proposal");
        assert!(top.caveat().contains("not a claim"));
        assert!(top.caveat().contains("must be graded and probed"));

        // Scores are unnormalized sums of reciprocals, so nothing about them
        // looks like a probability that a caller could threshold at 0.9.
        let score = top.proposal.score.unwrap();
        assert!(score > 0.0);
    }
}
