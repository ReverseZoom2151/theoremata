"""OPT-IN live compilation of the adversarial expected-verdict fixtures.

``benchmarks/adversarial.py`` registers Lean artifacts together with a verdict our
gate is supposed to reach on each one. Every one of those verdicts was written
from a TEXTUAL reading of the source. Nothing had ever been compiled, because we
believed no Lean toolchain existed on this machine. It does. This file closes
that gap by running a real elaborator over the very files the fixtures point at
and reporting where source and expectation disagree.

WHAT COMPILATION CAN AND CANNOT SETTLE
--------------------------------------
This is the part worth reading before trusting anything below.

* Successful elaboration shows the file is well formed and, if no ``sorry``
  warning is emitted, that its declarations are closed under the current Mathlib.
  It does NOT show a hypothesis is true, false, satisfiable, or unsatisfiable.
  A vacuously true theorem elaborates exactly as happily as a substantial one.
* Therefore no test here concludes "the hypothesis is false" from a compile.
  The one claim of that shape in the fixture set (BorweinSineSeries) is attacked
  differently: we compile a proof OF OUR OWN that relates the two statements, and
  a kernel-checked implication between them is real evidence in a way that
  elaborating either file alone never is.
* Absence of a ``sorry`` warning is checked by parsing the compiler's own
  "declaration uses `sorry`" diagnostics, not by grepping the source, because a
  ``sorry`` can arrive through an imported module or a macro.

TOOLCHAIN MISMATCH IS THE DOMINANT FAILURE MODE
-----------------------------------------------
The artifacts pin Lean v4.26.0 / v4.27.0 / v4.28.0 in their ``lean-toolchain``
files and every one of them does ``import Mathlib``. The only Mathlib built on
this machine is a much newer one. So a compile failure has two possible causes
that this harness cannot tell apart from the outside: the artifact is wrong, or
Mathlib renamed something under it. Conflating those would make the whole
harness produce misleading red, which is worse than not running it at all.

The rule every test follows:

* compiles clean  -> positive evidence, and it is evidence ACROSS a version gap,
  which is stronger than compiling under the pinned version would have been.
* fails, pinned toolchain differs from ours -> ``toolchain_mismatch``. Reported
  with the real compiler output, never asserted on. This is not evidence the
  artifact is wrong.
* fails, pinned toolchain equals ours -> ``contradiction``. Asserted on, loudly,
  because at that point the expected verdict is the thing in doubt.

Since no artifact currently pins our toolchain, no artifact compile failure can
turn this file red today. That is deliberate. The tests that CAN fail are the
ones whose subject is our own claim rather than the vendor's code: the sorry
census, and the implication proof we write ourselves.

SECURITY
--------
Everything under ``resources/`` is untrusted third-party data, and this file
executes a compiler over it, which is a real escalation over merely reading it.

* We never run any script shipped by those repos. Several of them include a
  ``verify.py`` that posts file contents to a vendor endpoint, and their
  ``task.md`` / ``requirement.md`` files are imperative prose aimed at a prover.
  None of that is invoked or ingested here.
* We never run ``lake`` inside a vendored directory either. Lake reads the
  vendored ``lakefile.toml`` and ``lake-manifest.json``, which can name arbitrary
  git dependencies and would fetch and build them. We invoke OUR OWN ``lean``
  binary directly on the source file with an explicit ``LEAN_PATH`` pointing at
  OUR OWN Mathlib build, so the only vendored bytes that reach a process are the
  ``.lean`` source itself.
* Elaborating a ``.lean`` file still runs code: elaborators, macros,
  ``#eval``, ``initialize`` blocks and ``native_decide`` all execute at compile
  time with the compiler's privileges. There is no sandbox here, so every
  compile is wrapped in a hard timeout, and :func:`_scan_for_execution_markers`
  reports which of those constructs a file contains so a reader knows what was
  actually run.

HOW TO RUN
----------
Every test skips unless all of these hold, so the default state in CI and on a
machine without Lean is a clean skip::

    export THEOREMATA_LEAN_LIVE=1          # opt in; these are minutes, not ms
    # a Mathlib checkout with a populated .lake/build/lib/lean, either at
    # resources/mathlib4-master/mathlib4-master or at:
    export THEOREMATA_MATHLIB_ROOT=/path/to/mathlib4
    # plus the elan toolchain matching that Mathlib's lean-toolchain

Then::

    python -m pytest components/eval/tests/test_adversarial_fixtures_live.py -q -s

``-s`` matters: the tests print the compile status table, which is the output
worth reading even when everything passes.

RUNTIME
-------
A warm ``import Mathlib`` costs about 20 seconds on this machine and a cold one
about 3.5 minutes, so the first compile dominates. The default sweep compiles
roughly a dozen files and takes 5 to 10 minutes warm. A session-wide wall-clock
budget (``THEOREMATA_LEAN_LIVE_BUDGET_S``, default 1800) and a per-file timeout
(``THEOREMATA_LEAN_LIVE_TIMEOUT_S``, default 420) mean it cannot hang: once the
budget is spent the remaining tests skip rather than run.
"""
from __future__ import annotations

