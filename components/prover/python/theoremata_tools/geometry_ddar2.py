"""DDAR2: a unified exact algebraic geometry engine (AlphaGeometry2-style).

This module is a clean-room reimplementation of the *architectural core* of
AlphaGeometry2's **DDAR2** (see ``docs/resource-mining/new/alphageometry2.md`` and
``docs/paper-mining/alphageometry2-gold-medalist.md``), plus the **constant-value
predicates** of Newclid (``docs/resource-mining/new/newclid.md``). It is a sibling
of :mod:`theoremata_tools.geometry_ddar` (the DD+AR engine with two bespoke
angle/length chasers) and reuses the predicate/numeric model of
:mod:`theoremata_tools.geometry` *without modifying it*.

The one idea
------------
DDAR1 kept a text rule file plus separate coefficient tables for angles, ratios
and lengths. DDAR2 collapses all of that into **one exact incremental Gaussian
eliminator** (:class:`ElimCore`) reused for **three change-of-variable groups**:

* **angles** -- line directions in units of pi, reduced as linear relations among
  ``dir(line)`` unknowns (``para``/``perp``/``eqangle``/``collinear`` and the
  constant ``aconst``/``s_angle`` all become linear rows; ``perp`` = ``pi/2``);
* **log-distances** -- multiplicative lengths/ratios linearized as
  ``L(seg) = log|seg|``; numeric constants are kept **exact** by prime
  factorization (``rconst``/``lconst`` pin ``L`` to an integer combination of
  ``log(prime)`` atoms -- no floating error ever enters the constant sub-lattice);
* **positions** -- additive signed segment lengths (``distseq``: a signed sum of
  segments equals a constant), the group that models betweenness / ``AB+BC=AC``.

``para``/``perp``/``eqangle``/``cong``/``eqratio``/``collinear`` and the
constant-value predicates ``aconst``/``rconst``/``lconst``/``s_angle`` all reduce
to a linear row in one of these three systems. A goal is **proved** iff its
normalized row lies in the row space of the hypothesis rows, and :class:`ElimCore`
returns the **witnessing linear combination** (the exact rational multipliers on
the hypothesis equations) as a certificate.

Constant-value predicates ("fix" rows)
--------------------------------------
Following Newclid's ``fixc/fixl/...`` mechanism, a numeric constant is pinned by a
row that mentions a distinguished constant atom: the angle system carries a single
``ONE`` atom (rational multiples of pi are exact), and the length system carries
``("logp", prime)`` atoms (any positive-rational ratio is an exact integer
combination of them). So ``aconst(A,B,C)=pi/3`` becomes
``dir(BC) - dir(BA) - 1/3*ONE = 0`` and DDAR2 can *prove an angle equals 60
degrees* -- something the plain 5-rule chainer in :mod:`geometry` cannot state.

Deductive database (optional, interleaved)
------------------------------------------
The focus is the unified AR engine, but a light **DD** pass (reusing
:data:`geometry._RULES` -- midpoint, two-perpendiculars-are-parallel, transports,
transitivities) forward-chains the canonicalizable facts and injects any derived
``parallel``/``perpendicular``/``cong``/``collinear`` back into the AR systems, so
geometric rules and algebra feed each other to a joint fixpoint.

Honest scope
------------
* CAN: chase directed angles (incl. numeric angle constants), log-length ratios
  (incl. numeric ratio/length constants) and additive segment sums to closure,
  and emit an exact rational certificate; do everything the 5-rule chainer does.
* CANNOT: introduce auxiliary points (no construction search); decide
  *inequalities* / betweenness orientation (the theory is directed-angle-mod-pi +
  log-length + signed-position *equalities*); mix the three groups
  multiplicatively (AG2's ``transfer_*`` synchronization is not implemented here).
  A proved goal is sound **modulo non-degeneracy** (referenced lines/segments are
  genuinely determined -- no coincident endpoints) and, for angles, modulo the
  usual ``mod pi`` branch (constants are compared as exact rationals of pi). A
  goal reached by neither DD nor AR is ``proved=False`` -- **inconclusive**, never
  a disproof (falsity is decided only by the optional numeric screen).
* This module does **NOT** ship a traceback/minimal-dependency proof extractor --
  that (AG2's dropped ``why()``/``trace_back``) is a sibling concern. It returns
  *whether* a fact holds and the linear-combination certificate, not a minimal
  human proof DAG.

Pure standard library (``math``, ``fractions``, ``itertools``); numpy is **not**
required (an optional test cross-checks a solve against numpy if installed).
Deterministic: the AR engine has no randomness; every numeric step is driven by
the caller-supplied ``seed``.
"""
from __future__ import annotations

