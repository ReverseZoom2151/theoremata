//! **Four-valued declaration lookup** — the cure for *environmental scope
//! collapse*.
//!
//! ## The failure mode this exists to prevent
//!
//! A model writes `Nat.sub_one_lt`, the checker answers `unknown identifier`,
//! and the model concludes *"Mathlib does not have this lemma"* — then abandons a
//! branch that was entirely provable, because the lemma exists and merely lived
//! one `import` away. The mined system's phrasing is exact: the model is
//!
//! > **not inventing a capability, but confidently inventing a limitation.**
//!
//! The root cause is a **type error in the model's world-model**, not a retrieval
//! miss: a *boolean* answer (`found` / `not found`) cannot distinguish four
//! situations that demand four different actions. Collapsing them loses the
//! information that would have kept the branch alive. This module refuses the
//! boolean and returns [`Verdict`], which has exactly four inhabitants:
//!
//! | Verdict | What it means | The ACTION it licenses |
//! |---|---|---|
//! | [`Verdict::Found`] | resolvable under the current imports | use it |
//! | [`Verdict::NotInCurrentImportScope`] | exists in the wider library, not under **this** manifest | **add the import** |
//! | [`Verdict::UnknownDeclaration`] | genuinely absent everywhere we looked | **try another name** |
//! | [`Verdict::EnvironmentError`] | the *lookup itself* failed | **retry / degrade — conclude NOTHING** |
//!
//! ## The critical invariant: `EnvironmentError` is evidence of NOTHING
//!
//! [`Verdict::EnvironmentError`] must **never** be collapsed into
//! [`Verdict::UnknownDeclaration`]. A dead toolchain, a missing index file, or a
//! timeout is a fact about *our infrastructure*, not a fact about *mathematics*.
//! Reading an infra failure as "the library lacks this lemma" is precisely the
//! bug this design prevents — and it is the *worse* half of the bug, because it
//! manufactures a false mathematical belief out of an unrelated operational
//! hiccup, then propagates it into the model's reasoning as if it were a checked
//! result.
//!
//! The invariant is enforced structurally, not by convention:
//!
//! * [`DeclarationIndex`] returns `Result<Option<Declaration>, IndexError>`. The
//!   two axes are *orthogonal by type*: `Err` is "the lookup failed",
//!   `Ok(None)` is "the lookup succeeded and the name is not there". No code path
//!   can turn one into the other by accident.
//! * Every `Err` short-circuits into [`Verdict::EnvironmentError`] **before** any
//!   absence is inferred — see [`deep_check`].
//! * [`Verdict::is_evidence_of_absence`] is `true` for exactly one variant, and
//!   the module test suite asserts it.
//!
//! ## The second invariant: absence from the manifest is not absence
//!
//! The fast path ([`fast_check`], manifest only, ~milliseconds) can *confirm* a
//! name but can never *refute* one. When the manifest does not cover it,
//! [`fast_check`] returns `None` — meaning **"undecided, escalate"** — never
//! `Unknown`. Only [`deep_check`], which actually consults the wider library
//! (the mined system measured seconds vs. 15–40s for these two paths), is
//! permitted to reach a negative conclusion. A caller who runs only the fast path
//! and reports "not found" has re-introduced the bug.
//!
//! ## Name representation: one representation suffices
//!
//! Lean's dotted names (`Nat.succ_le_succ`), Rocq's qualified names
//! (`Coq.Init.Nat.add`), Isabelle's theory-qualified facts (`Nat.add_commute`)
//! and Agda's module-qualified names (`Data.Nat.Base.suc`) are all
//! `.`-separated `module`+`base` paths. Rather than four parallel name types, we
//! keep **one** `String` representation and dispatch only the two things that
//! genuinely differ per system: the qualifier [`separator`] (Metamath has none —
//! its labels are flat) and the [`normalize`] rules. This is deliberate
//! simplicity; if a system later needs true structural divergence, it gets a
//! `separator`/`normalize` arm, not a new type.
//!
//! ## Purity
//!
//! No IO, no clock, no RNG, no `unsafe`, std only. Every capability arrives
//! through the [`DeclarationIndex`] seam, mirroring how [`crate::guardrails`]
//! and [`crate::concurrent`] inject capability, so the whole decision procedure
//! is testable offline against mock indices.

use crate::prover::formal::FormalSystem;

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

/// The qualifier separator for `system`'s declaration names, or `None` for a
/// system with flat, unqualified labels.
///
/// Every qualified system in the crate uses `.`; Metamath labels
/// (`ax-mp`, `df-clab`) carry no module path at all.
pub fn separator(system: FormalSystem) -> Option<char> {
    match system {
        FormalSystem::Lean | FormalSystem::Rocq | FormalSystem::Isabelle | FormalSystem::Agda => {
            Some('.')
        }
        // HOL Light theorem names (`ADD_SYM`, `ETA_AX`) and Metamath labels
        // (`ax-mp`, `df-clab`) are flat: no module path to qualify or import.
        FormalSystem::Candle | FormalSystem::Metamath => None,
    }
}

