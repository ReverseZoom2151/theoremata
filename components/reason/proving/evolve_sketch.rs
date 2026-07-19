//! EVOLVE-BLOCK editable-sketch + Elo-ranked population evolutionary loop
//! (AlphaProof Nexus, agent configuration "C"/"D").
//!
//! The paper's core search object is a *proof sketch*: a Lean file with the
//! target theorem plus editable regions delimited by `EVOLVE-BLOCK` (a region a
//! mutator may rewrite — add lemmas / steps) and `EVOLVE-VALUE` (a scalar/choice
//! the agent tunes, so the object *and* its proof are co-discovered). Because a
//! formal-proof landscape is essentially binary (compiles / has `sorry`),
//! evolution needs a *graded* fitness signal: cheap rating agents produce
//! relative rankings of sketches which are aggregated into an **Elo** rating and
//! consumed by P-UCB / best-first selection. This module models exactly that
//! loop, deterministically and offline:
//!
//! * [`EditableSketch`] — a template of ordered [`Segment`]s: immutable literal
//!   prose interleaved with marked [`EvolveBlock`] / [`EvolveValue`] regions (the
//!   `EVOLVE-BLOCK` / `EVOLVE-VALUE` / `sorry`-hole analogue). It can be *parsed*
//!   from marked template text, *built* programmatically, or *bridged* from an
//!   [`InformalSketch`] (each hole becomes an editable block).
//! * [`Population`] — a set of [`EditableSketch`] variants whose fitness is an
//!   [`EloRanker`]; variants are scored by among-set comparison outcomes.
//! * [`evolve`] — the generational loop: rank the population into Elo, select the
//!   top-Elo parents, mutate one marked region of each (injected [`Mutator`]),
//!   evaluate the offspring (injected [`Evaluator`]), update Elo from the
//!   among-set ranking, cull the worst to hold the population size fixed, and
//!   terminate on a solved variant or `max_generations`.
//!
//! Determinism is total: no wall clock, no unseeded randomness. A single `u64`
//! seed in [`EvolveConfig`] is threaded through a splitmix64 mixer into every
//! mutation, so the same inputs always reproduce the same result. All region
//! content is treated as opaque data — it is only ever rewritten and rendered,
//! never executed here.

use crate::config::Config;
use crate::db::Store;
use crate::fitness::EloRanker;
use crate::model::ModelRequest;
use crate::prover::formal::{backend_for, FormalBackend, FormalSystem};
use crate::provider::ModelProvider;
use crate::sketch::InformalSketch;
use anyhow::Result;
use serde_json::json;
use std::cmp::Ordering;
use std::collections::HashMap;

/// An editable region that a mutator may freely rewrite (the `EVOLVE-BLOCK`
/// analogue — a place to add lemmas / definitions / proof steps).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvolveBlock {
    /// Stable, caller-supplied region id (the mutation target key).
    pub id: String,
    /// The current (rewritable) region body.
    pub content: String,
}

/// An editable scalar / choice the agent tunes (the `EVOLVE-VALUE` analogue —
/// e.g. an algorithm parameter searched jointly with its proof).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvolveValue {
    /// Stable, caller-supplied region id.
    pub id: String,
    /// The current value (held as opaque text; the domain interprets it).
    pub value: String,
}

/// One ordered piece of an [`EditableSketch`]: fixed prose or a marked region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// Immutable template text (never mutated; carried verbatim on render).
    Literal(String),
    /// An editable `EVOLVE-BLOCK` region.
    Block(EvolveBlock),
    /// An editable `EVOLVE-VALUE` region.
    Value(EvolveValue),
}

/// A borrowed view of an editable region, for ordered iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvolveRegion<'a> {
    Block(&'a EvolveBlock),
    Value(&'a EvolveValue),
}

/// Marker lines recognised by [`EditableSketch::parse`].
const BLOCK_START: &str = "-- EVOLVE-BLOCK ";
const BLOCK_END: &str = "-- END-EVOLVE-BLOCK";
const VALUE_MARK: &str = "-- EVOLVE-VALUE ";

/// A proof-sketch template: an ordered sequence of literal prose and marked
/// editable regions, plus a stable variant `id` used as its Elo key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditableSketch {
    /// Unique variant id (the [`EloRanker`] key). Reassigned per offspring.
    pub id: String,
    /// The target statement this sketch proves.
    pub statement: String,
    segments: Vec<Segment>,
}

