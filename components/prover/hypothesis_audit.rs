//! HYPOTHESIS-DISCHARGE audit (trust-boundary layer 2d).
//!
//! This is OUR code — a *trust-boundary* checker built from first principles,
//! std-only + `serde` / `serde_json` (already dependencies of
//! [`crate::prover::formal`]), deterministic and offline (no clock, no RNG, no
//! IO, no network). It closes a confirmed hole in the 3+1-layer gate:
//!
//! > A theorem can be `sorry`-free, axiom-free, kernel-clean AND
//! > statement-preserved while still being **conditional on unproved
//! > mathematics carried in its own signature**.
//!
//! Two mechanisms were observed in the wild:
//!
//! * **(a) Prop-valued hypothesis arguments.** `theorem phi3_bijOn (hGlaisher :
//!   Glaisher3) (N : Nat) : …` where `Glaisher3` is a `def Glaisher3 : Prop`
//!   that is *stated and never proved*. The theorem is then a conditional whose
//!   antecedent is an open conjecture. `#print axioms` sees nothing (no axiom is
//!   declared) and a `sorry` grep sees nothing (no goal is admitted).
//!
//! * **(b) Uninhabited assumption-bundling structures.** `structure
//!   RamanujanTau where …` bundling five properties, used as a hypothesis type,
//!   with NO instance ever constructed anywhere in the submission. Same
//!   invisibility: the structure is a perfectly legal (empty) type.
//!
//! [`crate::prover::statement_preservation`]'s
//! [`BindersChanged`](crate::prover::statement_preservation::PreservationVerdict::BindersChanged)
//! does NOT cover this: it is a *drift* check (did the delivered binders change
//! relative to the canonical statement?), not a *discharge* check (is each
//! binder's proposition actually established?). A submission whose binders match
//! the canonical statement exactly still passes preservation while remaining
//! conditional on `Glaisher3`.
//!
//! ## What this module does
//!
//! [`audit_hypotheses`] locates the delivered declaration for the canonical
//! statement's name, enumerates every free hypothesis binder in its signature,
//! and classifies each as:
//!
//! * [`Discharged`](HypothesisStatus::Discharged) — a proof / instance / term of
//!   that proposition exists elsewhere in the submitted source;
//! * [`Allowlisted`](HypothesisStatus::Allowlisted) — the caller explicitly
//!   declared it a designated INPUT of the task (a genuine antecedent);
//! * [`Unaccounted`](HypothesisStatus::Unaccounted) — neither: the theorem is
//!   silently conditional on it.
//!
//! Any [`Unaccounted`](HypothesisStatus::Unaccounted) hypothesis makes the report
//! un-`clean`.
//!
//! ## Fail-closed
//!
//! An unparseable canonical statement, or a submission in which the delivered
//! declaration cannot be found, yields `clean == false` with a finding saying so.
//! The module never defaults to clean on a parse failure.
//!
//! ## Scope (deliberate, documented limits)
//!
//! Only binders whose type resolves to something the *submission itself*
//! declares are audited: a `def`/`abbrev … : Prop`, or a `structure`/`class`.
//! Ordinary mathematical antecedents (`(h : 0 < n)`, `(hp : Nat.Prime p)`) are
//! propositional expressions, not named opaque assumptions, and are NOT flagged
//! — they are part of the canonical statement and are policed by the
//! preservation layer. One extra narrow rule catches the case where the opaque
//! Prop is *imported* rather than declared locally: a binder whose name looks
//! like a hypothesis (`h`/`H` prefix) and whose type is a single bare identifier
//! we cannot resolve is flagged [`Opaque`](HypothesisFlavor::Opaque) — the caller
//! allowlists it if it is a genuine input.
//!
//! Parsing is Lean 4 syntax, dispatched per [`FormalSystem`] in the
//! [`check_entry_signature`](crate::prover::statement_preservation::check_entry_signature)
//! idiom. Non-Lean systems are reported as NOT APPLICABLE (`applicable == false`,
//! `clean == true`) rather than fail-closed, so wiring this layer into `verify()`
//! only ever tightens Lean and never regresses a backend it cannot parse.

use crate::prover::formal::{FormalSystem, ScanReport};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ===========================================================================
// Report types
// ===========================================================================

/// Whether a hypothesis binder is actually accounted for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HypothesisStatus {
    /// A proof / instance / term of this proposition exists in the submission.
    Discharged,
    /// The caller declared this binder (or its type) a designated INPUT.
    Allowlisted,
    /// Neither discharged nor allowlisted — the theorem is silently conditional
    /// on it. Gate-failing.
    Unaccounted,
}

impl HypothesisStatus {
    /// Stable tag for finding strings / JSON detail.
    pub fn tag(self) -> &'static str {
        match self {
            HypothesisStatus::Discharged => "discharged",
            HypothesisStatus::Allowlisted => "allowlisted",
            HypothesisStatus::Unaccounted => "unaccounted",
        }
    }
}

/// What KIND of assumption a flagged binder carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HypothesisFlavor {
    /// Mechanism (a): the binder's type head is a `def`/`abbrev … : Prop`
    /// declared in the submission.
    PropDef,
    /// Mechanism (b): the binder's type head is a `structure`/`class` declared in
    /// the submission — an assumption-bundling type.
    Bundle,
    /// The binder's type is a single bare identifier we cannot resolve locally
    /// and the binder is named like a hypothesis (`h`/`H` prefix).
    Opaque,
}

impl HypothesisFlavor {
    /// Stable tag for finding strings / JSON detail.
    pub fn tag(self) -> &'static str {
        match self {
            HypothesisFlavor::PropDef => "prop_def",
            HypothesisFlavor::Bundle => "bundle",
            HypothesisFlavor::Opaque => "opaque",
        }
    }
}