import os
import re
import subprocess
import time
from pathlib import Path

import pytest

from theoremata_tools.benchmarks.adversarial import (
    EXPECT_ACCEPT,
    EXPECT_ACCEPT_CONDITIONAL,
    EXPECT_REJECT,
    load_borwein_vacuity,
    load_erdos_public,
    load_higher_dyson,
    load_partition_elliptic,
    load_ramanujan_tau,
)

# components/eval/tests/this_file.py -> [0]=tests [1]=eval [2]=components [3]=root
REPO_ROOT = Path(__file__).resolve().parents[3]

# Mathlib packages are laid out one directory per dependency; we assemble
# LEAN_PATH from whatever is actually present rather than a hardcoded list, so a
# checkout that gains or drops a dependency still resolves.
_MATHLIB_DEFAULT = REPO_ROOT / "resources" / "mathlib4-master" / "mathlib4-master"

OPT_IN = os.environ.get("THEOREMATA_LEAN_LIVE", "") not in ("", "0", "false")
PER_FILE_TIMEOUT_S = float(os.environ.get("THEOREMATA_LEAN_LIVE_TIMEOUT_S", "420"))
SESSION_BUDGET_S = float(os.environ.get("THEOREMATA_LEAN_LIVE_BUDGET_S", "1800"))


# --------------------------------------------------------------------------- #
# Discovery. All of it happens at import, which is when pytest evaluates skipif.
# --------------------------------------------------------------------------- #

def _mathlib_root() -> Path | None:
    """Return a Mathlib checkout that has actually been BUILT, else None.

    The build check is the load-bearing half. A source-only checkout resolves
    every path we need and then fails every import at run time, which would look
    exactly like a broken artifact.
    """
    override = os.environ.get("THEOREMATA_MATHLIB_ROOT")
    root = Path(override) if override else _MATHLIB_DEFAULT
    if not (root / "lean-toolchain").is_file():
        return None
    lib = root / ".lake" / "build" / "lib" / "lean"
    if not lib.is_dir():
        return None
    # One olean is enough to distinguish "built" from "directory exists".
    if not any(lib.glob("Mathlib/**/*.olean")) and not (lib / "Mathlib.olean").is_file():
        return None
    return root


def _read_toolchain(root: Path) -> str:
    return (root / "lean-toolchain").read_text(encoding="utf-8").strip()


def _elan_toolchain_dir(toolchain: str) -> Path:
    """Map ``leanprover/lean4:v4.32.0-rc1`` to its elan directory name.

    elan encodes ``/`` as ``--`` and ``:`` as ``---``.
    """
    return (
        Path.home()
        / ".elan"
        / "toolchains"
        / toolchain.replace("/", "--").replace(":", "---")
    )


def _lean_binary(toolchain: str) -> Path | None:
    """Return the lean binary for EXACTLY this toolchain, or None.

    We deliberately refuse to fall back to whatever ``lean`` is on PATH. Olean
    files carry the compiler's githash, so a mismatched binary rejects Mathlib's
    build wholesale and produces a page of errors that have nothing to do with
    the artifact under test. A skip is the honest outcome; misleading red is not.
    """
    base = _elan_toolchain_dir(toolchain) / "bin"
    for name in ("lean.exe", "lean"):
        candidate = base / name
        if candidate.is_file():
            return candidate
    return None


