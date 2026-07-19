//! Runner-agnostic exec bridge for the live formal-system gates (Phase 2).
//!
//! Every toolchain invocation (Lean's `lean`, Rocq's `coqc`/`coqchk`, Isabelle's
//! `isabelle build`) is dispatched through a single [`run`] entry point against a
//! per-system [`Runner`]:
//!
//! * [`Runner::Native`]   — a direct `Command` in the workspace dir.
//! * [`Runner::Wsl`]      — `wsl.exe -d <distro> -- bash -lc 'cd <mnt> && …'`,
//!   translating the Windows workspace path to `/mnt/<drive>/…`.
//! * [`Runner::Docker`]   — `docker run --rm -v <host>:/work -w /work <image> …`
//!   (degrades to a not-launched outcome when Docker is absent — never panics).
//!
//! Nothing here hardcodes WSL: which runner a system uses is read from
//! [`FormalRunners`] on `Config`, so "drive Lean under WSL" or "drive Rocq
//! natively" is a config flip, not a code change. The defaults on this machine
//! are `lean = Native`, `rocq = Wsl{Ubuntu}`, `isabelle = Wsl{Ubuntu}`.

use crate::prover::formal::FormalSystem;
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};
use wait_timeout::ChildExt;

/// Fail-closed resource caps applied to every external toolchain invocation in
/// the verification gate. Defends against a "proof-DDOS": a malicious or
/// pathological proof/lemma that makes the external checker hang forever or
/// flood its output. On exceeding either cap the child is killed and the run is
/// reported as a (non-zero) failure, never a hang. Configurable via
/// `THEOREMATA_EXEC_TIMEOUT_SECS` and `THEOREMATA_EXEC_MAX_OUTPUT_BYTES`.
#[derive(Debug, Clone, Copy)]
pub struct ResourceLimits {
    pub timeout: Duration,
    pub max_output_bytes: usize,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            // Matches the historical check_axioms default; a whole checker run.
            timeout: Duration::from_secs(300),
            // 16 MiB of captured stdout+stderr each is plenty for any honest
            // proof; beyond it we keep draining but stop storing.
            max_output_bytes: 16 * 1024 * 1024,
        }
    }
}

impl ResourceLimits {
    /// Read the caps from the environment, falling back to [`Default`].
    pub fn from_env() -> Self {
        let d = Self::default();
        let timeout = std::env::var("THEOREMATA_EXEC_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .filter(|&s| s > 0)
            .map(Duration::from_secs)
            .unwrap_or(d.timeout);
        let max_output_bytes = std::env::var("THEOREMATA_EXEC_MAX_OUTPUT_BYTES")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|&b| b > 0)
            .unwrap_or(d.max_output_bytes);
        Self {
            timeout,
            max_output_bytes,
        }
    }
}

/// How to execute a toolchain command for one formal system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Runner {
    /// Run the binary directly on this host, in the workspace directory.
    Native,
    /// Run inside a WSL distro via `wsl.exe -d <distro> -- bash -lc`.
    Wsl {
        #[serde(default = "default_distro")]
        distro: String,
    },
    /// Run inside a one-shot Docker container with the workspace bind-mounted.
    Docker { image: String },
}

fn default_distro() -> String {
    "Ubuntu".to_string()
}

impl Runner {
    /// Apply environment overrides (e.g. `THEOREMATA_WSL_DISTRO`) to a configured
    /// runner without mutating the stored config.
    pub fn resolved(&self) -> Runner {
        match self {
            Runner::Wsl { distro } => Runner::Wsl {
                distro: env_or("THEOREMATA_WSL_DISTRO", distro),
            },
            other => other.clone(),
        }
    }

    /// A short human tag for provenance/detail payloads.
    pub fn tag(&self) -> String {
        match self {
            Runner::Native => "native".into(),
            Runner::Wsl { distro } => format!("wsl:{distro}"),
            Runner::Docker { image } => format!("docker:{image}"),
        }
    }
}

/// The per-formal-system runner map carried on `Config`. Defaults match this
/// machine (`lean` native on Windows; `rocq`/`isabelle` via WSL Ubuntu) but any
/// system can be pointed at Native / Wsl / Docker purely through config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormalRunners {
    #[serde(default = "runner_native")]
    pub lean: Runner,
    #[serde(default = "runner_wsl_ubuntu")]
    pub rocq: Runner,
    #[serde(default = "runner_wsl_ubuntu")]
    pub isabelle: Runner,
    /// Candle (verified HOL Light on CakeML). Defaults to WSL Ubuntu, where the
    /// HOL4/PolyML/CakeML toolchain that builds `candle` lives on this machine.
    #[serde(default = "runner_wsl_ubuntu")]
    pub candle: Runner,
    #[serde(default = "runner_native")]
    pub agda: Runner,
    #[serde(default = "runner_native")]
    pub metamath: Runner,
}

