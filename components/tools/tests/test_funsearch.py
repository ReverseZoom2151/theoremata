"""Tests for the FunSearch-style program-search engine.

All offline, deterministic, seeded, with mock generator + evaluator seams. No
candidate program is ever executed in-process -- the evaluator seam is the sole
boundary, and here it reads programs as inert data.
"""
from __future__ import annotations

from theoremata_tools.funsearch import (
    Evaluator,
    FunSearchConfig,
    HostileEchoGenerator,
    Island,
    MapElitesArchive,
    MockMutationGenerator,
    MockTargetEvaluator,
    Program,
    ProgramGenerator,
    archive_best_shot_prompt,
    best_shot_prompt,
    default_behavior_descriptor,
    funsearch,
    rank_programs,
    run,
)


# --------------------------------------------------------------------------- #
# helpers
# --------------------------------------------------------------------------- #


def _base_request(**overrides):
    req = {
        "problem_spec": {
            "name": "closest-int",
            "description": "find the integer closest to a hidden target",
            "target": 42,
            "bounds": [0, 100],
        },
        "config": {
            "seed": 20260709,
            "islands": 4,
            "budget": 400,
            "top_k": 3,
            "population_size": 12,
            "migration_interval": 40,
            "seed_program": "value = 0",
        },
        "generator": {"kind": "mock_mutation", "step": 5, "bounds": [0, 100]},
        "evaluator": {"kind": "mock_target"},
    }
    for k, v in overrides.items():
        req[k] = v
    return req


# --------------------------------------------------------------------------- #
# convergence
# --------------------------------------------------------------------------- #


def test_converges_to_high_scoring_program():
    """On a mock problem scored by a hidden target, the loop climbs to a
    near-optimal (ideally exact) program starting from a distant seed."""
    result = funsearch(
        problem_spec={"target": 42, "bounds": [0, 100]},
        generator=MockMutationGenerator(step=5, bounds=(0, 100)),
        evaluator=MockTargetEvaluator(),
        config=FunSearchConfig(seed=1, islands=4, budget=400, seed_program="value = 0"),
    )
    # Seed "value = 0" scores 1/43; a converged run must be far better.
    assert result["best_score"] > 0.9
    # In fact it should land exactly on the target (score == 1.0).
    assert result["best_score"] == 1.0
    assert "42" in result["best_program"]


def test_run_op_converges():
    """The `funsearch` worker op path (offline mock seams) also converges."""
    result = run(_base_request())
    assert result["best_score"] > 0.9
    assert result["evaluations"] == 400
    assert result["migrations"] == 10


# --------------------------------------------------------------------------- #
# islands / diversity
# --------------------------------------------------------------------------- #


def test_islands_maintain_diversity_bad_island_does_not_collapse_run():
    """Distinct per-island seeds + no migration => islands evolve independently.
    Even with one island seeded at a far, poor value, the *global* best is high
    (a bad island doesn't collapse the run) and the islands do not all collapse
    into one identical population (diversity preserved)."""
    result = funsearch(
        problem_spec={"target": 42, "bounds": [0, 100]},
        generator=MockMutationGenerator(step=5, bounds=(0, 100)),
        evaluator=MockTargetEvaluator(),
        config=FunSearchConfig(
            seed=7,
            islands=4,
            budget=240,
            migration_interval=10_000,  # effectively disable migration
            # island 3 is the "bad" one: seeded at the far boundary.
            seed_programs=["value = 30", "value = 50", "value = 40", "value = 100"],
        ),
    )
    # The good islands carry the run to a high global best.
    assert result["best_score"] > 0.9
    # Every island still made progress / holds members.
    assert all(isl["size"] >= 1 for isl in result["islands"])
    # Islands did not all collapse to a single shared population.
    best_programs = {isl["best_program"] for isl in result["islands"]}
    assert len(best_programs) >= 2, best_programs


