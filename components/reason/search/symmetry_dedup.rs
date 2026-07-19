//! Symmetry / orbit-based de-duplication for the search layer (the Rust companion
//! to the Python WLOG / invariance registry).
//!
//! Many search states are equivalent under a *symmetry group*: relabelling
//! interchangeable variables, permuting the coordinates of a tuple, reflecting or
//! rotating a small graph/configuration all yield states that are the *same* up to
//! symmetry. A naive dedup keys on the raw state, so the given-clause loop explores
//! — and the goal cache stores — every member of an orbit separately, multiplying
//! the search by the size of the group. [`OrbitDedup`] collapses each orbit to a
//! single [`CanonKey`], so a state symmetric to one already seen is recognised as a
//! duplicate and skipped.
//!
//! ## The abstraction
//!
//! A [`SymmetryGroup`] is a set of *generating* transformations over a
//! caller-defined key type `T` (any `Ord + Clone` — a tuple, a `Vec` of vertex
//! labels, a normalised term string, …). Each generator is a plain
//! `Fn(&T) -> T`: "swap these two coordinates", "reflect this configuration",
//! "relabel under this permutation". You supply generators, not the whole group;
//! [`SymmetryGroup::orbit`] closes them under composition (identity is always
//! implicit) to recover the full orbit of a state.
//!
//! [`canonical_key`] then picks the orbit's *lexicographically-minimal* member as
//! its canonical representative, wrapped in a [`CanonKey`]. Two states share a
//! [`CanonKey`] iff they lie in the same orbit, so the key is a sound dedup key:
//! equal keys ⇒ symmetric, distinct keys ⇒ genuinely different (relative to the
//! supplied group).
//!
//! ## Scope / soundness limits (read before trusting it)
//!
//! * The result is only as complete as the generators you supply. If the generator
//!   set does not generate the intended group, two states that *are* symmetric
//!   under the full group may get distinct keys — a *false miss* (redundant work),
//!   never a false merge. This matches the conservative bias of [`subsumption`].
//! * The orbit is materialised by breadth-first closure. For a genuinely finite
//!   group this terminates at the orbit size; a mis-specified *non-finite*
//!   generator (e.g. `n -> n + 1`) would enumerate forever, so closure is capped at
//!   [`MAX_ORBIT`]. On hitting the cap it stops expanding and canonicalises over the
//!   states seen so far — still deterministic, but no longer a guaranteed orbit
//!   minimum, so keep generators finite (permutations, reflections, rotations).
//! * Canonicalisation is purely structural over `T`'s [`Ord`]: it knows nothing of
//!   the state's meaning beyond the transformations and ordering you give it.

use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Safety cap on the number of distinct states enumerated while closing an orbit,
/// so a mis-specified non-finite generator cannot loop forever. Finite symmetry
/// groups (permutations, reflections, rotations of small configurations) stay far
/// below this.
pub const MAX_ORBIT: usize = 100_000;

/// A single canonicalizing transformation of a state: a permutation of
/// interchangeable coordinates, a reflection/rotation, a relabelling, …
type Transform<T> = Box<dyn Fn(&T) -> T>;

/// A symmetry group over states of type `T`, given by a set of *generating*
/// transformations. The identity is always implicit, and [`orbit`](Self::orbit)
/// closes the generators under composition, so callers need only supply generators
/// (e.g. adjacent-coordinate swaps generate the whole symmetric group).
#[derive(Default)]
pub struct SymmetryGroup<T> {
    generators: Vec<Transform<T>>,
}

impl<T: Ord + Clone> SymmetryGroup<T> {
    /// An empty group: only the identity, so every state is its own orbit and
    /// [`canonical_key`] is the identity map (dedup degrades to exact equality).
    pub fn new() -> Self {
        Self {
            generators: Vec::new(),
        }
    }

    /// Add a generating transformation, returning `self` for builder-style chaining.
    pub fn with(mut self, generator: impl Fn(&T) -> T + 'static) -> Self {
        self.generators.push(Box::new(generator));
        self
    }

    /// Add a generating transformation in place.
    pub fn add(&mut self, generator: impl Fn(&T) -> T + 'static) {
        self.generators.push(Box::new(generator));
    }

