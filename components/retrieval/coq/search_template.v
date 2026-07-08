(*
Theoremata retrieval (Rocq / Coq): one-shot premise-search theory.

This is the Coq analogue of retrieval/lean/dump_decls.lean -- a committed query
scaffold that the Python retriever fills in and compiles, rather than an inline
string baked into the module. It is rendered and run by
components/retrieval/python/theoremata_tools/rocq_retrieval.py (see `search`),
which substitutes the placeholders below, writes the result to a throwaway
TheoremataQuery.v, compiles it with `coqc` (native or via WSL), and parses the
printed premises back into the shared {name, module, kind, score} contract.

The leading comment block you are reading is a HEADER: rocq_retrieval.py strips
everything up to and including the first close-comment marker before compiling,
so the generated .v begins directly at the __IMPORTS__ line and is byte-for-byte
identical to what the module produced when the scaffold was an inline string.

Placeholders (substituted verbatim by rocq_retrieval._build_vfile):
  __IMPORTS__         One require line per requested library, newline-joined:
                      `Require Import Coq.Arith.Arith.` for dotted stdlib paths,
                      `From mathcomp Require Import ssrnat.` for mathcomp roots.
                      Defaults to Coq.Arith.Arith + Coq.Lists.List.
  __SEARCH_COMMAND__  The single search vernacular built from the query + mode:
                      `Search "add_comm".` (name-shaped),
                      `Search (?n + ?m = ?m + ?n).` (term pattern),
                      `SearchPattern (?n <= ?n).` / `SearchRewrite (...).`.
*)
__IMPORTS__
__SEARCH_COMMAND__
