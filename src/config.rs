use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub database: PathBuf,
    pub workspace: PathBuf,
    pub resources: PathBuf,
    pub model_command: Option<String>,
    pub max_iterations: u32,
    pub command_timeout_seconds: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            database: PathBuf::from(".theoremata/theoremata.db"),
            workspace: PathBuf::from(".theoremata/workspaces"),
            resources: PathBuf::from("resources"),
            model_command: std::env::var("THEOREMATA_MODEL_COMMAND").ok(),
            max_iterations: 3,
            command_timeout_seconds: 60,
        }
    }
}

impl Config {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let path = path.unwrap_or_else(|| Path::new(".theoremata/config.json"));
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        Ok(serde_json::from_str(&raw)
            .with_context(|| format!("parsing config {}", path.display()))?)
    }

    pub fn initialize(&self) -> Result<()> {
        if let Some(parent) = self.database.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::create_dir_all(&self.workspace)?;
        let path = Path::new(".theoremata/config.json");
        if !path.exists() {
            fs::write(path, serde_json::to_string_pretty(self)?)?;
        }
        Ok(())
    }
}
