//! Mathlib-contribution export: turn gate-passing verified skills from the
//! growing lemma library ([`crate::library::LemmaLibrary`], backed by
//! [`crate::db::LibraryLemma`]) into Mathlib-style Lean declaration artifacts
//! ready to be *proposed* as a contribution.
//!
//! The evolver/library produces verified `(statement, proof)` skills. This
//! module is the *last mile*: it formats each admitted skill as a well-formed
//! Lean `theorem` — proper (deterministically-derived) name, statement, proof,
//! `/-- … -/` docstring carrying provenance, wrapped in a configured namespace
//! and an `import Mathlib`-headed module.
//!
//! HONEST SCOPE. The output is a *formatting artifact*, not a certified
//! contribution: this module does not re-run the 3+1 gate, does not typecheck
//! against a real Mathlib, and does not invent Mathlib-canonical names. A human
//! (and CI) must review every emitted declaration before it becomes a real
//! Mathlib PR. The value here is mechanical: shape verified skills into the form
//! a reviewer expects, with a stable name and an audit trail in the docstring.
//!
//! DETERMINISM & UNTRUSTED DATA. Names are derived purely from statement content
//! (a documented ASCII slug + FNV-1a hash — no RNG, no `DefaultHasher`, no
//! wall-clock), so the same statement always yields the same identifier. All
//! lemma/proof/provenance text is untrusted data: it is only ever formatted into
//! output, never executed here, and docstring text is sanitized against Lean
//! block-comment delimiters so a hostile provenance string cannot break out of
//! the `/-- … -/` comment.

use crate::config::Config;
use crate::db::LibraryLemma as Lemma;
use crate::db::Store;
use crate::prover::formal::{backend_for, FormalSystem};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashSet;

/// How to shape an export run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportConfig {
    /// The Lean namespace the declarations are opened inside (e.g.
    /// `"Theoremata"`).
    pub namespace: String,
    /// The logical module name, used in the generated file's header comment
    /// (e.g. `"Theoremata.Generated"`).
    pub module: String,
    /// When `true`, obviously-trivial skills are dropped into
    /// [`ExportBundle::skipped`] instead of emitted. See [`is_trivial`].
    pub skip_trivial: bool,
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            namespace: "Theoremata".to_owned(),
            module: "Theoremata.Generated".to_owned(),
            skip_trivial: true,
        }
    }
}

/// A single Mathlib-style declaration, ready to render.
///
/// Field set is deliberately flat and self-describing: `render()` needs nothing
/// but this struct. `name` is the deterministically-derived Lean identifier;
/// `source_provenance` is the untrusted provenance string carried through for
/// the audit trail (also folded into `doc`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MathlibDecl {
    pub name: String,
    pub statement: String,
    pub proof: String,
    pub doc: String,
    pub source_provenance: String,
}

impl MathlibDecl {
    /// Render this declaration as a well-formed Mathlib-style block: a
    /// `/-- … -/` docstring followed by `theorem <name> : <statement> := <proof>`
    /// (the proof is emitted verbatim when it already begins with `by`, otherwise
    /// wrapped in a `by` tactic block). The docstring is sanitized against Lean
    /// comment delimiters. Intended to be emitted *inside* the configured
    /// namespace (see [`ExportBundle::render_file`]).
    pub fn render(&self) -> String {
        let doc = sanitize_doc(&self.doc);
        format!(
            "/-- {doc} -/\ntheorem {name} : {stmt} :=\n{proof}",
            doc = doc,
            name = self.name,
            stmt = self.statement.trim(),
            proof = render_proof(&self.proof),
        )
    }
}

/// A whole export run: the namespaced set of declarations plus the skills that
/// were skipped as trivial.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportBundle {
    pub module: String,
    pub namespace: String,
    pub decls: Vec<MathlibDecl>,
    /// Statements of skills dropped by `skip_trivial` (audit / diagnostics).
    pub skipped: Vec<String>,
}