/// One audited hypothesis binder of the delivered theorem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hypothesis {
    /// Binder name (`hGlaisher`). `_` / anonymous instance binders yield `"_"`.
    pub name: String,
    /// Binder type text, whitespace-normalized (`Glaisher3`).
    pub ty: String,
    /// The head identifier of the type (`Glaisher3` in `Glaisher3 N`).
    pub head: String,
    /// Which assumption mechanism this binder carries.
    pub flavor: HypothesisFlavor,
    /// Whether it is discharged / allowlisted / unaccounted.
    pub status: HypothesisStatus,
    /// The declaration that discharges it, when [`Discharged`](HypothesisStatus::Discharged).
    pub witness: Option<String>,
}

/// A field of an assumption-bundling structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructField {
    pub name: String,
    pub ty: String,
}

/// An assumption-bundling `structure`/`class` used as a hypothesis type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleStructure {
    /// The structure/class name (`RamanujanTau`).
    pub name: String,
    /// Declaration keyword (`structure` / `class`).
    pub kind: String,
    /// The properties it bundles, in declaration order.
    pub fields: Vec<StructField>,
    /// Whether ANY instance / term of this type is constructed in the submission.
    pub instantiated: bool,
}

/// The result of [`audit_hypotheses`], in the
/// [`ScanReport`](crate::prover::formal::ScanReport) idiom (`clean` / `findings`
/// / `detail`) plus the structured per-binder classification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HypothesisReport {
    /// Fail-closed verdict: `false` if ANY hypothesis is
    /// [`Unaccounted`](HypothesisStatus::Unaccounted), or if the signature could
    /// not be parsed at all.
    pub clean: bool,
    /// Whether this system's syntax is audited at all (Lean only today). When
    /// `false`, `clean` is vacuously `true` and the single finding says so.
    pub applicable: bool,
    /// The delivered declaration name we audited (`None` when unparseable).
    pub target: Option<String>,
    /// Human-readable finding lines (empty iff nothing to report).
    pub findings: Vec<String>,
    /// Every audited hypothesis binder, in signature order.
    pub hypotheses: Vec<Hypothesis>,
    /// Every assumption-bundling structure used as a hypothesis type.
    pub bundles: Vec<BundleStructure>,
    /// Structured detail for the gate's JSON report.
    pub detail: Value,
}

impl HypothesisReport {
    /// Number of [`Unaccounted`](HypothesisStatus::Unaccounted) hypotheses.
    pub fn unaccounted_count(&self) -> usize {
        self.hypotheses
            .iter()
            .filter(|h| h.status == HypothesisStatus::Unaccounted)
            .count()
    }

    /// Layer-2d view: a [`ScanReport`] a backend `source_scan` (or the `verify()`
    /// orchestration) can fold in conjunctively. `clean` mirrors `self.clean`
    /// (fail-closed); the finding lines carry over verbatim.
    pub fn into_scan_report(self) -> ScanReport {
        let detail = json!({
            "check": "hypothesis_audit",
            "applicable": self.applicable,
            "target": self.target,
            "unaccounted": self.unaccounted_count(),
            "hypotheses": self.hypotheses,
            "bundles": self.bundles,
        });
        ScanReport {
            clean: self.clean,
            findings: self.findings,
            detail,
        }
    }
}

fn report(
    clean: bool,
    applicable: bool,
    target: Option<String>,
    findings: Vec<String>,
    hypotheses: Vec<Hypothesis>,
    bundles: Vec<BundleStructure>,
) -> HypothesisReport {
    let detail = json!({
        "check": "hypothesis_audit",
        "applicable": applicable,
        "target": &target,
        "hypotheses": &hypotheses,
        "bundles": &bundles,
    });
    HypothesisReport {
        clean,
        applicable,
        target,
        findings,
        hypotheses,
        bundles,
        detail,
    }
}

/// Fail-closed report for an input we could not parse. NEVER clean.
fn unparsable(finding: String) -> HypothesisReport {
    report(false, true, None, vec![finding], Vec::new(), Vec::new())
}

// ===========================================================================
// Entry point
// ===========================================================================

/// Audit the delivered theorem's hypothesis binders for undischarged
/// assumptions.
///
/// `canonical_statement` supplies the NAME of the declaration under
/// certification (parsed with the same Lean signature grammar as
/// [`crate::prover::statement_preservation`]); `submitted_code` is the full
/// source the model delivered — both the theorem and any supporting
/// declarations. `allowlist` names the binders (by binder name OR by type head)
/// the caller has explicitly designated as INPUTS to the task; those are not
/// counted against the submission.
///
/// Deterministic and pure: same inputs, same report. Fail-closed on every parse
/// failure.
pub fn audit_hypotheses(
    system: FormalSystem,
    canonical_statement: &str,
    submitted_code: &str,
    allowlist: &[String],
) -> HypothesisReport {
    match system {
        FormalSystem::Lean => audit_lean(canonical_statement, submitted_code, allowlist),
        // Rocq / Isabelle / Candle / Agda / Metamath do not share Lean 4's binder
        // grammar. Rather than fail their gate on a parser we do not have, report
        // NOT APPLICABLE explicitly so the call site can see the gap.
        other => report(
            true,
            false,
            None,
            vec![format!(
                "hypothesis audit not applicable: no binder-discharge parser for `{}` \
                 (Lean 4 only) — this layer neither passes nor fails the submission",
                other.as_str()
            )],
            Vec::new(),
            Vec::new(),
        ),
    }
}

