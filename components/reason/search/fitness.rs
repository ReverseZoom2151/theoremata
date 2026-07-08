//! Elo / Plackett-Luce fitness over *incomplete* proof candidates, plus a
//! predictor-UCB (P-UCB) selection score.
//!
//! Full proof search rarely gets a clean 0/1 verifier signal on partial work:
//! most candidates are incomplete blueprints or half-closed proof DAGs that can
//! only be judged *relatively* ("this partial proof looks more promising than
//! that one"). [`EloRanker`] turns such pairwise / among-set comparison outcomes
//! into scalar ratings so partial candidates can be ranked, and [`p_ucb`] turns a
//! rating into a selection score that trades exploitation (high rating) against
//! exploration (few visits) — the P-UCB rule an MCTS/best-first search uses to
//! pick which candidate to expand next.
//!
//! ## Model
//!
//! Ratings follow the standard logistic Elo/Bradley-Terry model: the expected
//! score of `a` against `b` is `1 / (1 + 10^((r_b - r_a)/400))`, and an observed
//! outcome nudges both ratings by `K · (observed − expected)`. An among-set
//! ranking (a Plackett-Luce style ordering, best-first) is decomposed into the
//! pairwise "each candidate beats everyone ranked below it" outcomes and applied
//! in that order. Every update is deterministic — no clock, no randomness — so a
//! fixed sequence of outcomes always yields the same ratings.

use std::collections::HashMap;

/// The default starting rating for an unseen candidate (classic Elo anchor).
pub const DEFAULT_RATING: f64 = 1500.0;

/// The default Elo K-factor (update step size).
pub const DEFAULT_K: f64 = 32.0;

/// Maintains logistic Elo / Bradley-Terry ratings for a set of proof candidates,
/// updated from pairwise or among-set comparison outcomes.
#[derive(Debug, Clone)]
pub struct EloRanker {
    ratings: HashMap<String, f64>,
    initial: f64,
    k: f64,
}

impl Default for EloRanker {
    fn default() -> Self {
        Self::new(DEFAULT_RATING, DEFAULT_K)
    }
}

impl EloRanker {
    /// A ranker with a custom starting rating and K-factor.
    pub fn new(initial: f64, k: f64) -> Self {
        Self {
            ratings: HashMap::new(),
            initial,
            k,
        }
    }

    /// The current rating of `id` (the starting rating if never compared).
    pub fn rating(&self, id: &str) -> f64 {
        self.ratings.get(id).copied().unwrap_or(self.initial)
    }

    /// Ensure `id` has a rating entry, returning it.
    fn ensure(&mut self, id: &str) -> f64 {
        let init = self.initial;
        *self.ratings.entry(id.to_string()).or_insert(init)
    }

    /// The expected score of a rating `r_a` against `r_b` under the logistic Elo
    /// curve — the probability `a` "wins", in `(0, 1)`.
    pub fn expected(r_a: f64, r_b: f64) -> f64 {
        1.0 / (1.0 + 10f64.powf((r_b - r_a) / 400.0))
    }

    /// Record one pairwise outcome: `winner` beat `loser`. Updates both ratings.
    pub fn record_outcome(&mut self, winner: &str, loser: &str) {
        self.record_score(winner, loser, 1.0);
    }

    /// Record a drawn comparison between `a` and `b` (neither strictly better).
    pub fn record_draw(&mut self, a: &str, b: &str) {
        self.record_score(a, b, 0.5);
    }

    /// Core update: `score` is `a`'s observed result against `b` (`1.0` win,
    /// `0.5` draw, `0.0` loss). Both ratings move by `K · (observed − expected)`,
    /// conserving total rating.
    fn record_score(&mut self, a: &str, b: &str, score: f64) {
        let ra = self.ensure(a);
        let rb = self.ensure(b);
        let ea = Self::expected(ra, rb);
        let delta = self.k * (score - ea);
        self.ratings.insert(a.to_string(), ra + delta);
        self.ratings.insert(b.to_string(), rb - delta);
    }

    /// Record an among-set outcome as a best-first `ranking` (index `0` is the
    /// strongest candidate). Plackett-Luce style: decomposed into the pairwise
    /// "each candidate beats every candidate ranked strictly below it" outcomes,
    /// applied in a fixed order so the update is deterministic.
    pub fn record_ranking(&mut self, ranking: &[&str]) {
        for i in 0..ranking.len() {
            for j in (i + 1)..ranking.len() {
                self.record_outcome(ranking[i], ranking[j]);
            }
        }
    }

