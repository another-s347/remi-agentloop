use async_stream::stream;
use futures::Stream;
use remi_core::error::AgentError;
use remi_core::tool::{parse_arguments, schema_for_type, Tool, ToolOutput, ToolResult};
use remi_core::types::{ChatCtx, ResumePayload};
use schemars::JsonSchema;
use serde::Deserialize;

#[allow(dead_code)]
#[derive(Debug, Deserialize, JsonSchema)]
struct BashArgs {
    command: String,
    timeout_ms: Option<i64>,
}

/// Executes shell commands (bash -c)
pub struct BashTool;

impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }
    fn description(&self) -> &str {
        "Execute a bash shell command and return the output."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        schema_for_type::<BashArgs>()
    }

    fn execute(
        &self,
        arguments: serde_json::Value,
        _resume: Option<ResumePayload>,
        _ctx: ChatCtx,
    ) -> impl std::future::Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>>
    {
        async move {
            let BashArgs {
                command,
                timeout_ms: _,
            } = parse_arguments("bash", arguments)?;

            Ok(ToolResult::Output(stream! {
                yield ToolOutput::Delta(format!("$ {}", command));
                let output = tokio::process::Command::new("bash")
                    .arg("-c")
                    .arg(&command)
                    .output()
                    .await;
                match output {
                    Err(e) => yield ToolOutput::text(format!("error: {}", e)),
                    Ok(out) => {
                        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                        let result = if !stderr.is_empty() {
                            format!("stdout:\n{}\nstderr:\n{}", stdout, stderr)
                        } else {
                            stdout
                        };
                        yield ToolOutput::text(result);
                    }
                }
            }))
        }
    }
}
