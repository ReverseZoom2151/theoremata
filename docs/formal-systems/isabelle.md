# Isabelle as a Theoremata Formal-System Backend — Integration Reference

Build-ready integration notes for adding the **Isabelle** proof assistant as a parallel
formal-system backend alongside the existing Lean integration. Release studied:
**Isabelle2025-2 (January 2026)**.

Sources: official docs at `isabelle.in.tum.de` (Munich master) read via the browsable HTML
manuals on the Cambridge mirror `www.cl.cam.ac.uk/research/hvg/Isabelle/dist/library/Doc/…`
(the Munich host intermittently reset TCP connections during research; both serve identical
content), cross-checked against the Mercurial source at `isabelle.in.tum.de/repos/isabelle/` and
Wenzel's Sketis notes. Every non-obvious fact is cited to a `Doc/<Manual>/<Chapter>.html` file or
a `/doc/<name>.pdf`.

**Mapping to our Lean stack (one line each):**
- warm Lean REPL → **Isabelle Server** (TCP, JSON, async tasks) + `isabelle console`
- `#print axioms` + kernel re-check → `thm_oracles` + `thm_deps` + proof-terms + clean `isabelle build`
- Lake workspace + build → session `ROOT`/`ROOTS` files + `isabelle build` + heap images
- mathlib retrieval → `Main`/HOL + AFP + `find_theorems`/`find_consts`/Find_Facts
- (no Lean-core analogue) → **Sledgehammer** — the standout reason to add Isabelle

---

## 0. Full website sitemap

Root `https://isabelle.in.tum.de/` (mirrored `.uk` Cambridge, `.au` Sydney, `.us` Potsdam NY).
Maintainers: Nipkow, Paulson, Wenzel, Klein, Haftmann, Weber, Hölzl.

| Section | URL | Purpose |
|---|---|---|
| Home / overview | `/index.html` | What Isabelle is; current release; hardware reqs; support links |
| Download apps | linked from home | Linux Intel/ARM, Windows `.exe`, macOS `.tar.gz` bundles |
| Installation | `/installation.html` | Per-platform install; **Docker** headless image; Windows/Cygwin & Defender notes; macOS notarization |
| Distribution archive | `/dist/` | All release files + SHA256; `contrib/` components (JDK 21, Poly/ML 5.9.2, E 3.2, cvc5 1.2.0, SPASS, Scala 3.3.4, Cygwin, jEdit, Solr 9.9, …) |
| Documentation index | `/documentation.html` | Master list of tutorials + reference manuals |
| Doc PDFs | `/doc/<name>.pdf` | e.g. `/doc/system.pdf`, `/doc/sledgehammer.pdf`, `/doc/isar-ref.pdf`, `/doc/implementation.pdf` |
| Browsable HTML library | `/dist/library/` (mirror `.../hvg/Isabelle/dist/library/Doc/`) | HTML of every manual, chapter-by-chapter |
| NEWS / changelog | `/doc/NEWS.html` | Cumulative release notes |
| Mailing lists | linked from home | `isabelle-users@cl.cam.ac.uk`, `isabelle-dev@in.tum.de` (archives linked) |
| Zulip / Stack Overflow | linked from home | Real-time chat / Q&A |
| Mercurial repo | `/repos/isabelle/` | Source history; dev snapshots |
| AFP | `https://www.isa-afp.org/` | Archive of Formal Proofs — large refereed corpus (§6) |
| Past releases | `/website-Isabelle<VER>/` | Frozen per-version sites + docs |

### Documentation index — full doc tree (`documentation.html`)

**Tutorials:** `prog-prove` (Programming and Proving in Isabelle/HOL), `locales`, `classes`,
`datatypes`, `functions`, `corec`, `codegen`, `nitpick` (counterexample finder), **`sledgehammer`**
(§5), `eisbach` (proof-method language), `sugar` (LaTeX).