fn runner_native() -> Runner {
    Runner::Native
}

fn runner_wsl_ubuntu() -> Runner {
    Runner::Wsl {
        distro: default_distro(),
    }
}

impl Default for FormalRunners {
    fn default() -> Self {
        Self {
            lean: runner_native(),
            rocq: runner_wsl_ubuntu(),
            isabelle: runner_wsl_ubuntu(),
            candle: runner_wsl_ubuntu(),
            agda: runner_native(),
            metamath: runner_native(),
        }
    }
}

impl FormalRunners {
    /// The configured runner for `system`, with env overrides applied.
    pub fn for_system(&self, system: FormalSystem) -> Runner {
        let base = match system {
            FormalSystem::Lean => &self.lean,
            FormalSystem::Rocq => &self.rocq,
            FormalSystem::Isabelle => &self.isabelle,
            FormalSystem::Candle => &self.candle,
            FormalSystem::Agda => &self.agda,
            FormalSystem::Metamath => &self.metamath,
        };
        base.resolved()
    }
}

/// The captured result of one command. `launched` distinguishes "the runner
/// itself could not start the process" (missing `wsl.exe`/`docker`) from "the
/// process ran and exited non-zero".
#[derive(Debug, Clone)]
pub struct ExecOutcome {
    pub launched: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// The run was killed for exceeding the wall-clock cap (fail-closed).
    pub timed_out: bool,
    /// stdout/stderr hit the byte cap and the remainder was discarded.
    pub output_capped: bool,
}

impl ExecOutcome {
    /// A timed-out run never counts as success (fail-closed): `code` is `None`.
    pub fn success(&self) -> bool {
        self.launched && !self.timed_out && self.code == Some(0)
    }

    /// True when this failure is *deterministic* for the same (input, limits)
    /// pair — i.e. re-running the identical command would fail the identical
    /// way, so a retry only burns budget and duplicates the trace entry.
    ///
    /// Currently this is exactly the wall-clock kill: the checker did not
    /// disagree with the proof, it ran out of the budget it will be given again.
    ///
    /// Deliberately NOT covered:
    /// * `output_capped` — it is not itself a failure (a capped run can still
    ///   exit 0), and it is also set by the post-kill reader grace timeout in
    ///   [`spawn_with`], which is a timing artifact rather than a property of
    ///   the input. Folding it in here would make a merely chatty checker
    ///   unretryable.
    /// * `launched == false` — a missing `wsl.exe`/`docker` is an environment
    ///   fault that a caller may legitimately fix and retry.
    /// * a plain non-zero exit — that is the semantic failure retries exist for.
    pub fn is_deterministic_failure(&self) -> bool {
        self.timed_out
    }

    fn not_launched(err: impl std::fmt::Display) -> Self {
        Self {
            launched: false,
            code: None,
            stdout: String::new(),
            stderr: err.to_string(),
            timed_out: false,
            output_capped: false,
        }
    }
}

/// Read an env var, falling back to `fallback` when unset/empty.
pub fn env_or(name: &str, fallback: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

/// Strip a Windows extended-length prefix (`\\?\`) that `canonicalize` adds.
fn strip_extended(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    s.strip_prefix("//?/").map(str::to_string).unwrap_or(s)
}

/// Translate a Windows path (`C:\Users\x`) to a WSL `/mnt/c/Users/x` path.
/// Already-POSIX paths are returned with separators normalized.
pub fn to_wsl_path(path: &Path) -> String {
    let s = strip_extended(path);
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && (bytes[0] as char).is_ascii_alphabetic() {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        // `s[2..]` already starts with `/` after separator normalization.
        format!("/mnt/{drive}{}", &s[2..])
    } else {
        s
    }
}

/// Single-quote a string for a POSIX shell (used for the `cd` target only, so
/// workspace paths with spaces are safe while tool argv keep `~` expansion).
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Drain a child pipe, storing at most `cap` bytes and discarding the rest (so a
/// flooding child cannot exhaust memory, yet the pipe keeps draining so the child
/// isn't wedged on a full buffer). Returns the captured text and whether it was
/// truncated.
fn read_capped<R: Read>(reader: Option<R>, cap: usize) -> (String, bool) {
    let mut buf: Vec<u8> = Vec::new();
    let mut capped = false;
    if let Some(mut r) = reader {
        let mut chunk = [0u8; 8192];
        loop {
            match r.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if buf.len() < cap {
                        let take = (cap - buf.len()).min(n);
                        buf.extend_from_slice(&chunk[..take]);
                        if take < n {
                            capped = true;
                        }
                    } else {
                        capped = true; // keep draining, drop the bytes
                    }
                }
                Err(_) => break,
            }
        }
    }
    (String::from_utf8_lossy(&buf).into_owned(), capped)
}