def _lean_path(root: Path) -> str:
    """Build LEAN_PATH: Mathlib's own build dir plus one per dependency."""
    parts = [root / ".lake" / "build" / "lib" / "lean"]
    packages = root / ".lake" / "packages"
    if packages.is_dir():
        parts += [
            pkg / ".lake" / "build" / "lib" / "lean"
            for pkg in sorted(packages.iterdir())
            if pkg.is_dir()
        ]
    return os.pathsep.join(str(p) for p in parts)


MATHLIB_ROOT = _mathlib_root()
OUR_TOOLCHAIN = _read_toolchain(MATHLIB_ROOT) if MATHLIB_ROOT else ""
LEAN_BIN = _lean_binary(OUR_TOOLCHAIN) if OUR_TOOLCHAIN else None
RESOURCES_PRESENT = (REPO_ROOT / "resources").is_dir()

_why_skip = None
if not OPT_IN:
    _why_skip = (
        "live Lean compilation is opt-in (minutes per run): set "
        "THEOREMATA_LEAN_LIVE=1"
    )
elif not RESOURCES_PRESENT:
    _why_skip = "resources/ is absent (gitignored; this is the CI condition)"
elif MATHLIB_ROOT is None:
    _why_skip = (
        "no BUILT Mathlib found; set THEOREMATA_MATHLIB_ROOT to a checkout whose "
        ".lake/build/lib/lean contains oleans"
    )
elif LEAN_BIN is None:
    _why_skip = (
        f"Mathlib pins {OUR_TOOLCHAIN!r} but that elan toolchain is not "
        f"installed at {_elan_toolchain_dir(OUR_TOOLCHAIN)}; refusing to compile "
        "with a different lean, whose githash would reject these oleans"
    )

needs_lean = pytest.mark.skipif(_why_skip is not None, reason=_why_skip or "")

# Corpus presence is separate from toolchain presence: a fixture set can be empty
# because that one repo was never vendored, which is not a Lean problem.
needs_corpus = pytest.mark.skipif(
    not RESOURCES_PRESENT, reason="resources/ is absent"
)


# --------------------------------------------------------------------------- #
# Compilation
# --------------------------------------------------------------------------- #

STATUS_COMPILED = "compiled"
STATUS_MISMATCH = "toolchain_mismatch"
STATUS_TIMEOUT = "timeout"
STATUS_CONTRADICTION = "contradiction"

#: Filled as tests run; the last test prints it. Keeping a tally is the whole
#: reason to run this file: one red assertion says less than the census does.
OBSERVED: dict[str, dict[str, object]] = {}

_BUDGET = {"remaining": SESSION_BUDGET_S}

_ERROR_RE = re.compile(r": error: ", re.MULTILINE)
_SORRY_RE = re.compile(r"declaration uses `sorry`")

#: Constructs that make elaboration execute nontrivial code. Reported, not
#: blocked: the point is that a reader of a failure knows what ran.
_EXECUTION_MARKERS = ("native_decide", "#eval", "initialize ", "implemented_by")


def _scan_for_execution_markers(source: str) -> list[str]:
    return [m for m in _EXECUTION_MARKERS if m in source]


def _pinned_toolchain(path: Path) -> str:
    """Walk up from a vendored file to its repo's ``lean-toolchain``."""
    for parent in path.parents:
        candidate = parent / "lean-toolchain"
        if candidate.is_file():
            return candidate.read_text(encoding="utf-8").strip()
        if parent == REPO_ROOT:
            break
    return ""


def _abs_fixture_path(item: dict) -> Path:
    """Resolve a fixture's provenance path, which is stored repo-relative."""
    raw = Path(item["provenance"]["path"])
    return raw if raw.is_absolute() else (REPO_ROOT / raw)


