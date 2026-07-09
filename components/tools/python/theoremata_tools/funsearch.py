"""FunSearch-style program-search / discovery engine (DeepMind FunSearch).

A *discovery* mode distinct from our LEGO-Prover-style lemma evolver
(``components/reason/proving/library.rs``, which grows *verified lemmas*). Here
we evolve **programs** -- text that *constructs* or *scores* a mathematical
object -- against an automated **evaluator**, to discover constructions, bounds
or heuristics for open problems. The loop mirrors the FunSearch recipe
(https://deepmind.google/blog/funsearch-...): an LLM proposes programs, an
evaluator runs them and keeps only the passing ones (hallucination-proof), an
**island** population model maintains diversity, **best-shot** prompting feeds
the top-scoring programs back into the prompt, and selection is biased toward
**short / low-Kolmogorov-complexity** programs.

Safety boundary (mirrors :mod:`.falsify`). Every candidate program string is
treated as **UNTRUSTED DATA**. This module NEVER ``exec``/``eval``/``compile``s
a candidate in-process -- there is no code path that does. Execution happens
*only* inside the injected :class:`Evaluator` seam: in tests a pure-Python mock
that reads the program as data; in production a sandbox that runs the candidate
in a hard-killed child process exactly like :func:`falsify.search`. The
generator and evaluator are the *only* seams that ever touch program text as
anything other than an opaque string.

Determinism. All randomness is seeded from a single integer carried IN via the
request/config -- no unseeded RNG, no wall-clock, no network. The same request
yields the same ``best_program``.
"""
from __future__ import annotations

import abc
import random
import re
from dataclasses import dataclass, field
from typing import Any, Callable, Optional

# --------------------------------------------------------------------------- #
# Injected seams
# --------------------------------------------------------------------------- #


class ProgramGenerator(abc.ABC):
    """The proposer seam: ``generate(prompt, seed) -> new_program_str``.

    Contract: given a *best-shot* prompt (assembled from the top-scoring
    programs of an island, as strings) and an integer ``seed``, return a new
    candidate program as a **string**. Must be deterministic in ``(prompt,
    seed)``. In tests this is a pure mock; in production it is an LLM
    (PaLM 2 / Gemini in the original FunSearch) prompted with the best programs.
    The returned string is opaque, untrusted data to this module.
    """

    @abc.abstractmethod
    def generate(self, prompt: str, seed: int) -> str:
        raise NotImplementedError


class Evaluator(abc.ABC):
    """The trust anchor seam: ``evaluate(program, problem_spec) -> {score, valid}``.

    Contract: run/inspect ``program`` (a string) against ``problem_spec`` and
    return a dict with a numeric ``score`` (higher is better) and a boolean
    ``valid``. Only ``valid`` programs enter the database (keep-only-passing),
    so hallucinated / non-running programs cannot poison the population. Must be
    deterministic. **This is the only place a candidate program is ever
    executed.** In tests: a pure-Python mock scoring against a hidden target. In
    production: a sandbox that runs the candidate in a hard-killed subprocess
    (see :func:`falsify.search`) against held-out test cases.
    """

    @abc.abstractmethod
    def evaluate(self, program: str, problem_spec: dict[str, Any]) -> dict[str, Any]:
        raise NotImplementedError


# --------------------------------------------------------------------------- #
# Program records + complexity-biased ranking
# --------------------------------------------------------------------------- #


@dataclass(frozen=True)
class Program:
    """A scored candidate program. ``length`` is a Kolmogorov-complexity proxy
    (token count) used as the tie-break; ``uid`` is a stable insertion-order id
    for a fully deterministic final tie-break."""

    code: str
    score: float
    valid: bool
    length: int
    uid: int

    @staticmethod
    def make(code: str, score: float, valid: bool, uid: int) -> "Program":
        return Program(
            code=code,
            score=float(score),
            valid=bool(valid),
            length=_token_length(code),
            uid=uid,
        )


def _token_length(code: str) -> int:
    """Kolmogorov proxy: number of whitespace/punctuation-delimited tokens.

    Falls back to character count for token-free strings so *some* length
    signal always exists for the complexity bias."""
    tokens = re.findall(r"\w+", code)
    return len(tokens) if tokens else len(code)


