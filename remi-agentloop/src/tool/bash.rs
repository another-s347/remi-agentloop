use async_stream::stream;
use futures::Stream;
use crate::error::AgentError;
use crate::tool::{Tool, ToolOutput, ToolResult};

/// Executes shell commands (bash -c)
pub struct BashTool;

impl Tool for BashTool {
    fn name(&self) -> &str { "bash" }
    fn description(&self) -> &str { "Execute a bash shell command and return the output." }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The shell command to execute" },
                "timeout_ms": { "type": "integer", "description": "Optional timeout in milliseconds" }
            },
            "required": ["command"]
        })
    }

    fn execute(&self, arguments: serde_json::Value) -> impl std::future::Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>> {
        async move {
            let command = arguments["command"].as_str()
                .ok_or_else(|| AgentError::tool("bash", "missing 'command' argument"))?
                .to_string();

            Ok(ToolResult::Output(stream! {
                yield ToolOutput::Delta(format!("$ {}", command));
                let output = tokio::process::Command::new("bash")
                    .arg("-c")
                    .arg(&command)
                    .output()
                    .await;
                match output {
                    Err(e) => yield ToolOutput::Result(format!("error: {}", e)),
                    Ok(out) => {
                        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                        let result = if !stderr.is_empty() {
                            format!("stdout:\n{}\nstderr:\n{}", stdout, stderr)
                        } else {
                            stdout
                        };
                        yield ToolOutput::Result(result);
                    }
                }
            }))
        }
    }
}
