use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use async_stream::stream;
use futures::Stream;
use remi_core::error::AgentError;
use remi_core::tool::{Tool, ToolContext, ToolOutput, ToolResult};
use remi_core::types::ResumePayload;

/// In-memory virtual filesystem tool
#[derive(Clone)]
pub struct VirtualFsTool {
    files: Arc<Mutex<HashMap<String, String>>>,
}

impl VirtualFsTool {
    pub fn new() -> Self {
        Self { files: Arc::new(Mutex::new(HashMap::new())) }
    }

    pub fn with_file(self, path: impl Into<String>, content: impl Into<String>) -> Self {
        self.files.lock().unwrap().insert(path.into(), content.into());
        self
    }
}

impl Tool for VirtualFsTool {
    fn name(&self) -> &str { "vfs" }
    fn description(&self) -> &str { "Read or write files in an in-memory virtual filesystem." }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "op": { "type": "string", "enum": ["read", "write", "list", "delete"] },
                "path": { "type": "string" },
                "content": { "type": "string" }
            },
            "required": ["op", "path"]
        })
    }

    fn execute(&self, arguments: serde_json::Value, _resume: Option<ResumePayload>, _ctx: &ToolContext) -> impl std::future::Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>> {
        let files = self.files.clone();
        async move {
            let op = arguments["op"].as_str().unwrap_or("").to_string();
            let path = arguments["path"].as_str().unwrap_or("").to_string();

            Ok(ToolResult::Output(stream! {
                match op.as_str() {
                    "read" => {
                        let files = files.lock().unwrap();
                        match files.get(&path) {
                            Some(content) => yield ToolOutput::Result(content.clone()),
                            None => yield ToolOutput::Result(format!("error: file not found: {}", path)),
                        }
                    }
                    "write" => {
                        let content = arguments["content"].as_str().unwrap_or("").to_string();
                        let len = content.len();
                        files.lock().unwrap().insert(path.clone(), content);
                        yield ToolOutput::Result(format!("written {} bytes to {}", len, path));
                    }
                    "list" => {
                        let files = files.lock().unwrap();
                        let entries: Vec<_> = files.keys().cloned().collect();
                        yield ToolOutput::Result(entries.join("\n"));
                    }
                    "delete" => {
                        let removed = files.lock().unwrap().remove(&path).is_some();
                        if removed {
                            yield ToolOutput::Result(format!("deleted {}", path));
                        } else {
                            yield ToolOutput::Result(format!("error: file not found: {}", path));
                        }
                    }
                    _ => yield ToolOutput::Result(format!("unknown op: {}", op)),
                }
            }))
        }
    }
}
