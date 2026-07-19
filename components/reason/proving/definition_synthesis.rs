//! On-the-fly definition-synthesis scaffold (the "Aristotle can define the
//! structure you're talking about on the fly" pattern — for fields Mathlib lacks).
//!
//! When a formal statement references a structure / definition that the corpus
//! (the library / `known_symbols`) does not yet provide, we cannot prove it —
//! the symbol is undefined. This module builds the SCAFFOLD that:
//!
//! 1. DETECTS the missing symbols — parses a formal statement for referenced
//!    identifiers that are absent from a provided `known_symbols` set
//!    ([`detect_missing`]);
//! 2. PROPOSES candidate definitions for each missing symbol via an injected
//!    [`DefinitionProposer`] (a deterministic mock in tests; a model in
//!    production);
//! 3. SCREENS each candidate via an injected [`DefinitionScreen`] — conceptually a
//!    compile-check ("would this type-check as a definition?") plus a
//!    non-degeneracy check ("is it non-trivial / not vacuous?"); and
//! 4. DEDUPs canonically-identical candidates and RECOMMENDS the best well-formed,
//!    non-degenerate candidate per symbol — **never auto-committing**. Every
//!    candidate is returned with its flags for a human or the next stage to
//!    admit (e.g. into [`crate::library::LemmaLibrary`]).
//!
//! This mirrors the injected-model-seam + screen pattern of
//! [`crate::formalize_portfolio`] (fan-out → screen → dedup → recommend-first,
//! never auto-commit) and the deterministic-mock / untrusted-text discipline of
//! [`crate::sketch`]. Both the [`DefinitionProposer`] and the [`DefinitionScreen`]
//! are injected, and generation is threaded an explicit `seed` (never wall-clock
//! or an unseeded RNG), so a synthesis run is reproducible.
//!
//! HONEST SCOPE: this is a scaffold. The architecture is buildable and testable
//! now behind the injected seams, but it cannot produce *good* definitions
//! without a live model wired into [`DefinitionProposer`] and a real
//! compiler/non-triviality gate wired into [`DefinitionScreen`]. All proposed
//! `def_source` text is untrusted data: it is only ever stored, screened, and
//! reported — never executed here.

use crate::db::Store;
use crate::library::ProposedLemma;
use crate::model::ModelRequest;
use crate::provider::ModelProvider;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeSet;

/// A referenced identifier that the corpus / library does not define.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissingSymbol {
    /// The referenced identifier (e.g. `IsPerfectoid`).
    pub name: String,
    /// The statement text in which it was referenced (audit context for the
    /// proposer — untrusted data, carried verbatim, never executed).
    pub context: String,
}

/// Logical / syntactic keywords that are NOT definition references: they name no
/// structure the corpus could be missing, so a bare occurrence never counts as a
/// missing symbol. Kept small and conservative — a real corpus would supply the
/// full vocabulary via `known_symbols`.
const KEYWORD_STOPLIST: &[&str] = &[
    "forall", "exists", "fun", "let", "in", "if", "then", "else", "match", "with", "and", "or",
    "not", "iff", "True", "False", "Prop", "Type", "Sort", "by",
];

/// Scan `statement` for referenced identifiers absent from `known_symbols`.
///
/// Deterministic and purely string/identifier-level. An identifier is a maximal
/// run beginning with an ASCII letter or `_` and continuing with alphanumerics,
/// `_`, `'`, or `.` (so qualified names like `Foo.bar` stay whole). A token is a
/// *definition-like reference* — and thus a candidate missing symbol — iff it:
///   * is longer than one character (single letters are treated as bound
///     variables, not definitions);
///   * is not in the [`KEYWORD_STOPLIST`]; and
///   * is not already in `known_symbols`.
///
/// Results are de-duplicated and returned in first-seen order (stable). The
/// `context` of every hit is the trimmed statement.
pub fn detect_missing(statement: &str, known_symbols: &BTreeSet<String>) -> Vec<MissingSymbol> {
    let context = statement.trim().to_string();
    let mut out: Vec<MissingSymbol> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for token in identifier_tokens(statement) {
        if token.chars().count() <= 1 {
            continue; // bound variable / index, not a definition reference.
        }
        if KEYWORD_STOPLIST.contains(&token.as_str()) {
            continue;
        }
        if known_symbols.contains(&token) {
            continue; // already defined in the corpus.
        }
        if seen.insert(token.clone()) {
            out.push(MissingSymbol {
                name: token,
                context: context.clone(),
            });
        }
    }
    out
}

