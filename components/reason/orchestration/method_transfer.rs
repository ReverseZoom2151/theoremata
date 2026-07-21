//! Method-transfer / batch-application driver (Terence Tao's "systematically
//! exploring the application of well-understood methods to reasonably large
//! classes of problems at a scale impracticable for humans").
//!
//! Where [`crate::blueprint_run`] drives ONE result's internal DAG (lemmas →
//! theorem), this module drives ONE *method* across a whole FAMILY of sibling
//! problems: given a proven technique (its reusable lemmas + a proof-shape hint)
//! and a family of related statements (the Erdős-#728-solved-#729 pattern, with
//! `related_to` cross-links pointing at other family members or external prior
//! problems like #401/#400), it applies the method to each problem in turn,
//! THREADING every already-solved family member back in as reusable context, and
//! reports coverage: which problems the method closed, which it could not, and
//! which are already known in the literature.
//!
//! The design deliberately mirrors [`crate::blueprint_run`]:
//! * an injected prover seam ([`MethodProver`], cf. `ObligationProver`) keeps the
//!   whole run testable offline with a deterministic mock; the production impl
//!   would prime the sketch/portfolio path + seed the [`crate::library`] lemma
//!   store with the method's lemmas;
//! * an `available: &[SolvedProblem]` slice threads earlier proofs forward,
//!   exactly like `blueprint_run`'s `AvailableLemma` context — the family-level
//!   analogue of the cross-run reuse offered by [`crate::library::LemmaLibrary`]
//!   and [`crate::goal_cache::GoalCache`];
//! * a deterministic order (soft topo-sort over `related_to`, stable tie-break by
//!   label) and an honest [`FamilyReport`] (`coverage = n_solved / n_items`).
//!
//! An optional [`NoveltyOracle`] (conceptually the Python `novelty` worker in
//! `theoremata_tools.novelty`) is consulted BEFORE applying the method: a family
//! member that is already published short-circuits to `AlreadyKnown` and is never
//! (falsely) counted as a fresh solve.
//!
//! Determinism: no wall-clock, no unseeded RNG. A base `seed` from
//! [`TransferConfig`] is mixed with each problem's label into a per-problem seed
//! threaded into [`MethodProver::apply`]. All method/problem/proof text is
//! UNTRUSTED DATA — it is only ever stored, threaded to the injected prover, or
//! scanned for reused labels; it is never executed.

use crate::{
    config::Config, db::Store, portfolio::portfolio_prove, prover::formal::FormalSystem,
    provider::ModelProvider,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet};

// ------------------------------------------------------------------------
// The method and the family
// ------------------------------------------------------------------------

/// One reusable piece of a proven technique: a lemma statement and its proof.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MethodLemma {
    /// The lemma's statement (untrusted prose / formal goal).
    pub statement: String,
    /// The proof that discharges it.
    pub proof: String,
}

/// A proven METHOD: the technique's name, its reusable key lemmas, and a
/// proof-shape hint describing how the method is applied. In production the
/// lemmas would be seeded into the [`crate::library::LemmaLibrary`] and the hint
/// would prime the sketch generator; here they are threaded to [`MethodProver`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Method {
    /// A human-readable name for the method (e.g. `"Erdős-728 sieve"`).
    pub name: String,
    /// The method's reusable key lemmas.
    #[serde(default)]
    pub lemmas: Vec<MethodLemma>,
    /// A proof-shape hint: how the method's steps are assembled.
    #[serde(default)]
    pub shape_hint: String,
}

/// A single problem in the family the method is being applied to.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FamilyProblem {
    /// A stable, unique label for this problem (e.g. `"erdos-729"`).
    pub label: String,
    /// The problem's statement (untrusted prose / formal goal).
    pub statement: String,
    /// Soft cross-links: labels of related problems (other family members, or
    /// external prior problems like `"erdos-401"`). In-family links order the
    /// family (dependencies applied first); all links are reported verbatim.
    #[serde(default)]
    pub related_to: Vec<String>,
}

// ------------------------------------------------------------------------
// The prover seam + novelty oracle
// ------------------------------------------------------------------------

