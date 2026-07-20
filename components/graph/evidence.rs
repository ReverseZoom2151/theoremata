//! Canonical evidence-type strings for `Store::add_evidence`, plus a drift
//! guard that keeps this list honest against the code that actually emits.
//!
//! Evidence rows are the audit trail. Declaring a type here is free; emitting
//! the WRONG one is not, because a reader (and `docs/TRUST_BOUNDARIES.md`)
//! treats an evidence row as a record that a named check really ran. So the
//! rule enforced below is deliberately asymmetric:
//!
//! * every declared type must either have a real producer in the tree, or be
//!   listed in [`RESERVED_UNEMITTED`] with the producer it is waiting on;
//! * a type listed in [`RESERVED_UNEMITTED`] must have NO producer, so the
//!   reserved list cannot quietly rot into a lie once someone wires the
//!   producer up.
//!
//! The guard reads the source of every `add_evidence` call and looks at the
//! third argument (`kind`). It cannot be satisfied by a comment, a doc string,
//! or a mention of the name in a JSON payload: comments are stripped before the
//! scan, and only the `kind` argument position counts.
//!
//! This module is a registry, not a producer, so it is excluded from its own
//! scan. Evidence must be written where the check runs.

#![allow(dead_code)]

/// Lean file typechecked. Producer: `components/reason/orchestration/agent.rs`
/// (source `verifier`) and `components/reason/orchestration/observe.rs`
/// (source `lean`).
pub const LEAN_COMPILE: &str = "lean_compile";

/// Transitive axiom set within the whitelist.
///
/// RESERVED, currently unemitted. The audit itself does run, but its result is
/// folded into the [`LEAN_COMPILE`] verdict (`compiles && axioms_clean`) and
/// into the `formal_verify` report payload, so no standalone row is written. If
/// the axiom gate ever needs to be queryable on its own, the producer is the
/// same verifier path in `agent.rs`; add the row there and drop this from
/// [`RESERVED_UNEMITTED`].
pub const AXIOM_AUDIT: &str = "axiom_audit";

/// N consecutive clean passes before certify. Producer:
/// `components/reason/orchestration/agent.rs` (source `verifier`).
pub const K_CONSECUTIVE_CLEAN: &str = "k_consecutive_clean";

/// Adversarial battery on certified nodes. Producer:
/// `components/reason/orchestration/agent.rs` (source `lean_paranoia`).
///
/// Note that `components/verify/hardening.rs` also writes a row for the same
/// run, but under kind `lean_paranoia` with source `hardening`, i.e. the two
/// fields are swapped relative to this one. That kind is intentionally not
/// declared here; the guard only requires declared -> emitted, never the
/// converse, because plenty of local kinds (`critique`, `sketch_hole`,
/// `pool_meta_gate`, ...) are legitimately not part of the trust-boundary
/// vocabulary.
pub const HARDENING: &str = "hardening";

/// Numeric/symbolic counterexample screen. Producer:
/// `components/reason/orchestration/agent.rs` (source `falsifier`).
pub const FALSIFICATION: &str = "falsification";

/// Candidate lemmas (untrusted hints). Producer:
/// `components/reason/orchestration/agent.rs` (source `librarian`).
pub const RETRIEVAL: &str = "retrieval";

/// Externally generated Lean plus request provenance.
///
/// EMITTED by the completion branch of `poll` in
/// `components/prover/backends/{aristotle,leandojo,reprover}.rs`, which is the
/// only point where the request id, the artifact directory and the graph
/// coordinates (`job.project_id`, `job.node_id`) are all in hand. Payload is
/// [`external_prover_payload`]. Note leandojo and reprover write no artifact
/// directory at all, so their site is the completion branch itself rather than
/// a `write_artifact` line.
///
/// Verdict is `recorded`, deliberately not a gate result: the prover's own
/// claimed status travels in the payload as `claimed_status`, because an
/// external producer's opinion of its own output is not a verdict of ours.
pub const EXTERNAL_PROVER_ARTIFACT: &str = "external_prover_artifact";

/// Output of an external producer locally re-verified (trust-but-verify).
///
/// EMITTED by `components/prover/session/verify.rs` on the round-trip path, in
/// the branch where a `HardeningContext` is available. The storeless entry
/// points still write nothing rather than a weaker row.
///
/// Verdict is scoped to what was actually established: `lexical_screen_clean`
/// or `lexical_screen_flagged`, because `report.live` is always false on this
/// path and a verdict reading like a certification would overstate it.
pub const EXTERNAL_PRODUCER_CHECKED: &str = "external_producer_checked";

/// MILP reformulation equivalence attempt.
///
/// RESERVED, currently unemitted. There is no reformulation/FLARE track in the
/// tree at all, so this has no candidate producer yet; it is a placeholder for
/// a component that does not exist.
pub const REFORMULATION_CHECK: &str = "reformulation_check";

