//! GraphRAG retrieval over the lemma/proof dependency DAG (the structural
//! complement to lexical/dense premise retrieval).
//!
//! We own the proof/lemma dependency graph (`graph::model` nodes + typed edges),
//! yet the existing retrieval cascade only ranks premises by *lexical* token
//! overlap and *dense* embedding cosine (see [`crate::library::LemmaLibrary::retrieve`]).
//! Neither uses the one signal the graph makes free: **structure**. A lemma that
//! sits one dependency hop from the goal — a premise the goal already leans on, or
//! a sibling premise co-used by the goal's neighbours — is far likelier to be
//! relevant than a lexically-similar lemma on the other side of the library. This
//! module retrieves *by graph proximity*, so it can be blended into the cascade as
//! a third, orthogonal retriever.
//!
//! ## The view
//!
//! [`GraphView`] is a directed dependency DAG over opaque string ids: an edge
//! `dependent → dependency` means "to state/prove `dependent` you need
//! `dependency`" (the premise relation). It stores both a forward map
//! (`node → its direct dependencies`, i.e. its premises) and the reverse
//! (`node → its direct dependents`, the lemmas that build on it). It is built
//! either from [`crate::model::Edge`]s (via [`GraphView::from_model_edges`], which
//! reads `DependsOn` / `DerivedFrom` as premise edges) or from raw
//! `(dependent, dependency)` pairs (via [`GraphView::from_pairs`]) so it is
//! testable without constructing full graph records.
//!
//! ## Terminology (fixed, because "ancestor" is ambiguous for a dependency DAG)
//!
//! * **dependencies / ancestors** — what a node is *built upon*: follow premise
//!   edges forward (`node → dependency`). These are exactly the lemmas you would
//!   want to retrieve to prove the node. [`GraphView::ancestors`] is the bounded
//!   transitive closure of [`GraphView::dependencies`].
//! * **dependents / descendants** — what *builds upon* a node: follow premise
//!   edges backward. [`GraphView::descendants`] is the bounded transitive closure
//!   of [`GraphView::dependents`].
//!
//! ## Determinism & cycle-safety
//!
//! Every adjacency list is stored sorted & de-duplicated in a [`BTreeMap`], every
//! traversal is a breadth-first walk guarded by a `visited` [`BTreeSet`] and a
//! depth/frontier bound ([`MAX_TRAVERSAL_DEPTH`]), and every ranked result breaks
//! score ties on the lexicographic id. A cyclic input therefore terminates (the
//! `visited` set caps work at one visit per node) and the output is byte-identical
//! across runs. There is no wall-clock and no unseeded randomness. All ids are
//! untrusted data: they are only ever compared and returned, never executed.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Hard bound on transitive-closure depth, so [`GraphView::ancestors`] /
/// [`GraphView::descendants`] terminate on any input — including a cyclic one —
/// and never wander further than a plausible proof spine.
pub const MAX_TRAVERSAL_DEPTH: usize = 64;

/// Weight on the proximity term in [`GraphView::graph_rank`] (shorter dependency
/// distance ⇒ higher score). Kept dominant over co-usage so a direct dependency
/// always outranks a merely well-co-used distant lemma.
pub const PROXIMITY_WEIGHT: f64 = 1.0;

/// Weight on the co-usage term in [`GraphView::graph_rank`]: a candidate that is
/// frequently used *together with* the goal's neighbourhood is nudged up. Small
/// relative to [`PROXIMITY_WEIGHT`] so it breaks ties and reranks within a
/// distance band rather than overturning the distance ordering.
pub const CO_USAGE_WEIGHT: f64 = 0.15;

/// A directed dependency DAG over lemma/proof ids. See the module docs for the
/// premise-edge orientation and the fixed ancestor/descendant terminology.
#[derive(Debug, Clone, Default)]
pub struct GraphView {
    /// `node → its direct dependencies` (the premises it is built upon). Sorted,
    /// de-duplicated.
    deps: BTreeMap<String, Vec<String>>,
    /// `node → its direct dependents` (the lemmas built upon it). Sorted,
    /// de-duplicated. The transpose of `deps`.
    dependents: BTreeMap<String, Vec<String>>,
    /// Every id that appears as either endpoint of any edge.
    nodes: BTreeSet<String>,
}

