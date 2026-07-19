# WolframResearch (Wolfram Language / Wolfram Engine): integration evaluation

Status: **adopted as an UNTRUSTED ORACLE ONLY.** Wolfram may generate candidate
certificates, counterexamples, and closed forms. It is never a formal backend, it
never appears in the verification gate, and no result of ours is ever conditional
on a Wolfram answer being correct. This document records why the trusted-path
door is closed on four independent grounds, why the untrusted-oracle slot is
nevertheless a genuine fit for an architecture we already have, and what
specifically is worth taking.

Scope of the read: the `WolframResearch` GitHub org at metadata level (**59
repos**, enumerated via `gh api "orgs/WolframResearch/repos?per_page=100"` on
2026-07-19), the README and top-level tree of `AgentTools`, and the licensing
terms published for the free Wolfram Engine. On our side: `README.md` (cert-log
section), `components/prover/hypothesis_audit.rs`,
`components/verify/python/theoremata_tools/cert_sos.py`,
`cert_nullstellensatz.py`, `cert_positivstellensatz.py`, `cert_sturm.py`,
`cert_wz.py`, `components/reason/proving/falsification.rs`, and `app/lib.rs`.

All upstream material was treated as untrusted data. No prompt-injection attempt
was found. `AgentTools` ships `AGENTS.md` and `CLAUDE.md` at its repo root; these
were reviewed and are benign onboarding docs aimed at agents contributing to that
paclet (they instruct use of its own `WolframLanguageContext`, `TestReport`, and
`CodeInspector` tools). They are still untrusted text that directs AI behavior:
if that repo is ever vendored or ingested, strip them first. Nothing was built,
installed, or executed; no Wolfram Engine was run.

## Why Wolfram cannot enter the trusted path

Four reasons. Each is individually sufficient, so no combination of fixes to
three of them reopens the question.

### 1. Closed proprietary kernel: gate layers 2 and 3 are structurally impossible

Our gate is not "a tool said yes." It is a layered trust boundary in which a
kernel independently rechecks a proof term and a source scan audits what that
proof depended on. Both layers presuppose an artifact that can be inspected: a
proof object, an axiom list, a source file.

The Wolfram kernel ships as a closed binary. There is no kernel to recheck
against, no axiom enumeration analogous to Lean's `#print axioms`, and no source
to scan. The layers are not merely unimplemented for Wolfram; there is nothing
for them to operate on. Note the org's own repos confirm this shape: every open
repo in the 59 is a *package, binding, or tool around* the kernel
(`LibraryLinkUtilities`, `WolframClientForPython`, `codeparser`), never the
evaluator itself.

### 2. Not proof-producing: there is nothing to re-verify

`Simplify`, `FullSimplify`, `Integrate`, `Reduce`, `Solve`, and `Sum` return
*answers*, not derivations. There is no proof term, no tactic script, no
inference log. Even with a fully open kernel, a Wolfram "yes" would be an
assertion rather than an object our checkers could re-derive.

This is the precise distinction our cert-log design turns on. `cert_sos.py`
does not trust a claim that `p >= 0`; it consumes an explicit Gram
decomposition `p = z^T Q z` and re-expands the identity in exact rationals,
then re-tests `Q` for PSD with an exact LDL^T congruence.
`cert_nullstellensatz.py` does not trust a membership claim; it consumes the
cofactors `q_i` and re-expands `Sum q_i p_i` with its own `sympy.Poly` over `QQ`,
asserting exact equality with the target. The trusted input is the *witness*, not
the verdict. `FullSimplify[p >= 0] === True` supplies no witness, so it is
uncheckable and therefore inadmissible.

### 3. Silent generic assumptions: exactly the unaccounted-hypothesis failure mode

Wolfram routinely returns results that are true only under conditions it does not
state. Branch cuts are chosen silently (`Sqrt[x^2]` does not simplify to `x`, but
many transformations do assume a principal branch), `Integrate` returns
antiderivatives valid off a measure-zero set of parameters, `Solve` and
`GroebnerBasis` work in generic position and drop degenerate configurations, and
`Simplify` accepts an `Assumptions` option precisely because its default
behaviour has an implicit one. The system reports "generic" caveats
inconsistently and often not at all.