/// A family member the method has already solved, threaded forward as reusable
/// context to later applications — the family-level analogue of
/// [`crate::blueprint_run::AvailableLemma`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolvedProblem {
    /// The solved problem's label.
    pub label: String,
    /// Its statement.
    pub statement: String,
    /// The proof the method produced for it.
    pub proof: String,
}

/// Applies a proven method to a single family problem. `Ok(Some(proof))` = the
/// method closed the problem; `Ok(None)` = attempted but not closed (never a
/// false solve); `Err` is treated by the driver as a non-close (recorded
/// `Failed`). `available` carries every already-solved family member so the
/// method can reuse earlier proofs; `seed` is the deterministic per-problem seed.
///
/// This is the single injection seam. The deterministic MOCK drives the tests;
/// production would prime the sketch/portfolio path and the lemma library with
/// `method.lemmas` before attempting `problem`.
pub trait MethodProver {
    fn apply(
        &self,
        method: &Method,
        problem: &FamilyProblem,
        available: &[SolvedProblem],
        seed: u64,
    ) -> anyhow::Result<Option<String>>;
}

/// A prior-work hit: the closest already-published result to a statement.
/// Mirrors the record the Python `novelty` worker returns
/// (`theoremata_tools.novelty`): a title, a citation ref, and a bounded score.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PriorWork {
    /// The matching known result's title.
    pub title: String,
    /// A citation reference for it (e.g. a paper handle).
    pub reference: String,
    /// Similarity in `[0, 1]` (higher = closer to a duplicate).
    pub score: f64,
}

/// An optional prior-work checker, consulted before applying the method to a
/// problem. `Some(prior)` flags the statement as already known (short-circuits to
/// [`ApplicationStatus::AlreadyKnown`]); `None` means "no close prior work found,
/// proceed to apply". Conceptually the Python `novelty` worker; injected so the
/// driver stays deterministic and offline.
pub trait NoveltyOracle {
    fn known(&self, statement: &str) -> Option<PriorWork>;
}

// ------------------------------------------------------------------------
// The report
// ------------------------------------------------------------------------

/// The outcome of applying the method to one family problem.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ApplicationStatus {
    /// The method closed the problem (a fresh solve).
    Applied,
    /// The method was attempted but did not close the problem.
    Failed,
    /// A novelty oracle flagged the problem as already published — not attempted.
    AlreadyKnown {
        /// The prior-work hit that caused the short-circuit.
        prior: PriorWork,
    },
}

/// Per-problem entry of the family report.
#[derive(Debug, Clone, Serialize)]
pub struct FamilyItemReport {
    /// The problem's label.
    pub label: String,
    /// The problem's statement.
    pub statement: String,
    /// Applied / failed / already-known.
    #[serde(flatten)]
    pub status: ApplicationStatus,
    /// The produced proof, present iff `status == Applied`.
    pub proof: Option<String>,
    /// Labels of earlier-solved family members this proof reused (detected by
    /// scanning the produced proof for available labels). Empty unless applied.
    pub reused_from: Vec<String>,
}

impl FamilyItemReport {
    /// Whether the method closed this problem (a fresh solve).
    pub fn is_applied(&self) -> bool {
        matches!(self.status, ApplicationStatus::Applied)
    }
}

/// The structured result of transferring one method across a family.
#[derive(Debug, Clone, Serialize)]
pub struct FamilyReport {
    /// The method's name.
    pub method: String,
    /// The deterministic order the family was driven in (labels).
    pub order: Vec<String>,
    /// Per-problem reports, in `order`.
    pub items: Vec<FamilyItemReport>,
    /// Problems the method freshly closed.
    pub n_solved: usize,
    /// Problems the method attempted but could not close.
    pub n_failed: usize,
    /// Problems short-circuited as already known.
    pub n_known: usize,
    /// Honest coverage: `n_solved / n_items` (0.0 for an empty family). Already
    /// known problems are NOT counted as solves.
    pub coverage: f64,
}

impl FamilyReport {
    /// Total number of family problems.
    pub fn n_items(&self) -> usize {
        self.items.len()
    }
}

// ------------------------------------------------------------------------
// Config
// ------------------------------------------------------------------------

