//! Global persistent goal cache (AlphaProof "Nexus").
//!
//! Within a single search, the transposition table already dedups goals via
//! [`subsumption`]: two α-equivalent or hypothesis-reordered goals share a
//! canonical key, and a more-general proven goal subsumes a more-specific one.
//! But that table is per-search and in-memory — the moment a search ends, every
//! proven sub-goal is forgotten. [`GoalCache`] lifts that dedup to a durable,
//! [`Store`]-backed cache keyed by canonical goal, so a sub-goal proven in one
//! search (or one run, or one process) is reused by the next.
//!
//! Two lookup modes, both built on [`subsumption`]:
//! * [`GoalCache::get_exact`] — an exact canonical-key hit (α/reorder invariant);
//! * [`GoalCache::get_subsuming`] — a subsumption-aware hit: a cached goal that is
//!   *more general* than the query (its proof already proves the query).
//!
//! ## Soundness
//!
//! A cached hit is only ever returned when the CACHED goal subsumes the QUERY —
//! i.e. the cached goal is more general (weaker premises, same conclusion), so a
//! proof of it is a proof of the query. Never the reverse: a proof of a *more
//! specific* goal does NOT prove a more general query, and returning it would be
//! unsound. Because [`subsumption`] is a deliberately conservative, string-level
//! canonicalizer (see its module docs), it errs toward *false misses* — reporting
//! "no hit" when a semantic hit exists. That is the safe direction: a false miss
//! merely costs re-proving work; it never yields an unsound proof.

use crate::db::{GoalCacheEntry, Store};
use anyhow::Result;

use super::subsumption::{self, CanonicalGoal};

/// A [`Store`]-backed canonical-goal → proof cache shared across searches/runs.
pub struct GoalCache<'a> {
    store: &'a Store,
}

impl<'a> GoalCache<'a> {
    /// Wrap a store. All state lives in the `goal_cache` table; the cache holds
    /// only a borrow.
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// Canonical key for a goal — the shared contract with the search layer's
    /// transposition table (α/hypothesis-order invariant).
    fn key_of(goal: &str) -> String {
        CanonicalGoal::parse(goal).key()
    }

    /// Cache `proof` for `goal`, keyed by its canonical form. Idempotent on the
    /// canonical key: if an entry already exists for the same canonical goal
    /// (even a differently-spelled but α/reorder-equivalent restatement), this
    /// is a no-op and keeps the existing proof — so repeated puts do not
    /// duplicate rows.
    pub fn put(&self, project_id: &str, goal: &str, proof: &str) -> Result<()> {
        let key = Self::key_of(goal);
        if self.store.goal_cache_by_key(project_id, &key)?.is_some() {
            return Ok(());
        }
        self.store
            .add_goal_cache_entry(project_id, &key, goal, proof)?;
        Ok(())
    }

    /// Exact canonical-key hit: return the cached proof for a goal whose
    /// canonical form matches `goal` (invariant under α-renaming and hypothesis
    /// reordering). A miss returns `None`.
    pub fn get_exact(&self, project_id: &str, goal: &str) -> Result<Option<String>> {
        let key = Self::key_of(goal);
        Ok(self
            .store
            .goal_cache_by_key(project_id, &key)?
            .map(|e| e.proof))
    }

    /// Subsumption-aware hit: return a cached `(goal, proof)` whose CACHED goal
    /// subsumes `query` — i.e. the cached goal is more general, so its proof
    /// proves the query. Only ever returns a hit in that (sound) direction; a
    /// cached goal that is more *specific* than the query is not a hit.
    ///
    /// An exact hit trivially subsumes, so this is a strict superset of
    /// [`Self::get_exact`]; it is separated out because the scan is O(n) over the
    /// project's cached entries, whereas [`Self::get_exact`] is an indexed lookup.
    /// A miss returns `None`.
    pub fn get_subsuming(&self, project_id: &str, query: &str) -> Result<Option<(String, String)>> {
        for entry in self.store.goal_cache_entries(project_id)? {
            // SOUNDNESS: cached (general) must subsume query (specific), never the
            // reverse. A proof of a more-general goal proves the query.
            if subsumption::subsumes_str(&entry.goal, query) {
                return Ok(Some((entry.goal, entry.proof)));
            }
        }
        Ok(None)
    }

