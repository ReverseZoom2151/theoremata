use crate::{config::Config, model::ToolResult};
use anyhow::{anyhow, Context, Result};
use serde_json::json;
use sha2::{Digest, Sha256};

/// Lowercase hex of a byte slice. sha2 0.11's digest output no longer implements
/// `LowerHex`, so we format the bytes explicitly.
fn hex_lower(bytes: impl AsRef<[u8]>) -> String {
    use std::fmt::Write as _;
    let bytes = bytes.as_ref();
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}
use std::{
    collections::HashMap,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    time::Instant,
};

pub trait Tool {
    fn name(&self) -> &str;
    fn available(&self) -> bool;
    fn run(&self, input: serde_json::Value) -> Result<ToolResult>;
}

/// True if `cmd <version_arg>` actually runs and exits successfully. This is the
/// reliable availability test: it rejects the Microsoft Store `python`/`python3`
/// stubs, which are on the Windows PATH but exit non-zero with an install prompt.
fn runs(cmd: &str, version_arg: &str) -> bool {
    Command::new(cmd)
        .arg(version_arg)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Resolve an executable that actually runs, as a spawnable command string.
/// Tries the bare candidate names against this process's PATH first (correct on
/// Linux, macOS, and WSL), then falls back to a login shell — which sources the
/// user's profile — to recover the absolute native path. Needed on Windows,
/// where a toolchain installed via elan/pip may be off the process PATH or
/// shadowed there by a Microsoft Store stub.
fn resolve_command(candidates: &[&str], version_arg: &str) -> Option<String> {
    for candidate in candidates {
        if runs(candidate, version_arg) {
            return Some((*candidate).to_owned());
        }
    }
    let names = candidates.join(" ");
    let script = format!(
        "for p in {names}; do if command -v \"$p\" >/dev/null 2>&1 && \"$p\" {version_arg} \
         >/dev/null 2>&1; then cygpath -w \"$(command -v \"$p\")\" 2>/dev/null || command -v \"$p\"; \
         break; fi; done"
    );
    let out = Command::new("bash").args(["-lc", &script]).output().ok()?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!path.is_empty() && runs(&path, version_arg)).then_some(path)
}

pub fn python_command() -> Option<String> {
    resolve_command(&["python3", "python"], "--version")
}

fn finish(
    name: &str,
    started: Instant,
    output: std::process::Output,
    metadata: serde_json::Value,
) -> ToolResult {
    ToolResult {
        tool: name.into(),
        success: output.status.success(),
        summary: if output.status.success() {
            "completed".into()
        } else {
            format!("exited with {}", output.status)
        },
        stdout: String::from_utf8_lossy(&output.stdout).into(),
        stderr: String::from_utf8_lossy(&output.stderr).into(),
        duration_ms: started.elapsed().as_millis(),
        metadata,
    }
}

pub struct MathlibSearch {
    root: PathBuf,
}
impl MathlibSearch {
    pub fn new(config: &Config) -> Self {
        Self {
            root: config
                .resources
                .join("mathlib4-master/mathlib4-master/Mathlib"),
        }
    }

    fn rg_command(&self) -> Option<String> {
        resolve_command(&["rg", "ripgrep"], "--version")
    }

    fn mathlib_root(&self) -> PathBuf {
        self.root
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.root.clone())
    }
}