/// Canonicalize a name the model produced into the form the index is keyed by.
///
/// Deliberately conservative — it only removes decoration that is unambiguously
/// *not* part of the name, so a normalized miss is a real miss:
///
/// * surrounding whitespace, backticks, and Lean's `‹…›` / `«…»` delimiters;
/// * a Rocq-style leading `.` (an absolute qualification marker);
/// * runs of repeated separators (`Nat..add` → `Nat.add`).
///
/// It never changes case (all six systems are case-sensitive) and never guesses
/// at a module prefix.
pub fn normalize(system: FormalSystem, raw: &str) -> String {
    let mut s = raw.trim();
    loop {
        let before = s;
        s = s.trim_matches(|c: char| c.is_whitespace() || c == '`' || c == '\'' || c == '"');
        s = s
            .strip_prefix('‹')
            .and_then(|r| r.strip_suffix('›'))
            .unwrap_or(s);
        s = s
            .strip_prefix('«')
            .and_then(|r| r.strip_suffix('»'))
            .unwrap_or(s);
        if s == before {
            break;
        }
    }
    let Some(sep) = separator(system) else {
        return s.to_string();
    };
    let s = s.trim_matches(sep);
    // Collapse repeated separators without allocating per segment.
    let mut out = String::with_capacity(s.len());
    let mut last_was_sep = false;
    for c in s.chars() {
        if c == sep {
            if !last_was_sep {
                out.push(c);
            }
            last_was_sep = true;
        } else {
            out.push(c);
            last_was_sep = false;
        }
    }
    out
}

/// The module/theory path that owns `name`, or `None` for an unqualified name.
///
/// `Nat.succ_le_succ` → `Some("Nat")`; `ax-mp` (Metamath) → `None`.
pub fn module_of(system: FormalSystem, name: &str) -> Option<String> {
    let sep = separator(system)?;
    let normalized = normalize(system, name);
    let (module, _base) = normalized.rsplit_once(sep)?;
    if module.is_empty() {
        None
    } else {
        Some(module.to_string())
    }
}

// ---------------------------------------------------------------------------
// Import manifest
// ---------------------------------------------------------------------------

/// The set of modules a problem currently imports — the *scope* against which
/// the fast path resolves.
///
/// Coverage is prefix-based on module path segments, matching how `import
/// Mathlib.Data.Nat` in Lean (and `Require Import Coq.Init` in Rocq) brings a
/// whole subtree into scope. Entries are normalized, de-duplicated, and sorted at
/// construction so a manifest's behavior never depends on caller ordering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportManifest {
    system: FormalSystem,
    imports: Vec<String>,
}

impl ImportManifest {
    /// Build a manifest for `system` from raw import strings.
    pub fn new<I, S>(system: FormalSystem, imports: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut imports: Vec<String> = imports
            .into_iter()
            .map(|s| normalize(system, s.as_ref()))
            .filter(|s| !s.is_empty())
            .collect();
        imports.sort();
        imports.dedup();
        Self { system, imports }
    }

    /// An empty manifest: nothing is in scope.
    pub fn empty(system: FormalSystem) -> Self {
        Self {
            system,
            imports: Vec::new(),
        }
    }

    pub fn system(&self) -> FormalSystem {
        self.system
    }

    /// The imports, normalized and in sorted order.
    pub fn imports(&self) -> &[String] {
        &self.imports
    }

    /// Whether `module` is brought into scope by this manifest — either imported
    /// exactly, or as a descendant of an imported module path.
    ///
    /// Segment-aware: `Mathlib.Data` covers `Mathlib.Data.Nat` but NOT
    /// `Mathlib.DataFlow`.
    pub fn covers_module(&self, module: &str) -> bool {
        let module = normalize(self.system, module);
        let Some(sep) = separator(self.system) else {
            return self.imports.iter().any(|i| i == &module);
        };
        self.imports.iter().any(|import| {
            module == *import
                || (module.len() > import.len()
                    && module.starts_with(import.as_str())
                    && module[import.len()..].starts_with(sep))
        })
    }

    /// Whether a declaration is in scope. A declaration with no known module is
    /// treated as **not** in scope: an unknown location is not a licence to
    /// assume visibility (fail-closed toward escalation, never toward absence).
    pub fn covers(&self, decl: &Declaration) -> bool {
        match decl.module.as_deref() {
            Some(module) => self.covers_module(module),
            None => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Declarations and index errors
// ---------------------------------------------------------------------------

/// A declaration record as an index knows it: the resolved name, where it lives,
/// and its signature/type if the index is type-aware.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    pub system: FormalSystem,
    /// The normalized, fully-qualified name.
    pub name: String,
    /// Owning module / theory / file, when the index tracks location.
    pub module: Option<String>,
    /// The declaration's type or statement, when the index is type-aware.
    pub signature: Option<String>,
    /// `theorem` / `def` / `lemma` / `axiom` / …, when the index tracks kind.
    pub kind: Option<String>,
}

impl Declaration {
    /// A minimal record: a name only, no location or signature.
    pub fn new(system: FormalSystem, name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            name: normalize(system, &name),
            system,
            module: None,
            signature: None,
            kind: None,
        }
    }

    /// A record located in `module`.
    pub fn in_module(system: FormalSystem, name: impl Into<String>, module: impl Into<String>) -> Self {
        let module = module.into();
        Self {
            module: Some(normalize(system, &module)),
            ..Self::new(system, name)
        }
    }

    pub fn with_signature(mut self, signature: impl Into<String>) -> Self {
        self.signature = Some(signature.into());
        self
    }

    pub fn with_kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    /// The module the record carries, else the one implied by its dotted name.
    /// Used to name the import to add.
    pub fn effective_module(&self) -> Option<String> {
        self.module
            .clone()
            .or_else(|| module_of(self.system, &self.name))
    }
}

/// Why a lookup *itself* failed. Each of these is an operational fact about our
/// tooling and carries **zero** information about whether the declaration exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IndexErrorKind {
    /// The prover toolchain (lean / rocq / isabelle binary, REPL, worker) is
    /// absent or would not start.
    ToolchainUnavailable,
    /// The declaration index has not been built, or its file is missing.
    IndexMissing,
    /// The lookup exceeded its deadline.
    Timeout,
    /// The index responded, but with output we could not parse.
    Malformed,
    /// Anything else that broke the lookup.
    Other,
}

