//! Hash-consed checker-result cache: remember that a goal has already cleared
//! the verification gate so its subproof is not re-verified across candidates.
//!
//! During a proving/search run the same sub-goal recurs constantly: two candidate
//! proofs of a theorem often share a lemma or an obligation, a portfolio re-attacks
//! the same statement across rounds, and a refinement loop re-emits an unchanged
//! sub-goal. Re-running the (expensive) checker/gate on an *already-verified*
//! sub-goal is pure waste. This module memoises the gate's verdict, keyed on a
//! stable hash of the complete verification input: formal system, canonical
//! statement, ordered context, exact proof source, checker identity, and gate
//! policy. A verdict is reusable only when every one of those fields matches.
//!
//! ## What is cached — and why it is SOUND
//!
//! The cache records that one exact proof artifact cleared one exact gate. It is
//! not a theorem database and does not substitute one proof for another. Including
//! the proof source prevents a valid proof of a goal from laundering a different,
//! unchecked candidate for the same goal. Including checker identity and policy
//! prevents reuse across toolchains, live/mock modes, axiom policies, or gate
//! revisions.
//!
//! Project/corpus identity belongs in `checker_identity`; callers must include it
//! when it can affect elaboration.
//!
//! ## Why the IMPORT MANIFEST is its own key field
//!
//! `checker_identity` is an INSTALLATION identity — binaries, runner, pinned
//! toolchain, project root, cache epoch. It is computed once per run and is
//! therefore constant across every problem in that run. It does NOT capture the
//! per-problem IMPORT CLOSURE, and neither does `proof_source` in general: a
//! backend may prepend imports drawn from the task/project rather than from the
//! candidate text, in which case two candidates with byte-identical source can be
//! elaborated against different environments.
//!
//! That gap is not hypothetical. A mined system recorded a live failure in which an
//! unvalidated import list let
//!
//! ```text
//! Mathlib
//! axiom cheat : False
//! ```
//!
//! through as an "import", baking a false axiom into the environment of every
//! subsequent proof. Under a key blind to imports, the FIRST proof verified in the
//! poisoned environment would be cached and then replayed as verified for inputs
//! elaborated in a clean one — and, worse, a proof verified in a clean environment
//! would be served for a request made in the poisoned one, hiding the poisoning.
//! [`VerificationCacheKey::import_manifest`] closes that: the environment a proof
//! was checked in is part of what the cache remembers.
//!
//! This is a KEY field, not a validator. It makes a changed environment a
//! mandatory MISS; it does not decide whether an import list is legitimate. Import
//! *validation* is the backend/gate's job, upstream.
//!
//! ## Why the RESOLVED ENVIRONMENT is its own key field
//!
//! `import_manifest` records the import NAMES a candidate is elaborated against
//! (`["Mathlib"]`). Names are not revisions. `checker_identity` records binary
//! PATHS, the runner tag, the project root, the requested toolchain string, and a
//! manual `cache_epoch`. Paths are not revisions either.
//!
//! So the pre-`v4` key was blind to the one thing that actually decides what
//! `import Mathlib` MEANS: the library revision behind that name at that path.
//! Update Mathlib in place, at the same path, under the same Lean, and every
//! previous key field is byte-identical while the elaboration environment has
//! changed underneath. A verdict earned against the old library is then replayed
//! as if it had been earned against the new one. That is a STALE GREEN, and
//! `THEOREMATA_CHECKER_CACHE_EPOCH` is no defence: a manual escape hatch is
//! invalidation, not detection, and it fires only when a human already knew.
//!
//! [`EnvironmentFingerprint`] closes that by hashing what was actually RESOLVED
//! rather than what was requested: for a Lake project the `lake-manifest.json`
//! content (which is exactly the per-package revision record Lake wrote when it
//! resolved the dependency set), the `lean-toolchain` pin beside it, and the
//! canonical project path.
//!
//! ### Failing to resolve is NOT permission to reuse
//!
//! When the environment cannot be determined, the cache is NOT used for that
//! entry: [`cache_key`] returns `None`, so a lookup misses and an insertion is
//! refused. A miss costs one redundant verification; a hit on an unknown
//! environment costs the soundness of the gate. We always prefer the miss.
//! The unresolved state still carries an explicit REASON and is still mixed into
//! [`key_material`], so "this project declares no dependencies" and "we did not
//! look" remain distinguishable to anyone auditing the key derivation, even
//! though neither of the latter is reusable.
//!
//! Systems other than Lean have their own notions of a dependency set
//! (`_CoqProject`, session `ROOT`, `.agda-lib`, a `set.mm` database). None of
//! them is resolved here yet, so they report `Unresolved` explicitly rather than
//! silently omitting the field: the cache goes quiet for those live backends
//! instead of going wrong.
//!
//! ## What is stored alongside the verdict
//!
//! [`StatementIdentity`] pins the statement the verdict is about. Today this
//! layer only sees SOURCE TEXT: the key is built BEFORE `FormalBackend::verify`
//! runs, and the report that comes back carries a source-level parsed signature
//! and an axiom list, not a kernel-level type. So `elaborated` is recorded as
//! `None`, meaning UNAVAILABLE, never faked from the source text, which would
//! make a later staleness discriminator confidently wrong. The moment a backend
//! publishes an elaborated form under
//! [`ELABORATED_STATEMENT_DETAIL_KEY`], it is captured with its provenance.
//!
//! Contrast [`goal_cache`](crate::goal_cache),
//! which is a project-scoped cache of goal → proof text. This cache only skips a
//! repeated verification of the same proof under the same gate.
//!
//! ## The gate invariant (do not weaken it)
//!
//! The cache stores the OUTCOME of the gate; it never bypasses or manufactures a
//! gate decision. Two properties keep an UNVERIFIED result from ever being read as
//! verified:
//!
//! 1. **Live successes only, authoritatively.**
//!    [`insert_verified`](CheckerCache::insert_verified) is the sole insertion
//!    path. It requires every gate bit plus `live`; mock reports and partial or
//!    failed reports are refused.
//! 2. **No negative caching.** A *failed* attempt is NOT cached. Caching a failure
//!    would be unsound as a "do not try" signal — a goal that one proof strategy
//!    failed to discharge may well be provable by another — so a miss (whether the
//!    goal was never seen, or a prior attempt failed) always forces the caller back
//!    through the real gate. Omitting negative caching is both simpler and strictly
//!    safer than an advisory negative cache, so we omit it entirely.
//!
//! A cache hit defensively rechecks those report bits before returning the stored
//! report. The cache adds no trust; it only avoids identical recomputation.
//!
//! ## Key stability & determinism
//!
//! The key is a SHA-256 over length-framed, domain-separated fields (the same
//! anti-ambiguity framing as [`proof_import::content_id`](crate::proof_import)),
//! computed over whitespace-normalized statement/context inputs so cosmetically
//! different restatements (`"a  +\tb"` vs `"a + b"`) share a key. Proof source,
//! checker identity, and policy are hashed byte-for-byte; changing any of them is
//! a mandatory miss.
//! Context/hypotheses are treated as an ORDERED list — we do **not** reorder them —
//! so two different hypothesis orderings yield different keys. That errs toward
//! false MISSES (a redundant re-verification), never false HITS (an unsound reuse);
//! order-invariant goal matching is the (soundness-audited) job of
//! [`subsumption`](crate::subsumption)/[`goal_cache`](crate::goal_cache), not of
//! this cheap string-level memo. No wall-clock reads, no RNG: the same inputs
//! always hash to the same key.