impl ExportBundle {
    /// Emit a complete, self-contained Lean module: a header comment, an
    /// `import Mathlib`, then `namespace … end` wrapping every declaration.
    pub fn render_file(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "/-\n  {module} — auto-generated from the Theoremata verified-lemma library.\n\
             \n  These declarations are FORMATTING ARTIFACTS. Each was produced from a\n\
             gate-passing skill, but a human must review it (name, statement shape,\n\
             proof, Mathlib fit) before it becomes a real Mathlib contribution.\n-/\n",
            module = sanitize_doc(&self.module),
        ));
        out.push_str("import Mathlib\n\n");
        out.push_str(&format!("namespace {}\n\n", self.namespace));
        for (i, decl) in self.decls.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&decl.render());
            out.push('\n');
        }
        out.push_str(&format!("\nend {}\n", self.namespace));
        out
    }
}

/// Format one verified skill as a [`MathlibDecl`]. The name is derived
/// deterministically from the statement (see [`derive_name`]); the docstring
/// carries the skill's provenance for the reviewer's audit trail.
pub fn export_lemma(lemma: &Lemma, cfg: &ExportConfig) -> MathlibDecl {
    let _ = cfg; // cfg shapes the bundle (namespace/module/filtering), not the decl body.
    let doc = format!(
        "Auto-exported Theoremata skill. Provenance: {}",
        lemma.provenance.trim(),
    );
    MathlibDecl {
        name: derive_name(&lemma.statement),
        statement: lemma.statement.clone(),
        proof: lemma.proof.clone(),
        doc,
        source_provenance: lemma.provenance.clone(),
    }
}

/// Format a whole library slice into an [`ExportBundle`]. Skips obviously-trivial
/// skills when `cfg.skip_trivial` is set (their statements land in `skipped`),
/// and deduplicates by derived name so two skills with the same statement yield a
/// single declaration.
pub fn export_library(lemmas: &[Lemma], cfg: &ExportConfig) -> ExportBundle {
    let mut decls = Vec::new();
    let mut skipped = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for lemma in lemmas {
        if cfg.skip_trivial && is_trivial(lemma) {
            skipped.push(lemma.statement.clone());
            continue;
        }
        let decl = export_lemma(lemma, cfg);
        if seen.insert(decl.name.clone()) {
            decls.push(decl);
        }
    }
    ExportBundle {
        module: cfg.module.clone(),
        namespace: cfg.namespace.clone(),
        decls,
        skipped,
    }
}

// --- CLI entry point -------------------------------------------------------

