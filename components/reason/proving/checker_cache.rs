//! Hash-consed checker-result cache: remember that a goal has already cleared
//! the verification gate so its subproof is not re-verified across candidates.
//!
//! During a proving/search run the same sub-goal recurs constantly: two candidate
//! proofs of a theorem often share a lemma or an obligation, a portfolio re-attacks
//! the same statement across rounds, and a refinement loop re-emits an unchanged
//! sub-goal. Re-running the (expensive) checker/gate on an *already-verified*
//! sub-goal is pure waste. This module memoises the gate's verdict, keyed on a
//! stable hash of the complete verification input: formal system, canonical
//! statement, ordered context, exact proof source, checker identity, and gate
//! policy. A verdict is reusable only when every one of those fields matches.
//!
//! ## What is cached — and why it is SOUND
//!
//! The cache records that one exact proof artifact cleared one exact gate. It is
//! not a theorem database and does not substitute one proof for another. Including
//! the proof source prevents a valid proof of a goal from laundering a different,
//! unchecked candidate for the same goal. Including checker identity and policy
//! prevents reuse across toolchains, live/mock modes, axiom policies, or gate
//! revisions.
//!
//! Project/corpus identity belongs in `checker_identity`; callers must include it
//! when it can affect elaboration.
//!
//! ## Why the IMPORT MANIFEST is its own key field
//!
//! `checker_identity` is an INSTALLATION identity — binaries, runner, pinned
//! toolchain, project root, cache epoch. It is computed once per run and is
//! therefore constant across every problem in that run. It does NOT capture the
//! per-problem IMPORT CLOSURE, and neither does `proof_source` in general: a
//! backend may prepend imports drawn from the task/project rather than from the
//! candidate text, in which case two candidates with byte-identical source can be
//! elaborated against different environments.
//!
//! That gap is not hypothetical. A mined system recorded a live failure in which an
//! unvalidated import list let
//!
//! ```text
//! Mathlib
//! axiom cheat : False
//! ```
//!
//! through as an "import", baking a false axiom into the environment of every
//! subsequent proof. Under a key blind to imports, the FIRST proof verified in the
//! poisoned environment would be cached and then replayed as verified for inputs
//! elaborated in a clean one — and, worse, a proof verified in a clean environment
//! would be served for a request made in the poisoned one, hiding the poisoning.
//! [`VerificationCacheKey::import_manifest`] closes that: the environment a proof
//! was checked in is part of what the cache remembers.
//!
//! This is a KEY field, not a validator. It makes a changed environment a
//! mandatory MISS; it does not decide whether an import list is legitimate. Import
//! *validation* is the backend/gate's job, upstream.
//!
//! Contrast [`goal_cache`](crate::goal_cache),
//! which is a project-scoped cache of goal → proof text. This cache only skips a
//! repeated verification of the same proof under the same gate.
//!
//! ## The gate invariant (do not weaken it)
//!
//! The cache stores the OUTCOME of the gate; it never bypasses or manufactures a
//! gate decision. Two properties keep an UNVERIFIED result from ever being read as
//! verified:
//!
//! 1. **Live successes only, authoritatively.**
//!    [`insert_verified`](CheckerCache::insert_verified) is the sole insertion
//!    path. It requires every gate bit plus `live`; mock reports and partial or
//!    failed reports are refused.
//! 2. **No negative caching.** A *failed* attempt is NOT cached. Caching a failure
//!    would be unsound as a "do not try" signal — a goal that one proof strategy
//!    failed to discharge may well be provable by another — so a miss (whether the
//!    goal was never seen, or a prior attempt failed) always forces the caller back
//!    through the real gate. Omitting negative caching is both simpler and strictly
//!    safer than an advisory negative cache, so we omit it entirely.
//!
//! A cache hit defensively rechecks those report bits before returning the stored
//! report. The cache adds no trust; it only avoids identical recomputation.
//!
//! ## Key stability & determinism
//!
//! The key is a SHA-256 over length-framed, domain-separated fields (the same
//! anti-ambiguity framing as [`proof_import::content_id`](crate::proof_import)),
//! computed over whitespace-normalized statement/context inputs so cosmetically
//! different restatements (`"a  +\tb"` vs `"a + b"`) share a key. Proof source,
//! checker identity, and policy are hashed byte-for-byte; changing any of them is
//! a mandatory miss.
//! Context/hypotheses are treated as an ORDERED list — we do **not** reorder them —
//! so two different hypothesis orderings yield different keys. That errs toward
//! false MISSES (a redundant re-verification), never false HITS (an unsound reuse);
//! order-invariant goal matching is the (soundness-audited) job of
//! [`subsumption`](crate::subsumption)/[`goal_cache`](crate::goal_cache), not of
//! this cheap string-level memo. No wall-clock reads, no RNG: the same inputs
//! always hash to the same key.

