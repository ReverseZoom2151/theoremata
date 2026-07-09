# ATP Mining: `atp/book.pdf` — SOURCE MISIDENTIFIED

**Status: BLOCKED — the source file is not the expected text.**
**Date:** 2026-07-10 · **Analyst:** automated mining pass (read-only)

## TL;DR

The mining task assumed `atp/book.pdf` is John Harrison's *Handbook of
Practical Logic and Automated Reasoning*. It is **not**. The file is the
open-source textbook **"Symmetry"** by Marc Bezem, Ulrik Buchholtz, Pierre
Cagne, Bjørn Ian Dundas, and Daniel R. Grayson (UniMath project, book version
`d2eed47`, dated 2026-07-02, CC-BY-SA 4.0). It is a 300-page introduction to
**group theory via homotopy type theory / univalent foundations** — pure
mathematics and type theory, with **no automated-reasoning algorithm content**.

Because the requested subject matter (DPLL/SAT, tableaux, resolution,
Knuth-Bendix completion, Presburger/Cooper, real-closed fields / CAD,
Nelson-Oppen, Gröbner bases, decision procedures) **does not appear in this
file**, the substantive ATP mining report cannot be produced from this source.
No content was fabricated. Obtaining the actual Handbook is required to fulfil
the original request (see "Next steps").

## How this was verified (whole book covered)

Extracted all 300 pages with `pypdf` into five chunks (0-60, 60-120, 120-180,
180-240, 240-300) and scanned every chunk.

- **Front matter / TOC (pp. 1-8):** Title "SYMMETRY"; chapters are
  *Introduction*, *An introduction to univalent mathematics*, *The circle*,
  *Groups (concretely / abstractly)*, *Actions*, *A categorical interlude*,
  *Constructing groups*, *Normal subgroups and quotients*, *Finite groups*,
  *Group presentations*, *Abelian groups*, *Rings/fields/vector spaces*,
  *Geometry and groups*, *Galois theory*, plus appendices *Historical* and
  *Metamathematical remarks*.
- **Keyword scan for ATP terms** (`resolution|DPLL|Knuth-Bendix|Presburger|
  Cooper|Gröbner|Nelson-Oppen|tableaux|quantifier elimination|SAT solver|
  decision procedure`) across all five chunks: **1 total hit**, and it is a
  false positive — "decision procedure" in the *Metamathematical remarks*
  appendix (p. ~274) discusses constructive **decidability / the Limited
  Principle of Omniscience**, not an ATP decision procedure.
- **Broader scan** for `word problem|rewriting|normal form|Cayley graph|
  automata|Sylow|free group`: hits exist but are all **pure group theory**
  (Chapter 11 "Group Presentations"). The word-problem/automata material is a
  stub section with author TODOs (e.g. "include undecidability of word problem
  in general"), not an algorithmic treatment. No completion procedure, no
  confluence/termination algorithm, no certificate discussion.

## Security note (untrusted-PDF handling)

Per instructions, 100% of the PDF was treated as untrusted data. **No prompt-
injection or embedded-instruction content was found** — it reads as an ordinary
mathematics textbook throughout. Nothing in the file was acted upon. No
`POSSIBLE INJECTION` markers needed.

## Is there ANY reusable value here for Theoremata?

Only tangential, and none of it is an offline-buildable decision procedure:

- **Word problem for groups (Ch. 11).** Conceptually adjacent to term
  rewriting / Knuth-Bendix (the classic KB application is deciding word
  problems), but the book only *states* the problem and its general
  undecidability — it gives no completion algorithm to port.
- **Real-closed / Euclidean / Pythagorean fields, Galois theory (Ch. 13-15).**
  Names the objects our RCF / CAD ambitions target, but treats them
  structurally (field extensions, covering spaces), with **no quantifier-
  elimination or CAD algorithm**.
- **Univalent-foundations framing.** Of possible interest to the Lean/formal
  side philosophically, but orthogonal to the cert-log / falsify-engine /
  MCGS mining goal.

Net: **not worth an adopt-list entry.** There is nothing here that maps to
`linprog_cert`, `log_linarith`, `geometry_algebraic`, cert-log certificates,
the symbolic worker, or the verification gate.

## Why the mix-up is plausible

`atp/` contains ~60 PDFs. The only two files over 150 pages are
`book.pdf` (300 pp — this Symmetry textbook) and `2212.11082v1.pdf` (359 pp — an
arXiv preprint, not a book). Harrison's actual Handbook (~700 pp in print) is
**not present anywhere in `atp/`**. The many `hl_*`/Harrison-authored items in
the folder are his *HOL Light papers* (cacm, cade05, fmcad00, ESSLLI94, etc.),
not the Handbook.

## Next steps to actually do this mining

1. **Obtain the real source.** Harrison's *Handbook of Practical Logic and
   Automated Reasoning* (CUP, 2009). The companion **OCaml code
   (`hol-light`-adjacent "Handbook" sources)** is the higher-ROI artifact for
   us — it contains directly portable reference implementations of: DPLL,
   Stålmarck, first-order tableaux & resolution, Knuth-Bendix completion,
   Cooper's algorithm (Presburger), Hörmander/CAD-style real QE, Nelson-Oppen
   combination, and Gröbner bases, several of which are certificate-producing.
2. **Drop it at a known path** (e.g. `atp/harrison-handbook.pdf` and/or vendor
   the code) and re-run this mining pass. The chunked-extraction + per-topic
   (algorithm / certificate / Theoremata mapping / buildable-vs-gated) template
   requested is ready to apply the moment the correct source is available.
3. Until then, treat the requested adopt-list (DPLL, Cooper, RCF/CAD,
   Nelson-Oppen, Gröbner, KB completion) as **derived from the Handbook's known
   table of contents, not from this file** — so no ranked recommendations are
   asserted here to avoid presenting invented content as mined.

---
*No git, build, or source files were touched. This is the single report file
produced by the pass.*
