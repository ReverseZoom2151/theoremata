# Rocq (formerly Coq) — Backend Integration Study for Theoremata

Study date: 2026-07-07. Rocq is the 2024 rebrand of the Coq proof assistant.
Primary sources: <https://rocq-prover.org/> and <https://rocq-prover.org/docs>.
Latest release at time of writing: **Rocq Prover 9.2.0**, **Rocq Platform 2025.08.3**
(docs "master" = 9.3+alpha). Some older docs still say "Coq" and live under
`rocq-prover.org/doc/V8.x/` or historically `coq.inria.fr` (now redirected).

Goal: mount Rocq as a parallel FORMAL-SYSTEM backend alongside the existing Lean
backend (warm REPL loop, `#print axioms` + kernel re-check + soundness scan, Lake
scaffold, model-generated proofs, mathlib retrieval). Each section below maps a
Rocq mechanism to its Lean analogue.

---

# A. Full website sitemap (rocq-prover.org)

Top-level nav: Home · Standard Library · Learn (docs) · Platform · Packages ·
Community · Consortium · News.

## Home / marketing
| Page | URL | Purpose |
|---|---|---|
| Home | `https://rocq-prover.org/` | Landing page, release banners, featured resources |
| About | `https://rocq-prover.org/about` | What Rocq is; `#history`, `#Name` (Coq→Rocq rename), `#awards` |
| Why Rocq? | `https://rocq-prover.org/why` | Motivation / value proposition |
| Roadmap | `https://rocq-prover.org/roadmap` | Planned direction |
| Logo | `https://rocq-prover.org/logo` | Brand assets |

## Install / releases
| Page | URL | Purpose |
|---|---|---|
| Install | `https://rocq-prover.org/install` | Install matrix: Platform installers, opam, Nix, Docker, per-OS |
| Releases (all) | `https://rocq-prover.org/releases` | Full release list |
| Rocq Prover 9.2.0 | `https://rocq-prover.org/releases/9.2.0` | Core prover release notes |
| Rocq Platform 2025.08.3 | `https://rocq-prover.org/releases/2025.08.3` | Curated distribution release |
| Changelog / News | `https://rocq-prover.org/changelog` | Cross-version change log (also the "News" nav item) |

## Learn / documentation (see section B.1 for the doc tree)
| Page | URL | Purpose |
|---|---|---|
| Docs overview | `https://rocq-prover.org/docs` | Learning hub, beginner→advanced tracks; `#beginner_section` |
| Platform docs | `https://rocq-prover.org/docs/platform-docs` | Tutorials for Platform-bundled tools |
| Supporting tools | `https://rocq-prover.org/docs/tools` | IDEs / editor integrations |
| A Tour of Rocq | `https://rocq-prover.org/docs/tour-of-rocq` | Interactive intro |
| Using opam | `https://rocq-prover.org/docs/using-opam` | opam workflow incl. `#installing-rocq-packages` |
| opam packaging | `https://rocq-prover.org/docs/opam-packaging` | How to publish a package |
| Reference Manual | `https://rocq-prover.org/refman` | Canonical language + tooling manual |
| Stdlib Manual | `https://rocq-prover.org/refman-stdlib` | Prose manual for the standard library |
| Corelib theories | `https://rocq-prover.org/corelib` | Generated API of the minimal core library |
| Stdlib theories | `https://rocq-prover.org/stdlib` (nav "Standard Library") | Generated API of the full stdlib |
| OCaml API | `https://rocq-prover.org/api` | Rocq's internal OCaml API (plugin authors) |
| Books | `https://rocq-prover.org/books` | Software Foundations, Coq'Art, MathComp book, CPDT, etc. |
| Exercises | `https://rocq-prover.org/exercises` | Practice problems |
| Papers | `https://rocq-prover.org/papers` | Academic bibliography |

## Platform / packages / ecosystem
| Page | URL | Purpose |
|---|---|---|
| Platform | `https://rocq-prover.org/platform` | The Rocq Platform (prover + curated package set) |
| Packages | `https://rocq-prover.org/packages` | opam package index (**576 packages**), search + "Most Used/New/Recently Updated" |
| Platform Starter (GH) | `https://github.com/rocq-prover/rocq-platform-starter` | Experimental graphical installer (Platform + VS Code + VsRocq) |

## Community / governance / org
| Page | URL | Purpose |
|---|---|---|
| Community | `https://rocq-prover.org/community` | Entry to chat/forums/teams |
| Consortium | `https://rocq-prover.org/consortium` | Funding/membership body |
| Rocq Team | `https://rocq-prover.org/rocq-team` | Core dev team |
| Events | `https://rocq-prover.org/events` | Workshops / meetings |
| Rocq Planet | `https://rocq-prover.org/rocq-planet` (RSS `/planet.xml`) | Aggregated community blogs |
| Jobs | `https://rocq-prover.org/jobs` | Job board |
| Industrial Users | `https://rocq-prover.org/industrial-users` | Adoption showcase |
| Academic Users | `https://rocq-prover.org/academic-users` | Adoption showcase |
| Governance | `https://rocq-prover.org/policies/governance` | Project governance |
| Privacy Policy | `https://rocq-prover.org/policies/privacy-policy` | Policy |
| Code of Conduct | `https://rocq-prover.org/policies/code-of-conduct` | Policy |

## External / social
| Resource | URL | Purpose |
|---|---|---|
| Zulip (primary chat) | `https://rocq-prover.zulipchat.com` | Real-time dev + user chat |
| Discourse (forum) | `https://discourse.rocq-prover.org` | Long-form Q&A |
| GitHub org | `https://github.com/rocq-prover` | Source org |
| Core repo | `https://github.com/rocq-prover/rocq` | The prover source |
| Platform repo | `https://github.com/rocq-prover/platform` | Platform build scripts |
| VsRocq | `https://github.com/rocq-prover/vsrocq` | Official VS Code extension |
| Docker | `https://hub.docker.com/r/rocq/rocq-prover` | Official images (CI/headless) |
| Mastodon | `https://mastodon.acm.org/@RocqProver` | Announcements |

Note: a mirror of the docs also serves under `https://docs.rocq-prover.org/`.

