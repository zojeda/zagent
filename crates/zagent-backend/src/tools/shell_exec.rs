use async_trait::async_trait;
use serde_json::Value;
use std::process::Stdio;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::shell_stream::{self, ShellStreamChunk};
use zagent_core::Result;
use zagent_core::tools::Tool;

/// Execute shell commands on the host system.
pub struct ShellExecTool {
    working_dir: String,
}

impl ShellExecTool {
    pub fn new(working_dir: impl Into<String>) -> Self {
        Self {
            working_dir: working_dir.into(),
        }
    }
}

#[async_trait]
impl Tool for ShellExecTool {
    fn name(&self) -> &str {
        "shell_exec"
    }

    fn description(&self) -> &str {
        "Execute a shell command and return its output (stdout + stderr). \
         Use this for running programs, installing packages, compiling code, \
         running tests, git operations, and any other command-line tasks."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute (passed to /bin/sh -c)"
                },
                "working_dir": {
                    "type": "string",
                    "description": "Working directory for the command. Defaults to the session working directory."
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Timeout in seconds (default: 60)"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| zagent_core::Error::tool("shell_exec", "Missing 'command' parameter"))?;

        let working_dir = args
            .get("working_dir")
            .and_then(|v| v.as_str())
            .unwrap_or(&self.working_dir);

        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(60);

        let stream_id = args
            .get("_zagent_tool_call_id")
            .and_then(|v| v.as_str())
            .unwrap_or("shell_exec");

        let mut child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(working_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| zagent_core::Error::tool("shell_exec", format!("Failed to spawn: {e}")))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| zagent_core::Error::tool("shell_exec", "Failed to capture stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| zagent_core::Error::tool("shell_exec", "Failed to capture stderr"))?;

        let stdout_stream_id = stream_id.to_string();
        let stderr_stream_id = stream_id.to_string();
        let stdout_task =
            tokio::spawn(
                async move { read_shell_stream(stdout, stdout_stream_id, "stdout").await },
            );
        let stderr_task =
            tokio::spawn(
                async move { read_shell_stream(stderr, stderr_stream_id, "stderr").await },
            );

        let timeout = tokio::time::sleep(std::time::Duration::from_secs(timeout_secs));
        tokio::pin!(timeout);

        let (status, timed_out) = tokio::select! {
            status = child.wait() => (
                status.map_err(|e| zagent_core::Error::tool("shell_exec", format!("Failed waiting for command: {e}")))?,
                false,
            ),
            _ = &mut timeout => {
                let _ = child.kill().await;
                let status = child.wait().await.map_err(|e| {
                    zagent_core::Error::tool("shell_exec", format!("Failed waiting for timed out command: {e}"))
                })?;
                (status, true)
            }
        };

        let stdout = stdout_task.await.map_err(|e| {
            zagent_core::Error::tool("shell_exec", format!("stdout task failed: {e}"))
        })??;
        let stderr = stderr_task.await.map_err(|e| {
            zagent_core::Error::tool("shell_exec", format!("stderr task failed: {e}"))
        })??;

        if timed_out {
            shell_stream::publish(ShellStreamChunk {
                stream_id: stream_id.to_string(),
                channel: "system".to_string(),
                text: format!("Command timed out after {timeout_secs}s\n"),
            });
            return Err(zagent_core::Error::tool(
                "shell_exec",
                format!("Command timed out after {timeout_secs}s: {command}"),
            ));
        }

        let exit_code = status.code().unwrap_or(-1);

        let mut result = String::new();
        result.push_str(&format!("Exit code: {exit_code}\n"));

        if !stdout.is_empty() {
            result.push_str(&format!("--- stdout ---\n{stdout}"));
        }
        if !stderr.is_empty() {
            result.push_str(&format!("--- stderr ---\n{stderr}"));
        }
        if stdout.is_empty() && stderr.is_empty() {
            result.push_str("(no output)\n");
        }

        Ok(result)
    }
}

async fn read_shell_stream<R>(
    mut reader: R,
    stream_id: String,
    channel: &'static str,
) -> Result<String>
where
    R: AsyncReadExt + Unpin,
{
    let mut output = String::new();
    let mut buf = [0u8; 2048];

    loop {
        let read = reader.read(&mut buf).await.map_err(|e| {
            zagent_core::Error::tool("shell_exec", format!("Failed reading {channel}: {e}"))
        })?;
        if read == 0 {
            break;
        }
        let text = String::from_utf8_lossy(&buf[..read]).to_string();
        output.push_str(&text);
        shell_stream::publish(ShellStreamChunk {
            stream_id: stream_id.clone(),
            channel: channel.to_string(),
            text,
        });
    }

    Ok(output)
}