impl Tool for MathlibSearch {
    fn name(&self) -> &str {
        "mathlib_search"
    }
    fn available(&self) -> bool {
        self.root.exists()
            && (self.rg_command().is_some() || PythonCheck::new().available())
    }
    fn run(&self, input: serde_json::Value) -> Result<ToolResult> {
        let query = input["query"]
            .as_str()
            .ok_or_else(|| anyhow!("query is required"))?;
        let limit = input["limit"].as_u64().unwrap_or(30).min(200);
        let started = Instant::now();
        if let Some(rg) = self.rg_command() {
            let output = Command::new(rg)
                .args(["-n", "-i", "-m", &limit.to_string(), query])
                .arg(&self.root)
                .output()
                .context("running ripgrep over Mathlib")?;
            return Ok(finish(
                self.name(),
                started,
                output,
                json!({"query":query,"root":self.root,"backend":"ripgrep"}),
            ));
        }
        let py = PythonCheck::new();
        if !py.available() {
            return Err(anyhow!(
                "mathlib_search requires ripgrep or the Python retrieval worker"
            ));
        }
        let result = py.run(json!({
            "tool": "retrieve",
            "root": self.mathlib_root(),
            "imports": ["Mathlib"],
            "query": query,
            "limit": limit,
            "op": "retrieve",
        }))?;
        Ok(ToolResult {
            tool: self.name().into(),
            success: result.success,
            summary: result.summary,
            stdout: result.stdout,
            stderr: result.stderr,
            duration_ms: started.elapsed().as_millis(),
            metadata: json!({
                "query": query,
                "root": self.mathlib_root(),
                "backend": "python_retrieve",
                "worker": result.metadata,
            }),
        })
    }
}

/// A `-c` bootstrap that puts every component's `python/` dir on `sys.path`
/// (the `theoremata_tools` namespace package is split across them) and runs a
/// worker module's `main`. Relative to the process cwd (the repo root).
pub fn python_bootstrap(module: &str) -> String {
    format!(
        "import sys,glob,os;\
         [sys.path.insert(0,os.path.abspath(p)) for p in sorted(glob.glob('components/*/python'))];\
         from theoremata_tools.{module} import main;main()"
    )
}

pub struct PythonCheck;
impl PythonCheck {
    pub fn new() -> Self {
        Self
    }
}
impl Tool for PythonCheck {
    fn name(&self) -> &str {
        "python_check"
    }
    fn available(&self) -> bool {
        PathBuf::from("components/tools/python/theoremata_tools/worker.py").exists()
            && python_command().is_some()
    }
    fn run(&self, input: serde_json::Value) -> Result<ToolResult> {
        let started = Instant::now();
        let bootstrap = python_bootstrap("worker");
        let python = python_command()
            .ok_or_else(|| anyhow!("no python interpreter found (tried python3, python)"))?;
        // `-E` (ignore PYTHON* env vars) rather than `-I`: the worker must be
        // able to import its trusted dependencies (SymPy, z3) from site-packages,
        // while untrusted *expressions* are sandboxed by safe_eval's AST allowlist
        // and empty builtins, not by process isolation.
        let mut child = Command::new(python)
            .args(["-E", "-c", &bootstrap])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.to_string().as_bytes())?;
        let output = child.wait_with_output()?;
        Ok(finish(
            self.name(),
            started,
            output,
            json!({"workers": "components/*/python"}),
        ))
    }
}