impl EditableSketch {
    /// Start an empty template for `statement` under variant `id`.
    pub fn template(id: impl Into<String>, statement: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            statement: statement.into(),
            segments: Vec::new(),
        }
    }

    /// Builder: append a fixed (non-editable) literal segment.
    pub fn add_literal(mut self, text: impl Into<String>) -> Self {
        self.segments.push(Segment::Literal(text.into()));
        self
    }

    /// Builder: append an editable `EVOLVE-BLOCK` region.
    pub fn add_block(mut self, id: impl Into<String>, content: impl Into<String>) -> Self {
        self.segments.push(Segment::Block(EvolveBlock {
            id: id.into(),
            content: content.into(),
        }));
        self
    }

    /// Builder: append an editable `EVOLVE-VALUE` region.
    pub fn add_value(mut self, id: impl Into<String>, value: impl Into<String>) -> Self {
        self.segments.push(Segment::Value(EvolveValue {
            id: id.into(),
            value: value.into(),
        }));
        self
    }

    /// Return a clone tagged with a fresh variant `id` (used when an offspring is
    /// born so each population member has a distinct Elo key).
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Ordered iterator over just the editable regions.
    pub fn regions(&self) -> impl Iterator<Item = EvolveRegion<'_>> {
        self.segments.iter().filter_map(|s| match s {
            Segment::Block(b) => Some(EvolveRegion::Block(b)),
            Segment::Value(v) => Some(EvolveRegion::Value(v)),
            Segment::Literal(_) => None,
        })
    }

    /// Ordered iterator over the editable `EVOLVE-BLOCK` regions.
    pub fn blocks(&self) -> impl Iterator<Item = &EvolveBlock> {
        self.segments.iter().filter_map(|s| match s {
            Segment::Block(b) => Some(b),
            _ => None,
        })
    }

    /// Ordered iterator over the editable `EVOLVE-VALUE` regions.
    pub fn values(&self) -> impl Iterator<Item = &EvolveValue> {
        self.segments.iter().filter_map(|s| match s {
            Segment::Value(v) => Some(v),
            _ => None,
        })
    }

    /// The ids of the editable blocks, in order (mutation candidates).
    pub fn block_ids(&self) -> Vec<String> {
        self.blocks().map(|b| b.id.clone()).collect()
    }

    /// The block with `id`, if present.
    pub fn block(&self, id: &str) -> Option<&EvolveBlock> {
        self.blocks().find(|b| b.id == id)
    }

    /// The value region with `id`, if present.
    pub fn value(&self, id: &str) -> Option<&EvolveValue> {
        self.values().find(|v| v.id == id)
    }

    /// The current content of block `id`, if present.
    pub fn block_content(&self, id: &str) -> Option<&str> {
        self.block(id).map(|b| b.content.as_str())
    }

    /// Return a clone with block `id`'s content replaced (the primitive a
    /// [`Mutator`] uses to rewrite one marked region). Unknown ids are a no-op.
    pub fn with_block(&self, id: &str, content: impl Into<String>) -> Self {
        let mut next = self.clone();
        let content = content.into();
        for seg in &mut next.segments {
            if let Segment::Block(b) = seg {
                if b.id == id {
                    b.content = content;
                    break;
                }
            }
        }
        next
    }

    /// Return a clone with value region `id`'s value replaced. Unknown ids are a
    /// no-op.
    pub fn with_value(&self, id: &str, value: impl Into<String>) -> Self {
        let mut next = self.clone();
        let value = value.into();
        for seg in &mut next.segments {
            if let Segment::Value(v) = seg {
                if v.id == id {
                    v.value = value;
                    break;
                }
            }
        }
        next
    }

    /// Render the whole sketch back to marked template text (round-trips through
    /// [`EditableSketch::parse`], modulo interior blank-line normalisation).
    pub fn render(&self) -> String {
        let mut out: Vec<String> = Vec::new();
        for seg in &self.segments {
            match seg {
                Segment::Literal(t) => out.push(t.clone()),
                Segment::Block(b) => {
                    out.push(format!("{BLOCK_START}{}", b.id));
                    out.push(b.content.clone());
                    out.push(BLOCK_END.to_string());
                }
                Segment::Value(v) => {
                    out.push(format!("{VALUE_MARK}{} = {}", v.id, v.value));
                }
            }
        }
        out.join("\n")
    }

    /// Parse marked template text into an ordered [`EditableSketch`].
    ///
    /// Recognised markers (each on its own line):
    /// * `-- EVOLVE-BLOCK <id>` … `-- END-EVOLVE-BLOCK` — an editable block; the
    ///   lines in between (joined by `\n`) become its content.
    /// * `-- EVOLVE-VALUE <id> = <value>` — an editable value.
    ///
    /// Every other line is fixed literal prose. Input is treated as opaque data.
    pub fn parse(id: impl Into<String>, statement: impl Into<String>, template: &str) -> Self {
        let mut segments: Vec<Segment> = Vec::new();
        let mut literal: Vec<&str> = Vec::new();
        let flush = |literal: &mut Vec<&str>, segments: &mut Vec<Segment>| {
            if !literal.is_empty() {
                segments.push(Segment::Literal(literal.join("\n")));
                literal.clear();
            }
        };
        let mut lines = template.lines().peekable();
        while let Some(line) = lines.next() {
            let trimmed = line.trim_start();
            if let Some(bid) = trimmed.strip_prefix(BLOCK_START) {
                flush(&mut literal, &mut segments);
                let mut body: Vec<&str> = Vec::new();
                for inner in lines.by_ref() {
                    if inner.trim_start().starts_with(BLOCK_END) {
                        break;
                    }
                    body.push(inner);
                }
                segments.push(Segment::Block(EvolveBlock {
                    id: bid.trim().to_string(),
                    content: body.join("\n"),
                }));
            } else if let Some(rest) = trimmed.strip_prefix(VALUE_MARK) {
                flush(&mut literal, &mut segments);
                let (vid, value) = match rest.split_once('=') {
                    Some((k, v)) => (k.trim().to_string(), v.trim().to_string()),
                    None => (rest.trim().to_string(), String::new()),
                };
                segments.push(Segment::Value(EvolveValue { id: vid, value }));
            } else {
                literal.push(line);
            }
        }
        flush(&mut literal, &mut segments);
        Self {
            id: id.into(),
            statement: statement.into(),
            segments,
        }
    }

    /// Bridge an [`InformalSketch`] into an editable sketch: each *hole* step
    /// becomes an editable [`EvolveBlock`] (its subgoal is the initial content —
    /// the `sorry`-hole a mutator will fill), and each prose step becomes fixed
    /// literal text. This is the direct analogue of the sketch→holes decomposition
    /// in [`crate::sketch`], made evolvable.
    pub fn from_sketch(id: impl Into<String>, sketch: &InformalSketch) -> Self {
        let mut out = Self::template(id, sketch.statement.clone())
            .add_literal(format!("-- Proof of: {}", sketch.statement));
        for step in &sketch.steps {
            out = out.add_literal(format!("-- Step {}: {}", step.id, step.prose));
            if let Some(hole) = &step.hole {
                out = out.add_block(step.id.clone(), hole.subgoal.clone());
            }
        }
        out
    }
}