fn spawn(cmd: Command) -> ExecOutcome {
    spawn_with(cmd, ResourceLimits::from_env())
}

/// Spawn `cmd` under fail-closed resource caps: pipes are drained on threads with
/// a byte cap, and the child is killed if it outlives `limits.timeout`. A killed
/// child yields `timed_out = true` and thus `success() == false`.
fn spawn_with(mut cmd: Command, limits: ResourceLimits) -> ExecOutcome {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return ExecOutcome::not_launched(e),
    };
    // Drain stdout/stderr concurrently (so a large writer never deadlocks on a
    // full pipe) and deliver each result over a channel, so we can bound how long
    // we wait for a reader: after a kill, a surviving grandchild may still hold
    // the pipe open, and we must not block on that indefinitely.
    let cap = limits.max_output_bytes;
    let out = child.stdout.take();
    let err = child.stderr.take();
    let (otx, orx) = std::sync::mpsc::channel();
    let (etx, erx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = otx.send(read_capped(out, cap));
    });
    std::thread::spawn(move || {
        let _ = etx.send(read_capped(err, cap));
    });

    let (timed_out, code) = match child.wait_timeout(limits.timeout) {
        Ok(Some(status)) => (false, status.code()),
        Ok(None) => {
            // Exceeded the wall-clock cap: kill and reap.
            let _ = child.kill();
            let _ = child.wait();
            (true, None)
        }
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return ExecOutcome::not_launched(e);
        }
    };
    // On the normal path the process has exited, so the pipes are closed and the
    // readers return at once. On a kill, grace-bound the wait so a grandchild
    // holding the pipe cannot re-introduce a hang.
    let grace = Duration::from_secs(2);
    let (stdout, out_capped) = orx.recv_timeout(grace).unwrap_or_else(|_| {
        (
            "[theoremata] stdout reader did not drain in time".into(),
            true,
        )
    });
    let (mut stderr, err_capped) = erx
        .recv_timeout(grace)
        .unwrap_or_else(|_| (String::new(), true));
    if timed_out {
        stderr.push_str(&format!(
            "\n[theoremata] killed: exceeded {}s verification resource limit",
            limits.timeout.as_secs()
        ));
    }
    ExecOutcome {
        launched: true,
        code,
        stdout,
        stderr,
        timed_out,
        output_capped: out_capped || err_capped,
    }
}

/// Run `argv` (program + args) with working directory `workspace`, dispatching on
/// `runner`. `argv` elements are passed to the tool verbatim; for WSL they are
/// joined unquoted so a leading `~` in a binary path still expands (workspace
/// paths, which may contain spaces, are quoted). Never panics: a runner that
/// cannot start yields `launched = false`.
pub fn run(runner: &Runner, argv: &[&str], workspace: &Path) -> ExecOutcome {
    if argv.is_empty() {
        return ExecOutcome::not_launched("empty argv");
    }
    match runner {
        Runner::Native => {
            let mut cmd = Command::new(argv[0]);
            cmd.args(&argv[1..]).current_dir(workspace);
            spawn(cmd)
        }
        Runner::Wsl { distro } => {
            let mnt = to_wsl_path(workspace);
            let script = format!("cd {} && {}", sh_quote(&mnt), argv.join(" "));
            let mut cmd = Command::new("wsl.exe");
            cmd.args(["-d", distro.as_str(), "--", "bash", "-lc", script.as_str()]);
            spawn(cmd)
        }
        Runner::Docker { image } => {
            let host = strip_extended(workspace);
            let mut cmd = Command::new("docker");
            cmd.arg("run")
                .arg("--rm")
                .arg("-v")
                .arg(format!("{host}:/work"))
                .arg("-w")
                .arg("/work")
                .arg(image)
                .args(argv);
            spawn(cmd)
        }
    }
}