/// Split a statement into identifier tokens (see [`detect_missing`] for the
/// grammar). Non-identifier characters are separators.
fn identifier_tokens(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        let start_ok = c.is_ascii_alphabetic() || c == '_';
        let cont_ok = c.is_ascii_alphanumeric() || c == '_' || c == '\'' || c == '.';
        if cur.is_empty() {
            if start_ok {
                cur.push(c);
            }
        } else if cont_ok {
            cur.push(c);
        } else {
            // Trim a trailing '.' so `Foo.` does not keep the separator dot.
            push_token(&mut tokens, &mut cur);
        }
    }
    push_token(&mut tokens, &mut cur);
    tokens
}

/// Flush the in-progress token, trimming any trailing `.` (a sentence/qualifier
/// separator rather than part of the name).
fn push_token(tokens: &mut Vec<String>, cur: &mut String) {
    if cur.is_empty() {
        return;
    }
    let trimmed = cur.trim_end_matches('.').to_string();
    if !trimmed.is_empty() {
        tokens.push(trimmed);
    }
    cur.clear();
}

/// A candidate definition for a missing symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateDef {
    /// The symbol this defines (should match the [`MissingSymbol::name`]).
    pub name: String,
    /// The definition source (untrusted model text — stored/screened, never run).
    pub def_source: String,
    /// A short docstring / rationale for the definition.
    pub doc: String,
}

impl CandidateDef {
    /// Map a chosen definition into the [`ProposedLemma`] shape so a downstream
    /// stage can offer it to [`crate::library::LemmaLibrary::record_lemma`] for
    /// admission. This module never admits anything itself — a synthesized
    /// definition is only ever *proposed*; a human or the library's verifier
    /// gate decides. The `proof` is empty because a definition is admitted by
    /// well-formedness (the screen), not by a proof term.
    pub fn to_proposed_lemma(&self, provenance: impl Into<String>) -> ProposedLemma {
        ProposedLemma {
            statement: self.def_source.clone(),
            proof: String::new(),
            provenance: provenance.into(),
        }
    }
}

/// Proposes candidate definitions for a missing symbol (the model seam).
///
/// Injected: a deterministic mock in tests, a model in production. The `seed` is
/// threaded in for reproducibility — implementations MUST NOT read wall-clock
/// time or an unseeded RNG.
pub trait DefinitionProposer {
    /// Candidate definitions for `symbol`, generated under `seed`.
    fn propose(&self, symbol: &MissingSymbol, seed: u64) -> Vec<CandidateDef>;
}

/// The verdict of screening one candidate definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenResult {
    /// Would this compile as a well-formed *definition* (type-checks as a
    /// `def`/`structure`, independent of usefulness)?
    pub well_formed: bool,
    /// Is the definition degenerate — vacuous, trivially-true, or an empty
    /// structure that carries no content (the non-triviality check)?
    pub degenerate: bool,
    /// A short human-readable rationale for the two flags.
    pub note: String,
}

/// Screens a candidate definition for well-formedness + non-degeneracy.
///
/// Injected: the test mock is deterministic; production wires the real compiler
/// (well-formedness) and a non-triviality gate (degeneracy).
pub trait DefinitionScreen {
    /// Judge one candidate definition.
    fn screen(&self, def: &CandidateDef) -> ScreenResult;
}

/// One distinct, screened candidate definition (its flags surfaced for a human).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenedCandidateDef {
    /// The candidate definition.
    pub candidate: CandidateDef,
    /// Well-formedness verdict from the screen.
    pub well_formed: bool,
    /// Degeneracy verdict from the screen.
    pub degenerate: bool,
    /// The screen's rationale.
    pub note: String,
}

/// The synthesis outcome for one missing symbol: every screened candidate plus
/// an OPTIONAL recommendation — never an auto-commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SynthesizedSymbol {
    /// The missing symbol this addresses.
    pub symbol: MissingSymbol,
    /// The recommended definition — the FIRST well-formed, non-degenerate
    /// candidate — or `None` when no candidate qualifies (graceful). Advisory
    /// only: the caller inspects every candidate and chooses.
    pub chosen: Option<CandidateDef>,
    /// Every distinct candidate with its screen flags, in first-seen order.
    pub candidates: Vec<ScreenedCandidateDef>,
}