use crate::prover::formal::FormalSystem;
use crate::prover::model::VerificationReport;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::HashMap;

/// Domain-separation tag mixed in first so this key space can never collide with
/// another SHA-256 pre-image in the crate.
/// Bumped to `v3` when the import manifest joined the key: a v2 key and a v3 key
/// for the same inputs must not collide, and no v2-era entry may be read as v3.
const DOMAIN: &[u8] = b"theoremata.checker_cache.v3";

/// Absorb one length-framed, tagged field into the hasher. Framing every field
/// with its byte length makes the pre-image UNAMBIGUOUS: no choice of
/// system/statement/hypothesis text can be re-split into different fields that
/// collide (the classic concatenation ambiguity). Mirrors
/// [`proof_import`](crate::proof_import)'s `absorb`.
fn absorb(hasher: &mut Sha256, tag: &[u8], data: &[u8]) {
    hasher.update((tag.len() as u64).to_be_bytes());
    hasher.update(tag);
    hasher.update((data.len() as u64).to_be_bytes());
    hasher.update(data);
}

/// Lowercase hex of a digest (the crate's shared idiom, re-inlined per module).
fn hex_lower(bytes: impl AsRef<[u8]>) -> String {
    use std::fmt::Write as _;
    let bytes = bytes.as_ref();
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Deterministic whitespace normalization: trim, and collapse every run of ASCII
/// whitespace (spaces, tabs, newlines) to a single space. This is the ONLY
/// canonicalization applied — it is content-preserving, so it can only merge
/// cosmetically-equivalent restatements, never conflate genuinely different ones.
fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Every input that can change a verification verdict.
#[derive(Debug, Clone, Copy)]
pub struct VerificationCacheKey<'a> {
    pub system: FormalSystem,
    /// Canonical theorem statement. Whitespace is normalized before hashing.
    pub canonical_statement: &'a str,
    /// Ordered local hypotheses/import context. Order is intentionally preserved.
    pub ordered_context: &'a [String],
    /// Exact candidate source. This is never normalized.
    pub proof_source: &'a str,
    /// The per-problem IMPORT CLOSURE this candidate is elaborated against, in
    /// the order the backend applies it (`["Mathlib", "Mathlib.Tactic"]`).
    ///
    /// Order is preserved and each entry is hashed whitespace-normalized. Pass
    /// the manifest the backend will ACTUALLY use, not the one the model asked
    /// for. An empty slice means "no imports beyond what `proof_source` itself
    /// declares" — pass `&[]` only when that is literally true, because an empty
    /// manifest and a populated one are different keys, which is the point.
    ///
    /// See the module docs for why `checker_identity` does not cover this.
    pub import_manifest: &'a [String],
    /// Backend/toolchain/corpus identity, including live vs mock mode.
    pub checker_identity: &'a str,
    /// Gate/policy fingerprint (axiom whitelist, hardening switches, gate epoch).
    pub policy_fingerprint: &'a str,
}

/// Stable hex SHA-256 for a complete verification input. Pure and deterministic;
/// exposed so key derivation can be audited directly.
pub fn cache_key(input: &VerificationCacheKey<'_>) -> String {
    let mut hasher = Sha256::new();
    absorb(&mut hasher, DOMAIN, &[]);
    absorb(&mut hasher, b"system", input.system.as_str().as_bytes());
    absorb(
        &mut hasher,
        b"stmt",
        normalize(input.canonical_statement).as_bytes(),
    );
    // Frame the hypothesis count so `[h]` with a long `h` cannot look like `[h, g]`.
    hasher.update((input.ordered_context.len() as u64).to_be_bytes());
    for hyp in input.ordered_context {
        absorb(&mut hasher, b"hyp", normalize(hyp).as_bytes());
    }
    absorb(&mut hasher, b"proof", input.proof_source.as_bytes());
    // Frame the import count for the same anti-ambiguity reason as the context.
    hasher.update((input.import_manifest.len() as u64).to_be_bytes());
    for import in input.import_manifest {
        absorb(&mut hasher, b"import", normalize(import).as_bytes());
    }
    absorb(&mut hasher, b"checker", input.checker_identity.as_bytes());
    absorb(&mut hasher, b"policy", input.policy_fingerprint.as_bytes());
    hex_lower(hasher.finalize())
}

