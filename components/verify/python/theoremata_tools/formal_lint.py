"""Axiom-allowlist lint + ``#print axioms`` audit (soundness layer, TorchLean-style).

Where :mod:`formal_source_scan` (layer 2c) is a *lexical* pre-screen for the
escape hatches a proof text can use, this module *enforces an axiom allowlist*:
it rejects anything that depends on an axiom outside a per-system whitelist.
It is a direct port of TorchLean's ``scripts/checks/repo_lint.py`` discipline,
which allowlists the exact axiom names and fails CI on anything else.

It works on two complementary channels:

* **audit** -- the ground-truth output of the toolchain's assumption printer
  (Lean ``#print axioms <thm>``, Rocq ``Print Assumptions <thm>``, Isabelle
  ``thm_deps`` / oracle listing).  This is the *authoritative* check: it sees
  axioms pulled in transitively through imports, which a lexical scan cannot.
  Any listed axiom not in the allowlist (and any oracle / ``sorryAx``) fails.
* **source** -- a lexical layer (reusing :mod:`formal_source_scan`) that flags
  in-source ``axiom`` declarations and escape hatches (``sorry``,
  type-in-type, oracles, ...) before a build is even attempted.

Default allowlists (overridable per call):

* Lean:      ``{propext, Classical.choice, Quot.sound}`` (the Mathlib-blessed
  trio; nothing else -- ``sorryAx`` in particular is never allowed);
* Rocq:      ``{}`` strict by default (any ``Axiom`` / assumption fails);
* Isabelle:  ``{}`` strict by default (any oracle / added axiom fails).

A **BugZoo-style adversarial corpus** (:data:`BUGZOO`) of small known-bad
snippets per system -- hidden axioms, ``sorry``, type-in-type, oracles pulled
in by an external producer -- is shipped as a regression fixture: :func:`lint`
MUST flag every one of them, and pass the paired clean snippet.

Public API::

    lint(system, *, source=None, audit=None, axioms=None, allow=None) -> report
    parse_audit(system, audit) -> {"axioms": [...], "closed": bool}
    run(request) -> lint(...)   (worker dispatch, key ``formal_lint``)
"""
from __future__ import annotations

import json
import re
import sys
from typing import Any, Iterable

from .formal_source_scan import scan as _source_scan

CRITICAL = "critical"
WARNING = "warning"

SYSTEMS = ("lean", "rocq", "isabelle")

_ALIASES = {
    "coq": "rocq",
    "isabelle/hol": "isabelle",
    "hol": "isabelle",
    "lean4": "lean",
}

# Per-system default axiom allowlists.
DEFAULT_ALLOW: dict[str, frozenset[str]] = {
    # Mathlib's blessed trio; sorryAx / any custom axiom are rejected.
    "lean": frozenset({"propext", "Classical.choice", "Quot.sound"}),
    # Strict by default: any global assumption is a violation.
    "rocq": frozenset(),
    # Strict by default: any oracle / axiomatization is a violation.
    "isabelle": frozenset(),
}

# Axioms that are ALWAYS a violation regardless of the caller's allowlist:
# these are unsoundness holes, not honest classical axioms.
_NEVER_ALLOW: dict[str, frozenset[str]] = {
    "lean": frozenset({"sorryAx", "Lean.ofReduceBool", "Lean.ofReduceNat",
                       "Lean.trustCompiler"}),
    "rocq": frozenset(),
    "isabelle": frozenset({"Pure.skip_proof"}),
}


def _canon(system: str) -> str:
    key = system.strip().lower()
    key = _ALIASES.get(key, key)
    if key not in SYSTEMS:
        raise ValueError(
            f"unknown formal system {system!r}; expected one of {SYSTEMS}")
    return key


# --------------------------------------------------------------------------- #
# Audit parsers: extract the set of axioms/assumptions a theorem depends on
# from the toolchain's own assumption printer.
# --------------------------------------------------------------------------- #

