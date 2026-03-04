//! Workspace-rooted filesystem tools and bash tool.
//!
//! All `Rooted*` tools sandbox every path under a root directory:
//! `/etc/passwd` → `<root>/etc/passwd`, `src/main.rs` → `<root>/src/main.rs`.
//!
//! `WorkspaceBashTool` runs real `bash -c` with `current_dir` set to the root
//! so relative paths inside scripts resolve correctly.

use async_stream::stream;
use futures::Stream;
use std::path::{Path, PathBuf};

use remi_core::error::AgentError;
use remi_core::tool::{Tool, ToolContext, ToolOutput, ToolResult};
use remi_core::types::ResumePayload;

// ── Common helper ─────────────────────────────────────────────────────────────

/// Join `root` with `path`, stripping any leading `/` so that absolute paths
/// are treated as relative to the root rather than escaping it.
fn resolve(root: &Path, path: &str) -> PathBuf {
    let stripped = path.trim_start_matches('/');
    root.join(stripped)
}

// ── WorkspaceBashTool ─────────────────────────────────────────────────────────

/// Real `bash -c` executor with its working directory set to the workspace root.
/// Relative paths in scripts resolve under the workspace directory.
pub struct WorkspaceBashTool {
    pub root: PathBuf,
}

impl WorkspaceBashTool {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl Tool for WorkspaceBashTool {
    fn name(&self) -> &str { "bash" }
    fn description(&self) -> &str {
        "Execute a bash shell command. Working directory is the agent workspace. \
         Relative paths resolve under the workspace."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command":    { "type": "string",  "description": "The shell command to execute" },
                "timeout_ms": { "type": "integer", "description": "Optional timeout in milliseconds" }
            },
            "required": ["command"]
        })
    }
    fn execute(
        &self,
        arguments: serde_json::Value,
        _resume: Option<ResumePayload>,
        _ctx: &ToolContext,
    ) -> impl std::future::Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>>
    {
        let root = self.root.clone();
        async move {
            let command = arguments["command"]
                .as_str()
                .ok_or_else(|| AgentError::tool("bash", "missing 'command' argument"))?
                .to_string();

            Ok(ToolResult::Output(stream! {
                yield ToolOutput::Delta(format!("$ {}", command));
                // Ensure workspace dir exists before running
                let _ = std::fs::create_dir_all(&root);
                let output = tokio::process::Command::new("bash")
                    .arg("-c")
                    .arg(&command)
                    .current_dir(&root)
                    .output()
                    .await;
                match output {
                    Err(e) => yield ToolOutput::Result(format!("error: {}", e)),
                    Ok(out) => {
                        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                        let exit_code = out.status.code().unwrap_or(-1);
                        let mut result = String::new();
                        if !stdout.is_empty() { result.push_str(&stdout); }
                        if !stderr.is_empty() {
                            if !result.is_empty() { result.push('\n'); }
                            result.push_str("[stderr] ");
                            result.push_str(&stderr);
                        }
                        if exit_code != 0 {
                            result.push_str(&format!("\n[exit {}]", exit_code));
                        }
                        yield ToolOutput::Result(result);
                    }
                }
            }))
        }
    }
}

// ── RootedFsReadTool ──────────────────────────────────────────────────────────

pub struct RootedFsReadTool { pub root: PathBuf }

impl Tool for RootedFsReadTool {
    fn name(&self) -> &str { "fs_read" }
    fn description(&self) -> &str {
        "Read a file in the workspace. Path is relative to the workspace root."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path (relative to workspace root)" }
            },
            "required": ["path"]
        })
    }
    fn execute(
        &self,
        arguments: serde_json::Value,
        _resume: Option<ResumePayload>,
        _ctx: &ToolContext,
    ) -> impl std::future::Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>>
    {
        let root = self.root.clone();
        async move {
            let path_str = arguments["path"]
                .as_str()
                .ok_or_else(|| AgentError::tool("fs_read", "missing 'path'"))?
                .to_string();
            let full = resolve(&root, &path_str);
            Ok(ToolResult::Output(stream! {
                match tokio::fs::read_to_string(&full).await {
                    Ok(content) => yield ToolOutput::Result(content),
                    Err(e)      => yield ToolOutput::Result(format!("error: {}", e)),
                }
            }))
        }
    }
}

// ── RootedFsWriteTool ─────────────────────────────────────────────────────────

pub struct RootedFsWriteTool { pub root: PathBuf }

impl Tool for RootedFsWriteTool {
    fn name(&self) -> &str { "fs_write" }
    fn description(&self) -> &str {
        "Write text to a file in the workspace. Path is relative to the workspace root. \
         Parent directories must exist."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path":    { "type": "string", "description": "File path (relative to workspace root)" },
                "content": { "type": "string", "description": "Text content to write" }
            },
            "required": ["path", "content"]
        })
    }
    fn execute(
        &self,
        arguments: serde_json::Value,
        _resume: Option<ResumePayload>,
        _ctx: &ToolContext,
    ) -> impl std::future::Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>>
    {
        let root = self.root.clone();
        async move {
            let path_str = arguments["path"]
                .as_str()
                .ok_or_else(|| AgentError::tool("fs_write", "missing 'path'"))?
                .to_string();
            let content = arguments["content"]
                .as_str()
                .ok_or_else(|| AgentError::tool("fs_write", "missing 'content'"))?
                .to_string();
            let bytes = content.len();
            let full = resolve(&root, &path_str);
            Ok(ToolResult::Output(stream! {
                // Auto-create parent dirs
                if let Some(parent) = full.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                match tokio::fs::write(&full, content.as_bytes()).await {
                    Ok(()) => yield ToolOutput::Result(format!("wrote {} bytes to {}", bytes, path_str)),
                    Err(e) => yield ToolOutput::Result(format!("error: {}", e)),
                }
            }))
        }
    }
}