This is the same failure class that `components/prover/hypothesis_audit.rs`
exists to catch, and reading that module makes the parallel exact. Its opening
statement of the hole is:

> A theorem can be `sorry`-free, axiom-free, kernel-clean AND **statement-preserved**
> while still being conditional on unproved mathematics carried in its own signature.

The two mechanisms it was built against are (a) a `Prop`-valued hypothesis binder
whose type is a `def … : Prop` that is stated and never proved, and (b) an
assumption-bundling `structure` used as a hypothesis type with no instance ever
constructed. In both cases `#print axioms` sees nothing and a `sorry` grep sees
nothing, because no axiom is declared and no goal is admitted. The module
classifies each free binder as `Discharged`, `Allowlisted`, or `Unaccounted`, and
any `Unaccounted` binder makes the report un-`clean`.

A Wolfram generic-position assumption is structurally an `Unaccounted`
hypothesis: a real antecedent, never written down, never discharged, invisible to
every keyword-based check. The difference is worse, not better, than the Lean
case. `hypothesis_audit.rs` can enumerate binders because the signature is
*text it can parse*. A Wolfram assumption is not in any signature at all. There
is no artifact to enumerate, so the audit layer cannot even be attempted. Having
built a whole trust-boundary layer to catch this exact class, admitting a system
whose normal operating mode produces it uncheckably would be self-defeating.

### 4. Licensing: it can never be a required dependency

