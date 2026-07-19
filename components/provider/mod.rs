use crate::{
    model::{ModelRequest, ModelResponse, ModelStreamEvent},
    model_router::ModelEndpoint,
};
use anyhow::{anyhow, Context, Result};
use std::{
    io::{self, Read, Write},
    process::{Child, ChildStderr, ChildStdout, Command, Stdio},
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
    thread,
    time::{Duration, Instant},
};

const DEFAULT_COMMAND_TIMEOUT_SECONDS: u64 = 60;
const DEFAULT_COMMAND_MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const PIPE_DRAIN_GRACE: Duration = Duration::from_secs(1);
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(20);

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
    limits: CommandProviderLimits,
}

/// Resource limits for an external model command. `new` obtains these from
/// `THEOREMATA_MODEL_COMMAND_TIMEOUT_SECONDS` and
/// `THEOREMATA_MODEL_COMMAND_MAX_OUTPUT_BYTES`, falling back to conservative
/// defaults when either value is missing or invalid.
#[derive(Debug, Clone, Copy)]
pub struct CommandProviderLimits {
    pub timeout: Duration,
    /// Per-stream capture limit. Both stdout and stderr continue to be drained
    /// after this limit is crossed, but the command is terminated and fails.
    pub max_output_bytes: usize,
}

impl CommandProviderLimits {
    fn from_env() -> Self {
        Self {
            timeout: Duration::from_secs(env_positive_u64(
                "THEOREMATA_MODEL_COMMAND_TIMEOUT_SECONDS",
                DEFAULT_COMMAND_TIMEOUT_SECONDS,
            )),
            max_output_bytes: env_positive_usize(
                "THEOREMATA_MODEL_COMMAND_MAX_OUTPUT_BYTES",
                DEFAULT_COMMAND_MAX_OUTPUT_BYTES,
            ),
        }
    }
}

impl CommandProvider {
    pub fn new(command: impl Into<String>) -> Self {
        Self::with_limits(command, CommandProviderLimits::from_env())
    }

