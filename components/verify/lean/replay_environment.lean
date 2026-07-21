/-
ATTRIBUTION AND PROVENANCE
==========================
This file is a MODIFIED PORT of `SafeVerify.lean` as vendored at
`resources/Putnam2025-main/Putnam2025-main/SafeVerify.lean`, which carries its own
attribution chain:

    Taken from: https://github.com/GasStationManager/SafeVerify/blob/e3c776dc864aeac04f6c1a7af925205f43d96a92/Main.lean
    Author: https://github.com/GasStationManager
    Adapted from https://github.com/leanprover/lean4checker/blob/master/Main.lean
    and https://github.com/kim-em/lean-training-data/blob/master/scripts/declaration_types.lean

The vendoring repository ships this MIT licence, reproduced in full as the licence
requires:

    MIT License

    Copyright (c) 2025 Axiom Math.

    Permission is hereby granted, free of charge, to any person obtaining a copy
    of this software and associated documentation files (the "Software"), to deal
    in the Software without restriction, including without limitation the rights
    to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
    copies of the Software, and to permit persons to whom the Software is
    furnished to do so, subject to the following conditions:

    The above copyright notice and this permission notice shall be included in all
    copies or substantial portions of the Software.

    THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
    IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
    FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
    AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
    LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
    OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
    SOFTWARE.

`Lean.Environment.replay` itself is toolchain code (Kim Morrison, Apache 2.0) and is
called, not copied.

WHAT THIS PORT CHANGED relative to upstream `SafeVerify.lean`
------------------------------------------------------------
  1. SCOPE NARROWED TO THE REPLAY. Upstream bundles three separable jobs into one
     executable: the environment replay, an allowed-axiom check, and a target-versus-
     submission signature comparison. Theoremata already owns the other two elsewhere
     (`components/verify/python/theoremata_tools/axioms.py` plus
     `components/verify/lean/audit_axioms.lean` for axioms,
     `components/prover/statement_preservation.rs` for signature conformance), and this
     port must not reroute or duplicate an existing gate layer. Only the replay is here.
  2. `unsafe` DROPPED. Upstream's `replayFile` is `unsafe` because of the older
     `importModules` and `Environment.replay` signatures it targeted. On the toolchain
     this file is written against, neither is `unsafe`, and the `open private
     setImportedEntries finalizePersistentExtensions` line upstream needed (and left
     commented out) is gone with it. That also removes the Batteries dependency, so this
     file builds against bare core Lean.
  3. MACHINE-READABLE OUTPUT. Upstream pretty-prints every replayed declaration's type
     for a human reading CI logs. The caller here is Python, so output is one JSON
     object per line behind a fixed marker, exactly as `audit_axioms.lean` does.
  4. FAIL-LOUDLY-ON-ZERO-DECLARATIONS GUARD ADDED. Upstream will happily "replay" an
     olean with nothing in it and print `Finished with no errors.`. That is the
     fail-open shape this project cares most about, so a zero-declaration replay is a
     hard error here, on stderr, with no summary line and a non-zero exit code. This is
     the same guard, and the same reasoning, as `audit_axioms.lean`'s
     `zeroDeclDiagnostic`.
  5. SKIPPED CONSTANTS ARE REPORTED, NOT SWALLOWED. `Environment.replay` deliberately
     does not send `unsafe` or `partial` constants to the kernel. Upstream noticed the
     related risk and threw on any `unsafe`/`partial` definition it found afterwards;
     it could only see the ones that survived into the replayed environment. This port
     counts and names them from the module data up front, before replay, and reports
     them so the caller can refuse. Reporting rather than throwing keeps the policy with
     the caller, matching the choice made in `audit_axioms.lean` for the axiom allowlist.
  6. EXIT CODE INSTEAD OF `IO.Process.exit`, and every emitted string materialized
     before the compacted region can go away. Both are load-bearing and marked `WHY:`
     below.
  7. Upstream's `IO.asTask` wrapper around the second file is dropped along with the
     two-file target/submission mode.

WHAT A SUCCESSFUL REPLAY ESTABLISHES
------------------------------------
Exactly this: every non-`unsafe`, non-`partial` constant stored in the given `.olean`
was re-accepted by the Lean kernel, in an environment built from that module's OWN
declared imports as they exist on disk right now, and the constructors and recursors
stored in it are identical to the ones the kernel regenerates from the replayed
inductives. So the module's declarations follow from its imports.