import itertools
from fractions import Fraction
from typing import Any

from theoremata_tools import geometry

# Reused verbatim from geometry.py (its predicate model + audited DD rules):
_canonical = geometry._canonical        # canonical hashable fact keys
_describe = geometry._describe           # human rendering of a fact
_seg_key = geometry._seg_key             # unordered segment key
_GEOMETRY_RULES = geometry._RULES        # the five sound forward-chaining rules
Fact = geometry.Fact


# =========================================================================== #
# Linear-combination helpers.  A LinComb is a plain ``dict[atom, Fraction]``
# understood as ``sum coeff*atom == 0``.  Atoms are hashable tuples.
# =========================================================================== #
Atom = Any
LinComb = dict          # dict[Atom, Fraction]

ONE: Atom = ("1",)      # the distinguished constant atom (value 1)


def _lc_axpy(dst: LinComb, src: LinComb, coef: Fraction) -> None:
    """In place ``dst += coef*src``, dropping resulting zeros."""
    for a, c in src.items():
        nc = dst.get(a, Fraction(0)) + coef * c
        if nc == 0:
            dst.pop(a, None)
        else:
            dst[a] = nc


def _lc_clean(comb: LinComb) -> LinComb:
    return {a: c for a, c in comb.items() if c != 0}


# --------------------------------------------------------------------------- #
# Exact integer factorization for the log-distance constant sub-lattice.
# --------------------------------------------------------------------------- #
def _factor(n: int) -> dict[int, int]:
    """Prime-factorize a positive integer (small; exact)."""
    out: dict[int, int] = {}
    d = 2
    while d * d <= n:
        while n % d == 0:
            out[d] = out.get(d, 0) + 1
            n //= d
        d += 1 if d == 2 else 2
    if n > 1:
        out[n] = out.get(n, 0) + 1
    return out


def _logp(prime: int) -> Atom:
    return ("logp", prime)


def _add_log_const(comb: LinComb, k: Fraction, sign: int) -> None:
    """Add ``sign*log(k)`` to a length LinComb, exactly, as prime-log atoms.

    ``k`` must be a positive rational; ``log(k) = sum e_p*log(p)`` with the
    numerator's exponents positive and the denominator's negative.
    """
    if k <= 0:
        raise ValueError(f"ratio/length constant must be positive, got {k}")
    for p, e in _factor(k.numerator).items():
        _lc_axpy(comb, {_logp(p): Fraction(1)}, Fraction(sign * e))
    for p, e in _factor(k.denominator).items():
        _lc_axpy(comb, {_logp(p): Fraction(1)}, Fraction(-sign * e))


def _is_const_atom(a: Atom) -> bool:
    """Constant atoms are passengers: never chosen as elimination pivots."""
    return a == ONE or (isinstance(a, tuple) and a and a[0] == "logp")


