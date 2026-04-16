//! TodoLayer — wraps any `Agent<Request=LoopInput, Response=DeepAgentEvent>` and
//! adds five todo management tools using the core external tool layer.

pub mod tools;

use futures::Stream;
use remi_core::agent::{Agent, AgentExt};
use remi_core::error::AgentError;
use remi_core::tool::{
    registry::{DefaultToolRegistry, ToolLayerAgent, ToolLayerHook, ToolRegistry},
};
use remi_core::types::{ChatCtx, LoopInput, ParsedToolCall};

use crate::events::{DeepAgentEvent, TodoEvent};
use tools::{TodoAddTool, TodoCompleteTool, TodoListTool, TodoRemoveTool, TodoUpdateTool};

// ── TodoAgent ─────────────────────────────────────────────────────────────────

/// Wraps an inner agent and adds todo tools.
pub struct TodoAgent<A> {
    inner: ToolLayerAgent<A, DefaultToolRegistry, TodoHook>,
}

impl<A> TodoAgent<A>
where
    A: Agent<Request = LoopInput, Response = DeepAgentEvent, Error = AgentError>,
{
    pub fn new(inner: A) -> Self {
        let tools = DefaultToolRegistry::new()
            .tool(TodoAddTool)
            .tool(TodoListTool)
            .tool(TodoCompleteTool)
            .tool(TodoUpdateTool)
            .tool(TodoRemoveTool);
        Self {
            inner: inner.layer(tools.into_layer_with_hook(TodoHook)),
        }
    }
}

struct TodoHook;

impl ToolLayerHook<DeepAgentEvent> for TodoHook {
    fn on_tool_call(&self, tool_call: &ParsedToolCall, ctx: &ChatCtx) -> Vec<DeepAgentEvent> {
        make_todo_event(tool_call, ctx)
            .map(DeepAgentEvent::Todo)
            .into_iter()
            .collect()
    }
}

// ── Agent impl ────────────────────────────────────────────────────────────────

impl<A> Agent for TodoAgent<A>
where
    A: Agent<Request = LoopInput, Response = DeepAgentEvent, Error = AgentError>,
{
    type Request = LoopInput;
    type Response = DeepAgentEvent;
    type Error = AgentError;

    async fn chat(
        &self,
        ctx: ChatCtx,
        input: LoopInput,
    ) -> Result<impl Stream<Item = DeepAgentEvent>, AgentError> {
        self.inner.chat(ctx, input).await
    }
}

fn make_todo_event(tc: &ParsedToolCall, ctx: &ChatCtx) -> Option<TodoEvent> {
    match tc.name.as_str() {
        "todo__add" => {
            let content = tc.arguments["content"].as_str()?.to_string();
            let todos: Vec<crate::todo::tools::TodoItem> = ctx.with_user_state(|us| {
                serde_json::from_value(us["__todos"].clone()).unwrap_or_default()
            });
            let id = todos.iter().map(|t| t.id).max().unwrap_or(0);
            Some(TodoEvent::Added { id, content })
        }
        "todo__complete" => {
            let id = tc.arguments["id"].as_u64()?;
            Some(TodoEvent::Completed { id })
        }
        "todo__update" => {
            let id = tc.arguments["id"].as_u64()?;
            let content = tc.arguments["content"].as_str()?.to_string();
            Some(TodoEvent::Updated { id, content })
        }
        "todo__remove" => {
            let id = tc.arguments["id"].as_u64()?;
            Some(TodoEvent::Removed { id })
        }
        _ => None,
    }
}

// ── TodoLayer ─────────────────────────────────────────────────────────────────

/// Layer adapter — apply with `agent.layer(TodoLayer)`.
pub struct TodoLayer;

impl<A> remi_core::agent::Layer<A> for TodoLayer
where
    A: Agent<Request = LoopInput, Response = DeepAgentEvent, Error = AgentError>,
{
    type Output = TodoAgent<A>;
    fn layer(self, inner: A) -> Self::Output {
        TodoAgent::new(inner)
    }
}
