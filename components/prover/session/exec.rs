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
    path::Path,
    process::{Command, Stdio},
};

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
}

impl ExecOutcome {
    pub fn success(&self) -> bool {
        self.launched && self.code == Some(0)
    }

    fn from_output(out: std::process::Output) -> Self {
        Self {
            launched: true,
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    fn not_launched(err: impl std::fmt::Display) -> Self {
        Self {
            launched: false,
            code: None,
            stdout: String::new(),
            stderr: err.to_string(),
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

fn spawn(mut cmd: Command) -> ExecOutcome {
    cmd.stdin(Stdio::null());
    match cmd.output() {
        Ok(out) => ExecOutcome::from_output(out),
        Err(e) => ExecOutcome::not_launched(e),
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
