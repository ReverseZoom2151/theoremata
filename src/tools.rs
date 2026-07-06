use crate::{config::Config, model::ToolResult};
use anyhow::{anyhow, Context, Result};
use serde_json::json;
use std::{
    path::PathBuf,
    process::{Command, Stdio},
    time::Instant,
};

pub trait Tool {
    fn name(&self) -> &str;
    fn available(&self) -> bool;
    fn run(&self, input: serde_json::Value) -> Result<ToolResult>;
}

fn command_exists(name: &str) -> bool {
    Command::new("bash")
        .args(["-lc", &format!("command -v {name}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
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

fn python_command() -> Option<String> {
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
}
impl Tool for MathlibSearch {
    fn name(&self) -> &str {
        "mathlib_search"
    }
    fn available(&self) -> bool {
        self.root.exists() && command_exists("rg")
    }
    fn run(&self, input: serde_json::Value) -> Result<ToolResult> {
        let query = input["query"]
            .as_str()
            .ok_or_else(|| anyhow!("query is required"))?;
        let limit = input["limit"].as_u64().unwrap_or(30).min(200);
        let started = Instant::now();
        let output = Command::new("rg")
            .args(["-n", "-i", "-m", &limit.to_string(), query])
            .arg(&self.root)
            .output()
            .context("running ripgrep over Mathlib")?;
        Ok(finish(
            self.name(),
            started,
            output,
            json!({"query":query,"root":self.root}),
        ))
    }
}

pub struct PythonCheck {
    package_root: PathBuf,
}
impl PythonCheck {
    pub fn new() -> Self {
        Self {
            package_root: PathBuf::from("python"),
        }
    }
}
impl Tool for PythonCheck {
    fn name(&self) -> &str {
        "python_check"
    }
    fn available(&self) -> bool {
        self.package_root
            .join("theoremata_tools/worker.py")
            .exists()
            && python_command().is_some()
    }
    fn run(&self, input: serde_json::Value) -> Result<ToolResult> {
        let started = Instant::now();
        let bootstrap = format!(
            "import sys;sys.path.insert(0,{:?});from theoremata_tools.worker import main;main()",
            self.package_root.canonicalize()?.to_string_lossy()
        );
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
            json!({"package_root":self.package_root}),
        ))
    }
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
        let started = Instant::now();
        let output = match (&self.project, resolve_command(&["lake"], "--version")) {
            (Some(project), Some(lake)) => Command::new(lake)
                .current_dir(project)
                .args(["env", "lean"])
                .arg(&path)
                .output()?,
            (None, Some(lake)) => Command::new(lake).args(["env", "lean"]).arg(&path).output()?,
            (_, None) => {
                let lean = resolve_command(&["lean"], "--version")
                    .ok_or_else(|| anyhow!("no Lean toolchain found (tried lake, lean)"))?;
                Command::new(lean).arg(&path).output()?
            }
        };
        Ok(finish(
            self.name(),
            started,
            output,
            json!({"file":file,"project":self.project}),
        ))
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
}
impl Tool for LeanParanoia {
    fn name(&self) -> &str {
        "lean_paranoia"
    }
    fn available(&self) -> bool {
        self.root.exists() && command_exists("lake")
    }
    fn run(&self, input: serde_json::Value) -> Result<ToolResult> {
        let theorem = input["theorem"]
            .as_str()
            .ok_or_else(|| anyhow!("theorem is required"))?;
        let started = Instant::now();
        let output = Command::new("lake")
            .current_dir(&self.root)
            .args(["exe", "paranoia", theorem])
            .output()?;
        Ok(finish(
            self.name(),
            started,
            output,
            json!({"theorem":theorem}),
        ))
    }
}

pub struct Comparator;
impl Tool for Comparator {
    fn name(&self) -> &str {
        "comparator"
    }
    fn available(&self) -> bool {
        command_exists("comparator")
    }
    fn run(&self, input: serde_json::Value) -> Result<ToolResult> {
        let config = input["config"]
            .as_str()
            .ok_or_else(|| anyhow!("config is required"))?;
        let started = Instant::now();
        let output = Command::new("comparator").arg(config).output()?;
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
    json!({
        "model_provider": config.model_command.as_ref().map(|_|"command").unwrap_or("offline"),
        "tools": tools.into_iter().map(|t|json!({"name":t.name(),"available":t.available()})).collect::<Vec<_>>(),
        "resources": config.resources,
        "database": config.database,
    })
}