def _rank_key(p: Program) -> tuple[float, int, int]:
    """Sort key implementing the **complexity bias**: primary = highest score;
    on a tie prefer the **shorter** program (lower length = lower complexity);
    final tie-break = older program (lower uid) for determinism. Used with
    ``min`` / ``sorted`` (all ascending), so we negate score."""
    return (-p.score, p.length, p.uid)


def rank_programs(programs: list[Program]) -> list[Program]:
    """Return ``programs`` best-first under the complexity-biased order."""
    return sorted(programs, key=_rank_key)


# --------------------------------------------------------------------------- #
# Island model
# --------------------------------------------------------------------------- #


@dataclass
class Island:
    """One evolutionary sub-population. Islands evolve independently and only
    exchange members at migration, so a stuck/bad island cannot collapse the
    whole run -- the global best is a max over islands."""

    index: int
    capacity: int
    members: list[Program] = field(default_factory=list)

    def add(self, program: Program) -> None:
        """Admit a program, then cap the population to the top ``capacity`` by
        the complexity-biased ranking (selection pressure)."""
        self.members.append(program)
        if len(self.members) > self.capacity:
            self.members = rank_programs(self.members)[: self.capacity]

    def best(self) -> Optional[Program]:
        if not self.members:
            return None
        return min(self.members, key=_rank_key)

    def top_k(self, k: int) -> list[Program]:
        return rank_programs(self.members)[:k]

    def signature(self) -> frozenset[str]:
        """The set of distinct program texts held -- used by tests to observe
        that islands maintain diversity (do not collapse into one another)."""
        return frozenset(m.code for m in self.members)


# --------------------------------------------------------------------------- #
# MAP-Elites archive (AlphaEvolve, DeepMind 2025)
# --------------------------------------------------------------------------- #
#
# FunSearch's successor, AlphaEvolve, layers a MAP-Elites archive over the
# island idea: instead of (only) capped island populations, programs are binned
# by a low-dimensional *behavior descriptor* and each cell keeps a single elite
# (highest-scoring valid program, ties broken by the same complexity-biased
# ``_rank_key``). This maintains diversity across the *descriptor space*, not
# just across islands -- distinct behaviors are preserved even when one behavior
# scores higher, so the search does not collapse onto a single mode.


#: The behavior-descriptor seam: ``(program, evaluator_result) -> cell_key``.
#: Maps a program to a low-dimensional, hashable behavior cell. Injected so a
#: caller can define the diversity axes; the default is
#: :func:`default_behavior_descriptor`.
BehaviorDescriptor = Callable[[str, dict[str, Any]], tuple]


def default_behavior_descriptor(program: str, evaluator_result: dict[str, Any]) -> tuple:
    """A deterministic default descriptor derived from the program (and, when
    present, a coarse ``value`` feature from the evaluator result).

    Cell = ``(length_bucket, value_bucket)`` where ``length_bucket`` bins the
    Kolmogorov proxy (token count) and ``value_bucket`` bins any integer feature
    the evaluator surfaced (e.g. the mock evaluator's ``value``), falling back to
    the first integer found in the program text. Coarse bins keep the archive
    low-dimensional so distinct behaviors share cells sensibly. No execution --
    the program is read as data only.
    """
    length_bucket = _token_length(program) // 4
    feature = evaluator_result.get("value")
    if feature is None:
        m = _INT_RE.search(program or "")
        feature = int(m.group()) if m else 0
    value_bucket = int(feature) // 5
    return (length_bucket, value_bucket)


class MapElitesArchive:
    """A grid keyed by a behavior descriptor; each cell holds one elite.

    Admission (``add``) places a valid program in its behavior cell iff the cell
    is empty or the newcomer out-ranks the incumbent under ``_rank_key`` (higher
    score wins; on a score tie the shorter program wins). A lower-scoring
    program never displaces a higher-scoring elite, so every occupied behavior
    is preserved -- diversity across the descriptor space."""

    def __init__(self, descriptor: BehaviorDescriptor):
        self.descriptor = descriptor
        self.cells: dict[tuple, Program] = {}

    def add(self, program: Program, evaluator_result: dict[str, Any]) -> bool:
        """Attempt to admit ``program`` (assumed valid) to its behavior cell.
        Returns whether it became (or remained) the cell's elite by replacing a
        weaker/absent incumbent."""
        key = self.descriptor(program.code, evaluator_result)
        incumbent = self.cells.get(key)
        if incumbent is None or _rank_key(program) < _rank_key(incumbent):
            self.cells[key] = program
            return True
        return False

    def elites(self) -> list[Program]:
        """All current cell elites."""
        return list(self.cells.values())

    def top_k(self, k: int) -> list[Program]:
        """The ``k`` best elites, best-first under the complexity-biased order."""
        return rank_programs(self.elites())[:k]

    def best(self) -> Optional[Program]:
        elites = self.elites()
        return min(elites, key=_rank_key) if elites else None

    def coverage(self) -> int:
        """Number of occupied behavior cells (a diversity measure)."""
        return len(self.cells)