WHAT IT DOES NOT ESTABLISH
--------------------------
  * NOT that the proof is sound. A replay of a module whose only theorem is
    `theorem t : True := trivial` passes. Replay says nothing about WHICH theorem was
    proved, whether it is the theorem we asked for, or whether it is trivial.
  * NOT that the axiom closure is clean. `sorryAx` replays perfectly well: it is a
    genuine axiom and the kernel accepts declarations that use it. The axiom question is
    `check_axioms` and `audit_axioms.lean`, and this changes nothing about them.
  * NOT that the imports are themselves trustworthy. Replay rebuilds from the imports;
    it does not audit them. A doctored `.olean` in the import closure is replayed INTO
    the base environment by `importModules` at the trust level given, not re-checked. To
    cover the whole closure, every module in it must be replayed.
  * NOT that `unsafe` or `partial` constants are valid. They are skipped by
    `Environment.replay`; this file reports them so the caller can refuse.
  * NOT anything about the environment extensions (instances, simp sets, attributes).
    Only the kernel-level constants are replayed.
-/
import Lean
import Lean.Replay

open Lean

namespace Theoremata.EnvReplay

/-- Marker prefixing the single result line of a successful replay. Callers key on this
rather than on free-form text so unrelated Lean chatter on stdout can never be mistaken
for a result. -/
def resultMarker : String := "THEOREMATA_ENV_REPLAY"

/-- Marker prefixing the summary line. Its absence is itself a failure signal: a run that
produced no summary did not complete, and the caller must not read that as clean. -/
def summaryMarker : String := "THEOREMATA_ENV_REPLAY_SUMMARY"

/-- Marker prefixing every hard error. Its presence must make the caller report
not-clean regardless of anything else on the stream. -/
def errorMarker : String := "THEOREMATA_ENV_REPLAY_ERROR"

private def escape (s : String) : String :=
  s.foldl (init := "") fun acc ch =>
    match ch with
    | '"'  => acc ++ "\\\""
    | '\\' => acc ++ "\\\\"
    | '\n' => acc ++ "\\n"
    | '\r' => acc ++ "\\r"
    | '\t' => acc ++ "\\t"
    | c    => acc.push c

private def jsonStrings (xs : Array String) : String :=
  String.intercalate "," (xs.map (fun s => "\"" ++ escape s ++ "\"")).toList

/-- What a replay attempt found out about one module, with every `Name` already turned
into a `String`.

WHY strings and not `Name`s: declaration and module `Name`s read out of an `.olean` live
in a memory-mapped compacted region. Formatting them after that region is released is a
use-after-free, and this project has the same note on `audit_axioms.lean`. Everything
this file ever prints is materialized here, at the point of reading, so no later code
path can reach back into mapped memory. -/
structure Report where
  /-- Path of the `.olean` that was read, as given. -/
  oleanPath : String
  /-- The module's own declared imports. This is the entire basis the replay reconstructs
  from; nothing else is taken on trust from the file. -/
  imports : Array String
  /-- Every constant stored in the module. -/
  totalConstants : Nat
  /-- Constants `Environment.replay` will actually send to the kernel. -/
  replayable : Nat
  /-- Names `Environment.replay` will NOT send to the kernel, because they are `unsafe`
  or `partial`. A non-empty list here is a hole in the guarantee and the caller is
  expected to treat it as such. -/
  skipped : Array String

/-- Render the successful-replay result line. -/
def renderResult (r : Report) : String :=
  resultMarker ++ " {\"olean\":\"" ++ escape r.oleanPath
    ++ "\",\"imports\":[" ++ jsonStrings r.imports
    ++ "],\"total_constants\":" ++ toString r.totalConstants
    ++ ",\"replayed\":" ++ toString r.replayable
    ++ ",\"skipped\":[" ++ jsonStrings r.skipped ++ "]}"

/-- Render the summary line. Emitted only after the kernel accepted every replayed
constant. -/
def renderSummary (replayed : Nat) : String :=
  summaryMarker ++ " {\"replayed\":" ++ toString replayed ++ "}"

/-- The message emitted when zero declarations were replayed.

WHY this is the most important line in the file: a replay that checks nothing and reports
clean is a fail-open soundness bug, and this project has shipped exactly that shape before
(a Metamath audit that returned `within_whitelist = true` unconditionally). An empty
module, a path pointing at the wrong `.olean`, or a module whose every constant is
`unsafe`, must all be LOUD and must be distinguishable by the caller from a clean replay.
It is therefore reported on stderr, with no summary line, and with a non-zero exit code.
Upstream SafeVerify does not have this guard; it prints "Finished with no errors." -/
def zeroDeclDiagnostic (context : String) : String :=
  errorMarker ++ " replayed 0 declarations (" ++ context ++
    "): the replay is miswired or the module is empty, and this is NOT a clean result."

/-- Render a replay failure. This is the tamper-detected path as well as the
kernel-rejected-something path; the two are not distinguishable from here and the message
carries whatever the kernel said. -/
def renderFailure (oleanPath : String) (message : String) : String :=
  errorMarker ++ " {\"olean\":\"" ++ escape oleanPath
    ++ "\",\"error\":\"" ++ escape message ++ "\"}"

