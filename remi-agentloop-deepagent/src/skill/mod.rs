//! SkillLayer — wraps any `Agent<Request=LoopInput, Response=DeepAgentEvent>` and
//! adds four skill management tools using the core external tool layer.

pub mod store;
pub mod tools;

use futures::Stream;
use remi_core::agent::{Agent, AgentExt};
use remi_core::error::AgentError;
use remi_core::tool::{
    registry::{DefaultToolRegistry, ToolLayerAgent, ToolLayerHook, ToolRegistry},
};
use remi_core::types::{ChatCtx, LoopInput, ParsedToolCall};
use std::sync::Arc;

use crate::events::{DeepAgentEvent, SkillEvent};
use store::SkillStore;
use tools::{SkillDeleteTool, SkillGetTool, SkillListTool, SkillSaveTool};

// ── SkillAgent ────────────────────────────────────────────────────────────────

pub struct SkillAgent<A, S> {
    inner: ToolLayerAgent<A, DefaultToolRegistry, SkillHook>,
    _store: Arc<S>,
}

impl<A, S> SkillAgent<A, S>
where
    A: Agent<Request = LoopInput, Response = DeepAgentEvent, Error = AgentError>,
    S: SkillStore,
{
    pub fn new(inner: A, store: S) -> Self {
        let store = Arc::new(store);
        let tools = DefaultToolRegistry::new()
            .tool(SkillSaveTool {
                store: Arc::clone(&store),
            })
            .tool(SkillGetTool {
                store: Arc::clone(&store),
            })
            .tool(SkillListTool {
                store: Arc::clone(&store),
            })
            .tool(SkillDeleteTool {
                store: Arc::clone(&store),
            });
        let hook = SkillHook;
        Self {
            inner: inner.layer(tools.into_layer_with_hook(hook)),
            _store: store,
        }
    }
}

struct SkillHook;

impl ToolLayerHook<DeepAgentEvent> for SkillHook {
    fn on_tool_call(&self, tool_call: &ParsedToolCall, ctx: &ChatCtx) -> Vec<DeepAgentEvent> {
        make_skill_event(tool_call, ctx)
            .map(DeepAgentEvent::Skill)
            .into_iter()
            .collect()
    }
}

// ── Agent impl ────────────────────────────────────────────────────────────────

impl<A, S> Agent for SkillAgent<A, S>
where
    A: Agent<Request = LoopInput, Response = DeepAgentEvent, Error = AgentError>,
    S: SkillStore,
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

fn make_skill_event(tc: &ParsedToolCall, _ctx: &ChatCtx) -> Option<SkillEvent> {
    match tc.name.as_str() {
        "skill__save" => {
            let name = tc.arguments["name"].as_str()?.to_string();
            // The actual path is returned in the tool result — we don't have it here
            // so we emit a placeholder.
            Some(SkillEvent::Saved {
                name: name.clone(),
                path: format!(".deepagent/skills/{name}.md"),
            })
        }
        "skill__delete" => {
            let name = tc.arguments["name"].as_str()?.to_string();
            Some(SkillEvent::Deleted { name })
        }
        _ => None,
    }
}

// ── SkillLayer ────────────────────────────────────────────────────────────────

pub struct SkillLayer<S> {
    pub store: S,
}

impl<A, S> remi_core::agent::Layer<A> for SkillLayer<S>
where
    A: Agent<Request = LoopInput, Response = DeepAgentEvent, Error = AgentError>,
    S: SkillStore,
{
    type Output = SkillAgent<A, S>;
    fn layer(self, inner: A) -> Self::Output {
        SkillAgent::new(inner, self.store)
    }
}