def archive_best_shot_prompt(
    archive: MapElitesArchive, problem_spec: dict[str, Any], top_k: int
) -> str:
    """Assemble a best-shot prompt from the archive's ``top_k`` elites
    (best-first). Structurally identical to :func:`best_shot_prompt` but sourced
    from the MAP-Elites grid; program bodies are inlined as untrusted text and
    nothing is executed."""
    header_lines = [
        "# AlphaEvolve/MAP-Elites: improve the best elite below.",
        f"# problem: {problem_spec.get('description', problem_spec.get('name', 'unnamed'))}",
    ]
    if "bounds" in problem_spec:
        header_lines.append(f"# bounds: {problem_spec['bounds']}")
    parts = ["\n".join(header_lines)]
    for rank, prog in enumerate(archive.top_k(top_k)):
        parts.append(
            f"# elite {rank} (score={prog.score:.6g}, len={prog.length}):\n{prog.code}"
        )
    if len(parts) == 1:
        parts.append("# (no seed programs yet)")
    return "\n\n".join(parts)


# --------------------------------------------------------------------------- #
# best-shot prompting
# --------------------------------------------------------------------------- #


def best_shot_prompt(
    island: Island, problem_spec: dict[str, Any], top_k: int
) -> str:
    """Assemble a prompt from an island's ``top_k`` highest-scoring programs
    (best-first). The best program appears first so a mutation-style generator
    naturally builds on the current champion. Pure string assembly -- the
    program bodies are inlined verbatim as untrusted text; nothing is executed.
    """
    header_lines = [
        "# FunSearch: improve the best program below.",
        f"# problem: {problem_spec.get('description', problem_spec.get('name', 'unnamed'))}",
    ]
    if "bounds" in problem_spec:
        header_lines.append(f"# bounds: {problem_spec['bounds']}")
    parts = ["\n".join(header_lines)]
    for rank, prog in enumerate(island.top_k(top_k)):
        parts.append(
            f"# candidate {rank} (score={prog.score:.6g}, len={prog.length}):\n{prog.code}"
        )
    if len(parts) == 1:
        parts.append("# (no seed programs yet)")
    return "\n\n".join(parts)


# --------------------------------------------------------------------------- #
# Configuration
# --------------------------------------------------------------------------- #


@dataclass
class FunSearchConfig:
    """All knobs, with every source of randomness fixed by ``seed``."""

    seed: int = 0
    islands: int = 4
    budget: int = 200
    top_k: int = 3
    population_size: int = 12
    migration_interval: int = 50
    seed_program: str = ""
    #: optional per-island seed programs (overrides ``seed_program`` per island)
    seed_programs: Optional[list[str]] = None
    #: population model: ``"islands"`` (FunSearch, default) or ``"map_elites"``
    #: (AlphaEvolve behavior archive).
    population: str = "islands"

    @staticmethod
    def from_dict(d: dict[str, Any]) -> "FunSearchConfig":
        d = dict(d or {})
        population = str(d.get("population", "islands"))
        if population not in ("islands", "map_elites"):
            raise ValueError(f"unknown population model: {population!r}")
        return FunSearchConfig(
            seed=int(d.get("seed", 0)),
            islands=max(1, int(d.get("islands", 4))),
            budget=max(0, int(d.get("budget", 200))),
            top_k=max(1, int(d.get("top_k", 3))),
            population_size=max(1, int(d.get("population_size", 12))),
            migration_interval=max(1, int(d.get("migration_interval", 50))),
            seed_program=str(d.get("seed_program", "")),
            seed_programs=list(d["seed_programs"]) if d.get("seed_programs") else None,
            population=population,
        )


