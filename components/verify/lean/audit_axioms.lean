/-
Copyright (c) 2026 The Tau Ceti contributors. All rights reserved.
Released under Apache 2.0 license as described in the file LICENSE.

ATTRIBUTION AND PROVENANCE
==========================
This file is a MODIFIED PORT of `scripts/Axioms.lean` from the Tau Ceti project
(https://github.com/TauCetiProject, Apache License, Version 2.0). The upstream repository ships the
Apache-2.0 licence text with unfilled appendix placeholders and no NOTICE file, so no
NOTICE obligation is inherited; the project-wide copyright line above is reproduced
because the upstream per-file headers are real and must travel with the code. A copy of
the Apache-2.0 licence is available at http://www.apache.org/licenses/LICENSE-2.0 .

The upstream file itself carries two further attributions, reproduced here as required:

  * `withImportedEnv` is inlined from `importGraph`'s `Core.withImportModules`
    (Kim Morrison, Paul Lezeau; Apache 2.0).
  * The shared-memo axiom reachability pass is adapted from Robin Arnez's mathlib-wide
    version (leanprover Zulip, #general, "Checking which axioms are used in a project").

WHAT THIS PORT CHANGED relative to upstream `scripts/Axioms.lean`
----------------------------------------------------------------
  1. The memo carries the EXACT axiom set per constant (`NameMap (Array Name)`) rather
     than upstream's collapsed `Bool`. Theoremata's gate must report the closure itself,
     not just a clean/dirty bit, because the reported closure is cross-validated against
     the pre-existing `#print axioms` path. The sets are tiny (a handful of names) and the
     memo is still shared across every audited declaration, so the asymptotics upstream
     cared about are unchanged.
  2. The audited root is not hardcoded to one library. Two entry points are provided: a
     `#audit_axioms` command for auditing named declarations elaborated in the current
     file (Theoremata generates proofs ad hoc; it has no prebuilt library to import), and
     a `main` executable that reproduces upstream's "audit every declaration defined under
     a module prefix" behaviour.
  3. Output is machine-readable, one JSON object per line behind a fixed marker, because
     the caller is Python rather than a human reading CI logs.
  4. The allowlist is not applied inside the traversal. This file reports the closure; the
     allowlist decision is made by the caller, so a single audit run can serve callers with
     different allowlists without the traversal silently encoding one of them.

Everything the upstream mining pass flagged as load-bearing is preserved and marked
`WHY:` below: strings materialized inside the import callback, an exit code instead of
`IO.Process.exit`, and the fail-loudly-on-zero-declarations guard.
-/
import Lean

open Lean

namespace Theoremata.AxiomAudit

/-- Marker prefixing every per-declaration result line. Callers key on this rather than on
free-form text so that unrelated Lean chatter on stdout can never be mistaken for a
result. -/
def resultMarker : String := "THEOREMATA_AXIOM_AUDIT"

/-- Marker prefixing the single summary line. Its absence is itself a failure signal: a run
that produced no summary did not complete, and the caller must not read that as clean. -/
def summaryMarker : String := "THEOREMATA_AXIOM_AUDIT_SUMMARY"

/-- Shared-memo state for the axiom reachability pass. The `Environment` is read-only; the
`NameMap (Array Name)` memoizes, for every constant visited, its sorted transitive axiom
closure.

WHY one map threaded across all candidates: the stock `Lean.collectAxioms` rebuilds its
state on every invocation, so auditing n declarations re-walks the shared library closure n
times. Upstream measured that at roughly 77ms per declaration, which dominated their audit.
One shared memo makes the whole audit near-linear in the reachable closure instead. -/
abbrev AxiomCacheM := ReaderT Environment (StateM (NameMap (Array Name)))

/-- The sorted transitive axiom closure of `c`, memoized.

The case split mirrors `Lean.CollectAxioms.collect` in the toolchain so that this and
`#print axioms` agree by construction; the differences are the shared memo and returning
the set rather than accumulating into ambient state.

WHY a constant is memoized as `#[]` before recursing: the constant dependency graph is
acyclic for definitions and theorems (a declaration can only mention earlier ones) but NOT
for inductive families, where an inductive's type and its constructors' types refer to each
other. The sentinel makes those cycles terminate. Upstream documents the resulting
limitation and it is preserved honestly here: inside such a cycle a member may be memoized
with an under-approximated set. It cannot hide a violation from the declaration that
directly mentions an axiom, since that declaration's own edge to the axiom leaf is not a
back edge; it can only under-report for other members of the same inductive cluster, whose
types are the only place this arises and which do not carry proof axioms in practice. -/
partial def closure (c : Name) : AxiomCacheM (Array Name) := do
  if let some axs := (← get).find? c then
    return axs
  modify (·.insert c #[])
  let env ← read
  let ofExprs (es : Array Expr) : AxiomCacheM NameSet :=
    es.foldlM (init := ({} : NameSet)) fun acc e =>
      e.getUsedConstants.foldlM (init := acc) fun acc' n => do
        return (← closure n).foldl (init := acc') NameSet.insert
  let ofNames (ns : List Name) (acc : NameSet) : AxiomCacheM NameSet :=
    ns.foldlM (init := acc) fun acc' n => do
      return (← closure n).foldl (init := acc') NameSet.insert
  -- WHY `env.checked.get`: the kernel environment, not the elaboration environment. Under
  -- asynchronous elaboration the two can disagree exactly when something went wrong, and
  -- the kernel's view is the one the gate is entitled to trust. This matches the toolchain.
  let res : NameSet ← match env.checked.get.find? c with
    | some (.axiomInfo v)  => (fun s => s.insert c) <$> ofExprs #[v.type]
    | some (.defnInfo v)   => ofExprs #[v.type, v.value]
    | some (.thmInfo v)    => ofExprs #[v.type, v.value]
    | some (.opaqueInfo v) => ofExprs #[v.type, v.value]
    | some (.quotInfo _)   => pure {}
    | some (.ctorInfo v)   => ofExprs #[v.type]
    | some (.recInfo v)    => ofExprs #[v.type]
    | some (.inductInfo v) => do ofNames v.ctors (← ofExprs #[v.type])
    -- WHY `none` is empty rather than an error: an unknown name here is a name the kernel
    -- environment does not have, which the callers below reject up front. Reaching this
    -- case from inside a traversal would mean a dangling reference in an already-checked
    -- environment. See the caller-side guards; nothing downstream treats this as clean.
    | none                 => pure {}
  let arr := res.toArray.qsort Name.lt
  modify (·.insert c arr)
  return arr

/-- Known limitation, recorded rather than papered over: `Lean.collectAxioms` has been
reported to miss axioms reachable only through the TYPE of another axiom
(leanprover/lean4 issue 8840). This port walks `v.type` for `axiomInfo` exactly as the
toolchain does, so it inherits whatever the installed toolchain's behaviour is, no better
and no worse. Theoremata's gate documentation carries the same note. Callers must not treat
"the audit says clean" as stronger than the toolchain's own guarantee. -/
def issue8840Note : String :=
  "collectAxioms may miss axioms reachable only via an axiom's type (lean4 issue 8840)"

private def escape (s : String) : String :=
  s.foldl (init := "") fun acc ch =>
    match ch with
    | '"'  => acc ++ "\\\""
    | '\\' => acc ++ "\\\\"
    | '\n' => acc ++ "\\n"
    | c    => acc.push c

/-- Render one declaration's result as a marker-prefixed JSON line.

WHY the rendering happens here, eagerly, rather than by handing `Name`s back to the caller:
declaration and axiom `Name`s loaded from `.olean`s live in a memory-mapped region that is
unmapped when `withImportModules` returns its callback. Formatting them after the callback
has returned is a use-after-free. Every string this module emits is materialized inside the
callback for that reason. -/
def renderLine (decl : Name) (axs : Array Name) : String :=
  let items := axs.map (fun a => "\"" ++ escape a.toString ++ "\"")
  let joined := String.intercalate "," items.toList
  resultMarker ++ " {\"decl\":\"" ++ escape decl.toString ++ "\",\"axioms\":[" ++ joined ++ "]}"

/-- Render the summary line. `audited` is the number of declarations actually walked. -/
def renderSummary (audited : Nat) : String :=
  summaryMarker ++ " {\"audited\":" ++ toString audited ++ "}"

/-- Audit `names` against `env` in a single shared-memo pass, returning fully materialized
output lines. Returns `none` when there is nothing to audit.

WHY `none` rather than an empty success: see `zeroDeclDiagnostic`. -/
def auditLines (env : Environment) (names : Array Name) : Option (Array String) :=
  if names.isEmpty then
    none
  else
    let results := (names.mapM (fun n => do return (n, ← closure n)) : AxiomCacheM _)
      |>.run env |>.run' {}
    let lines := results.map (fun (n, axs) => renderLine n axs)
    some (lines.push (renderSummary names.size))

/-- The message emitted when zero declarations were audited.

WHY this is the most important line in the file: an audit that examines nothing and reports
clean is a fail-open soundness bug, and this project has shipped exactly that bug before (a
Metamath audit that returned `within_whitelist = true` unconditionally). A miswired caller,
an empty target list, or a module prefix that matches nothing must be LOUD and must be
distinguishable by the caller from a clean audit. It is therefore reported on stderr, with
no summary line, and with a non-zero exit code. -/
def zeroDeclDiagnostic (context : String) : String :=
  "THEOREMATA_AXIOM_AUDIT_ERROR audited 0 declarations (" ++ context ++
    "): the audit is miswired and this is NOT a clean result."

open Lean Elab Command in
/-- `#audit_axioms foo bar` prints the transitive axiom closure of each named declaration,
computed in one shared-memo pass over the current environment.

This is the entry point Theoremata uses: proofs are elaborated ad hoc rather than built into
an importable library, so the audit runs in the same file as the theorems.

Fail-closed behaviour: an unresolvable name is a hard elaboration error (non-zero exit, no
summary line); an empty name list trips the zero-declaration guard. -/
elab "#audit_axioms" ids:(ppSpace colGt ident)* : command => do
  let names ← liftTermElabM <| ids.mapM fun i => realizeGlobalConstNoOverload i
  let env ← getEnv
  -- WHY re-check membership in the kernel environment: `realizeGlobalConstNoOverload`
  -- resolves against the elaboration environment. A name that resolves but is absent from
  -- the kernel environment means elaboration did not complete for it, which must fail
  -- rather than silently produce an empty closure via the `none` branch of `closure`.
  for n in names do
    if (env.checked.get.find? n).isNone then
      throwError "#audit_axioms: '{n}' is not present in the kernel environment"
  match auditLines env names with
  | none =>
      throwError zeroDeclDiagnostic "no declarations named in #audit_axioms"
  | some lines =>
      for l in lines do
        logInfo l

end Theoremata.AxiomAudit

-- THEOREMATA_SPLICE_END
-- Everything above this marker is spliced verbatim into generated proof files by the
-- Python driver. Everything below is the standalone executable path and must not be
-- spliced, since a generated file may legitimately define its own `main`.

namespace Theoremata.AxiomAudit

/-- Build the environment from `modules` and run `act` in `CoreM`. Inlined from
`importGraph`'s `Core.withImportModules` (Kim Morrison, Paul Lezeau; Apache 2.0).

`trustLevel := 1024` means imported constants are taken as type-correct rather than
re-checked. That is correct here because Theoremata's gate kernel-rechecks separately
(layer 3); this layer answers WHICH axioms a declaration depends on, not whether the proof
is valid. It is not a defence against stale or hand-forged `.olean`s. -/
def withImportedEnv {α} (modules : Array Name) (act : CoreM α) : IO α := do
  initSearchPath (← findSysroot)
  unsafe Lean.withImportModules (modules.map (fun m => { module := m })) {} (trustLevel := 1024)
    fun env => Prod.fst <$> Core.CoreM.toIO act
      (ctx := { fileName := "audit_axioms", fileMap := default }) (s := { env := env })

/-- Every declaration in the environment that was defined in a module at or under `root`. -/
def candidatesUnder (env : Environment) (root : Name) : Array Name :=
  let modNames := env.allImportedModuleNames
  env.constants.fold (init := #[]) fun acc declName _ =>
    match env.getModuleIdxFor? declName with
    | some idx =>
      match modNames[idx.toNat]? with
      | some m => if m == root || root.isPrefixOf m then acc.push declName else acc
      | none => acc
    | none => acc

/-- Audit every declaration defined under `root`, returning materialized output lines.
See `renderLine` for why the strings must be built here, inside the import callback. -/
def auditRoot (root : Name) : CoreM (Option (Array String)) := do
  let env ← getEnv
  return auditLines env (candidatesUnder env root)

end Theoremata.AxiomAudit

open Theoremata.AxiomAudit in
/-- Standalone entry point: `audit_axioms Root [ExtraImport ...]`.

WHY this returns a `UInt32` instead of calling `IO.Process.exit`: an abrupt `exit()` tears
the process down while the imported environment is still live and can segfault during
teardown. Returning the code lets the Lean runtime unwind in order. -/
def main (args : List String) : IO UInt32 := do
  match args with
  | [] =>
      IO.eprintln "audit_axioms: usage: audit_axioms Root [ExtraImport ...]"
      return 2
  | root :: rest =>
      let rootName := root.toName
      let modules := (#[rootName] ++ (rest.map String.toName).toArray)
      match ← withImportedEnv modules (auditRoot rootName) with
      | none =>
          IO.eprintln (zeroDeclDiagnostic s!"no declarations defined under {rootName}")
          return 1
      | some lines =>
          for l in lines do IO.println l
          return 0
