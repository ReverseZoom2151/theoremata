//! Informal-proof defect prior: a risk scorer over informal (LaTeX / prose)
//! proof text that flags the spans most likely to hide a real defect.
//!
//! # Where the weights come from — read this before trusting a number
//!
//! These weights are a **PRIOR derived from a single diffed case study**, not a
//! measured distribution. We diffed one 599-line informal LaTeX draft against
//! the 9,938-line Lean formalization that repaired it. n = 1. Every weight below
//! is a judgement call about how expensive that *one* paper's defects were, and
//! it should be **recalibrated the moment we have more paired informal/formal
//! data**. Do not read `risk_score` as a probability; read it as an ordering
//! hint. If a downstream component starts making irreversible decisions off
//! these numbers, that component is over-trusting them.
//!
//! What the case study actually showed:
//!
//! * The **analytic core of the informal proof was correct**. No defect sat in
//!   the hard analysis. Every defect sat at the *counting / elementary-estimate*
//!   boundary — exactly the places a human author considers beneath comment.
//! * The single most expensive defect was the parenthetical *"the claimed bound
//!   may be checked directly for 2 <= n <= 9"*. That clause cost roughly **400
//!   lines of Lean** to replace with a uniform argument. Hence
//!   [`DefectCategory::HandWavedFiniteCheck`] carries the top weight.
//! * A sieve computation that was **commented out in the LaTeX source** (`%`
//!   lines carrying real math) silently dropped an additive error term. A
//!   commented-out computation is a computation the author *ran and then hid*;
//!   it is evidence, not noise. Hence [`DefectCategory::OmittedComputation`].
//! * The five words *"since phi(n) -> infinity"* cost roughly **150 lines**.
//! * A subregion introduced as "necessary" was a **red herring** — defined, then
//!   never load-bearing. Hence [`DefectCategory::IntroducedNotion`], weighted
//!   lowest: it costs wasted effort, not unsoundness.
//!
//! # What this module is for
//!
//! The harness uses the findings two ways, via [`RiskReport::to_routing_hints`]:
//!
//! * regions whose defect is a **concrete computational claim** (a finite check,
//!   a hidden computation) are routed to [`Route::Falsify`] first — they are
//!   cheap to test numerically, and the falsify-before-prove policy in
//!   [`crate::router`] is exactly the right gate;
//! * regions whose defect is a **missing argument** (a standard estimate, an
//!   asymptotic hand-wave, an unjustified reduction) are routed to
//!   [`Route::Decompose`] — the fix is more obligations, not a counterexample.
//!
//! # Purity
//!
//! Everything here is pure: no IO, no clock, no RNG, no allocation-order
//! dependence. [`scan`] returns findings in a deterministic order (span start,
//! then category). Matching is ASCII-case-insensitive via
//! `to_ascii_lowercase`, which is byte-length preserving, so all reported spans
//! index the *original* text correctly even when it contains non-ASCII math.
//!
//! The `regex` crate is deliberately not used — it is not a dependency of this
//! crate and this does not justify adding one. Matching is plain substring plus
//! two small structural scanners.

use crate::router::Route;

/// The kind of defect a pattern indicates.
///
/// Ordering is stable and is used as the tie-breaker in [`scan`]'s output
/// ordering, so do not reorder variants casually — tests assert on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DefectCategory {
    /// "checked directly for 2 <= n <= 9", "by inspection", "routine check".
    /// The most expensive category in the case study (~400 Lean lines).
    HandWavedFiniteCheck,
    /// A LaTeX comment line carrying actual math: a computation the author ran
    /// and then hid. In the case study this dropped an additive error term.
    OmittedComputation,
    /// "since phi(n) -> infinity", "for sufficiently large n", "eventually".
    /// Five words that cost ~150 Lean lines.
    AsymptoticHandWave,
    /// "standard estimate", "well known", "it is classical".
    StandardEstimate,
    /// "without loss of generality", "clearly", "obviously", "trivially".
    UnjustifiedReduction,
    /// A notion introduced (named) and then never used again — a red herring
    /// that wastes formalization effort rather than breaking soundness.
    IntroducedNotion,
}

impl DefectCategory {
    /// Whether a defect of this kind states something *concrete enough to test*.
    /// Those get the cheap counterexample gate before any proof effort; the rest
    /// need more structure, not a numerical probe.
    pub fn prefers_falsification(self) -> bool {
        matches!(self, Self::HandWavedFiniteCheck | Self::OmittedComputation)
    }

    /// The route a router should try first for this defect kind.
    pub fn preferred_route(self) -> Route {
        if self.prefers_falsification() {
            Route::Falsify
        } else {
            Route::Decompose
        }
    }