# =========================================================================== #
# ElimCore: an exact incremental sparse Gaussian eliminator (RREF over
# fractions.Fraction) with provenance, so it returns the witnessing linear
# combination of the *labelled* input equations that proves an entailment.
# =========================================================================== #
class ElimCore:
    """Maintain a set of linear equalities ``sum coeff*atom == 0`` and answer
    entailment queries with a certificate.

    Public API
      * :meth:`add` -- assert one equation (a ``LinComb``) under a string label;
        returns True iff it added a new pivot (i.e. was not already entailed).
      * :meth:`entails` -- is a target ``LinComb`` in the row space? Returns the
        certificate ``list[(Fraction, label)]`` (an exact rational combination of
        the asserted equations equal to the target) or ``None``.
      * :meth:`inconsistent` -- True if the asserted equations are contradictory
        (e.g. ``1 == 0``); such a system is not used to claim proofs.
      * :meth:`clone` -- deep copy, for cheap branch/knowledge-sharing.

    Invariant: ``rows`` is a reduced row-echelon set keyed by pivot atom; each
    stored row has pivot-coefficient 1 and mentions no *other* pivot atom.
    Constant atoms (``ONE`` / ``("logp", .)``) are never pivots -- they ride
    along on the constant side, which is exactly what lets a "fix" row pin a
    variable to a numeric constant.
    """

    __slots__ = ("rows", "cert", "_inconsistent")

    def __init__(self) -> None:
        self.rows: dict[Atom, LinComb] = {}        # pivot -> reduced row
        self.cert: dict[Atom, LinComb] = {}        # pivot -> dict[label -> Fraction]
        self._inconsistent = False

    # -- internal: reduce a (comb, provenance) pair against the stored rows --- #
    def _reduce(self, comb: LinComb, prov: LinComb) -> tuple[LinComb, LinComb]:
        comb = dict(comb)
        prov = dict(prov)
        changed = True
        while changed:
            changed = False
            for a in list(comb):
                if comb.get(a) and a in self.rows:
                    f = comb[a]
                    _lc_axpy(comb, self.rows[a], -f)
                    _lc_axpy(prov, self.cert[a], -f)
                    changed = True
                    break
        return _lc_clean(comb), _lc_clean(prov)

    def _pivot_usage(self, atom: Atom) -> int:
        return sum(1 for r in self.rows.values() if atom in r)

    def add(self, comb: LinComb, label: str) -> bool:
        """Assert ``comb == 0`` (provenance = this single labelled equation)."""
        comb = _lc_clean({a: Fraction(c) for a, c in comb.items()})
        if not comb:
            return False
        comb, prov = self._reduce(comb, {label: Fraction(1)})
        if not comb:
            return False  # already entailed -- no new information
        var_atoms = [a for a in comb if not _is_const_atom(a)]
        if not var_atoms:
            # only constant atoms remain but the row is nonzero => contradiction
            self._inconsistent = True
            return False
        # sparsity heuristic (AG2): pivot on the least-used variable atom
        pivot = min(var_atoms, key=lambda a: (self._pivot_usage(a), repr(a)))
        inv = Fraction(1) / comb[pivot]
        row = {a: c * inv for a, c in comb.items()}
        rprov = {l: c * inv for l, c in prov.items()}
        # back-substitute the new pivot out of every existing row (keep RREF)
        for p in list(self.rows):
            r = self.rows[p]
            if pivot in r:
                f = r[pivot]
                _lc_axpy(r, row, -f)
                _lc_axpy(self.cert[p], rprov, -f)
                self.rows[p] = _lc_clean(r)
                self.cert[p] = _lc_clean(self.cert[p])
        self.rows[pivot] = _lc_clean(row)
        self.cert[pivot] = _lc_clean(rprov)
        return True

    def entails(self, target: LinComb) -> list[tuple[Fraction, str]] | None:
        """Return the certificate for ``target == 0`` or ``None``.

        The certificate is a list of ``(coeff, label)`` such that
        ``sum coeff*equation_label`` equals ``target`` as a formal identity in
        the atoms; since every asserted equation is ``== 0``, this proves
        ``target == 0``.
        """
        target = _lc_clean({a: Fraction(c) for a, c in target.items()})
        residual, prov = self._reduce(target, {})
        if residual:
            return None
        # _reduce tracks prov as the combination equal to the *reduced* comb (the
        # -f convention used by add); for the target the certificate is its
        # negation: target = sum f*row = sum f*cert*orig  =>  coeff = -prov.
        return sorted(((-c, l) for l, c in prov.items() if c != 0),
                      key=lambda t: t[1])

    @property
    def inconsistent(self) -> bool:
        return self._inconsistent

    def clone(self) -> "ElimCore":
        new = ElimCore()
        new.rows = {p: dict(r) for p, r in self.rows.items()}
        new.cert = {p: dict(c) for p, c in self.cert.items()}
        new._inconsistent = self._inconsistent
        return new

    def num_relations(self) -> int:
        return len(self.rows)