/// The graded result of evaluating one variant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Outcome {
    /// The variant is a complete, verified proof (a terminal success).
    pub solved: bool,
    /// Partial-progress score in `[0, 1]` for incomplete variants — the graded
    /// signal that turns the binary proof landscape into something rankable.
    pub progress: f64,
}

impl Outcome {
    /// A solved variant (full credit).
    pub fn solved() -> Self {
        Self {
            solved: true,
            progress: 1.0,
        }
    }

    /// An unsolved variant with the given partial-progress score.
    pub fn progress(progress: f64) -> Self {
        Self {
            solved: false,
            progress,
        }
    }

    /// An unsolved variant with no measurable progress.
    pub fn none() -> Self {
        Self::progress(0.0)
    }
}

/// Better-first comparison of two outcomes: solved beats unsolved; among equals,
/// higher progress wins. Total (NaN progress sorts as equal) and deterministic.
fn outcome_cmp(a: &Outcome, b: &Outcome) -> Ordering {
    a.solved.cmp(&b.solved).then_with(|| {
        a.progress
            .partial_cmp(&b.progress)
            .unwrap_or(Ordering::Equal)
    })
}

/// Rewrites exactly one marked region of a sketch (the LLM prover-subagent seam).
/// Injected so the loop runs deterministically with mocks; `seed` is threaded in
/// so an implementation can be pseudo-random yet fully reproducible.
pub trait Mutator {
    /// Return a new sketch with block `block_id` rewritten. The returned sketch's
    /// `id` is ignored by [`evolve`] (a fresh id is assigned to every offspring).
    fn mutate(&self, s: &EditableSketch, block_id: &str, seed: u64) -> EditableSketch;
}

/// Scores a sketch variant (the rating-agent / verifier seam). Injected so the
/// loop is deterministic under test.
pub trait Evaluator {
    fn evaluate(&self, s: &EditableSketch) -> Outcome;
}

/// A population of sketch variants whose fitness is an Elo rating updated from
/// among-set comparison outcomes.
#[derive(Debug, Clone)]
pub struct Population {
    members: Vec<EditableSketch>,
    ranker: EloRanker,
}

impl Population {
    /// A population seeded with `members` and a fresh [`EloRanker`].
    pub fn new(members: Vec<EditableSketch>) -> Self {
        Self {
            members,
            ranker: EloRanker::default(),
        }
    }

    /// The number of current members.
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Whether the population is empty.
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// The current members (unordered).
    pub fn members(&self) -> &[EditableSketch] {
        &self.members
    }

    /// The current Elo rating of variant `id`.
    pub fn rating(&self, id: &str) -> f64 {
        self.ranker.rating(id)
    }

    /// Add a member (e.g. a freshly born offspring).
    pub fn add(&mut self, member: EditableSketch) {
        self.members.push(member);
    }

    /// The member with `id`, if present.
    pub fn get(&self, id: &str) -> Option<&EditableSketch> {
        self.members.iter().find(|m| m.id == id)
    }

