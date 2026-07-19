//! Hash-consed checker-result cache: remember that a goal has already cleared
//! the verification gate so its subproof is not re-verified across candidates.
//!
//! During a proving/search run the same sub-goal recurs constantly: two candidate
//! proofs of a theorem often share a lemma or an obligation, a portfolio re-attacks
//! the same statement across rounds, and a refinement loop re-emits an unchanged
//! sub-goal. Re-running the (expensive) checker/gate on an *already-verified*
//! sub-goal is pure waste. This module memoises the gate's verdict, keyed on a
//! stable hash of `(formal system, normalized statement, normalized context)`, so
//! a sub-goal verified once is reused by every later candidate.
//!
//! ## What is cached — and why it is SOUND
//!
//! The cache records the fact that a GOAL — the statement `S` under hypotheses `H`
//! in formal system `Sys` — has a gate-accepted proof. It deliberately does **not**
//! include any proof term in the key: the verdict is a property of the *goal*, not
//! of one particular proof of it. A hit therefore means "this goal is already
//! established", so any candidate depending on `S` may skip re-verifying it. That
//! is sound because an established theorem stays established: once the gate has
//! accepted *some* proof of `S` under `H` in `Sys`, `S` is a theorem, and a later
//! candidate that relies on `S` inherits that fact regardless of how *it* would
//! have re-derived it.
//!
//! The key is intentionally **project-independent**: a formal verification verdict
//! is a property of `(system, statement, hypotheses)`, not of the project it was
//! discovered in — which is precisely what makes it reusable across candidates,
//! runs, and projects. (Contrast [`goal_cache`](crate::goal_cache), which is a
//! project-scoped, `Store`-backed cache of goal → *proof text* for reuse by the
//! prover; this cache is an in-memory memo of goal → *gate verdict* for reuse by
//! the verifier. They are complementary: one skips re-PROVING, this one skips
//! re-VERIFYING.)
//!
//! ## The gate invariant (do not weaken it)
//!
//! The cache stores the OUTCOME of the gate; it never bypasses or manufactures a
//! gate decision. Two properties keep an UNVERIFIED result from ever being read as
//! verified:
//!
//! 1. **Successes only, authoritatively.** [`insert_verified`](CheckerCache::insert_verified)
//!    is the sole insertion path, and it stores a [`VerificationReport`] only when
//!    that report is itself a pass (its `lexically_verified` floor holds). A caller
//!    is expected to offer only reports the gate actually accepted; the defensive
//!    floor refuses to cache an obviously-failed report even if mis-offered.
//! 2. **No negative caching.** A *failed* attempt is NOT cached. Caching a failure
//!    would be unsound as a "do not try" signal — a goal that one proof strategy
//!    failed to discharge may well be provable by another — so a miss (whether the
//!    goal was never seen, or a prior attempt failed) always forces the caller back
//!    through the real gate. Omitting negative caching is both simpler and strictly
//!    safer than an advisory negative cache, so we omit it entirely.
//!
//! A cache hit returns the very [`VerificationReport`] that was stored, so the
//! caller inspects a reused verdict exactly as it would a fresh one — the cache
//! adds no trust, it only avoids recomputation.
//!
//! ## Key stability & determinism
//!
//! The key is a SHA-256 over length-framed, domain-separated fields (the same
//! anti-ambiguity framing as [`proof_import::content_id`](crate::proof_import)),
//! computed over whitespace-*normalized* inputs so cosmetically different but
//! equivalent restatements (`"a  +\tb"` vs `"a + b"`) share a key. Normalization is
//! the conservative direction only: it collapses whitespace runs, nothing more.
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
const DOMAIN: &[u8] = b"theoremata.checker_cache.v1";

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