impl GraphView {
    /// An empty view.
    pub fn new() -> GraphView {
        GraphView::default()
    }

    /// Record that `dependent` depends on `dependency` (a premise edge
    /// `dependent → dependency`). Self-loops are ignored (a lemma is not its own
    /// premise); repeated edges collapse. Deterministic: adjacency stays sorted.
    pub fn add_dependency(&mut self, dependent: &str, dependency: &str) {
        self.nodes.insert(dependent.to_owned());
        self.nodes.insert(dependency.to_owned());
        if dependent == dependency {
            return;
        }
        insert_sorted(self.deps.entry(dependent.to_owned()).or_default(), dependency);
        insert_sorted(
            self.dependents.entry(dependency.to_owned()).or_default(),
            dependent,
        );
    }

    /// Build a view from raw `(dependent, dependency)` premise pairs.
    pub fn from_pairs<I, A, B>(pairs: I) -> GraphView
    where
        I: IntoIterator<Item = (A, B)>,
        A: AsRef<str>,
        B: AsRef<str>,
    {
        let mut g = GraphView::new();
        for (dependent, dependency) in pairs {
            g.add_dependency(dependent.as_ref(), dependency.as_ref());
        }
        g
    }

    /// Build a view from [`crate::model::Edge`]s, reading the premise relation off
    /// the edge kinds: `DependsOn` and `DerivedFrom` both mean "the source is
    /// built upon the target", so they become premise edges `source → target`.
    /// Every other [`crate::model::EdgeKind`] (support / contradiction /
    /// formalization / verification / supersession) is *not* a premise dependency
    /// and is skipped — callers that want those relations use [`from_pairs`].
    pub fn from_model_edges(edges: &[crate::model::Edge]) -> GraphView {
        use crate::model::EdgeKind;
        let mut g = GraphView::new();
        for e in edges {
            if matches!(e.kind, EdgeKind::DependsOn | EdgeKind::DerivedFrom) {
                g.add_dependency(&e.source_id, &e.target_id);
            }
        }
        g
    }

    /// Every id known to the view (sorted).
    pub fn nodes(&self) -> Vec<String> {
        self.nodes.iter().cloned().collect()
    }

    /// The number of distinct ids in the view.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the view holds no ids.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// `node`'s direct dependencies (premises it is built upon), sorted.
    pub fn dependencies(&self, node: &str) -> &[String] {
        self.deps.get(node).map(Vec::as_slice).unwrap_or(&[])
    }