/// Structured verifier stderr/stdout from a repair loop.
///
/// EMITTED by `components/reason/proving/refine_ops.rs::record_repair_evidence`,
/// at the call site rather than inside `repair_proof`, which stays storeless so
/// its injected verifier and repairer keep tests deterministic. The call site is
/// also the only place holding both the graph coordinates and the finished trace.
///
/// Verdict is `repaired` or `unrepaired`, never `pass`: a repair is not a
/// verification, and a repaired proof still faces the normal gate. The payload
/// carries the per-round trace from `RepairReport::rounds` and deliberately
/// excludes proof and candidate bodies, which are untrusted model text.
pub const REPAIR_LOOP: &str = "repair_loop";

/// Every canonical type declared above. The guard checks that this list and the
/// `pub const` declarations agree, so a new constant that is not registered
/// here fails the tests rather than silently escaping the drift check.
pub const ALL: &[&str] = &[
    LEAN_COMPILE,
    AXIOM_AUDIT,
    K_CONSECUTIVE_CLEAN,
    HARDENING,
    FALSIFICATION,
    RETRIEVAL,
    EXTERNAL_PROVER_ARTIFACT,
    EXTERNAL_PRODUCER_CHECKED,
    REFORMULATION_CHECK,
    REPAIR_LOOP,
];

/// Types that are declared but that nothing emits yet, each with a note above
/// naming the producer it is waiting on.
///
/// This exists so the gap is stated in code instead of only in prose. Removing
/// an entry is what wiring a producer looks like; adding one without a real
/// reason is visible in review, and adding one for a type that IS emitted fails
/// the guard.
pub const RESERVED_UNEMITTED: &[&str] = &[
    AXIOM_AUDIT,
    REFORMULATION_CHECK,
];

/// True when nothing in the tree writes rows of this type. Callers that render
/// an audit trail can use this to avoid claiming coverage that does not exist.
pub fn is_reserved_unemitted(kind: &str) -> bool {
    RESERVED_UNEMITTED.contains(&kind)
}

/// Build a provenance payload for externally generated Lean (Putnam/Aristotle pattern).
pub fn external_prover_payload(
    service: &str,
    request_id: Option<&str>,
    input_hash: Option<&str>,
    output_hash: Option<&str>,
    duration_ms: Option<u128>,
    cost: Option<f64>,
    extra: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "service": service,
        "request_id": request_id,
        "input_hash": input_hash,
        "output_hash": output_hash,
        "duration_ms": duration_ms,
        "cost_usd": cost,
        "extra": extra,
    })
}