/// Deterministic knobs for a transfer run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferConfig {
    /// Base seed mixed with each problem's label into the per-problem seed
    /// threaded to [`MethodProver::apply`]. No wall-clock, no unseeded RNG.
    pub seed: u64,
}

impl Default for TransferConfig {
    fn default() -> Self {
        Self { seed: 0 }
    }
}

// ------------------------------------------------------------------------
// The driver
// ------------------------------------------------------------------------

/// Apply `method` to every problem in `family`, reusing proven sub-results across
/// the family, and report coverage.
///
/// Steps, all deterministic:
/// 1. ORDER the family by a soft topo-sort over in-family `related_to` links
///    (related problems applied first), stable tie-break by label; any cyclic
///    remainder is appended in label order — a soft dependency never loops.
/// 2. For each problem, optionally consult `novelty` FIRST: a hit short-circuits
///    to `AlreadyKnown` (never counted as a solve, never added to `available`).
/// 3. Otherwise APPLY the method with every already-solved family member threaded
///    in as `available`, under a deterministic per-problem seed. On a close, the
///    problem joins `available` so later problems can reuse it; `reused_from`
///    records which available proofs the produced proof cites.
///
/// A prover `Err` is recorded as `Failed` (a non-close) rather than propagated,
/// keeping coverage honest and the signature total.
pub fn transfer_method(
    method: &Method,
    family: &[FamilyProblem],
    prover: &dyn MethodProver,
    novelty: Option<&dyn NoveltyOracle>,
    config: TransferConfig,
) -> FamilyReport {
    let order = order_family(family);

    // label -> problem, for O(1) lookup while walking the order.
    let by_label: HashMap<&str, &FamilyProblem> =
        family.iter().map(|p| (p.label.as_str(), p)).collect();

    let mut available: Vec<SolvedProblem> = Vec::new();
    let mut items = Vec::with_capacity(order.len());
    let (mut n_solved, mut n_failed, mut n_known) = (0usize, 0usize, 0usize);

    for label in &order {
        let Some(problem) = by_label.get(label.as_str()).copied() else {
            continue; // unreachable: order is built from family labels.
        };

        // 1. Novelty short-circuit (already-published results never false-solve).
        if let Some(prior) = novelty.and_then(|o| o.known(&problem.statement)) {
            n_known += 1;
            items.push(FamilyItemReport {
                label: problem.label.clone(),
                statement: problem.statement.clone(),
                status: ApplicationStatus::AlreadyKnown { prior },
                proof: None,
                reused_from: Vec::new(),
            });
            continue;
        }

        // 2. Apply the method, threading every already-solved member forward.
        let seed = mix_seed(config.seed, &problem.label);
        let outcome = prover.apply(method, problem, &available, seed);

        match outcome {
            Ok(Some(proof)) => {
                let reused_from = reused_labels(&proof, &available);
                n_solved += 1;
                available.push(SolvedProblem {
                    label: problem.label.clone(),
                    statement: problem.statement.clone(),
                    proof: proof.clone(),
                });
                items.push(FamilyItemReport {
                    label: problem.label.clone(),
                    statement: problem.statement.clone(),
                    status: ApplicationStatus::Applied,
                    proof: Some(proof),
                    reused_from,
                });
            }
            // Attempted-but-not-closed and prover errors both mean "not solved";
            // never a false solve.
            Ok(None) | Err(_) => {
                n_failed += 1;
                items.push(FamilyItemReport {
                    label: problem.label.clone(),
                    statement: problem.statement.clone(),
                    status: ApplicationStatus::Failed,
                    proof: None,
                    reused_from: Vec::new(),
                });
            }
        }
    }

    let n_items = items.len();
    let coverage = if n_items == 0 {
        0.0
    } else {
        n_solved as f64 / n_items as f64
    };

    FamilyReport {
        method: method.name.clone(),
        order,
        items,
        n_solved,
        n_failed,
        n_known,
        coverage,
    }
}

// ------------------------------------------------------------------------
// CLI entry point
// ------------------------------------------------------------------------