def _compile(path: Path, label: str) -> dict:
    """Elaborate one file with our own lean, our own Mathlib, and a hard timeout.

    Never runs lake, never runs anything the vendored repo ships. The vendored
    ``lakefile.toml`` is read by us for its ``lean-toolchain`` sibling only; it is
    never handed to a build tool that would act on its dependency list.
    """
    if _BUDGET["remaining"] <= 0:
        pytest.skip(
            f"session compile budget of {SESSION_BUDGET_S}s is spent "
            f"(raise THEOREMATA_LEAN_LIVE_BUDGET_S to compile {label})"
        )
    env = dict(os.environ)
    env["LEAN_PATH"] = _lean_path(MATHLIB_ROOT)
    source = path.read_text(encoding="utf-8", errors="replace")
    timeout = min(PER_FILE_TIMEOUT_S, _BUDGET["remaining"])
    start = time.monotonic()
    try:
        proc = subprocess.run(
            # These two mirror the vendored lakefiles' [leanOptions]. Without
            # them autoImplicit is on, which is MORE permissive than the artifact
            # was written under and could hide a genuine unknown identifier.
            [
                str(LEAN_BIN),
                "-DautoImplicit=false",
                "-DrelaxedAutoImplicit=false",
                str(path),
            ],
            capture_output=True,
            text=True,
            errors="replace",
            timeout=timeout,
            env=env,
            # Run from a neutral directory so nothing in the vendored tree is
            # picked up implicitly as a working-directory-relative import root.
            cwd=str(REPO_ROOT),
        )
        timed_out = False
        output = (proc.stdout or "") + (proc.stderr or "")
        returncode = proc.returncode
    except subprocess.TimeoutExpired as exc:
        timed_out = True
        output = ((exc.stdout or b"").decode("utf-8", "replace")
                  + (exc.stderr or b"").decode("utf-8", "replace"))
        returncode = None
    elapsed = time.monotonic() - start
    _BUDGET["remaining"] -= elapsed

    pinned = _pinned_toolchain(path)
    ok = (not timed_out) and returncode == 0 and not _ERROR_RE.search(output)
    if ok:
        status = STATUS_COMPILED
    elif timed_out:
        status = STATUS_TIMEOUT
    elif pinned and pinned != OUR_TOOLCHAIN:
        # The decisive branch. We cannot attribute this failure, so we do not.
        status = STATUS_MISMATCH
    else:
        status = STATUS_CONTRADICTION

    result = {
        "label": label,
        "path": str(path),
        "status": status,
        "ok": ok,
        "returncode": returncode,
        "elapsed_s": round(elapsed, 1),
        "pinned_toolchain": pinned,
        "our_toolchain": OUR_TOOLCHAIN,
        "uses_sorry": bool(_SORRY_RE.search(output)),
        "errors": [ln for ln in output.splitlines() if ": error: " in ln],
        "output": output,
        "execution_markers": _scan_for_execution_markers(source),
    }
    OBSERVED[label] = result
    print(
        f"[lean] {label:<34} {status:<19} rc={returncode} "
        f"{result['elapsed_s']}s sorry={result['uses_sorry']} "
        f"errors={len(result['errors'])} pinned={pinned or '?'}"
    )
    return result


def _tail(result: dict, limit: int = 2000) -> str:
    """The compiler's own words, for an assertion message.

    A live-harness failure is only useful if it teaches the reader the real
    state, so the raw output travels with every assertion rather than a summary.
    """
    body = result["output"].strip() or "(no output)"
    return body[-limit:]


def _require_attributable(result: dict) -> None:
    """Skip unless a failure could honestly be blamed on the artifact.

    Called by tests whose assertion only makes sense when the compile actually
    ran the artifact's own mathematics rather than tripping over Mathlib drift.
    """
    if result["status"] == STATUS_MISMATCH:
        pytest.skip(
            f"{result['label']}: failed under {OUR_TOOLCHAIN} but pins "
            f"{result['pinned_toolchain']}, so this is NOT evidence the artifact "
            f"is wrong. First errors: {result['errors'][:3]}"
        )
    if result["status"] == STATUS_TIMEOUT:
        pytest.skip(
            f"{result['label']}: exceeded {PER_FILE_TIMEOUT_S}s, inconclusive"
        )


def _items(loader, verdict=None) -> list[dict]:
    items = loader()
    if verdict is not None:
        items = [i for i in items if i["expected"]["verdict"] == verdict]
    return items


# --------------------------------------------------------------------------- #
# 0. What we are running, and what the version gap actually is.
# --------------------------------------------------------------------------- #