def _derive_seed(base: int, *counters: int) -> int:
    """A stable, arithmetic (hash-randomisation-free) seed derivation so every
    RNG stream is reproducible from the single master ``base`` seed."""
    h = base & 0xFFFFFFFF
    for c in counters:
        h = (h * 1000003 + (c & 0xFFFFFFFF)) & 0xFFFFFFFF
    return h


# --------------------------------------------------------------------------- #
# The FunSearch loop
# --------------------------------------------------------------------------- #


def funsearch(
    problem_spec: dict[str, Any],
    generator: ProgramGenerator,
    evaluator: Evaluator,
    config: FunSearchConfig,
    behavior_descriptor: Optional[BehaviorDescriptor] = None,
) -> dict[str, Any]:
    """Run the FunSearch discovery loop.

    Seed each island with the seed program, then repeatedly: pick an island
    (round-robin, deterministic), build a best-shot prompt from its top-k, ask
    the generator for a new program, evaluate it, admit it iff ``valid``
    (keep-only-passing), and migrate champions on a fixed schedule. Return the
    single best program discovered plus per-island summaries, the full
    evaluation history, and counts.

    In ``population="map_elites"`` mode (AlphaEvolve) the same loop runs against
    a :class:`MapElitesArchive`: best-shot prompts are drawn from the archive's
    top-k elites, valid programs are admitted to their behavior cell (keeping
    only the per-cell elite), and the global best is the top elite.

    Never executes a candidate program: the only thing done with generated text
    is (a) inline it into a prompt string and (b) hand it to
    ``evaluator.evaluate`` -- the injected trust boundary.
    """
    if config.population == "map_elites":
        return _funsearch_map_elites(
            problem_spec, generator, evaluator, config,
            behavior_descriptor or default_behavior_descriptor,
        )

    uid = 0

    def _admit_seed(island: Island, code: str) -> None:
        nonlocal uid
        if not code:
            return
        verdict = evaluator.evaluate(code, problem_spec)
        if verdict.get("valid"):
            island.add(
                Program.make(code, verdict.get("score", 0.0), True, uid)
            )
            uid += 1

    islands = [Island(index=i, capacity=config.population_size) for i in range(config.islands)]
    for i, island in enumerate(islands):
        if config.seed_programs is not None:
            seed_code = config.seed_programs[i % len(config.seed_programs)]
        else:
            seed_code = config.seed_program
        _admit_seed(island, seed_code)

    history: list[dict[str, Any]] = []
    evaluations = 0
    migrations = 0

    for iteration in range(config.budget):
        island = islands[iteration % config.islands]
        prompt = best_shot_prompt(island, problem_spec, config.top_k)
        gen_seed = _derive_seed(config.seed, island.index, iteration)

        # UNTRUSTED text returned here. It is only ever inlined into a prompt or
        # passed to the evaluator seam -- never exec/eval'd in this process.
        candidate = generator.generate(prompt, gen_seed)
        verdict = evaluator.evaluate(candidate, problem_spec)
        evaluations += 1

        valid = bool(verdict.get("valid"))
        score = float(verdict.get("score", 0.0))
        history.append(
            {
                "iteration": iteration,
                "island": island.index,
                "score": score,
                "valid": valid,
                "length": _token_length(candidate),
            }
        )
        if valid:  # keep-only-passing
            island.add(Program.make(candidate, score, True, uid))
            uid += 1

        # Ring migration on schedule: copy each island's champion into the next
        # island. Spreads good genes without merging populations.
        if config.migration_interval and (iteration + 1) % config.migration_interval == 0:
            champions = [isl.best() for isl in islands]
            for src_idx, champ in enumerate(champions):
                if champ is None:
                    continue
                dst = islands[(src_idx + 1) % config.islands]
                dst.add(Program.make(champ.code, champ.score, True, uid))
                uid += 1
            migrations += 1

    # Global best = complexity-biased best across all islands.
    all_members = [m for isl in islands for m in isl.members]
    global_best = min(all_members, key=_rank_key) if all_members else None

    island_summaries = []
    for isl in islands:
        b = isl.best()
        island_summaries.append(
            {
                "index": isl.index,
                "size": len(isl.members),
                "best_score": b.score if b else None,
                "best_program": b.code if b else None,
            }
        )

    return {
        "best_program": global_best.code if global_best else None,
        "best_score": global_best.score if global_best else None,
        "population": "islands",
        "islands": island_summaries,
        "history": history,
        "evaluations": evaluations,
        "migrations": migrations,
    }


