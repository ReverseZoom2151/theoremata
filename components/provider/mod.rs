use crate::{
    model::{ModelRequest, ModelResponse, ModelStreamEvent},
    model_router::ModelEndpoint,
};
use anyhow::{anyhow, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

pub trait ModelProvider {
    fn complete(&self, request: &ModelRequest) -> Result<ModelResponse>;

    /// Execute a concrete routed endpoint. Providers that do not support model
    /// selection inherit this implementation: it preserves their historical
    /// completion path while making the requested endpoint explicit in the
    /// request context. Command providers therefore receive the selected model
    /// even though the command itself remains configured out of band.
    fn complete_at(
        &self,
        endpoint: &ModelEndpoint,
        request: &ModelRequest,
    ) -> Result<ModelResponse> {
        self.complete(&request_for_endpoint(request, endpoint))
    }
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

/// Add the requested endpoint under a reserved context key without discarding
/// caller-provided context. Non-object contexts are preserved under
/// `request_context`, so every routed command receives an inspectable endpoint
/// choice and no legacy request shape is silently lost.
pub fn request_for_endpoint(request: &ModelRequest, endpoint: &ModelEndpoint) -> ModelRequest {
    let mut routed = request.clone();
    let endpoint = serde_json::json!({
        "provider": endpoint.provider,
        "model": endpoint.model,
        "tier": endpoint.tier,
    });
    match &mut routed.context {
        serde_json::Value::Object(context) => {
            context.insert("model_endpoint".into(), endpoint);
        }
        original => {
            let previous = std::mem::take(original);
            *original = serde_json::json!({
                "request_context": previous,
                "model_endpoint": endpoint,
            });
        }
    }
    routed
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_router::ModelTier;
    use serde_json::json;

    fn request(context: serde_json::Value) -> ModelRequest {
        ModelRequest {
            role: "test".into(),
            task: "route".into(),
            context,
            output_schema: json!({}),
        }
    }

    #[test]
    fn endpoint_is_carried_without_losing_object_context() {
        let endpoint = ModelEndpoint::new("command", "fast-model", ModelTier::Fast);
        let routed = request_for_endpoint(&request(json!({"goal": "True"})), &endpoint);
        assert_eq!(routed.context["goal"], "True");
        assert_eq!(routed.context["model_endpoint"]["provider"], "command");
        assert_eq!(routed.context["model_endpoint"]["model"], "fast-model");
        assert_eq!(routed.context["model_endpoint"]["tier"], "fast");
    }

    #[test]
    fn endpoint_wraps_non_object_context_without_dropping_it() {
        let endpoint = ModelEndpoint::new("command", "strong-model", ModelTier::Strong);
        let routed = request_for_endpoint(&request(json!(["legacy", "context"])), &endpoint);
        assert_eq!(
            routed.context["request_context"],
            json!(["legacy", "context"])
        );
        assert_eq!(routed.context["model_endpoint"]["model"], "strong-model");
    }
}