    /// Stable snake_case label.
    pub fn label(self) -> &'static str {
        match self {
            Self::HandWavedFiniteCheck => "hand_waved_finite_check",
            Self::OmittedComputation => "omitted_computation",
            Self::AsymptoticHandWave => "asymptotic_hand_wave",
            Self::StandardEstimate => "standard_estimate",
            Self::UnjustifiedReduction => "unjustified_reduction",
            Self::IntroducedNotion => "introduced_notion",
        }
    }
}

/// One phrase-level detector.
///
/// `phrase` is matched ASCII-case-insensitively as a plain substring. `weight`
/// is the prior described in the module docs.
#[derive(Debug, Clone, Copy)]
pub struct DefectPattern {
    pub category: DefectCategory,
    /// Lowercase needle. Must be lowercase or it will never match.
    pub phrase: &'static str,
    pub weight: f64,
    /// Why this phrase is suspicious, in words a human reviewer can act on.
    pub rationale: &'static str,
}

/// Rationale reused by the finite-check family — the case study's headline.
const R_FINITE: &str =
    "A finite/small-case check the author waved through. In the diffed case study this exact \
     move ('may be checked directly for 2 <= n <= 9') cost ~400 lines of Lean, because the \
     formal proof had to replace the enumeration with a uniform argument. Treat the bound as \
     unproven and test it numerically first.";

const R_ASYMPTOTIC: &str =
    "An asymptotic claim asserted without an effective threshold. 'since phi(n) -> infinity' \
     cost ~150 lines of Lean: the formal proof needs an explicit N and a monotonicity or \
     lower-bound lemma that the prose never supplies.";

const R_STANDARD: &str =
    "An appeal to a 'standard' or 'classical' fact with no citation and no statement. Either \
     the fact exists in the library (then name it) or it must be proved. This is where a \
     retrieval step, not a proof step, should run.";

const R_WLOG: &str =
    "A reduction asserted rather than performed. 'WLOG'/'clearly'/'obviously' hides a symmetry \
     or case-split argument that the formal proof must actually carry out.";