use crate::prover::formal::FormalSystem;
use crate::prover::model::VerificationReport;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

/// Domain-separation tag mixed in first so this key space can never collide with
/// another SHA-256 pre-image in the crate.
/// Bumped to `v3` when the import manifest joined the key: a v2 key and a v3 key
/// for the same inputs must not collide, and no v2-era entry may be read as v3.
/// Bumped to `v4` when the RESOLVED ENVIRONMENT joined the key. Every v3 entry
/// was derived without any library-revision input, so no v3 entry may be read as
/// v4: a key-schema change that silently reuses pre-schema entries would leave
/// exactly the stale greens this change exists to stop.
const DOMAIN: &[u8] = b"theoremata.checker_cache.v4";

/// The file Lake writes when it RESOLVES a dependency set. Its content is the
/// per-package revision record, which is why it is what we hash.
const LAKE_MANIFEST_FILE: &str = "lake-manifest.json";

/// The single-line toolchain pin beside a Lake manifest.
const LEAN_TOOLCHAIN_FILE: &str = "lean-toolchain";

/// Absorb one length-framed, tagged field into the hasher. Framing every field
/// with its byte length makes the pre-image UNAMBIGUOUS: no choice of
/// system/statement/hypothesis text can be re-split into different fields that
/// collide (the classic concatenation ambiguity). Mirrors
/// [`proof_import`](crate::proof_import)'s `absorb`.
fn absorb(hasher: &mut Sha256, tag: &[u8], data: &[u8]) {
    hasher.update((tag.len() as u64).to_be_bytes());
    hasher.update(tag);
    hasher.update((data.len() as u64).to_be_bytes());
    hasher.update(data);
}

/// Lowercase hex of a digest (the crate's shared idiom, re-inlined per module).
fn hex_lower(bytes: impl AsRef<[u8]>) -> String {
    use std::fmt::Write as _;
    let bytes = bytes.as_ref();
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Deterministic whitespace normalization: trim, and collapse every run of ASCII
/// whitespace (spaces, tabs, newlines) to a single space. This is the ONLY
/// canonicalization applied — it is content-preserving, so it can only merge
/// cosmetically-equivalent restatements, never conflate genuinely different ones.
fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Normalize file content for hashing: strip UTF-8 BOM and carriage returns, and
/// trim trailing whitespace on the whole blob. This is deliberately minimal:
/// only line-ending and BOM noise, which differs between checkouts of the SAME
/// bytes, is removed. Any real content change still changes the digest, so this
/// can produce extra MISSES but never a false HIT.
fn normalize_file(content: &str) -> String {
    content
        .trim_start_matches('\u{feff}')
        .replace('\r', "")
        .trim_end()
        .to_string()
}

/// What the verification was ACTUALLY elaborated against, as opposed to what the
/// caller asked for.
///
/// The distinction is the whole point. `import Mathlib` is a request;
/// `lake-manifest.json` records what that request resolved to. Only the second
/// changes when a library is updated in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvironmentFingerprint {
    /// We determined the environment. `digest` is a SHA-256 over sorted,
    /// normalized content, so it is stable for an unchanged environment.
    Resolved {
        /// What kind of environment this is (`lake`, `mock`, ...). Part of the
        /// hashed material so two kinds can never share a digest by accident.
        kind: String,
        /// Hex SHA-256 over the resolved content.
        digest: String,
        /// Human-readable provenance for logs. NOT hashed: it exists so an
        /// operator can see which files were read, and changing the wording of a
        /// log line must not invalidate a cache.
        detail: String,
    },
    /// We could NOT determine the environment. A key carrying this is not
    /// reusable at all; see [`cache_key`].
    Unresolved {
        /// Why resolution failed, recorded verbatim so "declares nothing" and
        /// "we did not look" stay distinguishable.
        reason: String,
    },
}

impl EnvironmentFingerprint {
    /// Build a resolved fingerprint from named parts. Parts are sorted by tag
    /// before hashing so caller-side ordering can never make the digest wobble
    /// run to run; a fingerprint that is not deterministic makes the cache
    /// useless.
    pub fn from_parts(kind: &str, detail: &str, parts: &[(&str, String)]) -> Self {
        let mut sorted: Vec<(&str, &str)> =
            parts.iter().map(|(t, v)| (*t, v.as_str())).collect();
        sorted.sort_unstable();
        let mut hasher = Sha256::new();
        absorb(&mut hasher, b"env.kind", kind.as_bytes());
        hasher.update((sorted.len() as u64).to_be_bytes());
        for (tag, value) in sorted {
            absorb(&mut hasher, tag.as_bytes(), value.as_bytes());
        }
        Self::Resolved {
            kind: kind.to_string(),
            digest: hex_lower(hasher.finalize()),
            detail: detail.to_string(),
        }
    }