/// Export a project's verified-lemma library as a Mathlib-style Lean module,
/// RE-VERIFYING every declaration through the real formal gate first.
///
/// SOUNDNESS. Exporting a declaration outward is a claim that it is proved. The
/// persisted [`Lemma`] records carry a `provenance` string but NO verification
/// verdict: nothing in the row proves the 3+1 gate ever passed on it. So this
/// entry point does not trust the row. It renders each skill to a full theorem
/// (via [`export_lemma`] + [`MathlibDecl::render`]) and runs that source through
/// the LIVE backend gate ([`FormalBackend::verify`](crate::prover::formal::FormalBackend::verify)):
/// only a declaration whose report is BOTH `live` AND `lexically_verified` is
/// emitted. Everything else is refused with a reason: a skill we could not
/// actually check is never emitted, because "we did not check" must never read as
/// "it passed".
///
/// Fail-closed when there is no live gate: if `system`'s toolchain is unavailable
/// or the config is in mock mode (a mock pass is at most informal, never a
/// certification), EVERY skill is refused and the module is not written. Offline,
/// this therefore exports nothing, which is the safe result.
///
/// Returns a JSON summary: the module/namespace, the names actually exported, the
/// refused skills with their reasons, the trivial skills skipped by config, and
/// the rendered Lean `file` (present only when at least one declaration verified,
/// and containing ONLY verified declarations). `all_verified` is true iff nothing
/// was refused. Emits one `mathlib_export.completed` store event.
pub fn export_verified(
    store: &Store,
    config: &Config,
    project_id: &str,
    system: FormalSystem,
    cfg: &ExportConfig,
) -> Result<Value> {
    // Validate the project exists (and scope the event to it).
    store.project(project_id)?;
    let lemmas = store.library_lemmas(project_id)?;

    // The live gate. A mock backend or an absent toolchain is NOT a gate we may
    // certify an outward export against, so treat either as "no live gate" and
    // refuse everything rather than emit an unchecked declaration.
    let backend = backend_for(config, system, false);
    let gate_live = !config.prover_mock && backend.available();

    let mut exported: Vec<MathlibDecl> = Vec::new();
    let mut refused: Vec<Value> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for lemma in &lemmas {
        if cfg.skip_trivial && is_trivial(lemma) {
            skipped.push(lemma.statement.clone());
            continue;
        }
        let decl = export_lemma(lemma, cfg);
        // Dedup by derived name, exactly as `export_library` does.
        if !seen.insert(decl.name.clone()) {
            continue;
        }

        if !gate_live {
            refused.push(json!({
                "name": decl.name,
                "statement": lemma.statement,
                "reason": "no live formal gate available; refusing to export unverified",
            }));
            continue;
        }

        // Re-verify the RENDERED theorem (the exact source we would emit) against
        // the stated statement, through the full 3+1 gate.
        let report = backend.verify(config, &decl.render(), &lemma.statement)?;
        if report.live && report.lexically_verified {
            exported.push(decl);
        } else {
            refused.push(json!({
                "name": decl.name,
                "statement": lemma.statement,
                "reason": "live gate did not certify the rendered declaration",
                "report": report,
            }));
        }
    }

    // Build the module from ONLY the verified declarations.
    let bundle = ExportBundle {
        module: cfg.module.clone(),
        namespace: cfg.namespace.clone(),
        decls: exported,
        skipped: skipped.clone(),
    };
    let file = if bundle.decls.is_empty() {
        Value::Null
    } else {
        Value::String(bundle.render_file())
    };
    let exported_names: Vec<String> = bundle.decls.iter().map(|d| d.name.clone()).collect();
    let all_verified = refused.is_empty();

    let summary = json!({
        "project_id": project_id,
        "system": system.as_str(),
        "module": bundle.module,
        "namespace": bundle.namespace,
        "gate_live": gate_live,
        "n_library": lemmas.len(),
        "exported": exported_names,
        "n_exported": bundle.decls.len(),
        "refused": refused,
        "n_refused": refused.len(),
        "skipped_trivial": skipped,
        "all_verified": all_verified,
        "file": file,
    });

    store.event(
        Some(project_id),
        None,
        "mathlib_export.completed",
        "mathlib_export",
        json!({
            "system": system.as_str(),
            "gate_live": gate_live,
            "n_exported": bundle.decls.len(),
            "n_refused": refused.len(),
            "all_verified": all_verified,
        }),
    )?;

    Ok(summary)
}

// --- name derivation -------------------------------------------------------

/// Derive a stable, valid Lean identifier from a statement.
///
/// Scheme (pure function of the statement, no RNG / clock):
///   1. *Canonicalize* the statement by collapsing all runs of whitespace to a
///      single space and trimming — this is what is hashed, so incidental
///      formatting differences don't change the name.
///   2. *Slug*: lowercase every ASCII-alphanumeric run of the canonical form
///      (non-ASCII and symbols become separators), take up to the first
///      [`SLUG_TOKENS`] tokens, and join them with `_`. Empty ⇒ `"anon"`.
///   3. *Hash*: FNV-1a 64-bit over the canonical form, rendered as 16 lowercase
///      hex digits — a stable disambiguator so distinct statements almost never
///      collide even when their slugs coincide.
///   4. Name = `thm_<slug>_<hash>`. The fixed `thm_` lead guarantees the
///      identifier starts with a letter and is never a Lean keyword; every other
///      character is `[a-z0-9_]`, so the result is always a valid ASCII Lean
///      identifier regardless of the (untrusted) statement content.
pub fn derive_name(statement: &str) -> String {
    let canonical = statement.split_whitespace().collect::<Vec<_>>().join(" ");
    let tokens: Vec<String> = canonical
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .take(SLUG_TOKENS)
        .map(str::to_owned)
        .collect();
    let slug = if tokens.is_empty() {
        "anon".to_owned()
    } else {
        tokens.join("_")
    };
    format!("thm_{slug}_{:016x}", fnv1a(&canonical))
}

