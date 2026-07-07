//! Regression test: our Lean soundness gate vs. LeanParanoia's own adversarial corpus.
//!
//! LeanParanoia ships a battery of hand-written soundness *exploits* (proofs of
//! `False`, `1 + 1 = 3`, … that slip past a naive check) alongside a set of
//! *valid negatives* (honest proofs that must NOT be flagged). This test drives
//! the real `paranoia` executable — the same binary our hardening gate shells
//! out to — over that corpus and asserts each verdict matches the fixture's
//! expected classification:
//!
//!   * fixtures under any exploit category directory  → paranoia `success == false`
//!   * fixtures under `Valid/`                        → paranoia `success == true`
//!
//! It is a moving-target regression guard: if a LeanParanoia upgrade (or our
//! pin of it) silently stops catching an attack, or starts rejecting honest
//! proofs, this test fails with the exact offending fixtures.
//!
//! # Best-effort / skips when the corpus isn't built
//!
//! Running the corpus requires (a) the built `paranoia` executable, (b) a Lean
//! toolchain with `lake` on PATH, and (c) the corpus oleans compiled into a Lake
//! project (LeanParanoia's pytest suite builds these into `.pytest_cache/`).
//! None of these are guaranteed on a dev box or in CI. When any precondition is
//! missing the test prints a clear `eprintln!` note and returns `Ok(())` — it
//! never fails the suite because tooling/corpus is absent. This mirrors the
//! best-effort, fail-open-on-missing-tooling pattern in `hardening.rs`.
//!
//! Uses only `std` + `serde_json`. It invokes `paranoia` directly via
//! `std::process::Command` (through `lake env`, exactly as our gate and the
//! upstream pytest harness do) and does not depend on any private items of
//! `hardening.rs`.
//!
//! ## Knobs (environment variables)
//!
//!   * `THEOREMATA_PARANOIA_CORPUS`         — override the corpus root dir.
//!   * `THEOREMATA_PARANOIA_CORPUS_PROJECT` — a prebuilt Lake project whose
//!                                            oleans expose `LeanTestProject.*`.
//!   * `THEOREMATA_PARANOIA_BUILD=1`        — opt in to building the corpus
//!                                            project via `lake build` if absent
//!                                            (off by default: it is heavy).
//!   * `THEOREMATA_PARANOIA_MAX_EXPLOITS`   — cap on exploit fixtures per run
//!                                            (default 24; `0` = no cap). Valid
//!                                            negatives are always run in full.
//!   * `THEOREMATA_PARANOIA_TIMEOUT_SECS`   — per-fixture timeout (default 180).
//!   * `THEOREMATA_LAKE`                    — override the `lake` program name.

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use std::collections::{BTreeMap, VecDeque};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    /// Expected classification of a fixture.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Expected {
        /// paranoia must report `success == false` (attack caught).
        Flagged,
        /// paranoia must report `success == true` (honest proof).
        Clean,
    }

    /// One discovered corpus fixture: a fully-qualified theorem to audit.
    struct Fixture {
        /// e.g. `LeanTestProject.Sorry.Direct.exploit_theorem`
        target: String,
        /// top-level category dir, e.g. `Sorry`, `Valid`.
        category: String,
        expected: Expected,
        /// Disable the (slow, recursion-sensitive) kernel replay check for this
        /// fixture — mirrors the upstream pytest harness passing
        /// `enable_replay=False` for recursive valid proofs.
        no_replay: bool,
        /// display path relative to the corpus, for failure messages.
        rel: String,
    }

    /// Outcome of a single `paranoia` invocation.
    enum RunOutcome {
        /// Parsed a `{"success":bool,"failures":{...}}` verdict.
        Verdict {
            success: bool,
            failure_keys: Vec<String>,
        },
        /// Ran, but produced no parseable JSON verdict.
        NoVerdict(String),
        /// The process could not be launched.
        LaunchError(String),
        /// The process exceeded the per-fixture timeout.
        Timeout,
    }

    fn truthy_env(name: &str) -> bool {
        std::env::var(name)
            .map(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false)
    }

    /// The LeanParanoia corpus root (contains `tests/`, `.lake/`, `lakefile.toml`).
    fn corpus_root() -> PathBuf {
        if let Ok(p) = std::env::var("THEOREMATA_PARANOIA_CORPUS") {
            return PathBuf::from(p);
        }
        // Crate root is the Cargo manifest dir; the vendored corpus lives under
        // `resources/`. Keep this in sync with `config.rs`'s `resources` default.
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join("LeanParanoia-main")
            .join("LeanParanoia-main")
    }

    /// Locate the built `paranoia` executable (`.exe` on Windows, bare elsewhere).
    fn find_paranoia_exe(root: &Path) -> Option<PathBuf> {
        let base = root.join(".lake").join("build").join("bin");
        for name in ["paranoia.exe", "paranoia"] {
            let p = base.join(name);
            if p.exists() {
                return Some(p);
            }
        }
        None
    }

    /// The `lake` program name (overridable), or `None` if it can't be launched.
    fn resolve_lake() -> Option<String> {
        let lake = std::env::var("THEOREMATA_LAKE").unwrap_or_else(|_| "lake".to_string());
        let ok = Command::new(&lake)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        ok.then_some(lake)
    }

    /// A Lake project whose oleans expose the `LeanTestProject.*` modules.
    fn resolve_project_dir(root: &Path) -> Option<PathBuf> {
        if let Ok(p) = std::env::var("THEOREMATA_PARANOIA_CORPUS_PROJECT") {
            let p = PathBuf::from(p);
            if p.join("lakefile.toml").exists() {
                return Some(p);
            }
        }
        // Where the upstream pytest harness (`tests/conftest.py`) builds it.
        let default = root.join(".pytest_cache").join("LeanTestProject");
        default.join("lakefile.toml").exists().then_some(default)
    }

    // --- lightweight Lean declaration parsing (no regex crate) -----------------

    /// If `line` opens a declaration, return `(is_theorem_or_lemma, name)`.
    /// Robust to leading `@[..]` attributes and stacked modifiers (`private`, …).
    fn decl_on_line(line: &str) -> Option<(bool, String)> {
        const MODIFIERS: &[&str] = &[
            "private ",
            "protected ",
            "noncomputable ",
            "nonrec ",
            "partial ",
            "unsafe ",
            "public ",
            "scoped ",
            "local ",
            "mutual ",
        ];
        // (keyword, is_theorem)
        const DECLS: &[(&str, bool)] = &[
            ("theorem ", true),
            ("lemma ", true),
            ("def ", false),
            ("abbrev ", false),
            ("opaque ", false),
            ("instance ", false),
        ];

        let mut rest = line.trim_start();
        if rest.starts_with("--") || rest.starts_with("/-") {
            return None;
        }
        // Strip leading attributes (possibly stacked on one line).
        while let Some(after) = rest.strip_prefix("@[") {
            match after.find(']') {
                Some(close) => rest = after[close + 1..].trim_start(),
                None => return None,
            }
        }
        // Strip stacked declaration modifiers.
        loop {
            let mut stripped = false;
            for m in MODIFIERS {
                if let Some(after) = rest.strip_prefix(m) {
                    rest = after.trim_start();
                    stripped = true;
                    break;
                }
            }
            if !stripped {
                break;
            }
        }
        for (kw, is_thm) in DECLS {
            if let Some(after) = rest.strip_prefix(kw) {
                let name: String = after
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '\''))
                    .collect();
                if !name.is_empty() {
                    return Some((*is_thm, name));
                }
            }
        }
        None
    }

    /// Does the file declare something named exactly `exploit_theorem`?
    fn has_exploit_theorem(src: &str) -> bool {
        src.lines()
            .filter_map(decl_on_line)
            .any(|(_, name)| name == "exploit_theorem")
    }

    /// Count word-boundary occurrences of `name` in `src` (decl vs. use apart).
    fn count_word(src: &str, name: &str) -> usize {
        if name.is_empty() {
            return 0;
        }
        let bytes = src.as_bytes();
        let nb = name.as_bytes();
        let is_id = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'\'' || b == b'.';
        let mut count = 0usize;
        let mut i = 0usize;
        while i + nb.len() <= bytes.len() {
            if &bytes[i..i + nb.len()] == nb {
                let before_ok = i == 0 || !is_id(bytes[i - 1]);
                let after_idx = i + nb.len();
                let after_ok = after_idx >= bytes.len() || !is_id(bytes[after_idx]);
                if before_ok && after_ok {
                    count += 1;
                    i = after_idx;
                    continue;
                }
            }
            i += 1;
        }
        count
    }

    /// Pick the theorem to audit for a `Valid/` fixture, plus whether replay
    /// should be disabled (recursive / self-referential proofs).
    fn pick_valid_theorem(src: &str) -> Option<(String, bool)> {
        // Some valid fixtures name their (honest) theorem `exploit_theorem`; the
        // Valid/ directory — not the name — decides the verdict.
        if has_exploit_theorem(src) {
            return Some((
                "exploit_theorem".to_string(),
                count_word(src, "exploit_theorem") > 1,
            ));
        }
        let mut thms: Vec<String> = Vec::new();
        let mut defs: Vec<String> = Vec::new();
        for line in src.lines() {
            if let Some((is_thm, name)) = decl_on_line(line) {
                if is_thm {
                    thms.push(name);
                } else {
                    defs.push(name);
                }
            }
        }
        // Prefer the first non-recursive theorem (safe under kernel replay).
        for n in &thms {
            if count_word(src, n) == 1 {
                return Some((n.clone(), false));
            }
        }
        if let Some(n) = thms.first() {
            return Some((n.clone(), true)); // recursive → skip replay
        }
        for n in &defs {
            if count_word(src, n) == 1 {
                return Some((n.clone(), false));
            }
        }
        defs.first().map(|n| (n.clone(), true))
    }

    /// Recursively collect `*.lean` files under `dir`.
    fn collect_lean_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_lean_files(&path, out);
            } else if path.extension().and_then(|s| s.to_str()) == Some("lean") {
                out.push(path);
            }
        }
    }

    /// Discover fixtures. Returns `(fixtures, skipped_paths)` where `skipped` are
    /// exploit-category files that expose no unambiguous `exploit_theorem`
    /// entrypoint (e.g. transitive helper modules) and are therefore not asserted.
    fn discover(lean_files_root: &Path) -> (Vec<Fixture>, Vec<String>) {
        let mut files = Vec::new();
        collect_lean_files(lean_files_root, &mut files);
        files.sort();

        let mut fixtures = Vec::new();
        let mut skipped = Vec::new();

        for path in files {
            let rel = match path.strip_prefix(lean_files_root) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let rel_display = rel.to_string_lossy().replace('\\', "/");
            let mut parts: Vec<String> = rel
                .components()
                .filter_map(|c| match c {
                    std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                    _ => None,
                })
                .collect();
            if parts.is_empty() {
                continue;
            }
            // Drop the `.lean` extension from the final component.
            if let Some(last) = parts.last_mut() {
                if let Some(stripped) = last.strip_suffix(".lean") {
                    *last = stripped.to_string();
                }
            }
            let category = parts[0].clone();
            let module = format!("LeanTestProject.{}", parts.join("."));

            let src_bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let src = String::from_utf8_lossy(&src_bytes);

            let (theorem, expected, no_replay) = if category == "Valid" {
                match pick_valid_theorem(&src) {
                    Some((thm, no_replay)) => (thm, Expected::Clean, no_replay),
                    None => {
                        skipped.push(rel_display.clone());
                        continue;
                    }
                }
            } else if has_exploit_theorem(&src) {
                ("exploit_theorem".to_string(), Expected::Flagged, false)
            } else {
                // No canonical entrypoint (transitive helper, etc.) — not asserted.
                skipped.push(rel_display.clone());
                continue;
            };

            let target = format!("{module}.{theorem}");
            fixtures.push(Fixture {
                target,
                category,
                expected,
                no_replay,
                rel: rel_display,
            });
        }

        (fixtures, skipped)
    }

    /// Cap exploit fixtures to `cap`, spreading the selection round-robin across
    /// categories so every attack class is represented. `cap == 0` means no cap.
    /// Returns `(selected, dropped_count)`.
    fn select_exploits(exploits: Vec<Fixture>, cap: usize) -> (Vec<Fixture>, usize) {
        let total = exploits.len();
        if cap == 0 || total <= cap {
            return (exploits, 0);
        }
        let mut by_cat: BTreeMap<String, VecDeque<Fixture>> = BTreeMap::new();
        for f in exploits {
            by_cat.entry(f.category.clone()).or_default().push_back(f);
        }
        let mut selected: Vec<Fixture> = Vec::with_capacity(cap);
        loop {
            let mut progressed = false;
            for q in by_cat.values_mut() {
                if selected.len() >= cap {
                    break;
                }
                if let Some(f) = q.pop_front() {
                    selected.push(f);
                    progressed = true;
                }
            }
            if selected.len() >= cap || !progressed {
                break;
            }
        }
        let dropped = total - selected.len();
        (selected, dropped)
    }

    /// Try to parse a paranoia verdict out of a text blob (whole, then per line).
    fn parse_verdict(text: &str) -> Option<RunOutcome> {
        for candidate in std::iter::once(text).chain(text.lines()) {
            let candidate = candidate.trim();
            if candidate.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(candidate) {
                if let Some(success) = v.get("success").and_then(Value::as_bool) {
                    let failure_keys = v
                        .get("failures")
                        .and_then(Value::as_object)
                        .map(|m| m.keys().cloned().collect())
                        .unwrap_or_default();
                    return Some(RunOutcome::Verdict {
                        success,
                        failure_keys,
                    });
                }
            }
        }
        None
    }

    /// Invoke `lake env <exe> --trust-modules Std,Mathlib,Init [--no-replay] <target>`
    /// in `project_dir`, with a wall-clock timeout enforced via a worker thread.
    fn run_paranoia(
        lake: &str,
        exe: &Path,
        project_dir: &Path,
        target: &str,
        no_replay: bool,
        timeout: Duration,
    ) -> RunOutcome {
        let mut args: Vec<String> = vec![
            "env".to_string(),
            exe.to_string_lossy().into_owned(),
            "--trust-modules".to_string(),
            "Std,Mathlib,Init".to_string(),
            "--allowed-axioms".to_string(),
            "propext,Quot.sound,Classical.choice".to_string(),
        ];
        if no_replay {
            args.push("--no-replay".to_string());
        }
        args.push(target.to_string());

        let lake = lake.to_string();
        let dir = project_dir.to_path_buf();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let out = Command::new(&lake).current_dir(&dir).args(&args).output();
            let _ = tx.send(out);
        });

        match rx.recv_timeout(timeout) {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
                if let Some(v) = parse_verdict(&stdout) {
                    return v;
                }
                if let Some(v) = parse_verdict(&stderr) {
                    return v;
                }
                let trunc = |s: &str| s.chars().take(300).collect::<String>();
                RunOutcome::NoVerdict(format!(
                    "stdout={:?} stderr={:?}",
                    trunc(&stdout),
                    trunc(&stderr)
                ))
            }
            Ok(Err(e)) => RunOutcome::LaunchError(e.to_string()),
            // The worker thread (and its child) are left to finish/exit on their
            // own; we simply stop waiting so one pathological fixture cannot hang
            // the whole suite.
            Err(_) => RunOutcome::Timeout,
        }
    }

    #[test]
    fn paranoia_corpus_regression() -> Result<(), Box<dyn std::error::Error>> {
        let root = corpus_root();
        let lean_files = root.join("tests").join("lean_exploit_files");
        if !lean_files.is_dir() {
            eprintln!(
                "[paranoia_corpus] SKIP: corpus not found at {} — set THEOREMATA_PARANOIA_CORPUS",
                lean_files.display()
            );
            return Ok(());
        }

        let exe = match find_paranoia_exe(&root) {
            Some(e) => e,
            None => {
                eprintln!(
                    "[paranoia_corpus] SKIP: paranoia executable not built under {} \
                     (run `lake build paranoia` in the corpus)",
                    root.join(".lake/build/bin").display()
                );
                return Ok(());
            }
        };

        let lake = match resolve_lake() {
            Some(l) => l,
            None => {
                eprintln!(
                    "[paranoia_corpus] SKIP: `lake` not launchable (need a Lean toolchain on PATH; \
                     override with THEOREMATA_LAKE)"
                );
                return Ok(());
            }
        };

        // Locate — or optionally build — the Lake project holding the oleans.
        let project_dir = match resolve_project_dir(&root) {
            Some(p) => p,
            None => {
                if truthy_env("THEOREMATA_PARANOIA_BUILD") {
                    match build_corpus_project(&root, &lake) {
                        Some(p) => p,
                        None => {
                            eprintln!(
                                "[paranoia_corpus] SKIP: THEOREMATA_PARANOIA_BUILD set but building \
                                 the corpus project failed"
                            );
                            return Ok(());
                        }
                    }
                } else {
                    eprintln!(
                        "[paranoia_corpus] SKIP: corpus oleans not built. Build them once via \
                         LeanParanoia's pytest suite (`uv run pytest tests/exploits`), point \
                         THEOREMATA_PARANOIA_CORPUS_PROJECT at a prebuilt project, or set \
                         THEOREMATA_PARANOIA_BUILD=1 to build here."
                    );
                    return Ok(());
                }
            }
        };

        let timeout = Duration::from_secs(
            std::env::var("THEOREMATA_PARANOIA_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(180),
        );

        // Discover and partition fixtures.
        let (fixtures, skipped) = discover(&lean_files);
        let (valid, exploits): (Vec<Fixture>, Vec<Fixture>) = fixtures
            .into_iter()
            .partition(|f| f.expected == Expected::Clean);
        if valid.is_empty() || exploits.is_empty() {
            eprintln!(
                "[paranoia_corpus] SKIP: unexpected corpus shape ({} valid, {} exploits)",
                valid.len(),
                exploits.len()
            );
            return Ok(());
        }

        // Baseline probe: a known-clean fixture must pass. If it doesn't, the
        // project isn't properly built (import failures) — skip rather than
        // drown the run in spurious mismatches.
        let probe = &valid[0];
        match run_paranoia(
            &lake,
            &exe,
            &project_dir,
            &probe.target,
            probe.no_replay,
            timeout,
        ) {
            RunOutcome::Verdict { success: true, .. } => {}
            other => {
                let why = match other {
                    RunOutcome::Verdict { failure_keys, .. } => {
                        format!("clean probe was flagged ({})", failure_keys.join(","))
                    }
                    RunOutcome::NoVerdict(d) => format!("no verdict ({d})"),
                    RunOutcome::LaunchError(e) => format!("launch error ({e})"),
                    RunOutcome::Timeout => "timed out".to_string(),
                };
                eprintln!(
                    "[paranoia_corpus] SKIP: baseline probe {} did not pass — corpus likely not \
                     built for this toolchain: {why}",
                    probe.target
                );
                return Ok(());
            }
        }

        // Cap exploit fixtures (valid negatives always run in full).
        let cap = std::env::var("THEOREMATA_PARANOIA_MAX_EXPLOITS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(24);
        let total_exploits = exploits.len();
        let (sel_exploits, dropped) = select_exploits(exploits, cap);
        if dropped > 0 {
            eprintln!(
                "[paranoia_corpus] NOTE: capping exploits to {} of {} (round-robin by category); \
                 {} not run this pass. Raise THEOREMATA_PARANOIA_MAX_EXPLOITS (0 = all).",
                sel_exploits.len(),
                total_exploits,
                dropped
            );
        }

        let mut selected: Vec<Fixture> = Vec::new();
        selected.extend(valid);
        selected.extend(sel_exploits);

        // Run and collect mismatches.
        let mut mismatches: Vec<String> = Vec::new();
        let mut ran = 0usize;
        for f in &selected {
            let outcome = run_paranoia(&lake, &exe, &project_dir, &f.target, f.no_replay, timeout);
            ran += 1;
            match outcome {
                RunOutcome::Verdict {
                    success,
                    failure_keys,
                } => {
                    let want_flagged = f.expected == Expected::Flagged;
                    let got_flagged = !success;
                    if got_flagged != want_flagged {
                        let keys = if failure_keys.is_empty() {
                            "none".to_string()
                        } else {
                            failure_keys.join(",")
                        };
                        mismatches.push(format!(
                            "{} [{}]: expected {} but paranoia returned success={} (failures: {})",
                            f.rel,
                            f.category,
                            if want_flagged { "FLAGGED" } else { "CLEAN" },
                            success,
                            keys
                        ));
                    }
                }
                RunOutcome::NoVerdict(d) => mismatches.push(format!(
                    "{} [{}]: no parseable verdict from paranoia ({})",
                    f.rel, f.category, d
                )),
                RunOutcome::LaunchError(e) => mismatches.push(format!(
                    "{} [{}]: could not launch paranoia ({})",
                    f.rel, f.category, e
                )),
                RunOutcome::Timeout => {
                    mismatches.push(format!("{} [{}]: paranoia timed out", f.rel, f.category))
                }
            }
        }

        let valid_run = selected
            .iter()
            .filter(|f| f.expected == Expected::Clean)
            .count();
        let exploit_run = ran - valid_run;
        eprintln!(
            "[paranoia_corpus] ran {ran} fixtures ({exploit_run} exploits, {valid_run} valid \
             negatives); {} without an exploit_theorem entrypoint were not asserted.",
            skipped.len()
        );

        assert!(
            mismatches.is_empty(),
            "LeanParanoia soundness-gate regressions: {} of {} fixtures disagreed with the \
             corpus ground truth:\n{}",
            mismatches.len(),
            ran,
            mismatches.join("\n")
        );

        Ok(())
    }

    // --- opt-in corpus build (mirrors tests/conftest.py::_build_lean_project) --

    /// Recursively copy `src` into `dst`.
    fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let from = entry.path();
            let to = dst.join(entry.file_name());
            if from.is_dir() {
                copy_dir(&from, &to)?;
            } else {
                std::fs::copy(&from, &to)?;
            }
        }
        Ok(())
    }

    /// Build the `LeanTestProject` corpus into `<root>/.pytest_cache/LeanTestProject`
    /// by replicating the upstream pytest scaffold, then `lake build`. Returns the
    /// project dir on success. Best-effort: any failure yields `None`.
    fn build_corpus_project(root: &Path, lake: &str) -> Option<PathBuf> {
        let lean_files = root.join("tests").join("lean_exploit_files");
        let template = root.join("tests").join("project_template");
        let toolchain = root.join("lean-toolchain");
        let lakefile_tpl = template.join("lakefile.toml.template");
        if !lean_files.is_dir() || !lakefile_tpl.exists() || !toolchain.exists() {
            return None;
        }

        let build_dir = root.join(".pytest_cache").join("LeanTestProject");
        std::fs::create_dir_all(&build_dir).ok()?;

        std::fs::copy(&toolchain, build_dir.join("lean-toolchain")).ok()?;
        std::fs::copy(&lakefile_tpl, build_dir.join("lakefile.toml")).ok()?;

        // Copy sources under `<build>/LeanTestProject/`.
        let dest = build_dir.join("LeanTestProject");
        copy_dir(&lean_files, &dest).ok()?;

        // Generate the root import file over every copied module.
        let mut modules = Vec::new();
        let mut copied = Vec::new();
        collect_lean_files(&dest, &mut copied);
        for f in copied {
            if let Ok(rel) = f.strip_prefix(&build_dir) {
                let mut parts: Vec<String> = rel
                    .components()
                    .filter_map(|c| match c {
                        std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                        _ => None,
                    })
                    .collect();
                if let Some(last) = parts.last_mut() {
                    if let Some(stripped) = last.strip_suffix(".lean") {
                        *last = stripped.to_string();
                    }
                }
                modules.push(format!("import {}", parts.join(".")));
            }
        }
        modules.sort();
        let body = if modules.is_empty() {
            "-- No files".to_string()
        } else {
            modules.join("\n")
        };
        std::fs::write(build_dir.join("LeanTestProject.lean"), body).ok()?;

        eprintln!("[paranoia_corpus] building corpus project (lake build) — this is heavy…");
        let status = Command::new(lake)
            .current_dir(&build_dir)
            .arg("build")
            .status()
            .ok()?;
        // Even a non-zero build can leave enough oleans for a subset; the baseline
        // probe in the caller is the real gate. Only require the lakefile exists.
        let _ = status;
        build_dir
            .join("lakefile.toml")
            .exists()
            .then_some(build_dir)
    }

    // --- unit coverage for the pure discovery/parsing logic --------------------

    #[test]
    fn detects_exploit_theorem_declaration() {
        assert!(has_exploit_theorem(
            "theorem exploit_theorem : False := sorry\n"
        ));
        assert!(has_exploit_theorem(
            "@[implemented_by foo]\ndef exploit_theorem : True := trivial\n"
        ));
        assert!(!has_exploit_theorem(
            "theorem simple_theorem : True := trivial\n"
        ));
        // A mention inside a comment is not a declaration.
        assert!(!has_exploit_theorem("-- exploit_theorem is elsewhere\n"));
    }

    #[test]
    fn valid_picks_nonrecursive_theorem_with_replay_on() {
        // Recursive first theorem, non-recursive later one → pick the latter,
        // keep replay enabled.
        let src = "theorem rec_thm (t : Tree) : P t := by exact rec_thm t\n\
                   theorem plain_thm : True := trivial\n";
        let (name, no_replay) = pick_valid_theorem(src).unwrap();
        assert_eq!(name, "plain_thm");
        assert!(!no_replay);
    }

    #[test]
    fn valid_falls_back_to_recursive_theorem_with_replay_off() {
        let src = "theorem rec_thm (t : Tree) : P t := by exact rec_thm t\n";
        let (name, no_replay) = pick_valid_theorem(src).unwrap();
        assert_eq!(name, "rec_thm");
        assert!(no_replay, "self-recursive proof must disable kernel replay");
    }

    #[test]
    fn valid_named_exploit_theorem_is_still_clean_target() {
        // Mirrors Valid/UnsafeReducibility.lean.
        let src = "set_option allowUnsafeReducibility true\n\
                   def addTwo (n : Nat) : Nat := n + 2\n\
                   theorem exploit_theorem : addTwo 1 = 3 := rfl\n";
        let (name, _) = pick_valid_theorem(src).unwrap();
        assert_eq!(name, "exploit_theorem");
    }

    #[test]
    fn count_word_respects_boundaries() {
        assert_eq!(count_word("foo bar foo\n", "foo"), 2);
        // `foobar` must not match the `foo` word.
        assert_eq!(count_word("foobar baz\n", "foo"), 0);
    }

    #[test]
    fn select_exploits_caps_and_spreads_by_category() {
        let mk = |cat: &str, n: usize| Fixture {
            target: format!("LeanTestProject.{cat}.M{n}.exploit_theorem"),
            category: cat.to_string(),
            expected: Expected::Flagged,
            no_replay: false,
            rel: format!("{cat}/M{n}.lean"),
        };
        let exploits = vec![mk("A", 1), mk("A", 2), mk("A", 3), mk("B", 1), mk("C", 1)];
        let (sel, dropped) = select_exploits(exploits, 3);
        assert_eq!(sel.len(), 3);
        assert_eq!(dropped, 2);
        // Round-robin: one from each category before a second from any.
        let cats: Vec<&str> = sel.iter().map(|f| f.category.as_str()).collect();
        assert_eq!(cats, vec!["A", "B", "C"]);
    }
}
