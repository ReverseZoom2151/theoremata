# Lean — Full Website Map + Integration-Critical Docs

Grounding reference for describing Lean symmetrically alongside Isabelle and Rocq in
Theoremata. Compiled from `lean-lang.org`, `leanprover-community.github.io`, the Lean
Language Reference, and the relevant GitHub repos (see cited URLs throughout).

Two organizations matter: the **Lean FRO** (Functional Research Organization; runs
`lean-lang.org`, the compiler/toolchain, the reference manual) and the community
(`leanprover-community.github.io`; runs mathlib4, install docs, tactic docs, Zulip).

---

## A. Full Website Sitemap

### A.1 `lean-lang.org` — top-level sections

| Section | URL | Purpose |
|---|---|---|
| Home | https://lean-lang.org/ | Landing page; entry to install / learn / community |
| Install | https://lean-lang.org/install/ | Setup via VS Code + Lean 4 extension (auto-installs elan/lake) |
| Install (manual) | https://lean-lang.org/install/manual | Command-line / manual toolchain install |
| Learn | https://lean-lang.org/learn/ | Hub of core documentation, books, courses, tools (see B.1) |
| Community | https://lean-lang.org/community/ | Zulip, office hours, community meeting, YouTube |
| Use Cases | https://lean-lang.org/use-cases/ | Real-world deployments + papers (see A.4) |
| FAQ | https://lean-lang.org/faq | Frequently asked questions |
| Language Reference | https://lean-lang.org/doc/reference/latest/ | The official Lean Language Reference (spec) |
| Lean API docs | https://lean-lang.org/doc/api/ | Core/stdlib API reference |
| Playground | https://live.lean-lang.org/?from=lean | In-browser Lean (Lean4Web instance) |

### A.2 Lean FRO subsite (`/fro`)

| Page | URL | Purpose |
|---|---|---|
| FRO Home | https://lean-lang.org/fro | Vision / overview of the Lean FRO |
| About | https://lean-lang.org/fro/about | History and impact |
| Team | https://lean-lang.org/fro/team | Leadership / staff |
| Roadmap | https://lean-lang.org/fro/roadmap | Technical priorities and deliverables |
| Contact | https://lean-lang.org/fro/contact | Contact information |
| Founder's Blog | https://leodemoura.github.io/blog/ | Leo de Moura's essays (news/blog) |

### A.3 FRO-operated ecosystem services / tools

| Service | URL | Purpose |
|---|---|---|
| Loogle | https://loogle.lean-lang.org/ | Type/name/subexpression declaration search (see B.6) |
| Reservoir | https://reservoir.lean-lang.org/ | Lean package registry/index |
| Verso | https://verso.lean-lang.org/ | Documentation authoring platform (used by the reference manual) |
| Mathlib Initiative | https://mathlib-initiative.org/ | Professional support org for the math library |
| CSLib | https://www.cslib.io/ | Computer-science library for Lean |

### A.4 Use cases / papers (`/use-cases/*`)

| Case | URL | Purpose |
|---|---|---|
| Cedar (AWS) | https://lean-lang.org/use-cases/cedar | AWS authorization-language verification in Lean |
| Aeneas | https://lean-lang.org/use-cases/aeneas | Rust → Lean functional verification |
| ArkLib | https://lean-lang.org/use-cases/arklib | Formally verified SNARK / proofs of knowledge |
| Veil | https://lean-lang.org/use-cases/veil | Verification of distributed protocols |
| Mathlib | https://lean-lang.org/use-cases/mathlib | The mathematics library as a foundation |
| Fermat's Last Theorem | https://lean-lang.org/use-cases/flt | FLT formalization project (Imperial College) |

### A.5 `leanprover-community.github.io` — community site

| Page | URL | Purpose |
|---|---|---|
| Community home | https://leanprover-community.github.io/ | Community-maintained hub for mathlib and Lean |
| Get started / install | https://leanprover-community.github.io/get_started.html | Community install instructions (all platforms) |
| Project setup | https://leanprover-community.github.io/install/project.html | Create/clone a Lake project; `lake exe cache get` (see B.4) |
| Mathlib4 docs | https://leanprover-community.github.io/mathlib4_docs/ | Full API reference (core + std + Mathlib) |
| Tactics doc | https://leanprover-community.github.io/mathlib4_docs/tactics.html | Tactic reference index |
| Mathlib overview | https://leanprover-community.github.io/mathlib-overview.html | High-level survey of library contents |
| Undergrad math list | https://leanprover-community.github.io/undergrad.html | Coverage of undergraduate topics |
| Contribute | https://leanprover-community.github.io/contribute/index.html | Contribution guidelines |
| Mathematics in Lean | https://leanprover-community.github.io/mathematics_in_lean/ | Book: formalizing math (see B.1) |
| Blog | https://leanprover-community.github.io/blog/ | Community blog / announcements |
| Zulip archive | https://leanprover-community.github.io/archive/ | Searchable Zulip archive |

### A.6 Community / social channels

| Channel | URL |
|---|---|
| Zulip chat (primary) | https://leanprover.zulipchat.com/ |
| GitHub — compiler | https://github.com/leanprover/lean4 |
| GitHub — mathlib4 | https://github.com/leanprover-community/mathlib4 |
| YouTube | https://www.youtube.com/@leanprovercommunity5485 |
| Proof Assistants Stack Exchange | https://proofassistants.stackexchange.com/ |