    /// An environment we failed to determine. `reason` should say what was
    /// missing, not merely that something was.
    pub fn unresolved(reason: impl Into<String>) -> Self {
        Self::Unresolved {
            reason: reason.into(),
        }
    }

    /// Whether this environment may back a cache entry at all.
    pub fn is_resolved(&self) -> bool {
        matches!(self, Self::Resolved { .. })
    }

    /// The field mixed into the cache key. Both states are representable so the
    /// key derivation itself distinguishes them; only the resolved state is
    /// USABLE, which [`cache_key`] enforces separately.
    pub fn key_field(&self) -> String {
        match self {
            Self::Resolved { kind, digest, .. } => format!("resolved:{kind}:{digest}"),
            Self::Unresolved { reason } => format!("unresolved:{reason}"),
        }
    }

    /// One-line description for events and logs.
    pub fn describe(&self) -> String {
        match self {
            Self::Resolved {
                kind,
                digest,
                detail,
            } => format!("resolved {kind} [{}] {detail}", &digest[..16.min(digest.len())]),
            Self::Unresolved { reason } => format!("unresolved: {reason}"),
        }
    }

    /// Resolve a Lean environment from a Lake project root.
    ///
    /// We hash the manifest CONTENT rather than a mtime or a version string: the
    /// manifest is the record Lake itself wrote when it resolved every package to
    /// a concrete revision, so its content changes exactly when the library a
    /// proof was elaborated against changes. The per-package revisions are also
    /// extracted and hashed separately, so a manifest that is merely reformatted
    /// still moves the digest (safe: an extra miss) while a genuine revision bump
    /// moves it for a reason we can name in `detail`.
    ///
    /// Every failure path returns [`EnvironmentFingerprint::Unresolved`], which
    /// disables the cache for that entry. Not knowing which Mathlib is on disk is
    /// never a licence to reuse a green earned against some other one.
    pub fn resolve_lake_project(root: Option<&Path>) -> Self {
        let Some(root) = root else {
            // No project means `import Mathlib` resolves through whatever
            // LEAN_PATH/elan happens to offer, which we cannot pin from here.
            return Self::unresolved(
                "lean: no lake project configured, so no dependency revision can be pinned",
            );
        };
        if !root.is_dir() {
            return Self::unresolved(format!(
                "lean: configured lake project {} is not a directory on disk",
                root.display()
            ));
        }
        let manifest_path = root.join(LAKE_MANIFEST_FILE);
        let manifest = match std::fs::read_to_string(&manifest_path) {
            Ok(text) => normalize_file(&text),
            Err(err) => {
                return Self::unresolved(format!(
                    "lean: cannot read {}: {err}",
                    manifest_path.display()
                ))
            }
        };

        // Per-package revisions, sorted by package name. `unparsed` is recorded
        // explicitly when the manifest is not the JSON shape we expect, so the
        // key never silently degrades to "no packages".
        let packages = match serde_json::from_str::<Value>(&manifest) {
            Ok(value) => lake_package_pins(&value),
            Err(err) => vec![format!("unparsed-manifest: {err}")],
        };

        // The toolchain pin beside the manifest. ABSENT is a determinable fact
        // (the file is not there) and is recorded as such; it is not the same as
        // an unreadable project, which is unresolvable above.
        let toolchain_path = root.join(LEAN_TOOLCHAIN_FILE);
        let toolchain = std::fs::read_to_string(&toolchain_path)
            .map(|t| normalize_file(&t))
            .unwrap_or_else(|_| "absent".to_string());

        // Canonicalize so two spellings of one project (symlink, relative path)
        // do not look like two environments. Falling back to the display form
        // keeps this total.
        let canonical = std::fs::canonicalize(root)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| root.display().to_string());

        let detail = format!(
            "{} ({} package pin(s)), toolchain {}",
            manifest_path.display(),
            packages.len(),
            toolchain
        );
        Self::from_parts(
            "lake",
            &detail,
            &[
                ("env.root", canonical),
                ("env.manifest", manifest),
                ("env.packages", packages.join("\n")),
                ("env.toolchain", toolchain),
            ],
        )
    }

    /// The environment a mock backend elaborates against: none. A mock consults
    /// no library, so this is genuinely RESOLVED rather than unknown. (Mock
    /// reports are refused by [`CheckerCache::insert_verified`] anyway, so this
    /// state never actually backs a stored verdict; it exists so an offline run
    /// does not report a resolution failure it does not have.)
    pub fn mock() -> Self {
        Self::from_parts(
            "mock",
            "mock backend consults no library",
            &[("env.mock", "no-library".to_string())],
        )
    }

    /// Resolve the environment for one system.
    ///
    /// Only Lean has a dependency-set resolver here. Rocq (`_CoqProject`),
    /// Isabelle (session `ROOT`), Agda (`.agda-lib`), Metamath (the database
    /// file) and Candle each have one in principle, and each is deliberately
    /// reported as UNRESOLVED until it is implemented and tested. The cost is a
    /// quiet cache on those live backends; the alternative is a cache that
    /// cannot tell one library revision from another, which is the bug.
    pub fn resolve(system: FormalSystem, is_mock: bool, lean_project: Option<&Path>) -> Self {
        if is_mock {
            return Self::mock();
        }
        match system {
            FormalSystem::Lean => Self::resolve_lake_project(lean_project),
            other => Self::unresolved(format!(
                "{}: no dependency-set resolver implemented, environment not inspected",
                other.as_str()
            )),
        }
    }
}