/// A transfer run described as data, so a CLI dispatch arm can deserialize one
/// from a JSON request. Mirrors the driver's inputs: the method, the family, and
/// the deterministic base seed; `systems` restricts the portfolio the production
/// prover fans out over (empty = all backends), and `project` scopes the emitted
/// store event.
#[derive(Debug, Clone, Deserialize)]
pub struct TransferSpec {
    /// The proven method to transfer.
    pub method: Method,
    /// The family of sibling problems to apply it to.
    #[serde(default)]
    pub family: Vec<FamilyProblem>,
    /// Formal systems the portfolio prover may attempt (empty = all three).
    #[serde(default)]
    pub systems: Vec<FormalSystem>,
    /// Base seed threaded to the driver (defaults to 0, matching
    /// [`TransferConfig::default`]).
    #[serde(default)]
    pub seed: u64,
    /// Optional project id used only to scope the emitted store event.
    #[serde(default)]
    pub project: Option<String>,
}

/// The production [`MethodProver`]: prove each family problem through the real
/// portfolio gate ([`portfolio_prove`]). A close is reported ONLY when a system
/// WON, and [`portfolio_prove`] sets a winner solely on `report.live &&
/// lexically_verified`, so a mock/source-scan pass is never mistaken for a solve.
/// Fail-closed: no winner yields `Ok(None)` (attempted-but-not-closed), never a
/// false solve.
///
/// This is a thin adapter: it does NOT yet thread the method's lemmas into the
/// library or the already-solved `available` members / per-problem `seed` into
/// the sketch generator (the module doc's "production would prime the
/// sketch/portfolio path" is still future work). Those seams are ignored here
/// rather than faked, so the coverage the driver reports is honest.
struct PortfolioMethodProver<'a> {
    store: &'a Store,
    config: &'a Config,
    provider: &'a dyn ModelProvider,
    systems: Vec<FormalSystem>,
}

impl MethodProver for PortfolioMethodProver<'_> {
    fn apply(
        &self,
        _method: &Method,
        problem: &FamilyProblem,
        _available: &[SolvedProblem],
        _seed: u64,
    ) -> Result<Option<String>> {
        let result = portfolio_prove(
            self.store,
            self.config,
            self.provider,
            &problem.statement,
            &self.systems,
        )?;
        // `winner` is Some only on a live, gate-verified pass. Return that
        // system's accepted source as the proof; anything else is a non-close.
        Ok(result.winner.and_then(|system| {
            result
                .per_system
                .iter()
                .find(|a| a.system == system && a.verified)
                .and_then(|a| a.code.clone())
        }))
    }
}

/// CLI entry point: transfer a proven method across a family of sibling problems,
/// proving each through the real portfolio gate, and report coverage.
///
/// A thin adapter over [`transfer_method`]: it builds the production
/// [`MethodProver`] (portfolio gate) and drives the family. No novelty oracle is
/// wired: the `novelty` short-circuit is a Python worker
/// (`theoremata_tools.novelty`) with no offline Rust impl, so no family member is
/// pre-flagged as already known here; every problem is genuinely attempted.
///
/// Returns the full [`FamilyReport`] as JSON (method name, driven order,
/// per-problem verdicts, and honest `coverage = n_solved / n_items`). Emits one
/// `method_transfer.completed` store event carrying the method, family size, and
/// the three outcome tallies, scoped to `spec.project` when supplied.
pub fn transfer(
    store: &Store,
    config: &Config,
    provider: &dyn ModelProvider,
    spec: &TransferSpec,
) -> Result<Value> {
    let prover = PortfolioMethodProver {
        store,
        config,
        provider,
        systems: spec.systems.clone(),
    };
    // No offline novelty oracle exists in the Rust core (see fn doc); pass None.
    let report = transfer_method(
        &spec.method,
        &spec.family,
        &prover,
        None,
        TransferConfig { seed: spec.seed },
    );

    store.event(
        spec.project.as_deref(),
        None,
        "method_transfer.completed",
        "method_transfer",
        json!({
            "method": report.method,
            "n_items": report.n_items(),
            "n_solved": report.n_solved,
            "n_failed": report.n_failed,
            "n_known": report.n_known,
            "coverage": report.coverage,
        }),
    )?;

    serde_json::to_value(&report).context("serialize family report")
}