/// The Lean 4 hypothesis-discharge audit (see [`audit_hypotheses`]).
fn audit_lean(
    canonical_statement: &str,
    submitted_code: &str,
    allowlist: &[String],
) -> HypothesisReport {
    let canonical = parse_all_decls(canonical_statement);
    let Some(canonical) = canonical.iter().find(|d| is_theorem_kind(&d.kind)) else {
        return unparsable(
            "canonical statement did not parse into a `theorem`/`lemma` signature \
             (fail-closed: cannot enumerate hypothesis binders)"
                .to_string(),
        );
    };

    let decls = parse_all_decls(submitted_code);
    let Some(target) = decls
        .iter()
        .find(|d| is_theorem_kind(&d.kind) && d.name == canonical.name)
    else {
        return unparsable(format!(
            "no `{}` declaration found in the submission (fail-closed: cannot audit the \
             hypotheses of a signature we cannot see)",
            canonical.name
        ));
    };

    let binders = parse_binder_groups(&target.binders);
    let mut hypotheses: Vec<Hypothesis> = Vec::new();
    let mut bundles: Vec<BundleStructure> = Vec::new();
    let mut findings: Vec<String> = Vec::new();

    for binder in &binders {
        let ty = norm_ws(&binder.ty);
        let Some(head) = head_ident(&ty) else {
            continue;
        };
        let Some(flavor) = classify_flavor(&decls, &head, &ty, &binder.names) else {
            continue;
        };

        // One entry per bound name in the group (`(h1 h2 : Glaisher3)`).
        let names: Vec<String> = if binder.names.is_empty() {
            vec!["_".to_string()]
        } else {
            binder.names.clone()
        };

        for name in names {
            let allowlisted = allowlist.iter().any(|a| *a == name || *a == head);
            let witness = witness_for(&decls, &target.name, &head);
            let status = if allowlisted {
                HypothesisStatus::Allowlisted
            } else if witness.is_some() {
                HypothesisStatus::Discharged
            } else {
                HypothesisStatus::Unaccounted
            };

            if status == HypothesisStatus::Unaccounted {
                findings.push(unaccounted_finding(
                    &target.name,
                    &name,
                    &ty,
                    &head,
                    flavor,
                ));
            }

            hypotheses.push(Hypothesis {
                name,
                ty: ty.clone(),
                head: head.clone(),
                flavor,
                status,
                witness: witness.clone(),
            });
        }

        // Mechanism (b) bookkeeping: record the bundle and its fields once.
        if flavor == HypothesisFlavor::Bundle && !bundles.iter().any(|b| b.name == head) {
            if let Some(decl) = decls
                .iter()
                .find(|d| is_structure_kind(&d.kind) && d.name == head)
            {
                let instantiated = witness_for(&decls, &target.name, &head).is_some();
                if !instantiated {
                    findings.push(format!(
                        "assumption-bundling structure `{}` ({}) is used as a hypothesis of \
                         `{}` but NO instance of it is ever constructed in the submission — \
                         it bundles {} unproved propert{}: {}",
                        head,
                        decl.kind,
                        target.name,
                        decl.fields.len(),
                        if decl.fields.len() == 1 { "y" } else { "ies" },
                        field_summary(&decl.fields),
                    ));
                }
                bundles.push(BundleStructure {
                    name: head.clone(),
                    kind: decl.kind.clone(),
                    fields: decl.fields.clone(),
                    instantiated,
                });
            }
        }
    }

    let clean = !hypotheses
        .iter()
        .any(|h| h.status == HypothesisStatus::Unaccounted);

    report(
        clean,
        true,
        Some(target.name.clone()),
        findings,
        hypotheses,
        bundles,
    )
}

/// The human-readable finding for one undischarged hypothesis.
fn unaccounted_finding(
    target: &str,
    name: &str,
    ty: &str,
    head: &str,
    flavor: HypothesisFlavor,
) -> String {
    match flavor {
        HypothesisFlavor::PropDef => format!(
            "undischarged hypothesis: theorem `{target}` takes `({name} : {ty})`, where \
             `{head}` is a `Prop` STATED but never PROVED in the submission — the result is \
             conditional on unproved mathematics carried in its own signature (invisible to \
             `#print axioms` and to a `sorry` scan)"
        ),
        HypothesisFlavor::Bundle => format!(
            "undischarged hypothesis: theorem `{target}` takes `({name} : {ty})`, where \
             `{head}` is an assumption-bundling structure with no constructed instance — the \
             result is vacuously conditional on a possibly-uninhabited assumption bundle"
        ),
        HypothesisFlavor::Opaque => format!(
            "unaccounted hypothesis: theorem `{target}` takes `({name} : {ty})`, an opaque \
             named assumption `{head}` that is neither discharged in the submission nor \
             declared a designated input (fail-closed: allowlist it if it is a genuine \
             hypothesis of the task)"
        ),
    }
}