/// Extract `name@rev` style pins from a parsed Lake manifest, sorted so the order
/// Lake happened to write them in cannot change the digest.
fn lake_package_pins(manifest: &Value) -> Vec<String> {
    let Some(packages) = manifest.get("packages").and_then(Value::as_array) else {
        // A manifest with no `packages` array is a real, determinable state
        // ("this project pulls in nothing"), distinct from an unparsable one.
        return vec!["no-packages-array".to_string()];
    };
    let mut pins: Vec<String> = packages
        .iter()
        .map(|pkg| {
            let field = |name: &str| {
                pkg.get(name)
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string()
            };
            format!(
                "{}|rev={}|inputRev={}|url={}|dir={}",
                field("name"),
                field("rev"),
                field("inputRev"),
                field("url"),
                field("dir"),
            )
        })
        .collect();
    pins.sort();
    pins
}

/// The `VerificationReport::detail` key under which a backend may publish the
/// checker's own elaborated form of the accepted statement.
///
/// Nothing writes it today. It is named here so the capture path exists and so a
/// backend that gains the ability (a REPL that can pretty-print the elaborated
/// type, or hand back a term hash) has one agreed place to put it.
pub const ELABORATED_STATEMENT_DETAIL_KEY: &str = "elaborated_statement";

/// The checker's own identity for a statement, when it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElaboratedStatement {
    /// Where this came from, e.g. `lean.repl.elaborated_type`. Recorded so a
    /// consumer never has to guess how much the value is worth.
    pub provenance: String,
    /// Hex SHA-256 of the normalized elaborated form.
    pub digest: String,
}

/// What we can honestly say about the statement a stored verdict is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementIdentity {
    /// Digest of the normalized SOURCE statement text. Always available.
    pub source_digest: String,
    /// The ELABORATED identity, when the checker supplied one.
    ///
    /// `None` means UNAVAILABLE, not "empty". At this layer it is always `None`
    /// today: the key is built before the checker runs, and the report that comes
    /// back carries a source-level parsed signature plus an axiom list, neither
    /// of which is an elaborated type. Deriving a fake value from the source text
    /// would give a later staleness discriminator something confident and wrong
    /// to compare, so we record the absence instead.
    pub elaborated: Option<ElaboratedStatement>,
}

impl StatementIdentity {
    /// Capture whatever identity is available for one verified input.
    fn capture(input: &VerificationCacheKey<'_>, report: &VerificationReport) -> Self {
        let mut hasher = Sha256::new();
        absorb(&mut hasher, b"stmt.source", normalize(input.canonical_statement).as_bytes());
        let source_digest = hex_lower(hasher.finalize());
        Self {
            source_digest,
            elaborated: elaborated_from_detail(&report.detail),
        }
    }
}

/// Read an elaborated form out of a report's `detail`, if a backend published
/// one. Anything malformed yields `None`: an unreadable field is no evidence.
fn elaborated_from_detail(detail: &Value) -> Option<ElaboratedStatement> {
    let node = detail.get(ELABORATED_STATEMENT_DETAIL_KEY)?;
    let form = node.get("form").and_then(Value::as_str)?;
    let provenance = node
        .get("provenance")
        .and_then(Value::as_str)
        .unwrap_or("unattributed")
        .to_string();
    let mut hasher = Sha256::new();
    absorb(&mut hasher, b"stmt.elaborated", normalize(form).as_bytes());
    Some(ElaboratedStatement {
        provenance,
        digest: hex_lower(hasher.finalize()),
    })
}

/// One stored verdict plus the identity of the statement it is about.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    /// The gate verdict, exactly as the gate produced it.
    pub report: VerificationReport,
    /// What we could pin about the statement. Informational today; it is the
    /// input a later staleness discriminator will compare against.
    pub statement: StatementIdentity,
}

/// Every input that can change a verification verdict.
#[derive(Debug, Clone, Copy)]
pub struct VerificationCacheKey<'a> {
    pub system: FormalSystem,
    /// Canonical theorem statement. Whitespace is normalized before hashing.
    pub canonical_statement: &'a str,
    /// Ordered local hypotheses/import context. Order is intentionally preserved.
    pub ordered_context: &'a [String],
    /// Exact candidate source. This is never normalized.
    pub proof_source: &'a str,
    /// The per-problem IMPORT CLOSURE this candidate is elaborated against, in
    /// the order the backend applies it (`["Mathlib", "Mathlib.Tactic"]`).
    ///
    /// Order is preserved and each entry is hashed whitespace-normalized. Pass
    /// the manifest the backend will ACTUALLY use, not the one the model asked
    /// for. An empty slice means "no imports beyond what `proof_source` itself
    /// declares" — pass `&[]` only when that is literally true, because an empty
    /// manifest and a populated one are different keys, which is the point.
    ///
    /// See the module docs for why `checker_identity` does not cover this.
    pub import_manifest: &'a [String],
    /// The RESOLVED environment those imports actually name: library revisions,
    /// toolchain pin, project path. This is what makes an in-place library update
    /// a mandatory miss. An [`EnvironmentFingerprint::Unresolved`] here makes the
    /// whole key unusable rather than merely different.
    pub environment: &'a EnvironmentFingerprint,
    /// Backend/toolchain/corpus identity, including live vs mock mode.
    pub checker_identity: &'a str,
    /// Gate/policy fingerprint (axiom whitelist, hardening switches, gate epoch).
    pub policy_fingerprint: &'a str,
}