/// Process-global cache of Lean check results keyed on (project, file contents).
/// Recompiling identical Lean against the same Mathlib project is deterministic,
/// so a hit avoids a fresh (slow) `lake env lean` invocation.
fn lean_cache() -> &'static Mutex<HashMap<String, ToolResult>> {
    static CACHE: OnceLock<Mutex<HashMap<String, ToolResult>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub struct LeanCheck {
    project: Option<PathBuf>,
}
impl LeanCheck {
    /// Build a checker that compiles inside the configured Mathlib Lake project
    /// (when it exists) so `import Mathlib` resolves against its olean cache.
    pub fn new(config: &Config) -> Self {
        Self {
            project: config.lean_project.clone().filter(|p| p.exists()),
        }
    }
}
impl Tool for LeanCheck {
    fn name(&self) -> &str {
        "lean_check"
    }
    fn available(&self) -> bool {
        resolve_command(&["lake"], "--version").is_some()
            || resolve_command(&["lean"], "--version").is_some()
    }
    fn run(&self, input: serde_json::Value) -> Result<ToolResult> {
        let file = input["file"]
            .as_str()
            .ok_or_else(|| anyhow!("file is required"))?;
        // Absolute path: the working directory may change to the Lake project.
        let path = std::fs::canonicalize(file).unwrap_or_else(|_| PathBuf::from(file));

        // Cache identical (project, file contents) checks — the compile is
        // deterministic, so return a prior result rather than recompiling.
        let contents = std::fs::read_to_string(&path).unwrap_or_default();
        let project_key = self
            .project
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let key = hex_lower(Sha256::digest(format!("{project_key}\u{0}{contents}")));
        if let Some(mut hit) = lean_cache().lock().unwrap().get(&key).cloned() {
            hit.metadata["cached"] = json!(true);
            return Ok(hit);
        }

        let started = Instant::now();
        let output = match (&self.project, resolve_command(&["lake"], "--version")) {
            (Some(project), Some(lake)) => Command::new(lake)
                .current_dir(project)
                .args(["env", "lean"])
                .arg(&path)
                .output()?,
            (None, Some(lake)) => Command::new(lake)
                .args(["env", "lean"])
                .arg(&path)
                .output()?,
            (_, None) => {
                let lean = resolve_command(&["lean"], "--version")
                    .ok_or_else(|| anyhow!("no Lean toolchain found (tried lake, lean)"))?;
                Command::new(lean).arg(&path).output()?
            }
        };
        let result = finish(
            self.name(),
            started,
            output,
            json!({"file":file,"project":self.project,"cached":false}),
        );
        lean_cache().lock().unwrap().insert(key, result.clone());
        Ok(result)
    }
}

pub struct LeanParanoia {
    root: PathBuf,
}
impl LeanParanoia {
    pub fn new(config: &Config) -> Self {
        Self {
            root: config.resources.join("LeanParanoia-main/LeanParanoia-main"),
        }
    }
    /// The built `paranoia` executable, if present.
    fn exe(&self) -> Option<PathBuf> {
        for name in ["paranoia.exe", "paranoia"] {
            let candidate = self.root.join(".lake/build/bin").join(name);
            if candidate.exists() {
                return Some(candidate);
            }
        }
        None
    }
}
impl Tool for LeanParanoia {
    fn name(&self) -> &str {
        "lean_paranoia"
    }
    fn available(&self) -> bool {
        self.exe().is_some() && resolve_command(&["lake"], "--version").is_some()
    }
    fn run(&self, input: serde_json::Value) -> Result<ToolResult> {
        let theorem = input["theorem"]
            .as_str()
            .ok_or_else(|| anyhow!("theorem is required"))?;
        let started = Instant::now();
        let lake = resolve_command(&["lake"], "--version")
            .ok_or_else(|| anyhow!("no Lean toolchain found (lake)"))?;
        // Run the built exe under `lake env` (in the project dir) so it inherits
        // the correct LEAN_PATH; fall back to `lake exe paranoia` if unbuilt.
        let output = match self.exe() {
            Some(exe) => Command::new(lake)
                .current_dir(&self.root)
                .arg("env")
                .arg(&exe)
                .arg("--trust-modules")
                .arg("Std,Mathlib,Init")
                .arg(theorem)
                .output()?,
            None => Command::new(lake)
                .current_dir(&self.root)
                .args([
                    "exe",
                    "paranoia",
                    "--trust-modules",
                    "Std,Mathlib,Init",
                    theorem,
                ])
                .output()?,
        };
        Ok(finish(
            self.name(),
            started,
            output,
            json!({"theorem":theorem,"root":self.root,"trust_modules":"Std,Mathlib,Init"}),
        ))
    }
}

pub struct Comparator;

fn comparator_path() -> Option<String> {
    if let Ok(path) = std::env::var("THEOREMATA_COMPARATOR") {
        if std::path::Path::new(&path).exists() {
            return Some(path);
        }
    }
    resolve_command(&["comparator"], "--version")
}