/// The stable cache key for a goal: a hex SHA-256 over the domain tag, the formal
/// system, the whitespace-normalized statement, and each whitespace-normalized
/// hypothesis (in order, count-framed). Pure and deterministic — exposed so the
/// key derivation can be tested and audited directly.
pub fn cache_key(system: FormalSystem, statement: &str, context: &[String]) -> String {
    let mut hasher = Sha256::new();
    absorb(&mut hasher, DOMAIN, &[]);
    absorb(&mut hasher, b"system", system.as_str().as_bytes());
    absorb(&mut hasher, b"stmt", normalize(statement).as_bytes());
    // Frame the hypothesis count so `[h]` with a long `h` cannot look like `[h, g]`.
    hasher.update((context.len() as u64).to_be_bytes());
    for hyp in context {
        absorb(&mut hasher, b"hyp", normalize(hyp).as_bytes());
    }
    hex_lower(hasher.finalize())
}

/// In-memory, hash-consed cache of gate verdicts for already-verified goals.
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

    /// Number of distinct verified goals currently cached.
    pub fn len(&self) -> usize {
        self.entries.borrow().len()
    }

    /// Whether the cache holds no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.borrow().is_empty()
    }

    /// Look up the cached gate verdict for a goal `(system, statement, context)`.
    /// Returns a clone of the stored [`VerificationReport`] on a hit, or `None` on
    /// a miss. A `None` MUST send the caller back through the real gate — the cache
    /// never stands in for the gate on a miss.
    pub fn get(
        &self,
        system: FormalSystem,
        statement: &str,
        context: &[String],
    ) -> Option<VerificationReport> {
        let key = cache_key(system, statement, context);
        self.entries.borrow().get(&key).cloned()
    }

    /// Whether a goal already has a cached VERIFIED verdict.
    pub fn contains_verified(
        &self,
        system: FormalSystem,
        statement: &str,
        context: &[String],
    ) -> bool {
        let key = cache_key(system, statement, context);
        self.entries.borrow().contains_key(&key)
    }

    /// Record that a goal `(system, statement, context)` cleared the gate, keyed by
    /// its stable hash. This is the SOLE insertion path and stores successes only.
    ///
    /// The caller is expected to offer only a report the gate actually accepted; as
    /// a defensive floor this refuses to cache a report whose `lexically_verified`
    /// is `false` (no genuine acceptance has that false), and returns `false`
    /// without storing. On a genuine pass it stores the report and returns `true`.
    /// Note: this floor is a guard against mis-wiring, NOT a certification decision
    /// — deciding what counts as "verified" remains the gate's job, upstream.
    ///
    /// Idempotent on the key: re-inserting the same goal overwrites with the newer
    /// (equally-verified) report and does not grow the cache.
    pub fn insert_verified(
        &self,
        system: FormalSystem,
        statement: &str,
        context: &[String],
        report: VerificationReport,
    ) -> bool {
        // Successes only: never let a non-passing report enter the cache, so a
        // later `get` can never surface an unverified result as verified.
        if !report.lexically_verified {
            return false;
        }
        let key = cache_key(system, statement, context);
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

    fn ctx(hyps: &[&str]) -> Vec<String> {
        hyps.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn miss_then_hit_returns_the_stored_verdict() {
        let cache = CheckerCache::new();
        let hyps = ctx(&["P x"]);

        // Miss before insert.
        assert!(cache.get(FormalSystem::Lean, "P x ⊢ Q x", &hyps).is_none());
        assert!(!cache.contains_verified(FormalSystem::Lean, "P x ⊢ Q x", &hyps));

        // Insert a verified verdict, then it hits and round-trips the report.
        assert!(cache.insert_verified(FormalSystem::Lean, "P x ⊢ Q x", &hyps, verified_report()));
        let got = cache
            .get(FormalSystem::Lean, "P x ⊢ Q x", &hyps)
            .expect("verified goal must hit");
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
        // The public key function collapses whitespace deterministically …
        let a = cache_key(FormalSystem::Lean, "a  +\t b   =  b + a", &ctx(&["x  >  0"]));
        let b = cache_key(FormalSystem::Lean, "a + b = b + a", &ctx(&["x > 0"]));
        assert_eq!(a, b, "whitespace-equivalent goals must share a key");

        // … and the cache honors that: an insert under one spelling hits under a
        // cosmetically different but equivalent spelling.
        let cache = CheckerCache::new();
        cache.insert_verified(
            FormalSystem::Lean,
            "  a + b   =   b + a ",
            &ctx(&["x > 0"]),
            verified_report(),
        );
        assert!(
            cache
                .get(FormalSystem::Lean, "a + b = b + a", &ctx(&["x   >   0"]))
                .is_some(),
            "a whitespace-equivalent restatement must hit the cache"
        );
    }

    #[test]
    fn distinct_statement_context_or_system_do_not_collide() {
        let base = cache_key(FormalSystem::Lean, "P x ⊢ Q x", &ctx(&["P x"]));

        // Different statement …
        assert_ne!(base, cache_key(FormalSystem::Lean, "P x ⊢ R x", &ctx(&["P x"])));
        // … different context …
        assert_ne!(base, cache_key(FormalSystem::Lean, "P x ⊢ Q x", &ctx(&["Q x"])));
        // … different context arity …
        assert_ne!(
            base,
            cache_key(FormalSystem::Lean, "P x ⊢ Q x", &ctx(&["P x", "R y"]))
        );
        // … different hypothesis ORDER (deliberately conservative: not merged) …
        let ord_a = cache_key(FormalSystem::Lean, "⊢ G", &ctx(&["A", "B"]));
        let ord_b = cache_key(FormalSystem::Lean, "⊢ G", &ctx(&["B", "A"]));
        assert_ne!(ord_a, ord_b, "hypothesis order is significant (no false hit)");
        // … and different formal system.
        assert_ne!(base, cache_key(FormalSystem::Rocq, "P x ⊢ Q x", &ctx(&["P x"])));

        // The framing prevents the classic concatenation collision: a statement
        // that "swallows" the hypothesis text must NOT key-equal the split form.
        assert_ne!(
            cache_key(FormalSystem::Lean, "P xP y", &ctx(&["", ""])),
            cache_key(FormalSystem::Lean, "P x", &ctx(&["P y"])),
        );

        // Distinct goals also do not collide in the live cache: two inserts land as
        // two entries and each returns its own verdict.
        let cache = CheckerCache::new();
        cache.insert_verified(FormalSystem::Lean, "P x ⊢ Q x", &ctx(&["P x"]), verified_report());
        cache.insert_verified(FormalSystem::Rocq, "P x ⊢ Q x", &ctx(&["P x"]), verified_report());
        assert_eq!(cache.len(), 2, "same goal in two systems are two entries");
        assert!(cache.get(FormalSystem::Isabelle, "P x ⊢ Q x", &ctx(&["P x"])).is_none());
    }

    #[test]
    fn a_failed_report_is_never_cached() {
        // The soundness floor: a non-passing report is refused, so a later `get`
        // can never surface an unverified result as verified.
        let cache = CheckerCache::new();
        let hyps = ctx(&["h"]);
        assert!(!cache.insert_verified(FormalSystem::Lean, "⊢ hard", &hyps, failed_report()));
        assert!(cache.is_empty());
        assert!(cache.get(FormalSystem::Lean, "⊢ hard", &hyps).is_none());
    }

    #[test]
    fn insert_is_idempotent_on_the_key() {
        let cache = CheckerCache::new();
        let hyps = ctx(&["P x"]);
        assert!(cache.insert_verified(FormalSystem::Lean, "P x ⊢ Q x", &hyps, verified_report()));
        // Re-insert the same goal (whitespace-different) — overwrite, not grow.
        assert!(cache.insert_verified(
            FormalSystem::Lean,
            "P x   ⊢   Q x",
            &ctx(&["P   x"]),
            verified_report(),
        ));
        assert_eq!(cache.len(), 1, "re-inserting the same goal does not grow the cache");
    }

    #[test]
    fn key_is_a_stable_constant_no_wall_clock() {
        // Determinism guard (mirrors proof_import's stability test): the key is a
        // fixed function of its inputs — recomputing yields the identical digest.
        let k1 = cache_key(FormalSystem::Lean, "P x ⊢ Q x", &ctx(&["P x"]));
        let k2 = cache_key(FormalSystem::Lean, "P x ⊢ Q x", &ctx(&["P x"]));
        assert_eq!(k1, k2);
        assert_eq!(k1.len(), 64, "SHA-256 hex is 64 chars");
        assert!(k1.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