    /// Number of generating transformations (excluding the implicit identity).
    pub fn generator_count(&self) -> usize {
        self.generators.len()
    }

    /// The orbit of `item`: every state reachable from it by repeatedly applying
    /// the generators, including `item` itself. Returned as a sorted, de-duplicated
    /// [`BTreeSet`], so iteration is deterministic and the first element is the
    /// orbit minimum. Closure stops at [`MAX_ORBIT`] states (see the module docs).
    pub fn orbit(&self, item: &T) -> BTreeSet<T> {
        let mut seen = BTreeSet::new();
        seen.insert(item.clone());
        // BFS/DFS frontier — order is irrelevant since we accumulate a set and take
        // its minimum, making the result independent of generator/exploration order.
        let mut frontier = vec![item.clone()];
        while let Some(state) = frontier.pop() {
            for gen in &self.generators {
                let next = gen(&state);
                if seen.len() >= MAX_ORBIT {
                    // Defensive cap: a non-finite generator would never converge.
                    return seen;
                }
                if seen.insert(next.clone()) {
                    frontier.push(next);
                }
            }
        }
        seen
    }

    /// The canonical representative of `item`'s orbit: the lexicographically-minimal
    /// state reachable under the group. Two states in the same orbit return the same
    /// representative. An orbit is never empty (it always contains `item`), so this
    /// never panics.
    pub fn canonical_representative(&self, item: &T) -> T {
        // `orbit` always contains at least `item`, so `next()` is `Some`.
        self.orbit(item)
            .into_iter()
            .next()
            .expect("orbit always contains the item itself")
    }
}

/// The canonical key of a state's orbit under a [`SymmetryGroup`]: the orbit's
/// lexicographically-minimal member. Two states share a `CanonKey` iff they are
/// symmetric under the group, so it is the dedup key used by [`OrbitDedup`].
///
/// Ordering/equality/hashing are those of the wrapped representative, so a
/// `CanonKey` is a drop-in key for a `BTreeSet`/`HashSet` or a transposition table.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CanonKey<T>(pub T);

impl<T> CanonKey<T> {
    /// Borrow the underlying canonical representative.
    pub fn get(&self) -> &T {
        &self.0
    }

    /// Unwrap into the underlying canonical representative.
    pub fn into_inner(self) -> T {
        self.0
    }
}

/// Compute the canonical key of `item`'s orbit under `group` — the
/// lexicographically-minimal transform, so two symmetric items map to the same key.
///
/// Idempotent: `canonical_key(canonical_key(x).get(), g) == canonical_key(x, g)`,
/// because a representative's orbit is the same orbit and hence has the same
/// minimum. Independent of the order generators were added.
pub fn canonical_key<T: Ord + Clone>(item: &T, group: &SymmetryGroup<T>) -> CanonKey<T> {
    CanonKey(group.canonical_representative(item))
}

/// A set that de-duplicates states up to symmetry: it stores one [`CanonKey`] per
/// orbit, so inserting any state symmetric to one already present is a no-op.
///
/// Plug it into the given-clause loop or goal cache to skip subgoals that are mere
/// relabelings/reflections of ones already enqueued. Iteration is deterministic
/// (sorted by canonical key).
pub struct OrbitDedup<T: Ord + Clone> {
    group: SymmetryGroup<T>,
    keys: BTreeSet<CanonKey<T>>,
}

impl<T: Ord + Clone> OrbitDedup<T> {
    /// A dedup set over the given symmetry group.
    pub fn new(group: SymmetryGroup<T>) -> Self {
        Self {
            group,
            keys: BTreeSet::new(),
        }
    }

    /// The canonical key `item` would occupy, without inserting it.
    pub fn key_of(&self, item: &T) -> CanonKey<T> {
        canonical_key(item, &self.group)
    }

    /// Insert `item`, returning `true` if it opened a genuinely new orbit and
    /// `false` if a symmetric equivalent was already present (in which case the set
    /// is unchanged). The stored representative is the canonical (minimal) member,
    /// so the set holds one entry per orbit regardless of which member is inserted.
    pub fn insert(&mut self, item: T) -> bool {
        let key = self.key_of(&item);
        self.keys.insert(key)
    }