# Lean:  'thm' depends on axioms: [propext, Classical.choice, sorryAx]
#   or:  'thm' does not depend on any axioms
_LEAN_DEPENDS = re.compile(
    r"depends on axioms\s*:\s*\[([^\]]*)\]", re.IGNORECASE)
_LEAN_NODEPS = re.compile(
    r"does not depend on any axioms", re.IGNORECASE)

# Rocq:  Print Assumptions -> "Axioms:\n name : type\n ..." or
#        "Closed under the global context"
_ROCQ_CLOSED = re.compile(r"Closed under the global context", re.IGNORECASE)
_ROCQ_SECTION = re.compile(
    r"^\s*(Axioms|Parameters|Variables|Hypotheses|Opaque|Answers?|"
    r"Transparent|Axiom)\s*:\s*$", re.IGNORECASE)
# "name : Type ..."  (assumption entry; name is the leading identifier)
_ROCQ_ENTRY = re.compile(r"^\s*([A-Za-z_][\w'.]*)\s*:")

# Isabelle:  thm_deps / oracle listing -> "oracles: {skip_proof, ...}" or a
#   list of axioms;  "no oracles" / empty means clean.
_ISA_ORACLES = re.compile(r"oracles?\s*[:=]\s*\{([^}]*)\}", re.IGNORECASE)
_ISA_AXIOMS = re.compile(r"axioms?\s*[:=]\s*\{([^}]*)\}", re.IGNORECASE)


def _split_names(blob: str) -> list[str]:
    return [t.strip() for t in re.split(r"[,\s]+", blob) if t.strip()]


def parse_audit(system: str, audit: str) -> dict[str, Any]:
    """Parse an assumption-printer transcript into a set of axiom names.

    Returns ``{"axioms": [...sorted...], "closed": bool, "parsed": bool}``
    where ``closed`` means the printer explicitly reported *no* dependencies.
    """
    key = _canon(system)
    axioms: set[str] = set()
    closed = False
    parsed = False

    if key == "lean":
        if _LEAN_NODEPS.search(audit):
            closed = True
            parsed = True
        for m in _LEAN_DEPENDS.finditer(audit):
            parsed = True
            axioms.update(_split_names(m.group(1)))
    elif key == "rocq":
        if _ROCQ_CLOSED.search(audit):
            closed = True
            parsed = True
        in_section = False
        for line in audit.splitlines():
            if _ROCQ_SECTION.match(line):
                in_section = True
                parsed = True
                continue
            if in_section:
                m = _ROCQ_ENTRY.match(line)
                if m:
                    axioms.add(m.group(1))
                elif line.strip() == "":
                    continue
                else:
                    in_section = False
    else:  # isabelle
        for m in _ISA_ORACLES.finditer(audit):
            parsed = True
            axioms.update(_split_names(m.group(1)))
        for m in _ISA_AXIOMS.finditer(audit):
            parsed = True
            axioms.update(_split_names(m.group(1)))
        if re.search(r"no oracles", audit, re.IGNORECASE):
            closed = True
            parsed = True

    return {"axioms": sorted(axioms), "closed": closed, "parsed": parsed}


# --------------------------------------------------------------------------- #
# The lint.
# --------------------------------------------------------------------------- #