impl IndexErrorKind {
    /// Stable snake_case tag for logs and observability payloads.
    pub fn tag(self) -> &'static str {
        match self {
            IndexErrorKind::ToolchainUnavailable => "toolchain_unavailable",
            IndexErrorKind::IndexMissing => "index_missing",
            IndexErrorKind::Timeout => "timeout",
            IndexErrorKind::Malformed => "malformed",
            IndexErrorKind::Other => "other",
        }
    }

    /// Whether retrying the same lookup could plausibly succeed without any
    /// operator action. Advisory only — it never affects the verdict.
    pub fn is_transient(self) -> bool {
        matches!(self, IndexErrorKind::Timeout | IndexErrorKind::Other)
    }
}

/// A failure of the lookup mechanism. Distinct *by type* from `Ok(None)`, which
/// is the only thing that may ever be read as absence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexError {
    pub kind: IndexErrorKind,
    /// Human-readable detail (which binary, which index file, which deadline).
    pub detail: String,
}

impl IndexError {
    pub fn new(kind: IndexErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    pub fn toolchain_unavailable(detail: impl Into<String>) -> Self {
        Self::new(IndexErrorKind::ToolchainUnavailable, detail)
    }

    pub fn index_missing(detail: impl Into<String>) -> Self {
        Self::new(IndexErrorKind::IndexMissing, detail)
    }

    pub fn timeout(detail: impl Into<String>) -> Self {
        Self::new(IndexErrorKind::Timeout, detail)
    }
}

// ---------------------------------------------------------------------------
// The capability seam
// ---------------------------------------------------------------------------

/// The injected lookup capability. Implementors do IO; this module does not.
///
/// **Contract — the whole design rests on it:**
///
/// * `Ok(Some(decl))` — the name resolves, here is the record.
/// * `Ok(None)` — the lookup *ran to completion* and the name is not present in
///   the searched scope. This is the ONLY value that may be read as absence.
/// * `Err(IndexError)` — the lookup did not run to completion. An implementor
///   that catches its own failure and returns `Ok(None)` has laundered an infra
///   failure into a mathematical claim and re-created the exact bug this module
///   exists to prevent. **When in doubt, return `Err`.**
///
/// The two methods correspond to the two cost tiers the mined system measured:
/// [`lookup_in_manifest`](DeclarationIndex::lookup_in_manifest) is the
/// milliseconds-to-seconds path over what is already imported;
/// [`lookup_in_library`](DeclarationIndex::lookup_in_library) is the 15–40s path
/// over the whole library.
pub trait DeclarationIndex {
    /// Resolve `name` **restricted to** `manifest`'s import scope. Must not
    /// consult the wider library.
    fn lookup_in_manifest(
        &self,
        system: FormalSystem,
        name: &str,
        manifest: &ImportManifest,
    ) -> Result<Option<Declaration>, IndexError>;

    /// Resolve `name` across the **entire** library for `system`, ignoring any
    /// import manifest. This is the expensive path.
    fn lookup_in_library(
        &self,
        system: FormalSystem,
        name: &str,
    ) -> Result<Option<Declaration>, IndexError>;
}

// ---------------------------------------------------------------------------
// The four-valued verdict
// ---------------------------------------------------------------------------

/// The result of a declaration lookup. **Four** values, because four different
/// actions are correct — see the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Resolvable right now under the current imports.
    Found(Declaration),
    /// It **exists** — just not under this problem's import manifest. The branch
    /// is alive; add the import.
    NotInCurrentImportScope {
        /// The declaration as the wider library knows it.
        declaration: Declaration,
        /// The module to import to bring it into scope, when known.
        add_import: Option<String>,
    },
    /// Genuinely not found anywhere we searched. The only verdict that is
    /// evidence of absence. Try another name.
    UnknownDeclaration {
        /// The normalized name that was searched for.
        queried: String,
    },
    /// The lookup mechanism failed. **Evidence of nothing.** Never conclude the
    /// declaration is absent from this; retry, or proceed without the
    /// information and say so.
    EnvironmentError(IndexError),
}

