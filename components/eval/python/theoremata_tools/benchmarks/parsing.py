"""Text extractors for the formalization corpora.

Three parsers, each deliberately dependency-free (regex only) so ingestion runs
with no Lean toolchain present:

* :func:`parse_blueprint_nodes`  — leanblueprint ``\\label/\\lean/\\uses/\\leanok``
  theorem-environment DAG nodes (ZkLinalg, strongpnt, Kakeya, RHCurves,
  FrontierMath-Hypergraphs, Erdos1196, Sphere-Packing).
* :func:`parse_fqb_main`         — a FormalQualBench ``Main.lean`` stub
  (``namespace`` + docstring + ``theorem MainTheorem … := by sorry``).
* :func:`extract_sorry_obligations` — every ``sorry``-bearing decl in a Lean
  file, with comments/strings masked so commented-out ``sorry``s don't count.
"""
from __future__ import annotations

import re
from typing import Any

# --------------------------------------------------------------------------- #
# leanblueprint theorem-environment parser
# --------------------------------------------------------------------------- #

_NODE_ENVS = ("theorem", "lemma", "proposition", "corollary", "definition", "claim")
_ENV_RE = re.compile(
    r"\\begin\{(?P<env>" + "|".join(_NODE_ENVS) + r")\}(?P<body>.*?)\\end\{(?P=env)\}",
    re.DOTALL,
)
_LABEL_RE = re.compile(r"\\label\{([^}]*)\}")
_LEAN_RE = re.compile(r"\\lean\{([^}]*)\}")
_USES_RE = re.compile(r"\\uses\{([^}]*)\}")
_LEANOK_RE = re.compile(r"\\leanok\b")
# macros to strip when recovering the human statement text
_STRIP_MACROS = re.compile(
    r"\\(label|lean|uses|proves|leanok|mathlibok|notready|discussion|todo)\b(\{[^}]*\})?"
)


def _clean_statement(body: str) -> str:
    """Recover readable prose from a theorem-env body: drop the blueprint macros
    and the optional ``[Name]`` tag, collapse whitespace."""
    text = _STRIP_MACROS.sub("", body)
    text = re.sub(r"^\s*\[[^\]]*\]", "", text.strip())  # leading [Nickname]
    return re.sub(r"\s+", " ", text).strip()


def parse_blueprint_nodes(tex: str) -> list[dict[str, Any]]:
    """Extract blueprint DAG nodes from one or more concatenated ``.tex`` files.

    Only environments carrying a ``\\label`` are nodes (the blueprint contract).
    Returns one dict per node with keys ``label, env, lean_names, uses,
    leanok, statement``.
    """
    nodes: list[dict[str, Any]] = []
    for m in _ENV_RE.finditer(tex):
        body = m.group("body")
        label_m = _LABEL_RE.search(body)
        if not label_m:
            continue  # unlabeled env is not a DAG node
        lean_names: list[str] = []
        for lm in _LEAN_RE.finditer(body):
            lean_names.extend(n.strip() for n in lm.group(1).split(",") if n.strip())
        uses: list[str] = []
        for um in _USES_RE.finditer(body):
            uses.extend(u.strip() for u in um.group(1).split(",") if u.strip())
        nodes.append(
            {
                "label": label_m.group(1).strip(),
                "env": m.group("env"),
                "lean_names": lean_names,
                "uses": uses,
                "leanok": bool(_LEANOK_RE.search(body)),
                "statement": _clean_statement(body),
            }
        )
    return nodes


# --------------------------------------------------------------------------- #
# FormalQualBench Main.lean parser
# --------------------------------------------------------------------------- #

_NAMESPACE_RE = re.compile(r"^\s*namespace\s+([A-Za-z_][\w'.]*)", re.MULTILINE)
_DOCSTRING_RE = re.compile(r"/--(.*?)-/", re.DOTALL)
_MAIN_THM_RE = re.compile(
    r"(theorem|lemma|def)\s+MainTheorem\b(?P<sig>.*?):=\s*by",
    re.DOTALL,
)


def parse_fqb_main(text: str) -> dict[str, Any] | None:
    """Parse a FormalQualBench ``Main.lean`` stub. Returns ``None`` if it has no
    ``MainTheorem`` (defensive; all 23 do)."""
    ns_m = _NAMESPACE_RE.search(text)
    namespace = ns_m.group(1) if ns_m else "FormalQualBench"

    thm_m = _MAIN_THM_RE.search(text)
    if not thm_m:
        return None
    # the docstring immediately preceding MainTheorem (last one before it)
    docstring = ""
    for dm in _DOCSTRING_RE.finditer(text, 0, thm_m.start()):
        docstring = dm.group(1).strip()
    signature = re.sub(r"\s+", " ", thm_m.group("sig")).strip()
    formal = f"theorem MainTheorem {signature} := by sorry"
    return {
        "namespace": namespace,
        "id": f"{namespace}.MainTheorem",
        "docstring": docstring,
        "formal": formal,
    }


# --------------------------------------------------------------------------- #
# Lean sorry-obligation extractor
# --------------------------------------------------------------------------- #

_LINE_COMMENT_RE = re.compile(r"--[^\n]*")
_DECL_RE = re.compile(
    r"(?m)^(?:@\[[^\]]*\]\s*)?"
    r"(?:noncomputable\s+|private\s+|protected\s+|scoped\s+|local\s+)*"
    r"(theorem|lemma|def|instance|example)\s+(?P<name>[A-Za-z_][\w'.]*)?"
)
_SORRY_RE = re.compile(r"\bsorry\b")


def _mask_comments(text: str) -> str:
    """Blank out ``/- … -/`` block comments (nested) and ``-- …`` line comments,
    preserving offsets so decl positions stay valid."""
    out = list(text)
    # block comments (nested)
    i = 0
    depth = 0
    n = len(text)
    while i < n - 1:
        two = text[i] + text[i + 1]
        if two == "/-":
            depth += 1
            out[i] = out[i + 1] = " "
            i += 2
            continue
        if two == "-/" and depth > 0:
            depth -= 1
            out[i] = out[i + 1] = " "
            i += 2
            continue
        if depth > 0 and text[i] != "\n":
            out[i] = " "
        i += 1
    masked = "".join(out)
    # line comments (only outside the now-blanked block comments)
    masked = _LINE_COMMENT_RE.sub(lambda m: " " * len(m.group(0)), masked)
    return masked


def extract_sorry_obligations(text: str) -> list[dict[str, Any]]:
    """Return every ``sorry``-bearing declaration in a Lean source file.

    Each entry: ``{name, kind, signature, line}``. Commented-out ``sorry``s and
    ``:= sorry`` inside comments are ignored via comment masking.
    """
    masked = _mask_comments(text)
    decls = list(_DECL_RE.finditer(masked))
    results: list[dict[str, Any]] = []
    for idx, m in enumerate(decls):
        start = m.start()
        end = decls[idx + 1].start() if idx + 1 < len(decls) else len(masked)
        if not _SORRY_RE.search(masked[start:end]):
            continue
        name = m.group("name") or f"anon_{start}"
        # signature: from the decl keyword up to the proof separator ':=' / 'where'
        span = text[start:end]
        sig = re.split(r":=|\bwhere\b", span, maxsplit=1)[0]
        sig = re.sub(r"\s+", " ", sig).strip()
        results.append(
            {
                "name": name,
                "kind": m.group(1),
                "signature": sig,
                "line": text.count("\n", 0, start) + 1,
            }
        )
    return results
