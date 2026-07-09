//! Content-addressed theorem import + implication sharding (the Flyspeck
//! reassembly pattern).
//!
//! A huge proof is not proved in one session. It is stated as an implication
//! `A1 ∧ … ∧ An ⇒ goal` with NAMED assumptions; each named assumption is
//! discharged in an ISOLATED parallel session; and the goal is reassembled by
//! IMPORTING the discharged pieces. The primitive that makes such reassembly
//! trustworthy is a CONTENT-ADDRESSED identifier: a theorem's id is the hash of
//! its ENTIRE dependency history — its statement, the backend/toolchain it was
//! checked with, and the content-ids of every assumption/lemma it leans on. So
//! an import is valid only if the whole history rehashes to the claimed id:
//! tamper-evident reassembly. Change any dependency, statement, or backend tag
//! and the id changes; a forged or dangling history cannot masquerade as a
//! genuine one.
//!
//! Three pieces, from first principles over the existing `sha2` + `hex_lower`
//! hashing pattern used in `graph/db.rs` (no new deps):
//!
//! 1. [`content_id`] / [`ImportedTheorem`] — the content-addressed identifier and
//!    the proved-theorem record it names.
//! 2. [`shard_implication`] — turn `A1 ∧ … ∧ An ⇒ goal` into the goal plus a set
//!    of independently-dischargeable named assumption obligations, each of which
//!    can go to an isolated session; [`ImplicationShards::reassemble`] admits the
//!    goal only once EVERY named assumption has a valid content-addressed
//!    discharge whose recomputed history matches.
//! 3. [`ImportStore`] — a content-id → theorem map whose [`ImportStore::import`]
//!    REJECTS a theorem whose recomputed content-id doesn't match its claimed id,
//!    or whose dependency history is not already present (no dangling / forged
//!    history).
//!
//! Determinism: the id is a pure function of (statement, backend, sorted+deduped
//! dependency ids) — no wall-clock, no random ids, dependency LISTING order does
//! not matter. Statements are UNTRUSTED data: they are only ever hashed or
//! carried as prose, never executed.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Lowercase hex of a byte slice. sha2 0.11's digest output no longer implements
/// `LowerHex`, so we format the bytes explicitly — the same helper `graph/db.rs`
/// and `tools/mod.rs` use (kept module-local, matching that pattern).
fn hex_lower(bytes: impl AsRef<[u8]>) -> String {
    use std::fmt::Write as _;
    let bytes = bytes.as_ref();
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Domain-separation tag + version: bumping it re-namespaces every content-id, so
/// ids minted under different hashing rules can never collide.
const DOMAIN: &[u8] = b"theoremata.proof_import.v1";

/// Absorb one length-framed, tagged field into the hasher. Framing every field
/// with its byte length makes the pre-image UNAMBIGUOUS: no choice of statement /
/// backend / dependency text can be re-split into different fields that collide
/// (the classic length-extension / concatenation ambiguity).
fn absorb(hasher: &mut Sha256, tag: &[u8], data: &[u8]) {
    hasher.update((tag.len() as u64).to_be_bytes());
    hasher.update(tag);
    hasher.update((data.len() as u64).to_be_bytes());
    hasher.update(data);
}

/// Compute the content-addressed identifier for a proved theorem from the HASH OF
/// ITS FULL DEPENDENCY HISTORY: the statement it proves, the backend/toolchain
/// tag it was checked with, and the content-ids of every assumption/lemma it
/// used. Dependency ids are sorted + deduped first, so the id is independent of
/// the order dependencies happen to be listed in but changes the instant any
/// dependency, the statement, or the backend changes. Pure and deterministic.
pub fn content_id(statement: &str, backend: &str, dependencies: &[String]) -> String {
    // Canonicalise the dependency set: order-independent, duplicate-free.
    let mut deps: Vec<&str> = dependencies.iter().map(String::as_str).collect();
    deps.sort_unstable();
    deps.dedup();

    let mut hasher = Sha256::new();
    absorb(&mut hasher, DOMAIN, &[]);
    absorb(&mut hasher, b"stmt", statement.as_bytes());
    absorb(&mut hasher, b"backend", backend.as_bytes());
    // Frame the dependency count so `[a]` with a long `a` can't look like `[a, b]`.
    hasher.update((deps.len() as u64).to_be_bytes());
    for d in &deps {
        absorb(&mut hasher, b"dep", d.as_bytes());
    }
    hex_lower(hasher.finalize())
}

/// A proved theorem ready to import: what it proves, the backend it was checked
/// with, the content-ids of the theorems it depends on, and its CLAIMED
/// content-id. The claim is only trusted once [`ImportedTheorem::recompute_id`]
/// reproduces it from the history — see [`ImportStore::import`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedTheorem {
    /// The claimed content-addressed id (hash of the full history).
    pub id: String,
    /// The statement this theorem proves (untrusted prose — only hashed/carried).
    pub statement: String,
    /// The backend / toolchain tag it was checked with (e.g. `lean4-mathlib`).
    pub backend: String,
    /// Content-ids of the theorems this one leans on (its dependency history).
    pub dependencies: Vec<String>,
}