---

# B. Integration-critical deep-dive

## B.1 Documentation sitemap (the doc tree)

Root: `https://rocq-prover.org/refman` → versioned tree at
`https://rocq-prover.org/doc/<VERSION>/refman/...` where `<VERSION>` ∈
`{master (9.3+alpha), V9.2.0, v9.0, V8.19.1, ...}`. Key nodes relevant to us:

- **Reference Manual** `.../refman/index.html` — language, tactics, tooling.
  - Commands / vernacular: `.../refman/proof-engine/vernacular-commands.html`
    (hosts `Print Assumptions`, `Print … Dependencies`, `Search`, `SearchPattern`).
  - The Rocq Prover commands: `.../refman/practical-tools/coq-commands.html`
    (`rocq compile`/`coqc`, `rocq repl`/`coqtop`, `rocq check`/`coqchk`, `_CoqProject`, `-R`/`-Q`).
  - Building projects: `.../refman/practical-tools/utilities.html`
    (`coq_makefile`, `rocq makefile`, dune guidance, dependency tools).
  - Tactics chapters incl. `auto`/`eauto`, `lia`/`nia`/`lra`/`nra`, `ring`/`field`,
    `Ltac`, `Ltac2`, `ssreflect` (`.../refman/proofs/...`, `.../refman/addendum/...`).
  - Recent changes: `.../refman/changes.html`.
