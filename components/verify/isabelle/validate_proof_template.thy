(*
  Theoremata — Isabelle/HOL SOUNDNESS-GATE template (`thm_oracles` + clean build)
  ==============================================================================

  Isabelle counterpart of `components/verify/lean/validate_proof_template.lean`.
  Where the Lean template runs the LeanDojo `validateProof` checks *inside* the
  Lean process, Isabelle's LCF architecture gives the gate for free at two
  levels, which this template makes explicit:

    * Isabelle is LCF-style: a value of type `thm` can ONLY be built by the `Thm`
      kernel module, so any `thm` is valid-by-construction relative to
      axioms + oracles. Merely obtaining a `thm` for the goal is the analogue of
      Lean's kernel `addDecl` succeeding — a proof that does not kernel-check
      never yields a `thm`.
    * The remaining question is therefore purely: WHICH axioms and oracles does
      this `thm` rest on? `thm_oracles` answers it over the full transitive
      derivation graph — the faithful `#print axioms` analogue.

  This file is a WORKED REFERENCE: genuine `theorem ... by ...` proofs, then a
  doc block showing exactly what the gate runs (the oracle audit, the clean
  build, the escape-hatch scan) and what a PASS looks like. It mirrors the live
  gate (each system's CLI); it is not itself wired into the runtime.

  THE FOUR CHECKS (all must pass to accept a closed goal — parity with Lean):

    1. It must be a real `thm`: the proof reaches `qed`/terminal `by`/`done` with
       NO `sorry` and NO `oops` in the script. `sorry` is the `Pure.skip_proof`
       oracle (works only under `quick_and_dirty`) and taints the derivation;
       `oops` abandons the proof and yields no `thm` at all.
    2. `thm_oracles <thm>` ⇒ the empty oracle set (or ⊆ an approved whitelist).
       This is the trust half — covers the FULL transitive dependency graph, so
       a `sorry` or custom `oracle` buried in any ancestor lemma surfaces here.
       ML hard form: `Thm_Deps.all_oracles [thm] = []`.
    3. Independent kernel replay: a clean `isabelle build` from sources in a
       fresh environment re-runs every primitive inference (return code 0), built
       with `quick_and_dirty=false` so `sorry` cannot be legalized.
    4. Source/option scan rejects the escape hatches: `sorry`, `oops`,
       `quick_and_dirty`, `axiomatization`/`oracle`/`ML` axiom injection,
       and any `smt`/`eval` route left in oracle mode.

  Only when all four pass is a `reward = 1.0` terminal treated as certified.

  ----------------------------------------------------------------------------
  HOW A WARM Isabelle DRIVER WOULD CALL THIS (parity with lean_session.rs)
  ----------------------------------------------------------------------------
  The Isabelle backend generates a full `Scratch.thy`, submits it to the Isabelle
  Server via `use_theories`, and reads `use_theories_results.{ok, errors,
  nodes[].messages}`. The `thm_oracles`/`thm_deps` diagnostics are injected INTO
  the theory text and their output read back from `messages`. The gate folds into
  the same `CheckOutcome` shape the Lean session emits:

      { "ok":           <use_theories_results.ok && no sorry/oops in script>,
        "axioms_clean": <thm_oracles empty (or ⊆ whitelist)>,
        "messages":     [ <build errors / oracle names / scan hits on reject> ],
        "axioms":       [ <oracle + axiom names parsed from thm_oracles/print_axioms> ] }

  So `ok` is the *soundness* half (did the whole theory check to a real `thm` of
  the target, hole-free?) and `axioms_clean` is the *trust* half (empty oracle
  set — no sorry, no external oracle, no smt oracle-mode). Both must hold.

  Toolchain note: syntactically Isabelle/HOL-correct against `Main`; the
  diagnostic commands (`thm_oracles`, `thm_deps`, `print_axioms`) and the
  `isabelle build` invocation are exact, but treat the whole as the SHAPE of the
  gate — pin to your release (studied against Isabelle2025-2).
*)

theory validate_proof_template
  imports Main
begin

subsection \<open>1. Worked, genuinely-closed theorems — the ACCEPT case\<close>

text \<open>
  Each closes to a real @{typ thm} with no @{command sorry}. They use only
  constructive @{theory_text Main} facts, so their oracle set is empty — the
  shape the model emits and the gate certifies.\<close>

theorem add_comm_example: "n + m = m + (n :: nat)"
  by (rule add.commute)

theorem concrete_fact: "(2 :: nat) + 3 = 5"
  by simp

text \<open>A slightly longer structured Isar proof, still oracle-free:\<close>
theorem le_trans_example:
  fixes a b c :: nat
  assumes "a \<le> b" and "b \<le> c"
  shows "a \<le> c"
proof -
  from assms show ?thesis by (rule order_trans)
qed

subsection \<open>2. The oracle audit — `thm_oracles` (the `#print axioms` analogue)\<close>

text \<open>
  Run the audit on the target. A clean proof reports NO oracles:

    thm_oracles add_comm_example        \<comment> \<open>expect: (no oracles) / empty set\<close>
    thm_oracles concrete_fact le_trans_example

  @{command thm_oracles} "displays all oracles used in the internal derivation of
  the given theorems; this covers the full graph of transitive dependencies" —
  so a @{command sorry} (@{text Pure.skip_proof}) or a custom @{command oracle}
  hiding in ANY ancestor lemma surfaces here. The gate PASSES iff the reported
  set is empty, or a subset of an explicit whitelist.

  Provenance companions (not soundness by themselves):
    thm_deps add_comm_example      \<comment> \<open>immediate fact dependencies only\<close>
    print_axioms                   \<comment> \<open>the object-logic axiom base (whitelist HOL's)\<close>

  ML hard gate (run via `isabelle ML_process` or an `ML \<open> ... \<close>` block):
    Thm_Deps.all_oracles [@{thm add_comm_example}] = []       (* assert empty *)
    Thm.extra_shyps @{thm add_comm_example}          = []       (* no smuggled sort hyps *)
  Optionally set `Proofterm.proofs := 1` first to record propositions with the
  oracle names for a richer audit.\<close>

subsection \<open>3. Independent kernel re-check — a clean `isabelle build`\<close>

text \<open>
  Because a @{typ thm} already passed the kernel, the independent re-check is to
  rebuild the generated session from sources in a fresh environment — this
  re-runs every primitive inference (the LeanParanoia replay analogue):

    isabelle build -c -o quick_and_dirty=false -d . Theoremata_Scratch

  Return code 0 = the whole selected session re-checked. The
  @{text "quick_and_dirty=false"} override is essential: it makes @{command sorry}
  a hard ERROR rather than a silently-admitted oracle, so a build that would rely
  on @{command sorry} fails here. For an independent external checker, record
  proof terms (`Proofterm.proofs := 2`) and export them via the session
  @{text export_files} / `use_theories` @{text export_pattern}.\<close>

subsection \<open>4. Source / option scan for the escape hatches\<close>

text \<open>
  Regex-scan the generated @{text .thy} and the build options, REJECT on any of:

    \<^item> @{command sorry}                    — the @{text Pure.skip_proof} oracle.
    \<^item> @{command oops}                     — abandons the proof (no @{typ thm}); a
                                            leftover @{command oops} means the
                                            "proof" proves nothing.
    \<^item> @{text quick_and_dirty}  (build option or @{command declare}) — legalizes
                                            @{command sorry}; the build MUST set it false.
    \<^item> @{command axiomatization} / @{command oracle} / an @{command ML} block calling
      @{text "Thm.add_oracle"} — inject axioms / oracle functions.
    \<^item> @{method smt} left in oracle mode, or a raw @{method eval} whose result routes
      through the @{text Eval} oracle — both leave an oracle tag; prefer
      @{method metis}/@{method normalization} reconstructions and re-audit.

  Rationale: @{command sorry}, oracles, and @{text quick_and_dirty} are exactly
  the ways to obtain a @{typ thm} that did NOT go through a full kernel
  derivation. Checks 2 (oracle audit) and 3 (clean build with
  @{text "quick_and_dirty=false"}) catch them, and the scan fails fast before a
  build is even attempted.

  Note: @{command sledgehammer}'s suggested @{text "by (metis ...)"} lines are
  genuine kernel derivations (no oracle) and PASS the gate; watch @{text "by (smt ...)"}
  which may carry an oracle tag — re-audit any @{method smt} closure with
  @{command thm_oracles}.\<close>

subsection \<open>5. Negative reference — what the gate MUST reject\<close>

text \<open>
  Kept commented so this file stays oracle-free and builds clean; uncommenting
  must trip the corresponding check.

    \<comment> \<open>REJECT (check 1, 2 & 4): `sorry` is the Pure.skip_proof oracle; it would
        show up as an oracle in `thm_oracles bad_sorry` and needs quick_and_dirty.\<close>
    theorem bad_sorry: "n = n + (0 :: nat)"
      sorry

    \<comment> \<open>REJECT (check 2 & 4): an injected axiom — appears via `print_axioms` and, once
        used, in the `thm_oracles`/assumption graph of any dependent theorem.\<close>
    axiomatization where bad_axiom: "\<And>P. P"

    \<comment> \<open>REJECT (check 1 & 4): `oops` abandons the proof — no theorem is produced, so
        any downstream reference is a dangling/undefined name.\<close>
    theorem bad_oops: "False"
      oops
\<close>

end
