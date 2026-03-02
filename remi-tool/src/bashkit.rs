use std::sync::Arc;
use async_stream::stream;
use futures::Stream;
use tokio::sync::Mutex;

use remi_core::error::AgentError;
use remi_core::tool::{Tool, ToolContext, ToolOutput, ToolResult};
use remi_core::types::ResumePayload;

// ── VirtualBashTool ───────────────────────────────────────────────────────────

/// Virtual bash interpreter tool powered by [bashkit](https://docs.rs/bashkit).
///
/// Executes bash commands in a sandboxed, in-process virtual environment with:
/// - **Virtual filesystem** — no real filesystem access by default
/// - **Resource limits** — command count, loop iterations, function depth, timeout
/// - **Network allowlist** — controlled HTTP access (curl/wget)
/// - **100+ built-in commands** — echo, grep, sed, awk, jq, tar, etc.
/// - **Cross-platform** — works on Linux, macOS, and Windows (no system bash needed)
///
/// The interpreter is stateful: variables, virtual files, and shell state persist
/// across multiple `execute()` calls within the same `VirtualBashTool` instance.
///
/// # Example
///
/// ```rust,no_run
/// use remi_agentloop::tool::bashkit::VirtualBashTool;
///
/// let tool = VirtualBashTool::new();
/// // Register with AgentBuilder via .tool(tool)
/// ```
pub struct VirtualBashTool {
    inner: Arc<Mutex<bashkit::BashTool>>,
    name: String,
}

impl VirtualBashTool {
    /// Create a new `VirtualBashTool` with default configuration.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(bashkit::BashTool::default())),
            name: "bash".into(),
        }
    }

    /// Create a builder for advanced configuration.
    pub fn builder() -> VirtualBashToolBuilder {
        VirtualBashToolBuilder {
            bashkit_builder: bashkit::BashToolBuilder::new(),
            name: "bash".into(),
        }
    }
}

// ── VirtualBashToolBuilder ────────────────────────────────────────────────────

/// Builder for configuring a [`VirtualBashTool`].
///
/// # Example
///
/// ```rust,no_run
/// use remi_agentloop::tool::bashkit::VirtualBashTool;
///
/// let tool = VirtualBashTool::builder()
///     .username("deploy")
///     .hostname("prod-server")
///     .env("HOME", "/home/deploy")
///     .build();
/// ```
pub struct VirtualBashToolBuilder {
    bashkit_builder: bashkit::BashToolBuilder,
    name: String,
}

impl VirtualBashToolBuilder {
    /// Override the tool name exposed to the LLM (default: `"bash"`).
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Set custom username for virtual identity (`whoami`, `$USER`).
    pub fn username(mut self, username: impl Into<String>) -> Self {
        self.bashkit_builder = self.bashkit_builder.username(username);
        self
    }

    /// Set custom hostname for virtual identity (`hostname`, `uname -n`).
    pub fn hostname(mut self, hostname: impl Into<String>) -> Self {
        self.bashkit_builder = self.bashkit_builder.hostname(hostname);
        self
    }

    /// Set execution limits (command count, loop iterations, timeout, etc.).
    pub fn limits(mut self, limits: bashkit::ExecutionLimits) -> Self {
        self.bashkit_builder = self.bashkit_builder.limits(limits);
        self
    }

    /// Add an environment variable visible to scripts.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.bashkit_builder = self.bashkit_builder.env(key, value);
        self
    }

    /// Build the configured [`VirtualBashTool`].
    pub fn build(self) -> VirtualBashTool {
        VirtualBashTool {
            inner: Arc::new(Mutex::new(self.bashkit_builder.build())),
            name: self.name,
        }
    }
}

// ── Tool impl ─────────────────────────────────────────────────────────────────

impl Tool for VirtualBashTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "Execute bash commands in a sandboxed virtual environment. \
         Supports variables, pipes, redirections, control flow, \
         functions, and 100+ built-in commands (grep, sed, awk, jq, etc.). \
         All file operations use a virtual filesystem."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command(s) to execute"
                }
            },
            "required": ["command"]
        })
    }

    fn execute(
        &self,
        arguments: serde_json::Value,
        _resume: Option<ResumePayload>,
        _ctx: &ToolContext,
    ) -> impl std::future::Future<
        Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>,
    > {
        let inner = self.inner.clone();

        async move {
            let command = arguments["command"]
                .as_str()
                .ok_or_else(|| AgentError::tool("bash", "missing 'command' argument"))?
                .to_string();

            let mut tool = inner.lock().await;

            let request = bashkit::ToolRequest {
                commands: command.clone(),
                timeout_ms: None,
            };

            let response = bashkit::tool::Tool::execute(&mut *tool, request).await;

            let exit_code = response.exit_code;
            let stdout = response.stdout;
            let stderr = response.stderr;

            // Drop the lock before entering the stream
            drop(tool);

            Ok(ToolResult::Output(stream! {
                yield ToolOutput::Delta(format!("$ {}", command));

                let result = if !stderr.is_empty() {
                    serde_json::json!({
                        "exit_code": exit_code,
                        "stdout": stdout,
                        "stderr": stderr,
                    })
                    .to_string()
                } else {
                    serde_json::json!({
                        "exit_code": exit_code,
                        "stdout": stdout,
                    })
                    .to_string()
                };

                yield ToolOutput::Result(result);
            }))
        }
    }
}
