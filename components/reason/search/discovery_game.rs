//! Discovery-as-a-game vertical — the AlphaTensor framing
//! (plan `docs/paper-mining/deepmind-articles.md` §4, "algorithm discovery as a
//! game").
//!
//! AlphaTensor recasts a *discovery* problem (finding a faster matrix-multiplication
//! algorithm) as a **single-player game with a verifiable certificate**: moves
//! subtract rank-1 updates from a residual tensor, and *fully zeroing the residual*
//! is a provably-correct decomposition — a discovery. Fewer moves = lower "rank" =
//! a better (faster) algorithm. The zero-residual check is the **sound boundary**:
//! a discovery is only real once the certificate verifies, exactly Theoremata's
//! verifier-as-ground-truth thesis (see the cross-cutting themes in that doc).
//!
//! This module provides that framing as reusable machinery:
//! * [`DiscoveryGame`] — an injectable single-player game (`legal_moves` / `apply`
//!   / `certificate` / `cost`). A deterministic mock backs every test; a real
//!   domain (a TensorGame, a construction game, a gadget search) plugs into the
//!   same seam.
//! * [`Certificate`] — the discovered object (`moves`) plus its verification bit.
//!   `certificate()` returning `Some` is the *only* thing that counts as a
//!   discovery; the search NEVER reports an uncertified terminal.
//! * [`search_discovery`] — a bounded, seeded, deterministic best-first (A*-style)
//!   search that returns the **lowest-cost certified discovery** found, or none.
//!
//! Relationship to the MCGS driver ([`crate::search::driver`]): the discovery game
//! maps cleanly onto the driver's `GoalState`/`TacticExpander` seam
//! (`is_closed == certificate().is_some()`), and [`confirm_reachable_via_mcgs`]
//! reuses the real `ProofSearchDriver` to *independently confirm* a discovery is
//! reachable. But the driver stops at the **first** closed state and returns no
//! path or cost, whereas a discovery vertical must reconstruct the discovered
//! object and prefer the **lowest-cost** of several certified terminals — so the
//! primary [`search_discovery`] is a self-contained best-first search purpose-built
//! for that. There is **no** wall-clock or unseeded randomness: a `seed` is
//! threaded through and only breaks ties deterministically.

use super::driver::{GoalState, ProofSearchDriver, TacticExpander, TacticStep};
use super::mcts::SearchConfig;
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

/// A verified discovery: the sequence of `moves` that reaches a certified terminal,
/// its `cost` (fewer/cheaper moves = better, AlphaTensor's "rank"), and the
/// `verified` bit carried from the game's sound certificate check.
///
/// A `Certificate` only ever exists for a state the game certifies
/// ([`DiscoveryGame::certificate`] returned `Some`); it is the discovered object
/// together with the proof that it is real.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Certificate<M> {
    /// The moves that produced the discovery, from the root to the certified
    /// terminal. Filled in by [`search_discovery`] from the search path (the game
    /// certifies the terminal; the search knows how it got there).
    pub moves: Vec<M>,
    /// The discovery cost — lower is better. [`search_discovery`] reports the
    /// number of moves (the AlphaTensor "rank": fewer steps ⇒ faster algorithm).
    pub cost: f64,
    /// The verification bit from the game's certificate check. The sound boundary:
    /// the search only ever reports a discovery when this is `true`.
    pub verified: bool,
}

/// A single-player discovery game with a verifiable certificate (the AlphaTensor
/// framing). Injected — a deterministic mock backs the tests; a real domain game
/// implements the same trait.
///
/// The game is the **sound boundary**: reaching a state where
/// [`certificate`](DiscoveryGame::certificate) returns `Some` is the *definition*
/// of a discovery. Everything else is search bookkeeping.
pub trait DiscoveryGame {
    /// A position in the game (e.g. the current residual tensor).
    type State: Clone;
    /// A move / action (e.g. subtracting one rank-1 update).
    type Move: Clone;

    /// The moves legal from `state`. An empty result is a dead end (no discovery
    /// down this branch unless `state` is itself certified).
    fn legal_moves(&self, state: &Self::State) -> Vec<Self::Move>;

