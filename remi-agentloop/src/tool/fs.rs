use async_stream::stream;
use futures::Stream;
use crate::error::AgentError;
use crate::tool::{Tool, ToolOutput, ToolResult};

/// Read/write files on the physical filesystem
pub struct FsTool;

impl Tool for FsTool {
    fn name(&self) -> &str { "fs" }
    fn description(&self) -> &str { "Read or write files on the local filesystem." }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "op": {
                    "type": "string",
                    "enum": ["read", "write", "list", "delete"],
                    "description": "Operation to perform"
                },
                "path": { "type": "string", "description": "File path" },
                "content": { "type": "string", "description": "Content for write operations" }
            },
            "required": ["op", "path"]
        })
    }

    fn execute(&self, arguments: serde_json::Value) -> impl std::future::Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>> {
        async move {
            let op = arguments["op"].as_str()
                .ok_or_else(|| AgentError::tool("fs", "missing 'op'"))?
                .to_string();
            let path = arguments["path"].as_str()
                .ok_or_else(|| AgentError::tool("fs", "missing 'path'"))?
                .to_string();

            Ok(ToolResult::Output(stream! {
                match op.as_str() {
                    "read" => {
                        match tokio::fs::read_to_string(&path).await {
                            Ok(content) => yield ToolOutput::Result(content),
                            Err(e) => yield ToolOutput::Result(format!("error: {}", e)),
                        }
                    }
                    "write" => {
                        let content = arguments["content"].as_str().unwrap_or("").to_string();
                        match tokio::fs::write(&path, &content).await {
                            Ok(_) => yield ToolOutput::Result(format!("written {} bytes to {}", content.len(), path)),
                            Err(e) => yield ToolOutput::Result(format!("error: {}", e)),
                        }
                    }
                    "list" => {
                        match tokio::fs::read_dir(&path).await {
                            Err(e) => yield ToolOutput::Result(format!("error: {}", e)),
                            Ok(mut dir) => {
                                let mut entries = Vec::new();
                                loop {
                                    match dir.next_entry().await {
                                        Ok(None) => break,
                                        Ok(Some(entry)) => entries.push(entry.file_name().to_string_lossy().to_string()),
                                        Err(e) => { yield ToolOutput::Result(format!("error: {}", e)); return; }
                                    }
                                }
                                yield ToolOutput::Result(entries.join("\n"));
                            }
                        }
                    }
                    "delete" => {
                        match tokio::fs::remove_file(&path).await {
                            Ok(_) => yield ToolOutput::Result(format!("deleted {}", path)),
                            Err(e) => yield ToolOutput::Result(format!("error: {}", e)),
                        }
                    }
                    _ => yield ToolOutput::Result(format!("unknown op: {}", op)),
                }
            }))
        }
    }
}