- **Stdlib manual (prose)** `https://rocq-prover.org/refman-stdlib`.
- **Stdlib theories (generated API)** `https://rocq-prover.org/stdlib`.
- **Corelib theories** `https://rocq-prover.org/corelib` (minimal core).
- **OCaml API** `https://rocq-prover.org/api` (internal, for plugins).
- **Learning tracks** `https://rocq-prover.org/docs`: beginner (Tour of Rocq,
  Software Foundations Vol.1, Programs and Proofs, Coq'Art), intermediate
  (SF Vol.3, FRAP), advanced (CPDT).

## B.2 Programmatic interaction (the warm-REPL analogue)

Three headless drivers exist. **Ranked recommendation for Theoremata: coq-lsp
(with its Petanque sub-API) first, SerAPI as fallback, raw coqtop/coqidetop last.**

### coq-lsp — LSP / JSON-RPC + Petanque  (recommended)
Repo: `https://github.com/ejgallego/coq-lsp`.
Protocol spec: `https://github.com/ejgallego/coq-lsp/blob/main/etc/doc/PROTOCOL.md`.
Speaks standard **LSP over JSON-RPC** (LSP framing: `Content-Length: N\r\n\r\n<json>`)
with Rocq-specific extensions; built on the **"Flèche"** incremental document engine.
Install: `opam install coq-lsp` (and optionally `code --install-extension
ejgallego.coq-lsp`). Binaries: **`coq-lsp`** (the server) and **`fcc`**
("Flèche Coq Compiler") for one-shot machine-friendly checking without a full LSP
client. Version support (as of coq-lsp tracking master): **Rocq 9.2 / 9.1 / 9.0,
Coq 8.20, and Rocq `master`**; OCaml 4.14 / 5.3 / 5.4 — pin the coq-lsp version to the
Rocq version. It reads **`_RocqProject`** (and legacy `_CoqProject`); a malformed
project file makes the parsing library `exit 1`. On Windows the server may need
`--coqlib=<path> --coqcorelib=<path> --ocamlpath=<path>` flags.

**Data-shape reference (from PROTOCOL.md; TypeScript-flavored):**
```typescript
interface Hyp<Pp>        { names: Pp[]; def?: Pp; ty: Pp; }
interface Goal<Pp>       { hyps: Hyp<Pp>[]; ty: Pp; }
interface GoalConfig<G,Pp> {
  goals: Goal<G>[];                 // foreground goals
  stack: [Goal<G>[], Goal<G>[]][];  // focus zipper (bullets/{})
  bullet?: Pp; shelf: Goal<G>[]; given_up: Goal<G>[];  // given_up = admitted
}
interface Message<Pp> { range?: Range; level: number; text: Pp; }
interface GoalAnswer<G,Pp> {
  textDocument: VersionedTextDocumentIdentifier; position: Position; range?: Range;
  goals?: GoalConfig<G,Pp>; messages: Pp[] | Message<Pp>[]; error?: Pp;
  program?: ProgramInfo;
}
```
Custom methods that matter:

- **`proof/goals`** — the core "read the proof state" call. Request params:
  `textDocument: VersionedTextDocumentIdentifier`, `position: Position`,
  `pp_format?: 'Box'|'Pp'|'Str'`, `compact?: boolean`, `pretac?: string`
  (speculatively run a tactic first — great for "what if" probing without editing
  the doc), `command?: string`, `mode?: 'Prev'|'After'` (goals *before* or *after*
  the sentence at `position`). Response = `GoalAnswer` above.
- **`$/coq/fileProgress`** — server→client notification with
  `processing: {range, kind?}[]` where `kind ∈ {Processing=1, FatalError=2}`.
  Checking is **continuous + incremental**: you edit/append text and re-query rather
  than replay a state machine. Use it to know when a range is done before asking for goals.
- **`$/coq/serverStatus`** — `{status:"Busy", modname}` | `{status:"Idle"|"Stopped"}`.
- **`coq/getDocument`** — request `{textDocument, ast?:bool, goals?:'Pp'|'Str'}` →
  `{spans: RangedSpan[], completed:{status:['Yes'|'Stopped'|'Failed'], range}}`;
  each span carries `{range, ast?, goals?}`. This dumps the whole checked document
  (per-sentence AST + goals) in one shot — useful to snapshot a generated file.
- **`coq/saveVo`** — request `{textDocument}` → writes the `.vo` (compile via the LSP).
- **`coq/viewRange`**, `coq/trimCaches`, `coq/workspace_update` — housekeeping.
- Standard `textDocument/publishDiagnostics` returns errors;
  `textDocument/completion` (with `initializationOptions.completion.unicode`) and
  `textDocument/hover` are available.
- Init: `initializationOptions` = `CoqLspServerConfig`. Fields relevant to an agent:
  `eager_diagnostics`, `goal_after_tactic`, `admit_on_bad_qed` (**keep false** so a
  bad `Qed` is a hard error, not silently admitted), `max_errors`,
  `pp_type: 0|1|2` (0=Pp structured, 1=string, 2=box), `check_only_on_request`
  (lazy checking — set true to only check on `proof/goals`), `send_perf_data`,
  `debug`.

**Petanque** — a low-overhead, **session/state-handle** sub-API in the same server,
purpose-built for ML/agent proof search. This is the closest match to a warm Lean
REPL step loop: each call returns an integer **state handle `st`** you thread into the
next call (functional, immutable states → trivial to tree-search / backtrack by
reusing an older handle). Common opts: `Run_opts = {memo?:bool=true, hash?:bool=true}`.
Methods (exact params → response):
- **`petanque/get_root_state`** `{uri, opts?}` → `Run_result` — state at file start.
- **`petanque/get_state_at_pos`** `{uri, opts?, position}` → `Run_result`.
- **`petanque/start`** `{uri, opts?, pre_commands?:string, thm:string}` →
  `Run_result` — open the goal for theorem `thm` (optionally running `pre_commands`
  like extra `Require`/`Import` first).
- **`petanque/run`** `{opts?, st, tac:string}` → `Run_result` — run tactic(s) `tac`
  from state `st`, get the next state. **The core step.**
- **`petanque/run_at_pos`** `{opts?, textDocument, position, command}` → `Run_result`.
- **`petanque/goals`** `{st, opts?:{compact:bool}}` → `GoalConfig<string>` — read the
  goal state at handle `st`.
- **`petanque/premises`** `{st}` → `Premise[]` where
  `Premise = {full_name:string, file:string, info: Info|string}` and
  `Info = {kind, range?, offset:[int,int], raw_text}` — **premise retrieval** (see B.6);
  every lemma/def visible at that state, with source location + raw text.
- **`petanque/state/hash`** `{st}`→`number`, **`petanque/state/eq`**
  `{kind?:'Physical'|'Goals', st1, st2}`→`bool`, plus proof-only variants
  `petanque/state/proof/hash`, `petanque/state/proof/equal` — **dedup/loop-detection
  primitives for proof search** (compare states by goals to prune the tree).
- **`petanque/ast`** `{st, text}`→`Run_result<Option<Ast>>`,
  **`petanque/ast_at_pos`** `{uri, position}`→`Option<Ast>`.
- **`petanque/proof_info`** `{st}` → `Option<{name, statements:string[], range?}>`.

`Run_result` shape: `{st:number, hash?:int, proof_finished:bool, feedback:[int,string][]}`
— note `proof_finished` gives you the "is the proof done?" bit directly, and `feedback`
carries messages. Petanque ships a standalone JSON-RPC server binary **`pet`** /
**`pet-server`** (used by RL/agent frameworks; speaks the `petanque/*` methods over a
socket/stdio without the full LSP layer). Client libraries: **Coqpyt** (Python, drives
coq-lsp), `rocq-lsp-client` (OCaml). **For Theoremata's step loop, `pet-server` +
`start`/`run`/`goals`/`premises` is the leanest, most direct analogue of the warm Lean
REPL.**

### SerAPI — s-expression protocol  (mature fallback)
Repo: `https://github.com/ejgallego/coq-serapi`. Serializes Rocq's OCaml datatypes
to **sexps** (or JSON via `--printer=json`). Binaries: **`sertop`** (interactive REPL,
e.g. `rlwrap sertop --printer=human -Q .,Gen`), **`sercomp`** (batch compiler,
`.v`→sexp), **`sertok`** (tokenizer). Common flags: `--printer=human|sexp|json`,
`--implicit`, `-Q dir,lib`, `-R dir,lib` (note SerAPI uses **comma** `dir,lib`, not a
space, unlike coqc), `--topfile file`, `--omit_loc`, `--async`.

Protocol: send `(tag cmd)` (tag optional; SerAPI auto-tags); server acks
`(Answer tag Ack)` on parse success (or `SexpError` on parse failure) and finishes
each command with `(Answer tag Completed)`. It exposes an **explicit
document/state machine** (sentence ids = sids), which is excellent for tree search.
Core commands:
- **`Add`** `(Add (opts) "source")` — submit source; opts include `lim`, `ontop`
  (sid to add after), `newtip`, `verb`. Returns `(Answer tag (Added sid loc NewTip))`
  per sentence.
- **`Exec`** `(Exec sid)` — execute a sentence id (advance kernel state).
- **`Cancel`** `(Cancel (sid1 sid2 ...))` — retract sentences (backtrack — the
  explicit state machine; reuse an earlier sid to branch).
- **`Query`** `(Query ((sid N) (preds ...) (limit N) (pp (...))) <Objective>)` —
  Objectives: **`Goals`**, `EGoals` (existential goals), `Ast`, `TypeOf`, `Names`,
  `Definition`, `PNotations`, **`Locate`**, **`Search`**, `Vernac`, `LtacProfResults`,
  `Option`. This reads proof state / goals / search results.
- **`Print`** `(Print ((sid N) (pp ...)) object)` — invoke Rocq's pretty-printers.
- **`Parse`** — tokenize.
Answer/feedback shapes: `(Answer tag Ack)`, `(Answer tag (Added sid loc NewTip))`,
`(Answer tag (ObjList ((obj) (obj) ...)))`, `(Answer tag (CoqExn ...))` (error),
`(Answer tag Completed)`, and async `(Feedback ((doc_id D)(span_id sid)(route N)(contents msg)))`.
Example:
```
(Add () "Lemma x : 1=1. Proof. trivial. Qed.")
  → (Answer 0 Ack) (Answer 0 (Added 2 ... NewTip)) ... (Answer 0 Completed)
(Exec 2)                       → ... (Answer 1 Completed)
(Query ((sid 2)) Goals)        → (Answer 2 (ObjList ((CoqGoal ...)))) (Answer 2 Completed)
```
SerAPI historically tracks Coq/Rocq versions with a small lag (verify a `sertop`
build exists for your Rocq before choosing it over coq-lsp).

### coqtop / coqidetop (lowest level)
`rocq repl` (= `coqtop`) is the interactive toplevel; `coqidetop` is the XML-protocol
backend behind RocqIDE. Usable but the XML protocol is clunkier than LSP/sexp for an
agent — prefer coq-lsp/SerAPI. `rocq repl-with-drop` allows dropping to an OCaml
toplevel via `Drop.`. `rocq repl` honors `-topfile`, `-l file` (load a `.v`),
`-require`/`-ri qualid`, `-q`/`-noinit`, and the flag **`Rocqtop Exit On Error`**
(off by default) → exit code 1 on any error, handy for scripted headless checks.
Source: `https://rocq-prover.org/doc/master/refman/practical-tools/coq-commands.html`.

## B.3 Soundness / verification gate

This is the crux for parity with the Lean gate (`#print axioms` + LeanParanoia replay
+ soundness scan). Rocq gives a **two-layer** audit:

### Layer 1 — `Print Assumptions` (the `#print axioms` analogue)
On `.../refman/proof-engine/vernacular-commands.html`. "Displays all the assumptions
(axioms, parameters and variables) one or more theorems or definitions depends on."
Syntax: `Print Assumptions <qualid>.` Output semantics:
- If the term is fully constructive, it prints exactly **`Closed under the global
  context`** (i.e. no axioms) — the green-light string to match on.
- Otherwise it prints sections like `Axioms:` / `Variables:` and lists each
  axiom/parameter/variable with its type, e.g.
  `functional_extensionality : forall ... , f = g`, `Classical_Prop.classic`,
  `proof_irrelevance`, or a section `Variable`.

Related, finer-grained variants (same page) — all take `qualid+`:
- `Print Opaque Dependencies <qualid>.` — opaque constants relied on (`Qed`-sealed).
- `Print Transparent Dependencies <qualid>.` — transparent constants relied on.
- `Print All Dependencies <qualid>.` — union of the above (axioms + all constants).

**Reading it programmatically:** run it as a vernac via `petanque/run`/SerAPI
`(Query ... Vernac)` (or `coqc`-capture) and parse. Structured access: `Print
Assumptions` output is a message you get in the `feedback`/`messages` channel — parse
the head line: `Closed under the global context` = PASS-clean; otherwise split the
listed `name : type` entries.

**Whitelist gate design:** run `Print Assumptions` on the target theorem, parse the
listed names, and PASS only if the set is a subset of an approved whitelist
(commonly: empty = "Closed under the global context", or the classical-logic set
{`classic`, `functional_extensionality`, `functional_extensionality_dep`,
`proof_irrelevance`, `Eqdep.Eq_rect_eq.eq_rect_eq`, `constructive_indefinite_description`}).
Anything else (esp. an unexpected `Axiom` or a section `Variable`) fails the gate.
Caveat: `Print Assumptions` reports what is *reachable through the term*, so it is the
authoritative axiom audit — an `Admitted` lemma surfaces here as an axiom named after
the lemma. But see the cheat vectors below (disabled kernel checks are NOT axioms and
will NOT appear here — that is why Layer 3 exists).

### Layer 2 — `coqchk` / `rocq check` (the LeanParanoia kernel-replay analogue)
Man page: `https://man.archlinux.org/man/extra/rocq/rocqchk.1.en`;
refman `.../refman/practical-tools/coq-commands.html`. "**rocqchk is the standalone
checker of compiled libraries (.vo files produced by rocqcompile).**" It re-runs the
kernel type-checker on compiled `.vo` files **in a separate, minimal trusted-base
binary that cannot be tainted by plugins/`Ltac`/vernacular tricks** — exactly the
LeanParanoia "independent re-check" role. Usage: `rocqchk [options] modules`
(modules by logical name or `.vo` path; dependencies are found on the load path and
checked recursively). Flags:
- `-R dir rocqdir` / `-Q dir rocqdir` — map physical dir to logical path (same as prover).
- `-I dir` — add include path.
- `-o` / `--output-context` — print a summary of the logical content verified
  (**use this to enumerate exactly what was admitted into the trusted context**).
- `-norec module` — check the module but not its dependencies.
- `-admit module` — mark module + deps as trusted (skip re-check) — **avoid in the gate.**
- `-silent`, `-m/--memory`, `-impredicative-set`, `-coqlib dir`, `-where`, `-v`, `-h`.
- Exit code **0 = all requested checks succeeded**; non-zero = failure. Clean CI signal.

### Layer 3 — source-level scan for cheat vectors
`Print Assumptions` + `coqchk` catch axioms, but the agent should also textually /
AST-scan generated `.v` (best via `coq/getDocument`'s per-span AST, or a regex
pre-filter) for:
- **`Admitted.`** — ends a proof and **declares the initial goal as an axiom** (refman
  proof-mode). Surfaces in `Print Assumptions` on that lemma, but scan anyway to fail fast.
- **`admit`** tactic (leaves a proof hole) and **`give_up`** (ssreflect) — leave
  `given_up` goals; in Petanque, `proof_finished` will be true yet the term is admitted.
- Assumption commands (refman `language/core/assumptions.html`): grammar
  `assumption_token Inline? (ident_decl+ : type)+` where `assumption_token ∈
  {Axiom, Axioms, Conjecture, Conjectures, Parameter, Parameters, Hypothesis,
  Hypotheses, Variable, Variables}`. `Axiom/Conjecture/Parameter` extend the **global**
  environment (accept `#[local]`); `Hypothesis/Variable` are **section-local**
  (discharged into dependents on `End`). Also scan `Context`. The `:>` form declares a
  coercion. All of these introduce postulates → fail the gate unless whitelisted.
- **`Admit Obligations`** / `Obligation Tactic := admit` (Program mode).
- **Disabled kernel checks (the dangerous, silent ones — NOT reported by `Print
  Assumptions`):** `Unset Guard Checking` (allows non-terminating fixpoints),
  `Unset Positivity Checking` (non-strictly-positive inductives → `False` provable),
  `Unset Universe Checking` / `-type-in-type` (Type:Type → inconsistent),
  `-impredicative-set`, and SProp misuse (`-allow-sprop`, definitional proof
  irrelevance). Scan for every `Unset ... Checking`, `#[bypass_check(...)]` attribute,
  and the CLI flags in `-arg`.

Kernel/opacity notes: proofs closed with `Qed` are **opaque** (kernel type-checks the
term, then stores it as an opaque constant, body hidden from conversion); `Defined`
keeps them **transparent** (body usable in conversion/unfolding). **Both are fully
kernel-verified — opacity is not a soundness risk.** The real risks are (a) axioms,
(b) the disabled-checks above. `coqchk`/`rocqchk` re-runs the kernel and will reject a
`.vo` whose terms don't type-check, but it **honors the same relaxed flags** the `.vo`
was built with — so a `.vo` built with `-type-in-type` can still pass `rocqchk`. That
is precisely why the source scan (Layer 3) is mandatory, not optional.

**Recommended Rocq gate = `Print Assumptions` (whitelist) + compile to `.vo` with
`coqc`/`rocq compile` + independent `rocqchk -o -silent` (exit 0) + source scan for
admit/axiom/Unset-Checking/bypass_check.** All three layers; none subsumes another.

## B.4 Project / build system (scaffold + compile one generated `.v`)

Refman: `.../refman/practical-tools/coq-commands.html` and `.../utilities.html`.

- **`coqc` / `rocq compile` (`rocq c`)** — compile a `.v` into a `.vo`
  (plus optional `.vos`/`.vok`/`.glob`). The file's **basename must be a valid Rocq
  identifier** (`Generated.v` OK; `my-file.v` NOT). Key flags:
  `-R dir path`, `-Q dir path`, `-I dir` (OCaml objects), `-o file` (output name),
  `-noinit` (skip `Init.Prelude`), `-w <warnings>` (e.g. `-w +all` / `-w -notation`),
  `-native-compiler yes|no|ondemand`, `-vos` / `-vok` (see below), `-boot`,
  `-arg X` (pass X through). Exit code **0 = success, 1 = failure** (any error).
  Minimal headless build of a single generated file:
  ```
  rocq compile -R . Theoremata Generated.v      # or: coqc -R . Theoremata Generated.v
  rocq check   -R . Theoremata.Generated          # or: rocqchk -R . -o -silent Theoremata.Generated
  ```
- **`-R dir dirpath`** vs **`-Q dir dirpath`**: both bind a physical dir to a logical
  module path. With `-R .`, `./File.v` is loadable as `dirpath.File` **and** its short
  names are importable *partially-qualified* (no `From`). `-Q` maps the same but
  requires *fully-qualified* `Require` (or `From dirpath Require File`). Use these
  instead of hard-coding paths. (Note: SerAPI writes these as `-R dir,dirpath` with a
  comma; coqc/coq-lsp use a space.)
- **Output artifacts:** `.vo` = compiled, fully kernel-checked library object (the unit
  `rocqchk` re-checks); `.vos` = interface-only quick object (proofs **skipped**, bodies
  elided — fast, **not** kernel-sound, for IDE/dev only); `.vok` = a marker that the
  proofs of a `.vos` were later checked; `.glob` = cross-reference/index data (feeds
  `coqdoc` and premise indexing); `.aux` = internal. **For the soundness gate always
  build real `.vo` (never gate on `.vos`).**
- **`_CoqProject`** (Rocq also reads **`_RocqProject`**) — the project manifest read by
  IDEs, `coq_makefile`, dune, and coq-lsp. Recognized line types:
  `-R <path> <Module>`, `-Q <path> <Module>`, `-I <dir>` (OCaml libs),
  `-arg <flag>` (extra coqc arg), `-docroot <path>`, `-exclude-dir <dir>`
  (default excludes `CVS`, `_darcs`), and bare lines listing the `.v` files. This is
  the file to generate when scaffolding a workspace (analogue of the Lake config).
- **`coq_makefile` / `rocq makefile`** — generate a `Makefile` from the project file:
  `rocq makefile -f _CoqProject -o Makefile` (or emits `RocqMakefile` + `RocqMakefile.conf`).
  Targets include `all`, `install`, `clean`, plus `.vos`/`.vok` quick targets. Simple,
  no extra deps — good default for a throwaway single-file build. It calls `coqdep`
  internally for ordering. (Docs advise checking generated files into VCS and
  regenerating on Rocq upgrade.)
- **`coqdep` / `rocq dep`** — computes inter-`.v` dependencies for build ordering.
- **`.vos`/`.vok` quick workflow** — `coqc -vos File.v` produces interfaces fast
  (proofs deferred); a later `coqc -vok` / `make vok` checks the deferred proofs. Use
  only to accelerate large-project *development*; the gate must use full `.vo`.
- **dune** — the OCaml build tool also builds Rocq projects via `(coq.theory (name X)
  (package P) (theories dep1 dep2) (flags ...))` stanzas plus `(lang coq X.Y)` in
  `dune-project`; heavier but the ecosystem standard for multi-package projects. For a
  single generated file, `coqc`/`rocq compile` directly (or coq-lsp `coq/saveVo` /
  `fcc`) is lighter.

Scaffold plan for Theoremata: emit `_CoqProject` (`-R . Gen` + the `.v`), write
`Generated.v`, run `rocq compile`, then `rocqchk`. Mirrors the Lake scaffold + build the
Lean backend already does. Concrete end-to-end recipe in **B.8**.

## B.5 Automation (candidate tools to expose to the agent)

Built-in (refman tactic chapters):
- **`auto` / `eauto`** — Prolog-style hint-database proof search (`eauto` allows
  existentials); tunable with `Hint` databases. First-line closer.
- **`intuition` / `tauto`** — propositional / intuitionistic logic solver.
- **`firstorder`** — first-order logic proof search.
- **`lia` / `nia`** — linear / nonlinear integer arithmetic (Presburger; the Rocq
  `omega` successor, from `Coq.micromega`). **`lra` / `nra`** — real/ordered-field
  arithmetic. `psatz` for positivstellensatz.
- **`ring` / `ring_simplify`** — equalities in commutative rings; **`field`** — field
  equalities (division).
- **`congruence`** — congruence closure; **`btauto`** — boolean tautologies.
- **`Ltac`** and **`Ltac2`** — the tactic metalanguages for writing custom automation
  (Ltac2 is typed, better for programmatic generation).
- **ssreflect** — the Small-Scale Reflection tactic language (`//`, `//=`, `rewrite`,
  `case`, `elim`, `have`), the backbone of MathComp; strong for structured math proofs.

External / heavyweight — **CoqHammer** (repo `https://github.com/lukaszcz/coqhammer`;
site `https://coqhammer.github.io/`): **the Sledgehammer analogue.** Two independently
usable parts:

**(1) `hammer`** — ML premise-selection + translation of the goal to external ATPs,
then **reconstruction** of a native Rocq proof (the ATP is a *hint oracle only*; the
reconstructed term is kernel-checked, so ATP bugs cannot compromise soundness).
Load: `From Hammer Require Import Hammer.` Supported ATPs (≥1 on `PATH`):
**Vampire** (`vampire`, recommended), **CVC4/cvc5** (`cvc4`, ≥1.6, GPL build faster),
**E** (`eprover`), **Z3** (`z3_tptp`, needs the TPTP frontend built). Config (all
`Set`/`Unset`):
`Set Hammer ATPLimit n` (ATP timeout s, default 20), `Set Hammer ReconstrLimit n`
(reconstruction timeout, default 5), `Set Hammer SAutoLimit n` (pre-attempt, default
1), `Set Hammer GSMode n` (n>0 = n parallel strategies; 0 = ordinary),
`Set Hammer Predictions n` (default 1024), `Set Hammer PredictMethod "knn"|"nbayes"`,
`Set/Unset Hammer Vampire|CVC4|Eprover|Z3` (per-prover toggle),
`Set Hammer PredictPath "/path"`, `Set/Unset Hammer FilterProgram|FilterClasses`
(skip `Coq.Program.*`/`Coq.Classes.*`, default on), `Add Hammer Filter <mod>`,
`Set Hammer MinimizationThreshold n`. Diagnostics: `predict n`, `Hammer_version`,
`Hammer_cleanup`, `Set Hammer Debug`, `hammer_features`. **On success `hammer` prints
a reconstruction tactic (e.g. `hauto use: ... unfold: ...`) with no ATP calls and no
time limits — Theoremata should REPLACE the `hammer` call with that reconstruction
line in the final `.v`, because raw `hammer` success is not reproducible.**

**(2) The `sauto` family** — a powerful general CIC proof-search, **no ATPs, pure
tactics**, load `From Hammer Require Import Tactics.` Strength ladder (fast→strong):
`sdone` (leaf, no backtracking) < `strivial` < `qauto` (limit 100) < `hauto`
(= `sauto inv: - ctrs: -`) < `sauto` (full inhabitation search). Plus `best`
(auto-searches for the best `sauto` option set and prints it), `srun tac`,
`sfinal tac`, and never-failing simplifiers `simp_hyps`, `sintuition`, `qsimpl`,
`ssimpl`. Key `sauto`/`hauto`/`qauto` options (share syntax):
`use: [lemmas]`, `inv: [inds]` (inversion targets; default `*`), `ctrs: [inds]`
(constructors; default `*`), `unfold: [consts]` / `unfold!:` (forced),
`db: [hintdbs]`, `depth: n`, `limit: n` (cost, default 1000),
`brefl:on|off` (boolean reflection), `dep:on|off`, `quick:/q:`, `lazy:/l:`.
**Important limitation: `sauto` never performs induction — the agent must supply
`elim`/`induction` manually before calling it.**

Install via opam: **`coq-hammer`** (full plugin, needs ATPs on `PATH`) and
**`coq-hammer-tactics`** (the `sauto`/`hauto` tactics only, **no external ATPs**):
`opam install coq-hammer` / `opam install coq-hammer-tactics`. Track the Rocq version
(latest per first-pass: `v1.3.3-rocq9.2`, July 2026, tracks Rocq 9.2; LGPL-2.1).

**Verdict for Theoremata: YES, expose in two tiers.** **`coq-hammer-tactics`
(`sauto`/`hauto`/`qauto`/`best`) always** — pure, no infra, no soundness cost (proofs
are ordinary kernel-checked terms). Full **`hammer`** only when Vampire/E/Z3/CVC4 are
provisioned (CI/Docker) — and always gate its output through the same `Print
Assumptions`/`rocqchk`/source-scan audit as any other proof; substitute the printed
reconstruction tactic for the non-deterministic `hammer` call.

## B.6 Library / retrieval corpus

- **Standard library** — `https://rocq-prover.org/stdlib` (generated API),
  `https://rocq-prover.org/refman-stdlib` (prose); minimal **corelib** at
  `https://rocq-prover.org/corelib`.
- **MathComp (Mathematical Components / ssreflect)** — the large algebra/number-theory
  corpus, the closest thing to mathlib for *structured* math and the prime retrieval
  target. Built with **Hierarchy-Builder**; SSReflect itself is now part of the core
  Rocq distribution. Package split (all under the `mathcomp` logical root):
  `rocq-mathcomp-boot`, `rocq-mathcomp-ssreflect`, `rocq-mathcomp-order`,
  `rocq-mathcomp-fingroup`, `rocq-mathcomp-finite-group`, `rocq-mathcomp-algebra`,
  `rocq-mathcomp-solvable`, `rocq-mathcomp-field`, `rocq-mathcomp-character`,
  `rocq-mathcomp-group-representation` (legacy `coq-mathcomp-*` still exist);
  **analysis** is a separate library (`github.com/math-comp/analysis`). Install:
  `opam install rocq-mathcomp-algebra` (pulls ssreflect/boot/fingroup/order deps).
  Consumed via, e.g., `From mathcomp Require Import all_ssreflect.` or granular
  `From mathcomp Require Import ssreflect ssrbool ssrnat eqtype seq.`. Lemma naming is
  systematic (e.g. `addnC`, `mulnA`, `subnn`, `leq_add`) — the `_C`/`_A`/`_r`/`_l`
  suffix convention makes `Search`/`SearchPattern` highly effective. Book:
  `https://math-comp.github.io/mcb/`; per-version HTML API at math-comp.github.io.
  License CeCILL-B. **ssreflect tactic cheatsheet for generation** (from the refman
  SSReflect chapter): `move=> pat` (intro), `move: h` (generalize), `case: h`,
  `elim: h` (induction), `apply: lem`, `exact: t`, `rewrite lem` / `rewrite -lem`
  (right-to-left) / `rewrite /def` (unfold) / `rewrite {2}lem` (occurrence) with the
  switches `//` (close trivial goals by `done`), `/=` (simplify), `//=` (both);
  forward steps `have h : T`, `have h : T by tac`, `suff/suffices`, `wlog`, `pose`,
  `set x := t`; closers `by [ ]` / `done`; congruence `congr`; under-binder rewrite
  `under ... => ...` / `over`; the **views/reflect** mechanism `apply/viewName`,
  `move/viewName` bridging `bool ↔ Prop` (the `reflect P b` idiom).
- **opam ecosystem** — package index at `https://rocq-prover.org/packages`
  (**576 packages**; tabs Most Used / New / Recently Updated; free-text
  "Search Rocq packages"). Other notable libs: `coq-flocq` (floats), `coq-ext-lib`,
  `rocq-equations`, `coq-stdpp`. Discover via `opam search --or rocq coq`,
  `opam show <pkg>`.
- **Premise search / retrieval tooling** (map to mathlib retrieval):
  - In-session vernacular (refman `vernacular-commands.html`):
    **`Search <search_query>+ [inside|outside qualid+]`** — the workhorse. A
    `search_query` combines **items** with `-` (exclude), `[a | b]` (disjunction of
    conjunctions), and location filters `head:`, `hyp:`, `concl:`, `headhyp:`,
    `headconcl:`, kind filter `is: (Theorem|Lemma|Definition|Fixpoint|Axiom|…)`,
    pattern items `one_pattern` (holes `_`/`?x`), notation items `"str"%scope`, and
    substring items `"str"`. **`SearchPattern <one_pattern>`** (conclusion/hyp matches
    a pattern), **`SearchRewrite <one_pattern>`** (lemmas usable as rewrites), legacy
    `SearchAbout`. Scope with `inside M` / `outside M`. Tuning: the `Search Blacklist`
    table (`Add Search Blacklist "str"`) and flag `Search Output Name Only`. Also
    **`Locate <name>`** (find qualified name/notation), **`About <ref>`**,
    **`Print <ref>`** (kind, long name, type, opacity, implicits, scopes),
    **`Inspect <n>`** (n most-recent global objects), `Print All`, `Print Section`.
  - Programmatic: **coq-lsp `petanque/premises`** returns declared objects visible at
    a state — structured `{full_name, file, info:{kind, range?, offset, raw_text}}`,
    ideal for feeding a retriever/embedder. coq-lsp also offers LSP
    `textDocument/completion`. Run `Search`/`SearchPattern` via SerAPI `(Query ((...))
    Search)` (Objective `Search`) or `petanque/run` and parse results for a retrieval
    index. Build a corpus by `.glob` scraping (names+kinds+locations),
    `rocqchk -o --output-context` (enumerate what entered the trusted context), or by
    enumerating loaded modules' constants.

## B.7 Install / toolchain

Source: `https://rocq-prover.org/install`, `https://rocq-prover.org/docs/using-opam`.
- **opam (fine-grained; recommended for the backend)** — exact sequence:
  ```
  opam init                                    # first time only
  eval $(opam env)
  opam switch create rocq-9.2 5.3.0            # dedicated switch on an OCaml compiler
  eval $(opam env --switch=rocq-9.2)
  opam repo add rocq-released https://rocq-prover.org/opam/released
  opam update
  opam install rocq-prover                      # core prover (rocq-core, rocq-stdlib)
  # pin a version:  opam install rocq-prover rocq-core=9.2.0   (see `opam info rocq-core`)
  #                 opam pin add rocq-core 9.2.0
  opam install coq-lsp                          # the recommended driver (+ pet-server)
  opam install rocq-mathcomp-algebra            # retrieval corpus (pulls ssreflect deps)
  opam install coq-hammer-tactics               # sauto/hauto (no ATPs)
  opam install coq-hammer                        # full hammer (needs ATPs on PATH)
  opam install rocqide                           # optional GUI (not needed headless)
  ```
  Package names: `rocq-prover`, `rocq-core`, `rocq-stdlib`, `coq` (compat),
  `rocqide`/`coqide`, `rocq-mathcomp-*`, `coq-lsp`, `coq-serapi` (`sertop`),
  `coq-hammer`, `coq-hammer-tactics`. On Windows run the Platform scripts first to set
  env vars, or use opam under WSL.
- **Rocq Platform** — binary installers for macOS + Windows
  (`/releases/2025.08.3#recommended-binary-installers`); Linux/macOS/Windows also build
  from source via Platform scripts (`github.com/rocq-prover/platform`, per-OS
  `README_{Linux,macOS,Windows}.md`). Bundles a consistent prover + curated package set
  (incl. MathComp). Rocq Platform Starter adds VS Code + VsRocq.
- **Docker (preferred for headless/CI + reproducible ATPs)** — image
  **`rocq/rocq-prover`** for Rocq ≥ 9.0 (Debian-12-slim + opam 2.x; legacy
  **`coqorg/coq`** for Coq ≤ 8.20.1). Tags: `9.2`, `9.2.0`, `latest`, `dev`, plus
  `-ocaml-4.14-flambda` / `-native` variants. Default in-container user is `rocq`
  (`coq` on legacy). Canonical one-off compile (docker-coq wiki pattern):
  ```
  docker pull rocq/rocq-prover:9.2
  docker run --rm -v "$PWD:/home/rocq/project" -w /home/rocq/project \
    rocq/rocq-prover:9.2 bash --login -c "rocq compile -R . Gen Generated.v"
  ```
  CI: `rocq-community/docker-coq-action@v1` with `image: 'rocq/rocq-prover:dev'`
  (`github.com/rocq-community/docker-rocq`, `.../docker-coq/wiki`).
- **Nix** — well-maintained (nixpkgs). **Homebrew** on macOS.
- **Windows** — native Platform installer exists; **WSL recommended** for full parity
  with Linux methods (opam, Docker, coq-lsp/`pet-server` in WSL). Per project memory
  this dev machine lacks native `lean`/`lake`; **plan to run the Rocq backend via WSL
  or the `rocq/rocq-prover` Docker image**, not native Windows.
- **CI / headless** — everything (`rocq compile`/`coqc`, `rocqchk`, `coq-lsp`+`fcc`,
  `pet-server`, `sertop`) runs without a GUI; drive from the Docker image for determinism.
- **Editor front-ends** (not needed for the agent): VsRocq (VS Code), Proof General +
  company-coq (Emacs), Coqtail (Vim), RocqIDE.

## B.8 End-to-end build-ready recipes

**(1) Scaffold + compile + gate one generated `.v` (the core loop, no server):**
```
# _CoqProject
-R . Gen
-arg -w -arg +all
Generated.v
```
```bash
rocq makefile -f _CoqProject -o Makefile     # optional; or call coqc directly
rocq compile -R . Gen Generated.v            # → Generated.vo ; exit 0 = compiled
rocqchk -R . -o -silent Gen.Generated        # independent kernel re-check ; exit 0 = trusted
# then: run `Print Assumptions Gen.thm.` (via coqc capture or a driver) + source-scan.
```

**(2) Step a proof and read goals — Petanque (JSON-RPC over `pet-server`):**
```jsonc
// 1. open the goal for theorem "foo" in file:///.../Generated.v
→ {"method":"petanque/start","params":{"uri":"file:///abs/Generated.v","thm":"foo"}}
← {"result":{"st":1,"proof_finished":false,"feedback":[]}}
// 2. read the goal state
→ {"method":"petanque/goals","params":{"st":1}}
← {"result":{"goals":[{"hyps":[...],"ty":"..."}],"stack":[],"shelf":[],"given_up":[]}}
// 3. run a tactic → new immutable state handle
→ {"method":"petanque/run","params":{"st":1,"tac":"induction n."}}
← {"result":{"st":2,"proof_finished":false,"feedback":[]}}
// 4. keep going; proof_finished:true means QED-able. Backtrack = reuse an older st.
```
SerAPI equivalent: `(Add () "Lemma foo ... . Proof.")` → note the sid → `(Exec sid)` →
`(Query ((sid sid)) Goals)` → `(Add ((ontop sid)) "induction n.")` → `(Exec sid2)` …;
`(Cancel (sid2))` to backtrack.

**(3) Audit axioms + kernel-recheck:**
```
Print Assumptions foo.        (* expect: "Closed under the global context" or a whitelisted set *)
```
```bash
rocqchk -R . -o -silent Gen.Generated   # exit 0 required; -o prints the verified context
```
Then source-scan for `Admitted|admit|give_up|Axiom|Parameter|Hypothesis|Variable|
Conjecture|Unset .* Checking|bypass_check|-type-in-type|-impredicative-set`.

**(4) Run CoqHammer:**
```coq
From Hammer Require Import Hammer Tactics.
Set Hammer ATPLimit 10.
Lemma foo : P. Proof. hammer. Qed.        (* prints a reconstruction tactic to substitute *)
(* pure/no-ATP fallback: *) Proof. hauto use: lem1, lem2 unfold: def1. Qed.
```

**(5) Search premises:**
```coq
Search (_ + _ = _ + _) inside Nat.       (* pattern + module scope *)
SearchPattern (?n <= ?n).
SearchRewrite (_ * (_ + _)).
Search is:Lemma "assoc" -concl:(_ = _).  (* kind + substring + exclusion filters *)
```
Programmatic: `petanque/premises {st}` for the full visible-object list at a state.

---

## Integration summary for Theoremata

| Lean backend piece | Rocq equivalent |
|---|---|
| Warm Lean REPL step loop | **coq-lsp `proof/goals` + Petanque (`start`/`run`/`goals`)**; SerAPI `Add`/`Exec`/`Query Goals` fallback |
| `#print axioms` | **`Print Assumptions <thm>.`** ("Closed under the global context" = clean) |
| LeanParanoia kernel replay | **`rocqchk`/`rocq check -o -silent`** on the `.vo` (independent trusted-base re-check, exit 0) |
| Soundness scan | scan for `Admitted`/`admit`/`Axiom`/`Parameter`/`Variable`/`Unset * Checking` |
| Lake workspace scaffold | **`_CoqProject` + `coqc`/`rocq makefile`** (or dune) |
| mathlib retrieval | **MathComp + stdlib** via `Search`/`SearchPattern` + `petanque/premises` |
| Sledgehammer-class automation | **CoqHammer** (`sauto`/`hauto` always; `hammer`+ATPs in Docker) |
| Proof-state dedup / search pruning | **`petanque/state/hash` + `state/eq` (kind:Goals)**; SerAPI reuse-sid branching |
| Compile via the driver | **coq-lsp `coq/saveVo`** / `fcc`; or `rocq compile` directly |

### Sources read cover-to-cover (this expansion)
Refman chapters: `vernacular-commands`, `practical-tools/coq-commands`,
`practical-tools/utilities`, `proof-engine/ltac2`, `language/core/assumptions`,
`proofs/writing-proofs/proof-mode`, `proof-engine/ssreflect-proof-language`.
coq-lsp `etc/doc/PROTOCOL.md` (full, incl. all Petanque methods) + README.
coq-serapi README (sertop protocol). CoqHammer site (full). MathComp
GitHub + site. Install page, using-opam, docker-rocq.

**Deliberately skipped as out-of-scope for integration** (noted per instructions):
tutorial/marketing pages (Tour of Rocq, Why Rocq, books/exercises prose), the full
tactic *catalogue* beyond automation/ssreflect (e.g. detailed `ring`/`field`/`lia`
internals — names captured in B.5, deep theory omitted), governance/community/jobs
pages, the OCaml plugin API (not needed to drive Rocq externally), and RocqIDE/editor
UI docs. These add no build-ready integration detail.