    /// The state that results from playing `mv` in `state`. Must be a pure
    /// function of `(state, mv)` — no randomness — so the search is reproducible.
    fn apply(&self, state: &Self::State, mv: &Self::Move) -> Self::State;

    /// The **sound boundary**: `Some(certificate)` iff `state` is a *certified
    /// terminal* — a real, verifiable discovery (e.g. the residual is fully zeroed).
    /// `None` for every non-terminal or unverified state. The returned
    /// certificate's `verified` bit is authoritative; its `moves`/`cost` are
    /// overwritten by [`search_discovery`] with the reconstructed path (the game
    /// cannot know how the search arrived).
    fn certificate(&self, state: &Self::State) -> Option<Certificate<Self::Move>>;

    /// A cost / heuristic estimate for `state` in `[0, ∞)` — AlphaTensor's "rank"
    /// intuition (`0` at a certified terminal, larger = further away). Used as an
    /// admissible search heuristic to steer toward cheaper discoveries; it does not
    /// affect soundness. Defaults to `0` (uninformed search).
    fn cost(&self, _state: &Self::State) -> f64 {
        0.0
    }

    /// A canonical transposition key: two states with equal keys are the *same*
    /// game position and are de-duplicated (the MCGS graph idea). A real game keys
    /// on a normalised encoding of the position; the mock keys on its debug form.
    fn dedup_key(&self, state: &Self::State) -> String;
}

/// Search budget for [`search_discovery`].
#[derive(Debug, Clone, Copy)]
pub struct DiscoveryConfig {
    /// Hard cap on the number of states expanded. Bounds the search; a discovery
    /// that needs more than this many expansions is reported as *not found* (never
    /// a false discovery).
    pub max_states: usize,
    /// Maximum move-depth from the root. Guards against unbounded deepening in
    /// games with cycles or infinite branches.
    pub max_depth: usize,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            max_states: 10_000,
            max_depth: 64,
        }
    }
}

/// The outcome of a discovery search.
#[derive(Debug, Clone, Serialize)]
pub struct DiscoveryResult<M> {
    /// The lowest-cost certified discovery found, or `None` if none was reached
    /// within budget. `Some` ⇒ a *real, verified* discovery — never a false one.
    pub certificate: Option<Certificate<M>>,
    /// The cost of `certificate`, surfaced for convenience (`None` if no
    /// discovery). Equals `certificate.as_ref().map(|c| c.cost)`.
    pub best_cost: Option<f64>,
    /// How many states the search expanded (bounded by `config.max_states`).
    pub states_explored: usize,
    /// `true` iff a certified discovery was found *and* its `verified` bit is set —
    /// the explicit "this is a real discovery" flag.
    pub certified: bool,
}

/// One arena node on the search path — used to reconstruct the move sequence.
struct PathNode<G: DiscoveryGame> {
    state: G::State,
    parent: Option<usize>,
    mv: Option<G::Move>,
    depth: usize,
}

/// A frontier entry ordered by A* priority `f = g + h` (lower is better). The
/// `BinaryHeap` is a max-heap, so [`Ord`] is written so the *smallest* `f` (then
/// smallest seeded tie-break) compares as *greatest* and pops first.
struct Frontier {
    f: f64,
    tie: u64,
    node: usize,
}