    /// `node`'s direct dependents (lemmas built upon it), sorted.
    pub fn dependents(&self, node: &str) -> &[String] {
        self.dependents.get(node).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Up to `k` immediate neighbours of `node` in *either* direction —
    /// dependencies ∪ dependents — sorted and de-duplicated, then truncated to
    /// `k`. Excludes `node` itself. This is the 1-hop structural neighbourhood a
    /// retriever seeds from.
    pub fn neighbors(&self, node: &str, k: usize) -> Vec<String> {
        let mut out: BTreeSet<String> = BTreeSet::new();
        for d in self.dependencies(node) {
            out.insert(d.clone());
        }
        for d in self.dependents(node) {
            out.insert(d.clone());
        }
        out.remove(node);
        out.into_iter().take(k).collect()
    }

    /// The bounded transitive closure of [`dependencies`](Self::dependencies): all
    /// premises `node` is (directly or indirectly) built upon, up to
    /// [`MAX_TRAVERSAL_DEPTH`] hops. Cycle-safe and `node`-excluding.
    pub fn ancestors(&self, node: &str) -> Vec<String> {
        self.closure(node, MAX_TRAVERSAL_DEPTH, Direction::Dependencies)
    }

    /// The bounded transitive closure of [`dependents`](Self::dependents): all
    /// lemmas that (directly or indirectly) build upon `node`, up to
    /// [`MAX_TRAVERSAL_DEPTH`] hops. Cycle-safe and `node`-excluding.
    pub fn descendants(&self, node: &str) -> Vec<String> {
        self.closure(node, MAX_TRAVERSAL_DEPTH, Direction::Dependents)
    }

    /// Every node reachable from `node` within `k` *undirected* hops (following
    /// premise edges either way), excluding `node`. Sorted, cycle-safe.
    pub fn k_hop(&self, node: &str, k: usize) -> Vec<String> {
        self.closure(node, k, Direction::Undirected)
    }

    /// Bounded breadth-first closure in the requested direction. The `visited`
    /// set caps each node at one expansion, so a cyclic graph terminates; `depth`
    /// bounds the radius. The seed `node` is marked visited but excluded from the
    /// result. Returns a sorted id list (deterministic).
    fn closure(&self, node: &str, depth: usize, dir: Direction) -> Vec<String> {
        let mut visited: BTreeSet<String> = BTreeSet::new();
        visited.insert(node.to_owned());
        let mut frontier: VecDeque<(String, usize)> = VecDeque::new();
        frontier.push_back((node.to_owned(), 0));
        let mut found: BTreeSet<String> = BTreeSet::new();
        while let Some((cur, d)) = frontier.pop_front() {
            if d >= depth {
                continue;
            }
            for next in self.step(&cur, dir) {
                if visited.insert(next.clone()) {
                    found.insert(next.clone());
                    frontier.push_back((next, d + 1));
                }
            }
        }
        found.into_iter().collect()
    }

    /// The out-neighbours of `cur` in a given traversal direction (already sorted,
    /// courtesy of the adjacency maps).
    fn step(&self, cur: &str, dir: Direction) -> Vec<String> {
        match dir {
            Direction::Dependencies => self.dependencies(cur).to_vec(),
            Direction::Dependents => self.dependents(cur).to_vec(),
            Direction::Undirected => {
                let mut v: BTreeSet<String> = BTreeSet::new();
                for x in self.dependencies(cur) {
                    v.insert(x.clone());
                }
                for x in self.dependents(cur) {
                    v.insert(x.clone());
                }
                v.into_iter().collect()
            }
        }
    }

    /// Undirected shortest-path hop distances from `node` to every node reachable
    /// within [`MAX_TRAVERSAL_DEPTH`] hops (BFS gives the minimum). `node` maps to
    /// `0`. Cycle-safe (first visit wins, one visit per node).
    fn distances_from(&self, node: &str) -> BTreeMap<String, usize> {
        let mut dist: BTreeMap<String, usize> = BTreeMap::new();
        dist.insert(node.to_owned(), 0);
        let mut frontier: VecDeque<String> = VecDeque::new();
        frontier.push_back(node.to_owned());
        while let Some(cur) = frontier.pop_front() {
            let d = dist[&cur];
            if d >= MAX_TRAVERSAL_DEPTH {
                continue;
            }
            for next in self.step(&cur, Direction::Undirected) {
                if !dist.contains_key(&next) {
                    dist.insert(next.clone(), d + 1);
                    frontier.push_back(next);
                }
            }
        }
        dist
    }

    /// Co-usage weight of `candidate` w.r.t. `goal`: how embedded `candidate` is in
    /// the goal's immediate neighbourhood — i.e. how many members of
    /// `{goal} ∪ direct-neighbours(goal)` are themselves *directly adjacent* to
    /// `candidate` (name it as a premise or build upon it). A lemma co-required by
    /// many of the goal's neighbours — a widely-shared sibling premise — scores
    /// higher than one only a single neighbour touches. `candidate == goal`
    /// contributes nothing, and a neighbour never counts itself.
    fn co_usage(&self, goal: &str, candidate: &str) -> usize {
        if goal == candidate {
            return 0;
        }
        // Reference set: the goal plus its immediate neighbours (both directions).
        let mut refs: BTreeSet<String> = BTreeSet::new();
        refs.insert(goal.to_owned());
        for n in self.neighbors(goal, usize::MAX) {
            refs.insert(n);
        }
        let mut score = 0usize;
        for r in &refs {
            if r == candidate {
                continue;
            }
            let adjacent = self.dependencies(r).iter().any(|x| x == candidate)
                || self.dependents(r).iter().any(|x| x == candidate);
            if adjacent {
                score += 1;
            }
        }
        score
    }

    /// Structural relevance of `candidate` to `goal`: proximity (closer in the
    /// dependency graph ⇒ higher, via `1/(1+distance)`) plus a small co-usage
    /// boost. Unreachable candidates get proximity `0`. See [`PROXIMITY_WEIGHT`]
    /// and [`CO_USAGE_WEIGHT`].
    pub fn relevance(&self, goal: &str, candidate: &str, distances: &BTreeMap<String, usize>) -> f64 {
        let proximity = match distances.get(candidate) {
            Some(&d) if candidate != goal => 1.0 / (1.0 + d as f64),
            _ => 0.0,
        };
        let co_usage = self.co_usage(goal, candidate) as f64;
        PROXIMITY_WEIGHT * proximity + CO_USAGE_WEIGHT * co_usage
    }

    /// Score `candidates` by structural relevance to `goal` and return them
    /// best-first as `(id, score)`. Distances are computed once. Deterministic:
    /// higher score first, lexicographic id breaking ties. Duplicate candidate ids
    /// are collapsed (first occurrence wins).
    pub fn graph_rank<S: AsRef<str>>(&self, goal: &str, candidates: &[S]) -> Vec<(String, f64)> {
        let distances = self.distances_from(goal);
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut scored: Vec<(String, f64)> = Vec::new();
        for c in candidates {
            let id = c.as_ref().to_owned();
            if !seen.insert(id.clone()) {
                continue;
            }
            let score = self.relevance(goal, &id, &distances);
            scored.push((id, score));
        }
        sort_scored(&mut scored);
        scored
    }

    /// Retrieve the top `budget` graph-relevant premises for `goal`: take every
    /// node within [`MAX_TRAVERSAL_DEPTH`] undirected hops as the candidate pool,
    /// rank it by [`graph_rank`](Self::graph_rank), and return the best `budget`
    /// as `(id, score)`. This is the structural retriever the cascade blends
    /// alongside lexical + dense retrieval. Deterministic; empty when the goal has
    /// no neighbourhood.
    pub fn graph_retrieve(&self, goal: &str, budget: usize) -> Vec<(String, f64)> {
        let distances = self.distances_from(goal);
        let mut scored: Vec<(String, f64)> = distances
            .keys()
            .filter(|id| id.as_str() != goal)
            .map(|id| (id.clone(), self.relevance(goal, id, &distances)))
            .collect();
        sort_scored(&mut scored);
        scored.truncate(budget);
        scored
    }
}

/// The direction a closure/traversal follows premise edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    /// Forward: toward premises (dependencies / ancestors).
    Dependencies,
    /// Backward: toward consumers (dependents / descendants).
    Dependents,
    /// Both: reachability regardless of edge orientation.
    Undirected,
}