def _funsearch_map_elites(
    problem_spec: dict[str, Any],
    generator: ProgramGenerator,
    evaluator: Evaluator,
    config: FunSearchConfig,
    descriptor: BehaviorDescriptor,
) -> dict[str, Any]:
    """The AlphaEvolve MAP-Elites variant of the loop: one behavior archive
    replaces the island populations. Same seams, same determinism, same no-exec
    boundary -- only the population structure changes."""
    archive = MapElitesArchive(descriptor)
    uid = 0

    def _admit(code: str) -> None:
        nonlocal uid
        verdict = evaluator.evaluate(code, problem_spec)
        if verdict.get("valid"):
            archive.add(
                Program.make(code, verdict.get("score", 0.0), True, uid), verdict
            )
            uid += 1

    # Seed the archive (distinct seed programs land in distinct cells).
    seeds = config.seed_programs if config.seed_programs is not None else [config.seed_program]
    for seed_code in seeds:
        if seed_code:
            _admit(seed_code)

    history: list[dict[str, Any]] = []
    evaluations = 0

    for iteration in range(config.budget):
        prompt = archive_best_shot_prompt(archive, problem_spec, config.top_k)
        gen_seed = _derive_seed(config.seed, iteration)

        # UNTRUSTED text: only inlined into a prompt or handed to the evaluator.
        candidate = generator.generate(prompt, gen_seed)
        verdict = evaluator.evaluate(candidate, problem_spec)
        evaluations += 1

        valid = bool(verdict.get("valid"))
        score = float(verdict.get("score", 0.0))
        cell = descriptor(candidate, verdict) if valid else None
        history.append(
            {
                "iteration": iteration,
                "score": score,
                "valid": valid,
                "length": _token_length(candidate),
                "cell": cell,
            }
        )
        if valid:  # keep-only-passing, then MAP-Elites per-cell admission
            archive.add(Program.make(candidate, score, True, uid), verdict)
            uid += 1

    best = archive.best()
    cells = [
        {
            "cell": key,
            "score": prog.score,
            "program": prog.code,
            "length": prog.length,
        }
        for key, prog in sorted(archive.cells.items(), key=lambda kv: _rank_key(kv[1]))
    ]
    return {
        "best_program": best.code if best else None,
        "best_score": best.score if best else None,
        "population": "map_elites",
        "coverage": archive.coverage(),
        "cells": cells,
        "history": history,
        "evaluations": evaluations,
        "migrations": 0,
    }


# --------------------------------------------------------------------------- #
# Offline mock seams (used by the `run` op path; also injectable directly)
# --------------------------------------------------------------------------- #

_INT_RE = re.compile(r"-?\d+")
#: the shared numeric encoding used by both mock seams: ``value = <int>``.
_VALUE_RE = re.compile(r"value\s*=\s*(-?\d+)")


class MockTargetEvaluator(Evaluator):
    """Deterministic offline evaluator: scores a program by how close a single
    integer *read out of its text* is to a hidden ``target`` in ``problem_spec``.

    The program is parsed as pure DATA (regex over its text) -- it is never
    executed. ``valid`` iff an integer is present and within ``[lower, upper]``.
    ``score = 1 / (1 + |value - target|)`` in ``(0, 1]``, peaking at the target.
    Stands in for a production sandbox that would run the candidate for real.
    """

    def evaluate(self, program: str, problem_spec: dict[str, Any]) -> dict[str, Any]:
        target = int(problem_spec["target"])
        bounds = problem_spec.get("bounds", [-10_000, 10_000])
        lower, upper = int(bounds[0]), int(bounds[1])
        m = _INT_RE.search(program or "")
        if m is None:
            return {"score": 0.0, "valid": False}
        value = int(m.group())
        if not (lower <= value <= upper):
            return {"score": 0.0, "valid": False, "value": value}
        return {"score": 1.0 / (1 + abs(value - target)), "valid": True, "value": value}


