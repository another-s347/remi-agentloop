use super::{tool_to_definition, BoxedToolResult, DynTool, Tool, ToolContext, ToolDefinition};
use crate::error::AgentError;
use crate::types::{ParsedToolCall, ResumePayload};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

// ── ToolRegistry trait ────────────────────────────────────────────────────────

/// Trait abstracting a registry of tools available to the agent loop.
///
/// Implement this trait to provide custom tool lookup, routing, or execution
/// strategies (e.g. remote registries, per-request filtering, sandboxing).
///
/// The framework ships [`DefaultToolRegistry`] as the standard in-process
/// implementation.
pub trait ToolRegistry: Send + Sync {
    /// Returns the list of tool definitions to advertise to the model.
    ///
    /// `user_state` is the current `AgentState.user_state`; implementations
    /// should use [`Tool::enabled`] to filter tools for progressive disclosure.
    fn definitions(&self, user_state: &serde_json::Value) -> Vec<ToolDefinition>;

    /// Returns `true` when no tools are registered.
    fn is_empty(&self) -> bool;

    /// Returns `true` if this registry can execute a tool with the given name.
    fn contains(&self, name: &str) -> bool;

    /// Execute a batch of tool calls, returning `(call_id, result)` pairs.
    ///
    /// `resume_map` maps `tool_call_id` → [`ResumePayload`] for calls that
    /// are resuming from a previous interrupt.
    ///
    /// `ctx` provides runtime context (config, thread_id, run_id, metadata)
    /// that is forwarded to each tool.
    ///
    /// Implementors may choose sequential or parallel execution strategies.
    /// The default [`DefaultToolRegistry`] runs calls sequentially.
    fn execute_parallel<'a>(
        &'a self,
        calls: &'a [ParsedToolCall],
        resume_map: &'a HashMap<String, ResumePayload>,
        ctx: &'a ToolContext,
    ) -> Pin<Box<dyn Future<Output = Vec<(String, Result<BoxedToolResult<'a>, AgentError>)>> + 'a>>;
}

// ── DefaultToolRegistry ───────────────────────────────────────────────────────

/// The standard in-process tool registry backed by a `HashMap` index.
pub struct DefaultToolRegistry {
    tools: Vec<Box<dyn DynTool>>,
    index: HashMap<String, usize>,
}

impl DefaultToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: Vec::new(),
            index: HashMap::new(),
        }
    }

    /// Register a tool. Overwrites any previously registered tool with the same name.
    pub fn register(&mut self, tool: impl Tool + Send + Sync + 'static) {
        let name = tool.name().to_string();
        let idx = self.tools.len();
        self.tools.push(Box::new(tool));
        self.index.insert(name, idx);
    }

    pub(crate) fn get(&self, name: &str) -> Option<&dyn DynTool> {
        self.index.get(name).map(|&i| self.tools[i].as_ref())
    }
}

impl ToolRegistry for DefaultToolRegistry {
    fn definitions(&self, user_state: &serde_json::Value) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .filter(|t| t.enabled(user_state))
            .map(|t| tool_to_definition(t.as_ref()))
            .collect()
    }

    fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    fn contains(&self, name: &str) -> bool {
        self.index.contains_key(name)
    }

    fn execute_parallel<'a>(
        &'a self,
        calls: &'a [ParsedToolCall],
        resume_map: &'a HashMap<String, ResumePayload>,
        ctx: &'a ToolContext,
    ) -> Pin<Box<dyn Future<Output = Vec<(String, Result<BoxedToolResult<'a>, AgentError>)>> + 'a>>
    {
        Box::pin(async move {
            let mut results = Vec::with_capacity(calls.len());
            for tc in calls {
                let resume = resume_map.get(&tc.id).cloned();
                let result = match self.get(&tc.name) {
                    Some(tool) => tool.execute_boxed(tc.arguments.clone(), resume, ctx).await,
                    None => Err(AgentError::ToolNotFound(tc.name.clone())),
                };
                results.push((tc.id.clone(), result));
            }
            results
        })
    }
}

impl Default for DefaultToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
