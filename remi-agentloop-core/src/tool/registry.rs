use super::{
    tool_to_definition, BoxedToolResult, DynTool, Tool, ToolDefinition,
    ToolDefinitionContext,
};
use crate::error::AgentError;
use crate::types::{ChatCtx, ParsedToolCall, ResumePayload};
use futures::future::join_all;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

pub use super::external::{
    AgentEventEnvelope, ExternalToolAgent as ToolLayerAgent,
    ExternalToolHook as ToolLayerHook, ExternalToolLayer as ToolLayer,
    NoopExternalToolHook as NoopToolLayerHook,
};

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

    /// Returns tool definitions using the full runtime definition context.
    ///
    /// Implementations that don't need request metadata or run identifiers can
    /// rely on the default behaviour, which falls back to [`definitions`] and
    /// preserves the pre-existing user-state-only contract.
    fn definitions_with_context(&self, ctx: &ToolDefinitionContext) -> Vec<ToolDefinition> {
        self.definitions(&ctx.user_state)
    }

    /// Returns `true` when no tools are registered.
    fn is_empty(&self) -> bool;

    /// Returns `true` if this registry can execute a tool with the given name.
    fn contains(&self, name: &str) -> bool;

    /// Turn this registry into a stackable outer tool layer.
    fn into_layer(self) -> ToolLayer<Self, NoopToolLayerHook>
    where
        Self: Sized,
    {
        ToolLayer::with_registry(self)
    }

    /// Turn this registry into a stackable outer tool layer with a custom hook.
    fn into_layer_with_hook<H>(self, hook: H) -> ToolLayer<Self, H>
    where
        Self: Sized,
    {
        ToolLayer::with_registry(self).with_hook(hook)
    }

    /// Execute a batch of tool calls, returning `(call_id, result)` pairs.
    ///
    /// `resume_map` maps `tool_call_id` → [`ResumePayload`] for calls that
    /// are resuming from a previous interrupt.
    ///
    /// `ctx` provides the shared chat context for the current run
    /// that is forwarded to each tool.
    ///
    /// Implementors may choose sequential or parallel execution strategies.
    /// The default [`DefaultToolRegistry`] runs calls sequentially.
    fn execute_parallel<'a>(
        &'a self,
        calls: &'a [ParsedToolCall],
        resume_map: &'a HashMap<String, ResumePayload>,
        ctx: &'a ChatCtx,
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

    /// Builder-style tool registration for layer composition.
    pub fn tool(mut self, tool: impl Tool + Send + Sync + 'static) -> Self {
        self.register(tool);
        self
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
        let ctx = ToolDefinitionContext::from_user_state(user_state.clone());
        self.definitions_with_context(&ctx)
    }

    fn definitions_with_context(&self, ctx: &ToolDefinitionContext) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .filter(|t| t.enabled(&ctx.user_state))
            .map(|t| tool_to_definition(t.as_ref(), ctx))
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
        ctx: &'a ChatCtx,
    ) -> Pin<Box<dyn Future<Output = Vec<(String, Result<BoxedToolResult<'a>, AgentError>)>> + 'a>>
    {
        Box::pin(async move {
            let futures = calls.iter().map(|tc| async move {
                let resume = resume_map.get(&tc.id).cloned();
                let result = match self.get(&tc.name) {
                    Some(tool) => {
                        let tool_ctx = ctx.fork_for_tool(tc.id.clone(), tc.name.clone());
                        tool.execute_boxed(tc.arguments.clone(), resume, tool_ctx).await
                    }
                    None => Err(AgentError::ToolNotFound(tc.name.clone())),
                };
                (tc.id.clone(), result)
            });
            join_all(futures).await
        })
    }
}

impl Default for DefaultToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