/// Stable hex SHA-256 over a complete verification input, computed REGARDLESS of
/// whether the environment resolved.
///
/// Exposed so key derivation can be audited directly, including the fact that an
/// unresolved environment produces different key material from a resolved one and
/// from a differently-unresolved one. This is derivation, not permission: use
/// [`cache_key`] for anything that reads or writes the cache.
pub fn key_material(input: &VerificationCacheKey<'_>) -> String {
    let mut hasher = Sha256::new();
    absorb(&mut hasher, DOMAIN, &[]);
    absorb(&mut hasher, b"system", input.system.as_str().as_bytes());
    absorb(
        &mut hasher,
        b"stmt",
        normalize(input.canonical_statement).as_bytes(),
    );
    // Frame the hypothesis count so `[h]` with a long `h` cannot look like `[h, g]`.
    hasher.update((input.ordered_context.len() as u64).to_be_bytes());
    for hyp in input.ordered_context {
        absorb(&mut hasher, b"hyp", normalize(hyp).as_bytes());
    }
    absorb(&mut hasher, b"proof", input.proof_source.as_bytes());
    // Frame the import count for the same anti-ambiguity reason as the context.
    hasher.update((input.import_manifest.len() as u64).to_be_bytes());
    for import in input.import_manifest {
        absorb(&mut hasher, b"import", normalize(import).as_bytes());
    }
    absorb(
        &mut hasher,
        b"environment",
        input.environment.key_field().as_bytes(),
    );
    absorb(&mut hasher, b"checker", input.checker_identity.as_bytes());
    absorb(&mut hasher, b"policy", input.policy_fingerprint.as_bytes());
    hex_lower(hasher.finalize())
}

/// The USABLE cache key for a verification input, or `None` when there is none.
///
/// `None` is returned exactly when the resolved environment is unknown. Callers
/// must treat that as a MISS and go back through the real gate: a redundant
/// verification is cheap, and reusing a green whose environment we cannot name is
/// the failure this module exists to prevent.
pub fn cache_key(input: &VerificationCacheKey<'_>) -> Option<String> {
    if !input.environment.is_resolved() {
        return None;
    }
    Some(key_material(input))
}

fn report_is_live_success(report: &VerificationReport) -> bool {
    report.live
        && report.lexically_verified
        && report.axioms_clean
        && report.statement_preserved
        && report.lexical_clean
        && report.hardening_clean == Some(true)
}

/// In-memory, hash-consed cache of live gate verdicts for exact proof inputs.
///
/// Interior mutability via [`RefCell`] (matching the single-threaded interior
/// mutability used elsewhere in `proving/`, e.g. `refine_ops`/`sketch`) so a
/// borrowed `&CheckerCache` can be threaded through a candidate loop and both
/// looked up and populated without a `&mut` at every call site. It is deliberately
/// NOT `Sync`; a threaded search that needs to share one cache should wrap it in a
/// `Mutex` at that boundary rather than complicate this memo.
#[derive(Debug, Default)]
pub struct CheckerCache {
    entries: RefCell<HashMap<String, CacheEntry>>,
}

impl CheckerCache {
    /// A fresh, empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of distinct verified inputs currently cached.
    pub fn len(&self) -> usize {
        self.entries.borrow().len()
    }