/// The full phrase catalogue.
///
/// Longer phrases are allowed to overlap shorter ones; [`scan`] resolves
/// overlaps greedily (earliest start, then longest match) so a single defect is
/// never counted twice.
pub fn patterns() -> &'static [DefectPattern] {
    const P: &[DefectPattern] = &[
        // ---- HandWavedFiniteCheck -------------------------------------------
        DefectPattern {
            category: DefectCategory::HandWavedFiniteCheck,
            phrase: "may be checked directly",
            weight: 1.0,
            rationale: R_FINITE,
        },
        DefectPattern {
            category: DefectCategory::HandWavedFiniteCheck,
            phrase: "checked directly",
            weight: 0.9,
            rationale: R_FINITE,
        },
        DefectPattern {
            category: DefectCategory::HandWavedFiniteCheck,
            phrase: "may be verified for small",
            weight: 1.0,
            rationale: R_FINITE,
        },
        DefectPattern {
            category: DefectCategory::HandWavedFiniteCheck,
            phrase: "verified for small",
            weight: 0.9,
            rationale: R_FINITE,
        },
        DefectPattern {
            category: DefectCategory::HandWavedFiniteCheck,
            phrase: "for small values",
            weight: 0.7,
            rationale: R_FINITE,
        },
        DefectPattern {
            category: DefectCategory::HandWavedFiniteCheck,
            phrase: "by inspection",
            weight: 0.8,
            rationale: R_FINITE,
        },
        DefectPattern {
            category: DefectCategory::HandWavedFiniteCheck,
            phrase: "routine check",
            weight: 0.8,
            rationale: R_FINITE,
        },
        DefectPattern {
            category: DefectCategory::HandWavedFiniteCheck,
            phrase: "routine verification",
            weight: 0.8,
            rationale: R_FINITE,
        },
        DefectPattern {
            category: DefectCategory::HandWavedFiniteCheck,
            phrase: "a direct computation shows",
            weight: 0.7,
            rationale: R_FINITE,
        },
        // The literal small-range guard, in ASCII and LaTeX spellings. These are
        // the textual fingerprint of "enumerate the small cases".
        DefectPattern {
            category: DefectCategory::HandWavedFiniteCheck,
            phrase: "n <= ",
            weight: 0.7,
            rationale: R_FINITE,
        },
        DefectPattern {
            category: DefectCategory::HandWavedFiniteCheck,
            phrase: "n \\le ",
            weight: 0.7,
            rationale: R_FINITE,
        },
        DefectPattern {
            category: DefectCategory::HandWavedFiniteCheck,
            phrase: "n \\leq ",
            weight: 0.7,
            rationale: R_FINITE,
        },
        // ---- StandardEstimate ------------------------------------------------
        DefectPattern {
            category: DefectCategory::StandardEstimate,
            phrase: "standard estimate",
            weight: 0.5,
            rationale: R_STANDARD,
        },
        DefectPattern {
            category: DefectCategory::StandardEstimate,
            phrase: "standard argument",
            weight: 0.5,
            rationale: R_STANDARD,
        },
        DefectPattern {
            category: DefectCategory::StandardEstimate,
            phrase: "it is well known",
            weight: 0.5,
            rationale: R_STANDARD,
        },
        DefectPattern {
            category: DefectCategory::StandardEstimate,
            phrase: "well known",
            weight: 0.45,
            rationale: R_STANDARD,
        },
        DefectPattern {
            category: DefectCategory::StandardEstimate,
            phrase: "well-known",
            weight: 0.45,
            rationale: R_STANDARD,
        },
        DefectPattern {
            category: DefectCategory::StandardEstimate,
            phrase: "it is classical",
            weight: 0.5,
            rationale: R_STANDARD,
        },
        DefectPattern {
            category: DefectCategory::StandardEstimate,
            phrase: "by a classical result",
            weight: 0.5,
            rationale: R_STANDARD,
        },
        // ---- AsymptoticHandWave ---------------------------------------------
        DefectPattern {
            category: DefectCategory::AsymptoticHandWave,
            phrase: "tends to infinity",
            weight: 0.6,
            rationale: R_ASYMPTOTIC,
        },
        DefectPattern {
            category: DefectCategory::AsymptoticHandWave,
            phrase: "-> infinity",
            weight: 0.6,
            rationale: R_ASYMPTOTIC,
        },
        DefectPattern {
            category: DefectCategory::AsymptoticHandWave,
            phrase: "\\to \\infty",
            weight: 0.6,
            rationale: R_ASYMPTOTIC,
        },
        DefectPattern {
            category: DefectCategory::AsymptoticHandWave,
            phrase: "\\to\\infty",
            weight: 0.6,
            rationale: R_ASYMPTOTIC,
        },
        DefectPattern {
            category: DefectCategory::AsymptoticHandWave,
            phrase: "for sufficiently large",
            weight: 0.6,
            rationale: R_ASYMPTOTIC,
        },
        DefectPattern {
            category: DefectCategory::AsymptoticHandWave,
            phrase: "sufficiently large",
            weight: 0.55,
            rationale: R_ASYMPTOTIC,
        },
        DefectPattern {
            category: DefectCategory::AsymptoticHandWave,
            phrase: "for large enough",
            weight: 0.55,
            rationale: R_ASYMPTOTIC,
        },
        DefectPattern {
            category: DefectCategory::AsymptoticHandWave,
            phrase: "eventually",
            weight: 0.4,
            rationale: R_ASYMPTOTIC,
        },
        // ---- UnjustifiedReduction -------------------------------------------
        DefectPattern {
            category: DefectCategory::UnjustifiedReduction,
            phrase: "without loss of generality",
            weight: 0.4,
            rationale: R_WLOG,
        },
        DefectPattern {
            category: DefectCategory::UnjustifiedReduction,
            phrase: "clearly",
            weight: 0.35,
            rationale: R_WLOG,
        },
        DefectPattern {
            category: DefectCategory::UnjustifiedReduction,
            phrase: "obviously",
            weight: 0.35,
            rationale: R_WLOG,
        },
        DefectPattern {
            category: DefectCategory::UnjustifiedReduction,
            phrase: "trivially",
            weight: 0.35,
            rationale: R_WLOG,
        },
        DefectPattern {
            category: DefectCategory::UnjustifiedReduction,
            phrase: "it is easy to see",
            weight: 0.4,
            rationale: R_WLOG,
        },
    ];
    P
}

/// Weight for a commented-out computation. Structural, not phrase-based, so it
/// lives here rather than in [`patterns`].
const OMITTED_COMPUTATION_WEIGHT: f64 = 0.8;

const R_OMITTED: &str =
    "A LaTeX comment line containing actual mathematics: a computation the author ran and then \
     hid rather than deleted. In the diffed case study a commented-out sieve computation was \
     where an additive error term went missing. Recover it and check it explicitly.";

/// Weight for a notion introduced and never reused.
const INTRODUCED_NOTION_WEIGHT: f64 = 0.3;

const R_NOTION: &str =
    "A notion is named here and never referred to again. In the case study a subregion \
     introduced as 'necessary' was a red herring. This is a cost risk, not a soundness risk: \
     do not spend formalization effort on it until something depends on it.";

/// One flagged span.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DefectFinding {
    /// Byte offset into the ORIGINAL text (inclusive).
    pub start: usize,
    /// Byte offset into the ORIGINAL text (exclusive).
    pub end: usize,
    pub category: DefectCategory,
    /// Prior weight; see the module docs on how little this is worth.
    pub weight: f64,
    /// The matched text, sliced from the original (original casing preserved).
    pub matched: String,
    pub rationale: &'static str,
}

