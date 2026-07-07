# LeanParanoia — Full Resource-Mining Pass

Source: `resources/LeanParanoia-main/LeanParanoia-main/`
Upstream: `github.com/oOo0oOo/LeanParanoia` (MIT). Direct dependency: `lean4checker` (leanprover FRO).
Pass type: **FULL** (all Lean + prose + Python read in full; large data dirs catalogued+sampled — see §5).

---

## 1) What it is (scope, size, structure)

LeanParanoia is a **configurable soundness/attack checker for compiled Lean 4 proofs**. Given a fully-qualified theorem name and a set of already-built `.olean` files on the Lean search path, it loads the environment, walks the theorem's transitive dependency DAG, runs a battery of soundness checks against every constant, and finishes with a **kernel environment replay** (via `lean4checker`). It emits a single JSON verdict `{success, failures, [errorTrace]}`.

Key design points (README + `Checker.lean`):
- **No trusted reference file.** Unlike SafeVerify/Comparator (challenge-solution verifiers), it does *not* compare against an expected statement. It only inspects the proof term + its dependency closure + replays the kernel. README explicitly warns it "cannot guarantee complete soundness" and to also use challenge-solution verifiers for critical proofs.
- **Operates on pre-compiled oleans** — it does not compile source; it assumes elaboration already happened and inspects the resulting `Environment`.

**Size / structure** (176 MB total, but ~99% is `.lake` build artifacts + a vendored `lean4checker` git checkout — skipped per instructions). Hand-written source is tiny:

| File | Lines | Role |
|------|------|------|
| `LeanParanoia/Checker.lean` | 435 | All 14 checks + orchestrator `runChecks` |
| `LeanParanoia/Helpers.lean` | 251 | Expr folding, dep BFS, caches, trust/core predicates |
| `LeanParanoia/Config.lean` | 36 | `VerificationConfig` (toggles, whitelists, defaults) |
| `Main.lean` | 149 | CLI arg parsing, module import, JSON output |
| `LeanParanoia.lean` | 4 | Umbrella import |
| `.lake/packages/lean4checker/Lean4Checker/Replay.lean` | 177 | Patched `Environment.replay'` (kernel re-check) |
| `README.md`, `VERIFIER_COMPARISON.md` | — | Docs + tool-comparison matrix |
| `tests/**` (Python pytest) | ~20 test modules + 66 `.lean` exploit/valid fixtures | See §5 |

CLI (from `Main.lean` / README): `paranoia [OPTIONS] Module.Sub.theorem`, exit 0/1, JSON on stdout. Every check has a `--no-*` toggle; plus `--allowed-axioms`, `--source-blacklist`, `--source-whitelist`, `--trust-modules`, `--fail-fast`.

---

## 2) Reusable ideas / EVERY soundness check (diff target for our `hardening.rs`)

> **Framing correction that matters for the whole exercise:** our `components/verify/hardening.rs` is **not a reimplementation** of these checks. It is a thin **wrapper**: it scaffolds a Lake workspace on local Mathlib, `lake build`s the generated proof module, then shells out to the *actual* `paranoia.exe` with **default config** and parses the top-level `success` boolean. So every check below is "ported" **by delegation** — provided the binary runs and resolves the theorem. That makes the port faithful *by construction*, but shifts the risk from "did we re-implement each check" to "does our wrapper actually invoke the binary on the right constant, with the right config, and interpret the result safely." Those operational gaps are the real findings (§4).

The full config surface (`Config.lean`), all default-on except `failFast`:

```
checkSorry checkMetavariables checkUnsafe checkPartial checkAxioms
checkConstructors checkRecursors checkExtern checkImplementedBy checkCSimp
checkNativeComputation checkOpaqueBodies checkSource enableReplay
allowedAxioms = [propext, Quot.sound, Classical.choice]
sourceBlacklist = [local instance, local notation, local macro_rules, local syntax,
                   local infix, local infixl, local infixr, local prefix, local postfix,
                   scoped instance, scoped notation]
trustModules = []
```

### The 14 checks (with the actual detection code)