/// Probe whether `argv` (typically `["<bin>", "--version"]` or a `command -v`
/// check) succeeds under `runner`. Runs in the system temp dir so it needs no
/// scaffolded workspace. Returns false when the runner cannot start.
pub fn probe(runner: &Runner, argv: &[&str]) -> bool {
    run(runner, argv, &std::env::temp_dir()).success()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn read_capped_truncates_past_the_cap_but_keeps_draining() {
        let data = vec![b'x'; 5000];
        let (s, capped) = read_capped(Some(Cursor::new(data.clone())), 1000);
        assert_eq!(s.len(), 1000);
        assert!(capped, "over-cap output must be flagged");
        let (s2, capped2) = read_capped(Some(Cursor::new(data)), 8000);
        assert_eq!(s2.len(), 5000);
        assert!(!capped2, "under-cap output must not be flagged");
        let (s3, capped3) = read_capped::<Cursor<Vec<u8>>>(None, 10);
        assert!(s3.is_empty() && !capped3);
    }

    /// A portable command that runs longer than the test's timeout.
    fn sleeper() -> Command {
        // Direct child (no shell wrapper) so a kill closes the pipe promptly.
        #[cfg(windows)]
        {
            // ping delays ~1s per extra echo; -n 5 ~= 4s.
            let mut c = Command::new("ping");
            c.args(["127.0.0.1", "-n", "5"]);
            c
        }
        #[cfg(not(windows))]
        {
            let mut c = Command::new("sleep");
            c.args(["4"]);
            c
        }
    }

    fn echoer() -> Command {
        #[cfg(windows)]
        {
            let mut c = Command::new("cmd");
            c.args(["/C", "echo hello"]);
            c
        }
        #[cfg(not(windows))]
        {
            let mut c = Command::new("sh");
            c.args(["-c", "printf hello"]);
            c
        }
    }

    #[test]
    fn spawn_with_kills_a_runaway_and_fails_closed() {
        let limits = ResourceLimits {
            timeout: Duration::from_millis(700),
            max_output_bytes: 1024,
        };
        let start = std::time::Instant::now();
        let out = spawn_with(sleeper(), limits);
        // Killed well before the ~4s sleeper would finish.
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "guard did not kill promptly"
        );
        assert!(out.timed_out, "a runaway must be flagged timed_out");
        assert!(!out.success(), "a timed-out run must fail closed");
        assert!(out.stderr.contains("resource limit"));
    }

    #[test]
    fn spawn_with_captures_a_fast_command() {
        let out = spawn_with(echoer(), ResourceLimits::default());
        assert!(out.success(), "fast command should succeed: {out:?}");
        assert!(!out.timed_out);
        assert!(out.stdout.contains("hello"), "stdout was {:?}", out.stdout);
    }

    #[test]
    fn timed_out_run_is_a_deterministic_failure() {
        let limits = ResourceLimits {
            timeout: Duration::from_millis(700),
            max_output_bytes: 1024,
        };
        let out = spawn_with(sleeper(), limits);
        assert!(out.timed_out);
        assert!(
            out.is_deterministic_failure(),
            "a resource-limit kill must be reported as deterministic"
        );
    }

    #[test]
    fn ordinary_failures_are_not_deterministic() {
        // A plain non-zero exit is a semantic failure: retryable.
        let nonzero = ExecOutcome {
            launched: true,
            code: Some(1),
            stdout: String::new(),
            stderr: "error: unknown identifier 'foo'".into(),
            timed_out: false,
            output_capped: false,
        };
        assert!(!nonzero.success());
        assert!(!nonzero.is_deterministic_failure());

        // Capped output on its own is not a deterministic failure (and here is
        // not a failure at all).
        let capped = ExecOutcome {
            output_capped: true,
            code: Some(0),
            ..nonzero.clone()
        };
        assert!(!capped.is_deterministic_failure());

        // Nor is a runner that could not start.
        let unlaunched = ExecOutcome::not_launched("no such file");
        assert!(!unlaunched.is_deterministic_failure());
    }

    #[test]
    fn resource_limits_default_is_sane() {
        let d = ResourceLimits::default();
        assert_eq!(d.timeout, Duration::from_secs(300));
        assert!(d.max_output_bytes >= 1024 * 1024);
    }
}