    /// Whether the cache holds no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.borrow().is_empty()
    }

    /// Look up the cached gate verdict for a complete verification input.
    /// Returns a clone of the stored [`VerificationReport`] on a hit, or `None` on
    /// a miss. A `None` MUST send the caller back through the real gate — the cache
    /// never stands in for the gate on a miss.
    /// An unresolved environment is a guaranteed miss: `cache_key` yields no key
    /// at all, so there is nothing to look up.
    pub fn get(&self, input: &VerificationCacheKey<'_>) -> Option<VerificationReport> {
        self.get_entry(input).map(|entry| entry.report)
    }

    /// Like [`get`](Self::get) but also returns the [`StatementIdentity`] stored
    /// with the verdict.
    pub fn get_entry(&self, input: &VerificationCacheKey<'_>) -> Option<CacheEntry> {
        let key = cache_key(input)?;
        self.entries
            .borrow()
            .get(&key)
            .filter(|entry| report_is_live_success(&entry.report))
            .cloned()
    }

    /// Whether this exact verification input has a cached live verdict.
    pub fn contains_verified(&self, input: &VerificationCacheKey<'_>) -> bool {
        self.get(input).is_some()
    }

    /// Record that one complete verification input cleared the live gate. This is
    /// the sole insertion path and stores live successes only.
    ///
    /// The caller is expected to offer only a report the gate actually accepted; as
    /// a defensive floor this requires every gate bit and `live`, returning
    /// `false` without storing otherwise. Mock reports are never cached.
    /// Note: this floor is a guard against mis-wiring, NOT a certification decision
    /// — deciding what counts as "verified" remains the gate's job, upstream.
    ///
    /// It also refuses any input whose environment did not resolve, returning
    /// `false` without storing.
    ///
    /// Idempotent on the key: re-inserting the same input overwrites with the newer
    /// (equally-verified) report and does not grow the cache.
    pub fn insert_verified(
        &self,
        input: &VerificationCacheKey<'_>,
        report: VerificationReport,
    ) -> bool {
        if !report_is_live_success(&report) {
            return false;
        }
        // No usable key means no entry. Storing under a key that does not name
        // the environment is how a later run gets served a stale green.
        let Some(key) = cache_key(input) else {
            return false;
        };
        let statement = StatementIdentity::capture(input, &report);
        self.entries
            .borrow_mut()
            .insert(key, CacheEntry { report, statement });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A report that reflects a genuine gate pass.
    fn verified_report() -> VerificationReport {
        VerificationReport {
            lexically_verified: true,
            axioms_clean: true,
            statement_preserved: true,
            lexical_clean: true,
            hardening_clean: Some(true),
            live: true,
            detail: json!({"marker": "unit-test"}),
        }
    }

    /// A report that did NOT pass the lexical screen (a would-be failure).
    fn failed_report() -> VerificationReport {
        VerificationReport {
            lexically_verified: false,
            axioms_clean: false,
            statement_preserved: false,
            lexical_clean: false,
            hardening_clean: Some(false),
            live: true,
            detail: json!({"marker": "failed"}),
        }
    }

    fn mock_report() -> VerificationReport {
        VerificationReport {
            live: false,
            detail: json!({"marker": "mock"}),
            ..verified_report()
        }
    }

    fn ctx(hyps: &[&str]) -> Vec<String> {
        hyps.iter().map(|s| s.to_string()).collect()
    }

    /// A fixed RESOLVED environment for the tests that are about other fields.
    fn env() -> EnvironmentFingerprint {
        EnvironmentFingerprint::from_parts(
            "test",
            "unit-test environment",
            &[("env.fixture", "one".to_string())],
        )
    }

    /// Write a minimal Lake project and return its root.
    fn lake_project(dir: &Path, mathlib_rev: &str, toolchain: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("lake-manifest.json"),
            format!(
                r#"{{"version": "1.1.0", "packages": [
                     {{"name": "mathlib", "rev": "{mathlib_rev}",
                      "inputRev": "master", "url": "https://example/mathlib"}}
                   ]}}"#
            ),
        )
        .unwrap();
        std::fs::write(dir.join("lean-toolchain"), toolchain).unwrap();
    }

    /// A scratch directory unique to one test, cleaned up on entry.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("theoremata-checker-cache-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// No-import shorthand used by the pre-existing tests, which are about the
    /// other key fields.
    fn input<'a>(
        system: FormalSystem,
        statement: &'a str,
        context: &'a [String],
        proof: &'a str,
        checker: &'a str,
        policy: &'a str,
    ) -> VerificationCacheKey<'a> {
        input_with_imports(system, statement, context, &[], proof, checker, policy)
    }

    #[allow(clippy::too_many_arguments)]
    fn input_with_imports<'a>(
        system: FormalSystem,
        statement: &'a str,
        context: &'a [String],
        imports: &'a [String],
        proof: &'a str,
        checker: &'a str,
        policy: &'a str,
    ) -> VerificationCacheKey<'a> {
        VerificationCacheKey {
            system,
            canonical_statement: statement,
            ordered_context: context,
            proof_source: proof,
            import_manifest: imports,
            environment: env_ref(),
            checker_identity: checker,
            policy_fingerprint: policy,
        }
    }

    /// A process-lifetime borrow of [`env`], since a key borrows its environment.
    fn env_ref() -> &'static EnvironmentFingerprint {
        static CELL: std::sync::OnceLock<EnvironmentFingerprint> = std::sync::OnceLock::new();
        CELL.get_or_init(env)
    }

    #[test]
    fn miss_then_hit_returns_the_stored_verdict() {
        let cache = CheckerCache::new();
        let hyps = ctx(&["P x"]);
        let key = input(
            FormalSystem::Lean,
            "P x ⊢ Q x",
            &hyps,
            "theorem q : Q x := proof",
            "lean:live:v4.19",
            "gate-v2:axioms=[]",
        );

        assert!(cache.get(&key).is_none());
        assert!(!cache.contains_verified(&key));

        assert!(cache.insert_verified(&key, verified_report()));
        let got = cache.get(&key).expect("verified input must hit");
        assert!(got.lexically_verified);
        // The very report that was stored comes back (compared structurally, as
        // VerificationReport is not PartialEq).
        assert_eq!(
            serde_json::to_value(&got).unwrap(),
            serde_json::to_value(&verified_report()).unwrap(),
        );
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn key_is_stable_across_whitespace_equivalent_inputs() {
        let ctx_a = ctx(&["x  >  0"]);
        let ctx_b = ctx(&["x > 0"]);
        let a = input(
            FormalSystem::Lean,
            "a  +\t b   =  b + a",
            &ctx_a,
            "proof",
            "checker",
            "policy",
        );
        let b = input(
            FormalSystem::Lean,
            "a + b = b + a",
            &ctx_b,
            "proof",
            "checker",
            "policy",
        );
        assert_eq!(cache_key(&a), cache_key(&b));

        let cache = CheckerCache::new();
        assert!(cache.insert_verified(&a, verified_report()));
        assert!(cache.get(&b).is_some());
    }

    #[test]
    fn every_trust_relevant_input_is_keyed() {
        let ab = ctx(&["A", "B"]);
        let ba = ctx(&["B", "A"]);
        let base = input(
            FormalSystem::Lean,
            "⊢ G",
            &ab,
            "proof-one",
            "lean:live:v4.19",
            "policy-one",
        );
        let base_hash = cache_key(&base);
        let different = [
            input(
                FormalSystem::Rocq,
                "⊢ G",
                &ab,
                "proof-one",
                "lean:live:v4.19",
                "policy-one",
            ),
            input(
                FormalSystem::Lean,
                "⊢ H",
                &ab,
                "proof-one",
                "lean:live:v4.19",
                "policy-one",
            ),
            input(
                FormalSystem::Lean,
                "⊢ G",
                &ba,
                "proof-one",
                "lean:live:v4.19",
                "policy-one",
            ),
            input(
                FormalSystem::Lean,
                "⊢ G",
                &ab,
                "proof-two",
                "lean:live:v4.19",
                "policy-one",
            ),
            input(
                FormalSystem::Lean,
                "⊢ G",
                &ab,
                "proof-one",
                "lean:mock",
                "policy-one",
            ),
            input(
                FormalSystem::Lean,
                "⊢ G",
                &ab,
                "proof-one",
                "lean:live:v4.19",
                "policy-two",
            ),
        ];
        for changed in &different {
            assert_ne!(base_hash, cache_key(changed));
        }

        let cache = CheckerCache::new();
        assert!(cache.insert_verified(&base, verified_report()));
        for changed in &different {
            assert!(cache.get(changed).is_none(), "changed input must be a miss");
        }
    }

    #[test]
    fn failures_and_mock_reports_are_never_cached() {
        let hyps = ctx(&["h"]);
        let live_key = input(
            FormalSystem::Lean,
            "⊢ hard",
            &hyps,
            "proof",
            "lean:live",
            "policy",
        );
        let mock_key = input(
            FormalSystem::Lean,
            "⊢ hard",
            &hyps,
            "proof",
            "lean:mock",
            "policy",
        );
        let cache = CheckerCache::new();
        assert!(!cache.insert_verified(&live_key, failed_report()));
        assert!(!cache.insert_verified(&mock_key, mock_report()));
        assert!(cache.is_empty());
        assert!(cache.get(&live_key).is_none());
        assert!(cache.get(&mock_key).is_none());
    }

    #[test]
    fn insert_is_idempotent_on_the_complete_key() {
        let hyps = ctx(&["P x"]);
        let key = input(
            FormalSystem::Lean,
            "P x ⊢ Q x",
            &hyps,
            "proof",
            "checker",
            "policy",
        );
        let cache = CheckerCache::new();
        assert!(cache.insert_verified(&key, verified_report()));
        assert!(cache.insert_verified(&key, verified_report()));
        assert_eq!(cache.len(), 1);
    }

    /// The environment a proof was checked in is part of the key. This is the
    /// mined "unvalidated import list bakes in `axiom cheat : False`" failure:
    /// everything else about the request is identical, so without this field the
    /// poisoned and the clean environment would share one cache entry.
    #[test]
    fn changing_the_import_manifest_changes_the_key() {
        let hyps = ctx(&[]);
        let clean = ctx(&["Mathlib"]);
        let poisoned = ctx(&["Mathlib\naxiom cheat : False"]);
        let extra = ctx(&["Mathlib", "Mathlib.Tactic"]);
        let reordered = ctx(&["Mathlib.Tactic", "Mathlib"]);

        let base = input_with_imports(
            FormalSystem::Lean,
            "⊢ G",
            &hyps,
            &clean,
            "theorem g : G := proof",
            "lean:live:v4.19",
            "policy",
        );
        let base_hash = cache_key(&base);

        for changed in [&poisoned, &extra, &reordered, &hyps] {
            let other = input_with_imports(
                FormalSystem::Lean,
                "⊢ G",
                &hyps,
                changed,
                "theorem g : G := proof",
                "lean:live:v4.19",
                "policy",
            );
            assert_ne!(
                base_hash,
                cache_key(&other),
                "a different import closure must be a different key: {changed:?}"
            );
        }

        // And it is a real cache MISS, not merely a different hash: a verdict
        // earned in the clean environment is never served for the poisoned one.
        let cache = CheckerCache::new();
        assert!(cache.insert_verified(&base, verified_report()));
        let poisoned_key = input_with_imports(
            FormalSystem::Lean,
            "⊢ G",
            &hyps,
            &poisoned,
            "theorem g : G := proof",
            "lean:live:v4.19",
            "policy",
        );
        assert!(
            cache.get(&poisoned_key).is_none(),
            "a proof checked against a clean import closure must NOT be reused \
             for one checked against a poisoned closure"
        );
        // The converse direction is equally required.
        assert!(cache.insert_verified(&poisoned_key, verified_report()));
        assert!(cache.get(&base).is_some());
        assert_eq!(cache.len(), 2, "the two environments occupy distinct slots");
    }

    /// Imports are normalized like the other text fields, so cosmetic whitespace
    /// still shares a key.
    #[test]
    fn import_manifest_is_whitespace_normalized() {
        let hyps = ctx(&[]);
        let a = ctx(&["  Mathlib.Tactic  "]);
        let b = ctx(&["Mathlib.Tactic"]);
        let ka = input_with_imports(FormalSystem::Lean, "⊢ G", &hyps, &a, "p", "c", "pol");
        let kb = input_with_imports(FormalSystem::Lean, "⊢ G", &hyps, &b, "p", "c", "pol");
        assert_eq!(cache_key(&ka), cache_key(&kb));
    }

    #[test]
    fn key_is_a_stable_sha256_constant() {
        let hyps = ctx(&["P x"]);
        let key = input(
            FormalSystem::Lean,
            "P x ⊢ Q x",
            &hyps,
            "proof",
            "checker",
            "policy",
        );
        let k1 = cache_key(&key).expect("a resolved environment yields a key");
        let k2 = cache_key(&key).expect("a resolved environment yields a key");
        assert_eq!(k1, k2);
        assert_eq!(k1.len(), 64, "SHA-256 hex is 64 chars");
        assert!(k1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// THE HEADLINE. Import names, binary paths, proof text, policy: all
    /// identical. Only the lake-manifest on disk changed, exactly as an in-place
    /// `lake update` of Mathlib would change it. That must be a different key and
    /// a real cache MISS.
    #[test]
    fn a_changed_lake_manifest_changes_the_key_with_everything_else_identical() {
        let root = scratch("manifest-bump");
        lake_project(&root, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "leanprover/lean4:v4.19.0");
        let before = EnvironmentFingerprint::resolve_lake_project(Some(&root));
        assert!(before.is_resolved());

        let hyps = ctx(&[]);
        let imports = ctx(&["Mathlib"]);
        let key_before = VerificationCacheKey {
            system: FormalSystem::Lean,
            canonical_statement: "⊢ G",
            ordered_context: &hyps,
            proof_source: "theorem g : G := by simp",
            import_manifest: &imports,
            environment: &before,
            checker_identity: "lean:live:/usr/bin/lean",
            policy_fingerprint: "gate-v2",
        };

        let cache = CheckerCache::new();
        assert!(cache.insert_verified(&key_before, verified_report()));
        assert!(cache.get(&key_before).is_some());

        // Mathlib updated in place: same path, same Lean binary, same imports.
        lake_project(&root, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "leanprover/lean4:v4.19.0");
        let after = EnvironmentFingerprint::resolve_lake_project(Some(&root));
        assert!(after.is_resolved());
        assert_ne!(before, after, "a new manifest is a new environment");

        let key_after = VerificationCacheKey {
            environment: &after,
            ..key_before
        };
        assert_ne!(cache_key(&key_before), cache_key(&key_after));
        assert!(
            cache.get(&key_after).is_none(),
            "a verdict earned against the old Mathlib must NOT be served for the new one"
        );

        // And the toolchain pin is keyed too, independently of the manifest.
        lake_project(&root, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "leanprover/lean4:v4.20.0");
        let newer_toolchain = EnvironmentFingerprint::resolve_lake_project(Some(&root));
        assert_ne!(after, newer_toolchain);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The same unchanged project resolves to the same fingerprint every time.
    /// A fingerprint that wobbles run to run makes the cache useless.
    #[test]
    fn an_unchanged_environment_resolves_identically() {
        let root = scratch("stable");
        lake_project(&root, "cccccccccccccccccccccccccccccccccccccccc", "leanprover/lean4:v4.19.0");
        let a = EnvironmentFingerprint::resolve_lake_project(Some(&root));
        let b = EnvironmentFingerprint::resolve_lake_project(Some(&root));
        assert_eq!(a, b);

        // Rewriting the same content with CRLF line endings is not a change.
        let manifest = std::fs::read_to_string(root.join("lake-manifest.json")).unwrap();
        std::fs::write(
            root.join("lake-manifest.json"),
            manifest.replace('\n', "\r\n"),
        )
        .unwrap();
        assert_eq!(a, EnvironmentFingerprint::resolve_lake_project(Some(&root)));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An environment we could not resolve yields NO usable key, so nothing can
    /// be stored under it and nothing can be read back.
    #[test]
    fn an_unresolvable_environment_is_never_reusable() {
        let missing = scratch("absent").join("no-such-project");
        let unresolved = EnvironmentFingerprint::resolve_lake_project(Some(&missing));
        assert!(!unresolved.is_resolved(), "an absent project cannot resolve");
        assert!(!EnvironmentFingerprint::resolve_lake_project(None).is_resolved());
        // A project directory with no manifest is equally unresolvable: we know
        // where to look and found no dependency record.
        let bare = scratch("bare");
        assert!(!EnvironmentFingerprint::resolve_lake_project(Some(&bare)).is_resolved());
        // Every non-Lean live system reports unresolved rather than omitting.
        for system in [
            FormalSystem::Rocq,
            FormalSystem::Isabelle,
            FormalSystem::Agda,
            FormalSystem::Metamath,
            FormalSystem::Candle,
        ] {
            assert!(!EnvironmentFingerprint::resolve(system, false, None).is_resolved());
        }
        assert!(EnvironmentFingerprint::resolve(FormalSystem::Lean, true, None).is_resolved());

        let hyps = ctx(&[]);
        let key = VerificationCacheKey {
            system: FormalSystem::Lean,
            canonical_statement: "⊢ G",
            ordered_context: &hyps,
            proof_source: "theorem g : G := by simp",
            import_manifest: &[],
            environment: &unresolved,
            checker_identity: "lean:live",
            policy_fingerprint: "gate-v2",
        };
        assert!(cache_key(&key).is_none());
        let cache = CheckerCache::new();
        assert!(
            !cache.insert_verified(&key, verified_report()),
            "a verdict with an unknown environment must not be stored"
        );
        assert!(cache.is_empty());
        assert!(cache.get(&key).is_none());

        // The unresolved marker is still explicit in the derived material, so
        // "declared nothing" and "did not look" are distinguishable on audit.
        let other = EnvironmentFingerprint::unresolved("a different reason");
        let other_key = VerificationCacheKey {
            environment: &other,
            ..key
        };
        assert_ne!(key_material(&key), key_material(&other_key));
        let resolved = env();
        let resolved_key = VerificationCacheKey {
            environment: &resolved,
            ..key
        };
        assert_ne!(key_material(&key), key_material(&resolved_key));
        let _ = std::fs::remove_dir_all(&bare);
    }

    /// A verdict stored while an unresolved environment was in play must not be
    /// readable later just because resolution started working.
    #[test]
    fn resolution_recovering_does_not_resurrect_an_unstored_verdict() {
        let hyps = ctx(&[]);
        let unresolved = EnvironmentFingerprint::unresolved("toolchain not inspected");
        let resolved = env();
        let key = VerificationCacheKey {
            system: FormalSystem::Lean,
            canonical_statement: "⊢ G",
            ordered_context: &hyps,
            proof_source: "p",
            import_manifest: &[],
            environment: &unresolved,
            checker_identity: "lean:live",
            policy_fingerprint: "gate-v2",
        };
        let cache = CheckerCache::new();
        assert!(!cache.insert_verified(&key, verified_report()));
        let now_resolved = VerificationCacheKey {
            environment: &resolved,
            ..key
        };
        assert!(cache.get(&now_resolved).is_none());
    }

    /// Task 2 shape check: the stored entry records the statement identity, and
    /// says UNAVAILABLE rather than inventing an elaborated form.
    #[test]
    fn stored_entries_pin_the_statement_and_admit_what_they_cannot_pin() {
        let hyps = ctx(&[]);
        let key = input(
            FormalSystem::Lean,
            "P x ⊢ Q x",
            &hyps,
            "proof",
            "checker",
            "policy",
        );
        let cache = CheckerCache::new();
        assert!(cache.insert_verified(&key, verified_report()));
        let entry = cache.get_entry(&key).expect("hit");
        assert_eq!(entry.statement.source_digest.len(), 64);
        assert!(
            entry.statement.elaborated.is_none(),
            "no backend publishes an elaborated form at this layer today"
        );

        // When one does, it is captured with its provenance and never guessed.
        let mut report = verified_report();
        assert_eq!(ELABORATED_STATEMENT_DETAIL_KEY, "elaborated_statement");
        report.detail = json!({
            "elaborated_statement": {
                "provenance": "lean.repl.elaborated_type",
                "form": "∀ (x : ℕ), P x → Q x",
            }
        });
        assert!(cache.insert_verified(&key, report));
        let elaborated = cache
            .get_entry(&key)
            .and_then(|e| e.statement.elaborated)
            .expect("published elaborated form is captured");
        assert_eq!(elaborated.provenance, "lean.repl.elaborated_type");
        assert_eq!(elaborated.digest.len(), 64);
    }
}