impl Verdict {
    /// Stable snake_case tag for logs and observability payloads.
    pub fn tag(&self) -> &'static str {
        match self {
            Verdict::Found(_) => "found",
            Verdict::NotInCurrentImportScope { .. } => "not_in_current_import_scope",
            Verdict::UnknownDeclaration { .. } => "unknown_declaration",
            Verdict::EnvironmentError(_) => "environment_error",
        }
    }

    /// **The load-bearing predicate.** `true` for exactly one variant:
    /// [`Verdict::UnknownDeclaration`].
    ///
    /// In particular it is `false` for [`Verdict::EnvironmentError`] — a broken
    /// toolchain is not a statement about the library. Any caller tempted to
    /// write `matches!(v, Unknown | EnvironmentError)` should call this instead.
    pub fn is_evidence_of_absence(&self) -> bool {
        matches!(self, Verdict::UnknownDeclaration { .. })
    }

    /// Whether the name is known to exist *somewhere* (in scope or not).
    pub fn exists_somewhere(&self) -> bool {
        matches!(
            self,
            Verdict::Found(_) | Verdict::NotInCurrentImportScope { .. }
        )
    }

    /// Whether this verdict licenses abandoning the branch. Only a genuine
    /// [`Verdict::UnknownDeclaration`] does — and even then only the *name*, not
    /// the mathematical branch.
    pub fn licenses_abandoning_the_name(&self) -> bool {
        self.is_evidence_of_absence()
    }

    /// The one-line action this verdict licenses, phrased for a model prompt.
    /// This is the text that keeps a provable branch from being abandoned.
    pub fn action(&self) -> String {
        match self {
            Verdict::Found(d) => format!(
                "`{}` resolves under the current imports — use it as written.",
                d.name
            ),
            Verdict::NotInCurrentImportScope {
                declaration,
                add_import,
            } => match add_import {
                Some(module) => format!(
                    "`{}` EXISTS in the library but is not in this problem's import scope. \
                     ADD THE IMPORT `{module}` and retry. Do NOT conclude the library lacks it.",
                    declaration.name
                ),
                None => format!(
                    "`{}` EXISTS in the library but is not in this problem's import scope. \
                     Add the import that provides it and retry. Do NOT conclude the library lacks it.",
                    declaration.name
                ),
            },
            Verdict::UnknownDeclaration { queried } => format!(
                "`{queried}` was not found in the current scope or in the wider library. \
                 TRY ANOTHER NAME (search by statement shape or by the concept). \
                 This refutes the NAME, not the mathematical step."
            ),
            Verdict::EnvironmentError(e) => format!(
                "The declaration lookup itself FAILED ({}: {}). This is evidence of NOTHING \
                 about whether the declaration exists. Retry the lookup or proceed without it — \
                 do NOT treat this as 'the library does not have it'.",
                e.kind.tag(),
                e.detail
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// The lookup logic (pure)
// ---------------------------------------------------------------------------

/// **Fast path** (~ms–s): consult *only* the current import manifest.
///
/// Returns:
///
/// * `Some(Verdict::Found(_))` — resolved in scope, no deep search needed.
/// * `Some(Verdict::EnvironmentError(_))` — even the cheap lookup failed.
/// * **`None` — UNDECIDED.** The name is not in the manifest, and that is *not*
///   a finding. It is exactly as consistent with "one import away" as with
///   "does not exist", and the fast path cannot tell them apart. Escalate to
///   [`deep_check`].
///
/// Returning `None` rather than `Unknown` is the point: this function is
/// *structurally incapable* of producing the false-limitation verdict. It never
/// touches [`DeclarationIndex::lookup_in_library`].
pub fn fast_check(
    index: &dyn DeclarationIndex,
    system: FormalSystem,
    name: &str,
    manifest: &ImportManifest,
) -> Option<Verdict> {
    let normalized = normalize(system, name);
    match index.lookup_in_manifest(system, &normalized, manifest) {
        // Infra failure short-circuits FIRST, before any absence is inferred.
        Err(e) => Some(Verdict::EnvironmentError(e)),
        Ok(Some(decl)) => Some(Verdict::Found(decl)),
        Ok(None) => None,
    }
}

/// **Deep path** (~15–40s): fast path first, then the wider library.
///
/// This is the only function that may return [`Verdict::UnknownDeclaration`],
/// and it does so on exactly one path: the manifest lookup **succeeded** and
/// found nothing, *and* the library lookup **succeeded** and found nothing. If
/// either lookup errors, the verdict is [`Verdict::EnvironmentError`] and the
/// absence is never inferred — an infra failure can never be upgraded into a
/// mathematical fact.
pub fn deep_check(
    index: &dyn DeclarationIndex,
    system: FormalSystem,
    name: &str,
    manifest: &ImportManifest,
) -> Verdict {
    let normalized = normalize(system, name);

    // Tier 1 — the manifest. A hit or an error settles it without paying for the
    // library scan.
    // A `Some` here is Found or EnvironmentError; `None` means undecided — not in
    // scope, existence still entirely open.
    if let Some(verdict) = fast_check(index, system, &normalized, manifest) {
        return verdict;
    }

    // Tier 2 — the wider library. Only reached when tier 1 ran cleanly and found
    // nothing, so a negative here is a negative about the manifest too.
    match index.lookup_in_library(system, &normalized) {
        // Still an infra failure, still evidence of nothing.
        Err(e) => Verdict::EnvironmentError(e),
        Ok(Some(declaration)) => {
            let add_import = declaration.effective_module();
            Verdict::NotInCurrentImportScope {
                declaration,
                add_import,
            }
        }
        // Both tiers completed and neither has it. THIS is the only place a
        // genuine absence is concluded.
        Ok(None) => Verdict::UnknownDeclaration {
            queried: normalized,
        },
    }
}

/// Escalating convenience wrapper: run [`fast_check`], and consult the library
/// only when `deep` is set and the fast path came back undecided.
///
/// With `deep == false` an undecided fast path yields
/// [`Verdict::EnvironmentError`] with [`IndexErrorKind::IndexMissing`] — *not*
/// [`Verdict::UnknownDeclaration`]. That is intentional and is the honest report:
/// we did not look in the library, so we have no evidence either way, and the
/// verdict says exactly that.
pub fn check(
    index: &dyn DeclarationIndex,
    system: FormalSystem,
    name: &str,
    manifest: &ImportManifest,
    deep: bool,
) -> Verdict {
    if deep {
        return deep_check(index, system, name, manifest);
    }
    match fast_check(index, system, name, manifest) {
        Some(verdict) => verdict,
        None => Verdict::EnvironmentError(IndexError::new(
            IndexErrorKind::IndexMissing,
            format!(
                "`{}` is not in the current import scope, and the wider-library index was not \
                 consulted (deep_check disabled). No conclusion about existence is available.",
                normalize(system, name)
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    // -- mock indices ---------------------------------------------------------

    /// A mock index over an explicit `(name, module)` table, which records how
    /// many times each tier was consulted so tests can assert the fast path does
    /// not pay for the deep one.
    struct MockIndex {
        /// Everything the wider library knows.
        library: Vec<Declaration>,
        manifest_calls: Cell<usize>,
        library_calls: Cell<usize>,
        /// When set, the manifest tier fails instead of answering.
        manifest_error: Option<IndexError>,
        /// When set, the library tier fails instead of answering.
        library_error: Option<IndexError>,
    }

    impl MockIndex {
        fn new(library: Vec<Declaration>) -> Self {
            Self {
                library,
                manifest_calls: Cell::new(0),
                library_calls: Cell::new(0),
                manifest_error: None,
                library_error: None,
            }
        }

        fn failing_manifest(mut self, e: IndexError) -> Self {
            self.manifest_error = Some(e);
            self
        }

        fn failing_library(mut self, e: IndexError) -> Self {
            self.library_error = Some(e);
            self
        }
    }

    impl DeclarationIndex for MockIndex {
        fn lookup_in_manifest(
            &self,
            system: FormalSystem,
            name: &str,
            manifest: &ImportManifest,
        ) -> Result<Option<Declaration>, IndexError> {
            self.manifest_calls.set(self.manifest_calls.get() + 1);
            if let Some(e) = &self.manifest_error {
                return Err(e.clone());
            }
            Ok(self
                .library
                .iter()
                .find(|d| d.system == system && d.name == name && manifest.covers(d))
                .cloned())
        }

        fn lookup_in_library(
            &self,
            system: FormalSystem,
            name: &str,
        ) -> Result<Option<Declaration>, IndexError> {
            self.library_calls.set(self.library_calls.get() + 1);
            if let Some(e) = &self.library_error {
                return Err(e.clone());
            }
            Ok(self
                .library
                .iter()
                .find(|d| d.system == system && d.name == name)
                .cloned())
        }
    }

    fn mathlib() -> Vec<Declaration> {
        vec![
            Declaration::in_module(FormalSystem::Lean, "Nat.succ_le_succ", "Mathlib.Data.Nat.Basic")
                .with_signature("∀ {n m : ℕ}, n ≤ m → n.succ ≤ m.succ")
                .with_kind("theorem"),
            // Exists, but lives under a module the test manifest does not import.
            Declaration::in_module(
                FormalSystem::Lean,
                "Finset.sum_range_succ",
                "Mathlib.Algebra.BigOperators.Basic",
            )
            .with_kind("theorem"),
        ]
    }

    fn imports_nat_only() -> ImportManifest {
        ImportManifest::new(FormalSystem::Lean, ["Mathlib.Data.Nat"])
    }

    // -- the four verdicts are all producible and distinguishable -------------

    #[test]
    fn found_when_in_scope() {
        let index = MockIndex::new(mathlib());
        let v = deep_check(
            &index,
            FormalSystem::Lean,
            "Nat.succ_le_succ",
            &imports_nat_only(),
        );
        match &v {
            Verdict::Found(d) => {
                assert_eq!(d.name, "Nat.succ_le_succ");
                // The location/signature come back with the verdict.
                assert_eq!(d.module.as_deref(), Some("Mathlib.Data.Nat.Basic"));
                assert!(d.signature.is_some());
            }
            other => panic!("expected Found, got {other:?}"),
        }
        assert_eq!(v.tag(), "found");
        assert!(v.exists_somewhere());
        assert!(!v.is_evidence_of_absence());
        // A hit in the manifest never pays for the 15-40s library scan.
        assert_eq!(index.library_calls.get(), 0);
    }

    #[test]
    fn not_in_current_import_scope_when_present_only_in_the_wider_library() {
        let index = MockIndex::new(mathlib());
        let v = deep_check(
            &index,
            FormalSystem::Lean,
            "Finset.sum_range_succ",
            &imports_nat_only(),
        );
        match &v {
            Verdict::NotInCurrentImportScope {
                declaration,
                add_import,
            } => {
                assert_eq!(declaration.name, "Finset.sum_range_succ");
                // The ACTION is concrete: this exact import.
                assert_eq!(add_import.as_deref(), Some("Mathlib.Algebra.BigOperators.Basic"));
            }
            other => panic!("expected NotInCurrentImportScope, got {other:?}"),
        }
        assert!(v.exists_somewhere(), "it exists — the branch is still alive");
        assert!(
            !v.is_evidence_of_absence(),
            "an out-of-scope hit must NEVER read as absence"
        );
        assert!(v.action().contains("ADD THE IMPORT"));
    }

    #[test]
    fn unknown_declaration_when_absent_everywhere() {
        let index = MockIndex::new(mathlib());
        let v = deep_check(
            &index,
            FormalSystem::Lean,
            "Nat.totally_made_up_lemma",
            &imports_nat_only(),
        );
        assert_eq!(
            v,
            Verdict::UnknownDeclaration {
                queried: "Nat.totally_made_up_lemma".to_string()
            }
        );
        assert!(v.is_evidence_of_absence());
        assert!(!v.exists_somewhere());
        assert!(v.action().contains("TRY ANOTHER NAME"));
        // Both tiers were consulted before concluding absence.
        assert_eq!(index.manifest_calls.get(), 1);
        assert_eq!(index.library_calls.get(), 1);
    }

    #[test]
    fn all_four_verdicts_are_distinguishable() {
        let tags = [
            deep_check(
                &MockIndex::new(mathlib()),
                FormalSystem::Lean,
                "Nat.succ_le_succ",
                &imports_nat_only(),
            )
            .tag(),
            deep_check(
                &MockIndex::new(mathlib()),
                FormalSystem::Lean,
                "Finset.sum_range_succ",
                &imports_nat_only(),
            )
            .tag(),
            deep_check(
                &MockIndex::new(mathlib()),
                FormalSystem::Lean,
                "Nat.nope",
                &imports_nat_only(),
            )
            .tag(),
            deep_check(
                &MockIndex::new(mathlib()).failing_manifest(IndexError::timeout("repl deadline")),
                FormalSystem::Lean,
                "Nat.succ_le_succ",
                &imports_nat_only(),
            )
            .tag(),
        ];
        assert_eq!(
            tags,
            [
                "found",
                "not_in_current_import_scope",
                "unknown_declaration",
                "environment_error",
            ]
        );
        // Exactly one of the four is evidence of absence.
        assert_eq!(
            tags.iter().filter(|t| **t == "unknown_declaration").count(),
            1
        );
    }

    // -- THE critical invariant: infra failure is never absence ---------------

    #[test]
    fn a_failing_manifest_tier_yields_environment_error_never_unknown() {
        let index = MockIndex::new(mathlib())
            .failing_manifest(IndexError::toolchain_unavailable("lean binary not on PATH"));
        // Ask for a name that DOES exist, and for one that does NOT. Neither may
        // come back as UnknownDeclaration — the lookup never ran.
        for name in ["Nat.succ_le_succ", "Nat.totally_made_up_lemma"] {
            let v = deep_check(&index, FormalSystem::Lean, name, &imports_nat_only());
            match &v {
                Verdict::EnvironmentError(e) => {
                    assert_eq!(e.kind, IndexErrorKind::ToolchainUnavailable);
                }
                other => panic!("expected EnvironmentError for {name}, got {other:?}"),
            }
            assert!(
                !v.is_evidence_of_absence(),
                "an infra failure is evidence of NOTHING"
            );
            assert!(!v.exists_somewhere());
            assert!(!v.licenses_abandoning_the_name());
            assert!(v.action().contains("evidence of NOTHING"));
        }
        // A broken manifest tier short-circuits: we never even try the library.
        assert_eq!(index.library_calls.get(), 0);
    }

    #[test]
    fn a_failing_library_tier_yields_environment_error_never_unknown() {
        // The manifest tier runs cleanly and finds nothing — the exact situation
        // that WOULD have produced Unknown had the library tier answered. It
        // errors instead, so absence must not be inferred.
        let index = MockIndex::new(mathlib())
            .failing_library(IndexError::index_missing("decl index not built"));
        let v = deep_check(
            &index,
            FormalSystem::Lean,
            "Nat.totally_made_up_lemma",
            &imports_nat_only(),
        );
        match &v {
            Verdict::EnvironmentError(e) => assert_eq!(e.kind, IndexErrorKind::IndexMissing),
            other => panic!("expected EnvironmentError, got {other:?}"),
        }
        assert!(
            !v.is_evidence_of_absence(),
            "a dead index must never be read as 'the library lacks it'"
        );
        assert_eq!(index.library_calls.get(), 1, "the library tier was attempted");
    }

    #[test]
    fn environment_error_is_never_equal_to_unknown_for_the_same_name() {
        let good = MockIndex::new(mathlib());
        let broken =
            MockIndex::new(mathlib()).failing_library(IndexError::timeout("40s deadline hit"));
        let name = "Nat.totally_made_up_lemma";
        let real_absence = deep_check(&good, FormalSystem::Lean, name, &imports_nat_only());
        let infra_failure = deep_check(&broken, FormalSystem::Lean, name, &imports_nat_only());
        assert_ne!(
            real_absence, infra_failure,
            "the two must remain structurally distinct"
        );
        assert!(real_absence.is_evidence_of_absence());
        assert!(!infra_failure.is_evidence_of_absence());
    }

    // -- fast path: cheap, and structurally unable to say Unknown -------------

    #[test]
    fn fast_path_does_not_consult_the_deep_index() {
        let index = MockIndex::new(mathlib());

        // (a) an in-scope hit
        let hit = fast_check(
            &index,
            FormalSystem::Lean,
            "Nat.succ_le_succ",
            &imports_nat_only(),
        );
        assert!(matches!(hit, Some(Verdict::Found(_))));

        // (b) an out-of-scope name
        let miss = fast_check(
            &index,
            FormalSystem::Lean,
            "Finset.sum_range_succ",
            &imports_nat_only(),
        );
        assert_eq!(miss, None, "the fast path reports UNDECIDED, not Unknown");

        // (c) a name that exists nowhere
        let nowhere = fast_check(
            &index,
            FormalSystem::Lean,
            "Nat.totally_made_up_lemma",
            &imports_nat_only(),
        );
        assert_eq!(
            nowhere, None,
            "even a genuinely absent name is UNDECIDED on the fast path"
        );

        // The expensive tier was never touched on any of the three.
        assert_eq!(index.library_calls.get(), 0);
        assert_eq!(index.manifest_calls.get(), 3);
    }

    #[test]
    fn fast_path_never_produces_unknown_declaration() {
        let index = MockIndex::new(mathlib());
        for name in [
            "Nat.succ_le_succ",
            "Finset.sum_range_succ",
            "Nat.totally_made_up_lemma",
            "",
        ] {
            if let Some(v) = fast_check(&index, FormalSystem::Lean, name, &imports_nat_only()) {
                assert!(
                    !v.is_evidence_of_absence(),
                    "fast_check may never conclude absence (name: {name:?})"
                );
            }
        }
    }

    #[test]
    fn shallow_check_reports_no_evidence_rather_than_unknown() {
        // `check(.., deep = false)` on an out-of-scope name must not fabricate a
        // limitation; it must say "we did not look".
        let index = MockIndex::new(mathlib());
        let v = check(
            &index,
            FormalSystem::Lean,
            "Finset.sum_range_succ",
            &imports_nat_only(),
            false,
        );
        assert!(matches!(v, Verdict::EnvironmentError(_)));
        assert!(!v.is_evidence_of_absence());
        assert_eq!(index.library_calls.get(), 0);

        // The same name under deep_check resolves to the actionable verdict.
        let deep = check(
            &index,
            FormalSystem::Lean,
            "Finset.sum_range_succ",
            &imports_nat_only(),
            true,
        );
        assert!(matches!(deep, Verdict::NotInCurrentImportScope { .. }));
    }

    #[test]
    fn deep_check_agrees_with_fast_check_whenever_fast_check_decides() {
        let index = MockIndex::new(mathlib());
        let manifest = imports_nat_only();
        for name in ["Nat.succ_le_succ", "Finset.sum_range_succ", "Nat.nope"] {
            if let Some(fast) = fast_check(&index, FormalSystem::Lean, name, &manifest) {
                assert_eq!(
                    fast,
                    deep_check(&index, FormalSystem::Lean, name, &manifest),
                    "escalating must never change a decided verdict ({name})"
                );
            }
        }
    }

    // -- manifest scoping ------------------------------------------------------

    #[test]
    fn manifest_coverage_is_segment_aware() {
        let m = ImportManifest::new(FormalSystem::Lean, ["Mathlib.Data"]);
        assert!(m.covers_module("Mathlib.Data"));
        assert!(m.covers_module("Mathlib.Data.Nat.Basic"));
        assert!(
            !m.covers_module("Mathlib.DataFlow"),
            "prefix matching must respect segment boundaries"
        );
        assert!(!m.covers_module("Mathlib"));
        // A declaration with no known module is NOT assumed visible.
        assert!(!m.covers(&Declaration::new(FormalSystem::Lean, "orphan")));
    }

    #[test]
    fn manifest_is_order_independent_and_deduplicated() {
        let a = ImportManifest::new(FormalSystem::Lean, ["Mathlib.Order", "Mathlib.Data"]);
        let b = ImportManifest::new(
            FormalSystem::Lean,
            ["Mathlib.Data", "Mathlib.Order", "Mathlib.Data"],
        );
        assert_eq!(a, b);
        assert_eq!(a.imports(), ["Mathlib.Data", "Mathlib.Order"]);
        assert!(ImportManifest::empty(FormalSystem::Lean)
            .imports()
            .is_empty());
    }

    #[test]
    fn empty_manifest_still_finds_via_deep_check() {
        // The pathological scope-collapse setup: NOTHING is imported. Every name
        // must come back as "add the import", never as "the library lacks it".
        let index = MockIndex::new(mathlib());
        let empty = ImportManifest::empty(FormalSystem::Lean);
        for name in ["Nat.succ_le_succ", "Finset.sum_range_succ"] {
            let v = deep_check(&index, FormalSystem::Lean, name, &empty);
            assert!(
                matches!(v, Verdict::NotInCurrentImportScope { .. }),
                "{name} must be reported as importable, got {v:?}"
            );
        }
    }

    // -- names ------------------------------------------------------------------

    #[test]
    fn normalization_strips_decoration_without_changing_the_name() {
        assert_eq!(
            normalize(FormalSystem::Lean, "  `Nat.succ_le_succ`  "),
            "Nat.succ_le_succ"
        );
        assert_eq!(normalize(FormalSystem::Lean, "Nat..add"), "Nat.add");
        assert_eq!(normalize(FormalSystem::Rocq, ".Coq.Init.Nat.add"), "Coq.Init.Nat.add");
        assert_eq!(normalize(FormalSystem::Lean, "«odd name»"), "odd name");
        // Case is never touched: all six systems are case-sensitive.
        assert_eq!(normalize(FormalSystem::Lean, "Nat.Add"), "Nat.Add");
        // Metamath labels are flat; the separator rules do not apply.
        assert_eq!(separator(FormalSystem::Metamath), None);
        assert_eq!(normalize(FormalSystem::Metamath, " ax-mp "), "ax-mp");
        assert_eq!(module_of(FormalSystem::Metamath, "ax-mp"), None);
    }

    #[test]
    fn module_is_derived_from_the_qualified_name_when_the_index_omits_it() {
        assert_eq!(
            module_of(FormalSystem::Lean, "Mathlib.Data.Nat.Basic.succ_le"),
            Some("Mathlib.Data.Nat.Basic".to_string())
        );
        assert_eq!(module_of(FormalSystem::Isabelle, "Nat.add_commute"), Some("Nat".into()));
        assert_eq!(module_of(FormalSystem::Agda, "Data.Nat.Base.suc"), Some("Data.Nat.Base".into()));
        assert_eq!(module_of(FormalSystem::Lean, "unqualified"), None);
        // A record with no explicit module falls back to its dotted name.
        let d = Declaration::new(FormalSystem::Lean, "Nat.succ_le_succ");
        assert_eq!(d.effective_module(), Some("Nat".to_string()));
    }

    #[test]
    fn lookups_are_normalized_before_dispatch() {
        // A model that emits a backticked name must still get Found, not Unknown.
        let index = MockIndex::new(mathlib());
        let v = deep_check(
            &index,
            FormalSystem::Lean,
            " `Nat.succ_le_succ` ",
            &imports_nat_only(),
        );
        assert!(matches!(v, Verdict::Found(_)));
    }

    #[test]
    fn per_system_dispatch_does_not_cross_systems() {
        // The same name in a different system is a different declaration.
        let index = MockIndex::new(mathlib());
        let v = deep_check(
            &index,
            FormalSystem::Rocq,
            "Nat.succ_le_succ",
            &ImportManifest::new(FormalSystem::Rocq, ["Coq.Init"]),
        );
        assert!(v.is_evidence_of_absence(), "got {v:?}");
    }

    // -- determinism -----------------------------------------------------------

    #[test]
    fn verdicts_are_deterministic() {
        let manifest = imports_nat_only();
        for name in [
            "Nat.succ_le_succ",
            "Finset.sum_range_succ",
            "Nat.totally_made_up_lemma",
        ] {
            let a = deep_check(
                &MockIndex::new(mathlib()),
                FormalSystem::Lean,
                name,
                &manifest,
            );
            let b = deep_check(
                &MockIndex::new(mathlib()),
                FormalSystem::Lean,
                name,
                &manifest,
            );
            assert_eq!(a, b);
            assert_eq!(a.action(), b.action());
        }
    }

    #[test]
    fn every_verdict_carries_an_actionable_message() {
        let verdicts = [
            Verdict::Found(Declaration::new(FormalSystem::Lean, "Nat.add")),
            Verdict::NotInCurrentImportScope {
                declaration: Declaration::new(FormalSystem::Lean, "Finset.sum"),
                add_import: None,
            },
            Verdict::UnknownDeclaration {
                queried: "Nat.nope".into(),
            },
            Verdict::EnvironmentError(IndexError::timeout("deadline")),
        ];
        for v in &verdicts {
            assert!(!v.action().is_empty(), "{} needs an action", v.tag());
            assert!(!v.tag().is_empty());
        }
        // Only one is evidence of absence, and it is not the error.
        assert_eq!(
            verdicts.iter().filter(|v| v.is_evidence_of_absence()).count(),
            1
        );
    }

    #[test]
    fn index_error_kinds_have_stable_tags() {
        for kind in [
            IndexErrorKind::ToolchainUnavailable,
            IndexErrorKind::IndexMissing,
            IndexErrorKind::Timeout,
            IndexErrorKind::Malformed,
            IndexErrorKind::Other,
        ] {
            assert!(!kind.tag().is_empty());
            // Transience is advisory and never implies absence.
            let v = Verdict::EnvironmentError(IndexError::new(kind, "detail"));
            assert!(!v.is_evidence_of_absence());
        }
        assert!(IndexErrorKind::Timeout.is_transient());
        assert!(!IndexErrorKind::IndexMissing.is_transient());
    }
}