    /// Whether a state symmetric to `item` (or `item` itself) is already present.
    pub fn contains(&self, item: &T) -> bool {
        self.keys.contains(&self.key_of(item))
    }

    /// Number of distinct orbits stored.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether no orbit has been inserted yet.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Iterate the stored canonical keys in deterministic (sorted) order.
    pub fn iter(&self) -> impl Iterator<Item = &CanonKey<T>> {
        self.keys.iter()
    }
}

/// How [`dedup_candidates`] decides two candidates are "the same".
///
/// Recorded verbatim in every [`DedupReport`] so a consumer of the JSON never has
/// to guess whether the merge was decided or asserted.
pub const EQUIVALENCE_KIND: &str =
    "orbit under caller-supplied generators; generators are asserted by the caller, not verified";

/// Why a candidate was set aside by [`dedup_candidates`].
pub const DROP_REASON: &str = "shares an orbit key with an earlier candidate";

/// One candidate that survived dedup, with the input position it came from.
#[derive(Debug, Clone)]
pub struct KeptCandidate<C> {
    /// Position in the input slice, so the original order can be rebuilt.
    pub index: usize,
    /// The candidate itself, untouched.
    pub candidate: C,
}

/// One candidate that was set aside, kept whole so a later stage can put it back.
#[derive(Debug, Clone)]
pub struct DroppedCandidate<C> {
    /// Position in the input slice.
    pub index: usize,
    /// The candidate itself. Dedup never destroys a candidate, it only moves it
    /// off the hot path, because the orbit equivalence is asserted rather than
    /// decided and the dropped member may be the one that would have verified.
    pub candidate: C,
    /// Index of the surviving candidate this one collapsed onto.
    pub kept_index: usize,
}

/// JSON-able record of a single drop, for the run log.
#[derive(Debug, Clone, Serialize)]
pub struct DroppedRecord {
    /// Position of the dropped candidate in the input.
    pub index: usize,
    /// Caller-supplied label of the dropped candidate.
    pub label: String,
    /// Position of the survivor it collapsed onto.
    pub kept_index: usize,
    /// Caller-supplied label of that survivor.
    pub kept_label: String,
    /// Debug rendering of the shared canonical (orbit-minimal) key.
    pub orbit_key: String,
    /// Always [`DROP_REASON`]; carried explicitly so the log is self-describing.
    pub reason: &'static str,
}

/// JSON-able summary of one dedup pass.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DedupReport {
    /// Candidates handed in.
    pub input_count: usize,
    /// Candidates still on the hot path.
    pub kept_count: usize,
    /// Candidates set aside (recoverable, see [`DedupOutcome::restore_all`]).
    pub dropped_count: usize,
    /// Size of the generator set the merge was computed against.
    pub generator_count: usize,
    /// See [`EQUIVALENCE_KIND`].
    pub equivalence: String,
    /// Always `false`: nothing here proves the supplied generators really are
    /// symmetries of the problem, so a merge is never a verified equivalence.
    pub equivalence_verified: bool,
    /// Every drop, so a later stage can recover a candidate without re-running.
    pub dropped: Vec<DroppedRecord>,
}

/// Result of [`dedup_candidates`]: the reduced set, the set-aside candidates, and
/// a JSON-able summary.
#[derive(Debug, Clone)]
pub struct DedupOutcome<C> {
    /// Survivors, in input order, one per orbit.
    pub kept: Vec<KeptCandidate<C>>,
    /// Set-aside candidates, in input order. Never discarded.
    pub dropped: Vec<DroppedCandidate<C>>,
    /// Summary suitable for the run log.
    pub report: DedupReport,
}

impl<C> DedupOutcome<C> {
    /// The survivors alone, in input order, for feeding the checker.
    pub fn into_kept(self) -> Vec<C> {
        self.kept.into_iter().map(|k| k.candidate).collect()
    }

