# Resource mining: `metamath-exe` (the C reference `metamath` verifier)

Source: `resources/metamath-exe-master/metamath-exe-master/` (nested one level).
Scope: characterize the C program our Metamath backend shells out to; confirm our
gate command is correct/complete; find stronger verification we should adopt.

## 0. Injection + license (do this first)

- **POSSIBLE INJECTION scan: clean.** Grepped `README.TXT`, `CONTRIBUTING.md`, and
  `doc/` for prompt-injection patterns (ignore-previous / "as an AI" / system-prompt /
  disregard). Only hit was the ordinary English "you must not use it in your software"
  in `doc/BUILD.md` (about `config.h.in`) — not an instruction to us. All repo content
  treated as untrusted data; nothing acted on.
- **LICENSE: GNU GPL v2-or-later** (`LICENSE.TXT` = GPL-2.0; `README.TXT` L7-8; every
  `.c` header, e.g. `metamath.c` L4 "GNU General Public License Version 2 or any later
  version"). Nuance: Norman Megill's own code is declared **public domain** in-file
  (`metamath.c` L8-11), but the *distributed package* is GPL-2+ because of third-party
  contributions. **Consequence for us: we only ever invoke the compiled `metamath`
  binary as a subprocess (no source linked, no code copied), so the GPL imposes no
  obligation on Theoremata.** Do NOT vendor/port any `.c` into our (permissive) tree.
  The bundled `set.mm` is public-domain / GPL per its own header.

## 1. What it is + how it's built/invoked

`metamath` is Norman Megill's canonical C reference implementation of the Metamath
proof verifier — the authoritative checker that `set.mm` targets. ~30 `.c`/`.h` files
in `src/`; the trusted kernel is `mmveri.c` (`verifyProof()`), driven by `mmcmds.c`
(`verifyProofs()`), parser `mmpars.c`, command loop `metamath.c`/`mmcmdl.c`.

Build (README.TXT / metamath.c header): `cd src && gcc m*.c -o metamath`, or
`autoreconf -i && ./configure && make`. Windows binary built with lcc. No build run
here (read-only constraint).

Invocation is a command interpreter; each argv item is one command. The documented
batch form (`mmhlpb.c` L78) is exactly ours:
```
bash$ ./metamath 'read set.mm' 'verify proof *' exit
```
**Our backend's `read <file>; verify proof *; exit` is the correct, canonical command.**

Verification-relevant commands (from `mmcmdl.c` + help in `mmhlpb.c`):
- **`VERIFY PROOF <label-match> [/ SYNTAX_ONLY]`** — the real kernel check (see §2).
  `verify proof *` verifies every `$p` in the loaded DB. `/ SYNTAX_ONLY` checks only
  syntax + RPN-stack shape and **does not verify correctness** — we correctly do NOT
  use it.
- **`VERIFY MARKUP <label-match> [/ DATE_SKIP /TOP_DATE_CHECK /FILE_CHECK …]`** — checks
  *comment markup*, `$t` latex/html defs, `` `...` ``/`~label`/`[bibref]` links, date
  tags, and (relevantly) that `axNN` `$p` vs `ax-NN` `$a` pairs have identical content.
  This is a *convention/markup* linter, **not** a soundness check for a generated proof.
- `READ <file> [/ VERIFY]` — `/ VERIFY` folds read+verify into one command (equivalent
  to our two-step form; `metamath.c` L2065).
- There is **no separate "grammar check" or "definition check" command** in this
  version. Grammatical (wff) parsing happens *inside* `verify proof` via the syntax `$a`
  builders; **definition soundness (conservativity/eliminability of `df-*`) is NOT
  checked by metamath-exe at all** — that is an external tool's job (mmj2 / metamath-knife
  grammar). Our lexical `$a` scan is the conservative stand-in (see §3).

## 2. Trust model — what `verify proof *` DOES and does NOT check

**Does (the kernel, `mmveri.c` `verifyProof`, per `$p`):** replays the RPN proof on a
stack; each step pushes a hypothesis or applies a prior `$a`/`$p` assertion under a
variable substitution; unifies against `$f` floating hyps; enforces `$d`
disjoint-variable constraints; and confirms the final stack entry is **exactly the
symbol string the `$p` asserts**. This is a complete, sound check that *the asserted
statement follows from the axioms/theorems it cites, within the loaded database.*