impl Tool for Comparator {
    fn name(&self) -> &str {
        "comparator"
    }
    fn available(&self) -> bool {
        comparator_path().is_some()
    }
    fn run(&self, input: serde_json::Value) -> Result<ToolResult> {
        let config = input["config"]
            .as_str()
            .ok_or_else(|| anyhow!("config is required"))?;
        let started = Instant::now();
        let bin = comparator_path().unwrap_or_else(|| "comparator".to_string());
        let output = Command::new(bin).arg(config).output()?;
        Ok(finish(
            self.name(),
            started,
            output,
            json!({"config":config}),
        ))
    }
}

pub fn capability_report(config: &Config) -> serde_json::Value {
    let tools: Vec<Box<dyn Tool>> = vec![
        Box::new(MathlibSearch::new(config)),
        Box::new(PythonCheck::new()),
        Box::new(LeanCheck::new(config)),
        Box::new(LeanParanoia::new(config)),
        Box::new(Comparator),
    ];
    let lean_project = config
        .lean_project
        .clone()
        .unwrap_or_else(|| config.resources.join("mathlib4-master/mathlib4-master"));
    let corpora = [
        ("mathlib4", lean_project.exists()),
        ("datasets-main", config.resources.join("datasets-main").exists()),
        ("BRIDGE-main", config.resources.join("BRIDGE-main").exists()),
        ("QuantumLean-Bench-main", config.resources.join("QuantumLean-Bench-main").exists()),
        ("flare-main", config.resources.join("flare-main").exists()),
        ("IMO2025-main", config.resources.join("IMO2025-main").exists()),
        ("aristotle_putnam25-main", config.resources.join("aristotle_putnam25-main").exists()),
        (
            "LeanMillenniumPrizeProblems-main",
            config
                .resources
                .join("LeanMillenniumPrizeProblems-main")
                .exists(),
        ),
    ];
    // Live formal-system gates (Phase 2): each system's configured runner and a
    // real toolchain probe. Read-only, so `doctor` doubles as a live smoke.
    use crate::prover::formal::{backend_for, FormalSystem};
    let formal_backends = [
        FormalSystem::Lean,
        FormalSystem::Rocq,
        FormalSystem::Isabelle,
    ]
    .into_iter()
    .map(|system| {
        json!({
            "system": system.as_str(),
            "runner": config.formal_runners.for_system(system).tag(),
            "available": backend_for(config, system, false).available(),
        })
    })
    .collect::<Vec<_>>();
    json!({
        "model_provider": config.model_command.as_ref().map(|_|"command").unwrap_or("offline"),
        "tools": tools.into_iter().map(|t|json!({"name":t.name(),"available":t.available()})).collect::<Vec<_>>(),
        "formal_backends": formal_backends,
        "prover": {
            "backend": config.prover_backend,
            "max_polls": config.prover_max_polls,
            "aristotle_mock": std::env::var("THEOREMATA_ARISTOTLE_MOCK")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or_else(|_| std::env::var("THEOREMATA_ARISTOTLE_API_KEY").is_err()),
            "aristotle_live": std::env::var("THEOREMATA_ARISTOTLE_COMMAND").is_ok(),
            "leandojo": PathBuf::from("components/tools/python/theoremata_tools/worker.py").exists()
                && python_command().is_some(),
            "reprover": config.model_command.is_some()
                && PathBuf::from("components/tools/python/theoremata_tools/worker.py").exists(),
        },
        "corpora": corpora.into_iter().map(|(n,p)|json!({"name":n,"present":p})).collect::<Vec<_>>(),
        "env": {
            "THEOREMATA_RESOURCES": std::env::var("THEOREMATA_RESOURCES").ok(),
            "THEOREMATA_COMPARATOR": std::env::var("THEOREMATA_COMPARATOR").ok().map(|_|"set"),
            "THEOREMATA_ARISTOTLE_COMMAND": std::env::var("THEOREMATA_ARISTOTLE_COMMAND").ok().map(|_|"set"),
        },
        "trust_doc": "docs/TRUST_BOUNDARIES.md",
        "artifacts": config.artifacts,
        "lean_project": lean_project,
        "resources": config.resources,
        "database": config.database,
    })
}