class MockMutationGenerator(ProgramGenerator):
    """Deterministic offline generator: read the current champion's integer from
    the best-shot ``prompt`` and emit a seeded ±``step`` mutation, clamped to the
    problem bounds. It knows only the public bounds, never the hidden target --
    the evaluator supplies the gradient via selection, exactly as FunSearch's
    LLM proposer relies on the evaluator to rank its guesses.
    """

    def __init__(self, step: int = 5, bounds: tuple[int, int] = (-10_000, 10_000), base: int = 0):
        self.step = int(step)
        self.lower, self.upper = int(bounds[0]), int(bounds[1])
        self.base = int(base)

    def generate(self, prompt: str, seed: int) -> str:
        # Read the champion's value from the shared ``value = N`` encoding. The
        # best-shot prompt lists programs best-first, so the first match is the
        # current champion (header/metadata integers are ignored).
        m = _VALUE_RE.search(prompt or "")
        current = int(m.group(1)) if m else self.base
        rng = random.Random(seed)
        delta = rng.randint(-self.step, self.step)
        value = max(self.lower, min(self.upper, current + delta))
        return f"value = {value}"


class HostileEchoGenerator(ProgramGenerator):
    """Test/adversarial generator that always emits a hostile program string.

    Used to prove the safety boundary: this text round-trips to the evaluator as
    inert DATA and is never executed by the harness."""

    def __init__(self, payload: str = "import os\nos.system('rm -rf /')  # boom"):
        self.payload = payload

    def generate(self, prompt: str, seed: int) -> str:
        return self.payload


def _build_generator(spec: dict[str, Any], config: FunSearchConfig) -> ProgramGenerator:
    """Construct an *offline* generator for the op path. Only mock kinds are
    available here; a real ``llm`` generator is an injection point exercised by
    calling :func:`funsearch` directly with your own :class:`ProgramGenerator`.
    """
    spec = dict(spec or {})
    kind = spec.get("kind", "mock_mutation")
    if kind == "mock_mutation":
        bounds = tuple(spec.get("bounds", [-10_000, 10_000]))
        return MockMutationGenerator(
            step=int(spec.get("step", 5)),
            bounds=(int(bounds[0]), int(bounds[1])),
            base=int(spec.get("base", bounds[0])),
        )
    if kind == "hostile_echo":
        return HostileEchoGenerator(spec.get("payload", "import os\nos.system('boom')"))
    raise ValueError(
        f"generator kind {kind!r} is not available on the offline op path; "
        "inject a real ProgramGenerator by calling funsearch() directly"
    )


def _build_evaluator(spec: dict[str, Any]) -> Evaluator:
    """Construct an *offline* evaluator for the op path. Only mock kinds are
    available here; a real sandbox executor is an injection point exercised by
    calling :func:`funsearch` directly with your own :class:`Evaluator`."""
    spec = dict(spec or {})
    kind = spec.get("kind", "mock_target")
    if kind == "mock_target":
        return MockTargetEvaluator()
    raise ValueError(
        f"evaluator kind {kind!r} is not available on the offline op path; "
        "inject a real (sandboxed) Evaluator by calling funsearch() directly"
    )


# --------------------------------------------------------------------------- #
# worker op
# --------------------------------------------------------------------------- #


def run(request: dict[str, Any]) -> dict[str, Any]:
    """Worker op ``funsearch``.

    Request schema::

        {
          "problem_spec": {                # passed opaquely to the evaluator
             "name": str, "description": str,
             "target": int,               # (mock evaluator) hidden target
             "bounds": [int, int]         # public feasible range
          },
          "config": {                      # see FunSearchConfig
             "seed": int, "islands": int, "budget": int, "top_k": int,
             "population_size": int, "migration_interval": int,
             "seed_program": str, "seed_programs": [str, ...],
             "population": "islands" | "map_elites"   # AlphaEvolve archive
          },
          "generator": {"kind": "mock_mutation"|"hostile_echo", ...},
          "evaluator": {"kind": "mock_target"}
        }

    The seams are built as offline deterministic mocks here; production wires a
    real LLM generator and a sandboxed evaluator by calling :func:`funsearch`
    directly (documented injection points). Returns the :func:`funsearch` dict.
    """
    problem_spec = dict(request.get("problem_spec", {}))
    config = FunSearchConfig.from_dict(request.get("config", {}))
    generator = _build_generator(request.get("generator", {}), config)
    evaluator = _build_evaluator(request.get("evaluator", {}))
    return funsearch(problem_spec, generator, evaluator, config)