    /// Undo the dedup: every input candidate back in its original order. The
    /// escape hatch for when the kept members all fail and a dropped one may
    /// still verify (which is possible whenever a generator was wrong).
    pub fn restore_all(self) -> Vec<C> {
        let mut slots: Vec<Option<C>> = (0..self.kept.len() + self.dropped.len())
            .map(|_| None)
            .collect();
        for k in self.kept {
            slots[k.index] = Some(k.candidate);
        }
        for d in self.dropped {
            slots[d.index] = Some(d.candidate);
        }
        slots.into_iter().flatten().collect()
    }
}

/// Collapse a candidate set to one member per orbit, before the expensive checker
/// call.
///
/// `key_of` projects a candidate onto the state type the `group` acts on, and
/// `label_of` names it for the log. The first candidate of each orbit (in input
/// order) survives; later members of that orbit are moved to
/// [`DedupOutcome::dropped`], never deleted.
///
/// ## Why nothing is discarded
///
/// The equivalence here is "same orbit under the generators you supplied". The
/// module can close those generators under composition and pick a canonical
/// representative exactly, but it has no way to check that a generator really is a
/// symmetry of the theorem at hand. If a generator is wrong (two variables that
/// are not in fact interchangeable, a reflection the statement is not invariant
/// under), two genuinely different candidates get the same key and the one that
/// would have verified can be the one merged away. A missing generator is the safe
/// direction: it only costs redundant work. So drops are recorded in full and are
/// reversible via [`DedupOutcome::restore_all`].
pub fn dedup_candidates<C, T>(
    candidates: Vec<C>,
    group: &SymmetryGroup<T>,
    key_of: impl Fn(&C) -> T,
    label_of: impl Fn(&C) -> String,
) -> DedupOutcome<C>
where
    T: Ord + Clone + std::fmt::Debug,
{
    let mut report = DedupReport {
        input_count: candidates.len(),
        generator_count: group.generator_count(),
        equivalence: EQUIVALENCE_KIND.to_string(),
        equivalence_verified: false,
        ..Default::default()
    };
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    // Canonical key -> (input index, label) of the orbit's first-seen survivor.
    let mut seen: BTreeMap<CanonKey<T>, (usize, String)> = BTreeMap::new();

    for (index, candidate) in candidates.into_iter().enumerate() {
        let key = canonical_key(&key_of(&candidate), group);
        let label = label_of(&candidate);
        match seen.get(&key) {
            Some((kept_index, kept_label)) => {
                report.dropped.push(DroppedRecord {
                    index,
                    label,
                    kept_index: *kept_index,
                    kept_label: kept_label.clone(),
                    orbit_key: format!("{:?}", key.get()),
                    reason: DROP_REASON,
                });
                dropped.push(DroppedCandidate {
                    index,
                    candidate,
                    kept_index: *kept_index,
                });
            }
            None => {
                seen.insert(key, (index, label));
                kept.push(KeptCandidate { index, candidate });
            }
        }
    }

    report.kept_count = kept.len();
    report.dropped_count = dropped.len();
    DedupOutcome {
        kept,
        dropped,
        report,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The symmetric group on a pair's two coordinates, generated by the swap.
    fn swap_pair() -> SymmetryGroup<(i32, i32)> {
        SymmetryGroup::new().with(|&(a, b)| (b, a))
    }

    /// Reflection of a length-`n` configuration (reverse the vector).
    fn reflection() -> SymmetryGroup<Vec<i32>> {
        SymmetryGroup::new().with(|v: &Vec<i32>| {
            let mut r = v.clone();
            r.reverse();
            r
        })
    }

    /// Cyclic rotation group, generated by a single left-rotation. A one-generator
    /// group whose orbits (all rotations) require genuine closure under composition.
    fn rotation() -> SymmetryGroup<Vec<i32>> {
        SymmetryGroup::new().with(|v: &Vec<i32>| {
            if v.is_empty() {
                return v.clone();
            }
            let mut r = v.clone();
            r.rotate_left(1);
            r
        })
    }

    #[test]
    fn symmetric_items_collapse_to_one_orbit_key() {
        let g = swap_pair();
        // (2, 1) and (1, 2) lie in the same orbit under coordinate swap.
        assert_eq!(canonical_key(&(2, 1), &g), canonical_key(&(1, 2), &g));
        // The canonical representative is the lexicographic minimum of the orbit.
        assert_eq!(canonical_key(&(2, 1), &g).into_inner(), (1, 2));
    }

    #[test]
    fn reflection_collapses_mirror_images() {
        let g = reflection();
        let a = canonical_key(&vec![1, 2, 3], &g);
        let b = canonical_key(&vec![3, 2, 1], &g);
        assert_eq!(a, b, "a config and its mirror image share an orbit key");
        assert_eq!(a.get(), &vec![1, 2, 3]);
    }

    #[test]
    fn single_generator_orbit_closes_under_composition() {
        // Rotations: [3,1,2] -> [1,2,3] -> [2,3,1] -> back. All three collapse to
        // the minimal rotation, which only appears after closing the generator.
        let g = rotation();
        let k = canonical_key(&vec![1, 2, 3], &g);
        assert_eq!(canonical_key(&vec![3, 1, 2], &g), k);
        assert_eq!(canonical_key(&vec![2, 3, 1], &g), k);
        assert_eq!(k.into_inner(), vec![1, 2, 3]);
        // The full orbit is exactly the three rotations.
        assert_eq!(g.orbit(&vec![3, 1, 2]).len(), 3);
    }

    #[test]
    fn non_symmetric_items_stay_distinct() {
        let g = swap_pair();
        // {1,2} and {1,3} are different multisets — not related by any swap.
        assert_ne!(canonical_key(&(1, 2), &g), canonical_key(&(1, 3), &g));

        // Under reflection, a palindrome-inequivalent config stays distinct.
        let r = reflection();
        assert_ne!(
            canonical_key(&vec![1, 2, 3], &r),
            canonical_key(&vec![1, 3, 2], &r)
        );
    }

    #[test]
    fn insert_returns_false_for_symmetric_duplicate_true_for_new() {
        let mut dedup = OrbitDedup::new(swap_pair());
        assert!(dedup.insert((2, 1)), "first orbit is genuinely new");
        // (1, 2) is the swap of (2, 1): a symmetric duplicate.
        assert!(
            !dedup.insert((1, 2)),
            "symmetric equivalent already present"
        );
        // A genuinely different orbit.
        assert!(dedup.insert((1, 3)), "new orbit inserts");
        assert_eq!(dedup.len(), 2, "exactly two orbits stored");
        assert!(dedup.contains(&(2, 1)));
        assert!(dedup.contains(&(1, 2)));
    }

    #[test]
    fn canonical_key_is_idempotent() {
        let g = rotation();
        let once = canonical_key(&vec![3, 1, 2], &g);
        let twice = canonical_key(once.get(), &g);
        assert_eq!(once, twice, "canonicalizing a representative is a no-op");
    }

    #[test]
    fn independent_of_insertion_order() {
        // Insert the members of two orbits in two different orders; the resulting
        // key sets must be identical and deterministic.
        let orbit_members = [(2, 1), (1, 2), (5, 4), (4, 5), (0, 9)];

        let mut forward = OrbitDedup::new(swap_pair());
        for m in orbit_members.iter() {
            forward.insert(*m);
        }

        let mut backward = OrbitDedup::new(swap_pair());
        for m in orbit_members.iter().rev() {
            backward.insert(*m);
        }

        let fk: Vec<_> = forward.iter().cloned().collect();
        let bk: Vec<_> = backward.iter().cloned().collect();
        assert_eq!(fk, bk, "stored key set is independent of insertion order");
        assert_eq!(
            forward.len(),
            3,
            "three distinct orbits: (1,2), (4,5), (0,9)"
        );
    }

    #[test]
    fn empty_group_degrades_to_exact_equality() {
        // No generators: every state is its own orbit, so dedup is plain equality.
        let mut dedup: OrbitDedup<(i32, i32)> = OrbitDedup::new(SymmetryGroup::new());
        assert!(dedup.insert((2, 1)));
        assert!(!dedup.insert((2, 1)), "exact duplicate");
        assert!(
            dedup.insert((1, 2)),
            "swap is NOT merged without a generator"
        );
        assert_eq!(dedup.len(), 2);
    }

    #[test]
    fn orbit_is_deterministic_and_order_independent() {
        // The orbit set is identical no matter which member we start from.
        let g = rotation();
        let from_a = g.orbit(&vec![1, 2, 3]);
        let from_b = g.orbit(&vec![2, 3, 1]);
        assert_eq!(from_a, from_b);
        // And its minimum (the canonical key) is stable.
        assert_eq!(from_a.iter().next(), Some(&vec![1, 2, 3]));
    }

    /// A pipeline candidate: a name plus the state the symmetry group acts on.
    type Cand = (&'static str, (i32, i32));

    fn run(candidates: Vec<Cand>) -> DedupOutcome<Cand> {
        dedup_candidates(
            candidates,
            &swap_pair(),
            |c: &Cand| c.1,
            |c: &Cand| c.0.to_string(),
        )
    }

    #[test]
    fn dedup_keeps_first_of_each_orbit_and_sets_aside_the_rest() {
        let out = run(vec![
            ("a", (2, 1)),
            ("b", (1, 2)), // swap of a
            ("c", (1, 3)), // different orbit
            ("d", (3, 1)), // swap of c
        ]);

        let kept: Vec<&str> = out.kept.iter().map(|k| k.candidate.0).collect();
        assert_eq!(kept, vec!["a", "c"], "first member of each orbit survives");
        assert_eq!(out.report.kept_count, 2);
        assert_eq!(out.report.dropped_count, 2);
        assert_eq!(out.report.input_count, 4);
        assert_eq!(out.report.generator_count, 1);
    }

    #[test]
    fn every_drop_is_recorded_with_the_survivor_it_merged_into() {
        let out = run(vec![("a", (2, 1)), ("b", (1, 2))]);

        assert_eq!(out.report.dropped.len(), 1);
        let rec = &out.report.dropped[0];
        assert_eq!(rec.label, "b");
        assert_eq!(rec.index, 1);
        assert_eq!(rec.kept_label, "a");
        assert_eq!(rec.kept_index, 0);
        assert_eq!(rec.reason, DROP_REASON);
        assert_eq!(rec.orbit_key, format!("{:?}", (1, 2)));
        // The dropped candidate itself is still in hand, not deleted.
        assert_eq!(out.dropped[0].candidate.0, "b");
        assert_eq!(out.dropped[0].kept_index, 0);
    }

    #[test]
    fn dedup_is_reversible_in_original_order() {
        let input = vec![("a", (2, 1)), ("b", (1, 2)), ("c", (1, 3))];
        let out = run(input.clone());
        assert_eq!(out.restore_all(), input, "no candidate is lost");
    }

    #[test]
    fn report_never_claims_a_verified_equivalence() {
        let out = run(vec![("a", (2, 1)), ("b", (1, 2))]);
        assert!(!out.report.equivalence_verified);
        assert_eq!(out.report.equivalence, EQUIVALENCE_KIND);
    }

    #[test]
    fn report_serializes_to_json() {
        let out = run(vec![("a", (2, 1)), ("b", (1, 2))]);
        let json = serde_json::to_value(&out.report).expect("report is JSON-able");
        assert_eq!(json["dropped_count"], 1);
        assert_eq!(json["dropped"][0]["kept_label"], "a");
        assert_eq!(json["equivalence_verified"], false);
    }

    #[test]
    fn empty_group_drops_only_exact_duplicates() {
        // Without generators the equivalence degrades to equality, which is the
        // one setting where a drop is not a guess.
        let out = dedup_candidates(
            vec![("a", (2, 1)), ("b", (2, 1)), ("c", (1, 2))],
            &SymmetryGroup::new(),
            |c: &Cand| c.1,
            |c: &Cand| c.0.to_string(),
        );
        let kept: Vec<&str> = out.kept.iter().map(|k| k.candidate.0).collect();
        assert_eq!(kept, vec!["a", "c"]);
    }

    #[test]
    fn empty_input_yields_an_empty_report() {
        let out = run(Vec::new());
        assert_eq!(out.report.input_count, 0);
        assert!(out.kept.is_empty());
        assert!(out.report.dropped.is_empty());
    }
}