**Reference manuals** (integration-critical in bold): `main` (*What's in Main*, §6),
**`isar-ref`** (*Isabelle/Isar Reference*, §2/§3/§6), **`implementation`** (*Isar Implementation*,
ML/kernel, §3), **`system`** (*System Manual*: server/build/tools, §2/§4/§7), `jedit`.

**Old manuals:** `tutorial`, `intro`, `logics`, `logics-ZF`.
**Object logics/libraries:** HOL (primary), ZF, FOL, CCL, LCF, FOLP, Sequents, CTT, Cube, Pure.

**Browsable-HTML chapters that matter (under `Doc/`):**
- System: `System/Sessions.html`, `System/Server.html`, `System/Environment.html`,
  `System/Presentation.html`, `System/Scala.html`, `System/Base.html`, `System/Misc.html`,
  `System/Phabricator.html`
- Isar_Ref: `Isar_Ref/Outer_Syntax.html`, `Isar_Ref/Spec.html`, `Isar_Ref/Proof.html`,
  `Isar_Ref/Proof_Script.html`, `Isar_Ref/Generic.html`, `Isar_Ref/HOL_Specific.html`,
  `Isar_Ref/Inner_Syntax.html`, `Isar_Ref/Quick_Reference.html`
- Implementation: `Implementation/Logic.html`, `Implementation/Tactic.html`,
  `Implementation/Proof.html`, `Implementation/ML.html`, `Implementation/Prelim.html`

---

## 1. Documentation sitemap (task dimension 1)

Covered by §0. The four deep-read documents: **`system`** (server/build/tools), **`isar-ref`**
(user commands: theory structure, proofs, `sorry`/`oops`, diagnostics, retrieval),
**`implementation`** (ML kernel/oracle/proof-term API), **`sledgehammer`** (automation). Sections
below cite into these chapter-by-chapter.

---

## 2. Programmatic interaction — the "warm REPL" analogue

Three headless entry points. For the agent loop the **Isabelle Server** is the primitive;
`isabelle console` is a line-oriented Isar fallback; `isabelle ML_process` is raw batch ML.

### 2a. The Isabelle Server (`System/Server.html`, System manual ch. 4)

**Command-line tools:**
```
isabelle server [OPTIONS]        # ensure a named server process is running
  -L FILE   logging on FILE
  -c        console interaction with specified server
  -l        list servers (alternative operation)
  -n NAME   explicit server name (default: isabelle)
  -p PORT   explicit server port
  -s        assume existing server, no implicit startup
  -x        exit specified server (alternative operation)

isabelle client [OPTIONS]        # thin interactive client = wrapper for `isabelle server -s -c`
  -n NAME / -p PORT              # uses ISABELLE_LINE_EDITOR if available
```
The server "listens on a regular TCP socket, using a line-oriented protocol of structured
messages."

**Connection / auth handshake:** on opening the socket the client's **first line MUST be its UUID
password** as a *short message* ("without length indication"). Server replies `OK` (+ Isabelle
version info) or silently disconnects an illegal attempt. Passwords are UUIDs generated in
Isabelle/Scala, stored in a per-user database with restricted file permissions.

**Message framing (`System/Server.html`, "Protocol messages"):** uniform format `name argument`:
- *name* = longest prefix of ASCII letters/digits/`_`/`.`; separator = longest run of ASCII
  blanks; *argument* = rest of the message.
- **Short message** = "a single line: a sequence of arbitrary bytes excluding CR (13) and LF (10),
  terminated by CR-LF or just LF."
- **Long message** = "starts with a single line consisting of decimal digits: these are
  interpreted as length of the subsequent block of arbitrary bytes." A final line-terminator after
  the length line "may be included here, but is not required." (Use this for very long theory
  arguments — the server reads the block in one go.)
- *argument* payload is `Unit` (empty), a **YXML** XML element, or a **JSON** value. "Messages in
  JSON format always fit on a single line, due to escaping of newline characters within string
  literals" — so JSON commands are always sendable as short messages, but the length-prefixed long
  form "can be read more efficiently as a single block" for big payloads.
- Encoding: "the content is always interpreted as plain text in terms of the UTF-8 encoding";
  line endings are invariant wrt UTF-8, so framing can be computed before/after encoding.

**Output markers / async task model:**
- Synchronous reply: **`OK`** or **`ERROR`** — strictly alternating with commands at toplevel.
- Async command instead returns `OK {"task": <uuid>}`, runs in background, later emits
  **`FINISHED {"task": …}`** or **`FAILED {"task": …}`**.
- Progress: **`NOTE {"task": …}`** at any time.

**Full server command set** (name → argument → results):

| Command | Argument | Result(s) |
|---|---|---|
| `help` | — | `OK [string]` (command names) |
| `echo` | `any` | `OK any` (identity, sync) |
| `shutdown` | — | `OK` (stops all sessions, closes socket) |
| `cancel` | `{task: uuid}` | `OK` (best-effort hint) |
| `session_build` | `session_build_args` | `OK task` → `FINISHED task ⊕ session_build_results` / `FAILED task ⊕ error_message ⊕ session_build_results`; `NOTE` progress |
| `session_start` | `session_build_args ⊕ {print_mode?: [string]}` | `OK task` → `FINISHED task ⊕ session_id ⊕ {tmp_dir: string}` / `FAILED task ⊕ error_message` |
| `session_stop` | `{session_id: uuid}` | `OK task` → `FINISHED task ⊕ session_stop_result` / `FAILED …` |
| `use_theories` | `use_theories_arguments` | `OK task` → `FINISHED use_theories_results` |
| `purge_theories` | `purge_theories_arguments` | `OK purge_theories_result` |

**Exact JSON shapes** (verbatim field names, `System/Server.html`):
```
type session_build_args = {
  session: string, preferences?: string, options?: [string],
  dirs?: [string], include_sessions: [string], verbose?: bool }

type session_build_result  = { session: string, ok: bool, return_code: int, timeout: bool, timing: timing }
type session_build_results = { ok: bool, return_code: int, sessions: [session_build_result] }
type session_stop_result   = { ok: bool, return_code: int }

type use_theories_arguments = {
  session_id: uuid,
  theories: [string],
  master_dir?: string,          // default: session tmp_dir
  pretty_margin?: double,       // default: 76
  unicode_symbols?: bool,
  export_pattern?: string,
  check_delay?: double,         // default: 0.5
  check_limit?: int,
  watchdog_timeout?: double,    // default: 600.0
  nodes_status_delay?: double } // default: -1.0

type export        = { name: string, base64: bool, body: string }
type node_results  = { status: node_status, messages: [message], exports: [export] }
type nodes_status  = [node ⊕ {status: node_status}]
type use_theories_results = { ok: bool, errors: [message], nodes: [node ⊕ node_results] }

type purge_theories_arguments = { session_id: uuid, theories: [string], master_dir?: string, all?: bool }
type purge_theories_result    = { purged: [string] }
```
`node_status` fields (per-theory proof-state health): `ok: bool` (= `failed = 0`),
`total/unprocessed/running/warned/failed/finished: int`, `canceled: bool`,
`consolidated: bool` (whole theory checked, final `end` reached), `percentage: int` (0–99, 100
when consolidated). A `use_theories` task "succeeds eventually, when all theories have status
*terminated* or *consolidated*." `message` values carry a `kind` (error/warning/writeln/…) plus
`message` text and a source `pos`.

**The five headless operations (the loop):**
1. **Start a session:** `session_start {"session": "HOL"}` → capture `session_id`, `tmp_dir`.
   (Server produces/consumes a session image on demand via `session_build`.)
2. **Load a generated theory:** write `Scratch.thy` into `tmp_dir` (or set `master_dir`), then
   `use_theories {"session_id": "…", "theories": ["Scratch"]}`.
3. **Get the proof state / errors:** on `FINISHED`, read `use_theories_results.ok`; if `false`,
   iterate over `errors[]` and per-node `messages[]` (`kind`+`message`+`pos`). `node_status` gives
   the granular finished/failed/warned counts. (There is **no** tactic-by-tactic "return the
   intermediate goal" server command — granularity is whole-theory; see caveat below.)
4. **Confirm oracle-free / kernel-valid:** include diagnostic commands in the theory text
   (`thm_oracles my_goal`, `thm_deps my_goal`) and read their `messages`; or gate offline via a
   clean `isabelle build` (§3/§4).
5. **Run Sledgehammer:** put `sledgehammer` (or `sledgehammer [provers=…, timeout=…]`) into the
   `.thy` at the goal and read the suggested `by (metis …)` back from `messages` (§5).

Cleanup: `purge_theories` (idempotent; repeated `use_theories` is idempotent too),
`session_stop {"session_id": …}`, `shutdown`. Sessions outlive client connections (start on one,
`use_theories` on another by known `session_id`); shared imports load once, persist until purged.

Canonical minimal example (manual + Sketis "The Isabelle Server", sketis.net 2018):
```
isabelle server &
isabelle client
session_start {"session": "HOL"}
use_theories {"session_id": ..., "theories": ["~~/src/HOL/ex/Seq"]}
session_stop {"session_id": ...}
shutdown
```

**Caveat vs. the Lean REPL:** the server operates at **theory-file granularity**, not per-tactic.
There is no command to submit one tactic and receive the resulting subgoal. Fine-grained stepping
is a PIDE concern (what Isabelle/jEdit and Isabelle/VSCode drive over YXML). Practical agent
pattern: **generate a full `.thy`, submit, parse `errors`/`messages`**; use `sorry` placeholders +
`thm_oracles` to probe partial proofs.

### 2b. `isabelle console` / `isabelle ML_process` (`System/Environment.html`)
```
isabelle console [OPTIONS]       # interactive Isar + ML toplevel over a session image (closest to a REPL)
  -d DIR   include session directory        -m MODE  add print mode
  -i NAME  include session in theory name-space   -n  no build of session image on startup
  -l NAME  logic session name (default HOL)  -o OPT  override system option (NAME=VAL | NAME)
                                             -r  bootstrap from raw Poly/ML

isabelle ML_process [OPTIONS]    # batch ML evaluation (headless; call kernel/audit APIs directly)
  -e ML_EXPR   evaluate ML expression on startup   -l NAME  logic session (default ISABELLE_LOGIC=HOL)
  -f ML_FILE   evaluate ML file on startup         -m MODE / -o OPT / -d DIR
  -C DIR       change working directory            -r  redirect stderr to stdout
```
`isabelle console` is the line-oriented Isar loop that `isabelle client -c` / `isabelle server -c`
attach to. `isabelle ML_process` is for programmatic ML (e.g. running the §3 auditing functions).

### 2c. PIDE
PIDE ("Prover IDE") is the asynchronous document model behind jEdit/VSCode; native messages are
YXML (the server can carry YXML args too). For batch checking we do **not** need raw PIDE —
`session_start`/`use_theories` abstract it.

---

## 3. Soundness / verification gate — the `#print axioms` analogue

Isabelle is **LCF-style**: `thm` is an abstract datatype whose values can only be built by the
`Thm` kernel module, so any `thm` is valid by construction relative to axioms + oracles.
(`Implementation/Logic.html`.)

**Primitive kernel rules** (everything derives from these + axioms/oracles):
`Thm.assume: cterm -> thm`, `Thm.forall_intr`, `Thm.forall_elim`, `Thm.implies_intr: cterm -> thm -> thm`,
`Thm.implies_elim: thm -> thm -> thm`, etc.

**Sort hypotheses (hidden preconditions):** `Thm.extra_shyps: thm -> sort list` returns sort
constraints "not present within type variables of the statement" (dangling `s ⊢ φ`);
`Thm.strip_shyps: thm -> thm` discharges those witnessable from the type signature. A rigorous
gate should assert `Thm.extra_shyps` is empty (or expected) so no smuggled sort assumptions remain.

**Oracles — the escape hatch to audit:**
- Declared with `Thm.add_oracle: binding * ('a -> cterm) -> theory -> (string * ('a -> thm)) * theory`.
  "An oracle is a function that produces axioms on the fly … the inference kernel records oracle
  invocations within derivations of theorems by a unique tag."
- Isar-level: the `oracle name = "ML-text"` command (`Isar_Ref/Spec.html`) turns an ML expression
  into an oracle function bound to a global identifier.
- `sorry` (Isar, `Proof.html`) is exactly such an oracle (`Pure.skip_proof`): it "pretend[s] to
  solve the pending claim … only works in interactive development, or if the `quick_and_dirty`
  attribute is enabled", and marks the derivation as **tainted**, inspectable via `thm_oracles`.
  The `smt` proof method can likewise leave an oracle tag when run in oracle mode.
- `oops` (`Proof.html`) is different: it "discontinues the current proof attempt … goes back right
  to the theory level" and **returns no `thm`** — it can never contaminate downstream results.

**Auditing a finished theorem (this is our gate):**
- **`thm_oracles thms`** (Isar, `Isar_Ref/Spec.html`): "displays all oracles used in the internal
  derivation of the given theorems; **this covers the full graph of transitive dependencies**." →
  Whitelist check: assert the reported oracle set is empty (or ⊆ allowlist). Closest analogue to
  Lean's `#print axioms`.
- **`thm_deps thms`** (Isar, `Isar_Ref/Outer_Syntax.html`): prints **immediate** theorem
  dependencies (facts used directly, not the deep graph) — provenance, not soundness by itself.
- **`print_axioms`**: lists the axioms of the current theory (whitelist the object-logic axiom
  base, e.g. HOL's).
- **ML hard gate** (`Implementation/Logic.html`):
  - `Thm_Deps.all_oracles: thm list -> Proofterm.oracle list` — recovers all oracles in the
    derivations. Gate: assert `Thm_Deps.all_oracles [target] = []`.
  - `Thm.proof_of: thm -> proof` and `Thm.proof_body_of: thm -> proof_body`. The `proof_body`
    holds "a digest about oracles and promises occurring in the original proof … without the full
    overhead of explicit proof terms." The digest "only covers the directly visible part"; to get
    the full nested-theorem graph traverse via `Proofterm.fold_body_thms`.
  - **Recording level** `Proofterm.proofs: int Unsynchronized.ref` — **0** = oracle names only,
    **1** = names + propositions, **2** = full proof terms. (Officially named theorems are recorded
    regardless.) `Proofterm.reconstruct_proof` / `Proofterm.expand_proof` build explicit terms;
    proof-term constructors: `Abst`/`AbsP` (⋀/⟹ intro), `%`/`%%` (elim), `PBound`, `Hyp`, `PAxm`,
    `Oracle`, `PThm`, `MinProof`.
  - **Warning:** `Thm.proof_of` "involves a full join of internal futures that fulfill pending
    proof promises" — i.e. it forces Isabelle's parallel/deferred proofs. That is exactly what we
    want in a gate (it makes background proofs actually complete and be checked), but it is
    expensive; do it once at the gate, not in a hot loop.

**Kernel re-check:** because a `thm` already passed the kernel, the standard re-check is to
**rebuild the session from sources** with a clean `isabelle build` (§4) in a fresh environment —
this re-runs every primitive inference. For an independent checker, record proof terms
(`Proofterm.proofs := 2`) and export them.

**Recommended Isabelle gate (mirror of our Lean gate):**
1. Build the generated session clean: `isabelle build -c -o quick_and_dirty=false -d <dir> <S>`.
   Reject if build return code ≠ 0.
2. Assert `thm_oracles <target>` is empty/⊆whitelist (no `Pure.skip_proof`, no external oracle).
3. Optionally set `Proofterm.proofs := 1`, assert `Thm_Deps.all_oracles [target] = []` and
   `Thm.extra_shyps target = []`; use `thm_deps` for provenance.
4. Reject any build where `quick_and_dirty` was enabled (it legalizes `sorry`).

---

## 4. Project / build system — the Lake analogue

### Theory file structure (`Isar_Ref/Spec.html`)
```
theory A imports B1 … Bn
  keywords <kw-decls>          (* optional: declare new outer-syntax keywords *)
  abbrevs  <abbrevs>           (* optional: syntactic-completion abbreviations *)
begin
  <body>
end
```
"`theory A imports B1 … Bn begin` starts a new theory A based on the merge of existing theories
B1 … Bn." Local scoping: `context c begin … end` / `context begin … end` blocks. Axioms:
`axiomatization c1 … cm where φ1 … φn`. A minimal generated theory:
```
theory Scratch
  imports Main
begin
lemma my_goal: "…"
  <proof>          (* e.g. by auto  |  proof … qed  |  sledgehammer then by (metis …) *)
end
```

### Session `ROOT` / `ROOTS` files (`System/Sessions.html`)
A **session** is the verifiable unit. `ROOT` uses Isar outer syntax:
```
chapter NAME                         (* optional grouping *)
session A = B +                      (* new session A with parent B; '+' = inherit parent image *)
  description "…"
  options [x = a, y = b, z]          (* session-specific system options *)
  sessions X Y                       (* make other sessions' theories importable, qualified *)
  directories dir1 dir2              (* extra dirs holding .thy files *)
  theories                           (* blocks of theory files, optional per-block options *)
    T1 T2
  document_files "root.tex"          (* LaTeX sources for document prep *)
  export_files (in ".") "*:**"       (* theory exports to filesystem *)
```
- Hierarchy roots at `Pure`, then object logics like `HOL`. `session A = B +` inherits B's heap.
- **`ROOTS`** = a catalog file: "any session root directory may refer recursively to further
  directories … by listing them line-by-line in a catalog file `ROOTS`" — used to organize large
  collections or make `-d` persistent (e.g. `$ISABELLE_HOME_USER/ROOTS`).
- **Theory qualification:** theory names are normally qualified by session (`B.A` = theory `A` of
  session `B`) "to ensure globally unique names in big session graphs"; a theory tagged `(global)`
  is taken literally.

### `isabelle build` (headless, `System/Sessions.html`)
```
isabelle build [OPTIONS] [SESSIONS ...]
  -a        select all sessions             -g NAME  select session group NAME
  -b        build heap images               -x NAME  exclude session NAME and descendants
  -c        clean build (rebuild)           -R       select requirement (ancestor) sessions
  -d DIR    include session directory       -l       list session source files
  -o OPT    override system option          -e       export files (per ROOT export_files)
            (NAME=VAL | NAME)               -P DIR   enable HTML/PDF presentation (":" = default)
  -j INT    max parallel jobs (default 1 local)
  -n        dry run (use existing databases)
  -v        verbose
```
"The overall return code [is] the status of the set of selected sessions" (0 = all ok, non-zero =
some session failed) — use this as the build gate. Heap/session images and logs live under
`$ISABELLE_HEAPS` (user) vs `$ISABELLE_HEAPS_SYSTEM` (system); cluster builds use PostgreSQL.
`isabelle mkroot` scaffolds a default `ROOT` (+ document skeleton) in a new directory.

### Heap / session images
A **heap image** is a snapshot of the ML process state (compiled theories), like compiled object
code. `-b` forces production; inner-hierarchy images are saved automatically when a dependent needs
them. The server's `session_start`/`session_build` produce/consume these on demand so interactive
checking starts warm (no reprocessing of `HOL` each time).

### Scaffold + build a single generated theory
1. `mkdir scratch && cd scratch`, write `ROOT` (`session Theoremata_Scratch = HOL +` … `theories Scratch`)
   and `Scratch.thy`.
2. Batch: `isabelle build -c -d . Theoremata_Scratch` → check return code + stdout messages.
   Or warm: server `session_start {"session":"HOL"}` then `use_theories`.
3. Exports (e.g. proof terms) via `export_files` + `isabelle build -e`, or `use_theories`
   `export_pattern`.

---

## 5. Automation — Sledgehammer (the standout feature)

Sledgehammer (`/doc/sledgehammer.pdf`, tutorial "Hammering Away") is a "hammer": from the current
goal it fires a battery of **external ATPs and SMT solvers**, then **reconstructs** a proof that
Isabelle's own kernel re-checks. No Lean-core analogue — a strong argument for adding Isabelle as
an automation backend.

**Invocation (in a proof state):**
- `sledgehammer` — run on the first subgoal (subcommand `run`, the default).
- `sledgehammer [k1 = v1, …, kn = vn] (facts_override) [subgoal_num]` — one-shot with overrides;
  for boolean options `= true` is optional (e.g. `sledgehammer [isar_proofs, timeout = 120]`).
- `sledgehammer_params [options]` — set/display persistent defaults ("also prints the list of all
  available options with their current value").
- Other subcommands (source `sledgehammer_commands.ML`): `supported_provers`, `unlearn`,
  `learn_isar`, `learn_prover`, `relearn_isar`, `relearn_prover`, `refresh_tptp`.
- Companions: `try` (launches Solve_Direct + Quickcheck + Nitpick + Sledgehammer + Try0 at once),
  `try0` (tries the standard proof methods).

**Fact-override syntax** (after the command): `(f1 f2)` = bypass the relevance filter, use exactly
these facts; `(add: f1)` = force-include as highly relevant; `(del: f2)` = force-exclude;
`(add: f1 del: f2)`.

**Provers used:** ATPs **E, SPASS, Vampire, Zipperposition**; SMT **cvc5, Z3, veriT**; plus
internal proof-method "provers" (`metis`, `simp`, `auto`, `blast`, `fastforce`, `force`, `meson`,
`linarith`, `presburger`, `argo`, `order`, `algebra`, `satx`); remote provers via **SystemOnTPTP**
(`remote_…`). Select with `provers = "e vampire cvc5 z3 …"` (first prover is the one Auto
Sledgehammer uses).

**Proof reconstruction / output:** Sledgehammer does **not** trust the ATP. It prints a suggested
one-line Isar proof — **`Try this: by (metis …)`** (first-order resolution, a genuine kernel
derivation, no oracle) or `by (smt (verit) …)` / `by (smt (z3) …)` / `by (meson …)` /
`by (simp add: …)`. It **preplays** candidates (running them, with timing) and suggests the
fastest; with `isar_proofs` it can emit a structured multi-step Isar proof.

**Concrete default options** (source `sledgehammer_commands.ML` `default_default_params` +
manual §7): `timeout = 30` (seconds, soft limit), `slices = 24 × cores` (parallel time slices;
`dont_slice`/`slices = 1` disables), `max_facts = smart` (prover-dependent, ~0–1000),
`fact_filter = smart` (mepo/mash), `fact_thresholds = 0.45 0.85`, `learn = true` (MaSh),
`minimize = true`, `preplay_timeout = 1`, `max_proofs = 4`, `isar_proofs = smart`, `try0 = true`,
`smt_proofs = true`, `instantiate = smart`, `compress = smart`, `type_enc = smart`,
`strict = false`, `induction_rules = smart`, `abduce = 0`, `falsify = false`, `verbose/debug =
false`.

**Relevance filtering (premise selection):** two filters choose which library facts to send —
**MePo** (Meng–Paulson, syntactic symbol overlap) and **MaSh** (Machine Learning for Sledgehammer,
learned predictor; trained via `learn_isar`/`learn_prover`). Reusable as a retrieval signal (§6).

**Falsifiers / pre-filters:** **Nitpick** (`nitpick`) and **quickcheck** (`quickcheck`) cheaply
*refute* a goal before wasting prover time — natural falsifier hooks for our loop.

**Batch/benchmark over a corpus:** `isabelle mirabelle -A "sledgehammer[provers = e, timeout = 30]"
-d '$AFP' -O output <sessions>` runs a Sledgehammer action on all subgoals across theories — useful
for large-scale agent evaluation.

**Verdict:** yes — expose Sledgehammer. It is the single biggest reason to run Isabelle in parallel
with Lean; its `metis` reconstructions are kernel-checked and pass the §3 gate (watch `smt` oracle
mode). Drive it headlessly by injecting `sledgehammer [...]` into the theory and reading
`Try this:` out of the `use_theories` messages.

---

## 6. Library / retrieval corpus

**Base:** `Main` (`main` manual, *What's in Main*) is the standard HOL entry import; HOL is the
primary object logic where nearly all mathematics lives (others: ZF, FOL, CCL, LCF, FOLP,
Sequents, CTT, Cube).

**AFP — Archive of Formal Proofs** (`https://www.isa-afp.org/`): a refereed "scientific journal"
of Isabelle developments, continuously tested against the current release; BSD/LGPL; Mercurial on
Heptapod. Install/use (`isa-afp.org/help/`): `isabelle components -u /path/to/afp/thys` (registers
the whole AFP; Isabelle2021-1+), then `imports "EntryName.Some_Theory"`. Entries citable (ISSN
2150-914x); stable pages `isa-afp.org/entries/<Name>.html`.

**Premise search / retrieval tooling** (`Isar_Ref/Outer_Syntax.html`):
- `find_theorems (nat? with_dups?) <criteria>` — AND-combined criteria: `name: p` (wildcard `*` on
  qualified name), `intro`/`elim`/`dest` (matches current goal as that rule), `solves` (would close
  the goal), `simp: t` (rewrite whose LHS matches `t`), or a bare term pattern (dummies `_`,
  schematics, type constraints). Prefix `-` negates; empty criteria = *all* facts; default limit
  **40**; `with_dups` keeps duplicates.
- `find_consts <criteria>` — by type: `strict: ty` (exact type-pattern), bare `ty` (also matches
  subtypes), `name: p`, `-` negation.
- `thm_deps` / `unused_thms` — dependency provenance / dead-lemma detection.
- **Sledgehammer MePo/MaSh** (§5) — reusable learned/relevance premise selector.
- **Find_Facts** (`System/Presentation.html` §3.4; arXiv:2204.14191) — scalable full-text search
  over all built session content, backed by **Apache Solr** (bundled; `SOLR_JARS`):
  ```
  isabelle find_facts_index HOL          # build a Solr index from a build's session info
  isabelle find_facts_index_build        # pack index as reusable component (.db + etc/settings)
  isabelle find_facts_server [-p PORT] [-o OPT] [-v] [-d]   # web app + JSON REST search endpoints
  ```
  The Scala HTTP server exposes **REST endpoints returning JSON** (front-end in Elm) — directly
  usable as a retrieval microservice. Data in `$ISABELLE_HOME_USER/find_facts/`; DB name via option
  `find_facts_database_name`. Example: `isabelle find_facts_server -p 8080
  -o find_facts_database_name=isabelle` then query `…/find_facts#search?q=Hilbert`.

For our retrieval component: `find_theorems`/`find_consts` are the interactive, goal-aware
in-session primitives (call over the server session); **Find_Facts** is the corpus-scale JSON-API
premise index; MaSh is the learned ranker.

---

## 7. Install / toolchain

**Bundles** (`installation.html`): self-contained per-OS packages bundling **JDK 21**, **Poly/ML
5.9.2** (the ML runtime the kernel runs on), **Scala 3.3.4**, jEdit, and all `contrib/` provers —
no separate toolchain needed.
- Linux: `Isabelle2025-2_linux.tar.gz` / `_linux_arm.tar.gz`; run `Isabelle2025-2/bin/isabelle`.
- macOS: `Isabelle2025-2_macos.tar.gz` (Intel or Apple Silicon; unsigned → manual first-launch).
- **Windows (10/11): `Isabelle2025-2.exe`** self-extractor, ships its own **Cygwin**
  (`Cygwin-Terminal` runs `isabelle` CLI as on Unix). **Critical:** "Windows 10 Defender may
  prevent external provers from working (e.g. sledgehammer or the smt method) — exclude the whole
  Isabelle application directory from Virus & threat protection." App is unsigned.
- **Headless / CI: Docker** `makarius/isabelle:Isabelle2025-2` (Ubuntu 22.04; ARM variant
  `…_ARM`); `docker run makarius/isabelle:Isabelle2025-2` gives the `isabelle` wrapper, no GUI —
  cleanest way to run `isabelle build`/`isabelle server` in CI, avoiding Windows/Cygwin+Defender
  friction.
- Hardware: small 4 GB/2 cores; medium 8 GB/4; large 16 GB/8; XL 64 GB/16.

**Windows/WSL note for Theoremata** (dev env is Windows, no python3/lean/lake): prefer the Docker
image or WSL2 for the Isabelle backend — the native `.exe` works but Cygwin indirection + the
Defender exclusion make headless server/build automation fiddly. Server + `isabelle build` are
identical across platforms inside the container.

---

## Deliberately skipped as irrelevant to backend integration

- **Document preparation** (`Isar_Ref/Document_Preparation.html`, `Sugar`, LaTeX demo styles
  `Demo_*`, `document_files`): PDF/LaTeX output is not needed for a headless proving backend.
- **Isabelle/jEdit and VSCode GUI** (`jedit` manual): we drive PIDE headlessly via the server, not
  the IDE.
- **Non-HOL object logics** (ZF, FOL, CCL, LCF, FOLP, Sequents, CTT, Cube): integration targets
  HOL/`Main`; noted for completeness only.
- **Eisbach, code generation, (co)datatype/function/locale/class tutorials**: user-authoring
  features, orthogonal to submit-and-check + audit + hammer. `codegen` could matter later for
  executable extraction, but not for the proving loop.
- **`isabelle phabricator`, build-cluster/PostgreSQL, mailing-list/community pages**: ops/infra,
  not the integration surface.

## Integration recommendation (summary)

- **Interaction:** run `isabelle server`; `session_start {"session":"HOL"}` + `use_theories` over a
  generated `Scratch.thy`; parse `use_theories_results.{ok, errors, nodes[].messages}`. Whole-theory
  granularity, not tactic-stepping — generate full proofs, submit, read messages. Framing: JSON on
  one line, or length-prefixed long messages for big theory text.
- **Soundness gate:** clean `isabelle build -c -o quick_and_dirty=false` (return code 0), then
  `thm_oracles <lemma>` empty/whitelisted (full transitive oracle graph), optionally
  `Thm_Deps.all_oracles = []` + `Thm.extra_shyps = []` with proof recording. Faithful `#print
  axioms` analogue.
- **Automation:** expose **Sledgehammer** (`timeout=30`, `slices=24×cores` defaults); its `metis`
  reconstructions are kernel-checked and pass the gate; add `nitpick`/`quickcheck` as falsifiers;
  `isabelle mirabelle` for batch eval.
- **Retrieval:** `find_theorems`/`find_consts` in-session + **Find_Facts** JSON REST + AFP corpus.
- **Toolchain:** target the **Docker image** for headless/CI; on Windows exclude the Isabelle dir
  from Defender or run under WSL2/Docker.