    /// All rated candidates ordered best-first (`(id, rating)`). Ties break by id
    /// so the ordering is deterministic.
    pub fn ranking(&self) -> Vec<(String, f64)> {
        let mut v: Vec<(String, f64)> = self
            .ratings
            .iter()
            .map(|(k, &r)| (k.clone(), r))
            .collect();
        v.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        v
    }
}

/// Predictor-UCB (P-UCB) selection score for a candidate.
///
/// Combines exploitation (the candidate's `rating`) with an exploration bonus
/// that grows with the log of the total visit count and shrinks as this
/// candidate's own `visits` grow:
///
/// `p_ucb = rating + c · sqrt(ln(total_visits + 1) / (visits + 1))`
///
/// So among equally-visited candidates the higher rating wins, and among
/// equally-rated candidates the *less-visited* one wins — biasing search toward
/// promising-but-under-explored candidates. `c` tunes the exploration weight.
/// Deterministic and monotone: increasing `rating` or `total_visits` never
/// decreases the score; increasing `visits` never increases it.
pub fn p_ucb(rating: f64, visits: u32, total_visits: u32, c: f64) -> f64 {
    let exploration = c * ((total_visits as f64 + 1.0).ln() / (visits as f64 + 1.0)).sqrt();
    rating + exploration
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn winner_ranks_above_losers() {
        let mut r = EloRanker::default();
        // `champ` beats both challengers repeatedly.
        for _ in 0..5 {
            r.record_outcome("champ", "a");
            r.record_outcome("champ", "b");
        }
        assert!(
            r.rating("champ") > r.rating("a"),
            "a consistent winner must out-rate its losers"
        );
        assert!(r.rating("champ") > r.rating("b"));
        // The winner is first in the ranking.
        assert_eq!(r.ranking()[0].0, "champ");
    }

    #[test]
    fn among_set_ranking_orders_candidates() {
        let mut r = EloRanker::default();
        // best > mid > worst, observed several times.
        for _ in 0..4 {
            r.record_ranking(&["best", "mid", "worst"]);
        }
        let ranked: Vec<String> = r.ranking().into_iter().map(|(id, _)| id).collect();
        assert_eq!(ranked, vec!["best", "mid", "worst"]);
    }

    #[test]
    fn expected_score_is_symmetric_and_centered() {
        // Equal ratings => 50/50.
        assert!((EloRanker::expected(1500.0, 1500.0) - 0.5).abs() < 1e-9);
        // A higher-rated player is favoured.
        assert!(EloRanker::expected(1700.0, 1500.0) > 0.5);
        // Symmetry: the two expectations sum to 1.
        let ea = EloRanker::expected(1600.0, 1400.0);
        let eb = EloRanker::expected(1400.0, 1600.0);
        assert!((ea + eb - 1.0).abs() < 1e-9);
    }

    #[test]
    fn p_ucb_favors_high_rating_low_visit() {
        // High rating, few visits should beat low rating, many visits.
        let strong = p_ucb(1600.0, 1, 100, 50.0);
        let weak = p_ucb(1400.0, 40, 100, 50.0);
        assert!(strong > weak);

        // Same rating: the less-visited candidate scores higher (exploration).
        let fresh = p_ucb(1500.0, 1, 100, 50.0);
        let stale = p_ucb(1500.0, 50, 100, 50.0);
        assert!(fresh > stale, "exploration must favor the under-visited");

        // Same visits: the higher rating scores higher (exploitation).
        let hi = p_ucb(1550.0, 10, 100, 50.0);
        let lo = p_ucb(1500.0, 10, 100, 50.0);
        assert!(hi > lo);
    }

    #[test]
    fn ratings_are_deterministic() {
        let run = || {
            let mut r = EloRanker::default();
            r.record_outcome("a", "b");
            r.record_ranking(&["a", "c", "b"]);
            r.record_draw("a", "c");
            (r.rating("a"), r.rating("b"), r.rating("c"))
        };
        assert_eq!(run(), run(), "a fixed outcome sequence must reproduce");
    }
}