@needs_lean
def test_environment_and_version_gap_are_reported():
    """Baseline, and the context every other result must be read against.

    Prints the pin of every fixture corpus next to ours. If this ever shows a
    corpus pinning our toolchain, the mismatch escape hatch stops applying to it
    and its compile failures become real contradictions, so the number is worth
    seeing on every run.
    """
    assert LEAN_BIN is not None and MATHLIB_ROOT is not None
    print(f"[lean] binary        {LEAN_BIN}")
    print(f"[lean] mathlib       {MATHLIB_ROOT}")
    print(f"[lean] our toolchain {OUR_TOOLCHAIN}")

    pins: dict[str, str] = {}
    for loader in (
        load_borwein_vacuity,
        load_partition_elliptic,
        load_higher_dyson,
        load_erdos_public,
        load_ramanujan_tau,
    ):
        for item in loader():
            path = _abs_fixture_path(item)
            pins[item["id"]] = _pinned_toolchain(path) or "(none found)"
    for fixture_id, pin in sorted(pins.items()):
        gap = "SAME" if pin == OUR_TOOLCHAIN else "gap"
        print(f"[lean] pin {gap:<4} {fixture_id:<34} {pin}")
    # Not an assertion about the pins; an assertion that we could read them at
    # all, since the whole mismatch classification depends on this lookup.
    if pins:
        assert all(p != "(none found)" for p in pins.values()), pins


# --------------------------------------------------------------------------- #
# 1. BorweinSineSeries. The claim under test is a claim about MATHEMATICS, so
#    the evidence has to be a proof, not a successful elaboration.
# --------------------------------------------------------------------------- #


@needs_lean
def test_borwein_pair_both_elaborate_and_both_are_sorry_stubs():
    """Establishes what the two vendored files actually are.

    The fixture docstring describes the 7.6063 file as "sorry-free, kernel-clean
    and statement-preserved", which is what makes it an interesting vacuity
    probe. Both files are ``problem.lean``: statement stubs whose theorem body is
    ``sorry``. A stub cannot be sorry-free, and no vacuity check can be exercised
    by it, because there is no proof to be vacuous.
    """
    items = _items(load_borwein_vacuity)
    if not items:
        pytest.skip("borwein_vacuity corpus absent")
    for item in items:
        result = _compile(_abs_fixture_path(item), item["id"])
        assert result["status"] != STATUS_CONTRADICTION, (
            f"{item['id']} pins our own toolchain and still failed, so this is a "
            f"real defect rather than version drift:\n{_tail(result)}"
        )
        _require_attributable(result)
        assert result["ok"], _tail(result)
        assert result["uses_sorry"], (
            f"{item['id']}: expected a statement stub, but the compiler reported "
            f"no sorry. Output:\n{_tail(result)}"
        )


@needs_lean
def test_borwein_false_premise_verdict_is_contradicted_by_a_kernel_checked_proof(
    tmp_path,
):
    """THE headline test. The 7.6063 hypothesis is WEAKER than the 7.10321 one.

    The fixture asserts ``expect_reject`` with reason ``vacuous_hypothesis``, on
    the reading that 7.6063 sits below Salikhov's 7.606308... and is therefore
    false. Two things are wrong with that reading.

    First, direction. ``PiIrrBound e`` says ``exists C > 0, ... C / q ^ e < |pi -
    p / q|``. Raising ``e`` shrinks the left side, so a LARGER exponent is a
    WEAKER hypothesis. 7.6063 is larger than 7.10321, so the "false premise" file
    assumes strictly less than the "corrected" one.

    Second, provenance. GDM's own ``docs/BorweinSineSeries.md`` gives both facts:
    7.6063 is a hair under Salikhov's bound, AND the bound has since been
    improved to 7.10321 by arXiv:1912.06345. Their edit adopted the improved
    constant. Since mu(pi) <= 7.103205... < 7.6063, the 7.6063 hypothesis is
    TRUE, not false, and the theorem resting on it is not vacuous.

    We do not ask the reader to take the ordering argument on faith. We compile a
    Lean proof of ``PiIrrBound 7.10321 -> PiIrrBound 7.6063``, written here and
    therefore trusted source rather than vendored data. If that implication
    holds, "7.6063 is false" entails "7.10321 is false", which would condemn the
    fixture's own accept-control in the same breath. The verdict pair cannot be
    right.
    """
    items = _items(load_borwein_vacuity)
    ids = {i["id"] for i in items}
    if "borwein_vacuity:false_premise" not in ids:
        pytest.skip("borwein_vacuity probe absent")

    target = tmp_path / "BorweinExponentMonotone.lean"
    target.write_text(_BORWEIN_IMPLICATION_SOURCE, encoding="utf-8")

    result = _compile(target, "borwein:exponent_monotonicity")
    # This file is ours, so nothing here is excused by a vendored pin; the only
    # legitimate non-result is Mathlib having renamed a lemma we used.
    assert result["ok"], (
        "could not compile our own monotonicity proof, so the contradiction "
        "below is argued but not machine-checked. If the errors are missing "
        "lemma names this is our proof needing a refresh against current "
        f"Mathlib, not a claim about the fixture:\n{_tail(result)}"
    )
    assert not result["uses_sorry"], _tail(result)

    probe = next(i for i in items if i["id"] == "borwein_vacuity:false_premise")
    assert probe["expected"]["verdict"] == EXPECT_REJECT
    assert probe["expected"]["reason"] == "vacuous_hypothesis"
    pytest.fail(
        "EXPECTED VERDICT IS WRONG. Lean has just accepted "
        "`PiIrrBound 7.10321 -> PiIrrBound 7.6063`, so the 7.6063 hypothesis is "
        "implied by the 7.10321 one that the paired control assumes to be sound. "
        "It cannot be false while the control is true, and mu(pi) <= 7.103205... "
        "(arXiv:1912.06345) makes both of them true. "
        "borwein_vacuity:false_premise is registered as expect_reject / "
        "vacuous_hypothesis and should not be; the pair is a constant-currency "
        "edit by GDM, not a soundness pair. Delete this pytest.fail once "
        "adversarial.py is corrected."
    )