# =========================================================================== #
# Constant parsing (angles in units of pi; ratios/lengths as positive rationals)
# =========================================================================== #
def _parse_frac(value: Any) -> Fraction:
    if isinstance(value, Fraction):
        return value
    if isinstance(value, int):
        return Fraction(value)
    if isinstance(value, float):
        return Fraction(value).limit_denominator(10 ** 9)
    s = str(value).strip()
    if "/" in s:
        num, den = s.split("/", 1)
        return Fraction(_parse_frac(num.strip()), _parse_frac(den.strip()))
    return Fraction(s)


def _angle_const(spec: dict[str, Any]) -> Fraction:
    """Angle constant, returned in *units of pi* (so 60 degrees -> 1/3)."""
    if "deg" in spec:
        return _parse_frac(spec["deg"]) / 180
    raw = spec.get("const", spec.get("value"))
    if raw is None:
        raise ValueError("angle-constant predicate needs 'const' (units of pi) or 'deg'")
    s = str(raw).strip()
    if "pi" in s:
        # forms: 'pi', 'pi/3', '2pi/3', '7pi/30'
        s = s.replace("pi", "*")  # 'pi/3' -> '*/3', '2pi/3' -> '2*/3'
        if s.startswith("*"):
            s = "1" + s
        s = s.replace("*", "")     # coefficient string without the pi token
        return _parse_frac(s)
    return _parse_frac(raw)


def _ratio_const(spec: dict[str, Any]) -> Fraction:
    raw = spec.get("const", spec.get("ratio", spec.get("value")))
    if raw is None:
        raise ValueError("ratio/length-constant predicate needs 'const'")
    return _parse_frac(raw)


# =========================================================================== #
# Predicate -> (system, LinComb) encoding.  Orientation matters for directed
# angles, so this consumes argument order directly (not geometry's canonical
# form).  Returns a list because a few predicates emit several rows.
# =========================================================================== #
def _dir(p: str, q: str) -> Atom:
    return ("dir", frozenset((p, q)))


def _len(p: str, q: str) -> Atom:
    return ("len", frozenset((p, q)))


def _seg_signed(p: str, q: str) -> tuple[Atom, int]:
    """Signed 1-D segment atom (antisymmetric): seg(p,q) = -seg(q,p)."""
    if p == q:
        raise ValueError("degenerate segment")
    if p < q:
        return ("seg", (p, q)), 1
    return ("seg", (q, p)), -1


def _distinct(*pairs: tuple[str, str]) -> bool:
    return all(a != b for a, b in pairs)


ANGLE, LENGTH, POSITION = "angle", "length", "position"