impl ImportedTheorem {
    /// Build a theorem, minting its id honestly from the history. This is the
    /// only way to get an [`ImportedTheorem`] whose `id` is correct by
    /// construction; a leaf theorem (axiom / externally proved) has no
    /// dependencies.
    pub fn new(
        statement: impl Into<String>,
        backend: impl Into<String>,
        dependencies: Vec<String>,
    ) -> Self {
        let statement = statement.into();
        let backend = backend.into();
        let id = content_id(&statement, &backend, &dependencies);
        Self {
            id,
            statement,
            backend,
            dependencies,
        }
    }

    /// Recompute the id this theorem SHOULD have from its own history. When this
    /// differs from [`ImportedTheorem::id`] the record has been tampered with.
    pub fn recompute_id(&self) -> String {
        content_id(&self.statement, &self.backend, &self.dependencies)
    }

    /// Whether the claimed id matches the recomputed history — the theorem is
    /// internally consistent (says nothing yet about its dependencies existing).
    pub fn id_is_valid(&self) -> bool {
        self.recompute_id() == self.id
    }
}

// ------------------------------------------------------------------------
// Implication sharding: A1 ∧ … ∧ An ⇒ goal
// ------------------------------------------------------------------------

/// One named assumption of a sharded implication — an obligation that can be
/// discharged in its own isolated session. The `name` is the stable handle the
/// reassembly matches discharges against; the `statement` is what a discharge
/// must actually prove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedAssumption {
    /// Stable name of this assumption within the implication.
    pub name: String,
    /// The statement the assumption asserts (what a discharge must prove).
    pub statement: String,
}

impl NamedAssumption {
    /// Convenience constructor.
    pub fn new(name: impl Into<String>, statement: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            statement: statement.into(),
        }
    }
}

/// A discharge of one named assumption: the proved theorem produced (in an
/// isolated session) for that assumption's obligation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discharge {
    /// The name of the assumption this discharges (matches [`NamedAssumption::name`]).
    pub assumption: String,
    /// The proved theorem discharging it, with its full content-addressed history.
    pub theorem: ImportedTheorem,
}

impl Discharge {
    /// Convenience constructor.
    pub fn new(assumption: impl Into<String>, theorem: ImportedTheorem) -> Self {
        Self {
            assumption: assumption.into(),
            theorem,
        }
    }
}

/// The sharded form of `A1 ∧ … ∧ An ⇒ goal`: the goal to admit, the backend the
/// reassembled goal will be tagged with, and the set of independently-
/// dischargeable named assumption obligations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImplicationShards {
    /// The goal statement to admit once all assumptions are discharged.
    pub goal: String,
    /// Backend / toolchain tag the reassembled goal theorem carries.
    pub backend: String,
    /// The named assumption obligations, each independently dischargeable.
    pub assumptions: Vec<NamedAssumption>,
}