**(1) Sorry** — `checkNoSorry` (`Checker.lean:39`) + transitive (`Checker.lean:344`). Detection = scan the proof term for the `sorryAx` constant:
```lean
-- Helpers.lean:87
def hasSorry (e : Expr) : Bool :=
  foldExpr (fun acc expr => acc || match expr with
    | .const name _ => name == ``sorryAx
    | _ => false) false e
```
Applied to the theorem value **and** every dependency's value (`allowOpaque := checkOpaqueBodies`, so it peers inside `opaque` bodies too — catches `Sorry/Opaque.lean`). Also re-checked inside csimp source/target decls.

**(2) Metavariables** — `checkNoMetavars` (`Checker.lean:45`): `if e.hasMVar` → unresolved metavariables (incomplete elaboration). Target only.

**(3) Unsafe** — `checkNoUnsafe` (`Checker.lean:51`): `cinfo.isUnsafe`. Target + every dep.

**(4) Partial** — `checkNoPartial` (`Checker.lean:57`): `cinfo.isPartial`. Target + every dep.

**(5) CustomAxioms (axiom whitelist)** — the core check (`Checker.lean:373`). Walks deps; for any `ConstantInfo` that `isAxiom`, rejects unless its name string is in `allowedAxioms`:
```lean
-- Helpers.lean:92
def isAxiom (info : ConstantInfo) : Bool := match info with | .axiomInfo _ => true | _ => false
-- Checker.lean:373
if isAxiom depInfo then
  let axiomStr := depName.toString
  if !config.allowedAxioms.contains axiomStr then
    addFailure { name := "CustomAxioms", reason := s!"Uses disallowed axiom: {axiomStr}" }
```
This is what catches forged axioms (`run_cmd` adding `Declaration.axiomDecl`), axioms hidden behind macros/instances, `debug.skipKernelTC` (represented as an axiom), and native-decide's `Lean.ofReduceBool`/`Lean.trustCompiler` compiler axioms. Note whitelist is **by exact name string** — so `Std.TrustMe.forgedFalse` is *not* trusted just for being under `Std.` (the whitelist is names, not module prefixes).