def encode(pred: str, pts: list[str], spec: dict[str, Any] | None = None
           ) -> list[tuple[str, LinComb]]:
    """Translate one ordered predicate instance into AR rows.

    Returns a list of ``(system, LinComb)`` (usually length 1). Returns ``[]``
    for predicates AR does not model or for degenerate (coincident-endpoint)
    references.
    """
    spec = spec or {}

    # ---- angle group ----------------------------------------------------- #
    if pred == "parallel":
        a, b, c, d = pts
        if not _distinct((a, b), (c, d)):
            return []
        return [(ANGLE, {_dir(a, b): Fraction(1), _dir(c, d): Fraction(-1)})]

    if pred == "perpendicular":
        a, b, c, d = pts
        if not _distinct((a, b), (c, d)):
            return []
        return [(ANGLE, {_dir(a, b): Fraction(1), _dir(c, d): Fraction(-1),
                         ONE: Fraction(-1, 2)})]

    if pred == "eqangle":
        p0, p1, p2, p3, p4, p5 = pts
        if not _distinct((p1, p2), (p1, p0), (p4, p5), (p4, p3)):
            return []
        return [(ANGLE, {_dir(p1, p2): Fraction(1), _dir(p1, p0): Fraction(-1),
                         _dir(p4, p5): Fraction(-1), _dir(p4, p3): Fraction(1)})]

    if pred == "collinear":
        a, b, c = pts
        return [(ANGLE, {_dir(a, b): Fraction(1), _dir(a, c): Fraction(-1)}),
                (ANGLE, {_dir(a, b): Fraction(1), _dir(b, c): Fraction(-1)})]

    if pred == "aconst":
        # aconst(A,B,C): directed <ABC = const*pi  (vertex B)
        a, b, c = pts[:3]
        if not _distinct((b, c), (b, a)):
            return []
        k = _angle_const(spec)
        return [(ANGLE, {_dir(b, c): Fraction(1), _dir(b, a): Fraction(-1),
                         ONE: -k})]

    if pred == "s_angle":
        # s_angle(A,B,X): directed angle between line AB and line BX = const*pi
        a, b, x = pts[:3]
        if not _distinct((a, b), (b, x)):
            return []
        k = _angle_const(spec)
        return [(ANGLE, {_dir(b, x): Fraction(1), _dir(a, b): Fraction(-1),
                         ONE: -k})]

    # ---- log-distance (multiplicative) group ----------------------------- #
    if pred in ("cong", "eqlen"):
        a, b, c, d = pts
        if not _distinct((a, b), (c, d)):
            return []
        return [(LENGTH, {_len(a, b): Fraction(1), _len(c, d): Fraction(-1)})]

    if pred == "eqratio":
        a, b, c, d, e, f, g, h = pts
        if not _distinct((a, b), (c, d), (e, f), (g, h)):
            return []
        return [(LENGTH, {_len(a, b): Fraction(1), _len(c, d): Fraction(-1),
                          _len(e, f): Fraction(-1), _len(g, h): Fraction(1)})]

    if pred == "rconst":
        # rconst(A,B,C,D, const): |AB|/|CD| = const
        a, b, c, d = pts[:4]
        if not _distinct((a, b), (c, d)):
            return []
        comb: LinComb = {_len(a, b): Fraction(1), _len(c, d): Fraction(-1)}
        _add_log_const(comb, _ratio_const(spec), -1)
        return [(LENGTH, comb)]

    if pred == "lconst":
        # lconst(X,A, const): |XA| = const
        x, a = pts[:2]
        if not _distinct((x, a)):
            return []
        comb = {_len(x, a): Fraction(1)}
        _add_log_const(comb, _ratio_const(spec), -1)
        return [(LENGTH, comb)]

    # ---- additive-position group ----------------------------------------- #
    if pred == "distseq":
        # spec["terms"] = [[coeff, [p, q]], ...]; optional spec["const"].
        comb = {}
        for coeff, (p, q) in spec.get("terms", []):
            atom, sign = _seg_signed(p, q)
            _lc_axpy(comb, {atom: Fraction(1)}, Fraction(sign) * _parse_frac(coeff))
        if "const" in spec:
            _lc_axpy(comb, {ONE: Fraction(1)}, _parse_frac(spec["const"]))
        return [(POSITION, _lc_clean(comb))] if comb else []

    return []  # midpoint / concyclic handled by DD; unknown preds ignored


# For the DD read-back: which predicates does the numeric screen understand.
_NUMERIC_PREDS = {"collinear", "midpoint", "cong", "eqlen", "parallel",
                  "perpendicular", "eqangle", "concyclic"}