#: Written here rather than vendored, on purpose: it is the one Lean file in this
#: run whose provenance is our own, so its result is admissible as evidence
#: about the mathematics instead of merely about the artifact.
_BORWEIN_IMPLICATION_SOURCE = """\
import Mathlib

/-- The Borwein hypothesis at an arbitrary exponent. Copied from the two
vendored `problem.lean` files verbatim apart from abstracting the numeral. -/
noncomputable def PiIrrBoundAt (e : ℝ) : Prop :=
  ∃ (C : ℝ) (Q₀ : ℕ), 0 < C ∧ ∀ (p : ℤ) (q : ℕ), 0 < q → Q₀ ≤ q →
    C / (q : ℝ) ^ e < |Real.pi - (p : ℝ) / (q : ℝ)|

/-- A LARGER exponent is a WEAKER hypothesis. -/
theorem piIrrBoundAt_mono {e₁ e₂ : ℝ} (h : e₁ ≤ e₂) :
    PiIrrBoundAt e₁ → PiIrrBoundAt e₂ := by
  rintro ⟨C, Q₀, hC, hall⟩
  refine ⟨C, max Q₀ 1, hC, fun p q hq hQ => ?_⟩
  have hq1 : (1 : ℝ) ≤ (q : ℝ) := by
    exact_mod_cast le_trans (le_max_right Q₀ 1) hQ
  have hlt := hall p q hq (le_trans (le_max_left Q₀ 1) hQ)
  refine lt_of_le_of_lt ?_ hlt
  have hpow : (q : ℝ) ^ e₁ ≤ (q : ℝ) ^ e₂ :=
    Real.rpow_le_rpow_of_exponent_le hq1 h
  have hpos : (0 : ℝ) < (q : ℝ) ^ e₁ := Real.rpow_pos_of_pos (by linarith) _
  gcongr

/-- The concrete instance. If the 7.6063 statement were false then so would be
the 7.10321 statement that the paired accept-control assumes. -/
theorem borwein_7_6063_is_the_weaker_hypothesis :
    PiIrrBoundAt 7.10321 → PiIrrBoundAt 7.6063 :=
  piIrrBoundAt_mono (by norm_num)
"""


# --------------------------------------------------------------------------- #
# 2. The ACCEPT fixtures. If a file we told the gate to accept cannot even
#    elaborate, the expected verdict is what is wrong.
# --------------------------------------------------------------------------- #