/// A short `name : ty, …` rendering of a bundle's fields for the finding line.
fn field_summary(fields: &[StructField]) -> String {
    if fields.is_empty() {
        return "<no fields parsed>".to_string();
    }
    fields
        .iter()
        .map(|f| format!("`{} : {}`", f.name, f.ty))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Decide whether a binder carries an auditable assumption, and which kind.
/// `None` means "an ordinary data/antecedent binder" — not audited.
fn classify_flavor(
    decls: &[Decl],
    head: &str,
    ty: &str,
    names: &[String],
) -> Option<HypothesisFlavor> {
    if decls.iter().any(|d| is_prop_def(d) && d.name == head) {
        return Some(HypothesisFlavor::PropDef);
    }
    if decls
        .iter()
        .any(|d| is_structure_kind(&d.kind) && d.name == head)
    {
        return Some(HypothesisFlavor::Bundle);
    }
    // Narrow catch-all: `(hFoo : Bar)` where `Bar` is a single unresolved bare
    // identifier. A multi-token type (`0 < n`, `Nat.Prime p`) is an ordinary
    // propositional antecedent and is deliberately NOT flagged.
    let single_ident = ty == head && !ty.is_empty();
    let hypothesis_named = names
        .iter()
        .any(|n| n.starts_with('h') || n.starts_with('H'));
    if single_ident && hypothesis_named {
        return Some(HypothesisFlavor::Opaque);
    }
    None
}

/// The declaration that provides a proof / instance / term of `head`, if any.
///
/// A witness is any declaration OTHER than the target theorem whose kind can
/// inhabit a type (`theorem` / `lemma` / `example` / `instance` / `def` /
/// `abbrev`) and whose conclusion's head identifier is `head`. `axiom` and
/// `opaque` are deliberately NOT witnesses — asserting the assumption by fiat is
/// exactly the failure this layer exists to catch (and is the axiom audit's job
/// to report separately). A bare `Foo.mk` application also counts, for the
/// anonymous-constructor idiom.
fn witness_for(decls: &[Decl], target: &str, head: &str) -> Option<String> {
    for d in decls {
        if d.name == target || !is_inhabiting_kind(&d.kind) {
            continue;
        }
        // A declaration whose own name is `head` is the DEFINITION of the
        // assumption, not a proof of it (its conclusion is `Prop`), so it is
        // excluded by the conclusion-head test below rather than by name.
        if head_ident(&norm_ws(&d.conclusion)).as_deref() == Some(head) {
            return Some(format!("{} {}", d.kind, d.name));
        }
    }
    // `Foo.mk`-style construction bound under some other name/type ascription.
    let mk = format!("{head}.mk");
    if decls.iter().any(|d| {
        is_inhabiting_kind(&d.kind)
            && d.name != target
            && (norm_ws(&d.conclusion).contains(&mk) || d.body.contains(&mk))
    }) {
        return Some(format!("{head}.mk construction"));
    }
    None
}

fn is_theorem_kind(kind: &str) -> bool {
    matches!(kind, "theorem" | "lemma" | "example")
}

fn is_structure_kind(kind: &str) -> bool {
    matches!(kind, "structure" | "class")
}

fn is_inhabiting_kind(kind: &str) -> bool {
    matches!(
        kind,
        "theorem" | "lemma" | "example" | "instance" | "def" | "abbrev"
    )
}

/// Whether a declaration is a `def`/`abbrev` whose ascribed type is `Prop` —
/// i.e. mechanism (a): a proposition that is STATED, and whose proof (if any)
/// must live in some other declaration.
fn is_prop_def(d: &Decl) -> bool {
    matches!(d.kind.as_str(), "def" | "abbrev") && norm_ws(&d.conclusion) == "Prop"
}

// ===========================================================================
// Lean 4 declaration parsing
// ===========================================================================

/// Declaration keywords the parser recognizes. Order is irrelevant — every match
/// is whole-token — but `structure`/`class` must be present so a bundle
/// declaration terminates the preceding signature.
const DECL_KEYWORDS: &[&str] = &[
    "theorem",
    "lemma",
    "example",
    "instance",
    "structure",
    "class",
    "abbrev",
    "def",
    "axiom",
    "opaque",
];

/// A parsed Lean declaration: `KIND NAME <binders> : <conclusion> (:= body |
/// where fields)`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Decl {
    kind: String,
    name: String,
    /// Binder region (everything between the name and the top-level `:`).
    binders: String,
    /// Conclusion / ascribed type (everything after the top-level `:`).
    conclusion: String,
    /// Fields, for `structure` / `class` declarations only.
    fields: Vec<StructField>,
    /// Body text after `:=` (whitespace-normalized), for construction detection.
    body: String,
}

/// How a declaration's signature region terminated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SigEnd {
    /// A top-level `:=` (a definition body follows).
    Assign,
    /// A top-level `where` (structure fields follow).
    Where,
    /// The next top-level declaration keyword.
    NextDecl,
    /// End of input.
    Eof,
}