impl DefectFinding {
    /// The route a router should try first for this finding.
    pub fn preferred_route(&self) -> Route {
        self.category.preferred_route()
    }
}

/// A merged region of nearby findings — what a router actually acts on.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RiskRegion {
    pub start: usize,
    pub end: usize,
    /// Sum of the member findings' weights.
    pub weight: f64,
    /// Categories present, sorted and deduplicated.
    pub categories: Vec<DefectCategory>,
    /// Indices into the [`RiskReport::findings`] this region was built from.
    pub finding_indices: Vec<usize>,
    /// The route to try first for this region: [`Route::Falsify`] if ANY member
    /// finding is a concrete computational claim, else [`Route::Decompose`].
    pub route: Route,
}

/// Findings from one pass over one document, plus the aggregate score.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RiskReport {
    /// Deterministically ordered: by span start, then category.
    pub findings: Vec<DefectFinding>,
    /// Normalized to `[0, 1)`; see [`risk_score`].
    pub score: f64,
}

/// Structured hints for a router. Regions are highest-risk first within each
/// bucket.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RoutingHints {
    /// Regions to probe with the cheap counterexample gate before any proof
    /// effort — concrete finite checks and hidden computations.
    pub falsify_first: Vec<RiskRegion>,
    /// Regions to break into obligations — missing arguments, not testable
    /// claims.
    pub decompose_first: Vec<RiskRegion>,
    /// The document-level score, carried so a caller can compare documents.
    pub overall_risk: f64,
}

/// Findings closer together than this many bytes are merged into one region.
const REGION_MERGE_GAP: usize = 120;

/// The saturation constant in [`risk_score`]. A total weight of `K` maps to
/// ~0.63. Arbitrary, chosen so a handful of medium findings lands mid-scale —
/// another number to recalibrate against real data.
const SCORE_SATURATION: f64 = 3.0;

/// Scan `text` for defect-prone spans.
///
/// Returns findings sorted by `(start, category)`. Overlapping matches are
/// resolved greedily — earliest start wins, and among equal starts the longest
/// match wins — so a single defect is reported once, not once per pattern that
/// happened to fire on it.
pub fn scan(text: &str) -> Vec<DefectFinding> {
    let lower = text.to_ascii_lowercase();
    debug_assert_eq!(lower.len(), text.len(), "lowercasing must preserve offsets");

    // (start, end, category, weight, rationale)
    let mut candidates: Vec<(usize, usize, DefectCategory, f64, &'static str)> = Vec::new();

    for pat in patterns() {
        let mut from = 0usize;
        while let Some(rel) = lower[from..].find(pat.phrase) {
            let start = from + rel;
            let end = start + pat.phrase.len();
            candidates.push((start, end, pat.category, pat.weight, pat.rationale));
            // Advance by one byte so adjacent/overlapping occurrences of the
            // SAME phrase are all found; the greedy pass below dedups.
            from = start + 1;
            if from >= lower.len() {
                break;
            }
        }
    }

    candidates.extend(scan_commented_computations(text));
    candidates.extend(scan_introduced_notions(text, &lower));

    // Deterministic candidate order: earliest start, then longest span, then
    // category, then heavier weight. No f64 NaN can appear (all weights are
    // literals), so `total_cmp` is exact and total.
    candidates.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(b.1.cmp(&a.1))
            .then(a.2.cmp(&b.2))
            .then(b.3.total_cmp(&a.3))
    });

    let mut findings = Vec::new();
    let mut consumed_to = 0usize;
    for (start, end, category, weight, rationale) in candidates {
        if start < consumed_to {
            // Overlaps an already-accepted finding: same defect, don't double
            // count it.
            continue;
        }
        consumed_to = end;
        findings.push(DefectFinding {
            start,
            end,
            category,
            weight,
            matched: text[start..end].to_string(),
            rationale,
        });
    }

    // The greedy pass already emits ascending starts; sort again to make the
    // documented `(start, category)` contract explicit and independent of it.
    findings.sort_by(|a, b| a.start.cmp(&b.start).then(a.category.cmp(&b.category)));
    findings
}