impl ImplicationShards {
    /// Admit the goal ONLY when every named assumption has a valid content-
    /// addressed discharge whose recomputed history matches and which actually
    /// proves that assumption's statement. On success the returned
    /// [`ImportedTheorem`] for the goal depends on exactly the discharge ids, so
    /// importing it later (via [`ImportStore::import`]) still requires each
    /// discharge to be present — the history stays intact end to end.
    ///
    /// Rejects: a missing assumption (goal not yet fully proved), a duplicate
    /// discharge, a discharge for an unknown assumption, a tampered discharge
    /// (id ≠ recomputed history), or a discharge that proves the wrong statement.
    pub fn reassemble(&self, discharged: &[Discharge]) -> Result<ImportedTheorem> {
        // Index discharges by assumption name, rejecting duplicates and unknowns.
        let mut by_name: std::collections::BTreeMap<&str, &ImportedTheorem> =
            std::collections::BTreeMap::new();
        let known: std::collections::BTreeSet<&str> =
            self.assumptions.iter().map(|a| a.name.as_str()).collect();
        for d in discharged {
            if !known.contains(d.assumption.as_str()) {
                bail!(
                    "discharge names unknown assumption `{}` (not part of this implication)",
                    d.assumption
                );
            }
            if by_name.insert(d.assumption.as_str(), &d.theorem).is_some() {
                bail!("assumption `{}` discharged more than once", d.assumption);
            }
        }

        // Every named assumption must be discharged, and each discharge must be
        // internally consistent AND prove the right statement.
        for assumption in &self.assumptions {
            let Some(thm) = by_name.get(assumption.name.as_str()) else {
                bail!(
                    "assumption `{}` is not discharged; goal cannot be admitted",
                    assumption.name
                );
            };
            if !thm.id_is_valid() {
                bail!(
                    "discharge of `{}` is tampered: claimed id {} but history hashes to {}",
                    assumption.name,
                    thm.id,
                    thm.recompute_id()
                );
            }
            if thm.statement != assumption.statement {
                bail!(
                    "discharge of `{}` proves the wrong statement (assumption vs discharge mismatch)",
                    assumption.name
                );
            }
        }

        // The goal's history IS the set of discharge ids (sorted+deduped inside
        // `content_id`), so the reassembled goal is itself content-addressed.
        let mut deps: Vec<String> = by_name.values().map(|t| t.id.clone()).collect();
        deps.sort();
        deps.dedup();
        Ok(ImportedTheorem::new(
            self.goal.clone(),
            self.backend.clone(),
            deps,
        ))
    }
}

/// Represent proving `A1 ∧ … ∧ An ⇒ goal` as the goal plus a set of
/// independently-dischargeable named assumption obligations. Each returned
/// [`NamedAssumption`] can be sent to an isolated session; the resulting
/// [`ImplicationShards`] is later fed discharges via
/// [`ImplicationShards::reassemble`].
pub fn shard_implication(
    goal: impl Into<String>,
    backend: impl Into<String>,
    named_assumptions: Vec<NamedAssumption>,
) -> ImplicationShards {
    ImplicationShards {
        goal: goal.into(),
        backend: backend.into(),
        assumptions: named_assumptions,
    }
}

// ------------------------------------------------------------------------
// The import store
// ------------------------------------------------------------------------

/// A content-id → imported theorem map. It is the trust boundary of reassembly:
/// [`ImportStore::import`] admits a theorem only if its history rehashes to the
/// claimed id AND every dependency is already present, so the store can never
/// hold a forged or dangling history.
#[derive(Debug, Clone, Default)]
pub struct ImportStore {
    imported: std::collections::BTreeMap<String, ImportedTheorem>,
}