---

## B. Integration-Critical Docs Deep-Dive

### B.1 Documentation sitemap (the doc tree)

| Doc | URL | Role |
|---|---|---|
| **The Lean Language Reference** | https://lean-lang.org/doc/reference/latest/ | Authoritative spec. Chapters incl. *Introduction* (kernel), *Elaboration and Compilation*, *Axioms* (`.../Axioms/#axioms`), *The Type System*, *Tactic Proofs* (`.../Tactic-Proofs/#tactics`), *The Simplifier*, *Terms*, *Type Classes*, *Run-Time Code*, *Error Explanations*, *Index*. |
| **Theorem Proving in Lean 4** (TPIL) | https://lean-lang.org/theorem_proving_in_lean4/ | Canonical proof-development tutorial; incl. *Axioms and Computation* (see B.3). |
| **Functional Programming in Lean** (FPIL) | https://lean-lang.org/functional_programming_in_lean/ | Main resource for programmers. |
| **Mathematics in Lean** (MIL) | https://leanprover-community.github.io/mathematics_in_lean/ | Main resource for mathematicians formalizing math. |
| **Metaprogramming in Lean 4** | https://leanprover-community.github.io/lean4-metaprogramming-book/ | Elaboration, macros, custom tactics, `Environment` manipulation. |
| Logic and Proof | https://leanprover.github.io/logic_and_proof/ | Intro logic course in Lean. |
| The Mechanics of Proof | https://hrmacbeth.github.io/math2001/ | Undergraduate proof course. |
| Hitchhiker's Guide to Logical Verification | https://github.com/lean-forward/logical_verification_2025 | Graduate ITP course. |
| Natural Number Game / Lean Game Server | https://adam.math.hhu.de/ | Gamified intros. |
| VS Code extension manual | https://github.com/leanprover/vscode-lean4/blob/master/vscode-lean4/manual/manual.md | Editor integration. |

### B.2 Programmatic interaction (driving Lean headlessly)

The analogue of "how an external tool drives the prover" — this is the layer Theoremata's
Lean adapter binds to, mirroring how Isabelle/Rocq are driven.

#### B.2.1 `leanprover-community/repl` — the full JSON protocol

Repo: https://github.com/leanprover-community/repl . Transport is **JSON over stdin/stdout**,
one JSON object per request, **requests separated by a blank line** (an empty line flushes the
current object). The REPL is stateful: it accumulates numbered **environments** (`env`) and
**proof states** (`proofState`) that you reference by integer id to branch/backtrack.

**Build/run.**
- Inside the repl repo: `lake exe repl` (reads stdin, writes stdout).
- From *another* Lake project so its dependencies (e.g. Mathlib) are on `LEAN_PATH`, run the
  prebuilt binary under that project's env:
  `lake env /path/to/repl/.lake/build/bin/repl`.
- The binary is a filter: pipe requests in (`echo '{...}' | lake exe repl`) or drive it as a
  long-lived subprocess and read framed responses.

**(1) Command mode** — elaborate a whole Lean command/snippet.
```jsonc
// request — omit "env" to start from a fresh environment (imports allowed only here)
{"cmd": "def f := 2"}
// request — chain onto a previous environment by id
{"cmd": "example : f = 2 := rfl", "env": 0}
```
```jsonc
// response
{"env": 1,
 "messages": [
   {"severity": "error",                 // "error" | "warning" | "info"
    "pos":    {"line": 1, "column": 0},
    "endPos": {"line": 1, "column": 5},  // may be null
    "data":   "type mismatch ..."}],
 "sorries": [
   {"pos":    {"line": 1, "column": 18},
    "endPos": {"line": 1, "column": 23},
    "goal":   "⊢ Nat",                    // pretty-printed goal at the sorry
    "proofState": 0}]}                     // id to resume in tactic mode
```
Every accepted command returns a **new `env` id** (monotonic). `messages` carries all
diagnostics; `sorries` lists every `sorry`/hole with a resumable `proofState`.

**(2) Tactic mode** — step a proof from a `proofState` (usually one produced by a `sorry`).
```jsonc
// request
{"tactic": "apply Int.natAbs", "proofState": 0}
```
```jsonc
// response
{"proofState": 1,
 "goals": ["x : Unit\n⊢ Int"]}   // remaining goals; empty [] ⇒ proof complete
```
Each step yields a **new `proofState` id** and the list of remaining `goals` (pretty-printed
strings). `goals == []` means the branch is closed. Because ids are immutable you can fan out
several candidate tactics from the same `proofState` (search/backtracking) without re-running
prior steps.

**(3) File mode** — elaborate a file and optionally harvest every tactic step.
```jsonc
{"path": "test/file.lean"}                      // just elaborate; returns env + messages
{"path": "test/file.lean", "allTactics": true}  // also return every tactic invocation
```
```jsonc
// response (allTactics)
{"env": 0,
 "tactics": [
   {"tactic": "rw [h]",
    "proofState": 3,
    "pos":    {"line": 7, "column": 2},
    "endPos": {"line": 7, "column": 8},
    "goals":  "⊢ ..."}],
 "messages": [], "sorries": []}
```
An optional `{"path": "...", "allTactics": true, "infotree": "original"}` returns the raw
elaboration **infotree** (values: `"full"`, `"tactics"`, `"original"`, `"substantive"`) for
callers that need positions/term info beyond tactic strings.