/// Deterministic soft topological order over the family's in-family `related_to`
/// links (a related problem is applied before the problem that links to it),
/// with a stable ascending-label tie-break. External / dangling links are
/// ignored for ordering. `related_to` being a SOFT dependency, a cycle is never
/// an error and never loops: any nodes left in a cycle are appended in ascending
/// label order.
fn order_family(family: &[FamilyProblem]) -> Vec<String> {
    // Family labels in input order (dedup, keep first).
    let mut nodes: Vec<&str> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for p in family {
        if seen.insert(p.label.as_str()) {
            nodes.push(p.label.as_str());
        }
    }
    let present: HashSet<&str> = nodes.iter().copied().collect();

    // deps[label] = distinct in-family related_to links (excluding self).
    let mut deps: HashMap<&str, Vec<&str>> = HashMap::new();
    for p in family {
        let mut ds: Vec<&str> = Vec::new();
        let mut ds_seen: HashSet<&str> = HashSet::new();
        for r in &p.related_to {
            let r = r.as_str();
            if r != p.label && present.contains(r) && ds_seen.insert(r) {
                ds.push(r);
            }
        }
        deps.entry(p.label.as_str()).or_default().extend(ds);
    }

    // Kahn's algorithm, ready set popped in ascending label order.
    let mut indegree: HashMap<&str, usize> = HashMap::new();
    for &n in &nodes {
        indegree.insert(n, deps.get(n).map_or(0, |d| d.len()));
    }
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
    for &n in &nodes {
        for &d in deps.get(n).into_iter().flatten() {
            dependents.entry(d).or_default().push(n);
        }
    }
    let mut ready: BTreeSet<&str> = indegree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(&l, _)| l)
        .collect();

    let mut order: Vec<String> = Vec::with_capacity(nodes.len());
    while let Some(&label) = ready.iter().next() {
        ready.remove(label);
        order.push(label.to_string());
        for &dep in dependents.get(label).into_iter().flatten() {
            let deg = indegree.get_mut(dep).expect("dependent has an indegree");
            *deg -= 1;
            if *deg == 0 {
                ready.insert(dep);
            }
        }
    }

    // Soft dependency: append any cycle remainder in ascending label order.
    if order.len() != nodes.len() {
        let placed: HashSet<&str> = order.iter().map(|s| s.as_str()).collect();
        let mut leftover: Vec<&str> = nodes
            .iter()
            .copied()
            .filter(|l| !placed.contains(l))
            .collect();
        leftover.sort_unstable();
        order.extend(leftover.into_iter().map(str::to_string));
    }

    order
}

/// Which already-solved members the produced `proof` reused, detected by scanning
/// the proof text for each available label (a real proof cites the lemmas /
/// prior theorems it applies). Preserves `available` (solve) order; deduped.
fn reused_labels(proof: &str, available: &[SolvedProblem]) -> Vec<String> {
    let mut out = Vec::new();
    for solved in available {
        if proof.contains(&solved.label) && !out.contains(&solved.label) {
            out.push(solved.label.clone());
        }
    }
    out
}