/// Detect LaTeX comment lines that carry mathematics — a computation the author
/// commented out. A comment containing only prose is NOT a finding.
fn scan_commented_computations(
    text: &str,
) -> Vec<(usize, usize, DefectCategory, f64, &'static str)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut line_start = 0usize;

    while line_start <= bytes.len() {
        let line_end = match text[line_start..].find('\n') {
            Some(rel) => line_start + rel,
            None => bytes.len(),
        };
        let line = &text[line_start..line_end];

        // A LaTeX comment we care about starts the line (after whitespace) with
        // an UNESCAPED '%'. A trailing `x % comment` is usually a note about the
        // line it follows, not a hidden computation, so we ignore those.
        let trimmed_off = line.len() - line.trim_start().len();
        let body_start = line_start + trimmed_off;
        let rest = &text[body_start..line_end];
        if rest.starts_with('%') && !(body_start > 0 && bytes[body_start - 1] == b'\\') {
            let comment = rest.trim_start_matches('%');
            if comment_contains_math(comment) {
                out.push((
                    body_start,
                    line_end,
                    DefectCategory::OmittedComputation,
                    OMITTED_COMPUTATION_WEIGHT,
                    R_OMITTED,
                ));
            }
        }

        if line_end >= bytes.len() {
            break;
        }
        line_start = line_end + 1;
    }
    out
}

/// Whether a LaTeX comment body looks like mathematics rather than prose.
///
/// Deliberately conservative: a prose note ("% TODO: rewrite this paragraph")
/// must not fire, because a false positive here sends the harness chasing a
/// computation that does not exist.
fn comment_contains_math(comment: &str) -> bool {
    const MATH_COMMANDS: &[&str] = &[
        "\\sum",
        "\\prod",
        "\\int",
        "\\frac",
        "\\sqrt",
        "\\le",
        "\\ge",
        "\\leq",
        "\\geq",
        "\\cdot",
        "\\log",
        "\\phi",
        "\\varphi",
        "\\infty",
        "\\alpha",
        "\\epsilon",
        "\\left",
        "\\big",
        "\\equiv",
        "\\approx",
        "\\lfloor",
        "\\mathbb",
    ];
    if comment.contains('$') || comment.contains("\\[") || comment.contains("\\(") {
        return true;
    }
    let lower = comment.to_ascii_lowercase();
    if MATH_COMMANDS.iter().any(|c| lower.contains(c)) {
        return true;
    }
    // A relation between things, at least one of which is a numeral: `x = 3`,
    // `2 <= n`. Prose rarely does this; hidden arithmetic always does.
    let has_relation = comment.contains('=')
        || comment.contains("<=")
        || comment.contains(">=")
        || comment.contains('<')
        || comment.contains('>');
    has_relation && comment.chars().any(|c| c.is_ascii_digit())
}

/// Definitional cues. A notion is "introduced" when one of these is followed
/// closely by a delimited name.
const DEFINITION_CUES: &[&str] = &[
    "we call",
    "we shall call",
    "we say that",
    "is called",
    "are called",
    "we define",
    "we introduce",
    "call such",
    "refer to as",
];

/// How far past a cue we look for the delimited name.
const CUE_LOOKAHEAD: usize = 160;

/// Detect a notion that is named and then never mentioned again.
///
/// Only fires when the name is explicitly delimited (`\emph{...}`,
/// `\textit{...}`, `\emph`-less quotes). Undelimited "we define f to be ..."
/// is skipped on purpose — guessing the name from free prose produces false
/// positives, and a false positive here costs the harness a wasted decomposition.
fn scan_introduced_notions(
    text: &str,
    lower: &str,
) -> Vec<(usize, usize, DefectCategory, f64, &'static str)> {
    let mut out = Vec::new();

    for cue in DEFINITION_CUES {
        let mut from = 0usize;
        while let Some(rel) = lower[from..].find(cue) {
            let cue_start = from + rel;
            let cue_end = cue_start + cue.len();
            from = cue_start + 1;

            let window_end = (cue_end + CUE_LOOKAHEAD).min(text.len());
            // Never split a multi-byte char.
            let window_end = floor_char_boundary(text, window_end);
            if window_end <= cue_end {
                continue;
            }
            let Some((name_start, name_end)) = find_delimited_name(text, cue_end, window_end)
            else {
                continue;
            };
            let name = text[name_start..name_end].trim();
            if name.is_empty() || !name.chars().any(|c| c.is_alphabetic()) {
                continue;
            }
            // Used again anywhere after the definition?
            let name_lower = name.to_ascii_lowercase();
            let tail_start = name_end.min(lower.len());
            if lower[tail_start..].contains(&name_lower) {
                continue;
            }
            out.push((
                cue_start,
                name_end,
                DefectCategory::IntroducedNotion,
                INTRODUCED_NOTION_WEIGHT,
                R_NOTION,
            ));
            break; // one finding per cue phrase is enough signal
        }
    }
    out
}