@needs_corpus
def test_accept_fixtures_do_not_point_at_sorry_stubs():
    """Needs no compiler, and is the second-most valuable check in this file.

    Four of the five corpora ship both ``problem.lean`` (the statement, body
    ``sorry``) and ``solution.lean`` (the proof). The fixtures were wired to
    ``problem.lean`` for everything except erdos-public. An ``expect_accept`` on a
    statement stub asks the gate to certify a file whose only theorem is
    ``sorry``, which every sorry scan in the tree must refuse; the fixture would
    then be red forever for a reason having nothing to do with the property it
    was written to test.

    Textual, because a source-level ``sorry`` in the fixture's own target is
    enough to establish the wiring bug and does not need minutes of Mathlib.
    """
    offenders = []
    for loader in (
        load_borwein_vacuity,
        load_higher_dyson,
        load_erdos_public,
        load_ramanujan_tau,
    ):
        for item in loader():
            if item["expected"]["verdict"] not in (
                EXPECT_ACCEPT,
                EXPECT_ACCEPT_CONDITIONAL,
            ):
                continue
            path = _abs_fixture_path(item)
            if not path.is_file():
                continue
            text = path.read_text(encoding="utf-8", errors="replace")
            hits = [
                n for n, ln in enumerate(text.splitlines(), 1)
                if re.search(r"(^|[^A-Za-z_'])sorry([^A-Za-z_']|$)", ln)
            ]
            if hits:
                sibling = path.with_name("solution.lean")
                offenders.append(
                    f"{item['id']} -> {path.name} has sorry at lines {hits}; "
                    f"solution.lean {'exists' if sibling.is_file() else 'missing'} "
                    f"next to it ({path})"
                )
    assert not offenders, (
        "these fixtures expect the gate to ACCEPT a file whose theorem is "
        "`sorry`, so they are pointed at the statement stub rather than the "
        "proof:\n  " + "\n  ".join(offenders)
    )


@needs_lean
@pytest.mark.parametrize("fixture_id", ["erdos_public:erdos231", "erdos_public:erdos441"])
def test_small_erdos_accept_fixtures_really_compile(fixture_id):
    """The two smallest ACCEPT fixtures, both formal disproofs.

    erdos-public is the one corpus wired to ``solution.lean``, so these are real
    proofs and an ``expect_accept`` on them is a claim compilation can test. They
    pin v4.27.0 against our much newer Mathlib, so a failure is excused as drift;
    a SUCCESS across that gap is correspondingly strong.
    """
    items = {i["id"]: i for i in load_erdos_public()}
    if fixture_id not in items:
        pytest.skip(f"{fixture_id} absent")
    item = items[fixture_id]
    result = _compile(_abs_fixture_path(item), fixture_id)
    _require_attributable(result)
    assert result["ok"], (
        f"{fixture_id} is registered expect_accept and does not compile under a "
        "toolchain it does not pin, so the expected verdict is now in doubt:\n"
        f"{_tail(result)}"
    )
    assert not result["uses_sorry"], (
        f"{fixture_id} is registered expect_accept but the compiler reported a "
        f"sorry:\n{_tail(result)}"
    )
    if result["execution_markers"]:
        # Not a failure. `decide` through proved bridge lemmas is the documented
        # design of these refutations; native_decide would not be, and the
        # fixture notes claim it is absent, so print what is really there.
        print(f"[lean] {fixture_id} execution markers: {result['execution_markers']}")


@needs_lean
def test_remaining_erdos_accept_fixtures_compile_within_budget():
    """The other five, which the fixture module itself calls nightly-tier.

    Written to consume whatever budget is left rather than to compile a fixed
    set, so this test degrades to a skip instead of blowing the wall clock.
    """
    small = {"erdos_public:erdos231", "erdos_public:erdos441"}
    items = [i for i in load_erdos_public() if i["id"] not in small]
    if not items:
        pytest.skip("erdos_public corpus absent")
    failures, compiled, mismatched = [], 0, 0
    for item in items:
        if _BUDGET["remaining"] <= PER_FILE_TIMEOUT_S * 0.25:
            print("[lean] budget low, stopping the nightly-tier sweep early")
            break
        result = _compile(_abs_fixture_path(item), item["id"])
        if result["status"] == STATUS_COMPILED:
            compiled += 1
            if result["uses_sorry"]:
                failures.append(f"{item['id']}: compiled but uses sorry")
        elif result["status"] == STATUS_MISMATCH:
            mismatched += 1
        else:
            failures.append(
                f"{item['id']}: {result['status']}\n{_tail(result, 1200)}"
            )
    print(f"[lean] erdos nightly tier: compiled={compiled} mismatch={mismatched}")
    assert not failures, "\n\n".join(failures)