def test_migration_spreads_champions():
    """With migration on, the run still converges (champions propagate)."""
    result = funsearch(
        problem_spec={"target": 10, "bounds": [0, 100]},
        generator=MockMutationGenerator(step=5, bounds=(0, 100)),
        evaluator=MockTargetEvaluator(),
        config=FunSearchConfig(seed=3, islands=4, budget=300, migration_interval=25),
    )
    assert result["best_score"] == 1.0
    assert result["migrations"] == 12


# --------------------------------------------------------------------------- #
# keep-only-passing
# --------------------------------------------------------------------------- #


def test_invalid_programs_are_rejected():
    """Out-of-bounds integers are invalid and must never enter the database."""
    ev = MockTargetEvaluator()
    spec = {"target": 42, "bounds": [0, 100]}
    assert ev.evaluate("value = 999", spec)["valid"] is False
    assert ev.evaluate("no integer here", spec)["valid"] is False
    assert ev.evaluate("value = 42", spec)["valid"] is True

    # A generator that always proposes out-of-bounds values: nothing is admitted
    # beyond the seed, so the database best stays the (valid) seed program.
    class OutOfBounds(ProgramGenerator):
        def generate(self, prompt, seed):
            return "value = 500"  # outside [0, 100]

    result = funsearch(
        problem_spec=spec,
        generator=OutOfBounds(),
        evaluator=ev,
        config=FunSearchConfig(seed=1, islands=2, budget=50, seed_program="value = 40"),
    )
    # Only the seed survived; every proposal was rejected as invalid.
    for isl in result["islands"]:
        assert isl["best_program"] == "value = 40"
    valid_in_history = [h for h in result["history"] if h["valid"]]
    assert valid_in_history == []


# --------------------------------------------------------------------------- #
# complexity bias
# --------------------------------------------------------------------------- #


def test_ties_prefer_shorter_program():
    """On equal scores the ranking prefers the shorter (lower-complexity)
    program; the final tie-break is deterministic (older uid)."""
    long_prog = Program.make("value = 42  # a very long verbose explanation here", 0.5, True, uid=0)
    short_prog = Program.make("value = 42", 0.5, True, uid=1)
    ranked = rank_programs([long_prog, short_prog])
    assert ranked[0] is short_prog
    assert ranked[0].length < ranked[1].length

    # And through an island's best().
    isl = Island(index=0, capacity=10)
    isl.add(long_prog)
    isl.add(short_prog)
    assert isl.best() is short_prog


def test_ties_prefer_shorter_end_to_end():
    """A tie-inducing evaluator (every valid program scores the same) drives the
    complexity bias: the discovered best is the shortest valid program seen."""

    class FlatEvaluator(Evaluator):
        def evaluate(self, program, problem_spec):
            return {"score": 1.0, "valid": "value" in program}

    class LengthVaryingGenerator(ProgramGenerator):
        # Emits progressively shorter (then a very short) programs, all score 1.
        def generate(self, prompt, seed):
            import random as _r

            pad = "#" + "x" * _r.Random(seed).randint(0, 20)
            return f"value = 1 {pad}"

    result = funsearch(
        problem_spec={},
        generator=LengthVaryingGenerator(),
        evaluator=FlatEvaluator(),
        config=FunSearchConfig(seed=5, islands=2, budget=60, seed_program="value = 1 " + "#" * 50),
    )
    from theoremata_tools.funsearch import _token_length

    best_len = _token_length(result["best_program"])
    # The best must be no longer than any valid program in the history.
    seen_lengths = [h["length"] for h in result["history"] if h["valid"]]
    assert best_len <= min(seen_lengths)


# --------------------------------------------------------------------------- #
# safety boundary: candidate code is never exec'd in-process
# --------------------------------------------------------------------------- #