/// Find the first delimited name in `text[from..to]`, returning the byte span of
/// the name ITSELF (delimiters excluded).
fn find_delimited_name(text: &str, from: usize, to: usize) -> Option<(usize, usize)> {
    let window = &text[from..to];
    const BRACED: &[&str] = &["\\emph{", "\\textit{", "\\textbf{", "\\definition{"];
    let mut best: Option<(usize, usize)> = None;

    for open in BRACED {
        if let Some(rel) = window.find(open) {
            let inner = rel + open.len();
            if let Some(close_rel) = window[inner..].find('}') {
                let cand = (from + inner, from + inner + close_rel);
                best = Some(match best {
                    Some(b) if b.0 <= cand.0 => b,
                    _ => cand,
                });
            }
        }
    }
    // TeX-style quotes ``name'' and plain "name".
    for (open, close) in [("``", "''"), ("\"", "\"")] {
        if let Some(rel) = window.find(open) {
            let inner = rel + open.len();
            if let Some(close_rel) = window[inner..].find(close) {
                let cand = (from + inner, from + inner + close_rel);
                best = Some(match best {
                    Some(b) if b.0 <= cand.0 => b,
                    _ => cand,
                });
            }
        }
    }
    best
}

/// `str::floor_char_boundary` is unstable; this is the same thing.
fn floor_char_boundary(text: &str, mut idx: usize) -> usize {
    if idx >= text.len() {
        return text.len();
    }
    while idx > 0 && !text.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// Aggregate findings into a normalized risk score in `[0, 1)`.
///
/// `1 - exp(-total_weight / SCORE_SATURATION)`: zero on an empty finding set,
/// strictly increasing in both the number of findings and their weights, and
/// saturating so one pathological document cannot dominate a comparison. It is
/// an ORDERING, not a probability — see the module docs.
pub fn risk_score(findings: &[DefectFinding]) -> f64 {
    let total: f64 = findings.iter().map(|f| f.weight).sum();
    if total <= 0.0 {
        return 0.0;
    }
    1.0 - (-total / SCORE_SATURATION).exp()
}

/// Scan and score in one call.
pub fn analyze(text: &str) -> RiskReport {
    let findings = scan(text);
    let score = risk_score(&findings);
    RiskReport { findings, score }
}

/// Merge nearby findings into regions, highest total weight first.
///
/// Ties break on ascending start offset, so the output is fully deterministic.
pub fn rank_spans(findings: &[DefectFinding]) -> Vec<RiskRegion> {
    let mut regions: Vec<RiskRegion> = Vec::new();

    // `findings` is already start-ascending (scan guarantees it), but do not
    // rely on the caller having used `scan` — index by sorted order explicitly.
    let mut order: Vec<usize> = (0..findings.len()).collect();
    order.sort_by(|&a, &b| {
        findings[a]
            .start
            .cmp(&findings[b].start)
            .then(findings[a].category.cmp(&findings[b].category))
    });

    for idx in order {
        let f = &findings[idx];
        match regions.last_mut() {
            Some(cur) if f.start <= cur.end.saturating_add(REGION_MERGE_GAP) => {
                cur.end = cur.end.max(f.end);
                cur.weight += f.weight;
                if !cur.categories.contains(&f.category) {
                    cur.categories.push(f.category);
                }
                cur.finding_indices.push(idx);
            }
            _ => regions.push(RiskRegion {
                start: f.start,
                end: f.end,
                weight: f.weight,
                categories: vec![f.category],
                finding_indices: vec![idx],
                route: Route::Decompose, // fixed up below
            }),
        }
    }

    for r in &mut regions {
        r.categories.sort();
        r.categories.dedup();
        // Any concrete, testable claim in the region pulls the whole region to
        // the cheap falsification gate first — that is the falsify-before-prove
        // policy from `crate::router`, applied at the text level.
        r.route = if r.categories.iter().any(|c| c.prefers_falsification()) {
            Route::Falsify
        } else {
            Route::Decompose
        };
    }

    regions.sort_by(|a, b| b.weight.total_cmp(&a.weight).then(a.start.cmp(&b.start)));
    regions
}

impl RiskReport {
    /// Split the ranked regions into the two router buckets.
    pub fn to_routing_hints(&self) -> RoutingHints {
        let regions = rank_spans(&self.findings);
        let (falsify_first, decompose_first): (Vec<_>, Vec<_>) =
            regions.into_iter().partition(|r| r.route == Route::Falsify);
        RoutingHints {
            falsify_first,
            decompose_first,
            overall_risk: self.score,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A paragraph of real, non-hand-wavy mathematics. Nothing should fire; the
    /// scorer is worthless if it flags clean prose.
    const CLEAN: &str = "Let p be an odd prime and let q denote the least quadratic \
        non-residue modulo p. Multiplying both sides of the congruence by the inverse of a \
        modulo p yields the stated identity, since a is invertible in the ring of residues. \
        The bound then follows from Lemma 3.2 applied with parameter t = 1/2, whose hypotheses \
        were verified in Section 2.";

    #[test]
    fn clean_paragraph_scores_zero() {
        let report = analyze(CLEAN);
        assert!(
            report.findings.is_empty(),
            "clean text produced findings: {:?}",
            report.findings
        );
        assert_eq!(report.score, 0.0);
    }

    fn categories(text: &str) -> Vec<DefectCategory> {
        scan(text).into_iter().map(|f| f.category).collect()
    }

    #[test]
    fn hand_waved_finite_check_fires() {
        // The headline defect from the case study, verbatim.
        let cats = categories("the claimed bound may be checked directly for 2 <= n <= 9.");
        assert!(cats.contains(&DefectCategory::HandWavedFiniteCheck));
    }

    #[test]
    fn finite_check_variants_fire() {
        for t in [
            "This may be verified for small n.",
            "The remaining case is settled by inspection.",
            "We omit the routine check.",
        ] {
            assert!(
                categories(t).contains(&DefectCategory::HandWavedFiniteCheck),
                "no finite-check finding in {t:?}"
            );
        }
    }

    #[test]
    fn standard_estimate_fires() {
        for t in [
            "By the standard estimate for the divisor function, the sum is small.",
            "It is well known that this series converges.",
            "It is classical that no such configuration exists.",
            "A standard argument now gives the result.",
        ] {
            assert!(
                categories(t).contains(&DefectCategory::StandardEstimate),
                "no standard-estimate finding in {t:?}"
            );
        }
    }

    #[test]
    fn asymptotic_hand_wave_fires() {
        // The five words that cost ~150 lines of Lean.
        assert!(
            categories("since phi(n) -> infinity, the term is negligible")
                .contains(&DefectCategory::AsymptoticHandWave)
        );
        for t in [
            "The quantity tends to infinity with n.",
            "For sufficiently large n the bound holds.",
            "The inequality eventually holds.",
            "As $n \\to \\infty$ the error vanishes.",
        ] {
            assert!(
                categories(t).contains(&DefectCategory::AsymptoticHandWave),
                "no asymptotic finding in {t:?}"
            );
        }
    }

    #[test]
    fn unjustified_reduction_fires() {
        for t in [
            "Without loss of generality we may assume a < b.",
            "Clearly the map is injective.",
            "The claim obviously holds.",
            "This trivially implies the result.",
        ] {
            assert!(
                categories(t).contains(&DefectCategory::UnjustifiedReduction),
                "no reduction finding in {t:?}"
            );
        }
    }

    #[test]
    fn commented_out_computation_fires_on_math_not_prose() {
        let math = "Some prose.\n% \\sum_{n \\le x} \\phi(n) = 3x^2/\\pi^2 + O(x \\log x)\nMore.";
        assert!(
            categories(math).contains(&DefectCategory::OmittedComputation),
            "commented-out sieve computation was not flagged"
        );

        // A prose comment is NOT a hidden computation.
        let prose = "Some prose.\n% TODO: rewrite this paragraph before submission\nMore prose.";
        assert!(
            !categories(prose).contains(&DefectCategory::OmittedComputation),
            "prose comment was wrongly flagged as a computation"
        );
    }

    #[test]
    fn commented_arithmetic_fires_but_bare_prose_does_not() {
        assert!(categories("% here x = 12 and the total was 40\n")
            .contains(&DefectCategory::OmittedComputation));
        assert!(
            !categories("% ask Alice whether this section should stay\n")
                .contains(&DefectCategory::OmittedComputation)
        );
    }

    #[test]
    fn escaped_percent_is_not_a_comment() {
        // `\%` is a literal percent sign in LaTeX, not a comment.
        let t = "The density is 5\\% = 1/20 of the total.";
        assert!(!categories(t).contains(&DefectCategory::OmittedComputation));
    }

    #[test]
    fn introduced_notion_fires_only_when_never_reused() {
        // The red-herring subregion: named once, never load-bearing again.
        let dangling = "We call such a subregion \\emph{critical}. The proof proceeds by \
                        bounding the main term directly.";
        assert!(
            categories(dangling).contains(&DefectCategory::IntroducedNotion),
            "a notion defined and never reused was not flagged"
        );

        // Same sentence, but the notion actually carries weight later.
        let used = "We call such a subregion \\emph{critical}. Every critical subregion \
                    contributes at most one to the count, so the critical part is bounded.";
        assert!(
            !categories(used).contains(&DefectCategory::IntroducedNotion),
            "a notion that IS reused was wrongly flagged"
        );
    }

    #[test]
    fn findings_are_deterministically_ordered() {
        let text = "Clearly the bound may be checked directly for 2 <= n <= 9, and it is well \
                    known that the error tends to infinity.";
        let a = scan(text);
        let b = scan(text);
        assert_eq!(a, b, "scan is not deterministic");
        assert!(a.len() >= 3);
        for w in a.windows(2) {
            assert!(
                (w[0].start, w[0].category) <= (w[1].start, w[1].category),
                "findings not sorted by (start, category): {:?} then {:?}",
                w[0],
                w[1]
            );
        }
    }

    #[test]
    fn overlapping_matches_do_not_double_count() {
        // "may be checked directly" and "checked directly" both match here.
        let f = scan("the bound may be checked directly.");
        let finite: Vec<_> = f
            .iter()
            .filter(|x| x.category == DefectCategory::HandWavedFiniteCheck)
            .collect();
        assert_eq!(finite.len(), 1, "double-counted overlapping matches: {f:?}");
        // The longer, more specific phrase wins.
        assert_eq!(finite[0].matched, "may be checked directly");
    }

    #[test]
    fn spans_index_the_original_text() {
        let text = "Écrivons: clearly the map is injective.";
        for f in scan(text) {
            assert_eq!(&text[f.start..f.end], f.matched);
        }
    }

    #[test]
    fn risk_score_is_monotone_in_count() {
        let one = scan("Clearly this holds.");
        let two = scan("Clearly this holds. Obviously that holds too.");
        assert!(two.len() > one.len());
        assert!(risk_score(&two) > risk_score(&one));
        assert!(risk_score(&[]) == 0.0);
    }

    #[test]
    fn risk_score_is_monotone_in_weight_and_bounded() {
        let light = scan("Clearly this holds.");
        let heavy = scan("The bound may be checked directly.");
        assert!(heavy[0].weight > light[0].weight);
        assert!(risk_score(&heavy) > risk_score(&light));
        // Bounded below 1 no matter how bad the document is.
        let awful = scan(&"Clearly it may be checked directly. ".repeat(50));
        assert!(risk_score(&awful) < 1.0);
        assert!(risk_score(&awful) > 0.99);
    }

    #[test]
    fn rank_spans_orders_by_weight_and_merges_neighbours() {
        let text = "Clearly this is fine.\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\
                    \n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\
                    \n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\
                    \n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\
                    The bound may be checked directly.";
        let regions = rank_spans(&scan(text));
        assert_eq!(regions.len(), 2, "distant findings should not merge");
        assert!(regions[0].weight >= regions[1].weight);
        assert_eq!(regions[0].route, Route::Falsify);
        assert_eq!(regions[1].route, Route::Decompose);
    }

    #[test]
    fn nearby_findings_merge_into_one_region() {
        let regions = rank_spans(&scan("Clearly it is well known that this holds."));
        assert_eq!(regions.len(), 1);
        assert!(regions[0].finding_indices.len() >= 2);
    }

    #[test]
    fn routing_hints_split_by_testability() {
        let text = "It is well known that the sum converges.\n\
                    % \\sum_{n \\le 9} f(n) = 12\n\
                    The bound may be checked directly.";
        let hints = analyze(text).to_routing_hints();
        assert!(
            !hints.falsify_first.is_empty(),
            "concrete computational claims must be routed to falsification first"
        );
        for r in &hints.falsify_first {
            assert_eq!(r.route, Route::Falsify);
            assert!(r.categories.iter().any(|c| c.prefers_falsification()));
        }
        for r in &hints.decompose_first {
            assert_eq!(r.route, Route::Decompose);
            assert!(r.categories.iter().all(|c| !c.prefers_falsification()));
        }
        assert!(hints.overall_risk > 0.0);
        // Buckets are ordered highest-risk first.
        for w in hints.falsify_first.windows(2) {
            assert!(w[0].weight >= w[1].weight);
        }
    }

    #[test]
    fn category_route_mapping_matches_the_case_study() {
        assert_eq!(
            DefectCategory::HandWavedFiniteCheck.preferred_route(),
            Route::Falsify
        );
        assert_eq!(
            DefectCategory::OmittedComputation.preferred_route(),
            Route::Falsify
        );
        assert_eq!(
            DefectCategory::AsymptoticHandWave.preferred_route(),
            Route::Decompose
        );
        assert_eq!(
            DefectCategory::StandardEstimate.preferred_route(),
            Route::Decompose
        );
        assert_eq!(
            DefectCategory::UnjustifiedReduction.preferred_route(),
            Route::Decompose
        );
        assert_eq!(
            DefectCategory::IntroducedNotion.preferred_route(),
            Route::Decompose
        );
    }

    #[test]
    fn every_pattern_phrase_is_lowercase_and_fires_on_itself() {
        for p in patterns() {
            assert_eq!(
                p.phrase,
                p.phrase.to_ascii_lowercase(),
                "pattern phrase must be lowercase: {:?}",
                p.phrase
            );
            let findings = scan(p.phrase);
            assert!(
                !findings.is_empty(),
                "pattern {:?} does not fire on its own phrase",
                p.phrase
            );
        }
    }

    #[test]
    fn empty_text_is_safe() {
        assert!(scan("").is_empty());
        assert_eq!(analyze("").score, 0.0);
        assert!(rank_spans(&[]).is_empty());
    }
}