impl PartialEq for Frontier {
    fn eq(&self, other: &Self) -> bool {
        self.f == other.f && self.tie == other.tie
    }
}
impl Eq for Frontier {}
impl Ord for Frontier {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse on f (min-f first), then reverse on the seeded tie-break
        // (deterministic): whichever we want popped first must compare "greater".
        other
            .f
            .total_cmp(&self.f)
            .then_with(|| other.tie.cmp(&self.tie))
    }
}
impl PartialOrd for Frontier {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Deterministic per-state tie-break seed (FNV-1a of the dedup key, mixed with the
/// base seed). Same `(seed, key)` ⇒ same value, so equal-priority frontier entries
/// are ordered reproducibly and the whole search is a pure function of its inputs.
/// Mirrors the seed-mixing discipline in [`crate::search::driver`].
fn mix_seed(base: u64, key: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64 ^ base;
    for b in key.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Run a bounded, seeded, deterministic best-first search for the **lowest-cost
/// certified discovery** reachable from `root`.
///
/// Best-first over `f = g + h` where `g` = moves played and `h` =
/// [`DiscoveryGame::cost`] (an admissible steer toward cheaper discoveries). A
/// transposition table de-duplicates equivalent positions by
/// [`DiscoveryGame::dedup_key`] (the MCGS graph collapse). Every certified terminal
/// encountered is compared by cost; the cheapest is returned. The search is a pure
/// function of `(game, root, config, seed)` — no wall-clock, no unseeded
/// randomness; `seed` only breaks priority ties.
///
/// Soundness: a state contributes to the result **only** when
/// [`DiscoveryGame::certificate`] returns `Some`. An uncertified terminal (a dead
/// end that is not a discovery) is never reported. If nothing is certified within
/// budget, the result carries `certificate: None` — never a false discovery.
pub fn search_discovery<G: DiscoveryGame>(
    game: &G,
    root: G::State,
    config: DiscoveryConfig,
    seed: u64,
) -> DiscoveryResult<G::Move> {
    let mut arena: Vec<PathNode<G>> = Vec::new();
    // Best cost-to-reach per transposition key (uniform move cost = 1 per step).
    let mut best_g: HashMap<String, usize> = HashMap::new();
    let mut heap: BinaryHeap<Frontier> = BinaryHeap::new();

    let root_key = game.dedup_key(&root);
    let root_h = game.cost(&root);
    arena.push(PathNode {
        state: root,
        parent: None,
        mv: None,
        depth: 0,
    });
    best_g.insert(root_key.clone(), 0);
    heap.push(Frontier {
        f: root_h,
        tie: mix_seed(seed, &root_key),
        node: 0,
    });

    let mut states_explored = 0usize;
    let mut best: Option<Certificate<G::Move>> = None;

    while let Some(item) = heap.pop() {
        if states_explored >= config.max_states {
            break;
        }
        let node_idx = item.node;
        let g_cost = arena[node_idx].depth;

        // A stale heap entry: a cheaper path to this state was found after this
        // entry was pushed. Skip it (lazy decrease-key).
        {
            let key = game.dedup_key(&arena[node_idx].state);
            if best_g.get(&key).is_some_and(|&bg| bg < g_cost) {
                continue;
            }
        }

        states_explored += 1;

        // Certificate boundary: only a certified terminal is a discovery.
        if let Some(cert) = game.certificate(&arena[node_idx].state) {
            if cert.verified {
                let moves = reconstruct_moves::<G>(&arena, node_idx);
                let cost = moves.len() as f64;
                let discovery = Certificate {
                    moves,
                    cost,
                    verified: true,
                };
                let better = match &best {
                    None => true,
                    Some(b) => cost < b.cost,
                };
                if better {
                    best = Some(discovery);
                }
            }
            // A certified terminal has no successors worth expanding — the
            // discovery is complete at this node.
            continue;
        }

        if g_cost >= config.max_depth {
            continue;
        }

        // Expand: push each legal successor, de-duplicated by transposition key.
        let successors = game.legal_moves(&arena[node_idx].state);
        for mv in successors {
            let next = game.apply(&arena[node_idx].state, &mv);
            let next_key = game.dedup_key(&next);
            let next_g = g_cost + 1;
            // Skip if we already reached this position at least as cheaply.
            if best_g.get(&next_key).is_some_and(|&bg| bg <= next_g) {
                continue;
            }
            best_g.insert(next_key.clone(), next_g);
            let h = game.cost(&next);
            let child_idx = arena.len();
            arena.push(PathNode {
                state: next,
                parent: Some(node_idx),
                mv: Some(mv),
                depth: next_g,
            });
            heap.push(Frontier {
                f: next_g as f64 + h,
                tie: mix_seed(seed, &next_key),
                node: child_idx,
            });
        }
    }

    let best_cost = best.as_ref().map(|c| c.cost);
    let certified = best.as_ref().is_some_and(|c| c.verified);
    DiscoveryResult {
        certificate: best,
        best_cost,
        states_explored,
        certified,
    }
}

/// Walk the arena parent-chain from `leaf` back to the root, collecting the moves
/// in play order.
fn reconstruct_moves<G: DiscoveryGame>(arena: &[PathNode<G>], leaf: usize) -> Vec<G::Move> {
    let mut moves = Vec::new();
    let mut cur = Some(leaf);
    while let Some(idx) = cur {
        if let Some(mv) = &arena[idx].mv {
            moves.push(mv.clone());
        }
        cur = arena[idx].parent;
    }
    moves.reverse();
    moves
}

// ---------------------------------------------------------------------------
// MCGS-driver reuse: adapt a DiscoveryGame onto the driver's GoalState /
// TacticExpander seam and confirm a discovery is reachable with the *real*
// ProofSearchDriver. `is_closed == certificate().is_some()` is the exact map.
// ---------------------------------------------------------------------------

/// A [`GoalState`] view of a game position: `is_closed` is the certificate check.
/// `closed`/`key` are precomputed so the trait methods (which see only `&self`)
/// need no game reference.
#[derive(Clone)]
struct GameGoal<S: Clone> {
    state: S,
    key: String,
    closed: bool,
}

impl<S: Clone> GoalState for GameGoal<S> {
    fn dedup_key(&self) -> String {
        self.key.clone()
    }
    fn is_closed(&self) -> bool {
        self.closed
    }
}

/// A [`TacticExpander`] that enumerates a game's legal moves as tactic steps —
/// letting the MCGS `ProofSearchDriver` search the discovery game unmodified.
struct GameExpander<'g, G: DiscoveryGame> {
    game: &'g G,
}

impl<G: DiscoveryGame> TacticExpander for GameExpander<'_, G> {
    type State = GameGoal<G::State>;
    fn expand(&mut self, state: &Self::State, _seed: u64) -> Vec<TacticStep<Self::State>> {
        let mut out = Vec::new();
        for (i, mv) in self.game.legal_moves(&state.state).into_iter().enumerate() {
            let next = self.game.apply(&state.state, &mv);
            let key = self.game.dedup_key(&next);
            let closed = self.game.certificate(&next).is_some();
            out.push(TacticStep::new(
                format!("move#{i}"),
                1.0,
                GameGoal {
                    state: next,
                    key,
                    closed,
                },
            ));
        }
        out
    }
}

/// Independently confirm — with the *real* MCGS [`ProofSearchDriver`] — that a
/// certified discovery is reachable from `root` within `budget` node expansions.
/// This reuses the driver seam (`is_closed == certificate().is_some()`) as a
/// cross-check on [`search_discovery`]; the driver returns only reachability
/// (`solved`), not the cheapest path, which is why it is a confirmation and not the
/// primary search. Seeded and deterministic.
pub fn confirm_reachable_via_mcgs<G: DiscoveryGame>(
    game: &G,
    root: &G::State,
    budget: usize,
    seed: u64,
) -> bool {
    let root_goal = GameGoal {
        state: root.clone(),
        key: game.dedup_key(root),
        closed: game.certificate(root).is_some(),
    };
    let cfg = SearchConfig {
        max_nodes: budget.max(1),
        ..SearchConfig::default()
    };
    let mut driver = ProofSearchDriver::new(GameExpander { game })
        .with_seed(seed)
        .with_config(cfg);
    driver.run(root_goal).solved
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Worked fixture: a residual-vector reduction game (toy TensorGame) ----
    //
    // The residual is an integer vector; each move subtracts one allowed rank-1
    // "update" vector from it. A certified terminal = the residual is fully zeroed
    // (a correct decomposition). The discovery cost = number of moves = the "rank".
    // With basis {[1,0],[0,1],[1,1]} and root [2,2] there are two certified
    // solutions: two [1,1] moves (cost 2, optimal) or four axis moves (cost 4) — a
    // clean probe that the search prefers the lower-cost discovery.

    #[derive(Clone)]
    struct ResidualGame {
        basis: Vec<Vec<i64>>,
    }

    impl ResidualGame {
        fn new() -> Self {
            Self {
                basis: vec![vec![1, 0], vec![0, 1], vec![1, 1]],
            }
        }
    }

    impl DiscoveryGame for ResidualGame {
        type State = Vec<i64>;
        type Move = usize; // index into `basis`

        fn legal_moves(&self, state: &Vec<i64>) -> Vec<usize> {
            if state.iter().all(|&x| x == 0) {
                return Vec::new(); // terminal: no moves
            }
            // Only moves that do not push any coordinate below zero — keeps the
            // reachable space finite and every move genuine progress.
            (0..self.basis.len())
                .filter(|&i| {
                    state
                        .iter()
                        .zip(&self.basis[i])
                        .all(|(&s, &b)| s - b >= 0)
                })
                .collect()
        }

        fn apply(&self, state: &Vec<i64>, mv: &usize) -> Vec<i64> {
            state
                .iter()
                .zip(&self.basis[*mv])
                .map(|(&s, &b)| s - b)
                .collect()
        }

        fn certificate(&self, state: &Vec<i64>) -> Option<Certificate<usize>> {
            if state.iter().all(|&x| x == 0) {
                Some(Certificate {
                    moves: Vec::new(), // filled in by the search from the path
                    cost: 0.0,
                    verified: true,
                })
            } else {
                None
            }
        }

        fn cost(&self, state: &Vec<i64>) -> f64 {
            // Admissible: each move subtracts at most 1 from any single coordinate,
            // so the max coordinate is a lower bound on the moves remaining.
            state.iter().copied().max().unwrap_or(0).max(0) as f64
        }

        fn dedup_key(&self, state: &Vec<i64>) -> String {
            format!("{state:?}")
        }
    }

    #[test]
    fn solvable_game_reaches_a_certified_verified_discovery() {
        let game = ResidualGame::new();
        let result = search_discovery(&game, vec![2, 2], DiscoveryConfig::default(), 7);

        let cert = result
            .certificate
            .expect("a certified discovery must be found");
        assert!(cert.verified, "the discovery must be verified");
        assert!(result.certified);
        assert_eq!(result.best_cost, Some(cert.cost));
        // Replaying the moves from the root must actually zero the residual — the
        // certificate is genuine, not asserted.
        let mut s = vec![2, 2];
        for mv in &cert.moves {
            s = game.apply(&s, mv);
        }
        assert!(s.iter().all(|&x| x == 0), "replayed moves must zero the residual");
    }

    #[test]
    fn search_prefers_the_lower_cost_certified_solution() {
        // Two certified solutions exist (cost 2 via [1,1]×2, cost 4 via axes). The
        // search must return the rank-2 discovery.
        let game = ResidualGame::new();
        let result = search_discovery(&game, vec![2, 2], DiscoveryConfig::default(), 1);
        assert_eq!(
            result.best_cost,
            Some(2.0),
            "must prefer the 2-move discovery over the 4-move one"
        );
        assert_eq!(result.certificate.unwrap().moves.len(), 2);
    }

    #[test]
    fn unsolvable_in_budget_returns_no_discovery() {
        // A one-state budget cannot reach the zero residual from [3, 3]; the result
        // must be *no discovery* — never a false/uncertified one.
        let game = ResidualGame::new();
        let cfg = DiscoveryConfig {
            max_states: 1,
            max_depth: 64,
        };
        let result = search_discovery(&game, vec![3, 3], cfg, 0);
        assert!(result.certificate.is_none(), "no discovery within a 1-state budget");
        assert!(!result.certified);
        assert_eq!(result.best_cost, None);
        assert!(result.states_explored <= 1);
    }

    #[test]
    fn search_is_seeded_and_deterministic() {
        let game = ResidualGame::new();
        let r1 = search_discovery(&game, vec![2, 2], DiscoveryConfig::default(), 42);
        let r2 = search_discovery(&game, vec![2, 2], DiscoveryConfig::default(), 42);
        assert_eq!(r1.best_cost, r2.best_cost);
        assert_eq!(r1.states_explored, r2.states_explored);
        assert_eq!(
            r1.certificate.map(|c| c.moves),
            r2.certificate.map(|c| c.moves)
        );
    }

    // ---- Certificate boundary: an uncertified terminal is never a discovery ----

    /// A game with two terminals: a dead end (`no moves`, NOT certified) reachable
    /// by move `dead`, and a genuine discovery (`certified`) reachable by move
    /// `win`. The search must report only the certified one and never mistake the
    /// dead end for a discovery.
    struct BoundaryGame;

    impl DiscoveryGame for BoundaryGame {
        type State = &'static str;
        type Move = &'static str;

        fn legal_moves(&self, state: &&'static str) -> Vec<&'static str> {
            match *state {
                "root" => vec!["dead", "win"],
                _ => Vec::new(), // both "deadend" and "goal" are terminals
            }
        }
        fn apply(&self, _state: &&'static str, mv: &&'static str) -> &'static str {
            match *mv {
                "dead" => "deadend",
                _ => "goal",
            }
        }
        fn certificate(&self, state: &&'static str) -> Option<Certificate<&'static str>> {
            // Only "goal" is a certified discovery. "deadend" is a terminal (no
            // moves) but NOT certified — the boundary must reject it.
            if *state == "goal" {
                Some(Certificate {
                    moves: Vec::new(),
                    cost: 0.0,
                    verified: true,
                })
            } else {
                None
            }
        }
        fn dedup_key(&self, state: &&'static str) -> String {
            (*state).to_string()
        }
    }

    #[test]
    fn certificate_boundary_rejects_an_uncertified_terminal() {
        let game = BoundaryGame;
        let result = search_discovery(&game, "root", DiscoveryConfig::default(), 3);
        let cert = result.certificate.expect("the certified terminal is found");
        assert_eq!(cert.moves, vec!["win"], "only the certified path is a discovery");
        assert!(cert.verified);
    }

    /// A game whose only terminal is an *uncertified* dead end — there is no
    /// discovery at all. The search must return `None`, never inventing one.
    struct DeadEndGame;
    impl DiscoveryGame for DeadEndGame {
        type State = u32;
        type Move = u32;
        fn legal_moves(&self, state: &u32) -> Vec<u32> {
            if *state == 0 {
                Vec::new()
            } else {
                vec![state - 1]
            }
        }
        fn apply(&self, _state: &u32, mv: &u32) -> u32 {
            *mv
        }
        fn certificate(&self, _state: &u32) -> Option<Certificate<u32>> {
            None // nothing is ever certified — no discovery exists
        }
        fn dedup_key(&self, state: &u32) -> String {
            state.to_string()
        }
    }

    #[test]
    fn no_certified_terminal_means_no_false_discovery() {
        let game = DeadEndGame;
        let result = search_discovery(&game, 3, DiscoveryConfig::default(), 0);
        assert!(result.certificate.is_none());
        assert!(!result.certified);
        // It exhausted the reachable (uncertified) states without a discovery.
        assert!(result.states_explored >= 1);
    }

    // ---- MCGS-driver reuse: independent reachability confirmation ----

    #[test]
    fn mcgs_driver_confirms_a_reachable_discovery() {
        let game = ResidualGame::new();
        // The real ProofSearchDriver, driven through the game adapter, must find a
        // closed (certified) state reachable from [2, 2].
        assert!(
            confirm_reachable_via_mcgs(&game, &vec![2, 2], 500, 7),
            "the MCGS driver should confirm a discovery is reachable"
        );
    }

    #[test]
    fn mcgs_driver_finds_no_discovery_in_a_dead_end_game() {
        let game = DeadEndGame;
        // No state is ever certified, so the driver never closes — reachability is
        // false, matching search_discovery's None.
        assert!(!confirm_reachable_via_mcgs(&game, &5, 500, 1));
    }
}