# =========================================================================== #
# The unified store: one ElimCore per change-of-variable group.
# =========================================================================== #
class UnifiedAR:
    def __init__(self) -> None:
        self.cores: dict[str, ElimCore] = {
            ANGLE: ElimCore(), LENGTH: ElimCore(), POSITION: ElimCore(),
        }

    def add_pred(self, pred: str, pts: list[str], spec: dict[str, Any],
                 label: str) -> bool:
        added = False
        rows = encode(pred, pts, spec)
        for i, (system, comb) in enumerate(rows):
            lab = label if len(rows) == 1 else f"{label}#{i + 1}"
            if self.cores[system].add(comb, lab):
                added = True
        return added

    def prove_pred(self, pred: str, pts: list[str], spec: dict[str, Any]
                   ) -> tuple[str, list[tuple[Fraction, str]]] | None:
        """Certify one goal predicate. Returns ``(system, certificate)``."""
        # perpendicular is +/- pi/2 mod pi: try both signs.
        if pred == "perpendicular":
            a, b, c, d = pts
            if not _distinct((a, b), (c, d)):
                return None
            base = {_dir(a, b): Fraction(1), _dir(c, d): Fraction(-1)}
            for sign in (Fraction(-1, 2), Fraction(1, 2)):
                cert = self.cores[ANGLE].entails({**base, ONE: sign})
                if cert is not None:
                    return ANGLE, cert
            return None
        rows = encode(pred, pts, spec)
        if len(rows) == 1:
            system, comb = rows[0]
            cert = self.cores[system].entails(comb)
            return (system, cert) if cert is not None else None
        # multi-row goal (e.g. collinear): every row must be entailed; merge certs
        if not rows:
            return None
        merged: list[tuple[Fraction, str]] = []
        system = rows[0][0]
        for _, comb in rows:
            cert = self.cores[system].entails(comb)
            if cert is None:
                return None
            merged.extend(cert)
        return system, merged

    def num_relations(self, system: str) -> int:
        return self.cores[system].num_relations()

    def inconsistent(self) -> bool:
        return any(c.inconsistent for c in self.cores.values())


# =========================================================================== #
# Optional/interleaved DD: reuse geometry's five sound rules, feed results to AR.
# =========================================================================== #
def _dd_closure(hypotheses: list[dict[str, Any]], max_rounds: int = 16
                ) -> tuple[set[Fact], dict[Fact, tuple[str, list[Fact]]]]:
    facts: set[Fact] = set()
    reasons: dict[Fact, tuple[str, list[Fact]]] = {}
    for h in hypotheses:
        try:
            facts.add(_canonical(h["pred"], list(h.get("points", h.get("args", [])))))
        except ValueError:
            pass  # AR-only predicate (aconst/rconst/distseq/...)
    for _ in range(max_rounds):
        added = False
        for rule_name, rule in _GEOMETRY_RULES:
            for new_fact, premises in rule(facts):
                if new_fact not in facts:
                    facts.add(new_fact)
                    reasons.setdefault(new_fact, (rule_name, premises))
                    added = True
        if not added:
            break
    return facts, reasons


def _fact_to_pred(fact: Fact) -> tuple[str, list[str]] | None:
    """Map a canonical DD fact back to an ordered predicate for AR encoding."""
    kind = fact[0]
    if kind == "parallel":
        (a, b), (c, d) = (tuple(s) for s in fact[1])
        return "parallel", [a, b, c, d]
    if kind == "perpendicular":
        (a, b), (c, d) = (tuple(s) for s in fact[1])
        return "perpendicular", [a, b, c, d]
    if kind == "cong":
        (a, b), (c, d) = (tuple(s) for s in fact[1])
        return "cong", [a, b, c, d]
    if kind == "collinear":
        return "collinear", list(sorted(fact[1]))
    return None