/// How many leading tokens of the statement seed the human-readable slug.
const SLUG_TOKENS: usize = 6;

/// FNV-1a 64-bit — a fixed, deterministic hash (unlike `DefaultHasher`, which is
/// seeded randomly per process). Same choice as `library::fnv1a`.
fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

// --- rendering helpers -----------------------------------------------------

/// Render a proof body after `:=`. If it already starts with the `by` tactic
/// keyword it is emitted verbatim (indented); otherwise it is wrapped in a `by`
/// block, since library proofs are tactic-oriented. An empty proof degrades to
/// `by sorry` (a visible, review-forcing placeholder).
fn render_proof(proof: &str) -> String {
    let p = proof.trim();
    if p.is_empty() {
        return "  by sorry".to_owned();
    }
    let starts_with_by = p == "by" || p.starts_with("by ") || p.starts_with("by\n");
    let body = if starts_with_by {
        p.to_owned()
    } else {
        format!("by {p}")
    };
    // Indent every line by two spaces for a Mathlib-ish block.
    body.lines()
        .map(|l| format!("  {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Neutralize Lean block-comment delimiters in text that will sit inside a
/// `/-- … -/` docstring, so untrusted provenance/statement text cannot terminate
/// or nest the comment. Also collapses newlines to keep the docstring one-line.
fn sanitize_doc(s: &str) -> String {
    s.replace("/-", "/ -")
        .replace("-/", "- /")
        .replace(['\r', '\n'], " ")
        .trim()
        .to_owned()
}

// --- trivial-skill heuristic ----------------------------------------------

/// Whether a skill is "obviously trivial" and not worth proposing to Mathlib.
///
/// Documented, deterministic heuristic — a skill is trivial when EITHER:
///   * its statement has fewer than [`MIN_STMT_TOKENS`] ASCII-alphanumeric
///     tokens (too small to be a meaningful lemma, e.g. `n = n`, `True`); OR
///   * its proof, lowercased and stripped of a leading `by`, is one of a small
///     set of one-shot closers (`rfl`, `trivial`, `simp`, `tauto`, `decide`) —
///     proofs that carry no reusable content.
pub fn is_trivial(lemma: &Lemma) -> bool {
    let stmt_tokens = lemma
        .statement
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .count();
    if stmt_tokens < MIN_STMT_TOKENS {
        return true;
    }
    let mut p = lemma.proof.trim().to_ascii_lowercase();
    if let Some(rest) = p.strip_prefix("by") {
        p = rest.trim().to_owned();
    }
    matches!(
        p.as_str(),
        "" | "rfl" | "trivial" | "simp" | "tauto" | "decide"
    )
}

/// Minimum ASCII-alphanumeric token count for a statement to be non-trivial.
const MIN_STMT_TOKENS: usize = 3;

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a verified skill fixture with a fixed (deterministic) timestamp —
    /// no wall-clock, so tests are reproducible.
    fn lemma(statement: &str, proof: &str, provenance: &str) -> Lemma {
        let t = chrono::DateTime::from_timestamp(0, 0).unwrap();
        Lemma {
            id: "fixed-id".to_owned(),
            project_id: "p".to_owned(),
            statement: statement.to_owned(),
            proof: proof.to_owned(),
            provenance: provenance.to_owned(),
            embedding_key: "emb1:0".to_owned(),
            update_count: 0,
            created_at: t,
            updated_at: t,
        }
    }

    /// A valid ASCII Lean identifier: starts with a letter, then `[A-Za-z0-9_]`.
    fn is_valid_lean_ident(name: &str) -> bool {
        let mut chars = name.chars();
        match chars.next() {
            Some(c) if c.is_ascii_alphabetic() => {}
            _ => return false,
        }
        chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    }

    #[test]
    fn two_lemmas_yield_two_wellformed_decls_with_distinct_valid_idents() {
        let cfg = ExportConfig::default();
        let lemmas = [
            lemma("a + b = b + a", "commutativity by ring", "seed:comm"),
            lemma(
                "sum of two even numbers is even",
                "by parity",
                "seed:parity",
            ),
        ];
        let bundle = export_library(&lemmas, &cfg);

        assert_eq!(bundle.decls.len(), 2);
        for decl in &bundle.decls {
            assert!(
                is_valid_lean_ident(&decl.name),
                "derived name must be a valid Lean identifier, got {:?}",
                decl.name
            );
            let rendered = decl.render();
            assert!(rendered.contains(&format!("theorem {} :", decl.name)));
            assert!(rendered.contains("/--") && rendered.contains("-/"));
        }
        assert_ne!(
            bundle.decls[0].name, bundle.decls[1].name,
            "distinct statements must yield distinct identifiers"
        );
    }

    #[test]
    fn render_file_produces_namespaced_module_with_import_and_each_theorem() {
        let cfg = ExportConfig::default();
        let lemmas = [
            lemma("a + b = b + a", "by ring", "seed:comm"),
            lemma("x * 1 = x", "by simp only [mul_one]", "seed:mul"),
        ];
        let bundle = export_library(&lemmas, &cfg);
        let file = bundle.render_file();

        assert!(file.contains("import Mathlib"), "missing import header");
        assert!(
            file.contains("namespace Theoremata") && file.contains("end Theoremata"),
            "module must open and close the configured namespace"
        );
        for decl in &bundle.decls {
            assert!(
                file.contains(&format!("theorem {} :", decl.name)),
                "each theorem must appear in the module"
            );
        }
        // The namespace open must precede the first theorem, which precedes the close.
        let ns = file.find("namespace Theoremata").unwrap();
        let thm = file.find("theorem thm_").unwrap();
        let end = file.find("end Theoremata").unwrap();
        assert!(ns < thm && thm < end);
    }

    #[test]
    fn skip_trivial_moves_trivial_lemma_to_skipped() {
        let cfg = ExportConfig {
            skip_trivial: true,
            ..ExportConfig::default()
        };
        let lemmas = [
            // Non-trivial: 4 tokens, non-canned proof.
            lemma("a + b = b + a", "by ring", "seed:comm"),
            // Trivial: `n = n` is 2 tokens AND `rfl` is a canned closer.
            lemma("n = n", "rfl", "seed:refl"),
        ];
        let bundle = export_library(&lemmas, &cfg);

        assert_eq!(
            bundle.decls.len(),
            1,
            "only the non-trivial skill is emitted"
        );
        assert_eq!(bundle.skipped, vec!["n = n".to_owned()]);

        // With skip_trivial off, the trivial one is emitted too.
        let cfg_off = ExportConfig {
            skip_trivial: false,
            ..ExportConfig::default()
        };
        let bundle_off = export_library(&lemmas, &cfg_off);
        assert_eq!(bundle_off.decls.len(), 2);
        assert!(bundle_off.skipped.is_empty());
    }

    #[test]
    fn name_derivation_is_deterministic_and_dedups() {
        let cfg = ExportConfig::default();
        let l = lemma("a + b = b + a", "by ring", "seed:comm");

        // Same statement exported twice ⇒ identical derived name.
        let n1 = export_lemma(&l, &cfg).name;
        let n2 = export_lemma(&l, &cfg).name;
        assert_eq!(n1, n2, "export must be deterministic");
        // And the free function agrees.
        assert_eq!(n1, derive_name("a + b = b + a"));
        // Whitespace-only differences canonicalize to the same name.
        assert_eq!(n1, derive_name("a  +  b   =  b + a"));

        // Two identical skills collapse to a single decl (dedup by name).
        let bundle = export_library(&[l.clone(), l], &cfg);
        assert_eq!(bundle.decls.len(), 1);
    }

    #[test]
    fn provenance_is_carried_into_the_doc() {
        let cfg = ExportConfig::default();
        let l = lemma("a + b = b + a", "by ring", "evolver:parameterize#42");
        let decl = export_lemma(&l, &cfg);

        assert_eq!(decl.source_provenance, "evolver:parameterize#42");
        assert!(decl.doc.contains("evolver:parameterize#42"));
        assert!(
            decl.render().contains("evolver:parameterize#42"),
            "provenance must survive into the rendered docstring"
        );
    }

    #[test]
    fn docstring_sanitizes_hostile_comment_delimiters() {
        // Untrusted provenance that tries to close the docstring early.
        let cfg = ExportConfig::default();
        let l = lemma("a + b = b + a", "by ring", "evil -/ #check attack /- x");
        let rendered = export_lemma(&l, &cfg).render();
        // Exactly one docstring open and close: the injected `-/` was neutralized.
        assert_eq!(rendered.matches("-/").count(), 1);
        assert!(!rendered.contains("evil -/"));
    }

    // --- CLI entry point --------------------------------------------------
    // `Config`, `Store`, `FormalSystem` come through `use super::*`.

    use std::path::Path;

    /// Mock config: no live toolchain is assumed, so the export gate is offline
    /// and must fail closed.
    fn mock_config() -> Config {
        Config {
            prover_mock: true,
            ..Config::default()
        }
    }

    #[test]
    fn export_refuses_everything_without_a_live_gate() {
        // The soundness property: with no live gate (mock/offline), a perfectly
        // well-formed, non-trivial library skill is REFUSED, never exported. An
        // unchecked proof is never emitted as if it had passed.
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "t").unwrap();
        store
            .add_library_lemma(
                &project.id,
                "a + b = b + a",
                "by ring",
                "evolver:comm",
                "emb1:0",
            )
            .unwrap();

        let summary = export_verified(
            &store,
            &mock_config(),
            &project.id,
            FormalSystem::Lean,
            &ExportConfig::default(),
        )
        .unwrap();

        assert_eq!(summary["gate_live"], false);
        assert_eq!(summary["n_exported"], 0);
        assert_eq!(summary["n_refused"], 1);
        assert_eq!(summary["all_verified"], false);
        // No module is written when nothing verified.
        assert!(summary["file"].is_null());
        // The refusal names the skill and gives a reason (never a silent drop).
        assert_eq!(summary["refused"][0]["statement"], "a + b = b + a");
        assert!(summary["refused"][0]["reason"].is_string());

        // The completion event landed, scoped to the project.
        let events = store.events(&project.id, 10).unwrap();
        assert!(events
            .iter()
            .any(|e| e.event_type == "mathlib_export.completed"));
    }

    #[test]
    fn export_skips_trivial_and_still_refuses_the_rest_offline() {
        // Trivial skills are filtered by config into `skipped_trivial`; the
        // non-trivial one is not trivially dropped but still refused offline
        // (no live gate). The two buckets are disjoint and nothing is exported.
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "t").unwrap();
        // Trivial: `n = n` is a 2-token statement AND `rfl` is a canned closer.
        store
            .add_library_lemma(&project.id, "n = n", "rfl", "seed:refl", "emb1:1")
            .unwrap();
        // Non-trivial.
        store
            .add_library_lemma(
                &project.id,
                "sum of two even numbers is even",
                "by parity",
                "evolver:parity",
                "emb1:2",
            )
            .unwrap();

        let summary = export_verified(
            &store,
            &mock_config(),
            &project.id,
            FormalSystem::Lean,
            &ExportConfig::default(),
        )
        .unwrap();

        assert_eq!(summary["n_library"], 2);
        assert_eq!(summary["skipped_trivial"][0], "n = n");
        assert_eq!(summary["n_exported"], 0);
        assert_eq!(
            summary["n_refused"], 1,
            "the non-trivial skill is refused offline"
        );
        assert!(summary["file"].is_null());
    }

    #[test]
    fn export_of_empty_library_is_vacuously_all_verified() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "t").unwrap();
        let summary = export_verified(
            &store,
            &mock_config(),
            &project.id,
            FormalSystem::Lean,
            &ExportConfig::default(),
        )
        .unwrap();
        assert_eq!(summary["n_library"], 0);
        assert_eq!(summary["n_exported"], 0);
        assert_eq!(summary["n_refused"], 0);
        // Nothing was refused, so all_verified is vacuously true, but no file is
        // emitted because there is nothing to export.
        assert_eq!(summary["all_verified"], true);
        assert!(summary["file"].is_null());
    }
}
