use crate::model::{ModelRequest, ModelResponse, ModelStreamEvent};
use anyhow::{anyhow, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

pub trait ModelProvider {
    fn complete(&self, request: &ModelRequest) -> Result<ModelResponse>;
    fn stream(
        &self,
        request: &ModelRequest,
        on_event: &mut dyn FnMut(ModelStreamEvent),
    ) -> Result<ModelResponse> {
        on_event(ModelStreamEvent::Started {
            provider: self.name().into(),
        });
        let response = self.complete(request)?;
        on_event(ModelStreamEvent::Completed {
            response: response.clone(),
        });
        Ok(response)
    }
    fn name(&self) -> &str;
}

pub struct CommandProvider {
    command: String,
}

impl CommandProvider {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
        }
    }
}

impl ModelProvider for CommandProvider {
    fn complete(&self, request: &ModelRequest) -> Result<ModelResponse> {
        self.stream(request, &mut |_| {})
    }

    fn stream(
        &self,
        request: &ModelRequest,
        on_event: &mut dyn FnMut(ModelStreamEvent),
    ) -> Result<ModelResponse> {
        on_event(ModelStreamEvent::Started {
            provider: self.name().into(),
        });
        let mut child = Command::new("bash")
            .args(["-lc", &self.command])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("starting model command: {}", self.command))?;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(serde_json::to_string(request)?.as_bytes())?;
        let stdout = child.stdout.take().context("capturing model stdout")?;
        let mut raw_lines = Vec::new();
        let mut final_response = None;
        for line in BufReader::new(stdout).lines() {
            let line = line?;
            if let Ok(event) = serde_json::from_str::<ModelStreamEvent>(&line) {
                if let ModelStreamEvent::Completed { response } = &event {
                    final_response = Some(response.clone());
                }
                on_event(event);
            } else {
                raw_lines.push(line);
            }
        }
        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err(anyhow!(
                "model command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        if let Some(response) = final_response {
            return Ok(response);
        }
        let raw = raw_lines.join("\n");
        let response = serde_json::from_str(&raw).unwrap_or_else(|_| ModelResponse {
            content: serde_json::json!({"message": raw.trim()}),
            model: "external-command".into(),
            provider: self.name().into(),
        });
        on_event(ModelStreamEvent::Completed {
            response: response.clone(),
        });
        Ok(response)
    }
    fn name(&self) -> &str {
        "command"
    }
}

pub struct OfflineProvider;
impl ModelProvider for OfflineProvider {
    fn complete(&self, request: &ModelRequest) -> Result<ModelResponse> {
        Err(anyhow!(
            "no model provider configured for role '{}'; set THEOREMATA_MODEL_COMMAND \
             to a command that reads a ModelRequest JSON object from stdin and writes ModelResponse JSON",
            request.role
        ))
    }
    fn name(&self) -> &str {
        "offline"
    }
}