def _dd_derivation(goal_fact: Fact, facts: set[Fact],
                   reasons: dict[Fact, tuple[str, list[Fact]]]
                   ) -> list[dict[str, Any]] | None:
    if goal_fact not in facts:
        return None
    order: list[Fact] = []
    seen: set[Fact] = set()

    def emit(f: Fact) -> None:
        if f in seen:
            return
        seen.add(f)
        if f in reasons:
            for p in reasons[f][1]:
                emit(p)
            order.append(f)

    emit(goal_fact)
    return [{"fact": _describe(f), "rule": reasons[f][0],
             "from": [_describe(p) for p in reasons[f][1]]} for f in order]


# =========================================================================== #
# Numeric falsify-before-prove screen (reuses geometry.numeric_check).
# =========================================================================== #
def _numeric_screen(request: dict[str, Any]) -> dict[str, Any] | None:
    construction = request.get("construction")
    goal = request["goal"]
    if construction is None or goal["pred"] not in _NUMERIC_PREDS:
        return None
    res = geometry.numeric_check(
        construction, goal,
        seed=int(request["seed"]),
        trials=int(request.get("trials", 40)),
        tol=float(request.get("tol", 1e-6)),
        spread=float(request.get("spread", 10.0)),
    )
    if not res["holds"] and res.get("trials_valid", 0) > 0 and "counterexample" in res:
        return {
            "op": "geometry_ddar2",
            "proved": False,
            "falsified": True,
            "counterexample": res["counterexample"],
            "reason": "numeric counterexample: goal fails on a clean realization",
            "trials_valid": res["trials_valid"],
        }
    return None


def _hyp_label(h: dict[str, Any]) -> str:
    pred = h["pred"]
    pts = list(h.get("points", h.get("args", [])))
    base = f"{pred}({','.join(pts)}"
    for key in ("const", "deg", "ratio", "value"):
        if key in h:
            base += f",{key}={h[key]}"
    if "terms" in h:
        base += f",terms={h['terms']}"
    return base + ")"


def _fmt_frac(c: Fraction) -> str:
    return str(c.numerator) if c.denominator == 1 else f"{c.numerator}/{c.denominator}"


# =========================================================================== #
# Top-level prover.
# =========================================================================== #
def prove(hypotheses: list[dict[str, Any]], goal: dict[str, Any],
          request: dict[str, Any]) -> dict[str, Any]:
    # 1. numeric falsify-before-prove (optional; needs a construction)
    screen = _numeric_screen(request)
    if screen is not None:
        return screen

    # 2. build the three AR systems from hypotheses ...
    ar = UnifiedAR()
    for h in hypotheses:
        ar.add_pred(h["pred"], list(h.get("points", h.get("args", []))),
                    h, _hyp_label(h))

    # ... interleave a DD pass and feed derived geometric facts back into AR.
    facts, reasons = _dd_closure(hypotheses)
    for fact in facts:
        mapped = _fact_to_pred(fact)
        if mapped is not None:
            pred, pts = mapped
            ar.add_pred(pred, pts, {}, _describe(fact))

    pred = goal["pred"]
    pts = list(goal.get("points", goal.get("args", [])))

    # 3a. AR certificate (unified angle/length/position elimination)
    ar_result = ar.prove_pred(pred, pts, goal)
    # 3b. DD derivation (for geometry-canonicalizable goals)
    dd_deriv = None
    try:
        goal_fact = _canonical(pred, pts)
        dd_deriv = _dd_derivation(goal_fact, facts, reasons)
    except ValueError:
        pass

    proved = ar_result is not None or dd_deriv is not None
    result: dict[str, Any] = {
        "op": "geometry_ddar2",
        "proved": bool(proved),
        "falsified": False,
        "goal": {"pred": pred, "points": pts},
        "ar_angle_relations": ar.num_relations(ANGLE),
        "ar_length_relations": ar.num_relations(LENGTH),
        "ar_position_relations": ar.num_relations(POSITION),
    }
    if ar.inconsistent():
        result["inconsistent_hypotheses"] = True

    if ar_result is not None:
        system, cert = ar_result
        result["ar_system"] = system
        result["certificate"] = [
            {"coeff": _fmt_frac(c), "equation": label} for c, label in cert
        ]
        result["ar_method"] = (
            "exact incremental Gaussian elimination over fractions.Fraction: the "
            "goal's normalized linear form is the above rational combination of "
            f"hypothesis rows in the {system} change-of-variable group "
            "(angles = line directions mod pi with a pi constant; lengths = "
            "log-lengths with prime-factored constants; positions = signed sums)."
        )
    if dd_deriv is not None:
        result["derivation"] = dd_deriv

    if proved:
        result.setdefault("certificate_note", (
            "sound modulo non-degeneracy (no coincident points on a referenced "
            "line/segment) and, for angle constants, modulo the pi branch."
        ))
    else:
        result["inconclusive"] = True
        result["note"] = (
            "goal not reached by the unified AR systems or the DD pass. This is "
            "INCONCLUSIVE, not a disproof: supply a construction for the numeric "
            "screen, or try Wu's method (geometry_algebraic)."
        )
    return result