**(6) Extern / Export / Init** — `checkNoExtern` (`Checker.lean:63`). Three foreign-code vectors in one check:
```lean
let hasExtern := Lean.isExtern env cinfo.name
let hasExport := (Lean.getExportNameFor? env cinfo.name).isSome
let hasInit   := isIOUnitRegularInitFn env cinfo.name
              || isIOUnitBuiltinInitFn env cinfo.name
              || hasInitAttr env cinfo.name
```
Detects `@[extern "c_fn"]`, `@[export]`, and `@[init]`/`@[builtin_init]` initialization hooks (FFI / arbitrary C / load-time hooks). Skipped for trusted/core modules. Target + every dep. Fake `Lean.*`/`Std.*` namespaces do **not** bypass it (they aren't real core modules per `isCoreModuleName`).

**(7) ImplementedBy** — `checkNoImplementedBy` (`Checker.lean:80`): `Lean.Compiler.implementedByAttr.getParam? env name`. Rejects `@[implemented_by target]` unless *both* the decl and the impl target are trusted. Crucially, the dep-BFS **follows the impl target into the graph** (`Helpers.lean:167`), so `@[implemented_by]` chains ending in an `@[extern]` (`ImplementedBy/ChainedReplacement.lean`) are caught by the Extern check on the chained target.

**(8) CSimp** — `checkNoCSimp` / `collectCSimpIssues` / `analyzeConstantForCSimp` (`Checker.lean:91–156`) **plus a global scan** `checkGlobalCSimps` (`Checker.lean:256`). `@[csimp] lhs = rhs` swaps a function's compiled implementation; the check inspects the csimp *theorem*, its *source* decl, and its *target* decl for unsafe/partial/**axiom**/sorry/implemented_by/extern. `checkGlobalCSimps` iterates **every csimp entry in every imported module** (not just the dep closure), because a csimp anywhere can silently miscompile a dependency. Uses a `CSimpCache` (`Helpers.lean:193`) to scan each module once.

**(9) NativeComputation** — `checkNoNativeComputation` (`Checker.lean:191`). Scans the term and deps for compiler primitives by name prefix:
```lean
-- Helpers.lean:16
def nativeComputationPrefixes : Array String :=
  #["Lean.ofReduce", "Lean.reduce", "Lean.nativeDecide", "Lean.trustCompiler"]
```
Catches `native_decide` and `ofReduceBool`/`ofReduceNat` (`NativeComputation/*.lean`). (These *also* trip CustomAxioms + Replay — belt & suspenders.)

**(10) ConstructorIntegrity** — `checkConstructorIntegrity` (`Checker.lean:158`): an `inductInfo` with an empty `ctors` list (other than the builtin empties `False/True/Empty/PEmpty`, `isBuiltinInductive`) is suspicious — someone declared an empty inductive then supplied a *manual* `axiom`-based "constructor" to fabricate an inhabitant (`ConstructorIntegrity/ManualConstructor.lean`).

**(11) RecursorIntegrity** — `checkRecursorIntegrity` (`Checker.lean:173`): inductive with no ctors, or **missing its auto-generated `<Name>.rec` recursor** → recursor tampering.

**(12) SourcePatterns** — `checkSourcePatterns` (`Checker.lean:203`). The only **source-level (lexical) check**: it locates the `.lean` source of each non-core dep (`findSourceFile`, `Helpers.lean:224`, with a `SourceFileCache`), skips `--` comment lines, and flags any line containing a blacklist pattern (`local instance`, `scoped notation`, `local macro_rules`, etc.). Rationale: `local`/`scoped` notation/instance/macro redefinitions can make a *displayed* statement mean something different from the elaborated one (`SourcePatterns/*.lean`). Whitelist can re-permit specific patterns.

**(13) Replay (environment replay)** — `checkEnvironmentReplay` (`Checker.lean:228`) → `lean4checker`'s `Environment.replay'` (`Replay.lean:163`). For every non-trusted module in the dep closure (`collectModulesToReplay`, `Helpers.lean:184`): re-read the `.olean`, re-import, and **re-send every constant to the kernel** via `addDeclCore`, then verify **postponed constructors and recursors are byte-identical** to those the kernel regenerates from the inductives:
```lean
-- Replay.lean:135 / 146
if ! (info == info') then throw <| IO.userError s!"Invalid constructor {ctor}"
...
if ! (info == info') then throw <| IO.userError s!"Invalid recursor {ctor}"
```
This is the deepest check and the catch-all for **environment tampering, corrupted/forged `.olean` (supply-chain), auxiliary-declaration shadowing (`bar.match_1`), fake theorem/type-signature substitution, and native_decide's missing `_nativeDecide_` aux decls**. It's why `AuxiliaryShadowing/TheoremShadowing` and `TypeSignature` are caught by `Replay` *only* (no dedicated check exists for them).

**(14) KernelRejection** — in `Main.lean:114` and `runChecks` (`Checker.lean:279`): if the module import throws (missing olean, kernel rejected a non-positive inductive / unsafe cast at compile time) or the theorem isn't found, emit `KernelRejection`. Catches `KernelRejection/NonPositive.lean`, `UnsafeCast.lean`.

### Foundational machinery worth stealing
- **Transitive dependency BFS** `visitTransitiveDeps` (`Helpers.lean:139`): custom two-list queue `Deps.Collector`, dedup via `HashSet`, enqueues (a) direct constants of the value, (b) direct constants of the type, and (c) the `implemented_by` target. `skip` = trust-module prefix match only.
- **Trust / core predicates** (`Helpers.lean:45–83`): `matchesTrustedPrefix` (prefix + dot boundary), `isCoreModuleName` (roots `Lean/Init/Nat/String/Array/List/IO/System`), `isLeanCoreConstant` (roots `Lean/Init/Std/Mathlib`), `shouldSkipTrustedConstant` (also always skips `sorryAx`). These decide what gets exempted from source/csimp/extern checks.
- **`allowOpaqueBodies`** — inspecting `opaque` constant bodies (`value? (allowOpaque := true)`) is what defeats `opaque`-hidden `sorryAx`.
- **Caches**: `CSimpCache`, `SourceFileCache` — scan-once patterns for performance over Mathlib-scale closures.
- **`foldExpr`** (`Helpers.lean:23`): a worklist-based (non-recursive-blowup) expression folder reused by `hasSorry`, `collectDirectConstants`, `findNativeComputationInExpr?`.

---

## 3) Schema / report format it emits

`VerificationResult` (`Checker.lean:19`), serialized by the `ToJson` instance (`Checker.lean:32`). Shape:
```json
{ "success": false,
  "failures": { "<CheckName>": ["reason", ...], ... },
  "errorTrace": "…"   // optional, only when set
}
```
`failures` is a **map from check name → list of human-readable reasons**, grouped by `VerificationResult.fromFailures` (`Checker.lean:25`). Check names (the map keys) are the stable vocabulary: `Sorry, Metavariables, Unsafe, Partial, CustomAxioms, Extern, ImplementedBy, CSimp, NativeComputation, ConstructorIntegrity, RecursorIntegrity, SourcePatterns, Replay, KernelRejection`. Exit code = `0` iff `success`. The example (README + NativeDecide) shows one exploit tripping **three** keys at once — the categories are informative, not mutually exclusive.

`VERIFIER_COMPARISON.md` is a second emitted artifact (generated by `tests/benchmark/generate_exploits_table.py`): a per-exploit matrix of LeanParanoia vs lean4checker vs SafeVerify vs Comparator with 🟢/🛑/🟡 verdicts, timings, and collapsible failure details.

---

## 4) What our earlier targeted pass likely MISSED / port gaps

Because our port delegates, "missed check" ≈ "a check the binary runs that our wrapper can't invoke, mis-targets, or ignores." Concrete gaps in `components/verify/hardening.rs`:

1. **Fail-open on unresolved theorems (soundness-relevant).** `hardening.rs:262-285`: when paranoia's stdout isn't parseable JSON (import/name-resolution failure), we fall back to `clean = paranoia_success.unwrap_or(true)` → **treated as CLEAN**. A proof paranoia never actually audited passes the hardening layer. It's non-fatal by design (comment leans on the `#print axioms` gate), but the hardening signal can silently no-op. Should be surfaced/logged as "not audited," not folded into `clean=true`.
2. **Wrong-target risk.** `first_theorem_name` (`hardening.rs:79`) picks only the **first** `theorem`/`lemma` and forms `{qualified}.{thm}`. If the goal constant is a `def`/`example`/`instance`/`noncomputable def` (several exploit fixtures use `def exploit_theorem`), or not first, we audit the wrong constant or fail to resolve — then hit gap #1. paranoia's own `Main.lean:124` handles this by trying the fully-qualified name, else the last name component.
3. **No `--trust-modules` → replays all of Mathlib.** We invoke with default config, and `collectModulesToReplay` only excludes *trust-module* prefixes (empty by default). So `enableReplay` will attempt a kernel replay of the **entire transitive Mathlib closure** — the exact scenario the README/tests mitigate with `--trust-modules Std,Mathlib`. Practically this means our hardening run is very slow or times out on any real proof. We should pass `--trust-modules Std,Mathlib,Init` (matching `tests/config/test_trust_policies.py`).
4. **Config is hard-wired to defaults.** We cannot set `--allowed-axioms` (e.g., to permit a legitimately-needed standard axiom) or `--fail-fast`. Fine for now but note it.
5. **We discard the failure taxonomy for feedback.** We stash `failures`/`errorTrace` into `details` but the loop only consumes the `success` bool + a summary string. The per-check keys (`CustomAxioms`, `Replay`, …) are exactly the actionable signal a falsifier/repair step wants; we throw them away at the gate.

Checks that are easy to overlook in a *targeted* skim (verify these are understood, since our wrapper depends on them existing in the binary — they are NOT re-implemented on our side): **Metavariables** (`hasMVar`), **Extern's export+init sub-vectors** (`getExportNameFor?`, `@[init]`/`@[builtin_init]` hooks — not just `@[extern]`), the **CSimp global scan** across *all* modules (`checkGlobalCSimps`, not just the dep closure), **ConstructorIntegrity + RecursorIntegrity**, **SourcePatterns** lexical scan, the **implemented_by-chain-follows-into-DAG** behavior, **`allowOpaqueBodies`** opaque-body inspection, and that **AuxiliaryShadowing / TypeSignature exploits are caught by Replay only** (disabling replay for speed silently drops that whole class).

---

## 5) Test / benchmark value

**Excellent, directly reusable red-team corpus.** Two parallel fixture trees, 66 `.lean` files:
- `tests/lean_exploit_files/**` (source; catalogued in full — 66 files across 15 attack families) and `tests/benchmark/references/**` (a curated subset, same layout).
- Sampled in full (read verbatim): `AuxiliaryShadowing/TheoremShadowing`, `MatcherShadowing`; `CustomAxioms/ForgeRunCmd` (run_cmd forging `Declaration.axiomDecl`), `HiddenInMacro`, `SkipKernelTC` (`set_option debug.skipKernelTC`), `FakeStdLib` (forge under `Std.*`); `Extern/InitHook`, `CoreNamespace` (fake `Lean.Evil`/`Lean.Magic`); `ImplementedBy/ChainedReplacement`; `NativeComputation/NativeDecide`; `CSimp/WithAxiom`; `ConstructorIntegrity/ManualConstructor`; `Sorry/ByAsSorry` (`debug.byAsSorry`), `Sorry/Opaque` (opaque-hidden `sorryAx`); `KernelRejection/NonPositive` (non-positive inductive). Families: AuxiliaryShadowing(5), CSimp(2), ConstructorIntegrity(1), CustomAxioms(8), Extern(6), ImplementedBy(3), KernelRejection(2), Metavariables(2), NativeComputation(2), Partial(1), RecursorIntegrity(1), Sorry(8), SourcePatterns(6), Transitive(10), Unsafe(1), Valid(6 negatives).
- **Valid/** are the negative controls (must PASS): `Simple`, `Helper`, `Dependencies`, `ComplexExample`, `WithAxioms` (legit standard axioms), `UnsafeReducibility`.

**Python harness** (`tests/conftest.py`): `Verifier.verify_theorem(...)` builds the exploit project once (session-scoped, version-cached), runs paranoia via `lake env`, parses JSON, exposes `success/failures/failed_tests/errors`. Test modules under `tests/exploits/` (18), `tests/config/` (4, exercising toggles/whitelists/trust), `tests/integration/` (4, incl. multi-file + a **live olean-corruption** test `test_replay_corruption.py` that flips bytes in a built `.olean` and asserts Replay fails). Benchmark dir builds a real Mathlib project and generates the comparison matrix.

**Value for Theoremata:** (a) a regression suite for our hardening gate — run each `Valid/*` (expect clean) and each exploit family (expect flagged); (b) an adversarial generator seed for our own falsify-before-prove stage; (c) `test_replay_corruption.py`'s byte-flip technique is a cheap supply-chain integrity smoke test we could adopt for our olean cache.

---

## 6) New vs. already-in-our-design

**Already in our design / pipeline:**
- `#print axioms` gate ≈ LeanParanoia's **CustomAxioms** whitelist (we already treat the axiom check as authoritative; hardening.rs comment says so).
- Lean compile gate ≈ **KernelRejection** (import/compile failure).
- We already *invoke the whole battery* by shelling to `paranoia.exe`, so Sorry/Unsafe/Partial/Extern/ImplementedBy/CSimp/NativeComputation/Constructor/Recursor/Source/Replay all run when the binary runs.

**New / not yet exploited on our side:**
- Treating the **per-check failure taxonomy** as structured, actionable feedback into the loop (currently discarded — §4.5).
- **`--trust-modules Std,Mathlib` tuning** to make replay tractable (§4.3) — without it our hardening step is effectively non-functional on real proofs.
- **Robust theorem-target resolution** independent of the `theorem`/`lemma` keyword (§4.2).
- The **exploit corpus as a CI regression suite** and the **olean byte-corruption integrity check** (§5) — neither is in our design yet.
- The **`replay'` API itself** (`lean4checker`) as an independent, in-process re-verification we could call directly rather than only through paranoia, if we ever want the deep check without the wrapper/build dance.

---

### Bottom line for the port
Our `hardening.rs` is faithful *by delegation* — it does not re-implement any check, so no check's *logic* was mistranslated. The exposure is operational: (1) fail-open when the theorem can't be resolved, (2) fragile single-keyword target selection, (3) no `--trust-modules`, so replay tries all of Mathlib and likely times out, (4) config frozen at defaults, (5) the actionable per-check failure categories are dropped at the gate. Fixing (1)–(3) is what makes the hardening layer actually bite.