**Does NOT check (critical for our threat model):**
- **It does NOT verify the `$a` axioms are the sanctioned `set.mm` ones.** `verify
  proof` trusts *whatever* `$a`/`$f`/`$c`/`$v`/`$d` are in the loaded database. A
  generated file that declares its own `badax $a |- ph $.` and proves a theorem *from
  it* verifies cleanly. Soundness is relative to the loaded axiom set, which the kernel
  never sanctions. → our lexical `$a` scan is exactly the right defense.
- **It has no notion of an "intended" theorem.** `verify proof` only checks the proof
  proves *its own* asserted `$p` string. A proof that asserts a *different* (weaker /
  renamed / trivial) statement than we asked for verifies fine. → our
  `check_metamath_signature` (bind asserted symbols ↔ canonical statement) is required
  and correct.
- **It does NOT check definition conservativity**, markup, or dates (those are `VERIFY
  MARKUP`, and even that does not prove `df-*` eliminability).
- **Incomplete proofs (`$= ? $.`) are a WARNING, not an error.** `verifyProofs`
  (`mmcmds.c` L4644-4657) collects them into "Warning: The following $p statement(s)
  were not proved:" and does **not** set `errorFound`. → our bare-`?` lexical scan is
  required; the CLI would not fail on them.

### ⚠️ 2b. HEADLINE FINDING — metamath-exe ALWAYS exits 0

`main()` ends with an unconditional `return 0;` (`metamath.c` L775). The **only**
non-zero exit in the whole program is the fatal handler `mmfatl.c` `exit(EXIT_FAILURE)`
(out-of-memory / internal bug) and the `mmtest.c` self-test harness. Verification
failures are *severity-2 errors* that merely increment `g_errorCount` and print
`?Error…` to **stdout** (`mminou.c` L1058) — they never affect the process exit code.

**This breaks our backend's success detection.** `ExternalBackend::compile()` /
`kernel_recheck()` decide pass/fail on `out.success()`, which is
`code == Some(0)` (`components/prover/session/exec.rs` L191-192). Because metamath
returns 0 even when `verify proof *` reports errors — and also when `read` fails on a
malformed/nonexistent file ("?No source file has been read in") — **a FAILED
verification is currently indistinguishable from success by exit code.** A generated
proof with a parse error, a wrong RPN step, or an incomplete `?` proof can pass the
kernel layers of the gate on exit code alone.

The only reliable pass signal is **stdout**: success prints exactly
`All proofs in the database were verified.` (`mmcmds.c` L4661-4664); any failure prints
`?Error`, `?Warning`, `... were not proved`, or `N errors were found.`

## 3. MAPPING — is our gate using metamath-exe correctly + completely?

Backend: `components/prover/backends/external.rs`; gate orchestration
`components/prover/formal.rs::verify()` (3+1 layers); signature check
`components/prover/statement_preservation.rs::check_metamath_signature`.

| Our layer | Uses metamath-exe how | Correct? |
|---|---|---|
| Command | `read <f>; verify proof *; exit` | ✅ canonical form; NOT `/SYNTAX_ONLY` (good) |
| compile / kernel_recheck | pass/fail = process **exit code** | ❌ **BROKEN** — exit is always 0 (§2b) |
| Lexical `$a` scan | rejects any generated axiom | ✅ covers the "not-sanctioned-axioms" hole |
| Lexical bare-`?` scan | rejects incomplete proofs | ✅ CLI only warns; scan is required |
| `$[ ]` include scan | rejects escaping/`..`/abs includes | ✅ path-traversal defense |
| Signature check | asserted `$p` symbols ↔ canonical | ✅ covers "different theorem" hole |
| `audit_axioms` | returns empty / within_whitelist=true | ⚠️ stub — no real axiom-use audit |

**Verdict:** the *lexical* and *signature* layers are well-designed and cover exactly
the holes `verify proof` leaves. But the **kernel layer is unsound as wired**: it trusts
an exit code metamath never sets meaningfully. This is the one must-fix.