/// Deterministically mix the base seed with a problem label (FNV-1a) into a
/// per-problem seed. Order-independent, reproducible, no RNG.
fn mix_seed(base: u64, label: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ base;
    for b in label.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn method() -> Method {
        Method {
            name: "erdos-728-sieve".into(),
            lemmas: vec![MethodLemma {
                statement: "density increment".into(),
                proof: "by sieve".into(),
            }],
            shape_hint: "apply the sieve, then bound the tail".into(),
        }
    }

    fn problem(label: &str, statement: &str, related: &[&str]) -> FamilyProblem {
        FamilyProblem {
            label: label.into(),
            statement: statement.into(),
            related_to: related.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Deterministic prover: closes every problem NOT in `fails`, producing a
    /// proof that CITES each available (already-solved) member whose label the
    /// problem `related_to` — so `reused_from` detection has something to find.
    struct MockProver {
        fails: HashSet<String>,
        /// Records the (label, seed, available-labels) each apply saw.
        seen: RefCell<Vec<(String, u64, Vec<String>)>>,
    }
    impl MockProver {
        fn new(fails: &[&str]) -> Self {
            Self {
                fails: fails.iter().map(|s| s.to_string()).collect(),
                seen: RefCell::new(Vec::new()),
            }
        }
    }
    impl MethodProver for MockProver {
        fn apply(
            &self,
            method: &Method,
            problem: &FamilyProblem,
            available: &[SolvedProblem],
            seed: u64,
        ) -> anyhow::Result<Option<String>> {
            self.seen.borrow_mut().push((
                problem.label.clone(),
                seed,
                available.iter().map(|s| s.label.clone()).collect(),
            ));
            if self.fails.contains(&problem.label) {
                return Ok(None);
            }
            // Cite the related, already-solved members (the reuse).
            let cited: Vec<&str> = available
                .iter()
                .filter(|s| problem.related_to.contains(&s.label))
                .map(|s| s.label.as_str())
                .collect();
            Ok(Some(format!(
                "apply({}) shape[{}] citing[{}] seed={} -> QED {}",
                method.name,
                method.shape_hint,
                cited.join(","),
                seed,
                problem.label
            )))
        }
    }

    /// Novelty oracle that flags an explicit set of statements as known.
    struct MockNovelty {
        known: HashMap<String, PriorWork>,
    }
    impl MockNovelty {
        fn with(pairs: &[(&str, &str)]) -> Self {
            Self {
                known: pairs
                    .iter()
                    .map(|(stmt, title)| {
                        (
                            stmt.to_string(),
                            PriorWork {
                                title: title.to_string(),
                                reference: "Pomerance 2014".into(),
                                score: 0.87,
                            },
                        )
                    })
                    .collect(),
            }
        }
    }
    impl NoveltyOracle for MockNovelty {
        fn known(&self, statement: &str) -> Option<PriorWork> {
            self.known.get(statement).cloned()
        }
    }

    #[test]
    fn family_of_three_two_applied_one_already_known() {
        // The method solves two; the novelty oracle flags the third as published.
        let family = vec![
            problem("even_sum", "sum of two evens is even", &[]),
            problem("odd_sum", "sum of two odds is even", &[]),
            problem("prime_gap", "there are arbitrarily large prime gaps", &[]),
        ];
        let prover = MockProver::new(&[]);
        let novelty = MockNovelty::with(&[(
            "there are arbitrarily large prime gaps",
            "Bertrand-style gap theorem",
        )]);

        let report = transfer_method(
            &method(),
            &family,
            &prover,
            Some(&novelty),
            TransferConfig::default(),
        );

        assert_eq!(report.n_items(), 3);
        assert_eq!(report.n_solved, 2);
        assert_eq!(report.n_failed, 0);
        assert_eq!(report.n_known, 1);
        assert!((report.coverage - 2.0 / 3.0).abs() < 1e-9);

        // The known one is reported AlreadyKnown with the prior work, no proof,
        // and was never handed to the prover.
        let gap = report
            .items
            .iter()
            .find(|i| i.label == "prime_gap")
            .unwrap();
        match &gap.status {
            ApplicationStatus::AlreadyKnown { prior } => {
                assert_eq!(prior.title, "Bertrand-style gap theorem");
                assert!(prior.score > 0.5);
            }
            other => panic!("expected AlreadyKnown, got {other:?}"),
        }
        assert!(gap.proof.is_none());
        assert!(prover
            .seen
            .borrow()
            .iter()
            .all(|(label, _, _)| label != "prime_gap"));
    }

    #[test]
    fn available_grows_and_later_problems_reuse_earlier_proofs() {
        // A chain: q3 relates to q2 relates to q1. Applied in dependency order,
        // each later proof cites the earlier solved member -> reused_from grows.
        let family = vec![
            problem("q3", "third", &["q2"]),
            problem("q2", "second", &["q1"]),
            problem("q1", "first", &[]),
        ];
        let prover = MockProver::new(&[]);
        let report = transfer_method(&method(), &family, &prover, None, TransferConfig::default());

        // Soft topo order applies dependencies first.
        assert_eq!(report.order, vec!["q1", "q2", "q3"]);
        assert_eq!(report.n_solved, 3);
        assert!((report.coverage - 1.0).abs() < 1e-9);

        let q1 = &report.items[0];
        let q2 = &report.items[1];
        let q3 = &report.items[2];
        assert!(q1.is_applied() && q2.is_applied() && q3.is_applied());
        assert!(q1.reused_from.is_empty(), "first has nothing to reuse");
        assert_eq!(q2.reused_from, vec!["q1".to_string()]);
        assert_eq!(q3.reused_from, vec!["q2".to_string()]);

        // `available` genuinely grew: q3's apply saw both earlier solves.
        let seen = prover.seen.borrow();
        let q3_seen = seen.iter().find(|(l, _, _)| l == "q3").unwrap();
        assert_eq!(q3_seen.2, vec!["q1".to_string(), "q2".to_string()]);
    }

    #[test]
    fn unclosable_problem_is_failed_never_false_solved() {
        let family = vec![
            problem("solvable", "closes fine", &[]),
            problem("hard", "method cannot close this", &[]),
        ];
        let prover = MockProver::new(&["hard"]);
        let report = transfer_method(&method(), &family, &prover, None, TransferConfig::default());

        assert_eq!(report.n_solved, 1);
        assert_eq!(report.n_failed, 1);
        assert_eq!(report.n_known, 0);
        assert!((report.coverage - 0.5).abs() < 1e-9);

        let hard = report.items.iter().find(|i| i.label == "hard").unwrap();
        assert_eq!(hard.status, ApplicationStatus::Failed);
        assert!(
            hard.proof.is_none(),
            "a failed problem never carries a proof"
        );
        assert!(hard.reused_from.is_empty());
    }

    #[test]
    fn seeded_run_is_deterministic() {
        // Same seed -> identical report (order, seeds threaded, proofs, coverage).
        let family = vec![
            problem("b", "beta", &["a"]),
            problem("a", "alpha", &[]),
            problem("c", "gamma", &["a", "b"]),
        ];
        let cfg = TransferConfig { seed: 42 };

        let run = || {
            let prover = MockProver::new(&[]);
            let report = transfer_method(&method(), &family, &prover, None, cfg);
            let seeds: Vec<u64> = prover.seen.borrow().iter().map(|(_, s, _)| *s).collect();
            (report, seeds)
        };
        let (r1, s1) = run();
        let (r2, s2) = run();

        assert_eq!(r1.order, r2.order);
        assert_eq!(s1, s2, "per-problem seeds are reproducible");
        assert_eq!(
            serde_json::to_string(&r1.items).unwrap(),
            serde_json::to_string(&r2.items).unwrap()
        );
        // A different base seed changes the threaded per-problem seeds.
        let cfg2 = TransferConfig { seed: 7 };
        let prover = MockProver::new(&[]);
        transfer_method(&method(), &family, &prover, None, cfg2);
        let s3: Vec<u64> = prover.seen.borrow().iter().map(|(_, s, _)| *s).collect();
        assert_ne!(s1, s3, "the base seed actually reaches the prover");
    }

    #[test]
    fn coverage_is_honest_with_a_mix_of_outcomes() {
        // 4 problems: 2 solved, 1 failed, 1 known -> coverage counts only solves.
        let family = vec![
            problem("p1", "one", &[]),
            problem("p2", "two", &[]),
            problem("p3", "three (unclosable)", &[]),
            problem("p4", "four (known)", &[]),
        ];
        let prover = MockProver::new(&["p3"]);
        let novelty = MockNovelty::with(&[("four (known)", "Known Four")]);
        let report = transfer_method(
            &method(),
            &family,
            &prover,
            Some(&novelty),
            TransferConfig::default(),
        );

        assert_eq!(report.n_solved, 2);
        assert_eq!(report.n_failed, 1);
        assert_eq!(report.n_known, 1);
        // Honest: 2 / 4, NOT 2/3 or 3/4 — known is neither a solve nor removed.
        assert!((report.coverage - 0.5).abs() < 1e-9);
    }

    #[test]
    fn order_tie_breaks_by_label_and_respects_soft_deps() {
        // Two independent roots (z, a) both feed t; stable tie-break orders the
        // roots ascending by label regardless of input order.
        let family = vec![
            problem("z", "zeta", &[]),
            problem("t", "target", &["z", "a"]),
            problem("a", "alpha", &[]),
        ];
        let prover = MockProver::new(&[]);
        let report = transfer_method(&method(), &family, &prover, None, TransferConfig::default());
        assert_eq!(report.order, vec!["a", "z", "t"]);
    }

    #[test]
    fn cyclic_related_to_is_soft_and_never_loops() {
        // r1 <-> r2 form a related_to cycle; a soft dependency must not error or
        // loop — both appear, ordered by label, and both still get driven.
        let family = vec![problem("r2", "two", &["r1"]), problem("r1", "one", &["r2"])];
        let prover = MockProver::new(&[]);
        let report = transfer_method(&method(), &family, &prover, None, TransferConfig::default());
        assert_eq!(report.order, vec!["r1", "r2"]);
        assert_eq!(report.n_solved, 2);
    }

    #[test]
    fn empty_family_reports_zero_coverage() {
        let prover = MockProver::new(&[]);
        let report = transfer_method(&method(), &[], &prover, None, TransferConfig::default());
        assert_eq!(report.n_items(), 0);
        assert_eq!(report.n_solved, 0);
        assert_eq!(report.coverage, 0.0);
        assert!(report.order.is_empty());
    }

    // --- CLI entry point --------------------------------------------------
    // `Config`, `Store`, `FormalSystem` come through `use super::*`.

    use crate::provider::OfflineProvider;
    use std::path::Path;

    /// Mock-backend config: no real toolchain is assumed, so the portfolio runs
    /// deterministically offline (and a mock pass is never `report.live`).
    fn mock_config() -> Config {
        Config {
            prover_mock: true,
            ..Config::default()
        }
    }

    #[test]
    fn transfer_spec_deserializes_with_defaulted_optional_fields() {
        // A minimal spec: only names and statements. lemmas/shape_hint/related_to
        // /systems/seed/project all default, so callers may omit them.
        let spec: TransferSpec = serde_json::from_str(
            r#"{
                "method": { "name": "sieve" },
                "family": [
                    { "label": "q1", "statement": "first" },
                    { "label": "q2", "statement": "second", "related_to": ["q1"] }
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(spec.method.name, "sieve");
        assert!(spec.method.lemmas.is_empty());
        assert_eq!(spec.seed, 0);
        assert_eq!(spec.family.len(), 2);
        assert_eq!(spec.family[1].related_to, vec!["q1".to_string()]);
        assert!(spec.systems.is_empty());
        assert!(spec.project.is_none());
    }

    #[test]
    fn transfer_entry_reports_and_emits_event_offline() {
        // Offline the portfolio can never produce a LIVE gate pass, so the sound
        // adapter closes nothing: every problem is Failed, coverage is 0, and no
        // unverified attempt is ever miscounted as a solve.
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "t").unwrap();
        let spec = TransferSpec {
            method: Method {
                name: "erdos-728-sieve".into(),
                lemmas: Vec::new(),
                shape_hint: String::new(),
            },
            family: vec![
                FamilyProblem {
                    label: "q1".into(),
                    statement: "first".into(),
                    related_to: Vec::new(),
                },
                FamilyProblem {
                    label: "q2".into(),
                    statement: "second".into(),
                    related_to: vec!["q1".into()],
                },
            ],
            systems: vec![FormalSystem::Lean],
            seed: 0,
            project: Some(project.id.clone()),
        };

        let value = transfer(&store, &mock_config(), &OfflineProvider, &spec).unwrap();

        assert_eq!(value["method"], "erdos-728-sieve");
        // The returned value is the serialized FamilyReport; item count lives in
        // the `items` array (n_items is a method, absent from the JSON).
        assert_eq!(value["items"].as_array().unwrap().len(), 2);
        assert_eq!(
            value["n_solved"], 0,
            "no live gate offline means no false solve"
        );
        assert_eq!(value["n_failed"], 2);
        assert_eq!(value["coverage"], 0.0);
        // Dependency order is still honoured in the report.
        assert_eq!(value["order"][0], "q1");
        assert_eq!(value["order"][1], "q2");

        // The completion event landed, scoped to the project.
        let events = store.events(&project.id, 10).unwrap();
        assert!(events
            .iter()
            .any(|e| e.event_type == "method_transfer.completed"));
    }
}