def lint(system: str, *, source: str | None = None, audit: str | None = None,
         axioms: Iterable[str] | None = None,
         allow: Iterable[str] | None = None) -> dict[str, Any]:
    """Enforce an axiom allowlist across the audit and/or source channels.

    At least one of ``source``, ``audit`` or ``axioms`` should be supplied.
    ``allow`` overrides the per-system default allowlist (:data:`DEFAULT_ALLOW`);
    the hard :data:`_NEVER_ALLOW` set (``sorryAx``, oracles, ...) is always
    rejected even if the caller tries to allow it.

    Returns ``{clean, system, allowlist, axioms, violations: [...], source_scan}``
    where ``clean`` is True iff there are zero ``critical`` violations.
    """
    key = _canon(system)
    allowset = set(DEFAULT_ALLOW[key] if allow is None else allow)
    never = _NEVER_ALLOW[key]
    allowset -= never  # a caller can never re-permit an unsoundness hole

    violations: list[dict[str, Any]] = []
    seen_axioms: set[str] = set()
    audit_info: dict[str, Any] | None = None

    # --- audit channel (authoritative) ---
    audit_axioms: set[str] = set()
    if audit is not None:
        audit_info = parse_audit(key, audit)
        audit_axioms.update(audit_info["axioms"])
    if axioms is not None:
        audit_axioms.update(str(a) for a in axioms)
    for ax in sorted(audit_axioms):
        seen_axioms.add(ax)
        if ax in never:
            violations.append({
                "kind": "forbidden_axiom", "channel": "audit",
                "axiom": ax, "severity": CRITICAL,
                "reason": "unsoundness hole (never allowed)",
            })
        elif ax not in allowset:
            violations.append({
                "kind": "axiom_not_in_allowlist", "channel": "audit",
                "axiom": ax, "severity": CRITICAL,
                "reason": f"axiom {ax!r} outside allowlist",
            })

    # --- source channel (lexical pre-screen, reuses layer 2c) ---
    source_scan: dict[str, Any] | None = None
    if source is not None:
        source_scan = _source_scan(key, source)
        for flag in source_scan["flags"]:
            violations.append({
                "kind": "source_escape_hatch", "channel": "source",
                "pattern": flag["pattern"], "severity": flag["severity"],
                "line": flag["line"], "snippet": flag["snippet"],
            })

    critical = [v for v in violations if v["severity"] == CRITICAL]
    return {
        "clean": len(critical) == 0,
        "system": key,
        "allowlist": sorted(allowset),
        "axioms": sorted(seen_axioms),
        "audit": audit_info,
        "violations": violations,
        "source_scan": source_scan,
    }


# --------------------------------------------------------------------------- #
# BugZoo: adversarial regression corpus of external-producer bugs.
#
# Every entry MUST be flagged by lint(); the paired "clean" entries MUST pass.
# ``channel`` says which input the bug arrives on.  These model the class of
# defect a black-box external prover/producer can smuggle past a naive gate:
# hidden axioms, sorry, type-in-type, oracles.
# --------------------------------------------------------------------------- #

BUGZOO: dict[str, list[dict[str, Any]]] = {
    "lean": [
        {"name": "hidden_sorry", "channel": "source", "why": "sorry hole",
         "source": "theorem bad : False := by\n  sorry\n"},
        {"name": "sorryAx_audit", "channel": "audit",
         "why": "sorryAx surfaces in #print axioms",
         "audit": "'bad' depends on axioms: [propext, sorryAx, "
                  "Classical.choice]\n"},
        {"name": "hidden_axiom_decl", "channel": "source",
         "why": "custom axiom declared in source",
         "source": "axiom cheat : (0 : Nat) = 1\n"
                   "theorem bad : (0:Nat) = 1 := cheat\n"},
        {"name": "custom_axiom_audit", "channel": "audit",
         "why": "non-allowlisted axiom pulled in via imports",
         "audit": "'thm' depends on axioms: [propext, Classical.choice, "
                  "myProject.unsafeAxiom]\n"},
        {"name": "native_decide", "channel": "source",
         "why": "native_decide trusts the compiler",
         "source": "theorem bad : True := by native_decide\n"},
        {"name": "ofReduceBool_audit", "channel": "audit",
         "why": "Lean.ofReduceBool is an unsoundness hole",
         "audit": "'bad' depends on axioms: [Lean.ofReduceBool]\n"},
    ],
    "rocq": [
        {"name": "admitted", "channel": "source", "why": "Admitted proof",
         "source": "Lemma bad : False.\nProof.\nAdmitted.\n"},
        {"name": "axiom_decl", "channel": "source", "why": "declared Axiom",
         "source": "Axiom cheat : False.\n"},
        {"name": "type_in_type", "channel": "source",
         "why": "universe checking disabled",
         "source": "Unset Universe Checking.\n"},
        {"name": "assumption_audit", "channel": "audit",
         "why": "Print Assumptions lists a global axiom",
         "audit": "Axioms:\nclassic : forall P : Prop, P \\/ ~ P\n"},
    ],
    "isabelle": [
        {"name": "sorry", "channel": "source", "why": "sorry (skip_proof)",
         "source": 'lemma bad: "False"\n  sorry\n'},
        {"name": "axiomatization", "channel": "source",
         "why": "axiomatization injects facts",
         "source": 'axiomatization where cheat: "False"\n'},
        {"name": "oracle_audit", "channel": "audit",
         "why": "theorem carries the skip_proof oracle",
         "audit": "oracles: {Pure.skip_proof}\n"},
    ],
}

