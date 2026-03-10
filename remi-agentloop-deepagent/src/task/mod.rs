//! `SubAgentTaskTool` — delegates tasks to a spawned inner agent.
//!
//! The tool accepts a `task` string, creates a fresh `AgentLoop` with bash
//! and filesystem tools, runs the task to completion, and returns the agent's
//! final text response.  This lets the outer agent offload focused subtasks
//! to a "worker" without polluting its own context.
//!
//! ## Claude Code inspiration
//!
//! Claude Code's "subagent" pattern: the orchestrator agent plans and breaks
//! down work, then delegates self-contained subtasks to isolated agents that
//! each get a clean context.  This prevents context bloat from large
//! intermediate outputs and allows independent retry of failed subtasks.

use async_stream::stream;
use futures::{Future, Stream, StreamExt};
use remi_core::agent::Agent;
use remi_core::builder::AgentBuilder;
use remi_core::error::AgentError;
use remi_core::model::ChatModel;
use remi_core::tool::{Tool, ToolContext, ToolOutput, ToolResult};
use remi_core::types::{AgentEvent, LoopInput, ResumePayload};
use remi_tool::{
    BashTool, LocalFsCreateTool, LocalFsLsTool, LocalFsReadTool, LocalFsRemoveTool,
    LocalFsWriteTool,
};
use serde_json::json;
use std::pin::Pin;
use std::sync::Arc;

// ── Type alias ────────────────────────────────────────────────────────────────

/// Object-safe runner type: takes an owned task string, returns a boxed future.
pub type RunnerFn = dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<String, AgentError>>>>
    + Send
    + Sync;

// ── SubAgentTaskTool ──────────────────────────────────────────────────────────

/// A tool that delegates a task to a sub-agent and returns its final response.
pub struct SubAgentTaskTool {
    runner: Arc<RunnerFn>,
    tool_description: String,
}

impl SubAgentTaskTool {
    /// Build a `SubAgentTaskTool` backed by `model`.
    ///
    /// Each invocation constructs a temporary `AgentLoop<M>` with bash + fs
    /// tools, cloning `model` so the original remains usable.
    pub fn new<M>(
        model: M,
        system_prompt: impl Into<String>,
        max_turns: usize,
    ) -> Self
    where
        M: ChatModel + Clone + Send + Sync + 'static,
    {
        let system_prompt = system_prompt.into();
        let runner: Arc<RunnerFn> = Arc::new(move |task: String| {
            let model = model.clone();
            let system_prompt = system_prompt.clone();
            Box::pin(async move {
                let agent = AgentBuilder::new()
                    .model(model)
                    .system(system_prompt)
                    .tool(BashTool)
                    .tool(LocalFsReadTool)
                    .tool(LocalFsWriteTool)
                    .tool(LocalFsCreateTool)
                    .tool(LocalFsRemoveTool)
                    .tool(LocalFsLsTool)
                    .max_turns(max_turns)
                    .build_loop();

                let stream = agent.chat(LoopInput::start(&task)).await?;
                let mut stream = std::pin::pin!(stream);
                let mut text = String::new();
                while let Some(ev) = stream.next().await {
                    match ev {
                        AgentEvent::TextDelta(t) => text.push_str(&t),
                        AgentEvent::Done => break,
                        AgentEvent::Error(e) => return Err(e),
                        _ => {}
                    }
                }
                Ok(text)
            })
        });

        Self {
            runner,
            tool_description: "Delegate a focused subtask to a worker sub-agent. \
                The sub-agent has access to bash and filesystem tools. \
                Use this for self-contained tasks (file operations, code generation, \
                research) that you want to keep isolated from the main context."
                .to_string(),
        }
    }
}

// ── Tool impl ─────────────────────────────────────────────────────────────────

impl Tool for SubAgentTaskTool {
    fn name(&self) -> &str { "task__run" }

    fn description(&self) -> &str {
        &self.tool_description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "Complete, self-contained task description for the sub-agent. \
                        Include all necessary context since the sub-agent starts with a clean history."
                }
            },
            "required": ["task"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        _resume: Option<ResumePayload>,
        _ctx: &ToolContext,
    ) -> Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError> {
        let task = arguments["task"]
            .as_str()
            .ok_or_else(|| AgentError::tool("task__run", "missing 'task'"))?
            .to_string();

        let runner = Arc::clone(&self.runner);
        let result = (runner)(task).await?;

        Ok(ToolResult::Output(stream! {
            yield ToolOutput::Delta("[sub-agent running…]".to_string());
            yield ToolOutput::text(result);
        }))
    }
}