// ---------------------------------------------------------------------------
// Drift guard: declared types versus the code that actually emits them.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod drift_guard {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};

    /// Directories scanned for producers, relative to the crate root.
    const SCAN_ROOTS: &[&str] = &["components", "app"];

    /// This registry is not a producer, so an `add_evidence` call written here
    /// must not be able to satisfy the guard for its own constants.
    const SELF_PATH: &str = "evidence.rs";

    fn is_ident(b: u8) -> bool {
        b.is_ascii_alphanumeric() || b == b'_'
    }

    /// If a string, raw-string, byte-string or char literal starts at `i`,
    /// return the index just past it.
    ///
    /// Lifetimes (`'a`) deliberately return `None`: treating them as char
    /// literals would swallow the rest of the line and desync every later
    /// offset.
    fn literal_end(b: &[u8], i: usize) -> Option<usize> {
        let prefix_ok = i == 0 || !is_ident(b[i - 1]);
        let mut j = i;
        if prefix_ok && b[j] == b'b' {
            j += 1;
        }
        // Raw string: r"..." / r#"..."# / br#"..."#
        if prefix_ok && j < b.len() && b[j] == b'r' {
            let mut hashes = 0usize;
            let mut k = j + 1;
            while k < b.len() && b[k] == b'#' {
                hashes += 1;
                k += 1;
            }
            if k < b.len() && b[k] == b'"' {
                k += 1;
                while k < b.len() {
                    if b[k] == b'"' {
                        let mut h = 0usize;
                        while h < hashes && k + 1 + h < b.len() && b[k + 1 + h] == b'#' {
                            h += 1;
                        }
                        if h == hashes {
                            return Some(k + 1 + hashes);
                        }
                    }
                    k += 1;
                }
                return Some(b.len());
            }
        }
        // Ordinary (or byte) string.
        if j < b.len() && b[j] == b'"' && (j == i || prefix_ok) {
            let mut k = j + 1;
            while k < b.len() {
                match b[k] {
                    b'\\' => k += 2,
                    b'"' => return Some(k + 1),
                    _ => k += 1,
                }
            }
            return Some(b.len());
        }
        // Char literal, escaped or not. Anything else beginning with a quote is
        // a lifetime.
        if b[i] == b'\'' {
            if i + 2 < b.len() && b[i + 1] == b'\\' {
                let mut k = i + 2;
                while k < b.len() && b[k] != b'\'' {
                    k += 1;
                }
                return Some((k + 1).min(b.len()));
            }
            if i + 2 < b.len() && b[i + 2] == b'\'' {
                return Some(i + 3);
            }
            return None;
        }
        None
    }

    /// Replace comments with spaces, preserving byte offsets and newlines so
    /// reported line numbers still refer to the original file. String literals
    /// are kept verbatim: they are the payload we are trying to read.
    fn blank_comments(src: &str) -> String {
        let b = src.as_bytes();
        let mut out: Vec<u8> = Vec::with_capacity(b.len());
        let mut i = 0usize;
        while i < b.len() {
            if let Some(end) = literal_end(b, i) {
                out.extend_from_slice(&b[i..end]);
                i = end;
                continue;
            }
            if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
                while i < b.len() && b[i] != b'\n' {
                    out.push(b' ');
                    i += 1;
                }
                continue;
            }
            if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
                let mut depth = 1usize;
                out.extend_from_slice(b"  ");
                i += 2;
                while i < b.len() && depth > 0 {
                    if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
                        depth += 1;
                        out.extend_from_slice(b"  ");
                        i += 2;
                    } else if b[i] == b'*' && i + 1 < b.len() && b[i + 1] == b'/' {
                        depth -= 1;
                        out.extend_from_slice(b"  ");
                        i += 2;
                    } else {
                        out.push(if b[i] == b'\n' { b'\n' } else { b' ' });
                        i += 1;
                    }
                }
                continue;
            }
            out.push(b[i]);
            i += 1;
        }
        String::from_utf8(out).expect("blanking comments preserves UTF-8 boundaries")
    }

    /// Byte range of the `n`th top-level argument of a call whose `(` ends at
    /// `start`. Nesting, string literals and char literals are skipped, so a
    /// `json!({ "repair_loop": ... })` in a later argument cannot be mistaken
    /// for the `kind`.
    fn arg_range(b: &[u8], start: usize, n: usize) -> Option<(usize, usize)> {
        let mut depth = 0i32;
        let mut idx = 0usize;
        let mut arg_start = start;
        let mut i = start;
        while i < b.len() {
            if let Some(end) = literal_end(b, i) {
                i = end;
                continue;
            }
            match b[i] {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => {
                    if depth == 0 {
                        return if idx == n { Some((arg_start, i)) } else { None };
                    }
                    depth -= 1;
                }
                b',' if depth == 0 => {
                    if idx == n {
                        return Some((arg_start, i));
                    }
                    idx += 1;
                    arg_start = i + 1;
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// Every literal `kind` passed to `add_evidence` in one file, with the line
    /// it appears on. A non-literal `kind` (a variable, a `format!`) is not
    /// counted: the guard is about statically provable producers, and counting
    /// dynamic kinds would let anything satisfy anything.
    fn emitted_in(src: &str) -> Vec<(String, usize)> {
        let code = blank_comments(src);
        let b = code.as_bytes();
        let mut found = Vec::new();
        let needle = "add_evidence(";
        let mut from = 0usize;
        while let Some(rel) = code[from..].find(needle) {
            let open = from + rel + needle.len();
            from = open;
            if let Some((s, e)) = arg_range(b, open, 2) {
                let arg = code[s..e].trim();
                if arg.len() >= 2 && arg.starts_with('"') && arg.ends_with('"') {
                    // Line of the literal itself, not of the comma before it.
                    let lit = s + code[s..e].find('"').unwrap_or(0);
                    let line = code[..lit].bytes().filter(|&c| c == b'\n').count() + 1;
                    found.push((arg[1..arg.len() - 1].to_string(), line));
                }
            }
        }
        found
    }

    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs")
                && path.file_name().and_then(|f| f.to_str()) != Some(SELF_PATH)
            {
                out.push(path);
            }
        }
    }

    /// kind -> the `file:line` sites that emit it.
    fn producers() -> BTreeMap<String, Vec<String>> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        for r in SCAN_ROOTS {
            rust_files(&root.join(r), &mut files);
        }
        assert!(
            files.len() > 50,
            "source scan found only {} files under {:?}; the guard would pass \
             vacuously, which is worse than no guard",
            files.len(),
            SCAN_ROOTS
        );
        let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for f in files {
            let Ok(src) = std::fs::read_to_string(&f) else {
                continue;
            };
            let rel = f.strip_prefix(root).unwrap_or(&f).display().to_string();
            for (kind, line) in emitted_in(&src) {
                map.entry(kind).or_default().push(format!("{rel}:{line}"));
            }
        }
        map
    }

    // -- the parser must be shown to work before its verdicts mean anything --

    #[test]
    fn parser_reads_the_kind_argument_and_ignores_decoys() {
        let sample = r#"
            // store.add_evidence(p, n, "commented_out", "s", "v", json!({}))
            /* add_evidence(p, n, "block_commented", "s", "v", x) */
            let doc = "add_evidence(p, n, \"in_a_string\", \"s\", \"v\", x)";
            self.store.add_evidence(
                project_id,
                &node.id,
                "real_kind",
                "some_source",
                if ok { "pass" } else { "fail" },
                json!({ "decoy_kind": 1, "note": "add_evidence(a, b, \"nope\", c, d, e)" }),
            )?;
            s.add_evidence(&p.id, &n.id, "one_liner", "sympy", "ok", json!({}))
            store.add_evidence(project_id, node_id, dynamic_kind, "s", "v", payload)
        "#;
        let kinds: BTreeSet<String> = emitted_in(sample).into_iter().map(|(k, _)| k).collect();
        assert_eq!(
            kinds,
            ["one_liner", "real_kind"]
                .iter()
                .map(|s| s.to_string())
                .collect::<BTreeSet<_>>(),
            "the scan must see exactly the two literal `kind` arguments"
        );
    }

    #[test]
    fn parser_finds_the_known_real_producers() {
        // If these ever stop being found, the guard has gone blind and every
        // other assertion in this module is meaningless.
        let map = producers();
        for kind in [
            super::LEAN_COMPILE,
            super::K_CONSECUTIVE_CLEAN,
            super::HARDENING,
            super::FALSIFICATION,
            super::RETRIEVAL,
        ] {
            assert!(
                map.contains_key(kind),
                "scan lost the producer for {kind:?}; guard is blind"
            );
        }
    }

    // -- the guard itself --

    #[test]
    fn every_declared_type_is_emitted_or_explicitly_reserved() {
        let map = producers();
        let orphans: Vec<&str> = super::ALL
            .iter()
            .copied()
            .filter(|k| !map.contains_key(*k) && !super::RESERVED_UNEMITTED.contains(k))
            .collect();
        assert!(
            orphans.is_empty(),
            "evidence types declared with no producer anywhere: {orphans:?}. \
             Either wire a producer (an `add_evidence` call whose `kind` is this \
             string) or add it to RESERVED_UNEMITTED with a note naming the \
             producer it waits on. Do not document an audit trail the database \
             does not contain."
        );
    }

    #[test]
    fn reserved_types_really_have_no_producer() {
        let map = producers();
        let live: Vec<String> = super::RESERVED_UNEMITTED
            .iter()
            .filter_map(|k| {
                map.get(*k)
                    .map(|sites| format!("{k} emitted at {}", sites.join(", ")))
            })
            .collect();
        assert!(
            live.is_empty(),
            "RESERVED_UNEMITTED claims these are never written, but they are: \
             {live:?}. Remove them from RESERVED_UNEMITTED (and update \
             docs/TRUST_BOUNDARIES.md) in the same change that wired the producer."
        );
    }

    #[test]
    fn reserved_is_a_subset_of_all_and_all_is_duplicate_free() {
        let unique: BTreeSet<&str> = super::ALL.iter().copied().collect();
        assert_eq!(unique.len(), super::ALL.len(), "duplicate entry in ALL");
        for k in super::RESERVED_UNEMITTED {
            assert!(unique.contains(k), "{k:?} is reserved but not in ALL");
        }
    }

    /// A constant declared here but left out of `ALL` would escape every check
    /// above, so the registry is checked against its own source text.
    #[test]
    fn all_matches_the_declared_constants() {
        let src = include_str!("evidence.rs");
        let declared: BTreeSet<String> = src
            .lines()
            .filter_map(|l| {
                let l = l.trim();
                let rest = l.strip_prefix("pub const ")?;
                let (_, after) = rest.split_once(": &str = \"")?;
                Some(after.strip_suffix("\";")?.to_string())
            })
            .collect();
        let registered: BTreeSet<String> = super::ALL.iter().map(|s| s.to_string()).collect();
        assert_eq!(
            declared, registered,
            "the `pub const` evidence types and ALL disagree; every declared \
             type must be registered so the drift guard can see it"
        );
    }

    #[test]
    fn is_reserved_unemitted_agrees_with_the_list() {
        assert!(super::is_reserved_unemitted(super::AXIOM_AUDIT));
        assert!(!super::is_reserved_unemitted(super::LEAN_COMPILE));
        assert!(!super::is_reserved_unemitted("not_an_evidence_type"));
    }
}
