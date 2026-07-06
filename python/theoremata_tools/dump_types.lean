/-
Theoremata retrieval Layer C: environment declaration dumper WITH types.

Run with:  lean --run dump_types.lean [Module.Name ...]
       or  lake env lean --run dump_types.lean [Module.Name ...]

Like dump_decls.lean, but each JSONL record also carries a pretty-printed
`type` string (the declaration's type signature), which the head-symbol index
buckets on.
-/
import Lean
open Lean Meta

def kindOf : ConstantInfo → String
  | .axiomInfo _   => "axiom"
  | .defnInfo _    => "def"
  | .thmInfo _     => "theorem"
  | .opaqueInfo _  => "opaque"
  | .quotInfo _    => "quot"
  | .inductInfo _  => "inductive"
  | .ctorInfo _    => "constructor"
  | .recInfo _     => "recursor"

/-- JSON string escaping for names, modules, and pretty-printed types. -/
def esc (s : String) : String :=
  s.foldl (init := "") fun acc c =>
    acc ++ (if c == '"' then "\\\""
            else if c == '\\' then "\\\\"
            else if c == '\n' then "\\n"
            else if c == '\r' then ""
            else if c == '\t' then " "
            else String.singleton c)

def moduleOf (env : Environment) (name : Name) : String :=
  match env.getModuleIdxFor? name with
  | some idx =>
    match env.header.moduleNames[idx.toNat]? with
    | some m => m.toString
    | none   => ""
  | none => ""

/-- Emit one JSONL record per non-internal declaration, pretty-printing types
    inside `MetaM`. Types that fail to pretty-print are skipped rather than
    aborting the whole dump. -/
def dumpAll : MetaM Unit := do
  let env ← getEnv
  for (name, info) in env.constants.toList do
    if name.isInternal then
      continue
    let tyStr ← (do
      try
        let fmt ← ppExpr info.type
        pure (toString fmt)
      catch _ =>
        pure "")
    let line :=
      "{\"name\":\"" ++ esc name.toString
        ++ "\",\"kind\":\"" ++ kindOf info
        ++ "\",\"module\":\"" ++ esc (moduleOf env name)
        ++ "\",\"type\":\"" ++ esc tyStr ++ "\"}"
    IO.println line

def main (args : List String) : IO Unit := do
  initSearchPath (← findSysroot)
  let mods : Array Import :=
    if args.isEmpty then #[{ module := `Init }]
    else (args.map fun m => ({ module := m.toName } : Import)).toArray
  let env ← importModules mods {} (trustLevel := 0)
  let ctx : Core.Context := { fileName := "<dump_types>", fileMap := default }
  let state : Core.State := { env := env }
  discard <| (dumpAll.run {} {}).toIO ctx state