    /// Evaluate every member, fold the resulting best-first ordering into Elo (a
    /// Plackett-Luce style among-set update), and return each `(id, outcome)`.
    /// Ties break by id so the update — and thus every rating — is deterministic.
    pub fn rank_by<E: Evaluator>(&mut self, evaluator: &E) -> Vec<(String, Outcome)> {
        let mut scored: Vec<(usize, Outcome)> = self
            .members
            .iter()
            .enumerate()
            .map(|(i, m)| (i, evaluator.evaluate(m)))
            .collect();
        // Best-first: better outcome first, ties broken by id.
        scored.sort_by(|a, b| {
            outcome_cmp(&b.1, &a.1).then_with(|| self.members[a.0].id.cmp(&self.members[b.0].id))
        });
        let order: Vec<&str> = scored
            .iter()
            .map(|(i, _)| self.members[*i].id.as_str())
            .collect();
        self.ranker.record_ranking(&order);
        scored
            .iter()
            .map(|(i, o)| (self.members[*i].id.clone(), *o))
            .collect()
    }

    /// Members ordered best-first by current Elo (ties by id — deterministic).
    pub fn elo_order(&self) -> Vec<&EditableSketch> {
        let rank = self.ranker.ranking();
        let pos: HashMap<&str, usize> = rank
            .iter()
            .enumerate()
            .map(|(i, (id, _))| (id.as_str(), i))
            .collect();
        let mut ms: Vec<&EditableSketch> = self.members.iter().collect();
        ms.sort_by(|a, b| {
            let pa = pos.get(a.id.as_str()).copied().unwrap_or(usize::MAX);
            let pb = pos.get(b.id.as_str()).copied().unwrap_or(usize::MAX);
            pa.cmp(&pb).then_with(|| a.id.cmp(&b.id))
        });
        ms
    }

    /// The top-Elo member, if any.
    pub fn best(&self) -> Option<&EditableSketch> {
        self.elo_order().into_iter().next()
    }

    /// Cull the worst so exactly `n` best-Elo members remain (a no-op if already
    /// `<= n`). The retained order is best-first.
    pub fn cull_to(&mut self, n: usize) {
        if self.members.len() <= n {
            return;
        }
        let keep: Vec<String> = self
            .elo_order()
            .into_iter()
            .take(n)
            .map(|m| m.id.clone())
            .collect();
        let mut kept: Vec<EditableSketch> = Vec::with_capacity(keep.len());
        for id in &keep {
            if let Some(m) = self.members.iter().find(|m| &m.id == id) {
                kept.push(m.clone());
            }
        }
        self.members = kept;
    }
}

/// Tuning knobs for [`evolve`]. `seed` makes every run reproducible.
#[derive(Debug, Clone)]
pub struct EvolveConfig {
    /// Fixed population size held across generations.
    pub population_size: usize,
    /// Hard cap on generations (a solved variant terminates earlier).
    pub max_generations: u32,
    /// How many top-Elo parents reproduce each generation.
    pub parents_per_gen: usize,
    /// How many offspring each selected parent produces.
    pub offspring_per_parent: usize,
    /// Master RNG seed threaded into every mutation (determinism).
    pub seed: u64,
}

impl Default for EvolveConfig {
    fn default() -> Self {
        Self {
            population_size: 6,
            max_generations: 40,
            parents_per_gen: 2,
            offspring_per_parent: 2,
            seed: 0,
        }
    }
}

/// The outcome of an [`evolve`] run.
#[derive(Debug, Clone)]
pub struct EvolveResult {
    /// The best variant found (the solved one if any, else top-Elo).
    pub best: EditableSketch,
    /// The best variant's final Elo rating.
    pub best_elo: f64,
    /// Whether a fully solved variant was reached.
    pub solved: bool,
    /// Generations actually run (0 if the seed population already solved it).
    pub generations: u32,
    /// The final population size (invariant: equals `config.population_size`).
    pub population_size: usize,
}

/// splitmix64 finaliser — a deterministic scalar mixer (no external rand crate).
fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Mix four coordinates (master seed, generation, parent index, offspring index)
/// into one reproducible per-mutation seed.
fn mix4(a: u64, b: u64, c: u64, d: u64) -> u64 {
    splitmix64(
        splitmix64(splitmix64(splitmix64(a).wrapping_add(b)).wrapping_add(c)).wrapping_add(d),
    )
}

/// Deterministically choose which block of `parent` to mutate, from `seed`.
/// `None` when the parent has no editable blocks.
fn choose_block(parent: &EditableSketch, seed: u64) -> Option<String> {
    let ids = parent.block_ids();
    if ids.is_empty() {
        return None;
    }
    let idx = (seed % ids.len() as u64) as usize;
    Some(ids[idx].clone())
}

/// Mutate one region of `parent` (chosen deterministically from `seed`), or clone
/// it unchanged if there is nothing editable.
fn mutate_one<M: Mutator>(parent: &EditableSketch, seed: u64, mutator: &M) -> EditableSketch {
    match choose_block(parent, seed) {
        Some(bid) => mutator.mutate(parent, &bid, seed),
        None => parent.clone(),
    }
}