/// Parse EVERY declaration in `src`, in source order. Operates over
/// comment/string-stripped source so a keyword inside `-- …`, `/- … -/`, or a
/// `"…"` literal can never start a phantom declaration.
fn parse_all_decls(src: &str) -> Vec<Decl> {
    let sanitized: Vec<char> = sanitize_lean(src).chars().collect();
    let chars = &sanitized[..];
    let mut out: Vec<Decl> = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        if i == 0 || !is_word(chars[i - 1]) {
            let mut matched: Option<&str> = None;
            for kw in DECL_KEYWORDS {
                if token_at(chars, i, kw) {
                    matched = Some(kw);
                    break;
                }
            }
            if let Some(kw) = matched {
                if let Some((decl, consumed)) = parse_decl_at(chars, kw, i + kw.chars().count()) {
                    out.push(decl);
                    i = consumed.max(i + 1);
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// Parse one declaration whose keyword ended at `after_kw`. Returns the parsed
/// declaration and the index just past its signature.
fn parse_decl_at(chars: &[char], kw: &str, after_kw: usize) -> Option<(Decl, usize)> {
    let mut j = after_kw;
    while j < chars.len() && chars[j].is_whitespace() {
        j += 1;
    }
    let name_start = j;
    while j < chars.len() && is_name(chars[j]) {
        j += 1;
    }
    let name: String = chars[name_start..j].iter().collect();
    let name = if name.is_empty() {
        // `example : T := …` and `instance : C := …` are legally anonymous.
        if kw == "example" || kw == "instance" {
            format!("<{kw}>")
        } else {
            return None;
        }
    } else {
        name
    };

    let sig_start = j;
    let (sig_end, terminator) = signature_end(chars, sig_start);
    let sig: &[char] = &chars[sig_start..sig_end.min(chars.len())];
    let (binder_chars, conclusion_chars) = split_binders_conclusion(sig);
    let binders: String = binder_chars.iter().collect();
    let conclusion: String = conclusion_chars.iter().collect();

    let mut fields: Vec<StructField> = Vec::new();
    let mut body = String::new();

    match terminator {
        SigEnd::Where => {
            // `structure S where\n  a : T\n  b : U`
            let from = sig_end + "where".chars().count();
            let to = region_end(chars, from);
            if is_structure_kind(kw) {
                fields = parse_where_fields(&chars[from.min(chars.len())..to.min(chars.len())]);
            }
        }
        SigEnd::Assign => {
            let from = sig_end + 2;
            let to = region_end(chars, from);
            let region: String = chars[from.min(chars.len())..to.min(chars.len())]
                .iter()
                .collect();
            if is_structure_kind(kw) {
                // Lean 4 also accepts `structure S := (a : T) (b : U)`.
                fields = parse_binder_groups(&region)
                    .into_iter()
                    .flat_map(|b| {
                        let ty = norm_ws(&b.ty);
                        b.names
                            .into_iter()
                            .map(move |n| StructField {
                                name: n,
                                ty: ty.clone(),
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect();
            }
            body = norm_ws(&region);
        }
        SigEnd::NextDecl | SigEnd::Eof => {}
    }

    Some((
        Decl {
            kind: kw.to_string(),
            name,
            binders: norm_ws(&binders),
            conclusion: norm_ws(&conclusion),
            fields,
            body,
        },
        sig_end,
    ))
}

/// Where a declaration's signature ends: the first top-level (bracket depth 0)
/// `:=`, `where`, or next declaration keyword, else end of input.
fn signature_end(chars: &[char], start: usize) -> (usize, SigEnd) {
    let mut depth = 0i32;
    let mut i = start;
    while i < chars.len() {
        match chars[i] {
            '(' | '[' | '{' | '⟨' | '⦃' => depth += 1,
            ')' | ']' | '}' | '⟩' | '⦄' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            ':' if depth == 0 && chars.get(i + 1) == Some(&'=') => return (i, SigEnd::Assign),
            _ => {
                if depth == 0 && (i == 0 || !is_word(chars[i - 1])) {
                    if token_at(chars, i, "where") {
                        return (i, SigEnd::Where);
                    }
                    for kw in DECL_KEYWORDS {
                        if token_at(chars, i, kw) {
                            return (i, SigEnd::NextDecl);
                        }
                    }
                }
            }
        }
        i += 1;
    }
    (chars.len(), SigEnd::Eof)
}

/// End of a body / field region: the next top-level declaration keyword, else
/// end of input.
fn region_end(chars: &[char], start: usize) -> usize {
    let mut depth = 0i32;
    let mut i = start.min(chars.len());
    while i < chars.len() {
        match chars[i] {
            '(' | '[' | '{' | '⟨' | '⦃' => depth += 1,
            ')' | ']' | '}' | '⟩' | '⦄' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            _ => {
                if depth == 0 && (i == 0 || !is_word(chars[i - 1])) {
                    for kw in DECL_KEYWORDS {
                        if token_at(chars, i, kw) {
                            return i;
                        }
                    }
                }
            }
        }
        i += 1;
    }
    chars.len()
}

/// Split a signature region into `(binders, conclusion)` at the first depth-0
/// `:` (the statement colon; binder-local colons sit at depth > 0). With no
/// depth-0 colon the whole region is binders and the conclusion is empty.
fn split_binders_conclusion(sig: &[char]) -> (&[char], &[char]) {
    let mut depth = 0i32;
    let mut i = 0usize;
    while i < sig.len() {
        match sig[i] {
            '(' | '[' | '{' | '⟨' | '⦃' => depth += 1,
            ')' | ']' | '}' | '⟩' | '⦄' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            ':' if depth == 0 => {
                if sig.get(i + 1) != Some(&'=') {
                    return (&sig[..i], &sig[i + 1..]);
                }
            }
            _ => {}
        }
        i += 1;
    }
    (sig, &[])
}

/// `structure … where` fields: one `name … : type` per (possibly continued)
/// line. Lines without a depth-0 colon (`deriving Repr`, blank lines) are
/// skipped.
fn parse_where_fields(region: &[char]) -> Vec<StructField> {
    let text: String = region.iter().collect();
    let mut out: Vec<StructField> = Vec::new();
    for line in text.lines() {
        let chars: Vec<char> = line.chars().collect();
        let (names_part, ty_part) = split_binders_conclusion(&chars);
        let ty = norm_ws(&ty_part.iter().collect::<String>());
        if ty.is_empty() {
            continue;
        }
        let names = split_idents(names_part);
        for name in names {
            out.push(StructField {
                name,
                ty: ty.clone(),
            });
        }
    }
    out
}

// ===========================================================================
// Binder parsing
// ===========================================================================

/// One binder group: the names it binds and the type they share.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BinderGroup {
    names: Vec<String>,
    ty: String,
}

/// Parse a Lean binder region into its bracketed groups — `(h : P)`, `{n : Nat}`,
/// `[Group G]`, `⦃x : α⦄`. Unbracketed binders (`∀`-style trailing names) carry
/// no type ascription and are not audited.
fn parse_binder_groups(binders: &str) -> Vec<BinderGroup> {
    let chars: Vec<char> = binders.chars().collect();
    let mut out: Vec<BinderGroup> = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let open = chars[i];
        let close = match open {
            '(' => Some(')'),
            '{' => Some('}'),
            '[' => Some(']'),
            '⟨' => Some('⟩'),
            '⦃' => Some('⦄'),
            _ => None,
        };
        let Some(close) = close else {
            i += 1;
            continue;
        };
        // Matching close bracket at the same depth.
        let mut depth = 1i32;
        let mut k = i + 1;
        while k < chars.len() {
            if chars[k] == open {
                depth += 1;
            } else if chars[k] == close {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            k += 1;
        }
        let inner = &chars[(i + 1).min(chars.len())..k.min(chars.len())];
        let (name_part, ty_part) = split_binders_conclusion(inner);
        let ty: String = if ty_part.is_empty() {
            // `[Group G]` — an anonymous instance binder: the whole group is the
            // type.
            name_part.iter().collect()
        } else {
            ty_part.iter().collect()
        };
        let names = if ty_part.is_empty() {
            Vec::new()
        } else {
            split_idents(name_part)
        };
        out.push(BinderGroup {
            names,
            ty: norm_ws(&ty),
        });
        i = k + 1;
    }
    out
}

// ===========================================================================
// Lexical helpers
// ===========================================================================

/// A Lean identifier / word char for token-boundary tests (NOT including `.`).
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '\''
}

/// A name char, including `.` for namespaced identifiers (`Nat.add_comm`).
fn is_name(c: char) -> bool {
    is_word(c) || c == '.'
}

/// Whether `needle` occurs as a whole token starting exactly at `i`.
fn token_at(chars: &[char], i: usize, needle: &str) -> bool {
    let n: Vec<char> = needle.chars().collect();
    if i + n.len() > chars.len() {
        return false;
    }
    if chars[i..i + n.len()] != n[..] {
        return false;
    }
    let before_ok = i == 0 || !is_word(chars[i - 1]);
    let after_ok = chars.get(i + n.len()).map_or(true, |&c| !is_word(c));
    before_ok && after_ok
}

/// Collapse all runs of whitespace to a single space and trim.
fn norm_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The whitespace-separated identifiers in a slice, dropping the anonymous `_`.
fn split_idents(chars: &[char]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for &c in chars {
        if is_name(c) {
            cur.push(c);
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.into_iter().filter(|s| s != "_").collect()
}

/// The head identifier of a type expression: the first identifier token, skipping
/// leading punctuation / prefix notation (`(`, `¬`, `∀`-free forms). `None` when
/// the expression contains no identifier.
fn head_ident(ty: &str) -> Option<String> {
    let chars: Vec<char> = ty.chars().collect();
    let mut i = 0usize;
    while i < chars.len() && !(chars[i].is_alphabetic() || chars[i] == '_') {
        i += 1;
    }
    let start = i;
    while i < chars.len() && is_name(chars[i]) {
        i += 1;
    }
    if i == start {
        return None;
    }
    Some(chars[start..i].iter().collect())
}

/// Replace Lean `--` line comments, nested `/- … -/` block comments, and `"…"`
/// string literals with spaces, preserving newlines and the char count so line
/// numbers and offsets stay aligned.
fn sanitize_lean(code: &str) -> String {
    let chars: Vec<char> = code.chars().collect();
    let mut out = String::with_capacity(chars.len());
    let mut i = 0usize;
    let mut block_depth = 0usize;
    let mut in_line = false;
    let mut in_string = false;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if in_line {
            out.push(if c == '\n' { '\n' } else { ' ' });
            if c == '\n' {
                in_line = false;
            }
            i += 1;
            continue;
        }
        if block_depth > 0 {
            if c == '/' && next == Some('-') {
                block_depth += 1;
                out.push(' ');
                out.push(' ');
                i += 2;
                continue;
            }
            if c == '-' && next == Some('/') {
                block_depth -= 1;
                out.push(' ');
                out.push(' ');
                i += 2;
                continue;
            }
            out.push(if c == '\n' { '\n' } else { ' ' });
            i += 1;
            continue;
        }
        if in_string {
            if c == '\\' && next.is_some() {
                out.push(' ');
                out.push(' ');
                i += 2;
                continue;
            }
            if c == '"' {
                in_string = false;
                out.push(' ');
                i += 1;
                continue;
            }
            out.push(if c == '\n' { '\n' } else { ' ' });
            i += 1;
            continue;
        }
        if c == '-' && next == Some('-') {
            in_line = true;
            out.push(' ');
            out.push(' ');
            i += 2;
            continue;
        }
        if c == '/' && next == Some('-') {
            block_depth = 1;
            out.push(' ');
            out.push(' ');
            i += 2;
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(' ');
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audit(canonical: &str, code: &str) -> HypothesisReport {
        audit_hypotheses(FormalSystem::Lean, canonical, code, &[])
    }

    /// An unconditional theorem — only data binders and an ordinary
    /// propositional antecedent — is clean.
    #[test]
    fn unconditional_theorem_is_clean() {
        let canonical = "theorem add_pos (n : Nat) (hn : 0 < n) : 0 < n + 1";
        let code = "theorem add_pos (n : Nat) (hn : 0 < n) : 0 < n + 1 := by omega\n";
        let r = audit(canonical, code);
        assert!(r.clean, "unconditional theorem must pass: {:?}", r.findings);
        assert!(r.applicable);
        assert_eq!(r.target.as_deref(), Some("add_pos"));
        assert!(r.findings.is_empty());
        assert!(r.hypotheses.is_empty(), "{:?}", r.hypotheses);
        assert!(r.into_scan_report().clean);
    }

    /// Mechanism (a): a Prop-valued hypothesis whose proposition is STATED but
    /// never PROVED is Unaccounted and fails the gate.
    #[test]
    fn unproved_prop_hypothesis_is_unaccounted() {
        let canonical = "theorem phi3_bijOn (hGlaisher : Glaisher3) (N : Nat) : N = N";
        let code = "\
def Glaisher3 : Prop := ∀ n : Nat, n * 3 = 3 * n

theorem phi3_bijOn (hGlaisher : Glaisher3) (N : Nat) : N = N := rfl
";
        let r = audit(canonical, code);
        assert!(!r.clean, "conditional theorem must fail closed");
        assert_eq!(r.unaccounted_count(), 1);
        let h = r
            .hypotheses
            .iter()
            .find(|h| h.name == "hGlaisher")
            .expect("hypothesis binder must be enumerated");
        assert_eq!(h.status, HypothesisStatus::Unaccounted);
        assert_eq!(h.flavor, HypothesisFlavor::PropDef);
        assert_eq!(h.head, "Glaisher3");
        assert!(h.witness.is_none());
        // The data binder `N : Nat` is not an assumption and is not flagged.
        assert!(r.hypotheses.iter().all(|h| h.name != "N"));
        assert!(r.findings.iter().any(|f| f.contains("Glaisher3")));
        assert!(!r.into_scan_report().clean);
    }

    /// The SAME submission, with the caller declaring `Glaisher3` a designated
    /// input, is Allowlisted and clean.
    #[test]
    fn allowlisted_hypothesis_is_not_unaccounted() {
        let canonical = "theorem phi3_bijOn (hGlaisher : Glaisher3) (N : Nat) : N = N";
        let code = "\
def Glaisher3 : Prop := ∀ n : Nat, n * 3 = 3 * n

theorem phi3_bijOn (hGlaisher : Glaisher3) (N : Nat) : N = N := rfl
";
        let allow = vec!["Glaisher3".to_string()];
        let r = audit_hypotheses(FormalSystem::Lean, canonical, code, &allow);
        assert!(r.clean, "allowlisted input must pass: {:?}", r.findings);
        assert_eq!(r.unaccounted_count(), 0);
        assert_eq!(r.hypotheses[0].status, HypothesisStatus::Allowlisted);
        // Allowlisting by BINDER name works too.
        let by_binder = vec!["hGlaisher".to_string()];
        let r2 = audit_hypotheses(FormalSystem::Lean, canonical, code, &by_binder);
        assert!(r2.clean, "{:?}", r2.findings);
    }

    /// A Prop hypothesis that IS proved elsewhere in the submission is
    /// Discharged.
    #[test]
    fn proved_prop_hypothesis_is_discharged() {
        let canonical = "theorem phi3_bijOn (hGlaisher : Glaisher3) (N : Nat) : N = N";
        let code = "\
def Glaisher3 : Prop := ∀ n : Nat, n * 3 = 3 * n

theorem glaisher3_holds : Glaisher3 := by intro n; ring

theorem phi3_bijOn (hGlaisher : Glaisher3) (N : Nat) : N = N := rfl
";
        let r = audit(canonical, code);
        assert!(r.clean, "discharged hypothesis must pass: {:?}", r.findings);
        assert_eq!(r.hypotheses[0].status, HypothesisStatus::Discharged);
        assert_eq!(
            r.hypotheses[0].witness.as_deref(),
            Some("theorem glaisher3_holds")
        );
    }

    /// Mechanism (b): an assumption-bundling structure with NO instance is
    /// flagged, and its fields are reported.
    #[test]
    fn uninhabited_assumption_structure_is_flagged() {
        let canonical = "theorem tau_bound (hT : RamanujanTau) (n : Nat) : n = n";
        let code = "\
structure RamanujanTau where
  tau_one : True
  tau_mul : True
  tau_prime_power : True
  tau_bound : True
  tau_nonzero : True

theorem tau_bound (hT : RamanujanTau) (n : Nat) : n = n := rfl
";
        let r = audit(canonical, code);
        assert!(!r.clean, "uninhabited assumption bundle must fail closed");
        assert_eq!(r.hypotheses[0].flavor, HypothesisFlavor::Bundle);
        assert_eq!(r.hypotheses[0].status, HypothesisStatus::Unaccounted);

        let bundle = r
            .bundles
            .iter()
            .find(|b| b.name == "RamanujanTau")
            .expect("bundle must be reported");
        assert!(!bundle.instantiated);
        assert_eq!(bundle.kind, "structure");
        assert_eq!(bundle.fields.len(), 5, "fields: {:?}", bundle.fields);
        let names: Vec<&str> = bundle.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "tau_one",
                "tau_mul",
                "tau_prime_power",
                "tau_bound",
                "tau_nonzero"
            ]
        );
        // The finding names the structure and enumerates the bundled properties.
        assert!(r
            .findings
            .iter()
            .any(|f| f.contains("RamanujanTau") && f.contains("tau_prime_power")));
    }

    /// The same structure WITH an instance constructed is Discharged and clean.
    #[test]
    fn instantiated_assumption_structure_is_discharged() {
        let canonical = "theorem tau_bound (hT : RamanujanTau) (n : Nat) : n = n";
        let code = "\
structure RamanujanTau where
  tau_one : True
  tau_mul : True

instance : RamanujanTau := ⟨trivial, trivial⟩

theorem tau_bound (hT : RamanujanTau) (n : Nat) : n = n := rfl
";
        let r = audit(canonical, code);
        assert!(r.clean, "instantiated bundle must pass: {:?}", r.findings);
        assert_eq!(r.hypotheses[0].status, HypothesisStatus::Discharged);
        let bundle = r
            .bundles
            .iter()
            .find(|b| b.name == "RamanujanTau")
            .expect("bundle must still be reported");
        assert!(bundle.instantiated);
        assert!(r.findings.is_empty(), "{:?}", r.findings);
    }

    /// A named `def` witness of the bundle counts as construction too.
    #[test]
    fn def_witness_discharges_a_bundle() {
        let canonical = "theorem tau_bound (hT : RamanujanTau) : True";
        let code = "\
structure RamanujanTau where
  tau_one : True

def ramanujanTauHolds : RamanujanTau := ⟨trivial⟩

theorem tau_bound (hT : RamanujanTau) : True := trivial
";
        let r = audit(canonical, code);
        assert!(r.clean, "{:?}", r.findings);
        assert_eq!(
            r.hypotheses[0].witness.as_deref(),
            Some("def ramanujanTauHolds")
        );
    }

    /// An `axiom` is NOT a discharge — asserting the assumption by fiat is the
    /// very failure this layer exists to catch.
    #[test]
    fn axiom_does_not_discharge() {
        let canonical = "theorem phi3 (hG : Glaisher3) : True";
        let code = "\
def Glaisher3 : Prop := True

axiom glaisher3_ax : Glaisher3

theorem phi3 (hG : Glaisher3) : True := trivial
";
        let r = audit(canonical, code);
        assert!(!r.clean, "an axiom must not count as a discharge");
        assert_eq!(r.hypotheses[0].status, HypothesisStatus::Unaccounted);
    }

    /// An unresolved bare-identifier hypothesis (`(hR : RiemannHypothesis)` with
    /// no local declaration) is Unaccounted — fail-closed, allowlistable.
    #[test]
    fn opaque_imported_hypothesis_is_unaccounted() {
        let canonical = "theorem cond (hR : RiemannHypothesis) (n : Nat) : n = n";
        let code = "theorem cond (hR : RiemannHypothesis) (n : Nat) : n = n := rfl\n";
        let r = audit(canonical, code);
        assert!(!r.clean);
        assert_eq!(r.hypotheses[0].flavor, HypothesisFlavor::Opaque);
        assert_eq!(r.hypotheses[0].status, HypothesisStatus::Unaccounted);
        // Allowlisting it clears the gate.
        let allow = vec!["RiemannHypothesis".to_string()];
        let r2 = audit_hypotheses(FormalSystem::Lean, canonical, code, &allow);
        assert!(r2.clean, "{:?}", r2.findings);
    }

    /// An unparseable canonical statement fails CLOSED — never clean.
    #[test]
    fn unparsable_canonical_fails_closed() {
        let r = audit("-- just a comment, no theorem", "theorem T : True := trivial");
        assert!(!r.clean, "unparseable input must never default to clean");
        assert!(r.target.is_none());
        assert!(r.findings.iter().any(|f| f.contains("fail-closed")));
        assert!(!r.into_scan_report().clean);
    }

    /// A submission that does not contain the canonical declaration at all fails
    /// closed (we cannot audit a signature we cannot see).
    #[test]
    fn missing_target_declaration_fails_closed() {
        let r = audit(
            "theorem main (n : Nat) : n = n",
            "theorem helper (n : Nat) : n = n := rfl",
        );
        assert!(!r.clean);
        assert!(r.findings.iter().any(|f| f.contains("main")));
    }

    /// A declaration-shaped COMMENT cannot supply a discharge.
    #[test]
    fn commented_out_witness_does_not_discharge() {
        let canonical = "theorem phi3 (hG : Glaisher3) : True";
        let code = "\
def Glaisher3 : Prop := True

-- theorem glaisher3_holds : Glaisher3 := by trivial

theorem phi3 (hG : Glaisher3) : True := trivial
";
        let r = audit(canonical, code);
        assert!(!r.clean, "a commented-out proof must not discharge");
        assert_eq!(r.hypotheses[0].status, HypothesisStatus::Unaccounted);
    }

    /// Every bound name of a shared binder group is audited separately.
    #[test]
    fn grouped_binders_are_each_enumerated() {
        let canonical = "theorem two (h1 h2 : Glaisher3) : True";
        let code = "\
def Glaisher3 : Prop := True

theorem two (h1 h2 : Glaisher3) : True := trivial
";
        let r = audit(canonical, code);
        assert_eq!(r.hypotheses.len(), 2);
        assert_eq!(r.hypotheses[0].name, "h1");
        assert_eq!(r.hypotheses[1].name, "h2");
        assert_eq!(r.unaccounted_count(), 2);
    }

    /// A non-Lean system is reported NOT APPLICABLE rather than fail-closed, so
    /// wiring this layer in never regresses a backend it cannot parse.
    #[test]
    fn non_lean_system_is_not_applicable() {
        let r = audit_hypotheses(
            FormalSystem::Rocq,
            "Theorem t : True.",
            "Theorem t : True. Proof. trivial. Qed.",
            &[],
        );
        assert!(!r.applicable);
        assert!(r.clean);
        assert!(r.findings.iter().any(|f| f.contains("not applicable")));
    }

    /// Pure and deterministic: identical inputs yield an identical report.
    #[test]
    fn audit_is_deterministic() {
        let canonical = "theorem t (hG : Glaisher3) (hT : RamanujanTau) (n : Nat) : n = n";
        let code = "\
def Glaisher3 : Prop := True

structure RamanujanTau where
  tau_one : True

theorem t (hG : Glaisher3) (hT : RamanujanTau) (n : Nat) : n = n := rfl
";
        let a = audit(canonical, code);
        let b = audit(canonical, code);
        assert_eq!(a, b);
        assert!(!a.clean);
        assert_eq!(a.unaccounted_count(), 2);
    }
}