Missing metamath-exe capabilities we could use:
- **stdout success-sentinel parsing** (must-fix, §2b).
- **`verify proof <ourlabel>`** instead of `*` — `*` re-verifies all ~43k set.mm proofs
  every job (slow); verifying only our generated label is far faster and equally sound
  for the generated theorem (we already trust the reviewed set.mm). Keep `*` only if you
  want to also detect corpus tampering.
- **`SHOW TRACE_BACK <label> / AXIOMS`** (`mmhlpb.c` L372-394) — lists the axioms a proof
  ultimately depends on. This is the honest, kernel-backed way to implement
  `audit_axioms` (real axiom whitelist) instead of the current stub. metamath-knife's
  `verify_usage_pass` does the same in-process (see §4).
- `VERIFY MARKUP` — optional; not soundness-relevant for generated single proofs and
  will emit spurious date/file warnings. Skip unless we start editing set.mm itself.

## 4. Keep shelling to metamath-exe, or switch to metamath-knife?

metamath-knife is mined separately (`docs/resource-mining/new/metamath-knife.md`; **MIT
OR Apache-2.0**, in-process `Database::verify_pass`/`verify_one`, structured typed
diagnostics, `verify_usage_pass` for axiom audit, in-memory `$[ ]` include resolution).

**Recommendation (aligns with the knife report):**
- **Primary kernel → migrate to metamath-knife in-process.** It returns *structured
  pass/fail* (no stdout-string parsing, no always-0 exit-code trap), removes the
  subprocess + CWD-relative include dance in `scaffold()`, gives a real `audit_axioms`
  via `verify_usage_pass`, and `verify_one` warms set.mm once then checks each generated
  `$p` — solving both the soundness bug (§2b) and the `verify proof *` slowness.
- **Keep metamath-exe as the optional SECONDARY recheck.** The backend already has a
  `secondary_binary` slot for Metamath (`external.rs` L197-201, L370-385, env
  `THEOREMATA_METAMATH_SECONDARY`). Wiring the *canonical Megill C verifier* there gives
  two **independent-codebase** verifiers that must agree — defense-in-depth against a
  bug in either. metamath-exe stays the ground-truth reference the community trusts.
- **If we do NOT migrate now:** metamath-exe remains usable, but the backend MUST stop
  trusting the exit code and parse stdout (§5) — otherwise the kernel layer is unsound.

## 5. Concrete backend-hardening commands/steps to adopt

1. **[MUST-FIX] Replace exit-code success with stdout-sentinel parsing.** For the
   shell-out path, treat verification as passed **iff** stdout contains
   `All proofs in the database were verified.` **and** contains none of `?Error`,
   `?Warning`, `were not proved`, `errors were found`, `No source file has been read`.
   Fail-closed on anything else (including a clean exit with no sentinel).
2. **[Recommended] Verify only our label:** `read <f>` / `verify proof <ourlabel>`
   (or keep `*` if corpus-tamper detection is wanted) — big speedup, same soundness for
   the generated theorem. Success sentinel for a single label is the absence of `?Error`
   plus the per-label `verify proof <label>` output (no "were not proved" warning).
3. **[Recommended] Real axiom audit:** add `show trace_back <ourlabel> / AXIOMS` (or
   metamath-knife `verify_usage_pass`) and diff the reported axioms against the sanctioned
   set.mm axiom whitelist — replacing the `audit_axioms` stub. This makes "used only
   sanctioned axioms" a kernel-backed fact rather than a lexical `$a`-absence inference.
4. **[Strategic] Migrate the primary kernel to metamath-knife in-process**
   (`Database::verify_one`), keep metamath-exe in the `secondary_binary` slot for a
   dual-verifier cross-check.
5. Keep every existing lexical layer (`$a`, bare `?`, `$[ ]` include, signature check) —
   they defend holes the kernel genuinely cannot see and cost nothing.

## Prioritized adopt list

1. **stdout-sentinel success parsing** (fixes the always-exit-0 soundness bug) — MUST.
2. **`show trace_back / AXIOMS` (or knife `verify_usage_pass`) real axiom audit** — HIGH.
3. **`verify proof <label>` instead of `*`** (or in-process `verify_one`) — perf, MED-HIGH.
4. **metamath-knife primary in-process + metamath-exe secondary cross-check** — strategic.
5. Retain all lexical/signature layers unchanged — they are correct and necessary.