/// Build a [`EvolveResult`] for a solved variant.
fn solved_result(pop: &Population, id: &str, generations: u32) -> EvolveResult {
    let best = pop
        .get(id)
        .cloned()
        .unwrap_or_else(|| pop.best().cloned().expect("non-empty population"));
    EvolveResult {
        best_elo: pop.rating(id),
        best,
        solved: true,
        generations,
        population_size: pop.len(),
    }
}

/// Run the EVOLVE-BLOCK editable-sketch evolutionary loop.
///
/// Starting from the `seed` sketch, the population is filled to
/// `config.population_size` by mutation, then each generation:
/// 1. selects the top-Elo parents,
/// 2. mutates one marked region of each to spawn offspring (each a fresh variant
///    id, seeded deterministically),
/// 3. evaluates the combined pool and folds the among-set ranking into Elo,
/// 4. culls the worst so the population size stays fixed.
///
/// It terminates as soon as a variant is [`Outcome::solved`], or after
/// `config.max_generations`. Deterministic given `config.seed`, the mutator, and
/// the evaluator. Returns the best variant, its Elo, whether it was solved, and
/// the generation count.
pub fn evolve<M: Mutator, E: Evaluator>(
    seed: EditableSketch,
    config: &EvolveConfig,
    mutator: &M,
    evaluator: &E,
) -> EvolveResult {
    let n = config.population_size.max(1);
    let parents_k = config.parents_per_gen.max(1);
    let offspring_k = config.offspring_per_parent.max(1);

    // Seed population: the seed variant plus mutated copies up to size `n`.
    let mut members = vec![seed.clone()];
    for i in 1..n {
        let s = mix4(config.seed, 0, i as u64, 0);
        let child = mutate_one(&seed, s, mutator).with_id(format!("init-{i}"));
        members.push(child);
    }
    let mut pop = Population::new(members);

    // Bootstrap ranking so Elo is meaningful before the first selection.
    let bootstrap = pop.rank_by(evaluator);
    if let Some((id, _)) = bootstrap.iter().find(|(_, o)| o.solved) {
        return solved_result(&pop, id, 0);
    }
    pop.cull_to(n);

    for gen in 1..=config.max_generations {
        // 1. Select top-Elo parents (clone out so we can mutate the population).
        let parents: Vec<EditableSketch> = pop
            .elo_order()
            .into_iter()
            .take(parents_k)
            .cloned()
            .collect();

        // 2. Mutate one marked region of each parent to spawn offspring.
        for (pi, parent) in parents.iter().enumerate() {
            for k in 0..offspring_k {
                let s = mix4(config.seed, gen as u64, pi as u64, k as u64);
                let child = mutate_one(parent, s, mutator).with_id(format!("g{gen}-p{pi}-o{k}"));
                pop.add(child);
            }
        }

        // 3. Evaluate the combined pool and fold the ranking into Elo.
        let scored = pop.rank_by(evaluator);
        let solved = scored
            .iter()
            .find(|(_, o)| o.solved)
            .map(|(id, _)| id.clone());

        // 4. Cull the worst back to the fixed population size.
        pop.cull_to(n);

        if let Some(id) = solved {
            return solved_result(&pop, &id, gen);
        }
    }

    // Exhausted the generation budget without a solved variant.
    let best = pop.best().cloned().expect("non-empty population");
    let best_elo = pop.rating(&best.id);
    EvolveResult {
        best,
        best_elo,
        solved: false,
        generations: config.max_generations,
        population_size: pop.len(),
    }
}

// ---------------------------------------------------------------------------
// CLI entry point: the production seams for the evolutionary loop
// ---------------------------------------------------------------------------

/// [`Mutator`] backed by the model provider. This is the production
/// prover-subagent that rewrites one marked region. Any provider or parse
/// failure degrades to an
/// unchanged clone rather than fabricating a mutation, so a broken model can
/// never manufacture progress it did not produce.
struct ModelMutator<'a> {
    provider: &'a dyn ModelProvider,
}

impl Mutator for ModelMutator<'_> {
    fn mutate(&self, s: &EditableSketch, block_id: &str, seed: u64) -> EditableSketch {
        let current = s.block_content(block_id).unwrap_or("");
        // The current region body is prior model output; fence it as data.
        let request = ModelRequest {
            role: "evolve_sketch_block".into(),
            task: "Rewrite ONLY the marked proof-sketch region so the whole sketch moves \
                   toward a complete, machine-checkable proof of the target statement. \
                   Return the new region body verbatim; do not restate the target."
                .into(),
            context: json!({
                "target": s.statement,
                "block_id": block_id,
                "current": crate::guard::wrap_untrusted("sketch_block", current),
                "seed": seed,
            }),
            output_schema: json!({
                "type": "object",
                "required": ["content"],
                "properties": {"content": {"type": "string"}}
            }),
        };
        match self.provider.complete(&request) {
            Ok(resp) => match resp.content.get("content").and_then(|v| v.as_str()) {
                Some(next) => s.with_block(block_id, next),
                None => s.clone(),
            },
            Err(_) => s.clone(),
        }
    }
}