// ── RootedFsCreateTool ────────────────────────────────────────────────────────

pub struct RootedFsCreateTool { pub root: PathBuf }

impl Tool for RootedFsCreateTool {
    fn name(&self) -> &str { "fs_mkdir" }
    fn description(&self) -> &str {
        "Create a directory in the workspace. Path is relative to the workspace root."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path":      { "type": "string",  "description": "Directory path" },
                "recursive": { "type": "boolean", "description": "Create parent dirs (mkdir -p)", "default": true }
            },
            "required": ["path"]
        })
    }
    fn execute(
        &self,
        arguments: serde_json::Value,
        _resume: Option<ResumePayload>,
        _ctx: &ToolContext,
    ) -> impl std::future::Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>>
    {
        let root = self.root.clone();
        async move {
            let path_str = arguments["path"]
                .as_str()
                .ok_or_else(|| AgentError::tool("fs_mkdir", "missing 'path'"))?
                .to_string();
            let recursive = arguments["recursive"].as_bool().unwrap_or(true);
            let full = resolve(&root, &path_str);
            Ok(ToolResult::Output(stream! {
                let result = if recursive {
                    tokio::fs::create_dir_all(&full).await
                } else {
                    tokio::fs::create_dir(&full).await
                };
                match result {
                    Ok(()) => yield ToolOutput::Result(format!("created directory {}", path_str)),
                    Err(e) => yield ToolOutput::Result(format!("error: {}", e)),
                }
            }))
        }
    }
}

// ── RootedFsRemoveTool ────────────────────────────────────────────────────────

pub struct RootedFsRemoveTool { pub root: PathBuf }

impl Tool for RootedFsRemoveTool {
    fn name(&self) -> &str { "fs_remove" }
    fn description(&self) -> &str {
        "Remove a file or directory in the workspace. Path is relative to the workspace root."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path":      { "type": "string",  "description": "Path to remove" },
                "recursive": { "type": "boolean", "description": "Remove directory contents (rm -r)", "default": false }
            },
            "required": ["path"]
        })
    }
    fn execute(
        &self,
        arguments: serde_json::Value,
        _resume: Option<ResumePayload>,
        _ctx: &ToolContext,
    ) -> impl std::future::Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>>
    {
        let root = self.root.clone();
        async move {
            let path_str = arguments["path"]
                .as_str()
                .ok_or_else(|| AgentError::tool("fs_remove", "missing 'path'"))?
                .to_string();
            let recursive = arguments["recursive"].as_bool().unwrap_or(false);
            let full = resolve(&root, &path_str);
            Ok(ToolResult::Output(stream! {
                let result = if recursive {
                    tokio::fs::remove_dir_all(&full).await
                } else {
                    match tokio::fs::remove_file(&full).await {
                        Ok(()) => Ok(()),
                        Err(_) => tokio::fs::remove_dir(&full).await,
                    }
                };
                match result {
                    Ok(()) => yield ToolOutput::Result(format!("removed {}", path_str)),
                    Err(e) => yield ToolOutput::Result(format!("error: {}", e)),
                }
            }))
        }
    }
}

// ── RootedFsLsTool ────────────────────────────────────────────────────────────

pub struct RootedFsLsTool { pub root: PathBuf }

impl Tool for RootedFsLsTool {
    fn name(&self) -> &str { "fs_ls" }
    fn description(&self) -> &str {
        "List directory contents in the workspace. \
         Path is relative to the workspace root. Use '.' or '' for root."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Directory path (relative to workspace)" }
            },
            "required": ["path"]
        })
    }
    fn execute(
        &self,
        arguments: serde_json::Value,
        _resume: Option<ResumePayload>,
        _ctx: &ToolContext,
    ) -> impl std::future::Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>>
    {
        let root = self.root.clone();
        async move {
            let path_str = arguments["path"]
                .as_str()
                .ok_or_else(|| AgentError::tool("fs_ls", "missing 'path'"))?
                .to_string();
            let full = if path_str.is_empty() || path_str == "." {
                root.clone()
            } else {
                resolve(&root, &path_str)
            };
            Ok(ToolResult::Output(stream! {
                match tokio::fs::read_dir(&full).await {
                    Err(e) => { yield ToolOutput::Result(format!("error: {}", e)); }
                    Ok(mut dir) => {
                        let mut entries = Vec::new();
                        loop {
                            match dir.next_entry().await {
                                Ok(None)        => break,
                                Err(e)          => { yield ToolOutput::Result(format!("error: {}", e)); return; }
                                Ok(Some(entry)) => {
                                    let name = entry.file_name().to_string_lossy().into_owned();
                                    if let Ok(meta) = entry.metadata().await {
                                        let kind = if meta.is_dir() { "directory" } else { "file" };
                                        entries.push(serde_json::json!({
                                            "name": name,
                                            "type": kind,
                                            "size": meta.len(),
                                        }));
                                    } else {
                                        entries.push(serde_json::json!({ "name": name }));
                                    }
                                }
                            }
                        }
                        entries.sort_by(|a, b| {
                            a["name"].as_str().unwrap_or("").cmp(b["name"].as_str().unwrap_or(""))
                        });
                        yield ToolOutput::Result(serde_json::Value::Array(entries).to_string());
                    }
                }
            }))
        }
    }
}