    /// Construct a command provider with explicit bounds. This is useful for
    /// embeddings that already own configuration, and for deterministic tests.
    pub fn with_limits(command: impl Into<String>, limits: CommandProviderLimits) -> Self {
        Self {
            command: command.into(),
            limits,
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

        // All three pipes are serviced independently. In particular, a model
        // command that fills stderr must not prevent stdout events from being
        // read or make `wait` hang on a full OS pipe buffer.
        let stdout = child.stdout.take().context("capturing model stdout")?;
        let stderr = child.stderr.take().context("capturing model stderr")?;
        let stdin = child.stdin.take().context("capturing model stdin")?;
        let request_json = serde_json::to_vec(request)?;
        let (line_tx, line_rx) = mpsc::channel();
        let (limit_tx, limit_rx) = mpsc::channel();
        let stdout_done = spawn_stdout_drain(
            stdout,
            self.limits.max_output_bytes,
            line_tx,
            limit_tx.clone(),
        );
        let stderr_done = spawn_stderr_drain(stderr, self.limits.max_output_bytes, limit_tx);
        let stdin_done = spawn_stdin_write(stdin, request_json);

        let mut raw_lines = Vec::new();
        let mut final_response = None;
        let deadline = Instant::now() + self.limits.timeout;
        let mut timed_out = false;
        let mut exceeded_stream = None;
        let mut stdin_result = None;

        let status = loop {
            if let Err(error) =
                drain_stream_lines(&line_rx, &mut raw_lines, &mut final_response, on_event)
            {
                abort_child(&mut child);
                return Err(error);
            }
            if stdin_result.is_none() {
                stdin_result = poll_stdin_result(&stdin_done);
            }
            if exceeded_stream.is_none() {
                exceeded_stream = poll_limit(&limit_rx);
            }
            if matches!(stdin_result, Some(Err(_))) || exceeded_stream.is_some() {
                terminate_child(&mut child);
                break child
                    .wait()
                    .context("waiting for terminated model command")?;
            }
            if let Some(status) = child.try_wait().context("polling model command")? {
                break status;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                timed_out = true;
                terminate_child(&mut child);
                break child
                    .wait()
                    .context("waiting for timed-out model command")?;
            }
            match line_rx.recv_timeout(remaining.min(PROCESS_POLL_INTERVAL)) {
                Ok(line) => {
                    if let Err(error) =
                        process_stream_line(line, &mut raw_lines, &mut final_response, on_event)
                    {
                        abort_child(&mut child);
                        return Err(error);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {}
            }
        };

        // The child has exited. Let the drainers finish within a bounded grace
        // period, then flush every stdout line they already delivered.
        let stdout_capture = receive_drain_result(stdout_done, "stdout")?;
        let stderr_capture = receive_drain_result(stderr_done, "stderr")?;
        drain_stream_lines(&line_rx, &mut raw_lines, &mut final_response, on_event)?;
        let stdin_result = stdin_result.unwrap_or_else(|| receive_stdin_result(stdin_done));

        if timed_out {
            return Err(anyhow!(
                "model command timed out after {}s: {}",
                self.limits.timeout.as_secs_f64(),
                self.command
            ));
        }
        if let Some(stream) = exceeded_stream.or_else(|| {
            stdout_capture
                .truncated
                .then_some("stdout")
                .or_else(|| stderr_capture.truncated.then_some("stderr"))
        }) {
            return Err(anyhow!(
                "model command exceeded the {} output limit of {} bytes: {}",
                stream,
                self.limits.max_output_bytes,
                self.command
            ));
        }
        if let Err(error) = stdin_result {
            return Err(anyhow!("writing model request to command stdin: {error}"));
        }
        if !status.success() {
            return Err(anyhow!(
                "model command failed: {}",
                String::from_utf8_lossy(&stderr_capture.bytes)
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

#[derive(Debug)]
struct DrainCapture {
    bytes: Vec<u8>,
    truncated: bool,
}

fn env_positive_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value: &u64| *value > 0)
        .unwrap_or(default)
}

fn env_positive_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value: &usize| *value > 0)
        .unwrap_or(default)
}

fn spawn_stdin_write(
    mut stdin: std::process::ChildStdin,
    request_json: Vec<u8>,
) -> Receiver<Result<(), String>> {
    let (done_tx, done_rx) = mpsc::channel();
    thread::spawn(move || {
        let result = stdin
            .write_all(&request_json)
            .and_then(|_| stdin.flush())
            .map_err(|error| error.to_string());
        let _ = done_tx.send(result);
    });
    done_rx
}

fn spawn_stdout_drain(
    stdout: ChildStdout,
    max_bytes: usize,
    line_tx: Sender<Vec<u8>>,
    limit_tx: Sender<&'static str>,
) -> Receiver<Result<DrainCapture, String>> {
    let (done_tx, done_rx) = mpsc::channel();
    thread::spawn(move || {
        let result =
            drain_stdout(stdout, max_bytes, line_tx, limit_tx).map_err(|error| error.to_string());
        let _ = done_tx.send(result);
    });
    done_rx
}

fn spawn_stderr_drain(
    stderr: ChildStderr,
    max_bytes: usize,
    limit_tx: Sender<&'static str>,
) -> Receiver<Result<DrainCapture, String>> {
    let (done_tx, done_rx) = mpsc::channel();
    thread::spawn(move || {
        let result =
            drain_capture(stderr, max_bytes, "stderr", limit_tx).map_err(|error| error.to_string());
        let _ = done_tx.send(result);
    });
    done_rx
}

fn drain_stdout(
    stdout: ChildStdout,
    max_bytes: usize,
    line_tx: Sender<Vec<u8>>,
    limit_tx: Sender<&'static str>,
) -> io::Result<DrainCapture> {
    let mut reader = io::BufReader::new(stdout);
    let mut captured = Vec::new();
    let mut line_start = 0;
    let mut truncated = false;
    let mut buffer = [0u8; 8192];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        for byte in &buffer[..read] {
            if !truncated {
                if captured.len() == max_bytes {
                    truncated = true;
                    let _ = limit_tx.send("stdout");
                    continue;
                }
                captured.push(*byte);
                if *byte == b'\n' {
                    let _ = line_tx.send(captured[line_start..captured.len()].to_vec());
                    line_start = captured.len();
                }
            }
        }
    }
    if !truncated && line_start < captured.len() {
        let _ = line_tx.send(captured[line_start..].to_vec());
    }
    Ok(DrainCapture {
        bytes: captured,
        truncated,
    })
}

fn drain_capture<R: Read>(
    reader: R,
    max_bytes: usize,
    stream: &'static str,
    limit_tx: Sender<&'static str>,
) -> io::Result<DrainCapture> {
    let mut reader = io::BufReader::new(reader);
    let mut captured = Vec::new();
    let mut truncated = false;
    let mut buffer = [0u8; 8192];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if !truncated {
            let available = max_bytes.saturating_sub(captured.len());
            let keep = available.min(read);
            captured.extend_from_slice(&buffer[..keep]);
            if keep < read {
                truncated = true;
                let _ = limit_tx.send(stream);
            }
        }
    }
    Ok(DrainCapture {
        bytes: captured,
        truncated,
    })
}

fn terminate_child(child: &mut Child) {
    let _ = child.kill();
}

fn abort_child(child: &mut Child) {
    terminate_child(child);
    let _ = child.wait();
}

fn receive_drain_result(
    receiver: Receiver<Result<DrainCapture, String>>,
    stream: &str,
) -> Result<DrainCapture> {
    receiver
        .recv_timeout(PIPE_DRAIN_GRACE)
        .map_err(|_| anyhow!("model command {stream} pipe did not close after process exit"))?
        .map_err(|error| anyhow!("reading model command {stream}: {error}"))
}

fn receive_stdin_result(receiver: Receiver<Result<(), String>>) -> Result<(), String> {
    receiver
        .recv_timeout(PIPE_DRAIN_GRACE)
        .map_err(|_| "model command stdin pipe did not close after process exit".to_string())?
}

fn poll_stdin_result(receiver: &Receiver<Result<(), String>>) -> Option<Result<(), String>> {
    match receiver.try_recv() {
        Ok(result) => Some(result),
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => Some(Err(
            "model command stdin writer disconnected unexpectedly".into(),
        )),
    }
}

fn poll_limit(receiver: &Receiver<&'static str>) -> Option<&'static str> {
    match receiver.try_recv() {
        Ok(stream) => Some(stream),
        Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => None,
    }
}

fn drain_stream_lines(
    receiver: &Receiver<Vec<u8>>,
    raw_lines: &mut Vec<String>,
    final_response: &mut Option<ModelResponse>,
    on_event: &mut dyn FnMut(ModelStreamEvent),
) -> Result<()> {
    while let Ok(line) = receiver.try_recv() {
        process_stream_line(line, raw_lines, final_response, on_event)?;
    }
    Ok(())
}

fn process_stream_line(
    line: Vec<u8>,
    raw_lines: &mut Vec<String>,
    final_response: &mut Option<ModelResponse>,
    on_event: &mut dyn FnMut(ModelStreamEvent),
) -> Result<()> {
    let line = String::from_utf8(line)
        .map_err(|_| anyhow!("model command wrote non-UTF-8 data to stdout"))?;
    let line = line.trim_end_matches(['\r', '\n']);
    if let Ok(event) = serde_json::from_str::<ModelStreamEvent>(line) {
        if let ModelStreamEvent::Completed { response } = &event {
            *final_response = Some(response.clone());
        }
        on_event(event);
    } else {
        raw_lines.push(line.into());
    }
    Ok(())
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
    use std::time::{Duration, Instant};

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

    fn bounded_command(
        command: &str,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> CommandProvider {
        CommandProvider::with_limits(
            command,
            CommandProviderLimits {
                timeout,
                max_output_bytes,
            },
        )
    }

    fn bash_available() -> bool {
        // The repository's command-provider contract is Bash-based. Windows
        // cargo tests in this checkout do not have a runnable WSL Bash, while
        // the Linux CI jobs do; avoid noisy failed launches locally.
        if cfg!(windows) {
            return false;
        }
        Command::new("bash")
            .args(["-lc", "exit 0"])
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    #[test]
    fn command_provider_drains_a_full_stderr_pipe_while_reading_stdout() {
        if !bash_available() {
            return;
        }
        // This is substantially larger than common OS pipe buffers. The old
        // stdout-then-stderr implementation could wait forever here.
        let provider = bounded_command(
            r#"printf '%*s' 262144 '' >&2; printf '%s\n' '{"content":{"ok":true},"model":"m","provider":"p"}'"#,
            Duration::from_secs(2),
            512 * 1024,
        );
        let response = provider.complete(&request(json!({}))).unwrap();
        assert_eq!(response.content, json!({"ok": true}));
    }

    #[test]
    fn command_provider_timeout_is_bounded_and_explicit() {
        if !bash_available() {
            return;
        }
        let provider = bounded_command("sleep 5", Duration::from_millis(100), 1024);
        let started = Instant::now();
        let error = provider.complete(&request(json!({}))).unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(error.to_string().contains("timed out"));
    }

    #[test]
    fn command_provider_rejects_capped_stdout_without_hanging() {
        if !bash_available() {
            return;
        }
        let provider = bounded_command("printf '0123456789abcdef'", Duration::from_secs(2), 8);
        let error = provider.complete(&request(json!({}))).unwrap_err();
        assert!(error.to_string().contains("stdout output limit"));
    }

    #[test]
    fn command_provider_preserves_stream_events_and_does_not_duplicate_completion() {
        if !bash_available() {
            return;
        }
        let provider = bounded_command(
            r#"printf '%s\n' '{"type":"delta","text":"first"}'; printf '%s\n' '{"type":"completed","response":{"content":{"answer":42},"model":"m","provider":"p"}}'"#,
            Duration::from_secs(2),
            1024,
        );
        let mut events = Vec::new();
        let response = provider
            .stream(&request(json!({})), &mut |event| events.push(event))
            .unwrap();
        assert_eq!(response.content, json!({"answer": 42}));
        assert!(matches!(
            events.first(),
            Some(ModelStreamEvent::Started { .. })
        ));
        assert!(matches!(events.get(1), Some(ModelStreamEvent::Delta { text }) if text == "first"));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, ModelStreamEvent::Completed { .. }))
                .count(),
            1
        );
    }
}