fn report_is_live_success(report: &VerificationReport) -> bool {
    report.live
        && report.lexically_verified
        && report.axioms_clean
        && report.statement_preserved
        && report.lexical_clean
        && report.hardening_clean == Some(true)
}

/// In-memory, hash-consed cache of live gate verdicts for exact proof inputs.
///
/// Interior mutability via [`RefCell`] (matching the single-threaded interior
/// mutability used elsewhere in `proving/`, e.g. `refine_ops`/`sketch`) so a
/// borrowed `&CheckerCache` can be threaded through a candidate loop and both
/// looked up and populated without a `&mut` at every call site. It is deliberately
/// NOT `Sync`; a threaded search that needs to share one cache should wrap it in a
/// `Mutex` at that boundary rather than complicate this memo.
#[derive(Debug, Default)]
pub struct CheckerCache {
    entries: RefCell<HashMap<String, VerificationReport>>,
}

impl CheckerCache {
    /// A fresh, empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of distinct verified inputs currently cached.
    pub fn len(&self) -> usize {
        self.entries.borrow().len()
    }

    /// Whether the cache holds no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.borrow().is_empty()
    }

    /// Look up the cached gate verdict for a complete verification input.
    /// Returns a clone of the stored [`VerificationReport`] on a hit, or `None` on
    /// a miss. A `None` MUST send the caller back through the real gate — the cache
    /// never stands in for the gate on a miss.
    pub fn get(&self, input: &VerificationCacheKey<'_>) -> Option<VerificationReport> {
        let key = cache_key(input);
        self.entries
            .borrow()
            .get(&key)
            .filter(|report| report_is_live_success(report))
            .cloned()
    }

    /// Whether this exact verification input has a cached live verdict.
    pub fn contains_verified(&self, input: &VerificationCacheKey<'_>) -> bool {
        self.get(input).is_some()
    }

    /// Record that one complete verification input cleared the live gate. This is
    /// the sole insertion path and stores live successes only.
    ///
    /// The caller is expected to offer only a report the gate actually accepted; as
    /// a defensive floor this requires every gate bit and `live`, returning
    /// `false` without storing otherwise. Mock reports are never cached.
    /// Note: this floor is a guard against mis-wiring, NOT a certification decision
    /// — deciding what counts as "verified" remains the gate's job, upstream.
    ///
    /// Idempotent on the key: re-inserting the same input overwrites with the newer
    /// (equally-verified) report and does not grow the cache.
    pub fn insert_verified(
        &self,
        input: &VerificationCacheKey<'_>,
        report: VerificationReport,
    ) -> bool {
        if !report_is_live_success(&report) {
            return false;
        }
        let key = cache_key(input);
        self.entries.borrow_mut().insert(key, report);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A report that reflects a genuine gate pass.
    fn verified_report() -> VerificationReport {
        VerificationReport {
            lexically_verified: true,
            axioms_clean: true,
            statement_preserved: true,
            lexical_clean: true,
            hardening_clean: Some(true),
            live: true,
            detail: json!({"marker": "unit-test"}),
        }
    }

    /// A report that did NOT pass the lexical screen (a would-be failure).
    fn failed_report() -> VerificationReport {
        VerificationReport {
            lexically_verified: false,
            axioms_clean: false,
            statement_preserved: false,
            lexical_clean: false,
            hardening_clean: Some(false),
            live: true,
            detail: json!({"marker": "failed"}),
        }
    }

    fn mock_report() -> VerificationReport {
        VerificationReport {
            live: false,
            detail: json!({"marker": "mock"}),
            ..verified_report()
        }
    }

    fn ctx(hyps: &[&str]) -> Vec<String> {
        hyps.iter().map(|s| s.to_string()).collect()
    }

    /// No-import shorthand used by the pre-existing tests, which are about the
    /// other key fields.
    fn input<'a>(
        system: FormalSystem,
        statement: &'a str,
        context: &'a [String],
        proof: &'a str,
        checker: &'a str,
        policy: &'a str,
    ) -> VerificationCacheKey<'a> {
        input_with_imports(system, statement, context, &[], proof, checker, policy)
    }

    #[allow(clippy::too_many_arguments)]
    fn input_with_imports<'a>(
        system: FormalSystem,
        statement: &'a str,
        context: &'a [String],
        imports: &'a [String],
        proof: &'a str,
        checker: &'a str,
        policy: &'a str,
    ) -> VerificationCacheKey<'a> {
        VerificationCacheKey {
            system,
            canonical_statement: statement,
            ordered_context: context,
            proof_source: proof,
            import_manifest: imports,
            checker_identity: checker,
            policy_fingerprint: policy,
        }
    }

    #[test]
    fn miss_then_hit_returns_the_stored_verdict() {
        let cache = CheckerCache::new();
        let hyps = ctx(&["P x"]);
        let key = input(
            FormalSystem::Lean,
            "P x ⊢ Q x",
            &hyps,
            "theorem q : Q x := proof",
            "lean:live:v4.19",
            "gate-v2:axioms=[]",
        );

        assert!(cache.get(&key).is_none());
        assert!(!cache.contains_verified(&key));

        assert!(cache.insert_verified(&key, verified_report()));
        let got = cache.get(&key).expect("verified input must hit");
        assert!(got.lexically_verified);
        // The very report that was stored comes back (compared structurally, as
        // VerificationReport is not PartialEq).
        assert_eq!(
            serde_json::to_value(&got).unwrap(),
            serde_json::to_value(&verified_report()).unwrap(),
        );
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn key_is_stable_across_whitespace_equivalent_inputs() {
        let ctx_a = ctx(&["x  >  0"]);
        let ctx_b = ctx(&["x > 0"]);
        let a = input(
            FormalSystem::Lean,
            "a  +\t b   =  b + a",
            &ctx_a,
            "proof",
            "checker",
            "policy",
        );
        let b = input(
            FormalSystem::Lean,
            "a + b = b + a",
            &ctx_b,
            "proof",
            "checker",
            "policy",
        );
        assert_eq!(cache_key(&a), cache_key(&b));

        let cache = CheckerCache::new();
        assert!(cache.insert_verified(&a, verified_report()));
        assert!(cache.get(&b).is_some());
    }

    #[test]
    fn every_trust_relevant_input_is_keyed() {
        let ab = ctx(&["A", "B"]);
        let ba = ctx(&["B", "A"]);
        let base = input(
            FormalSystem::Lean,
            "⊢ G",
            &ab,
            "proof-one",
            "lean:live:v4.19",
            "policy-one",
        );
        let base_hash = cache_key(&base);
        let different = [
            input(
                FormalSystem::Rocq,
                "⊢ G",
                &ab,
                "proof-one",
                "lean:live:v4.19",
                "policy-one",
            ),
            input(
                FormalSystem::Lean,
                "⊢ H",
                &ab,
                "proof-one",
                "lean:live:v4.19",
                "policy-one",
            ),
            input(
                FormalSystem::Lean,
                "⊢ G",
                &ba,
                "proof-one",
                "lean:live:v4.19",
                "policy-one",
            ),
            input(
                FormalSystem::Lean,
                "⊢ G",
                &ab,
                "proof-two",
                "lean:live:v4.19",
                "policy-one",
            ),
            input(
                FormalSystem::Lean,
                "⊢ G",
                &ab,
                "proof-one",
                "lean:mock",
                "policy-one",
            ),
            input(
                FormalSystem::Lean,
                "⊢ G",
                &ab,
                "proof-one",
                "lean:live:v4.19",
                "policy-two",
            ),
        ];
        for changed in &different {
            assert_ne!(base_hash, cache_key(changed));
        }

        let cache = CheckerCache::new();
        assert!(cache.insert_verified(&base, verified_report()));
        for changed in &different {
            assert!(cache.get(changed).is_none(), "changed input must be a miss");
        }
    }

    #[test]
    fn failures_and_mock_reports_are_never_cached() {
        let hyps = ctx(&["h"]);
        let live_key = input(
            FormalSystem::Lean,
            "⊢ hard",
            &hyps,
            "proof",
            "lean:live",
            "policy",
        );
        let mock_key = input(
            FormalSystem::Lean,
            "⊢ hard",
            &hyps,
            "proof",
            "lean:mock",
            "policy",
        );
        let cache = CheckerCache::new();
        assert!(!cache.insert_verified(&live_key, failed_report()));
        assert!(!cache.insert_verified(&mock_key, mock_report()));
        assert!(cache.is_empty());
        assert!(cache.get(&live_key).is_none());
        assert!(cache.get(&mock_key).is_none());
    }

    #[test]
    fn insert_is_idempotent_on_the_complete_key() {
        let hyps = ctx(&["P x"]);
        let key = input(
            FormalSystem::Lean,
            "P x ⊢ Q x",
            &hyps,
            "proof",
            "checker",
            "policy",
        );
        let cache = CheckerCache::new();
        assert!(cache.insert_verified(&key, verified_report()));
        assert!(cache.insert_verified(&key, verified_report()));
        assert_eq!(cache.len(), 1);
    }

    /// The environment a proof was checked in is part of the key. This is the
    /// mined "unvalidated import list bakes in `axiom cheat : False`" failure:
    /// everything else about the request is identical, so without this field the
    /// poisoned and the clean environment would share one cache entry.
    #[test]
    fn changing_the_import_manifest_changes_the_key() {
        let hyps = ctx(&[]);
        let clean = ctx(&["Mathlib"]);
        let poisoned = ctx(&["Mathlib\naxiom cheat : False"]);
        let extra = ctx(&["Mathlib", "Mathlib.Tactic"]);
        let reordered = ctx(&["Mathlib.Tactic", "Mathlib"]);

        let base = input_with_imports(
            FormalSystem::Lean,
            "⊢ G",
            &hyps,
            &clean,
            "theorem g : G := proof",
            "lean:live:v4.19",
            "policy",
        );
        let base_hash = cache_key(&base);

        for changed in [&poisoned, &extra, &reordered, &hyps] {
            let other = input_with_imports(
                FormalSystem::Lean,
                "⊢ G",
                &hyps,
                changed,
                "theorem g : G := proof",
                "lean:live:v4.19",
                "policy",
            );
            assert_ne!(
                base_hash,
                cache_key(&other),
                "a different import closure must be a different key: {changed:?}"
            );
        }

        // And it is a real cache MISS, not merely a different hash: a verdict
        // earned in the clean environment is never served for the poisoned one.
        let cache = CheckerCache::new();
        assert!(cache.insert_verified(&base, verified_report()));
        let poisoned_key = input_with_imports(
            FormalSystem::Lean,
            "⊢ G",
            &hyps,
            &poisoned,
            "theorem g : G := proof",
            "lean:live:v4.19",
            "policy",
        );
        assert!(
            cache.get(&poisoned_key).is_none(),
            "a proof checked against a clean import closure must NOT be reused \
             for one checked against a poisoned closure"
        );
        // The converse direction is equally required.
        assert!(cache.insert_verified(&poisoned_key, verified_report()));
        assert!(cache.get(&base).is_some());
        assert_eq!(cache.len(), 2, "the two environments occupy distinct slots");
    }

    /// Imports are normalized like the other text fields, so cosmetic whitespace
    /// still shares a key.
    #[test]
    fn import_manifest_is_whitespace_normalized() {
        let hyps = ctx(&[]);
        let a = ctx(&["  Mathlib.Tactic  "]);
        let b = ctx(&["Mathlib.Tactic"]);
        let ka = input_with_imports(FormalSystem::Lean, "⊢ G", &hyps, &a, "p", "c", "pol");
        let kb = input_with_imports(FormalSystem::Lean, "⊢ G", &hyps, &b, "p", "c", "pol");
        assert_eq!(cache_key(&ka), cache_key(&kb));
    }

    #[test]
    fn key_is_a_stable_sha256_constant() {
        let hyps = ctx(&["P x"]);
        let key = input(
            FormalSystem::Lean,
            "P x ⊢ Q x",
            &hyps,
            "proof",
            "checker",
            "policy",
        );
        let k1 = cache_key(&key);
        let k2 = cache_key(&key);
        assert_eq!(k1, k2);
        assert_eq!(k1.len(), 64, "SHA-256 hex is 64 chars");
        assert!(k1.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
