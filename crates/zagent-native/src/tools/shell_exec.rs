use async_trait::async_trait;
use serde_json::Value;
use std::process::Stdio;
use tokio::process::Command;

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

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            Command::new("sh")
                .arg("-c")
                .arg(command)
                .current_dir(working_dir)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output(),
        )
        .await
        .map_err(|_| {
            zagent_core::Error::tool(
                "shell_exec",
                format!("Command timed out after {timeout_secs}s: {command}"),
            )
        })?
        .map_err(|e| zagent_core::Error::tool("shell_exec", format!("Failed to spawn: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let exit_code = output.status.code().unwrap_or(-1);

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