# Paired clean snippets that MUST pass the lint for each system.
CLEAN_FIXTURES: dict[str, dict[str, Any]] = {
    "lean": {
        "source": "theorem good : 1 + 1 = 2 := by rfl\n",
        "audit": "'good' depends on axioms: [propext, Classical.choice, "
                 "Quot.sound]\n",
    },
    "rocq": {
        "source": "Lemma good : 1 + 1 = 2.\nProof. reflexivity. Qed.\n",
        "audit": "Closed under the global context\n",
    },
    "isabelle": {
        "source": 'lemma good: "1 + 1 = (2::nat)" by simp\n',
        "audit": "no oracles\n",
    },
}


def lint_bugzoo_entry(system: str, entry: dict[str, Any]) -> dict[str, Any]:
    """Run :func:`lint` on a single BugZoo entry using its declared channel."""
    kwargs: dict[str, Any] = {}
    if "source" in entry:
        kwargs["source"] = entry["source"]
    if "audit" in entry:
        kwargs["audit"] = entry["audit"]
    return lint(system, **kwargs)


# --------------------------------------------------------------------------- #
# Worker dispatch.
# --------------------------------------------------------------------------- #

def run(request: dict[str, Any]) -> dict[str, Any]:
    """Worker entrypoint (key ``formal_lint``).

    ``{system, source?, audit?, axioms?, allow?}`` -> :func:`lint` report.
    Special ``op == "bugzoo"`` runs the whole adversarial corpus and reports
    per-entry results (all bad flagged, all clean passing).
    """
    if request.get("op") == "bugzoo":
        results = []
        all_ok = True
        for system, entries in BUGZOO.items():
            for entry in entries:
                rep = lint_bugzoo_entry(system, entry)
                flagged = not rep["clean"]
                all_ok = all_ok and flagged
                results.append({"system": system, "name": entry["name"],
                                "flagged": flagged, "why": entry["why"]})
            clean = lint(system, **CLEAN_FIXTURES[system])
            passed = clean["clean"]
            all_ok = all_ok and passed
            results.append({"system": system, "name": "CLEAN",
                            "flagged": False, "passed": passed})
        return {"ok": all_ok, "results": results}

    return lint(
        request["system"],
        source=request.get("source"),
        audit=request.get("audit"),
        axioms=request.get("axioms"),
        allow=request.get("allow"),
    )


def main() -> None:
    if len(sys.argv) >= 2:
        with open(sys.argv[1], encoding="utf-8") as fh:
            req = json.load(fh)
    else:
        req = json.load(sys.stdin)
    result = run(req)
    print(json.dumps(result, indent=2, ensure_ascii=False))
    ok = result.get("ok", result.get("clean", False))
    raise SystemExit(0 if ok else 1)


if __name__ == "__main__":
    main()