# =========================================================================== #
# Worker entrypoint.
# =========================================================================== #
def run(request: dict[str, Any]) -> dict[str, Any]:
    """Worker entrypoint (suggested op/tool name: ``geometry_ddar2``).

    Inputs
      * ``hypotheses`` -- list of predicate dicts (see below).
      * ``goal``       -- one predicate dict.
      * ``seed``       -- int seed for numeric ops / the falsify screen.
      * ``construction`` (optional) -- a :mod:`geometry` construction list; enables
        the numeric falsify-before-prove screen and the numeric ops.

    Predicate dict: ``{"pred": <name>, "points": [...], ...constants...}``. For
    constant-value predicates the numeric constant is a sibling key:
      * ``aconst``/``s_angle``: ``"const"`` in units of pi (e.g. ``"1/3"`` or
        ``"pi/3"``) or ``"deg"`` (e.g. ``60``).
      * ``rconst``/``lconst``: ``"const"`` a positive rational (e.g. ``"1/2"``).
      * ``distseq``: ``"terms": [[coeff, [p, q]], ...]`` and optional ``"const"``.

    Predicates (change-of-variable group)
      angle: ``parallel``, ``perpendicular``, ``eqangle(A,B,C,D,E,F)``,
      ``collinear``, ``aconst(A,B,C)``, ``s_angle(A,B,X)``.
      log-distance: ``cong``/``eqlen``, ``eqratio``, ``rconst(A,B,C,D)``,
      ``lconst(X,A)``.
      additive-position: ``distseq``.

    Ops
      * ``prove``   -- unified AR + interleaved DD. Returns ``proved`` and, when
        proved, a ``certificate`` (exact linear combination) and/or DD
        ``derivation``. Optional numeric pre-screen if ``construction`` given.
      * ``falsify`` -- reuses :func:`geometry.numeric_check`; returns
        ``falsified`` + ``counterexample`` (requires ``construction``).
      * ``check``   -- reuses :func:`geometry.numeric_check` (requires
        ``construction``).
    """
    op = request.get("op", "prove")

    if op == "prove":
        return prove(request.get("hypotheses", []), request["goal"], request)

    if op in ("check", "falsify"):
        result = geometry.numeric_check(
            request["construction"], request["goal"],
            seed=int(request["seed"]),
            trials=int(request.get("trials", 40)),
            tol=float(request.get("tol", 1e-6)),
            spread=float(request.get("spread", 10.0)),
        )
        if op == "falsify":
            return {
                "op": "geometry_ddar2",
                "sub_op": "falsify",
                "goal": result["goal"],
                "falsified": (not result["holds"]
                              and result["trials_valid"] > 0
                              and "counterexample" in result),
                "counterexample": result.get("counterexample"),
                "trials_valid": result["trials_valid"],
                "trials_degenerate": result["trials_degenerate"],
                "reason": result.get("reason"),
            }
        return {"op": "geometry_ddar2", "sub_op": "check", **result}

    raise ValueError(f"unknown op: {op!r}")