@needs_lean
def test_ramanujan_tau_conditional_fixture_target_is_the_stub_not_the_proof():
    """The hard positive, whose fixture is aimed at the wrong file.

    The fixture docstring describes 5,601 lines and 497 lemmas, sorry-free and
    axiom-free. That is ``solution.lean``. The fixture resolves to
    ``problem.lean``, 171 lines with four ``sorry`` bodies. Compiling the target
    settles which file the gate would actually see.
    """
    items = load_ramanujan_tau()
    if not items:
        pytest.skip("ramanujan_tau corpus absent")
    item = items[0]
    path = _abs_fixture_path(item)
    result = _compile(path, item["id"])
    _require_attributable(result)
    assert result["ok"], _tail(result)
    solution = path.with_name("solution.lean")
    assert not result["uses_sorry"], (
        f"{item['id']} is registered {EXPECT_ACCEPT_CONDITIONAL} but its target "
        f"{path.name} elaborates with sorries, so no gate can accept it. The "
        f"proof the fixture docstring describes is {solution} "
        f"({'present' if solution.is_file() else 'MISSING'}).\n{_tail(result)}"
    )


# --------------------------------------------------------------------------- #
# 3. The REJECT fixtures. Elaboration is expected to SUCCEED here, and that
#    success is not a counterexample to anything.
# --------------------------------------------------------------------------- #


@needs_lean
@pytest.mark.parametrize(
    "loader,name",
    [(load_partition_elliptic, "partition_elliptic"),
     (load_higher_dyson, "higher_dyson")],
)
def test_reject_fixtures_elaborate_which_is_not_evidence_against_the_verdict(
    loader, name
):
    """Confirms the reject probes are well formed Lean, and says why that is all.

    Both reject reasons here (``unencoded_side_condition``, ``missing_witness``)
    are properties of the RELATIONSHIP between prose and formal statement, or of
    what is absent from a repo. Neither is visible to an elaborator, so a clean
    compile is exactly what these files should produce and tells us nothing about
    whether the verdict is right. What it does rule out is the boring failure
    mode where a reject fixture is "adversarial" only because it is broken.
    """
    items = _items(loader, EXPECT_REJECT)
    if not items:
        pytest.skip(f"{name} probe absent")
    for item in items:
        result = _compile(_abs_fixture_path(item), item["id"])
        if result["status"] == STATUS_MISMATCH:
            print(
                f"[lean] {item['id']}: inconclusive across the version gap, "
                f"first errors {result['errors'][:2]}"
            )
            continue
        assert result["status"] != STATUS_CONTRADICTION, (
            f"{item['id']} pins our toolchain and fails to elaborate:\n"
            f"{_tail(result)}"
        )
        assert result["ok"], _tail(result)
        print(
            f"[lean] {item['id']}: elaborates cleanly (sorry={result['uses_sorry']}). "
            "This neither supports nor undermines the expect_reject verdict."
        )


# --------------------------------------------------------------------------- #
# 4. The census. Runs last so it can read everything the others recorded.
# --------------------------------------------------------------------------- #


@needs_lean
def test_zz_report_compile_census():
    """Prints the tally and fails only on an unattributable-free contradiction.

    Named ``zz`` so file-order collection puts it last. It asserts almost
    nothing: its job is to make the run legible, and specifically to keep the
    number of ``toolchain_mismatch`` results visible, because that number is the
    honest measure of how much this harness could NOT establish.
    """
    if not OBSERVED:
        pytest.skip("nothing was compiled in this session")
    tally: dict[str, int] = {}
    for result in OBSERVED.values():
        tally[str(result["status"])] = tally.get(str(result["status"]), 0) + 1
    print("[lean] ===== census =====")
    for status, count in sorted(tally.items()):
        print(f"[lean] {status:<20} {count}")
    print(f"[lean] budget remaining {_BUDGET['remaining']:.0f}s of {SESSION_BUDGET_S}s")
    contradictions = [
        r["label"] for r in OBSERVED.values()
        if r["status"] == STATUS_CONTRADICTION
    ]
    assert not contradictions, (
        "these files failed under the very toolchain they pin, so version drift "
        f"does not explain them: {contradictions}"
    )