/// The full report: the detected missing symbols and, for each, its synthesis
/// outcome. Nothing here is committed to the corpus — this is a worklist for a
/// human / the next stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SynthesisReport {
    /// Every missing symbol detected in the statement.
    pub missing: Vec<MissingSymbol>,
    /// Per-symbol synthesis outcomes, aligned with `missing` (same order).
    pub synthesized: Vec<SynthesizedSymbol>,
}

/// Knobs for a synthesis run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthConfig {
    /// Seed threaded into [`DefinitionProposer::propose`] for reproducibility.
    pub seed: u64,
    /// Upper bound on candidates screened per symbol (generation is capped
    /// before dedup so a runaway proposer cannot blow up the screen budget).
    pub max_candidates_per_symbol: usize,
}

impl Default for SynthConfig {
    fn default() -> Self {
        SynthConfig {
            seed: 0,
            max_candidates_per_symbol: 8,
        }
    }
}

/// Detect the missing symbols in `statement`, propose + screen candidate
/// definitions for each, DEDUP identical `def_source`s, and RECOMMEND the first
/// well-formed, non-degenerate candidate per symbol (or `None`).
///
/// Nothing is auto-committed: the report is a worklist. A chosen candidate can be
/// mapped to a [`ProposedLemma`] via [`CandidateDef::to_proposed_lemma`] and
/// offered to the library for admission by a later stage.
pub fn synthesize_definitions(
    statement: &str,
    known_symbols: &BTreeSet<String>,
    proposer: &dyn DefinitionProposer,
    screen: &dyn DefinitionScreen,
    config: &SynthConfig,
) -> SynthesisReport {
    let missing = detect_missing(statement, known_symbols);

    let synthesized = missing
        .iter()
        .map(|symbol| synthesize_one(symbol, proposer, screen, config))
        .collect();

    SynthesisReport {
        missing,
        synthesized,
    }
}

/// Propose → cap → dedup → screen → recommend-first for a single missing symbol.
fn synthesize_one(
    symbol: &MissingSymbol,
    proposer: &dyn DefinitionProposer,
    screen: &dyn DefinitionScreen,
    config: &SynthConfig,
) -> SynthesizedSymbol {
    let raw = proposer.propose(symbol, config.seed);
    let mut seen_sources: Vec<String> = Vec::new();
    let mut candidates: Vec<ScreenedCandidateDef> = Vec::new();

    for candidate in raw.into_iter().take(config.max_candidates_per_symbol) {
        let key = candidate.def_source.trim().to_string();
        if key.is_empty() {
            continue; // an empty proposal carries no definition.
        }
        if seen_sources.iter().any(|s| s == &key) {
            continue; // a byte-identical proposal already kept.
        }
        seen_sources.push(key);

        let verdict = screen.screen(&candidate);
        candidates.push(ScreenedCandidateDef {
            candidate,
            well_formed: verdict.well_formed,
            degenerate: verdict.degenerate,
            note: verdict.note,
        });
    }

    // Recommend the FIRST well-formed, non-degenerate candidate — never a
    // degenerate or ill-formed one, and `None` (graceful) when none qualifies.
    let chosen = candidates
        .iter()
        .find(|c| c.well_formed && !c.degenerate)
        .map(|c| c.candidate.clone());

    SynthesizedSymbol {
        symbol: symbol.clone(),
        chosen,
        candidates,
    }
}

// ---------------------------------------------------------------------------
// CLI entry point: model-backed seams over the deterministic scaffold
// ---------------------------------------------------------------------------

/// A [`DefinitionProposer`] that asks an injected model for candidate
/// definitions. An offline or erroring provider yields NO candidates rather than
/// a fabricated one, so a missing definition simply stays on the worklist and is
/// never invented. The `seed` is threaded into the request context so a re-run
/// under the same seed asks for the same thing; the model is the only
/// nondeterminism.
struct ModelDefinitionProposer<'a> {
    provider: &'a dyn ModelProvider,
}