/-- Read `path` and summarize what a replay of it would cover, materializing every string
while the compacted region is certainly still mapped.

Returns the report, the module's imports verbatim, and the constant map for the replay.
The imports are handed back as the stored `Import` records rather than rebuilt from the
report's strings, because `Import` carries `importAll`, `isExported` and `isMeta` flags
that decide how much of a module is loaded. Rebuilding from a name would silently replay
against a different, usually smaller, base environment. -/
def readReport (path : System.FilePath) :
    IO (Report × Array Import × Std.HashMap Name ConstantInfo) := do
  let (mod, _region) ← readModuleData path
  let mut newConstants : Std.HashMap Name ConstantInfo := {}
  let mut replayable := 0
  let mut skipped : Array String := #[]
  for name in mod.constNames, ci in mod.constants do
    newConstants := newConstants.insert name ci
    -- This condition is copied from `Lean.Environment.replay` itself, so the count
    -- reported is the count the kernel will really see rather than an estimate.
    if !ci.isUnsafe && !ci.isPartial then
      replayable := replayable + 1
    else
      skipped := skipped.push name.toString
  let report : Report := {
    oleanPath := path.toString
    imports := mod.imports.map (fun i => i.module.toString)
    totalConstants := mod.constNames.size
    replayable := replayable
    skipped := skipped }
  return (report, mod.imports, newConstants)

/-- Replay one `.olean`, printing a result line and a summary line on success.

Returns the process exit code rather than calling `IO.Process.exit`.

WHY a return code and not `IO.Process.exit`: an abrupt `exit()` tears the process down
while the imported environment and its mapped regions are still live, and can segfault
during teardown. Returning the code lets the Lean runtime unwind in order. Same reason as
`audit_axioms.lean`'s `main`. -/
def replayOne (path : System.FilePath) : IO UInt32 := do
  unless (← path.pathExists) do
    IO.eprintln (renderFailure path.toString s!"object file '{path}' does not exist")
    return 2
  -- A missing or unreadable `.olean` throws here rather than in the replay. It must still
  -- surface as a marked error with a run-could-not-happen exit code, never as an
  -- unmarked crash the caller has to interpret from stderr prose.
  --
  -- HONEST LIMIT, measured: this catch only covers errors Lean raises. `readModuleData`
  -- parses a memory-mapped region in native code, and a TRUNCATED `.olean` segfaults the
  -- process there, before any Lean-level exception exists to catch. Nothing in this file
  -- can improve on that. It stays fail-closed only because the process then dies without
  -- ever printing a summary line, and the Python caller treats a crash exit code as
  -- could-not-run rather than clean. That contract is load-bearing, not incidental.
  let read ←
    try
      some <$> readReport path
    catch e =>
      IO.eprintln (renderFailure path.toString s!"could not read module data: {e}")
      pure none
  let some (report, imports, newConstants) := read | return 2
  if report.replayable == 0 then
    IO.eprintln (zeroDeclDiagnostic s!"no replayable constants in {path}")
    return 1
  -- The environment is rebuilt from the module's OWN declared imports. Whatever
  -- environment the producer claims to have typechecked in is not consulted, and that
  -- is the entire point of the exercise.
  let base ←
    try
      some <$> importModules imports {} 0
    catch e =>
      IO.eprintln (renderFailure path.toString s!"import failed: {e}")
      pure none
  let some base := base | return 2
  let replayed ←
    try
      some <$> base.replay newConstants
    catch e =>
      -- A kernel rejection lands here. This is the detection we are here for, so the
      -- message is surfaced verbatim rather than summarized.
      IO.eprintln (renderFailure path.toString (toString e))
      pure none
  match replayed with
  | none => return 1
  | some _ =>
      IO.println (renderResult report)
      IO.println (renderSummary report.replayable)
      return 0

end Theoremata.EnvReplay

open Theoremata.EnvReplay in
/-- Standalone entry point: `replay_environment MODULE.olean [SEARCH_PATH ...]`.

The optional trailing arguments are added to the Lean search path so the module's imports
resolve. They are search roots, not modules.

Exit codes, which the Python caller keys on:
  0  every replayable constant was re-accepted by the kernel
  1  the kernel rejected something, or zero declarations were replayable
  2  the run could not happen at all (missing file, import failure, bad usage) -/
def main (args : List String) : IO UInt32 := do
  match args with
  | [] =>
      IO.eprintln (errorMarker ++
        " usage: replay_environment MODULE.olean [SEARCH_PATH ...]")
      return 2
  | path :: searchPaths =>
      initSearchPath (← findSysroot) (searchPaths.map System.FilePath.mk)
      replayOne path