**(4) Pickling (persist/restore state to disk)** — avoids re-importing Mathlib (minutes) on
every process start.
```jsonc
{"pickleTo": "env.olean", "env": 1}                 // → {"env": 1}
{"unpickleEnvFrom": "env.olean"}                    // → {"env": <n>} restored
{"pickleTo": "ps.olean", "proofState": 5}           // → {"proofState": 5}
{"unpickleProofStateFrom": "ps.olean"}              // → {"proofState": <n>}
```
Pattern: build a Mathlib-loaded env once, `pickleTo` it, then each worker `unpickleEnvFrom`
that file to warm-start.

**Error shapes.** Malformed JSON or an internal error returns `{"message": "..."}` (top-level,
not inside `messages`). Lean-level failures (type errors, unknown identifiers, unsolved goals)
come back as normal `messages` with `severity:"error"`; the request still "succeeds" at the
protocol level and yields an `env`. Distinguish "REPL/transport error" (top-level `message`)
from "Lean rejected the code" (`messages[].severity == "error"`).

**Integration notes for Theoremata's adapter.**
- Treat `env`/`proofState` ints as opaque handles in a session; never assume they reset.
- A proof is *complete* when the tactic-mode `goals` list is empty **and** the command that
  introduced it reports no `error` messages and no residual `sorries`.
- After completion, run the axiom gate (B.3) — the REPL does not audit axioms for you.

#### B.2.2 Other headless drivers

- **Lean LSP server** — `lake serve` / the server built into the toolchain; drives the VS Code
  extension (goal state, diagnostics, hovers). JSON-RPC (LSP + Lean extensions like
  `$/lean/plainGoal`). Suitable for interactive/editor-style automation; heavier than the REPL
  for batch stepping.
- **`lake env lean <file>`** — run the bare compiler under Lake's resolved environment (correct
  `LEAN_PATH`/olean search). `lake env lean --run file.lean` executes a `main`; plain
  `lake env lean file.lean` type-checks/elaborates one file headlessly and prints diagnostics to
  stdout/stderr. Simplest "compile one generated file" primitive.
- Related machine-interaction tools: **Pantograph** (https://github.com/leanprover/Pantograph —
  goal-centric API, sketch/draft support), **LeanDojo** (https://leandojo.org/ — trace + gym-style
  env for RL), **Lean4Web** (https://github.com/leanprover-community/lean4web — the Playground
  backend).

### B.3 Soundness / verification gate (what "LeanParanoia"-style checking must do)

Lean's trust story: a small **kernel** re-checks every proof term; "bugs in tactics do not
threaten the soundness of Lean as a whole." The reference chapter **Validating a Lean Proof**
(https://lean-lang.org/doc/reference/latest/ValidatingProofs/) lays out an *escalating* ladder
of checks, and it is the canonical spec for a paranoia gate. It also draws the honest-vs-
malicious line: it tolerates "mistakes and bugs in proofs and meta-code (tactics, attributes,
commands)" but not "code that clearly only serves to circumvent the system (such as using
`debug.skipKernelTC`)." A separate concern it stresses: *validity of the proof* ≠ *meaning of
the statement* — custom notation/`Decidable`/type-class instances can make a theorem statement
say less than it appears. Any gate must therefore also pin the **statement** (see comparator's
challenge/solution split below).

**The escalation ladder (weakest → strongest):**

1. **Kernel-accepted (blue double-check in VS Code).** The statement elaborated and the kernel
   accepted a proof term for it — for *this* theorem in *this* file. Guards against incomplete
   proofs, explicit `sorry`, and honest tactic bugs. Assumes statements mean what you think and
   library authors are honest. This is the baseline the REPL gives you (B.2.1: empty `goals` +
   no `error` messages).

