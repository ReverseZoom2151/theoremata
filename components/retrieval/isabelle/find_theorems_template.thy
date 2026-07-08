(*
Theoremata retrieval (Isabelle): one-shot find_theorems premise-search theory.

This is the Isabelle analogue of retrieval/lean/dump_decls.lean -- a committed
query scaffold that the Python retriever fills in and runs, rather than an inline
string baked into the module. It is rendered and run by
components/retrieval/python/theoremata_tools/isabelle_retrieval.py (see
`_theory_text` / `search`), which substitutes the placeholders below, writes the
result to a throwaway Scratch.thy, runs it through
`isabelle process_theories -O` (native or via WSL), and parses the printed
`name: statement` matches into the shared {name, module, kind, score} contract.

The leading comment block you are reading is a HEADER: isabelle_retrieval.py
strips everything up to and including the first close-comment marker before
running, so the generated theory begins directly at `theory Scratch` and is
byte-for-byte identical to what the module produced when the scaffold was an
inline string.

The theory imports Main (HOL); the requested `session` selects the object logic
at the driver level and is informational here, so it does not appear below.

Placeholders (substituted verbatim by isabelle_retrieval._theory_text):
  __LIMIT__     Max results, the (N) in `find_theorems (N) ...` (integer >= 1).
  __CRITERIA__  The find_theorems criterion built from the query + mode:
                `"_ + 0 = _"` (bare term pattern), `name: "add_commute"`
                (name mode), or a goal-relative word such as `intro "..."`.
*)
theory Scratch
  imports Main
begin
find_theorems (__LIMIT__) __CRITERIA__
end
