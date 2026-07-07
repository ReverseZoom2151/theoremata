/-
Theoremata retrieval Layer B: environment declaration dumper.

Run with:  lean --run dump_decls.lean [Module.Name ...]
       or  lake env lean --run dump_decls.lean [Module.Name ...]

Imports the requested modules (default `Init`), walks the resulting
environment, and prints one JSON object per line (JSONL) for every
non-internal declaration: fully-qualified name, constant kind, defining
module, and whether it is an axiom.
-/
import Lean
open Lean

def kindOf : ConstantInfo → String
  | .axiomInfo _   => "axiom"
  | .defnInfo _    => "def"
  | .thmInfo _     => "theorem"
  | .opaqueInfo _  => "opaque"
  | .quotInfo _    => "quot"
  | .inductInfo _  => "inductive"
  | .ctorInfo _    => "constructor"
  | .recInfo _     => "recursor"

def isAxiom : ConstantInfo → Bool
  | .axiomInfo _ => true
  | _            => false

/-- Minimal JSON string escaping for declaration names / module names. -/
def esc (s : String) : String :=
  s.foldl (init := "") fun acc c =>
    acc ++ (if c == '"' then "\\\"" else if c == '\\' then "\\\\" else String.singleton c)

def moduleOf (env : Environment) (name : Name) : String :=
  match env.getModuleIdxFor? name with
  | some idx =>
    match env.header.moduleNames[idx.toNat]? with
    | some m => m.toString
    | none   => ""
  | none => ""

def main (args : List String) : IO Unit := do
  initSearchPath (← findSysroot)
  let mods : Array Import :=
    if args.isEmpty then #[{ module := `Init }]
    else (args.map fun m => ({ module := m.toName } : Import)).toArray
  let env ← importModules mods {} (trustLevel := 0)
  for (name, info) in env.constants.toList do
    if name.isInternal then
      continue
    let line :=
      "{\"name\":\"" ++ esc name.toString
        ++ "\",\"kind\":\"" ++ kindOf info
        ++ "\",\"module\":\"" ++ esc (moduleOf env name)
        ++ "\",\"is_axiom\":" ++ (if isAxiom info then "true" else "false") ++ "}"
    IO.println line