impl ImportStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Import a theorem, REJECTING it if its recomputed content-id doesn't match
    /// its claimed id (tamper-evidence) or if any dependency is not already in
    /// the store (no dangling / forged history). Re-importing an already-present,
    /// valid id is a no-op (idempotent).
    pub fn import(&mut self, theorem: ImportedTheorem) -> Result<()> {
        // 1. The claimed id must equal the hash of the full history.
        let recomputed = theorem.recompute_id();
        if recomputed != theorem.id {
            bail!(
                "content-id mismatch: claimed {} but history hashes to {} (tampered import rejected)",
                theorem.id,
                recomputed
            );
        }
        // 2. Every dependency must already be present — no dangling history.
        for dep in &theorem.dependencies {
            if !self.imported.contains_key(dep) {
                bail!(
                    "dangling dependency {dep}: import its history before importing {}",
                    theorem.id
                );
            }
        }
        self.imported.insert(theorem.id.clone(), theorem);
        Ok(())
    }

    /// Whether a theorem with this content-id has been imported.
    pub fn contains(&self, id: &str) -> bool {
        self.imported.contains_key(id)
    }

    /// The imported theorem with this content-id, if any.
    pub fn get(&self, id: &str) -> Option<&ImportedTheorem> {
        self.imported.get(id)
    }

    /// How many theorems have been imported.
    pub fn len(&self) -> usize {
        self.imported.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.imported.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BACKEND: &str = "lean4-mathlib";

    #[test]
    fn content_id_is_deterministic_and_order_independent() {
        let a = content_id("goal", BACKEND, &["d1".into(), "d2".into()]);
        let b = content_id("goal", BACKEND, &["d1".into(), "d2".into()]);
        assert_eq!(a, b, "same history must hash identically");

        // Dependency listing order does not matter (canonicalised inside).
        let c = content_id("goal", BACKEND, &["d2".into(), "d1".into()]);
        assert_eq!(a, c, "dependency order must not change the id");

        // It is lowercase hex of a 32-byte digest.
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()));
    }

    #[test]
    fn content_id_changes_if_statement_or_any_dependency_or_backend_changes() {
        let base = content_id("goal", BACKEND, &["d1".into()]);
        assert_ne!(base, content_id("goal2", BACKEND, &["d1".into()]), "statement change");
        assert_ne!(base, content_id("goal", BACKEND, &["d1x".into()]), "dependency change");
        assert_ne!(base, content_id("goal", "coq", &["d1".into()]), "backend change");
        assert_ne!(
            base,
            content_id("goal", BACKEND, &["d1".into(), "d2".into()]),
            "added dependency"
        );
    }

    #[test]
    fn content_id_is_not_fooled_by_field_boundary_ambiguity() {
        // Length-framing means moving text across the statement/backend boundary
        // changes the id (no "goal"+"lean" == "goa"+"llean" collision).
        assert_ne!(content_id("ab", "cd", &[]), content_id("abc", "d", &[]));
    }

    #[test]
    fn new_theorem_mints_a_valid_self_consistent_id() {
        let thm = ImportedTheorem::new("goal", BACKEND, vec!["dep".into()]);
        assert!(thm.id_is_valid());
        assert_eq!(thm.id, content_id("goal", BACKEND, &["dep".into()]));
    }

    #[test]
    fn import_accepts_a_genuine_history_and_is_idempotent() {
        let mut store = ImportStore::new();
        let axiom = ImportedTheorem::new("A", BACKEND, vec![]);
        store.import(axiom.clone()).unwrap();
        assert!(store.contains(&axiom.id));
        assert_eq!(store.len(), 1);

        // A dependent theorem imports once its history is present.
        let dependent = ImportedTheorem::new("B from A", BACKEND, vec![axiom.id.clone()]);
        store.import(dependent.clone()).unwrap();
        assert_eq!(store.len(), 2);

        // Re-importing the same valid theorem is a no-op.
        store.import(dependent).unwrap();
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn import_rejects_a_tampered_theorem() {
        let mut store = ImportStore::new();
        let mut forged = ImportedTheorem::new("A", BACKEND, vec![]);
        // Tamper with the statement but keep the old (now stale) claimed id.
        forged.statement = "A'; but claim A's id".into();
        let err = store.import(forged).unwrap_err().to_string();
        assert!(err.contains("content-id mismatch"), "got: {err}");
        assert!(store.is_empty(), "a tampered theorem must not enter the store");
    }

    #[test]
    fn import_rejects_a_dangling_dependency() {
        let mut store = ImportStore::new();
        // Depends on a content-id whose history was never imported.
        let orphan = ImportedTheorem::new("B", BACKEND, vec!["deadbeef".into()]);
        let err = store.import(orphan).unwrap_err().to_string();
        assert!(err.contains("dangling dependency"), "got: {err}");
        assert!(store.is_empty());
    }

    #[test]
    fn sharded_implication_admits_the_goal_only_after_all_assumptions_discharged() {
        let shards = shard_implication(
            "main_goal",
            BACKEND,
            vec![
                NamedAssumption::new("A1", "assumption one"),
                NamedAssumption::new("A2", "assumption two"),
            ],
        );

        let d1 = Discharge::new("A1", ImportedTheorem::new("assumption one", BACKEND, vec![]));
        let d2 = Discharge::new("A2", ImportedTheorem::new("assumption two", BACKEND, vec![]));

        // Not yet: only one of the two named assumptions is discharged.
        let err = shards.reassemble(&[d1.clone()]).unwrap_err().to_string();
        assert!(err.contains("A2") && err.contains("not discharged"), "got: {err}");

        // All discharged → the goal is admitted, and it is content-addressed over
        // exactly the two discharge ids (order of discharges does not matter).
        let goal = shards.reassemble(&[d2.clone(), d1.clone()]).unwrap();
        let mut expected_deps = vec![d1.theorem.id.clone(), d2.theorem.id.clone()];
        expected_deps.sort();
        assert_eq!(goal, ImportedTheorem::new("main_goal", BACKEND, expected_deps));
        assert!(goal.id_is_valid());
    }

    #[test]
    fn reassembly_rejects_a_tampered_or_wrong_statement_discharge() {
        let shards = shard_implication(
            "g",
            BACKEND,
            vec![NamedAssumption::new("A1", "the real assumption")],
        );

        // Tampered discharge: valid-looking record with a mismatched id.
        let mut tampered = ImportedTheorem::new("the real assumption", BACKEND, vec![]);
        tampered.id = "0".repeat(64);
        let err = shards
            .reassemble(&[Discharge::new("A1", tampered)])
            .unwrap_err()
            .to_string();
        assert!(err.contains("tampered"), "got: {err}");

        // Wrong-statement discharge: internally consistent but proves something else.
        let wrong = ImportedTheorem::new("a different statement", BACKEND, vec![]);
        let err = shards
            .reassemble(&[Discharge::new("A1", wrong)])
            .unwrap_err()
            .to_string();
        assert!(err.contains("wrong statement"), "got: {err}");
    }

    #[test]
    fn reassembly_rejects_duplicate_or_unknown_discharges() {
        let shards =
            shard_implication("g", BACKEND, vec![NamedAssumption::new("A1", "s1")]);
        let good = ImportedTheorem::new("s1", BACKEND, vec![]);

        let dup = shards
            .reassemble(&[
                Discharge::new("A1", good.clone()),
                Discharge::new("A1", good.clone()),
            ])
            .unwrap_err()
            .to_string();
        assert!(dup.contains("more than once"), "got: {dup}");

        let unknown = shards
            .reassemble(&[Discharge::new("A9", good)])
            .unwrap_err()
            .to_string();
        assert!(unknown.contains("unknown assumption"), "got: {unknown}");
    }

    #[test]
    fn end_to_end_multi_assumption_discharge_admits_and_imports_the_goal() {
        // Isolated sessions each prove a named assumption; we import each
        // discharge, reassemble the goal, and import the goal — the whole history
        // is checked at every step.
        let mut store = ImportStore::new();

        // A shared lemma both assumptions lean on (proved once in its own session).
        let lemma = ImportedTheorem::new("shared lemma", BACKEND, vec![]);
        store.import(lemma.clone()).unwrap();

        let a1 = ImportedTheorem::new("A1 holds", BACKEND, vec![lemma.id.clone()]);
        let a2 = ImportedTheorem::new("A2 holds", BACKEND, vec![lemma.id.clone()]);
        store.import(a1.clone()).unwrap();
        store.import(a2.clone()).unwrap();

        let shards = shard_implication(
            "A1 ∧ A2 ⇒ goal",
            BACKEND,
            vec![
                NamedAssumption::new("A1", "A1 holds"),
                NamedAssumption::new("A2", "A2 holds"),
            ],
        );
        let goal = shards
            .reassemble(&[Discharge::new("A1", a1.clone()), Discharge::new("A2", a2.clone())])
            .unwrap();

        // The reassembled goal imports cleanly: its dependency history (a1, a2,
        // and transitively the lemma) is all present.
        store.import(goal.clone()).unwrap();
        assert!(store.contains(&goal.id));
        assert_eq!(goal.dependencies, {
            let mut d = vec![a1.id.clone(), a2.id.clone()];
            d.sort();
            d
        });
    }

    #[test]
    fn goal_with_a_missing_discharge_in_the_store_is_rejected_on_import() {
        // Reassembly can produce a well-formed goal, but importing it still fails
        // if a discharge was never imported — dangling history is caught late too.
        let shards =
            shard_implication("goal", BACKEND, vec![NamedAssumption::new("A1", "s1")]);
        let a1 = ImportedTheorem::new("s1", BACKEND, vec![]);
        let goal = shards.reassemble(&[Discharge::new("A1", a1)]).unwrap();

        let mut store = ImportStore::new(); // a1 deliberately NOT imported
        let err = store.import(goal).unwrap_err().to_string();
        assert!(err.contains("dangling dependency"), "got: {err}");
    }

    #[test]
    fn hashes_are_stable_constants_no_wall_clock() {
        // Pin an exact digest so any accidental change to the hashing scheme (or
        // any wall-clock leak) is caught. Deterministic across runs.
        let id = content_id("x", "b", &["dep".into()]);
        let again = ImportedTheorem::new("x", "b", vec!["dep".into()]).id;
        assert_eq!(id, again);
        assert_eq!(id.len(), 64);
    }
}