    /// Number of cached goals for a project.
    pub fn stats(&self, project_id: &str) -> Result<usize> {
        self.store.count_goal_cache(project_id)
    }

    /// All cached entries for a project (insertion order). Exposed for callers
    /// that want to inspect provenance beyond a single proof.
    pub fn entries(&self, project_id: &str) -> Result<Vec<GoalCacheEntry>> {
        self.store.goal_cache_entries(project_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn store() -> Store {
        Store::open(Path::new(":memory:")).unwrap()
    }

    #[test]
    fn put_then_get_exact_hits() {
        let s = store();
        let p = s.create_project("p", "t").unwrap();
        let cache = GoalCache::new(&s);
        cache.put(&p.id, "P x ⊢ Q x", "by simp").unwrap();
        assert_eq!(
            cache.get_exact(&p.id, "P x ⊢ Q x").unwrap().as_deref(),
            Some("by simp")
        );
    }

    #[test]
    fn alpha_renamed_and_reordered_restatement_hits_via_canonical_key() {
        let s = store();
        let p = s.create_project("p", "t").unwrap();
        let cache = GoalCache::new(&s);
        // Store under one spelling …
        cache.put(&p.id, "P x, Q y ⊢ ∀ a, R a", "proof").unwrap();
        // … query with hypotheses reordered and the bound variable renamed.
        assert_eq!(
            cache
                .get_exact(&p.id, "Q y, P x ⊢ ∀ b, R b")
                .unwrap()
                .as_deref(),
            Some("proof"),
            "an α-renamed / reordered restatement must hit via the canonical key"
        );
    }

    #[test]
    fn get_subsuming_returns_general_for_specific_but_not_vice_versa() {
        let s = store();
        let p = s.create_project("p", "t").unwrap();
        let cache = GoalCache::new(&s);
        // Cache the MORE GENERAL goal (proves R from just {P x}).
        cache.put(&p.id, "P x ⊢ R z", "general proof").unwrap();
        // A more-specific query (also assumes Q y) is subsumed by the cached
        // general goal: its proof proves the query.
        let hit = cache.get_subsuming(&p.id, "P x, Q y ⊢ R z").unwrap();
        assert_eq!(
            hit,
            Some(("P x ⊢ R z".to_string(), "general proof".to_string()))
        );

        // The reverse must NOT hit: cache only the more-specific goal …
        let p2 = s.create_project("p2", "t").unwrap();
        cache
            .put(&p2.id, "P x, Q y ⊢ R z", "specific proof")
            .unwrap();
        // … a more-general query is NOT proved by the specific cached proof.
        assert_eq!(
            cache.get_subsuming(&p2.id, "P x ⊢ R z").unwrap(),
            None,
            "a more-specific cached proof must NOT satisfy a more-general query"
        );
    }

    #[test]
    fn miss_returns_none() {
        let s = store();
        let p = s.create_project("p", "t").unwrap();
        let cache = GoalCache::new(&s);
        cache.put(&p.id, "P x ⊢ Q x", "pf").unwrap();
        assert_eq!(cache.get_exact(&p.id, "A ⊢ B").unwrap(), None);
        assert_eq!(cache.get_subsuming(&p.id, "A ⊢ B").unwrap(), None);
    }

    #[test]
    fn idempotent_put_on_same_key_does_not_duplicate() {
        let s = store();
        let p = s.create_project("p", "t").unwrap();
        let cache = GoalCache::new(&s);
        cache.put(&p.id, "P x, Q y ⊢ R", "first").unwrap();
        // Same canonical goal, different spelling (reordered) + a would-be new
        // proof: must be a no-op, keeping the first proof and one row.
        cache.put(&p.id, "Q y, P x ⊢ R", "second").unwrap();
        assert_eq!(cache.stats(&p.id).unwrap(), 1);
        assert_eq!(
            cache.get_exact(&p.id, "P x, Q y ⊢ R").unwrap().as_deref(),
            Some("first"),
            "idempotent put keeps the original proof"
        );
    }
}