/// Insert `value` into a sorted vector, keeping it sorted and de-duplicated.
fn insert_sorted(vec: &mut Vec<String>, value: &str) {
    if let Err(pos) = vec.binary_search_by(|probe| probe.as_str().cmp(value)) {
        vec.insert(pos, value.to_owned());
    }
}

/// Sort `(id, score)` pairs best-first: descending score, then ascending id as a
/// stable, deterministic tie-break (NaN-free by construction — scores are finite).
fn sort_scored(scored: &mut [(String, f64)]) {
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small mock lemma DAG (premise edges `dependent → dependency`):
    ///
    /// ```text
    ///   goal ── depends on ──▶ premise_a ── depends on ──▶ base
    ///     │                        ▲
    ///     └── depends on ──▶ premise_b ─┘   (both a and b lean on base)
    ///                                       distant ── depends on ──▶ base
    /// ```
    /// So: goal's direct premises are {premise_a, premise_b}; both transitively
    /// reach `base`; `distant` also uses `base` but is 2 hops from `goal`.
    fn mock_dag() -> GraphView {
        GraphView::from_pairs([
            ("goal", "premise_a"),
            ("goal", "premise_b"),
            ("premise_a", "base"),
            ("premise_b", "base"),
            ("distant", "base"),
        ])
    }

    #[test]
    fn direct_dependencies_and_dependents_are_correct() {
        let g = mock_dag();
        assert_eq!(g.dependencies("goal"), &["premise_a", "premise_b"]);
        assert_eq!(g.dependencies("premise_a"), &["base"]);
        // `base` is depended upon by three lemmas (sorted).
        assert_eq!(g.dependents("base"), &["distant", "premise_a", "premise_b"]);
        // A leaf premise has no further dependencies.
        assert!(g.dependencies("base").is_empty());
    }

    #[test]
    fn neighbors_unions_both_directions_and_caps_at_k() {
        let g = mock_dag();
        // goal's 1-hop neighbours: its two premises (goal has no dependents).
        assert_eq!(g.neighbors("goal", 10), vec!["premise_a", "premise_b"]);
        // base's neighbours: its three dependents (base has no dependencies).
        assert_eq!(
            g.neighbors("base", 10),
            vec!["distant", "premise_a", "premise_b"]
        );
        // k caps the count deterministically (sorted order ⇒ first two).
        assert_eq!(g.neighbors("base", 2), vec!["distant", "premise_a"]);
    }

    #[test]
    fn ancestors_and_descendants_are_transitive_and_bounded() {
        let g = mock_dag();
        // goal is built upon a, b, and (transitively) base.
        assert_eq!(g.ancestors("goal"), vec!["base", "premise_a", "premise_b"]);
        // base is built upon by everything that transitively uses it.
        assert_eq!(
            g.descendants("base"),
            vec!["distant", "goal", "premise_a", "premise_b"]
        );
        // A leaf has no ancestors; the root goal has no descendants.
        assert!(g.ancestors("base").is_empty());
        assert!(g.descendants("goal").is_empty());
    }

    #[test]
    fn k_hop_respects_the_radius() {
        let g = mock_dag();
        // 1 undirected hop from goal: just its two premises.
        assert_eq!(g.k_hop("goal", 1), vec!["premise_a", "premise_b"]);
        // 2 hops also reaches `base` (goal→premise→base).
        assert_eq!(g.k_hop("goal", 2), vec!["base", "premise_a", "premise_b"]);
        // 3 hops additionally reaches `distant` (goal→premise→base→distant).
        assert_eq!(
            g.k_hop("goal", 3),
            vec!["base", "distant", "premise_a", "premise_b"]
        );
    }

    #[test]
    fn graph_rank_ranks_a_direct_dependency_above_a_distant_one() {
        let g = mock_dag();
        // premise_a is a DIRECT dependency (dist 1); `distant` is 3 undirected hops
        // away (goal→premise→base→distant). Proximity must put premise_a first.
        let ranked = g.graph_rank("goal", &["distant", "premise_a"]);
        assert_eq!(ranked[0].0, "premise_a");
        assert_eq!(ranked[1].0, "distant");
        assert!(
            ranked[0].1 > ranked[1].1,
            "direct dependency must outscore the distant lemma: {ranked:?}"
        );
    }

    #[test]
    fn co_usage_boosts_a_frequently_co_used_lemma() {
        // Two candidates at the SAME undirected distance from goal, so proximity is
        // a tie; co-usage must break it. `shared` is a premise of BOTH of the
        // goal's neighbours (p1 and p2), so it is co-used across the goal's
        // neighbourhood; `lonely` is a premise of p1 only. Both sit 2 hops from
        // the goal (goal→p→{shared|lonely}).
        //
        //   goal → p1 ; goal → p2
        //   p1 → shared ; p2 → shared         (shared co-used with BOTH neighbours)
        //   p1 → lonely                        (lonely used once, by p1 only)
        let g = GraphView::from_pairs([
            ("goal", "p1"),
            ("goal", "p2"),
            ("p1", "shared"),
            ("p2", "shared"),
            ("p1", "lonely"),
        ]);
        let d = g.distances_from("goal");
        // Same distance (both 2 hops: goal→p→{shared|lonely}).
        assert_eq!(d.get("shared"), Some(&2));
        assert_eq!(d.get("lonely"), Some(&2));
        let ranked = g.graph_rank("goal", &["lonely", "shared"]);
        assert_eq!(
            ranked[0].0, "shared",
            "the co-used lemma must rank first: {ranked:?}"
        );
        assert!(ranked[0].1 > ranked[1].1);
    }

    #[test]
    fn graph_retrieve_returns_top_budget_premises() {
        let g = mock_dag();
        let hits = g.graph_retrieve("goal", 2);
        assert_eq!(hits.len(), 2);
        // The two closest (direct) premises come first, and the goal is excluded.
        let ids: Vec<&str> = hits.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["premise_a", "premise_b"]);
        assert!(hits.iter().all(|(id, _)| id != "goal"));
        // Budget larger than the pool returns the whole neighbourhood (a,b,base,distant).
        assert_eq!(g.graph_retrieve("goal", 100).len(), 4);
    }

    #[test]
    fn cycle_safe_bounded_no_infinite_loop() {
        // A 3-cycle a→b→c→a plus a tail into d. Every traversal must terminate.
        let g = GraphView::from_pairs([("a", "b"), ("b", "c"), ("c", "a"), ("c", "d")]);
        // Transitive closures terminate and cover the cycle + tail.
        assert_eq!(g.ancestors("a"), vec!["b", "c", "d"]);
        assert_eq!(g.descendants("a"), vec!["b", "c"]);
        // Undirected reach from `a` visits the whole component, once each.
        assert_eq!(g.k_hop("a", MAX_TRAVERSAL_DEPTH), vec!["b", "c", "d"]);
        // Ranking over a cyclic graph is finite and well-formed.
        let ranked = g.graph_rank("a", &["b", "c", "d"]);
        assert_eq!(ranked.len(), 3);
        // graph_retrieve also terminates.
        assert_eq!(g.graph_retrieve("a", 10).len(), 3);
    }

    #[test]
    fn deterministic_across_runs() {
        let g = mock_dag();
        let a = g.graph_retrieve("goal", 10);
        let b = g.graph_retrieve("goal", 10);
        assert_eq!(a, b, "graph_retrieve must be byte-identical across runs");
        let r1 = g.graph_rank("goal", &["distant", "base", "premise_b", "premise_a"]);
        let r2 = g.graph_rank("goal", &["premise_a", "distant", "premise_b", "base"]);
        // Ranking is a pure function of the graph + candidate set, not input order.
        assert_eq!(r1, r2, "graph_rank must not depend on candidate input order");
    }

    #[test]
    fn self_loops_and_duplicate_edges_are_ignored() {
        let mut g = GraphView::new();
        g.add_dependency("x", "x"); // self-loop dropped
        g.add_dependency("x", "y");
        g.add_dependency("x", "y"); // duplicate collapses
        assert_eq!(g.dependencies("x"), &["y"]);
        assert!(g.dependencies("x").iter().all(|d| d != "x"));
        // `x` still registered as a node despite the dropped self-loop.
        assert!(g.nodes().contains(&"x".to_string()));
    }

    #[test]
    fn from_model_edges_reads_only_premise_kinds() {
        use crate::model::{Edge, EdgeKind, EdgeStrength, DepScope};
        use chrono::Utc;
        let mk = |src: &str, tgt: &str, kind: EdgeKind| Edge {
            id: 0,
            project_id: "p".to_owned(),
            source_id: src.to_owned(),
            target_id: tgt.to_owned(),
            kind,
            evidence_strength: EdgeStrength::ProseProof,
            dep_scope: DepScope::Statement,
            created_at: Utc::now(),
        };
        let edges = vec![
            mk("goal", "lemma1", EdgeKind::DependsOn),
            mk("goal", "lemma2", EdgeKind::DerivedFrom),
            // Non-premise kinds must be ignored.
            mk("goal", "note", EdgeKind::Supports),
            mk("goal", "cex", EdgeKind::Contradicts),
        ];
        let g = GraphView::from_model_edges(&edges);
        assert_eq!(g.dependencies("goal"), &["lemma1", "lemma2"]);
        assert!(!g.nodes().contains(&"note".to_string()));
    }
}