def test_hostile_program_round_trips_as_data_never_executed():
    """A hostile `import os; os.system(...)` string must reach the evaluator as
    inert DATA and be rejected -- never exec/eval'd by the harness."""
    seen: list[str] = []

    class SpyEvaluator(Evaluator):
        def evaluate(self, program, problem_spec):
            seen.append(program)  # observe the program as a plain string
            return {"score": 0.0, "valid": False}

    hostile = "import os\nos.system('rm -rf /')  # boom"
    result = funsearch(
        problem_spec={},
        generator=HostileEchoGenerator(payload=hostile),
        evaluator=SpyEvaluator(),
        config=FunSearchConfig(seed=1, islands=1, budget=5, seed_program=""),
    )
    # The hostile text was handed to the evaluator seam verbatim, as data.
    assert hostile in seen
    # It was rejected (keep-only-passing) and never became a best program.
    assert result["best_program"] is None
    assert all(h["valid"] is False for h in result["history"])


def test_source_never_execs_candidate_code():
    """Structural guarantee: the module contains no exec/eval/compile of
    candidate program text -- the evaluator seam is the only boundary."""
    import inspect

    import theoremata_tools.funsearch as fs

    src = inspect.getsource(fs)
    # No dynamic-execution builtins are invoked on candidate text.
    assert "exec(" not in src
    assert "eval(" not in src
    # The only `compile(` in the module is the stdlib `re.compile` for parsing
    # -- never a bare builtin `compile()` of a candidate program.
    import re as _re

    for m in _re.finditer(r"(\w*)\.?compile\(", src):
        assert m.group(1) == "re", f"unexpected compile() call: {m.group(0)!r}"


# --------------------------------------------------------------------------- #
# determinism
# --------------------------------------------------------------------------- #


def test_deterministic_same_seed_same_best():
    """Same request => identical best program and score."""
    r1 = run(_base_request())
    r2 = run(_base_request())
    assert r1["best_program"] == r2["best_program"]
    assert r1["best_score"] == r2["best_score"]
    assert r1["history"] == r2["history"]


def test_different_seed_can_differ_but_stays_valid():
    """A different seed is still deterministic and still converges."""
    a = run(_base_request(config={**_base_request()["config"], "seed": 111}))
    b = run(_base_request(config={**_base_request()["config"], "seed": 222}))
    assert a["best_score"] > 0.9 and b["best_score"] > 0.9
    # Re-running seed 111 reproduces exactly.
    a2 = run(_base_request(config={**_base_request()["config"], "seed": 111}))
    assert a["best_program"] == a2["best_program"]


# --------------------------------------------------------------------------- #
# best-shot prompting
# --------------------------------------------------------------------------- #


def test_best_shot_prompt_lists_top_programs_best_first():
    isl = Island(index=0, capacity=10)
    isl.add(Program.make("value = 5", 0.2, True, uid=0))
    isl.add(Program.make("value = 9", 0.9, True, uid=1))
    isl.add(Program.make("value = 7", 0.5, True, uid=2))
    prompt = best_shot_prompt(isl, {"description": "d", "bounds": [0, 10]}, top_k=2)
    # Best (score 0.9) appears before the next; low scorer omitted (top_k=2).
    assert prompt.index("value = 9") < prompt.index("value = 7")
    assert "value = 5" not in prompt


# --------------------------------------------------------------------------- #
# MAP-Elites archive (AlphaEvolve)
# --------------------------------------------------------------------------- #


def _fixed_descriptor(cell):
    """A descriptor that forces a given cell key, to test cell contention."""
    return lambda program, result: cell


def test_map_elites_cell_keeps_only_highest_scoring_elite():
    """Two programs mapping to the same cell: only the higher scorer survives;
    a higher score replaces the incumbent, a lower score does not."""
    archive = MapElitesArchive(_fixed_descriptor((0, 0)))

    low = Program.make("value = 1", 0.3, True, uid=0)
    high = Program.make("value = 2", 0.8, True, uid=1)

    assert archive.add(low, {}) is True  # empty cell -> admitted
    assert archive.best().code == "value = 1"

    # Higher score replaces the incumbent.
    assert archive.add(high, {}) is True
    assert archive.best().code == "value = 2"
    assert archive.coverage() == 1  # still one cell

    # A lower score does NOT displace the elite.
    lower = Program.make("value = 9", 0.1, True, uid=2)
    assert archive.add(lower, {}) is False
    assert archive.best().code == "value = 2"
    assert len(archive.elites()) == 1


