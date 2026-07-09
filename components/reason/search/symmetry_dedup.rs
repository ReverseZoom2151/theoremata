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

use std::collections::BTreeSet;

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
        assert!(!dedup.insert((1, 2)), "symmetric equivalent already present");
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
        assert_eq!(forward.len(), 3, "three distinct orbits: (1,2), (4,5), (0,9)");
    }

    #[test]
    fn empty_group_degrades_to_exact_equality() {
        // No generators: every state is its own orbit, so dedup is plain equality.
        let mut dedup: OrbitDedup<(i32, i32)> = OrbitDedup::new(SymmetryGroup::new());
        assert!(dedup.insert((2, 1)));
        assert!(!dedup.insert((2, 1)), "exact duplicate");
        assert!(dedup.insert((1, 2)), "swap is NOT merged without a generator");
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
}