impl DefinitionProposer for ModelDefinitionProposer<'_> {
    fn propose(&self, symbol: &MissingSymbol, seed: u64) -> Vec<CandidateDef> {
        let request = ModelRequest {
            role: "definition_proposer".into(),
            task: "Propose candidate definitions for the missing symbol so a human \
                   can admit one. Each candidate is the source text of a single \
                   definition plus a one-line rationale. Do not prove anything."
                .into(),
            context: json!({
                "symbol": symbol.name,
                "statement_context": symbol.context,
                "seed": seed,
            }),
            output_schema: json!({
                "type":"object","required":["candidates"],"properties":{
                    "candidates":{"type":"array","items":{"type":"object",
                        "required":["name","def_source","doc"],
                        "properties":{
                            "name":{"type":"string"},
                            "def_source":{"type":"string"},
                            "doc":{"type":"string"}}}}}
            }),
        };
        // A provider error is an absence of evidence, not a signal: return an
        // empty candidate list so nothing is proposed for this symbol.
        let Ok(response) = self.provider.complete(&request) else {
            return Vec::new();
        };
        response.content["candidates"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|c| CandidateDef {
                        name: c["name"].as_str().unwrap_or(&symbol.name).into(),
                        def_source: c["def_source"].as_str().unwrap_or("").into(),
                        doc: c["doc"].as_str().unwrap_or("").into(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// A [`DefinitionScreen`] backed by a model. This is only a HEURISTIC advisory,
/// never a compiler's verdict (see the module scope note): a model asserting a
/// definition type-checks is not the same as it type-checking. It is admissible
/// here solely because this module never auto-commits, so a weak screen weakens
/// only the advisory recommendation and cannot admit anything. Fail closed: when
/// the screen cannot run, the candidate is marked NOT well-formed so it is never
/// recommended.
struct ModelDefinitionScreen<'a> {
    provider: &'a dyn ModelProvider,
}

impl DefinitionScreen for ModelDefinitionScreen<'_> {
    fn screen(&self, def: &CandidateDef) -> ScreenResult {
        let request = ModelRequest {
            role: "definition_screen".into(),
            task: "Judge whether this candidate definition is well-formed (would \
                   type-check as a definition) and whether it is degenerate \
                   (vacuous, trivially true, or content-free). Report both flags."
                .into(),
            context: json!({ "name": def.name, "def_source": def.def_source, "doc": def.doc }),
            output_schema: json!({
                "type":"object","required":["well_formed","degenerate"],"properties":{
                    "well_formed":{"type":"boolean"},
                    "degenerate":{"type":"boolean"},
                    "note":{"type":"string"}}
            }),
        };
        match self.provider.complete(&request) {
            Ok(response) => ScreenResult {
                // Absent flags default to the conservative verdict: unknown
                // well-formedness is treated as ill-formed, unknown degeneracy as
                // degenerate, so an under-specified response never recommends.
                well_formed: response.content["well_formed"].as_bool().unwrap_or(false),
                degenerate: response.content["degenerate"].as_bool().unwrap_or(true),
                note: response.content["note"]
                    .as_str()
                    .unwrap_or("model screen advisory")
                    .into(),
            },
            Err(_) => ScreenResult {
                well_formed: false,
                degenerate: true,
                note: "screen unavailable".into(),
            },
        }
    }
}

/// Detect the missing symbols in `statement`, ask the model to propose and screen
/// candidate definitions for each, and return an advisory JSON worklist. Nothing
/// is admitted to the corpus or the library: the recommendation per symbol is the
/// first well-formed, non-degenerate candidate, surfaced for a human to accept.
///
/// The known-symbols corpus is not yet wired, so detection runs against an EMPTY
/// known set: every non-keyword, multi-character identifier is surfaced as a
/// worklist item. That is the conservative (over-inclusive) default; a real
/// corpus would prune already-defined symbols.
///
/// Offline (no model) the proposer returns nothing, so the report is
/// detection-only: the missing symbols are listed with no candidates and no
/// recommendation.
///
/// Emits a `definition_synthesis.completed` store event and closes a run row.
pub fn synthesize(
    store: &Store,
    provider: &dyn ModelProvider,
    project_id: &str,
    statement: &str,
) -> anyhow::Result<serde_json::Value> {
    let run = store.begin_run(project_id, "definition_synthesis")?;

    let known: BTreeSet<String> = BTreeSet::new();
    let proposer = ModelDefinitionProposer { provider };
    let screen = ModelDefinitionScreen { provider };
    let report =
        synthesize_definitions(statement, &known, &proposer, &screen, &SynthConfig::default());

    let recommended = report
        .synthesized
        .iter()
        .filter(|s| s.chosen.is_some())
        .count();
    let offline = provider.name() == "offline";
    let state = if offline {
        "completed_no_model"
    } else {
        "completed"
    };

    let summary = json!({
        "project_id": project_id,
        "run_id": run,
        "statement": statement,
        "model": provider.name(),
        "missing_count": report.missing.len(),
        "recommended_count": recommended,
        "report": serde_json::to_value(&report)?,
        // The recommendation is advisory only; the screen is a model heuristic,
        // not a compiler gate, and nothing here is admitted anywhere.
        "caveat": "advisory worklist: screen flags are a model heuristic, not a compiler gate; nothing is admitted to the library",
    });

    store.event(
        Some(project_id),
        Some(&run),
        "definition_synthesis.completed",
        "definition_synthesis",
        json!({
            "missing_count": report.missing.len(),
            "recommended_count": recommended,
            "model": provider.name(),
        }),
    )?;
    store.update_run(project_id, &run, state, "complete", 0)?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelResponse;
    use crate::provider::OfflineProvider;
    use anyhow::Result;
    use std::path::Path;

    fn known(symbols: &[&str]) -> BTreeSet<String> {
        symbols.iter().map(|s| s.to_string()).collect()
    }

    /// A proposer that, for any symbol, emits one well-formed candidate followed
    /// by one degenerate one (plus a byte-identical duplicate of the first that
    /// must collapse). Ignores the seed — seed threading is covered separately.
    struct TwoCandidateProposer;
    impl DefinitionProposer for TwoCandidateProposer {
        fn propose(&self, symbol: &MissingSymbol, _seed: u64) -> Vec<CandidateDef> {
            let n = &symbol.name;
            vec![
                CandidateDef {
                    name: n.clone(),
                    def_source: format!("def {n} := structure with content"),
                    doc: format!("a non-degenerate {n}"),
                },
                CandidateDef {
                    name: n.clone(),
                    def_source: format!("def {n} := True"),
                    doc: format!("a vacuous {n}"),
                },
                // Byte-identical duplicate of the first → must dedup.
                CandidateDef {
                    name: n.clone(),
                    def_source: format!("def {n} := structure with content"),
                    doc: "duplicate".to_string(),
                },
            ]
        }
    }

    /// A screen: well-formed iff the source starts with `def `, degenerate iff it
    /// defines to `True` (a vacuous / content-free definition).
    struct MarkerScreen;
    impl DefinitionScreen for MarkerScreen {
        fn screen(&self, def: &CandidateDef) -> ScreenResult {
            let well_formed = def.def_source.trim_start().starts_with("def ");
            let degenerate = def.def_source.trim_end().ends_with(":= True");
            ScreenResult {
                well_formed,
                degenerate,
                note: format!("wf={well_formed} degenerate={degenerate}"),
            }
        }
    }

    /// A proposer whose output DEPENDS on the seed (deterministically, no RNG).
    struct SeedEchoProposer;
    impl DefinitionProposer for SeedEchoProposer {
        fn propose(&self, symbol: &MissingSymbol, seed: u64) -> Vec<CandidateDef> {
            vec![CandidateDef {
                name: symbol.name.clone(),
                def_source: format!("def {} := seed{seed}", symbol.name),
                doc: format!("derived from seed {seed}"),
            }]
        }
    }

    /// A proposer that produces nothing (models a proposer with no idea).
    struct EmptyProposer;
    impl DefinitionProposer for EmptyProposer {
        fn propose(&self, _symbol: &MissingSymbol, _seed: u64) -> Vec<CandidateDef> {
            Vec::new()
        }
    }

    #[test]
    fn unknown_symbol_is_flagged_as_missing() {
        let missing = detect_missing(
            "IsPerfectoid R -> Fintype R",
            &known(["Fintype"].as_slice()),
        );
        // `IsPerfectoid` is unknown (flagged); `Fintype` is known; `R` is a single
        // letter (bound variable, not a definition).
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].name, "IsPerfectoid");
        assert!(missing[0].context.contains("IsPerfectoid R"));
    }

    #[test]
    fn known_symbol_is_not_flagged() {
        let missing = detect_missing("Fintype R and Group G", &known(&["Fintype", "Group"]));
        // Both structures are in the corpus; only single-letter vars remain, which
        // are never flagged.
        assert!(
            missing.is_empty(),
            "known symbols must not be flagged, got {missing:?}"
        );
    }

    #[test]
    fn keywords_and_single_letters_are_not_flagged() {
        let missing = detect_missing("forall x, P x -> True", &known(&[]));
        // `forall`/`True` are keywords; `x`/`P` are single letters. Nothing left.
        assert!(missing.is_empty(), "got {missing:?}");
    }

    #[test]
    fn detection_is_deduped_and_first_seen_order() {
        let missing = detect_missing(
            "IsPerfectoid R and IsPerfectoid S and Perfectoidification R",
            &known(&[]),
        );
        let names: Vec<&str> = missing.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["IsPerfectoid", "Perfectoidification"]);
    }

    #[test]
    fn chosen_is_the_well_formed_non_degenerate_candidate() {
        let report = synthesize_definitions(
            "IsPerfectoid R",
            &known(&[]),
            &TwoCandidateProposer,
            &MarkerScreen,
            &SynthConfig::default(),
        );
        assert_eq!(report.missing.len(), 1);
        assert_eq!(report.synthesized.len(), 1);
        let syn = &report.synthesized[0];

        // The duplicate collapsed: two distinct candidates screened.
        assert_eq!(
            syn.candidates.len(),
            2,
            "byte-identical duplicate must dedup"
        );

        // The chosen one is well-formed and non-degenerate — the content-bearing
        // definition, not the `:= True` vacuous one.
        let chosen = syn.chosen.as_ref().expect("a good candidate exists");
        assert!(chosen.def_source.contains("structure with content"));
        assert!(!chosen.def_source.ends_with(":= True"));

        // Both candidates are retained with flags for the human/next stage.
        assert!(syn.candidates.iter().any(|c| c.degenerate));
        assert!(syn
            .candidates
            .iter()
            .any(|c| c.well_formed && !c.degenerate));
    }

    #[test]
    fn no_proposer_output_yields_none_gracefully() {
        let report = synthesize_definitions(
            "IsPerfectoid R",
            &known(&[]),
            &EmptyProposer,
            &MarkerScreen,
            &SynthConfig::default(),
        );
        let syn = &report.synthesized[0];
        assert!(syn.candidates.is_empty());
        assert!(syn.chosen.is_none(), "no candidate ⇒ chosen None, no panic");
    }

    #[test]
    fn all_degenerate_yields_no_recommendation() {
        // A proposer whose only candidate is degenerate: nothing is recommended.
        struct DegenerateOnly;
        impl DefinitionProposer for DegenerateOnly {
            fn propose(&self, symbol: &MissingSymbol, _seed: u64) -> Vec<CandidateDef> {
                vec![CandidateDef {
                    name: symbol.name.clone(),
                    def_source: format!("def {} := True", symbol.name),
                    doc: "vacuous".into(),
                }]
            }
        }
        let report = synthesize_definitions(
            "Widget R",
            &known(&[]),
            &DegenerateOnly,
            &MarkerScreen,
            &SynthConfig::default(),
        );
        let syn = &report.synthesized[0];
        assert_eq!(syn.candidates.len(), 1);
        assert!(syn.candidates[0].degenerate);
        assert!(
            syn.chosen.is_none(),
            "a degenerate candidate is never chosen"
        );
    }

    #[test]
    fn max_candidates_caps_before_dedup() {
        let cfg = SynthConfig {
            seed: 0,
            max_candidates_per_symbol: 1,
        };
        let report = synthesize_definitions(
            "IsPerfectoid R",
            &known(&[]),
            &TwoCandidateProposer,
            &MarkerScreen,
            &cfg,
        );
        // Only the first raw candidate is seen — the good one — and it is chosen.
        let syn = &report.synthesized[0];
        assert_eq!(syn.candidates.len(), 1);
        assert!(syn.chosen.is_some());
    }

    #[test]
    fn seeded_synthesis_is_deterministic_and_threads_the_seed() {
        let ks = known(&[]);
        let cfg7 = SynthConfig {
            seed: 7,
            max_candidates_per_symbol: 8,
        };
        let a = synthesize_definitions("Foo R", &ks, &SeedEchoProposer, &MarkerScreen, &cfg7);
        let b = synthesize_definitions("Foo R", &ks, &SeedEchoProposer, &MarkerScreen, &cfg7);
        assert_eq!(a, b, "same seed must yield an identical report");

        // A different seed threads through to different candidate text.
        let cfg8 = SynthConfig {
            seed: 8,
            max_candidates_per_symbol: 8,
        };
        let c = synthesize_definitions("Foo R", &ks, &SeedEchoProposer, &MarkerScreen, &cfg8);
        assert_ne!(a.synthesized, c.synthesized, "distinct seeds must diverge");
        assert!(a.synthesized[0]
            .candidates
            .iter()
            .any(|x| x.candidate.def_source.contains("seed7")));
        assert!(c.synthesized[0]
            .candidates
            .iter()
            .any(|x| x.candidate.def_source.contains("seed8")));
    }

    #[test]
    fn chosen_candidate_maps_to_a_proposed_lemma_for_admission() {
        // The admission seam: a chosen definition maps to a `library::ProposedLemma`
        // that a later stage can hand to `LemmaLibrary::record_lemma`. Nothing is
        // admitted here.
        let report = synthesize_definitions(
            "IsPerfectoid R",
            &known(&[]),
            &TwoCandidateProposer,
            &MarkerScreen,
            &SynthConfig::default(),
        );
        let chosen = report.synthesized[0].chosen.as_ref().unwrap();
        let proposed = chosen.to_proposed_lemma("definition_synthesis");
        assert_eq!(proposed.statement, chosen.def_source);
        assert_eq!(proposed.provenance, "definition_synthesis");
        assert!(
            proposed.proof.is_empty(),
            "a definition admits by well-formedness"
        );
    }

    /// A provider that drives the two model seams: it proposes one content-bearing
    /// and one vacuous definition, and screens each by the same `def `/`:= True`
    /// markers the deterministic tests use, so the recommendation is predictable.
    struct SynthProvider;
    impl ModelProvider for SynthProvider {
        fn complete(&self, request: &ModelRequest) -> Result<ModelResponse> {
            let content = match request.role.as_str() {
                "definition_proposer" => {
                    let name = request.context["symbol"].as_str().unwrap_or("Sym");
                    json!({"candidates":[
                        {"name":name,"def_source":format!("def {name} := structure with content"),"doc":"content"},
                        {"name":name,"def_source":format!("def {name} := True"),"doc":"vacuous"}
                    ]})
                }
                "definition_screen" => {
                    let src = request.context["def_source"].as_str().unwrap_or("");
                    json!({
                        "well_formed": src.trim_start().starts_with("def "),
                        "degenerate": src.trim_end().ends_with(":= True"),
                        "note": "marker screen"
                    })
                }
                _ => json!({}),
            };
            Ok(ModelResponse {
                content,
                model: "test".into(),
                provider: "command".into(),
            })
        }
        fn name(&self) -> &str {
            "command"
        }
    }

    #[test]
    fn synthesize_recommends_the_screened_candidate_and_emits_a_run() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "IsPerfectoid R").unwrap();
        let out = synthesize(&store, &SynthProvider, &project.id, "IsPerfectoid R").unwrap();

        // `IsPerfectoid` is the one missing symbol (`R` is a bound single letter).
        assert_eq!(out["missing_count"], 1);
        assert_eq!(out["recommended_count"], 1);
        assert_eq!(out["model"], "command");
        // The recommended candidate is the content-bearing one, not `:= True`.
        let chosen = &out["report"]["synthesized"][0]["chosen"]["def_source"];
        assert!(chosen.as_str().unwrap().contains("structure with content"));

        // The run row was closed and the completion event recorded.
        let events = store.events(&project.id, 100).unwrap();
        assert!(events
            .iter()
            .any(|e| e.event_type == "definition_synthesis.completed"));
    }

    #[test]
    fn synthesize_offline_is_detection_only_and_recommends_nothing() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "IsPerfectoid R").unwrap();
        let out = synthesize(&store, &OfflineProvider, &project.id, "IsPerfectoid R").unwrap();

        // Detection still runs (it is pure), but no candidate is fabricated.
        assert_eq!(out["missing_count"], 1);
        assert_eq!(out["recommended_count"], 0);
        assert_eq!(out["model"], "offline");
        assert!(out["report"]["synthesized"][0]["candidates"]
            .as_array()
            .unwrap()
            .is_empty());
    }
}