def test_map_elites_tie_prefers_shorter():
    """On a score tie within a cell, the shorter (lower-complexity) program wins."""
    archive = MapElitesArchive(_fixed_descriptor((0, 0)))
    long_prog = Program.make("value = 2  # verbose padding here indeed", 0.5, True, uid=0)
    short_prog = Program.make("value = 2", 0.5, True, uid=1)
    archive.add(long_prog, {})
    # Equal score, shorter -> replaces.
    assert archive.add(short_prog, {}) is True
    assert archive.best() is short_prog


def test_map_elites_maintains_diversity_across_descriptors():
    """Programs with distinct descriptors occupy distinct cells and are all
    preserved -- diversity across the behavior space, even at different scores."""
    # default descriptor = (length_bucket, value_bucket); vary the value widely.
    archive = MapElitesArchive(default_behavior_descriptor)
    ev = MockTargetEvaluator()
    spec = {"target": 42, "bounds": [0, 100]}
    for v in (0, 20, 40, 60, 80):
        code = f"value = {v}"
        archive.add(Program.make(code, ev.evaluate(code, spec)["score"], True, uid=v), {"value": v})
    # Five distinct value buckets -> five occupied cells, none evicted.
    assert archive.coverage() == 5
    # The lower-scoring far-from-target elites are still present (not collapsed).
    codes = {p.code for p in archive.elites()}
    assert {"value = 0", "value = 80"} <= codes


def test_map_elites_mode_converges_on_target():
    """The full loop in map_elites mode climbs to the hidden target."""
    result = funsearch(
        problem_spec={"target": 42, "bounds": [0, 100]},
        generator=MockMutationGenerator(step=5, bounds=(0, 100)),
        evaluator=MockTargetEvaluator(),
        config=FunSearchConfig(
            seed=9, budget=500, seed_program="value = 0", population="map_elites"
        ),
    )
    assert result["population"] == "map_elites"
    assert result["best_score"] == 1.0
    assert "42" in result["best_program"]
    # The archive discovered several distinct behavior cells along the way.
    assert result["coverage"] >= 2
    assert "cells" in result and len(result["cells"]) == result["coverage"]


def test_map_elites_run_op_and_determinism():
    """The `run` op honors population=map_elites and is deterministic."""
    req = _base_request()
    req["config"] = {**req["config"], "population": "map_elites", "budget": 400}
    r1 = run(req)
    r2 = run(req)
    assert r1["population"] == "map_elites"
    assert r1["best_score"] > 0.9
    assert r1["best_program"] == r2["best_program"]
    assert r1["history"] == r2["history"]
    assert r1["cells"] == r2["cells"]


def test_map_elites_archive_best_shot_prompt_lists_elites():
    archive = MapElitesArchive(default_behavior_descriptor)
    # Distinct value buckets (10//5=2, 42//5=8) -> two cells, both retained.
    archive.add(Program.make("value = 10", 0.5, True, uid=0), {"value": 10})
    archive.add(Program.make("value = 42", 1.0, True, uid=1), {"value": 42})
    prompt = archive_best_shot_prompt(archive, {"description": "d", "bounds": [0, 100]}, top_k=2)
    assert "elite" in prompt
    # Best elite (score 1.0) appears first.
    assert prompt.index("value = 42") < prompt.index("value = 10")


def test_islands_mode_unchanged_reports_population():
    """Default islands mode is untouched and now tags its population model."""
    result = run(_base_request())
    assert result["population"] == "islands"
    assert "islands" in result
