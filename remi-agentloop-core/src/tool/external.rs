use crate::agent::{Agent, Layer};
use crate::error::AgentError;
use crate::tool::{
    registry::{DefaultToolRegistry, ToolRegistry},
    Tool, ToolDefinition, ToolDefinitionContext, ToolOutput, ToolResult,
};
use crate::types::{
    AgentEvent, ChatCtx, ChatCtxState, Content, LoopInput, ParsedToolCall, ToolCallOutcome,
};
use async_stream::stream;
use futures::{Stream, StreamExt};
use std::collections::HashMap;

/// Envelope trait for response enums that can carry [`AgentEvent`] values.
///
/// This lets generic layers work with plain [`AgentEvent`] streams as well as
/// wrapper enums such as deep-agent specific event types.
pub trait AgentEventEnvelope: Sized {
    fn from_agent_event(event: AgentEvent) -> Self;
    fn into_agent_event(self) -> Result<AgentEvent, Self>;
}

impl AgentEventEnvelope for AgentEvent {
    fn from_agent_event(event: AgentEvent) -> Self {
        event
    }

    fn into_agent_event(self) -> Result<AgentEvent, Self> {
        Ok(self)
    }
}

/// Optional hook for emitting domain-specific events when a tool-layer-owned tool executes.
pub trait ExternalToolHook<E: AgentEventEnvelope>: Send + Sync {
    fn on_tool_call(&self, _tool_call: &ParsedToolCall, _ctx: &ChatCtx) -> Vec<E> {
        vec![]
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoopExternalToolHook;

impl<E: AgentEventEnvelope> ExternalToolHook<E> for NoopExternalToolHook {}

/// Compatibility implementation backing the public
/// [`ToolLayer`](crate::tool::registry::ToolLayer) API exposed from
/// [`crate::tool::registry`].
///
/// This layer injects tool definitions into [`LoopInput::Start`] and handles
/// matching [`AgentEvent::NeedToolExecution`] calls on behalf of the wrapped agent.
/// It is best suited for lightweight tool groups owned by an outer, stackable layer.
///
/// Note: interrupts requested by layer-owned tools are currently converted into
/// tool-call errors because `LoopInput` resume data does not carry tool resume
/// payloads for outer layers yet.
pub struct ExternalToolLayer<R = DefaultToolRegistry, H = NoopExternalToolHook> {
    registry: R,
    hook: H,
}

impl ExternalToolLayer<DefaultToolRegistry, NoopExternalToolHook> {
    pub fn new() -> Self {
        Self {
            registry: DefaultToolRegistry::new(),
            hook: NoopExternalToolHook,
        }
    }

    pub fn tool(mut self, tool: impl Tool + Send + Sync + 'static) -> Self {
        self.registry.register(tool);
        self
    }
}

impl Default for ExternalToolLayer<DefaultToolRegistry, NoopExternalToolHook> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R> ExternalToolLayer<R, NoopExternalToolHook> {
    pub fn with_registry(registry: R) -> Self {
        Self {
            registry,
            hook: NoopExternalToolHook,
        }
    }
}

impl<R, H> ExternalToolLayer<R, H> {
    pub fn with_hook<H2>(self, hook: H2) -> ExternalToolLayer<R, H2> {
        ExternalToolLayer {
            registry: self.registry,
            hook,
        }
    }
}

pub struct ExternalToolAgent<A, R, H> {
    inner: A,
    registry: R,
    hook: H,
}

impl<A, R, H> ExternalToolAgent<A, R, H> {
    pub fn registry(&self) -> &R {
        &self.registry
    }
}

impl<A, R, H> Layer<A> for ExternalToolLayer<R, H>
where
    A: Agent<Request = LoopInput, Error = AgentError>,
    R: ToolRegistry,
    H: ExternalToolHook<A::Response>,
    A::Response: AgentEventEnvelope,
{
    type Output = ExternalToolAgent<A, R, H>;

    fn layer(self, inner: A) -> Self::Output {
        ExternalToolAgent {
            inner,
            registry: self.registry,
            hook: self.hook,
        }
    }
}

impl<A, R, H, E> Agent for ExternalToolAgent<A, R, H>
where
    A: Agent<Request = LoopInput, Response = E, Error = AgentError>,
    R: ToolRegistry,
    H: ExternalToolHook<E>,
    E: AgentEventEnvelope,
{
    type Request = LoopInput;
    type Response = E;
    type Error = AgentError;

    async fn chat(
        &self,
        ctx: ChatCtx,
        input: LoopInput,
    ) -> Result<impl Stream<Item = E>, AgentError> {
        let first_input = self.inject_definitions(input);

        Ok(stream! {
            let mut next_input = Some(first_input);

            loop {
                let input = next_input.take().unwrap();
                let inner_stream = match self.inner.chat(ctx.clone(), input).await {
                    Ok(stream) => stream,
                    Err(error) => {
                        yield E::from_agent_event(AgentEvent::Error(error));
                        return;
                    }
                };
                let mut inner_stream = std::pin::pin!(inner_stream);
                let mut should_restart = false;

                while let Some(event) = inner_stream.next().await {
                    match event.into_agent_event() {
                        Ok(AgentEvent::NeedToolExecution {
                            mut state,
                            tool_calls,
                            completed_results,
                        }) => {
                            let (mine, external): (Vec<_>, Vec<_>) = tool_calls
                                .iter()
                                .cloned()
                                .partition(|tc| self.registry.contains(&tc.name));

                            let mut all_outcomes = completed_results;

                            if !mine.is_empty() {
                                let tool_ctx = ChatCtx::with_ids(
                                    state.thread_id.clone(),
                                    state.run_id.clone(),
                                    ChatCtxState {
                                        user_state: state.user_state.clone(),
                                        metadata: state.config.metadata.clone(),
                                        ..ChatCtxState::default()
                                    },
                                );
                                let resume_map = HashMap::new();
                                let results = self
                                    .registry
                                    .execute_parallel(&mine, &resume_map, &tool_ctx)
                                    .await;

                                for (call_id, result) in results {
                                    let tc = mine.iter().find(|t| t.id == call_id).unwrap();

                                    for hook_event in self.hook.on_tool_call(tc, &tool_ctx) {
                                        yield hook_event;
                                    }

                                    match result {
                                        Err(error) => {
                                            all_outcomes.push(ToolCallOutcome::Error {
                                                tool_call_id: call_id,
                                                tool_name: tc.name.clone(),
                                                error: error.to_string(),
                                            });
                                        }
                                        Ok(ToolResult::Interrupt(_)) => {
                                            all_outcomes.push(ToolCallOutcome::Error {
                                                tool_call_id: call_id,
                                                tool_name: tc.name.clone(),
                                                error: "layer-owned tool interrupts are not supported".to_string(),
                                            });
                                        }
                                        Ok(ToolResult::Output(mut stream)) => {
                                            let mut final_content: Option<Content> = None;
                                            while let Some(output) = stream.next().await {
                                                match output {
                                                    ToolOutput::Delta(delta) => {
                                                        yield E::from_agent_event(AgentEvent::ToolDelta {
                                                            id: call_id.clone(),
                                                            name: tc.name.clone(),
                                                            delta,
                                                        });
                                                    }
                                                    ToolOutput::SubSession(mut event) => {
                                                        if event.parent_tool_call_id.is_empty() {
                                                            event.parent_tool_call_id = call_id.clone();
                                                        }
                                                        yield E::from_agent_event(AgentEvent::SubSession(event));
                                                    }
                                                    ToolOutput::Custom { event_type, extra } => {
                                                        yield E::from_agent_event(AgentEvent::Custom {
                                                            event_type,
                                                            extra,
                                                        });
                                                    }
                                                    ToolOutput::Result(content) => {
                                                        yield E::from_agent_event(AgentEvent::ToolResult {
                                                            id: call_id.clone(),
                                                            name: tc.name.clone(),
                                                            result: content.text_content(),
                                                        });
                                                        final_content = Some(content);
                                                    }
                                                }
                                            }

                                            all_outcomes.push(ToolCallOutcome::Result {
                                                tool_call_id: call_id,
                                                tool_name: tc.name.clone(),
                                                content: final_content.unwrap_or_else(|| Content::text("")),
                                            });
                                        }
                                    }
                                }

                                state.user_state = tool_ctx.user_state();
                            }

                            if !external.is_empty() {
                                yield E::from_agent_event(AgentEvent::NeedToolExecution {
                                    state,
                                    tool_calls: external,
                                    completed_results: all_outcomes,
                                });
                                return;
                            }

                            next_input = Some(LoopInput::resume(state, vec![], all_outcomes));
                            should_restart = true;
                            break;
                        }
                        Ok(agent_event) => {
                            yield E::from_agent_event(agent_event);
                        }
                        Err(other) => {
                            yield other;
                        }
                    }
                }

                if !should_restart {
                    return;
                }
            }
        })
    }
}

impl<A, R, H> ExternalToolAgent<A, R, H>
where
    R: ToolRegistry,
{
    fn inject_definitions(&self, input: LoopInput) -> LoopInput {
        match input {
            LoopInput::Start {
                message,
                history,
                mut extra_tools,
                model,
                temperature,
                max_tokens,
                metadata,
            } => {
                extra_tools.extend(self.tool_definitions(metadata.clone()));
                LoopInput::Start {
                    message,
                    history,
                    extra_tools,
                    model,
                    temperature,
                    max_tokens,
                    metadata,
                }
            }
            other => other,
        }
    }

    fn tool_definitions(&self, metadata: Option<serde_json::Value>) -> Vec<ToolDefinition> {
        let ctx = ToolDefinitionContext {
            metadata,
            user_state: serde_json::Value::Null,
            ..ToolDefinitionContext::default()
        };
        self.registry.definitions_with_context(&ctx)
    }
}