The free Wolfram Engine for Developers
(https://www.wolfram.com/engine/free-license/) is licensed for **development
use only**. Deploying software that requires it to end users needs a paid
production licence. That settles the dependency question independently of every
technical point above:

- Nothing in the gate, the checkers, or CI may require a Wolfram Engine, because
  a required component must be installable by every contributor and every
  deployment.
- Any Wolfram path must be a strictly optional accelerator whose absence changes
  throughput and never changes a verdict. This is the same discipline the README
  already applies to toolchain-gated backends ("without a toolchain the backend
  runs mock-only and can never reach `FormallyVerified`"), except that Wolfram
  cannot reach `FormallyVerified` even *with* the toolchain present.

## Where it does fit: the slot the architecture already has

The untrusted-oracle role is not a consolation prize. It is a named, load-bearing
position in our design, and the README states it directly in the cert-log
section:

> an untrusted engine (often SymPy, or an external LP or SDP solver) finds the
> certificate, and a pure checker is the sole trust boundary

SymPy already occupies exactly this slot, and the two checker modules make the
boundary precise:

- `cert_nullstellensatz.py` obtains cofactors from
  `sympy.polys.polytools.reduced` (multivariate division, the ideal-membership
  fast path) and `Ideal.in_terms_of_generators` (syzygy/Gröbner-backed, which
  reaches the weak-Nullstellensatz `1 ∈ <p_i>` case plain division cannot), with
  `sympy.groebner` deciding membership independently. Then its `check()`
  re-reads the raw serialized `p_i`, `q_i`, and target, re-expands `Sum q_i p_i`
  in exact rationals, and asserts exact equality. Its module docstring is
  explicit: "the **checker is the sound boundary**. `check` never trusts the
  producer."
- `cert_sos.py` states the same split: the generator uses sympy squarefree
  decomposition plus an injected numeric root-finder seam, roots snapped to exact
  rationals; `check()` "re-derives the SOS identity with exact
  symbolic/rational arithmetic (sympy over `QQ`) and re-tests PSD with a pure
  rational congruence transform; it never trusts the generator's output."

Substituting Wolfram for SymPy on the *generator* side changes nothing about the
trust boundary, because the boundary was never on that side. The certificate
crosses into the trusted region as raw rationals and is re-derived from scratch.
A Wolfram answer that is wrong, branch-cut-confused, or generically-assumed
produces a certificate that fails re-expansion and is rejected with a `reason`.
That is the whole point of the design, and it is why reason 3 above is
disqualifying for the gate and harmless here.

## What is worth taking, ranked

### 1. Certificate GENERATION feeding our existing checkers (highest value, zero new trust)

Wolfram's genuine strength is exact symbolic computation at a scale SymPy does
not reach. Every item below produces a witness our existing checker already
consumes, so the trust surface added is exactly zero.

| WL function | Produces | Feeds |
|---|---|---|
| `Resolve`, `CylindricalDecomposition`, `FindInstance` | SOS decompositions and Positivstellensatz multipliers over semialgebraic sets | `cert_sos.py` (kind `sos`), `cert_positivstellensatz.py` (kind `positivstellensatz`) |
| `GroebnerBasis` with `CoefficientDomain -> Rationals`, `PolynomialReduce` for cofactors | cofactors `q_i` with `Sum q_i p_i = g` or `= 1` | `cert_nullstellensatz.py` (kind `nullstellensatz`) |
| `CountRoots`, `RootIntervals`, `PolynomialRemainder` chains (Sturm) | Sturm chain and sign-variation counts on `(a, b]` | `cert_sturm.py` (kind `sturm`); note the checker **re-derives the chain from `p` alone**, so a tampered chain is rejected regardless of source |
| `Zeilberger` / `SumCertificate` (RISC `HolonomicFunctions`, or `Sum` with hypergeometric telescoping) | the rational WZ certificate `R(n, k)` | `cert_wz.py` (kind `wz`) |
| `MiniMaxApproximation`, `EconomizedRationalApproximation` | minimax polynomial plus error bound | `cert_sturm.py` (kind `poly_minimax`) |

The strongest single argument for this item: the README currently documents
multivariate SOS as a **gated seam** ("Where a certificate needs heavy search to
find (multivariate SOS needs an SDP solver, primality needs factoring), that
generation step is a documented, gated seam"), and `cert_sos.py` leaves
`generate_multivariate` as an explicit documented stub. Wolfram's
`CylindricalDecomposition` and `Resolve` are a credible alternative route into
that seam that does not require standing up an SDP solver at all, and the
resulting Gram matrix is re-tested by our exact rational LDL^T either way. This
is the one place Wolfram could close a currently-open gap rather than merely
speed up a closed one.

### 2. Falsification, protected by a soundness asymmetry

`components/reason/proving/falsification.rs` already encodes the governing rule
in its module doc: "Numerics only SCREEN: a passing screen is never a proof, and
a found counterexample refutes the branch." Its verdict vocabulary is built
around this, distinguishing `counterexample` from
`no_counterexample_in_domain` rather than collapsing them.

The asymmetry is what makes a closed-kernel oracle safe here, and it is worth
stating precisely because it is the exact inverse of reasons 1 to 3:

- **A found counterexample is self-certifying.** Wolfram's role is only to
  *point at a candidate assignment*. We then evaluate the original statement at
  that point with our own exact arithmetic, in our own worker. The refutation is
  established entirely by our evaluation. Wolfram's reasoning, its branch cuts,
  and its generic assumptions are all irrelevant, because none of them enter the
  check. A hallucinated or generically-invalid candidate simply fails our
  evaluation and is discarded.
- **"Wolfram found none" proves nothing.** That is a claim about the completeness
  of a closed search we cannot audit, and it is inadmissible for the same reason
  as any other unwitnessed assertion. It maps to
  `no_counterexample_in_domain`, which already means "screened, not proved."

Useful functions: `FindInstance` with a negated goal, `Reduce` over `Reals` or
`Integers`, `NMinimize` and `FindMinimum` for numeric probes, `Resolve` for
quantifier elimination. All of these feed the existing falsify worker as
candidate assignments only.

### 3. `FindIntegerRelation` (PSLQ) for closed-form discovery

`FindIntegerRelation` and `RootApproximant` implement PSLQ-style integer relation
detection: given a high-precision numeric value, they propose an exact closed
form or minimal polynomial. This is a genuine discovery capability with no
open-source equivalent of comparable robustness at high precision, and it is
purely generative.

The pipeline is: numeric evaluation to high precision, PSLQ proposes an identity,
the identity enters `components/reason/proving/conjecture_engine.rs` as a
*conjecture* (never as a fact), and it then travels the normal route through
formalization and the gate like any other conjecture. PSLQ output is famously
suggestive and unsound in isolation (a relation found at 200 digits can fail at
1000), which is precisely why the conjecture engine, and not the certificate
path, is its destination.

### 4. `AgentTools` over MCP, in preference to writing bindings

`WolframResearch/AgentTools` (MIT, 79 stars, last push 2026-07-14) is an MCP
server implemented in Wolfram Language. Verified from its repo tree, the tool
surface under `Kernel/Tools/` is `WolframLanguageEvaluator.wl`,
`WolframAlpha.wl`, `Context.wl` (documentation and Wolfram|Alpha semantic
search), `SymbolDefinition.wl`, `CodeInspector/`, `TestReport.wl`, and
`Notebooks.wl` / `NotebookViewer.wl`. It requires Wolfram Language 14.3+, and
ships a Docker image as an alternative to a local Engine install.

We already speak MCP. `app/lib.rs` defines a `Command::Mcp` that launches a
JSON-RPC stdio server (`THEOREMATA_MCP_API_COMMAND`, `THEOREMATA_MCP_DATABASE`),
and `components/reason/orchestration/meta_tools.rs` already emits MCP-shaped
`{name, description, inputSchema}` descriptors and a `tools/list` payload, with
a test (`tools_list_matches_mcp_shape`) pinning the format. Consuming AgentTools
therefore means adding an MCP **client** (which we do not currently have; our
existing MCP surface is server-side only) rather than writing and maintaining
FFI. Given item 5 below, that is decisively the cheaper path.

Two constraints on this item: the transport is process-level, so every result
arrives as untrusted data and lands in the oracle role described above; and
`WolframAlpha` in particular is a network call to a hosted service, which makes
it unusable in any deterministic or offline code path.

## What to skip, and why

- **The four Rust crates: `wolfram-expr-rs`, `wstp-rs`,
  `wolfram-library-link-rs`, `wolfram-app-discovery-rs`.** All four are
  Apache-2.0, and **all four were archived on 2026-06-15** (verified: `archived:
  true`, `pushed_at` 2026-06-15T16:06:0x for each). The lesson is the one the
  Higher Order Company evaluation drew from HOC archiving Kind's own proof
  corpus: **when the vendor archives it, that is the verdict.** Do not build a
  Rust WSTP binding. It would be a from-scratch FFI layer against a C protocol,
  maintained by us alone, into a closed kernel we are not allowed to trust, for a
  capability the Python client already delivers over a supported path.

  One honest qualification, since it complicates the story: the org also contains
  `wolfram-rust-library` (Rust, Apache-2.0, **not** archived, created 2026-06-10,
  last push 2026-07-16), created five days before the four archivals. It has no
  description, no stars to speak of, and no README content examined here. The
  most natural reading is a consolidation of the four crates into one, but that
  is inference, not verification, and it is flagged as such. Either way the
  recommendation is unchanged: a five-week-old, undocumented, six-star crate is
  not a dependency for a verification-first project, and the Python path is
  mature. Re-check it if the Rust question ever reopens (see revisit triggers).

- **`QuantumFramework`.** Quantum circuit simulation. Not our domain, and not a
  proof capability.

- **`codeparser`, `codeformatter`, `codeinspector`, `LSPServer`,
  `vscode-wolfram`, `tree-sitter-wolfram`, `zed-wolfram-lsp`,
  `zed-wolfram-highlighter`, `Sublime-WolframLanguage`.** This is a large,
  well-maintained, MIT-licensed cluster (roughly nine repos) for *authoring
  Wolfram Language*. We would be calling Wolfram, never writing it at any scale,
  so parsing, formatting, linting, and editor integration for WL are irrelevant
  to us.

- **The paclet CI machinery: `PacletCICD`, `check-paclet`, `build-paclet`,
  `test-paclet`, `submit-paclet`, and the five `PacletCICD-Examples-*` repos.**
  Eleven repos of publishing infrastructure for shipping Wolfram paclets. We are
  not publishing a paclet.

- **`GurobiLink`.** The link is MIT, but Gurobi itself is separately and
  expensively licensed, so this compounds licensing reason 4 with a second
  commercial dependency to reach an LP/MIP capability our exact-rational Farkas
  checkers already cover on the trusted side.

- **Domain packages: `BioFormatsLink`, `OpenCascadeLink`, `RhinoLink`,
  `FEMAddOns`, plus `DistMesh`, `MongoLink`, `GitLink`, `CSSTools`, `HEIFTools`,
  `ImageMetadataTools`, `semantic-math`, `draw`.** Microscopy formats, CAD
  geometry kernels, Rhino3D, finite elements, mesh generation, databases, image
  metadata. No relation to theorem proving. Several are GPL-2.0 or GPL-3.0
  (`DistMesh`, `BioFormatsLink`, `HEIFTools`, `ImageMetadataTools`), which is a
  vendoring hazard for an Apache/MIT project and a second reason to keep clear.

- Also outside scope, recorded for completeness: the deployment and notebook
  cluster (`WolframLanguageForJupyter`, `WolframWebEngineForPython`,
  `wolfram-notebook-embedder`, `WAS-Kubernetes`, `AWSLambda-WolframLanguage`),
  the three tiny JS utility repos (`loggers-js`, `sync-promise-js`,
  `reback-js`), the LLM/notebook products (`Chatbook`, `skills`,
  `system-modeler-ai-toolkit`), and the demo and training material
  (`Arrival-Movie-Live-Coding` at 1152 stars, `Data-Curation-Training`,
  `GitLink-Talk`).

## Org survey: the 59 repos by category

Recorded so the next reader does not re-enumerate. Counts are approximate at the
boundaries because a few repos span categories.

| Category | Count | Health |
|---|---:|---|
| Rust bindings | 6 | 4 archived 2026-06-15; `wolfram-rust-library` (new, undocumented) and `zed-wolfram-lsp` live |
| Python clients | 2 | `WolframClientForPython` MIT, 485 stars, push 2026-04-13 (alive); `WolframWebEngineForPython` stale since 2023 |
| WL authoring tooling (parser/formatter/inspector/LSP/editors) | ~9 | actively maintained, mostly pushed 2026-03 to 2026-07 |
| Paclet CI and publishing | 11 | mostly frozen 2022-2023; `PacletCICD` last touched 2024-12 |
| Domain and external-library links | ~12 | mixed; several GPL; many stale since 2017-2022 |
| Agent / LLM / skills | 4 | the most active cluster: `AgentTools` 2026-07-14, `Chatbook` 2026-07-19 |
| Deployment / notebooks | 5 | `WAS-Kubernetes` 2026-07-17 active; rest stale |
| Demos, training, misc, org profile | ~10 | mostly dormant |

The shape is clear and consistent with the recommendation: the org's live
investment in 2026 is agent and LLM integration plus WL authoring tooling, its
Rust surface just contracted, and its Python client remains the stable
general-purpose integration path.

## Risks

1. **Licence drift.** The free Engine's development-only terms are a vendor
   policy that can change in either direction, and the terms differ by Engine
   version. Any Wolfram code path must be optional at runtime and gated behind a
   config flag, so that tightening terms is a configuration change rather than a
   code change.
2. **Heavyweight optional dependency.** A Wolfram Engine install is multi-gigabyte
   and requires activation. It cannot be assumed present in CI, in a contributor
   checkout, or in a container. Every Wolfram-touching code path needs a
   deterministic non-Wolfram fallback, and the test suite must pass with no
   Engine present, exactly as the README already requires for toolchain-gated
   backends.
3. **Non-determinism across engine versions.** Wolfram's simplification and
   decomposition results are version-dependent and not guaranteed stable.
   `GroebnerBasis` orderings, `CylindricalDecomposition` output, and
   `FullSimplify` normal forms can all differ between releases. The operational
   consequence is a hard rule: **a Wolfram-generated certificate must be
   re-checked by our own checker on every run and must never be cached as
   trusted.** Caching a *verified* certificate keyed on its own content is fine,
   because the cached object was validated by our exact-rational checker and can
   be revalidated at any time; caching "Wolfram said this holds" is not, because
   there is no artifact behind it. This is the same distinction that makes
   `cert_sos.py`'s checker, and not its generator, the sound boundary.
4. **A free-licence Engine cannot be used in a deployed product.** Worth stating
   separately from risk 1 because it constrains the roadmap and not just the
   build: if Theoremata ever ships to end users, any Wolfram capability either
   disappears from that build or becomes a paid dependency that the user supplies.
   Design every integration so that the first option is a no-op configuration
   change.
5. **Process and network boundary.** The Python client and AgentTools both cross
   a process boundary, and the `WolframAlpha` tool crosses a network boundary to
   a hosted service. Anything reached that way is untrusted input by
   construction, must be parsed defensively, and is unusable in offline or
   deterministic modes. Our checkers are documented as "pure, offline, and
   deterministic"; nothing about that property may weaken to accommodate an
   oracle.

## Revisit triggers

1. **Wolfram ships a proof-object or derivation-trace API.** If `Simplify` or
   `Reduce` could emit a machine-checkable justification (a rewrite trace, a
   sequence of lemma applications, an explicit assumption set), reason 2 would
   weaken and reason 3 would become auditable. Reasons 1 and 4 would still stand,
   so this alone reopens nothing about the gate; it would only enlarge the set of
   certificate kinds Wolfram can generate.
2. **`wolfram-rust-library` acquires a README, a stable API, and about a year of
   maintained history.** That would reopen only the *ergonomics* question of how
   to call the Engine, never the trust question. Even then, prefer MCP over FFI
   unless a measured throughput problem justifies the binding, since FFI into a
   closed kernel is maintenance we would own alone.
3. **A production Engine licence is acquired, or Wolfram relicenses for
   deployment.** This would let Wolfram move from "optional accelerator" to
   "supported optional accelerator," meaning it could appear in CI and in
   documented workflows. It still would not enter the gate.
4. **The multivariate SOS seam becomes a measured bottleneck.** If the
   `generate_multivariate` stub in `cert_sos.py` is blocking real problems and an
   SDP-solver route proves fragile, `CylindricalDecomposition` and `Resolve`
   become the first alternative to try, since the checker side needs no change.
5. **An MCP client lands in the harness for any other reason.** The marginal cost
   of consuming AgentTools drops to near zero at that point, which changes the
   ordering of item 4 in the adopt list.

## Coverage gaps

Recorded honestly so the next reader knows what was and was not checked.

- **Metadata-level org survey only.** All 59 repos were enumerated with name,
  language, licence, star count, archived flag, and last push. Only `AgentTools`
  was examined beyond metadata (README plus top-level and `Kernel/Tools` trees).
  No repo was cloned and no source was read in depth.
- **Nothing was executed.** No Wolfram Engine was installed or run. Every claim
  about WL function behaviour (branch cuts, generic-position assumptions,
  `CylindricalDecomposition` output shape, `FindIntegerRelation` stability) comes
  from documented and widely reported behaviour of the system, not from testing
  in this environment. These are the load-bearing claims of item 1 in the adopt
  list, and they should be confirmed empirically before any implementation work.
- **The free-licence terms were not fetched during this pass.** The
  development-only restriction is stated from the published licence at
  https://www.wolfram.com/engine/free-license/ and is consistent with Wolfram's
  long-standing policy, but the page text was not retrieved and diffed here.
  Confirm before relying on it commercially.
- **Two task-supplied figures did not match the API.** The brief cited
  `WolframClientForPython` as updated 2026-07-10 and `AgentTools` at 78 stars
  updated 2026-07-18. The API returned `pushed_at` 2026-04-13 for the former and
  79 stars with `pushed_at` 2026-07-14 for the latter. The discrepancies are
  small and change no conclusion (both repos are alive; `AgentTools` is among the
  most recently touched in the org), but the values in this document are the
  verified ones.
- **`wolfram-rust-library`'s purpose is inferred, not verified.** See the skip
  section. Its contents were not read.
- **Zeilberger availability is qualified.** `Zeilberger` is not a kernel builtin;
  creative telescoping in the Wolfram ecosystem is generally reached through
  RISC's `HolonomicFunctions` package, which carries its own licence terms that
  were not examined. `cert_wz.py` already obtains `R(n, k)` from
  `sympy.concrete.gosper.gosper_term`, so this row of the certificate-generation
  table is the weakest of the five and the least urgent.
