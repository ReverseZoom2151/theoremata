# Cross-system proof-pattern templates

Per-system **documented reference patterns** for Theoremata's two soundness
idioms, written for each formal-system backend Theoremata targets — **Lean 4**,
**Rocq (Coq)**, and **Isabelle/HOL**. They exist so a reader (or the model)
can see the *same* two patterns expressed idiomatically in each system, with the
trust story spelled out per kernel.

These are **reference artifacts, not runtime code.** Nothing here is imported by
the Rust core or the Python tools. The *live* soundness gates call each system's
real CLI/driver (`lean_session.rs` + `#print axioms`; `rocqchk` +
`Print Assumptions`; `isabelle build` + `thm_oracles`). These files document the
shape those gates enforce. They are written to be well-formed and idiomatic but
need not compile against any specific library version.

## The two idioms

### 1. Verified-`decide` / finite-certificate (`verified_decide_template.*`)
The model emits a **finite certificate table** that the **kernel** checks
exhaustively, via a computable checker + a soundness bridge to an abstract spec.
The proof is closed by a *kernel-trusted computation* — never by an external
oracle or trusted compiled code.

| System | File | Closer | Kernel-trusted mechanism |
|---|---|---|---|
| Lean 4 | [`lean/verified_decide_template.lean`](lean/verified_decide_template.lean) | `by decide` | `Decidable` instance from a `checkValid_iff_isValid` bridge; **never** `native_decide` |
| Rocq | [`coq/verified_decide_template.v`](coq/verified_decide_template.v) | `vm_compute; reflexivity` | `reflect`/`Bool.reflect` bridge; `vm_compute` is kernel-re-run at `Qed`, **not** an oracle |
| Isabelle | [`isabelle/verified_decide_template.thy`](isabelle/verified_decide_template.thy) | `by normalization` / `by eval` | code-equation reflection; `normalization` is kernel-checked (nbe), **not** a `code`/oracle escape |

### 2. Soundness-gate / proof-validation (`validate_proof_template.*`)
The in-kernel re-check + trust audit that certifies a closed goal before a
`reward = 1.0` terminal is trusted: reject `sorry`/holes/oracles and confirm the
proof rests only on whitelisted axioms.

| System | File | Trust audit (`#print axioms` analogue) | Independent kernel replay |
|---|---|---|---|
| Lean 4 | [`lean/validate_proof_template.lean`](lean/validate_proof_template.lean) | in-process `validateProof` (instantiate mvars, defeq, reject `sorry`/mvars, `addDecl`) + `#print axioms` allowlist | kernel `addDecl` re-check |
| Rocq | [`coq/validate_proof_template.v`](coq/validate_proof_template.v) | `Print Assumptions` ⇒ "Closed under the global context" (or whitelist) | `rocqchk`/`coqchk -o -silent` on the `.vo` |
| Isabelle | [`isabelle/validate_proof_template.thy`](isabelle/validate_proof_template.thy) | `thm_oracles` empty (full transitive graph) / `Thm_Deps.all_oracles = []` | clean `isabelle build -o quick_and_dirty=false` |

Each `validate_proof` template also documents a **source scan** for the silent
escape hatches that are *not* axioms and so do not appear in the trust audit:
Lean's `native_decide`/`sorryAx`; Rocq's `Unset … Checking` / `-type-in-type` /
`bypass_check` / `Admitted`; Isabelle's `sorry`/`oops`/`quick_and_dirty`.

## Provenance

The Lean pair is the original; the Rocq and Isabelle pairs mirror it for
cross-system parity. The per-system mechanisms are grounded in the integration
studies under [`docs/formal-systems/`](../../docs/formal-systems/)
(`rocq.md`, `isabelle.md`).