2. **`#print axioms <name>` — audit the transitive axiom set.**
   Reports every axiom a declaration transitively depends on (ref also TPIL *Axioms and
   Computation*, https://lean-lang.org/theorem_proving_in_lean4/Axioms-and-Computation/).
   - **Whitelist (accepted; mathlib's standard):** with exact signatures —
     ```lean
     axiom propext        {a b : Prop} : (a ↔ b) → a = b
     axiom Classical.choice {α : Sort u} : Nonempty α → α
     axiom Quot.sound     : ∀ {α : Type u} {r : α → α → Prop} {a b : α},
                              r a b → Quot.mk r a = Quot.mk r b
     ```
     (`Quot`, `Quot.mk`, `Quot.lift`, `Quot.ind` are kernel primitives, not axioms; funext and
     `Classical.em` are *derived* from the above, so they never appear as axioms themselves.)
     A proof whose axiom set ⊆ {`propext`, `Classical.choice`, `Quot.sound`} is "clean."
     Typical clean output: `'thm' depends on axioms: [propext, Classical.choice, Quot.sound]`.
   - **Reject signals:**
     - `sorryAx` — surfaced whenever any `sorry` (or a failed/`admit`ted goal, or a
       `native_decide` that errored) is anywhere in the dependency graph. Primary
       "incomplete proof" detector. (The REPL also flags these as `sorries`, but `#print
       axioms` catches ones hidden in *dependencies* the REPL didn't re-elaborate.)
     - **Custom `axiom` declarations** — any name outside the whitelist means the theorem's
       validity is conditional on that axiom's soundness.
     - **Native evaluation axioms** — `native_decide` / `decide +native` / `bv_decide` admit
       results computed by *compiled native code*, expanding the TCB to the whole Lean
       compiler + every `@[implemented_by]` in scope. Historically this surfaced as the single
       `Lean.trustCompiler` axiom; **as of Lean 4.29.0 each native computation instead injects
       a dedicated auto-generated axiom (one per computation)** plus `Lean.ofReduceBool` /
       `Lean.ofReduceNat`. Detection rule: reject if the axiom set contains `Lean.ofReduceBool`,
       `Lean.ofReduceNat`, `Lean.trustCompiler`, or any auto-generated native axiom. External
       checkers (below) *cannot* validate these proofs.
     - `@[implemented_by]` / `@[extern]` — replace a definition's *runtime* behavior with
       unverified code. Not axioms themselves, but they are the mechanism native evaluation
       trusts; part of the paranoia surface whenever a proof leans on evaluation.
   - Drive it via the REPL: `{"cmd": "#print axioms myThm", "env": <n>}` and parse the `info`
     message `data`. Machine-check the axiom list against the whitelist as a hard gate.

3. **`lean4checker` / `leanchecker` — replay the `.olean` through the kernel in a fresh
   process.** https://github.com/leanprover/lean4checker . Reads declarations/proof terms **as
   stored in the compiled `.olean`** and replays them through the kernel independently, catching
   **environment hacking** — metaprograms that build an inconsistent `Environment` the
   interactive elaborator would accept, or `.olean` kernel-state tricks. Merged into Lean core as
   **`leanchecker`** (v4.28.0+; installs with the toolchain, no separate build):
   - `lake env leanchecker` — recheck all `.olean`s on the search path, in parallel.
   - `lake env leanchecker Mathlib.Data.Nat.Basic` — recheck one module.
   - `lake env leanchecker --fresh Mathlib` — replay **all** constants into a *fresh*
     environment (single-threaded, much slower, strongest). `--fresh` is the only documented
     flag.
   - Legacy standalone form: `lake exe lean4checker [--fresh] <Module>`.
   - Run it **after `lake build`**. It uses the Lean kernel itself (not an independent verifier),
     so it hardens against metaprogramming abuse but does not establish *external* soundness. It
     trusts `.olean` structural integrity and cannot stop malicious code that runs *during* the
     build.

4. **Comparator + external checkers (gold standard; adversarial/LLM setting).**
   https://github.com/leanprover/comparator — built by the Lean FRO for **trustworthy LLM Lean
   evaluation** (AIMO/Kaggle), i.e. exactly Theoremata's "judge an untrusted generated proof"
   problem. It separates a **trusted `Challenge.lean`** (your statement, possibly `sorry`'d)
   from an **untrusted `Solution.lean`** (the candidate proof of the *same-named* theorem), and
   guarantees the solution (a) proves the *same statement*, (b) uses no axiom outside a permitted
   list, and (c) is kernel-accepted. Config JSON:
   ```json
   {
     "challenge_module": "Challenge",
     "solution_module":  "Solution",
     "theorem_names":    ["todo1"],
     "definition_names": [],
     "permitted_axioms": ["propext", "Quot.sound", "Classical.choice"],
     "enable_nanoda":    false
   }
   ```
   Invoke (Linux) sandboxed:
   ```sh
   systemd-run --property=RestrictAddressFamilies=~AF_UNIX --user --pty \
     -E PATH="$PATH" --working-directory $(pwd) -- \
     bash -c 'lake env path/to/comparator config.json'
   ```
   Requires `landrun` (sandbox), `lean4export` (serializes the environment to a text export —
   comparator deliberately **does not mmap `.olean`s**, treating them as an attack surface), and
   optionally **`nanoda`** (`nanoda_bin`) for a second, independent Rust kernel. Steps: build
   `Challenge` and `Solution` each in a `landrun` sandbox → `lean4export` each → verify the
   statement's declarations match between environments → verify `Solution` bodies use only
   `permitted_axioms` → replay `Solution` into the Lean kernel (and nanoda if enabled). Setting
   `enable_nanoda: true` reduces the trust assumption to "the Lean **or** the nanoda kernel is
   correct." Also supports **definition holes** (`definition_names`) for conjecture-style
   challenges (`def ChallengeSolution : Prop := sorry`), but warns these can be *gamed* (a
   solution could define the hole as the conjecture itself and prove by `rfl`), so hole solutions
   must always get an extra human/verifier check.
   - Independent external kernels usable here: **nanoda** (Rust, https://github.com/ammkrn/nanoda_lib),
     **lean4lean** (Lean-in-Lean kernel, https://github.com/digama0/lean4lean), plus the export
     format from **lean4export** (https://github.com/leanprover/lean4export). These consume the
     *export*, not `.olean`s, so they catch checker-implementation bugs that the built-in kernel
     shares.

**Practical gate for Theoremata (recommended layering):**
1. Build/elaborate the generated file (REPL command mode or `lake env lean`); require no `error`
   messages and no residual `sorries`/empty `goals`.
2. `#print axioms <target>` and assert the set ⊆ whitelist with **no** `sorryAx`, no
   native/`ofReduceBool`/`trustCompiler` axiom, no custom axiom.
3. `lake build` then `lake env leanchecker --fresh <Module>` to re-replay through the kernel.
4. For *adversarial / model-generated* proofs where the statement itself must be pinned, use
   **comparator** (challenge = your canonical statement, solution = the generated proof) with
   `permitted_axioms` = whitelist and `enable_nanoda: true`.

### B.4 Project / build system (Lake)

Ref: https://leanprover-community.github.io/install/project.html and the reference manual's Lake
chapter (https://lean-lang.org/doc/reference/latest/Build-Tools-and-Distribution/Lake/).

**Project skeleton.** A Lake package needs three things at its root:
- **`lakefile.toml`** *or* **`lakefile.lean`** — package config, dependencies, targets.
- **`lean-toolchain`** — a single line pinning the exact toolchain, e.g. `leanprover/lean4:v4.29.0`
  or `nightly-2024-10-01`. `elan` reads this to select/download the compiler; `lake update` will
  rewrite it to a dependency-compatible version unless you pass `--keep-toolchain`.
- **`.lake/`** — local build dir; compiled **`.olean`** (+ `.ilean`, native `.o`/`.so`) live under
  `.lake/build/`, and fetched dependencies under `.lake/packages/`.

**`lakefile.toml` (declarative — preferred for generated projects).**
```toml
name = "my_package"
version = "0.1.0"
defaultTargets = ["MyLib"]
# compiler flags applied to every module:
leanOptions = ["--timeout=1000"]

[[require]]
name = "mathlib"
scope = "leanprover-community"          # Reservoir scope; or use git =
git  = "https://github.com/leanprover-community/mathlib4"
rev  = "v4.29.0"                        # tag / branch / commit
# local dep instead of git:
# path = "../mylib"

[[lean_lib]]
name   = "MyLib"
srcDir = "."                            # dir containing MyLib.lean / MyLib/*.lean

[[lean_exe]]
name = "myexe"
root = "Main"                           # module with `def main : IO Unit`
```

**`lakefile.lean` (DSL — programmatic).**
```lean
import Lake
open Lake DSL

package «my_package» where
  leanOptions := #[⟨`maxHeartbeats, 400000⟩]

require mathlib from git
  "https://github.com/leanprover-community/mathlib4" @ "v4.29.0"

@[default_target]
lean_lib «MyLib» where

lean_exe «myexe» where
  root := `Main
```

**`require` forms.** `require name from git "url" @ "rev"` · `require name from path "../dir"` ·
Reservoir-registry `require name @ "ver"` (resolved via https://reservoir.lean-lang.org/). TOML
uses `[[require]]` tables with `name` + one of `git`/`path`, plus optional `rev`, `scope`,
`subDir`.

**`elan`** — toolchain multiplexer (installs/selects Lean per `lean-toolchain`; toolchains in
`~/.elan/toolchains`). `elan which lean`, `elan toolchain list`, `elan default <ver>`.

**Lake CLI (the commands a headless adapter needs).**
- `lake new <name> [std|exe|lib|math] [.lean|.toml]` — scaffold in a **new** dir;
  `lake init <name> …` — scaffold in the **current** dir. `math` template adds the Mathlib dep.
  Example: `lake +v4.29.0 new my_project math` then `cd my_project && lake update`.
- `lake update [dep]` — resolve deps, write `lake-manifest.json`, sync `lean-toolchain`
  (`--keep-toolchain` to freeze it).
- `lake exe cache get` — **download prebuilt Mathlib `.olean` cache** (skips the multi-hour
  Mathlib build); `lake exe cache get!` forces re-download of corrupt/partial cache. Cache lives
  under `~/.cache/mathlib`. (This is a Mathlib-provided `cache` exe, distinct from the
  experimental built-in `lake cache` artifact store.)
- `lake build [targets]` — build headless. Target syntax: `MyLib` (default facet), `@pkg`
  (package default targets), `+Some.Module` (one module's artifacts), `:leanArts`. Flags:
  `-R/--reconfigure` (re-elaborate lakefile), `--no-build` (fail if not up-to-date),
  `--no-cache`, `--try-cache`, `-v/--verbose`, `-q/--quiet`, `--wfail` (fail on warnings),
  `--fail-level=error`.
- `lake exe <target> [args]` / `lake exec` — build then run an executable (e.g. `lake exe repl`).
- `lake env [cmd args]` — run `cmd` with `LEAN_PATH`/`LEAN_SRC_PATH`/`LEAN_SYSROOT` set to the
  resolved workspace env. `lake env lean File.lean` type-checks one file; `lake env lean --run
  File.lean` runs its `main`; `lake env leanchecker …` runs the kernel replay (B.3).
- `lake clean`, `lake test`, `lake lint`, `lake script run <name>`.
- Key env vars Lake sets: `LEAN_PATH`, `LEAN_SRC_PATH`, `LEAN_SYSROOT`, `LAKE`, `LAKE_HOME`;
  `LAKE_NO_CACHE=1` disables cloud caches.

**Minimal "scaffold + build one generated `.lean` file" recipe (headless).**
```sh
lake +v4.29.0 new gen math            # or reuse a warm project that already has `cache get`
cd gen
lake exe cache get                    # pull Mathlib oleans (only if depending on Mathlib)
printf 'import Mathlib\ntheorem foo : 1 + 1 = 2 := by norm_num\n' > Gen/Target.lean
# add `Gen.Target` to a lean_lib root, or just check the file directly:
lake env lean Gen/Target.lean         # elaborate + kernel-check; exit 0 on success
lake env leanchecker Gen.Target       # optional: replay through the kernel (after `lake build`)
```

### B.5 Automation tactics (vs Sledgehammer / CoqHammer)

| Tactic | What it does |
|---|---|
| `simp` / `simp_all` | Rewriting with the `@[simp]` lemma set; core normalizer. `simp only [lemmas]` restricts to given lemmas. |
| `omega` | Decision procedure for linear integer/nat arithmetic (Presburger fragment). |
| `norm_num` | Normalizes/decides numeric (in)equalities over ordered fields. |
| `decide` | Synthesize `Decidable p` and reduce it to `isTrue` **in the kernel** — no extra axioms. |
| `native_decide` (= `decide +native`) | Same, but compiles + runs the `Decidable` instance as native code; **admits via `Lean.ofReduceBool` + a per-computation axiom** (4.29+) — see B.3. |
| `aesop` / `aesop?` | White-box best-first proof search over `@[aesop]`-tagged rules; `aesop?` prints the found script. https://github.com/leanprover-community/aesop |
| `exact?` | Library search for a **single lemma that closes** the goal (`exact? says exact <lemma>`). |
| `apply?` | Library search for lemmas whose conclusion **unifies** with the goal (leaves subgoals). |
| `rw?` | Suggest rewrites (`rw [lemma]`) applicable to the goal. |
| `polyrith` | Linear-combination prover over commutative rings (calls out to Sage over the network). |
| `linarith` / `nlinarith` | Linear (and some nonlinear) arithmetic over ordered fields. |

#### B.5.1 `aesop` — build-ready detail (the closest thing to a "tactic hammer")

Add as a dep (Mathlib already transitively provides it):
```lean
require aesop from git "https://github.com/leanprover-community/aesop"   -- lakefile.lean
```
Invoke `by aesop`; use `aesop?` to emit a `Try this:` reconstructed script (note: script
generation "has known bugs" and may need hand-editing — but it *is* an auditable, kernel-checked
script, unlike a black-box hammer).

**Registering rules** — `@[aesop <phase>? <priority>? <builder>? <rule_sets>?] decl`:
- **Phases:** `safe` (applied eagerly, never backtracked), `unsafe` (backtrackable; **requires a
  success-probability %**, e.g. `unsafe 50%`), `norm` (normalization; optional integer penalty).
- **Builders:** `apply`, `simp` (norm only), `unfold`, `constructors`, `cases`, `forward`
  (`(immediate := [x])`), `destruct`, `tactic` (wrap `(by norm_num)`), `default`. Transparency:
  `(transparency := reducible)`; `transparency! := default` disables indexing (tries on every goal).
```lean
@[aesop unsafe 50% apply] theorem my_lemma : ... := ...
@[aesop safe apply (transparency := reducible)] theorem l2 : ... := ...
@[aesop unsafe [constructors 75%, cases 90%]] inductive T ...
```
**Per-call add/erase and rule sets:**
```lean
aesop (add safe foo, 10% cases Or, unsafe 50% apply bar)
aesop (erase A, baz)
aesop (rule_sets := [MyRules, -default, -builtin])
aesop (config := { maxRuleApplicationDepth := 10, enableSimp := false })
declare_aesop_rule_sets [MyRules] (default := true)
```
**Normalization loop** each step: negative-penalty norm rules → `simp_all` with global `@[simp]`
lemmas + aesop simp rules → positive-penalty norm rules, until fixpoint. So aesop *subsumes*
`simp`: any `@[simp]` lemma is used without re-registration.

#### B.5.2 Why there is no true "hammer"

Isabelle's **Sledgehammer** and Coq/Rocq's **CoqHammer** are *black-box*: they ship the goal +
selected premises to external ATP/SMT solvers (E, Vampire, Z3, CVC5) and then **reconstruct** the
resulting proof in the kernel. Lean has **no in-tree equivalent operating at Mathlib scale**.
Its nearest analogue is `aesop` — deliberately **white-box** (you see/control the rule set;
`aesop?` emits an auditable script) — with `exact?`/`apply?`/`rw?` covering the premise-selection
role a hammer performs implicitly. External/experimental hammers exist but are not in core/Mathlib
by default: **LeanHammer** (premise selection + ATP), **Duper** (a superposition prover written in
Lean that can reconstruct in-kernel, https://github.com/leanprover-community/duper), and
**lean-auto** (a translation/bridge to external provers feeding Duper). For Theoremata: treat
"hammer" as *not available as a single reliable tactic*; approximate it by (premise search via
Loogle/`exact?` → candidate lemmas) + (`aesop`/`simp`/`omega`/`decide`) and, if desired, wire
Duper as an optional external reconstructor.

### B.6 Library / retrieval corpus

- **mathlib4** — https://github.com/leanprover-community/mathlib4 ; docs at
  https://leanprover-community.github.io/mathlib4_docs/ . The unified math library and the
  primary retrieval corpus.
- **Syntactic / structural premise search:**
  - **Loogle** — https://loogle.lean-lang.org/ . Five filter kinds, **comma = AND**:
    | Kind | Example | Matches |
    |---|---|---|
    | By constant | `Real.sin` | lemmas mentioning `Real.sin` anywhere |
    | By name substring | `"differ"` | lemmas whose *name* contains `differ` |
    | By subexpression | `_ * (_ ^ _)` | any lemma with that shape (`_` = wildcard) |
    | Metavariable (linked) | `Real.sqrt ?a * Real.sqrt ?a` | `?a` must be the *same* term at both sites |
    | By main conclusion | `\|- tsum _ = _ * tsum _` | matches the goal (right of all `→`/`∀`) |
    | By type | `⊢ (_ : Type _)` / `⊢ (_ : Prop)` | definitions / theorems respectively |

    Combined: `Real.sin, "two", tsum, _ * _, |- _ < _ → _`.
  - **Loogle HTTP/JSON API** (concrete — verified live): `GET https://loogle.lean-lang.org/json?q=<url-encoded-query>`
    (the plain UI is `/?q=…`; swap to `/json` for machine output). Response shape:
    ```json
    {
      "count": 320,
      "header": "Found 320 declarations mentioning Real.sin.\nOf these, only the first 200 are shown.\n",
      "heartbeats": 3,
      "hits": [
        { "name":   "Real.sin",
          "type":   "(x : ℝ) : ℝ",
          "module": "Mathlib.Analysis.Complex.Trigonometric",
          "doc":    "The real sine function ..." }   // doc may be null
      ]
    }
    ```
    Only the first ~200 hits are returned regardless of `count`. On a bad query the JSON carries an
    `"error"` (and often `"suggestions"`) field instead of `hits`. The maintainer warns **"no
    stability of the format is guaranteed"** — treat fields defensively. `curl` example:
    `curl -s 'https://loogle.lean-lang.org/json?q=Real.sqrt%20%3Fa%20*%20Real.sqrt%20%3Fa'`.
    (This is the single most build-ready premise-search endpoint for an out-of-process adapter — no
    Lean toolchain required to query it.)
  - In-editor `#loogle "query"` runs the same engine locally (via `LeanSearchClient`, see below).
  - In-tactic: `exact?`, `apply?`, `rw?` (see B.5) — search over the *loaded* environment (needs a
    live Lean process / the REPL).
  - `#find` — name/pattern search in the editor.
- **Semantic / natural-language premise search:**
  - **LeanSearch** — https://leansearch.net/ (PKU BICMR); informal↔formal aligned corpus.
  - **Moogle** — https://moogle.ai/ (Morph Labs); first semantic Mathlib search (now dated).
  - **LeanExplore** — https://www.leanexplore.com/ ; hybrid search, selectable libraries
    (arXiv:2506.11085).
  - **LeanStateSearch** — search by current proof *goal state*.
  - **`LeanSearchClient`** — https://github.com/leanprover-community/LeanSearchClient .
    In-Lean commands (command/term/tactic modes): **`#search "query"`** (backend-configurable),
    **`#leansearch "query"`** (→ leansearch.net), **`#loogle <query>`** (→ Loogle),
    **`#statesearch`** (→ LeanStateSearch, no string arg).

### B.7 Install / toolchain (incl. Windows reality)

- Recommended path (all platforms): install **VS Code** + the **Lean 4 extension**
  (https://marketplace.visualstudio.com/items?itemName=leanprover.lean4); the extension
  bootstraps **elan** and **lake** automatically. Manual/CLI path:
  https://lean-lang.org/install/manual and
  https://leanprover-community.github.io/get_started.html .
- Toolchain = **elan** (version manager) + **lake** (build) + **lean** (compiler/kernel),
  all pinned per-project by `lean-toolchain`.
- **Windows caveats** (relevant: this project's box lacks a working `lake`/`lean` — commands
  aliased/absent). From the community install docs:
  - Antivirus can break the mathlib cache download: `curl: (35) schannel: next
    InitializeSecurityContext failed` → disable AV and run `lake exe cache get!`.
  - Corrupted toolchain downloads → `uncaught exception: no such file or directory`; fix by
    deleting the bad toolchain under `.elan\toolchains` and its `.elan\update-hashes` entry.
  - Practical implication for Theoremata on this host: the Lean adapter cannot rely on a
    local `lake`/`lean` being on PATH; drive Lean out-of-process via a known-good REPL binary
    (B.2), a containerized toolchain, or a remote worker, and gate results with `#print axioms`
    + `leanchecker` there rather than assuming a working local install.
- **Container / remote-worker options (the realistic path on this box):**
  - **Docker.** Base image installs elan non-interactively, then bakes a warm Mathlib cache:
    ```dockerfile
    FROM ubuntu:24.04
    RUN apt-get update && apt-get install -y curl git build-essential
    RUN curl -sSf https://elan.lean-lang.org/elan-init.sh | sh -s -- -y
    ENV PATH="/root/.elan/bin:${PATH}"
    # copy a project with lean-toolchain + lakefile, then:
    RUN cd /work && lake exe cache get && lake build
    ```
    (`elan` installer URL: `https://elan.lean-lang.org/elan-init.sh`; the older
    `leanprover/elan` GitHub releases also work.) Drive the container's `lake exe repl` over
    stdio, or `docker exec` per file.
  - **Remote worker.** Run the REPL (B.2.1) as a persistent service on a Linux host with the
    toolchain + Mathlib cache warm; pickle a Mathlib env once (`pickleTo`) and `unpickleEnvFrom`
    per request to keep latency low. Do the axiom gate + `leanchecker` (+ optional comparator)
    on that host.
  - **Managed services:** the Playground backend (Lean4Web) and cloud infra like AXLE
    (arXiv:2606.26442) exist for hosted Lean utilities if self-hosting is undesirable.

### B.8 Metaprogramming (driving elaboration / `Environment` / axiom internals)

Ref: **Metaprogramming in Lean 4** (https://leanprover-community.github.io/lean4-metaprogramming-book/).
Relevant when the adapter needs behavior the REPL doesn't expose (custom `#print axioms`-style
audits, bespoke goal extraction, environment introspection). Key chapters:
- **Expressions** (`.../main/03_expressions.html`) — the `Expr` type (the kernel's term language);
  what proof terms *are* when you audit them.
- **MetaM** (`.../main/04_metam.html`) — the elaboration monad stack `CoreM ⊂ MetaM ⊂ TermElabM ⊂
  CommandElabM`; where the `Environment`, metavariable context, and options live. `CoreM` holds the
  `Environment`; `MetaM` adds metavars/unification.
- **Macros** (`.../main/06_macros.html`) and **Elaboration** (`.../main/07_elaboration.html`) —
  how surface syntax becomes `Expr`; where `elab`/`macro` custom commands are defined.
- **Tactics** (`.../main/09_tactics.html`) — writing custom tactics operating on `TacticM` goals
  (`MVarId`s); the layer the REPL's tactic mode sits on.
- **Options** (`.../extra/01_options.html`) — `set_option`/`getBoolOption`, e.g. `maxHeartbeats`,
  `pp.*`.

Concrete internals a gate can reuse:
- `#print axioms` is implemented by `Lean.collectAxioms` (walks a constant's transitive
  `ConstantInfo` dependencies collecting `axiomInfo` names) — reproducible in `MetaM`/`CoreM` if
  you need programmatic (non-string) axiom sets rather than parsing REPL message text.
- The `Environment` maps `Name → ConstantInfo`; `env.find? name` + `.isUnsafe`/`.value?` lets you
  inspect a declaration's kind (axiom / def / theorem) and body.
- Relevant `#`-commands for interaction (ref chapter *Interacting with Lean*,
  https://lean-lang.org/doc/reference/latest/Interacting-with-Lean/): `#print axioms`,
  `#print <name>` (definition), `#check <term>` (elaborate + type), `#eval` (compile+run; `#eval!`
  to bypass the `sorry` guard), `#reduce` (normal form), `#synth <class>` (instance synthesis),
  `#guard_msgs` (assert exact messages — useful for regression-pinning generated output), `#where`,
  `#version`. Options: `set_option maxHeartbeats 400000`, `pp.all`, `pp.mvars.anonymous`.

---

### Source URLs (primary; all fetched cover-to-cover for this pass)

- Reference manual: https://lean-lang.org/doc/reference/latest/ — **ValidatingProofs/**,
  **Interacting-with-Lean/**, **The-Type-System/**, **Elaboration-and-Compilation/**,
  **Build-Tools-and-Distribution/Lake/**, releases/v4.29.0/ (per-computation native axioms).
- https://lean-lang.org/theorem_proving_in_lean4/Axioms-and-Computation/ (exact axiom signatures).
- https://github.com/leanprover-community/repl (full JSON protocol),
  /leanprover/lean4checker (leanchecker), /leanprover/comparator (challenge/solution judge),
  /ammkrn/nanoda_lib, /digama0/lean4lean, /leanprover/lean4export (external checkers),
  /leanprover-community/aesop, /leanprover-community/duper (external hammer),
  /leanprover-community/LeanSearchClient.
- https://loogle.lean-lang.org/ + the `/json?q=` API (schema verified live).
- https://leanprover-community.github.io/lean4-metaprogramming-book/ ,
  https://leanprover-community.github.io/ , /get_started.html , /install/project.html , /mathlib4_docs/ .
- https://leansearch.net/ , https://moogle.ai/ , https://www.leanexplore.com/ .

### Skipped as not integration-relevant (noted per instructions)

- Pedagogical books beyond metaprogramming (TPIL body, FPIL, MIL, Logic and Proof, Mechanics of
  Proof, Natural Number Game) — learning material, no build/integration surface; sitemap-listed
  only (B.1).
- FRO org pages (about/team/roadmap/blog), use-case marketing pages (Cedar/Aeneas/ArkLib/Veil/FLT),
  and community/social channels — context, not integration.
- Reference chapters on surface-language features not touched by the adapter (basic types, term
  syntax, type-class *authoring*, notation/`syntax` design, error-explanation catalogue).