/// [`Evaluator`] backed by the real formal verifier. A variant counts as
/// [`Outcome::solved`] ONLY when a LIVE backend passes every gate layer on the
/// rendered sketch; a mock backend (or an unavailable / erroring toolchain) never
/// yields a solve. Partial progress is deliberately reported as `0.0`: there is no
/// sound cheap grade for an incomplete formal proof, so we refuse to invent one.
/// The Elo ranking still orders solved above unsolved.
struct BackendEvaluator<'a> {
    config: &'a Config,
    system: FormalSystem,
}

impl Evaluator for BackendEvaluator<'_> {
    fn evaluate(&self, s: &EditableSketch) -> Outcome {
        let code = s.render();
        // Honour offline mode: `prover_mock` builds the mock backend, which reports
        // `live: false` and therefore can never be mistaken for certification.
        let backend = backend_for(self.config, self.system, self.config.prover_mock);
        match backend.verify(self.config, &code, &s.statement) {
            Ok(report) if report.live && report.lexically_verified => Outcome::solved(),
            _ => Outcome::none(),
        }
    }
}

/// Run the EVOLVE-BLOCK editable-sketch loop against live seams: the model
/// provider mutates marked regions, the real formal backend judges each variant.
///
/// `template` is marked sketch text (see [`EditableSketch::parse`]); `statement`
/// is the target it must prove. Returns a JSON summary of the run. `solved` is
/// true only when a live backend certified the best variant, so it is honest in
/// offline / mock mode (always false there). This stage never sets any node status
/// to verified; certification remains the formal gate's sole responsibility, hence
/// the explicit `"certified": false` in the summary. Emits an `evolve_sketch.*`
/// run trace to the store.
pub fn evolve_proof_sketch(
    store: &Store,
    config: &Config,
    provider: &dyn ModelProvider,
    project_id: &str,
    statement: &str,
    template: &str,
    evolve_config: &EvolveConfig,
) -> Result<serde_json::Value> {
    let run = store.begin_run(project_id, "evolve_sketch")?;

    let seed = EditableSketch::parse("seed", statement, template);
    let mutator = ModelMutator { provider };
    let evaluator = BackendEvaluator {
        config,
        system: config.target_system,
    };
    let result = evolve(seed, evolve_config, &mutator, &evaluator);

    store.event(
        Some(project_id),
        Some(&run),
        "evolve_sketch.completed",
        "evolve_sketch",
        json!({
            "solved": result.solved,
            "generations": result.generations,
            "population_size": result.population_size,
            "best_variant_id": result.best.id,
        }),
    )?;
    let state = if result.solved {
        "completed"
    } else {
        "completed_unsolved"
    };
    store.update_run(project_id, &run, state, "complete", result.generations)?;

    Ok(json!({
        "run_id": run,
        "statement": statement,
        // `solved`/`live_verified` are true only after a real live backend passed
        // the 3+1 gate on the best variant; false in offline / mock mode.
        "solved": result.solved,
        "live_verified": result.solved,
        // This stage screens and evolves; it never certifies a graph node.
        "certified": false,
        "generations": result.generations,
        "population_size": result.population_size,
        "best_elo": result.best_elo,
        "best_variant_id": result.best.id,
        "best_render": result.best.render(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sketch::{InformalSketch, SketchStep};

    /// Longest common char-prefix length of `a` and `b`.
    fn common_prefix(a: &str, b: &str) -> usize {
        a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
    }

    /// A deterministic mock mutator that reveals one more character of `target`
    /// in the chosen block — a monotone climb toward the solved content. It
    /// ignores `seed` (the reveal is content-driven), which keeps the test's
    /// convergence exactly predictable.
    struct RevealMutator {
        target: String,
    }
    impl Mutator for RevealMutator {
        fn mutate(&self, s: &EditableSketch, block_id: &str, _seed: u64) -> EditableSketch {
            let cur = s.block_content(block_id).unwrap_or("");
            let have = common_prefix(cur, &self.target);
            let next_len = (have + 1).min(self.target.chars().count());
            let next: String = self.target.chars().take(next_len).collect();
            s.with_block(block_id, next)
        }
    }

    /// Rewards a block whose content matches (a prefix of) `target`. Solved when
    /// the content equals `target`; otherwise graded by matching-prefix fraction.
    struct PrefixEvaluator {
        block_id: String,
        target: String,
    }
    impl Evaluator for PrefixEvaluator {
        fn evaluate(&self, s: &EditableSketch) -> Outcome {
            let c = s.block_content(&self.block_id).unwrap_or("");
            if c == self.target {
                Outcome::solved()
            } else {
                let frac =
                    common_prefix(c, &self.target) as f64 / self.target.chars().count() as f64;
                Outcome::progress(frac)
            }
        }
    }

    fn seed_sketch() -> EditableSketch {
        EditableSketch::template("seed", "P holds")
            .add_literal("-- Proof of: P holds")
            .add_block("goal", "")
    }

    #[test]
    fn converges_to_solved_within_budget() {
        let seed = seed_sketch();
        let config = EvolveConfig {
            population_size: 4,
            max_generations: 12,
            parents_per_gen: 2,
            offspring_per_parent: 2,
            seed: 42,
        };
        let mutator = RevealMutator {
            target: "qed".into(),
        };
        let evaluator = PrefixEvaluator {
            block_id: "goal".into(),
            target: "qed".into(),
        };
        let result = evolve(seed, &config, &mutator, &evaluator);

        assert!(
            result.solved,
            "the reveal mutator must reach the solved content"
        );
        // "qed" needs 3 reveals; convergence well within the 12-generation budget.
        assert!(
            result.generations <= 5,
            "converged in {} gens",
            result.generations
        );
        assert_eq!(result.best.block_content("goal"), Some("qed"));
        assert_eq!(result.population_size, 4, "population size stays fixed");
    }

    #[test]
    fn elo_ranking_orders_better_variants_above_worse() {
        // Two static variants: `hi` always out-progresses `lo`.
        struct FixedEvaluator;
        impl Evaluator for FixedEvaluator {
            fn evaluate(&self, s: &EditableSketch) -> Outcome {
                match s.id.as_str() {
                    "hi" => Outcome::progress(0.9),
                    _ => Outcome::progress(0.1),
                }
            }
        }
        let hi = EditableSketch::template("hi", "t").add_block("b", "x");
        let lo = EditableSketch::template("lo", "t").add_block("b", "y");
        let mut pop = Population::new(vec![lo, hi]);
        for _ in 0..4 {
            pop.rank_by(&FixedEvaluator);
        }
        assert!(
            pop.rating("hi") > pop.rating("lo"),
            "the consistently-better variant must out-rate the worse one"
        );
        assert_eq!(pop.best().unwrap().id, "hi");
        assert_eq!(pop.elo_order()[0].id, "hi");
    }

    #[test]
    fn population_size_stays_fixed_across_generations() {
        // A mutator that keeps producing distinct content (never solves), so every
        // generation adds offspring that must be culled back to `n`.
        struct GrowMutator;
        impl Mutator for GrowMutator {
            fn mutate(&self, s: &EditableSketch, block_id: &str, seed: u64) -> EditableSketch {
                let cur = s.block_content(block_id).unwrap_or("");
                s.with_block(block_id, format!("{cur}{seed}."))
            }
        }
        // Progress rises with content length but never reaches a solved state.
        struct LengthEvaluator;
        impl Evaluator for LengthEvaluator {
            fn evaluate(&self, s: &EditableSketch) -> Outcome {
                let len = s.block_content("goal").map(|c| c.len()).unwrap_or(0);
                Outcome::progress((len as f64) / 1000.0)
            }
        }
        let seed = seed_sketch();
        let config = EvolveConfig {
            population_size: 5,
            max_generations: 6,
            parents_per_gen: 3,
            offspring_per_parent: 2,
            seed: 7,
        };
        let result = evolve(seed, &config, &GrowMutator, &LengthEvaluator);
        assert!(!result.solved);
        assert_eq!(result.generations, 6);
        assert_eq!(result.population_size, 5, "culling holds the size fixed");
    }

    #[test]
    fn deterministic_same_seed_same_result() {
        let mutator = RevealMutator {
            target: "abcd".into(),
        };
        let evaluator = PrefixEvaluator {
            block_id: "goal".into(),
            target: "abcd".into(),
        };
        let config = EvolveConfig {
            population_size: 5,
            max_generations: 15,
            parents_per_gen: 2,
            offspring_per_parent: 3,
            seed: 123,
        };
        let run = || evolve(seed_sketch(), &config, &mutator, &evaluator);
        let a = run();
        let b = run();
        assert_eq!(a.best.id, b.best.id);
        assert_eq!(a.best.render(), b.best.render());
        assert_eq!(a.solved, b.solved);
        assert_eq!(a.generations, b.generations);
        assert_eq!(a.best_elo.to_bits(), b.best_elo.to_bits());
    }

    #[test]
    fn no_improvement_terminates_at_max_generations() {
        // A mutator that changes nothing and an evaluator that never solves: the
        // loop must run to the budget and return cleanly (no panic).
        struct IdentityMutator;
        impl Mutator for IdentityMutator {
            fn mutate(&self, s: &EditableSketch, _block_id: &str, _seed: u64) -> EditableSketch {
                s.clone()
            }
        }
        struct NeverSolves;
        impl Evaluator for NeverSolves {
            fn evaluate(&self, _s: &EditableSketch) -> Outcome {
                Outcome::progress(0.25)
            }
        }
        let config = EvolveConfig {
            population_size: 3,
            max_generations: 8,
            parents_per_gen: 2,
            offspring_per_parent: 2,
            seed: 99,
        };
        let result = evolve(seed_sketch(), &config, &IdentityMutator, &NeverSolves);
        assert!(!result.solved);
        assert_eq!(result.generations, 8);
        assert_eq!(result.population_size, 3);
    }

    #[test]
    fn parse_extracts_ordered_blocks_and_values() {
        let template = "\
theorem foo : True
-- EVOLVE-VALUE k = 3
-- EVOLVE-BLOCK lemma1
have h : True := trivial
-- END-EVOLVE-BLOCK
-- closing
-- EVOLVE-BLOCK main
exact h
-- END-EVOLVE-BLOCK";
        let s = EditableSketch::parse("v1", "True", template);
        let block_ids: Vec<String> = s.blocks().map(|b| b.id.clone()).collect();
        assert_eq!(block_ids, vec!["lemma1", "main"]);
        assert_eq!(s.value("k").map(|v| v.value.as_str()), Some("3"));
        assert_eq!(s.block_content("lemma1"), Some("have h : True := trivial"));
        assert_eq!(s.block_content("main"), Some("exact h"));
        // Ordered regions interleave value then blocks in source order.
        let region_ids: Vec<&str> = s
            .regions()
            .map(|r| match r {
                EvolveRegion::Block(b) => b.id.as_str(),
                EvolveRegion::Value(v) => v.id.as_str(),
            })
            .collect();
        assert_eq!(region_ids, vec!["k", "lemma1", "main"]);
    }

    // --- entry point (offline: mock provider + mock backend) -----------------

    use crate::model::ModelResponse;
    use std::path::Path;

    /// A mock provider that rewrites a block to a fixed body. It never claims a
    /// proof; the (mock) backend is what decides `solved`, so this exercises the
    /// entry wiring without any live toolchain.
    struct RewriteProvider;
    impl ModelProvider for RewriteProvider {
        fn complete(&self, request: &ModelRequest) -> Result<ModelResponse> {
            let content = match request.role.as_str() {
                "evolve_sketch_block" => json!({"content": "exact rfl"}),
                _ => json!({}),
            };
            Ok(ModelResponse {
                content,
                model: "test".into(),
                provider: "test".into(),
            })
        }
        fn name(&self) -> &str {
            "test"
        }
    }

    #[test]
    fn entry_point_runs_offline_and_never_reports_a_mock_solve() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "P holds").unwrap();

        // Mock backend: offline, so a variant can never be reported as certified.
        let mut config = Config {
            prover_mock: true,
            ..Config::default()
        };
        config.workspace = temp.path().join("workspaces");

        let template = "-- Proof of: P holds\n-- EVOLVE-BLOCK goal\nsorry\n-- END-EVOLVE-BLOCK";
        let evolve_config = EvolveConfig {
            population_size: 3,
            max_generations: 2,
            parents_per_gen: 2,
            offspring_per_parent: 2,
            seed: 1,
        };

        let summary = evolve_proof_sketch(
            &store,
            &config,
            &RewriteProvider,
            &project.id,
            "P holds",
            template,
            &evolve_config,
        )
        .unwrap();

        // Offline runs must never claim a solve or a certification.
        assert_eq!(summary["solved"], serde_json::json!(false));
        assert_eq!(summary["live_verified"], serde_json::json!(false));
        assert_eq!(summary["certified"], serde_json::json!(false));
        // The loop ran to its budget and returned a real best variant.
        assert_eq!(summary["generations"], serde_json::json!(2));
        assert_eq!(summary["population_size"], serde_json::json!(3));
        assert!(summary["best_render"].as_str().unwrap().contains("P holds"));

        // The run was traced to the store.
        let events = store.events(&project.id, 50).unwrap();
        assert!(events
            .iter()
            .any(|e| e.event_type == "evolve_sketch.completed"));
    }

    #[test]
    fn bridges_an_informal_sketch_into_editable_blocks() {
        // Each hole-bearing step of an InformalSketch becomes an editable block.
        let sketch = InformalSketch::new(
            "P holds for all n",
            vec![
                SketchStep::hole("s1", "base case", "base : P 0"),
                SketchStep::prose("s2", "conclude by induction"),
                SketchStep::hole("s3", "step", "step : forall n, P n -> P (n+1)"),
            ],
        );
        let editable = EditableSketch::from_sketch("from-sketch", &sketch);
        let block_ids: Vec<String> = editable.blocks().map(|b| b.id.clone()).collect();
        assert_eq!(block_ids, vec!["s1", "s3"], "only holes are editable");
        assert_eq!(editable.block_content("s1"), Some("base : P 0"));
        // The prose step survives as fixed literal text in the render.
        let rendered = editable.render();
        assert!(rendered.contains("conclude by induction"));
        assert!(rendered.contains("P holds for all n"));
    }
}
